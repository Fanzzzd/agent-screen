//! set_value's steps, by the control's role (#79). A closed pop-up button
//! takes its item's title as type-ahead; a combo box takes AXValue, then
//! AXConfirm; anything else takes AXValue. No step opens a menu: an open
//! menu takes the user's keyboard (ADR 0001). Each step checks for a
//! Takeover first, and so does the job that sends it, so nothing more goes
//! out once the user has the display. Tests stand in for the AX and input
//! calls; they say nothing about which apps honor them.

use super::server::ControlError;
use agent_screen_model::selection::{ValueRoute, popup_title_is_safe, value_route};
use std::time::Duration;

/// What became of an AXValue write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Set {
    /// The element doesn't let clients change its value: nothing was sent.
    NotSettable,
    /// Accessibility reported an error, as when the app answered late; the
    /// value may have changed anyway.
    Refused,
    Sent,
    /// The user took over before the write went out.
    Stopped,
}

/// What became of AXConfirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    /// The element has no AXConfirm action, as a web page's combobox.
    Unsupported,
    Refused,
    Done,
    Stopped,
}

/// The AX and input calls set_value makes. The jobs behind `set` and
/// `confirm` check `stopped` again just before they act.
pub(super) trait ValueAccess {
    async fn role(&mut self) -> Option<String>;
    fn stopped(&self) -> bool;
    async fn type_popup(&mut self, title: &str) -> Result<(), ControlError>;
    async fn set(&mut self, value: &str) -> Set;
    async fn read(&mut self) -> Option<String>;
    async fn confirm(&mut self) -> Confirm;
    async fn pause(&mut self, duration: Duration);
}

/// `act`, unless the user took over since the caller looked.
pub(super) fn unless_stopped<T>(stopped: impl Fn() -> bool, act: impl FnOnce() -> T) -> Option<T> {
    (!stopped()).then(act)
}

fn check(access: &impl ValueAccess, display: u32) -> Result<(), ControlError> {
    if access.stopped() {
        return Err(ControlError::takeover(format!(
            "the user took over display {display} during set_value, and nothing more was sent; the value may already have changed. Call wait_for_control to wait until they hand it back."
        )));
    }
    Ok(())
}

/// What an element shows, for a message.
fn shown_text(shown: &Option<String>) -> String {
    match shown {
        Some(text) => format!("{text:?}"),
        None => "no readable value".into(),
    }
}

fn same(shown: &str, value: &str) -> bool {
    shown.replace('\r', "\n") == value.replace('\r', "\n")
}

pub(super) async fn set_value(
    access: &mut impl ValueAccess,
    display: u32,
    value: &str,
) -> Result<(), ControlError> {
    check(access, display)?;
    let role = access.role().await;
    check(access, display)?;
    match value_route(role.as_deref()) {
        ValueRoute::PopupTypeAhead => popup(access, display, value).await,
        ValueRoute::ComboConfirm => {
            attribute(access, display, value, true).await?;
            combo(access, display, value).await
        }
        ValueRoute::Attribute => attribute(access, display, value, false).await,
    }
}

async fn popup(
    access: &mut impl ValueAccess,
    display: u32,
    title: &str,
) -> Result<(), ControlError> {
    if !popup_title_is_safe(title) {
        return Err(ControlError::invalid(
            "a pop-up button takes the exact full title of one of its items, nonempty, without leading whitespace, control characters or function-key characters. Nothing was sent",
        ));
    }
    access.type_popup(title).await?;
    // The app takes the last key, then updates the button.
    let mut shown = String::new();
    for _ in 0..100 {
        check(access, display)?;
        shown = access.read().await.unwrap_or_default();
        check(access, display)?;
        if shown == title {
            return Ok(());
        }
        access.pause(Duration::from_millis(25)).await;
    }
    check(access, display)?;
    Err(ControlError::failed(format!(
        "typed {title:?} into the closed pop-up button, but it shows {shown:?}: type-ahead chose another item, or none matched. Pass the exact full title of an item, as get_window_state lists them. No menu was opened"
    )))
}

async fn attribute(
    access: &mut impl ValueAccess,
    display: u32,
    value: &str,
    combo: bool,
) -> Result<(), ControlError> {
    let set = access.set(value).await;
    check(access, display)?;
    let sent = match set {
        Set::Stopped => {
            return Err(ControlError::takeover(format!(
                "the user took over display {display} during set_value, and nothing was sent. Call wait_for_control to wait until they hand it back."
            )));
        }
        Set::NotSettable => {
            return Err(ControlError::failed(
                "the element's value cannot be set, and nothing was sent; focus it and type instead",
            ));
        }
        Set::Refused => false,
        Set::Sent => true,
    };
    // Some elements, static text among them, report success and keep their
    // value; others report an error and change: read it back.
    let shown = access.read().await;
    check(access, display)?;
    match shown {
        Some(shown) if same(&shown, value) => Ok(()),
        // A combo box is confirmed only once it shows the value.
        None if sent && !combo => Ok(()),
        shown if sent => Err(ControlError::failed(format!(
            "set the element's value to {value:?}, but it shows {}; the app may have changed or rejected it. Inspect it before retrying",
            shown_text(&shown)
        ))),
        shown => Err(ControlError::failed(format!(
            "the element refused the value {value:?} and shows {}, which may still change. Inspect it before retrying, or focus it and type instead",
            shown_text(&shown)
        ))),
    }
}

async fn combo(
    access: &mut impl ValueAccess,
    display: u32,
    value: &str,
) -> Result<(), ControlError> {
    check(access, display)?;
    let confirm = access.confirm().await;
    check(access, display)?;
    match confirm {
        Confirm::Unsupported => return Ok(()),
        Confirm::Stopped => {
            return Err(ControlError::takeover(format!(
                "the user took over display {display} during set_value, after the combo box took its value and before it was confirmed. Call wait_for_control to wait until they hand it back."
            )));
        }
        Confirm::Refused => {
            return Err(ControlError::failed(
                "the combo box took its value but refused AXConfirm, so the app may not have committed it. No menu was opened and no click was sent. Inspect it before retrying",
            ));
        }
        Confirm::Done => {}
    }
    // The app's commit handler may reject or rewrite the text: read it
    // once the handler has had a turn.
    access.pause(Duration::from_millis(250)).await;
    check(access, display)?;
    let shown = access.read().await;
    check(access, display)?;
    if shown.as_deref() != Some(value) {
        return Err(ControlError::failed(format!(
            "the combo box confirmed {value:?}, but now shows {}; the app rejected or changed the value",
            shown_text(&shown)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Stands in for the element and the display's input. `stop_at` names
    /// the step during which the user takes over.
    struct Fake {
        role: Option<&'static str>,
        set: Set,
        confirm: Confirm,
        reads: VecDeque<Option<&'static str>>,
        stop_at: Option<&'static str>,
        stopped: bool,
        trace: Vec<&'static str>,
    }

    impl Fake {
        fn new(role: &'static str) -> Self {
            Fake {
                role: Some(role),
                set: Set::Sent,
                confirm: Confirm::Done,
                reads: VecDeque::from([Some("Cherry")]),
                stop_at: None,
                stopped: false,
                trace: Vec::new(),
            }
        }

        fn step(&mut self, name: &'static str) {
            self.trace.push(name);
            if self.stop_at == Some(name) {
                self.stopped = true;
            }
        }
    }

    impl ValueAccess for Fake {
        async fn role(&mut self) -> Option<String> {
            self.step("role");
            self.role.map(Into::into)
        }
        fn stopped(&self) -> bool {
            self.stopped
        }
        async fn type_popup(&mut self, _: &str) -> Result<(), ControlError> {
            self.step("type");
            Ok(())
        }
        async fn set(&mut self, _: &str) -> Set {
            self.step("set");
            let set = self.set;
            unless_stopped(|| self.stopped, || set).unwrap_or(Set::Stopped)
        }
        async fn read(&mut self) -> Option<String> {
            self.step("read");
            let next = self.reads.pop_front().flatten();
            // The last answer holds.
            if self.reads.is_empty() {
                self.reads.push_back(next);
            }
            next.map(Into::into)
        }
        async fn confirm(&mut self) -> Confirm {
            self.step("confirm");
            let confirm = self.confirm;
            unless_stopped(|| self.stopped, || confirm).unwrap_or(Confirm::Stopped)
        }
        async fn pause(&mut self, _: Duration) {
            self.step("pause");
        }
    }

    fn run(fake: &mut Fake, value: &str) -> Result<(), ControlError> {
        futures::executor::block_on(set_value(fake, 1, value))
    }

    #[test]
    fn a_popup_is_chosen_by_typing_and_read_back() {
        let mut fake = Fake::new("AXPopUpButton");
        run(&mut fake, "Cherry").unwrap();
        assert_eq!(fake.trace, ["role", "type", "read"]);
    }

    #[test]
    fn a_popup_never_gets_an_axvalue_write() {
        let mut fake = Fake::new("AXPopUpButton");
        fake.reads = VecDeque::from([Some("Apple")]);
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert_eq!(error.code, "failed");
        assert!(
            error.message.contains("shows \"Apple\""),
            "{}",
            error.message
        );
        assert!(!fake.trace.contains(&"set"));
    }

    #[test]
    fn a_popup_title_must_match_exactly() {
        let mut fake = Fake::new("AXPopUpButton");
        fake.reads = VecDeque::from([Some("Banana")]);
        assert!(run(&mut fake, "banana").is_err());
    }

    #[test]
    fn an_unsafe_popup_title_sends_nothing() {
        for title in ["", " Cherry", "Cherry\n", "\u{f701}"] {
            let mut fake = Fake::new("AXPopUpButton");
            assert_eq!(run(&mut fake, title).unwrap_err().code, "invalid");
            assert_eq!(fake.trace, ["role"]);
        }
    }

    #[test]
    fn a_takeover_during_the_role_lookup_sends_nothing() {
        for role in ["AXPopUpButton", "AXComboBox", "AXTextField"] {
            let mut fake = Fake::new(role);
            fake.stop_at = Some("role");
            assert_eq!(run(&mut fake, "Cherry").unwrap_err().code, "takeover");
            assert_eq!(fake.trace, ["role"]);
        }
    }

    #[test]
    fn a_takeover_while_the_popup_settles_stops_the_wait() {
        let mut fake = Fake::new("AXPopUpButton");
        fake.reads = VecDeque::from([Some("Apple")]);
        fake.stop_at = Some("pause");
        assert_eq!(run(&mut fake, "Cherry").unwrap_err().code, "takeover");
        assert_eq!(fake.trace, ["role", "type", "read", "pause"]);
    }

    #[test]
    fn a_combo_is_set_read_confirmed_and_read_again() {
        let mut fake = Fake::new("AXComboBox");
        run(&mut fake, "Cherry").unwrap();
        assert_eq!(
            fake.trace,
            ["role", "set", "read", "confirm", "pause", "read"]
        );
    }

    #[test]
    fn a_combo_that_did_not_take_the_value_is_not_confirmed() {
        for shown in [None, Some("Apple")] {
            let mut fake = Fake::new("AXComboBox");
            fake.reads = VecDeque::from([shown]);
            assert!(run(&mut fake, "Cherry").is_err());
            assert!(!fake.trace.contains(&"confirm"));
        }
    }

    #[test]
    fn a_rejected_combo_commit_names_what_it_shows() {
        let mut fake = Fake::new("AXComboBox");
        fake.reads = VecDeque::from([Some("Cherry"), Some("Blue")]);
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("Blue"), "{}", error.message);
    }

    #[test]
    fn a_combo_without_axconfirm_ends_once_it_shows_the_value() {
        let mut fake = Fake::new("AXComboBox");
        fake.confirm = Confirm::Unsupported;
        run(&mut fake, "Cherry").unwrap();
        assert_eq!(fake.trace, ["role", "set", "read", "confirm"]);
    }

    #[test]
    fn a_refused_confirm_says_the_value_may_not_be_committed() {
        let mut fake = Fake::new("AXComboBox");
        fake.confirm = Confirm::Refused;
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("may not have committed"));
        assert!(!fake.trace.contains(&"pause"));
    }

    #[test]
    fn a_takeover_during_the_write_stops_the_combo_before_axconfirm() {
        let mut fake = Fake::new("AXComboBox");
        fake.stop_at = Some("set");
        assert_eq!(run(&mut fake, "Cherry").unwrap_err().code, "takeover");
        assert_eq!(fake.trace, ["role", "set"]);
    }

    #[test]
    fn a_takeover_inside_the_confirm_job_sends_no_axconfirm() {
        let mut fake = Fake::new("AXComboBox");
        fake.stop_at = Some("confirm");
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert_eq!(error.code, "takeover");
        assert!(!fake.trace.contains(&"pause"));
    }

    #[test]
    fn a_value_that_landed_despite_an_error_is_a_success() {
        let mut fake = Fake::new("AXTextField");
        fake.set = Set::Refused;
        run(&mut fake, "Cherry").unwrap();
    }

    #[test]
    fn a_refused_value_that_did_not_land_says_it_may_still_change() {
        let mut fake = Fake::new("AXTextField");
        fake.set = Set::Refused;
        fake.reads = VecDeque::from([Some("")]);
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(
            error.message.contains("may still change"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_field_that_cannot_be_set_is_left_alone() {
        let mut fake = Fake::new("AXStaticText");
        fake.set = Set::NotSettable;
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("nothing was sent"));
        assert_eq!(fake.trace, ["role", "set"]);
    }

    #[test]
    fn an_unreadable_field_counts_as_set() {
        let mut fake = Fake::new("AXTextArea");
        fake.reads = VecDeque::from([None]);
        run(&mut fake, "Cherry").unwrap();
    }

    #[test]
    fn a_stop_after_the_check_keeps_the_job_from_acting() {
        let stopped = std::cell::Cell::new(false);
        let acted = std::cell::Cell::new(false);
        stopped.set(true);
        assert!(unless_stopped(|| stopped.get(), || acted.set(true)).is_none());
        assert!(!acted.get());
    }
}
