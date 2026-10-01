import AppKit
import SidescreenPrivate

/// The Agent Session an Agent Display belongs to.
struct DisplayOwner: Equatable {
    /// Stable per client session: Claude Code's session ID, Codex's
    /// `session_id` from request metadata, or the MCP process as a fallback.
    let sessionKey: String
    /// "Claude Code", "Codex", or whatever the client reports.
    let client: String
    /// Task title the agent gave, or the project folder name.
    let title: String
    /// The control connection that created the display. When it closes, the
    /// display becomes an Ended Display.
    let connectionID: Int
}

/// One virtual display. macOS removes the display when this object is freed.
@MainActor
final class VirtualDisplay {
    let id: CGDirectDisplayID
    let name: String
    /// Logical size in points. Pixels are twice this because the display is HiDPI.
    let size: CGSize
    /// Nil for displays the user created from the menu.
    let owner: DisplayOwner?
    /// Set once the owning Agent Session is gone.
    private(set) var ended = false
    private let handle: CGVirtualDisplay

    /// Display Label: "Claude Code · Book flights".
    var label: String {
        guard let owner else { return name }
        return "\(owner.client) · \(owner.title)"
    }

    /// Windows Sidescreen put on this display. Anything else found here got
    /// here by accident, for example through a window manager.
    private(set) var windows: Set<CGWindowID> = []

    /// The user is operating the display in the Viewer; agent input is refused.
    fileprivate(set) var takeover = false
    fileprivate var waiters: [UUID: CheckedContinuation<ControlWait, Never>] = [:]

    func markEnded() { ended = true }

    func claim(_ windowID: CGWindowID) { windows.insert(windowID) }

    init?(name: String, size: CGSize, serial: UInt32, owner: DisplayOwner?) {
        let descriptor = CGVirtualDisplayDescriptor()
        descriptor.queue = .main
        descriptor.name = name
        descriptor.maxPixelsWide = UInt32(size.width * 2)
        descriptor.maxPixelsHigh = UInt32(size.height * 2)
        // Report a ~110 ppi panel so macOS picks sensible default scaling.
        let millimetersPerPoint = 25.4 / 110
        descriptor.sizeInMillimeters = CGSize(
            width: size.width * millimetersPerPoint,
            height: size.height * millimetersPerPoint
        )
        descriptor.vendorID = 0x5353  // "SS"
        descriptor.productID = 0x0001
        descriptor.serialNum = serial

        guard let handle = CGVirtualDisplay(descriptor: descriptor) else { return nil }

        let settings = CGVirtualDisplaySettings()
        settings.hiDPI = 1
        settings.modes = [
            CGVirtualDisplayMode(width: UInt32(size.width), height: UInt32(size.height), refreshRate: 60)
        ]
        guard handle.apply(settings) else { return nil }

        self.handle = handle
        self.id = handle.displayID
        self.name = name
        self.size = size
        self.owner = owner
    }

    /// Global bounds in CoreGraphics coordinates (top-left origin, points).
    var bounds: CGRect { CGDisplayBounds(id) }
}

@MainActor
final class DisplayManager {
    private(set) var displays: [VirtualDisplay] = []
    private var nextSerial: UInt32 = 1
    private var observers: [() -> Void] = []

    /// Computer-use models downscale screenshots to about 1280 px wide, so a
    /// larger default only blurs text in what the agent sees.
    static let defaultSize = CGSize(width: 1440, height: 900)

    func observe(_ handler: @escaping () -> Void) {
        observers.append(handler)
    }

    @discardableResult
    func create(size: CGSize = DisplayManager.defaultSize, owner: DisplayOwner? = nil) -> VirtualDisplay? {
        let serial = nextSerial
        nextSerial += 1
        guard let display = VirtualDisplay(name: "Sidescreen \(serial)", size: size, serial: serial, owner: owner)
        else {
            return nil
        }
        displays.append(display)
        // WindowServer needs a moment before the display accepts configuration.
        let id = display.id
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) {
            MainActor.assumeIsolated { DisplayArrangement.tuckAway(id, among: self.displays.map(\.id)) }
        }
        notify()
        return display
    }

    func remove(_ display: VirtualDisplay) {
        displays.removeAll { $0 === display }
        notify()
    }

    func display(withID id: CGDirectDisplayID) -> VirtualDisplay? {
        displays.first { $0.id == id }
    }

    /// The live display of an Agent Session, if it has one.
    func display(forSession sessionKey: String) -> VirtualDisplay? {
        displays.first { $0.owner?.sessionKey == sessionKey && !$0.ended }
    }

    func end(_ display: VirtualDisplay) {
        guard !display.ended else { return }
        display.markEnded()
        display.takeover = false
        resumeWaiters(of: display, with: .ended)
        notify()
        notifyState(display)
    }

    private func notify() {
        observers.forEach { $0() }
    }

    // MARK: Control

    /// Fires when Takeover changes on a display.
    func observeState(_ handler: @escaping (VirtualDisplay) -> Void) {
        stateObservers.append(handler)
    }

    private var stateObservers: [(VirtualDisplay) -> Void] = []

    private func notifyState(_ display: VirtualDisplay) {
        stateObservers.forEach { $0(display) }
    }

    /// Only a live Agent Display has an Agent Session to refuse.
    func beginTakeover(_ display: VirtualDisplay) {
        guard display.owner != nil, !display.ended, !display.takeover else { return }
        display.takeover = true
        notifyState(display)
    }

    func endTakeover(_ display: VirtualDisplay) {
        guard display.takeover else { return }
        display.takeover = false
        resumeWaiters(of: display, with: .returned)
        notifyState(display)
    }

    /// Suspend until the user hands the display back, the display ends, or
    /// the timeout passes.
    func waitForControl(_ display: VirtualDisplay, timeout: Duration) async -> ControlWait {
        if display.ended { return .ended }
        if !display.takeover { return .returned }
        let id = UUID()
        return await withCheckedContinuation { continuation in
            display.waiters[id] = continuation
            Task { @MainActor in
                try? await Task.sleep(for: timeout)
                display.waiters.removeValue(forKey: id)?.resume(returning: .timedOut)
            }
        }
    }

    private func resumeWaiters(of display: VirtualDisplay, with outcome: ControlWait) {
        let waiters = display.waiters.values
        display.waiters.removeAll()
        waiters.forEach { $0.resume(returning: outcome) }
    }
}

enum ControlWait: String {
    case returned, timedOut, ended
}

enum DisplayArrangement {
    /// Park a virtual display diagonally off the bottom-right corner of the
    /// physical displays so the real cursor rarely wanders onto it.
    static func tuckAway(_ id: CGDirectDisplayID, among virtualIDs: [CGDirectDisplayID]) {
        var count: UInt32 = 0
        CGGetActiveDisplayList(0, nil, &count)
        var active = [CGDirectDisplayID](repeating: 0, count: Int(count))
        CGGetActiveDisplayList(count, &active, &count)

        let others = active.filter { $0 != id }
        let physical = others.filter { !virtualIDs.contains($0) }
        let anchor = (physical.isEmpty ? [CGMainDisplayID()] : physical)
            .map { CGDisplayBounds($0) }
            .reduce(CGRect.null) { $0.union($1) }
        // Stack additional virtual displays to the right of earlier ones.
        let occupied = others.filter { virtualIDs.contains($0) }
            .map { CGDisplayBounds($0) }
            .reduce(anchor) { $0.union($1) }

        var config: CGDisplayConfigRef?
        guard CGBeginDisplayConfiguration(&config) == .success, let config else { return }
        CGConfigureDisplayOrigin(config, id, Int32(occupied.maxX), Int32(anchor.maxY))
        CGCompleteDisplayConfiguration(config, .forSession)
    }
}

extension NSScreen {
    var displayID: CGDirectDisplayID? {
        (deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value
    }
}

enum ScreenSpace {
    /// Height of the primary display, the pivot between Cocoa (bottom-left)
    /// and CoreGraphics (top-left) global coordinates.
    @MainActor static var primaryHeight: CGFloat {
        NSScreen.screens.first?.frame.maxY ?? CGDisplayBounds(CGMainDisplayID()).height
    }

    @MainActor static func toCG(_ point: CGPoint) -> CGPoint {
        CGPoint(x: point.x, y: primaryHeight - point.y)
    }

    @MainActor static func toCocoa(_ rect: CGRect) -> CGRect {
        CGRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }
}
