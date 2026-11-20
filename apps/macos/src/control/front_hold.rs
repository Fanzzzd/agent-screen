//! The front hold (ADR 0013): the user pressed "Let the agent continue"
//! beside a display's waiting line. The actions waiting in the Input Queue
//! for the user to leave their front app run now: Agent Screen becomes the
//! front process for the moment they need, each goes in through the
//! background route as its waiting loop sees the app no longer in front
//! (`front_queue::drive`), and each hands the app's focus back to the
//! user's window. Then the app gets the front back with that window key.
//! The keys the user types meanwhile are held at the tap and go to their
//! window after; a mouse press ends the hold, and the rest waits again.
//! The rules are in `agent_screen_model::front_hold`.

use super::off;
use crate::core::Core;
use crate::mac::key_hold;
use crate::model::{DisplayId, WindowTarget};
use agent_screen_model::front_hold::{self, End, Next, Phase, Start, UserInput};
use gpui::{App, AsyncApp, Entity};
use std::time::{Duration, Instant};

/// How often the hold looks at the front and the waiting actions.
const TICK: Duration = Duration::from_millis(20);

/// After the hold ends, how long actions already sending may take.
const SETTLE: Duration = Duration::from_millis(1000);

/// The user pressed the button for display `display` in Agent Screen's
/// window number `window`, which is key.
pub fn press(core: Entity<Core>, display: DisplayId, window: u32, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let end = hold(&core, display, window, cx).await;
        log::info!("front hold for display {display}: {end:?}");
    })
    .detach();
}

/// Run display `display`'s waiting actions, holding the front if their app
/// has it. None when nothing waits, or a hold goes on already.
pub async fn hold(
    core: &Entity<Core>,
    display: DisplayId,
    window: u32,
    cx: &mut AsyncApp,
) -> Option<End> {
    let own = std::process::id() as i32;
    let app = core.read_with(cx, |c, _| c.input_queue.waits_on(display))?;
    let start = front_hold::start(crate::mac::input::front(), own, app);
    if start == Start::NotNeeded {
        log::info!("front hold: pid {app} isn't in front; its actions run by themselves");
        return None;
    }
    if !core.update(cx, |c, _| c.input_queue.start_hold(app)) {
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
    let keys = user.and_then(|user| key_hold::start_with(user, Some(own)));
    let started = Instant::now();
    let mut phase = Phase::Taking;
    let mut since = Instant::now();
    if start == Start::Take {
        let taken = crate::mac::input::take_front(window);
        log::info!("front hold: Agent Screen takes the front from pid {app}: {taken}");
    }
    let end = loop {
        let input = match &keys {
            Some(keys) if !keys.holding() => UserInput::Pressed,
            Some(keys) if keys.typed() => UserInput::Typed,
            _ => UserInput::None,
        };
        let own_front = crate::mac::input::server_front() == own;
        // Not those on a display in Takeover: they wait for its end.
        let pending = core.read_with(cx, |c, _| {
            c.input_queue
                .pending_on(app, |id| c.displays.get(id).is_some_and(|d| !d.takeover))
        });
        let elapsed = since.elapsed().as_millis() as u64;
        match front_hold::next(phase, elapsed, own_front, pending, input) {
            Next::Wait => {}
            Next::Run => {
                phase = Phase::Running;
                since = Instant::now();
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
    while core.read_with(cx, |c, _| c.input_queue.running_on(app)) > 0 && ending.elapsed() < SETTLE
    {
        cx.background_executor().timer(TICK).await;
    }
    let held_ms = started.elapsed().as_millis();
    if front_hold::gives_back(end) {
        let back = off(cx, move || give_back(app, user)).await;
        log::info!(
            "front hold: {end:?} after {held_ms} ms; pid {app} back in front with the user's window key: {back}"
        );
        if let Some(keys) = keys {
            let id = keys.detach();
            std::thread::spawn(move || {
                key_hold::finish(id, agent_screen_model::key_hold::End::HandedBack)
            });
        }
    } else {
        log::info!("front hold: {end:?} after {held_ms} ms; the user went elsewhere");
        drop(keys);
    }
    core.update(cx, |c, cx| {
        c.input_queue.clear_hold();
        cx.notify();
    });
    Some(end)
}

/// Give app `app` the front back, with the user's window `user` key, and
/// say whether both hold. Blocks up to about 1.2 s.
fn give_back(app: i32, user: Option<WindowTarget>) -> bool {
    let key = || match user {
        Some(user) => crate::mac::ax::focused_window_id(app) == Some(user.window_id),
        None => true,
    };
    // Each action's hand-back made the user's window the app's key window;
    // make sure, as one may have been cut short.
    if let Some(user) = user
        && !key()
    {
        crate::mac::ax::make_main(user);
    }
    let asked = Instant::now();
    let sent = crate::mac::input::give_front(app, user.map_or(0, |u| u.window_id));
    while crate::mac::input::server_front() != app && asked.elapsed() < Duration::from_millis(200) {
        std::thread::sleep(Duration::from_millis(10));
    }
    if crate::mac::input::server_front() != app {
        log::warn!(
            "front hold: pid {app} didn't take the front back (asked: {sent}); activating it"
        );
        crate::mac::ime::return_front(app);
    }
    let front = crate::mac::input::server_front() == app;
    if front {
        crate::mac::input::note_activation(Some(app));
    }
    front && key()
}
