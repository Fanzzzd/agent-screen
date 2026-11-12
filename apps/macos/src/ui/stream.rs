//! Live frames of Agent Displays for any number of views. Each display gets
//! at most one capture stream, running at the highest frame rate any view
//! asked for, and only while some view wants it.

use crate::mac::capture::{Capture, Latest};
use crate::model::DisplayId;
use core_video::pixel_buffer::CVPixelBuffer;
use futures::StreamExt;
use futures::channel::mpsc;
use gpui::{App, AppContext, AsyncApp, Context, Entity, EventEmitter};
use std::collections::HashMap;
use std::sync::Arc;

struct Stream {
    capture: Option<Arc<Capture>>,
    /// A capture is starting on a background thread.
    starting: bool,
    /// Failed starts in a row.
    failures: u32,
    /// What `running` says, for reconfigurations on other threads.
    target: Arc<std::sync::Mutex<(i32, (u32, u32))>>,
    latest: Arc<Latest>,
    /// Frame rate and pixel size the capture runs at.
    running: (i32, (u32, u32)),
    /// Frame rate and pixel size wanted, by subscriber.
    wants: HashMap<&'static str, (i32, (u32, u32))>,
    /// Restarts in a row of a stream that showed another screen or that
    /// macOS stopped, since its last frame that showed the display.
    breaks: u32,
    /// Restarts of broken streams so far, for `debugStreams`.
    repairs: u32,
}

/// A display has a new frame.
pub struct NewFrame(pub DisplayId);

thread_local! {
    static GLOBAL: std::cell::RefCell<Option<gpui::WeakEntity<Streams>>> =
        const { std::cell::RefCell::new(None) };
}

/// The app's streams, for the control socket's settle wait.
pub fn global() -> Option<Entity<Streams>> {
    GLOBAL.with(|g| g.borrow().as_ref().and_then(|w| w.upgrade()))
}

pub struct Streams {
    streams: HashMap<DisplayId, Stream>,
    frames: HashMap<DisplayId, CVPixelBuffer>,
    wake: mpsc::UnboundedSender<DisplayId>,
    /// A capture, by display and serial, needs starting again.
    broken: mpsc::UnboundedSender<(DisplayId, u64)>,
}

impl EventEmitter<NewFrame> for Streams {}

impl Streams {
    pub fn new(cx: &mut App) -> Entity<Streams> {
        let (wake, mut woken) = mpsc::unbounded::<DisplayId>();
        let (broken, mut breaks) = mpsc::unbounded::<(DisplayId, u64)>();
        let entity = cx.new(|_| Streams {
            streams: HashMap::new(),
            frames: HashMap::new(),
            wake,
            broken,
        });
        let weak = entity.downgrade();
        GLOBAL.with(|g| *g.borrow_mut() = Some(weak.clone()));
        // Frames arrive on ScreenCaptureKit's queue; hand the newest to views.
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some(id) = woken.next().await {
                let Some(this) = weak.upgrade() else { break };
                this.update(cx, |streams, cx| {
                    let Some(stream) = streams.streams.get_mut(&id) else {
                        return;
                    };
                    if let Some(frame) = stream.latest.take() {
                        stream.breaks = 0;
                        streams.frames.insert(id, frame);
                        cx.emit(NewFrame(id));
                    }
                });
            }
        })
        .detach();
        // A stream that shows another screen keeps doing so: ScreenCaptureKit
        // can mix displays up after an unlock or another display change,
        // and the change notification may have come before its first wrong
        // frame. It starts again as soon as it says so.
        let weak = entity.downgrade();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Some((id, serial)) = breaks.next().await {
                let Some(this) = weak.upgrade() else { break };
                this.update(cx, |streams, cx| streams.repair(id, serial, cx));
            }
        })
        .detach();
        entity
    }

    /// Frames that showed a change on a display so far: idle frames do not
    /// count, so the number stops while the display is still.
    pub fn changes(&self, id: DisplayId) -> Option<u64> {
        self.streams
            .get(&id)
            .map(|s| s.latest.changes.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// The display's latest frame and change count, to watch off the main
    /// thread.
    pub fn latest(&self, id: DisplayId) -> Option<Arc<Latest>> {
        self.streams.get(&id).map(|s| s.latest.clone())
    }

    /// Leave out of every running capture the status items that appeared
    /// since it started. Lists shareable content off the main thread.
    pub fn refresh_filters(&self, cx: &mut Context<Self>) {
        for (&id, stream) in &self.streams {
            if let Some(capture) = stream.capture.clone() {
                cx.background_spawn(async move { capture.refresh_filter(id) })
                    .detach();
            }
        }
    }

    /// Whether the display's stream showed another display last.
    pub fn misrouted(&self, id: DisplayId) -> bool {
        self.streams
            .get(&id)
            .and_then(|s| s.capture.as_ref())
            .is_some_and(|c| c.misrouted())
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
            starting: false,
            failures: 0,
            target: Default::default(),
            latest: Arc::new(Latest::default()),
            running: (0, (0, 0)),
            wants: HashMap::new(),
            breaks: 0,
            repairs: 0,
        });
        if fps > 0 {
            stream.wants.insert(who, (fps, pixels));
        } else {
            stream.wants.remove(who);
        }
        if who == "agent" {
            stream.latest.count_changes(fps > 0);
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
        let before = stream.running;
        stream.running = wanted;
        *stream.target.lock().unwrap() = wanted;
        let fps = wanted.0;
        // Starting, stopping and reconfiguring a stream each block for
        // 100 ms or more, so they run off the main thread.
        if fps == 0 {
            if let Some(capture) = stream.capture.take() {
                cx.background_spawn(async move { drop(capture) }).detach();
            }
        } else if grows(before.1, wanted.1)
            && let Some(capture) = stream.capture.take()
        {
            // ScreenCaptureKit sends a frame only when the picture changes:
            // resized in place, a still display would keep showing the
            // small frame, scaled up and blurry. A new stream sends one
            // at once.
            cx.background_spawn(async move { drop(capture) }).detach();
            if !stream.starting {
                self.start(id, cx);
            }
        } else if let Some(capture) = stream.capture.clone() {
            let target = stream.target.clone();
            cx.background_spawn(async move {
                if let Err(error) = capture.reconfigure(&target) {
                    log::warn!("capture for display {id} kept its old rate: {error}");
                }
            })
            .detach();
        } else if !stream.starting {
            self.start(id, cx);
        }
    }

    /// Start capturing at the rate the stream should run at. When the
    /// start lands, the stream may want another rate, or none.
    fn start(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        let Some(stream) = self.streams.get_mut(&id) else {
            return;
        };
        stream.starting = true;
        let (fps, pixels) = stream.running;
        let latest = stream.latest.clone();
        let wake = self.wake.clone();
        let broken = self.broken.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let started = std::time::Instant::now();
                    let result = Capture::start(
                        id,
                        pixels,
                        fps,
                        latest,
                        move || {
                            let _ = wake.unbounded_send(id);
                        },
                        move |serial| {
                            let _ = broken.unbounded_send((id, serial));
                        },
                    );
                    log::debug!(
                        "capture of display {id} at {fps} fps, {pixels:?}: {:?}",
                        started.elapsed()
                    );
                    result
                })
                .await;
            let _ = this.update(cx, |streams, cx| {
                let stream = streams.streams.get_mut(&id);
                match (stream, result) {
                    (Some(stream), Ok(capture)) => {
                        stream.starting = false;
                        stream.failures = 0;
                        let capture = Arc::new(capture);
                        let running = stream.running;
                        if running.0 == 0 {
                            cx.background_spawn(async move { drop(capture) }).detach();
                            return;
                        }
                        // Wanted larger meanwhile: start again, as above.
                        if grows(pixels, running.1) {
                            cx.background_spawn(async move { drop(capture) }).detach();
                            streams.start(id, cx);
                            return;
                        }
                        stream.capture = Some(capture.clone());
                        // It broke before it was kept here, and said so to
                        // no one.
                        if capture.broken() {
                            streams.repair(id, capture.serial(), cx);
                            return;
                        }
                        if running != (fps, pixels) {
                            let target = stream.target.clone();
                            cx.background_spawn(async move {
                                let _ = capture.reconfigure(&target);
                            })
                            .detach();
                        }
                    }
                    (Some(stream), Err(error)) => {
                        if stream.failures == 0 {
                            log::warn!("capture for display {id} failed: {error}");
                        } else {
                            log::debug!("capture for display {id} failed again: {error}");
                        }
                        stream.starting = false;
                        // A display macOS mixes up with another isn't
                        // captured: its last frame, maybe a thumbnail
                        // scaled up, would pass for a live picture. The
                        // surfaces say why instead (`peek::waiting`).
                        if crate::mac::cg::unit_twin(id).is_some() {
                            streams.frames.remove(&id);
                            cx.notify();
                        }
                        // A just-created display can take a moment to show
                        // up; without Screen Recording it never does, so the
                        // wait doubles up to 30 s.
                        let wait = std::time::Duration::from_millis(500)
                            .saturating_mul(1 << stream.failures.min(6))
                            .min(std::time::Duration::from_secs(30));
                        stream.failures += 1;
                        cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(wait).await;
                            let _ = this.update(cx, |streams, cx| {
                                let retry = streams.streams.get(&id).is_some_and(|s| {
                                    s.running.0 > 0 && s.capture.is_none() && !s.starting
                                });
                                if retry {
                                    streams.start(id, cx);
                                }
                            });
                        })
                        .detach();
                    }
                    // The display went away meanwhile.
                    (None, Ok(capture)) => {
                        cx.background_spawn(async move { drop(capture) }).detach();
                    }
                    (None, Err(_)) => {}
                }
            });
        })
        .detach();
    }

    /// Start again every stream that shows another display or that macOS
    /// stopped: after a display change, a new stream may get the right
    /// picture.
    pub fn restart_misrouted(&mut self, cx: &mut Context<Self>) {
        let broken: Vec<(DisplayId, u64)> = self
            .streams
            .iter()
            .filter_map(|(&id, s)| {
                s.capture
                    .as_ref()
                    .filter(|c| c.broken())
                    .map(|c| (id, c.serial()))
            })
            .collect();
        for (id, serial) in broken {
            self.repair(id, serial, cx);
        }
    }

    /// Start a broken capture again, if it is still the display's. The
    /// first restart is at once; one that breaks again before showing the
    /// display waits, doubling up to 30 s. Its last frame stays up through
    /// the first restart, a moment old and of the right display; after
    /// that the surfaces say they wait for a picture.
    fn repair(&mut self, id: DisplayId, serial: u64, cx: &mut Context<Self>) {
        let Some(stream) = self.streams.get_mut(&id) else {
            return;
        };
        if stream.capture.as_ref().map(|c| c.serial()) != Some(serial) {
            return;
        }
        if let Some(capture) = stream.capture.take() {
            cx.background_spawn(async move { drop(capture) }).detach();
        }
        if stream.breaks > 0 && self.frames.remove(&id).is_some() {
            cx.notify();
        }
        let wait = match stream.breaks {
            0 => std::time::Duration::ZERO,
            n => std::time::Duration::from_millis(500)
                .saturating_mul(1 << (n - 1).min(6))
                .min(std::time::Duration::from_secs(30)),
        };
        stream.breaks += 1;
        stream.repairs += 1;
        log::info!(
            "display {id}: its stream starts again in {wait:?} ({} in a row)",
            stream.breaks
        );
        cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            let _ = this.update(cx, |streams, cx| {
                let restart = streams
                    .streams
                    .get(&id)
                    .is_some_and(|s| s.running.0 > 0 && s.capture.is_none() && !s.starting);
                if restart {
                    streams.start(id, cx);
                }
            });
        })
        .detach();
    }

    /// Each stream's state, for `debugStreams`: whether it runs, shows
    /// another screen, or needs starting again, its frames delivered, and
    /// how often it was started again.
    pub fn debug(&self) -> Vec<serde_json::Value> {
        let mut ids: Vec<_> = self.streams.keys().copied().collect();
        ids.sort();
        ids.iter()
            .map(|id| {
                let s = &self.streams[id];
                serde_json::json!({
                    "displayId": id,
                    "running": s.capture.is_some(),
                    "starting": s.starting,
                    "misrouted": s.capture.as_ref().is_some_and(|c| c.misrouted()),
                    "broken": s.capture.as_ref().is_some_and(|c| c.broken()),
                    "delivered": s.latest.delivered.load(std::sync::atomic::Ordering::Relaxed),
                    "hasFrame": self.frames.contains_key(id),
                    "breaks": s.breaks,
                    "repairs": s.repairs,
                    "fps": s.running.0,
                })
            })
            .collect()
    }

    pub fn drop_display(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        if let Some(capture) = self.streams.remove(&id).and_then(|s| s.capture) {
            cx.background_spawn(async move { drop(capture) }).detach();
        }
        self.frames.remove(&id);
    }
}

/// Whether `to` needs more pixels than `from` in either direction.
fn grows(from: (u32, u32), to: (u32, u32)) -> bool {
    to.0 > from.0 || to.1 > from.1
}
