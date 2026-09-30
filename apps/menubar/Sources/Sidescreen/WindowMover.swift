import AppKit
import SidescreenPrivate

/// Moves other apps' windows between displays through the Accessibility API.
@MainActor
enum WindowMover {
    enum Failure: Error {
        case noFrontmostApp, noWindow, rejected
    }

    /// The focused window of the frontmost app other than us.
    static func frontmostWindow() throws(Failure) -> WindowTarget {
        guard let app = NSWorkspace.shared.frontmostApplication,
              app.processIdentifier != getpid()
        else { throw .noFrontmostApp }
        let element = AXUIElementCreateApplication(app.processIdentifier)
        guard let window = copy(element, kAXFocusedWindowAttribute) ?? copy(element, kAXMainWindowAttribute)
        else { throw .noWindow }
        var windowID: CGWindowID = 0
        guard SSAXWindowID(window, &windowID) else { throw .noWindow }
        return WindowTarget(pid: app.processIdentifier, windowID: windowID)
    }

    /// Move a specific window onto `bounds`.
    static func move(_ target: WindowTarget, into bounds: CGRect) throws(Failure) {
        let app = AXUIElementCreateApplication(target.pid)
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
              let windows = value as? [AXUIElement]
        else { throw .noWindow }
        let match = windows.first { window in
            var windowID: CGWindowID = 0
            return SSAXWindowID(window, &windowID) && windowID == target.windowID
        }
        guard let match else { throw .noWindow }
        try place(match, into: bounds)
    }

    /// Fit the window inside `bounds` (CG global coordinates) with a margin.
    private static func place(_ window: AXUIElement, into bounds: CGRect) throws(Failure) {
        let area = bounds.insetBy(dx: 40, dy: 40)
        var size = CGSize(width: 800, height: 600)
        if let raw = copy(window, kAXSizeAttribute) {
            AXValueGetValue(raw as! AXValue, .cgSize, &size)
        }
        var fitted = CGSize(width: min(size.width, area.width), height: min(size.height, area.height))
        var origin = CGPoint(x: area.midX - fitted.width / 2, y: area.midY - fitted.height / 2)

        guard let position = AXValueCreate(.cgPoint, &origin),
              AXUIElementSetAttributeValue(window, kAXPositionAttribute as CFString, position) == .success
        else { throw .rejected }
        if fitted != size, let sizeValue = AXValueCreate(.cgSize, &fitted) {
            AXUIElementSetAttributeValue(window, kAXSizeAttribute as CFString, sizeValue)
        }
    }

    private static func copy(_ element: AXUIElement, _ attribute: String) -> AXUIElement? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID()
        else { return nil }
        return (value as! AXUIElement)
    }
}

enum Permissions {
    static var screenRecording: Bool { CGPreflightScreenCaptureAccess() }
    static var accessibility: Bool { AXIsProcessTrusted() }

    /// Screen Recording feeds the previews. Accessibility lets us post input
    /// to other apps and move their windows.
    static func requestMissing() {
        if !screenRecording { CGRequestScreenCaptureAccess() }
        if !accessibility {
            AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
        }
    }
}
