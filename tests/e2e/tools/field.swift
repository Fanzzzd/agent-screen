// A window with one text view, for input tests. Each change to the text,
// marked text included, is written to $FIELD_OUT as "text\tmarked".
// FIELD_CONTROLS=1 adds a password field and a view that takes keys but
// holds no text, as a game does, under the text view, and appends
// "\tpassword\tkeys\tdown\tpin" to the line: the key view's characters,
// with Shift's press and release as "[S]" and "[s]", the keycodes it has
// down, comma-separated, and what the PIN field holds. The PIN field
// reports itself as a password field to accessibility but turns no secure
// input on, as a page's password field in a window that isn't key; text
// that ends in "→" moves the focus to it 60 ms later, as a form that
// advances on its own. There, Tab in the text view moves to the
// password field 60 ms later, as Chrome's renderer moves its focus after
// the key, and the file $FIELD_OUT.focus, once it exists, makes the
// password field take the keyboard by itself, as a page's autofocus.
import AppKit

/// Takes the keyboard on a click and keeps the characters of its keys,
/// Shift's changes, and which keys are down.
final class Keys: NSView {
  var typed = ""
  var down = Set<UInt16>()
  override var acceptsFirstResponder: Bool { true }
  override func mouseDown(with event: NSEvent) { window?.makeFirstResponder(self) }
  override func keyDown(with event: NSEvent) {
    typed += event.characters ?? ""
    down.insert(event.keyCode)
  }
  override func keyUp(with event: NSEvent) { down.remove(event.keyCode) }
  override func flagsChanged(with event: NSEvent) {
    guard event.keyCode == 56 || event.keyCode == 60 else { return }
    typed += event.modifierFlags.contains(.shift) ? "[S]" : "[s]"
  }
  override func draw(_ dirtyRect: NSRect) {
    NSColor.systemTeal.setFill()
    bounds.fill()
  }
}

/// A password field to accessibility only.
final class Pin: NSTextField {
  override func accessibilitySubrole() -> NSAccessibility.Subrole? { .secureTextField }
}

final class Delegate: NSObject, NSApplicationDelegate, NSTextViewDelegate {
  var window: NSWindow!
  let text = NSTextView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
  let usr1 = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
  let usr2 = DispatchSource.makeSignalSource(signal: SIGUSR2, queue: .main)
  let controls = ProcessInfo.processInfo.environment["FIELD_CONTROLS"] == "1"
  let password = NSSecureTextField(frame: NSRect(x: 10, y: 56, width: 580, height: 24))
  let keys = Keys(frame: NSRect(x: 0, y: 20, width: 600, height: 28))
  let pin = Pin(frame: NSRect(x: 10, y: 0, width: 580, height: 18))

  /// Keeps App Nap from stretching the 60 ms moves: Field sits behind
  /// the user's front app, and a browser's focus change runs on no timer.
  var awake: NSObjectProtocol?

  func applicationDidFinishLaunching(_ note: Notification) {
    awake = ProcessInfo.processInfo.beginActivity(
      options: [.userInitiated, .latencyCritical], reason: "moves its focus on time for tests")
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
    if controls {
      let content = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 300))
      text.frame = NSRect(x: 0, y: 88, width: 600, height: 212)
      content.addSubview(text)
      content.addSubview(password)
      content.addSubview(keys)
      content.addSubview(pin)
      window.contentView = content
    } else {
      window.contentView = text
    }
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

  func textDidChange(_ note: Notification) {
    guard controls, text.string.hasSuffix("→") else { return }
    let asked = Date()
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) {
      self.window.makeFirstResponder(self.pin)
      self.noteMove(since: asked)
    }
  }

  /// How long after it was asked for a delayed move ran, in ms, one line
  /// each in $FIELD_OUT.moves.
  func noteMove(since asked: Date) {
    guard let out = ProcessInfo.processInfo.environment["FIELD_OUT"] else { return }
    let line = "\(Int(Date().timeIntervalSince(asked) * 1000))\n"
    let path = out + ".moves"
    if let handle = FileHandle(forWritingAtPath: path) {
      handle.seekToEndOfFile()
      handle.write(line.data(using: .utf8)!)
      handle.closeFile()
    } else {
      try? line.write(toFile: path, atomically: true, encoding: .utf8)
    }
  }

  func textView(_ view: NSTextView, doCommandBy selector: Selector) -> Bool {
    guard controls, selector == #selector(NSResponder.insertTab(_:)) else { return false }
    let asked = Date()
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.06) {
      self.window.makeFirstResponder(self.password)
      self.noteMove(since: asked)
    }
    return true
  }

  func save() {
    guard let out = ProcessInfo.processInfo.environment["FIELD_OUT"] else { return }
    let marked = text.hasMarkedText() ? (text.string as NSString).substring(with: text.markedRange()) : ""
    let focus = out + ".focus"
    if controls, FileManager.default.fileExists(atPath: focus) {
      try? FileManager.default.removeItem(atPath: focus)
      window.makeFirstResponder(password)
    }
    let down = keys.down.sorted().map(String.init).joined(separator: ",")
    let extra = controls
      ? "\t\(password.stringValue)\t\(keys.typed)\t\(down)\t\(pin.stringValue)" : ""
    try? "\(text.string)\t\(marked)\(extra)".write(toFile: out, atomically: true, encoding: .utf8)
  }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
