//! Live frames of one display through ScreenCaptureKit, as 420f
//! CVPixelBuffers that GPUI's `surface` element draws without a copy.
//!
//! With several virtual displays connected, a stream started after a
//! display change can deliver another virtual display's picture, whatever
//! its filter says (Apple FB17797423): here, AeroSpace's closet with the
//! user's hidden windows. Each frame says which screen it shows, so frames
//! from the wrong one are dropped, and screenshots, which that bug spares,
//! stand in until the stream shows the right display again.

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
use screencapturekit::cm::{CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::SCScreenshotManager;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The newest frame. Older ones are dropped: the Viewer only ever shows the
/// latest.
#[derive(Default)]
pub struct Latest {
    frame: Mutex<Option<Frame>>,
    pub delivered: AtomicU64,
    /// Frames that changed the picture visibly below the menu bar. An
    /// animated wallpaper drifts a little everywhere and a menu bar clock
    /// ticks; neither counts, so the number stops once the UI is still.
    pub changes: AtomicU64,
    /// Luma samples of the previous frame, for the comparison.
    samples: Mutex<Vec<u8>>,
    /// Count changes: only while an agent waits for the display to settle.
    /// Sampling locks each frame's IOSurface for the CPU, the largest cost
    /// of showing a 60 fps display in the Viewer.
    counting: std::sync::atomic::AtomicBool,
}

impl Latest {
    pub fn count_changes(&self, on: bool) {
        self.counting.store(on, Ordering::Relaxed);
        if !on {
            self.samples.lock().unwrap().clear();
        }
    }
}

/// Samples per row and column of the comparison grid.
const GRID: (usize, usize) = (160, 100);
/// A sample changed visibly when its luma moved by more than this.
const LEVELS: u8 = 24;
/// The picture changed when at least this many samples did.
const SAMPLES: usize = 3;

/// Luma on a coarse grid below the top `skip` rows. Locks the buffer.
fn luma_grid(buffer: &CVPixelBuffer, skip: f64) -> Vec<u8> {
    const READ_ONLY: u64 = 1;
    if buffer.lock_base_address(READ_ONLY) != 0 {
        return Vec::new();
    }
    let (width, height) = (buffer.get_width_of_plane(0), buffer.get_height_of_plane(0));
    let row_bytes = buffer.get_bytes_per_row_of_plane(0);
    let base = unsafe { buffer.get_base_address_of_plane(0) } as *const u8;
    let top = ((height as f64) * skip) as usize;
    let mut out = Vec::with_capacity(GRID.0 * GRID.1);
    if !base.is_null() && width > 0 && height > top {
        for gy in 0..GRID.1 {
            let y = top + (height - top - 1) * gy / (GRID.1 - 1);
            for gx in 0..GRID.0 {
                let x = (width - 1) * gx / (GRID.0 - 1);
                out.push(unsafe { *base.add(y * row_bytes + x) });
            }
        }
    }
    buffer.unlock_base_address(READ_ONLY);
    out
}

/// Whether `now` differs visibly from `before`.
fn changed(before: &[u8], now: &[u8]) -> bool {
    if before.len() != now.len() {
        return true;
    }
    before
        .iter()
        .zip(now)
        .filter(|(a, b)| a.abs_diff(**b) > LEVELS)
        .nth(SAMPLES - 1)
        .is_some()
}

pub struct Frame(pub CVPixelBuffer);
// CVPixelBuffer is a retained, immutable CF object; moving it between
// threads is safe.
unsafe impl Send for Frame {}

impl Latest {
    pub fn take(&self) -> Option<CVPixelBuffer> {
        self.frame.lock().unwrap().take().map(|f| f.0)
    }
}

/// Screenshots stand in for a misrouted stream at most this often: each
/// takes 70 to 100 ms.
const STAND_IN_FPS: i32 = 15;

/// What the stream's frame handler and the stand-in thread share.
struct Shared {
    display_id: u32,
    filter: Mutex<SCContentFilter>,
    target: Mutex<(i32, (u32, u32))>,
    /// The stream's last frame showed another screen.
    misrouted: AtomicBool,
    stopped: AtomicBool,
    deliver: Box<dyn Fn(CVPixelBuffer) + Send + Sync>,
}

/// Whether a frame shows `display_id`, by the screen rect ScreenCaptureKit
/// attached to it. A frame without one passes.
fn shows(sample: &CMSampleBuffer, display_id: u32) -> Result<(), CGRect> {
    let Some(screen) = sample.screen_rect() else {
        return Ok(());
    };
    let want = super::cg::display_bounds(display_id);
    let near = |a: f64, b: f64| (a - b).abs() < 1.0;
    if near(screen.origin.x, want.x)
        && near(screen.origin.y, want.y)
        && near(screen.size.width, want.width)
        && near(screen.size.height, want.height)
    {
        Ok(())
    } else {
        Err(screen)
    }
}

/// Screenshots of the display while its stream is misrouted, paced to the
/// wanted rate up to `STAND_IN_FPS`. Sleeps otherwise.
fn stand_in(shared: Arc<Shared>) {
    while !shared.stopped.load(Ordering::Relaxed) {
        if !shared.misrouted.load(Ordering::Relaxed) {
            std::thread::park_timeout(Duration::from_millis(500));
            continue;
        }
        let started = Instant::now();
        let (fps, pixels) = *shared.target.lock().unwrap();
        let filter = shared.filter.lock().unwrap().clone();
        match SCScreenshotManager::capture_sample_buffer(
            &filter,
            &configuration(pixels, fps.max(1)),
        ) {
            Ok(sample) => {
                let raw = sample.image_buffer_ptr_borrowed();
                // The stream may have come right meanwhile; its frames win.
                if !raw.is_null() && shared.misrouted.load(Ordering::Relaxed) {
                    let buffer =
                        unsafe { CVPixelBuffer::wrap_under_get_rule(raw as CVPixelBufferRef) };
                    (shared.deliver)(buffer);
                }
            }
            Err(error) => log::debug!(
                "stand-in screenshot of display {} failed: {error:?}",
                shared.display_id
            ),
        }
        let period = Duration::from_secs(1) / fps.clamp(1, STAND_IN_FPS) as u32;
        if let Some(rest) = period.checked_sub(started.elapsed()) {
            std::thread::park_timeout(rest);
        }
    }
}

pub struct Capture {
    stream: SCStream,
    turn: Mutex<()>,
    shared: Arc<Shared>,
    stand_in: std::thread::Thread,
}

impl Capture {
    /// Start capturing `display_id` at its full pixel size. `on_frame` runs on
    /// ScreenCaptureKit's queue after each new frame is stored.
    pub fn start(
        display_id: u32,
        pixels: (u32, u32),
        fps: i32,
        latest: Arc<Latest>,
        on_frame: impl Fn() + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        // The menu bar strip, about 24 to 38 pt, is left out of the change
        // count: it shows the user's own menus and clock.
        let skip = 40.0 / super::cg::display_bounds(display_id).height.max(1.0);
        let filter =
            super::screenshot::filter(display_id, true, false).map_err(anyhow::Error::msg)?;
        let deliver = move |buffer: CVPixelBuffer| {
            if latest.counting.load(Ordering::Relaxed) {
                let now = luma_grid(&buffer, skip);
                let mut before = latest.samples.lock().unwrap();
                if changed(&before, &now) {
                    latest.changes.fetch_add(1, Ordering::Relaxed);
                }
                *before = now;
            }
            *latest.frame.lock().unwrap() = Some(Frame(buffer));
            latest.delivered.fetch_add(1, Ordering::Relaxed);
            on_frame();
        };
        let shared = Arc::new(Shared {
            display_id,
            filter: Mutex::new(filter.clone()),
            target: Mutex::new((fps, pixels)),
            misrouted: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            deliver: Box::new(deliver),
        });
        let stand_in = {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name(format!("stand-in {display_id}"))
                .spawn(move || stand_in(shared))?
                .thread()
                .clone()
        };
        let mut stream = SCStream::new(&filter, &configuration(pixels, fps))?;
        let handler = shared.clone();
        let waker = stand_in.clone();
        // Tests: every frame counts as another display's, so screenshots
        // stand in whatever the stream does.
        let forced = cfg!(debug_assertions)
            && std::env::var_os("SIDESCREEN_STAND_IN").is_some_and(|v| v == "1");
        stream.add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                // Idle frames repeat the last image: the display did not
                // change, so there is nothing to draw.
                if kind != SCStreamOutputType::Screen
                    || sample.frame_status() != Some(SCFrameStatus::Complete)
                {
                    return;
                }
                let verdict = if forced {
                    Err(None)
                } else {
                    shows(&sample, display_id).map_err(Some)
                };
                if let Err(screen) = verdict {
                    if !handler.misrouted.swap(true, Ordering::Relaxed) {
                        let at = screen.map(|s| (s.origin.x, s.origin.y, s.size.width, s.size.height));
                        log::warn!(
                            "display {display_id}: the stream shows the screen at {at:?}; screenshots stand in"
                        );
                        waker.unpark();
                    }
                    return;
                }
                if handler.misrouted.swap(false, Ordering::Relaxed) {
                    log::info!("display {display_id}: the stream shows it again");
                }
                let raw = sample.image_buffer_ptr_borrowed();
                if raw.is_null() {
                    return;
                }
                // Retain: the sample buffer goes away after this callback.
                let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(raw as CVPixelBufferRef) };
                (handler.deliver)(buffer);
            },
            SCStreamOutputType::Screen,
        )?;
        if let Err(error) = stream.start_capture() {
            shared.stopped.store(true, Ordering::Relaxed);
            stand_in.unpark();
            return Err(error.into());
        }
        Ok(Self {
            stream,
            turn: Mutex::new(()),
            shared,
            stand_in,
        })
    }

    /// Whether screenshots stand in for this stream: it last showed another
    /// display.
    pub fn misrouted(&self) -> bool {
        self.shared.misrouted.load(Ordering::Relaxed)
    }

    /// Change the rate and size to `target` without restarting the
    /// stream. Calls run one at a time and read `target` when their turn
    /// comes, so the last one leaves the newest rate in place.
    pub fn reconfigure(&self, target: &Mutex<(i32, (u32, u32))>) -> anyhow::Result<()> {
        let _turn = self.turn.lock().unwrap();
        let (fps, pixels) = *target.lock().unwrap();
        *self.shared.target.lock().unwrap() = (fps, pixels);
        if fps > 0 {
            self.stream
                .update_configuration(&configuration(pixels, fps))?;
        }
        Ok(())
    }
}

impl Capture {
    /// Leave out status items that appeared since the stream started.
    /// Blocking.
    pub fn refresh_filter(&self, display_id: u32) {
        let _turn = self.turn.lock().unwrap();
        if let Ok(filter) = super::screenshot::filter(display_id, false, false) {
            if let Err(error) = self.stream.update_content_filter(&filter) {
                log::debug!("capture filter of display {display_id} stays: {error}");
            }
            *self.shared.filter.lock().unwrap() = filter;
        }
    }
}

fn configuration(pixels: (u32, u32), fps: i32) -> SCStreamConfiguration {
    let mut configuration = SCStreamConfiguration::new()
        .with_width(pixels.0)
        .with_height(pixels.1)
        .with_pixel_format(PixelFormat::YCbCr_420f)
        .with_minimum_frame_interval(&CMTime::new(1, fps))
        .with_shows_cursor(false)
        .with_queue_depth(6);
    // Automatic renders a virtual display at 1x and scales that up to the
    // size asked for, so a 2x frame of it is blurry. Best renders at 2x.
    if let Err(error) = configuration.set_capture_resolution_type(
        screencapturekit::stream::configuration::SCCaptureResolutionType::Best,
    ) {
        log::debug!("capture resolution stays automatic: {error:?}");
    }
    configuration
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Relaxed);
        self.stand_in.unpark();
        let _ = self.stream.stop_capture();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_everywhere_is_not_a_change() {
        let before = vec![100u8; GRID.0 * GRID.1];
        let now: Vec<u8> = before.iter().map(|v| v + 10).collect();
        assert!(!changed(&before, &now));
    }

    #[test]
    fn a_few_samples_changing_a_lot_is_a_change() {
        let before = vec![100u8; GRID.0 * GRID.1];
        let mut now = before.clone();
        for v in now.iter_mut().take(SAMPLES) {
            *v = 200;
        }
        assert!(changed(&before, &now));
        now[SAMPLES - 1] = 100;
        assert!(!changed(&before, &now));
    }
}
