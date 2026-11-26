//! Test-only control methods (`debugTakeover`, `debugShow`, ...). Enabled
//! only in debug builds or with `AGENTSCREEN_DEBUG=1` (for measuring a
//! release build).

use super::agent::{Args, display, usable};
use super::off;
use super::server::ControlError;
use super::shared_app;
use crate::core::Core;
use crate::mac::{ax, cg, devtools, windows};
use crate::model::{DisplayId, WindowTarget};
use agent_screen_model::held::{Held, Place};
use agent_screen_model::request::{Kind, NamedWindow, RequestId};
use gpui::{AsyncApp, Entity};
use serde_json::{Map, Value, json};
use std::time::Duration;

/// Test hooks: in debug builds, or with `AGENTSCREEN_DEBUG=1` for measuring a
/// release build.
pub(crate) fn enabled() -> bool {
    cfg!(debug_assertions) || std::env::var_os("AGENTSCREEN_DEBUG").is_some_and(|v| v == "1")
}

/// Handle a `debug*` control method when debug methods are enabled. Returns
/// `None` if debug methods are off or `method` is not a debug method.
pub(super) async fn handle(
    core: &Entity<Core>,
    connection: u64,
    method: &str,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Option<Result<Value, ControlError>> {
    if !enabled() {
        return None;
    }
    let result = match method {
        // Test hook: stands in for the user's input in the Viewer.
        "debugTakeover" => {
            let id = match display(core, p, false, cx) {
                Ok(id) => id,
                Err(e) => return Some(Err(e)),
            };
            let active = p.flag("active");
            core.update(cx, |c, cx| {
                if active {
                    c.take_over(id, cx)
                } else {
                    c.hand_back(id, cx)
                }
            });
            Ok(json!({}))
        }
        // Move any window, by pid and id, to a global point: a test puts a
        // system alert on an agent display with it.
        "debugMoveWindow" => {
            let target = WindowTarget {
                pid: p.int("pid").unwrap_or(0) as i32,
                window_id: p.int("windowId").unwrap_or(0) as u32,
            };
            let (x, y) = (p.number("x").unwrap_or(0.0), p.number("y").unwrap_or(0.0));
            let moved = off(cx, move || {
                ax::window_element(target).is_some_and(|w| ax::set_position(&w, x, y))
            })
            .await;
            Ok(json!({ "moved": moved }))
        }
        "debugViewer" => debug_viewer(cx),
        "debugStreams" => {
            // Each display's capture stream: running, showing another
            // screen, frames delivered, restarts.
            let streams = cx.update(|cx| {
                crate::ui::stream::global()
                    .map(|streams| streams.read(cx).debug())
                    .unwrap_or_default()
            });
            Ok(json!({ "streams": streams }))
        }
        "debugWebPreview" => Ok(crate::web::debug_status()),
        // The latest display change's regather (#291): its generation and
        // whether it still runs, so a test can wait for it to settle.
        "debugRegather" => {
            let status = cx.update(|cx| {
                crate::ui::shell::current()
                    .and_then(|shell| shell.upgrade())
                    .map(|shell| shell.read(cx).regather_status())
            });
            let (generation, running) = status.unwrap_or((0, false));
            Ok(json!({ "generation": generation, "running": running }))
        }
        "debugViewerInput" => debug_viewer_input(p, cx).await,
        // Test hook: the front app's focused window reads as unknown, as
        // when the app doesn't answer accessibility in time.
        "debugFocusUnknown" => {
            if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
                return Some(Err(ControlError::invalid(
                    "debugFocusUnknown requires the isolated e2e instance",
                )));
            }
            let on = p.bool("on").unwrap_or(true);
            shared_app::force_focus_unknown(on);
            Ok(json!({ "on": on }))
        }
        // Test hook: how long a call waits for an action the user's front
        // app blocks before it answers input_pending; 0 restores 30 s.
        "debugFrontWait" => {
            if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
                return Some(Err(ControlError::invalid(
                    "debugFrontWait requires the isolated e2e instance",
                )));
            }
            let ms = p.int("ms").unwrap_or(0).max(0) as u64;
            super::front_queue::set_wait_for_tests(ms);
            // The waiting line shows as soon as the call answers.
            core.update(cx, |c, cx| {
                c.input_queue.set_line_ms_for_tests(ms);
                cx.notify();
            });
            Ok(json!({ "ms": ms }))
        }
        // Test hook: what the last hold of the user's keys during a launch
        // did (#110): how many events and key downs, how long, and why it
        // ended. Counts only, never which keys.
        "debugKeyHold" => Ok(match crate::mac::key_hold::last() {
            Some(s) => json!({
                "events": s.events,
                "keyDowns": s.key_downs,
                "ms": s.ms,
                "end": format!("{:?}", s.end),
                "sent": s.sent,
                "discarded": s.discarded,
            }),
            None => json!({ "events": null }),
        }),
        // Test hook (#222): an app's AXManualAccessibility and
        // AXEnhancedUserInterface, with `on` set to turn the latter on or
        // off first; with `windowId`, each element of that window's page
        // with the actions it lists.
        "debugEnhancedUi" => {
            if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
                return Some(Err(ControlError::invalid(
                    "debugEnhancedUi requires the isolated e2e instance",
                )));
            }
            let pid = p.int("pid").unwrap_or(0) as i32;
            let on = p.bool("on");
            let window = p.int("windowId").map(|w| WindowTarget {
                pid,
                window_id: w as u32,
            });
            Ok(off(cx, move || {
                let answered = on.map(|on| ax::set_enhanced_ui(pid, on));
                let elements: Vec<Value> = window
                    .map(|w| ax::web_actions(w, 400))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(role, label, actions)| {
                        json!({ "role": role, "label": label, "actions": actions })
                    })
                    .collect();
                json!({
                    "answered": answered,
                    "manual": ax::app_flag(pid, "AXManualAccessibility"),
                    "enhanced": ax::app_flag(pid, "AXEnhancedUserInterface"),
                    "elements": elements,
                })
            })
            .await)
        }
        // Test hook: turn the check for a press's effect beside the user's
        // window off or on, to time what it costs.
        "debugPressCheck" => {
            let on = p.bool("on").unwrap_or(true);
            shared_app::set_press_check(on);
            Ok(json!({ "on": on }))
        }
        // Test hook: the user hands a window over from the Give menu.
        "debugHandOver" => debug_hand_over(core, p, cx),
        // Test hook: the user answers the display's Window Request, as the
        // card's buttons do, once one is open.
        "debugAnswerWindow" => debug_answer_window(core, p, cx).await,
        // Test hook: the user answers the display's Help Request, as the
        // card's buttons or the notification's actions do. `answer` is
        // continue, reply (with `text`), later or takeOver; `via`
        // notification takes the notification's path, for the request
        // `requestId` when given, as a notification posted for it carries.
        "debugAnswerHelp" => debug_answer_help(core, p, cx),
        // Test hook: the user takes a window back from the Take Back menu.
        "debugTakeBack" => debug_take_back(core, p, cx).await,
        // Test hook: the session that holds the user stand-in stage gives
        // up a window it launched there, so the window counts as the user's
        // own, as on a real screen of theirs. The claim stays, so AeroSpace
        // leaves the window on the stage.
        #[cfg(debug_assertions)]
        "debugUserWindow" => debug_user_window(core, connection, p, cx).await,
        // Test hook: which agent the setup window says loaded the tools.
        "debugAgentReady" => {
            let shell = match crate::ui::shell::current() {
                Some(shell) => shell,
                None => return Some(Err(ControlError::failed("no UI"))),
            };
            let agent = match shell
                .read_with(cx, |shell, _| shell.last_agent_ready())
                .map_err(|e| ControlError::failed(e.to_string()))
            {
                Ok(agent) => agent,
                Err(e) => return Some(Err(e)),
            };
            Ok(json!({ "agent": agent }))
        }
        // Test hook: only an Input Loan's focus step on `windowId`, which
        // takes the front app and gives it back, watched for `watchMs`.
        "debugLoanFocus" => {
            let Some(window) = p.int("windowId").and_then(|w| windows::find(w as u32)) else {
                return Some(Err(ControlError::not_found("no window with that id")));
            };
            let target = window.target;
            let watch = Duration::from_millis(p.int("watchMs").unwrap_or(2000) as u64);
            Ok(off(cx, move || {
                crate::mac::real_input::debug_forward(target, watch)
            })
            .await)
        }
        // Test hook: macOS last named `pid` the front app, as when the user
        // had just used it, without making it active.
        "debugFront" => {
            crate::mac::input::note_activation(p.int("pid").map(|pid| pid as i32));
            Ok(json!({}))
        }
        // Test hook: the user brought `pid` to the front, as from the Dock:
        // the displays the reminder of where its windows are named.
        "debugActivated" => debug_activated(p, cx).await,
        // Test hooks for updates: check the feed now without a window, and
        // what is waiting to be installed.
        "debugCheckForUpdates" => {
            crate::mac::update::check_in_background();
            Ok(json!({ "available": crate::mac::update::available() }))
        }
        "debugUpdateState" => Ok(json!({
            "available": crate::mac::update::available(),
            "pending": crate::mac::update::pending(),
            // What Install and Relaunch in Sparkle's window reads.
            "openDisplays": crate::mac::update::open_displays(),
        })),
        // Test hook: the user's left click at a global point, as the global
        // monitor reports it, such as on an app's Dock icon.
        "debugUserClick" => debug_user_click(p, cx).await,
        // Test hook: what Agent Screen would do now if the agents' browser
        // instance `pid` came to the front by another app, from the user's
        // latest input, without doing it (ADR 0022).
        "debugAgentsActivated" => {
            let pid = p.int("pid").unwrap_or(0) as i32;
            let agents = devtools::is_agents(pid);
            let decided = off(cx, move || devtools::on_agents_activated(pid)).await;
            Ok(json!({ "agents": agents, "decision": format!("{decided:?}") }))
        }
        // Test hook: a DevTools command through Agent Screen's pipe to
        // `pid`, in the page `target` when given, for verifying a browser
        // (ADR 0022).
        "debugDevtools" => {
            let pid = p.int("pid").unwrap_or(0) as i32;
            let method = p.string("method").unwrap_or_default();
            let params = p.0.get("params").cloned().unwrap_or(json!({}));
            let target = p.string("target");
            off(cx, move || {
                let channel = devtools::channel(pid, Duration::from_secs(2))
                    .ok_or_else(|| format!("pid {pid} has no pipe"))?;
                channel.raw(&method, params, target.as_deref())
            })
            .await
            .map(|result| json!({ "result": result }))
            .map_err(ControlError::failed)
        }
        // Test hook: a key press of the user's that went to `pid`, as the
        // global key monitor reports one typed into Spotlight.
        "debugUserKey" => {
            let pid = p.int("pid").unwrap_or(0) as i32;
            off(cx, move || crate::mac::catch::user_key_to(pid)).await;
            Ok(json!({}))
        }
        // A panic on the main thread after the reply, as one in an AppKit
        // callback: the app aborts.
        "debugPanic" => {
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                panic!("debugPanic");
            })
            .detach();
            Ok(json!({}))
        }
        "debugQuit" => {
            // Quits the way the menu does, after this reply goes out.
            cx.spawn(async |cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
            Ok(json!({}))
        }
        // Test hook: Move to Applications from `from` into the folder `to`,
        // as the banner's button runs it, without opening the copy. The
        // e2e instance only, and never into an Applications folder.
        "debugMoveApp" => {
            let from = std::path::PathBuf::from(p.string("from").unwrap_or_default());
            let to = std::path::PathBuf::from(p.string("to").unwrap_or_default());
            let replace = p.0.get("replace").and_then(Value::as_bool).unwrap_or(false);
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            let applications = to.starts_with("/Applications")
                || home.is_some_and(|h| to.starts_with(h.join("Applications")));
            if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e")
                || applications
                || !to.is_absolute()
                || !from.is_absolute()
            {
                return Some(Err(ControlError::invalid(
                    "debugMoveApp needs the e2e instance and absolute paths outside Applications",
                )));
            }
            use crate::mac::relocate::{MoveError, move_to};
            Ok(match off(cx, move || move_to(&from, &to, replace)).await {
                Ok(dest) => json!({ "moved": dest.display().to_string() }),
                Err(MoveError::Exists(dest)) => json!({ "exists": dest.display().to_string() }),
                Err(MoveError::Failed(error)) => json!({ "error": error }),
            })
        }
        // Test hook: Reset Permissions' helper for `bundleId` and
        // `bundlePath`, waiting for `pid`, with the stand-ins the e2e
        // instance names in AGENTSCREEN_TCCUTIL and AGENTSCREEN_OPEN. The
        // app doesn't quit. Refused without both stand-ins.
        "debugResetHelper" => {
            use crate::mac::permissions::{reset_commands, reset_tools, spawn_reset};
            let stand_ins = std::env::var_os("AGENTSCREEN_TCCUTIL").is_some()
                && std::env::var_os("AGENTSCREEN_OPEN").is_some();
            if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") || !stand_ins {
                return Some(Err(ControlError::invalid(
                    "debugResetHelper needs the e2e instance with AGENTSCREEN_TCCUTIL and AGENTSCREEN_OPEN",
                )));
            }
            let id = p.string("bundleId").unwrap_or_default();
            let bundle = std::path::PathBuf::from(p.string("bundlePath").unwrap_or_default());
            let pid = p.int("pid").unwrap_or(0) as u32;
            reset_tools()
                .and_then(|tools| {
                    reset_commands(&id, &bundle, pid, &tools).map_err(|e| e.to_string())
                })
                .and_then(|script| spawn_reset(&script).map(|()| script.steps))
                .map(|steps| json!({ "steps": steps }))
                .map_err(ControlError::invalid)
        }
        "debugInputObserver" => debug_input_observer(p, cx),
        // The Help Card's width and its buttons' bounds at the last paint.
        "debugCard" => debug_card(cx),
        // Which picture the menu bar icon shows: idle, working, asking or setup.
        "debugIcon" => debug_icon(cx),
        "debugShow" => debug_show(core, connection, p, cx).await,
        "debugGlow" => debug_glow(p, cx).await,
        _ => return None,
    };
    Some(result)
}

fn debug_viewer(cx: &mut AsyncApp) -> Result<Value, ControlError> {
    let (frame, image, key) = crate::mac::viewer_input::debug_layout()
        .ok_or_else(|| ControlError::not_found("the Viewer shows no display"))?;
    // Whether the shown display's stream showed another display.
    let misrouted = cx.update(|cx| {
        crate::ui::shell::current()
            .and_then(|shell| shell.upgrade())
            .and_then(|shell| shell.read(cx).viewer_display())
            .zip(crate::ui::stream::global())
            .is_some_and(|(id, streams)| streams.read(cx).misrouted(id))
    });
    // The header's Take Back button, in points from the Viewer's
    // top-left corner, as last painted.
    let take_back = crate::ui::viewer::WindowMenu::TakeBack.button().map(|b| {
        [
            f64::from(b.origin.x),
            f64::from(b.origin.y),
            f64::from(b.size.width),
            f64::from(b.size.height),
        ]
    });
    let let_continue = crate::ui::viewer::let_continue_button().map(|b| {
        [
            f64::from(b.origin.x),
            f64::from(b.origin.y),
            f64::from(b.size.width),
            f64::from(b.size.height),
        ]
    });
    Ok(json!({
        "frame": frame,
        "image": image,
        // The header's Let the agent continue button, likewise, while a
        // waiting line shows.
        "letContinue": let_continue,
        "key": key,
        // The input method's marked text in the Viewer, and the
        // target's caret, where it shows, in points from the
        // Viewer's top-left corner.
        "marked": crate::mac::viewer_input::marked(),
        // The tap that keeps presses in the Viewer from other apps
        // is installed and enabled (ADR 0014).
        "privatePress": crate::mac::viewer_input::private_press(),
        // The Viewer's input context is AppKit's current one: the
        // input method composes only then.
        "inputContext": crate::mac::viewer_input::input_context_current(),
        // The kind of input source the Viewer types with now.
        "source": match crate::mac::ime::current_kind() {
            agent_screen_model::viewer_keys::Source::Layout => "layout",
            agent_screen_model::viewer_keys::Source::InputMethod => "inputMethod",
        },
        "caret": crate::mac::viewer_input::caret_in_window(),
        // A password field has the keyboard in the target, as last
        // looked up, or secure input is on.
        "secure": crate::mac::viewer_input::secure(),
        // "unknown" until a lookup answers after the focus may
        // have moved; keys go as keys meanwhile.
        "secrecy": secrecy_name(crate::mac::viewer_input::secrecy()),
        // How long the latest focus lookups took, and how long
        // held keys waited for them, in milliseconds.
        "lookupMs": crate::mac::input::lookup_ms(),
        "heldMs": crate::mac::viewer_input::held_ms(),
        // Each key press's final route, by key code, in order, and
        // how many compositions a lookup dropped for a password
        // field, which a lookup in time leaves at none.
        "routes": crate::mac::viewer_input::routes()
            .into_iter()
            .map(|(code, route)| json!([code, route]))
            .collect::<Vec<_>>(),
        "secureFound": crate::mac::viewer_input::secure_found_count(),
        // Lookups of the target's focus asked for, none with a
        // keyboard layout, and any made on the main thread.
        "inspections": crate::mac::viewer_input::inspections(),
        "mainThreadLookups": crate::mac::ax::MAIN_THREAD_LOOKUPS
            .load(std::sync::atomic::Ordering::Relaxed),
        "misrouted": misrouted,
        "takeBack": take_back,
    }))
}

async fn debug_viewer_input(p: &Args<'_>, cx: &mut AsyncApp) -> Result<Value, ControlError> {
    use crate::mac::viewer_input;
    // A frame from the display's stream leaves the Viewer to draw,
    // as it nearly always is while the user works in it.
    if p.int("key") == Some(1) {
        cx.update(|cx| cx.refresh_windows());
    }
    let mut routed = None;
    let mut secrecy = None;
    // `source` stands in for the user's input source: "layout" or
    // "inputMethod".
    let source = {
        use agent_screen_model::viewer_keys::Source;
        match p.string("source").as_deref() {
            None => None,
            Some("layout") => Some(Source::Layout),
            Some("inputMethod") => Some(Source::InputMethod),
            Some(_) => {
                return Err(ControlError::failed("source is layout or inputMethod"));
            }
        }
    };
    match p.string("kind").as_deref() {
        Some("click") => viewer_input::inject_click(
            p.number("x").unwrap_or(0.5),
            p.number("y").unwrap_or(0.5),
            p.int("key").unwrap_or(0) == 1,
            source,
        ),
        // `characters` are what the key types, and `ime` what the
        // input method does with it: "pass", "nothing",
        // "mark:TEXT", "insert:TEXT" or "insertcommand:TEXT". The
        // reply says what the key came to.
        Some("key") => {
            let script = match p.string("ime") {
                None => None,
                Some(ime) => Some(viewer_input::Script::parse(&ime).ok_or_else(|| {
                    ControlError::failed(
                        "ime is pass, nothing, mark:TEXT, insert:TEXT or insertcommand:TEXT",
                    )
                })?),
            };
            // `phase`: "down" or "up" alone, else both; `repeat`:
            // the press is a held key's repeat.
            use objc2_app_kit::NSEventType;
            let kinds: &[NSEventType] = match p.string("phase").as_deref() {
                None => &[NSEventType::KeyDown, NSEventType::KeyUp],
                Some("down") => &[NSEventType::KeyDown],
                Some("up") => &[NSEventType::KeyUp],
                Some(_) => return Err(ControlError::failed("phase is down or up")),
            };
            secrecy = Some(secrecy_name(viewer_input::secrecy()));
            let characters = p.string("characters").unwrap_or_default();
            let key = viewer_input::TestKey {
                code: p.int("code").unwrap_or(0) as u16,
                shift: p.int("shift").unwrap_or(0) == 1,
                command: p.int("command").unwrap_or(0) == 1,
                characters: &characters,
                repeat: p.int("repeat").unwrap_or(0) == 1,
            };
            viewer_input::inject_key(key, source, script, kinds).map(|route| routed = Some(route))
        }
        // Shift pressed (`down`: 1) or released.
        Some("shift") => {
            secrecy = Some(secrecy_name(viewer_input::secrecy()));
            viewer_input::inject_shift(p.int("down").unwrap_or(0) == 1, source)
        }
        // A press, a drag in steps and a release, as a hand on
        // the trackpad makes them.
        Some("drag") => {
            use objc2_app_kit::NSEventType;
            let from = (p.number("x").unwrap_or(0.5), p.number("y").unwrap_or(0.5));
            let to = (
                p.number("toX").unwrap_or(0.5),
                p.number("toY").unwrap_or(0.5),
            );
            viewer_input::inject_mouse(NSEventType::LeftMouseDown, from.0, from.1)
                .map_err(ControlError::failed)?;
            for step in 1..=20 {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let t = step as f64 / 20.0;
                viewer_input::inject_mouse(
                    NSEventType::LeftMouseDragged,
                    from.0 + (to.0 - from.0) * t,
                    from.1 + (to.1 - from.1) * t,
                )
                .map_err(ControlError::failed)?;
            }
            viewer_input::inject_mouse(NSEventType::LeftMouseUp, to.0, to.1)
        }
        // The input method's calls on the Viewer's view while the
        // user composes: text marked, or the composition ended.
        Some(kind @ ("mark" | "commit")) => {
            // A frame paints the Viewer's input handler first.
            cx.update(|cx| cx.refresh_windows());
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            viewer_input::inject_text_input(&p.string("text").unwrap_or_default(), kind == "commit")
        }
        _ => Err("kind is click, key, shift, drag, mark or commit".into()),
    }
    .map_err(ControlError::failed)?;
    // `secrecy`: what the key was routed by.
    Ok(json!({ "route": routed, "secrecy": secrecy }))
}

fn debug_hand_over(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, false, cx)?;
    // As the Give menu does, unless `gave` is false: a drag.
    let gave = p.bool("gave") != Some(false);
    let window = p
        .int("windowId")
        .and_then(|w| windows::find(w as u32))
        .ok_or_else(|| ControlError::not_found("no window with that id"))?;
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    shell
        .update(cx, |shell, cx| {
            shell.hand_over(window.target, Some(window.bounds), id, gave, cx)
        })
        .map_err(|e| ControlError::failed(e.to_string()))?;
    Ok(json!({}))
}

async fn debug_answer_window(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, false, cx)?;
    let give = p.flag("give");
    for _ in 0..50 {
        let open = core.read_with(cx, |c, _| {
            c.displays
                .get(id)
                .is_some_and(|d| d.requests.is_open(Kind::Window))
        });
        if open {
            break;
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let answered = shell
        .update(cx, |shell, cx| shell.answer_window(id, give, cx))
        .map_err(|e| ControlError::failed(e.to_string()))?;
    Ok(json!({ "answered": answered }))
}

fn debug_answer_help(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, false, cx)?;
    let answer = p.string("answer").unwrap_or_default();
    let text = p.string("text").unwrap_or_default();
    let via_notification = p.string("via").as_deref() == Some("notification");
    let request = p.int("requestId").map(|r| r as RequestId);
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    // Typed into the card's field, then Send: the card tells the
    // Shell, as a click does, so the Shell isn't updating here.
    if answer == "reply" && !via_notification {
        let card = shell
            .read_with(cx, |shell, _| shell.card_window())
            .map_err(|e| ControlError::failed(e.to_string()))?
            .ok_or_else(|| ControlError::failed("no Help Card"))?;
        let sent = card
            .update(cx, |card, window, cx| {
                card.debug_reply(id, &text, window, cx)
            })
            .map_err(|e| ControlError::failed(e.to_string()))?;
        return Ok(json!({ "answered": sent }));
    }
    let answered = shell
        .update(cx, |shell, cx| {
            shell.debug_answer_help(id, &answer, text, via_notification, request, cx)
        })
        .map_err(|e| ControlError::failed(e.to_string()))?
        .map_err(ControlError::invalid)?;
    Ok(json!({ "answered": answered }))
}

async fn debug_take_back(
    core: &Entity<Core>,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = display(core, p, false, cx)?;
    let target = p
        .int("windowId")
        .and_then(|w| windows::find(w as u32))
        .ok_or_else(|| ControlError::not_found("no window with that id"))?
        .target;
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let left = shell
        .update(cx, |shell, cx| shell.take_back(target, id, cx))
        .map_err(|e| ControlError::failed(e.to_string()))?
        .await;
    cx.background_executor()
        .timer(Duration::from_millis(500))
        .await;
    Ok(json!({ "left": left }))
}

#[cfg(debug_assertions)]
async fn debug_user_window(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let id = p.int("displayId").unwrap_or(0) as DisplayId;
    usable(core, connection, p, id, "debugUserWindow", cx)?;
    if !cg::window_display_role(id).accepts_window_request() {
        return Err(ControlError::invalid(
            "debugUserWindow needs this session's user stand-in stage (debugShow userStage)",
        ));
    }
    let window = p
        .int("windowId")
        .ok_or_else(|| ControlError::invalid("windowId is required"))? as u32;
    let here = off(cx, move || {
        windows::on_display(id)
            .iter()
            .any(|w| w.target.window_id == window)
    })
    .await;
    if !here {
        return Err(ControlError::not_found(
            "no window with that id on the stage",
        ));
    }
    // Only a window the claim file holds on the stage: AeroSpace
    // would take any other for the user's workspace.
    let claimed = off(cx, move || crate::mac::claims::claim_confirmed(window, id)).await;
    if !claimed {
        return Err(ControlError::failed(
            "the window's claim for the stage wasn't published",
        ));
    }
    core.update(cx, |c, _| {
        c.displays.forget_on(id, window);
        if let Some(d) = c.displays.get_mut(id) {
            d.user_made.insert(window);
        }
    });
    Ok(json!({}))
}

async fn debug_activated(p: &Args<'_>, cx: &mut AsyncApp) -> Result<Value, ControlError> {
    let pid = p.int("pid").unwrap_or(0) as i32;
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let task = shell
        .update(cx, |shell, cx| shell.remind_where(pid, false, cx))
        .map_err(|e| ControlError::failed(e.to_string()))?;
    let reminded = task.await;
    let peek = shell
        .update(cx, |shell, cx| shell.reminder(cx))
        .ok()
        .flatten();
    Ok(json!({ "reminded": reminded, "peek": peek }))
}

async fn debug_user_click(p: &Args<'_>, cx: &mut AsyncApp) -> Result<Value, ControlError> {
    let at = (p.require("x")?, p.require("y")?);
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let task = shell
        .update(cx, |shell, cx| shell.user_clicked(at, cx))
        .map_err(|e| ControlError::failed(e.to_string()))?;
    let reminded = task.await;
    let peek = shell
        .update(cx, |shell, cx| shell.reminder(cx))
        .ok()
        .flatten();
    Ok(json!({ "reminded": reminded, "peek": peek }))
}

fn debug_input_observer(p: &Args<'_>, cx: &mut AsyncApp) -> Result<Value, ControlError> {
    if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
        return Err(ControlError::invalid(
            "input observer tests require the isolated e2e instance",
        ));
    }
    let timeout = match p.string("action").as_deref().unwrap_or("status") {
        "status" => false,
        "timeout" => true,
        _ => return Err(ControlError::invalid("action must be status or timeout")),
    };
    Ok(cx.update(|_| {
        if timeout {
            crate::mac::real_input::debug_observer_timeout();
        }
        json!({
            "watching": crate::mac::real_input::watching(),
            "enabled": crate::mac::real_input::observer_enabled(),
        })
    }))
}

fn debug_card(cx: &mut AsyncApp) -> Result<Value, ControlError> {
    let (width, buttons) = crate::ui::help_card::painted_buttons();
    let buttons: Map<String, Value> = buttons
        .into_iter()
        .map(|(id, r)| (id.to_string(), json!(r)))
        .collect();
    // The reply field, and whether the card has the keyboard.
    let state = match crate::ui::shell::current()
        .and_then(|shell| shell.read_with(cx, |shell, _| shell.card_window()).ok())
        .flatten()
    {
        Some(card) => card
            .update(cx, |card, window, cx| card.debug_state(window, cx))
            .unwrap_or(Value::Null),
        None => Value::Null,
    };
    let report = crate::ui::shell::current()
        .and_then(|shell| shell.read_with(cx, |shell, _| shell.debug_reports()).ok())
        .unwrap_or(Value::Null);
    let spaces = crate::ui::shell::current()
        .and_then(|shell| shell.read_with(cx, |shell, _| shell.debug_spaces()).ok())
        .unwrap_or(Value::Null);
    Ok(json!({
        "report": report,
        "spaces": spaces,
        "width": width,
        "buttons": buttons,
        "queueHeight": crate::ui::help_card::queue_height(),
        "key": state.get("key"),
        "cardKeys": state.get("cardKeys"),
        "highlighted": state.get("highlighted"),
        "reply": state.get("reply"),
    }))
}

fn debug_icon(cx: &mut AsyncApp) -> Result<Value, ControlError> {
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let (state, asking_count) = shell
        .update(cx, |shell, _| (shell.icon_state(), shell.asking_count()))
        .map_err(|e| ControlError::failed(e.to_string()))?;
    Ok(json!({
        "state": state,
        "askingCount": asking_count,
    }))
}

async fn debug_show(
    core: &Entity<Core>,
    connection: u64,
    p: &Args<'_>,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    let surface = p.string("surface").unwrap_or_else(|| "viewer".into());
    let id = p.int("displayId").unwrap_or(0) as DisplayId;
    // They show on the user's real screens.
    if matches!(
        surface.as_str(),
        "glow-live" | "glow-armed" | "glow-off" | "input-bar-keyboard"
    ) && agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e")
    {
        return Err(ControlError::invalid(
            "glow and Input Bar previews require the isolated e2e instance",
        ));
    }
    #[cfg(debug_assertions)]
    if surface == "userStage" {
        usable(core, connection, p, id, "debugShow", cx)?;
        let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
        let selected = shell
            .update(cx, |shell, _| shell.stage_display() == Some(id))
            .unwrap_or(false);
        let held = core.read_with(cx, |c, _| c.displays.get(id).is_some());
        if !held || !selected || !cg::is_agent_screen(id) || !cg::is_online(id) {
            return Err(ControlError::invalid(
                "userStage requires this session's selected live stage display",
            ));
        }
        cg::debug_user_stage(id, connection);
        return Ok(json!({}));
    }
    // An action that has waited 30 s for the user to leave an app, as
    // its display's card and the Viewer header say. It never runs.
    if surface == "waiting-line" {
        usable(core, connection, p, id, "debugShow", cx)?;
        let app = p.string("app").unwrap_or_else(|| "Dia".into());
        core.update(cx, |c, cx| -> Result<(), ControlError> {
            let owner = c
                .input_queue
                .owner(id, connection)
                .map_err(ControlError::invalid)?;
            c.input_queue
                .preview_waiting(owner, app)
                .map_err(ControlError::invalid)?;
            cx.notify();
            Ok(())
        })?;
        return Ok(json!({}));
    }
    if matches!(
        surface.as_str(),
        "input-queue"
            | "input-queue-approved"
            | "input-queue-drags"
            | "input-queue-running"
            | "input-queue-click"
            | "input-queue-menu"
            | "input-queue-allow"
            | "input-queue-decline"
            | "input-bar-stop"
    ) {
        if agent_screen_mcp::endpoint::instance().as_deref() != Some("e2e") {
            return Err(ControlError::invalid(
                "input queue previews require the isolated e2e instance",
            ));
        }
        usable(core, connection, p, id, "debugShow", cx)?;
        // A new preview's steps replace an earlier preview's.
        if !matches!(
            surface.as_str(),
            "input-queue-allow" | "input-queue-decline" | "input-bar-stop"
        ) {
            crate::ui::help_card::preview_steps(Vec::new());
        }
    }
    // The card for a menu item an Electron app needs a real click
    // for. Its call never waits, so the request never runs.
    if surface == "input-queue-menu" {
        core.update(cx, |c, cx| -> Result<(), ControlError> {
            let owner = c
                .input_queue
                .owner(id, connection)
                .map_err(ControlError::invalid)?;
            let item = crate::input_queue::MenuItem {
                target: WindowTarget {
                    pid: 0,
                    window_id: 0,
                },
                area: cg::display_bounds(id),
                app: p.string("app").unwrap_or_else(|| "Slack".into()),
                path: vec![(4, "Go".into()), (1, "Back".into())],
            };
            let request = c
                .input_queue
                .submit(
                    owner,
                    format!("preview-menu:{id}"),
                    "Preview a menu item that needs a real click".into(),
                    Some(crate::input_queue::Plan::MenuItem(item)),
                )
                .map_err(ControlError::invalid)?;
            c.input_queue.detach(request);
            cx.notify();
            Ok(())
        })?;
    }
    // Drags across a board on the display, `count` of them (1 to
    // 3, default 3): one request, whose call never waits, so it
    // never runs, even allowed, and the rest shown after it, then
    // a click with `presses` when given. `input-queue-running`
    // allows it and shows the second as running.
    if matches!(
        surface.as_str(),
        "input-queue-drags" | "input-queue-running"
    ) {
        let count = p.int("count").unwrap_or(3).clamp(1, 3) as usize;
        core.update(cx, |c, cx| -> Result<(), ControlError> {
            let owner = c
                .input_queue
                .owner(id, connection)
                .map_err(ControlError::invalid)?;
            let area = cg::display_bounds(id);
            // The design's paths, on a 1280 by 800 board.
            let paths = [
                ((450.0, 179.0), (1094.0, 320.0)),
                ((450.0, 269.0), (772.0, 232.0)),
                ((260.0, 500.0), (340.0, 500.0)),
            ];
            let at = |(x, y): (f64, f64)| {
                (
                    area.x + x / 1280.0 * area.width,
                    area.y + y / 800.0 * area.height,
                )
            };
            let drags = paths.into_iter().take(count).map(|(from, to)| {
                crate::input_queue::Plan::Drag(crate::input_queue::Drag {
                    target: WindowTarget {
                        pid: 0,
                        window_id: 0,
                    },
                    frame: area,
                    drop: WindowTarget {
                        pid: 0,
                        window_id: 0,
                    },
                    drop_frame: area,
                    area,
                    from: at(from),
                    to: at(to),
                    flags: 0,
                })
            });
            let click = p.int("presses").map(|presses| {
                crate::input_queue::Plan::Click(preview_click(
                    area,
                    presses,
                    p,
                    c.input_queue.now(),
                ))
            });
            let mut plans = drags.chain(click);
            // A session waits on one request at a time, so the
            // first is a real request and the rest only show.
            let first = plans.next().expect("one preview drag");
            let request = c
                .input_queue
                .submit(
                    owner,
                    format!("preview-drag:{id}"),
                    "Preview a drag that needs the real pointer".into(),
                    Some(first),
                )
                .map_err(ControlError::invalid)?;
            c.input_queue.detach(request);
            crate::ui::help_card::preview_steps(plans.map(|plan| (id, plan)).collect());
            cx.notify();
            Ok(())
        })?;
    }
    // A click with `presses` (1 to 3, default 1) in `app`'s window
    // `title` (default WeCom, Chats), on a chat list's row. Its
    // call never waits, so it never runs, even allowed.
    if surface == "input-queue-click" {
        let presses = p.int("presses").unwrap_or(1);
        core.update(cx, |c, cx| -> Result<(), ControlError> {
            let owner = c
                .input_queue
                .owner(id, connection)
                .map_err(ControlError::invalid)?;
            let plan = preview_click(cg::display_bounds(id), presses, p, c.input_queue.now());
            let request = c
                .input_queue
                .submit(
                    owner,
                    format!("preview-click:{id}"),
                    "Preview a click that needs the real pointer".into(),
                    Some(crate::input_queue::Plan::Click(plan)),
                )
                .map_err(ControlError::invalid)?;
            c.input_queue.detach(request);
            cx.notify();
            Ok(())
        })?;
    }
    // Allow itself, through the shell, comes after this.
    if matches!(surface.as_str(), "input-queue" | "input-queue-approved") {
        core.update(cx, |c, cx| -> Result<(), ControlError> {
            let owner = c
                .input_queue
                .owner(id, connection)
                .map_err(ControlError::invalid)?;
            c.input_queue
                .submit(
                    owner,
                    format!("preview:{id}"),
                    "Preview a queued drag on this display".into(),
                    None,
                )
                .map_err(ControlError::invalid)?;
            cx.notify();
            Ok(())
        })?;
    }
    // A Window Request for no real window, so scripts can look at
    // the card with any `app` and `title`. Keep answers it.
    // `holder`, a Display Label, shows the card for a window another
    // session holds; `place` review or screen says it is on a Review
    // Display or the user's screen.
    if surface == "window-request" {
        let app = p.string("app").unwrap_or_else(|| "TextEdit".into());
        let title = p.string("title").unwrap_or_default();
        let target = WindowTarget {
            pid: 0,
            window_id: 0,
        };
        let held = p.string("holder").map(|label| Held {
            display: 0,
            agent: label.split(" · ").next().unwrap_or_default().to_string(),
            label,
            place: match p.string("place").as_deref() {
                Some("review") => Place::Review,
                Some("screen") => Place::UserScreen,
                _ => Place::Display,
            },
            status: None,
        });
        let _answer = core
            .update(cx, |c, cx| {
                let rx = c
                    .displays
                    .request_window(id, NamedWindow { target, app, title }, held);
                cx.notify();
                rx
            })
            .ok_or_else(|| ControlError::failed(format!("display {id} has ended")))?;
        cx.background_executor()
            .timer(Duration::from_millis(900))
            .await;
        return Ok(json!({}));
    }
    // `x` and `y`, in global top-left points, open a Give or Take
    // Back menu there instead of at the pointer.
    let menu_at = p.number("x").zip(p.number("y"));
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    shell
        .update(cx, |shell, cx| {
            shell.debug_menu_at = menu_at;
            let shown = shell.debug_show(&surface, id, cx);
            // A surface that opened no menu leaves no point for
            // the next real one.
            shell.debug_menu_at = None;
            shown
        })
        .map_err(|e| ControlError::failed(e.to_string()))?
        .map_err(ControlError::invalid)?;
    cx.background_executor()
        .timer(Duration::from_millis(900))
        .await;
    Ok(json!({}))
}

/// The Red Glow as it is: its state and preview, and per user screen its
/// window and, unless `alpha` is false, the alpha `glow::CONTRACT` is
/// read at, from a capture of that window alone. Live is read at the top
/// of its breath: the breath stops for the capture.
async fn debug_glow(p: &Args<'_>, cx: &mut AsyncApp) -> Result<Value, ControlError> {
    use crate::mac::native;
    use agent_screen_model::glow::Glow;
    let shell = crate::ui::shell::current().ok_or_else(|| ControlError::failed("no UI"))?;
    let (state, preview, stop, restore) = shell
        .update(cx, |shell, _| shell.glow_debug())
        .map_err(|e| ControlError::failed(e.to_string()))?;
    let measure = p.bool("alpha").unwrap_or(true);
    let probes = native::glow_probe();
    let held = measure && state == Glow::Live;
    if held {
        native::glow_hold_breath(true);
        // A frame for the held breath to reach the screen.
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
    let shots: Vec<(u32, usize, usize)> = probes
        .iter()
        .filter(|g| measure && g.visible)
        .map(|g| {
            (
                g.window,
                (g.frame.width * g.backing).round() as usize,
                (g.frame.height * g.backing).round() as usize,
            )
        })
        .collect();
    let captured = off(cx, move || {
        shots
            .into_iter()
            .map(|(window, w, h)| {
                (
                    window,
                    crate::mac::screenshot::window_alpha(window, (w, h)).map(|a| (w, h, a)),
                )
            })
            .collect::<Vec<_>>()
    })
    .await;
    if held {
        native::glow_hold_breath(false);
    }
    // Where each window lies, by the window server's displays: the main
    // screen's top-left is the origin, y down.
    let top = cg::display_bounds(cg::main_display()).height;
    let elsewhere = |g: &native::GlowProbe| {
        let middle = (
            g.frame.x + g.frame.width / 2.0,
            top - g.frame.y - g.frame.height / 2.0,
        );
        cg::active_displays().into_iter().any(|id| {
            (cg::is_agent_screen(id) || cg::is_closet(id))
                && cg::display_bounds(id).contains(middle)
        })
    };
    let on_agent_displays = probes.iter().filter(|g| elsewhere(g)).count();
    let screens: Vec<Value> = probes
        .iter()
        .map(|g| {
            let alpha = captured
                .iter()
                .find(|(w, _)| *w == g.window)
                .map(|(_, shot)| match shot {
                    Ok((w, h, a)) => {
                        let read = |(x, y): (f64, f64)| {
                            let px = ((x * g.backing) as usize).min(w - 1);
                            let py = ((y * g.backing) as usize).min(h - 1);
                            (f64::from(a[py * w + px]) / 255.0 * 1000.0).round() / 1000.0
                        };
                        let mut places = Map::new();
                        for (place, d, at) in &g.samples {
                            if *place == "centre" {
                                places.insert("centre".into(), json!(read(*at)));
                                continue;
                            }
                            places
                                .entry(place.to_string())
                                .or_insert_with(|| json!({}))
                                .as_object_mut()
                                .expect("an object")
                                .insert(format!("{d:.0}"), json!(read(*at)));
                        }
                        Value::Object(places)
                    }
                    Err(e) => json!({ "error": e }),
                });
            json!({
                "displayId": g.display,
                "frame": [g.frame.x, g.frame.y, g.frame.width, g.frame.height],
                "visible": g.visible,
                "ignoresMouse": g.ignores_mouse,
                "level": g.level,
                "scale": (g.scale * 10000.0).round() / 10000.0,
                "alpha": alpha,
            })
        })
        .collect();
    Ok(json!({
        "state": state.name(),
        "preview": preview.map(|g| g.name()),
        "screens": screens,
        "onAgentDisplays": on_agent_displays,
        "rasterized": !probes.is_empty() && probes.iter().all(|g| g.rasterized),
        "animations": probes.first().map(|g| g.animations.clone()).unwrap_or_default(),
        "reduceMotion": objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion(),
        "stopToOffMs": stop.map(|d| d.as_secs_f64() * 1000.0),
        "restoreMs": restore.map(|d| d.as_secs_f64() * 1000.0),
    }))
}

/// A preview click with `presses` (1 to 3) in `app`'s window `title`
/// (default WeCom, Chats), on a chat list's row of a display at `area`.
fn preview_click(
    area: crate::model::Rect,
    presses: i64,
    p: &Args<'_>,
    now: u64,
) -> crate::input_queue::Click {
    let click = agent_screen_model::real_click::Click::new(
        agent_screen_model::real_click::Button::Left,
        presses.clamp(1, 3),
    )
    .expect("one to three presses");
    crate::input_queue::Click {
        target: WindowTarget {
            pid: 0,
            window_id: 0,
        },
        frame: area,
        menu: None,
        ax_pid: 0,
        area,
        // A row of a sidebar, on a 1280 by 800 board.
        at: (
            area.x + 180.0 / 1280.0 * area.width,
            area.y + 236.0 / 800.0 * area.height,
        ),
        click,
        flags: 0,
        app: p.string("app").unwrap_or_else(|| "WeCom".into()),
        title: Some(p.string("title").unwrap_or_else(|| "Chats".into())),
        region: area,
        look: Vec::new(),
        element: None,
        asked_ms: now,
    }
}

/// What the Viewer knows of the target's focus, for debug replies.
fn secrecy_name(secrecy: agent_screen_model::viewer_keys::Secrecy) -> &'static str {
    use agent_screen_model::viewer_keys::Secrecy;
    match secrecy {
        Secrecy::Unknown => "unknown",
        Secrecy::Secure => "secure",
        Secrecy::Open => "open",
    }
}
