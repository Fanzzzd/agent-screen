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
/// The last listing, when it was made, the ids of the windows on screen
/// then, and the display change stamp then.
struct Listing<T> {
    content: Arc<T>,
    at: Instant,
    ids: std::collections::HashSet<u32>,
    stamp: u64,
}

struct CacheState<T> {
    listing: Option<Listing<T>>,
    generation: u64,
}

impl<T> CacheState<T> {
    const fn new() -> Self {
        Self {
            listing: None,
            generation: 0,
        }
    }
}

/// Cache for `SCShareableContent` that never holds its lock across the
/// blocking `fetch` IPC. `forget()` bumps `generation` so an in-flight fetch
/// started before invalidation cannot store a stale listing back into the cache.
struct ContentCache<T> {
    state: Mutex<CacheState<T>>,
}

impl<T> ContentCache<T> {
    const fn new() -> Self {
        Self {
            state: Mutex::new(CacheState::new()),
        }
    }

    fn get_or_fetch<E>(
        &self,
        ids: Option<std::collections::HashSet<u32>>,
        stamp: u64,
        load_ids: impl FnOnce() -> std::collections::HashSet<u32>,
        fetch: impl FnOnce() -> Result<T, E>,
    ) -> Result<Arc<T>, E> {
        let generation = {
            let cache = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = &cache.listing {
                let current = entry.stamp == stamp
                    && match &ids {
                        Some(now) => now == &entry.ids,
                        None => entry.at.elapsed() < CONTENT_AGE,
                    };
                if current {
                    return Ok(entry.content.clone());
                }
            }
            cache.generation
        };
        let ids = ids.unwrap_or_else(load_ids);
        let content = Arc::new(fetch()?);
        let mut cache = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if cache.generation == generation {
            cache.listing = Some(Listing {
                content: content.clone(),
                at: Instant::now(),
                ids,
                stamp,
            });
        }
        Ok(content)
    }

    fn forget(&self) {
        let mut cache = self.state.lock().unwrap_or_else(|e| e.into_inner());
        cache.listing = None;
        cache.generation = cache.generation.wrapping_add(1);
    }
}

static CONTENT: ContentCache<SCShareableContent> = ContentCache::new();
const CONTENT_AGE: Duration = Duration::from_secs(2);

/// What ScreenCaptureKit can capture. A listing takes ScreenCaptureKit about
/// 20 ms; a fresh one is only needed once a window has come or gone, which
/// the window server's list of ids, at well under a millisecond, tells.
fn content(fresh: bool) -> Result<Arc<SCShareableContent>, String> {
    let ids = fresh.then(super::cg::on_screen_ids);
    content_with_ids(ids)
}

fn content_with_ids(
    ids: Option<std::collections::HashSet<u32>>,
) -> Result<Arc<SCShareableContent>, String> {
    // A listing from before a display change describes displays as they
    // were: a stream built from it after an unlock can show another
    // display (FB17797423).
    let stamp = super::cg::display_change_stamp();
    CONTENT.get_or_fetch(ids, stamp, super::cg::on_screen_ids, || {
        SCShareableContent::create()
            .with_on_screen_windows_only(true)
            .get()
            .map_err(|e| format!("could not list displays: {e}"))
    })
}

/// Whether ScreenCaptureKit lets this process list the screen. Blocking.
pub fn can_capture() -> bool {
    forget();
    content(true).is_ok()
}

/// Drop the kept listing: the next one is made anew. A window that moved
/// keeps its id, so a capture that failed may have used its old place.
fn forget() {
    CONTENT.forget();
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
    let area = super::cg::display_bounds(id);
    // Where windows are now, from the window server. Include desktop elements
    // so the same single WindowServer call supplies both frames and the exact
    // on-screen id set `content_with_ids` compares against (wallpaper windows
    // sit below layer 0 and are excluded by the `layer >= 24` and `0..1000`
    // filters below).
    let frames = super::cg::on_screen_frames_with_desktop();
    let across = |f: crate::model::Rect| {
        f.x < area.max_x() && f.max_x() > area.x && f.y < area.max_y() && f.max_y() > area.y
    };
    // The bar is one Window Server window at level 24, the status items
    // level-25 windows and the recording indicator a dot far above, all
    // within the top 40 pt. Full-screen windows at those levels, such as
    // the screenshot tool's, stay.
    let in_bar: std::collections::HashSet<u32> = frames
        .iter()
        .filter(|&&(_, layer, f)| {
            across(f)
                && layer >= 24
                && f.height <= 40.0
                && f.y >= area.y - 1.0
                && f.y < area.y + 40.0
        })
        .map(|&(window_id, _, _)| window_id)
        .collect();
    let screen_ids: Option<std::collections::HashSet<u32>> =
        fresh.then(|| frames.iter().map(|&(window_id, _, _)| window_id).collect());
    let mut content = content_with_ids(screen_ids.clone())?;
    let mut windows = content.windows();
    // A bar window the listing doesn't have yet can't be left out: list anew.
    let all_listed = |windows: &[SCWindow]| {
        if in_bar.is_empty() {
            return true;
        }
        let ids: std::collections::HashSet<u32> = windows.iter().map(|w| w.window_id()).collect();
        in_bar.iter().all(|w| ids.contains(w))
    };
    if !all_listed(&windows) {
        forget();
        let retry_ids = screen_ids
            .unwrap_or_else(|| frames.iter().map(|&(window_id, _, _)| window_id).collect());
        content = content_with_ids(Some(retry_ids))?;
        windows = content.windows();
        if !all_listed(&windows) {
            log::debug!("display {id}: a menu bar window is not in the listing yet");
        }
    }
    let display = content
        .displays()
        .into_iter()
        .find(|d| d.display_id() == id)
        .ok_or("display is not capturable yet; retry in a moment")?;
    let bar: Vec<SCWindow> = windows
        .into_iter()
        .filter(|w| {
            // The wallpaper and desktop icons sit below every normal level,
            // and stay where they are.
            let f = w.frame();
            let desktop = plain
                && w.window_layer() < 0
                && across(crate::model::Rect::new(
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                ));
            in_bar.contains(&w.window_id()) || desktop
        })
        .collect();
    // What an agent could see: app windows, panels and menus. The cursor,
    // the recording indicator and other overlays from layer 1000 up, and
    // 1-point status windows, don't make a picture.
    let left = frames
        .into_iter()
        .filter(|&(window_id, layer, f)| {
            (0..1000).contains(&layer)
                && f.width > 1.0
                && f.height > 1.0
                && across(f)
                && !in_bar.contains(&window_id)
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

/// Red, green and blue of `source`, a display-local rect in points, one
/// three-byte sample per point, rows top to bottom. What lies there, as an
/// agent's screenshot shows it:
/// a real click compares it before and after the user's answer (ADR 0013,
/// "Clicks"). Blocking: call off the main thread.
pub fn samples(id: DisplayId, source: crate::model::Rect) -> Result<Vec<u8>, String> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{CGBitmapContextGetData, CGContext};
    let (width, height) = (source.width as usize, source.height as usize);
    if width == 0 || height == 0 {
        return Err("nothing to capture".into());
    }
    let capture = || -> Result<Option<CGImage>, String> {
        let (filter, left) = filter_counting(id, true, true)?;
        if left == 0 {
            return Ok(None);
        }
        let mut config = SCStreamConfiguration::new()
            .with_width(width as u32)
            .with_height(height as u32)
            .with_shows_cursor(false);
        config.set_background_color(0.13, 0.13, 0.14);
        config.set_source_rect(screencapturekit::cg::CGRect::new(
            source.x,
            source.y,
            source.width,
            source.height,
        ));
        SCScreenshotManager::capture_image(&filter, &config)
            .map(Some)
            .map_err(|e| e.to_string())
    };
    let mut result = capture();
    for wait in [100, 300] {
        if result.is_ok() {
            break;
        }
        forget();
        std::thread::sleep(Duration::from_millis(wait));
        result = capture();
    }
    let image = result?;
    let context = rgb_bitmap_context(width, height, width * 4, "could not draw the region")?;
    unsafe {
        let all = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        match &image {
            Some(image) => {
                CGContextDrawImage(
                    objc2_core_foundation::CFRetained::as_ptr(&context)
                        .as_ptr()
                        .cast(),
                    all,
                    image.as_ptr().cast(),
                );
            }
            // Nothing but the desktop: the plain background.
            None => {
                CGContext::set_rgb_fill_color(Some(&context), 0.13, 0.13, 0.14, 1.0);
                CGContext::fill_rect(Some(&context), all);
            }
        }
        let data = CGBitmapContextGetData(Some(&context)) as *const u8;
        if data.is_null() {
            return Err("could not read the region".into());
        }
        let (pixels, _) = std::slice::from_raw_parts(data, width * height * 4).as_chunks::<4>();
        Ok(pixels.iter().flat_map(|p| [p[0], p[1], p[2]]).collect())
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
        let shot = draw_and_encode(cut, width, height, format, "could not draw the screenshot");
        CFRelease(cut);
        shot
    }
}

/// One window without what lies above or below it. Blocking.
pub fn window(target: WindowTarget, size: (f64, f64)) -> Result<Shot, String> {
    let started = std::time::Instant::now();
    // The list shared with display screenshots: while displays record,
    // each listing waits seconds on ScreenCaptureKit.
    let find = |content: &SCShareableContent| {
        content
            .windows()
            .into_iter()
            .find(|w| w.window_id() == target.window_id)
    };
    let window = match find(&*content(false)?) {
        Some(window) => window,
        None => find(&*self::content(true)?)
            .ok_or("the window is not capturable; is Screen Recording granted?")?,
    };
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

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGContextDrawImage(
        context: *const c_void,
        rect: objc2_core_foundation::CGRect,
        image: *const c_void,
    );
    fn CGImageCreateWithImageInRect(
        image: *const c_void,
        rect: objc2_core_foundation::CGRect,
    ) -> *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: *const c_void);
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    fn VTCreateCGImageFromCVPixelBuffer(
        pixel_buffer: *const c_void,
        options: *const c_void,
        image_out: *mut *mut c_void,
    ) -> i32;
}

fn rgb_bitmap_context(
    width: usize,
    height: usize,
    bytes_per_row: usize,
    error: &str,
) -> Result<objc2_core_foundation::CFRetained<objc2_core_graphics::CGContext>, String> {
    let space = objc2_core_graphics::CGColorSpace::new_device_rgb().ok_or("no color space")?;
    unsafe {
        objc2_core_graphics::CGBitmapContextCreate(
            std::ptr::null_mut(),
            width,
            height,
            8,
            bytes_per_row,
            Some(&space),
            objc2_core_graphics::CGImageAlphaInfo::NoneSkipLast.0,
        )
    }
    .ok_or_else(|| error.to_string())
}

unsafe fn draw_and_encode(
    image: *const c_void,
    width: usize,
    height: usize,
    format: Format,
    error: &str,
) -> Result<Shot, String> {
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    let context = rgb_bitmap_context(width, height, 0, error)?;
    unsafe {
        CGContextDrawImage(
            objc2_core_foundation::CFRetained::as_ptr(&context)
                .as_ptr()
                .cast(),
            CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(width as f64, height as f64),
            ),
            image,
        );
        let drawn = objc2_core_graphics::CGBitmapContextCreateImage(Some(&context))
            .ok_or_else(|| error.to_string())?;
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

/// Convert a 420f `CVPixelBuffer` from `Capture` into a JPEG or PNG, scaling
/// down proportionally when wider than `max_width`. Blocking: call off the
/// main thread.
pub fn encode_pixel_buffer(
    buffer: &core_video::pixel_buffer::CVPixelBuffer,
    max_width: usize,
    format: Format,
) -> Result<Shot, String> {
    use core_foundation::base::TCFType;

    struct CfImage(*mut c_void);
    impl Drop for CfImage {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }

    let src_w = buffer.get_width();
    let src_h = buffer.get_height();
    if src_w == 0 || src_h == 0 {
        return Err("empty pixel buffer".into());
    }
    let mut raw_image: *mut c_void = std::ptr::null_mut();
    let status = unsafe {
        VTCreateCGImageFromCVPixelBuffer(
            buffer.as_concrete_TypeRef().cast(),
            std::ptr::null(),
            &mut raw_image,
        )
    };
    if status != 0 || raw_image.is_null() {
        return Err(format!("could not convert pixel buffer ({status})"));
    }
    let image = CfImage(raw_image);

    let max_w = max_width.max(1);
    if src_w <= max_w {
        return encode_raw(image.0, src_w, src_h, format);
    }
    let dst_w = max_w;
    let dst_h = ((src_h * dst_w + (src_w / 2)) / src_w).max(1);
    unsafe { draw_and_encode(image.0, dst_w, dst_h, format, "could not draw scaled frame") }
}

/// The empty desktop agents see: the background color, at the capture's
/// size.
fn plain(size: (f64, f64), format: Format) -> Result<Shot, String> {
    use objc2_core_graphics::{CGBitmapContextCreateImage, CGContext};
    let (width, height) = (size.0 as usize, size.1 as usize);
    let context = rgb_bitmap_context(width, height, 0, "could not draw the screenshot")?;
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
    let options: Option<Retained<NSDictionary<NSString, AnyObject>>> = (format == Format::Jpeg)
        .then(|| {
            NSDictionary::from_retained_objects(
                &[&*NSString::from_str(
                    "kCGImageDestinationLossyCompressionQuality",
                )],
                &[Retained::into_super(Retained::into_super(NSNumber::new_f64(0.8))).into()],
            )
        });
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
        let properties = options
            .as_ref()
            .map_or(std::ptr::null(), |o| Retained::as_ptr(o).cast());
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

#[cfg(test)]
mod content_cache_tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Barrier;

    fn set(ids: &[u32]) -> HashSet<u32> {
        ids.iter().copied().collect()
    }

    #[test]
    fn content_cache_reuses_entry_when_ids_and_stamp_match() {
        let cache = ContentCache::new();
        let mut fetches = 0usize;
        let a = cache
            .get_or_fetch(
                Some(set(&[1, 2])),
                10,
                || unreachable!(),
                || -> Result<usize, ()> {
                    fetches += 1;
                    Ok(fetches)
                },
            )
            .unwrap();
        let b = cache
            .get_or_fetch(
                Some(set(&[1, 2])),
                10,
                || unreachable!(),
                || -> Result<usize, ()> {
                    fetches += 1;
                    Ok(fetches)
                },
            )
            .unwrap();
        assert_eq!(*a, 1);
        assert_eq!(*b, 1);
        assert_eq!(fetches, 1);

        // Changed window id set triggers refresh.
        let c = cache
            .get_or_fetch(
                Some(set(&[1, 2, 3])),
                10,
                || unreachable!(),
                || -> Result<usize, ()> {
                    fetches += 1;
                    Ok(fetches)
                },
            )
            .unwrap();
        assert_eq!(*c, 2);

        // Changed display stamp triggers refresh even when ids match.
        let d = cache
            .get_or_fetch(
                Some(set(&[1, 2, 3])),
                11,
                || unreachable!(),
                || -> Result<usize, ()> {
                    fetches += 1;
                    Ok(fetches)
                },
            )
            .unwrap();
        assert_eq!(*d, 3);
    }

    #[test]
    fn content_cache_does_not_hold_lock_during_fetch_and_discards_stale_after_forget() {
        let cache = Arc::new(ContentCache::new());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));

        let c1 = Arc::clone(&cache);
        let e1 = Arc::clone(&entered);
        let r1 = Arc::clone(&release);
        let slow = std::thread::spawn(move || {
            c1.get_or_fetch(
                Some(set(&[1])),
                1,
                || unreachable!(),
                || -> Result<&'static str, ()> {
                    e1.wait();
                    r1.wait();
                    Ok("stale")
                },
            )
            .unwrap()
        });

        // Wait until `slow` is inside `fetch()`.
        entered.wait();
        // `forget()` and a newer fetch must complete immediately without blocking on `slow`.
        let start = Instant::now();
        cache.forget();
        let fresh = cache
            .get_or_fetch(
                Some(set(&[1])),
                1,
                || unreachable!(),
                || -> Result<&'static str, ()> { Ok("fresh") },
            )
            .unwrap();
        assert_eq!(*fresh, "fresh");
        assert!(start.elapsed() < Duration::from_millis(50));

        // Let `slow` finish; its stale result must not overwrite `"fresh"`.
        release.wait();
        assert_eq!(*slow.join().unwrap(), "stale");
        let after = cache
            .get_or_fetch(
                Some(set(&[1])),
                1,
                || unreachable!(),
                || -> Result<&'static str, ()> { Ok("unexpected") },
            )
            .unwrap();
        assert_eq!(*after, "fresh");
    }

    #[test]
    fn frames_with_desktop_matches_on_screen_ids_set() {
        // Both query `OptionOnScreenOnly`, so their sets only differ if a
        // window opened or closed between the two calls.
        let from_frames: HashSet<u32> = super::super::cg::on_screen_frames_with_desktop()
            .into_iter()
            .map(|(id, _, _)| id)
            .collect();
        let from_ids = super::super::cg::on_screen_ids();
        assert!(!from_frames.is_empty());
        assert!(!from_ids.is_empty());
        // Overlap should be nearly 100% (allowing at most transient window churn).
        let overlap = from_frames.intersection(&from_ids).count();
        assert!(
            overlap * 10 >= from_ids.len() * 8,
            "expected high overlap between on_screen_frames_with_desktop ({}) and on_screen_ids ({}), got {overlap}",
            from_frames.len(),
            from_ids.len()
        );
    }
}
