// Closed-control choice fixture. Never shows a window outside the Agent Display.
import AppKit
final class Delegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
  var window: NSWindow!
  var opens = 0
  let pop = NSPopUpButton(frame: NSRect(x: 30, y: 160, width: 220, height: 28), pullsDown: false)
  let combo = NSComboBox(frame: NSRect(x: 30, y: 90, width: 220, height: 28))
  func applicationDidFinishLaunching(_ note: Notification) {
    guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue == ProcessInfo.processInfo.environment["MENU_DISPLAY_ID"] }) else { exit(3) }
    window = NSWindow(contentRect: NSRect(x: 50, y: 100, width: 360, height: 260), styleMask: [.titled], backing: .buffered, defer: true, screen: screen)
    window.setFrameOrigin(NSPoint(x: screen.frame.minX + 50, y: screen.frame.minY + 100))
    guard screen.frame.contains(window.frame) else { exit(4) }
    pop.addItems(withTitles: ["Apple", "Banana", "Cherry"])
    pop.setAccessibilityLabel("Fruit")
    pop.target = self; pop.action = #selector(changed); pop.menu?.delegate = self
    combo.addItems(withObjectValues: ["Red", "Blue", "Green", "Reject"])
    combo.setAccessibilityLabel("Color")
    combo.target = self; combo.action = #selector(changed)
    window.contentView!.addSubview(pop); window.contentView!.addSubview(combo)
    window.orderFrontRegardless(); changed()
    Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
      guard screen.frame.contains(self.window.frame), NSScreen.screens.contains(screen) else { exit(5) }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 55) { exit(0) }
  }
  @objc func changed() { if combo.stringValue == "Reject" { combo.stringValue = "Blue" }; window.title = "fruit=\(pop.titleOfSelectedItem ?? "") color=\(combo.stringValue) opens=\(opens)" }
  func menuWillOpen(_ menu: NSMenu) { opens += 1; changed(); DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { menu.cancelTrackingWithoutAnimation() } }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = Delegate(); app.delegate = delegate; app.run()
