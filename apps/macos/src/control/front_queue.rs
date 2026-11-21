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
use agent_screen_model::input_queue::{RequestId, Status, Stop};
use agent_screen_model::real_click;
use gpui::{AsyncApp, Entity};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long one call waits for the user to leave the app before it
/// answers `input_pending`: well under the tool timeouts of the MCP
/// clients, of which Codex's 60 s default is the shortest.
const WAIT: Duration = Duration::from_secs(30);

/// Test hook (`debugFrontWait`): a shorter wait, in milliseconds, so
/// scripts reach `input_pending` fast. 0 means `WAIT`.
static WAIT_MS: AtomicU64 = AtomicU64::new(0);

pub fn set_wait_for_tests(ms: u64) {
    WAIT_MS.store(ms, Ordering::Relaxed);
}

/// How long a front request may wait for its app to leave the front,
/// from its ask, before it lapses unsent.
const MAX_AGE: Duration = Duration::from_secs(600);

fn wait_limit() -> Duration {
    match WAIT_MS.load(Ordering::Relaxed) {
        0 => WAIT,
        ms => Duration::from_millis(ms),
    }
}

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
            if let Ok(id) = display(core, p, true, cx) {
                replaced_by(core, connection, id, cx);
            }
            return first;
        }
    };
    let refused = || ControlError::failed(why.clone());
    let Some((pid, app)) = first.as_ref().err().and_then(|e| e.blocked.clone()) else {
        return Err(refused());
    };
    let id = display(core, p, true, cx).map_err(|_| refused())?;
    let Ok(owner) = core.read_with(cx, |c, _| c.input_queue.owner(id, connection)) else {
        return Err(refused());
    };
    // A click runs later only if what is under its point is still there:
    // without a first look, nothing can tell, so it isn't queued.
    let look = match method {
        "click" => match look_at(core, p, cx).await {
            Some(look) => Some(look),
            None => {
                return Err(ControlError::failed(format!(
                    "{}. The click can't wait for the user to leave {app}: Agent Screen couldn't capture what is under its point to compare before it runs. Take a screenshot and try again.",
                    why.trim_end_matches('.')
                )));
            }
        },
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

/// The agent acted on display `id` another way: its waiting front request
/// no longer applies.
pub fn replaced_by(core: &Entity<Core>, connection: u64, id: DisplayId, cx: &mut AsyncApp) {
    core.update(cx, |c, cx| {
        if let Ok(owner) = c.input_queue.owner(id, connection) {
            c.input_queue.replace_fronts(&owner);
            cx.notify();
        }
    });
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

/// Why a front request stopped, in words for the agent.
fn stopped(why: Stop) -> &'static str {
    match why {
        Stop::Withdrawn => "was withdrawn",
        Stop::Replaced => "was replaced by a newer action of yours",
        Stop::DisplayEnded => "stopped because its display ended",
        Stop::Disconnected => "stopped because your session disconnected",
        Stop::Takeover => "stopped because the user took over the display",
        Stop::User => "was stopped by the user",
        Stop::Failed => "could not run",
    }
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
/// then run it. `why` says what blocked it. Past `WAIT` the call answers
/// `input_pending`, and a background run carries the request on.
async fn run(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    request: RequestId,
    why: &str,
    cx: &mut AsyncApp,
) -> Reply {
    drive(core, connection, id, request, Some(why), cx).await
}

/// The loop behind `run`, and behind a request's background run when
/// `why` is None: that one never answers a call. It runs the request once
/// the user's window leaves the front, within `MAX_AGE` of its ask, and
/// keeps what came of it for the agent's next call.
async fn drive(
    core: &Entity<Core>,
    connection: u64,
    id: DisplayId,
    request: RequestId,
    why: Option<&str>,
    cx: &mut AsyncApp,
) -> Reply {
    let background = why.is_none();
    let started = Instant::now();
    loop {
        let (status, front, paused, age) = core.read_with(cx, |c, _| {
            (
                c.input_queue.queue.request(request).map(|r| r.status),
                c.input_queue.front(request).cloned(),
                c.displays.get(id).is_some_and(|d| d.takeover),
                c.input_queue.age(request).unwrap_or_default(),
            )
        });
        let (Some(status), Some(front)) = (status, front) else {
            return Err(ControlError::not_found(format!(
                "input request {request} was replaced by a newer action of yours, or its display ended, so it was not sent. Take a screenshot before you act again."
            )));
        };
        let action = verb(&front.method).to_lowercase();
        match status {
            Status::Queued => {}
            // The other loop on it, a call's or the background run, is
            // sending it: wait for what comes of it.
            Status::Running => {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                continue;
            }
            ended => {
                if background {
                    return Err(ControlError::failed("ended"));
                }
                let kept = core.update(cx, |c, _| c.input_queue.take_outcome(request));
                forget(core, request, cx);
                if let Some(outcome) = kept {
                    return outcome.map_err(|(code, message)| ControlError {
                        code,
                        message,
                        blocked: None,
                    });
                }
                let how = match ended {
                    Status::Stopped(why) => stopped(why),
                    Status::Completed => "already ran",
                    _ => "is running in another call",
                };
                return Err(ControlError::failed(format!(
                    "the queued {action} (input request {request}) {how}; nothing more was sent. Take a screenshot before you act again."
                )));
            }
        }
        if paused {
            if !background {
                detach(core, request, cx);
                return Err(ControlError::takeover(format!(
                    "the user took over display {id}, so your queued {action} (input request {request}) waits. Call wait_for_control, then take a screenshot before you act again."
                )));
            }
        } else if crate::mac::input::front() != Some(front.pid) {
            if let Some(look) = &front.look
                && !look_kept(look, cx).await
            {
                let refused = ControlError::failed(format!(
                    "The click (input request {request}) waited until the user's window of {} lost focus, but {}, so it was not sent. Take a screenshot and click again.",
                    front.app,
                    real_click::CONTENT_CHANGED
                ));
                return settle(core, request, background, Err(refused), cx);
            }
            let started_front = core.update(cx, |c, cx| {
                let started = c.input_queue.start_front(request);
                cx.notify();
                started
            });
            if started_front {
                let params = front.params.as_object().cloned().unwrap_or_default();
                // A posted click or key makes the agent's window the app's
                // key window. The user's window gets that back after, with
                // a focus record that doesn't bring the app forward, so
                // switching back to the app lands them in their window.
                let forwarder = core.update(cx, |c, _| c.forwarder(id));
                forwarder.remember_user_window(front.pid).await;
                let result = replay(core, connection, &front.method, &Args(&params), cx).await;
                forwarder.keys_done().await;
                match result {
                    // The app came back to the front first: wait on.
                    Err(e) if e.code == "front_blocked" => {
                        core.update(cx, |c, _| c.input_queue.requeue_front(request));
                    }
                    Err(e) => return settle(core, request, background, Err(e), cx),
                    Ok(mut answer) => {
                        let waited = age as f64 / 1000.0;
                        let after = if background {
                            ", after your call had returned input_pending,"
                        } else {
                            ""
                        };
                        let note = format!(
                            "It waited {waited:.1} s in the Input Queue (input request {request}){after} until the user's window of {} no longer had focus, then went in through the background route.",
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
                        return settle(core, request, background, Ok(answer), cx);
                    }
                }
            }
        }
        if let Some(why) = why
            && started.elapsed() >= wait_limit()
        {
            detach(core, request, cx);
            let first = core.update(cx, |c, _| c.input_queue.drive(request));
            if first {
                let core = core.clone();
                cx.spawn(async move |cx: &mut AsyncApp| {
                    let _ = drive(&core, connection, id, request, None, cx).await;
                })
                .detach();
            }
            let why = why.trim_end_matches('.');
            return Err(ControlError {
                blocked: None,
                code: "input_pending",
                message: format!(
                    "{why}. It waits as input request {request} and runs by itself, through the background route, once the user's window of {app} no longer has focus, for up to {minutes} minutes from now; repeat the same call, or call input_queue wait with request_id {request}, to wait for it and read its result. Meanwhile reads still work. Any other action of yours on this display replaces it, and input_queue withdraw cancels it. Agent Screen won't switch apps for the user.",
                    app = front.app,
                    minutes = MAX_AGE.as_secs() / 60,
                ),
            });
        }
        if background && Duration::from_millis(age) >= MAX_AGE {
            let minutes = MAX_AGE.as_secs() / 60;
            let why = if paused {
                format!("the user still had display {id} in a Takeover after {minutes} minutes")
            } else {
                format!(
                    "the user's window of {} kept focus for {minutes} minutes",
                    front.app
                )
            };
            let lapsed = ControlError::failed(format!(
                "the queued {action} (input request {request}) was not sent: {why}. Take a screenshot before you act again."
            ));
            return settle(core, request, true, Err(lapsed), cx);
        }
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
    }
}

/// What came of front request `request`: a call reads it now, so it goes;
/// a background run keeps it for the agent's next call.
fn settle(
    core: &Entity<Core>,
    request: RequestId,
    background: bool,
    outcome: Reply,
    cx: &mut AsyncApp,
) -> Reply {
    let failure = outcome.as_ref().err().map(|e| e.message.clone());
    if background {
        core.update(cx, |c, cx| {
            c.input_queue.finish_front(request, failure);
            let kept = outcome.clone().map_err(|e| (e.code, e.message));
            c.input_queue.keep_outcome(request, kept);
            cx.notify();
        });
    } else {
        finish(core, request, failure, cx);
    }
    outcome
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
