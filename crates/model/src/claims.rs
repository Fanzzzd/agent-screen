//! What Agent Screen tells a window manager is its own: the displays it made
//! and the windows on them that are the agents'. AeroSpace leaves both alone
//! while the claim is live (the fork's AGENT-SCREEN.md, Claims).

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Claims {
    displays: BTreeSet<u32>,
    /// Each claimed window, by the display it is on or on its way to.
    windows: BTreeMap<u32, u32>,
    #[cfg(debug_assertions)]
    generations: BTreeMap<u32, u64>,
    #[cfg(debug_assertions)]
    generation: u64,
}

impl Claims {
    pub const fn new() -> Self {
        Claims {
            displays: BTreeSet::new(),
            windows: BTreeMap::new(),
            #[cfg(debug_assertions)]
            generations: BTreeMap::new(),
            #[cfg(debug_assertions)]
            generation: 0,
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
        #[cfg(debug_assertions)]
        self.generations.retain(|w, _| self.windows.contains_key(w));
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
        if !self.displays.contains(&display) {
            return false;
        }
        #[cfg(debug_assertions)]
        {
            self.generation = self
                .generation
                .checked_add(1)
                .expect("claim generation exhausted");
            self.generations.insert(window, self.generation);
        }
        self.windows.insert(window, display) != Some(display)
    }

    pub fn release(&mut self, window: u32) -> bool {
        #[cfg(debug_assertions)]
        self.generations.remove(&window);
        self.windows.remove(&window).is_some()
    }

    #[cfg(debug_assertions)]
    pub fn generation_of(&self, window: u32) -> Option<u64> {
        self.generations.get(&window).copied()
    }

    /// A cancelled stage must not release a later claim, even if a window
    /// id was reused on the same display.
    #[cfg(debug_assertions)]
    pub fn release_if(&mut self, window: u32, display: u32, generation: u64) -> bool {
        self.display_of(window) == Some(display)
            && self.generation_of(window) == Some(generation)
            && self.release(window)
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
            self.published = self.desired.clone();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(debug_assertions)]
    #[test]
    fn stage_cleanup_cannot_release_a_handover_or_a_reused_window_id() {
        let mut c = Claims::new();
        c.add_display(3);
        c.add_display(4);
        c.claim(7, 3);
        let old = c.generation_of(7).unwrap();
        c.claim(7, 4);
        assert!(!c.release_if(7, 3, old));
        c.remove_display(3);
        assert_eq!(c.display_of(7), Some(4));
        c.add_display(3);
        c.release(7);
        c.claim(7, 3);
        assert!(!c.release_if(7, 3, old));
        assert!(c.release_if(7, 3, c.generation_of(7).unwrap()));
    }

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
}
