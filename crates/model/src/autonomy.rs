//! The Autonomy Mode (ADR 0025): the user's standing answer to how an
//! Agent Session may take what they are using, a window, their pointer and
//! keyboard, or the front of their app. Ask is the behavior before the
//! mode existed, at every crossing. Auto takes a window that is not In Use
//! and uses the pointer and keyboard while the hands are At Rest. YOLO
//! takes them at once.
//!
//! Only the user changes the mode, in Agent Screen's own UI. Ask and Auto
//! are stored; YOLO lives in memory and ends with the sitting.

use crate::model::WindowId;
use crate::user_activity::Hands;
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Mode {
    #[default]
    Ask,
    Auto,
    Yolo,
}

impl Mode {
    /// The name agents and the log read.
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Auto => "auto",
            Mode::Yolo => "yolo",
        }
    }

    pub fn parse(name: &str) -> Option<Mode> {
        match name {
            "ask" => Some(Mode::Ask),
            "auto" => Some(Mode::Auto),
            "yolo" => Some(Mode::Yolo),
            _ => None,
        }
    }

    /// Whether the mode decides a crossing by itself: Auto and YOLO.
    pub fn acts(self) -> bool {
        self != Mode::Ask
    }
}

/// What is stored: never YOLO.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Base {
    #[default]
    Ask,
    Auto,
}

impl Base {
    pub fn mode(self) -> Mode {
        match self {
            Base::Ask => Mode::Ask,
            Base::Auto => Mode::Auto,
        }
    }
}

/// Why YOLO ended, for one line of copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YoloEnd {
    Locked,
    Slept,
    SwitchedOut,
    Restarted,
    Stopped,
}

impl YoloEnd {
    pub fn as_str(self) -> &'static str {
        match self {
            YoloEnd::Locked => "locked",
            YoloEnd::Slept => "slept",
            YoloEnd::SwitchedOut => "switchedOut",
            YoloEnd::Restarted => "restarted",
            YoloEnd::Stopped => "stopped",
        }
    }

    pub fn parse(name: &str) -> Option<YoloEnd> {
        match name {
            "locked" => Some(YoloEnd::Locked),
            "slept" => Some(YoloEnd::Slept),
            "switchedOut" => Some(YoloEnd::SwitchedOut),
            "restarted" => Some(YoloEnd::Restarted),
            "stopped" => Some(YoloEnd::Stopped),
            _ => None,
        }
    }

    /// YOLO ended by itself, which Settings says once: not on Stop, which
    /// the user did.
    pub fn by_itself(self) -> bool {
        self != YoloEnd::Stopped
    }
}

/// The user's setting.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Setting {
    pub base: Base,
    /// Memory only.
    pub yolo: bool,
    pub yolo_ended: Option<YoloEnd>,
    /// The bundle ids the user added to the Protected Apps.
    pub protected: BTreeSet<String>,
}

impl Setting {
    pub fn mode(&self) -> Mode {
        if self.yolo {
            Mode::Yolo
        } else {
            self.base.mode()
        }
    }

    /// The user picked `mode`. YOLO keeps the stored mode under it; only
    /// the confirmation in Settings calls it with YOLO.
    pub fn choose(&mut self, mode: Mode) {
        match mode {
            Mode::Ask | Mode::Auto => {
                self.base = if mode == Mode::Ask {
                    Base::Ask
                } else {
                    Base::Auto
                };
                self.yolo = false;
                self.yolo_ended = None;
            }
            Mode::Yolo => {
                self.yolo = true;
                self.yolo_ended = None;
            }
        }
    }

    /// Stop: the chord, the Input Bar's Stop or the menu's. Always Ask.
    pub fn stop(&mut self) {
        if self.yolo {
            self.yolo_ended = Some(YoloEnd::Stopped);
        }
        self.yolo = false;
        self.base = Base::Ask;
    }

    /// YOLO ended by itself: the stored mode applies again. False when it
    /// was not on.
    pub fn end_yolo(&mut self, why: YoloEnd) -> bool {
        if !self.yolo {
            return false;
        }
        self.yolo = false;
        self.yolo_ended = Some(why);
        true
    }

    /// What the file holds. `yolo` is never written: while it is on the
    /// file says it ended on a restart, which a clean end overwrites, so
    /// after a crash or an update that line is true.
    pub fn to_json(&self) -> Value {
        let ended = if self.yolo {
            Some(YoloEnd::Restarted)
        } else {
            self.yolo_ended
        };
        json!({
            "mode": self.base.mode().as_str(),
            "protected": self.protected.iter().collect::<Vec<_>>(),
            "yoloEnded": ended.map(YoloEnd::as_str),
        })
    }

    /// A file that says YOLO, an unknown mode or unreadable JSON loads as
    /// Ask. So does one whose Protected Apps can't be read, with only the
    /// built-in ones: a damaged list never leaves Auto acting on apps the
    /// user protected. Auto also needs its whole record, as `to_json`
    /// writes it: a file missing `protected` or `yoloEnded`, or with
    /// `yoloEnded` unreadable, loads as Ask.
    pub fn from_json(text: &str) -> Setting {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Setting::default();
        };
        let protected: Option<BTreeSet<String>> = match value.get("protected") {
            None => Some(BTreeSet::new()),
            Some(Value::Array(list)) => list
                .iter()
                .map(|id| id.as_str().map(str::trim))
                .filter(|id| id != &Some(""))
                .map(|id| id.map(String::from))
                .collect(),
            Some(_) => None,
        };
        let Some(protected) = protected else {
            return Setting::default();
        };
        let yolo_ended = match value.get("yoloEnded") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(Value::String(end)) => YoloEnd::parse(end).map(Some),
            Some(_) => None,
        };
        let whole = value.get("protected").is_some() && yolo_ended.is_some();
        let base = match value.get("mode").and_then(Value::as_str) {
            Some("auto") if whole => Base::Auto,
            _ => Base::Ask,
        };
        let yolo_ended = yolo_ended.flatten();
        Setting {
            base,
            yolo: false,
            yolo_ended,
            protected,
        }
    }
}

/// Apps no mode acts on by itself, by exact bundle id.
pub const PROTECTED: &[&str] = &[
    "com.apple.systempreferences",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.bitwarden.desktop",
    "com.lastpass.LastPass",
    "com.dashlane.dashlanephonefinal",
    "org.keepassxc.keepassxc",
    "in.sinew.Enpass-Desktop",
    "me.proton.pass.electron",
    "com.keepersecurity.passwordmanager",
];

/// And by prefix, for apps with one id per build.
pub const PROTECTED_PREFIXES: &[&str] = &["com.1password.", "com.agilebits."];

/// Whether the app `bundle` is a Protected App: one of the list, the app a
/// connected session's client runs in (`clients`), or one the user added.
/// An app without a bundle id is not.
pub fn protected(
    bundle: Option<&str>,
    clients: &BTreeSet<String>,
    added: &BTreeSet<String>,
) -> bool {
    let Some(bundle) = bundle.filter(|b| !b.is_empty()) else {
        return false;
    };
    PROTECTED.contains(&bundle)
        || PROTECTED_PREFIXES.iter().any(|p| bundle.starts_with(p))
        || clients.contains(bundle)
        || added.contains(bundle)
}

/// Where an Agent Session crosses into the user's territory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crossing {
    /// One of the user's windows, for `place_window`.
    UserWindow,
    /// A window another live Agent Session holds.
    HeldWindow,
    /// A window waiting on a Review Display.
    ReviewWindow,
    /// The Input Queue: the user's real pointer and keyboard.
    RealInput,
    /// Taking the front from the user's app for a front request.
    FrontHold,
    /// `via_input_method`.
    KeyboardBorrow,
}

/// What the crossing is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Subject {
    /// The window, or the target of real input, belongs to a Protected App.
    pub protected: bool,
    /// The window is a Kept Window.
    pub kept: bool,
    /// The window is In Use.
    pub in_use: bool,
    /// The user holds a mouse button with the pointer on the window.
    pub pressed: bool,
}

/// Why the hands are not free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    /// Input within the rest time.
    Active,
    /// A mouse button or a key is held.
    ButtonHeld,
    /// An input-method composition is open.
    Composing,
    SecureInput,
    /// A macOS system prompt is in front.
    SystemPrompt,
    /// The screen is locked.
    Locked,
    /// A hold ended less than `HOLD_GAP_MS` ago.
    HoldGap,
}

impl Busy {
    /// `input_queue status`'s `waitingOn`.
    pub fn waiting_on(self) -> &'static str {
        match self {
            Busy::Active | Busy::ButtonHeld | Busy::HoldGap => "userActive",
            Busy::Composing => "composing",
            Busy::SecureInput => "secureInput",
            Busy::SystemPrompt => "systemPrompt",
            Busy::Locked => "locked",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Today's behavior for this crossing: the card, the button, the rule.
    AskPath,
    /// Now.
    Go,
    /// Until the window is no longer In Use, or the button on it is let go.
    WaitWindow,
    /// Until the hands are At Rest, or the one condition YOLO respects.
    WaitHands(Busy),
}

fn hands(result: Result<(), Busy>) -> Decision {
    match result {
        Ok(()) => Decision::Go,
        Err(busy) => Decision::WaitHands(busy),
    }
}

/// The matrix of ADR 0025, and nothing else.
pub fn decide(mode: Mode, crossing: Crossing, subject: Subject, hands_now: &Hands) -> Decision {
    let Subject {
        protected,
        kept,
        in_use,
        pressed,
    } = subject;
    if mode == Mode::Ask || protected || kept {
        return Decision::AskPath;
    }
    // No mode moves a window while the screen is locked: In Use reads
    // nothing then, and the user may have been typing in it.
    if hands_now.locked && matches!(crossing, Crossing::UserWindow | Crossing::ReviewWindow) {
        return Decision::WaitHands(Busy::Locked);
    }
    match (crossing, mode) {
        (_, Mode::Ask) => Decision::AskPath,
        // Arbitration between two agents: no mode decides it.
        (Crossing::HeldWindow, _) => Decision::AskPath,
        // Not yet reviewed is not unused.
        (Crossing::ReviewWindow, Mode::Auto) => Decision::AskPath,
        (Crossing::ReviewWindow, Mode::Yolo) => Decision::Go,
        (Crossing::UserWindow, Mode::Auto) if in_use || pressed => Decision::WaitWindow,
        (Crossing::UserWindow, Mode::Auto) => Decision::Go,
        (Crossing::UserWindow, Mode::Yolo) if pressed => Decision::WaitWindow,
        (Crossing::UserWindow, Mode::Yolo) => Decision::Go,
        (Crossing::RealInput | Crossing::FrontHold, Mode::Auto) => hands(hands_now.at_rest()),
        (Crossing::KeyboardBorrow, Mode::Auto) => hands(hands_now.quiet_for_borrow()),
        (Crossing::RealInput | Crossing::FrontHold | Crossing::KeyboardBorrow, Mode::Yolo) => {
            hands(hands_now.yolo_may_start())
        }
    }
}

/// The mode an operation on a window of `protected`'s app runs under:
/// a Protected App keeps Ask's rules in every mode.
pub fn for_target(mode: Mode, protected: bool) -> Mode {
    if protected { Mode::Ask } else { mode }
}

/// Whether what an armed hold kept may go out after a mode's grant: the
/// saved front app `front` is in front again (`front_now`), and its saved
/// focused window, when there was one, is focused in it (`focused_now`).
/// The app reporting its window alone isn't enough: after a refused
/// activation the loan's app is still in front and would get the keys.
pub fn replay_confirmed(
    front: i32,
    window: Option<u32>,
    front_now: Option<i32>,
    focused_now: Option<u32>,
) -> bool {
    front_now == Some(front) && window.is_none_or(|w| focused_now == Some(w))
}

/// `kVK_ANSI_Period`.
pub const PERIOD_KEYCODE: u16 = 47;
/// `CGEventFlags`' device-independent modifier bits.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_OPTION: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;

/// The Stop chord, ⌃⌥⌘.: Control, Option and Command with Period, and no
/// Shift.
pub fn stop_chord(keycode: u16, flags: u64) -> bool {
    let modifiers = flags & (FLAG_SHIFT | FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND);
    keycode == PERIOD_KEYCODE && modifiers == FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND
}

/// Kept Windows: those the user took back or answered Keep for. No mode
/// takes one for any Agent Session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Kept(BTreeSet<WindowId>);

impl Kept {
    /// Take Back or Keep.
    pub fn keep(&mut self, window: WindowId) {
        self.0.insert(window);
    }

    /// The user handed it over themselves, or their words named it.
    pub fn release(&mut self, window: WindowId) -> bool {
        self.0.remove(&window)
    }

    /// The window closed.
    pub fn closed(&mut self, window: WindowId) {
        self.0.remove(&window);
    }

    /// Forget the windows not in `open`.
    pub fn retain_open(&mut self, open: &BTreeSet<WindowId>) {
        self.0.retain(|w| open.contains(w));
    }

    pub fn contains(&self, window: WindowId) -> bool {
        self.0.contains(&window)
    }

    pub fn ids(&self) -> impl Iterator<Item = WindowId> + '_ {
        self.0.iter().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_activity::{HOLD_GAP_MS, REST_MS, Sample, Tracker};

    const ALL: [Crossing; 6] = [
        Crossing::UserWindow,
        Crossing::HeldWindow,
        Crossing::ReviewWindow,
        Crossing::RealInput,
        Crossing::FrontHold,
        Crossing::KeyboardBorrow,
    ];

    fn sample(idle_ms: u64) -> Sample {
        Sample {
            now_ms: 100_000,
            idle_ms,
            ..Sample::default()
        }
    }

    fn hands_of(sample: Sample) -> Hands {
        let mut t = Tracker::default();
        t.observe(&sample);
        t.hands(sample.now_ms)
    }

    fn rested() -> Hands {
        hands_of(sample(60_000))
    }

    fn free() -> Subject {
        Subject::default()
    }

    fn used() -> Subject {
        Subject {
            in_use: true,
            ..Subject::default()
        }
    }

    #[test]
    fn ask_takes_todays_path_at_every_crossing() {
        for crossing in ALL {
            for subject in [free(), used()] {
                for h in [rested(), hands_of(sample(0))] {
                    assert_eq!(decide(Mode::Ask, crossing, subject, &h), Decision::AskPath);
                }
            }
        }
    }

    #[test]
    fn auto_takes_a_window_not_in_use_and_waits_for_one_in_use() {
        let busy = hands_of(sample(0));
        assert_eq!(
            decide(Mode::Auto, Crossing::UserWindow, free(), &busy),
            Decision::Go
        );
        assert_eq!(
            decide(Mode::Auto, Crossing::UserWindow, used(), &rested()),
            Decision::WaitWindow
        );
    }

    #[test]
    fn no_mode_takes_a_window_while_the_screen_is_locked() {
        let locked = hands_of(Sample {
            locked: true,
            ..sample(60_000)
        });
        for mode in [Mode::Auto, Mode::Yolo] {
            for crossing in [Crossing::UserWindow, Crossing::ReviewWindow] {
                let d = decide(mode, crossing, free(), &locked);
                assert!(
                    d == Decision::WaitHands(Busy::Locked) || d == Decision::AskPath,
                    "{mode:?} {crossing:?}: {d:?}"
                );
            }
            assert_eq!(
                decide(mode, Crossing::UserWindow, free(), &locked),
                Decision::WaitHands(Busy::Locked)
            );
        }
    }

    #[test]
    fn yolo_takes_a_window_in_use() {
        assert_eq!(
            decide(
                Mode::Yolo,
                Crossing::UserWindow,
                used(),
                &hands_of(sample(0))
            ),
            Decision::Go
        );
    }

    #[test]
    fn a_held_button_makes_yolo_wait_for_a_window() {
        let pressed = Subject {
            in_use: true,
            pressed: true,
            ..Subject::default()
        };
        for mode in [Mode::Auto, Mode::Yolo] {
            assert_eq!(
                decide(mode, Crossing::UserWindow, pressed, &rested()),
                Decision::WaitWindow
            );
        }
    }

    #[test]
    fn no_mode_takes_a_kept_window() {
        let kept = Subject {
            kept: true,
            ..Subject::default()
        };
        for mode in [Mode::Ask, Mode::Auto, Mode::Yolo] {
            for crossing in [Crossing::UserWindow, Crossing::ReviewWindow] {
                assert_eq!(decide(mode, crossing, kept, &rested()), Decision::AskPath);
            }
        }
    }

    #[test]
    fn no_mode_takes_a_protected_apps_window_or_sends_real_input_into_it() {
        let protected = Subject {
            protected: true,
            ..Subject::default()
        };
        for mode in [Mode::Ask, Mode::Auto, Mode::Yolo] {
            for crossing in ALL {
                assert_eq!(
                    decide(mode, crossing, protected, &rested()),
                    Decision::AskPath
                );
            }
        }
    }

    #[test]
    fn no_mode_moves_a_live_holders_window() {
        for mode in [Mode::Ask, Mode::Auto, Mode::Yolo] {
            assert_eq!(
                decide(mode, Crossing::HeldWindow, free(), &rested()),
                Decision::AskPath
            );
        }
    }

    #[test]
    fn only_yolo_moves_a_review_displays_window() {
        assert_eq!(
            decide(Mode::Auto, Crossing::ReviewWindow, free(), &rested()),
            Decision::AskPath
        );
        assert_eq!(
            decide(Mode::Yolo, Crossing::ReviewWindow, free(), &rested()),
            Decision::Go
        );
    }

    #[test]
    fn auto_opens_real_input_only_at_rest() {
        assert_eq!(
            decide(Mode::Auto, Crossing::RealInput, free(), &rested()),
            Decision::Go
        );
        assert_eq!(
            decide(
                Mode::Auto,
                Crossing::RealInput,
                free(),
                &hands_of(sample(REST_MS - 1))
            ),
            Decision::WaitHands(Busy::Active)
        );
        let held = Sample {
            button_down: true,
            ..sample(60_000)
        };
        assert_eq!(
            decide(Mode::Auto, Crossing::RealInput, free(), &hands_of(held)),
            Decision::WaitHands(Busy::ButtonHeld)
        );
    }

    #[test]
    fn yolo_opens_real_input_at_once_but_respects_secure_input_prompts_and_lock() {
        assert_eq!(
            decide(
                Mode::Yolo,
                Crossing::RealInput,
                free(),
                &hands_of(sample(0))
            ),
            Decision::Go
        );
        let composing = Sample {
            composing: true,
            ..sample(0)
        };
        assert_eq!(
            decide(
                Mode::Yolo,
                Crossing::RealInput,
                free(),
                &hands_of(composing)
            ),
            Decision::Go
        );
        for (s, busy) in [
            (
                Sample {
                    secure_input: true,
                    ..sample(0)
                },
                Busy::SecureInput,
            ),
            (
                Sample {
                    system_prompt: true,
                    ..sample(0)
                },
                Busy::SystemPrompt,
            ),
            (
                Sample {
                    locked: true,
                    ..sample(0)
                },
                Busy::Locked,
            ),
            (
                Sample {
                    key_down: true,
                    ..sample(0)
                },
                Busy::ButtonHeld,
            ),
        ] {
            assert_eq!(
                decide(Mode::Yolo, Crossing::RealInput, free(), &hands_of(s)),
                Decision::WaitHands(busy)
            );
        }
        // The gap after a hold holds in YOLO too.
        let mut t = Tracker::default();
        t.observe(&sample(0));
        t.hold_ended(100_000 - HOLD_GAP_MS + 1);
        assert_eq!(
            decide(Mode::Yolo, Crossing::RealInput, free(), &t.hands(100_000)),
            Decision::WaitHands(Busy::HoldGap)
        );
    }

    #[test]
    fn auto_starts_a_front_hold_at_rest_and_never_during_composition() {
        assert_eq!(
            decide(Mode::Auto, Crossing::FrontHold, free(), &rested()),
            Decision::Go
        );
        let composing = Sample {
            composing: true,
            ..sample(60_000)
        };
        assert_eq!(
            decide(
                Mode::Auto,
                Crossing::FrontHold,
                free(),
                &hands_of(composing.clone())
            ),
            Decision::WaitHands(Busy::Composing)
        );
        assert_eq!(
            decide(
                Mode::Yolo,
                Crossing::FrontHold,
                free(),
                &hands_of(composing)
            ),
            Decision::Go
        );
    }

    #[test]
    fn auto_keyboard_borrow_waits_for_two_seconds_of_quiet() {
        use crate::user_activity::BORROW_REST_MS;
        assert_eq!(
            decide(
                Mode::Auto,
                Crossing::KeyboardBorrow,
                free(),
                &hands_of(sample(BORROW_REST_MS))
            ),
            Decision::Go
        );
        assert_eq!(
            decide(
                Mode::Auto,
                Crossing::KeyboardBorrow,
                free(),
                &hands_of(sample(BORROW_REST_MS - 1))
            ),
            Decision::WaitHands(Busy::Active)
        );
        let composing = Sample {
            composing: true,
            ..sample(60_000)
        };
        assert_eq!(
            decide(
                Mode::Auto,
                Crossing::KeyboardBorrow,
                free(),
                &hands_of(composing)
            ),
            Decision::WaitHands(Busy::Composing)
        );
    }

    #[test]
    fn the_client_app_of_any_session_is_protected() {
        let clients: BTreeSet<String> = ["com.mitchellh.ghostty".to_string()].into();
        let none = BTreeSet::new();
        assert!(protected(Some("com.mitchellh.ghostty"), &clients, &none));
        assert!(!protected(Some("com.mitchellh.ghostty"), &none, &none));
        let added: BTreeSet<String> = ["com.example.bank".to_string()].into();
        assert!(protected(Some("com.example.bank"), &none, &added));
        assert!(protected(Some("com.apple.systempreferences"), &none, &none));
        assert!(!protected(None, &clients, &added));
        assert!(!protected(Some("com.apple.Safari"), &clients, &added));
    }

    #[test]
    fn protected_prefixes_cover_1password_builds() {
        let none = BTreeSet::new();
        for id in ["com.1password.1password", "com.agilebits.onepassword7"] {
            assert!(protected(Some(id), &none, &none), "{id}");
        }
        assert!(!protected(Some("com.1passwordx"), &none, &none));
    }

    #[test]
    fn yolo_is_never_stored() {
        let mut s = Setting::default();
        s.choose(Mode::Auto);
        s.choose(Mode::Yolo);
        let text = s.to_json().to_string();
        assert!(!text.contains("yolo\""), "{text}");
        assert_eq!(s.to_json()["mode"], "auto");
        // While YOLO is on, the file says it ended on a restart.
        assert_eq!(s.to_json()["yoloEnded"], "restarted");
        let loaded = Setting::from_json(&text);
        assert_eq!(loaded.mode(), Mode::Auto);
        assert_eq!(loaded.yolo_ended, Some(YoloEnd::Restarted));
    }

    #[test]
    fn a_file_that_says_yolo_loads_as_ask() {
        let s = Setting::from_json(r#"{"mode":"yolo","protected":["com.example.bank"]}"#);
        assert_eq!(s.mode(), Mode::Ask);
        assert!(s.protected.contains("com.example.bank"));
        assert_eq!(Setting::from_json(r#"{"mode":"turbo"}"#).mode(), Mode::Ask);
    }

    #[test]
    fn unreadable_json_loads_as_ask() {
        for text in ["", "{", "null", "[1,2]", "\"auto\""] {
            assert_eq!(Setting::from_json(text), Setting::default(), "{text}");
        }
    }

    #[test]
    fn stop_returns_to_ask_from_auto_and_from_yolo() {
        let mut s = Setting::default();
        s.choose(Mode::Auto);
        s.stop();
        assert_eq!(s.mode(), Mode::Ask);
        assert_eq!(s.yolo_ended, None);
        s.choose(Mode::Auto);
        s.choose(Mode::Yolo);
        s.stop();
        assert_eq!(s.mode(), Mode::Ask);
        assert_eq!(s.base, Base::Ask);
        assert_eq!(s.yolo_ended, Some(YoloEnd::Stopped));
        let loaded = Setting::from_json(&s.to_json().to_string());
        assert_eq!(loaded.yolo_ended, Some(YoloEnd::Stopped));
        // Settings shows no line for it: the user did it.
        assert!(!YoloEnd::Stopped.by_itself());
    }

    #[test]
    fn yolo_ends_on_lock_sleep_and_switch_and_keeps_the_stored_mode() {
        for (base, why) in [
            (Mode::Ask, YoloEnd::Locked),
            (Mode::Auto, YoloEnd::Slept),
            (Mode::Auto, YoloEnd::SwitchedOut),
        ] {
            let mut s = Setting::default();
            s.choose(base);
            s.choose(Mode::Yolo);
            assert!(s.end_yolo(why));
            assert_eq!(s.mode(), base);
            assert_eq!(s.yolo_ended, Some(why));
            assert_eq!(s.to_json()["yoloEnded"], why.as_str());
            // Not on: nothing ends.
            assert!(!s.end_yolo(YoloEnd::Locked));
            assert_eq!(s.yolo_ended, Some(why));
        }
        // The line stays until the next change of mode.
        let mut s = Setting::default();
        s.choose(Mode::Yolo);
        s.end_yolo(YoloEnd::Locked);
        s.choose(Mode::Ask);
        assert_eq!(s.yolo_ended, None);
    }

    #[test]
    fn choosing_yolo_keeps_the_stored_mode() {
        let mut s = Setting::default();
        s.choose(Mode::Auto);
        s.choose(Mode::Yolo);
        assert_eq!(s.mode(), Mode::Yolo);
        assert_eq!(s.base, Base::Auto);
        s.choose(Mode::Ask);
        assert_eq!(s.mode(), Mode::Ask);
        assert!(!s.yolo);
    }

    #[test]
    fn a_kept_window_is_released_by_the_users_own_handover_and_by_closing() {
        let mut kept = Kept::default();
        kept.keep(7);
        kept.keep(8);
        kept.keep(9);
        assert!(kept.contains(7));
        assert!(kept.release(7));
        assert!(!kept.contains(7));
        kept.closed(8);
        assert!(!kept.contains(8));
        kept.retain_open(&BTreeSet::new());
        assert!(!kept.contains(9));
        assert_eq!(kept.ids().count(), 0);
    }

    #[test]
    fn the_stop_chord_is_control_option_command_period_only() {
        let chord = FLAG_CONTROL | FLAG_OPTION | FLAG_COMMAND;
        assert!(stop_chord(PERIOD_KEYCODE, chord));
        // Caps Lock and the numeric-pad bit don't count.
        assert!(stop_chord(
            PERIOD_KEYCODE,
            chord | 0x0001_0000 | 0x0020_0000
        ));
        assert!(!stop_chord(PERIOD_KEYCODE, chord | FLAG_SHIFT));
        assert!(!stop_chord(PERIOD_KEYCODE, FLAG_CONTROL | FLAG_COMMAND));
        assert!(!stop_chord(PERIOD_KEYCODE, FLAG_COMMAND));
        assert!(!stop_chord(43, chord));
    }

    #[test]
    fn modes_read_and_write_their_names() {
        for mode in [Mode::Ask, Mode::Auto, Mode::Yolo] {
            assert_eq!(Mode::parse(mode.as_str()), Some(mode));
        }
        for end in [
            YoloEnd::Locked,
            YoloEnd::Slept,
            YoloEnd::SwitchedOut,
            YoloEnd::Restarted,
            YoloEnd::Stopped,
        ] {
            assert_eq!(YoloEnd::parse(end.as_str()), Some(end));
        }
    }

    #[test]
    fn a_damaged_protected_list_fails_closed() {
        let ok = Setting::from_json(
            r#"{"mode":"auto","protected":["com.example.bank"," "],"yoloEnded":null}"#,
        );
        assert_eq!(ok.mode(), Mode::Auto);
        assert_eq!(ok.protected.len(), 1);
        for text in [
            r#"{"mode":"auto","protected":"com.example.bank"}"#,
            r#"{"mode":"auto","protected":["com.example.bank",7]}"#,
            r#"{"mode":"auto","protected":null}"#,
            r#"{"mode":"auto","protected":{"a":1}}"#,
            r#"{"mode":"auto"}"#,
            r#"{"mode":"auto","yoloEnded":null}"#,
            r#"["auto"]"#,
            "{\"mode\":\"auto\"",
            r#"{"mode":"yolo"}"#,
            r#"{"mode":"AUTO"}"#,
        ] {
            let s = Setting::from_json(text);
            assert_eq!(s.mode(), Mode::Ask, "{text}");
            assert!(s.protected.is_empty(), "{text}");
        }
        let odd = Setting::from_json(r#"{"mode":"auto","protected":["x"],"yoloEnded":"later"}"#);
        assert_eq!(odd.mode(), Mode::Ask);
    }

    #[test]
    fn a_protected_app_keeps_ask_in_every_mode() {
        for mode in [Mode::Ask, Mode::Auto, Mode::Yolo] {
            assert_eq!(for_target(mode, true), Mode::Ask);
            assert_eq!(for_target(mode, false), mode);
        }
    }

    #[test]
    fn held_keys_go_out_only_with_the_front_and_window_back() {
        // Front and window back.
        assert!(replay_confirmed(7, Some(70), Some(7), Some(70)));
        // The app still reports its window, but the loan's app is front.
        assert!(!replay_confirmed(7, Some(70), Some(9), Some(70)));
        // Front back, another window focused.
        assert!(!replay_confirmed(7, Some(70), Some(7), Some(71)));
        // No saved window: the front decides.
        assert!(replay_confirmed(7, None, Some(7), None));
        assert!(!replay_confirmed(7, None, Some(9), None));
    }
}
