//! Whether keys posted to an app reach the agent's window (#120). An app
//! sends every key it gets to its key window, whichever that is, so keys
//! meant for the agent's window reach the user's when one of theirs in the
//! same app has key focus. The app's focused window, as Accessibility
//! reports it, is checked before any key goes out.

/// One of the app's windows, as the window list shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppWindow {
    pub id: u32,
    /// On one of Agent Screen's displays, not one of the user's screens.
    pub on_agent_display: bool,
}

/// Where keys posted to the app go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyWindow {
    /// The agent's window has key focus.
    Target,
    /// Another window on an Agent Display has it, or Accessibility doesn't
    /// say while every window of the app is on one: the keys stay off the
    /// user's screens.
    AgentDisplay,
    /// A window off the Agent Displays has it, maybe the user's, or
    /// Accessibility doesn't say while the app has such a window: no key
    /// may go out. The window, when known.
    Elsewhere(Option<u32>),
}

impl KeyWindow {
    pub fn safe(self) -> bool {
        !matches!(self, KeyWindow::Elsewhere(_))
    }
}

/// Where keys go with `focused` the app's focused window, for keys meant
/// for window `target`. A focused window missing from `windows` is hidden
/// or minimized, so it can't be shown to be on an Agent Display.
pub fn key_window(target: u32, focused: Option<u32>, windows: &[AppWindow]) -> KeyWindow {
    match focused {
        Some(id) if id == target => KeyWindow::Target,
        Some(id) if windows.iter().any(|w| w.id == id && w.on_agent_display) => {
            KeyWindow::AgentDisplay
        }
        Some(id) => KeyWindow::Elsewhere(Some(id)),
        None if windows.iter().all(|w| w.on_agent_display) => KeyWindow::AgentDisplay,
        None => KeyWindow::Elsewhere(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: AppWindow = AppWindow {
        id: 1,
        on_agent_display: true,
    };
    const OTHER_AGENT: AppWindow = AppWindow {
        id: 2,
        on_agent_display: true,
    };
    const USER: AppWindow = AppWindow {
        id: 3,
        on_agent_display: false,
    };

    #[test]
    fn the_agents_window_focused_takes_the_keys() {
        assert_eq!(key_window(1, Some(1), &[AGENT, USER]), KeyWindow::Target);
    }

    #[test]
    fn the_users_window_focused_refuses_them() {
        let reach = key_window(1, Some(3), &[AGENT, USER]);
        assert_eq!(reach, KeyWindow::Elsewhere(Some(3)));
        assert!(!reach.safe());
    }

    #[test]
    fn a_focused_window_not_listed_may_be_the_users() {
        assert_eq!(
            key_window(1, Some(9), &[AGENT]),
            KeyWindow::Elsewhere(Some(9))
        );
    }

    #[test]
    fn another_window_on_an_agent_display_keeps_them_off_the_users_screens() {
        let reach = key_window(1, Some(2), &[AGENT, OTHER_AGENT, USER]);
        assert_eq!(reach, KeyWindow::AgentDisplay);
        assert!(reach.safe());
    }

    #[test]
    fn unknown_focus_is_safe_only_with_no_window_elsewhere() {
        assert_eq!(
            key_window(1, None, &[AGENT, OTHER_AGENT]),
            KeyWindow::AgentDisplay
        );
        assert_eq!(
            key_window(1, None, &[AGENT, USER]),
            KeyWindow::Elsewhere(None)
        );
    }
}
