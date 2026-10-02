import AppKit

/// The menus of the app in front on an Agent Display, opened from the
/// Viewer's menu bar strip.
///
/// That strip mirrors the menu bar of the user's own front app, not the
/// display's, and posted clicks never open a menu bar anyway. So a click
/// there shows the display's front app menus as one native menu, read
/// through accessibility, and the chosen item is pressed.
@MainActor
enum MenuBarProxy {
    /// Height of the menu bar on `display`, in points.
    static func height(of display: VirtualDisplay) -> CGFloat {
        guard let screen = NSScreen.screens.first(where: { $0.displayID == display.id }) else { return 24 }
        return max(screen.frame.maxY - screen.visibleFrame.maxY, 24)
    }

    /// The front app's menus on `display`, one submenu per menu title,
    /// or nil when no app there has any.
    static func menu(for display: VirtualDisplay) -> NSMenu? {
        guard let pid = WindowCatalog.windows(on: display).first?.target.pid else { return nil }
        let app = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(app, 1)
        guard let bar: AXUIElement = attribute(app, kAXMenuBarAttribute) else { return nil }
        let name = NSRunningApplication(processIdentifier: pid)?.localizedName ?? ""
        let menu = NSMenu(title: name)
        menu.autoenablesItems = false
        let header = menu.addItem(withTitle: name, action: nil, keyEquivalent: "")
        header.isEnabled = false
        // The first title is the Apple menu, which belongs to the system.
        for item in children(bar).dropFirst() {
            guard let submenu = children(item).first else { continue }
            let title = title(of: item)
            let entry = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            let content = NSMenu(title: title)
            fill(content, from: submenu)
            entry.submenu = content
            menu.addItem(entry)
        }
        return menu.items.count > 1 ? menu : nil
    }

    private static func fill(_ menu: NSMenu, from axMenu: AXUIElement) {
        menu.autoenablesItems = false
        for element in children(axMenu) {
            let name = title(of: element)
            if name.isEmpty {
                menu.addItem(.separator())
                continue
            }
            let item = ProxyItem(element: element, title: name)
            item.isEnabled = (attribute(element, kAXEnabledAttribute) as Bool?) ?? true
            if let key: String = attribute(element, kAXMenuItemCmdCharAttribute), !key.isEmpty {
                item.keyEquivalent = key.lowercased()
                let mods: Int = attribute(element, kAXMenuItemCmdModifiersAttribute) ?? 0
                item.keyEquivalentModifierMask = modifiers(mods)
            }
            if (attribute(element, kAXMenuItemMarkCharAttribute) as String?)?.isEmpty == false { item.state = .on }
            if let sub = children(element).first {
                let submenu = NSMenu(title: name)
                fill(submenu, from: sub)
                item.submenu = submenu
            }
            menu.addItem(item)
        }
    }

    /// AXMenuItemCmdModifiers: bit 3 means no Command; bits 0-2 add
    /// Shift, Option and Control.
    private static func modifiers(_ raw: Int) -> NSEvent.ModifierFlags {
        var flags: NSEvent.ModifierFlags = raw & 8 == 0 ? [.command] : []
        if raw & 1 != 0 { flags.insert(.shift) }
        if raw & 2 != 0 { flags.insert(.option) }
        if raw & 4 != 0 { flags.insert(.control) }
        return flags
    }

    private static func title(of element: AXUIElement) -> String {
        (attribute(element, kAXTitleAttribute) as String?) ?? ""
    }

    private static func children(_ element: AXUIElement) -> [AXUIElement] {
        (attribute(element, kAXChildrenAttribute) as [AXUIElement]?) ?? []
    }

    private static func attribute<T>(_ element: AXUIElement, _ name: String) -> T? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return nil }
        return value as? T
    }
}

/// A menu item that presses its accessibility twin.
private final class ProxyItem: NSMenuItem {
    private let element: AXUIElement

    init(element: AXUIElement, title: String) {
        self.element = element
        super.init(title: title, action: #selector(press), keyEquivalent: "")
        target = self
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError() }

    @objc private func press() {
        AXUIElementPerformAction(element, kAXPressAction as CFString)
    }
}
