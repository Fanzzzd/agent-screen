//! The front hold (ADR 0013): the user pressed "Let the agent continue"
//! beside a display's waiting line. The actions waiting in the Input Queue
//! for the user to leave their front app run now: Agent Screen becomes the
//! front process for the moment they need, each goes in through the
//! background route as its waiting loop sees the app no longer in front
//! (`front_queue::drive`), and each hands the app's focus back to the
//! user's window. Then the app gets the front back with that window key.
//! The keys the user types meanwhile are held at the tap and go to their
//! window after; a mouse press ends the hold, and the rest waits again.
//! The Red Glow shows, Live, from before the front is taken until the hold
//! is over; without it the hold doesn't start.
//! The rules are in `agent_screen_model::front_hold`.

use super::off;
use crate::core::Core;
use crate::mac::key_hold;
use crate::model::{DisplayId, WindowTarget};
use agent_screen_model::autonomy::Mode;
use agent_screen_model::front_hold::{
    self, End, GIVE_BACK_MS, Initiator, Keys, Next, Phase, SETTLE_MS, Start,
};
use gpui::{App, AsyncApp, Entity};
use std::time::{Duration, Instant};

/// How often the hold looks at the front and the waiting actions.
const TICK: Duration = Duration::from_millis(20);

/// The user pressed the button for display `display` in Agent Screen's
/// window number `window`, which is key.
pub fn press(core: Entity<Core>, display: DisplayId, window: u32, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let end = hold(&core, display, window, Initiator::User, cx).await;
        log::info!("front hold for display {display}: {end:?}");
    })
    .detach();
}

/// The Autonomy Mode starts display `display`'s front hold by itself: in
/// Auto with the hands At Rest, in YOLO once the request is queued
/// (ADR 0025). Agent Screen takes the front with `native::front_anchor`.
pub fn by_mode(core: Entity<Core>, display: DisplayId, mode: Mode, cx: &mut App) {
    let Some(window) = crate::mac::native::front_anchor() else {
        return;
    };
    log::info!(
        "autonomy: {} starts the front hold for display {display}",
        mode.as_str()
    );
    cx.spawn(async move |cx: &mut AsyncApp| {
        let end = hold(&core, display, window, Initiator::Mode(mode), cx).await;
        crate::mac::native::front_anchor_away();
        if let Some(end) = end {
            crate::mac::activity::hold_ended();
            match end {
                End::UserActed(_) => crate::mac::activity::interrupted(),
                End::Ran => crate::mac::activity::completed_untouched(),
                _ => {}
            }
        }
        log::info!("front hold for display {display} by mode: {end:?}");
    })
    .detach();
}

/// Run display `display`'s waiting actions, holding the front if their app
/// has it. None when nothing waits, or a hold goes on already.
pub async fn hold(
    core: &Entity<Core>,
    display: DisplayId,
    window: u32,
    initiator: Initiator,
    cx: &mut AsyncApp,
) -> Option<End> {
    let own = std::process::id() as i32;
    let app = core.read_with(cx, |c, _| c.input_queue.waits_on(display))?;
    let start = front_hold::start(crate::mac::input::front(), own, app);
    if start == Start::NotNeeded {
        log::info!("front hold: pid {app} isn't in front; its actions run by themselves");
        return None;
    }
    // The user's window, to give the front back with: the app's focused
    // window, unless it is the agent's.
    let user = off(cx, move || {
        let focused = crate::mac::ax::focused_window_id(app);
        let agents = focused.is_some_and(crate::mac::input::on_agent_display);
        agent_screen_model::shared_app::user_window_to_remember(focused, agents).map(|window_id| {
            WindowTarget {
                pid: app,
                window_id,
            }
        })
    })
    .await;
    // Without the user's keys held, they would go to Agent Screen's key
    // window while it stands in front, which may be the Viewer, which
    // sends keys to the agent's display: no hold then.
    let Some(user) = user else {
        log::info!("front hold: pid {app} has no window of the user's focused; nothing runs");
        return None;
    };
    let Some(keys) = key_hold::start_with(user, Some(own)) else {
        log::info!("front hold: another key hold goes on; nothing runs");
        return None;
    };
    if !core.update(cx, |c, _| c.input_queue.start_hold(display, app)) {
        drop(keys);
        return None;
    }
    // However the hold ends, even by a panic, its record goes, and the
    // glow with it.
    let _clear = Clear(core.clone(), cx.clone());
    // The glow first: the front isn't taken without it.
    if !super::agent::sync_glow(cx) {
        log::warn!("front hold: the red glow didn't show; nothing runs");
        core.update(cx, |c, _| c.input_queue.end_hold());
        drop(keys);
        return None;
    }
    let started = Instant::now();
    let mut phase = Phase::Taking;
    if start == Start::Take {
        let taken = crate::mac::input::take_front(window);
        log::info!("front hold: Agent Screen asks for the front from pid {app}: {taken}");
    }
    let end = loop {
        let input = match keys.status() {
            key_hold::Status::Ended => Keys::Ended,
            key_hold::Status::Holding {
                asked: Some(agent_screen_model::key_hold::End::Pressed),
                ..
            } => Keys::Pressed,
            // Full: the user typed that much.
            key_hold::Status::Holding { asked: Some(_), .. }
            | key_hold::Status::Holding { typed: true, .. } => Keys::Typed,
            key_hold::Status::Holding { .. } => Keys::Quiet,
        };
        let own_front = crate::mac::input::server_front() == own;
        // Not while the display is in a Takeover: they wait for its end.
        let pending = core.read_with(cx, |c, _| {
            let live = c.displays.get(display).is_some_and(|d| !d.takeover);
            if live {
                c.input_queue.pending_on(display, app)
            } else {
                0
            }
        });
        let elapsed = started.elapsed().as_millis() as u64;
        match front_hold::next(
            phase,
            elapsed,
            own_front,
            pending,
            input,
            initiator.yields(),
        ) {
            Next::Wait => {}
            Next::Run => {
                phase = Phase::Running;
                // Only now, with WindowServer's word: the waiting actions
                // start once Agent Screen counts as the front app.
                if crate::mac::input::front() != Some(own) {
                    crate::mac::input::note_activation(Some(own));
                }
            }
            Next::End(end) => break end,
        }
        cx.background_executor().timer(TICK).await;
    };
    // Nothing more starts; what is sending finishes first.
    core.update(cx, |c, _| c.input_queue.end_hold());
    let ending = Instant::now();
    while core.read_with(cx, |c, _| c.input_queue.running_on(display, app)) > 0
        && ending.elapsed() < Duration::from_millis(SETTLE_MS)
    {
        cx.background_executor().timer(TICK).await;
    }
    let held_ms = started.elapsed().as_millis();
    let back = if front_hold::gives_back(end) {
        Some(off(cx, move || give_back(app, user)).await)
    } else {
        None
    };
    log::info!(
        "front hold: {end:?} after {held_ms} ms; pid {app} back in front with the user's window key: {back:?}"
    );
    // The keys typed meanwhile go to the user's window now, and a press
    // after them.
    let id = keys.detach();
    if let Some(why) = front_hold::keys_end(end) {
        std::thread::spawn(move || key_hold::finish(id, why));
    }
    Some(end)
}

/// Clears the hold's record when the hold ends. After a panic the record
/// stays, and the next press replaces it once it is stale
/// (`front_hold::STALE_MS`): touching the app mid-panic could panic again.
struct Clear(Entity<Core>, AsyncApp);

impl Drop for Clear {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.0.update(&mut self.1, |c, cx| {
            c.input_queue.clear_hold();
            cx.notify();
        });
        super::agent::sync_glow(&mut self.1);
    }
}

/// Give app `app` the front back, with the user's window `user` key, and
/// say whether both hold. Everything here counts against `GIVE_BACK_MS`,
/// from its first line: the front first, as `give_front` makes `user` key
/// with it, then, only if it didn't, an AXMain write on a thread of its
/// own, since that write can wait on the app and the input gate. A late
/// write lands on the user's own window.
fn give_back(app: i32, user: WindowTarget) -> bool {
    let asked = Instant::now();
    let limit = Duration::from_millis(GIVE_BACK_MS);
    let key = || crate::mac::ax::focused_window_id(app) == Some(user.window_id);
    let sent = crate::mac::input::give_front(app, user.window_id);
    let mut activated = false;
    let mut made_main = false;
    loop {
        let front = crate::mac::input::server_front() == app;
        // A focus read may take its 0.1 s timeout: none starts that late.
        let room = asked.elapsed() + Duration::from_millis(100) <= limit;
        if front && room && key() {
            crate::mac::input::note_activation(Some(app));
            return true;
        }
        if !room {
            if front {
                crate::mac::input::note_activation(Some(app));
            }
            return false;
        }
        if front && !made_main {
            // Each action's hand-back made the user's window the app's key
            // window; one may have been cut short.
            made_main = true;
            std::thread::spawn(move || crate::mac::ax::make_main(user));
        }
        // Some apps, Electron ones and Dia, stay behind: activate them.
        if !front && !activated && asked.elapsed() >= Duration::from_millis(200) {
            activated = true;
            log::warn!(
                "front hold: pid {app} didn't take the front back (asked: {sent}); activating it"
            );
            crate::mac::app::activate(app);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
