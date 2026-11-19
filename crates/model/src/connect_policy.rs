//! WindowServer's default for a new display that has no stored arrangement:
//! its "TV connect" policy, the `TVConnectPolicy` preference of
//! `com.apple.windowserver.displays` for the current user and host. 1
//! mirrors every display onto the new one, which is what flashed the user's
//! screen in #220. 0 extends the desktop onto it.
//!
//! Agent Screen sets `EXTEND` just before it makes a display and sets the
//! earlier value back right after. Several instances may make displays at
//! once, and an app may die while it holds the value, so the record of who
//! holds it and what to restore is kept on disk (`PolicyHold`).

/// The policy that extends the desktop onto a new display.
pub const EXTEND: i32 = 0;

/// Who set the policy to `EXTEND` and what it was before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PolicyHold {
    /// The value before Agent Screen set `EXTEND`. None while nothing is
    /// held.
    previous: Option<i32>,
    /// The processes making a display meanwhile.
    holders: Vec<u32>,
}

impl PolicyHold {
    /// Process `me` is about to make a display, and the policy reads
    /// `current`. Returns the value to set: `EXTEND` when the policy would
    /// mirror. A value the user set while it was held becomes the one to
    /// restore. A record left by a process no longer `alive` passes to
    /// `me`.
    pub fn begin(&mut self, current: i32, me: u32, alive: impl Fn(u32) -> bool) -> Option<i32> {
        self.holders.retain(|&p| p != me && alive(p));
        if current == EXTEND {
            // Extending already: the user's own choice, or still held.
            if self.previous.is_some() {
                self.holders.push(me);
            }
            return None;
        }
        self.previous = Some(current);
        self.holders.push(me);
        Some(EXTEND)
    }

    /// Process `me` made its display, or None at start-up, and the policy
    /// reads `current`. Returns the value to set back once no live process
    /// holds the policy. Nothing when the policy no longer reads `EXTEND`:
    /// the user changed it meanwhile, and their value stays.
    pub fn end(
        &mut self,
        current: i32,
        me: Option<u32>,
        alive: impl Fn(u32) -> bool,
    ) -> Option<i32> {
        self.holders.retain(|&p| Some(p) != me && alive(p));
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
        serde_json::json!({ "previous": self.previous, "holders": self.holders })
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
            .map(|p| p.as_u64().and_then(|p| u32::try_from(p).ok()))
            .collect::<Option<_>>()?;
        Some(Self { previous, holders })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIRROR: i32 = 1;
    fn all_alive(_: u32) -> bool {
        true
    }

    #[test]
    fn a_mirroring_policy_extends_while_a_display_is_made_then_comes_back() {
        let mut hold = PolicyHold::default();
        assert_eq!(hold.begin(MIRROR, 7, all_alive), Some(EXTEND));
        assert_eq!(hold.end(EXTEND, Some(7), all_alive), Some(MIRROR));
        assert!(hold.is_empty());
    }

    #[test]
    fn a_policy_that_extends_already_is_left_alone() {
        let mut hold = PolicyHold::default();
        assert_eq!(hold.begin(EXTEND, 7, all_alive), None);
        assert_eq!(hold.end(EXTEND, Some(7), all_alive), None);
        assert!(hold.is_empty());
    }

    #[test]
    fn a_value_the_user_set_meanwhile_is_kept() {
        let mut hold = PolicyHold::default();
        hold.begin(MIRROR, 7, all_alive);
        // The user picked presentation mode while the display was made.
        assert_eq!(hold.end(2, Some(7), all_alive), None);
        assert!(hold.is_empty());
    }

    #[test]
    fn the_last_of_two_instances_restores_the_first_ones_value() {
        let mut hold = PolicyHold::default();
        assert_eq!(hold.begin(MIRROR, 7, all_alive), Some(EXTEND));
        // The other instance finds it extending, held by 7.
        assert_eq!(hold.begin(EXTEND, 8, all_alive), None);
        assert_eq!(hold.end(EXTEND, Some(7), all_alive), None);
        assert_eq!(hold.end(EXTEND, Some(8), all_alive), Some(MIRROR));
    }

    #[test]
    fn a_crashed_holders_value_is_restored_at_the_next_start() {
        let mut hold = PolicyHold::default();
        hold.begin(MIRROR, 7, all_alive);
        let hold = PolicyHold::from_json(&hold.to_json()).expect("round trip");
        let mut hold = hold;
        assert_eq!(hold.end(EXTEND, None, |_| false), Some(MIRROR));
        assert!(hold.is_empty());
    }

    #[test]
    fn a_live_holder_keeps_it_at_another_instances_start() {
        let mut hold = PolicyHold::default();
        hold.begin(MIRROR, 7, all_alive);
        assert_eq!(hold.end(EXTEND, None, all_alive), None);
        assert!(!hold.is_empty());
    }

    #[test]
    fn a_crashed_holders_value_passes_to_the_next_display() {
        let mut hold = PolicyHold::default();
        hold.begin(MIRROR, 7, all_alive);
        // 7 died holding it; 8 makes a display before any restart.
        assert_eq!(hold.begin(EXTEND, 8, |p| p != 7), None);
        assert_eq!(hold.end(EXTEND, Some(8), |p| p != 7), Some(MIRROR));
    }

    #[test]
    fn json_other_than_its_own_is_refused() {
        assert_eq!(PolicyHold::from_json(&serde_json::json!({})), None);
        assert_eq!(
            PolicyHold::from_json(&serde_json::json!({"previous": "1", "holders": []})),
            None
        );
    }
}
