// A window with one text view, for input tests. Each change to the text,
// marked text included, is written to $FIELD_OUT as "text\tmarked".
import AppKit

final class Delegate: NSObject, NSApplicationDelegate, NSTextViewDelegate {
  var window: NSWindow!
  let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
  let usr1 = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
  let usr2 = DispatchSource.makeSignalSource(signal: SIGUSR2, queue: .main)

  func applicationDidFinishLaunching(_ note: Notification) {
    window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 600, height: 300),
                      styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
    window.title = "Field"
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
    if let screen = testScreen("FIELD_DISPLAY") {
      window.setFrameOrigin(NSPoint(x: screen.frame.minX + 80, y: screen.frame.minY + 80))
    }
    _ = testScreen("FIELD_MOVE_DISPLAY")
    if let ms = Double(env["FIELD_LIFETIME_MS"] ?? "") {
      DispatchQueue.main.asyncAfter(deadline: .now() + min(ms, 60000) / 1000) { exit(0) }
    }
    // FIELD_MIN_SIZE="width,height" in points, for a test of an app that
    // holds to a minimum size.
    if let min = ProcessInfo.processInfo.environment["FIELD_MIN_SIZE"]?.split(separator: ","),
       min.count == 2, let w = Double(min[0]), let h = Double(min[1]) {
      window.minSize = NSSize(width: w, height: h)
    }
    // As an app that resizes its window past a small display does (#19,
    // #47): SIGUSR1 puts the window on the main screen once, now; after
    // SIGUSR2, each time it lands on another screen, 300 ms later.
    let destination = { testScreen("FIELD_MOVE_DISPLAY") ?? NSScreen.screens[0] }
    let main = { NSPoint(x: destination().frame.minX + 80, y: destination().frame.minY + 80) }
    var back = false
    NotificationCenter.default.addObserver(
      forName: NSWindow.didChangeScreenNotification, object: window, queue: .main
    ) { [unowned self] _ in
      guard back, self.window.screen != destination() else { return }
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { self.window.setFrameOrigin(main()) }
    }
    signal(SIGUSR1, SIG_IGN)
    signal(SIGUSR2, SIG_IGN)
    usr1.setEventHandler { [unowned self] in self.window.setFrameOrigin(main()) }
    usr2.setEventHandler { back = true }
    usr1.resume()
    usr2.resume()
    text.font = .systemFont(ofSize: 24)
    text.delegate = self
    window.contentView = text
    window.makeFirstResponder(text)
    window.makeKeyAndOrderFront(nil)
    Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in self.save() }
    // FIELD_REOPEN=ms: close the window after ms and open a new one, titled
    // "Field again", as a VM viewer does when its guest reboots (#46).
    if let ms = Double(ProcessInfo.processInfo.environment["FIELD_REOPEN"] ?? "") {
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
