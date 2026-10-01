import AppKit

/// Shows a live display frame, aspect-fit, with rounded corners.
class DisplayView: NSView {
    let screenLayer = CALayer()
    /// Drawn over the frame, never into the display, so agent screenshots
    /// do not contain it.
    private let outlineLayer = CALayer()

    var displaySize = DisplayManager.defaultSize {
        didSet { needsLayout = true }
    }

    var cornerRadius: CGFloat = 10 {
        didSet { screenLayer.cornerRadius = cornerRadius }
    }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        screenLayer.contentsGravity = .resize
        screenLayer.masksToBounds = true
        screenLayer.cornerRadius = cornerRadius
        screenLayer.cornerCurve = .continuous
        screenLayer.backgroundColor = NSColor.black.withAlphaComponent(0.35).cgColor
        screenLayer.actions = ["contents": NSNull(), "bounds": NSNull(), "position": NSNull()]
        layer?.addSublayer(screenLayer)
        outlineLayer.cornerCurve = .continuous
        outlineLayer.actions = ["bounds": NSNull(), "position": NSNull()]
        layer?.addSublayer(outlineLayer)
    }

    /// A colored ring around the frame, or none.
    func setOutline(_ color: NSColor?, width: CGFloat = 2.5) {
        let animation = CABasicAnimation(keyPath: "borderWidth")
        animation.fromValue = outlineLayer.presentation()?.borderWidth ?? outlineLayer.borderWidth
        animation.duration = 0.2
        if let color { outlineLayer.borderColor = color.cgColor }
        outlineLayer.borderWidth = color == nil ? 0 : width
        outlineLayer.add(animation, forKey: "outline")
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        screenLayer.frame = contentRect
        outlineLayer.frame = contentRect
        outlineLayer.cornerRadius = cornerRadius
    }

    /// Where the display image sits inside the view.
    var contentRect: CGRect {
        guard bounds.width > 0, bounds.height > 0, displaySize.width > 0 else { return bounds }
        let scale = min(bounds.width / displaySize.width, bounds.height / displaySize.height)
        let size = CGSize(width: displaySize.width * scale, height: displaySize.height * scale)
        return CGRect(x: bounds.midX - size.width / 2, y: bounds.midY - size.height / 2,
                      width: size.width, height: size.height)
    }
}

/// A DisplayView that turns mouse, scroll and key events into input for the
/// virtual display it mirrors.
final class InteractiveDisplayView: DisplayView {
    var displayID: CGDirectDisplayID?
    var forwarder: InputForwarder?
    /// Fires before each click, scroll or key press the user sends.
    var onUserInput: (() -> Void)?

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(
            rect: .zero,
            options: [.mouseMoved, .activeAlways, .inVisibleRect, .cursorUpdate],
            owner: self
        ))
    }

    /// Map a point in this view to global CG coordinates on the virtual display.
    private func remotePoint(_ event: NSEvent, clamp: Bool = false) -> CGPoint? {
        guard let displayID else { return nil }
        let local = convert(event.locationInWindow, from: nil)
        let rect = contentRect
        var nx = (local.x - rect.minX) / rect.width
        var ny = (rect.maxY - local.y) / rect.height  // top-down
        if clamp {
            nx = min(max(nx, 0), 1)
            ny = min(max(ny, 0), 1)
        } else if !(0...1).contains(nx) || !(0...1).contains(ny) {
            return nil
        }
        let bounds = CGDisplayBounds(displayID)
        return CGPoint(x: bounds.minX + nx * (bounds.width - 1), y: bounds.minY + ny * (bounds.height - 1))
    }

    private func flags(_ event: NSEvent) -> CGEventFlags {
        CGEventFlags(rawValue: UInt64(event.modifierFlags.rawValue))
    }

    // MARK: Mouse

    override func mouseDown(with event: NSEvent) { down(event, .left) }
    override func rightMouseDown(with event: NSEvent) { down(event, .right) }
    override func otherMouseDown(with event: NSEvent) { down(event, .center) }
    override func mouseDragged(with event: NSEvent) { drag(event, .left) }
    override func rightMouseDragged(with event: NSEvent) { drag(event, .right) }
    override func otherMouseDragged(with event: NSEvent) { drag(event, .center) }
    override func mouseUp(with event: NSEvent) { up(event, .left) }
    override func rightMouseUp(with event: NSEvent) { up(event, .right) }
    override func otherMouseUp(with event: NSEvent) { up(event, .center) }

    private func down(_ event: NSEvent, _ button: CGMouseButton) {
        NSLog("Sidescreen input: panel mouseDown, forwarder=\(forwarder != nil) display=\(displayID.map(String.init) ?? "nil")")
        window?.makeKey()
        window?.makeFirstResponder(self)
        guard let point = remotePoint(event) else { return }
        onUserInput?()
        forwarder?.mouseDown(at: point, button: button, clickCount: event.clickCount, flags: flags(event))
    }

    private func drag(_ event: NSEvent, _ button: CGMouseButton) {
        guard let point = remotePoint(event, clamp: true) else { return }
        forwarder?.mouseDragged(to: point, button: button, flags: flags(event))
    }

    private func up(_ event: NSEvent, _ button: CGMouseButton) {
        guard let point = remotePoint(event, clamp: true) else { return }
        forwarder?.mouseUp(at: point, button: button, clickCount: event.clickCount, flags: flags(event))
    }

    override func mouseMoved(with event: NSEvent) {
        guard let point = remotePoint(event) else { return }
        forwarder?.mouseMoved(to: point, flags: flags(event))
    }

    override func cursorUpdate(with event: NSEvent) {
        NSCursor.arrow.set()
    }

    override func scrollWheel(with event: NSEvent) {
        guard let point = remotePoint(event, clamp: true) else { return }
        onUserInput?()
        let continuous = event.hasPreciseScrollingDeltas
        forwarder?.scroll(
            at: point,
            deltaX: event.scrollingDeltaX,
            deltaY: event.scrollingDeltaY,
            continuous: continuous,
            phase: Self.scrollPhase(event.phase),
            momentumPhase: Self.momentumPhase(event.momentumPhase),
            flags: flags(event)
        )
    }

    /// NSEvent.Phase to CGScrollPhase.
    private static func scrollPhase(_ phase: NSEvent.Phase) -> Int64 {
        if phase.contains(.began) { return 1 }
        if phase.contains(.changed) { return 2 }
        if phase.contains(.ended) { return 4 }
        if phase.contains(.cancelled) { return 8 }
        if phase.contains(.mayBegin) { return 128 }
        return 0
    }

    /// NSEvent.Phase to CGMomentumScrollPhase.
    private static func momentumPhase(_ phase: NSEvent.Phase) -> Int64 {
        if phase.contains(.began) { return 1 }
        if phase.contains(.changed) { return 2 }
        if phase.contains(.ended) || phase.contains(.cancelled) { return 3 }
        return 0
    }

    // MARK: Keyboard

    override func keyDown(with event: NSEvent) {
        onUserInput?()
        forwarder?.key(code: event.keyCode, down: true, flags: flags(event))
    }

    override func keyUp(with event: NSEvent) {
        forwarder?.key(code: event.keyCode, down: false, flags: flags(event))
    }

    override func flagsChanged(with event: NSEvent) {
        forwarder?.flagsChanged(code: event.keyCode, flags: flags(event))
    }

    /// Command shortcuts would otherwise reach our own (empty) menu first.
    /// AppKit rarely sends keyUp for them, so send the release right away.
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.firstResponder === self, event.type == .keyDown else { return false }
        onUserInput?()
        forwarder?.key(code: event.keyCode, down: true, flags: flags(event))
        forwarder?.key(code: event.keyCode, down: false, flags: flags(event))
        return true
    }
}
