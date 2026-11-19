//! WindowServer's default for a set of displays that has no stored
//! arrangement: its "TV connect" policy, the `TVConnectPolicy` preference
//! of `com.apple.windowserver.displays` for the current user and host. 1
//! mirrors every display onto the new one, which is what flashed the user's
//! screen in #220. 0 extends the desktop onto it.
//!
//! Agent Screen sets `EXTEND` just before a display change that may bring a
//! new set and sets the earlier value back right after. Several instances
//! may do so at once, and an app may die while it holds the value, so the
//! record of who holds it and what to restore is kept on disk
//! (`PolicyHold`), and stored before the policy is set (`extend`).

/// The policy that extends the desktop onto a new display.
pub const EXTEND: i32 = 0;

/// A process holding the policy. Only a process with the same id, start
/// time and login session is the same process: a pid is reused, after a
/// restart often by a long-lived daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: u32,
    /// The process's start, in microseconds since 1970.
    pub started_us: u64,
    pub session: String,
    /// The Agent Screen instance, for the record's reader; empty for the
    /// user's own.
    pub instance: String,
}

/// Who set the policy to `EXTEND` and what it was before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicyHold {
    /// The value before Agent Screen set `EXTEND`. None while nothing is
    /// held.
    previous: Option<i32>,
    /// The processes changing displays meanwhile.
    holders: Vec<Holder>,
}

/// What the app and the OS do for `extend` and `release`: the seam tests
/// replace.
pub trait PolicySystem {
    /// The policy now; None when SkyLight doesn't say.
    fn read(&mut self) -> Option<i32>;
    /// Ask SkyLight to set the policy; whether it said it did.
    fn set(&mut self, policy: i32) -> bool;
    /// Store the record: written beside its file and renamed in, or the
    /// file removed for an empty record. Whether that worked.
    fn save(&mut self, hold: &PolicyHold) -> bool;
    /// Whether `holder` still runs.
    fn alive(&self, holder: &Holder) -> bool;
}

/// How `extend` went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extend {
    /// Set to `EXTEND` from `was`.
    Set { was: i32 },
    /// SkyLight didn't confirm `EXTEND`; held anyway, so `release` puts
    /// `was` back if it took.
    Unconfirmed { was: i32 },
    /// Another process holds it; held by this one too.
    Joined,
    /// The user's own policy extends already: nothing held.
    Already,
    /// The policy couldn't be read: nothing held or set.
    Unread,
    /// The record couldn't be stored: nothing held or set.
    Unsaved,
}

impl Extend {
    /// Whether `release` must follow.
    pub fn held(self) -> bool {
        matches!(
            self,
            Self::Set { .. } | Self::Unconfirmed { .. } | Self::Joined
        )
    }
}

/// How `release` went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// Set back to this.
    Restored(i32),
    /// Another live process still holds it.
    StillHeld,
    /// Nothing to restore, or the user changed it meanwhile.
    Nothing,
    /// The policy couldn't be read: the record stays for a later try.
    Unread,
    /// SkyLight didn't take this value back: the record keeps it for a
    /// later try.
    RestoreFailed(i32),
    /// The record couldn't be stored.
    Unsaved,
}

/// Set the policy to `EXTEND` for `me`, the record stored first: a crash
/// right after the set still leaves what to restore. A record that can't
/// be stored sets nothing.
pub fn extend(hold: &mut PolicyHold, me: &Holder, system: &mut impl PolicySystem) -> Extend {
    let Some(current) = system.read() else {
        return Extend::Unread;
    };
    let mut next = hold.clone();
    let set = next.begin(current, me, |h| system.alive(h));
    if next != *hold {
        if !system.save(&next) {
            return Extend::Unsaved;
        }
        *hold = next;
    }
    match set {
        Some(policy) => {
            if system.set(policy) && system.read() == Some(policy) {
                Extend::Set { was: current }
            } else {
                Extend::Unconfirmed { was: current }
            }
        }
        None if hold.holders.contains(me) => Extend::Joined,
        None => Extend::Already,
    }
}

/// `me` is done, or None at start-up: set the policy back once no live
/// process holds it, then store the record.
pub fn release(
    hold: &mut PolicyHold,
    me: Option<&Holder>,
    system: &mut impl PolicySystem,
) -> Release {
    let Some(current) = system.read() else {
        return Release::Unread;
    };
    let mut next = hold.clone();
    let restore = next.end(current, me, |h| system.alive(h));
    let mut failed = None;
    if let Some(policy) = restore
        && !(system.set(policy) && system.read() == Some(policy))
    {
        // Kept for the next start or the next display to try again.
        next.previous = Some(policy);
        failed = Some(policy);
    }
    if next != *hold {
        if !system.save(&next) {
            return Release::Unsaved;
        }
        *hold = next;
    }
    match (restore, failed) {
        (_, Some(policy)) => Release::RestoreFailed(policy),
        (Some(policy), None) => Release::Restored(policy),
        (None, None) if hold.holders.is_empty() => Release::Nothing,
        (None, None) => Release::StillHeld,
    }
}

impl PolicyHold {
    /// Process `me` is about to change displays, and the policy reads
    /// `current`. Returns the value to set: `EXTEND` when the policy would
    /// mirror. A value the user set while it was held becomes the one to
    /// restore. A record left by a holder no longer `alive` passes to `me`.
    pub fn begin(
        &mut self,
        current: i32,
        me: &Holder,
        alive: impl Fn(&Holder) -> bool,
    ) -> Option<i32> {
        self.holders.retain(|h| h != me && alive(h));
        if current == EXTEND {
            // Extending already: the user's own choice, or still held.
            if self.previous.is_some() {
                self.holders.push(me.clone());
            }
            return None;
        }
        self.previous = Some(current);
        self.holders.push(me.clone());
        Some(EXTEND)
    }

    /// Process `me` is done, or None at start-up, and the policy reads
    /// `current`. Returns the value to set back once no live holder is
    /// left. Nothing when the policy no longer reads `EXTEND`: the user
    /// changed it meanwhile, and their value stays.
    pub fn end(
        &mut self,
        current: i32,
        me: Option<&Holder>,
        alive: impl Fn(&Holder) -> bool,
    ) -> Option<i32> {
        self.holders.retain(|h| Some(h) != me && alive(h));
        if !self.holders.is_empty() {
            return None;
        }
        let previous = self.previous.take()?;
        (current == EXTEND).then_some(previous)
    }

    /// Whether the record holds nothing, so its file can go.
    pub fn is_empty(&self) -> bool {
        self.previous.is_none() && self.holders.is_empty()
    }

    pub fn to_json(&self) -> serde_json::Value {
        let holders: Vec<_> = self
            .holders
            .iter()
            .map(|h| {
                serde_json::json!({
                    "pid": h.pid,
                    "started_us": h.started_us,
                    "session": h.session,
                    "instance": h.instance,
                })
            })
            .collect();
        serde_json::json!({ "previous": self.previous, "holders": holders })
    }

    /// None for anything but what `to_json` writes.
    pub fn from_json(value: &serde_json::Value) -> Option<Self> {
        let previous = match value.get("previous")? {
            serde_json::Value::Null => None,
            v => Some(i32::try_from(v.as_i64()?).ok()?),
        };
        let holders = value
            .get("holders")?
            .as_array()?
            .iter()
            .map(|h| {
                Some(Holder {
                    pid: u32::try_from(h.get("pid")?.as_u64()?).ok()?,
                    started_us: h.get("started_us")?.as_u64()?,
                    session: h.get("session")?.as_str()?.to_string(),
                    instance: h.get("instance")?.as_str()?.to_string(),
                })
            })
            .collect::<Option<_>>()?;
        Some(Self { previous, holders })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIRROR: i32 = 1;

    fn holder(pid: u32) -> Holder {
        Holder {
            pid,
            started_us: 1_000 + u64::from(pid),
            session: "s".into(),
            instance: String::new(),
        }
    }

    /// WindowServer's policy and the record's file, with every call
    /// logged in order.
    struct Fake {
        policy: Option<i32>,
        /// What `set` does to `policy`: false for a set SkyLight ignores.
        sets_take: bool,
        /// What `set` answers.
        sets_answer: bool,
        saves_work: bool,
        saved: Option<PolicyHold>,
        dead: Vec<Holder>,
        calls: Vec<String>,
    }

    impl Fake {
        fn new(policy: i32) -> Self {
            Self {
                policy: Some(policy),
                sets_take: true,
                sets_answer: true,
                saves_work: true,
                saved: None,
                dead: Vec::new(),
                calls: Vec::new(),
            }
        }
    }

    impl PolicySystem for Fake {
        fn read(&mut self) -> Option<i32> {
            self.calls.push("read".into());
            self.policy
        }
        fn set(&mut self, policy: i32) -> bool {
            self.calls.push(format!("set {policy}"));
            if self.sets_take {
                self.policy = Some(policy);
            }
            self.sets_answer
        }
        fn save(&mut self, hold: &PolicyHold) -> bool {
            self.calls.push("save".into());
            if self.saves_work {
                self.saved = Some(hold.clone());
            }
            self.saves_work
        }
        fn alive(&self, holder: &Holder) -> bool {
            !self.dead.contains(holder)
        }
    }

    /// The record as the next process reads it from the file.
    fn reread(fake: &Fake) -> PolicyHold {
        let saved = fake.saved.clone().unwrap_or_default();
        PolicyHold::from_json(&saved.to_json()).expect("round trip")
    }

    #[test]
    fn a_mirroring_policy_extends_while_displays_change_then_comes_back() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        assert_eq!(
            extend(&mut hold, &holder(7), &mut fake),
            Extend::Set { was: MIRROR }
        );
        assert_eq!(fake.policy, Some(EXTEND));
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Restored(MIRROR)
        );
        assert_eq!(fake.policy, Some(MIRROR));
        assert!(reread(&fake).is_empty());
    }

    #[test]
    fn the_record_is_stored_before_the_policy_is_set() {
        let mut fake = Fake::new(MIRROR);
        extend(&mut PolicyHold::default(), &holder(7), &mut fake);
        let save = fake.calls.iter().position(|c| c == "save");
        let set = fake.calls.iter().position(|c| c == "set 0");
        assert!(save < set && save.is_some(), "{:?}", fake.calls);
        // What a crash right after the set leaves for the next start.
        let mut left = reread(&fake);
        fake.dead.push(holder(7));
        assert_eq!(
            release(&mut left, None, &mut fake),
            Release::Restored(MIRROR)
        );
    }

    #[test]
    fn a_record_that_cant_be_stored_sets_nothing() {
        let mut fake = Fake::new(MIRROR);
        fake.saves_work = false;
        let mut hold = PolicyHold::default();
        let extended = extend(&mut hold, &holder(7), &mut fake);
        assert_eq!(extended, Extend::Unsaved);
        assert!(!extended.held());
        assert_eq!(fake.policy, Some(MIRROR));
        assert!(!fake.calls.iter().any(|c| c.starts_with("set")));
        assert!(hold.is_empty());
    }

    #[test]
    fn a_set_skylight_doesnt_confirm_is_still_restored() {
        // SkyLight set it but said no, or the read-back failed.
        let mut fake = Fake::new(MIRROR);
        fake.sets_answer = false;
        let mut hold = PolicyHold::default();
        let extended = extend(&mut hold, &holder(7), &mut fake);
        assert_eq!(extended, Extend::Unconfirmed { was: MIRROR });
        assert!(extended.held());
        assert_eq!(fake.policy, Some(EXTEND));
        fake.sets_answer = true;
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Restored(MIRROR)
        );
        assert_eq!(fake.policy, Some(MIRROR));
    }

    #[test]
    fn a_set_that_didnt_take_leaves_the_policy_and_clears_the_record() {
        let mut fake = Fake::new(MIRROR);
        fake.sets_take = false;
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Nothing
        );
        assert_eq!(fake.policy, Some(MIRROR));
        assert!(reread(&fake).is_empty());
    }

    #[test]
    fn a_restore_that_fails_is_kept_for_the_next_start() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        fake.sets_take = false;
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::RestoreFailed(MIRROR)
        );
        fake.sets_take = true;
        let mut left = reread(&fake);
        assert_eq!(
            release(&mut left, None, &mut fake),
            Release::Restored(MIRROR)
        );
    }

    #[test]
    fn an_unread_policy_sets_nothing_and_keeps_the_record() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        fake.policy = None;
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Unread
        );
        assert!(!reread(&fake).is_empty());
        let mut fresh = Fake::new(MIRROR);
        fresh.policy = None;
        assert_eq!(
            extend(&mut PolicyHold::default(), &holder(8), &mut fresh),
            Extend::Unread
        );
        assert!(fresh.calls.iter().all(|c| c == "read"));
    }

    #[test]
    fn a_policy_that_extends_already_is_left_alone() {
        let mut fake = Fake::new(EXTEND);
        let mut hold = PolicyHold::default();
        assert_eq!(extend(&mut hold, &holder(7), &mut fake), Extend::Already);
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Nothing
        );
        assert!(!fake.calls.iter().any(|c| c.starts_with("set")));
    }

    #[test]
    fn a_value_the_user_set_meanwhile_is_kept() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        // The user picked presentation mode while the display was made.
        fake.policy = Some(2);
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::Nothing
        );
        assert_eq!(fake.policy, Some(2));
    }

    #[test]
    fn the_last_of_two_instances_restores_the_first_ones_value() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        assert_eq!(
            extend(&mut hold, &holder(7), &mut fake),
            Extend::Set { was: MIRROR }
        );
        assert_eq!(extend(&mut hold, &holder(8), &mut fake), Extend::Joined);
        assert_eq!(
            release(&mut hold, Some(&holder(7)), &mut fake),
            Release::StillHeld
        );
        assert_eq!(fake.policy, Some(EXTEND));
        assert_eq!(
            release(&mut hold, Some(&holder(8)), &mut fake),
            Release::Restored(MIRROR)
        );
    }

    #[test]
    fn a_killed_holders_value_is_restored_at_the_next_start() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        fake.dead.push(holder(7));
        let mut left = reread(&fake);
        assert_eq!(
            release(&mut left, None, &mut fake),
            Release::Restored(MIRROR)
        );
        assert!(reread(&fake).is_empty());
    }

    #[test]
    fn a_reused_pid_is_not_the_holder() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        // After a restart, pid 7 is another process started later.
        let mut same_pid = holder(7);
        same_pid.started_us += 1;
        fake.dead.push(holder(7));
        assert!(!fake.dead.contains(&same_pid));
        assert_eq!(
            release(&mut hold, None, &mut fake),
            Release::Restored(MIRROR)
        );
    }

    #[test]
    fn a_live_holder_keeps_it_at_another_instances_start() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        assert_eq!(release(&mut hold, None, &mut fake), Release::StillHeld);
        assert_eq!(fake.policy, Some(EXTEND));
    }

    #[test]
    fn a_killed_holders_value_passes_to_the_next_display() {
        let mut fake = Fake::new(MIRROR);
        let mut hold = PolicyHold::default();
        extend(&mut hold, &holder(7), &mut fake);
        fake.dead.push(holder(7));
        assert_eq!(extend(&mut hold, &holder(8), &mut fake), Extend::Joined);
        assert_eq!(
            release(&mut hold, Some(&holder(8)), &mut fake),
            Release::Restored(MIRROR)
        );
    }

    #[test]
    fn json_other_than_its_own_is_refused() {
        assert_eq!(PolicyHold::from_json(&serde_json::json!({})), None);
        assert_eq!(
            PolicyHold::from_json(&serde_json::json!({"previous": "1", "holders": []})),
            None
        );
        // Holders by pid alone aren't holders.
        assert_eq!(
            PolicyHold::from_json(&serde_json::json!({"previous": 1, "holders": [7]})),
            None
        );
    }
}
