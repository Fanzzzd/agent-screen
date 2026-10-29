//! App windows as the agent sees them, and moving them between displays.

use super::aerospace::AeroSpace;
use super::{ax, cg};
use crate::model::{DisplayId, Rect, WindowTarget};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct Window {
    pub target: WindowTarget,
    pub app: String,
    pub title: String,
    /// Display-relative bounds for windows on a display, global otherwise.
    pub bounds: Rect,
    /// Window level: 0 for documents, 3 for floating panels, 8 for modal
    /// panels and alerts.
    pub layer: i64,
}

impl Window {
    pub fn json(&self) -> Value {
        json!({
            "windowId": self.target.window_id,
            "app": self.app,
            "title": self.title,
            "x": self.bounds.x as i64,
            "y": self.bounds.y as i64,
            "width": self.bounds.width as i64,
            "height": self.bounds.height as i64,
            "kind": match self.layer {
                0 => "window",
                l if l >= 8 => "dialog",
                _ => "panel",
            }
        })
    }
}

/// A window held by its title bar: a drag moves it, inside its display.
/// Posted events can't move a window. WindowServer takes a title-bar drag
/// to the real pointer, and while the user's button is down that is on
/// their own screen.
#[derive(Clone, Copy, Debug)]
pub struct Grab {
    pub target: WindowTarget,
    frame: Rect,
    from: (f64, f64),
    /// The display's global bounds: the window stays inside them.
    area: Rect,
}

impl Grab {
    /// The window on display `id` whose title bar is at a global point,
    /// held there. A user window that only overhangs the display is never
    /// taken.
    pub fn on_display(id: DisplayId, at: (f64, f64)) -> Option<Grab> {
        let area = cg::display_bounds(id);
        let window = on_display(id)
            .into_iter()
            .map(|w| Window {
                bounds: w.bounds.offset(area.x, area.y),
                ..w
            })
            .find(|w| w.bounds.contains(at))?;
        Grab::title_bar(window.target, window.bounds, at, area)
    }

    /// `target`, at global `bounds` inside `area`, if `at` is on its title
    /// bar.
    pub fn title_bar(
        target: WindowTarget,
        bounds: Rect,
        at: (f64, f64),
        area: Rect,
    ) -> Option<Grab> {
        Grab::held(target, bounds, at, area).filter(|_| ax::on_title_bar(target, bounds, at))
    }

    /// `target`, at global `bounds` inside `area`, pressed at `at`
    /// wherever that is on it.
    pub fn held(target: WindowTarget, bounds: Rect, at: (f64, f64), area: Rect) -> Option<Grab> {
        area.contains(bounds.mid()).then_some(Grab {
            target,
            frame: bounds,
            from: at,
            area,
        })
    }

    /// Whether the window has left its display, carried off by something
    /// other than this grab. Blocking.
    pub fn left_display(&self) -> bool {
        find(self.target.window_id).is_some_and(|w| !self.area.contains(w.bounds.mid()))
    }

    /// Where it was pressed.
    pub fn from(&self) -> (f64, f64) {
        self.from
    }

    /// Whether the pointer has gone far enough from the press to count as
    /// a drag rather than a click.
    pub fn moved(&self, to: (f64, f64)) -> bool {
        (to.0 - self.from.0).hypot(to.1 - self.from.1) >= 3.0
    }

    /// Move the window as far as the pointer moved from the press.
    /// Blocking.
    pub fn drag_to(&self, to: (f64, f64)) -> bool {
        let frame = self
            .frame
            .offset(to.0 - self.from.0, to.1 - self.from.1)
            .kept_inside(&self.area);
        ax::set_frame(self.target, frame).is_ok()
    }
}

/// Other apps' windows a user can act on, front to back, global bounds:
/// documents, floating panels such as inspectors, and modal panels and
/// alerts (levels 0 to 8). Menus, the Dock and status items sit higher.
pub fn all() -> Vec<Window> {
    let own = std::process::id() as i32;
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| {
            (0..=8).contains(&w.layer)
                && w.target.pid != own
                && w.bounds.width > 40.0
                && w.bounds.height > 40.0
        })
        .map(|w| Window {
            target: w.target,
            app: w.owner_name,
            title: w.title,
            bounds: w.bounds,
            layer: w.layer,
        })
        .collect()
}

/// Whether a window with `bounds` is on the display covering `area`: it
/// overlaps it, and no other display holds its center instead.
fn belongs(bounds: &Rect, area: &Rect, others: &[Rect]) -> bool {
    let center = bounds.mid();
    bounds.intersects(area) && (area.contains(center) || !others.iter().any(|o| o.contains(center)))
}

fn other_displays(display: DisplayId) -> Vec<Rect> {
    cg::active_displays()
        .into_iter()
        .filter(|&d| d != display)
        .map(cg::display_bounds)
        .collect()
}

/// How many windows each display has, from one window list.
pub fn counts(displays: &[DisplayId]) -> std::collections::HashMap<DisplayId, usize> {
    let windows = all();
    displays
        .iter()
        .map(|&id| {
            let (area, others) = (cg::display_bounds(id), other_displays(id));
            let n = windows
                .iter()
                .filter(|w| belongs(&w.bounds, &area, &others))
                .count();
            (id, n)
        })
        .collect()
}

/// Windows on a display, with display-relative bounds. A window counts if it
/// touches the display and its center is on no other display. A window
/// hanging off the bottom edge still counts, so dismissing the display does
/// not strand it off-screen.
pub fn on_display(display: DisplayId) -> Vec<Window> {
    let area = cg::display_bounds(display);
    let others = other_displays(display);
    all()
        .into_iter()
        .filter(|w| belongs(&w.bounds, &area, &others))
        .map(|w| Window {
            bounds: w.bounds.offset(-area.x, -area.y),
            ..w
        })
        .collect()
}

/// Menus open on a display: an app's pop-up menus, as a web page's select
/// shows, at the pop-up menu level (kCGPopUpMenuWindowLevel).
pub fn menus_on(display: DisplayId) -> Vec<u32> {
    let own = std::process::id() as i32;
    let area = cg::display_bounds(display);
    cg::on_screen_windows()
        .into_iter()
        .filter(|w| w.layer == 101 && w.target.pid != own && w.bounds.intersects(&area))
        .map(|w| w.target.window_id)
        .collect()
}

/// Move document windows that grew past the display's edges, such as
/// Calculator switching to Scientific, or under the strip that holds the
/// user's menu bar, as a zoomed window does, back inside. True when one
/// moved.
pub fn keep_inside(display: DisplayId) -> bool {
    let area = cg::display_bounds(display);
    let local = crate::model::usable_area(
        Rect::new(0.0, 0.0, area.width, area.height),
        super::menubar_height(display),
    );
    let mut moved = false;
    for window in on_display(display).into_iter().filter(|w| w.layer == 0) {
        let kept = window.bounds.kept_inside(&local);
        if kept != window.bounds {
            moved |= ax::set_frame(window.target, kept.offset(area.x, area.y)).is_ok();
        }
    }
    moved
}

pub fn find(window_id: u32) -> Option<Window> {
    all().into_iter().find(|w| w.target.window_id == window_id)
}

/// Whether the window sits on a virtual display of Agent Screen's, any
/// instance's, rather than on one of the user's screens. Blocking.
pub fn on_virtual_display(window_id: u32) -> bool {
    find(window_id).is_some_and(|w| display_role(&w) == crate::model::DisplayRole::Agent)
}

pub fn display_role(window: &Window) -> crate::model::DisplayRole {
    let mid = window.bounds.mid();
    cg::active_displays()
        .into_iter()
        .find(|&id| cg::display_bounds(id).contains(mid))
        .map(cg::window_display_role)
        .unwrap_or(crate::model::DisplayRole::User)
}

/// Windows on any of `displays` that AeroSpace leaves there. With
/// `ignored-monitors` it lets go of a window that lands on an Agent Display
/// only once its own list of screens has the display, which comes late
/// after a display appears: a window macOS puts back on a new display that
/// took an old one's place, and AeroSpace still files in the user's
/// workspace, goes back there. A new window it files for a few seconds
/// and then lets go. Blocking.
pub fn kept_on(displays: &[DisplayId]) -> std::collections::HashSet<u32> {
    let mut on: std::collections::HashSet<u32> = displays
        .iter()
        .filter(|&&d| cg::window_display_role(d) == crate::model::DisplayRole::Agent)
        .flat_map(|&d| on_display(d))
        .map(|w| w.target.window_id)
        .collect();
    if super::aerospace::ignores_now()
        && let Some(aerospace) = AeroSpace::current()
        && let Ok(managed) = aerospace.managed_windows()
    {
        on.retain(|w| !managed.contains(w));
    }
    on
}

/// Move a window onto a display in whatever way the user's window manager
/// respects. AeroSpace re-tiles any window moved behind its back, so with
/// AeroSpace running it moves the window to the workspace shown on the
/// display. AeroSpace with `ignored-monitors` doesn't list the display and
/// leaves windows on it alone; then, or without AeroSpace, the Accessibility
/// API moves it.
///
/// Blocking. `screen_name` is the display's localized name, read on the
/// main thread.
/// Whether a window placed on a display stays there. AeroSpace can take a
/// move for a window it hasn't registered yet, then lay the window out in
/// the user's workspace once it does: the window must sit in the workspace
/// AeroSpace shows on the display, not just inside the display's bounds.
pub fn held(target: WindowTarget, display: DisplayId, screen_name: Option<&str>) -> bool {
    if !on_display(display).iter().any(|w| w.target == target) {
        return false;
    }
    if super::aerospace::ignores_now() {
        return true;
    }
    let Some(aerospace) = AeroSpace::current() else {
        return true;
    };
    let Some(name) = screen_name else {
        return true;
    };
    match aerospace.whereabouts(target.window_id) {
        Ok(Some((monitor, visible))) => monitor == name && visible,
        // Not managed by AeroSpace, such as a panel: its bounds decide.
        Ok(None) => true,
        Err(_) => false,
    }
}

/// What Agent Screen knows of windows the user took back, and a lane per
/// window for the OS calls that move it. A move for the agent, a frame
/// write and a move home of one window take its lane in turn, so one
/// decided before a Take Back can't put the window on the display again
/// after it went home. The state lock is held only to read or change the
/// state, never across an OS call: the main thread reads it, and windows
/// never wait for each other.
#[derive(Default)]
pub struct Lanes {
    state: std::sync::Mutex<Returns>,
    lanes: std::sync::Mutex<std::collections::HashMap<u32, std::sync::Arc<std::sync::Mutex<()>>>>,
}

#[derive(Default)]
struct Returns {
    /// Taken back and not handed over since.
    returned: std::collections::BTreeSet<u32>,
    /// Moves home so far, by window.
    count: std::collections::HashMap<u32, u64>,
}

impl Lanes {
    fn state(&self) -> std::sync::MutexGuard<'_, Returns> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` in the window's lane, after the window's earlier OS calls.
    fn in_lane<T>(&self, window: u32, f: impl FnOnce() -> T) -> T {
        let lane = self
            .lanes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(window)
            .or_default()
            .clone();
        let held = lane.lock().unwrap_or_else(|e| e.into_inner());
        let out = f();
        drop(held);
        // Nobody else waits on it: let it go.
        let mut lanes = self.lanes.lock().unwrap_or_else(|e| e.into_inner());
        if lanes
            .get(&window)
            .is_some_and(|l| std::sync::Arc::strong_count(l) == 2)
        {
            lanes.remove(&window);
        }
        out
    }

    /// How many times a window went home so far. Never waits for an OS call.
    pub fn times_returned(&self, window: u32) -> u64 {
        self.state().count.get(&window).copied().unwrap_or(0)
    }

    /// Move for the agent, unless the user took the window back.
    pub fn agent_move<T>(
        &self,
        window: u32,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.in_lane(window, || {
            if self.state().returned.contains(&window) {
                return Err("the user took the window back".into());
            }
            f()
        })
    }

    /// Mark the window as the user's, then send it home with `home`.
    pub fn home(&self, window: u32, home: impl FnOnce()) {
        self.in_lane(window, || {
            {
                let mut state = self.state();
                state.returned.insert(window);
                *state.count.entry(window).or_default() += 1;
            }
            home()
        })
    }

    /// Run `f`, for a hand-over or a frame write decided when the window had
    /// gone home `seen` times, unless it went home again since. With
    /// `handed_over`, the window is the agent's to move from now on.
    pub fn unless_returned_since<T>(
        &self,
        window: u32,
        seen: u64,
        handed_over: bool,
        f: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.in_lane(window, || {
            if self.state().count.get(&window).copied().unwrap_or(0) != seen {
                return Err("the user took the window back meanwhile".into());
            }
            let result = f()?;
            if handed_over {
                self.state().returned.remove(&window);
            }
            Ok(result)
        })
    }
}

static LANES: std::sync::LazyLock<Lanes> = std::sync::LazyLock::new(Lanes::default);

/// Move a window onto a display for the agent, unless the user took it
/// back. Blocking.
pub fn place_for_agent(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    LANES.agent_move(target.window_id, || place(target, display, screen_name))
}

/// Mark a window as the user's, after any move for the agent in flight,
/// then send it home with `home`, before any later one. Blocking.
pub fn return_home(window: u32, home: impl FnOnce()) {
    LANES.home(window, home)
}

/// How many times a window went home so far: read on the main thread when
/// a move or frame write is decided. Doesn't wait for any OS call.
pub fn times_returned(window: u32) -> u64 {
    LANES.times_returned(window)
}

/// Move a window handed over onto a display, unless it went home again
/// since the hand-over was decided (`seen` from `times_returned`). It is
/// the agent's to keep there from now on. Blocking.
pub fn place_handed_over(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
    seen: u64,
) -> Result<(), String> {
    LANES.unless_returned_since(target.window_id, seen, true, || {
        place(target, display, screen_name)
    })
}

/// Write a window's frame for the agent, unless it went home since `seen`.
/// Blocking.
pub fn write_frame_unless_returned<T>(
    window: u32,
    seen: u64,
    write: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    LANES.unless_returned_since(window, seen, false, write)
}

pub fn place(
    target: WindowTarget,
    display: DisplayId,
    screen_name: Option<&str>,
) -> Result<(), String> {
    if !super::aerospace::ignores_now()
        && let Some(aerospace) = AeroSpace::current()
        && let Some(name) = screen_name
        && let Ok(Some(monitor)) = aerospace.monitor_id(name)
    {
        // AeroSpace doesn't manage panels, such as an Open panel, and
        // learns of a new window a moment late: move those directly.
        let Err(refused) = aerospace.move_to_monitor(target.window_id, &monitor) else {
            return Ok(());
        };
        return ax::move_into(target, cg::display_bounds(display))
            .map_err(|e| format!("AeroSpace refused ({refused}) and Accessibility refused ({e})"));
    }
    ax::move_into(target, cg::display_bounds(display))
        .map_err(|e| format!("Accessibility refused: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Runs `f` on a thread; whether it finished within 300 ms.
    fn finishes(f: impl FnOnce() + Send + 'static) -> bool {
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = done.send(());
        });
        finished.recv_timeout(Duration::from_millis(300)).is_ok()
    }

    #[test]
    fn a_stuck_move_home_blocks_neither_reads_nor_other_windows() {
        let lanes = std::sync::Arc::new(Lanes::default());
        let (release, held) = mpsc::channel::<()>();
        let (entered, inside) = mpsc::channel::<()>();
        let stuck = std::thread::spawn({
            let lanes = lanes.clone();
            move || {
                lanes.home(1, move || {
                    let _ = entered.send(());
                    // An app that doesn't answer Accessibility.
                    let _ = held.recv_timeout(Duration::from_secs(5));
                })
            }
        });
        inside.recv_timeout(Duration::from_secs(1)).unwrap();
        let read = finishes({
            let lanes = lanes.clone();
            move || {
                lanes.times_returned(1);
            }
        });
        let other_home = finishes({
            let lanes = lanes.clone();
            move || lanes.home(2, || {})
        });
        let other_move = finishes({
            let lanes = lanes.clone();
            move || {
                let _ = lanes.agent_move(3, || Ok(()));
                let _ = lanes.unless_returned_since(4, 0, true, || Ok(()));
            }
        });
        // The same window waits its turn.
        let same = finishes({
            let lanes = lanes.clone();
            move || {
                let _ = lanes.agent_move(1, || Ok(()));
            }
        });
        let _ = release.send(());
        stuck.join().unwrap();
        assert!(read, "the main thread's read waited for an OS call");
        assert!(
            other_home && other_move,
            "another window waited for this one's OS call"
        );
        assert!(!same, "the same window's move ran beside its move home");
        // Once home, the agent's move is refused.
        assert!(lanes.agent_move(1, || Ok(())).is_err());
    }

    #[test]
    fn a_move_decided_before_a_take_back_stays_home() {
        let lanes = Lanes::default();
        let seen = lanes.times_returned(5);
        lanes.home(5, || {});
        let mut moved = false;
        assert!(
            lanes
                .unless_returned_since(5, seen, true, || {
                    moved = true;
                    Ok(())
                })
                .is_err()
        );
        assert!(!moved);
        // Handed over again after it: the agent's to move once more.
        let now = lanes.times_returned(5);
        assert!(lanes.unless_returned_since(5, now, true, || Ok(())).is_ok());
        assert!(lanes.agent_move(5, || Ok(())).is_ok());
    }

    #[test]
    fn a_failed_handover_keeps_the_window_returned() {
        let lanes = Lanes::default();
        lanes.home(6, || {});
        let seen = lanes.times_returned(6);
        assert!(
            lanes
                .unless_returned_since(6, seen, true, || {
                    Err::<(), _>("the app refused the move".into())
                })
                .is_err()
        );
        assert!(lanes.agent_move(6, || Ok(())).is_err());
    }

    #[test]
    fn a_frame_write_after_the_window_went_home_doesnt_run() {
        let seen = times_returned(900_003);
        return_home(900_003, || {});
        let mut wrote = false;
        let result = write_frame_unless_returned(900_003, seen, || {
            wrote = true;
            Ok(())
        });
        assert!(!wrote && result.is_err(), "the writer ran after Take Back");
        // Decided after the return, it writes.
        let now = times_returned(900_003);
        assert!(write_frame_unless_returned(900_003, now, || Ok(())).is_ok());
    }
}
