import AppKit

/// The large panel that mirrors one virtual display and forwards input to it.
/// It grows out of whatever the user clicked (the status icon or a preview
/// tile) and collapses back into the status icon.
@MainActor
final class ControlPanelController: NSObject, NSWindowDelegate {
    enum State { case closed, opening, open, closing }

    private(set) var state = State.closed {
        didSet {
            #if DEBUG
            if state != oldValue { NSLog("Sidescreen viewer: \(oldValue) -> \(state)") }
            #endif
        }
    }
    private(set) var display: VirtualDisplay?

    var isVisible: Bool { state == .opening || state == .open }

    var onCloseRequest: (() -> Void)?
    var onDismiss: ((VirtualDisplay) -> Void)?

    private let hub: StreamHub
    private let forwarders: ForwarderPool
    private let displays: DisplayManager
    private let panel = FloatingPanel(level: .floating, keyable: true)
    private let card: NSView
    private let header = HeaderView()
    private let screen = InteractiveDisplayView()
    private let proxy = MorphProxy(cornerRadius: 22, screenCornerRadius: 13)

    /// Room around the card for its shadow.
    private let margin: CGFloat = 56
    private let cornerRadius: CGFloat = 22
    private let headerHeight: CGFloat = 52
    private let inset: CGFloat = 8

    /// Where the card rests, in screen coordinates. Survives close/open so
    /// the panel comes back where the user dragged it.
    private var restingRect: CGRect?
    /// Bumped on every transition so stale completion blocks do nothing.
    private var generation = 0
    /// The display is being dismissed: let go of it once the panel is closed,
    /// so macOS can free the virtual display.
    private var releaseOnClose = false
    private var sheet: DismissSheet?

    init(hub: StreamHub, forwarders: ForwarderPool, displays: DisplayManager) {
        self.hub = hub
        self.forwarders = forwarders
        self.displays = displays
        let (card, content) = Glass.card(cornerRadius: cornerRadius)
        self.card = card
        super.init()

        screen.cornerRadius = 13
        header.onClose = { [weak self] in self?.onCloseRequest?() }
        header.onPrimary = { [weak self] in
            guard let self, let display = self.display else { return }
            if display.ended {
                self.onDismiss?(display)
            } else if display.takeover {
                self.displays.endTakeover(display)
            } else {
                self.displays.beginTakeover(display)
                self.panel.makeKey()
                self.panel.makeFirstResponder(self.screen)
            }
        }
        header.onWindows = { [weak self] anchor in
            guard let self, let display = self.display else { return }
            let menu = self.windowMenu(for: display)
            menu.popUp(positioning: nil, at: NSPoint(x: 0, y: anchor.bounds.height + 4), in: anchor)
        }
        // The user's first input takes the display over from its agent.
        screen.onUserInput = { [weak self] in
            guard let self, let display = self.display else { return }
            self.displays.beginTakeover(display)
        }
        displays.observeState { [weak self] display in
            guard let self, display === self.display else { return }
            self.refreshState()
        }
        screen.onMenuBarClick = { [weak self] point, local in
            guard let self, let display = self.display,
                  point.y - display.bounds.minY < MenuBarProxy.height(of: display)
            else { return false }
            MenuBarProxy.menu(for: display)?.popUp(positioning: nil, at: local, in: self.screen)
            return true
        }

        content.addSubview(header)
        content.addSubview(screen)
        // Frames are set by layOutCard. Autoresizing would add the glass
        // content view's own resize on top and double every size change.
        header.autoresizingMask = []
        screen.autoresizingMask = []

        panel.contentView?.addSubview(card)
        panel.contentView?.addSubview(proxy.view)
        panel.delegate = self
        panel.hitRect = { [weak self] in
            guard let self, self.state == .open else { return nil }
            return self.panel.convertToScreen(self.card.frame)
        }
    }

    // MARK: Transitions

    func open(_ display: VirtualDisplay, from source: CGRect) {
        releaseOnClose = false
        if isVisible {
            if self.display !== display { show(display, crossfade: true) }
            return
        }
        // Reopening mid-close starts from wherever the proxy is right now.
        let interrupted = state == .closing ? proxy.view.layer?.presentation() : nil
        show(display, crossfade: false)

        let hostScreen = NSScreen.screens.first { $0.frame.intersects(source) } ?? NSScreen.main
        let size = cardSize(for: display, on: hostScreen)
        let target: CGRect
        if let restingRect {
            target = CGRect(x: restingRect.midX - size.width / 2, y: restingRect.maxY - size.height,
                            width: size.width, height: size.height)
        } else {
            let visible = hostScreen?.visibleFrame ?? .zero
            target = CGRect(x: visible.midX - size.width / 2, y: visible.midY - size.height / 2,
                            width: size.width, height: size.height).integral
        }
        restingRect = target
        layOutCard(at: target, stageIncluding: source)

        generation += 1
        let token = generation
        state = .opening
        // Watching only: the keyboard stays with the user's app until they
        // click into the display.
        panel.orderFrontRegardless()

        let layer = beginMorph()
        let collapsed = Motion.collapse(proxy.view.frame, into: panel.convertFromScreen(source))
        let fromTransform = interrupted?.transform ?? collapsed
        let fromOpacity = interrupted?.opacity ?? 0

        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.generation == token else { return }
            self.state = .open
            self.layOutCard(at: target, stageIncluding: nil)
            self.endMorph()
        }
        layer.add(Motion.openSpring("transform", from: NSValue(caTransform3D: fromTransform),
                                    to: NSValue(caTransform3D: CATransform3DIdentity)), forKey: "morph")
        layer.add(Motion.fade(from: fromOpacity, to: 1, duration: 0.16), forKey: "fade")
        CATransaction.commit()
    }

    func close(to destination: CGRect) {
        guard isVisible else { return }
        sheet?.onAnswer?(nil)
        // Collapsing keeps a Takeover: only 让 … 继续 hands the display back.
        // Interrupting an open continues from the proxy's current pose.
        let current = state == .opening ? proxy.view.layer?.presentation() : nil
        let restingOnScreen = panel.convertToScreen(card.frame)
        if state == .open { restingRect = restingOnScreen }
        layOutCard(at: restingOnScreen, stageIncluding: destination)

        generation += 1
        let token = generation
        state = .closing

        let layer = beginMorph()
        let collapsed = Motion.collapse(proxy.view.frame, into: panel.convertFromScreen(destination))
        let fromTransform = current?.transform ?? CATransform3DIdentity

        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.generation == token else { return }
            self.state = .closed
            self.panel.orderOut(nil)
            self.proxy.view.isHidden = true
            layer.removeAllAnimations()
            self.proxy.screenLayer.contents = nil
            if let display = self.display { self.hub.detach(self.screen.screenLayer, from: display.id) }
            if self.releaseOnClose { self.finishRelease() }
        }
        let morph = Motion.spring("transform", from: NSValue(caTransform3D: fromTransform),
                                  to: NSValue(caTransform3D: collapsed), duration: 0.34, bounce: 0)
        morph.isRemovedOnCompletion = false
        layer.add(morph, forKey: "morph")

        let fade = Motion.fade(from: current?.opacity ?? 1, to: 0, duration: 0.16,
                               delay: layer.convertTime(CACurrentMediaTime(), from: nil) + 0.14)
        fade.timingFunction = CAMediaTimingFunction(name: .easeIn)
        fade.isRemovedOnCompletion = false
        layer.add(fade, forKey: "fade")
        CATransaction.commit()
    }

    /// Swap the real card for the proxy at the card's current geometry.
    private func beginMorph() -> CALayer {
        // Hidden views draw nothing into a snapshot.
        card.isHidden = false
        proxy.mirror(card: card, header: header, screen: screen)
        proxy.screenLayer.contents = display.flatMap { hub.stillFrame(of: $0.id) }
        let layer = proxy.view.layer!
        layer.removeAllAnimations()
        layer.transform = CATransform3DIdentity
        layer.opacity = 1
        proxy.view.isHidden = false
        card.isHidden = true
        return layer
    }

    /// Reveal the real card under the proxy, then fade the proxy away.
    private func endMorph() {
        guard let layer = proxy.view.layer else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        proxy.view.frame = card.frame
        CATransaction.commit()
        card.isHidden = false
        panel.makeFirstResponder(screen)
        panel.updateMouseThrough()

        let token = generation
        CATransaction.begin()
        CATransaction.setCompletionBlock { [weak self] in
            guard let self, self.generation == token else { return }
            self.proxy.view.isHidden = true
            layer.removeAllAnimations()
            self.proxy.screenLayer.contents = nil
        }
        let fade = Motion.fade(from: 1, to: 0, duration: 0.18)
        fade.isRemovedOnCompletion = false
        layer.add(fade, forKey: "fade")
        CATransaction.commit()
    }

    /// Point the panel at a display: stream, title, input routing.
    private func show(_ display: VirtualDisplay, crossfade: Bool) {
        if let old = self.display, old !== display {
            sheet?.onAnswer?(nil)
            hub.detach(screen.screenLayer, from: old.id)
        }
        self.display = display
        screen.displayID = display.id
        screen.forwarder = forwarders.forwarder(for: display.id)
        screen.displaySize = display.size
        header.title = display.label
        refreshState()
        if crossfade {
            let transition = CATransition()
            transition.type = .fade
            transition.duration = 0.2
            screen.screenLayer.add(transition, forKey: "swap")
        }
        hub.attach(screen.screenLayer, to: display.id, fps: 60)
    }

    #if DEBUG
    /// Show the hover hint of the n-th header button, for screenshot checks.
    func debugHint(_ index: Int) {
        func buttons(in view: NSView) -> [ActionButton] {
            view.subviews.flatMap { ($0 as? ActionButton).map { [$0] } ?? buttons(in: $0) }
        }
        let all = buttons(in: header).filter { !$0.isHidden }
        guard all.indices.contains(index) else { return }
        all[index].showHint()
    }
    #endif

    #if DEBUG
    func debugAnswerDismiss(_ confirmed: Bool) { sheet?.answer(confirmed) }

    /// Make the panel key and report whether AppKit agrees.
    func debugFocus() {
        panel.makeKey()
        NSLog("Sidescreen focus: keyWindow is panel = \(NSApp.keyWindow === panel)")
    }

    #endif

    /// Let go of a display that is going away.
    func release(_ display: VirtualDisplay) {
        guard self.display === display else { return }
        releaseOnClose = true
        if state == .closed { finishRelease() }
    }

    private func finishRelease() {
        releaseOnClose = false
        display = nil
        screen.displayID = nil
        screen.forwarder = nil
        screen.showPointer(nil)
        screen.screenLayer.contents = nil
    }

    // MARK: Dismissal

    /// Ask on the open panel what happens to the display's windows.
    /// `answer` gets the adjusted plan, or nil when the user cancels or the
    /// panel moves on.
    func askDismiss(_ items: [Dismissal.Item], live: Bool, answer: @escaping ([Dismissal.Item]?) -> Void) {
        guard let display, sheet == nil else { return }
        let sheet = DismissSheet(items: items, display: display, live: live)
        sheet.onAnswer = { [weak self] result in
            guard let self, let sheet = self.sheet else { return }
            self.sheet = nil
            NSAnimationContext.runAnimationGroup({ context in
                context.duration = 0.12
                sheet.animator().alphaValue = 0
            }, completionHandler: { sheet.removeFromSuperview() })
            if self.panel.firstResponder === sheet { self.panel.makeFirstResponder(self.screen) }
            answer(result)
        }
        self.sheet = sheet
        sheet.frame = screen.frame
        sheet.alphaValue = 0
        screen.superview?.addSubview(sheet, positioned: .above, relativeTo: screen)
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.16
            sheet.animator().alphaValue = 1
        }
        panel.makeKey()
        panel.makeFirstResponder(sheet)
    }

    // MARK: Handover

    /// Where a dragged window can be dropped: the open card of a display
    /// that is not over.
    var dropRect: CGRect? {
        guard state == .open, let display, !display.ended else { return nil }
        return panel.convertToScreen(card.frame)
    }

    private var dropHighlighted = false

    /// Outline the screen while a dragged window hovers over the card.
    func showDropTarget(_ on: Bool) {
        dropHighlighted = on
        if on {
            screen.setOutline(.controlAccentColor, width: 4)
            let agent = display?.agentName ?? "agent"
            header.status = NSAttributedString(string: "松手把窗口交给 \(agent)", attributes: [
                .font: NSFont.systemFont(ofSize: 12), .foregroundColor: NSColor.controlAccentColor,
            ])
        } else {
            refreshState()
        }
    }

    /// A brief accent ring once a Handover landed.
    func flashAccepted() {
        screen.setOutline(.controlAccentColor, width: 4)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.45) {
            MainActor.assumeIsolated { if !self.dropHighlighted { self.refreshState() } }
        }
    }

    /// The header follows the display's state.
    private func refreshState() {
        guard let display else { return }
        let count = WindowCatalog.windows(on: display).count
        let state = DisplayStatus(display, windowCount: count)
        header.status = state.attributed(state.long, size: 12)
        header.windowCount = count
        header.primary = switch state.kind {
        case .manual: .none
        case .agent: .stepIn(urgent: false)
        case .help: .stepIn(urgent: true)
        case .takeover: .handBack(agent: display.agentName)
        case .review, .disconnected: .review
        }
        if !dropHighlighted { screen.setOutline(nil) }
        // The user's own hand needs no marker while they operate.
        screen.showPointer(display.takeover || display.ended ? nil : display.pointer)
    }

    /// 窗口 menu: hand one of the user's windows over, take one back, or
    /// close the display.
    private func windowMenu(for display: VirtualDisplay) -> NSMenu {
        let menu = NSMenu()
        menu.autoenablesItems = false
        let agent = display.agentName
        let handTitle = display.owner == nil ? "放到这块显示器上" : "交给 \(agent)"
        let hand = NSMenuItem(title: handTitle, action: nil, keyEquivalent: "")
        let handMenu = NSMenu()
        let agentScreens = displays.displays.map(\.bounds)
        for entry in WindowCatalog.all() where !agentScreens.contains(where: { $0.intersects(entry.global) }) {
            let window = entry.window
            handMenu.addItem(ActionMenuItem(title: Self.name(window), image: Self.icon(window.target.pid)) { [weak self] in
                Task {
                    do {
                        try await Handover.perform(window.target, from: nil, onto: display)
                        self?.flashAccepted()
                    } catch {
                        NSSound.beep()
                    }
                }
            })
        }
        if handMenu.items.isEmpty { handMenu.addItem(withTitle: "你的桌面上没有窗口", action: nil, keyEquivalent: "").isEnabled = false }
        hand.submenu = handMenu
        hand.isEnabled = !display.ended
        menu.addItem(hand)

        let back = NSMenuItem(title: "取回窗口", action: nil, keyEquivalent: "")
        let backMenu = NSMenu()
        for window in WindowCatalog.windows(on: display) {
            backMenu.addItem(ActionMenuItem(title: Self.name(window), image: Self.icon(window.target.pid)) { [weak self] in
                self?.takeBack(window.target, from: display)
            })
        }
        if backMenu.items.isEmpty { backMenu.addItem(withTitle: "这块显示器上没有窗口", action: nil, keyEquivalent: "").isEnabled = false }
        back.submenu = backMenu
        menu.addItem(back)

        menu.addItem(.separator())
        menu.addItem(ActionMenuItem(title: "关闭显示器…") { [weak self] in self?.onDismiss?(display) })
        return menu
    }

    /// Return one window to the user: its old place for a Handover, the
    /// main display otherwise.
    private func takeBack(_ target: WindowTarget, from display: VirtualDisplay) {
        let home = display.records[target.windowID]?.home
        display.forget(target.windowID)
        Task {
            await Dismissal.giveBack(Dismissal.Item(target: target, app: "", title: "", fate: .giveBack,
                                                    locked: true, home: home))
            self.refreshState()
        }
    }

    private static func name(_ window: WindowCatalog.Window) -> String {
        window.title.isEmpty || window.title == window.app ? window.app : "\(window.app) — \(window.title)"
    }

    private static func icon(_ pid: pid_t) -> NSImage? {
        guard let icon = NSRunningApplication(processIdentifier: pid)?.icon?.copy() as? NSImage else { return nil }
        icon.size = NSSize(width: 16, height: 16)
        return icon
    }

    // MARK: Layout

    /// Place the card at `rect` (screen space). During a transition the
    /// window grows to also cover `stage` so the shrunken card stays visible
    /// on its way to or from the status icon.
    private func layOutCard(at rect: CGRect, stageIncluding stage: CGRect?) {
        let windowFrame = (stage.map { rect.union($0) } ?? rect).insetBy(dx: -margin, dy: -margin)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        panel.setFrame(windowFrame, display: false)
        card.frame = panel.convertFromScreen(rect)
        Glass.updateShadowPath(card, cornerRadius: cornerRadius)
        let content = card.bounds
        header.frame = CGRect(x: 0, y: content.height - headerHeight, width: content.width, height: headerHeight)
        screen.frame = CGRect(x: inset, y: inset, width: content.width - inset * 2,
                              height: content.height - headerHeight - inset)
        sheet?.frame = screen.frame
        CATransaction.commit()
        panel.displayIfNeeded()
    }

    private func cardSize(for display: VirtualDisplay, on screen: NSScreen?) -> CGSize {
        let visible = screen?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1440, height: 900)
        let aspect = display.size.width / display.size.height
        var width = min(visible.width * 0.62, 1500)
        var height = (width - inset * 2) / aspect + headerHeight + inset
        let maxHeight = visible.height * 0.86
        if height > maxHeight {
            height = maxHeight
            width = (height - headerHeight - inset) * aspect + inset * 2
        }
        return CGSize(width: width.rounded(), height: height.rounded())
    }

    // MARK: NSWindowDelegate

    func windowDidMove(_ notification: Notification) {
        if state == .open { restingRect = panel.convertToScreen(card.frame) }
    }
}

/// Title strip: Display Label and status on the left, the action that
/// matters now, the window menu and collapse on the right. Dragging it
/// moves the panel.
private final class HeaderView: NSView {
    enum Primary: Equatable { case none, stepIn(urgent: Bool), handBack(agent: String), review }

    var onPrimary: (() -> Void)?
    /// Open the window menu under the given button.
    var onWindows: ((NSView) -> Void)?
    var onClose: (() -> Void)?

    var title = "" { didSet { titleLabel.stringValue = title; needsLayout = true } }
    var status = NSAttributedString() { didSet { statusLabel.attributedStringValue = status; needsLayout = true } }
    var primary = Primary.none { didSet { if primary != oldValue { updatePrimary() } } }
    var windowCount = 0 {
        didSet { windowsButton.set(title: "窗口 \(windowCount)", symbol: "macwindow.on.rectangle"); needsLayout = true }
    }

    private let titleLabel = NSTextField(labelWithString: "")
    private let statusLabel = NSTextField(labelWithString: "")
    private lazy var primaryButton = ActionButton(title: "") { [weak self] _ in self?.onPrimary?() }
    private lazy var windowsButton = ActionButton(title: "窗口 0", symbol: "macwindow.on.rectangle") { [weak self] button in
        self?.onWindows?(button)
    }
    private lazy var closeButton = ActionButton(title: "", symbol: "chevron.up", hint: "收起到菜单栏") { [weak self] _ in
        self?.onClose?()
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        titleLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        titleLabel.textColor = .labelColor
        titleLabel.lineBreakMode = .byTruncatingTail
        statusLabel.font = .systemFont(ofSize: 12)
        statusLabel.lineBreakMode = .byTruncatingTail
        primaryButton.isHidden = true
        [titleLabel, statusLabel, primaryButton, windowsButton, closeButton].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private func updatePrimary() {
        switch primary {
        case .none:
            primaryButton.isHidden = true
        case let .stepIn(urgent):
            primaryButton.set(title: "接手操作", symbol: urgent ? "hand.raised.fill" : nil)
            primaryButton.setProminent(urgent ? .systemOrange : nil)
            primaryButton.isHidden = false
        case let .handBack(agent):
            primaryButton.set(title: "让 \(agent) 继续", symbol: "play.fill")
            primaryButton.setProminent(.controlAccentColor)
            primaryButton.isHidden = false
        case .review:
            primaryButton.set(title: "处理窗口…", symbol: nil)
            primaryButton.setProminent(.controlAccentColor)
            primaryButton.isHidden = false
        }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let inset: CGFloat = 16
        var trailing = bounds.maxX - 12
        for button in [closeButton, windowsButton, primaryButton] where !button.isHidden {
            let size = button.intrinsicContentSize
            let width = button === closeButton ? size.height : size.width
            button.frame = CGRect(x: trailing - width, y: bounds.midY - size.height / 2, width: width, height: size.height)
            trailing = button.frame.minX - 8
        }
        let textWidth = max(0, trailing - 8 - inset)
        titleLabel.frame = CGRect(x: inset, y: bounds.midY + 1, width: textWidth, height: 17)
        statusLabel.frame = CGRect(x: inset, y: bounds.midY - 17, width: textWidth, height: 16)
    }

    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with event: NSEvent) {
        window?.performDrag(with: event)
    }
}
