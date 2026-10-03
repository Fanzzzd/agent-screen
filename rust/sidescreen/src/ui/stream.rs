//! Live frames of Agent Displays for any number of views. Each display gets
//! at most one capture stream, running at the highest frame rate any view
//! asked for, and only while some view wants it.

use crate::mac::capture::{Capture, Latest};
use crate::model::DisplayId;
use core_video::pixel_buffer::CVPixelBuffer;
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{App, AppContext, AsyncApp, Context, Entity};
use std::collections::HashMap;
use std::sync::Arc;

struct Stream {
    capture: Option<Capture>,
    latest: Arc<Latest>,
    /// Frame rate and pixel size the capture runs at.
    running: (i32, (u32, u32)),
    /// Frame rate and pixel size wanted, by subscriber.
    wants: HashMap<&'static str, (i32, (u32, u32))>,
}

pub struct Streams {
    streams: HashMap<DisplayId, Stream>,
    frames: HashMap<DisplayId, CVPixelBuffer>,
    wake: mpsc::UnboundedSender<DisplayId>,
}

impl Streams {
    pub fn new(cx: &mut App) -> Entity<Streams> {
        let (wake, mut woken) = mpsc::unbounded::<DisplayId>();
        let entity = cx.new(|_| Streams {
            streams: HashMap::new(),
            frames: HashMap::new(),
            wake,
        });
        let weak = entity.downgrade();
        // Frames arrive on ScreenCaptureKit's queue; hand the newest to views.
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(id) = woken.next().await {
                let Some(this) = weak.upgrade() else { break };
                this.update(cx, |streams, cx| {
                    if let Some(frame) = streams.streams.get(&id).and_then(|s| s.latest.take()) {
                        streams.frames.insert(id, frame);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        entity
    }

    /// The newest frame of a display.
    pub fn frame(&self, id: DisplayId) -> Option<CVPixelBuffer> {
        self.frames.get(&id).cloned()
    }

    /// `who` wants frames of `id` at `fps` and `pixels`; 0 fps stops
    /// wanting them. The capture runs at the highest rate and the largest
    /// size anyone wants: a thumbnail alone needs a fraction of the pixels.
    pub fn want(
        &mut self,
        id: DisplayId,
        who: &'static str,
        fps: i32,
        pixels: (u32, u32),
        cx: &mut Context<Self>,
    ) {
        let stream = self.streams.entry(id).or_insert_with(|| Stream {
            capture: None,
            latest: Arc::new(Latest::default()),
            running: (0, (0, 0)),
            wants: HashMap::new(),
        });
        if fps > 0 {
            stream.wants.insert(who, (fps, pixels));
        } else {
            stream.wants.remove(who);
        }
        let wanted = stream
            .wants
            .values()
            .fold((0, (0, 0)), |(f, (w, h)), &(fps, (pw, ph))| {
                (f.max(fps), (w.max(pw), h.max(ph)))
            });
        if wanted == stream.running {
            return;
        }
        // Stopping and starting a stream are synchronous; a restart at the
        // new rate takes a few milliseconds.
        stream.capture = None;
        stream.running = wanted;
        let (fps, pixels) = wanted;
        if fps == 0 {
            return;
        }
        let wake = self.wake.clone();
        match Capture::start(id, pixels, fps, stream.latest.clone(), move || {
            let _ = wake.unbounded_send(id);
        }) {
            Ok(capture) => stream.capture = Some(capture),
            Err(error) => {
                log::warn!("capture for display {id} failed: {error}");
                stream.running = (0, (0, 0));
                // A just-created display can take a moment to show up.
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(500))
                        .await;
                    let _ = this.update(cx, |streams, cx| {
                        let still = streams
                            .streams
                            .get(&id)
                            .is_some_and(|s| s.wants.get(who) == Some(&(fps, pixels)));
                        if still {
                            streams.want(id, who, fps, pixels, cx);
                        }
                    });
                })
                .detach();
            }
        }
    }

    pub fn drop_display(&mut self, id: DisplayId) {
        self.streams.remove(&id);
        self.frames.remove(&id);
    }
}
