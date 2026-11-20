//! Live frames of one display through ScreenCaptureKit, as 420f
//! CVPixelBuffers that GPUI's `surface` element draws without a copy.
//!
//! ScreenCaptureKit tells displays apart by unit number. When two share one,
//! a capture of either can show the other: AeroSpace's closet, with the
//! user's hidden windows. `screenshot::filter` refuses a display with a
//! twin (`cg::unit_twin`), and each frame's screen rect is checked as well:
//! frames of another screen are dropped.

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
use screencapturekit::cm::{CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The newest frame. Older ones are dropped: the Viewer only ever shows the
/// latest.
#[derive(Default)]
pub struct Latest {
    frame: Mutex<Option<Frame>>,
    current: Mutex<Option<(Frame, u64)>>,
    pub notify: tokio::sync::Notify,
    pub delivered: AtomicU64,
    /// The serial of the capture that stored the newest frame.
    pub from: AtomicU64,
    /// Highest capture serial that has delivered at least one verified frame
    /// of this display. Used to reset repair backoff even when no UI view is
    /// waking on frames.
    pub good_serial: AtomicU64,
    /// Frames that changed the picture visibly below the menu bar. An
    /// animated wallpaper drifts a little everywhere and a menu bar clock
    /// ticks; neither counts, so the number stops once the UI is still.
    pub changes: AtomicU64,
    /// Top-strip skip fraction (`f64::to_bits`) for seeding luma samples when
    /// change counting turns back on over a warm stream.
    skip_bits: AtomicU64,
    /// Luma samples of the previous frame and a reusable scratch buffer, so
    /// sampling does zero heap allocations per frame.
    samples: Mutex<(Vec<u8>, Vec<u8>)>,
    /// Count changes: only while an agent waits for the display to settle.
    /// Sampling locks each frame's IOSurface for the CPU, the largest cost of
    /// showing a 60 fps display in the Viewer.
    counting: std::sync::atomic::AtomicBool,
}

impl Latest {
    /// The changes counted so far, once counting compares frames: None
    /// while it is off, or before its first frame, which is what any
    /// change is compared with.
    pub fn counted(&self) -> Option<u64> {
        (self.counting.load(Ordering::Acquire) && !self.samples.lock().unwrap().0.is_empty())
            .then(|| self.changes.load(Ordering::Relaxed))
    }

    /// Whether changes are being counted, as while an agent acts.
    pub fn counting(&self) -> bool {
        self.counting.load(Ordering::Acquire)
    }

    pub fn count_changes(&self, on: bool) {
        if on {
            // Seed the comparison baseline from the warm stream's current frame
            // before flipping `counting` on: ScreenCaptureKit sends no frame
            // while the display is still, so without a seeded baseline the next
            // action's first changed frame would be treated as the baseline and
            // missed by `shared_app::Watcher`.
            let current = self.current();
            let mut s = self.samples.lock().unwrap();
            if s.0.is_empty()
                && let Some((frame, _)) = current
            {
                let skip = f64::from_bits(self.skip_bits.load(Ordering::Relaxed));
                luma_grid_into(&frame.0, skip, &mut s.0);
            }
            self.counting.store(true, Ordering::Release);
        } else {
            self.counting.store(false, Ordering::Release);
            let mut s = self.samples.lock().unwrap();
            s.0.clear();
            s.1.clear();
        }
    }

    /// Store a verified frame for `serial`, update change counting if enabled,
    /// record `good_serial`, and wake waiters.
    fn record_frame(&self, serial: u64, buffer: CVPixelBuffer, skip: f64) {
        self.skip_bits.store(skip.to_bits(), Ordering::Relaxed);
        if self.counting.load(Ordering::Acquire) {
            let mut guard = self.samples.lock().unwrap();
            let (before, now) = &mut *guard;
            luma_grid_into(&buffer, skip, now);
            if changed(before, now) {
                self.changes.fetch_add(1, Ordering::Relaxed);
            }
            std::mem::swap(before, now);
        }
        self.good_serial.fetch_max(serial, Ordering::Relaxed);
        self.from.store(serial, Ordering::Relaxed);
        let seq = self.delivered.fetch_add(1, Ordering::AcqRel) + 1;
        *self.current.lock().unwrap() = Some((Frame(buffer.clone()), seq));
        *self.frame.lock().unwrap() = Some(Frame(buffer));
        self.notify.notify_waiters();
    }
}

/// Samples per row and column of the comparison grid.
const GRID: (usize, usize) = (160, 100);
/// A sample changed visibly when its luma moved by more than this.
const LEVELS: u8 = 24;
/// The picture changed when at least this many samples did.
const SAMPLES: usize = 3;

/// Luma on a coarse grid below the top `skip` rows, written into `out` to
/// avoid per-frame allocations. Locks the buffer.
fn luma_grid_into(buffer: &CVPixelBuffer, skip: f64, out: &mut Vec<u8>) {
    out.clear();
    const READ_ONLY: u64 = 1;
    if buffer.lock_base_address(READ_ONLY) != 0 {
        return;
    }
    let (width, height) = (buffer.get_width_of_plane(0), buffer.get_height_of_plane(0));
    let row_bytes = buffer.get_bytes_per_row_of_plane(0);
    let base = unsafe { buffer.get_base_address_of_plane(0) } as *const u8;
    let top = ((height as f64) * skip) as usize;
    let needed = GRID.0 * GRID.1;
    if out.capacity() < needed {
        out.reserve(needed - out.len());
    }
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
}

/// Whether `now` differs visibly from `before`.
fn changed(before: &[u8], now: &[u8]) -> bool {
    // The first frame since counting began, which a stream started for the
    // wait sends at once, is what the display looks like: no change. Else
    // the wait ends on it, before the app draws what the action did.
    if before.is_empty() {
        return false;
    }
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

impl Frame {
    pub fn size(&self) -> (usize, usize) {
        (self.0.get_width(), self.0.get_height())
    }
}

impl Latest {
    pub fn take(&self) -> Option<CVPixelBuffer> {
        self.frame.lock().unwrap().take().map(|f| f.0)
    }

    /// Retain the latest frame and its `delivered` sequence number without
    /// removing it from `Streams`.
    pub fn current(&self) -> Option<(Frame, u64)> {
        self.current
            .lock()
            .unwrap()
            .as_ref()
            .map(|(f, seq)| (Frame(f.0.clone()), *seq))
    }

    pub fn clear(&self) {
        *self.frame.lock().unwrap() = None;
        *self.current.lock().unwrap() = None;
        let mut s = self.samples.lock().unwrap();
        s.0.clear();
        s.1.clear();
        self.notify.notify_waiters();
    }
}

/// Whether a frame shows the display whose bounds are `want`, by the screen
/// rect ScreenCaptureKit attached to it. A frame without one passes.
fn shows_in(sample: &CMSampleBuffer, want: crate::model::Rect) -> Result<(), CGRect> {
    let Some(screen) = sample.screen_rect() else {
        return Ok(());
    };
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

/// Check a frame's screen rect against cached display bounds, re-reading
/// live bounds once on mismatch so a park or resize that arrives ahead of the
/// display-change callback does not falsely trigger a misroute.
fn verify_frame_bounds(
    cached: &Mutex<(u64, crate::model::Rect)>,
    stamp: u64,
    mut read_bounds: impl FnMut() -> (u64, crate::model::Rect),
    mut check: impl FnMut(crate::model::Rect) -> Result<(), CGRect>,
) -> Result<crate::model::Rect, CGRect> {
    let want = {
        let mut guard = cached.lock().unwrap_or_else(|e| e.into_inner());
        if guard.0 != stamp {
            *guard = read_bounds();
        }
        guard.1
    };
    match check(want) {
        Ok(()) => Ok(want),
        Err(_) => {
            let fresh = read_bounds();
            *cached.lock().unwrap_or_else(|e| e.into_inner()) = fresh;
            check(fresh.1).map(|()| fresh.1)
        }
    }
}

pub struct Capture {
    stream: SCStream,
    turn: Mutex<()>,
    /// Tells this capture from the one that replaced it.
    serial: u64,
    /// The last frame showed another screen.
    misrouted: Arc<AtomicBool>,
    /// macOS stopped the stream.
    stopped: Arc<AtomicBool>,
}

static SERIAL: AtomicU64 = AtomicU64::new(1);

/// Test builds in the e2e instance with AGENTSCREEN_TEST_MISROUTE set take
/// every frame of a stream for another screen's once displays changed after
/// it started, as ScreenCaptureKit's streams can do after an unlock. A
/// stream started since passes.
fn misroute_test() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("AGENTSCREEN_TEST_MISROUTE").is_some()
        && std::env::var("AGENTSCREEN_INSTANCE").as_deref() == Ok("e2e")
}

impl Capture {
    /// Start capturing `display_id` at its full pixel size. `on_frame` runs on
    /// ScreenCaptureKit's queue after each new frame is stored. `on_broken`
    /// runs there with the capture's serial once the stream shows another
    /// screen or macOS stops it: it needs starting again.
    pub fn start(
        display_id: u32,
        pixels: (u32, u32),
        fps: i32,
        latest: Arc<Latest>,
        on_frame: impl Fn() + Send + Sync + 'static,
        on_broken: impl Fn(u64) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        // The menu bar strip, about 24 to 38 pt, is left out of the change
        // count: it shows the user's own menus and clock.
        let initial_bounds = super::cg::display_bounds(display_id);
        let initial_skip = 40.0 / initial_bounds.height.max(1.0);
        latest
            .skip_bits
            .store(initial_skip.to_bits(), Ordering::Relaxed);
        let filter =
            super::screenshot::filter(display_id, true, false).map_err(anyhow::Error::msg)?;
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let on_broken = Arc::new(on_broken);
        let stopped = Arc::new(AtomicBool::new(false));
        let delegate = {
            let stopped = stopped.clone();
            let on_broken = on_broken.clone();
            screencapturekit::stream::delegate_trait::StreamCallbacks::new().on_stop(move |error| {
                let error = error.unwrap_or_default();
                log::warn!("display {display_id}: macOS stopped its stream: {error}");
                crate::lifecycle::note("stream-stopped", display_id, &error);
                stopped.store(true, Ordering::Relaxed);
                on_broken(serial);
            })
        };
        let mut stream =
            SCStream::new_with_delegate(&filter, &configuration(pixels, fps), delegate)?;
        let misrouted = Arc::new(AtomicBool::new(false));
        let flag = misrouted.clone();
        let born = super::cg::display_change_stamp();
        let cached_bounds = Mutex::new((born, initial_bounds));
        let test = misroute_test();
        stream.add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                // Idle frames repeat the last image: the display did not
                // change, so there is nothing to draw.
                if kind != SCStreamOutputType::Screen
                    || sample.frame_status() != Some(SCFrameStatus::Complete)
                {
                    return;
                }
                let stamp = super::cg::display_change_stamp();
                let shown = verify_frame_bounds(
                    &cached_bounds,
                    stamp,
                    || {
                        (
                            super::cg::display_change_stamp(),
                            super::cg::display_bounds(display_id),
                        )
                    },
                    |bounds| shows_in(&sample, bounds),
                )
                .and_then(|want| {
                    if test && stamp != born {
                        Err(CGRect {
                            origin: CGPoint { x: -1.0, y: -1.0 },
                            size: CGSize {
                                width: 1.0,
                                height: 1.0,
                            },
                        })
                    } else {
                        Ok(want)
                    }
                });
                let want = match shown {
                    Ok(want) => want,
                    Err(screen) => {
                        if !flag.swap(true, Ordering::Relaxed) {
                            let at = format!(
                                "shows={},{} {}x{} twin={:?}",
                                screen.origin.x,
                                screen.origin.y,
                                screen.size.width,
                                screen.size.height,
                                super::cg::unit_twin(display_id),
                            );
                            log::warn!(
                                "display {display_id}: the stream {at}; its frames are dropped"
                            );
                            crate::lifecycle::note("misroute", display_id, &at);
                            on_broken(serial);
                        }
                        return;
                    }
                };
                if flag.swap(false, Ordering::Relaxed) {
                    log::info!("display {display_id}: the stream shows it again");
                }
                let raw = sample.image_buffer_ptr_borrowed();
                if raw.is_null() {
                    return;
                }
                // Retain: the sample buffer goes away after this callback.
                let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(raw as CVPixelBufferRef) };
                let skip = 40.0 / want.height.max(1.0);
                latest.record_frame(serial, buffer, skip);
                on_frame();
            },
            SCStreamOutputType::Screen,
        )?;
        stream.start_capture()?;
        Ok(Self {
            stream,
            turn: Mutex::new(()),
            serial,
            misrouted,
            stopped,
        })
    }

    pub fn serial(&self) -> u64 {
        self.serial
    }

    /// Whether the stream's last frame showed another screen.
    pub fn misrouted(&self) -> bool {
        self.misrouted.load(Ordering::Relaxed)
    }

    /// Whether the stream needs starting again: it shows another screen,
    /// or macOS stopped it.
    pub fn broken(&self) -> bool {
        self.misrouted() || self.stopped.load(Ordering::Relaxed)
    }

    /// Change the rate and size to `target` without restarting the
    /// stream. Calls run one at a time and read `target` when their turn
    /// comes, so the last one leaves the newest rate in place.
    pub fn reconfigure(&self, target: &Mutex<(i32, (u32, u32))>) -> anyhow::Result<()> {
        let _turn = self.turn.lock().unwrap();
        let (fps, pixels) = *target.lock().unwrap();
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
        if let Ok(filter) = super::screenshot::filter(display_id, false, false)
            && let Err(error) = self.stream.update_content_filter(&filter)
        {
            log::debug!("capture filter of display {display_id} stays: {error}");
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

    fn solid_luma_buffer(luma: u8) -> CVPixelBuffer {
        use core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
        let buffer = CVPixelBuffer::new(
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            320,
            200,
            None,
        )
        .expect("create 420f CVPixelBuffer");
        assert_eq!(buffer.lock_base_address(0), 0);
        let height = buffer.get_height_of_plane(0);
        let row_bytes = buffer.get_bytes_per_row_of_plane(0);
        let base = unsafe { buffer.get_base_address_of_plane(0) } as *mut u8;
        assert!(!base.is_null());
        unsafe {
            std::ptr::write_bytes(base, luma, height * row_bytes);
        }
        buffer.unlock_base_address(0);
        buffer
    }

    #[test]
    fn count_changes_re_enabling_on_warm_stream_seeds_baseline_and_catches_first_change() {
        let latest = Latest::default();
        let skip = 0.2;

        // Cold stream: turning counting on before any frame arrives leaves
        // `counted()` as `None`, and the first frame seeds the baseline without
        // counting as a change.
        latest.count_changes(true);
        assert_eq!(latest.counted(), None);
        latest.record_frame(1, solid_luma_buffer(40), skip);
        assert_eq!(latest.counted(), Some(0));
        assert_eq!(latest.good_serial.load(Ordering::Relaxed), 1);

        // Settle ends: `"agent"` drops while the warm stream keeps `current`.
        latest.count_changes(false);
        assert_eq!(latest.counted(), None);

        // Next action turns counting back on over the still warm stream:
        // baseline is seeded immediately from `current` (luma=40), so
        // `counted()` is ready right away and the first changed frame counts.
        latest.count_changes(true);
        assert_eq!(latest.counted(), Some(0));
        latest.record_frame(1, solid_luma_buffer(200), skip);
        assert_eq!(latest.counted(), Some(1));

        // Repeat another settle off -> warm frame while off -> counting on:
        latest.count_changes(false);
        latest.record_frame(1, solid_luma_buffer(60), skip);
        assert_eq!(latest.counted(), None);
        latest.count_changes(true);
        assert_eq!(latest.counted(), Some(1));
        latest.record_frame(1, solid_luma_buffer(220), skip);
        assert_eq!(latest.counted(), Some(2));
    }

    #[test]
    fn verify_frame_bounds_refreshes_once_on_mismatch_before_declaring_misroute() {
        let initial = crate::model::Rect {
            x: 0.0,
            y: 0.0,
            width: 1280.0,
            height: 800.0,
        };
        let moved = crate::model::Rect {
            x: 2000.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        let other_screen = CGRect {
            origin: CGPoint { x: -1920.0, y: 0.0 },
            size: CGSize {
                width: 1920.0,
                height: 1080.0,
            },
        };
        let check_against = |actual: crate::model::Rect, want: crate::model::Rect| {
            if (actual.x - want.x).abs() < 1.0
                && (actual.y - want.y).abs() < 1.0
                && (actual.width - want.width).abs() < 1.0
                && (actual.height - want.height).abs() < 1.0
            {
                Ok(())
            } else {
                Err(CGRect {
                    origin: CGPoint {
                        x: actual.x,
                        y: actual.y,
                    },
                    size: CGSize {
                        width: actual.width,
                        height: actual.height,
                    },
                })
            }
        };

        let cached = Mutex::new((1u64, initial));
        let mut reads = 0usize;

        // Matching cached bounds: zero CoreGraphics reads.
        let ok = verify_frame_bounds(
            &cached,
            1,
            || {
                reads += 1;
                (1, initial)
            },
            |want| check_against(initial, want),
        );
        assert_eq!(ok, Ok(initial));
        assert_eq!(reads, 0);

        // Park/resize happened before `stamp` bumped: mismatch re-reads once,
        // updates cache, and accepts the frame with no false misroute.
        let ok = verify_frame_bounds(
            &cached,
            1,
            || {
                reads += 1;
                (1, moved)
            },
            |want| check_against(moved, want),
        );
        assert_eq!(ok, Ok(moved));
        assert_eq!(reads, 1);

        // Subsequent frame at `moved` hits the refreshed cache with zero reads.
        let ok = verify_frame_bounds(
            &cached,
            1,
            || {
                reads += 1;
                (1, moved)
            },
            |want| check_against(moved, want),
        );
        assert_eq!(ok, Ok(moved));
        assert_eq!(reads, 1);

        // Genuine misroute refreshes once and still reports Err(other_screen).
        let err = verify_frame_bounds(
            &cached,
            1,
            || {
                reads += 1;
                (1, moved)
            },
            |_| Err(other_screen),
        );
        assert_eq!(err, Err(other_screen));
        assert_eq!(reads, 2);
    }
}
