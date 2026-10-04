//! Still images for agents: a whole display or one window, at one pixel per
//! point so screenshot pixels and input coordinates are the same numbers.

use crate::model::{DisplayId, WindowTarget};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSMutableData, NSNumber, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImage, SCScreenshotManager};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
}

pub struct Shot {
    pub data: Vec<u8>,
    pub mime_type: &'static str,
    pub width: usize,
    pub height: usize,
}

/// Displays looked up before. Listing shareable content costs tens of
/// milliseconds, more than the capture itself.
static DISPLAYS: Mutex<Option<HashMap<DisplayId, Arc<SCDisplay>>>> = Mutex::new(None);

fn lookup(id: DisplayId, fresh: bool) -> Result<Arc<SCDisplay>, String> {
    let mut cache = DISPLAYS.lock().unwrap();
    let map = cache.get_or_insert_with(HashMap::new);
    if !fresh && let Some(display) = map.get(&id) {
        return Ok(display.clone());
    }
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .get()
        .map_err(|e| format!("could not list displays: {e}"))?;
    map.clear();
    for display in content.displays() {
        map.insert(display.display_id(), Arc::new(display));
    }
    map.get(&id)
        .cloned()
        .ok_or_else(|| "display is not capturable yet; retry in a moment".into())
}

/// Blocking: call off the main thread. `cover` is the height, in image
/// pixels, of the strip at the top to paint over: macOS draws the user's
/// own menu bar there, on every display.
pub fn display(
    id: DisplayId,
    size: (f64, f64),
    format: Format,
    cover: f64,
) -> Result<Shot, String> {
    let capture = |target: &SCDisplay| -> Result<CGImage, String> {
        let filter = SCContentFilter::create()
            .with_display(target)
            .with_excluding_windows(&[])
            .build()
            .map_err(|e| e.to_string())?;
        let config = SCStreamConfiguration::new()
            .with_width(size.0 as u32)
            .with_height(size.1 as u32)
            .with_shows_cursor(false);
        SCScreenshotManager::capture_image(&filter, &config).map_err(|e| e.to_string())
    };
    // A cached display can go stale when displays change; list them again.
    let image = match capture(&*lookup(id, false)?) {
        Ok(image) => image,
        Err(_) => capture(&*lookup(id, true)?)?,
    };
    if cover <= 0.0 {
        return encode(&image, format);
    }
    let covered = cover_top(image.as_ptr().cast(), image.width(), image.height(), cover);
    if covered.is_null() {
        return encode(&image, format);
    }
    let shot = encode_raw(covered, image.width(), image.height(), format);
    unsafe { CFRelease(covered) };
    shot
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *const c_void,
        bitmap_info: u32,
    ) -> *mut c_void;
    fn CGBitmapContextCreateImage(context: *mut c_void) -> *const c_void;
    fn CGContextDrawImage(context: *mut c_void, rect: CGRect, image: *const c_void);
    fn CGContextSetRGBFillColor(context: *mut c_void, r: f64, g: f64, b: f64, a: f64);
    fn CGContextFillRect(context: *mut c_void, rect: CGRect);
    fn CGContextRelease(context: *mut c_void);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// A copy of `image` with its top `rows` pixels painted flat grey, or null.
fn cover_top(image: *const c_void, width: usize, height: usize, rows: f64) -> *const c_void {
    // kCGImageAlphaNoneSkipFirst | kCGBitmapByteOrder32Little: BGRX.
    const BGRX: u32 = 6 | (2 << 12);
    unsafe {
        let space = CGColorSpaceCreateDeviceRGB();
        let context = CGBitmapContextCreate(std::ptr::null_mut(), width, height, 8, 0, space, BGRX);
        CGColorSpaceRelease(space);
        if context.is_null() {
            return std::ptr::null();
        }
        let (w, h) = (width as f64, height as f64);
        CGContextDrawImage(
            context,
            CGRect {
                x: 0.0,
                y: 0.0,
                width: w,
                height: h,
            },
            image,
        );
        CGContextSetRGBFillColor(context, 0.9, 0.9, 0.9, 1.0);
        // CoreGraphics counts y from the bottom.
        CGContextFillRect(
            context,
            CGRect {
                x: 0.0,
                y: h - rows,
                width: w,
                height: rows,
            },
        );
        let covered = CGBitmapContextCreateImage(context);
        CGContextRelease(context);
        covered
    }
}

/// One window without what lies above or below it. Blocking.
pub fn window(target: WindowTarget, size: (f64, f64)) -> Result<Shot, String> {
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .get()
        .map_err(|e| format!("could not list windows: {e}"))?;
    let window = content
        .windows()
        .into_iter()
        .find(|w| w.window_id() == target.window_id)
        .ok_or("the window is not capturable; is Screen Recording granted?")?;
    let filter = SCContentFilter::create()
        .with_window(&window)
        .build()
        .map_err(|e| e.to_string())?;
    let mut config = SCStreamConfiguration::new()
        .with_width(size.0.max(1.0) as u32)
        .with_height(size.1.max(1.0) as u32)
        .with_shows_cursor(false);
    let _ = config.set_ignores_shadows_single_window(true);
    let image = SCScreenshotManager::capture_image(&filter, &config).map_err(|e| e.to_string())?;
    encode(&image, Format::Jpeg)
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageDestinationCreateWithData(
        data: *const c_void,
        kind: *const c_void,
        count: usize,
        options: *const c_void,
    ) -> *mut c_void;
    fn CGImageDestinationAddImage(
        destination: *mut c_void,
        image: *const c_void,
        properties: *const c_void,
    );
    fn CGImageDestinationFinalize(destination: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

/// JPEG at quality 0.8, or PNG, through ImageIO.
pub fn encode(image: &CGImage, format: Format) -> Result<Shot, String> {
    encode_raw(image.as_ptr().cast(), image.width(), image.height(), format)
}

fn encode_raw(
    image: *const c_void,
    width: usize,
    height: usize,
    format: Format,
) -> Result<Shot, String> {
    let (kind, mime_type) = match format {
        Format::Png => ("public.png", "image/png"),
        Format::Jpeg => ("public.jpeg", "image/jpeg"),
    };
    let data = NSMutableData::new();
    let kind = NSString::from_str(kind);
    let options: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_retained_objects(
        &[&*NSString::from_str(
            "kCGImageDestinationLossyCompressionQuality",
        )],
        &[Retained::into_super(Retained::into_super(NSNumber::new_f64(0.8))).into()],
    );
    unsafe {
        let destination = CGImageDestinationCreateWithData(
            Retained::as_ptr(&data).cast(),
            Retained::as_ptr(&kind).cast(),
            1,
            std::ptr::null(),
        );
        if destination.is_null() {
            return Err("could not encode the screenshot".into());
        }
        let properties = if format == Format::Jpeg {
            Retained::as_ptr(&options).cast()
        } else {
            std::ptr::null()
        };
        CGImageDestinationAddImage(destination, image, properties);
        let ok = CGImageDestinationFinalize(destination);
        CFRelease(destination);
        if !ok {
            return Err("could not encode the screenshot".into());
        }
    }
    Ok(Shot {
        data: data.to_vec(),
        mime_type,
        width,
        height,
    })
}
