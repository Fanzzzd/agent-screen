import AppKit

/// Owns the menu bar icon and decides when the preview and the control
/// panel appear.
@MainActor
final class StatusItemController: NSObject, NSMenuDelegate {
    private let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
    private let displays: DisplayManager
    private let hub: StreamHub
    private let forwarders: ForwarderPool
    private let preview: PreviewPanelController
    private let control: ControlPanelController
    private let aerospace: AeroSpaceGuard

    private var hoveringIcon = false
    private var hoveringPreview = false
    private var hoverTimer: Timer?

    init(displays: DisplayManager, hub: StreamHub, forwarders: ForwarderPool,
         preview: PreviewPanelController, control: ControlPanelController, aerospace: AeroSpaceGuard) {
        self.aerospace = aerospace
        self.displays = displays
        self.hub = hub
        self.forwarders = forwarders
        self.preview = preview
        self.control = control
        super.init()

        if let button = item.button {
            button.image = NSImage(systemSymbolName: "rectangle.on.rectangle", accessibilityDescription: "Sidescreen")
            button.image?.isTemplate = true
            button.target = self
            button.action = #selector(clicked)
            button.sendAction(on: [.leftMouseUp, .rightMouseUp])
            button.addTrackingArea(NSTrackingArea(
                rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect], owner: self))
        }

        preview.onHoverChange = { [weak self] inside in
            self?.hoveringPreview = inside
            self?.hoverChanged()
        }
        preview.onSelect = { [weak self] display, rect in
            self?.preview.hide()
            self?.openPanel(display, from: rect)
        }
        preview.onCreate = { [weak self] in self?.createDisplay() }

        control.onCloseRequest = { [weak self] in self?.closePanel() }
        control.onPullWindow = { [weak self] in self?.pullFrontmostWindow() }
        control.onSendWindowBack = { [weak self] in self?.sendWindowBack() }

        displays.observe { [weak self] in
            guard let self, self.preview.isShown, let anchor = self.iconRect else { return }
            self.preview.show(below: anchor)
        }
        displays.observeState { [weak self] _ in self?.updateIcon() }
    }

    /// A raised hand while any Help Request is open.
    private func updateIcon() {
        guard let button = item.button else { return }
        let asking = displays.displays.contains { $0.helpRequest != nil }
        let symbol = asking ? "hand.raised.fill" : "rectangle.on.rectangle"
        button.image = NSImage(systemSymbolName: symbol, accessibilityDescription: "Sidescreen")
        button.image?.isTemplate = true
        button.contentTintColor = asking ? .systemOrange : nil
    }

    /// Open a display that asked for help and take it over, as the user
    /// clicked the notification to do exactly that.
    func openForHelp(_ displayID: CGDirectDisplayID) {
        guard let display = displays.display(withID: displayID) else { return }
        preview.hide(animated: false)
        openPanel(display, from: morphAnchor)
        displays.beginTakeover(display)
    }

    /// The icon's frame on screen, or nil when the icon is not visible.
    /// Menu bar managers such as Ice park hidden items far off-screen.
    private var iconRect: CGRect? {
        guard let button = item.button, let window = button.window else { return nil }
        let rect = window.convertToScreen(button.convert(button.bounds, to: nil))
        return NSScreen.screens.contains { $0.frame.intersects(rect) } ? rect : nil
    }

    /// Where the panel grows from and shrinks back to: the icon, or the
    /// top center of the main screen when the icon is hidden.
    private var morphAnchor: CGRect {
        if let iconRect { return iconRect }
        let screen = NSScreen.main ?? NSScreen.screens[0]
        return CGRect(x: screen.frame.midX - 12, y: screen.visibleFrame.maxY - 24, width: 24, height: 24)
    }

    // MARK: Hover

    @objc(mouseEntered:) func mouseEntered(_ event: NSEvent) {
        hoveringIcon = true
        hoverChanged()
    }

    @objc(mouseExited:) func mouseExited(_ event: NSEvent) {
        hoveringIcon = false
        hoverChanged()
    }

    /// Show after a short intent delay; hide after a grace period so the
    /// pointer can travel from the icon down into the preview.
    private func hoverChanged() {
        hoverTimer?.invalidate()
        let wantsPreview = hoveringIcon || hoveringPreview
        guard wantsPreview != preview.isShown else { return }
        let delay = wantsPreview ? 0.12 : 0.28
        hoverTimer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                if wantsPreview, let anchor = self.iconRect {
                    self.preview.activeDisplayID = self.control.isVisible ? self.control.display?.id : nil
                    self.preview.show(below: anchor)
                } else if !wantsPreview {
                    self.preview.hide()
                }
            }
        }
    }

    // MARK: Clicks

    @objc private func clicked() {
        hoverTimer?.invalidate()
        if NSApp.currentEvent?.type == .rightMouseUp {
            preview.hide(animated: false)
            showMenu()
            return
        }
        preview.hide()
        if control.isVisible {
            closePanel()
        } else if let display = control.display ?? displays.displays.first ?? displays.create() {
            openPanel(display, from: morphAnchor)
        }
    }

    private func openPanel(_ display: VirtualDisplay, from rect: CGRect) {
        control.open(display, from: rect)
        preview.activeDisplayID = display.id
    }

    private func closePanel() {
        control.close(to: morphAnchor)
        preview.activeDisplayID = nil
    }

    private func createDisplay() {
        guard let display = displays.create() else {
            NSSound.beep()
            return
        }
        preview.hide()
        // Give WindowServer a beat to finish bringing the display up.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            MainActor.assumeIsolated { self.openPanel(display, from: self.morphAnchor) }
        }
    }

    private func remove(_ display: VirtualDisplay) {
        if control.display === display, control.isVisible { closePanel() }
        hub.drop(display.id)
        displays.remove(display)
    }

    private func pullFrontmostWindow() {
        guard let display = control.display ?? displays.displays.first else { return }
        Task {
            do {
                let window = try WindowMover.frontmostWindow()
                display.claim(window.windowID)
                try await Placement.place(window, onto: display.id)
            } catch {
                NSSound.beep()
            }
        }
    }

    private func sendWindowBack() {
        guard let display = control.display, let target = forwarders.forwarder(for: display.id).focusedTarget
        else { return NSSound.beep() }
        Task {
            do {
                try await Placement.place(target, onto: CGMainDisplayID())
            } catch {
                NSSound.beep()
            }
        }
    }

    func demo(_ what: String) {
        let anchor = morphAnchor
        switch what {
        case "preview": preview.show(below: anchor)
        default:
            if let display = displays.displays.first { openPanel(display, from: anchor) }
        }
    }

    // MARK: Menu

    private func showMenu() {
        let menu = NSMenu()
        menu.delegate = self
        menu.addItem(withTitle: "新建虚拟屏", action: #selector(menuCreate), keyEquivalent: "n").target = self
        if !displays.displays.isEmpty {
            menu.addItem(withTitle: "把前台窗口拉进虚拟屏", action: #selector(menuPull), keyEquivalent: "").target = self
            let removeMenu = NSMenu()
            for display in displays.displays {
                let entry = removeMenu.addItem(withTitle: display.label, action: #selector(menuRemove(_:)), keyEquivalent: "")
                entry.target = self
                entry.representedObject = NSNumber(value: display.id)
            }
            menu.addItem(withTitle: "移除虚拟屏", action: nil, keyEquivalent: "").submenu = removeMenu
        }
        menu.addItem(.separator())
        if !aerospace.conflicts.isEmpty {
            for conflict in aerospace.conflicts {
                let entry = menu.addItem(
                    withTitle: "AeroSpace 把工作区 \(conflict.workspace) 固定到了 \(conflict.display)",
                    action: nil, keyEquivalent: "")
                entry.isEnabled = false
            }
            let fix = menu.addItem(withTitle: "打开 AeroSpace 配置…", action: #selector(menuAeroSpaceConfig), keyEquivalent: "")
            fix.target = self
            fix.toolTip = "workspace-to-monitor-force-assignment 里的显示器编号会被虚拟屏占用。改用显示器名称，例如 et2 = 'DELL'。"
            menu.addItem(.separator())
        }
        if !Permissions.screenRecording || !Permissions.accessibility {
            let missing = [
                Permissions.screenRecording ? nil : "屏幕录制",
                Permissions.accessibility ? nil : "辅助功能",
            ].compactMap { $0 }.joined(separator: "、")
            menu.addItem(withTitle: "授予权限：\(missing)", action: #selector(menuPermissions), keyEquivalent: "").target = self
            menu.addItem(.separator())
        }
        menu.addItem(withTitle: "退出 Sidescreen", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        item.menu = menu
        item.button?.performClick(nil)
    }

    @objc private func menuAeroSpaceConfig() {
        let home = FileManager.default.homeDirectoryForCurrentUser
        let candidates = [".config/aerospace/aerospace.toml", ".aerospace.toml"].map { home.appendingPathComponent($0) }
        if let config = candidates.first(where: { FileManager.default.fileExists(atPath: $0.path) }) {
            NSWorkspace.shared.open(config)
        }
    }

    func menuDidClose(_ menu: NSMenu) {
        item.menu = nil
    }

    @objc private func menuCreate() { createDisplay() }
    @objc private func menuPull() { pullFrontmostWindow() }
    @objc private func menuPermissions() { Permissions.requestMissing() }

    @objc private func menuRemove(_ sender: NSMenuItem) {
        guard let id = (sender.representedObject as? NSNumber)?.uint32Value,
              let display = displays.display(withID: id)
        else { return }
        remove(display)
    }
}
