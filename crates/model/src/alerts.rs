//! System authentication and portal prompts belong to the user, wherever the OS
//! opens them. Which processes draw them, parameterized by platform owner list.

/// macOS processes that draw system alerts and authentication prompts.
pub const MACOS_ALERT_OWNERS: &[&str] = &[
    "SecurityAgent",
    "UserNotificationCenter",
    "CoreServicesUIAgent",
    "coreautha",
    "AuthenticationServicesAgent",
    "AKAuthorizationRemoteViewService",
    "akd",
    "universalAccessAuthWarn",
    "Captive Network Assistant",
    // The Apple Account sign-in alert and its verification code (#22).
    "FollowUpUI",
];

/// Linux polkit, portal, and pinentry processes that draw user-owned system prompts.
pub const LINUX_ALERT_OWNERS: &[&str] = &[
    "polkit-gnome-authentication-agent-1",
    "lxpolkit",
    "polkit-kde-authentication-agent-1",
    "hyprpolkitagent",
    "xdg-desktop-portal-gtk",
    "xdg-desktop-portal-gnome",
    "xdg-desktop-portal-kde",
    "pinentry-gtk-2",
    "pinentry-gnome3",
    "pinentry-qt",
];

/// Whether `owner` appears in the supplied platform alert-owner list.
pub fn is_system_alert_in(owner: &str, owners: &[&str]) -> bool {
    owners.contains(&owner)
}

/// A window of `owner` is one of macOS's alerts or prompts.
pub fn is_system_alert(owner: &str) -> bool {
    is_system_alert_in(owner, MACOS_ALERT_OWNERS)
}

/// A window of `owner` is one of Linux's polkit, portal, or pinentry prompts.
pub fn is_linux_system_alert(owner: &str) -> bool {
    is_system_alert_in(owner, LINUX_ALERT_OWNERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_and_notification_prompts_are_alerts() {
        assert!(is_system_alert("SecurityAgent"));
        assert!(is_system_alert("UserNotificationCenter"));
    }

    /// "Your Apple Account is being used to sign in to a new device", with
    /// Don't Allow and Allow, and the verification code after it: windows of
    /// CoreFollowUp's FollowUpUI.app (#22).
    #[test]
    fn apple_account_sign_in_alert_is_an_alert() {
        assert!(is_system_alert("FollowUpUI"));
    }

    #[test]
    fn linux_polkit_and_pinentry_prompts_are_alerts() {
        assert!(is_linux_system_alert("hyprpolkitagent"));
        assert!(is_linux_system_alert("pinentry-gnome3"));
        assert!(!is_linux_system_alert("chromium"));
    }

    #[test]
    fn apps_are_not_alerts() {
        assert!(!is_system_alert("Electron"));
        assert!(!is_system_alert("Finder"));
        assert!(!is_system_alert(""));
    }
}
