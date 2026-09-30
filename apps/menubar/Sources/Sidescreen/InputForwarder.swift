import AppKit
import SidescreenPrivate

struct WindowTarget: Equatable, Sendable {
    let pid: pid_t
    let windowID: CGWindowID
}

/// Delivers the user's input from the control panel to windows on a virtual
/// display. Events go straight to the owning process through SkyLight, so the
/// real cursor never leaves the panel and the user's frontmost app keeps its
/// place in the window stack.
///
/// All work runs on one serial queue: the focus handshake needs short sleeps
/// and CGWindowList lookups should stay off the main thread.
final class InputForwarder: @unchecked Sendable {
    private let queue = DispatchQueue(label: "dev.sidescreen.input", qos: .userInteractive)
    private let source = CGEventSource(stateID: .hidSystemState)

    // Queue-confined state.
    private var focused: WindowTarget?
    private var gesture: (target: WindowTarget, group: Int64)?
    private var windowCache: (list: [WindowInfo], time: CFAbsoluteTime)?

    /// The window that most recently received a click, read on the main thread.
    var focusedTarget: WindowTarget? { queue.sync { focused } }

    /// Block until every queued event has been posted.
    func flush() { queue.sync {} }

    // MARK: Mouse

    func mouseDown(at point: CGPoint, button: CGMouseButton, clickCount: Int, flags: CGEventFlags) {
        queue.async { [self] in
            guard let target = window(at: point, fresh: true) else {
                NSLog("Sidescreen input: no window at \(point)")
                return
            }
            NSLog("Sidescreen input: down at \(point) -> pid \(target.pid) window \(target.windowID)")
            if focused != target {
                // Let AppKit in the target update its key-window routing before
                // the click stream arrives (cua uses the same 50 ms settle).
                if SSFocusWithoutRaise(target.pid, target.windowID) { usleep(50_000) }
                focused = target
            }
            let group = Int64.random(in: 1...Int64(Int32.max))
            gesture = (target, group)
            post(.mouseMoved, at: point, to: target, button: button, clickState: 0, group: group, flags: flags)
            post(Self.downType(button), at: point, to: target, button: button,
                 clickState: Int64(clickCount), group: group, flags: flags)
        }
    }

    func mouseDragged(to point: CGPoint, button: CGMouseButton, flags: CGEventFlags) {
        queue.async { [self] in
            guard let gesture else { return }
            post(Self.dragType(button), at: point, to: gesture.target, button: button,
                 clickState: 1, group: gesture.group, flags: flags)
        }
    }

    func mouseUp(at point: CGPoint, button: CGMouseButton, clickCount: Int, flags: CGEventFlags) {
        queue.async { [self] in
            guard let gesture else { return }
            post(Self.upType(button), at: point, to: gesture.target, button: button,
                 clickState: Int64(clickCount), group: gesture.group, flags: flags)
            self.gesture = nil
        }
    }

    func mouseMoved(to point: CGPoint, flags: CGEventFlags) {
        queue.async { [self] in
            guard gesture == nil, let target = window(at: point, fresh: false) else { return }
            post(.mouseMoved, at: point, to: target, button: .left, clickState: 0, group: 0, flags: flags)
        }
    }

    func scroll(at point: CGPoint, deltaX: CGFloat, deltaY: CGFloat, continuous: Bool,
                phase: Int64, momentumPhase: Int64, flags: CGEventFlags) {
        queue.async { [self] in
            guard let target = window(at: point, fresh: false),
                  let event = CGEvent(scrollWheelEvent2Source: source, units: continuous ? .pixel : .line,
                                      wheelCount: 2, wheel1: Int32(deltaY.rounded()),
                                      wheel2: Int32(deltaX.rounded()), wheel3: 0)
            else { return }
            event.location = point
            event.flags = flags
            event.setIntegerValueField(.scrollWheelEventIsContinuous, value: continuous ? 1 : 0)
            event.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
            event.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentumPhase)
            if continuous {
                event.setDoubleValueField(.scrollWheelEventPointDeltaAxis1, value: Double(deltaY))
                event.setDoubleValueField(.scrollWheelEventPointDeltaAxis2, value: Double(deltaX))
            }
            stampRouting(event, at: point, to: target)
            deliver(event, to: target.pid, authenticate: false)
        }
    }

    // MARK: Agent input
    //
    // Whole gestures for Agent Sessions. Each returns once every event is
    // posted, so a tool call can take a screenshot straight after.

    func click(at point: CGPoint, button: CGMouseButton, count: Int, flags: CGEventFlags) async {
        for index in 1...max(count, 1) {
            mouseDown(at: point, button: button, clickCount: index, flags: flags)
            await pause(milliseconds: 30)
            mouseUp(at: point, button: button, clickCount: index, flags: flags)
            if index < count { await pause(milliseconds: 80) }
        }
        await drain()
    }

    func drag(from start: CGPoint, to end: CGPoint, flags: CGEventFlags) async {
        mouseDown(at: start, button: .left, clickCount: 1, flags: flags)
        let steps = 12
        for step in 1...steps {
            let t = CGFloat(step) / CGFloat(steps)
            await pause(milliseconds: 12)
            mouseDragged(to: CGPoint(x: start.x + (end.x - start.x) * t, y: start.y + (end.y - start.y) * t),
                         button: .left, flags: flags)
        }
        mouseUp(at: end, button: .left, clickCount: 1, flags: flags)
        await drain()
    }

    /// Type text as Unicode, independent of the keyboard layout. One
    /// character per event: many apps read only the first character of a
    /// key event, so batching drops text.
    func typeText(_ text: String, within bounds: CGRect) async {
        await focusSomething(within: bounds)
        for character in text {
            let units = Array(String(character).utf16)
            queue.async { [self] in
                guard let focused else { return }
                for down in [true, false] {
                    guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down) else { continue }
                    event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units)
                    deliver(event, to: focused.pid, authenticate: true)
                }
            }
            await pause(milliseconds: 4)
        }
        await drain()
    }

    func press(_ chord: KeyChord, repeat count: Int, within bounds: CGRect) async {
        await focusSomething(within: bounds)
        for _ in 0..<max(count, 1) {
            key(code: chord.code, down: true, flags: chord.flags)
            await pause(milliseconds: 20)
            key(code: chord.code, down: false, flags: chord.flags)
        }
        await drain()
    }

    /// Keys go to the last clicked window. When nothing on this display has
    /// been clicked yet, pick its topmost window so typing still lands.
    private func focusSomething(within bounds: CGRect) async {
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            queue.async { [self] in
                let list = Self.onScreenWindows()
                let stillThere = focused.flatMap { target in list.first { $0.target == target } }
                if stillThere.map({ bounds.intersects($0.bounds) }) != true,
                   let top = list.first(where: { bounds.contains(CGPoint(x: $0.bounds.midX, y: $0.bounds.midY)) })
                {
                    if SSFocusWithoutRaise(top.target.pid, top.target.windowID) { usleep(50_000) }
                    focused = top.target
                }
                done.resume()
            }
        }
    }

    private func drain() async {
        await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in
            queue.async { done.resume() }
        }
    }

    private func pause(milliseconds: UInt64) async {
        try? await Task.sleep(for: .milliseconds(milliseconds))
    }

    // MARK: Keyboard

    func key(code: CGKeyCode, down: Bool, flags: CGEventFlags) {
        queue.async { [self] in
            guard let focused, let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)
            else { return }
            event.flags = flags
            deliver(event, to: focused.pid, authenticate: true)
        }
    }

    func flagsChanged(code: CGKeyCode, flags: CGEventFlags) {
        queue.async { [self] in
            guard let focused, let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: true)
            else { return }
            event.type = .flagsChanged
            event.flags = flags
            deliver(event, to: focused.pid, authenticate: true)
        }
    }

    // MARK: Posting

    private func post(_ type: CGEventType, at point: CGPoint, to target: WindowTarget, button: CGMouseButton,
                      clickState: Int64, group: Int64, flags: CGEventFlags) {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type,
                                  mouseCursorPosition: point, mouseButton: button)
        else { return }
        event.flags = flags
        SSEventSetIntegerField(event, 1, clickState)  // kCGMouseEventClickState
        SSEventSetIntegerField(event, 3, Int64(button.rawValue))  // kCGMouseEventButtonNumber
        SSEventSetIntegerField(event, 7, 3)  // kCGMouseEventSubtype: touch
        if group != 0 {
            SSEventSetIntegerField(event, 58, group)  // click group: coalesces one gesture
        }
        stampRouting(event, at: point, to: target)
        // One route only: posting through SkyLight and the public API both
        // delivers every event twice to AppKit targets.
        deliver(event, to: target.pid, authenticate: false)
    }

    /// Fields WindowServer and Chromium use to route an event to a window that
    /// is neither key nor under the real cursor.
    private func stampRouting(_ event: CGEvent, at point: CGPoint, to target: WindowTarget) {
        let windowID = Int64(target.windowID)
        SSEventSetIntegerField(event, 40, Int64(target.pid))  // target pid
        SSEventSetIntegerField(event, 51, windowID)  // window number
        SSEventSetIntegerField(event, 91, windowID)  // window under pointer
        SSEventSetIntegerField(event, 92, windowID)  // window under pointer that can handle it
        SSEventSetWindowLocation(event, point)
    }

    private func deliver(_ event: CGEvent, to pid: pid_t, authenticate: Bool) {
        if !SSPostEventToPid(pid, event, authenticate) {
            event.postToPid(pid)
        }
    }

    // MARK: Hit testing

    private struct WindowInfo {
        let target: WindowTarget
        let bounds: CGRect
    }

    /// Topmost window at a global CG point. Moves and scrolls reuse a list up
    /// to 100 ms old; clicks always query WindowServer.
    private func window(at point: CGPoint, fresh: Bool) -> WindowTarget? {
        let now = CFAbsoluteTimeGetCurrent()
        let list: [WindowInfo]
        if !fresh, let cache = windowCache, now - cache.time < 0.1 {
            list = cache.list
        } else {
            list = Self.onScreenWindows()
            windowCache = (list, now)
        }
        return list.first { $0.bounds.contains(point) }?.target
    }

    private static func onScreenWindows() -> [WindowInfo] {
        // Desktop elements stay in: Finder's desktop window takes clicks on
        // an empty display. Only the wallpaper itself is skipped below.
        guard let raw = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
                as? [[String: Any]]
        else { return [] }
        let me = getpid()
        return raw.compactMap { info in
            guard let pid = info[kCGWindowOwnerPID as String] as? pid_t, pid != me,
                  let windowID = info[kCGWindowNumber as String] as? CGWindowID,
                  let layer = info[kCGWindowLayer as String] as? Int,
                  // Skip the menu bar and status items that sit on every display.
                  layer != Int(CGWindowLevelForKey(.mainMenuWindow)),
                  layer != Int(CGWindowLevelForKey(.statusWindow)),
                  // Wallpaper, menu bar backdrop and similar system surfaces.
                  !["Window Server", "Dock"].contains(info[kCGWindowOwnerName as String] as? String ?? ""),
                  (info[kCGWindowAlpha as String] as? Double ?? 1) > 0,
                  let boundsDict = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDict as CFDictionary)
            else { return nil }
            return WindowInfo(target: WindowTarget(pid: pid, windowID: windowID), bounds: bounds)
        }
    }

    private static func downType(_ button: CGMouseButton) -> CGEventType {
        switch button {
        case .right: .rightMouseDown
        case .center: .otherMouseDown
        default: .leftMouseDown
        }
    }

    private static func upType(_ button: CGMouseButton) -> CGEventType {
        switch button {
        case .right: .rightMouseUp
        case .center: .otherMouseUp
        default: .leftMouseUp
        }
    }

    private static func dragType(_ button: CGMouseButton) -> CGEventType {
        switch button {
        case .right: .rightMouseDragged
        case .center: .otherMouseDragged
        default: .leftMouseDragged
        }
    }
}
