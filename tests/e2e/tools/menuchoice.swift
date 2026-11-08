// Closed-control choice fixture. Never shows a window outside the Agent Display.
// Its title says what the controls hold and how often a menu opened.
import AppKit
import WebKit
final class SlowCombo: NSComboBox {
  override func setAccessibilityValue(_ value: Any?) {
    guard let text = value as? String else { super.setAccessibilityValue(value); return }
    if text == "Slow" {
      if let marker = ProcessInfo.processInfo.environment["MENU_SETTER_MARKER"] { try? "setting".write(toFile: marker, atomically: true, encoding: .utf8) }
      Thread.sleep(forTimeInterval: 0.4)
    }
    stringValue = text
  }
}
// A pop-up that never takes keyboard focus: keys sent to the app go elsewhere.
final class Unfocusable: NSPopUpButton {
  override var acceptsFirstResponder: Bool { false }
}
final class Delegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
  var window: NSWindow!
  var opens = 0
  let pop = NSPopUpButton(frame: NSRect(x: 30, y: 300, width: 220, height: 28), pullsDown: false)
  let combo = SlowCombo(frame: NSRect(x: 30, y: 240, width: 220, height: 28))
  let locked = Unfocusable(frame: NSRect(x: 30, y: 180, width: 220, height: 28), pullsDown: false)
  // A WebKit page's select: Agent Screen types into none.
  let web = WKWebView(frame: NSRect(x: 20, y: 20, width: 320, height: 140))
  func applicationDidFinishLaunching(_ note: Notification) {
    guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue == ProcessInfo.processInfo.environment["MENU_DISPLAY_ID"] }) else { exit(3) }
    window = NSWindow(contentRect: NSRect(x: 50, y: 100, width: 360, height: 350), styleMask: [.titled], backing: .buffered, defer: true, screen: screen)
    window.setFrameOrigin(NSPoint(x: screen.frame.minX + 50, y: screen.frame.minY + 100))
    guard screen.frame.contains(window.frame) else { exit(4) }
    pop.addItems(withTitles: ["Apple", "Banana", "Cherry", "Dragon fruit", "Dragon", "1", "10", "11", "中文"])
    pop.setAccessibilityLabel("Fruit")
    pop.target = self; pop.action = #selector(changed); pop.menu?.delegate = self
    combo.addItems(withObjectValues: ["Red", "Blue", "Green", "Reject", "Slow"])
    combo.setAccessibilityLabel("Color")
    combo.target = self; combo.action = #selector(changed)
    locked.addItems(withTitles: ["Apple", "Kiwi"])
    locked.setAccessibilityLabel("Locked")
    locked.target = self; locked.action = #selector(changed); locked.menu?.delegate = self
    web.loadHTMLString("<select aria-label='Web fruit'><option>Apple</option><option>Kiwi</option></select>", baseURL: nil)
    for view in [pop, combo, locked, web] as [NSView] { window.contentView!.addSubview(view) }
    window.orderFrontRegardless(); changed()
    Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
      guard screen.frame.contains(self.window.frame), NSScreen.screens.contains(screen) else { exit(5) }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 55) { exit(0) }
  }
  @objc func changed() {
    if combo.stringValue == "Reject" { combo.stringValue = "Blue" }
    window.title = "fruit=\(pop.titleOfSelectedItem ?? "") color=\(combo.stringValue) locked=\(locked.titleOfSelectedItem ?? "") opens=\(opens)"
  }
  func menuWillOpen(_ menu: NSMenu) { opens += 1; changed(); DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { menu.cancelTrackingWithoutAnimation() } }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Delegate(); app.delegate = delegate; app.run()
