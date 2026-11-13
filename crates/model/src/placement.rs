//! How `place_window` finds the window an agent names, and what it says
//! when it finds none (#133). An agent named WeCom by its process name,
//! 企业微信, and by its bundle id, while place_window compared only the
//! window list's owner name, "WeCom", and every answer was the same
//! `not_found`.

use crate::model::Rect;

/// An app as an agent named it for place_window, with the bundle id that
/// name resolved to once, by a running app's name or bundle id, an
/// installed app's name, or an .app path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedApp {
    text: String,
    bundle_id: Option<String>,
}

impl NamedApp {
    pub fn new(text: &str, resolved_bundle_id: Option<String>) -> Self {
        Self {
            text: text.trim().to_lowercase(),
            bundle_id: resolved_bundle_id.map(|b| b.to_lowercase()),
        }
    }

    /// Whether the window list's owner name alone names the app, so the
    /// app's own name and bundle id needn't be looked up.
    pub fn is_owner(&self, owner: &str) -> bool {
        !self.text.is_empty() && owner.trim().to_lowercase() == self.text
    }

    /// Whether a window with this owner name, of an app with this name and
    /// bundle id, is the app's: any of the three is the name the agent
    /// gave, ignoring case and surrounding spaces, or the bundle id is the
    /// one that name resolved to.
    pub fn is_app(&self, owner: &str, name: Option<&str>, bundle_id: Option<&str>) -> bool {
        let same = |n: &str| n.trim().to_lowercase() == self.text;
        self.is_owner(owner)
            || (!self.text.is_empty() && name.is_some_and(same))
            || bundle_id.is_some_and(|b| {
                let b = b.to_lowercase();
                (!self.text.is_empty() && b == self.text) || self.bundle_id.as_ref() == Some(&b)
            })
    }
}

/// What became of a window an agent asked for by id that isn't one of its
/// strays or a window of the user's on screen. Only a window on the user's
/// screen is named: the agent may have guessed the id of a hidden window
/// or of another agent's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowSighting {
    /// No window has that id.
    Gone,
    /// It exists, but not on screen: minimized, hidden, closed to the
    /// Dock, or on another macOS Space.
    OffScreen,
    /// On the user's screen, but a panel, dialog or menu rather than a
    /// document window.
    NotDocument { app: String },
    /// On another Agent Display, which `Holder` has.
    OnAgentDisplay(Holder),
}

/// Who has the Agent Display a window place_window didn't offer is on. A
/// `not_found` blames another agent session only when one holds the
/// display: an agent was told other sessions held a window that sat on its
/// own ended display, with no other session running (#161).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Holder {
    /// The asking Agent Session uses it: one of its own other displays.
    Mine,
    /// Another Agent Session uses it.
    OtherSession,
    /// A Review Display: its Agent Session stopped using it, and it waits
    /// for the user.
    Review,
    /// No Agent Session of this app uses it: a display the user made from
    /// Peek, a spare, or another Agent Screen instance's display.
    Other,
}

impl Holder {
    /// The holder to name for an app's windows on displays with these
    /// holders: the session's own display first, since place_window moves
    /// a window from there, then a Review Display, since the user can take
    /// a window back from it, and another session only when every display
    /// is one.
    pub fn of_all(holders: impl IntoIterator<Item = Holder>) -> Holder {
        let holders: Vec<Holder> = holders.into_iter().collect();
        if holders.contains(&Holder::Mine) {
            Holder::Mine
        } else if holders.contains(&Holder::Review) {
            Holder::Review
        } else if !holders.is_empty() && holders.iter().all(|h| *h == Holder::OtherSession) {
            Holder::OtherSession
        } else {
            Holder::Other
        }
    }
}

/// The `not_found` message for window `id`.
pub fn no_window(id: i64, sighting: &WindowSighting) -> String {
    match sighting {
        WindowSighting::Gone => format!(
            "no window has id {id}: it closed, or the id is from another list. Call list_windows with strays: true, or place_window with the app's name"
        ),
        WindowSighting::OffScreen => format!(
            "window {id} isn't on screen: it is minimized, hidden or on another macOS Space. Ask the user to bring it up, then call place_window again"
        ),
        WindowSighting::NotDocument { app } => format!(
            "window {id} ({app}) is a panel or dialog; place_window moves document windows. Place the {app} window it belongs to"
        ),
        WindowSighting::OnAgentDisplay(Holder::Mine) => format!(
            "window {id} is on another of your Agent Displays, and it moved or closed while place_window looked for it. Call list_windows on your displays, then place_window again"
        ),
        WindowSighting::OnAgentDisplay(Holder::OtherSession) => {
            format!("window {id} is on another Agent Display, which another agent session holds")
        }
        WindowSighting::OnAgentDisplay(Holder::Review) => format!(
            "window {id} is on an Agent Display whose session ended, waiting for the user to review it. Ask the user to take it back, then call place_window again"
        ),
        WindowSighting::OnAgentDisplay(Holder::Other) => format!(
            "window {id} is on another Agent Display, which isn't yours. Ask the user to bring it to their screen, then call place_window again"
        ),
    }
}

/// What place_window knows of an app no window it could offer matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppSighting {
    /// Nothing checked has that name or bundle id.
    Unknown,
    /// Installed, not running.
    NotRunning { name: String },
    /// Running, with no window on screen.
    NoWindow { name: String },
    /// Its only windows on screen are panels and dialogs.
    OnlyPanels { name: String },
    /// Its document window is on this display already.
    OnThisDisplay { window_id: u32 },
    /// Its document windows on screen are on other Agent Displays, which
    /// `Holder` has.
    OnAgentDisplay(Holder),
}

/// The `not_found` message for an app the agent named. It says what was
/// compared, so an agent can tell a name that matched nothing from an app
/// with no window to offer.
pub fn no_app_window(app: &str, sighting: &AppSighting) -> String {
    match sighting {
        AppSighting::Unknown => format!(
            "nothing matched {app:?}, ignoring case: no window's owner name, no running app's name or bundle id, no installed app macOS knows by that name, and none with that bundle id. Check the name, or pass a window_id"
        ),
        AppSighting::NotRunning { name } => format!(
            "{name} isn't running. Start it with launch_app, which opens its window on the display"
        ),
        AppSighting::NoWindow { name } => format!(
            "{name} is running, but none of its windows is on screen: they are closed, minimized, hidden or on another macOS Space. Ask the user to open or bring up its window, then call place_window again. An app whose only window was closed, as a chat app's main window, reopens it when the user clicks its Dock icon; launch_app opens no window in a running app whose menus have no New Window"
        ),
        AppSighting::OnlyPanels { name } => format!(
            "{name}'s only windows on the user's screen are panels or dialogs; place_window moves document windows. Ask the user to open its main window"
        ),
        AppSighting::OnThisDisplay { window_id } => format!(
            "its window {window_id} is on your display already; pass that window_id to set its frame"
        ),
        AppSighting::OnAgentDisplay(Holder::Mine) => {
            "its windows on your other Agent Displays are panels or dialogs, windows the user took back or opened, or on a display the user is operating, so none can move. Call list_windows on those displays, and ask the user for a window of theirs"
                .into()
        }
        AppSighting::OnAgentDisplay(Holder::OtherSession) => {
            "its windows on screen are on other Agent Displays, which other agent sessions hold"
                .into()
        }
        AppSighting::OnAgentDisplay(Holder::Review) => {
            "its window is on an Agent Display whose session ended, waiting for the user to review it. Ask the user to take it back, then call place_window again"
                .into()
        }
        AppSighting::OnAgentDisplay(Holder::Other) => {
            "its windows on screen are on another Agent Display, which isn't yours. Ask the user to bring one to their screen, then call place_window again"
                .into()
        }
    }
}

/// What a frame write did to a window, judged by the frame it has after the
/// write rather than by the app's Accessibility answer. Chromium answers a
/// size write with an error even when the window takes the size, and
/// place_window called a Dia window that had moved and resized rejected
/// (#167).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWrite {
    /// The window has the frame asked for.
    Took,
    /// It changed, but not all the way: the app kept to its own limits.
    Partly,
    /// It kept the frame it had.
    Unchanged,
}

/// How far, in points, a frame may be from another and count as the same:
/// apps round to whole points, some to their content's units.
const SAME_FRAME: f64 = 1.0;

/// What a write asking for `asked` did to a window at `before` that is at
/// `after` once it settled. All three are global frames in points.
pub fn frame_write(before: Rect, asked: Rect, after: Rect) -> FrameWrite {
    let same = |a: Rect, b: Rect| {
        [
            (a.x, b.x),
            (a.y, b.y),
            (a.width, b.width),
            (a.height, b.height),
        ]
        .iter()
        .all(|(p, q)| (p - q).abs() <= SAME_FRAME)
    };
    if same(after, asked) {
        FrameWrite::Took
    } else if same(after, before) {
        FrameWrite::Unchanged
    } else {
        FrameWrite::Partly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_write_is_judged_by_the_frame_after_it() {
        let before = Rect::new(38.0, 38.0, 1281.0, 772.0);
        let asked = Rect::new(0.0, 30.0, 960.0, 770.0);
        assert_eq!(frame_write(before, asked, asked), FrameWrite::Took);
        // Rounded by the app.
        assert_eq!(
            frame_write(before, asked, Rect::new(0.0, 30.5, 960.0, 769.0)),
            FrameWrite::Took
        );
        // Moved and resized, but kept a minimum height: #167's Dia window.
        assert_eq!(
            frame_write(before, asked, Rect::new(0.0, 30.0, 960.0, 790.0)),
            FrameWrite::Partly
        );
        // Moved only.
        assert_eq!(
            frame_write(before, asked, Rect::new(0.0, 30.0, 1281.0, 772.0)),
            FrameWrite::Partly
        );
        assert_eq!(frame_write(before, asked, before), FrameWrite::Unchanged);
        // Asked for the frame it had: that is taking it.
        assert_eq!(frame_write(before, before, before), FrameWrite::Took);
    }

    #[test]
    fn an_app_is_named_by_its_owner_its_name_or_its_bundle_id() {
        let wecom = |wanted| {
            NamedApp::new(wanted, None).is_app(
                "WeCom",
                Some("企业微信"),
                Some("com.tencent.WeWorkMac"),
            )
        };
        for wanted in [
            "WeCom",
            "wecom",
            "企业微信",
            "com.tencent.weworkmac",
            " WeCom ",
        ] {
            assert!(wecom(wanted), "{wanted}");
        }
        for wanted in ["WeWork", "", "  ", "com.tencent"] {
            assert!(!wecom(wanted), "{wanted}");
        }
        assert!(NamedApp::new("Field", None).is_owner("Field"));
        assert!(!NamedApp::new("企业微信", None).is_owner("WeCom"));
    }

    #[test]
    fn a_path_or_installed_name_matches_by_the_bundle_id_it_resolved_to() {
        let by_path = NamedApp::new(
            "/tmp/e2e/Field.app",
            Some("app.agentscreen.e2e.field".into()),
        );
        assert!(by_path.is_app("Field", Some("Field"), Some("app.agentscreen.e2e.Field")));
        assert!(!by_path.is_app("Other", Some("Other"), Some("com.example.other")));
        assert!(
            !by_path.is_app("Field", Some("Field"), None),
            "no bundle id to compare"
        );
    }

    #[test]
    fn each_not_found_says_what_was_searched_or_where_the_window_is() {
        let unknown = no_app_window("WeWork", &AppSighting::Unknown);
        assert!(unknown.contains("\"WeWork\"") && unknown.contains("bundle id"));
        assert!(
            !unknown.contains("is running"),
            "claims only what it checked"
        );
        let name = || "企业微信".to_string();
        let closed = no_app_window("企业微信", &AppSighting::NoWindow { name: name() });
        assert!(closed.starts_with("企业微信 is running"));
        // #162: how a closed main window comes back.
        assert!(closed.contains("Dock icon") && closed.contains("launch_app"));
        let panels = no_app_window("企业微信", &AppSighting::OnlyPanels { name: name() });
        assert!(panels.contains("panels or dialogs") && !panels.contains("none of its windows"));
        assert!(
            no_app_window("Field", &AppSighting::OnThisDisplay { window_id: 9 })
                .contains("window_id")
        );
        assert!(
            no_app_window("UTM", &AppSighting::NotRunning { name: "UTM".into() })
                .contains("launch_app")
        );
        assert!(no_window(7, &WindowSighting::Gone).starts_with("no window has id 7"));
        assert_eq!(
            no_window(7, &WindowSighting::OffScreen),
            "window 7 isn't on screen: it is minimized, hidden or on another macOS Space. Ask the user to bring it up, then call place_window again"
        );
        let other = no_window(7, &WindowSighting::OnAgentDisplay(Holder::OtherSession));
        assert!(other.starts_with("window 7 is on another Agent Display"));
        assert!(
            no_window(
                7,
                &WindowSighting::NotDocument {
                    app: "WeCom".into()
                }
            )
            .starts_with("window 7 (WeCom) is a panel")
        );
    }

    #[test]
    fn only_a_display_another_session_holds_is_blamed_on_one() {
        use Holder::*;
        assert_eq!(Holder::of_all([OtherSession, OtherSession]), OtherSession);
        assert_eq!(Holder::of_all([OtherSession, Review]), Review);
        assert_eq!(Holder::of_all([OtherSession, Other]), Other);
        assert_eq!(Holder::of_all([]), Other);
        assert_eq!(Holder::of_all([OtherSession, Review, Mine]), Mine);
        for text in [
            no_app_window("Field", &AppSighting::OnAgentDisplay(Mine)),
            no_window(7, &WindowSighting::OnAgentDisplay(Mine)),
        ] {
            assert!(
                text.contains("of your") || text.contains("your other"),
                "{text}"
            );
            assert!(!text.contains("session holds"), "{text}");
        }
        assert!(!no_window(7, &WindowSighting::OnAgentDisplay(Mine)).contains("operating"));
        for holder in [Review, Other] {
            let app = no_app_window("WeCom", &AppSighting::OnAgentDisplay(holder));
            let window = no_window(7, &WindowSighting::OnAgentDisplay(holder));
            for text in [&app, &window] {
                assert!(!text.contains("session holds"), "{text}");
                assert!(!text.contains("sessions hold"), "{text}");
                assert!(text.contains("place_window again"), "{text}");
            }
        }
        // #161: the session's own ended display, waiting for review.
        assert!(
            no_app_window("WeCom", &AppSighting::OnAgentDisplay(Review))
                .contains("waiting for the user to review it")
        );
        assert!(
            no_app_window("WeCom", &AppSighting::OnAgentDisplay(OtherSession))
                .contains("other agent sessions hold")
        );
    }
}
