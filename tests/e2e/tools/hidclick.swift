// A window that is one big button, for realclick. It takes a click only
// while the HID system reads the mouse button down, as apps that check the
// session's button state do (#134): a click posted to the process leaves it
// red, a real click turns it green. The title counts presses (down), real
// clicks (real), the highest click count of a real click (clicks) and real
// right clicks (right). SIGUSR1 paints it white, as a list that reordered
// under the point. HIDCLICK_DISPLAY names the Agent Display to open on.
import AppKit

final class Target: NSView {
  var down = 0, real = 0, clicks = 0, right = 0
  var pending = false
  var moved = false

  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) { press(.left) }
  override func mouseUp(with event: NSEvent) { release(event) }
  override func rightMouseDown(with event: NSEvent) { press(.right) }
  override func rightMouseUp(with event: NSEvent) { release(event) }

  func press(_ button: CGMouseButton) {
    down += 1
    pending = CGEventSource.buttonState(.hidSystemState, button: button)
    show()
  }

  func release(_ event: NSEvent) {
    if pending {
      real += 1
      clicks = max(clicks, event.clickCount)
      if event.type == .rightMouseUp { right += 1 }
    }
    pending = false
    show()
  }

  func show() {
    needsDisplay = true
    window?.title = "HID down=\(down) real=\(real) clicks=\(clicks) right=\(right)"
  }

  override func draw(_ dirtyRect: NSRect) {
    (moved ? NSColor.white : real > 0 ? NSColor.systemGreen : NSColor.systemRed).setFill()
    bounds.fill()
  }
}

final class Delegate: NSObject, NSApplicationDelegate {
  var window: NSWindow!
  var moved: DispatchSourceSignal?

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 400, height: 300),
                      styleMask: [.titled], backing: .buffered, defer: false)
    let target = Target(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
    window.contentView = target
    target.show()
    signal(SIGUSR1, SIG_IGN)
    moved = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
    moved?.setEventHandler {
      target.moved = true
      target.show()
    }
    moved?.resume()
    // Never fall back to the user's screen.
    guard let value = ProcessInfo.processInfo.environment["HIDCLICK_DISPLAY"],
          let id = UInt32(value), CGDisplayVendorNumber(id) == 0x5353,
          let screen = NSScreen.screens.first(where: {
            ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id
          }) else { exit(2) }
    window.setFrameOrigin(NSPoint(x: screen.frame.minX + 120, y: screen.frame.maxY - 420))
    window.orderFront(nil)
    DispatchQueue.main.asyncAfter(deadline: .now() + 180) { exit(0) }
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
