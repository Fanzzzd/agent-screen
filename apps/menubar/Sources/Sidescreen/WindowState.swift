import AppKit
import ScreenCaptureKit

/// A window as an agent that works per window sees it: a screenshot of just
/// that window and its accessibility tree, with numbered elements it can act
/// on. Coordinates are points from the window's top-left corner, the same
/// space as the screenshot.
@MainActor
final class WindowState {
    /// Elements from the latest snapshot of each window, by their number.
    private var elements: [CGWindowID: [AXUIElement]] = [:]

    struct Snapshot {
        let tree: String
        let count: Int
        let truncated: Bool
    }

    func snapshot(_ target: WindowTarget) throws -> Snapshot {
        guard let window = WindowMover.element(for: target) else {
            throw ControlError.notFound("the window has no accessibility element; is Accessibility granted?")
        }
        let app = AXUIElementCreateApplication(target.pid)
        AXUIElementSetMessagingTimeout(app, 1)
        // Chromium and Electron apps only publish their web content tree
        // once a client asks for it. Other apps ignore the attribute.
        AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)
        var walker = TreeWalker(origin: Self.frame(of: window)?.origin ?? .zero)
        walker.visit(window, depth: 0)
        elements[target.windowID] = walker.elements
        return Snapshot(tree: walker.lines.joined(separator: "\n"), count: walker.elements.count,
                        truncated: walker.truncated)
    }

    func element(_ index: Int, in windowID: CGWindowID) throws -> AXUIElement {
        guard let list = elements[windowID] else {
            throw ControlError.invalid("call get_window_state for this window first")
        }
        guard list.indices.contains(index) else {
            throw ControlError.notFound("no element \(index); call get_window_state again")
        }
        return list[index]
    }

    func forget(_ windowID: CGWindowID) { elements.removeValue(forKey: windowID) }

    /// CG global frame of an element.
    static func frame(of element: AXUIElement) -> CGRect? {
        var position = CGPoint.zero
        var size = CGSize.zero
        guard let rawPosition = attribute(element, kAXPositionAttribute),
              let rawSize = attribute(element, kAXSizeAttribute),
              AXValueGetValue(rawPosition as! AXValue, .cgPoint, &position),
              AXValueGetValue(rawSize as! AXValue, .cgSize, &size)
        else { return nil }
        return CGRect(origin: position, size: size)
    }

    static func attribute(_ element: AXUIElement, _ name: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
        return value
    }

    /// Screenshot of one window at one pixel per point, without what lies
    /// above or below it.
    static func capture(_ target: WindowTarget, size: CGSize) async throws -> Screenshot.Shot {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
        guard let window = content.windows.first(where: { $0.windowID == target.windowID }) else {
            throw ControlError.failed("the window is not capturable; is Screen Recording granted?")
        }
        let config = SCStreamConfiguration()
        config.width = max(Int(size.width), 1)
        config.height = max(Int(size.height), 1)
        config.showsCursor = false
        config.ignoreShadowsSingleWindow = true
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: SCContentFilter(desktopIndependentWindow: window), configuration: config)
        return try Screenshot.encode(image, format: .jpeg)
    }
}

/// Depth-first walk that numbers every element worth naming. Unlabelled
/// containers are skipped but their children are kept, which keeps the
/// tree short without losing anything an agent could act on.
private struct TreeWalker {
    let origin: CGPoint
    var lines: [String] = []
    var elements: [AXUIElement] = []
    var truncated = false

    static let maxElements = 400
    static let maxDepth = 40

    private static let names: [String] = [kAXRoleAttribute, kAXSubroleAttribute, kAXTitleAttribute, kAXDescriptionAttribute,
                                kAXValueAttribute, kAXEnabledAttribute, kAXFocusedAttribute, kAXPositionAttribute,
                                kAXSizeAttribute, kAXChildrenAttribute]

    private static let containers: Set<String> = ["AXGroup", "AXSplitGroup", "AXScrollArea", "AXLayoutArea",
                                                  "AXUnknown", "AXSplitter", "AXLayoutItem"]

    mutating func visit(_ element: AXUIElement, depth: Int) {
        guard depth <= Self.maxDepth else { return }
        guard elements.count < Self.maxElements else {
            truncated = true
            return
        }
        var raw: CFArray?
        guard AXUIElementCopyMultipleAttributeValues(element, Self.names as CFArray, [], &raw) == .success,
              let values = raw as? [Any], values.count == 10
        else { return }
        func string(_ index: Int) -> String? {
            guard let text = values[index] as? String else { return nil }
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : String(trimmed.prefix(80))
        }
        let role = string(0) ?? "AXUnknown"
        let label = string(2) ?? string(3)
        let value = (values[4] as? NSNumber).map { "\($0)" } ?? string(4)
        let children = values[9] as? [AXUIElement] ?? []

        var indent = depth
        if !Self.containers.contains(role) || label != nil {
            var line = "[\(elements.count)] \(role.dropFirst(2).lowercased())"
            if let subrole = string(1) { line += "(\(subrole.dropFirst(2).lowercased()))" }
            if let label { line += " \"\(label)\"" }
            if let value, value != label { line += " value=\"\(value)\"" }
            if (values[5] as? Bool) == false { line += " disabled" }
            if (values[6] as? Bool) == true { line += " focused" }
            if let frame = Self.frame(values[7], values[8]) {
                let x = Int(frame.minX - origin.x), y = Int(frame.minY - origin.y)
                line += " (\(x),\(y) \(Int(frame.width))×\(Int(frame.height)))"
            }
            lines.append(String(repeating: "  ", count: depth) + line)
            elements.append(element)
            indent = depth + 1
        }
        for child in children { visit(child, depth: indent) }
    }

    private static func frame(_ position: Any, _ size: Any) -> CGRect? {
        guard CFGetTypeID(position as CFTypeRef) == AXValueGetTypeID(),
              CFGetTypeID(size as CFTypeRef) == AXValueGetTypeID()
        else { return nil }
        var point = CGPoint.zero
        var extent = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
              AXValueGetValue(size as! AXValue, .cgSize, &extent), extent.width > 0, extent.height > 0
        else { return nil }
        return CGRect(origin: point, size: extent)
    }
}
