//! Still images for agents: a whole display or one window, at one pixel per
//! point so screenshot pixels and input coordinates are the same numbers.

use crate::model::{DisplayId, WindowTarget};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSDictionary, NSMutableData, NSNumber, NSString};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImage, SCScreenshotManager};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// Shareable content listed before, and when. Listing costs tens of
/// milliseconds, more than a capture; status items come and go rarely.
/// The last listing, when it was made, and the ids of the windows on screen
/// then.
type Listing = (
    Arc<SCShareableContent>,
    Instant,
    std::collections::HashSet<u32>,
);
static CONTENT: Mutex<Option<Listing>> = Mutex::new(None);
const CONTENT_AGE: Duration = Duration::from_secs(2);

/// What ScreenCaptureKit can capture. A listing takes ScreenCaptureKit about
/// 20 ms; a fresh one is only needed once a window has come or gone, which
/// the window server's list of ids, at well under a millisecond, tells.
fn content(fresh: bool) -> Result<Arc<SCShareableContent>, String> {
    let mut cache = CONTENT.lock().unwrap();
    let ids = fresh.then(super::cg::on_screen_ids);
    if let Some((content, at, then)) = &*cache {
        let current = match &ids {
            Some(now) => now == then,
            None => at.elapsed() < CONTENT_AGE,
        };
        if current {
            return Ok(content.clone());
        }
    }
    let ids = ids.unwrap_or_else(super::cg::on_screen_ids);
    let content = Arc::new(
        SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .get()
            .map_err(|e| format!("could not list displays: {e}"))?,
    );
    *cache = Some((content.clone(), Instant::now(), ids));
    Ok(content)
}

/// Whether ScreenCaptureKit lets this process list the screen. Blocking.
pub fn can_capture() -> bool {
    forget();
    content(true).is_ok()
}

/// Drop the kept listing: the next one is made anew. A window that moved
/// keeps its id, so a capture that failed may have used its old place.
fn forget() {
    *CONTENT.lock().unwrap() = None;
}

/// What a display shows, without the menu bar macOS draws on it: that bar,
/// its clock and status items belong to the user's front app, not to the
/// agent. Its strip shows the wallpaper instead. `plain` also leaves out the
/// wallpaper and desktop icons, for agents: a photo makes up most of a
/// screenshot's bytes and tells them nothing. Blocking.
pub fn filter(id: DisplayId, fresh: bool, plain: bool) -> Result<SCContentFilter, String> {
    filter_counting(id, fresh, plain).map(|(filter, _)| filter)
}

/// The filter, and how many windows on the display it leaves in.
fn filter_counting(
    id: DisplayId,
    fresh: bool,
    plain: bool,
) -> Result<(SCContentFilter, usize), String> {
    // ScreenCaptureKit would capture either display, maybe the other one.
    if let Some(twin) = super::cg::unit_twin(id) {
        log::debug!("display {id} shares its unit number with display {twin}; not capturing it");
        return Err("macOS mixes this display up with another one right now, so Agent Screen won't capture it".into());
    }
    let content = content(fresh)?;
    let display = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == id)
        .ok_or("display is not capturable yet; retry in a moment")?;
    let area = super::cg::display_bounds(id);
    // The bar is one Window Server window at level 24, the status items
    // level-25 windows and the recording indicator a dot far above, all
    // within the top 40 pt. Full-screen windows at those levels, such as
    // the screenshot tool's, stay.
    let bar: Vec<SCWindow> = content
        .windows()
        .into_iter()
        .filter(|w| {
            let f = w.frame();
            let across = f.origin.x < area.max_x()
                && f.origin.x + f.size.width > area.x
                && f.origin.y < area.max_y()
                && f.origin.y + f.size.height > area.y;
            // The wallpaper and desktop icons sit below every normal level.
            let desktop = plain && w.window_layer() < 0;
            let in_bar = w.window_layer() >= 24
                && f.size.height <= 40.0
                && f.origin.y >= area.y - 1.0
                && f.origin.y < area.y + 40.0;
            across && (in_bar || desktop)
        })
        .collect();
    // What an agent could see: app windows, panels and menus. The cursor,
    // the recording indicator and other overlays from layer 1000 up, and
    // 1-point status windows, don't make a picture. Where windows are now,
    // from the window server: the listing may be older than a window's move.
    let left = super::cg::on_screen_frames()
        .into_iter()
        .filter(|&(window_id, layer, f)| {
            (0..1000).contains(&layer)
                && f.width > 1.0
                && f.height > 1.0
                && f.x < area.max_x()
                && f.max_x() > area.x
                && f.y < area.max_y()
                && f.max_y() > area.y
                && !bar.iter().any(|b| b.window_id() == window_id)
        })
        .count();
    let refs: Vec<&SCWindow> = bar.iter().collect();
    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&refs)
        .build()
        .map_err(|e| e.to_string())?;
    Ok((filter, left))
}

/// Blocking: call off the main thread. `source` limits the capture to a
/// display-local rect in points, for a zoomed view.
pub fn display(
    id: DisplayId,
    size: (f64, f64),
    format: Format,
    source: Option<crate::model::Rect>,
) -> Result<Shot, String> {
    // Pixels `size` of `source`, in the display's points (the whole display
    // when none). None: nothing but the desktop.
    let capture =
        |size: (f64, f64), source: Option<crate::model::Rect>| -> Result<Option<CGImage>, String> {
            // A fresh listing every time: a cached one misses the wallpaper and
            // menu bar windows a new display gets, so they'd show, and names
            // windows that closed since, which fails the capture.
            let (filter, left) = filter_counting(id, true, true)?;
            // Nothing but the desktop: ScreenCaptureKit refuses to capture
            // an empty picture, and the answer is plain anyway.
            if left == 0 {
                return Ok(None);
            }
            let mut config = SCStreamConfiguration::new()
                .with_width(size.0 as u32)
                .with_height(size.1 as u32)
                .with_shows_cursor(false);
            // Where the wallpaper was: the dark gray of a window background.
            config.set_background_color(0.13, 0.13, 0.14);
            // Without the wallpaper, the content would shrink to the windows'
            // bounds; pin it to the whole display. The source is in the
            // display's points, `size` in the image's pixels.
            let r = source.unwrap_or_else(|| {
                let bounds = super::cg::display_bounds(id);
                crate::model::Rect::new(0.0, 0.0, bounds.width, bounds.height)
            });
            config.set_source_rect(screencapturekit::cg::CGRect::new(
                r.x, r.y, r.width, r.height,
            ));
            SCScreenshotManager::capture_image(&filter, &config)
                .map(Some)
                .map_err(|e| e.to_string())
        };
    let mut result = capture(size, source);
    // ScreenCaptureKit sometimes fails to start the capture for a moment,
    // after a window on the display was resized: try again a few times.
    for wait in [100, 300, 700] {
        if result.is_ok() {
            break;
        }
        forget();
        std::thread::sleep(Duration::from_millis(wait));
        result = capture(size, source);
    }
    // Still failing on a region: cut it from a whole capture instead.
    if let (Err(error), Some(region)) = (&result, source) {
        log::debug!("zoom of display {id} failed ({error}); cropping a whole capture");
        let bounds = super::cg::display_bounds(id);
        let scale = (size.0 / region.width)
            .max(size.1 / region.height)
            .clamp(0.5, 2.0);
        let whole = (
            (bounds.width * scale).round(),
            (bounds.height * scale).round(),
        );
        match capture(whole, None) {
            Ok(Some(image)) => {
                let pixels = crate::model::Rect::new(
                    region.x * scale,
                    region.y * scale,
                    region.width * scale,
                    region.height * scale,
                );
                return crop(&image, pixels, size, format);
            }
            Ok(None) => return plain(size, format),
            Err(_) => {}
        }
    }
    match result? {
        Some(image) => encode(&image, format),
        None => plain(size, format),
    }
}

/// `rect` of an image, in its pixels, drawn at `size` and encoded.
fn crop(
    image: &CGImage,
    rect: crate::model::Rect,
    size: (f64, f64),
    format: Format,
) -> Result<Shot, String> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
    };
    unsafe extern "C" {
        fn CGImageCreateWithImageInRect(image: *const c_void, rect: CGRect) -> *const c_void;
    }
    let (width, height) = (size.0 as usize, size.1 as usize);
    unsafe {
        let cut = CGImageCreateWithImageInRect(
            image.as_ptr().cast(),
            CGRect::new(
                CGPoint::new(rect.x, rect.y),
                CGSize::new(rect.width, rect.height),
            ),
        );
        if cut.is_null() {
            return Err("could not crop the screenshot".into());
        }
        let space = CGColorSpace::new_device_rgb().ok_or("no color space")?;
        let context = CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            0,
            Some(&space),
            CGImageAlphaInfo::NoneSkipLast.0,
        )
        .ok_or("could not draw the screenshot")?;
        unsafe extern "C" {
            fn CGContextDrawImage(context: *const c_void, rect: CGRect, image: *const c_void);
        }
        CGContextDrawImage(
            objc2_core_foundation::CFRetained::as_ptr(&context)
                .as_ptr()
                .cast(),
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size.0, size.1)),
            cut,
        );
        CFRelease(cut);
        let drawn =
            CGBitmapContextCreateImage(Some(&context)).ok_or("could not draw the screenshot")?;
        encode_raw(
            objc2_core_foundation::CFRetained::as_ptr(&drawn)
                .as_ptr()
                .cast(),
            width,
            height,
            format,
        )
    }
}

/// One window without what lies above or below it. Blocking.
pub fn window(target: WindowTarget, size: (f64, f64)) -> Result<Shot, String> {
    let started = std::time::Instant::now();
    // The list shared with display screenshots: while displays record,
    // each listing waits seconds on ScreenCaptureKit.
    let mut content = content(false)?;
    if !content
        .windows()
        .iter()
        .any(|w| w.window_id() == target.window_id)
    {
        content = self::content(true)?;
    }
    let window = content
        .windows()
        .into_iter()
        .find(|w| w.window_id() == target.window_id)
        .ok_or("the window is not capturable; is Screen Recording granted?")?;
    let listed = started.elapsed();
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
    log::debug!(
        "window picture {}: listed in {} ms, captured by {} ms",
        target.window_id,
        listed.as_millis(),
        started.elapsed().as_millis()
    );
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

/// The empty desktop agents see: the background color, at the capture's
/// size.
fn plain(size: (f64, f64), format: Format) -> Result<Shot, String> {
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGContext,
        CGImageAlphaInfo,
    };
    let (width, height) = (size.0 as usize, size.1 as usize);
    unsafe {
        let space = CGColorSpace::new_device_rgb().ok_or("no color space")?;
        let context = CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            0,
            Some(&space),
            CGImageAlphaInfo::NoneSkipLast.0,
        )
        .ok_or("could not draw the screenshot")?;
        CGContext::set_rgb_fill_color(Some(&context), 0.13, 0.13, 0.14, 1.0);
        CGContext::fill_rect(
            Some(&context),
            objc2_core_foundation::CGRect::new(
                objc2_core_foundation::CGPoint::new(0.0, 0.0),
                objc2_core_foundation::CGSize::new(size.0, size.1),
            ),
        );
        let image =
            CGBitmapContextCreateImage(Some(&context)).ok_or("could not draw the screenshot")?;
        encode_raw(
            objc2_core_foundation::CFRetained::as_ptr(&image)
                .as_ptr()
                .cast(),
            width,
            height,
            format,
        )
    }
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
