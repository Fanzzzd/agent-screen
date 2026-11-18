//! What Agent Screen tells a window manager is its own: the displays it made
//! and the windows on them that are the agents'. AeroSpace leaves both alone
//! while the claim is live (the fork's AGENT-SCREEN.md, Claims).
//!
//! A window is claimed before it moves onto a claimed display and keeps its
//! claim until it is off all of them. AeroSpace leaves an unclaimed window
//! where its app put it once it has taken it back before, so a window
//! released while still on an Agent Display can stay there, out of sight.

use crate::model::Rect;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Claims {
    displays: BTreeSet<u32>,
    /// Each claimed window, by the display it is on or on its way to.
    windows: BTreeMap<u32, u32>,
}

impl Claims {
    pub const fn new() -> Self {
        Claims {
            displays: BTreeSet::new(),
            windows: BTreeMap::new(),
        }
    }

    pub fn add_display(&mut self, display: u32) -> bool {
        self.displays.insert(display)
    }

    /// A display gone: its windows are nobody's to claim any more.
    pub fn remove_display(&mut self, display: u32) -> bool {
        let had = self.displays.remove(&display);
        let before = self.windows.len();
        self.windows.retain(|_, d| *d != display);
        had || self.windows.len() != before
    }

    pub fn display_of(&self, window: u32) -> Option<u32> {
        self.windows.get(&window).copied()
    }

    pub fn has_display(&self, display: u32) -> bool {
        self.displays.contains(&display)
    }

    /// Claim a window going onto one of the claimed displays. A window going
    /// anywhere else isn't claimed.
    pub fn claim(&mut self, window: u32, display: u32) -> bool {
        self.displays.contains(&display) && self.windows.insert(window, display) != Some(display)
    }

    /// The claimed display whose bounds are `target`, among the `online`
    /// displays and their bounds.
    pub fn display_at(&self, target: Rect, online: &[(u32, Rect)]) -> Option<u32> {
        online
            .iter()
            .find(|&&(display, bounds)| self.displays.contains(&display) && bounds == target)
            .map(|&(display, _)| display)
    }

    /// The bounds of the claimed displays among the `online` ones.
    pub fn claimed_bounds(&self, online: &[(u32, Rect)]) -> Vec<Rect> {
        online
            .iter()
            .filter(|(display, _)| self.displays.contains(display))
            .map(|&(_, bounds)| bounds)
            .collect()
    }

    /// A window about to move into `target`. Going onto a claimed display,
    /// it is claimed for that display first, and a claim for another one
    /// follows it there. Going anywhere else, it keeps the claim it has
    /// until [`after_leaving`] says it is off. True when the claims changed.
    pub fn moving_into(&mut self, window: u32, target: Rect, online: &[(u32, Rect)]) -> bool {
        self.display_at(target, online)
            .is_some_and(|display| self.claim(window, display))
    }

    pub fn release(&mut self, window: u32) -> bool {
        self.windows.remove(&window).is_some()
    }

    /// Nothing to claim: the claim file should not exist.
    pub fn is_empty(&self) -> bool {
        self.displays.is_empty() && self.windows.is_empty()
    }

    /// The claim file's contents, version 1.
    pub fn to_json(&self) -> String {
        let list = |ids: &mut dyn Iterator<Item = u32>| {
            ids.map(|id| id.to_string()).collect::<Vec<_>>().join(",")
        };
        format!(
            "{{\"version\":1,\"displays\":[{}],\"windows\":[{}]}}",
            list(&mut self.displays.iter().copied()),
            list(&mut self.windows.keys().copied())
        )
    }
}

/// What becomes of a window's claim after a move meant to take it off the
/// claimed displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterLeaving {
    /// Its frame is off every claimed display: the claim goes.
    Release,
    /// The move failed, the window is still on a claimed display, or where
    /// it is can't be read: the claim stays, so AeroSpace leaves the window
    /// alone instead of stranding it on a display the user can't see.
    Keep,
}

/// Whether a window that was to leave the claimed displays, with `frame`
/// now, is off them: its middle is on none of `claimed`, the way AeroSpace
/// places a window on a monitor. A window sent to an AeroSpace workspace
/// counts as off once AeroSpace files it there (`filed`), before its frame
/// follows: AeroSpace lays it out on that workspace's monitor, and a claim
/// back would pull it out of the workspace again.
pub fn after_leaving(
    moved: bool,
    frame: Option<Rect>,
    claimed: &[Rect],
    filed: bool,
) -> AfterLeaving {
    let off = frame.is_some_and(|frame| !claimed.iter().any(|b| b.contains(frame.mid())));
    if moved && (off || filed) {
        AfterLeaving::Release
    } else {
        AfterLeaving::Keep
    }
}

/// Desired claims survive a failed publication. Only `published` describes
/// the last successful file, so even an idempotent later operation retries
/// while desired state and that snapshot differ.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Publication {
    desired: Claims,
    published: Claims,
}

impl Publication {
    pub const fn new() -> Self {
        Self {
            desired: Claims::new(),
            published: Claims::new(),
        }
    }

    pub fn desired(&self) -> &Claims {
        &self.desired
    }
    pub fn published(&self) -> &Claims {
        &self.published
    }

    pub fn publish<E>(
        &mut self,
        force: bool,
        apply: impl FnOnce(&mut Claims) -> bool,
        persist: impl FnOnce(&Claims) -> Result<(), E>,
    ) -> Result<(), E> {
        apply(&mut self.desired);
        if force || self.desired.to_json() != self.published.to_json() {
            persist(&self.desired)?;
        }
        // The file names windows, not their displays: a claimed window
        // claimed for another display leaves it as it is, and it holds the
        // new claim too. Kept stale, a Handover going home to the user
        // stand-in stage read as not claimed there and stayed put.
        self.published = self.desired.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_display_registration_recovers_on_a_later_claim() {
        let mut state = Publication::new();
        assert!(
            state
                .publish(false, |c| c.add_display(3), |_| Err("disk full"))
                .is_err()
        );
        assert!(state.desired().has_display(3));
        assert!(!state.published().has_display(3));
        let mut file = String::new();
        state
            .publish(
                false,
                |c| c.claim(7, 3),
                |c| {
                    file = c.to_json();
                    Ok::<_, &str>(())
                },
            )
            .unwrap();
        assert_eq!(state.desired().display_of(7), Some(3));
        assert_eq!(file, r#"{"version":1,"displays":[3],"windows":[7]}"#);
        assert_eq!(state.desired(), state.published());
    }

    #[test]
    fn an_identical_claim_retries_after_failure_and_keeps_the_published_snapshot() {
        let mut state = Publication::new();
        state
            .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
            .unwrap();
        let file = state.published().to_json();
        assert!(
            state
                .publish(false, |c| c.claim(7, 3), |_| Err("disk full"))
                .is_err()
        );
        assert_eq!(state.desired().display_of(7), Some(3));
        assert_eq!(state.published().to_json(), file);
        let mut writes = 0;
        state
            .publish(
                false,
                |c| c.claim(7, 3),
                |_| {
                    writes += 1;
                    Ok::<_, &str>(())
                },
            )
            .unwrap();
        assert_eq!(writes, 1);
        assert_eq!(state.published().display_of(7), Some(3));
    }

    #[test]
    fn failed_removal_and_forced_flush_can_retry_without_losing_desired_state() {
        let mut state = Publication::new();
        state
            .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
            .unwrap();
        assert!(
            state
                .publish(false, |c| c.remove_display(3), |_| Err("unlink failed"))
                .is_err()
        );
        assert!(!state.desired().has_display(3));
        assert!(state.published().has_display(3));
        state
            .publish(false, |c| c.remove_display(3), |_| Ok::<_, &str>(()))
            .unwrap();
        assert!(!state.published().has_display(3));
        assert!(
            state
                .publish(true, |_| false, |_| Err("forced retry failed"))
                .is_err()
        );
        let mut writes = 0;
        state
            .publish(
                true,
                |_| false,
                |_| {
                    writes += 1;
                    Ok::<_, &str>(())
                },
            )
            .unwrap();
        assert_eq!(writes, 1);
    }

    #[test]
    fn claims_only_windows_going_onto_its_displays() {
        let mut claims = Claims::default();
        assert!(!claims.claim(7, 3));
        claims.add_display(3);
        assert!(claims.claim(7, 3));
        assert!(!claims.claim(7, 3));
        assert_eq!(claims.display_of(7), Some(3));
        assert_eq!(
            claims.to_json(),
            r#"{"version":1,"displays":[3],"windows":[7]}"#
        );
    }

    const AGENT: Rect = Rect::new(5000.0, 0.0, 1280.0, 800.0);
    const OTHER_AGENT: Rect = Rect::new(7000.0, 0.0, 1280.0, 800.0);
    const USER: Rect = Rect::new(0.0, 0.0, 1728.0, 1117.0);
    const ONLINE: [(u32, Rect); 3] = [(1, USER), (3, AGENT), (4, OTHER_AGENT)];

    fn claimed_on_3() -> Claims {
        let mut claims = Claims::default();
        claims.add_display(3);
        claims.add_display(4);
        claims.claim(7, 3);
        claims
    }

    #[test]
    fn a_window_leaving_for_the_users_screen_keeps_its_claim_on_the_way() {
        let mut claims = claimed_on_3();
        assert!(!claims.moving_into(7, USER, &ONLINE));
        assert_eq!(claims.display_of(7), Some(3));
    }

    #[test]
    fn a_window_moving_between_agent_displays_takes_its_claim_along() {
        let mut claims = claimed_on_3();
        assert!(claims.moving_into(7, OTHER_AGENT, &ONLINE));
        assert_eq!(claims.display_of(7), Some(4));
        // A window not claimed yet is claimed before it goes onto one.
        assert!(claims.moving_into(8, AGENT, &ONLINE));
        assert_eq!(claims.display_of(8), Some(3));
    }

    #[test]
    fn a_window_that_made_it_off_every_agent_display_is_released() {
        let claims = claimed_on_3();
        let claimed = claims.claimed_bounds(&ONLINE);
        assert_eq!(claimed, vec![AGENT, OTHER_AGENT]);
        let home = Rect::new(100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(true, Some(home), &claimed, false),
            AfterLeaving::Release
        );
    }

    #[test]
    fn a_failed_move_keeps_the_claim() {
        let claimed = claimed_on_3().claimed_bounds(&ONLINE);
        let still = Rect::new(5100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(false, Some(still), &claimed, false),
            AfterLeaving::Keep
        );
        // Even when the window ended up off by some other means: the move
        // didn't say so.
        let home = Rect::new(100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(false, Some(home), &claimed, false),
            AfterLeaving::Keep
        );
    }

    #[test]
    fn a_window_moved_but_still_on_an_agent_display_keeps_its_claim() {
        let claimed = claimed_on_3().claimed_bounds(&ONLINE);
        // The app put it back where it was.
        let back = Rect::new(5100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(true, Some(back), &claimed, false),
            AfterLeaving::Keep
        );
        // Or it landed on the other Agent Display.
        let other = Rect::new(7100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(true, Some(other), &claimed, false),
            AfterLeaving::Keep
        );
        // Hanging over the user's screen with its middle still on the display.
        let straddling = Rect::new(4500.0, 100.0, 1200.0, 600.0);
        assert_eq!(
            after_leaving(true, Some(straddling), &claimed, false),
            AfterLeaving::Keep
        );
        // Its frame can't be read: nothing says it is off.
        assert_eq!(
            after_leaving(true, None, &claimed, false),
            AfterLeaving::Keep
        );
    }

    #[test]
    fn a_window_aerospace_filed_in_its_workspace_is_off_before_its_frame_moves() {
        let claimed = claimed_on_3().claimed_bounds(&ONLINE);
        let still = Rect::new(5100.0, 100.0, 800.0, 600.0);
        assert_eq!(
            after_leaving(true, Some(still), &claimed, true),
            AfterLeaving::Release
        );
        // The command failed: whatever AeroSpace says, the move didn't go.
        assert_eq!(
            after_leaving(false, Some(still), &claimed, true),
            AfterLeaving::Keep
        );
    }

    #[test]
    fn a_display_going_takes_its_windows_claims_with_it() {
        let mut claims = Claims::default();
        claims.add_display(3);
        claims.add_display(4);
        claims.claim(7, 3);
        claims.claim(8, 4);
        assert!(claims.remove_display(3));
        assert_eq!(
            claims.to_json(),
            r#"{"version":1,"displays":[4],"windows":[8]}"#
        );
        claims.release(8);
        claims.remove_display(4);
        assert!(claims.is_empty());
    }

    #[test]
    fn a_window_claimed_for_another_display_is_published_there() {
        let mut state = Publication::new();
        let writes = std::cell::Cell::new(0);
        let write = |_: &Claims| -> Result<(), ()> {
            writes.set(writes.get() + 1);
            Ok(())
        };
        state.publish(false, |c| c.add_display(3), write).unwrap();
        state.publish(false, |c| c.add_display(4), write).unwrap();
        state.publish(false, |c| c.claim(7, 4), write).unwrap();
        assert_eq!(writes.get(), 3);
        // Home to display 3: the file reads the same, and isn't written.
        state.publish(false, |c| c.claim(7, 3), write).unwrap();
        assert_eq!(writes.get(), 3);
        assert_eq!(state.published().display_of(7), Some(3));
    }
}
