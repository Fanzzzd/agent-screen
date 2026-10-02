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
    /// A new display reconfigures the screens, and AppKit then reports
    /// fresh hover events for the icon. Ignore them for a moment.
    private var hoverQuietUntil = Date.distantPast
    /// A click opened Peek: it stays until the next click on the icon, a
    /// click elsewhere, or a tile is chosen, whatever the pointer does.
    private var peekLatched = false {
        didSet { peekLatched ? watchOutsideClicks() : stopWatchingOutsideClicks() }
    }
    private var outsideClickMonitors: [Any] = []

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
            self?.peekLatched = false
            self?.preview.hide()
            self?.openPanel(display, from: rect)
        }
        preview.onCreate = { [weak self] in self?.createDisplay() }
        preview.onDismiss = { [weak self] display in self?.dismiss(display) }

        control.onCloseRequest = { [weak self] in self?.closePanel() }
        control.onPullWindow = { [weak self] in self?.pullFrontmostWindow() }
        control.onSendWindowBack = { [weak self] in self?.sendWindowBack() }
        control.onDismiss = { [weak self] display in self?.dismiss(display) }

        displays.observe { [weak self] in
            guard let self, self.preview.isShown else { return }
            self.preview.show(below: self.iconRect ?? self.morphAnchor)
        }
        displays.observeState { [weak self] _ in self?.updateIcon() }

        dragWatcher.dropRect = { [weak control] in control?.dropRect }
        dragWatcher.onHover = { [weak control] inside in control?.showDropTarget(inside) }
        dragWatcher.onDrop = { [weak self] target, frame in
            guard let self, let display = self.control.display else { return }
            Task {
                // Let the window manager finish its own drop handling first.
                try? await Task.sleep(for: .milliseconds(150))
                do {
                    try await Handover.perform(target, from: frame, onto: display)
                    self.control.flashAccepted()
                } catch {
                    NSLog("Sidescreen: handover failed: \(error)")
                    NSSound.beep()
                }
            }
        }
        dragWatcher.start()
        #if DEBUG
        NotificationCenter.default.addObserver(forName: .debugShow, object: nil, queue: .main) { [weak self] note in
            guard let parts = (note.object as? String)?.split(separator: ":"), parts.count == 2 else { return }
            let surface = String(parts[0])
            let id = CGDirectDisplayID(parts[1]) ?? 0
            MainActor.assumeIsolated {
                guard let self else { return }
                switch surface {
                case "peek": self.preview.show(below: self.iconRect ?? self.morphAnchor)
                case "click": self.clicked()
                case "hide": self.preview.hide()
                case "collapse": self.closePanel()
                case "create": self.createDisplay()
                case "hint": self.control.debugHint(Int(id))
                case "answer": self.control.debugAnswerDismiss(id != 0)
                case "dismiss": if let display = self.displays.display(withID: id) { self.dismiss(display) }
                default:
                    if let display = self.displays.display(withID: id) { self.openPanel(display, from: self.morphAnchor) }
                }
            }
        }
        #endif
    }

    private let dragWatcher = DragHandoverWatcher()

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
        peekLatched = false
        preview.hide(animated: false)
        openPanel(display, from: morphAnchor)
        displays.beginTakeover(display)
    }

    /// The user's own screens. Our panels never go on an Agent Display,
    /// where the Viewer would mirror itself.
    private var userScreens: [NSScreen] {
        NSScreen.screens.filter { screen in !displays.displays.contains { $0.id == screen.displayID } }
    }

    /// The icon's frame on screen, or nil when the icon is not visible.
    /// Menu bar managers such as Ice park hidden items far off-screen, and
    /// while an app on an Agent Display is frontmost, macOS draws the icon
    /// in that display's menu bar.
    private var iconRect: CGRect? {
        guard let button = item.button, let window = button.window else { return nil }
        let rect = window.convertToScreen(button.convert(button.bounds, to: nil))
        return userScreens.contains { $0.frame.intersects(rect) } ? rect : nil
    }

    /// Where the panel grows from and shrinks back to: the icon, or the
    /// top center of the user's screen under the pointer.
    private var morphAnchor: CGRect {
        if let iconRect { return iconRect }
        let screens = userScreens
        let screen = screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? screens.first ?? NSScreen.screens[0]
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
        guard Date() >= hoverQuietUntil, !peekLatched else { return }
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
            peekLatched = false
            preview.hide(animated: false)
            showMenu()
            return
        }
        // The icon toggles Peek. A hover may already have shown it; the
        // click then keeps it up.
        if peekLatched {
            peekLatched = false
            preview.hide()
        } else {
            peekLatched = true
            preview.activeDisplayID = control.isVisible ? control.display?.id : nil
            preview.show(below: iconRect ?? morphAnchor)
        }
    }

    private func watchOutsideClicks() {
        guard outsideClickMonitors.isEmpty else { return }
        let kinds: NSEvent.EventTypeMask = [.leftMouseDown, .rightMouseDown]
        let check: @Sendable () -> Void = { [weak self] in
            DispatchQueue.main.async {
                MainActor.assumeIsolated { self?.closeLatchedPeekIfOutside() }
            }
        }
        if let global = NSEvent.addGlobalMonitorForEvents(matching: kinds, handler: { _ in check() }) {
            outsideClickMonitors.append(global)
        }
        if let local = NSEvent.addLocalMonitorForEvents(matching: kinds, handler: { event in
            check()
            return event
        }) {
            outsideClickMonitors.append(local)
        }
    }

    private func stopWatchingOutsideClicks() {
        outsideClickMonitors.forEach(NSEvent.removeMonitor)
        outsideClickMonitors.removeAll()
    }

    /// A click on the icon is handled by `clicked`; one on Peek by its tiles.
    private func closeLatchedPeekIfOutside() {
        let point = NSEvent.mouseLocation
        guard peekLatched, !preview.cardFrame.contains(point), iconRect?.contains(point) != true else { return }
        peekLatched = false
        hoverChanged()
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
        settleHoverAfterReconfiguration()
        guard let display = displays.create() else {
            NSSound.beep()
            return
        }
        // Peek stays up and grows a tile; the Viewer opens on the new
        // display, or switches to it when already open.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            MainActor.assumeIsolated { self.openPanel(display, from: self.iconRect ?? self.morphAnchor) }
        }
    }

    /// Once the screens settle, follow the pointer again.
    private func settleHoverAfterReconfiguration() {
        hoverTimer?.invalidate()
        hoverQuietUntil = Date().addingTimeInterval(1.2)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.25) {
            MainActor.assumeIsolated { self.hoverChanged() }
        }
    }

    /// Dismiss a display after the user confirms what happens to its windows.
    /// The question appears on the Viewer, so Peek and the Viewer stay up.
    func dismiss(_ display: VirtualDisplay) {
        let plan = Dismissal.plan(for: display)
        guard Dismissal.needsConfirmation(plan, for: display) else { return dismissNow(display, items: plan) }
        let live = Dismissal.isLive(display)
        openPanel(display, from: iconRect ?? morphAnchor)
        control.askDismiss(plan, live: live) { [weak self] items in
            guard let self, let items else { return }
            self.dismissNow(display, items: items)
        }
    }

    private func dismissNow(_ display: VirtualDisplay, items: [Dismissal.Item]) {
        displays.end(display)
        Task {
            await Dismissal.execute(items)
            remove(display)
        }
    }

    /// The Viewer moves on to another display, and collapses only when
    /// this was the last one.
    private func remove(_ display: VirtualDisplay) {
        if control.display === display, control.isVisible {
            if let next = displays.displays.first(where: { $0 !== display }) {
                openPanel(next, from: iconRect ?? morphAnchor)
            } else {
                closePanel()
            }
        }
        settleHoverAfterReconfiguration()
        control.release(display)
        hub.drop(display.id)
        forwarders.drop(display.id)
        displays.remove(display)
    }

    private func pullFrontmostWindow() {
        guard let display = control.display ?? displays.displays.first else { return }
        Task {
            do {
                try await Handover.perform(try WindowMover.frontmostWindow(), from: nil, onto: display)
            } catch {
                NSSound.beep()
            }
        }
    }

    private func sendWindowBack() {
        guard let display = control.display, let target = forwarders.forwarder(for: display.id).focusedTarget
        else { return NSSound.beep() }
        let record = display.records[target.windowID]
        display.forget(target.windowID)
        Task {
            await Dismissal.giveBack(Dismissal.Item(target: target, app: "", title: "", fate: .giveBack,
                                                    locked: true, home: record?.home))
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
            menu.addItem(withTitle: "关闭显示器", action: nil, keyEquivalent: "").submenu = removeMenu
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
        dismiss(display)
    }
}
