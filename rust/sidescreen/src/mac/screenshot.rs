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
static CONTENT: Mutex<Option<(Arc<SCShareableContent>, Instant)>> = Mutex::new(None);
const CONTENT_AGE: Duration = Duration::from_secs(2);

fn content(fresh: bool) -> Result<Arc<SCShareableContent>, String> {
    let mut cache = CONTENT.lock().unwrap();
    if !fresh
        && let Some((content, at)) = &*cache
        && at.elapsed() < CONTENT_AGE
    {
        return Ok(content.clone());
    }
    let content = Arc::new(
        SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .get()
            .map_err(|e| format!("could not list displays: {e}"))?,
    );
    *cache = Some((content.clone(), Instant::now()));
    Ok(content)
}

/// Whether ScreenCaptureKit lets this process list the screen. Blocking.
pub fn can_capture() -> bool {
    content(true).is_ok()
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
        return Err("macOS mixes this display up with another one right now, so Sidescreen won't capture it".into());
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
    // 1-point status windows, don't make a picture.
    let left = content
        .windows()
        .into_iter()
        .filter(|w| {
            let f = w.frame();
            (0..1000).contains(&w.window_layer())
                && f.size.width > 1.0
                && f.size.height > 1.0
                && f.origin.x < area.max_x()
                && f.origin.x + f.size.width > area.x
                && f.origin.y < area.max_y()
                && f.origin.y + f.size.height > area.y
                && !bar.iter().any(|b| b.window_id() == w.window_id())
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
    let capture = |fresh: bool| -> Result<Option<CGImage>, String> {
        let (filter, left) = filter_counting(id, fresh, true)?;
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
        // bounds; pin it to the whole display.
        let r = source.unwrap_or(crate::model::Rect::new(0.0, 0.0, size.0, size.1));
        config.set_source_rect(screencapturekit::cg::CGRect::new(
            r.x, r.y, r.width, r.height,
        ));
        SCScreenshotManager::capture_image(&filter, &config)
            .map(Some)
            .map_err(|e| e.to_string())
    };
    // A fresh listing every time: a cached one misses the wallpaper and
    // menu bar windows a new display gets, so they'd show, and names
    // windows that closed since, which fails the capture.
    let image = match capture(true) {
        Ok(image) => image,
        Err(_) => {
            std::thread::sleep(Duration::from_millis(100));
            capture(true)?
        }
    };
    match image {
        Some(image) => encode(&image, format),
        None => plain(size, format),
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
