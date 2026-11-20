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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Subscribers that read frames off the main thread (`Latest`) and do not need
/// GPUI `NewFrame` main-thread wakeups.
fn is_ui_subscriber(who: &str) -> bool {
    !matches!(who, "agent" | "agent-warm" | "web")
}

/// Minimum interval (in nanoseconds) between main-thread UI wakeups when
/// capture runs faster than any UI view asked for (for example, 30 fps for
/// `"agent"` while only `"warm"` at 1 fps wants UI frames).
/// Returns `0` when every frame should wake UI, or `u64::MAX` when no UI
/// subscriber is present.
fn ui_wake_interval_ns(running_fps: i32, ui_fps: i32) -> u64 {
    if ui_fps <= 0 {
        u64::MAX
    } else if ui_fps >= running_fps {
        0
    } else {
        1_000_000_000 / (ui_fps as u64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WakeAction {
    /// No UI subscriber is listening (`u64::MAX`).
    Suppress,
    /// Wake the UI immediately.
    WakeNow,
    /// A wake ran within `min_ns`; arm one trailing wake after `delay` with
    /// generation `generation` so the last frame of a burst is never left stale.
    ScheduleTrailing {
        delay: std::time::Duration,
        generation: u64,
    },
    /// A trailing wake is already armed for at least this soon.
    AlreadyScheduled,
}

#[derive(Default)]
struct WakeGateState {
    last_wake: Option<std::time::Instant>,
    trailing_due: Option<std::time::Instant>,
    trailing_gen: u64,
}

/// Rate-limits UI main-thread wakeups while guaranteeing a trailing wake after
/// any throttled burst so Peek, card thumbnails, and warm previews never stay
/// stuck on a stale frame.
#[derive(Default)]
struct UiWakeGate(std::sync::Mutex<WakeGateState>);

impl UiWakeGate {
    fn new() -> Self {
        Self::default()
    }

    fn on_frame(&self, min_ns: u64, now: std::time::Instant) -> WakeAction {
        if min_ns == u64::MAX {
            return WakeAction::Suppress;
        }
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if min_ns == 0 {
            state.trailing_gen = state.trailing_gen.wrapping_add(1);
            state.trailing_due = None;
            state.last_wake = Some(now);
            return WakeAction::WakeNow;
        }
        let min = std::time::Duration::from_nanos(min_ns);
        if let Some(prev) = state.last_wake {
            let elapsed = now.saturating_duration_since(prev);
            if elapsed < min {
                let due = prev + min;
                if state.trailing_due.is_none_or(|existing| due < existing) {
                    state.trailing_gen = state.trailing_gen.wrapping_add(1);
                    let generation = state.trailing_gen;
                    state.trailing_due = Some(due);
                    return WakeAction::ScheduleTrailing {
                        delay: min - elapsed,
                        generation,
                    };
                }
                return WakeAction::AlreadyScheduled;
            }
        }
        state.trailing_gen = state.trailing_gen.wrapping_add(1);
        state.trailing_due = None;
        state.last_wake = Some(now);
        WakeAction::WakeNow
    }

    fn finish_trailing(&self, generation: u64, min_ns: u64, now: std::time::Instant) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.trailing_gen != generation || state.trailing_due.is_none() {
            return false;
        }
        state.trailing_due = None;
        if min_ns == u64::MAX {
            return false;
        }
        state.last_wake = Some(now);
        true
    }
}

struct Stream {
    capture: Option<Arc<Capture>>,
    /// A capture is starting on a background thread.
    starting: bool,
    /// Failed starts in a row.
    failures: u32,
    /// Earliest instant a failed start's backoff expires.
    retry_at: Option<std::time::Instant>,
    /// What `running` says, for reconfigurations on other threads.
    target: Arc<std::sync::Mutex<(i32, (u32, u32))>>,
    latest: Arc<Latest>,
    /// Coalesces `wake` notifications so at most one pending wake per display
    /// sits in the channel.
    wake_pending: Arc<AtomicBool>,
    /// Minimum nanoseconds between main-thread UI wakeups (`0` = every frame,
    /// `u64::MAX` = no UI subscribers).
    ui_min_interval_ns: Arc<AtomicU64>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WantPlan {
    NoOp,
    Start,
    Stop,
    RestartFresh { start_now: bool },
    Reconfigure,
}

impl Stream {
    fn new() -> Self {
        Self {
            capture: None,
            starting: false,
            failures: 0,
            retry_at: None,
            target: Default::default(),
            latest: Arc::new(Latest::default()),
            wake_pending: Arc::new(AtomicBool::new(false)),
            ui_min_interval_ns: Arc::new(AtomicU64::new(u64::MAX)),
            running: (0, (0, 0)),
            wants: HashMap::new(),
            breaks: 0,
            repairs: 0,
        }
    }

    /// The stream should run, yet no capture runs or is starting.
    fn idle(&self) -> bool {
        self.running.0 > 0 && self.capture.is_none() && !self.starting
    }

    /// Update subscriber desires and return the platform capture transition
    /// needed now (M7).
    fn apply_want(
        &mut self,
        who: &'static str,
        fps: i32,
        pixels: (u32, u32),
        has_capture: bool,
        has_frame: bool,
        now: std::time::Instant,
    ) -> WantPlan {
        let newly_wanted = fps > 0 && !self.wants.contains_key(who);
        if fps > 0 {
            self.wants.insert(who, (fps, pixels));
        } else {
            self.wants.remove(who);
        }
        let should_count = self.wants.contains_key("agent");
        if should_count != self.latest.counting() {
            self.latest.count_changes(should_count);
        }
        let wanted = self
            .wants
            .values()
            .fold((0, (0, 0)), |(f, (w, h)), &(fps, (pw, ph))| {
                (f.max(fps), (w.max(pw), h.max(ph)))
            });
        let ui_fps = self
            .wants
            .iter()
            .filter(|(k, _)| is_ui_subscriber(k))
            .map(|(_, &(fps, _))| fps)
            .max()
            .unwrap_or(0);
        self.ui_min_interval_ns
            .store(ui_wake_interval_ns(wanted.0, ui_fps), Ordering::Relaxed);
        if wanted == self.running {
            let is_idle = self.running.0 > 0 && !has_capture && !self.starting;
            if is_idle && should_wake_idle_stream(newly_wanted, self.retry_at, now) {
                self.breaks = 0;
                return WantPlan::Start;
            }
            return WantPlan::NoOp;
        }
        let before = self.running;
        self.running = wanted;
        *self.target.lock().unwrap() = wanted;
        if wanted.0 == 0 {
            if has_capture {
                WantPlan::Stop
            } else {
                WantPlan::NoOp
            }
        } else if needs_fresh_stream(before, wanted, has_frame) && has_capture {
            self.breaks = 0;
            WantPlan::RestartFresh {
                start_now: !self.starting,
            }
        } else if has_capture {
            WantPlan::Reconfigure
        } else if !self.starting && should_wake_idle_stream(true, self.retry_at, now) {
            self.breaks = 0;
            WantPlan::Start
        } else {
            WantPlan::NoOp
        }
    }
    /// Update `breaks`/`repairs` for a broken capture of `serial` and return
    /// `(drop_cached_frame, wait)` for its restart. Resets `breaks` when
    /// `serial` delivered at least one verified frame, even if UI wakes were
    /// suppressed because no Viewer/Peek/card was open.
    fn plan_repair(&mut self, serial: u64) -> (bool, std::time::Duration) {
        if self.latest.good_serial.load(Ordering::Relaxed) >= serial {
            self.breaks = 0;
        }
        let drop_cached_frame = self.breaks > 0;
        let wait = match self.breaks {
            0 => std::time::Duration::ZERO,
            n => backoff(n - 1),
        };
        self.breaks += 1;
        self.repairs += 1;
        (drop_cached_frame, wait)
    }
}

/// The wait before the `n`th start in a row after the first: 500 ms,
/// doubling up to 30 s.
fn backoff(n: u32) -> std::time::Duration {
    std::time::Duration::from_millis(500)
        .saturating_mul(1 << n.min(6))
        .min(std::time::Duration::from_secs(30))
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
                    // Clear before `take()` so any frame arriving concurrently
                    // queues another wake rather than being missed.
                    stream.wake_pending.store(false, Ordering::SeqCst);
                    let from = stream
                        .latest
                        .from
                        .load(std::sync::atomic::Ordering::Relaxed);
                    if let Some(frame) = stream.latest.take() {
                        // Only the current capture's frame shows that it
                        // works: the one it replaced may still deliver.
                        if stream.capture.as_ref().is_some_and(|c| c.serial() == from) {
                            stream.breaks = 0;
                        }
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
        let has_frame = self.frames.contains_key(&id);
        let stream = self.streams.entry(id).or_insert_with(Stream::new);
        let has_capture = stream.capture.is_some();
        let plan = stream.apply_want(
            who,
            fps,
            pixels,
            has_capture,
            has_frame,
            std::time::Instant::now(),
        );
        // When a UI view subscribes while a background stream was already
        // producing frames at a throttled UI rate, hand it the newest frame
        // immediately so opening Viewer/Peek shows no stale frame.
        if fps > 0
            && is_ui_subscriber(who)
            && let Some(frame) = stream.latest.take()
        {
            self.frames.insert(id, frame);
            cx.emit(NewFrame(id));
        }
        // Starting, stopping and reconfiguring a stream each block for
        // 100 ms or more, so they run off the main thread.
        match plan {
            WantPlan::NoOp => {}
            WantPlan::Start => self.start(id, cx),
            WantPlan::Stop => {
                if let Some(capture) = self.streams.get_mut(&id).and_then(|s| s.capture.take()) {
                    cx.background_spawn(async move { drop(capture) }).detach();
                }
            }
            WantPlan::RestartFresh { start_now } => {
                // ScreenCaptureKit sends a frame only when the picture changes:
                // resized in place, or running with no cached frame yet, a still
                // display would keep showing the small or missing picture. A new
                // stream sends one at once.
                if let Some(capture) = self.streams.get_mut(&id).and_then(|s| s.capture.take()) {
                    cx.background_spawn(async move { drop(capture) }).detach();
                }
                if start_now {
                    self.start(id, cx);
                }
            }
            WantPlan::Reconfigure => {
                if let Some(stream) = self.streams.get(&id)
                    && let Some(capture) = stream.capture.clone()
                {
                    let target = stream.target.clone();
                    cx.background_spawn(async move {
                        if let Err(error) = capture.reconfigure(&target) {
                            log::warn!("capture for display {id} kept its old rate: {error}");
                        }
                    })
                    .detach();
                }
            }
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
        let wake_pending = stream.wake_pending.clone();
        let ui_min_interval_ns = stream.ui_min_interval_ns.clone();
        let broken = self.broken.clone();
        let bg = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let started = std::time::Instant::now();
                    let gate = Arc::new(UiWakeGate::new());
                    let result = Capture::start(
                        id,
                        pixels,
                        fps,
                        latest,
                        move || {
                            let min_ns = ui_min_interval_ns.load(Ordering::Relaxed);
                            match gate.on_frame(min_ns, std::time::Instant::now()) {
                                WakeAction::Suppress | WakeAction::AlreadyScheduled => {}
                                WakeAction::WakeNow => {
                                    if !wake_pending.swap(true, Ordering::AcqRel) {
                                        let _ = wake.unbounded_send(id);
                                    }
                                }
                                WakeAction::ScheduleTrailing { delay, generation } => {
                                    let bg_timer = bg.clone();
                                    let gate = gate.clone();
                                    let ui_min_interval_ns = ui_min_interval_ns.clone();
                                    let wake_pending = wake_pending.clone();
                                    let wake = wake.clone();
                                    bg.spawn(async move {
                                        bg_timer.timer(delay).await;
                                        let min_ns = ui_min_interval_ns.load(Ordering::Relaxed);
                                        if gate.finish_trailing(
                                            generation,
                                            min_ns,
                                            std::time::Instant::now(),
                                        ) && !wake_pending.swap(true, Ordering::AcqRel)
                                        {
                                            let _ = wake.unbounded_send(id);
                                        }
                                    })
                                    .detach();
                                }
                            }
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
                        stream.retry_at = None;
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
                        // A display macOS mixes up with another, or a stream
                        // that keeps failing to start, drops any old cached
                        // frame so the surfaces say they wait for a picture
                        // (`peek::waiting`) instead of showing a stale image.
                        if (stream.failures > 0 || crate::mac::cg::unit_twin(id).is_some())
                            && streams.frames.remove(&id).is_some()
                        {
                            cx.notify();
                        }
                        // A just-created display can take a moment to show
                        // up; without Screen Recording it never does, so the
                        // wait doubles up to 30 s.
                        let wait = backoff(stream.failures);
                        stream.failures += 1;
                        stream.retry_at = Some(std::time::Instant::now() + wait);
                        Self::start_after(id, wait, cx);
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
    /// stopped, and every stream waiting out a restart: after a display
    /// change, an unlock or a wake, a new stream may get the right picture
    /// at once.
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
        let waiting: Vec<DisplayId> = self
            .streams
            .iter()
            .filter(|(_, s)| s.idle())
            .map(|(&id, _)| id)
            .collect();
        for id in waiting {
            if let Some(stream) = self.streams.get_mut(&id) {
                stream.breaks = 0;
            }
            self.start(id, cx);
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
        let (drop_cached_frame, wait) = stream.plan_repair(serial);
        if drop_cached_frame && self.frames.remove(&id).is_some() {
            cx.notify();
        }
        log::info!(
            "display {id}: its stream starts again in {wait:?} ({} in a row)",
            stream.breaks
        );
        Self::start_after(id, wait, cx);
    }

    /// Start the display's stream after `wait`, if none runs or is
    /// starting then and it is still wanted.
    fn start_after(id: DisplayId, wait: std::time::Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            let _ = this.update(cx, |streams, cx| {
                if streams.streams.get(&id).is_some_and(Stream::idle) {
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
                let mut wants: Vec<&str> = s.wants.keys().copied().collect();
                wants.sort_unstable();
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
                    "wants": wants,
                })
            })
            .collect()
    }

    /// Subscribe `"web"` to `id` at `fps` and `pixels`, returning the shared
    /// `Latest` handle for off-thread JPEG encoding.
    pub fn subscribe_web(
        &mut self,
        id: DisplayId,
        fps: i32,
        pixels: (u32, u32),
        cx: &mut Context<Self>,
    ) -> Arc<Latest> {
        self.want(id, "web", fps, pixels, cx);
        self.streams
            .get(&id)
            .map(|s| s.latest.clone())
            .unwrap_or_default()
    }

    pub fn drop_display(&mut self, id: DisplayId, cx: &mut Context<Self>) {
        if let Some(stream) = self.streams.remove(&id) {
            stream.latest.clear();
            if let Some(capture) = stream.capture {
                cx.background_spawn(async move { drop(capture) }).detach();
            }
        }
        self.frames.remove(&id);
    }
}

/// Whether `to` needs more pixels than `from` in either direction.
fn grows(from: (u32, u32), to: (u32, u32)) -> bool {
    to.0 > from.0 || to.1 > from.1
}

/// Whether changing a running capture from `before` to `wanted` should start
/// a fresh `SCStream` rather than reconfiguring in place: only when a view
/// asks for more pixels, or when no frame is cached yet. Raising 1 fps to
/// Peek's 12 fps with a cached frame reconfigures in place without rebuilding
/// every display's stream on hover.
fn needs_fresh_stream(
    before: (i32, (u32, u32)),
    wanted: (i32, (u32, u32)),
    has_frame: bool,
) -> bool {
    wanted.0 > 0 && (grows(before.1, wanted.1) || !has_frame)
}

/// Whether `want` may start an idle stream immediately: only for a newly added
/// subscriber, and never before a failed start's exponential backoff (`retry_at`)
/// has elapsed.
fn should_wake_idle_stream(
    newly_wanted: bool,
    retry_at: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    newly_wanted && retry_at.is_none_or(|deadline| now >= deadline)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peek_reconfigures_in_place_when_frame_exists_and_rebuilds_only_when_needed() {
        let thumb = (800, 500);
        let full = (2560, 1600);
        assert!(!needs_fresh_stream((1, thumb), (12, thumb), true));
        assert!(needs_fresh_stream((1, thumb), (12, thumb), false));
        assert!(needs_fresh_stream((12, thumb), (60, full), true));
        assert!(!needs_fresh_stream((60, full), (12, thumb), true));
    }

    #[test]
    fn idle_stream_wakes_only_for_new_subscriber_past_failure_backoff() {
        let now = std::time::Instant::now();
        let future = now + std::time::Duration::from_secs(5);
        assert!(should_wake_idle_stream(true, None, now));
        // Repeated waitForChange calls from the same subscriber do not restart
        // an idle stream early.
        assert!(!should_wake_idle_stream(false, None, now));
        // A failing capture honors its backoff even for a new subscriber.
        assert!(!should_wake_idle_stream(true, Some(future), now));
        assert!(should_wake_idle_stream(true, Some(now), now));
    }

    #[test]
    fn stream_apply_want_wires_backoff_warm_reconfigure_and_pixel_growth() {
        let now = std::time::Instant::now();
        let future = now + std::time::Duration::from_secs(5);
        let thumb = (800, 500);
        let full = (2560, 1600);
        let mut s = Stream::new();

        // Warm 1 fps subscriber starts an idle stream immediately.
        assert_eq!(
            s.apply_want("warm", 1, thumb, false, false, now),
            WantPlan::Start
        );
        assert_eq!(s.running, (1, thumb));

        // Simulate failed start entering backoff: repeated wants and rate raises
        // update desired rate without bypassing `retry_at`.
        s.failures = 2;
        s.retry_at = Some(future);
        assert_eq!(
            s.apply_want("warm", 1, thumb, false, false, now),
            WantPlan::NoOp
        );
        assert_eq!(
            s.apply_want("agent", 1, thumb, false, false, now),
            WantPlan::NoOp
        );
        assert_eq!(
            s.apply_want("peek", 12, thumb, false, false, now),
            WantPlan::NoOp
        );
        assert_eq!(s.running, (12, thumb));

        // Once backoff elapses, a new subscriber wakes the idle stream.
        assert_eq!(
            s.apply_want("viewer_probe", 12, thumb, false, false, future),
            WantPlan::Start
        );

        // With a live capture and cached frame, dropping back to 1 fps and
        // hovering Peek (12 fps) reconfigures in place instead of rebuilding.
        s.retry_at = None;
        let _ = s.apply_want("peek", 0, (0, 0), true, true, future);
        let _ = s.apply_want("viewer_probe", 0, (0, 0), true, true, future);
        assert_eq!(s.running, (1, thumb));
        assert_eq!(
            s.apply_want("peek", 12, thumb, true, true, future),
            WantPlan::Reconfigure
        );
        // Opening Viewer at full pixels rebuilds the stream for higher resolution.
        assert_eq!(
            s.apply_want("viewer", 60, full, true, true, future),
            WantPlan::RestartFresh { start_now: true }
        );
        // Dropping every subscriber stops the running capture.
        let _ = s.apply_want("warm", 0, (0, 0), true, true, future);
        let _ = s.apply_want("agent", 0, (0, 0), true, true, future);
        let _ = s.apply_want("peek", 0, (0, 0), true, true, future);
        assert_eq!(
            s.apply_want("viewer", 0, (0, 0), true, true, future),
            WantPlan::Stop
        );
    }

    #[test]
    fn web_want_does_not_enable_agent_change_counting() {
        let now = std::time::Instant::now();
        let mut s = Stream::new();
        assert!(!s.latest.counting());

        let _ = s.apply_want("web", 8, (1280, 800), false, false, now);
        assert!(!s.latest.counting());

        let _ = s.apply_want("agent", 30, (416, 260), true, true, now);
        assert!(s.latest.counting());

        let _ = s.apply_want("agent", 0, (0, 0), true, true, now);
        assert!(!s.latest.counting());

        let _ = s.apply_want("web", 0, (0, 0), true, true, now);
        assert!(!s.latest.counting());
    }

    #[test]
    fn agent_warm_keeps_stream_warm_without_luma_counting_and_throttles_ui_wakes() {
        let now = std::time::Instant::now();
        let mut s = Stream::new();
        let thumb = (800, 500);
        let small = (416, 260);
        let full = (2560, 1600);

        // Non-UI subscriber only: no UI main-thread wakeups.
        let _ = s.apply_want("agent-warm", 30, small, false, false, now);
        assert!(!s.latest.counting());
        assert_eq!(s.ui_min_interval_ns.load(Ordering::Relaxed), u64::MAX);

        // Adding `"warm"` (1 fps UI) while `"agent-warm"` runs at 30 fps
        // throttles UI wakeups to 1 fps instead of waking the main thread 30/s.
        let _ = s.apply_want("warm", 1, thumb, true, true, now);
        assert_eq!(s.ui_min_interval_ns.load(Ordering::Relaxed), 1_000_000_000);

        // Active `"agent"` enables counting; dropping `"agent"` while keeping
        // `"agent-warm"` immediately turns counting off without stopping capture.
        let _ = s.apply_want("agent", 30, small, true, true, now);
        assert!(s.latest.counting());
        assert_eq!(
            s.apply_want("agent", 0, (0, 0), true, true, now),
            WantPlan::NoOp
        );
        assert!(!s.latest.counting());
        assert_eq!(s.running.0, 30);

        // Opening Viewer (60 fps UI) sets UI wake interval to 0 (every frame).
        let _ = s.apply_want("viewer", 60, full, true, true, now);
        assert_eq!(s.ui_min_interval_ns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn ui_wake_gate_schedules_single_trailing_wake_for_throttled_burst() {
        let gate = UiWakeGate::new();
        let t0 = std::time::Instant::now();
        let one_sec = 1_000_000_000u64;

        // Suppressed when no UI subscriber exists.
        assert_eq!(gate.on_frame(u64::MAX, t0), WakeAction::Suppress);

        // First frame wakes immediately.
        assert_eq!(gate.on_frame(one_sec, t0), WakeAction::WakeNow);

        // Burst within the 1 s window schedules one trailing wake and coalesces
        // further frames until it fires.
        let g1 = match gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(100)) {
            WakeAction::ScheduleTrailing { delay, generation } => {
                assert_eq!(delay, std::time::Duration::from_millis(900));
                generation
            }
            other => panic!("expected ScheduleTrailing, got {other:?}"),
        };
        assert_eq!(
            gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(250)),
            WakeAction::AlreadyScheduled
        );
        assert_eq!(
            gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(700)),
            WakeAction::AlreadyScheduled
        );

        // Trailing wake delivers the final frame of the burst and starts a new
        // throttle window from `t0 + 1000ms`.
        assert!(gate.finish_trailing(g1, one_sec, t0 + std::time::Duration::from_millis(1000)));
        assert!(!gate.finish_trailing(g1, one_sec, t0 + std::time::Duration::from_millis(1000)));

        // Another burst right after trailing wake schedules a new trailing wake.
        let g2 = match gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(1100)) {
            WakeAction::ScheduleTrailing { delay, generation } => {
                assert_eq!(delay, std::time::Duration::from_millis(900));
                generation
            }
            other => panic!("expected ScheduleTrailing, got {other:?}"),
        };

        // Raising UI rate (e.g. Peek at 10 fps = 100ms) while a 1s trailing
        // wake is pending either wakes immediately or supersedes with an earlier
        // trailing wake.
        let g3 = match gate.on_frame(200_000_000, t0 + std::time::Duration::from_millis(1150)) {
            WakeAction::ScheduleTrailing { delay, generation } => {
                assert_eq!(delay, std::time::Duration::from_millis(50));
                generation
            }
            other => panic!("expected earlier ScheduleTrailing, got {other:?}"),
        };
        assert!(!gate.finish_trailing(
            g2,
            200_000_000,
            t0 + std::time::Duration::from_millis(1200)
        ));
        assert!(gate.finish_trailing(g3, 200_000_000, t0 + std::time::Duration::from_millis(1200)));

        // Unthrottled (`min_ns == 0`) always wakes immediately and cancels any
        // prior trailing wake.
        let g4 = match gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(1300)) {
            WakeAction::ScheduleTrailing { generation, .. } => generation,
            other => panic!("expected ScheduleTrailing, got {other:?}"),
        };
        assert_eq!(
            gate.on_frame(0, t0 + std::time::Duration::from_millis(1350)),
            WakeAction::WakeNow
        );
        assert!(!gate.finish_trailing(g4, 0, t0 + std::time::Duration::from_millis(2200)));

        // Unsubscribing all UI views cancels a pending trailing wake.
        let g5 = match gate.on_frame(one_sec, t0 + std::time::Duration::from_millis(1400)) {
            WakeAction::ScheduleTrailing { generation, .. } => generation,
            other => panic!("expected ScheduleTrailing, got {other:?}"),
        };
        assert!(!gate.finish_trailing(g5, u64::MAX, t0 + std::time::Duration::from_millis(2350)));
    }

    #[test]
    fn plan_repair_resets_breaks_from_good_serial_without_ui_wakes() {
        let mut s = Stream::new();

        // Capture 1 delivered a verified frame (no UI view woke on it).
        s.latest.good_serial.fetch_max(1, Ordering::Relaxed);
        assert_eq!(s.plan_repair(1), (false, std::time::Duration::ZERO));
        assert_eq!(s.breaks, 1);

        // Capture 2 also delivered a verified frame without waking UI; when it
        // breaks later, `breaks` resets so restart is immediate (no backoff).
        s.latest.good_serial.fetch_max(2, Ordering::Relaxed);
        assert_eq!(s.plan_repair(2), (false, std::time::Duration::ZERO));
        assert_eq!(s.breaks, 1);

        // Captures 3 and 4 break before delivering any good frame: backoff
        // escalates normally.
        assert_eq!(
            s.plan_repair(3),
            (true, std::time::Duration::from_millis(500))
        );
        assert_eq!(s.breaks, 2);
        assert_eq!(
            s.plan_repair(4),
            (true, std::time::Duration::from_millis(1000))
        );
        assert_eq!(s.breaks, 3);

        // Capture 5 delivers a good frame: its later break resets backoff.
        s.latest.good_serial.fetch_max(5, Ordering::Relaxed);
        assert_eq!(s.plan_repair(5), (false, std::time::Duration::ZERO));
        assert_eq!(s.breaks, 1);
    }
}
