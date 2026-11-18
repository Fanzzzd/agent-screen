// A window with unsaved changes: closing it shows a sheet that asks whether
// to save, as document apps do. Writes what happened to $ASK_OUT: "asked",
// then "saved", "discarded" or "cancelled".
import AppKit

func note(_ s: String) {
  if let path = ProcessInfo.processInfo.environment["ASK_OUT"] {
    try? (s + "\n").appendLine(to: path)
  }
}
extension String {
  func appendLine(to path: String) throws {
    if let h = FileHandle(forWritingAtPath: path) { h.seekToEndOfFile(); h.write(data(using: .utf8)!); h.closeFile() }
    else { try write(toFile: path, atomically: true, encoding: .utf8) }
  }
}

final class Delegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
  var window: NSWindow!
  var closing = false
  func applicationDidFinishLaunching(_ n: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 600, height: 360),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.title = "Draft"
    window.isDocumentEdited = true
    window.delegate = self
    window.makeKeyAndOrderFront(nil)
  }
  func windowShouldClose(_ sender: NSWindow) -> Bool {
    if closing { return true }
    note("asked")
    let alert = NSAlert()
    alert.messageText = "Do you want to save the changes made to the document “Draft”?"
    alert.addButton(withTitle: "Save")
    alert.addButton(withTitle: "Don't Save")
    alert.addButton(withTitle: "Cancel")
    alert.beginSheetModal(for: sender) { response in
      switch response {
      case .alertFirstButtonReturn: note("saved")
      case .alertSecondButtonReturn: note("discarded")
      default: note("cancelled"); return
      }
      self.closing = true
      sender.close()
    }
    return false
  }
  func applicationShouldTerminateAfterLastWindowClosed(_ s: NSApplication) -> Bool { true }
}
let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
