// A window whose buttons change it a few seconds after they are pressed,
// for wait_for_change. Send adds "Sending…" to the Messages group at once
// and sets the status to "Status: sending"; 3 s later "Sending…" goes,
// "Delivered" comes and the status reads "Status: sent". Tick raises
// "Count N", outside Messages, after 1 s. Clear removes "Delivered" after
// 1.5 s. Close closes the window after 1.5 s. Every button works through
// accessibility, so a script presses it without the user's pointer. It
// never makes itself the active app: the user may be typing.
import AppKit

final class Group: NSView {
  override func isAccessibilityElement() -> Bool { true }
  override func accessibilityRole() -> NSAccessibility.Role? { .group }
  override func accessibilityLabel() -> String? { "Messages" }
}

final class Delegate: NSObject, NSApplicationDelegate {
  var window: NSWindow!
  let messages = Group(frame: NSRect(x: 20, y: 110, width: 380, height: 130))
  let status = NSTextField(labelWithString: "Status: idle")
  let count = NSTextField(labelWithString: "Count 0")
  var ticks = 0
  var sending: NSTextField?
  var delivered: NSTextField?

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 420, height: 260),
                      styleMask: [.titled, .closable], backing: .buffered, defer: false)
    window.title = "watch"
    window.isReleasedWhenClosed = false
    let content = NSView(frame: NSRect(x: 0, y: 0, width: 420, height: 260))
    let hello = NSTextField(labelWithString: "hello")
    hello.frame = NSRect(x: 10, y: 100, width: 300, height: 20)
    messages.addSubview(hello)
    content.addSubview(messages)
    status.frame = NSRect(x: 20, y: 80, width: 200, height: 20)
    count.frame = NSRect(x: 240, y: 80, width: 160, height: 20)
    content.addSubview(status)
    content.addSubview(count)
    for (i, title) in ["Send", "Tick", "Clear", "Close"].enumerated() {
      let button = NSButton(title: title, target: self, action: #selector(press(_:)))
      button.frame = NSRect(x: 20 + i * 98, y: 20, width: 90, height: 32)
      content.addSubview(button)
    }
    window.contentView = content
    window.orderFrontRegardless()
  }

  func label(_ text: String, y: CGFloat) -> NSTextField {
    let label = NSTextField(labelWithString: text)
    label.frame = NSRect(x: 10, y: y, width: 300, height: 20)
    messages.addSubview(label)
    return label
  }

  func later(_ seconds: Double, _ work: @escaping () -> Void) {
    DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: work)
  }

  @objc func press(_ sender: NSButton) {
    switch sender.title {
    case "Send":
      sending = label("Sending…", y: 70)
      status.stringValue = "Status: sending"
      later(3) {
        self.sending?.removeFromSuperview()
        self.sending = nil
        self.delivered = self.label("Delivered", y: 70)
        self.status.stringValue = "Status: sent"
      }
    case "Tick":
      later(1) {
        self.ticks += 1
        self.count.stringValue = "Count \(self.ticks)"
      }
    case "Clear":
      later(1.5) {
        self.delivered?.removeFromSuperview()
        self.delivered = nil
      }
    default:
      later(1.5) { self.window.close() }
    }
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
