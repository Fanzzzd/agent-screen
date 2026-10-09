//! Live frames of one display through ScreenCaptureKit, as 420f
//! CVPixelBuffers that GPUI's `surface` element draws without a copy.

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
use screencapturekit::cm::{CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

pub struct Capture {
    stream: SCStream,
    turn: Mutex<()>,
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
        let filter = super::screenshot::filter(display_id, true, false).map_err(anyhow::Error::msg)?;
        let mut stream = SCStream::new(&filter, &configuration(pixels, fps))?;
        stream.add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                // Idle frames repeat the last image: the display did not
                // change, so there is nothing to draw.
                if kind != SCStreamOutputType::Screen
                    || sample.frame_status() != Some(SCFrameStatus::Complete)
                {
                    return;
                }
                let raw = sample.image_buffer_ptr_borrowed();
                if raw.is_null() {
                    return;
                }
                // Retain: the sample buffer goes away after this callback.
                let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(raw as CVPixelBufferRef) };
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
            },
            SCStreamOutputType::Screen,
        )?;
        stream.start_capture()?;
        Ok(Self {
            stream,
            turn: Mutex::new(()),
        })
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
    SCStreamConfiguration::new()
        .with_width(pixels.0)
        .with_height(pixels.1)
        .with_pixel_format(PixelFormat::YCbCr_420f)
        .with_minimum_frame_interval(&CMTime::new(1, fps))
        .with_shows_cursor(false)
        .with_queue_depth(6)
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
}
