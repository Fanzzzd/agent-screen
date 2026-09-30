import AppKit

/// Development entry point that drives the input path without the UI:
///
///     Sidescreen --probe displays
///     Sidescreen --probe pull <displayID>        move the frontmost window onto a display
///     Sidescreen --probe click <x> <y>           global CG points
///     Sidescreen --probe type <text>             to the last clicked window
///
/// Run it from a terminal that already has Accessibility permission while the
/// menu bar app keeps the virtual display alive.
@MainActor
enum Probe {
    static func run(_ args: [String]) -> Int32 {
        let forwarder = InputForwarder()
        switch args.first {
        case "displays":
            var count: UInt32 = 0
            CGGetActiveDisplayList(0, nil, &count)
            var ids = [CGDirectDisplayID](repeating: 0, count: Int(count))
            CGGetActiveDisplayList(count, &ids, &count)
            for id in ids { print(id, CGDisplayBounds(id)) }
        case "pull" where args.count == 2:
            guard let id = UInt32(args[1]) else { return 2 }
            do {
                let target = try WindowMover.frontmostWindow()
                try WindowMover.move(target, into: CGDisplayBounds(id))
                print("moved pid \(target.pid) window \(target.windowID)")
            } catch {
                print("pull failed: \(error)")
                return 1
            }
        case "click" where args.count == 3:
            guard let x = Double(args[1]), let y = Double(args[2]) else { return 2 }
            let point = CGPoint(x: x, y: y)
            let before = CGEvent(source: nil)?.location ?? .zero
            forwarder.mouseDown(at: point, button: .left, clickCount: 1, flags: [])
            forwarder.mouseUp(at: point, button: .left, clickCount: 1, flags: [])
            forwarder.flush()
            let after = CGEvent(source: nil)?.location ?? .zero
            print("clicked \(point), target \(String(describing: forwarder.focusedTarget))")
            print("cursor before \(before) after \(after)")
        case "type" where args.count >= 2:
            // A click first so the forwarder knows its target window.
            guard let point = lastClick() else { return 2 }
            forwarder.mouseDown(at: point, button: .left, clickCount: 1, flags: [])
            forwarder.mouseUp(at: point, button: .left, clickCount: 1, flags: [])
            for character in args.dropFirst().joined(separator: " ") {
                guard let (code, shift) = keyCode(for: character) else { continue }
                let flags: CGEventFlags = shift ? .maskShift : []
                forwarder.key(code: code, down: true, flags: flags)
                forwarder.key(code: code, down: false, flags: flags)
            }
            forwarder.flush()
        default:
            print("usage: Sidescreen --probe displays | pull <displayID> | click <x> <y> | type <text>")
            return 2
        }
        return 0
    }

    /// `type` reuses the point from SIDESCREEN_PROBE_POINT="x,y".
    private static func lastClick() -> CGPoint? {
        let parts = (ProcessInfo.processInfo.environment["SIDESCREEN_PROBE_POINT"] ?? "")
            .split(separator: ",").compactMap { Double($0) }
        return parts.count == 2 ? CGPoint(x: parts[0], y: parts[1]) : nil
    }

    /// US ANSI key codes for the characters the probe needs.
    private static func keyCode(for character: Character) -> (CGKeyCode, Bool)? {
        let letters: [Character: CGKeyCode] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
            "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "o": 31, "u": 32,
            "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46, " ": 49,
        ]
        if let code = letters[character] { return (code, false) }
        if let lower = character.lowercased().first, character.isUppercase, let code = letters[lower] {
            return (code, true)
        }
        return nil
    }
}
