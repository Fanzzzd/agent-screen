//! Bringing a window the user minimized out onto an Agent Display without
//! drawing it on their screen, and putting it back in the Dock (#221).
//!
//! Taking a window out of the Dock in place plays the Dock's restore on the
//! user's screen. So the app hides first, the window leaves the Dock and
//! moves onto the display while the app is hidden, and the app shows again.
//! Hiding the app takes every window of it off the screen for that moment,
//! so it happens only when the app has no window the user could see or
//! come back to, and isn't the front app. An app the user hid stays
//! hidden: showing it would bring back windows the window server can't
//! tell apart from ones the app keeps out of sight.
//!
//! Every step after the first change can be undone, and a failure undoes
//! them, so the window is left in the Dock, with the user.

use crate::model::Rect;
use crate::placement::PutAway;

/// The window server's layer for status items (`kCGStatusWindowLevel`).
pub const STATUS_LAYER: i64 = 25;
/// The menu bar's height at most, notched displays included, in points.
const MENU_BAR_HEIGHT: f64 = 44.0;

/// Whether a window at `layer` with `frame` is a menu bar extra: a status
/// item in some display's menu bar, which stays when its app hides.
pub fn is_menu_extra(layer: i64, frame: Rect, displays: &[Rect]) -> bool {
    layer == STATUS_LAYER
        && displays.iter().any(|d| {
            frame.x >= d.x
                && frame.max_x() <= d.max_x()
                && frame.y >= d.y
                && frame.max_y() <= d.y + MENU_BAR_HEIGHT
        })
}

/// Another window of the app than the one coming out, as macOS reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OtherWindow {
    /// The window server lists it on screen, at any level and any size,
    /// on any display.
    pub on_screen: bool,
    /// Accessibility lists it with the app's windows.
    pub listed: bool,
    /// Accessibility says it is minimized.
    pub minimized: bool,
    /// How many Spaces hold it; -1 when unknown.
    pub spaces: i32,
    /// On screen on the Agent Display the window is leaving, as a text
    /// field's input indicator beside it: not on the user's screens.
    pub beside: bool,
    /// A menu bar extra ([`is_menu_extra`]): hiding the app leaves it.
    pub menu_extra: bool,
}

impl OtherWindow {
    /// Whether hiding the app would take it from where the user could see
    /// it, now or on another Space. Only a minimized window, or one the app
    /// keeps out of sight on no Space, would not; anything unclear would.
    pub fn would_vanish(self) -> bool {
        if self.menu_extra || (self.on_screen && self.beside) {
            return false;
        }
        if self.on_screen {
            return true;
        }
        if self.listed {
            return !self.minimized;
        }
        self.spaces != 0
    }
}

/// The app of the window, read right before a step that hides it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppState {
    pub hidden: bool,
    /// The front app, whose menus the menu bar shows.
    pub active: bool,
    /// Its windows other than the one coming out or going back.
    pub others: Vec<OtherWindow>,
}

impl AppState {
    /// Whether hiding it for a moment changes nothing the user sees: it is
    /// in the background, with no window that would vanish.
    pub fn may_hide(&self) -> bool {
        !self.hidden && !self.active && !self.others.iter().any(|w| w.would_vanish())
    }
}

/// Why a window off screen stays where the user put it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The user hid its app.
    AppHidden,
    /// It isn't minimized: on another Space, or kept out of sight.
    NotMinimized,
    /// Its app is the front app; hiding it would change the front app.
    AppInFront,
    /// Its app has other windows that hiding it would take away.
    OtherWindows,
}

impl Refusal {
    /// For the agent, after "could not move the <app> window: ".
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::AppHidden => {
                "its app is hidden, and showing it would bring all its windows back onto the user's screen. Ask the user to show the app, then call place_window again"
            }
            Refusal::NotMinimized => {
                "it is off screen but not minimized: on another macOS Space, or closed. Ask the user to bring it up, then call place_window again"
            }
            Refusal::AppInFront => {
                "it is minimized and its app is the front app, so taking it out of the Dock unseen would change the front app. Ask the user to click it in the Dock, then call place_window again"
            }
            Refusal::OtherWindows => {
                "it is minimized and its app has other windows the user can see, which taking it out of the Dock unseen would hide for a moment. Ask the user to click it in the Dock, then call place_window again"
            }
        }
    }
}

/// Whether a window off screen may come out unseen: a minimized window of
/// an app in the background with nothing else to see. Read again right
/// before the app hides.
pub fn may_bring_out(minimized: bool, app: &AppState) -> Result<(), Refusal> {
    if app.hidden {
        return Err(Refusal::AppHidden);
    }
    if !minimized {
        return Err(Refusal::NotMinimized);
    }
    if app.active {
        return Err(Refusal::AppInFront);
    }
    if app.others.iter().any(|w| w.would_vanish()) {
        return Err(Refusal::OtherWindows);
    }
    Ok(())
}

/// The steps that bring a minimized window out, in order. Each is
/// recorded as reached before it is tried: an app that misses
/// Accessibility's timeout, as one in App Nap while hidden, may still
/// apply the change later, so a failed step is undone with the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    /// The app hides.
    HidApp,
    /// The window leaves the Dock, still hidden with its app.
    LeftDock,
    /// It moves onto the display, and its frame there is read back.
    Moved,
    /// The app shows again, and the window is on the display.
    Shown,
}

impl Step {
    /// Every step, in the order they are tried.
    pub const ORDER: [Step; 4] = [Step::HidApp, Step::LeftDock, Step::Moved, Step::Shown];

    /// What undoes this step alone, up to the move.
    fn inverse(self) -> Option<Undo> {
        match self {
            Step::HidApp => Some(Undo::ShowApp),
            Step::LeftDock => Some(Undo::Minimize),
            Step::Moved => Some(Undo::RestoreFrame),
            Step::Shown => None,
        }
    }
}

/// One action that puts the window back where the user left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Undo {
    HideApp,
    /// Back to the frame it had in the Dock.
    RestoreFrame,
    Minimize,
    ShowApp,
}

/// What undoes the steps up to `reached` after a later one failed. The
/// app is as `app` reads now, without the window coming out. Up to the
/// move, the app is still hidden and nothing showed. Once the window is
/// out, it goes back as it would at the end ([`put_back`]).
pub fn undo(reached: Option<Step>, app: &AppState) -> Vec<Undo> {
    match reached {
        None => vec![],
        Some(Step::Shown) => put_back(app),
        // Each step's inverse, the last first.
        Some(last) => Step::ORDER
            .iter()
            .filter(|s| **s <= last)
            .rev()
            .filter_map(|s| s.inverse())
            .collect(),
    }
}

/// How a window brought out of the Dock goes back there: at its old frame,
/// minimized. Hidden with its app, it doesn't play the Dock's animation.
/// An app that may not hide now, being in front or showing other windows,
/// plays it from the Agent Display, the window minimized before its frame
/// goes back so it never stands on the user's screen; it is left with the
/// user all the same. An app the user hid since keeps hiding it.
pub fn put_back(app: &AppState) -> Vec<Undo> {
    if !app.hidden && app.may_hide() {
        vec![
            Undo::HideApp,
            Undo::RestoreFrame,
            Undo::Minimize,
            Undo::ShowApp,
        ]
    } else {
        vec![Undo::Minimize, Undo::RestoreFrame]
    }
}

/// The windows off screen place_window may take, by window id and why
/// each is off screen, best first: those it can bring out, the oldest
/// first, an app's main window before the ones it opened from it. The
/// window server lists windows off screen in no useful order.
pub fn preference(away: &[(u32, PutAway)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..away.len()).filter(|&i| away[i].1.comes_out()).collect();
    order.sort_by_key(|&i| (away[i].1.rank(), away[i].0));
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(on_screen: bool, listed: bool, minimized: bool, spaces: i32) -> OtherWindow {
        OtherWindow {
            on_screen,
            listed,
            minimized,
            spaces,
            beside: false,
            menu_extra: false,
        }
    }

    fn app(hidden: bool, active: bool, others: Vec<OtherWindow>) -> AppState {
        AppState {
            hidden,
            active,
            others,
        }
    }

    #[test]
    fn a_window_vanishes_with_its_app_unless_minimized_or_kept_out_of_sight() {
        // On screen, at any level or size: a pill, a status-level panel.
        assert!(other(true, false, false, 1).would_vanish());
        assert!(other(true, true, false, 1).would_vanish());
        // Listed and not minimized, though not on screen: unclear.
        assert!(other(false, true, false, 0).would_vanish());
        // On another Space, or nobody knows.
        assert!(other(false, false, false, 1).would_vanish());
        assert!(other(false, false, false, -1).would_vanish());
        // Minimized stays in the Dock. Ordered out, as an app's helper
        // window, on no Space.
        assert!(!other(false, true, true, 1).would_vanish());
        assert!(!other(false, false, false, 0).would_vanish());
    }

    #[test]
    fn a_menu_bar_icon_stays_when_its_app_hides_so_it_refuses_nothing() {
        let main = Rect::new(0.0, 0.0, 1728.0, 1117.0);
        let side = Rect::new(1728.0, -400.0, 2560.0, 1440.0);
        let displays = [main, side];
        // A status item in either display's menu bar, notch height included.
        assert!(is_menu_extra(
            25,
            Rect::new(1500.0, 0.0, 28.0, 24.0),
            &displays
        ));
        assert!(is_menu_extra(
            25,
            Rect::new(1400.0, 0.0, 30.0, 37.0),
            &displays
        ));
        assert!(is_menu_extra(
            25,
            Rect::new(3800.0, -400.0, 28.0, 24.0),
            &displays
        ));
        // The same layer elsewhere is a panel the user sees, or one kept out
        // of sight off every display: hiding the app takes it.
        assert!(!is_menu_extra(
            25,
            Rect::new(200.0, 300.0, 30.0, 30.0),
            &displays
        ));
        assert!(!is_menu_extra(
            25,
            Rect::new(-30000.0, -30000.0, 30.0, 30.0),
            &displays
        ));
        // In the menu bar at another layer: a window like any other.
        assert!(!is_menu_extra(
            0,
            Rect::new(1500.0, 0.0, 28.0, 24.0),
            &displays
        ));
        let icon = OtherWindow {
            menu_extra: true,
            ..other(true, false, false, 1)
        };
        assert!(!icon.would_vanish());
        assert_eq!(may_bring_out(true, &app(false, false, vec![icon])), Ok(()));
    }

    #[test]
    fn only_a_minimized_window_of_an_app_in_the_background_with_nothing_else_comes_out() {
        let helper = other(false, false, false, 0);
        let docked = other(false, true, true, 1);
        assert_eq!(may_bring_out(true, &app(false, false, vec![])), Ok(()));
        assert_eq!(
            may_bring_out(true, &app(false, false, vec![helper, docked])),
            Ok(())
        );
        // An app the user hid stays hidden, whatever its windows.
        assert_eq!(
            may_bring_out(true, &app(true, false, vec![])),
            Err(Refusal::AppHidden)
        );
        assert_eq!(
            may_bring_out(false, &app(true, false, vec![])),
            Err(Refusal::AppHidden)
        );
        assert_eq!(
            may_bring_out(false, &app(false, false, vec![])),
            Err(Refusal::NotMinimized)
        );
        assert_eq!(
            may_bring_out(true, &app(false, true, vec![])),
            Err(Refusal::AppInFront)
        );
        assert_eq!(
            may_bring_out(true, &app(false, false, vec![other(true, false, false, 1)])),
            Err(Refusal::OtherWindows)
        );
        assert_eq!(
            may_bring_out(
                true,
                &app(false, false, vec![other(false, false, false, 1)])
            ),
            Err(Refusal::OtherWindows)
        );
    }

    #[test]
    fn every_refusal_tells_the_agent_to_ask_the_user() {
        for refusal in [
            Refusal::AppHidden,
            Refusal::NotMinimized,
            Refusal::AppInFront,
            Refusal::OtherWindows,
        ] {
            assert!(refusal.reason().contains("Ask the user"), "{refusal:?}");
        }
        assert!(Refusal::AppHidden.reason().contains("hidden"));
        assert!(Refusal::OtherWindows.reason().contains("other windows"));
    }

    #[test]
    fn a_failure_undoes_every_step_before_it_while_the_app_is_still_hidden() {
        let background = app(false, false, vec![]);
        assert_eq!(undo(None, &background), vec![]);
        assert_eq!(undo(Some(Step::HidApp), &background), vec![Undo::ShowApp]);
        assert_eq!(
            undo(Some(Step::LeftDock), &background),
            vec![Undo::Minimize, Undo::ShowApp]
        );
        assert_eq!(
            undo(Some(Step::Moved), &background),
            vec![Undo::RestoreFrame, Undo::Minimize, Undo::ShowApp]
        );
        // Once out, it goes back as at the end.
        assert_eq!(undo(Some(Step::Shown), &background), put_back(&background));
    }

    #[test]
    fn a_step_that_failed_or_timed_out_is_undone_with_every_one_before_it() {
        let background = app(false, false, vec![]);
        for (i, step) in Step::ORDER.iter().enumerate() {
            let undo = undo(Some(*step), &background);
            for earlier in &Step::ORDER[..=i] {
                if let Some(inverse) = earlier.inverse() {
                    assert!(undo.contains(&inverse), "{step:?} misses {inverse:?}");
                }
            }
            // The app shows last, the window minimized before it does.
            if let Some(show) = undo.iter().position(|u| *u == Undo::ShowApp) {
                assert_eq!(show, undo.len() - 1, "{step:?}: {undo:?}");
            }
        }
        // Taking it out of the Dock timed out, yet the app may still do it:
        // the window goes back in before the app shows.
        assert_eq!(
            undo(Some(Step::LeftDock), &background),
            vec![Undo::Minimize, Undo::ShowApp]
        );
    }

    #[test]
    fn a_window_goes_back_to_the_dock_hidden_with_its_app_when_it_may_hide() {
        assert_eq!(
            put_back(&app(false, false, vec![other(false, false, false, 0)])),
            vec![
                Undo::HideApp,
                Undo::RestoreFrame,
                Undo::Minimize,
                Undo::ShowApp
            ]
        );
        // Beside it on the Agent Display it leaves, such as a text field's
        // input indicator: nothing on the user's screens hides.
        let indicator = OtherWindow {
            beside: true,
            ..other(true, true, false, 1)
        };
        assert!(!indicator.would_vanish());
        assert_eq!(
            put_back(&app(false, false, vec![indicator]))[0],
            Undo::HideApp
        );
        // In front, or showing another window now: into the Dock from the
        // Agent Display, then its frame goes back, so it never stands on
        // the user's screen.
        for now in [
            app(false, true, vec![]),
            app(false, false, vec![other(true, true, false, 1)]),
        ] {
            assert_eq!(put_back(&now), vec![Undo::Minimize, Undo::RestoreFrame]);
        }
        // The user hid the app since: it stays hidden.
        assert_eq!(
            put_back(&app(true, false, vec![])),
            vec![Undo::Minimize, Undo::RestoreFrame]
        );
    }

    #[test]
    fn the_oldest_minimized_window_comes_first_whatever_the_list_order() {
        let away = [
            (90, PutAway::Minimized),
            (40, PutAway::Closed),
            (70, PutAway::Minimized),
            (50, PutAway::OtherSpace),
        ];
        assert_eq!(preference(&away), vec![2, 0]);
        let mut reversed = away;
        reversed.reverse();
        let ids: Vec<u32> = preference(&reversed)
            .iter()
            .map(|&i| reversed[i].0)
            .collect();
        assert_eq!(ids, vec![70, 90]);
        // A hidden app's window doesn't come out, nor does a closed one.
        assert!(preference(&[(1, PutAway::Hidden), (2, PutAway::Closed)]).is_empty());
    }
}
