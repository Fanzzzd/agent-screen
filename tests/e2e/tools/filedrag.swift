// Two windows for filedrag (issue #166), one per process, as Finder and a
// browser are. FILEDRAG_ROLE=source is a file icon that starts a system
// drag session with FILEDRAG_FILE's URL once the pointer moves 4 points
// with the button down, as Finder's icons do. Its title counts presses
// (down), sessions begun (began), sessions ended (ended) and the last
// session's operation (op, 0 for none). FILEDRAG_ROLE=target takes a
// dropped file URL; its title counts drags that entered it (entered) and
// drops (drops), and names the last dropped file (got). FILEDRAG_DISPLAY
// names the Agent Display to open on, FILEDRAG_X the window's distance
// from the display's left edge. Agent Screen centers a new window, so the
// window takes its place again 1 and 2 s after it shows. SIGUSR1 moves it
// 60 points right, as a window the user moved.
import AppKit

final class Source: NSView, NSDraggingSource {
  var down = 0, began = 0, ended = 0, op: UInt = 0
  var start: NSPoint?
  let url: URL

  init(frame: NSRect, url: URL) {
    self.url = url
    super.init(frame: frame)
  }
  required init?(coder: NSCoder) { fatalError() }

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

  override func mouseDown(with event: NSEvent) {
    down += 1
    start = convert(event.locationInWindow, from: nil)
    show()
  }

  override func mouseDragged(with event: NSEvent) {
    guard let from = start else { return }
    let at = convert(event.locationInWindow, from: nil)
    if hypot(at.x - from.x, at.y - from.y) < 4 { return }
    start = nil
    began += 1
    let item = NSDraggingItem(pasteboardWriter: url as NSURL)
    item.setDraggingFrame(NSRect(x: from.x - 24, y: from.y - 24, width: 48, height: 48),
                          contents: NSWorkspace.shared.icon(forFile: url.path))
    beginDraggingSession(with: [item], event: event, source: self)
    show()
  }

  override func mouseUp(with event: NSEvent) { start = nil }

  func draggingSession(_ session: NSDraggingSession,
                       sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
    .copy
  }

  func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint,
                       operation: NSDragOperation) {
    ended += 1
    op = operation.rawValue
    show()
  }

  func show() {
    needsDisplay = true
    window?.title = "Source down=\(down) began=\(began) ended=\(ended) op=\(op)"
  }

  override func draw(_ dirtyRect: NSRect) {
    NSColor.systemBlue.setFill()
    bounds.fill()
    NSWorkspace.shared.icon(forFile: url.path)
      .draw(in: NSRect(x: bounds.midX - 32, y: bounds.midY - 32, width: 64, height: 64))
  }
}

final class Drop: NSView {
  var entered = 0, drops = 0, got = "-"

  override init(frame: NSRect) {
    super.init(frame: frame)
    registerForDraggedTypes([.fileURL])
  }
  required init?(coder: NSCoder) { fatalError() }

  override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
    entered += 1
    show()
    return .copy
  }

  override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation { .copy }

  override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
    let urls = sender.draggingPasteboard.readObjects(
      forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL]
    drops += 1
    got = urls?.first?.lastPathComponent ?? "?"
    show()
    return true
  }

  func show() {
    needsDisplay = true
    window?.title = "Drop entered=\(entered) drops=\(drops) got=\(got)"
  }

  override func draw(_ dirtyRect: NSRect) {
    (drops > 0 ? NSColor.systemGreen : NSColor.systemGray).setFill()
    bounds.fill()
  }
}

final class Delegate: NSObject, NSApplicationDelegate {
  var window: NSWindow!
  var moved: DispatchSourceSignal?
  var shift = 0.0
  // App Nap would delay the timers and the signal of an app in the back.
  let awake = ProcessInfo.processInfo.beginActivity(
    options: [.userInitiated, .latencyCritical], reason: "filedrag fixture")

  func applicationDidFinishLaunching(_ note: Notification) {
    let env = ProcessInfo.processInfo.environment
    let frame = NSRect(x: 0, y: 0, width: 400, height: 300)
    window = NSWindow(contentRect: frame, styleMask: [.titled], backing: .buffered, defer: false)
    if env["FILEDRAG_ROLE"] == "source" {
      let path = env["FILEDRAG_FILE"] ?? "/tmp/filedrag.txt"
      let view = Source(frame: frame, url: URL(fileURLWithPath: path))
      window.contentView = view
      view.show()
    } else {
      let view = Drop(frame: frame)
      window.contentView = view
      view.show()
    }
    // Never fall back to the user's screen.
    guard let value = env["FILEDRAG_DISPLAY"],
          let id = UInt32(value), CGDisplayVendorNumber(id) == 0x5353,
          let screen = NSScreen.screens.first(where: {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id
          }) else { exit(2) }
    let x = Double(env["FILEDRAG_X"] ?? "") ?? 120
    let place = { [self] in
      window.setFrameOrigin(NSPoint(x: screen.frame.minX + x + shift, y: screen.frame.maxY - 420))
    }
    place()
    window.orderFront(nil)
    for delay in [1.0, 2.0] {
      DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: place)
    }
    signal(SIGUSR1, SIG_IGN)
    moved = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
    moved?.setEventHandler { [self] in
      shift = 60
      place()
    }
    moved?.resume()
    DispatchQueue.main.asyncAfter(deadline: .now() + 180) { exit(0) }
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
