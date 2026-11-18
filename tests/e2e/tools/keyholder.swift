// A stand-in for the user's front app: a window with a text field that
// becomes key, then prints "resign" each time it stops being key, and
// "done" after the given seconds.
//
//     keyholder 10
import AppKit

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "10") ?? 10
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 320, height: 80),
                      styleMask: [.titled], backing: .buffered, defer: false)
window.title = "Agent Screen E2E: the user's app"
let field = NSTextField(frame: NSRect(x: 20, y: 24, width: 280, height: 24))
window.contentView?.addSubview(field)
window.center()
NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: window, queue: .main) { _ in
  print("resign"); fflush(stdout)
}
NotificationCenter.default.addObserver(forName: NSWindow.didBecomeKeyNotification, object: window, queue: .main) { _ in
  print("key"); fflush(stdout)
}
window.makeKeyAndOrderFront(nil)
window.makeFirstResponder(field)
app.activate(ignoringOtherApps: true)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { print("done"); fflush(stdout); exit(0) }
app.run()
