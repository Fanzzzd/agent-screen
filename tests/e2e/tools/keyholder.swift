// A stand-in for the user's front app: a window with a text field that
// becomes key, then prints "resign" each time it stops being key, and
// "done" after the given seconds. With "app", each line starts with the
// time in ms, and it also prints "inactive" and "active" as the app stops
// and starts being the active app. KEYHOLDER_TEXT=1 also prints
// "text <value>" as the field changes.
//
//     keyholder 10 [app]
import AppKit

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "10") ?? 10
let appMode = CommandLine.arguments.dropFirst(2).first == "app"
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 320, height: 80),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Agent Screen E2E: the user's app"
let field = NSTextField(frame: NSRect(x: 20, y: 24, width: 280, height: 24))
window.contentView?.addSubview(field)
window.center()
func say(_ line: String) {
  print(appMode ? "\(Int(Date().timeIntervalSince1970 * 1000)) \(line)" : line)
  fflush(stdout)
}
NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: window, queue: .main) { _ in
  say("resign")
}
NotificationCenter.default.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main) { _ in
  say("key")
}
if appMode {
  NotificationCenter.default.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { _ in
    say("inactive")
  }
  NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
    say("active")
  }
}
// KEYHOLDER_TEXT=1: also print "text <value>" each time the field changes.
if ProcessInfo.processInfo.environment["KEYHOLDER_TEXT"] == "1" {
  NotificationCenter.default.addObserver(forName: NSControl.textDidChangeNotification, object: field, queue: .main) { _ in
    say("text \(field.stringValue)")
  }
}
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(field)
app.activate(ignoringOtherApps: true)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { say("done"); exit(0) }
app.run()
