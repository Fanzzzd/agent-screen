import AppKit

/// The large panel that mirrors one virtual display and forwards input to it.
/// It grows out of whatever the user clicked (the status icon or a preview
/// tile) and collapses back into the status icon.
@MainActor
final class ControlPanelController: NSObject, NSWindowDelegate {
    enum State { case closed, opening, open, closing }

    private(set) var state = State.closed
    private(set) var display: VirtualDisplay?

    var isVisible: Bool { state == .opening || state == .open }

    var onPullWindow: (() -> Void)?
    var onSendWindowBack: (() -> Void)?
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
    private let headerHeight: CGFloat = 40
    private let inset: CGFloat = 8

    /// Where the card rests, in screen coordinates. Survives close/open so
    /// the panel comes back where the user dragged it.
    private var restingRect: CGRect?
    /// Bumped on every transition so stale completion blocks do nothing.
    private var generation = 0

    init(hub: StreamHub, forwarders: ForwarderPool, displays: DisplayManager) {
        self.hub = hub
        self.forwarders = forwarders
        self.displays = displays
        let (card, content) = Glass.card(cornerRadius: cornerRadius)
        self.card = card
        super.init()

        screen.cornerRadius = 13
        header.onPull = { [weak self] in self?.onPullWindow?() }
        header.onSendBack = { [weak self] in self?.onSendWindowBack?() }
        header.onClose = { [weak self] in self?.onCloseRequest?() }
        header.onDismiss = { [weak self] in
            guard let self, let display = self.display else { return }
            self.onDismiss?(display)
        }
        header.onStepIn = { [weak self] in
            guard let self, let display = self.display else { return }
            self.displays.beginTakeover(display)
            self.panel.makeFirstResponder(self.screen)
        }
        header.onHandBack = { [weak self] in
            guard let self, let display = self.display else { return }
            self.displays.endTakeover(display)
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
        panel.orderFrontRegardless()
        panel.makeKey()
        panel.makeFirstResponder(screen)

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
        // Collapsing the Viewer hands the display back.
        if let display { displays.endTakeover(display) }
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
            hub.detach(screen.screenLayer, from: old.id)
            displays.endTakeover(old)
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

    /// Header and outline follow the display's control state.
    private func refreshState() {
        guard let display else { return }
        let size = "\(Int(display.size.width)) × \(Int(display.size.height))"
        if display.ended {
            header.mode = .ended
            header.subtitle = "已结束 · \(size)"
        } else if display.takeover {
            header.mode = .takeover
            header.subtitle = display.helpRequest.map { "你在操作 · 求助：\($0)" } ?? "你在操作，agent 暂停"
        } else if let reason = display.helpRequest {
            header.mode = .help
            header.subtitle = "需要你帮忙：\(reason)"
        } else {
            header.mode = display.owner == nil ? .idle : .agent
            header.subtitle = size
        }
        screen.setOutline(display.awaitsUser ? .systemOrange : nil)
        // The user's own hand needs no marker while they operate.
        screen.showPointer(display.takeover ? nil : display.pointer)
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

/// Title strip. Dragging it moves the panel.
private final class HeaderView: NSView {
    enum Mode { case idle, agent, help, takeover, ended }

    var onPull: (() -> Void)?
    var onSendBack: (() -> Void)?
    var onClose: (() -> Void)?
    var onDismiss: (() -> Void)?
    var onHandBack: (() -> Void)?
    var onStepIn: (() -> Void)?

    var title = "" { didSet { titleLabel.stringValue = title; needsLayout = true } }
    var subtitle = "" { didSet { subtitleLabel.stringValue = subtitle; needsLayout = true } }
    var mode = Mode.idle {
        didSet {
            dot.layer?.backgroundColor = switch mode {
            case .idle, .agent: NSColor.systemGreen.cgColor
            case .help, .takeover: NSColor.systemOrange.cgColor
            case .ended: NSColor.tertiaryLabelColor.cgColor
            }
            handBack.isHidden = mode != .takeover
            stepIn.isHidden = mode != .help
            needsLayout = true
        }
    }

    private lazy var stepIn = PillButton(title: "接手", tint: .systemOrange) { [weak self] in
        self?.onStepIn?()
    }
    private lazy var handBack = PillButton(title: "交还控制", tint: .systemOrange) { [weak self] in
        self?.onHandBack?()
    }

    private let dot = NSView()
    private let titleLabel = NSTextField(labelWithString: "")
    private let subtitleLabel = NSTextField(labelWithString: "")
    private lazy var buttons = NSStackView(views: [
        SymbolButton(symbol: "arrow.down.right.and.arrow.up.left.rectangle", tooltip: "把前台窗口拉进这块屏") { [weak self] in
            self?.onPull?()
        },
        SymbolButton(symbol: "rectangle.portrait.and.arrow.forward", tooltip: "把选中的窗口送回主屏") { [weak self] in
            self?.onSendBack?()
        },
        SymbolButton(symbol: "rectangle.badge.xmark", tooltip: "关闭这块显示器") { [weak self] in
            self?.onDismiss?()
        },
        SymbolButton(symbol: "xmark", tooltip: "收起") { [weak self] in self?.onClose?() },
    ])

    override init(frame: NSRect) {
        super.init(frame: frame)
        dot.wantsLayer = true
        dot.layer?.backgroundColor = NSColor.systemGreen.cgColor
        dot.layer?.cornerRadius = 3.5
        titleLabel.font = .systemFont(ofSize: 13, weight: .semibold)
        titleLabel.textColor = .labelColor
        subtitleLabel.font = .monospacedDigitSystemFont(ofSize: 11, weight: .regular)
        subtitleLabel.textColor = .tertiaryLabelColor
        buttons.spacing = 2
        handBack.isHidden = true
        stepIn.isHidden = true
        subtitleLabel.lineBreakMode = .byTruncatingTail
        [dot, titleLabel, subtitleLabel, stepIn, handBack, buttons].forEach(addSubview)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        let midY = bounds.midY
        dot.frame = CGRect(x: 18, y: midY - 3.5, width: 7, height: 7)
        titleLabel.sizeToFit()
        titleLabel.frame.origin = CGPoint(x: dot.frame.maxX + 9, y: midY - titleLabel.frame.height / 2)
        subtitleLabel.sizeToFit()
        subtitleLabel.frame.origin = CGPoint(x: titleLabel.frame.maxX + 8,
                                             y: midY - subtitleLabel.frame.height / 2)
        let size = buttons.fittingSize
        buttons.frame = CGRect(x: bounds.maxX - size.width - 10, y: midY - size.height / 2,
                               width: size.width, height: size.height)
        var trailing = buttons.frame.minX - 8
        for pill in [handBack, stepIn] where !pill.isHidden {
            let size = pill.fittingSize
            pill.frame = CGRect(x: trailing - size.width, y: midY - size.height / 2, width: size.width, height: size.height)
            trailing = pill.frame.minX - 8
        }
        subtitleLabel.frame.size.width = max(0, min(subtitleLabel.frame.width, trailing - subtitleLabel.frame.minX))
    }

    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with event: NSEvent) {
        window?.performDrag(with: event)
    }
}
