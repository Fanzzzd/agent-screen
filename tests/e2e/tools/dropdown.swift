// A search box's results list as Qt 5.15 shows it, as WeCom's is: a
// borderless window of its own over the main window's rows, closed by an
// NSEvent global monitor on any mouse press in another app while this
// app isn't active (qcocoawindow.mm, "Close popup when clicking outside
// it"). A real click in the Viewer is such a press.
//
// The main window has rows; "open" on its first row opens the list over
// the rows below it. Its title says what happened:
// "dropdown open=1|0 got=<what the last press hit> closed=<why>", where
// got is rowN for a row of the main window and itemN for an item of the
// list, and closed is outside for a press in another app, item for a pick.

import AppKit

let rowHeight: CGFloat = 50

/// Each press and close, with its time and point, for a test to read:
/// DROPDOWN_LOG names the file.
func note(_ line: String) {
  guard let path = ProcessInfo.processInfo.environment["DROPDOWN_LOG"] else { return }
  let text = String(format: "%.3f ", Date().timeIntervalSince1970) + line + "\n"
  if let handle = FileHandle(forWritingAtPath: path) {
    handle.seekToEndOfFile(); handle.write(text.data(using: .utf8)!); handle.closeFile()
  } else {
    FileManager.default.createFile(atPath: path, contents: text.data(using: .utf8))
  }
}

final class Rows: NSView {
  var onPress: (Int) -> Void = { _ in }
  override var isFlipped: Bool { true }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func draw(_ dirty: NSRect) {
    for i in 0..<Int(bounds.height / rowHeight) {
      (i % 2 == 0 ? NSColor.white : NSColor(white: 0.93, alpha: 1)).setFill()
      NSRect(x: 0, y: CGFloat(i) * rowHeight, width: bounds.width, height: rowHeight).fill()
      let label = i == 0 ? "open" : "row\(i)"
      label.draw(at: NSPoint(x: 12, y: CGFloat(i) * rowHeight + 16), withAttributes: [.font: NSFont.systemFont(ofSize: 15)])
    }
  }
  override func mouseDown(with event: NSEvent) {
    let y = convert(event.locationInWindow, from: nil).y
    note("press main y=\(y) row=\(Int(y / rowHeight)) window=\(event.windowNumber)")
    onPress(Int(y / rowHeight))
  }
}

final class Items: NSView {
  var onPress: (Int) -> Void = { _ in }
  override var isFlipped: Bool { true }
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func draw(_ dirty: NSRect) {
    NSColor(calibratedRed: 0.85, green: 0.92, blue: 1, alpha: 1).setFill()
    bounds.fill()
    for i in 1...Int(bounds.height / rowHeight) {
      "item\(i)".draw(at: NSPoint(x: 12, y: CGFloat(i - 1) * rowHeight + 16), withAttributes: [.font: NSFont.boldSystemFont(ofSize: 15)])
    }
  }
  override func mouseDown(with event: NSEvent) {
    let y = convert(event.locationInWindow, from: nil).y
    note("press list y=\(y) item=\(Int(y / rowHeight) + 1) window=\(event.windowNumber)")
    onPress(Int(y / rowHeight) + 1)
  }
}

final class App: NSObject, NSApplicationDelegate {
  var window: NSWindow!
  var list: NSPanel?
  var monitor: Any?
  var got = "none"
  var why = "none"

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(
      contentRect: NSRect(x: 200, y: 200, width: 400, height: 6 * rowHeight),
      styleMask: [.titled, .closable], backing: .buffered, defer: false)
    let rows = Rows(frame: window.contentView!.bounds)
    rows.onPress = { [weak self] row in
      guard let self else { return }
      if row == 0 { self.open() } else { self.got = "row\(row)"; self.update() }
    }
    window.contentView = rows
    update()
    window.makeKeyAndOrderFront(nil)
  }

  func open() {
    close(why: "reopen")
    let content = window.contentRect(forFrameRect: window.frame)
    // Over rows 1 to 4, as WeCom's list covers its chat rows.
    let frame = NSRect(
      x: content.minX + 40, y: content.maxY - 5 * rowHeight,
      width: content.width - 80, height: 4 * rowHeight)
    let panel = NSPanel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.level = .normal
    panel.hasShadow = true
    // A Qt pop-up stays when its app deactivates; only the monitor closes it.
    panel.hidesOnDeactivate = false
    let items = Items(frame: NSRect(origin: .zero, size: frame.size))
    items.onPress = { [weak self] item in
      guard let self else { return }
      self.got = "item\(item)"
      self.close(why: "item")
    }
    panel.contentView = items
    panel.orderFront(nil)
    list = panel
    note("open list window=\(panel.windowNumber) frame=\(frame)")
    // Qt installs the monitor unless the parent is its focus window, and
    // WeCom's list closed on a press in another app while the agent used
    // it. Here AppKit takes Agent Screen's background clicks for an
    // activation, so the monitor goes on whatever the app thinks.
    note("monitor on")
    monitor = NSEvent.addGlobalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) { [weak self] _ in
      self?.close(why: "outside")
    }
    update()
  }

  func close(why: String) {
    if list != nil { self.why = why; note("close why=\(why)") }
    if let monitor { NSEvent.removeMonitor(monitor) }
    monitor = nil
    list?.orderOut(nil)
    list = nil
    update()
  }

  func update() {
    window.title = "dropdown open=\(list == nil ? 0 : 1) got=\(got) closed=\(why)"
  }
}

let app = NSApplication.shared
let delegate = App()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
