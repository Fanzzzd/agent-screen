//! Native menu guard runtime. The helper owns input, not the requesting
//! future. No tap starts at app startup. Native live validation is required
//! before this module may be wired into ordinary menu-opening tool calls.

mod journal;

use super::{ax, cg};
use crate::model::{Rect, WindowTarget};
use agent_screen_model::menu_guard::{CancelReason, Event, Origin, Phase, Queue, ReplayId};
use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopDefaultMode};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use serde_json::{Value, json};
use std::{
    cell::UnsafeCell,
    collections::HashMap,
    ffi::c_void,
    io::{BufRead, Write},
    path::PathBuf,
    ptr::NonNull,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

unsafe extern "C" {
    fn SSMenuFocusAvailable() -> bool;
    fn SSMenuPrepareHelper() -> bool;
    fn SSMenuKeyPid() -> i32;
    fn SSMenuStealFocus(id: &mut i32) -> i32;
    fn SSMenuReleaseFocusID(id: i32) -> i32;
    fn IsSecureEventInputEnabled() -> bool;
}

const NOTICE: u32 = 21;
const CLOSE_RESERVE: Duration = Duration::from_millis(150);
const RECOVERY_LIMIT: Duration = Duration::from_secs(2);
const ACK_MAGIC: u32 = 0x4752_4451;

#[derive(Clone)]
struct Config {
    target: WindowTarget,
    area: Rect,
    title: String,
    nonce: u32,
    test_sender: Option<WindowTarget>,
    pick_after: Duration,
    pause_after: Option<u64>,
}

impl Config {
    fn parse(v: &Value) -> Result<Self, String> {
        let i = |key: &str| v[key].as_i64().ok_or_else(|| format!("missing {key}"));
        let n = |key: &str| {
            v[key]
                .as_f64()
                .filter(|x| x.is_finite())
                .ok_or_else(|| format!("missing {key}"))
        };
        let target = WindowTarget {
            pid: i("pid")?.try_into().map_err(|_| "invalid pid")?,
            window_id: i("window")?.try_into().map_err(|_| "invalid window")?,
        };
        let area = Rect::new(n("x")?, n("y")?, n("width")?, n("height")?);
        if target.pid <= 0 || area.width <= 0.0 || area.height <= 0.0 {
            return Err("invalid native guard target".into());
        }
        let nonce: u32 = i("nonce")?.try_into().map_err(|_| "invalid guard nonce")?;
        let title = v["title"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or("missing item title")?
            .to_owned();
        let test_sender = match (v["testSender"].as_i64(), v["testWindow"].as_u64()) {
            (Some(pid), Some(window))
                if cfg!(debug_assertions)
                    && std::env::var("AGENTSCREEN_INSTANCE").as_deref() == Ok("e2e") =>
            {
                Some(WindowTarget {
                    pid: pid.try_into().map_err(|_| "invalid test sender")?,
                    window_id: window.try_into().map_err(|_| "invalid test window")?,
                })
            }
            (None, None) => None,
            _ => return Err("marked test input requires the isolated debug e2e instance".into()),
        };
        let pick_after = Duration::from_millis(v["pickAfterMs"].as_u64().unwrap_or(0));
        if pick_after > Duration::from_millis(200)
            || (pick_after != Duration::ZERO && test_sender.is_none())
        {
            return Err("invalid native test pick delay".into());
        }
        let pause_after = v["pauseAfterAcks"].as_u64();
        if pause_after.is_some() && test_sender.is_none() {
            return Err("replay pause is test-only".into());
        }
        Ok(Self {
            target,
            area,
            title,
            nonce,
            test_sender,
            pick_after,
            pause_after,
        })
    }
}

struct Shared {
    cancel: AtomicBool,
    fault: AtomicI32,
    own_theft: AtomicI32,
    done: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Self {
            cancel: AtomicBool::new(false),
            fault: AtomicI32::new(0),
            own_theft: AtomicI32::new(0),
            done: AtomicBool::new(false),
        }
    }
    fn fail(&self, code: i32) {
        let _ = self
            .fault
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
        self.cancel.store(true, Ordering::Release);
    }
}

// Each CGEvent stays on the capture run-loop thread. Only serialized
// bytes cross to the custody writer, so no Send/Sync claim is needed.
#[derive(Clone)]
struct Saved {
    id: u64,
    event: CFRetained<CGEvent>,
    kind: Event,
    captured: Instant,
    simulated: bool,
}

impl Saved {
    fn event(&self) -> &CGEvent {
        &self.event
    }
}

enum Record {
    Held {
        id: u64,
        bytes: Vec<u8>,
    },
    Sent(u64),
    Ack(u64),
    Fault(String),
    Finish {
        retained: usize,
        snapshot: Vec<journal::RetainedRecord>,
        closed: bool,
        focus: bool,
        reply: mpsc::Sender<Result<(), String>>,
    },
}

fn logger(
    root: PathBuf,
    nonce: u64,
    target: i32,
    user: i32,
    shared: Arc<Shared>,
) -> Result<mpsc::Sender<Record>, String> {
    let journal = journal::Journal::create(&root, nonce, target, user)?;
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("agent-screen-input-custody".into())
        .spawn(move || {
            let mut journal = Some(journal);
            while let Ok(record) = rx.recv() {
                let result = match record {
                    Record::Held { id, bytes } => journal.as_mut().unwrap().held(id, &bytes),
                    Record::Sent(id) => journal.as_mut().unwrap().dispatched(id),
                    Record::Ack(id) => journal.as_mut().unwrap().acknowledged(id),
                    Record::Fault(reason) => journal.as_mut().unwrap().fault(&reason),
                    Record::Finish {
                        retained,
                        snapshot,
                        closed,
                        focus,
                        reply,
                    } => {
                        let current = journal.take().unwrap();
                        let result = if retained == 0 {
                            current.finish(0, closed, focus)
                        } else {
                            current.checkpoint_owned(&root, nonce, target, user, &snapshot)
                        };
                        let _ = reply.send(result);
                        return;
                    }
                };
                if result.is_err() {
                    shared.fail(7);
                }
            }
            // Dropping the writer intentionally leaves an incomplete journal.
        })
        .map_err(|e| e.to_string())?;
    Ok(tx)
}

struct Capture {
    queue: Queue<Saved>,
    shared: Arc<Shared>,
    records: mpsc::Sender<Record>,
    marker_prefix: u32,
    next: u64,
    logged: u64,
    pending: HashMap<u64, (ReplayId, Instant)>,
    held: u64,
    replayed: u64,
    max_delay: Duration,
    taps_disabled: u64,
    blocked_test_events: u64,
    notices_dropped: u64,
    quarantine: Vec<Saved>,
    pause_after: Option<u64>,
    pause_reported: bool,
    target: i32,
    active: bool,
    source: CFRetained<CGEventSource>,
    user: i32,
    user_window: WindowTarget,
    display_id: u32,
    test_sender: Option<WindowTarget>,
    test_nonce: u32,
    area: Rect,
}

fn event_kind(raw: u32) -> Event {
    match raw {
        10 | 11 => Event::Key,
        12 => Event::Modifiers,
        1 | 2 | 3 | 4 | 25 | 26 => Event::Button,
        22 => Event::Scroll,
        5 | 6 | 7 | 27 => Event::PointerMove,
        _ => Event::Other,
    }
}

fn own_for_custody(
    event: &CGEvent,
    copy: impl FnOnce() -> Option<CFRetained<CGEvent>>,
) -> CFRetained<CGEvent> {
    copy().unwrap_or_else(|| unsafe { CFRetained::retain(NonNull::from(event)) })
}

unsafe extern "C-unwind" fn capture_callback(
    _: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &mut *info.cast::<Capture>() };
    let before = state.queue.retained();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.capture(kind, unsafe { event.as_ref() })
    }));
    match result {
        Ok(true) => std::ptr::null_mut(),
        Ok(false) => event.as_ptr(),
        Err(_) => {
            state.shared.fail(4);
            // If custody was committed, never return the original too.
            if state.queue.retained() > before {
                std::ptr::null_mut()
            } else {
                event.as_ptr()
            }
        }
    }
}

unsafe extern "C-unwind" fn acknowledgement_callback(
    _: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &mut *info.cast::<Capture>() };
    if kind.0 >= u32::MAX - 1 {
        state.taps_disabled += 1;
        state.shared.fail(2);
    } else {
        let e = unsafe { event.as_ref() };
        if let Some(sender) = state.test_sender {
            let source =
                CGEvent::integer_value_field(Some(e), CGEventField::EventSourceUnixProcessID);
            let tag =
                CGEvent::integer_value_field(Some(e), CGEventField::EventSourceUserData) as u64;
            let raw_test =
                source == i64::from(sender.pid) && (tag >> 32) as u32 == state.test_nonce;
            let replay_test = source == i64::from(std::process::id())
                && (tag >> 32) as u32 == state.marker_prefix
                && tag & 1 != 0;
            if raw_test || replay_test {
                let destination =
                    CGEvent::integer_value_field(Some(e), CGEventField::EventTargetUnixProcessID);
                let allowed = destination == i64::from(sender.pid)
                    || (!replay_test
                        && event_kind(kind.0) == Event::PointerMove
                        && destination == i64::from(state.target));
                if !allowed {
                    if raw_test {
                        let copy = own_for_custody(e, || CGEvent::new_copy(Some(e)));
                        state.quarantine.push(Saved {
                            id: state.next,
                            event: copy,
                            kind: event_kind(kind.0),
                            captured: Instant::now(),
                            simulated: true,
                        });
                        state.next += 1;
                    }
                    state.blocked_test_events += 1;
                    state.shared.fail(12);
                    state.queue.fail(CancelReason::TapUnavailable);
                    return std::ptr::null_mut();
                }
            }
        }
        state.acknowledge(e, true);
    }
    event.as_ptr()
}

unsafe extern "C-unwind" fn notice_callback(
    _: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    info: *mut c_void,
) -> *mut CGEvent {
    let state = unsafe { &mut *info.cast::<Capture>() };
    if kind.0 >= u32::MAX - 1 {
        state.taps_disabled += 1;
        state.shared.fail(2);
        return event.as_ptr();
    }
    let e = unsafe { event.as_ref() };
    let sub = CGEvent::integer_value_field(Some(e), CGEventField(64));
    let thief = CGEvent::integer_value_field(Some(e), CGEventField(73));
    if state.active
        && kind.0 == NOTICE
        && sub == 0x4000
        && (thief == i64::from(state.target) || thief == i64::from(std::process::id()))
    {
        state.notices_dropped += 1;
        std::ptr::null_mut()
    } else {
        event.as_ptr()
    }
}

impl Capture {
    fn marker(&self, id: u64, simulated: bool) -> i64 {
        ((u64::from(self.marker_prefix) << 32)
            | ((id << 1) & u64::from(u32::MAX))
            | u64::from(simulated)) as i64
    }
    fn acknowledge(&mut self, event: &CGEvent, final_stage: bool) -> bool {
        if CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUnixProcessID)
            != i64::from(std::process::id())
        {
            return false;
        }
        let marker =
            CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) as u64;
        if (marker >> 32) as u32 != self.marker_prefix {
            return false;
        }
        if !final_stage {
            return true;
        }
        let id = (marker & u64::from(u32::MAX)) >> 1;
        if let Some((token, captured)) = self.pending.get(&id).copied()
            && self.queue.acknowledge(token)
        {
            self.pending.remove(&id);
            self.max_delay = self.max_delay.max(captured.elapsed());
            self.replayed += 1;
            let _ = self.records.send(Record::Ack(id));
        }
        true
    }
    fn capture(&mut self, raw: CGEventType, event: &CGEvent) -> bool {
        if raw.0 >= u32::MAX - 1 {
            self.taps_disabled += 1;
            self.shared.fail(2);
            return false;
        }
        if self.acknowledge(event, false) {
            return false;
        }
        let kind = event_kind(raw.0);
        let pid = CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUnixProcessID);
        let tag =
            CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) as u64;
        let simulated = self
            .test_sender
            .is_some_and(|sender| pid == i64::from(sender.pid))
            && (tag >> 32) as u32 == self.test_nonce;
        let origin = if pid == 0 || simulated {
            Origin::Hardware
        } else {
            Origin::Posted
        };
        if self.active
            && pid == 0
            && let Some(sender) = self.test_sender
        {
            self.shared.fail(10);
            self.queue.fail(CancelReason::OwnerGone);
            self.queue.fail(CancelReason::OwnerGone);
            let _ = sender;
        }
        if !self.active || !self.queue.should_hold(kind, origin) {
            return false;
        }
        let copy = own_for_custody(event, || CGEvent::new_copy(Some(event)));
        let saved = Saved {
            id: self.next,
            event: copy,
            kind,
            captured: Instant::now(),
            simulated,
        };
        self.next += 1;
        if self.queue.offer(kind, origin, saved).is_some() {
            return false;
        }
        self.held += 1;
        // Serialization and disk IO both stay outside the HID callback.
        true
    }
    fn persist_new(&mut self) {
        for saved in self
            .queue
            .retained_events()
            .filter(|saved| saved.id >= self.logged)
        {
            let Some(data) = CGEvent::new_data(None, Some(saved.event())) else {
                self.shared.fail(7);
                return;
            };
            let bytes =
                unsafe { std::slice::from_raw_parts(data.byte_ptr(), data.length() as usize) }
                    .to_vec();
            if self
                .records
                .send(Record::Held {
                    id: saved.id,
                    bytes,
                })
                .is_err()
            {
                self.shared.fail(7);
                return;
            }
        }
        self.logged = self.next;
    }

    fn replay(&mut self) {
        if self.shared.fault.load(Ordering::Acquire) == 0
            && self.pause_after.is_some_and(|limit| self.replayed >= limit)
        {
            if !self.pause_reported {
                output(
                    json!({"paused":true,"held":self.held,"replayed":self.replayed,"retained":self.queue.retained()}),
                );
                self.pause_reported = true;
            }
            return;
        }
        if self.queue.replay_front().is_some() {
            if self.test_sender.is_some()
                && matches!(self.shared.fault.load(Ordering::Acquire), 10 | 12)
            {
                return;
            }

            if !cg::is_online(self.display_id) || cg::display_bounds(self.display_id) != self.area {
                self.shared.fail(8);
                self.queue.fail(CancelReason::TapUnavailable);
                return;
            }
            let focus = unsafe { SSMenuKeyPid() };
            if focus != self.user {
                self.shared.fail(8);
                self.queue.fail(CancelReason::TapUnavailable);
                return;
            }
            if let Some(sender) = self.test_sender {
                let identity_matches = cg::on_screen_windows().iter().any(|window| {
                    window.target == sender && self.area.contains(window.bounds.mid())
                });
                if focus != sender.pid || !identity_matches {
                    self.shared.fail(8);
                    self.queue.fail(CancelReason::TapUnavailable);
                    return;
                }
            }
            let Some((token, saved)) = self.queue.next_replay() else {
                return;
            };
            let Some(copy) = CGEvent::new_copy(Some(saved.event())) else {
                self.shared.fail(7);
                self.queue.fail(CancelReason::TapUnavailable);
                return;
            };
            if matches!(saved.kind, Event::Button | Event::Scroll) {
                let point = CGEvent::location(Some(saved.event()));
                let same_window = cg::on_screen_windows()
                    .iter()
                    .find(|window| window.layer == 0 && window.bounds.contains((point.x, point.y)))
                    .is_some_and(|window| window.target == self.user_window);
                if !same_window {
                    self.shared.fail(8);
                    self.queue.fail(CancelReason::TapUnavailable);
                    return;
                }
                super::input::stamp_routing(&copy, (point.x, point.y), self.user_window);
            }
            CGEvent::set_source(Some(&copy), Some(&self.source));
            CGEvent::set_integer_value_field(
                Some(&copy),
                CGEventField::EventSourceUserData,
                self.marker(saved.id, saved.simulated),
            );
            self.pending.insert(saved.id, (token, saved.captured));
            let _ = self.records.send(Record::Sent(saved.id));
            let location = if matches!(saved.kind, Event::Button | Event::Scroll) {
                CGEventTapLocation::AnnotatedSessionEventTap
            } else {
                CGEventTapLocation::HIDEventTap
            };
            // Annotated mouse replay is deliberately not enabled in tools
            // until the one-session native probe proves cursor and routing.
            CGEvent::post(location, Some(&copy));
        }
    }
}

enum MenuEvent {
    FocusReleased,
    Closed,
    Picked(String),
    Failed(String),
}

fn visible_menus(config: &Config) -> Vec<cg::WindowInfo> {
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| w.target.pid == config.target.pid && w.layer == 101)
        .collect()
}

fn menu_worker(config: Config, user: i32, shared: Arc<Shared>, events: mpsc::Sender<MenuEvent>) {
    let mut picked = false;
    loop {
        let menus = visible_menus(&config);
        if shared.cancel.load(Ordering::Acquire) {
            return;
        } else if !menus.is_empty() && !picked {
            if menus
                .iter()
                .any(|window| !config.area.intersects(&window.bounds))
            {
                shared.fail(11);
                continue;
            }
            if unsafe { SSMenuKeyPid() } != config.target.pid {
                shared.fail(8);
                continue;
            }
            let mut own = 0;
            if unsafe { SSMenuStealFocus(&mut own) } != 0 {
                shared.fail(8);
                continue;
            }
            shared.own_theft.store(own, Ordering::Release);
            if unsafe { SSMenuReleaseFocusID(own) } != 0 {
                shared.fail(8);
                continue;
            }
            shared.own_theft.store(0, Ordering::Release);
            if own <= 1
                || unsafe { SSMenuKeyPid() } != config.target.pid
                || unsafe { SSMenuReleaseFocusID(own - 1) } != 0
                || unsafe { SSMenuKeyPid() } != user
            {
                shared.fail(8);
                continue;
            }
            let _ = events.send(MenuEvent::FocusReleased);
            let until = Instant::now() + config.pick_after;
            while Instant::now() < until && !shared.cancel.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(1));
            }
            let result = menus
                .first()
                .and_then(|w| ax::menu_for_window(config.target.pid, w.bounds))
                .ok_or("native menu has no readable AX menu".to_owned())
                .and_then(|menu| {
                    let mut found = Vec::new();
                    for item in ax::elements_attribute(&menu, "AXChildren") {
                        unsafe {
                            item.set_messaging_timeout(0.025);
                        }
                        if shared.cancel.load(Ordering::Acquire) {
                            return Err("native menu call was cancelled".into());
                        }
                        if ax::string_attribute(&item, "AXTitle").as_deref() == Some(&config.title)
                        {
                            found.push(item);
                        }
                    }
                    if found.len() != 1 {
                        return Err("native menu item is absent or ambiguous".into());
                    }
                    if shared.cancel.load(Ordering::Acquire) {
                        return Err("native menu call was cancelled".into());
                    }
                    if !ax::try_action(&found[0], "AXPress") {
                        return Err("native menu refused the item press".into());
                    }
                    Ok(config.title.clone())
                });
            match result {
                Ok(title) => {
                    let _ = events.send(MenuEvent::Picked(title));
                }
                Err(error) => {
                    let _ = events.send(MenuEvent::Failed(error));
                    shared.fail(9);
                }
            }
            picked = true;
            shared.cancel.store(true, Ordering::Release);
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn cancellation_worker(config: Config, shared: Arc<Shared>, events: mpsc::Sender<MenuEvent>) {
    let mut empty_since = None;
    while !shared.done.load(Ordering::Acquire) {
        if shared.cancel.load(Ordering::Acquire) {
            let menus = visible_menus(&config);
            for window in &menus {
                if let Some(menu) = ax::menu_for_window(config.target.pid, window.bounds) {
                    ax::try_action(&menu, "AXCancel");
                }
            }
            if menus.is_empty() && unsafe { SSMenuKeyPid() } != config.target.pid {
                let since = empty_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(25) {
                    let _ = events.send(MenuEvent::Closed);
                    return;
                }
            } else {
                empty_since = None;
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

struct Taps(Vec<CFRetained<CFMachPort>>);
impl Drop for Taps {
    fn drop(&mut self) {
        for port in &self.0 {
            port.invalidate();
        }
    }
}

fn attach(port: &CFMachPort) -> Result<(), String> {
    let source = CFMachPort::new_run_loop_source(None, Some(port), 0)
        .ok_or("no native guard run-loop source")?;
    CFRunLoop::current()
        .ok_or("no native guard run loop")?
        .add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
    CGEvent::tap_enable(port, true);
    Ok(())
}

fn output(value: Value) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

fn recovery_snapshot(state: &Capture) -> Result<Vec<journal::RetainedRecord>, String> {
    let mut result = Vec::new();
    for saved in state.queue.retained_events().chain(state.quarantine.iter()) {
        let data = CGEvent::new_data(None, Some(saved.event()))
            .ok_or("could not serialize retained recovery custody")?;
        let bytes =
            unsafe { std::slice::from_raw_parts(data.byte_ptr(), data.length() as usize) }.to_vec();
        result.push(journal::RetainedRecord {
            id: saved.id,
            bytes,
            dispatched: state.pending.contains_key(&saved.id),
        });
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckpointWait {
    Acquire,
    Publish,
}

fn persist_with_lease<L>(
    mut acquire: impl FnMut() -> Result<L, String>,
    mut publish: impl FnMut(&mut L) -> Result<(), String>,
    mut wait: impl FnMut(CheckpointWait),
) {
    let mut lease = loop {
        match acquire() {
            Ok(lease) => break lease,
            Err(_) => wait(CheckpointWait::Acquire),
        }
    };
    loop {
        match publish(&mut lease) {
            Ok(()) => return,
            Err(_) => wait(CheckpointWait::Publish),
        }
    }
}

fn finish_custody(
    state: &mut Capture,
    ports: &Taps,
    root: &std::path::Path,
    nonce: u32,
    closed: bool,
    focus: bool,
) -> Result<(), String> {
    state.active = false;
    for port in &ports.0 {
        port.invalidate();
    }
    state.persist_new();
    let retained = state.queue.retained() + state.quarantine.len();
    let snapshot = loop {
        match recovery_snapshot(state) {
            Ok(snapshot) => break snapshot,
            Err(_) => {
                output(
                    json!({"recoveryPending":true,"retained":retained,"custody":"helper-memory"}),
                );
                thread::sleep(Duration::from_secs(1));
            }
        }
    };
    let (tx, rx) = mpsc::channel();
    let _ = state.records.send(Record::Finish {
        retained,
        snapshot,
        closed,
        focus,
        reply: tx,
    });
    let writer = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| "input custody writer did not finish".to_owned())
        .and_then(|r| r);
    if retained != 0 {
        if writer.is_err() {
            let snapshot = loop {
                match recovery_snapshot(state) {
                    Ok(snapshot) => break snapshot,
                    Err(_) => {
                        output(
                            json!({"recoveryPending":true,"retained":retained,"custody":"helper-memory"}),
                        );
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            };
            // Acquisition errors happen before authority is reserved. Retry
            // the same frozen snapshot until one lease is acquired. Never
            // refresh that lease or its generation after publication starts.
            persist_with_lease(
                || {
                    journal::Journal::fallback(
                        root,
                        u64::from(nonce),
                        state.target,
                        state.user,
                        snapshot.clone(),
                    )
                },
                |lease| lease.retry(),
                |stage| {
                    output(
                        json!({"recoveryPending":true,"retained":retained,"custody":"helper-memory","acquiringAuthority":stage==CheckpointWait::Acquire}),
                    );
                    thread::sleep(Duration::from_secs(1));
                },
            );
        }
        return Err(
            "retained input is sealed in private recovery custody; automatic replay is forbidden"
                .into(),
        );
    }
    writer?;
    if !closed || !focus {
        return Err("native guard cleanup was not verified".into());
    }
    Ok(())
}

fn worker_started<T>(
    result: std::io::Result<T>,
    abort: impl FnOnce() -> Result<(), String>,
) -> Result<T, String> {
    match result {
        Ok(worker) => Ok(worker),
        Err(error) => {
            let cleanup = abort();
            Err(match cleanup {
                Ok(()) => format!("native guard worker could not start: {error}"),
                Err(custody) => format!("native guard worker could not start: {error}; {custody}"),
            })
        }
    }
}

/// Inspect counts only. Ambiguous delivery is never retried automatically.
pub fn recovery_summary() -> Result<(), String> {
    let root = agent_screen_mcp::endpoint::socket_path().with_extension("input-recovery");
    if !root.exists() {
        output(json!({"recovery":[]}));
        return Ok(());
    }
    let records = journal::summaries(&root)?;
    output(json!({"recovery":records}));
    Ok(())
}

/// Internal helper entry. Starting it performs no opening click. The parent
/// must wait for Ready and send Open before it posts an opening action.
pub fn helper_main() -> Result<(), String> {
    if !cfg!(debug_assertions)
        || std::env::var("AGENTSCREEN_INSTANCE").as_deref() != Ok("e2e")
        || std::env::var("AGENTSCREEN_GUARD_LIVE").as_deref() != Ok("1")
    {
        return Err("native guard live validation is not enabled; no tap was installed".into());
    }

    if unsafe { libc::isatty(libc::STDIN_FILENO) } != 0 {
        return Err("the native menu helper requires its private parent pipe".into());
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let first = lines
        .next()
        .ok_or("missing native guard request")?
        .map_err(|e| e.to_string())?;
    let request: Value = serde_json::from_str(&first).map_err(|e| e.to_string())?;
    let config = Config::parse(&request)?;
    if config.test_sender.is_none() {
        return Err(
            "native validation requires an owned stand-in; production interception is not enabled"
                .into(),
        );
    }

    drop(lines);
    if !unsafe { SSMenuFocusAvailable() } {
        return Err("native menu focus primitives are unavailable; no menu was opened".into());
    }
    if unsafe { IsSecureEventInputEnabled() } {
        return Err("secure input is on; no menu was opened".into());
    }
    if !unsafe { SSMenuPrepareHelper() } {
        return Err("native helper could not register a non-activating connection".into());
    }
    let user = unsafe { SSMenuKeyPid() };
    if user <= 0 || user == config.target.pid {
        return Err(
            "native menu guard cannot isolate the target's key focus; no menu was opened".into(),
        );
    }
    let display: u32 = request["displayId"]
        .as_u64()
        .ok_or("missing Agent Display id")?
        .try_into()
        .map_err(|_| "invalid Agent Display id")?;
    if !cg::is_agent_screen(display)
        || cg::is_closet(display)
        || !cg::is_online(display)
        || cg::display_bounds(display) != config.area
    {
        return Err("native guard area is not the requested online Agent Display".into());
    }
    let preflight = cg::on_screen_windows();
    if preflight.iter().any(|window| {
        window.target.pid == config.target.pid
            && (window.layer == 101
                || (window.layer == 0 && !config.area.contains(window.bounds.mid())))
    }) {
        return Err(
            "native guard target already has a menu or a window outside its Agent Display".into(),
        );
    }
    let window = cg::on_screen_windows()
        .into_iter()
        .find(|w| w.target == config.target && w.bounds.intersects(&config.area));
    if window.is_none() {
        return Err("native guard target is not on its Agent Display".into());
    }
    if let Some(sender) = config.test_sender
        && (user != sender.pid
            || !cg::on_screen_windows()
                .iter()
                .any(|window| window.target == sender && config.area.intersects(&window.bounds)))
    {
        return Err(
            "the owned stand-in lacks key focus on this Agent Display; no tap was installed".into(),
        );
    }
    let user_window = match config.test_sender {
        Some(sender) => sender,
        None => WindowTarget {
            pid: user,
            window_id: ax::focused_window_id(user)
                .ok_or("cannot identify the original user's key window; no menu was opened")?,
        },
    };
    let root = agent_screen_mcp::endpoint::socket_path().with_extension("input-recovery");
    let shared = Arc::new(Shared::new());
    let records = logger(
        root.clone(),
        u64::from(config.nonce),
        config.target.pid,
        user,
        shared.clone(),
    )?;
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .ok_or("native guard has no replay source")?;
    // Only the run-loop thread touches Capture. UnsafeCell permits the
    // registered callbacks to mutate it while no Rust reference is live.
    let state_cell = Box::new(UnsafeCell::new(Capture {
        queue: Queue::default(),
        shared: shared.clone(),
        records,
        marker_prefix: config.nonce ^ ACK_MAGIC,
        next: 0,
        logged: 0,
        pending: HashMap::new(),
        held: 0,
        replayed: 0,
        max_delay: Duration::ZERO,
        taps_disabled: 0,
        blocked_test_events: 0,
        notices_dropped: 0,
        quarantine: Vec::new(),
        pause_after: config.pause_after,
        pause_reported: false,
        target: config.target.pid,
        active: false,
        source,
        user,
        user_window,
        display_id: display,
        test_sender: config.test_sender,
        test_nonce: config.nonce,
        area: config.area,
    }));
    let data = state_cell.get().cast();
    let mask = [1, 2, 3, 4, 5, 6, 7, 10, 11, 12, 22, 25, 26, 27]
        .into_iter()
        .fold(0u64, |m, t| m | (1 << t));
    let ports = unsafe {
        [
            CGEvent::tap_create(
                CGEventTapLocation::HIDEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(capture_callback),
                data,
            ),
            CGEvent::tap_create(
                CGEventTapLocation::HIDEventTap,
                CGEventTapPlacement::TailAppendEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(capture_callback),
                data,
            ),
            CGEvent::tap_create(
                CGEventTapLocation::AnnotatedSessionEventTap,
                CGEventTapPlacement::TailAppendEventTap,
                CGEventTapOptions::Default,
                mask,
                Some(acknowledgement_callback),
                data,
            ),
            CGEvent::tap_create_for_pid(
                user,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                1 << NOTICE,
                Some(notice_callback),
                data,
            ),
            CGEvent::tap_create_for_pid(
                config.target.pid,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                1 << NOTICE,
                Some(notice_callback),
                data,
            ),
        ]
    };
    let ports = Taps(
        ports
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("a required native guard tap could not be created; no menu was opened")?,
    );
    for port in &ports.0 {
        attach(port)?;
    }
    let (commands, input) = mpsc::channel();
    thread::Builder::new()
        .name("agent-screen-guard-parent".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(line) => {
                        if commands.send(line).is_err() {
                            return;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .map_err(|e| e.to_string())?;
    output(json!({"ready":true,"pid":std::process::id()}));
    // While armed, do not intercept input. There is no menu yet.
    unsafe { &mut *state_cell.get() }.queue.focus_released();
    let armed = Instant::now();
    loop {
        CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.002, false);
        let state = unsafe { &mut *state_cell.get() };
        if armed.elapsed() > Duration::from_secs(2) {
            finish_custody(state, &ports, &root, config.nonce, true, true)?;
            return Err("native guard parent did not dispatch its opening action".into());
        }
        match input.try_recv() {
            Ok(command) if command == "open" => break,
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                finish_custody(state, &ports, &root, config.nonce, true, true)?;
                return Err("native menu opening was cancelled".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
    if unsafe { IsSecureEventInputEnabled() } || unsafe { SSMenuKeyPid() } != user {
        let state = unsafe { &mut *state_cell.get() };
        finish_custody(state, &ports, &root, config.nonce, true, true)?;
        return Err(
            "native guard preconditions changed before opening; no opening was authorized".into(),
        );
    }
    {
        let state = unsafe { &mut *state_cell.get() };
        state.queue = Queue::default();
        state.active = true;
    }
    let started = Instant::now();
    let (events, menus) = mpsc::channel();
    let cancel_shared = shared.clone();
    let cancel_config = config.clone();
    let cancel_events = events.clone();
    worker_started(
        thread::Builder::new()
            .name("agent-screen-guard-cancel".into())
            .spawn(move || cancellation_worker(cancel_config, cancel_shared, cancel_events)),
        || {
            shared.cancel.store(true, Ordering::Release);
            shared.done.store(true, Ordering::Release);
            finish_custody(
                unsafe { &mut *state_cell.get() },
                &ports,
                &root,
                config.nonce,
                true,
                true,
            )
        },
    )?;
    let worker_shared = shared.clone();
    let worker_config = config.clone();
    worker_started(
        thread::Builder::new()
            .name("agent-screen-guard-menu".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    menu_worker(worker_config, user, worker_shared.clone(), events.clone())
                }));
                if result.is_err() {
                    worker_shared.fail(4);
                    let _ = events.send(MenuEvent::Failed("native menu worker panicked".into()));
                }
            }),
        || {
            shared.cancel.store(true, Ordering::Release);
            shared.done.store(true, Ordering::Release);
            finish_custody(
                unsafe { &mut *state_cell.get() },
                &ports,
                &root,
                config.nonce,
                true,
                true,
            )
        },
    )?;
    output(json!({"opened":true}));
    let mut closed = false;
    let mut picked = None;
    let mut error = None;
    let panicked = {
        let mut drive = || {
            loop {
                CFRunLoop::run_in_mode(unsafe { kCFRunLoopDefaultMode }, 0.002, false);
                let state = unsafe { &mut *state_cell.get() };
                match input.try_recv() {
                    Ok(command) if command == "status" && config.test_sender.is_some() => output(
                        json!({"status":true,"held":state.held,"replayed":state.replayed,"retained":state.queue.retained(),"fault":shared.fault.load(Ordering::Acquire)}),
                    ),
                    Ok(command) if command == "disable" && config.test_sender.is_some() => {
                        CGEvent::tap_enable(&ports.0[0], false);
                        shared.fail(2);
                    }
                    Ok(command) if command == "secure" && config.test_sender.is_some() => {
                        shared.fail(1);
                    }
                    Ok(command) if command == "timeout" && config.test_sender.is_some() => {
                        shared.fail(5);
                    }
                    Ok(command) if command == "panic" && config.test_sender.is_some() => {
                        panic!("native guard test fault")
                    }
                    Ok(_) => shared.fail(3),
                    Err(mpsc::TryRecvError::Disconnected) => shared.fail(3),
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if shared.cancel.load(Ordering::Acquire) {
                    let own = shared.own_theft.swap(0, Ordering::AcqRel);
                    if own != 0 && unsafe { SSMenuReleaseFocusID(own) } != 0 {
                        shared.own_theft.store(own, Ordering::Release);
                    }
                }
                let secure = unsafe { IsSecureEventInputEnabled() };
                let taps_ready = ports.0.iter().all(|p| CGEvent::tap_is_enabled(p));
                if secure {
                    shared.fail(1);
                }
                if !taps_ready {
                    state.taps_disabled += 1;
                    shared.fail(2);
                    for port in ports.0.iter() {
                        CGEvent::tap_enable(port, true);
                    }
                }
                if started.elapsed().saturating_add(CLOSE_RESERVE)
                    >= agent_screen_model::menu_guard::DEADLINE
                    && picked.is_none()
                {
                    shared.fail(5);
                }
                while let Ok(event) = menus.try_recv() {
                    match event {
                        MenuEvent::FocusReleased => {
                            state.queue.focus_released();
                            output(json!({"focusReleased":true,"held":state.held}));
                        }
                        MenuEvent::Closed => closed = true,
                        MenuEvent::Picked(title) => picked = Some(title),
                        MenuEvent::Failed(message) => error = Some(message),
                    }
                }
                if shared.fault.load(Ordering::Acquire) != 0 {
                    let code = shared.fault.load(Ordering::Acquire);
                    let reason = match code {
                        1 => CancelReason::SecureInput,
                        2 => CancelReason::TapUnavailable,
                        3 => CancelReason::OwnerGone,
                        4 => CancelReason::Panic,
                        _ => CancelReason::Deadline,
                    };
                    if state.queue.failure().is_none() {
                        let _ = state
                            .records
                            .send(Record::Fault(format!("failure code {code}")));
                    }
                    state.queue.fail(reason);
                }
                let focus = unsafe { SSMenuKeyPid() };
                let returned =
                    focus > 0 && focus != config.target.pid && focus != std::process::id() as i32;
                if closed && returned {
                    if state.queue.failure().is_some() {
                        state
                            .queue
                            .resume_after_failure(true, true, secure, taps_ready);
                    } else {
                        state.queue.confirm_closed(true, true);
                    }
                }
                if !secure && taps_ready {
                    state.persist_new();
                    state.replay();
                }
                if state.queue.phase() == Phase::Done {
                    break;
                }
                if started.elapsed() >= RECOVERY_LIMIT {
                    error = Some("native guard cleanup is incomplete; retained input remains in private recovery custody".into());
                    break;
                }
            }
        };
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut drive));
        if run.is_err() {
            shared.fail(4);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut drive));
        }
        run.is_err()
    };
    if panicked {
        error = Some("native guard controller panicked; recovery kept input custody".into());
    }
    let state = unsafe { &mut *state_cell.get() };
    shared.cancel.store(true, Ordering::Release);
    shared.done.store(true, Ordering::Release);
    let own = shared.own_theft.swap(0, Ordering::AcqRel);
    if own != 0 {
        unsafe {
            SSMenuReleaseFocusID(own);
        }
    }
    let retained = state.queue.retained() + state.quarantine.len();
    let focus_returned = unsafe { SSMenuKeyPid() } == user;
    let custody = finish_custody(state, &ports, &root, config.nonce, closed, focus_returned);
    output(
        json!({"picked":picked,"error":error,"fault":shared.fault.load(Ordering::Acquire),"closed":closed,"retained":retained,"held":state.held,"replayed":state.replayed,"maxAddedLatencyMs":state.max_delay.as_secs_f64()*1000.0,"tapsDisabled":state.taps_disabled,"blockedTestEvents":state.blocked_test_events,"noticesDropped":state.notices_dropped,"custodyRemoved":custody.is_ok(),"elapsedMs":started.elapsed().as_secs_f64()*1000.0}),
    );
    if retained != 0 || !closed || !focus_returned || custody.is_err() {
        return Err("native guard retained input recovery is required".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_capture(root: &std::path::Path) -> Capture {
        let shared = Arc::new(Shared::new());
        let source = CGEventSource::new(CGEventSourceStateID::Private).unwrap();
        Capture {
            queue: Queue::default(),
            records: logger(root.to_path_buf(), 9, 20, 21, shared.clone()).unwrap(),
            shared,
            marker_prefix: 42,
            next: 0,
            logged: 0,
            pending: HashMap::new(),
            held: 0,
            replayed: 0,
            max_delay: Duration::ZERO,
            taps_disabled: 0,
            blocked_test_events: 0,
            notices_dropped: 0,
            quarantine: Vec::new(),
            pause_after: None,
            pause_reported: false,
            target: 20,
            active: false,
            source,
            user: 21,
            user_window: WindowTarget {
                pid: 21,
                window_id: 2,
            },
            display_id: 1,
            test_sender: None,
            test_nonce: 0,
            area: Rect::new(0.0, 0.0, 100.0, 100.0),
        }
    }

    static TEST_DIRECTORY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "agent-screen-custody-exit-{}-{}-{}",
            std::process::id(),
            TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn transient_acquisition_retries_frozen_snapshot_but_publication_keeps_one_lease() {
        use std::cell::Cell;
        let root = test_directory();
        let mut old = Some(journal::Journal::create(&root, 71, 20, 21).unwrap());
        old.as_mut().unwrap().held(1, b"old").unwrap();
        let frozen = vec![journal::RetainedRecord {
            id: 2,
            bytes: b"pending".to_vec(),
            dispatched: true,
        }];
        let acquisitions = Cell::new(0);
        let publications = Cell::new(0);
        let mut waits = Vec::new();
        persist_with_lease(
            || {
                acquisitions.set(acquisitions.get() + 1);
                assert_eq!(frozen[0].bytes, b"pending");
                if acquisitions.get() == 1 {
                    return Err("injected transient open EINTR".into());
                }
                journal::Journal::fallback(&root, 71, 20, 21, frozen.clone())
            },
            |lease| {
                publications.set(publications.get() + 1);
                if publications.get() == 1 {
                    assert!(
                        old.take().unwrap().finish(0, true, true).is_err(),
                        "reserved fallback must revoke the old writer"
                    );
                    return Err("injected transient publication error".into());
                }
                lease.retry()
            },
            |stage| waits.push(stage),
        );
        assert_eq!(acquisitions.get(), 2);
        assert_eq!(publications.get(), 2);
        assert_eq!(waits, [CheckpointWait::Acquire, CheckpointWait::Publish]);
        let inventory = journal::summaries(&root).unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0]["retained"], 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_worker_start_preserves_armed_quarantine_and_stops_workers() {
        let root = test_directory();
        let mut state = test_capture(&root);
        let event = CGEvent::new_keyboard_event(Some(&state.source), 0, true).unwrap();
        state.quarantine.push(Saved {
            id: 0,
            event,
            kind: Event::Key,
            captured: Instant::now(),
            simulated: true,
        });
        let shared = state.shared.clone();
        let failed: std::io::Result<()> = Err(std::io::Error::other("spawn fault"));
        let result = worker_started(failed, || {
            shared.cancel.store(true, Ordering::Release);
            shared.done.store(true, Ordering::Release);
            finish_custody(&mut state, &Taps(Vec::new()), &root, 9, true, true)
        });
        assert!(result.is_err());
        assert!(shared.done.load(Ordering::Acquire));
        assert!(shared.cancel.load(Ordering::Acquire));
        assert!(
            std::fs::read_dir(&root)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| journal::inspect(&entry.path())
                    .is_ok_and(|record| record.events.len() == 1))
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_copy_still_retains_callback_custody() {
        let source = CGEventSource::new(CGEventSourceStateID::Private).unwrap();
        let original = CGEvent::new_keyboard_event(Some(&source), 0, true).unwrap();
        CGEvent::set_timestamp(Some(&original), 777);
        let retained = own_for_custody(&original, || None);
        drop(original);
        assert_eq!(CGEvent::timestamp(Some(&retained)), 777);
    }

    #[test]
    fn all_replays_keep_originals_until_final_annotation() {
        let root = test_directory();
        let mut state = test_capture(&root);
        let event = CGEvent::new_keyboard_event(Some(&state.source), 0, true).unwrap();
        let saved = Saved {
            id: 0,
            event: event.clone(),
            kind: Event::Key,
            captured: Instant::now(),
            simulated: false,
        };
        let _ = state.queue.offer(Event::Key, Origin::Hardware, saved);
        state.next = 1;
        state.queue.focus_released();
        let (token, saved) = state.queue.next_replay().unwrap();
        state.pending.insert(0, (token, saved.captured));
        let replay = CGEvent::new_copy(Some(&event)).unwrap();
        CGEvent::set_integer_value_field(
            Some(&replay),
            CGEventField::EventSourceUserData,
            state.marker(0, false),
        );
        assert!(state.acknowledge(&replay, false));
        assert_eq!(state.queue.retained(), 1);
        assert_eq!(state.replayed, 0);
        assert!(state.acknowledge(&replay, true));
        assert_eq!(state.queue.retained(), 0);
        assert_eq!(state.replayed, 1);
        let _ = finish_custody(&mut state, &Taps(Vec::new()), &root, 9, true, true);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_before_open_seals_quarantined_input() {
        let root = test_directory();
        let mut state = test_capture(&root);
        let event = CGEvent::new_keyboard_event(Some(&state.source), 0, true).unwrap();
        state.quarantine.push(Saved {
            id: 0,
            event,
            kind: Event::Key,
            captured: Instant::now(),
            simulated: true,
        });
        assert!(finish_custody(&mut state, &Taps(Vec::new()), &root, 9, true, true).is_err());
        let preserved = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| {
                journal::inspect(&entry.path()).is_ok_and(|record| record.events.len() == 1)
            });
        assert!(preserved);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replay_source_is_our_pid_and_keeps_original_event_data() {
        let source = CGEventSource::new(CGEventSourceStateID::Private).expect("private source");
        let original = CGEvent::new_keyboard_event(Some(&source), 0, true).expect("key event");
        CGEvent::set_timestamp(Some(&original), 123456789);
        CGEvent::set_integer_value_field(Some(&original), CGEventField::EventSourceUserData, 7);
        let data = CGEvent::new_data(None, Some(&original)).expect("serialized key");
        let restored = CGEvent::from_data(None, Some(&data)).expect("restored key");
        let copy = CGEvent::new_copy(Some(&restored)).expect("replay copy");
        CGEvent::set_source(Some(&copy), Some(&source));
        CGEvent::set_integer_value_field(Some(&copy), CGEventField::EventSourceUserData, 99);
        assert_eq!(
            CGEvent::integer_value_field(Some(&copy), CGEventField::EventSourceUnixProcessID),
            i64::from(std::process::id())
        );
        assert_eq!(CGEvent::timestamp(Some(&copy)), 123456789);
        assert_eq!(
            CGEvent::integer_value_field(Some(&original), CGEventField::EventSourceUserData),
            7
        );
    }

    #[test]
    fn pointer_motion_is_never_a_hold_kind() {
        for raw in [5, 6, 7, 27] {
            assert_eq!(event_kind(raw), Event::PointerMove);
        }
    }
}
