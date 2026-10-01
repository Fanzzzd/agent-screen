import AppKit
import UserNotifications

/// Posts a system notification for each Help Request and withdraws it when
/// the request ends. Clicking the notification opens the display.
@MainActor
final class HelpNotifier: NSObject, UNUserNotificationCenterDelegate {
    var onOpen: ((CGDirectDisplayID) -> Void)?

    private let center = UNUserNotificationCenter.current()
    private var posted: [CGDirectDisplayID: String] = [:]

    init(displays: DisplayManager) {
        super.init()
        center.delegate = self
        displays.observeState { [weak self] display in self?.update(display) }
    }

    private func update(_ display: VirtualDisplay) {
        let id = "help-\(display.id)"
        guard let reason = display.helpRequest else {
            if posted.removeValue(forKey: display.id) != nil {
                center.removeDeliveredNotifications(withIdentifiers: [id])
            }
            return
        }
        guard posted[display.id] != reason else { return }
        posted[display.id] = reason

        let content = UNMutableNotificationContent()
        content.title = "\(display.label) 需要你帮忙"
        content.body = reason
        content.sound = .default
        content.userInfo = ["displayId": Int(display.id)]
        let request = UNNotificationRequest(identifier: id, content: content, trigger: nil)
        Task {
            do {
                guard try await center.requestAuthorization(options: [.alert, .sound]) else { return }
                try await center.add(request)
            } catch {
                NSLog("Sidescreen: notifications unavailable: \(error.localizedDescription)")
            }
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification)
        async -> UNNotificationPresentationOptions
    {
        [.banner, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse) async {
        guard let id = response.notification.request.content.userInfo["displayId"] as? Int else { return }
        await MainActor.run { onOpen?(CGDirectDisplayID(id)) }
    }
}
