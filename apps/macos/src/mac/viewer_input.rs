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

use super::click_diagnostics::{self as diagnostics, Click, Event as ClickEvent, Phase};
use super::input::{Button, Forwarder};
use super::native::{self, Native};
use super::{cg, menubar_height};
use agent_screen_model::viewer_keys::{self, Call, Focus, Outcome, Press, Route, Source};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventModifierFlags, NSEventPhase, NSEventType, NSWindow,
};
use objc2_core_graphics::CGEventFlags;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::ptr::NonNull;

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
    /// Keys sent with Command, whose release AppKit may never report.
    sent_up: HashSet<u16>,
    /// Keys the input method kept: their release goes nowhere.
    composed: HashSet<u16>,
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
}

/// The user's input source, or the one a test stands in.
fn source_now() -> Source {
    SOURCE
        .with(|s| s.get())
        .unwrap_or_else(super::ime::current_kind)
}

/// Look up the target's password field and caret, for an input method
/// only, off the main thread: nothing waits for it.
fn inspect_for_input_method(forwarder: &Forwarder, display: u32) {
    if source_now() == Source::InputMethod {
        forwarder.inspect(cg::display_bounds(display));
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
    } else {
        drop_composition();
    }
}

/// End the input method's composition without committing it: the
/// Takeover ended, the display changed or ended, or the sheet came up.
/// The input context discards its marked text too, and text from it that
/// comes after is dropped.
pub fn drop_composition() {
    next_generation();
    let had = MARKED.with(|m| m.borrow_mut().take()).is_some();
    // Releases of keys the input method had: the next real ones go out.
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.composed.clear();
        }
    });
    if had {
        log::debug!("viewer: composition dropped");
        discard_in_input_context();
    }
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
/// field focused, as last looked up, or secure input is on. Unknown is no.
pub fn secure() -> bool {
    STATE.with(|s| {
        s.try_borrow()
            .ok()
            .and_then(|s| s.display.as_ref().map(|(_, f)| secure_for(f)))
            .unwrap_or_else(super::ime::secure_input)
    })
}

fn secure_for(forwarder: &Forwarder) -> bool {
    super::ime::secure_input() || forwarder.text_focus() == Focus::Secure
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
    ROUTED.with(|r| r.set(Some(outcome.name())));
    let bounds = cg::display_bounds(key.display);
    match outcome {
        Outcome::Key => {
            key.forwarder.key(code, true, flags(&key.event), bounds);
            return;
        }
        Outcome::Text(text) => send_text(&text),
        Outcome::Nothing => {}
    }
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            s.composed.insert(code);
        }
    });
    // A composition starts: where its candidates go, without waiting.
    if !composing && MARKED.with(|m| m.borrow().is_some()) {
        key.forwarder.inspect(bounds);
    }
}

/// Tests: what the input method does, through the view's own
/// NSTextInputClient methods.
fn run_script(window: &NSWindow, script: Script, characters: &str) {
    let result = match script {
        Script::Mark(text) => text_input(window, &text, false),
        Script::Insert(text) => text_input(window, &text, true),
        Script::Pass => text_input(window, characters, true),
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
/// nothing yet.
pub fn inject_key(
    code: u16,
    shift: bool,
    command: bool,
    characters: &str,
    source: Option<Source>,
    script: Option<Script>,
) -> Result<&'static str, String> {
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
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            window.windowNumber(),
            None,
            &characters,
            &characters,
            false,
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
        s.display = display;
        s.pressed = None;
        s.click = None;
        s.composed.clear();
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
    let mut take_key = None;
    let mut through = None;
    let handled = handle_in(event, mtm, &mut take_key, &mut through);
    // Outside the state: a window becoming key draws at once, and its
    // paint reports the layout here.
    if let Some(window) = take_key {
        native::make_key(&window);
    }
    // Outside the state too: the input method calls back into the Viewer.
    if let Some(key) = through {
        through_input_method(key);
    }
    handled
}

fn handle_in(
    event: &NSEvent,
    mtm: MainThreadMarker,
    take_key: &mut Option<objc2::rc::Retained<NSWindow>>,
    through: &mut Option<Through>,
) -> bool {
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
                if !INJECTING.with(|i| i.get()) || TAKE_KEY.with(|k| k.get()) {
                    *take_key = Some(window.clone());
                }
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
                s.composed.clear();
                if MARKED.with(|m| m.borrow().is_some())
                    && let Some(text) = take_for_commit()
                {
                    forwarder.text(text, cg::display_bounds(display));
                }
                // The user's first input takes the display over from its agent.
                native::send(Native::ScreenTouched { display });
                super::catch::agent_acted(display);
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
                let held = event.modifierFlags();
                let press = Press {
                    code,
                    command: held.contains(NSEventModifierFlags::Command),
                    control: held.contains(NSEventModifierFlags::Control),
                };
                // What was last looked up, after a click: never waited for.
                let route = viewer_keys::route(source_now(), secure_for(&forwarder), press);
                if route == Route::InputMethod {
                    s.sent_up.remove(&code);
                    s.composed.remove(&code);
                    *through = Some(Through {
                        event: objc2::Message::retain(event),
                        window: window.clone(),
                        display,
                        forwarder: forwarder.clone(),
                    });
                    return true;
                }
                ROUTED.with(|r| r.set(Some("raw")));
                // A release the input method had and the Viewer missed.
                s.composed.remove(&code);
                forwarder.key(code, true, flags(event), cg::display_bounds(display));
                // Command shortcuts often never report their release, so
                // send it now and skip the real one if it comes.
                if flags(event).contains(CGEventFlags::MaskCommand) {
                    forwarder.key(code, false, flags(event), cg::display_bounds(display));
                    s.sent_up.insert(code);
                } else {
                    // A plain press: its release is a real one.
                    s.sent_up.remove(&code);
                }
                true
            }
            NSEventType::KeyUp => {
                let code = event.keyCode();
                if s.composed.remove(&code) {
                    return true;
                }
                if !s.sent_up.remove(&code) {
                    forwarder.key(code, false, flags(event), cg::display_bounds(display));
                }
                true
            }
            NSEventType::FlagsChanged => {
                forwarder.flags_changed(event.keyCode(), flags(event));
                // An input method switches modes on Shift alone: it sees
                // the modifiers too.
                if !INJECTING.with(|i| i.get())
                    && super::ime::current_kind() == Source::InputMethod
                    && !secure_for(&forwarder)
                {
                    input_method_sees(&window, event);
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
