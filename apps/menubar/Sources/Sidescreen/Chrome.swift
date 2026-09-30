import AppKit

/// Borderless, non-activating panel. Opening it never pulls the user out of
/// the app they are working in.
final class FloatingPanel: NSPanel {
    private let keyable: Bool

    init(level: NSWindow.Level, keyable: Bool) {
        self.keyable = keyable
        super.init(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel],
                   backing: .buffered, defer: false)
        self.level = level
        isOpaque = false
        backgroundColor = .clear
        // Layer shadows follow the morph transform; the window shadow would not.
        hasShadow = false
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        animationBehavior = .none
        acceptsMouseMovedEvents = true
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
        contentView = PassthroughRootView()
    }

    override var canBecomeKey: Bool { keyable }
    override var canBecomeMain: Bool { false }

    /// The part of the window that takes clicks, in screen coordinates.
    ///
    /// WindowServer routes clicks on a transparent window by pixel alpha, and
    /// live IOSurface frames and glass do not count as opaque pixels, so
    /// clicks on the display image fell through to the app below. Instead the
    /// panel sets `ignoresMouseEvents` from the pointer position: on while the
    /// pointer is outside this rect, off while it is inside.
    var hitRect: (() -> CGRect?)?
    private var monitors: [Any] = []

    override func orderFrontRegardless() {
        super.orderFrontRegardless()
        startTrackingPointer()
    }

    override func orderOut(_ sender: Any?) {
        super.orderOut(sender)
        monitors.forEach(NSEvent.removeMonitor)
        monitors.removeAll()
    }

    private func startTrackingPointer() {
        updateMouseThrough()
        guard monitors.isEmpty else { return }
        let kinds: NSEvent.EventTypeMask = [.mouseMoved, .leftMouseDragged, .rightMouseDragged]
        if let global = NSEvent.addGlobalMonitorForEvents(matching: kinds, handler: { [weak self] _ in
            MainActor.assumeIsolated { self?.updateMouseThrough() }
        }) {
            monitors.append(global)
        }
        if let local = NSEvent.addLocalMonitorForEvents(matching: kinds, handler: { [weak self] event in
            MainActor.assumeIsolated { self?.updateMouseThrough() }
            return event
        }) {
            monitors.append(local)
        }
    }

    func updateMouseThrough() {
        let inside = hitRect?()?.contains(NSEvent.mouseLocation) ?? false
        if ignoresMouseEvents == inside { ignoresMouseEvents = !inside }
    }
}

/// Root view that never claims clicks for itself, only for its subviews.
final class PassthroughRootView: NSView {
    override func hitTest(_ point: NSPoint) -> NSView? {
        let hit = super.hitTest(point)
        return hit === self ? nil : hit
    }
}

/// The card surface: Liquid Glass on macOS 26, a HUD material before that.
@MainActor
enum Glass {
    static func make(cornerRadius: CGFloat) -> (surface: NSView, content: NSView) {
        let content = NSView()
        if #available(macOS 26.0, *) {
            let glass = NSGlassEffectView()
            glass.cornerRadius = cornerRadius
            glass.contentView = content
            return (glass, content)
        }
        let effect = NSVisualEffectView()
        effect.material = .hudWindow
        effect.blendingMode = .behindWindow
        effect.state = .active
        effect.wantsLayer = true
        effect.layer?.cornerRadius = cornerRadius
        effect.layer?.cornerCurve = .continuous
        effect.layer?.masksToBounds = true
        content.frame = effect.bounds
        content.autoresizingMask = [.width, .height]
        effect.addSubview(content)
        return (effect, content)
    }

    private static let surfaceID = NSUserInterfaceItemIdentifier("glass-surface")

    /// A shadow-casting host for a glass surface. Transforms go on this view.
    static func card(cornerRadius: CGFloat) -> (card: NSView, content: NSView) {
        let card = NSView()
        card.wantsLayer = true
        let (surface, content) = make(cornerRadius: cornerRadius)
        surface.identifier = surfaceID
        card.addSubview(surface)
        if let layer = card.layer {
            layer.shadowColor = NSColor.black.cgColor
            layer.shadowOpacity = 0.28
            layer.shadowRadius = 24
            layer.shadowOffset = CGSize(width: 0, height: -10)
        }
        return (card, content)
    }

    static func updateShadowPath(_ card: NSView, cornerRadius: CGFloat) {
        card.layer?.shadowPath = CGPath(roundedRect: card.bounds, cornerWidth: cornerRadius,
                                        cornerHeight: cornerRadius, transform: nil)
        card.subviews.first { $0.identifier == surfaceID }?.frame = card.bounds
    }
}

/// Borderless SF Symbol button with a soft hover plate.
final class SymbolButton: NSButton {
    private var hovering = false { didSet { updateAppearance() } }

    init(symbol: String, tooltip: String, action: @escaping () -> Void) {
        self.handler = action
        super.init(frame: NSRect(x: 0, y: 0, width: 28, height: 28))
        image = NSImage(systemSymbolName: symbol, accessibilityDescription: tooltip)?
            .withSymbolConfiguration(.init(pointSize: 13, weight: .medium))
        toolTip = tooltip
        isBordered = false
        imagePosition = .imageOnly
        target = self
        self.action = #selector(fire)
        wantsLayer = true
        layer?.cornerRadius = 7
        layer?.cornerCurve = .continuous
        updateAppearance()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    private let handler: () -> Void

    @objc private func fire() { handler() }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }

    private func updateAppearance() {
        contentTintColor = hovering ? .labelColor : .secondaryLabelColor
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.12
            context.allowsImplicitAnimation = true
            layer?.backgroundColor = hovering ? NSColor.labelColor.withAlphaComponent(0.1).cgColor : .clear
        }
    }
}
