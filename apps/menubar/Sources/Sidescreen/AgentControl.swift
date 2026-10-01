import AppKit
import ScreenCaptureKit

/// One InputForwarder per display, so each display remembers its own focused
/// window. The Viewer and the Agent Session of a display share it.
@MainActor
final class ForwarderPool {
    private var forwarders: [CGDirectDisplayID: InputForwarder] = [:]

    func forwarder(for displayID: CGDirectDisplayID) -> InputForwarder {
        if let existing = forwarders[displayID] { return existing }
        let forwarder = InputForwarder()
        forwarders[displayID] = forwarder
        return forwarder
    }

    func drop(_ displayID: CGDirectDisplayID) {
        forwarders.removeValue(forKey: displayID)
    }
}

/// Serves control requests from Agent Sessions. Coordinates are in points
/// relative to the display's top-left corner, the same space as screenshots.
@MainActor
final class AgentControl {
    private let displays: DisplayManager
    private let forwarders: ForwarderPool
    private let windowState = WindowState()

    init(displays: DisplayManager, forwarders: ForwarderPool) {
        self.displays = displays
        self.forwarders = forwarders
    }

    func handle(connection: Int, method: String, params p: Params) async throws -> [String: Any] {
        switch method {
        case "createDisplay": return try createDisplay(connection: connection, p)
        case "endDisplay":
            displays.end(try display(p, live: false))
            return [:]
        case "screenshot": return try await screenshot(p)
        case "click":
            let (display, point) = try target(p)
            showPointer(display, at: point, click: true)
            await forwarders.forwarder(for: display.id).click(
                at: point, button: try button(p), count: p.int("count") ?? 1, flags: try modifiers(p))
            return [:]
        case "move":
            let (display, point) = try target(p)
            showPointer(display, at: point, click: false)
            forwarders.forwarder(for: display.id).mouseMoved(to: point, flags: [])
            return [:]
        case "drag":
            let display = try inputDisplay(p)
            let from = try global(display, x: p.require("fromX"), y: p.require("fromY"))
            let to = try global(display, x: p.require("toX"), y: p.require("toY"))
            showPointer(display, at: from, click: true)
            defer { showPointer(display, at: to, click: false) }
            await forwarders.forwarder(for: display.id).drag(from: from, to: to, flags: try modifiers(p))
            return [:]
        case "scroll":
            let (display, point) = try target(p)
            showPointer(display, at: point, click: false)
            forwarders.forwarder(for: display.id).scroll(
                at: point, deltaX: p.double("deltaX") ?? 0, deltaY: p.double("deltaY") ?? 0,
                continuous: false, phase: 0, momentumPhase: 0, flags: try modifiers(p))
            return [:]
        case "type":
            let display = try inputDisplay(p)
            guard let text = p.string("text") else { throw ControlError.invalid("text is required") }
            await forwarders.forwarder(for: display.id).typeText(text, within: display.bounds)
            return [:]
        case "key":
            let display = try inputDisplay(p)
            guard let keys = p.string("keys") else { throw ControlError.invalid("keys is required") }
            let chord: KeyChord
            do { chord = try KeyChord.parse(keys) } catch { throw ControlError.invalid(error.description) }
            await forwarders.forwarder(for: display.id).press(chord, repeat: p.int("repeat") ?? 1,
                                                              within: display.bounds)
            return [:]
        case "listWindows":
            let display = try display(p, live: false)
            return ["windows": WindowCatalog.windows(on: display).map { window in
                var json = window.json
                let record = display.records[window.target.windowID]
                json["origin"] = record?.origin.rawValue ?? "unknown"
                json["disposition"] = record?.disposition?.rawValue ?? NSNull()
                return json
            }]
        case "setDisposition":
            let display = try display(p)
            guard let windowID = p.int("windowId"),
                  let window = WindowCatalog.windows(on: display).first(where: { $0.target.windowID == windowID })
            else { throw ControlError.notFound("no window with that id on display \(display.id)") }
            guard let disposition = p.string("disposition").flatMap(Disposition.init(rawValue:)) else {
                throw ControlError.invalid("disposition must be keep or close")
            }
            display.setDisposition(disposition, of: window.target)
            let handedOver = display.records[window.target.windowID]?.origin == .handedOver
            return ["handedOver": handedOver]
        case "placeWindow":
            let display = try inputDisplay(p)
            guard let windowID = p.int("windowId"), let window = WindowCatalog.window(CGWindowID(windowID))
            else { throw ControlError.notFound("no window with that id") }
            display.claim(window.target, origin: .placed)
            do { try await Placement.place(window.target, onto: display.id) } catch {
                throw ControlError.failed("could not move the window: \(error)")
            }
            return [:]
        case "windowState":
            let display = try display(p, live: false)
            let window = try window(p, on: display)
            let snapshot = try windowState.snapshot(window.target)
            let shot = try await WindowState.capture(window.target, size: window.bounds.size)
            return ["window": window.json, "tree": snapshot.tree, "elements": snapshot.count,
                    "truncated": snapshot.truncated, "data": shot.data.base64EncodedString(),
                    "mimeType": shot.mimeType, "width": shot.width, "height": shot.height]
        case "elementAction":
            let display = try inputDisplay(p)
            let window = try window(p, on: display)
            guard let index = p.int("element") else { throw ControlError.invalid("element is required") }
            let element = try windowState.element(index, in: window.target.windowID)
            try await act(p.string("action") ?? "press", on: element, value: p.string("value"), display: display)
            return [:]
        case "launchApp":
            let display = try inputDisplay(p)
            guard let app = p.string("app") else { throw ControlError.invalid("app is required") }
            let window = try await AppLauncher.launch(app, onto: display)
            return window.map { ["window": $0.json] } ?? [:]
        case "requestHelp":
            let display = try display(p)
            guard let reason = p.string("reason"), !reason.isEmpty else {
                throw ControlError.invalid("reason is required")
            }
            displays.requestHelp(display, reason: reason)
            return [:]
        case "waitForControl":
            let display = try display(p, live: false)
            let seconds = min(max(p.double("timeoutSeconds") ?? 100, 1), 600)
            let outcome = await displays.waitForControl(display, timeout: .seconds(seconds))
            return ["outcome": outcome.rawValue]
        #if DEBUG
        // Test hook: stands in for the user's input in the Viewer.
        case "debugTakeover":
            let display = try display(p, live: false)
            if p.raw["active"] as? Bool == true { displays.beginTakeover(display) } else { displays.endTakeover(display) }
            return [:]
        // Test hook: opens the Viewer and reports its window number for screencapture -l.
        // Test hook: the user hands a window over, as the pull button does.
        case "debugHandover":
            let display = try display(p)
            guard let id = p.int("windowId"), let window = WindowCatalog.window(CGWindowID(id)) else {
                throw ControlError.notFound("no window with that id")
            }
            try await Handover.perform(window.target, from: nil, onto: display)
            let home = display.records[window.target.windowID]?.home
            return ["workspace": home?.workspace ?? NSNull(), "x": home.map { Int($0.frame.minX) } ?? NSNull()]
        // Test hook: shows a surface (viewer, peek, collapse, hide) and lists
        // our visible windows for screencapture -l.
        case "debugShow":
            let surface = p.string("surface") ?? "viewer"
            let id = p.int("displayId") ?? 0
            NotificationCenter.default.post(name: .debugShow, object: "\(surface):\(id)")
            try await Task.sleep(for: .milliseconds(900))
            return ["windows": NSApp.windows.filter(\.isVisible).map {
                ["number": $0.windowNumber, "level": $0.level.rawValue, "keyable": $0.canBecomeKey]
            }]
        #endif
        default:
            throw ControlError.invalid("unknown method \(method)")
        }
    }

    /// The control connection closed: its Agent Session is over.
    func connectionClosed(_ connection: Int) {
        for display in displays.displays where display.owner?.connectionID == connection {
            displays.end(display)
        }
    }

    // MARK: Methods

    private func createDisplay(connection: Int, _ p: Params) throws -> [String: Any] {
        guard let sessionKey = p.string("sessionKey"), !sessionKey.isEmpty else {
            throw ControlError.invalid("sessionKey is required")
        }
        if let existing = displays.display(forSession: sessionKey) {
            return describe(existing, reused: true)
        }
        let width = min(max(p.double("width") ?? DisplayManager.defaultSize.width, 640), 2560)
        let height = min(max(p.double("height") ?? DisplayManager.defaultSize.height, 480), 1600)
        let owner = DisplayOwner(
            sessionKey: sessionKey,
            client: p.string("client") ?? "Agent",
            title: p.string("title") ?? "Untitled",
            connectionID: connection
        )
        guard let display = displays.create(size: CGSize(width: width.rounded(), height: height.rounded()),
                                            owner: owner)
        else { throw ControlError.failed("macOS refused to create a virtual display") }
        return describe(display, reused: false)
    }

    private func screenshot(_ p: Params) async throws -> [String: Any] {
        let display = try display(p, live: false)
        let format = p.string("format") == "png" ? Screenshot.Format.png : .jpeg
        let shot = try await Screenshot.capture(display, format: format)
        return ["data": shot.data.base64EncodedString(), "mimeType": shot.mimeType,
                "width": shot.width, "height": shot.height]
    }

    /// One action on an accessibility element. AX actions reach background
    /// apps without focusing them; clicks go through the input forwarder at
    /// the element's center.
    private func act(_ action: String, on element: AXUIElement, value: String?, display: VirtualDisplay) async throws {
        let center = WindowState.frame(of: element).map { CGPoint(x: $0.midX, y: $0.midY) }
        func perform(_ name: String) -> Bool {
            if let center, display.bounds.contains(center) { showPointer(display, at: center, click: true) }
            return AXUIElementPerformAction(element, name as CFString) == .success
        }
        func click(_ button: CGMouseButton, count: Int) async throws {
            guard let frame = WindowState.frame(of: element), display.bounds.contains(CGPoint(x: frame.midX, y: frame.midY))
            else { throw ControlError.failed("the element has no visible frame on the display") }
            showPointer(display, at: CGPoint(x: frame.midX, y: frame.midY), click: true)
            await forwarders.forwarder(for: display.id).click(
                at: CGPoint(x: frame.midX, y: frame.midY), button: button, count: count, flags: [])
        }
        switch action {
        case "press":
            if !perform(kAXPressAction) { try await click(.left, count: 1) }
        case "click": try await click(.left, count: 1)
        case "double_click": try await click(.left, count: 2)
        case "right_click": try await click(.right, count: 1)
        case "show_menu":
            if !perform(kAXShowMenuAction) { try await click(.right, count: 1) }
        case "focus":
            guard AXUIElementSetAttributeValue(element, kAXFocusedAttribute as CFString, kCFBooleanTrue) == .success
            else { throw ControlError.failed("the element cannot take focus") }
        case "set_value":
            guard let value else { throw ControlError.invalid("value is required for set_value") }
            guard AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, value as CFString) == .success
            else { throw ControlError.failed("the element's value cannot be set; focus it and type instead") }
        case "scroll_into_view":
            guard perform("AXScrollToVisible") else { throw ControlError.failed("the element cannot scroll into view") }
        default:
            throw ControlError.invalid("unknown action \(action)")
        }
    }

    /// Show the Agent Pointer at a global point.
    private func showPointer(_ display: VirtualDisplay, at global: CGPoint, click: Bool) {
        let bounds = display.bounds
        displays.point(display, at: CGPoint(x: global.x - bounds.minX, y: global.y - bounds.minY), click: click)
    }

    // MARK: Parameters

    private func window(_ p: Params, on display: VirtualDisplay) throws -> WindowCatalog.Window {
        guard let id = p.int("windowId") else { throw ControlError.invalid("windowId is required") }
        guard let window = WindowCatalog.windows(on: display).first(where: { $0.target.windowID == id }) else {
            throw ControlError.notFound("no window \(id) on display \(display.id); see list_windows")
        }
        return window
    }

    private func display(_ p: Params, live: Bool = true) throws -> VirtualDisplay {
        guard let id = p.int("displayId") else { throw ControlError.invalid("displayId is required") }
        guard let display = displays.display(withID: CGDirectDisplayID(id)) else {
            throw ControlError.notFound("display \(id) does not exist; call create_display again")
        }
        if live, display.ended { throw ControlError.ended("display \(id) has ended") }
        return display
    }

    /// A display the Agent Session may send input to right now.
    private func inputDisplay(_ p: Params) throws -> VirtualDisplay {
        let display = try display(p)
        if display.takeover {
            throw ControlError.takeover(
                "the user is operating display \(display.id). Call wait_for_control to wait until they hand it back.")
        }
        return display
    }

    private func target(_ p: Params) throws -> (VirtualDisplay, CGPoint) {
        let display = try inputDisplay(p)
        return (display, try global(display, x: p.require("x"), y: p.require("y")))
    }

    private func global(_ display: VirtualDisplay, x: Double, y: Double) throws -> CGPoint {
        guard (0...display.size.width).contains(x), (0...display.size.height).contains(y) else {
            throw ControlError.invalid(
                "(\(Int(x)), \(Int(y))) is outside the \(Int(display.size.width))×\(Int(display.size.height)) display")
        }
        let bounds = display.bounds
        return CGPoint(x: bounds.minX + x, y: bounds.minY + y)
    }

    private func button(_ p: Params) throws -> CGMouseButton {
        switch p.string("button") ?? "left" {
        case "left": .left
        case "right": .right
        case "middle": .center
        case let other: throw ControlError.invalid("unknown button \(other)")
        }
    }

    private func modifiers(_ p: Params) throws -> CGEventFlags {
        guard let names = p.raw["modifiers"] as? [String], !names.isEmpty else { return [] }
        do { return try KeyChord.parse(names.joined(separator: "+") + "+a").flags } catch {
            throw ControlError.invalid(error.description)
        }
    }

    private func describe(_ display: VirtualDisplay, reused: Bool) -> [String: Any] {
        ["displayId": Int(display.id), "width": Int(display.size.width), "height": Int(display.size.height),
         "label": display.label, "reused": reused]
    }
}

/// Loosely typed request parameters from JSON.
struct Params {
    let raw: [String: Any]

    func string(_ key: String) -> String? { raw[key] as? String }
    func int(_ key: String) -> Int? { (raw[key] as? NSNumber)?.intValue }
    func double(_ key: String) -> Double? { (raw[key] as? NSNumber)?.doubleValue }

    func require(_ key: String) throws -> Double {
        guard let value = double(key) else { throw ControlError.invalid("\(key) is required") }
        return value
    }
}

enum Screenshot {
    enum Format { case png, jpeg }

    struct Shot {
        let data: Data
        let mimeType: String
        let width: Int
        let height: Int
    }

    /// Capture at one pixel per point, so screenshot pixels and input
    /// coordinates are the same numbers.
    @MainActor
    static func capture(_ display: VirtualDisplay, format: Format) async throws -> Shot {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let target = content.displays.first(where: { $0.displayID == display.id }) else {
            throw ControlError.failed("display is not capturable yet; retry in a moment")
        }
        let config = SCStreamConfiguration()
        config.width = Int(display.size.width)
        config.height = Int(display.size.height)
        config.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: SCContentFilter(display: target, excludingWindows: []), configuration: config)
        return try encode(image, format: format)
    }

    static func encode(_ image: CGImage, format: Format) throws -> Shot {
        let rep = NSBitmapImageRep(cgImage: image)
        let data: Data? = switch format {
        case .png: rep.representation(using: .png, properties: [:])
        case .jpeg: rep.representation(using: .jpeg, properties: [.compressionFactor: 0.8])
        }
        guard let data else { throw ControlError.failed("could not encode the screenshot") }
        return Shot(data: data, mimeType: format == .png ? "image/png" : "image/jpeg",
                    width: image.width, height: image.height)
    }
}

/// App windows as the agent sees them: ids plus display-relative bounds.
enum WindowCatalog {
    struct Window {
        let target: WindowTarget
        let app: String
        let title: String
        let bounds: CGRect

        var json: [String: Any] {
            ["windowId": Int(target.windowID), "app": app, "title": title,
             "x": Int(bounds.minX), "y": Int(bounds.minY), "width": Int(bounds.width), "height": Int(bounds.height)]
        }
    }

    /// Normal app windows (layer 0) of other apps, front to back.
    static func all() -> [(window: Window, global: CGRect)] {
        guard let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
                as? [[String: Any]]
        else { return [] }
        let me = getpid()
        return raw.compactMap { info in
            guard let pid = info[kCGWindowOwnerPID as String] as? pid_t, pid != me,
                  (info[kCGWindowLayer as String] as? Int) == 0,
                  let windowID = info[kCGWindowNumber as String] as? CGWindowID,
                  let boundsDict = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDict as CFDictionary),
                  bounds.width > 40, bounds.height > 40
            else { return nil }
            let window = Window(target: WindowTarget(pid: pid, windowID: windowID),
                                app: info[kCGWindowOwnerName as String] as? String ?? "",
                                title: info[kCGWindowName as String] as? String ?? "",
                                bounds: bounds)
            return (window, bounds)
        }
    }

    @MainActor
    static func windows(on display: VirtualDisplay) -> [Window] {
        let area = display.bounds
        return all().filter { area.contains(CGPoint(x: $0.global.midX, y: $0.global.midY)) }.map {
            Window(target: $0.window.target, app: $0.window.app, title: $0.window.title,
                   bounds: $0.global.offsetBy(dx: -area.minX, dy: -area.minY))
        }
    }

    static func window(_ id: CGWindowID) -> Window? {
        all().first { $0.window.target.windowID == id }?.window
    }
}

/// Opens an app in the background and places its new window on a display.
enum AppLauncher {
    @MainActor
    static func launch(_ app: String, onto display: VirtualDisplay) async throws -> WindowCatalog.Window? {
        guard let url = resolve(app) else { throw ControlError.notFound("no app named \(app)") }
        let before = Set(WindowCatalog.all().map(\.window.target.windowID))
        let wasRunning = NSWorkspace.shared.runningApplications.contains { $0.bundleURL == url }

        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = false
        let running = try await NSWorkspace.shared.openApplication(at: url, configuration: configuration)
        let pid = running.processIdentifier

        // A running app opens no window on launch; ask it for a new one.
        if wasRunning, let newWindow = try? KeyChord.parse("cmd+n") {
            for down in [true, false] {
                let event = CGEvent(keyboardEventSource: nil, virtualKey: newWindow.code, keyDown: down)
                event?.flags = newWindow.flags
                event?.postToPid(pid)
            }
        }

        for _ in 0..<40 {
            try await Task.sleep(for: .milliseconds(150))
            if let fresh = WindowCatalog.all().first(where: {
                $0.window.target.pid == pid && !before.contains($0.window.target.windowID)
            }) {
                return try await settle(fresh.window.target, onto: display)
            }
        }
        return nil
    }

    /// Place a new window and wait until it is really on the display. A window
    /// manager may not know about a window for a moment after it appears.
    @MainActor
    static func settle(_ target: WindowTarget, onto display: VirtualDisplay) async throws -> WindowCatalog.Window {
        var lastError: Error?
        display.claim(target, origin: .launched)
        for _ in 0..<12 {
            do { try await Placement.place(target, onto: display.id) } catch { lastError = error }
            try await Task.sleep(for: .milliseconds(200))
            if let placed = WindowCatalog.windows(on: display).first(where: { $0.target == target }) {
                return placed
            }
        }
        throw ControlError.failed("the window opened but could not be placed on the display"
            + (lastError.map { ": \($0)" } ?? ""))
    }

    private static func resolve(_ app: String) -> URL? {
        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: app) { return url }
        let name = app.hasSuffix(".app") ? app : "\(app).app"
        let folders = ["/Applications", "/System/Applications", "/System/Applications/Utilities",
                       "\(NSHomeDirectory())/Applications"]
        return folders.map { URL(fileURLWithPath: $0).appendingPathComponent(name) }
            .first { FileManager.default.fileExists(atPath: $0.path) }
    }
}

#if DEBUG
extension Notification.Name {
    static let debugShow = Notification.Name("SidescreenDebugShow")
}
#endif
