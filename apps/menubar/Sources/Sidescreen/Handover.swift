import AppKit

/// The user places one of their own windows onto an Agent Display, either
/// with the Viewer's pull button or by dragging the window onto the Viewer.
/// The window's frame and AeroSpace workspace are recorded first, so
/// dismissal can put it back exactly where it was.
@MainActor
enum Handover {
    static func perform(_ target: WindowTarget, from frame: CGRect?, onto display: VirtualDisplay) async throws {
        let frame = frame ?? WindowCatalog.all().first { $0.window.target == target }?.global
        let workspace = try? await AeroSpace.current?.workspace(of: target.windowID)
        display.claim(target, origin: .handedOver,
                      home: frame.map { WindowHome(frame: $0, workspace: workspace ?? nil) })
        try await Placement.place(target, onto: display.id)
    }
}

/// Watches the user's mouse for a window drag that ends over the Viewer.
///
/// Title-bar drags belong to WindowServer and the dragged app, so this only
/// observes: the window under the pointer at mouse-down is the candidate,
/// and it counts as dragged once its frame has moved. Releasing over the
/// Viewer hands it over.
@MainActor
final class DragHandoverWatcher {
    /// Where a drop counts, in Cocoa screen coordinates, or nil when the
    /// Viewer cannot take a window right now.
    var dropRect: (() -> CGRect?)?
    var onHover: ((Bool) -> Void)?
    var onDrop: ((WindowTarget, CGRect) -> Void)?

    private var candidate: (target: WindowTarget, frame: CGRect)?
    private var hovering = false
    private var lastCheck: CFAbsoluteTime = 0
    private var monitors: [Any] = []

    func start() {
        let kinds: NSEvent.EventTypeMask = [.leftMouseDown, .leftMouseDragged, .leftMouseUp]
        if let global = NSEvent.addGlobalMonitorForEvents(matching: kinds, handler: { [weak self] event in
            let type = event.type
            MainActor.assumeIsolated { self?.handle(type) }
        }) {
            monitors.append(global)
        }
    }

    private func handle(_ type: NSEvent.EventType) {
        switch type {
        case .leftMouseDown:
            candidate = nil
            guard dropRect?() != nil, let hit = Self.window(at: ScreenSpace.toCG(NSEvent.mouseLocation)) else { return }
            candidate = hit
        case .leftMouseDragged:
            guard candidate != nil else { return }
            let now = CFAbsoluteTimeGetCurrent()
            guard now - lastCheck > 0.05 else { return }
            lastCheck = now
            setHovering(isDragging() && isOverDropRect())
        case .leftMouseUp:
            defer {
                candidate = nil
                setHovering(false)
            }
            guard let candidate, isOverDropRect(), isDragging() else { return }
            onDrop?(candidate.target, candidate.frame)
        default:
            break
        }
    }

    private func setHovering(_ value: Bool) {
        guard hovering != value else { return }
        hovering = value
        onHover?(value)
    }

    private func isOverDropRect() -> Bool {
        dropRect?()?.contains(NSEvent.mouseLocation) ?? false
    }

    /// The candidate has moved since mouse-down, so this is a window drag.
    private func isDragging() -> Bool {
        guard let candidate,
              let now = WindowCatalog.all().first(where: { $0.window.target == candidate.target })?.global
        else { return false }
        return abs(now.minX - candidate.frame.minX) > 4 || abs(now.minY - candidate.frame.minY) > 4
    }

    /// The user's window under a CG point: a normal window of another app,
    /// on a physical display.
    private static func window(at point: CGPoint) -> (target: WindowTarget, frame: CGRect)? {
        guard let hit = WindowCatalog.all().first(where: { $0.global.contains(point) }) else { return nil }
        let onVirtual = NSScreen.screens.contains { screen in
            guard let id = screen.displayID else { return false }
            return CGDisplayIsBuiltin(id) == 0 && CGDisplayVendorNumber(id) == 0x5353
                && CGDisplayBounds(id).contains(point)
        }
        return onVirtual ? nil : (hit.window.target, hit.global)
    }
}
