//! Debug-only fixture preparation. Hardware claims do not grant session ownership.

/// A debug grant is failure-atomic. Unlike production desired changes,
/// an unacknowledged fixture must not leave a claim for a later unrelated
/// publish. Keep pre-existing desired state intact until this grant commits.
pub fn publish_fixture_claim<E>(
    state: &mut crate::claims::Publication,
    window: u32,
    display: u32,
    persist: impl FnOnce(&crate::claims::Claims) -> Result<(), E>,
) -> Result<Option<u64>, E> {
    if !state.desired().has_display(display) {
        return Ok(None);
    }
    let mut next = state.clone();
    next.publish(true, |c| c.claim(window, display), persist)?;
    let generation = next.desired().generation_of(window);
    *state = next;
    Ok(generation)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pid: i32,
    pub started: u64,
    pub window: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Prepared,
    Claimed,
    Shown,
    Cancelled,
}

/// Result of a process identity query, distinct from window visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Running(u64),
    Exited,
    Unknown,
}
impl ProcessState {
    pub fn confirmed_gone(self, expected: u64) -> bool {
        match self {
            Self::Running(actual) => actual != expected,
            Self::Exited => true,
            Self::Unknown => false,
        }
    }
}

/// A failed process query cannot authorize release, even if the window
/// query also fails. A different known window owner can prove id reuse
/// only while the original process identity is itself known.
pub fn may_release_fixture(
    expected: Identity,
    process: ProcessState,
    window_owner: Option<i32>,
) -> bool {
    process.confirmed_gone(expected.started)
        || (process == ProcessState::Running(expected.started)
            && window_owner.is_some_and(|pid| pid != expected.pid))
}

// Deliberately no Debug: tokens must not enter logs.
pub struct Gate {
    token: String,
    deadline: u64,
    lifetime: u64,
    phase: Phase,
    pub binding: Option<(Identity, u64)>,
}

impl Gate {
    pub fn new(token: String, now: u64) -> Self {
        Self {
            token,
            deadline: now + 10_000,
            lifetime: now + 60_000,
            phase: Phase::Prepared,
            binding: None,
        }
    }

    pub fn authorize(&mut self, token: &str, now: u64) -> Result<(), &'static str> {
        if now >= self.deadline {
            self.phase = Phase::Cancelled;
        }
        if token != self.token || self.phase == Phase::Cancelled {
            return Err("stage gate rejected or expired");
        }
        Ok(())
    }

    pub fn claim<E>(
        &mut self,
        token: &str,
        now: u64,
        identity: Identity,
        proof: bool,
        publish: impl FnOnce() -> Result<u64, E>,
    ) -> Result<(), &'static str> {
        self.authorize(token, now)?;
        if self.phase != Phase::Prepared || !proof {
            return Err("stage fixture identity rejected");
        }
        let generation = publish().map_err(|_| "stage claim publication failed")?;
        self.binding = Some((identity, generation));
        self.phase = Phase::Claimed;
        Ok(())
    }

    pub fn show(
        &mut self,
        token: &str,
        now: u64,
        identity: Identity,
        protected: bool,
    ) -> Result<(), &'static str> {
        self.authorize(token, now)?;
        if !protected || self.phase != Phase::Claimed || self.binding.map(|b| b.0) != Some(identity)
        {
            return Err("stage show identity or phase rejected");
        }
        self.phase = Phase::Shown;
        self.deadline = self.lifetime;
        Ok(())
    }

    pub fn cleanup_token(&self, token: &str) -> bool {
        token == self.token
    }

    /// A Handover may publish its target claim before its AX move finishes.
    /// Keep the source display until geometry confirms that move, even
    /// after EOF or timeout. Unknown geometry cannot authorize removal.
    pub fn needs_source_display(&self, process: ProcessState, on_source: Option<bool>) -> bool {
        let Some((expected, _)) = self.binding else {
            return false;
        };
        match process {
            ProcessState::Unknown => true,
            ProcessState::Running(started) if started == expected.started => {
                on_source != Some(false)
            }
            ProcessState::Exited | ProcessState::Running(_) => false,
        }
    }

    /// EOF, stage release, or connection closure can only cancel, never show.
    pub fn cancel(&mut self) {
        self.phase = Phase::Cancelled;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        claims::{Claims, Publication},
        model::{DisplayRole, Displays, Origin, Owner, WindowTarget},
    };

    fn identity() -> Identity {
        Identity {
            pid: 9,
            started: 100,
            window: 7,
        }
    }
    fn gate() -> Gate {
        Gate::new("local-test-token".into(), 0)
    }

    #[test]
    fn stage_publication_can_retry_and_acknowledge_only_the_successful_grant() {
        let mut state = Publication::new();
        state
            .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
            .unwrap();
        let mut g = gate();
        assert!(
            g.claim("local-test-token", 1, identity(), true, || {
                publish_fixture_claim(&mut state, 7, 3, |_| Err("disk full"))
                    .map(|generation| generation.unwrap())
            })
            .is_err()
        );
        assert!(g.binding.is_none());
        assert!(g.show("local-test-token", 2, identity(), true).is_err());
        g.claim("local-test-token", 3, identity(), true, || {
            publish_fixture_claim(&mut state, 7, 3, |_| Ok::<_, &str>(()))
                .map(|generation| generation.unwrap())
        })
        .unwrap();
        assert_eq!(state.desired().display_of(7), Some(3));
        assert_eq!(state.published().display_of(7), Some(3));
        assert_eq!(
            g.binding.unwrap().1,
            state.desired().generation_of(7).unwrap()
        );
        g.show("local-test-token", 4, identity(), true).unwrap();
    }

    #[test]
    fn failed_stage_publication_cannot_escape_after_fixture_exit_or_cancellation() {
        for cancelled in [false, true] {
            let mut state = Publication::new();
            state
                .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
                .unwrap();
            // Ordinary production desired state must keep its retry semantics.
            assert!(
                state
                    .publish(false, |c| c.add_display(4), |_| Err("disk full"))
                    .is_err()
            );
            {
                let mut g = gate();
                assert!(
                    g.claim("local-test-token", 1, identity(), true, || {
                        publish_fixture_claim(&mut state, 7, 3, |_| Err("stage write failed"))
                            .map(|generation| generation.unwrap())
                    })
                    .is_err()
                );
                assert!(g.binding.is_none());
                if cancelled {
                    g.cancel();
                }
                let reused_id = Identity {
                    pid: 99,
                    started: 200,
                    ..identity()
                };
                assert!(g.show("local-test-token", 2, reused_id, true).is_err());
            } // The exited/cancelled fixture has no binding for CAS cleanup.
            let mut file = String::new();
            state
                .publish(
                    false,
                    |c| c.claim(8, 4),
                    |c| {
                        file = c.to_json();
                        Ok::<_, &str>(())
                    },
                )
                .unwrap();
            assert_eq!(
                state.desired().display_of(7),
                None,
                "failed stage id must not protect its new user owner"
            );
            assert_eq!(state.published().display_of(7), None);
            assert_eq!(state.published().display_of(8), Some(4));
            assert_eq!(file, r#"{"version":1,"displays":[3,4],"windows":[8]}"#);
        }
    }

    #[test]
    fn failed_stage_publication_preserves_handover_and_reused_id_claims() {
        for destination in [4, 3] {
            let mut state = Publication::new();
            state
                .publish(false, |c| c.add_display(3), |_| Ok::<_, &str>(()))
                .unwrap();
            state
                .publish(false, |c| c.add_display(4), |_| Ok::<_, &str>(()))
                .unwrap();
            state
                .publish(false, |c| c.claim(7, 3), |_| Ok::<_, &str>(()))
                .unwrap();
            let old_stage = state.desired().generation_of(7).unwrap();
            // A normal Handover, or a new fixture reusing id 7 on the stage.
            state
                .publish(false, |c| c.claim(7, destination), |_| Ok::<_, &str>(()))
                .unwrap();
            let current = state.desired().generation_of(7).unwrap();
            let desired = state.desired().clone();
            let published = state.published().clone();
            assert!(
                publish_fixture_claim(&mut state, 7, 3, |_| Err("stage write failed")).is_err()
            );
            assert_eq!(state.desired(), &desired);
            assert_eq!(state.published(), &published);
            state
                .publish(
                    false,
                    |c| {
                        assert!(!c.release_if(7, 3, old_stage));
                        c.claim(8, 4)
                    },
                    |_| Ok::<_, &str>(()),
                )
                .unwrap();
            assert_eq!(state.desired().display_of(7), Some(destination));
            assert_eq!(state.desired().generation_of(7), Some(current));
            assert_eq!(state.published().display_of(7), Some(destination));
        }
    }

    #[test]
    fn a_failed_process_query_cannot_authorize_claim_cleanup() {
        let fake_query = || ProcessState::Unknown;
        assert!(!fake_query().confirmed_gone(identity().started));
        let mut claims = Claims::new();
        claims.add_display(3);
        claims.claim(7, 3);
        let generation = claims.generation_of(7).unwrap();
        for window_owner in [None, Some(identity().pid), Some(99)] {
            if may_release_fixture(identity(), fake_query(), window_owner) {
                claims.release_if(7, 3, generation);
            }
            assert_eq!(
                claims.display_of(7),
                Some(3),
                "unknown process query must retain the claim"
            );
        }
        let mut g = gate();
        g.claim("local-test-token", 1, identity(), true, || {
            Ok::<_, ()>(generation)
        })
        .unwrap();
        g.cancel();
        assert!(g.needs_source_display(fake_query(), Some(false)));
        assert!(g.needs_source_display(fake_query(), None));
        assert!(!may_release_fixture(
            identity(),
            ProcessState::Running(100),
            None
        ));
        assert!(may_release_fixture(identity(), ProcessState::Exited, None));
        assert!(may_release_fixture(
            identity(),
            ProcessState::Running(101),
            None
        ));
        assert!(ProcessState::Exited.confirmed_gone(identity().started));
        assert!(ProcessState::Running(101).confirmed_gone(identity().started));
        assert!(!ProcessState::Running(100).confirmed_gone(identity().started));
    }

    #[test]
    fn only_a_verified_successfully_published_claim_can_allow_show() {
        let mut g = gate();
        assert!(g.show("local-test-token", 1, identity(), true).is_err());
        assert!(
            g.claim("wrong", 1, identity(), true, || Ok::<_, ()>(1))
                .is_err()
        );
        assert!(
            g.claim(
                "local-test-token",
                1,
                identity(),
                false,
                || -> Result<u64, ()> { panic!("must not publish") }
            )
            .is_err()
        );
        assert!(
            g.claim("local-test-token", 1, identity(), true, || Err(()))
                .is_err()
        );
        assert!(g.binding.is_none());
        assert!(g.show("local-test-token", 2, identity(), true).is_err());
        g.claim("local-test-token", 3, identity(), true, || Ok::<_, ()>(1))
            .unwrap();
        assert!(
            g.claim(
                "local-test-token",
                4,
                identity(),
                true,
                || -> Result<u64, ()> { panic!("duplicate publication") }
            )
            .is_err()
        );
        assert!(
            g.show(
                "local-test-token",
                5,
                Identity {
                    started: 101,
                    ..identity()
                },
                true,
            )
            .is_err()
        );
        assert!(g.show("local-test-token", 5, identity(), false).is_err());
        g.show("local-test-token", 6, identity(), true).unwrap();
        assert!(g.show("local-test-token", 7, identity(), true).is_err());
        assert!(g.authorize("local-test-token", 60_000).is_err());
    }

    #[test]
    fn eof_or_handover_cannot_drop_the_source_display_before_the_move_finishes() {
        let mut g = gate();
        g.claim("local-test-token", 1, identity(), true, || Ok::<_, ()>(1))
            .unwrap();
        g.cancel();
        assert!(g.needs_source_display(ProcessState::Running(100), Some(true)));
        assert!(g.needs_source_display(ProcessState::Running(100), None));
        assert!(!g.needs_source_display(ProcessState::Running(100), Some(false)));
        assert!(!g.needs_source_display(ProcessState::Exited, None));
        assert!(g.cleanup_token("local-test-token"));
        assert!(!g.cleanup_token("wrong"));
    }

    #[test]
    fn timeout_and_eof_are_terminal_before_and_after_claim() {
        for claimed in [false, true] {
            for eof in [false, true] {
                let mut g = gate();
                if claimed {
                    g.claim("local-test-token", 1, identity(), true, || Ok::<_, ()>(1))
                        .unwrap();
                }
                if eof {
                    g.cancel();
                } else {
                    assert!(g.authorize("local-test-token", 10_000).is_err());
                }
                assert!(
                    g.show("local-test-token", 10_001, identity(), true)
                        .is_err()
                );
                assert!(
                    g.claim(
                        "local-test-token",
                        10_001,
                        identity(),
                        true,
                        || -> Result<u64, ()> { panic!("cancelled") }
                    )
                    .is_err()
                );
            }
        }
    }

    #[test]
    fn a_stage_hardware_claim_does_not_change_the_targets_candidates_or_ownership() {
        let mut displays = Displays::default();
        // These are the same model methods used by the app, not a second test rule.
        displays.add(
            4,
            "Target".into(),
            (1280.0, 800.0),
            Some(Owner {
                session_key: "target".into(),
                client: "test".into(),
                title: "test".into(),
                connection: 1,
            }),
            (),
        );
        let target = WindowTarget {
            pid: 9,
            window_id: 7,
        };
        let mut claims = Claims::new();
        claims.add_display(3);
        claims.add_display(4);
        for source in [
            DisplayRole::from_hardware(0x610, 1, false),
            DisplayRole::from_hardware(0x5353, 1, true),
        ] {
            for hardware_claim in [false, true] {
                if hardware_claim {
                    claims.claim(7, 3);
                }
                let d = displays.get_mut(4).unwrap();
                assert!(source.accepts_window_request());
                assert!(!d.is_stray_from(target, false, source, true));
                assert!(d.started.is_empty());
                assert!(d.records.is_empty());
            }
        }
        let generation = claims.generation_of(7).unwrap();
        displays
            .get_mut(4)
            .unwrap()
            .claim(target, Origin::HandedOver, None);
        claims.claim(7, 4);
        assert!(!claims.release_if(7, 3, generation));
        assert_eq!(
            displays.get(4).unwrap().records[&7].origin,
            Origin::HandedOver
        );
        assert!(displays.get(4).unwrap().started.is_empty());
    }
}
