//! Live frames of one display through ScreenCaptureKit, as 420f
//! CVPixelBuffers that GPUI's `surface` element draws without a copy.

use core_foundation::base::TCFType;
use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
use screencapturekit::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The newest frame. Older ones are dropped: the Viewer only ever shows the
/// latest.
#[derive(Default)]
pub struct Latest {
    frame: Mutex<Option<Frame>>,
    pub delivered: AtomicU64,
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
        let content = SCShareableContent::get()?;
        let display = content
            .displays()
            .into_iter()
            .find(|d| d.display_id() == display_id)
            .ok_or_else(|| anyhow::anyhow!("display {display_id} is not shareable"))?;
        let filter = SCContentFilter::create()
            .with_display(&display)
            .with_excluding_windows(&[])
            .build()?;
        let config = SCStreamConfiguration::new()
            .with_width(pixels.0)
            .with_height(pixels.1)
            .with_pixel_format(PixelFormat::YCbCr_420f)
            .with_minimum_frame_interval(&CMTime::new(1, fps))
            .with_shows_cursor(false)
            .with_queue_depth(6);
        let mut stream = SCStream::new(&filter, &config)?;
        stream.add_output_handler(
            move |sample: CMSampleBuffer, kind: SCStreamOutputType| {
                if kind != SCStreamOutputType::Screen {
                    return;
                }
                let raw = sample.image_buffer_ptr_borrowed();
                if raw.is_null() {
                    return;
                }
                // Retain: the sample buffer goes away after this callback.
                let buffer = unsafe { CVPixelBuffer::wrap_under_get_rule(raw as CVPixelBufferRef) };
                *latest.frame.lock().unwrap() = Some(Frame(buffer));
                latest.delivered.fetch_add(1, Ordering::Relaxed);
                on_frame();
            },
            SCStreamOutputType::Screen,
        )?;
        stream.start_capture()?;
        Ok(Self { stream })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.stream.stop_capture();
    }
}
