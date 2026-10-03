//! Still images for agents: a whole display or one window, at one pixel per
//! point so screenshot pixels and input coordinates are the same numbers.

use crate::model::{DisplayId, WindowTarget};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSMutableData, NSNumber, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImage, SCScreenshotManager};
use std::ffi::c_void;

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

/// Blocking: call off the main thread.
pub fn display(id: DisplayId, size: (f64, f64), format: Format) -> Result<Shot, String> {
    let content = SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .get()
        .map_err(|e| format!("could not list displays: {e}"))?;
    let target = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == id)
        .ok_or("display is not capturable yet; retry in a moment")?;
    let filter = SCContentFilter::create()
        .with_display(&target)
        .with_excluding_windows(&[])
        .build()
        .map_err(|e| e.to_string())?;
    let config = SCStreamConfiguration::new()
        .with_width(size.0 as u32)
        .with_height(size.1 as u32)
        .with_shows_cursor(false);
    let image = SCScreenshotManager::capture_image(&filter, &config).map_err(|e| e.to_string())?;
    encode(&image, format)
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
        CGImageDestinationAddImage(destination, image.as_ptr().cast(), properties);
        let ok = CGImageDestinationFinalize(destination);
        CFRelease(destination);
        if !ok {
            return Err("could not encode the screenshot".into());
        }
    }
    Ok(Shot {
        data: data.to_vec(),
        mime_type,
        width: image.width(),
        height: image.height(),
    })
}
