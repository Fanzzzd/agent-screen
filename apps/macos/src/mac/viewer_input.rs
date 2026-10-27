//! The user's input on the Viewer's display image, forwarded to the Agent
//! Display.
//!
//! A local event monitor takes the NSEvents themselves rather than GPUI's
//! versions of them: those drop scroll and momentum phases, turn a
//! Control-click into a right click, and keep only five modifier flags.
//! Events outside the image, and every event while the Dismissal sheet is
//! up, go on to GPUI.

use super::click_diagnostics::{self as diagnostics, Click, Event as ClickEvent, Phase};
use super::input::{Button, Forwarder};
use super::native::{self, Native};
use super::{cg, menubar_height};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventPhase, NSEventType, NSWindow};
use objc2_core_graphics::CGEventFlags;
use std::cell::RefCell;
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
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// Handling an event a test made: the Viewer must not take the keyboard.
    static INJECTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Except this one, which takes it as the user's click does.
    static TAKE_KEY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Tests: a click at a point of the display image, given as fractions of
/// its width and height, handled as if the user made it. With `key`, the
/// Viewer first gives up the keyboard and the click takes it back, as the
/// user's click on a Viewer behind another window does.
pub fn inject_click(x: f64, y: f64, key: bool) -> Result<(), String> {
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
/// with Shift and Command held as asked.
pub fn inject_key(code: u16, shift: bool, command: bool) -> Result<(), String> {
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
    let empty = objc2_foundation::NSString::new();
    for kind in [NSEventType::KeyDown, NSEventType::KeyUp] {
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            window.windowNumber(),
            None,
            &empty,
            &empty,
            false,
            code,
        )
        .ok_or("no event")?;
        inject(&event)?;
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
    });
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
    let handled = handle_in(event, mtm, &mut take_key);
    // Outside the state: a window becoming key draws at once, and its
    // paint reports the layout here.
    if let Some(window) = take_key {
        native::make_key(&window);
    }
    handled
}

fn handle_in(
    event: &NSEvent,
    mtm: MainThreadMarker,
    take_key: &mut Option<objc2::rc::Retained<NSWindow>>,
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
                if !s.sent_up.remove(&code) {
                    forwarder.key(code, false, flags(event), cg::display_bounds(display));
                }
                true
            }
            NSEventType::FlagsChanged => {
                forwarder.flags_changed(event.keyCode(), flags(event));
                true
            }
            _ => false,
        }
    })
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
