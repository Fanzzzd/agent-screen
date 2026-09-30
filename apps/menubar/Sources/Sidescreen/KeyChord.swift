import CoreGraphics

/// A key plus modifiers, parsed from xdotool-style names such as "Return",
/// "ctrl+c" or "cmd+shift+t", the syntax computer-use models already emit.
struct KeyChord: Equatable {
    let code: CGKeyCode
    let flags: CGEventFlags

    enum ParseError: Error, CustomStringConvertible {
        case unknownKey(String)
        case empty

        var description: String {
            switch self {
            case .unknownKey(let name): "unknown key \"\(name)\""
            case .empty: "empty key chord"
            }
        }
    }

    static func parse(_ text: String) throws(ParseError) -> KeyChord {
        let parts = text.split(separator: "+").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
        guard let keyName = parts.last else { throw .empty }
        var flags: CGEventFlags = []
        for modifier in parts.dropLast() {
            guard let flag = modifiers[modifier.lowercased()] else { throw .unknownKey(modifier) }
            flags.insert(flag)
        }
        if let code = keys[keyName.lowercased()] {
            return KeyChord(code: code, flags: flags)
        }
        // Uppercase single letters imply Shift, as in xdotool.
        if keyName.count == 1, let lower = keyName.lowercased().first, let code = keys[String(lower)] {
            if keyName.first!.isUppercase { flags.insert(.maskShift) }
            return KeyChord(code: code, flags: flags)
        }
        throw .unknownKey(keyName)
    }

    private static let modifiers: [String: CGEventFlags] = [
        "ctrl": .maskControl, "control": .maskControl,
        "shift": .maskShift,
        "alt": .maskAlternate, "option": .maskAlternate, "opt": .maskAlternate,
        "cmd": .maskCommand, "command": .maskCommand, "super": .maskCommand, "meta": .maskCommand,
        "fn": .maskSecondaryFn,
    ]

    /// US ANSI virtual key codes.
    private static let keys: [String: CGKeyCode] = {
        var map: [String: CGKeyCode] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
            "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
            "3": 20, "4": 21, "6": 22, "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28,
            "0": 29, "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
            "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45, "m": 46, ".": 47,
            "`": 50,
            "return": 36, "enter": 36, "kp_enter": 76, "tab": 48, "space": 49,
            "backspace": 51, "delete": 117, "escape": 53, "esc": 53,
            "left": 123, "right": 124, "down": 125, "up": 126,
            "home": 115, "end": 119, "page_up": 116, "pageup": 116, "prior": 116,
            "page_down": 121, "pagedown": 121, "next": 121,
            "minus": 27, "equal": 24, "bracketleft": 33, "bracketright": 30, "backslash": 42,
            "semicolon": 41, "apostrophe": 39, "grave": 50, "comma": 43, "period": 47, "slash": 44,
        ]
        let functionKeys: [CGKeyCode] = [122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111]
        for (index, code) in functionKeys.enumerated() { map["f\(index + 1)"] = code }
        return map
    }()
}
