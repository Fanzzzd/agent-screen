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

/// A Liquid Glass capsule button: an NSGlassEffectView, tinted for the
/// primary action. NSButton's own glass bezel is not used because AppKit
/// draws buttons of an inactive app grey and untinted, and Sidescreen never
/// becomes the active app. Glass views render the same either way.
/// Icon-only buttons explain themselves through HoverHint.
final class ActionButton: NSControl {
    private let handler: (ActionButton) -> Void
    private let hint: String?
    private let surface: NSView
    private let content: NSView
    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    private var tint: NSColor?
    private var hovering = false { didSet { updateAppearance() } }
    private var pressed = false { didSet { updateAppearance() } }
    private static let height: CGFloat = 28

    init(title: String, symbol: String? = nil, hint: String? = nil, action: @escaping (ActionButton) -> Void) {
        self.handler = action
        self.hint = hint
        (surface, content) = Glass.make(cornerRadius: Self.height / 2)
        super.init(frame: .zero)
        surface.frame = bounds
        surface.autoresizingMask = [.width, .height]
        addSubview(surface)
        label.font = .systemFont(ofSize: 12, weight: .semibold)
        icon.symbolConfiguration = .init(pointSize: 12, weight: .semibold)
        content.addSubview(icon)
        content.addSubview(label)
        set(title: title, symbol: symbol)
        setAccessibilityRole(.button)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    func set(title: String, symbol: String?) {
        label.stringValue = title
        label.isHidden = title.isEmpty
        icon.image = symbol.flatMap { NSImage(systemSymbolName: $0, accessibilityDescription: hint ?? title) }
        icon.isHidden = icon.image == nil
        setAccessibilityLabel(hint ?? title)
        invalidateIntrinsicContentSize()
        needsLayout = true
        updateAppearance()
    }

    /// The one action that matters right now, glass tinted with `tint`.
    func setProminent(_ tint: NSColor?) {
        self.tint = tint
        updateAppearance()
    }

    override var intrinsicContentSize: NSSize {
        if label.isHidden { return NSSize(width: Self.height, height: Self.height) }
        let text = ceil(label.intrinsicContentSize.width)
        return NSSize(width: text + (icon.isHidden ? 0 : 18) + 28, height: Self.height)
    }

    override func layout() {
        super.layout()
        let iconSize: CGFloat = 16
        let text = label.isHidden ? 0 : ceil(label.intrinsicContentSize.width)
        let total = text + (icon.isHidden ? 0 : iconSize) + (icon.isHidden || label.isHidden ? 0 : 4)
        var x = (bounds.width - total) / 2
        if !icon.isHidden {
            icon.frame = CGRect(x: x, y: (bounds.height - iconSize) / 2, width: iconSize, height: iconSize)
            x += iconSize + 4
        }
        let height = label.intrinsicContentSize.height
        label.frame = CGRect(x: x, y: (bounds.height - height) / 2, width: text, height: height)
    }

    private func updateAppearance() {
        if #available(macOS 26.0, *), let glass = surface as? NSGlassEffectView {
            glass.tintColor = tint
        }
        // Glass tint alone barely shows on a dark card; fill the content too.
        content.wantsLayer = true
        content.layer?.cornerRadius = Self.height / 2
        content.layer?.backgroundColor = tint?.withAlphaComponent(hovering ? 1 : 0.88).cgColor
        let foreground: NSColor = tint == nil ? .labelColor : .white
        label.textColor = foreground
        icon.contentTintColor = foreground
        alphaValue = pressed ? 0.7 : 1
        surface.alphaValue = hovering || tint != nil ? 1 : 0.85
    }

    override func mouseDown(with event: NSEvent) { pressed = true }

    override func mouseDragged(with event: NSEvent) {
        pressed = bounds.contains(convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) {
        let inside = bounds.contains(convert(event.locationInWindow, from: nil))
        pressed = false
        guard inside else { return }
        HoverHint.hide()
        handler(self)
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) {
        hovering = true
        showHint()
    }

    override func mouseExited(with event: NSEvent) {
        hovering = false
        HoverHint.hide()
    }

    func showHint() {
        if let hint { HoverHint.show(hint, for: self) }
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func accessibilityPerformPress() -> Bool {
        handler(self)
        return true
    }
}

/// A menu item that runs a closure.
final class ActionMenuItem: NSMenuItem {
    private let handler: () -> Void

    init(title: String, image: NSImage? = nil, action: @escaping () -> Void) {
        self.handler = action
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
        self.image = image
    }

    @available(*, unavailable)
    required init(coder: NSCoder) { fatalError() }

    @objc private func fire() { handler() }
}

/// A small label under a hovered control. AppKit only shows tooltips for the
/// active app, and our panels never activate it.
@MainActor
enum HoverHint {
    private static let bubble = HintBubble()
    private static var pending: DispatchWorkItem?

    static func show(_ text: String, for control: NSView) {
        pending?.cancel()
        let work = DispatchWorkItem { [weak control] in
            MainActor.assumeIsolated {
                guard let control, let content = control.window?.contentView else { return }
                bubble.present(text, under: control, in: content)
            }
        }
        pending = work
        // Short enough to feel immediate, long enough to skip passing pointers.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
    }

    static func hide() {
        pending?.cancel()
        pending = nil
        bubble.removeFromSuperview()
    }
}

private final class HintBubble: NSView {
    private let label = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.85).cgColor
        layer?.cornerRadius = 6
        layer?.cornerCurve = .continuous
        label.font = .systemFont(ofSize: 12, weight: .medium)
        label.textColor = .white
        addSubview(label)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    /// Centered under the control, kept inside the window; above it when
    /// there is no room below.
    func present(_ text: String, under control: NSView, in content: NSView) {
        label.stringValue = text
        label.sizeToFit()
        let size = CGSize(width: label.frame.width + 16, height: label.frame.height + 8)
        let anchor = control.convert(control.bounds, to: content)
        var origin = CGPoint(x: anchor.midX - size.width / 2, y: anchor.minY - 6 - size.height)
        // Stay on the card that holds the control, not the shadow margin.
        var card: NSView = control
        while let parent = card.superview, parent !== content { card = parent }
        let area = card.frame.insetBy(dx: 8, dy: 4)
        if origin.y < area.minY { origin.y = anchor.maxY + 6 }
        origin.x = min(max(origin.x, area.minX), area.maxX - size.width)
        frame = CGRect(origin: origin, size: size).integral
        label.frame.origin = CGPoint(x: 8, y: 4)
        content.addSubview(self, positioned: .above, relativeTo: nil)
        alphaValue = 0
        NSAnimationContext.runAnimationGroup { context in
            context.duration = 0.12
            animator().alphaValue = 1
        }
    }
}
