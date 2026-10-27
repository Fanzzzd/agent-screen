//! Production set_value orchestration. Tests replace only AX/input IO and
//! the timer; they do not prove real input routing, app support or focus.

use super::server::ControlError;
use agent_screen_model::{
    selection::{ValueRoute, value_route},
    typing,
};
use std::time::Duration;

pub(super) trait ValueAccess {
    async fn role(&mut self) -> Option<String>;
    fn stopped(&self) -> bool;
    async fn type_popup(&mut self, value: &str) -> Result<(), ControlError>;
    async fn set_attribute(&mut self, value: &str) -> bool;
    async fn read_value(&mut self) -> Option<String>;
    async fn confirm(&mut self) -> bool;
    async fn pause(&mut self, duration: Duration);
}

/// Also used inside the dispatched AX closure, after scheduling delays.
pub(super) fn mutate_if_current(
    stopped: impl FnOnce() -> bool,
    mutate: impl FnOnce() -> bool,
) -> bool {
    !stopped() && mutate()
}

pub(super) fn set_if_current(
    stopped: impl Fn() -> bool,
    capable: impl FnOnce() -> bool,
    mutate: impl FnOnce() -> bool,
) -> bool {
    if stopped() || !capable() {
        return false;
    }
    mutate_if_current(stopped, mutate)
}

fn check(access: &impl ValueAccess, display: u32) -> Result<(), ControlError> {
    if access.stopped() {
        Err(ControlError::takeover(format!(
            "the user took over display {display} during set_value; the value may already have changed, but no further mutation was sent. Call wait_for_control to wait until they hand it back."
        )))
    } else {
        Ok(())
    }
}

pub(super) async fn set_value(
    access: &mut impl ValueAccess,
    display: u32,
    value: &str,
) -> Result<(), ControlError> {
    check(access, display)?;
    let role = access.role().await;
    check(access, display)?;
    let route = value_route(role.as_deref());
    if route == ValueRoute::Unavailable {
        return Err(ControlError::failed(
            "the element's AX role is unavailable, so no value operation was sent. Refresh get_window_state before retrying",
        ));
    }

    if route == ValueRoute::RequiresMenu {
        return Err(ControlError::failed(
            "this menu button requires an open menu, but the runtime focus guard is not enabled. No menu was opened and no value was sent. Use request_help",
        ));
    }
    if route == ValueRoute::PopupTypeAhead {
        if !typing::popup_title_is_safe(value) {
            return Err(ControlError::invalid(
                "a pop-up choice needs the full, nonempty item title without control characters, function-key characters or leading whitespace",
            ));
        }
        check(access, display)?;
        access.type_popup(value).await?;
        check(access, display)?;
        let mut shown = String::new();
        for _ in 0..100 {
            check(access, display)?;
            shown = access.read_value().await.unwrap_or_default();
            check(access, display)?;
            if shown == value {
                return Ok(());
            }
            access.pause(Duration::from_millis(25)).await;
            check(access, display)?;
        }
        return Err(ControlError::failed(format!(
            "typed {value:?} into the closed pop-up button, but it shows {shown:?}; type-ahead may have selected a different item. Use the full item title. No menu was opened"
        )));
    }
    check(access, display)?;
    let set = access.set_attribute(value).await;
    check(access, display)?;
    if !set {
        let message = if route == ValueRoute::ComboConfirm {
            "the combo box could not set AXValue through the closed-control path; it may be a custom web control. Its value may have changed, so inspect before retrying. No menu or click fallback was sent. Use request_help if it needs a menu"
        } else {
            "the element's AXValue could not be set; its value may have changed. Inspect the window before retrying"
        };
        return Err(ControlError::failed(message));
    }
    let shown = access.read_value().await;
    check(access, display)?;
    let matches = shown
        .as_deref()
        .is_some_and(|now| now.replace('\r', "\n") == value.replace('\r', "\n"));
    if !matches && (shown.is_some() || route == ValueRoute::ComboConfirm) {
        return Err(ControlError::failed(format!(
            "sent AXValue {value:?}, but the element shows {shown:?}; no confirmation or click fallback was sent. Inspect before retrying"
        )));
    }
    if route == ValueRoute::ComboConfirm {
        check(access, display)?;
        let confirmed = access.confirm().await;
        check(access, display)?;
        if !confirmed {
            return Err(ControlError::failed(
                "the combo box accepted its value but refused AXConfirm; its value may have changed. No menu or click fallback was sent. Inspect before retrying or use request_help",
            ));
        }
        access.pause(Duration::from_millis(250)).await;
        check(access, display)?;
        let shown = access.read_value().await;
        check(access, display)?;
        if shown.as_deref() != Some(value) {
            return Err(ControlError::failed(format!(
                "the combo box confirmed {value:?}, but now shows {shown:?}; the app may have rejected or changed the value"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Access {
        role: &'static str,
        missing_role: bool,
        waits: usize,
        stop_at_wait: Option<usize>,
        stopped: bool,
        stop_after_role: bool,
        stop_at_dispatch: bool,
        stop_on_wait: bool,
        set_ok: bool,
        confirm_ok: bool,
        values: VecDeque<Option<String>>,
        trace: Vec<String>,
    }
    impl ValueAccess for Access {
        async fn role(&mut self) -> Option<String> {
            self.trace.push("role".into());
            if self.stop_after_role {
                self.stopped = true;
            }
            if self.missing_role {
                None
            } else {
                Some(self.role.into())
            }
        }
        fn stopped(&self) -> bool {
            self.stopped
        }
        async fn type_popup(&mut self, _: &str) -> Result<(), ControlError> {
            self.trace.push("type-ahead".into());
            Ok(())
        }
        async fn set_attribute(&mut self, _: &str) -> bool {
            self.trace.push("dispatch-set".into());
            if self.stop_at_dispatch {
                self.stopped = true;
            }
            let stopped = self.stopped;
            mutate_if_current(
                || stopped,
                || {
                    self.trace.push("AXValue".into());
                    self.set_ok
                },
            )
        }
        async fn read_value(&mut self) -> Option<String> {
            self.trace.push("read".into());
            self.values.pop_front().flatten()
        }
        async fn confirm(&mut self) -> bool {
            self.trace.push("AXConfirm".into());
            self.confirm_ok
        }
        async fn pause(&mut self, _: Duration) {
            self.trace.push("wait".into());
            self.waits += 1;
            if self.stop_on_wait || self.stop_at_wait == Some(self.waits) {
                self.stopped = true;
            }
        }
    }
    fn access(role: &'static str) -> Access {
        Access {
            role,
            missing_role: false,
            waits: 0,
            stop_at_wait: None,
            stopped: false,
            stop_after_role: false,
            stop_at_dispatch: false,
            stop_on_wait: false,
            set_ok: true,
            confirm_ok: true,
            values: VecDeque::from([Some("Cherry".into())]),
            trace: Vec::new(),
        }
    }
    fn run(io: &mut Access, value: &str) -> Result<(), ControlError> {
        futures::executor::block_on(set_value(io, 1, value))
    }
    #[test]
    fn unavailable_role_refuses_without_any_value_operation() {
        let mut io = access("AXMenuButton");
        io.missing_role = true;
        let error = run(&mut io, "Cherry").expect_err("unknown role reached generic setter");
        assert_eq!(error.code, "failed");
        assert_eq!(io.trace, ["role"]);
    }
    #[test]
    fn takeover_during_last_popup_wait_returns_takeover_not_retry_guidance() {
        let mut io = access("AXPopUpButton");
        io.values = VecDeque::from(vec![Some("Apple".into()); 100]);
        io.stop_at_wait = Some(100);
        let error = run(&mut io, "Cherry").unwrap_err();
        assert_eq!(error.code, "takeover");
        assert_eq!(io.waits, 100);
    }

    #[test]
    fn menu_only_value_refuses_before_any_attribute_or_opening_action() {
        let mut io = access("AXMenuButton");
        let error = run(&mut io, "Cherry").unwrap_err();
        assert_eq!(error.code, "failed");
        assert!(error.message.contains("guard is not enabled"));
        assert_eq!(io.trace, ["role"]);
    }
    #[test]
    fn takeover_during_role_lookup_prevents_the_value_setter() {
        let mut io = access("AXTextField");
        io.stop_after_role = true;
        let error = run(&mut io, "Cherry").unwrap_err();
        assert_eq!(error.code, "takeover");
        assert_eq!(io.trace, ["role"]);
    }
    #[test]
    fn takeover_during_capability_query_prevents_the_actual_write() {
        let stopped = std::cell::Cell::new(false);
        let wrote = std::cell::Cell::new(false);
        let result = set_if_current(
            || stopped.get(),
            || {
                stopped.set(true);
                true
            },
            || {
                wrote.set(true);
                true
            },
        );
        assert!(!result);
        assert!(!wrote.get());
    }

    #[test]
    fn dispatched_setter_rechecks_the_stop_flag() {
        let mut io = access("AXTextField");
        io.stop_at_dispatch = true;
        assert_eq!(run(&mut io, "Cherry").unwrap_err().code, "takeover");
        assert_eq!(io.trace, ["role", "dispatch-set"]);
    }
    #[test]
    fn native_and_electron_popups_share_the_closed_role_route() {
        for _family in ["native", "Electron"] {
            let mut io = access("AXPopUpButton");
            run(&mut io, "Cherry").unwrap();
            assert_eq!(io.trace, ["role", "type-ahead", "read"]);
        }
    }
    #[test]
    fn native_and_electron_editable_combos_set_verify_confirm_and_verify_again() {
        for _family in ["native", "Electron"] {
            let mut io = access("AXComboBox");
            io.values.push_back(Some("Cherry".into()));
            run(&mut io, "Cherry").unwrap();
            assert_eq!(
                io.trace,
                [
                    "role",
                    "dispatch-set",
                    "AXValue",
                    "read",
                    "AXConfirm",
                    "wait",
                    "read"
                ]
            );
        }
    }
    #[test]
    fn custom_combo_without_value_support_has_no_click_or_menu_fallback() {
        let mut io = access("AXComboBox");
        io.set_ok = false;
        let error = run(&mut io, "Cherry").unwrap_err();
        assert!(error.message.contains("custom web control"));
        assert!(error.message.contains("may have changed"));
        assert_eq!(io.trace, ["role", "dispatch-set", "AXValue"]);
    }
    #[test]
    fn combo_mismatch_or_unreadable_value_prevents_confirmation() {
        for value in [None, Some("Apple".into())] {
            let mut io = access("AXComboBox");
            io.values = VecDeque::from([value]);
            assert!(run(&mut io, "Cherry").is_err());
            assert!(!io.trace.iter().any(|s| s == "AXConfirm"));
        }
    }
    #[test]
    fn unsupported_confirm_reports_partial_change_and_does_not_fallback() {
        let mut io = access("AXComboBox");
        io.confirm_ok = false;
        let error = run(&mut io, "Cherry").unwrap_err();
        assert!(error.message.contains("No menu or click fallback"));
        assert!(!io.trace.iter().any(|s| s == "wait"));
    }
    #[test]
    fn rejected_combo_commit_reports_the_post_confirm_value() {
        let mut io = access("AXComboBox");
        io.values.push_back(Some("Apple".into()));
        let error = run(&mut io, "Cherry").unwrap_err();
        assert!(error.message.contains("Apple"));
    }
    #[test]
    fn popup_verification_stops_on_takeover_instead_of_continuing_to_poll() {
        let mut io = access("AXPopUpButton");
        io.values = VecDeque::from([Some("Apple".into())]);
        io.stop_on_wait = true;
        assert_eq!(run(&mut io, "Cherry").unwrap_err().code, "takeover");
        assert_eq!(io.trace, ["role", "type-ahead", "read", "wait"]);
    }
    #[test]
    fn invalid_popup_title_never_dispatches_typing() {
        let mut io = access("AXPopUpButton");
        assert_eq!(run(&mut io, "\n").unwrap_err().code, "invalid");
        assert_eq!(io.trace, ["role"]);
    }
}
