// A text window for input tests, plus a shared Foundation-only gate transport.
import Foundation
import Dispatch

private enum FixtureFrame {
  case message([String: String]), eof, invalid
}

// No partial frame is accepted on EOF. Limit memory before parsing JSON.
private func readFixtureFrame() -> FixtureFrame {
  var bytes = Data()
  while true {
    guard let next = try? FileHandle.standardInput.read(upToCount: 1), !next.isEmpty else {
      return bytes.isEmpty ? .eof : .invalid
    }
    if next[0] == 10 {
      guard let object = try? JSONSerialization.jsonObject(with: bytes) as? [String: String] else { return .invalid }
      return .message(object)
    }
    if bytes.count >= 1023 { return .invalid }
    bytes.append(next)
  }
}

// This is the transport used by Field. The compile-time offline adapter
// below substitutes only the window callback, never the protocol/state.
final class FixtureGate {
  let lock = NSLock()
  var last = ProcessInfo.processInfo.systemUptime
  var shown = false
  func start(windowNumber: Int, heartbeat: Double = 2, lifetime: Double = 60,
             show: @escaping () -> Bool) {
    guard windowNumber > 0, windowNumber <= Int(UInt32.max) else { exit(2) }
    Thread { [self] in
      guard case .message(let hello) = readFixtureFrame(), hello["op"] == "hello",
            let token = hello["token"], token.count == 64 else { exit(2) }
      let ready: [String: Any] = ["pid": getpid(), "window": windowNumber, "token": token]
      guard let data = try? JSONSerialization.data(withJSONObject: ready) else { exit(2) }
      FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
      while true {
        let frame = readFixtureFrame()
        if case .eof = frame { exit(0) }
        guard case .message(let message) = frame, message["token"] == token else { exit(2) }
        lock.lock()
        let op = message["op"]
        if op == "show", !shown {
          shown = true
          last = ProcessInfo.processInfo.systemUptime
          lock.unlock()
          DispatchQueue.main.async { if !show() { exit(2) } }
        } else if op == "hold", shown {
          last = ProcessInfo.processInfo.systemUptime
          lock.unlock()
        } else {
          lock.unlock(); exit(2)
        }
      }
    }.start()
    Thread { [self] in
      let deadline = ProcessInfo.processInfo.systemUptime + lifetime
      while ProcessInfo.processInfo.systemUptime < deadline {
        lock.lock(); let elapsed = ProcessInfo.processInfo.systemUptime - last; lock.unlock()
        if elapsed > heartbeat { exit(2) }
        Thread.sleep(forTimeInterval: min(heartbeat / 4, 0.1))
      }
      exit(0)
    }.start()
  }
}

#if FIELD_PROTOCOL_TEST
// This binary has no AppKit import, application object, window or display.
// A synthetic window number permits offline tests of the actual pipes.
let protocolGate = FixtureGate()
protocolGate.start(windowNumber: 7, heartbeat: 0.75, lifetime: 4) {
  if CommandLine.arguments.contains("--deny-show") { return false }
  if CommandLine.arguments.contains("--trace-show") {
    FileHandle.standardOutput.write(Data("{\"event\":\"show\"}\n".utf8))
  }
  return true
}
dispatchMain()
#else
import AppKit

// Bound AppKit startup before a window or the main run loop exists.
if ProcessInfo.processInfo.environment["FIELD_GATE"] == "1" {
  Thread { Thread.sleep(forTimeInterval: 60); exit(0) }.start()
}

final class Delegate: NSObject, NSApplicationDelegate, NSTextViewDelegate {
  var window: NSWindow!
  let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
  var gate: FixtureGate?
  var usr1: DispatchSourceSignal?
  var usr2: DispatchSourceSignal?

  func applicationDidFinishLaunching(_ note: Notification) {
    // Tests name real Agent Displays. A missing or released stage must
    // never fall back to the user's primary display.
    let env = ProcessInfo.processInfo.environment
    func testScreen(_ key: String) -> NSScreen? {
      guard let value = env[key] else { return nil }
      guard let id = UInt32(value), CGDisplayVendorNumber(id) == 0x5353,
            let screen = NSScreen.screens.first(where: {
              ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id
            }) else { exit(2) }
      return screen
    }
    let opening = testScreen("FIELD_DISPLAY")
    let gated = env["FIELD_GATE"] == "1"
    if gated && opening == nil { exit(2) }
    _ = testScreen("FIELD_MOVE_DISPLAY")
    let origin = opening.map { NSPoint(x: $0.frame.minX + 80, y: $0.frame.minY + 80) } ?? NSPoint(x: 200, y: 200)
    window = NSWindow(contentRect: NSRect(origin: origin, size: NSSize(width: 600, height: 300)),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.title = "Field"
    if let ms = Double(env["FIELD_LIFETIME_MS"] ?? "") {
      DispatchQueue.main.asyncAfter(deadline: .now() + min(ms, 60000) / 1000) { exit(0) }
    }
    // FIELD_MIN_SIZE="width,height" in points, for a test of an app that
    // holds to a minimum size.
    if let min = ProcessInfo.processInfo.environment["FIELD_MIN_SIZE"]?.split(separator: ","),
       min.count == 2, let w = Double(min[0]), let h = Double(min[1]) {
      window.minSize = NSSize(width: w, height: h)
    }
    signal(SIGUSR1, SIG_IGN)
    signal(SIGUSR2, SIG_IGN)
    if !gated {
      installMoveHandlers { testScreen("FIELD_MOVE_DISPLAY") ?? NSScreen.screens[0] }
    }
    text.font = .systemFont(ofSize: 24)
    text.delegate = self
    window.contentView = text
    window.makeFirstResponder(text)
    if gated {
      let gate = FixtureGate()
      self.gate = gate
      let expectedWindow = window.windowNumber
      gate.start(windowNumber: expectedWindow) { [unowned self] in
        guard let screen = testScreen("FIELD_DISPLAY") else { return false }
        guard self.window.windowNumber == expectedWindow, screen.frame.contains(self.window.frame) else { return false }
        self.window.orderFront(nil)
        return true
      }
    } else {
      window.makeKeyAndOrderFront(nil)
    }
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in self.save() }
    // FIELD_REOPEN=ms: close the window after ms and open a new one, titled
    // "Field again", as a VM viewer does when its guest reboots (#46).
    if !gated, let ms = Double(ProcessInfo.processInfo.environment["FIELD_REOPEN"] ?? "") {
      DispatchQueue.main.asyncAfter(deadline: .now() + ms / 1000) { [unowned self] in
        let old: NSWindow = self.window
        old.isReleasedWhenClosed = false
        old.contentView = nil
        old.close()
        self.window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 600, height: 300),
                               styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
        self.window.title = "Field again"
        self.window.contentView = self.text
        self.window.makeKeyAndOrderFront(nil)
      }
    }
  }

  // Legacy movement tests opt into these handlers. Gated fixtures never
  // create the signal sources or register the screen-change callback.
  func installMoveHandlers(destination: @escaping () -> NSScreen) {
    usr1 = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
    usr2 = DispatchSource.makeSignalSource(signal: SIGUSR2, queue: .main)
    let main = { NSPoint(x: destination().frame.minX + 80, y: destination().frame.minY + 80) }
    var back = false
    NotificationCenter.default.addObserver(
      forName: NSWindow.didChangeScreenNotification, object: window, queue: .main
    ) { [unowned self] _ in
      guard back, self.window.screen != destination() else { return }
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { self.window.setFrameOrigin(main()) }
    }
    usr1?.setEventHandler { [unowned self] in self.window.setFrameOrigin(main()) }
    usr2?.setEventHandler { back = true }
    usr1?.resume()
    usr2?.resume()
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

#endif
