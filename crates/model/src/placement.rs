//! How `place_window` finds the window an agent names, and what it says
//! when it finds none (#133). An agent named WeCom by its process name,
//! 企业微信, and by its bundle id, while place_window compared only the
//! window list's owner name, "WeCom", and every answer was the same
//! `not_found`.

/// Whether `wanted`, an app as an agent names it, is a window's app: the
/// window list's owner name, the app's name as the user sees it, or its
/// bundle id, ignoring case and surrounding spaces.
pub fn names_app(wanted: &str, owner: &str, name: Option<&str>, bundle_id: Option<&str>) -> bool {
    let wanted = wanted.trim().to_lowercase();
    !wanted.is_empty()
        && [Some(owner), name, bundle_id]
            .into_iter()
            .flatten()
            .any(|n| n.trim().to_lowercase() == wanted)
}

/// What became of a window an agent asked for by id that isn't one of its
/// strays or a window of the user's on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowSighting {
    /// No window has that id.
    Gone,
    /// It exists, but not on screen: minimized, hidden, closed to the
    /// Dock, or on another macOS Space.
    OffScreen { app: Option<String> },
    /// On screen, but a panel, dialog or menu rather than a document
    /// window.
    NotDocument { app: String },
    /// On another Agent Display.
    OnAgentDisplay { app: String },
}

/// The `not_found` message for window `id`.
pub fn no_window(id: i64, sighting: &WindowSighting) -> String {
    match sighting {
        WindowSighting::Gone => format!(
            "no window has id {id}: it closed, or the id is from another list. Call list_windows with strays: true, or place_window with the app's name"
        ),
        WindowSighting::OffScreen { app } => format!(
            "window {id}{} isn't on screen: it is minimized, hidden or on another macOS Space. Ask the user to bring it up, then call place_window again",
            app.as_deref()
                .map(|a| format!(" ({a})"))
                .unwrap_or_default()
        ),
        WindowSighting::NotDocument { app } => format!(
            "window {id} ({app}) is a panel or dialog; place_window moves document windows. Place the {app} window it belongs to"
        ),
        WindowSighting::OnAgentDisplay { app } => format!(
            "window {id} ({app}) is on another Agent Display, which another agent session holds"
        ),
    }
}

/// What place_window knows of an app no window matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppSighting {
    /// No running or installed app has that name or bundle id.
    Unknown,
    /// Installed, not running.
    NotRunning { name: String },
    /// Running, with no window on the user's screen.
    NoWindow { name: String },
}

/// The `not_found` message for an app the agent named. It says what was
/// compared, so an agent can tell a name that matched nothing from an app
/// with no window to offer.
pub fn no_app_window(app: &str, sighting: &AppSighting) -> String {
    match sighting {
        AppSighting::Unknown => format!(
            "no window's app is {app:?}, compared with each window's owner name, its app's name and its bundle id, ignoring case, and no app by that name or bundle id is running or installed. Check the name, or pass a window_id"
        ),
        AppSighting::NotRunning { name } => format!(
            "{name} isn't running. Start it with launch_app, which opens its window on the display"
        ),
        AppSighting::NoWindow { name } => format!(
            "{name} is running, but none of its windows is on the user's screen: they are closed, minimized, hidden or on another macOS Space. Ask the user to open or bring up its window, then call place_window again"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_is_named_by_its_owner_its_name_or_its_bundle_id() {
        let wecom = |wanted| {
            names_app(
                wanted,
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
        assert!(names_app("Field", "Field", None, None));
        assert!(!names_app("企业微信", "WeCom", None, None));
    }

    #[test]
    fn each_not_found_says_what_was_searched_or_where_the_window_is() {
        let unknown = no_app_window("WeWork", &AppSighting::Unknown);
        assert!(unknown.contains("\"WeWork\"") && unknown.contains("bundle id"));
        assert!(
            no_app_window(
                "企业微信",
                &AppSighting::NoWindow {
                    name: "企业微信".into()
                }
            )
            .starts_with("企业微信 is running")
        );
        assert!(
            no_app_window("UTM", &AppSighting::NotRunning { name: "UTM".into() })
                .contains("launch_app")
        );
        assert!(no_window(7, &WindowSighting::Gone).starts_with("no window has id 7"));
        assert!(
            no_window(
                7,
                &WindowSighting::OffScreen {
                    app: Some("WeCom".into())
                }
            )
            .starts_with("window 7 (WeCom) isn't on screen")
        );
        assert!(
            no_window(7, &WindowSighting::OffScreen { app: None })
                .starts_with("window 7 isn't on screen")
        );
        assert!(
            no_window(
                7,
                &WindowSighting::OnAgentDisplay {
                    app: "Field".into()
                }
            )
            .contains("another Agent Display")
        );
    }
}
