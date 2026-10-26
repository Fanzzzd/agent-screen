// A native login window, as aTrust's could be: a user field, which has the
// keyboard, and a password field. The title says which field has focus and
// what each holds: "login focus=user|password|none text=USER|PASSWORD
// key=0|1". A second window, "other", can take key status from it. It
// never makes itself the active app: the user may be typing.
import AppKit

final class Delegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
  var window: NSWindow!
  var other: NSWindow!
  let user = NSTextField(frame: NSRect(x: 20, y: 136, width: 320, height: 24))
  let password = NSSecureTextField(frame: NSRect(x: 20, y: 66, width: 320, height: 24))

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 360, height: 200),
                      styleMask: [.titled, .closable], backing: .buffered, defer: false)
    window.title = "login"
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 360, height: 200))
    content.addSubview(user)
    content.addSubview(password)
    window.contentView = content
    window.delegate = self
    window.makeFirstResponder(user)
    window.orderFrontRegardless()
    // A second window of the app, whose click takes key status from the
    // login window.
    other = NSWindow(contentRect: NSRect(x: 620, y: 200, width: 300, height: 200),
                     styleMask: [.titled, .closable], backing: .buffered, defer: false)
    other.title = "other"
    other.contentView = NSTextView(frame: NSRect(x: 0, y: 0, width: 300, height: 200))
    other.orderFrontRegardless()
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in self.show() }
  }

  func show() {
    var focus = "none"
    if let editor = window.firstResponder as? NSText, let field = editor.delegate as? NSTextField {
      focus = field === password ? "password" : field === user ? "user" : "none"
    }
    let title = "login focus=\(focus) text=\(user.stringValue)|\(password.stringValue) key=\(window.isKeyWindow ? 1 : 0)"
    if window.title != title { window.title = title }
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
