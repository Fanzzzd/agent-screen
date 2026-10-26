//! macOS's own prompts belong to the user, wherever macOS opens them. Which
//! processes draw them, by the owner name the window server lists.

/// Processes that draw the system's alerts and prompts.
const OWNERS: &[&str] = &[
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

/// A window of `owner` is one of macOS's alerts or prompts.
pub fn is_system_alert(owner: &str) -> bool {
    OWNERS.contains(&owner)
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
    fn apps_are_not_alerts() {
        assert!(!is_system_alert("Electron"));
        assert!(!is_system_alert("Finder"));
        assert!(!is_system_alert(""));
    }
}
