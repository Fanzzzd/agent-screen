//! Actions that only the user's front app blocks wait in the Input Queue
//! (ADR 0013) instead of failing (#219). When the user's window of the
//! target's app has focus, a posted click or key would reach their window
//! or take its focus, and some actions have no accessibility route there:
//! a key with no accessibility equivalent, a double or right click, a press
//! on a page element that ignores it. Such an action, refused before
//! anything was sent (`ControlError::front_blocked`), becomes a front
//! request. It needs no approval and never takes the user's pointer: once
//! the app is no longer in front, it runs through the usual background
//! route, while the agent's call waits. A call that waits too long answers
//! `input_pending`; the same call again, or `input_queue wait`, waits on.

use super::agent::{Args, display, replay, target};
use super::off;
use super::server::{ControlError, Reply};
use crate::core::Core;
use crate::input_queue::{Front, Look};
use crate::model::DisplayId;
use agent_screen_model::input_queue::{RequestId, Status};
use agent_screen_model::real_click;
use gpui::{AsyncApp, Entity};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How long one call waits for the user to leave the app before it
/// answers `input_pending`: well under the tool timeouts of the MCP
/// clients, of which Codex's 60 s default is the shortest.
const WAIT: Duration = Duration::from_secs(30);

/// `first`, the reply of a click, key or type call: unless the user's
/// front app refused it before anything was sent, as it is. Else the
/// action is queued, and this waits for it to run.
pub async fn queue_if_blocked(
    core: &Entity<Core>,
    connection: u64,
    method: &str,
    p: &Args<'_>,
    first: Reply,
    cx: &mut AsyncApp,
) -> Reply {
    let why = match &first {
        Err(e) if e.code == "front_blocked" => e.message.clone(),
        _ => {
            // Whatever it queued before no longer applies.
            if let Ok(id) = display(core, p, true, cx)
                && let Ok(owner) = core.read_with(cx, |c, _| c.input_queue.owner(id, connection))
            {
                core.update(cx, |c, cx| {
                    c.input_queue.replace_fronts(&owner);
                    cx.notify();
                });
            }
            return first;
        }
    };
    let refused = || ControlError::failed(why.clone());
    let Some((pid, app)) = super::shared_app::take_blocked() else {
        return Err(refused());
    };
    let id = display(core, p, true, cx).map_err(|_| refused())?;
    let Ok(owner) = core.read_with(cx, |c, _| c.input_queue.owner(id, connection)) else {
        return Err(refused());
    };
    let look = match method {
        "click" => look_at(core, p, cx).await,
        _ => None,
    };
    let params = Value::Object(p.0.clone());
    let operation = format!("{method} {params}");
    let reason = format!(
        "{} in {app} once the user's window there no longer has focus",
        verb(method)
    );
    let front = Front {
        method: method.into(),
        params,
        pid,
        app,
        look,
    };
    let request = core
        .update(cx, |c, cx| {
            let result = c.input_queue.submit_front(owner, operation, reason, front);
            cx.notify();
            result
        })
        .map_err(ControlError::failed)?;
    run(core, connection, id, request, &why, cx).await
}

/// What is under a click's point as it is queued: the window there, the
/// region around the point and its pixels, to compare before it runs.
async fn look_at(core: &Entity<Core>, p: &Args<'_>, cx: &mut AsyncApp) -> Option<Look> {
    let (id, at) = target(core, p, cx).await.ok()?;
    off(cx, move || {
        let (window, frame) = crate::mac::input::window_at(at)?;
        let area = crate::mac::cg::display_bounds(id);
        let region = real_click::look_region(at, area, frame, None);
        let samples = crate::mac::screenshot::samples(id, region).ok()?;
        Some(Look {
            display: id,
            at,
            window,
            region,
            samples,
        })
    })
    .await
}

/// Whether what was under the click's point when it was queued is still
/// there.
async fn look_kept(look: &Look, cx: &mut AsyncApp) -> bool {
    let look = look.clone();
    off(cx, move || {
        crate::mac::input::window_at(look.at).is_some_and(|(window, _)| window == look.window)
            && crate::mac::screenshot::samples(look.display, look.region)
                .is_ok_and(|now| real_click::looks_same(&look.samples, &now))
    })
    .await
}

fn verb(method: &str) -> &'static str {
    match method {
        "click" => "Click",
        "key" => "Keys",
        _ => "Typing",
    }
}

/// `input_queue wait` on front request `request`: wait for it to run, as
/// the call that queued it did.
pub async fn wait(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    request: RequestId,
    cx: &mut AsyncApp,
) -> Result<Value, ControlError> {
    core.update(cx, |c, _| c.input_queue.attach(request));
    let result = run(
        core,
        connection,
        id,
        request,
        "The action waits for the user's window of its app to lose focus",
        cx,
    )
    .await?;
    Ok(json!({
        "requests": [{ "requestId": request, "state": "completed" }],
        "result": result,
    }))
}

/// Wait up to `WAIT` for front request `request`'s app to leave the front,
/// then run it. `why` says what blocked it.
async fn run(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    request: RequestId,
    why: &str,
    cx: &mut AsyncApp,
) -> Reply {
    let started = Instant::now();
    loop {
        let (status, front, paused) = core.read_with(cx, |c, _| {
            (
                c.input_queue.queue.request(request).map(|r| r.status),
                c.input_queue.front(request).cloned(),
                c.displays.get(id).is_some_and(|d| d.takeover),
            )
        });
        let (Some(status), Some(front)) = (status, front) else {
            return Err(ControlError::not_found(format!(
                "input request {request} is gone; nothing was sent"
            )));
        };
        if status != Status::Queued {
            let how = match status {
                Status::Stopped(why) => format!("stopped ({why:?})"),
                Status::Completed => "already ran".into(),
                _ => "is running in another call".into(),
            };
            forget(core, request, cx);
            return Err(ControlError::failed(format!(
                "the queued {} (input request {request}) {how}; nothing more was sent. Take a screenshot before you act again.",
                verb(&front.method).to_lowercase()
            )));
        }
        if paused {
            detach(core, request, cx);
            return Err(ControlError::takeover(format!(
                "the user took over display {id}, so your queued {} (input request {request}) waits. Call wait_for_control, then take a screenshot before you act again.",
                verb(&front.method).to_lowercase()
            )));
        }
        if crate::mac::input::front() != Some(front.pid) {
            if let Some(look) = &front.look
                && !look_kept(look, cx).await
            {
                finish(core, request, Some(real_click::CONTENT_CHANGED.into()), cx);
                return Err(ControlError::failed(format!(
                    "The click (input request {request}) waited until the user's window of {} lost focus, but {}, so it was not sent. Take a screenshot and click again.",
                    front.app,
                    real_click::CONTENT_CHANGED
                )));
            }
            let started_front = core.update(cx, |c, cx| {
                let started = c.input_queue.start_front(request);
                cx.notify();
                started
            });
            if started_front {
                let params = front.params.as_object().cloned().unwrap_or_default();
                let result = replay(core, connection, &front.method, &Args(&params), cx).await;
                match result {
                    // The app came back to the front first: wait on.
                    Err(e) if e.code == "front_blocked" => {
                        super::shared_app::take_blocked();
                        core.update(cx, |c, _| c.input_queue.requeue_front(request));
                    }
                    Err(e) => {
                        finish(core, request, Some(e.message.clone()), cx);
                        return Err(e);
                    }
                    Ok(mut answer) => {
                        finish(core, request, None, cx);
                        let waited = started.elapsed().as_secs_f64();
                        let note = format!(
                            "It waited {waited:.1} s in the Input Queue (input request {request}) until the user's window of {} no longer had focus, then went in through the background route.",
                            front.app
                        );
                        let said = answer
                            .get("note")
                            .and_then(Value::as_str)
                            .map(|n| format!("{n} {note}"))
                            .unwrap_or(note);
                        answer.insert("note".into(), json!(said));
                        answer.insert("requestId".into(), json!(request));
                        answer.insert(
                            "waitedSeconds".into(),
                            json!((waited * 10.0).round() / 10.0),
                        );
                        return Ok(answer);
                    }
                }
            }
        }
        if started.elapsed() >= WAIT {
            detach(core, request, cx);
            let why = why.trim_end_matches('.');
            return Err(ControlError {
                code: "input_pending",
                message: format!(
                    "{why}. It waits as input request {request} and runs by itself, through the background route, once the user's window of {app} no longer has focus, while a call of yours waits for it: repeat the same call, or call input_queue wait with requestId {request}. Meanwhile reads and act_on_element still work. A newer action replaces it, and input_queue withdraw cancels it. Agent Screen won't switch apps for the user.",
                    app = front.app
                ),
            });
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}

fn detach(core: &Entity<Core>, request: RequestId, cx: &mut AsyncApp) {
    core.update(cx, |c, cx| {
        c.input_queue.detach(request);
        cx.notify();
    });
}

/// The request ran, or `failure` says why not; its call reads the result
/// now, so it goes, and the same action again is a new request.
fn finish(core: &Entity<Core>, request: RequestId, failure: Option<String>, cx: &mut AsyncApp) {
    core.update(cx, |c, cx| {
        c.input_queue.finish_front(request, failure);
        cx.notify();
    });
    forget(core, request, cx);
}

fn forget(core: &Entity<Core>, request: RequestId, cx: &mut AsyncApp) {
    core.update(cx, |c, cx| {
        c.input_queue.forget(request);
        cx.notify();
    });
}
