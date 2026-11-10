//! The user's input on the Viewer's display image, forwarded to the Agent
//! Display.
//!
//! A local event monitor takes the NSEvents themselves rather than GPUI's
//! versions of them: those drop scroll and momentum phases, turn a
//! Control-click into a right click, and keep only five modifier flags.
//! Events outside the image, and every event while the Dismissal sheet is
//! up, go on to GPUI.
//!
//! Keys follow `viewer_keys` (ADR 0008). With a keyboard layout they go to
//! the target as keys. With an input method, every key without Command or
//! Control goes through the Viewer's input context; a key the input method
//! passes through goes on as the key itself, and text it composed goes as
//! text. A composition belongs to one generation: a drop, a Takeover's
//! start or end and a display switch each start a new one, and text from
//! an older composition is dropped.
//!
//! While it is unknown whether a password field has the target's keyboard,
//! keys the input method would see wait, in order, for the lookup, up to
//! `viewer_keys::HOLD_MS`; every key event after them waits behind them.
//! The answer routes them; when the wait runs out they go as keys. Held
//! keys belong to a generation too, and a drop discards them.

use super::click_diagnostics::{self as diagnostics, Click, Event as ClickEvent, Phase};
use super::input::{Button, Forwarder};
use super::native::{self, Native};
use super::{cg, menubar_height};
use agent_screen_model::viewer_keys::{self, Call, Outcome, Press, Route, Secrecy, Source};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventPhase, NSEventType, NSWindow,
};
use objc2_core_graphics::CGEventFlags;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ptr::NonNull;
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    /// The Viewer's NSWindow.
    window: usize,
    display: Option<(u32, Forwarder)>,
    /// The display image in window points, top-left origin.
    content: Option<[f64; 4]>,
    /// The Dismissal sheet is up: everything goes to GPUI.
    blocked: bool,
    /// The button the user pressed on the image, until it comes up.
    pressed: Option<Button>,
    click: Option<Click>,
    frame_pixels: Option<[usize; 2]>,
    /// How each key's last press went, so its release goes the same way
    /// whatever changed meanwhile.
    downs: HashMap<u16, Down>,
    /// Key events waiting for the lookup of the target's focus, in order.
    held: VecDeque<Held>,
    /// When the current wait started.
    held_since: Option<Instant>,
    /// Counts waits, so the timer of an earlier one ends nothing.
    wait: u64,
    /// A wait ran out before any lookup answered: keys go as keys until
    /// one does, or the focus may have moved again.
    gave_up: bool,
}

/// A key event waiting for the lookup, with what it was typed with.
struct Held {
    event: Retained<NSEvent>,
    source: Source,
    /// Tests: what the input method does with it.
    script: Option<Script>,
    injected: bool,
    generation: u64,
}

/// How a key's press went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Down {
    /// To the target as the key: its release goes there too.
    Raw,
    /// To the target with its release, as for a Command shortcut, whose
    /// release AppKit may never report.
    UpSent,
    /// The input method kept it: its release goes nowhere.
    Kept,
}

/// A key on its way through the input context, out of the state's borrow:
/// the input method calls back into the Viewer.
struct Through {
    event: Retained<NSEvent>,
    window: Retained<NSWindow>,
    display: u32,
    forwarder: Forwarder,
}

/// Tests: what the input method does with a key, in place of the user's.
#[derive(Clone, Debug)]
pub enum Script {
    /// Marks this text.
    Mark(String),
    /// Inserts this text.
    Insert(String),
    /// Inserts the key's own characters.
    Pass,
    /// Inserts this text, then asks for the key's command, as Korean input
    /// does for Return after a syllable.
    InsertCommand(String),
    /// Nothing the Viewer sees, as for a command such as an arrow.
    Nothing,
}

impl Script {
    pub fn parse(text: &str) -> Option<Script> {
        Some(match text {
            "pass" => Script::Pass,
            "nothing" => Script::Nothing,
            _ => match text.split_once(':')? {
                ("mark", t) => Script::Mark(t.into()),
                ("insert", t) => Script::Insert(t.into()),
                ("insertcommand", t) => Script::InsertCommand(t.into()),
                _ => return None,
            },
        })
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Handling an event a test made: the Viewer must not take the keyboard.
    static INJECTING: Cell<bool> = const { Cell::new(false) };
    /// Except this one, which takes it as the user's click does.
    static TAKE_KEY: Cell<bool> = const { Cell::new(false) };
    /// The text the input method has marked in the Viewer, while it
    /// composes, and the generation it started in.
    static MARKED: RefCell<Option<(String, u64)>> = const { RefCell::new(None) };
    /// Counts drops, Takeover starts and ends, and display switches.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
    /// What the input method did with the key going through it now.
    static CALLS: RefCell<Option<Vec<Call>>> = const { RefCell::new(None) };
    /// Tests: the input source a key or click made by `inject_key` or
    /// `inject_click` is typed with, and what the input method does.
    static SOURCE: Cell<Option<Source>> = const { Cell::new(None) };
    static SCRIPT: RefCell<Option<Script>> = const { RefCell::new(None) };
    /// What the last key down came to: its route, or its outcome.
    static ROUTED: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// Replaying a held key event: it waits no longer.
    static REPLAYING: Cell<bool> = const { Cell::new(false) };
    /// The keys that made the composition under way, with their modifiers:
    /// should a password field turn out to have the keyboard, they go to
    /// it as keys, as they would have had the Viewer known.
    static COMPOSED_KEYS: RefCell<Vec<(u16, CGEventFlags)>> = const { RefCell::new(Vec::new()) };
    /// Tests: how long held keys waited, in milliseconds.
    static HELD_MS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
    /// Sending held keys before a click: none waits again until the click.
    static FLUSHING: Cell<bool> = const { Cell::new(false) };
    /// Tests: each key press's final route, in order, by key code, and how
    /// many compositions a password field found under way.
    static ROUTES: RefCell<Vec<(u16, &'static str)>> = const { RefCell::new(Vec::new()) };
    static SECURE_FOUND: Cell<u64> = const { Cell::new(0) };
}

/// A key press went this way: its route, or its outcome.
fn note_route(code: u16, route: &'static str) {
    ROUTED.with(|r| r.set(Some(route)));
    ROUTES.with(|r| {
        let mut r = r.borrow_mut();
        if r.len() >= 256 {
            r.remove(0);
        }
        r.push((code, route));
    });
}

/// Each key press's final route, by key code, for tests.
pub fn routes() -> Vec<(u16, &'static str)> {
    ROUTES.with(|r| r.borrow().clone())
}

/// How many compositions were dropped as a password field turned out to
/// have the keyboard, for tests: none should be, when the lookup came
/// first.
pub fn secure_found_count() -> u64 {
    SECURE_FOUND.with(Cell::get)
}

/// Whether a key, its wait over, may still wait: not while sending held
/// keys before a click, nor after a wait ran out.
fn given_up(s: &State) -> bool {
    s.gave_up || FLUSHING.with(Cell::get)
}

/// The user's input source, or the one a test stands in.
fn source_now() -> Source {
    SOURCE
        .with(|s| s.get())
        .unwrap_or_else(super::ime::current_kind)
}

/// The focus may have moved: look up whether it is a password field, and
/// its caret, for an input method only, off the main thread. Until the
/// answer, keys go to the target as keys.
fn inspect_for_input_method(forwarder: &Forwarder, display: u32) {
    if source_now() == Source::InputMethod {
        forwarder.inspect(cg::display_bounds(display), true);
    }
}

fn next_generation() -> u64 {
    GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    })
}

/// The input method marked `text`, or None: it cancelled. A composition
/// that starts belongs to the current generation.
pub fn set_marked(text: Option<&str>) {
    CALLS.with(|c| {
        if let Some(calls) = c.borrow_mut().as_mut() {
            calls.push(Call::Marked(text.unwrap_or_default().into()));
        }
    });
    if text.is_none() {
        COMPOSED_KEYS.with(|k| k.borrow_mut().clear());
    }
    let generation = GENERATION.with(Cell::get);
    MARKED.with(|m| {
        let mut m = m.borrow_mut();
        *m = text.map(|t| {
            let started = m.as_ref().map_or(generation, |(_, g)| *g);
            (t.to_string(), started)
        });
    });
}

pub fn marked() -> Option<String> {
    MARKED.with(|m| m.borrow().as_ref().map(|(t, _)| t.clone()))
}

/// The input method made a call the Viewer can't follow, such as
/// replacing text already sent.
pub fn ignored() {
    CALLS.with(|c| {
        if let Some(calls) = c.borrow_mut().as_mut() {
            calls.push(Call::Ignored);
        }
    });
}

/// The Takeover of `display` started or ended. An end drops a composition,
/// now, before the card or anything else can take the keyboard and commit
/// it. A start keeps the one the key that started it began.
pub fn takeover_changed(display: u32, active: bool) {
    let shown = STATE.with(|s| {
        s.try_borrow()
            .map(|s| s.display.as_ref().map(|(d, _)| *d))
            .unwrap_or(None)
    });
    if shown != Some(display) {
        return;
    }
    if active {
        let generation = next_generation();
        MARKED.with(|m| {
            if let Some((_, g)) = m.borrow_mut().as_mut() {
                *g = generation;
            }
        });
        retag_held(generation);
    } else {
        drop_composition();
        release_after_takeover();
    }
}

/// The display the Viewer shows, if any.
fn shown_display() -> Option<u32> {
    STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|s| s.display.as_ref().map(|(d, _)| *d))
    })
}

/// A Takeover ended: keys the target got the press of come up there now,
/// as their releases may never reach the Viewer.
fn release_after_takeover() {
    let busy = STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut s) => {
            if release_downs(&mut s) {
                log::debug!("viewer: keys released as the Takeover ended");
            }
            false
        }
        Err(_) => true,
    });
    if busy {
        native::main_async(release_after_takeover);
    }
}

/// End the input method's composition without committing it: the
/// Takeover ended, the display changed or ended, or the sheet came up.
/// The input context discards its marked text too, and text from it that
/// comes after is dropped.
/// Keys held for a lookup go with it.
pub fn drop_composition() {
    drop_marked();
    clear_held();
}

/// The composition alone, uncommitted: a new generation.
fn drop_marked() {
    next_generation();
    COMPOSED_KEYS.with(|k| k.borrow_mut().clear());
    let had = MARKED.with(|m| m.borrow_mut().take()).is_some();
    if had {
        log::debug!("viewer: composition dropped");
        discard_in_input_context();
    }
}

/// Discard the held key events; a release of a key held down goes nowhere.
fn clear_held() {
    STATE.with(|s| {
        // Busy inside an event: the generation tells the drain they're stale.
        let Ok(mut s) = s.try_borrow_mut() else {
            return;
        };
        let dropped = std::mem::take(&mut s.held);
        s.held_since = None;
        forget_downs(&mut s, &dropped);
        if !dropped.is_empty() {
            log::debug!("viewer: {} held key event(s) dropped", dropped.len());
        }
    });
}

/// Held key events dropped: a press never sent has its release go
/// nowhere; a release of a key sent down goes out, as does the modifiers'
/// state after a held modifier change, so no key stays down in the target.
fn forget_downs(s: &mut State, dropped: &VecDeque<Held>) {
    let shown = s.display.clone();
    let mut modifier = None;
    for held in dropped {
        let code = held.event.keyCode();
        match held.event.r#type() {
            // A repeat keeps the way its key's press went: a press sent
            // as the key still gets its release.
            NSEventType::KeyDown => {
                s.downs.entry(code).or_insert(Down::Kept);
            }
            NSEventType::KeyUp => {
                if s.downs.remove(&code) == Some(Down::Raw)
                    && let Some((display, forwarder)) = &shown
                {
                    let bounds = cg::display_bounds(*display);
                    forwarder.key(code, false, flags(&held.event), bounds);
                }
            }
            NSEventType::FlagsChanged => modifier = Some(code),
            _ => {}
        }
    }
    if let (Some(code), Some((_, forwarder))) = (modifier, &shown) {
        forwarder.flags_changed(code, modifiers_now());
    }
}

/// The modifiers held on the keyboard now.
fn modifiers_now() -> CGEventFlags {
    CGEventFlags(NSEvent::modifierFlags_class().0 as u64)
}

/// The display changes: keys sent down to the old one come up there, and
/// held ones are dropped.
fn release_all(s: &mut State) {
    let dropped = std::mem::take(&mut s.held);
    s.held_since = None;
    forget_downs(s, &dropped);
    let sent = release_downs(s);
    if let Some((display, _)) = &s.display
        && (sent || !dropped.is_empty())
    {
        log::debug!("viewer: keys released on display {display} as the Viewer left it");
    }
    // The next display's keys wait for its own lookup.
    s.gave_up = false;
}

/// Release every key the shown target got the press of. The keys stay
/// down on the keyboard: their releases, when they come, go nowhere.
/// Whether any went.
fn release_downs(s: &mut State) -> bool {
    let shown = s.display.clone();
    let mut sent = false;
    for (code, down) in s.downs.iter_mut() {
        if *down == Down::Raw {
            if let Some((display, forwarder)) = &shown {
                forwarder.key(*code, false, CGEventFlags(0), cg::display_bounds(*display));
                sent = true;
            }
            *down = Down::UpSent;
        }
    }
    sent
}

/// Held key events stay current across a new generation that drops no
/// keys: a Takeover's start, or a composition dropped for a password field.
fn retag_held(generation: u64) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            for held in s.held.iter_mut() {
                held.generation = generation;
            }
        }
    });
}

/// Hold `event` for the lookup, behind any held before it.
fn hold(s: &mut State, event: &NSEvent) {
    s.held.push_back(Held {
        event: objc2::Message::retain(event),
        source: source_now(),
        script: SCRIPT.with(|t| t.borrow_mut().take()),
        injected: INJECTING.with(Cell::get),
        generation: GENERATION.with(Cell::get),
    });
    if s.held_since.is_none() {
        start_wait(s);
    }
}

fn start_wait(s: &mut State) {
    s.held_since = Some(Instant::now());
    s.wait += 1;
    let wait = s.wait;
    native::main_after(Duration::from_millis(viewer_keys::HOLD_MS), move || {
        wait_ended(wait)
    });
}

fn note_wait(since: Instant) {
    HELD_MS.with(|h| {
        let mut h = h.borrow_mut();
        if h.len() >= 256 {
            h.remove(0);
        }
        h.push(since.elapsed().as_secs_f64() * 1000.0);
    });
}

/// How long held keys waited, in milliseconds, for tests.
pub fn held_ms() -> Vec<f64> {
    HELD_MS.with(|h| h.borrow().clone())
}

/// The wait ran out with no answer: held keys go as keys, the safe way
/// for a password field.
fn wait_ended(wait: u64) {
    let ended = STATE.with(|s| {
        s.try_borrow_mut().map(|mut s| {
            let ended = s.wait == wait && !s.held.is_empty();
            if ended {
                s.gave_up = true;
            }
            ended
        })
    });
    match ended {
        Err(_) => native::main_async(move || wait_ended(wait)),
        Ok(true) => {
            log::debug!(
                "viewer: no focus lookup answered in {} ms; held keys go as keys",
                viewer_keys::HOLD_MS
            );
            drain();
        }
        Ok(false) => {}
    }
}

/// A lookup of the shown target's focus answered: held keys go on as it
/// says, and a composition never reaches a password field.
pub fn lookup_answered() {
    secure_found();
    if secrecy() != Secrecy::Unknown {
        STATE.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.gave_up = false;
            }
        });
    }
    drain();
}

/// Whether held keys wait for the click's own lookup instead: a click goes
/// after the keys typed before it, which go now, as keys unless a lookup
/// already answered.
fn flush_before_click(event: &NSEvent, mtm: MainThreadMarker) {
    let held = STATE.with(|s| {
        let Ok(s) = s.try_borrow() else {
            return false;
        };
        let ours = event
            .window(mtm)
            .is_some_and(|w| &*w as *const NSWindow as usize == s.window);
        ours && !s.held.is_empty()
    });
    if held {
        // Even a held Tab, which makes the focus unknown again, doesn't
        // hold back the keys after it: they were typed before the click.
        let was = FLUSHING.with(|f| f.replace(true));
        drain();
        FLUSHING.with(|f| f.set(was));
    }
}

/// Route held key events, oldest first, while none of them has to wait:
/// one may again, as after a Tab that moved the focus.
fn drain() {
    enum Next {
        Busy,
        Done,
        Replay(Held),
    }
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let mut replayed = false;
    loop {
        let next = STATE.with(|state| {
            let Ok(mut s) = state.try_borrow_mut() else {
                return Next::Busy;
            };
            let generation = GENERATION.with(Cell::get);
            // Stale held keys never go out.
            let (current, stale): (VecDeque<Held>, VecDeque<Held>) = std::mem::take(&mut s.held)
                .into_iter()
                .partition(|h| h.generation == generation);
            s.held = current;
            forget_downs(&mut s, &stale);
            let Some(front) = s.held.front() else {
                if let Some(since) = s.held_since.take() {
                    note_wait(since);
                }
                return Next::Done;
            };
            let secrecy = s
                .display
                .as_ref()
                .map_or(Secrecy::Unknown, |(_, f)| routing_secrecy(f));
            let waits = match front.event.r#type() {
                NSEventType::KeyDown => {
                    viewer_keys::holds(front.source, secrecy, press_of(&front.event), given_up(&s))
                }
                NSEventType::FlagsChanged => viewer_keys::holds_modifier(
                    front.source,
                    secrecy,
                    front.event.keyCode(),
                    given_up(&s),
                ),
                _ => false,
            };
            if waits {
                // A key replayed now made the focus unknown again.
                if replayed {
                    if let Some(since) = s.held_since.take() {
                        note_wait(since);
                    }
                    start_wait(&mut s);
                }
                return Next::Done;
            }
            s.held.pop_front().map_or(Next::Done, Next::Replay)
        });
        match next {
            Next::Busy => {
                native::main_async(drain);
                return;
            }
            Next::Done => return,
            Next::Replay(held) => {
                replayed = true;
                replay(held, mtm);
            }
        }
    }
}

/// Handle a held key event as when it was typed, with its input source.
fn replay(held: Held, mtm: MainThreadMarker) {
    let was = (
        REPLAYING.with(|r| r.replace(true)),
        SOURCE.with(|s| s.replace(Some(held.source))),
        INJECTING.with(|i| i.replace(held.injected)),
    );
    SCRIPT.with(|s| *s.borrow_mut() = held.script);
    handle(&held.event, mtm);
    SCRIPT.with(|s| s.borrow_mut().take());
    REPLAYING.with(|r| r.set(was.0));
    SOURCE.with(|s| s.set(was.1));
    INJECTING.with(|i| i.set(was.2));
}

/// Commit the input method's marked text as typed, as a native field does
/// when the Viewer gives up the keyboard.
pub fn commit_composition() {
    if let Some(text) = take_for_commit() {
        send_text(&text);
    }
}

/// The marked text of the current generation, to commit: the composition
/// ends, and the input context is told to drop its copy. Anything it then
/// commits belongs to no composition, and is dropped.
fn take_for_commit() -> Option<String> {
    let (text, started) = MARKED.with(|m| m.borrow_mut().take())?;
    COMPOSED_KEYS.with(|k| k.borrow_mut().clear());
    discard_in_input_context();
    if started != GENERATION.with(Cell::get) {
        log::warn!("viewer: a composition from an earlier Takeover dropped");
        return None;
    }
    log::debug!("viewer: composition committed by the Viewer");
    Some(text)
}

/// The input context's discardMarkedText, after the current event: the
/// input method may call back into the Viewer's view, inside GPUI.
fn discard_in_input_context() {
    native::main_async(|| {
        let window = STATE.with(|s| s.try_borrow().map(|s| s.window).unwrap_or(0));
        if window == 0 {
            return;
        }
        let window = unsafe { &*(window as *const NSWindow) };
        if let Some(context) = input_context(window) {
            let _: () = unsafe { objc2::msg_send![context, discardMarkedText] };
        }
    });
}

/// How many focus lookups the shown display's Viewer asked for, for tests.
pub fn inspections() -> Option<u64> {
    STATE.with(|s| {
        s.try_borrow()
            .ok()?
            .display
            .as_ref()
            .map(|(_, f)| f.inspections())
    })
}

/// Whether the target of the display the Viewer shows has a password
/// field focused, as last looked up, or secure input is on.
pub fn secure() -> bool {
    secrecy() == Secrecy::Secure
}

/// Whether the shown target's focus is a password field, or unknown since
/// it may have moved.
pub fn secrecy() -> Secrecy {
    STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|s| s.display.as_ref().map(|(_, f)| secrecy_for(f)))
            .unwrap_or(Secrecy::Unknown)
    })
}

/// Secure input on is a password field focused somewhere, unless the
/// focus may have moved: an app turns it off only once it handled the
/// click or key that left its password field, so keys wait for the lookup
/// and then see whether it is still on.
fn secrecy_for(forwarder: &Forwarder) -> Secrecy {
    match (super::ime::secure_input(), forwarder.secrecy()) {
        (true, Secrecy::Unknown) => Secrecy::Unknown,
        (true, _) => Secrecy::Secure,
        (false, secrecy) => secrecy,
    }
}

/// What keys go by: as looked up, except that while the input method
/// composes, an unknown focus is open. The composition already shows the
/// focus is no password field, and a lookup that finds one drops it; a key
/// that waited, or went as the key, would land among its letters.
fn routing_secrecy(forwarder: &Forwarder) -> Secrecy {
    match secrecy_for(forwarder) {
        Secrecy::Unknown
            if MARKED.with(|m| m.borrow().is_some()) && !super::ime::secure_input() =>
        {
            Secrecy::Open
        }
        secrecy => secrecy,
    }
}

/// A lookup found a password field focused in a target: a composition in
/// the Viewer is dropped, never committed, so none of it reaches the field
/// as text, and the keys that made it go to the field as keys.
pub fn secure_found() {
    if marked().is_none() || !secure() {
        return;
    }
    let keys = COMPOSED_KEYS.with(|k| std::mem::take(&mut *k.borrow_mut()));
    SECURE_FOUND.with(|c| c.set(c.get() + 1));
    log::warn!(
        "viewer: a password field has the keyboard; composition dropped, its {} key(s) sent as keys",
        keys.len()
    );
    drop_marked();
    retag_held(GENERATION.with(Cell::get));
    let shown = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.display.clone()));
    if let Some((display, forwarder)) = shown {
        let bounds = cg::display_bounds(display);
        for (code, flags) in keys {
            forwarder.key(code, true, flags, bounds);
            forwarder.key(code, false, flags, bounds);
        }
    }
}

/// Text the input method inserted in the Viewer. For a key going through
/// it, the key's outcome decides; otherwise, as for a candidate the user
/// clicked, it goes to the target only if it ends a composition of the
/// current generation.
pub fn commit(text: &str) {
    let in_key = CALLS.with(|c| {
        c.borrow_mut()
            .as_mut()
            .map(|calls| calls.push(Call::Inserted(text.into())))
            .is_some()
    });
    if in_key {
        MARKED.with(|m| m.borrow_mut().take());
        return;
    }
    let started = MARKED.with(|m| m.borrow_mut().take()).map(|(_, g)| g);
    COMPOSED_KEYS.with(|k| k.borrow_mut().clear());
    if started != Some(GENERATION.with(Cell::get)) {
        log::warn!("viewer: input method text dropped, it belongs to no current composition");
        return;
    }
    send_text(text);
}

/// Text to the target where the user's keys go, through the same checks,
/// as Unicode text that no keyboard layout changes.
fn send_text(text: &str) {
    if text.is_empty() {
        return;
    }
    let shown = STATE.with(|s| match s.try_borrow() {
        Ok(s) if !s.blocked => Ok(s.display.clone()),
        Ok(_) => Ok(None),
        Err(_) => Err(()),
    });
    match shown {
        Ok(Some((display, forwarder))) => {
            native::send(Native::ScreenTouched { display });
            super::catch::agent_acted(display);
            forwarder.text(text.to_string(), cg::display_bounds(display));
            // Looks run, as the app may move its focus on the text, as a
            // form that advances to its password field does, but the focus
            // stays known: the next word's first letter mustn't wait. One
            // now, and one 100 ms on, by when a page's tree has caught up.
            // A Return or Tab that follows as the key looks up the focus
            // again itself, and a look that finds a password field drops a
            // composition.
            if source_now() == Source::InputMethod {
                let bounds = cg::display_bounds(display);
                forwarder.inspect(bounds, false);
                native::main_after(Duration::from_millis(100), move || {
                    if shown_display() == Some(display) {
                        forwarder.inspect(bounds, false);
                    }
                });
            }
        }
        Ok(None) => log::debug!("viewer: composed text dropped, no display to send it to"),
        // Inside the event handler, which holds the state: after it.
        Err(()) => {
            let text = text.to_string();
            native::main_async(move || send_text(&text));
        }
    }
}

/// A key through the input context, and what the target gets for it.
fn through_input_method(key: Through) {
    let code = key.event.keyCode();
    let characters = key
        .event
        .characters()
        .map(|c| c.to_string())
        .unwrap_or_default();
    let composing = MARKED.with(|m| m.borrow().is_some());
    watch_commands(&key.window);
    CALLS.with(|c| *c.borrow_mut() = Some(Vec::new()));
    match SCRIPT.with(|s| s.borrow_mut().take()) {
        Some(script) => run_script(&key.window, script, &characters),
        None => {
            if let Some(context) = input_context(&key.window) {
                let _: bool = unsafe { objc2::msg_send![context, handleEvent: &*key.event] };
            }
        }
    }
    let calls = CALLS.with(|c| c.borrow_mut().take()).unwrap_or_default();
    let outcome = viewer_keys::outcome(composing, &characters, &calls);
    note_route(code, outcome.name());
    let bounds = cg::display_bounds(key.display);
    let press = press_of(&key.event);
    // The keys of the composition under way, from the one that began it.
    let only_marked = outcome == Outcome::Nothing;
    COMPOSED_KEYS.with(|k| {
        let mut k = k.borrow_mut();
        if !composing || !only_marked {
            k.clear();
        }
        if MARKED.with(|m| m.borrow().is_some()) {
            k.push((code, flags(&key.event)));
        }
    });
    let down = match outcome {
        Outcome::Key => {
            key.forwarder.key(code, true, flags(&key.event), bounds);
            Down::Raw
        }
        // The text first, then the key, down now and up on its release.
        Outcome::TextThenKey(text) => {
            send_text(&text);
            key.forwarder.key(code, true, flags(&key.event), bounds);
            Down::Raw
        }
        Outcome::Text(text) => {
            send_text(&text);
            Down::Kept
        }
        Outcome::Nothing => Down::Kept,
    };
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.downs.insert(code, down);
        }
    });
    if down == Down::Raw && viewer_keys::moves_focus(press) {
        key.forwarder.inspect(bounds, true);
        STATE.with(|s| {
            if let Ok(mut s) = s.try_borrow_mut() {
                s.gave_up = false;
            }
        });
    } else if MARKED.with(|m| m.borrow().is_some()) {
        // Composing: the app may have moved its focus by itself, maybe to
        // a password field, which drops the composition. Without waiting.
        key.forwarder.inspect(bounds, false);
    }
}

fn press_of(event: &NSEvent) -> Press {
    let held = event.modifierFlags();
    Press {
        code: event.keyCode(),
        command: held.contains(NSEventModifierFlags::Command),
        control: held.contains(NSEventModifierFlags::Control),
        repeat: event.isARepeat(),
    }
}

/// GPUI's view drops the input method's doCommandBySelector: when no key
/// of its own is in flight, so the Viewer can't see a command that
/// follows composed text. Its implementation is wrapped once: while a key
/// goes through the input context, the call is noted; otherwise GPUI's
/// runs.
fn watch_commands(window: &NSWindow) {
    use objc2::runtime::{AnyObject, Imp, Sel};
    type Command = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Sel);
    static ORIGINAL: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    unsafe extern "C-unwind" fn noted(this: *mut AnyObject, cmd: Sel, selector: Sel) {
        let in_key = CALLS.with(|c| {
            c.borrow_mut()
                .as_mut()
                .map(|calls| calls.push(Call::Command))
                .is_some()
        });
        if in_key {
            return;
        }
        if let Some(Some(original)) = ORIGINAL.get() {
            let original: Command = unsafe { std::mem::transmute::<usize, Command>(*original) };
            unsafe { original(this, cmd, selector) };
        }
    }
    ORIGINAL.get_or_init(|| {
        let view = window.firstResponder()?;
        let method = view
            .class()
            .instance_method(objc2::sel!(doCommandBySelector:))?;
        let ours: Imp = unsafe { std::mem::transmute::<Command, Imp>(noted) };
        let original = unsafe { method.set_implementation(ours) };
        Some(original as usize)
    });
}

/// Tests: what the input method does, through the view's own
/// NSTextInputClient methods.
fn run_script(window: &NSWindow, script: Script, characters: &str) {
    let result = match script {
        Script::Mark(text) => text_input(window, &text, false),
        Script::Insert(text) => text_input(window, &text, true),
        Script::Pass => text_input(window, characters, true),
        Script::InsertCommand(text) => text_input(window, &text, true).map(|()| {
            let view = window.firstResponder();
            if let Some(view) = view {
                let _: () = unsafe {
                    objc2::msg_send![&*view, doCommandBySelector: objc2::sel!(insertNewline:)]
                };
            }
        }),
        Script::Nothing => Ok(()),
    };
    if let Err(e) = result {
        log::warn!("viewer: test input method failed: {e}");
    }
}

/// The target's caret in the Viewer's window, in points from its top-left
/// corner, as x, y, width, height: where the input method shows its
/// candidates. The point last clicked when the target doesn't say.
pub fn caret_in_window() -> Option<[f64; 4]> {
    STATE.with(|s| {
        let s = s.try_borrow().ok()?;
        let (display, forwarder) = s.display.as_ref()?;
        let content = s.content?;
        let caret = forwarder.caret()?;
        Some(caret_on_image(caret, cg::display_bounds(*display), content))
    })
}

/// A rect on the display, in global points, on the display image at
/// `content` in the window.
fn caret_on_image(
    caret: crate::model::Rect,
    display: crate::model::Rect,
    content: [f64; 4],
) -> [f64; 4] {
    let [cx, cy, cw, ch] = content;
    let (sx, sy) = (cw / display.width, ch / display.height);
    [
        cx + (caret.x - display.x) * sx,
        cy + (caret.y - display.y) * sy,
        caret.width * sx,
        caret.height * sy,
    ]
}

/// Tests: a click at a point of the display image, given as fractions of
/// its width and height, handled as if the user made it. With `key`, the
/// Viewer first gives up the keyboard and the click takes it back, as the
/// user's click on a Viewer behind another window does.
pub fn inject_click(x: f64, y: f64, key: bool, source: Option<Source>) -> Result<(), String> {
    SOURCE.with(|s| s.set(source));
    let clicked = inject_click_now(x, y, key);
    SOURCE.with(|s| s.set(None));
    clicked
}

fn inject_click_now(x: f64, y: f64, key: bool) -> Result<(), String> {
    if key {
        let window = STATE.with(|s| s.borrow().window);
        if window == 0 {
            return Err("no Viewer".into());
        }
        unsafe { &*(window as *const NSWindow) }.resignKeyWindow();
        TAKE_KEY.with(|k| k.set(true));
    }
    let down = inject_mouse(NSEventType::LeftMouseDown, x, y);
    TAKE_KEY.with(|k| k.set(false));
    down?;
    inject_mouse(NSEventType::LeftMouseUp, x, y)
}

/// Tests: one left-button event at a point of the display image, given as
/// fractions of its width and height, handled as if the user made it.
pub fn inject_mouse(kind: NSEventType, x: f64, y: f64) -> Result<(), String> {
    let (window, content) = STATE.with(|s| {
        let s = s.borrow();
        (s.window, s.content)
    });
    let [cx, cy, cw, ch] = content.ok_or("the Viewer shows no display")?;
    if window == 0 {
        return Err("no Viewer".into());
    }
    let window = unsafe { &*(window as *const NSWindow) };
    let height = window.contentView().ok_or("no Viewer")?.frame().size.height;
    let at = objc2_foundation::NSPoint::new(cx + x * cw, height - (cy + y * ch));
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
        kind,
        at,
        objc2_app_kit::NSEventModifierFlags::empty(),
        0.0,
        window.windowNumber(),
        None,
        0,
        1,
        1.0,
    )
    .ok_or("no event")?;
    inject(&event)
}

/// Tests: press and release a key on the Viewer, as if the user typed it,
/// with Shift and Command held as asked, typing `characters`. `source`,
/// when given, stands in for the input source selected, and `script` for
/// what the input method does with the key. What the key came to: "raw"
/// for the key itself, "text" for composed text, "inputMethod" for
/// nothing yet, "held" for waiting on the focus lookup.
/// A key a test types in the Viewer.
pub struct TestKey<'a> {
    pub code: u16,
    pub shift: bool,
    pub command: bool,
    pub characters: &'a str,
    /// A held key's repeat.
    pub repeat: bool,
}

pub fn inject_key(
    key: TestKey,
    source: Option<Source>,
    script: Option<Script>,
    kinds: &[NSEventType],
) -> Result<&'static str, String> {
    let TestKey {
        code,
        shift,
        command,
        characters,
        repeat,
    } = key;
    let window = STATE.with(|s| s.borrow().window);
    if window == 0 {
        return Err("no Viewer".into());
    }
    let window = unsafe { &*(window as *const NSWindow) };
    let mut flags = objc2_app_kit::NSEventModifierFlags::empty();
    if shift {
        flags |= objc2_app_kit::NSEventModifierFlags::Shift;
    }
    if command {
        flags |= objc2_app_kit::NSEventModifierFlags::Command;
    }
    let characters = objc2_foundation::NSString::from_str(characters);
    let mut routed = "raw";
    for &kind in kinds {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            window.windowNumber(),
            None,
            &characters,
            &characters,
            repeat && kind == NSEventType::KeyDown,
            code,
        )
        .ok_or("no event")?;
        SOURCE.with(|s| s.set(source));
        SCRIPT.with(|s| *s.borrow_mut() = script.clone());
        ROUTED.with(|r| r.set(None));
        let injected = inject(&event);
        SOURCE.with(|s| s.set(None));
        SCRIPT.with(|s| s.borrow_mut().take());
        if kind == NSEventType::KeyDown {
            routed = ROUTED.with(|r| r.get()).unwrap_or("raw");
        }
        injected?;
    }
    Ok(routed)
}

/// Tests: Shift pressed (`down`) or released in the Viewer, with `source`
/// standing in for the user's input source.
pub fn inject_shift(down: bool, source: Option<Source>) -> Result<(), String> {
    let window = STATE.with(|s| s.borrow().window);
    if window == 0 {
        return Err("no Viewer".into());
    }
    let window = unsafe { &*(window as *const NSWindow) };
    let flags = if down {
        objc2_app_kit::NSEventModifierFlags::Shift
    } else {
        objc2_app_kit::NSEventModifierFlags::empty()
    };
    let none = objc2_foundation::NSString::from_str("");
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::FlagsChanged,
        objc2_foundation::NSPoint::new(0.0, 0.0),
        flags,
        0.0,
        window.windowNumber(),
        None,
        &none,
        &none,
        false,
        56,
    )
    .ok_or("no event")?;
    SOURCE.with(|s| s.set(source));
    let injected = inject(&event);
    SOURCE.with(|s| s.set(None));
    injected
}

/// Tests: an input method's call on the Viewer's view, as the user's makes
/// it while they compose. With `commit`, insertText: the composition ends
/// with `text`. Otherwise setMarkedText: `text` is marked, the caret at its
/// end. Calls AppKit's NSTextInputClient methods on the view, so GPUI's own
/// routing runs. Never from inside a GPUI update: the view calls back into
/// it.
pub fn inject_text_input(text: &str, commit: bool) -> Result<(), String> {
    let window = STATE.with(|s| s.borrow().window);
    if window == 0 {
        return Err("no Viewer".into());
    }
    text_input(unsafe { &*(window as *const NSWindow) }, text, commit)
}

fn text_input(window: &NSWindow, text: &str, commit: bool) -> Result<(), String> {
    use objc2::msg_send;
    use objc2_foundation::{NSNotFound, NSString};
    /// NSRange as GPUI's view declares it, `{NSRange=QQ}`, which
    /// objc2's checked sends tell apart from AppKit's `{_NSRange=QQ}`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRange(usize, usize);
    unsafe impl objc2::Encode for NSRange {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Struct("NSRange", &[usize::ENCODING, usize::ENCODING]);
    }
    let view = window
        .firstResponder()
        .ok_or("the Viewer has no first responder")?;
    let string = NSString::from_str(text);
    let none = NSRange(NSNotFound as usize, 0);
    if commit {
        let _: () = unsafe { msg_send![&*view, insertText: &*string, replacementRange: none] };
    } else {
        let caret = NSRange(string.length(), 0);
        let _: () = unsafe {
            msg_send![&*view, setMarkedText: &*string, selectedRange: caret, replacementRange: none]
        };
    }
    Ok(())
}

fn inject(event: &NSEvent) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    INJECTING.with(|i| i.set(true));
    let handled = handle(event, mtm);
    INJECTING.with(|i| i.set(false));
    handled
        .then_some(())
        .ok_or_else(|| "the Viewer let the event through".into())
}

pub fn attach(window: &NSWindow) {
    STATE.with(|s| s.borrow_mut().window = window as *const NSWindow as usize);
}

/// The display the Viewer shows, or None once it is closed.
pub fn show(display: Option<(u32, Forwarder)>) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let id = |d: &Option<(u32, Forwarder)>| d.as_ref().map(|(id, _)| *id);
        if id(&s.display) != id(&display) {
            release_all(&mut s);
        }
        s.display = display;
        s.pressed = None;
        s.click = None;
        if let Some((display, forwarder)) = &s.display {
            inspect_for_input_method(forwarder, *display);
        }
    });
    // A composition belongs to the display it started on.
    drop_composition();
}

/// Where the image is, and whether the sheet covers it. The Viewer reports
/// this on every paint.
///
/// AppKit paints a window that becomes key at once, inside whatever made it
/// key. When that is the input handler, which holds the state, the report
/// waits for the event to finish: a second borrow would panic inside an
/// AppKit callback and abort the app (issue #40).
pub fn layout(content: Option<[f64; 4]>, blocked: bool, frame_pixels: Option<[usize; 2]>) {
    let busy = STATE.with(|s| match s.try_borrow_mut() {
        Ok(mut s) => {
            s.content = content;
            s.blocked = blocked;
            s.frame_pixels = frame_pixels;
            false
        }
        Err(_) => true,
    });
    if busy {
        log::debug!("viewer: layout reported during input handling, applied after it");
        native::main_async(move || layout(content, blocked, frame_pixels));
    }
}

/// The Viewer's frame and its display image, in global top-left points,
/// for tests. None while the Viewer shows no display.
pub fn debug_layout() -> Option<([f64; 4], [f64; 4], bool)> {
    STATE.with(|s| {
        let s = s.borrow();
        s.display.as_ref()?;
        let [cx, cy, cw, ch] = s.content?;
        let window = unsafe { &*(s.window as *const NSWindow) };
        let f = window.frame();
        let top = native::primary_height() - (f.origin.y + f.size.height);
        Some((
            [f.origin.x, top, f.size.width, f.size.height],
            [f.origin.x + cx, top + cy, cw, ch],
            window.isKeyWindow(),
        ))
    })
}

fn flags(event: &NSEvent) -> CGEventFlags {
    CGEventFlags(event.modifierFlags().0 as u64)
}

/// NSEventPhase to CGScrollPhase.
fn scroll_phase(phase: NSEventPhase) -> i64 {
    if phase.contains(NSEventPhase::Began) {
        1
    } else if phase.contains(NSEventPhase::Changed) {
        2
    } else if phase.contains(NSEventPhase::Ended) {
        4
    } else if phase.contains(NSEventPhase::Cancelled) {
        8
    } else if phase.contains(NSEventPhase::MayBegin) {
        128
    } else {
        0
    }
}

/// NSEventPhase to CGMomentumScrollPhase.
fn momentum_phase(phase: NSEventPhase) -> i64 {
    if phase.contains(NSEventPhase::Began) {
        1
    } else if phase.contains(NSEventPhase::Changed) {
        2
    } else if phase.contains(NSEventPhase::Ended) || phase.contains(NSEventPhase::Cancelled) {
        3
    } else {
        0
    }
}

/// The event's point on the display, in global CG coordinates, or None
/// outside the image unless `clamp`.
fn remote(
    event: &NSEvent,
    window: &NSWindow,
    content: [f64; 4],
    display: u32,
    clamp: bool,
    frame_pixels: Option<[usize; 2]>,
) -> Option<(f64, f64)> {
    let at = event.locationInWindow();
    let height = window.contentView()?.frame().size.height;
    remote_point(
        (at.x, at.y),
        height,
        content,
        clamp,
        || cg::display_bounds(display),
        |bounds, global| {
            let f = window.frame();
            ClickEvent::Mapping {
                display,
                window_frame: [f.origin.x, f.origin.y, f.size.width, f.size.height],
                frame_pixels,
                content,
                view_height: height,
                point: [at.x, at.y],
                display_bounds: bounds,
                global,
            }
        },
    )
}

// The real mapping seam. A rejected point still emits opt-in metadata,
// without querying display bounds. The metadata closure stays lazy.
fn remote_point(
    at: (f64, f64),
    height: f64,
    content: [f64; 4],
    clamp: bool,
    bounds: impl FnOnce() -> crate::model::Rect,
    metadata: impl FnOnce(Option<crate::model::Rect>, Option<(f64, f64)>) -> ClickEvent,
) -> Option<(f64, f64)> {
    let normalized = normalized_point(at, height, content, clamp);
    let bounds = normalized.map(|_| bounds());
    let global = normalized.zip(bounds).map(|(n, b)| mapped_point(n, b));
    diagnostics::emit(|| metadata(bounds, global));
    global
}

fn normalized_point(
    at: (f64, f64),
    height: f64,
    content: [f64; 4],
    clamp: bool,
) -> Option<(f64, f64)> {
    let (x, y) = (at.0, height - at.1);
    let [cx, cy, cw, ch] = content;
    let mut nx = (x - cx) / cw;
    let mut ny = (y - cy) / ch;
    if clamp {
        nx = nx.clamp(0.0, 1.0);
        ny = ny.clamp(0.0, 1.0);
    } else if !(0.0..=1.0).contains(&nx) || !(0.0..=1.0).contains(&ny) {
        return None;
    }
    Some((nx, ny))
}

fn mapped_point(normalized: (f64, f64), bounds: crate::model::Rect) -> (f64, f64) {
    (
        bounds.x + normalized.0 * (bounds.width - 1.0),
        bounds.y + normalized.1 * (bounds.height - 1.0),
    )
}

/// Handle one event; false lets it go on to GPUI.
fn handle(event: &NSEvent, mtm: MainThreadMarker) -> bool {
    if !REPLAYING.with(Cell::get)
        && matches!(
            event.r#type(),
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
        )
    {
        // Outside the state: a window becoming key draws at once, and its
        // paint reports the layout here.
        if let Some(window) = pressed_viewer(event, mtm) {
            native::make_key(&window);
        }
        flush_before_click(event, mtm);
    }
    // A composition's keys reach a password field before any key typed
    // after them, which the field gets as a key.
    if event.r#type() == NSEventType::KeyDown {
        secure_found();
    }
    let mut after = None;
    let handled = handle_in(event, mtm, &mut after);
    // Outside the state: the input method calls back into the Viewer.
    match after {
        Some(After::Through(key)) => through_input_method(key),
        Some(After::Sees(window, event)) => input_method_sees(&window, &event),
        None => {}
    }
    handled
}

/// The Viewer's window, when `event` is a press in it that takes the
/// keyboard: the user's, or a test's that stands in for one. The window
/// becomes key before the press is handled, as handling it makes Agent
/// Screen WindowServer's front process (`note_viewer_press`). The other
/// way round, AppKit takes that for an activation by a click and waits for
/// the press to finish it, and the monitor keeps the press from AppKit: the
/// app then has no key window, the Viewer's input context is never the
/// current one, and the input method composes nothing (ADR 0008).
fn pressed_viewer(event: &NSEvent, mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    if INJECTING.with(Cell::get) && !TAKE_KEY.with(Cell::get) {
        return None;
    }
    let window = event.window(mtm)?;
    let viewer = STATE.with(|s| s.try_borrow().map(|s| s.window).unwrap_or(0));
    (viewer != 0 && &*window as *const NSWindow as usize == viewer).then_some(window)
}

/// Work for after the event's state borrow, as it may come back into it.
enum After {
    /// A key through the input context.
    Through(Through),
    /// A modifier change for the input context to see.
    Sees(Retained<NSWindow>, Retained<NSEvent>),
}

fn handle_in(event: &NSEvent, mtm: MainThreadMarker, after: &mut Option<After>) -> bool {
    STATE.with(|state| {
        let mut s = state.borrow_mut();
        let Some(window) = event.window(mtm) else {
            return false;
        };
        if s.window == 0 || &*window as *const NSWindow as usize != s.window || s.blocked {
            return false;
        }
        let (Some((display, forwarder)), Some(content)) = (s.display.clone(), s.content) else {
            return false;
        };
        let kind = event.r#type();
        // Key events wait behind held ones, in order.
        if matches!(
            kind,
            NSEventType::KeyDown | NSEventType::KeyUp | NSEventType::FlagsChanged
        ) && !REPLAYING.with(Cell::get)
            && !s.held.is_empty()
        {
            if kind == NSEventType::KeyDown {
                ROUTED.with(|r| r.set(Some("held")));
            }
            hold(&mut s, event);
            return true;
        }
        let button = match kind {
            NSEventType::LeftMouseDown
            | NSEventType::LeftMouseDragged
            | NSEventType::LeftMouseUp => Button::Left,
            NSEventType::RightMouseDown
            | NSEventType::RightMouseDragged
            | NSEventType::RightMouseUp => Button::Right,
            _ => Button::Middle,
        };
        match kind {
            NSEventType::LeftMouseDown
            | NSEventType::RightMouseDown
            | NSEventType::OtherMouseDown => {
                let click = Click::begin();
                let Some(at) = diagnostics::within(click, Phase::Down, || {
                    remote(event, &window, content, display, false, s.frame_pixels)
                }) else {
                    return false;
                };
                // A test's press never takes the user's keyboard.
                if !INJECTING.with(|i| i.get()) {
                    super::input::note_viewer_press(window.windowNumber() as u32);
                }
                // The menu bar strip: only a click that opens a menu takes
                // the display over, which the shell decides.
                if button == Button::Left
                    && at.1 - cg::display_bounds(display).y < menubar_height(display)
                {
                    native::send(Native::MenuBarClick {
                        display,
                        x: event.locationInWindow().x,
                    });
                    return true;
                }
                // A click commits a composition, where the keys went, before
                // the focus moves.
                if MARKED.with(|m| m.borrow().is_some())
                    && let Some(text) = take_for_commit()
                {
                    forwarder.text(text, cg::display_bounds(display));
                }
                // The user's first input takes the display over from its agent.
                native::send(Native::ScreenTouched { display });
                super::catch::agent_acted(display);
                // The focus may move: a new wait may begin.
                s.gave_up = false;
                s.pressed = Some(button);
                s.click = click;
                forwarder.mouse_down(at, button, event.clickCount() as i64, flags(event), click);
                true
            }
            NSEventType::LeftMouseDragged
            | NSEventType::RightMouseDragged
            | NSEventType::OtherMouseDragged => {
                let Some(pressed) = s.pressed else {
                    return false;
                };
                if let Some(at) = remote(event, &window, content, display, true, s.frame_pixels) {
                    forwarder.mouse_dragged(at, pressed, flags(event));
                }
                true
            }
            NSEventType::LeftMouseUp | NSEventType::RightMouseUp | NSEventType::OtherMouseUp => {
                // Release whatever went down, even if a modifier changed
                // what the system calls this button meanwhile.
                let Some(pressed) = s.pressed.take() else {
                    return false;
                };
                let click = s.click.take();
                if let Some(at) = diagnostics::within(click, Phase::Up, || {
                    remote(event, &window, content, display, true, s.frame_pixels)
                }) {
                    forwarder.mouse_up(at, pressed, event.clickCount() as i64, flags(event), click);
                }
                // The click may have focused a field: once it lands.
                inspect_for_input_method(&forwarder, display);
                true
            }
            NSEventType::MouseMoved => {
                if s.pressed.is_none()
                    && let Some(at) =
                        remote(event, &window, content, display, false, s.frame_pixels)
                {
                    forwarder.mouse_moved(at, flags(event));
                }
                false
            }
            NSEventType::ScrollWheel => {
                let Some(at) = remote(event, &window, content, display, false, s.frame_pixels)
                else {
                    return false;
                };
                // Fingers resting on the trackpad send MayBegin and empty
                // events: only a scroll that moves something takes over.
                let moved = event.scrollingDeltaX() != 0.0 || event.scrollingDeltaY() != 0.0;
                if moved && !event.phase().contains(NSEventPhase::MayBegin) {
                    native::send(Native::ScreenTouched { display });
                    super::catch::agent_acted(display);
                }
                forwarder.scroll(
                    at,
                    (event.scrollingDeltaX(), event.scrollingDeltaY()),
                    event.hasPreciseScrollingDeltas(),
                    scroll_phase(event.phase()),
                    momentum_phase(event.momentumPhase()),
                    flags(event),
                );
                true
            }
            NSEventType::KeyDown => {
                let code = event.keyCode();
                native::send(Native::ScreenTouched { display });
                super::catch::agent_acted(display);
                let press = press_of(event);
                let source = source_now();
                // As last looked up, never waited for.
                let secrecy = routing_secrecy(&forwarder);
                let bounds = cg::display_bounds(display);
                // A lookup is on its way, or this asks for one.
                if source == Source::InputMethod && secrecy == Secrecy::Unknown {
                    forwarder.inspect_unless_pending(bounds);
                }
                if viewer_keys::holds(source, secrecy, press, given_up(&s)) {
                    ROUTED.with(|r| r.set(Some("held")));
                    hold(&mut s, event);
                    return true;
                }
                let route = viewer_keys::route(source, secrecy, press);
                if route == Route::InputMethod {
                    s.downs.remove(&code);
                    *after = Some(After::Through(Through {
                        event: objc2::Message::retain(event),
                        window: window.clone(),
                        display,
                        forwarder: forwarder.clone(),
                    }));
                    return true;
                }
                note_route(code, "raw");
                forwarder.key(code, true, flags(event), bounds);
                // Command shortcuts often never report their release, so
                // send it now and skip the real one if it comes.
                if flags(event).contains(CGEventFlags::MaskCommand) {
                    forwarder.key(code, false, flags(event), bounds);
                    s.downs.insert(code, Down::UpSent);
                } else {
                    s.downs.insert(code, Down::Raw);
                }
                if source == Source::InputMethod && viewer_keys::moves_focus(press) {
                    forwarder.inspect(bounds, true);
                    s.gave_up = false;
                }
                true
            }
            NSEventType::KeyUp => {
                let code = event.keyCode();
                // As its press went; one the Viewer never saw goes out.
                match s.downs.remove(&code) {
                    Some(Down::Kept) | Some(Down::UpSent) => {}
                    Some(Down::Raw) | None => {
                        forwarder.key(code, false, flags(event), cg::display_bounds(display));
                    }
                }
                true
            }
            NSEventType::FlagsChanged => {
                let source = source_now();
                let secrecy = routing_secrecy(&forwarder);
                // Shift switches the input method's mode: while the focus
                // is unknown, it waits with the keys, so the input method
                // sees it once the focus is known to be no password field.
                if viewer_keys::holds_modifier(source, secrecy, event.keyCode(), given_up(&s)) {
                    forwarder.inspect_unless_pending(cg::display_bounds(display));
                    hold(&mut s, event);
                    return true;
                }
                forwarder.flags_changed(event.keyCode(), flags(event));
                // An input method switches modes on Shift alone: it sees
                // the modifiers too, after the state's borrow.
                if !INJECTING.with(|i| i.get())
                    && source == Source::InputMethod
                    && secrecy == Secrecy::Open
                {
                    *after = Some(After::Sees(window.clone(), objc2::Message::retain(event)));
                }
                true
            }
            _ => false,
        }
    })
}

/// Hand `event` to the input context of the Viewer's view, as AppKit's
/// own text views do with modifier changes.
fn input_method_sees(window: &NSWindow, event: &NSEvent) {
    if let Some(context) = input_context(window) {
        let _: bool = unsafe { objc2::msg_send![context, handleEvent: event] };
    }
}

/// The input context of the Viewer's view, GPUI's.
fn input_context(window: &NSWindow) -> Option<&objc2::runtime::AnyObject> {
    let view = window.firstResponder()?;
    let context: *mut objc2::runtime::AnyObject = unsafe { objc2::msg_send![&*view, inputContext] };
    unsafe { context.as_ref() }
}

pub fn install() {
    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseDown
        | NSEventMask::RightMouseDragged
        | NSEventMask::RightMouseUp
        | NSEventMask::OtherMouseDown
        | NSEventMask::OtherMouseDragged
        | NSEventMask::OtherMouseUp
        | NSEventMask::MouseMoved
        | NSEventMask::ScrollWheel
        | NSEventMask::KeyDown
        | NSEventMask::KeyUp
        | NSEventMask::FlagsChanged;
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        let mtm = MainThreadMarker::new().expect("main thread");
        if handle(unsafe { event.as_ref() }, mtm) {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    if let Some(monitor) =
        unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) }
    {
        native::keep_monitor(monitor);
    }
}

#[cfg(test)]
mod click_diagnostics_tests {
    use super::*;
    use crate::model::Rect;
    use serde_json::json;

    const CONTENT: [f64; 4] = [20.0, 60.0, 640.0, 450.0];

    fn metadata(point: (f64, f64), bounds: Option<Rect>, global: Option<(f64, f64)>) -> ClickEvent {
        ClickEvent::Mapping {
            display: 66,
            window_frame: [10.0, 20.0, 680.0, 530.0],
            frame_pixels: Some([1280, 900]),
            content: CONTENT,
            view_height: 530.0,
            point: [point.0, point.1],
            display_bounds: bounds,
            global,
        }
    }

    #[test]
    fn a_caret_on_the_display_lands_on_its_image() {
        // A 1280x900 display shown at half size, 20 pt in and 60 pt down.
        let display = Rect::new(4288.0, -2340.0, 1280.0, 900.0);
        let image = [20.0, 60.0, 640.0, 450.0];
        let caret = Rect::new(4288.0 + 100.0, -2340.0 + 200.0, 2.0, 20.0);
        assert_eq!(
            caret_on_image(caret, display, image),
            [70.0, 160.0, 1.0, 10.0]
        );
    }

    #[test]
    fn real_mapping_and_stamp_coordinates_remain_correlated() {
        let point = (438.0, 215.5);
        let display = Rect::new(4288.0, -2340.0, 1280.0, 900.0);
        let mut mapped = None;
        let rows = diagnostics::tests::capture(|| {
            diagnostics::within(Click::when(true), Phase::Down, || {
                mapped = remote_point(
                    point,
                    530.0,
                    CONTENT,
                    false,
                    || display,
                    |b, g| metadata(point, b, g),
                );
            })
        });
        let global = mapped.unwrap();
        let modal = Rect::new(4680.0, -1981.0, 497.0, 182.0);
        let local = diagnostics::local_point(global, Some(modal));
        assert!((global.0 - 5123.346875).abs() < 0.000001);
        assert!((global.1 + 1831.565555555556).abs() < 0.000001);
        assert!(Rect::new(5090.0, -1844.0, 68.0, 26.0).contains(global));
        assert_eq!((local.0 + modal.x, local.1 + modal.y), global);
        assert_eq!(rows[0]["global"], json!([global.0, global.1]));
        assert_eq!(rows[0]["content"], json!(CONTENT));
        assert_eq!(rows[0]["accepted"], true);
    }

    #[test]
    fn rejected_mapping_keeps_the_click_id_without_a_bounds_query() {
        let click = Click::when(true);
        let point = (0.0, 600.0);
        let rows = diagnostics::tests::capture(|| {
            diagnostics::within(click, Phase::Down, || {
                assert!(
                    remote_point(
                        point,
                        530.0,
                        CONTENT,
                        false,
                        || panic!("rejected point queried CG bounds"),
                        |b, g| metadata(point, b, g)
                    )
                    .is_none()
                );
            });
            diagnostics::within(click, Phase::Up, || {
                assert!(
                    remote_point(
                        point,
                        530.0,
                        CONTENT,
                        true,
                        || Rect::new(4288.0, -2340.0, 1280.0, 900.0),
                        |b, g| metadata(point, b, g)
                    )
                    .is_some()
                );
            });
        });
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["click"], rows[1]["click"]);
        assert_eq!(rows[0]["accepted"], false);
        assert_eq!(rows[0]["point"], json!([0.0, 600.0]));
        assert_eq!(rows[0]["content"], json!(CONTENT));
        assert!(rows[0]["global"].is_null());
        assert!(rows[0]["display_bounds"].is_null());
        assert_eq!(rows[1]["global"], json!([4288.0, -2340.0]));
    }

    #[test]
    fn disabled_mapping_does_not_collect_metadata_or_change_query_order() {
        let point = (438.0, 215.5);
        let queries = std::cell::Cell::new(0);
        let rows = diagnostics::tests::capture(|| {
            diagnostics::within(Click::when(false), Phase::Down, || {
                assert!(
                    remote_point(
                        point,
                        530.0,
                        CONTENT,
                        false,
                        || {
                            queries.set(queries.get() + 1);
                            Rect::new(0.0, 0.0, 1280.0, 900.0)
                        },
                        |_, _| panic!("disabled metadata collected")
                    )
                    .is_some()
                );
            })
        });
        assert_eq!(queries.get(), 1);
        assert!(rows.is_empty());
    }
}
