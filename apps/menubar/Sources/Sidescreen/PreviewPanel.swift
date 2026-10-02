import AppKit

/// The strip of live thumbnails that drops out of the status icon on hover.
@MainActor
final class PreviewPanelController {
    var onSelect: ((VirtualDisplay, CGRect) -> Void)?
    var onCreate: (() -> Void)?
    /// Fires when the pointer enters or leaves the preview card.
    var onHoverChange: ((Bool) -> Void)?

    private(set) var isShown = false {
        didSet {
            #if DEBUG
            if isShown != oldValue { NSLog("Sidescreen peek: \(isShown ? "shown" : "hidden")") }
            #endif
        }
    }
    var onDismiss: ((VirtualDisplay) -> Void)?
    var activeDisplayID: CGDirectDisplayID? { didSet { tiles.forEach(markActive) } }

    /// Only a display tile can be the one open in the Viewer.
    private func markActive(_ tile: PreviewTile) {
        tile.isActive = tile.display != nil && tile.display?.id == activeDisplayID
    }

    private let hub: StreamHub
    private let displays: DisplayManager
    private let panel = FloatingPanel(level: .popUpMenu, keyable: false)
    private let card: NSView
    private let content: NSView
    private var tiles: [PreviewTile] = []
    private var generation = 0

    private let margin: CGFloat = 32
    private let padding: CGFloat = 10
    private let spacing: CGFloat = 10
    private let tileWidth: CGFloat = 208
    private let cornerRadius: CGFloat = 18

    init(hub: StreamHub, displays: DisplayManager) {
        self.hub = hub
        self.displays = displays
        (card, content) = Glass.card(cornerRadius: cornerRadius)
        let hover = HoverView()
        hover.onChange = { [weak self] inside in self?.onHoverChange?(inside) }
        hover.frame = card.bounds
        hover.autoresizingMask = [.width, .height]
        card.addSubview(hover, positioned: .below, relativeTo: nil)
        panel.contentView?.addSubview(card)
        panel.hitRect = { [weak self] in
            guard let self, self.isShown else { return nil }
            return self.panel.convertToScreen(self.card.frame)
        }
        // Tiles hold their displays; drop them when one goes away while
        // Peek is hidden, so macOS can free it.
        displays.observe { [weak self] in
            guard let self, !self.isShown else { return }
            self.tiles.forEach { $0.removeFromSuperview() }
            self.tiles = []
        }
        displays.observeState { [weak self] display in
            self?.tiles.first { $0.display === display }?.refresh()
        }
    }

    /// The card on screen.
    var cardFrame: CGRect { panel.convertToScreen(card.frame) }

    func show(below anchor: CGRect) {
        rebuildTiles()
        let size = cardSize
        let screen = NSScreen.screens.first { $0.frame.intersects(anchor) } ?? NSScreen.main
        let visible = screen?.frame ?? .zero
        var x = anchor.midX - size.width / 2
        x = min(max(x, visible.minX + 8), visible.maxX - size.width - 8)
        let rect = CGRect(x: x, y: anchor.minY - 6 - size.height, width: size.width, height: size.height).integral

        let frame = rect.insetBy(dx: -margin, dy: -margin)
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        card.frame = CGRect(x: margin, y: margin, width: size.width, height: size.height)
        Glass.updateShadowPath(card, cornerRadius: cornerRadius)
        layOutTiles()
        CATransaction.commit()

        for tile in tiles {
            if let display = tile.display { hub.attach(tile.thumbnail.screenLayer, to: display.id, fps: 12) }
        }

        // Live thumbnails ignore layer transforms, so the drop-in moves the
        // window itself: it slides 10 pt down out of the menu bar and fades in.
        if !isShown {
            panel.setFrame(frame.offsetBy(dx: 0, dy: 10), display: false)
            panel.alphaValue = 0
        }
        isShown = true
        generation += 1
        panel.orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.24
            context.timingFunction = Self.settle
            panel.animator().setFrame(frame, display: true)
            panel.animator().alphaValue = 1
        }
    }

    func hide(animated: Bool = true) {
        guard isShown else { return }
        isShown = false
        generation += 1
        let token = generation
        guard animated else { return finishHide(token) }

        NSAnimationContext.runAnimationGroup({ context in
            context.duration = 0.14
            context.timingFunction = CAMediaTimingFunction(name: .easeIn)
            panel.animator().setFrame(panel.frame.offsetBy(dx: 0, dy: 6), display: true)
            panel.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated { self?.finishHide(token) }
        })
    }

    private func finishHide(_ token: Int) {
        guard generation == token else { return }
        panel.orderOut(nil)
        for tile in tiles {
            if let display = tile.display { hub.detach(tile.thumbnail.screenLayer, from: display.id) }
        }
    }

    /// Fast start, long soft landing.
    private static let settle = CAMediaTimingFunction(controlPoints: 0.16, 1, 0.3, 1)

    // MARK: Tiles

    private var cardSize: CGSize {
        let width = tiles.map(\.frame.width).reduce(0, +) + CGFloat(max(tiles.count - 1, 0)) * spacing
        let height = tiles.map(\.frame.height).max() ?? 0
        return CGSize(width: padding * 2 + width, height: padding * 2 + height)
    }

    /// Displays in use first, then the ones waiting for review.
    private var ordered: [VirtualDisplay] {
        displays.displays.filter { !$0.ended } + displays.displays.filter(\.ended)
    }

    private func rebuildTiles() {
        let wanted = ordered.map(\.id)
        if tiles.compactMap({ $0.display?.id }) == wanted, tiles.last?.display == nil, !tiles.isEmpty {
            tiles.forEach { $0.refresh() }
            return
        }

        tiles.forEach { $0.removeFromSuperview() }
        let height = tileWidth / (DisplayManager.defaultSize.width / DisplayManager.defaultSize.height)
            + PreviewTile.labelHeight
        tiles = ordered.map { display in
            let tile = PreviewTile(display: display)
            tile.onClick = { [weak self, weak tile] in
                guard let self, let tile, let display = tile.display else { return }
                let rect = tile.window?.convertToScreen(tile.thumbnail.convert(tile.thumbnail.contentRect, to: nil))
                self.onSelect?(display, rect ?? .zero)
            }
            tile.onDismiss = { [weak self] in self?.onDismiss?(display) }
            return tile
        }
        let add = PreviewTile(display: nil)
        add.onClick = { [weak self] in self?.onCreate?() }
        tiles.append(add)
        for tile in tiles {
            // With displays around, adding one is a narrow "+" at the end.
            let narrow = tile.display == nil && tiles.count > 1
            tile.frame.size = CGSize(width: narrow ? 44 : tileWidth, height: height)
            markActive(tile)
            content.addSubview(tile)
        }
    }

    private func layOutTiles() {
        var x = padding
        for tile in tiles {
            tile.frame.origin = CGPoint(x: x, y: padding)
            x += tile.frame.width + spacing
        }
    }
}

/// One thumbnail, or the "new display" tile when `display` is nil.
private final class PreviewTile: NSView {
    static let labelHeight: CGFloat = 38

    let display: VirtualDisplay?
    let thumbnail = DisplayView()
    var onClick: (() -> Void)?
    var onDismiss: (() -> Void)?
    var isActive = false { didSet { updateRing() } }

    private let label = NSTextField(labelWithString: "")
    /// Control state under the title: a Help Request, Takeover, or ended.
    private let status = NSTextField(labelWithString: "")
    private let plus = NSImageView()
    private lazy var close: NSButton = {
        let button = FirstMouseButton(image: NSImage(systemSymbolName: "xmark.circle.fill", accessibilityDescription: "关闭显示器")!
            .withSymbolConfiguration(.init(pointSize: 17, weight: .regular).applying(.init(paletteColors: [.white, NSColor.black.withAlphaComponent(0.55)])))!,
            target: self, action: #selector(dismissTapped))
        button.isBordered = false
        button.isHidden = true
        return button
    }()
    private var hovering = false { didSet { lift() } }

    init(display: VirtualDisplay?) {
        self.display = display
        super.init(frame: .zero)
        wantsLayer = true
        thumbnail.cornerRadius = 9
        if let display {
            thumbnail.displaySize = display.size
        } else {
            label.stringValue = "新建显示器"
            thumbnail.screenLayer.backgroundColor = NSColor.labelColor.withAlphaComponent(0.06).cgColor
            plus.image = NSImage(systemSymbolName: "plus", accessibilityDescription: "新建显示器")?
                .withSymbolConfiguration(.init(pointSize: 20, weight: .regular))
            plus.contentTintColor = .secondaryLabelColor
            thumbnail.addSubview(plus)
        }
        label.font = .systemFont(ofSize: 11, weight: .medium)
        label.textColor = .secondaryLabelColor
        label.alignment = .center
        label.lineBreakMode = .byTruncatingTail
        status.font = .systemFont(ofSize: 10.5, weight: .regular)
        status.alignment = .center
        status.lineBreakMode = .byTruncatingTail
        addSubview(thumbnail)
        addSubview(label)
        addSubview(status)
        if display != nil { addSubview(close) }
        thumbnail.screenLayer.borderColor = NSColor.controlAccentColor.cgColor
        refresh()
    }

    /// Title and status line follow the display's state.
    func refresh() {
        guard let display else { return }
        label.stringValue = display.label
        let state = DisplayStatus(display, windowCount: WindowCatalog.windows(on: display).count)
        status.attributedStringValue = state.attributed(state.short, size: 10.5, alignment: .center)
        needsLayout = true
        toolTip = "\(display.label)\n\(state.long)"
        thumbnail.showPointer(display.takeover || display.ended ? nil : display.pointer)
        lift()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        thumbnail.frame = CGRect(x: 0, y: Self.labelHeight, width: bounds.width, height: bounds.height - Self.labelHeight)
        if display == nil, bounds.width < 60 {
            // The narrow "+": no caption, the symbol says it.
            label.frame = .zero
            status.frame = .zero
        } else if status.stringValue.isEmpty {
            label.frame = CGRect(x: 0, y: 11, width: bounds.width, height: 16)
            status.frame = .zero
        } else {
            label.frame = CGRect(x: 0, y: 18, width: bounds.width, height: 16)
            status.frame = CGRect(x: 0, y: 3, width: bounds.width, height: 14)
        }
        plus.frame = thumbnail.bounds
        close.frame = CGRect(x: thumbnail.frame.maxX - 26, y: thumbnail.frame.maxY - 26, width: 22, height: 22)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) {
        hovering = true
        if display == nil, bounds.width < 60 { HoverHint.show("新建显示器", for: self) }
    }

    override func mouseExited(with event: NSEvent) {
        hovering = false
        HoverHint.hide()
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    @objc private func dismissTapped() {
        HoverHint.hide()
        onDismiss?()
    }

    override func mouseDown(with event: NSEvent) {}

    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { onClick?() }
    }

    /// Hover and "currently open" both show as a ring. A scale lift would
    /// need a layer transform, which live IOSurface frames ignore.
    private func lift() {
        label.textColor = hovering ? .labelColor : .secondaryLabelColor
        close.isHidden = !hovering
        updateRing()
    }

    private func updateRing() {
        let layer = thumbnail.screenLayer
        let width: CGFloat = isActive ? 2 : (hovering ? 1.5 : 0)
        let color = isActive ? NSColor.controlAccentColor : NSColor.labelColor.withAlphaComponent(0.55)
        let animation = CABasicAnimation(keyPath: "borderWidth")
        animation.fromValue = layer.presentation()?.borderWidth ?? layer.borderWidth
        animation.duration = 0.15
        layer.borderColor = color.cgColor
        layer.borderWidth = width
        layer.add(animation, forKey: "ring")
    }
}

/// Reports pointer enter/exit for the whole card without eating clicks.
private final class HoverView: NSView {
    var onChange: ((Bool) -> Void)?

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) { onChange?(true) }
    override func mouseExited(with event: NSEvent) { onChange?(false) }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

/// Takes the click that also brings its non-key panel forward.
private final class FirstMouseButton: NSButton {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) {
        if let text = image?.accessibilityDescription { HoverHint.show(text, for: self) }
    }

    override func mouseExited(with event: NSEvent) { HoverHint.hide() }
}
