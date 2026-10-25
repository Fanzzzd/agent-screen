// A window with one text view, for input tests. Each change to the text,
// marked text included, is written to $FIELD_OUT as "text\tmarked".
import AppKit

final class Delegate: NSObject, NSApplicationDelegate, NSTextViewDelegate {
  var window: NSWindow!
  let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 600, height: 300),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.title = "Field"
    text.font = .systemFont(ofSize: 24)
    text.delegate = self
    window.contentView = text
    window.makeFirstResponder(text)
    window.makeKeyAndOrderFront(nil)
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in self.save() }
  }

  func save() {
    guard let out = ProcessInfo.processInfo.environment["FIELD_OUT"] else { return }
    let marked = text.hasMarkedText() ? (text.string as NSString).substring(with: text.markedRange()) : ""
    try? "\(text.string)\t\(marked)".write(toFile: out, atomically: true, encoding: .utf8)
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
