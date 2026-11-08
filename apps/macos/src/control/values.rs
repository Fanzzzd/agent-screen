//! set_value's steps, by the control's role (#79). A closed pop-up button
//! takes its item's title as type-ahead; a web page's select in a Chromium
//! app with a DevTools pipe (ADR 0010) is chosen through the pipe instead,
//! with no item in between. A combo box takes AXValue, then AXConfirm;
//! anything else takes AXValue. No step opens a menu: an open menu takes
//! the user's keyboard (ADR 0001). Each step checks for a Takeover first,
//! and so does the job that sends it, so nothing more goes out once the
//! user has the display. Tests stand in for the AX and input calls; they
//! say nothing about which apps honor them.

use super::server::ControlError;
use agent_screen_model::selection::{
    PopupMiss, ValueRoute, popup_miss, popup_title_is_safe, repeats_one_character, value_route,
};
use std::time::{Duration, Instant};

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

/// How far typing a title into a closed pop-up button went.
#[derive(Debug)]
pub(super) enum Typed {
    All,
    /// The user took over after this many characters.
    Stopped(usize),
    /// Another window of the app took key focus, or the button's window
    /// left the display, after this many characters.
    Interrupted(usize),
    /// The next key would have come too late to continue the search, and
    /// a space then would open the menu: stopped after this many.
    Slow(usize),
    /// Nothing was typed, for this reason.
    NotSent(ControlError),
}

/// What became of choosing a web page's select option through the pipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PageChoice {
    /// No pipe, or no select there: type instead.
    Unpiped,
    Chosen,
    Already,
    Missing {
        shown: String,
        near: Option<String>,
    },
    Disabled,
    /// The page didn't answer; the choice may have run.
    Unknown(String),
    Stopped,
}

/// The AX and input calls set_value makes. The jobs behind `set`,
/// `confirm` and `choose_in_page` check `stopped` again just before they
/// act.
pub(super) trait ValueAccess {
    async fn role(&mut self) -> Option<String>;
    fn stopped(&self) -> bool;
    /// Whether the element is in a web page.
    async fn in_web_page(&mut self) -> bool;
    async fn choose_in_page(&mut self, title: &str) -> PageChoice;
    async fn type_popup(&mut self, title: &str) -> Typed;
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

/// How long a pop-up gets to show its new item, and a combo its value.
const SETTLE: Duration = Duration::from_secs(3);

/// What an agent can do about a title type-ahead can't reach.
const UNREACHABLE: &str = "Call request_help for the user to choose it. In a Chromium app that launch_app started (a browser with its own --user-data-dir), set_value chooses any option of a select in a page's main frame directly";

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
    check(access, display)?;
    let before = access.read().await.unwrap_or_default();
    check(access, display)?;
    // Typing the title it shows could move it on: a web page's select goes
    // to the next item that starts with a repeated key.
    if before == title {
        return Ok(());
    }
    let web = access.in_web_page().await;
    check(access, display)?;
    if web {
        match access.choose_in_page(title).await {
            PageChoice::Unpiped => {}
            PageChoice::Already => return Ok(()),
            PageChoice::Chosen => return settle(access, display, title, &before, None).await,
            PageChoice::Missing { shown, near } => {
                let near = near
                    .map(|n| format!("; its option {n:?} differs in case only"))
                    .unwrap_or_default();
                return Err(ControlError::failed(format!(
                    "the select has no option titled {title:?}{near}, so nothing was chosen; it shows {shown:?}. Pass an option's exact title"
                )));
            }
            PageChoice::Disabled => {
                return Err(ControlError::failed(format!(
                    "the select or its option {title:?} is disabled, so nothing was chosen"
                )));
            }
            PageChoice::Unknown(why) => {
                return Err(ControlError::failed(format!(
                    "the page didn't answer the choice of {title:?} ({why}), so it may or may not have run. Inspect the select before retrying"
                )));
            }
            PageChoice::Stopped => {
                return Err(ControlError::takeover(format!(
                    "the user took over display {display} during set_value, and nothing was sent. Call wait_for_control to wait until they hand it back."
                )));
            }
        }
        check(access, display)?;
        if repeats_one_character(title) {
            return Err(ControlError::failed(format!(
                "a web page's select takes a repeated key as the next item that starts with it, so type-ahead can't reach {title:?}. Nothing was sent. {UNREACHABLE}"
            )));
        }
    }
    let typed = access.type_popup(title).await;
    let total = title.chars().count();
    let passed = if web {
        " A web page may have taken a change event for each item passed on the way."
    } else {
        ""
    };
    let (done, why) = match typed {
        Typed::All => return settle(access, display, title, &before, Some(web)).await,
        Typed::NotSent(error) => return Err(error),
        Typed::Stopped(done) => (done, None),
        Typed::Interrupted(done) => (
            done,
            Some("another window of its app took key focus, or its window left the display"),
        ),
        Typed::Slow(done) => (
            done,
            Some(
                "the app answered too slowly to keep the search going, and a late key could open the menu",
            ),
        ),
    };
    let shown = if done > 0 {
        let shown = access.read().await;
        format!(" It now shows {}.{passed}", shown_text(&shown))
    } else {
        String::new()
    };
    match why {
        None => Err(ControlError::takeover(format!(
            "the user took over display {display} after {done} of {total} characters of {title:?} went to the closed pop-up button, and nothing more was sent.{shown} Call wait_for_control to wait until they hand it back."
        ))),
        Some(why) => Err(ControlError::failed(format!(
            "typed {done} of {total} characters of {title:?} into the closed pop-up button, then stopped: {why}.{shown} Inspect it before retrying. No menu was opened"
        ))),
    }
}

/// Wait for the pop-up to show `title`, which the page chose (`typed`
/// None) or was typed into it (`typed` says whether in a web page); if it
/// doesn't, say why. A hung app gets `SETTLE` in all.
async fn settle(
    access: &mut impl ValueAccess,
    display: u32,
    title: &str,
    before: &str,
    typed: Option<bool>,
) -> Result<(), ControlError> {
    let deadline = Instant::now() + SETTLE;
    let mut shown = String::new();
    for _ in 0..100 {
        check(access, display)?;
        shown = access.read().await.unwrap_or_default();
        check(access, display)?;
        if shown == title {
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        access.pause(Duration::from_millis(25)).await;
    }
    check(access, display)?;
    let Some(typed_in_page) = typed else {
        return Err(ControlError::failed(format!(
            "the page chose the option {title:?}, but accessibility shows {shown:?}. Inspect the select before retrying"
        )));
    };
    let passed = if typed_in_page {
        " A web page may have taken a change event for each item passed on the way."
    } else {
        ""
    };
    let why = match popup_miss(title, before, &shown) {
        PopupMiss::Case => format!(
            "titles must match exactly, case included: call set_value again with {shown:?}"
        ),
        PopupMiss::Unchanged => format!(
            "no item's title starts with {title:?}, or the app ignored the keys. Pass an item's exact title"
        ),
        PopupMiss::EarlierItem => format!(
            "{shown:?} starts with the same text and comes first, and type-ahead stops at the first such item, so it can't reach {title:?} here. {UNREACHABLE}"
        ),
        PopupMiss::Other => "type-ahead chose another item. Pass an item's exact title, or call request_help for the user to choose it".to_string(),
    };
    Err(ControlError::failed(format!(
        "chose {title:?} in the closed pop-up button, but it shows {shown:?} (it showed {before:?} before): {why}.{passed} No menu was opened"
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
        Confirm::Unsupported => {
            return Err(ControlError::failed(format!(
                "set the combo box's text to {value:?}, but it has no confirm action, as a web page's combobox hasn't, so no option of its list is chosen, and many pages clear such text when focus leaves. To choose an option, press or click it in the list the combobox shows. If the field takes free text, the text is in place"
            )));
        }
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
    // once the handler has had a turn. A hung app gets `SETTLE` in all.
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
        web: bool,
        page: PageChoice,
        typed: Option<Typed>,
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
                web: false,
                page: PageChoice::Unpiped,
                typed: None,
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
        async fn in_web_page(&mut self) -> bool {
            self.step("web?");
            self.web
        }
        async fn choose_in_page(&mut self, _: &str) -> PageChoice {
            self.step("pipe");
            if self.stopped {
                return PageChoice::Stopped;
            }
            self.page.clone()
        }
        async fn type_popup(&mut self, _: &str) -> Typed {
            self.step("type");
            self.typed.take().unwrap_or(Typed::All)
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

    /// A pop-up that shows Apple, then `after`.
    fn popup(after: &'static str) -> Fake {
        let mut fake = Fake::new("AXPopUpButton");
        fake.reads = VecDeque::from([Some("Apple"), Some(after)]);
        fake
    }

    #[test]
    fn a_popup_is_chosen_by_typing_and_read_back() {
        let mut fake = popup("Cherry");
        run(&mut fake, "Cherry").unwrap();
        assert_eq!(fake.trace, ["role", "read", "web?", "type", "read"]);
    }

    #[test]
    fn a_popup_that_shows_the_title_gets_no_key() {
        let mut fake = Fake::new("AXPopUpButton");
        fake.reads = VecDeque::from([Some("1")]);
        run(&mut fake, "1").unwrap();
        assert_eq!(fake.trace, ["role", "read"]);
    }

    #[test]
    fn a_popup_never_gets_an_axvalue_write() {
        let mut fake = popup("Banana");
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert_eq!(error.code, "failed");
        assert!(
            error.message.contains("shows \"Banana\""),
            "{}",
            error.message
        );
        assert!(!fake.trace.contains(&"set"));
    }

    #[test]
    fn a_popup_title_must_match_exactly() {
        let mut fake = popup("Banana");
        let error = run(&mut fake, "banana").unwrap_err();
        assert!(error.message.contains("case included"), "{}", error.message);
    }

    #[test]
    fn an_earlier_item_with_the_same_start_is_named_as_the_reason() {
        let mut fake = popup("Dragon fruit");
        let error = run(&mut fake, "Dragon").unwrap_err();
        assert!(error.message.contains("can't reach"), "{}", error.message);
        assert!(error.message.contains("request_help"), "{}", error.message);
    }

    #[test]
    fn a_title_no_item_starts_with_leaves_it_unchanged() {
        let mut fake = popup("Apple");
        let error = run(&mut fake, "Kiwi").unwrap_err();
        assert!(
            error.message.contains("no item's title starts with"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_hung_popup_gets_a_bounded_wait() {
        let mut fake = popup("Apple");
        run(&mut fake, "Kiwi").unwrap_err();
        assert!(fake.trace.iter().filter(|s| **s == "read").count() <= 101);
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
    fn a_web_select_with_a_pipe_is_chosen_without_typing() {
        let mut fake = popup("Cherry");
        fake.web = true;
        fake.page = PageChoice::Chosen;
        run(&mut fake, "Cherry").unwrap();
        assert_eq!(fake.trace, ["role", "read", "web?", "pipe", "read"]);
    }

    #[test]
    fn a_web_select_without_the_option_says_so_and_types_nothing() {
        let mut fake = popup("Apple");
        fake.web = true;
        fake.page = PageChoice::Missing {
            shown: "Apple".into(),
            near: Some("Cherry".into()),
        };
        let error = run(&mut fake, "cherry").unwrap_err();
        assert!(
            error.message.contains("differs in case only"),
            "{}",
            error.message
        );
        assert!(!fake.trace.contains(&"type"));
    }

    #[test]
    fn an_unanswered_pipe_is_not_retried_by_typing() {
        let mut fake = popup("Apple");
        fake.web = true;
        fake.page = PageChoice::Unknown("no answer".into());
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("may or may not have run"));
        assert!(!fake.trace.contains(&"type"));
    }

    #[test]
    fn a_repeated_key_is_refused_in_a_web_select_without_a_pipe() {
        let mut fake = popup("10");
        fake.web = true;
        let error = run(&mut fake, "11").unwrap_err();
        assert!(
            error.message.contains("can't reach \"11\""),
            "{}",
            error.message
        );
        assert!(!fake.trace.contains(&"type"));
    }

    #[test]
    fn a_repeated_key_may_be_typed_into_a_native_popup() {
        let mut fake = popup("11");
        run(&mut fake, "11").unwrap();
    }

    #[test]
    fn a_web_select_without_a_pipe_warns_of_change_events() {
        let mut fake = popup("Banana");
        fake.web = true;
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("change event"), "{}", error.message);
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
    fn a_takeover_before_the_pipe_sends_nothing() {
        let mut fake = popup("Cherry");
        fake.web = true;
        fake.page = PageChoice::Chosen;
        fake.stop_at = Some("web?");
        assert_eq!(run(&mut fake, "Cherry").unwrap_err().code, "takeover");
        assert!(!fake.trace.contains(&"pipe"));
    }

    #[test]
    fn a_takeover_mid_title_says_what_the_popup_shows() {
        let mut fake = popup("Banana");
        fake.typed = Some(Typed::Stopped(2));
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert_eq!(error.code, "takeover");
        assert!(error.message.contains("after 2 of 6"), "{}", error.message);
        assert!(
            error.message.contains("now shows \"Banana\""),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_slow_app_stops_the_title_and_says_so() {
        let mut fake = popup("Dragon");
        fake.typed = Some(Typed::Slow(6));
        let error = run(&mut fake, "Dragon fruit").unwrap_err();
        assert!(error.message.contains("too slowly"), "{}", error.message);
        assert!(
            error.message.contains("now shows \"Dragon\""),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_takeover_while_the_popup_settles_stops_the_wait() {
        let mut fake = popup("Banana");
        fake.stop_at = Some("pause");
        assert_eq!(run(&mut fake, "Cherry").unwrap_err().code, "takeover");
        assert_eq!(
            fake.trace,
            ["role", "read", "web?", "type", "read", "pause"]
        );
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
    fn a_combo_without_axconfirm_says_no_option_is_chosen() {
        let mut fake = Fake::new("AXComboBox");
        fake.confirm = Confirm::Unsupported;
        let error = run(&mut fake, "Cherry").unwrap_err();
        assert!(error.message.contains("no option of its list is chosen"));
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
