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
// A pop-up that hands focus to `next` once it has taken its first key, as a
// form that moves on does. AppKit applies a type-ahead choice once, after
// the search, so a focus move on a new choice would come only after the
// last key: this one comes mid-title. `keys` counts the keys it took, and
// `took` writes the count into the title at once, action or not.
final class Jumpy: NSPopUpButton {
  weak var next: NSView?
  var keys = 0
  var took: (() -> Void)?
  override func keyDown(with event: NSEvent) {
    super.keyDown(with: event)
    keys += 1
    if keys == 1, let next { window?.makeFirstResponder(next) }
    took?()
  }
}
// A pop-up that never takes keyboard focus: keys sent to the app go elsewhere.
final class Unfocusable: NSPopUpButton {
  override var acceptsFirstResponder: Bool { false }
}

final class CustomDragBar: NSView {
  weak var stageScreen: NSScreen?
  var downScreenPt: NSPoint?
  var dragged = false
  override func mouseDown(with event: NSEvent) {
    downScreenPt = window?.convertPoint(toScreen: event.locationInWindow)
    dragged = false
  }
  override func mouseDragged(with event: NSEvent) {
    guard let start = downScreenPt,
          let cur = window?.convertPoint(toScreen: event.locationInWindow) else { return }
    if hypot(cur.x - start.x, cur.y - start.y) > 3 {
      dragged = true
    }
  }
  override func mouseUp(with event: NSEvent) {
    guard dragged, let win = window, let stage = stageScreen else { return }
    dragged = false
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) {
      win.setFrameOrigin(NSPoint(x: stage.frame.minX + 80, y: stage.frame.minY + 120))
    }
  }
}

final class TallToolbarView: NSView {
  weak var stageScreen: NSScreen?
  override func isAccessibilityElement() -> Bool { true }
  override func accessibilityRole() -> NSAccessibility.Role? { .toolbar }
  override func mouseDragged(with event: NSEvent) {
    guard let win = window, let stage = stageScreen else { return }
    win.setFrameOrigin(NSPoint(x: stage.frame.minX + 400, y: stage.frame.minY + 80))
  }
}

func findScreen(_ envKey: String) -> NSScreen? {
  guard let want = ProcessInfo.processInfo.environment[envKey] else { return nil }
  return NSScreen.screens.first {
    ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue == want
  }
}

final class DragDelegate: NSObject, NSApplicationDelegate {
  var customWin: NSWindow!
  var tallWin: NSWindow!
  var sheetHostWin: NSWindow!
  var attachedSheet: NSWindow!

  func applicationDidFinishLaunching(_ notification: Notification) {
    guard let screen = findScreen("MENU_DISPLAY_ID"),
          let stage = findScreen("MENU_STAGE_ID") else { exit(3) }

    let tallFrame = NSRect(
      x: screen.frame.minX + 300,
      y: screen.frame.maxY - 40 - 1300,
      width: 220,
      height: 1300
    )
    tallWin = NSWindow(
      contentRect: tallFrame,
      styleMask: [.titled, .fullSizeContentView],
      backing: .buffered,
      defer: false,
      screen: screen
    )
    tallWin.titlebarAppearsTransparent = true
    tallWin.titleVisibility = .hidden
    tallWin.minSize = NSSize(width: 220, height: 1300)
    tallWin.title = "TallDrag"
    tallWin.setFrame(tallFrame, display: true)
    let tallBar = TallToolbarView(frame: NSRect(x: 0, y: 1260, width: 220, height: 40))
    tallBar.stageScreen = stage
    tallWin.contentView!.addSubview(tallBar)
    tallWin.orderFrontRegardless()

    let customFrame = NSRect(
      x: screen.frame.minX + 40,
      y: screen.frame.maxY - 60 - 160,
      width: 220,
      height: 160
    )
    customWin = NSWindow(
      contentRect: customFrame,
      styleMask: [.titled, .fullSizeContentView],
      backing: .buffered,
      defer: false,
      screen: screen
    )
    customWin.titlebarAppearsTransparent = true
    customWin.titleVisibility = .hidden
    customWin.title = "CustomDrag"
    customWin.setFrame(customFrame, display: true)
    let bar = CustomDragBar(frame: NSRect(x: 0, y: 0, width: 220, height: 160))
    bar.stageScreen = stage
    customWin.contentView!.addSubview(bar)
    customWin.orderFrontRegardless()

    let sheetHostFrame = NSRect(
      x: screen.frame.minX + 550,
      y: screen.frame.maxY - 260 - 190,
      width: 230,
      height: 190
    )
    sheetHostWin = NSWindow(
      contentRect: sheetHostFrame,
      styleMask: [.titled, .closable],
      backing: .buffered,
      defer: false,
      screen: screen
    )
    sheetHostWin.title = "SheetDrag"
    sheetHostWin.setFrame(sheetHostFrame, display: true)
    attachedSheet = NSWindow(
      contentRect: NSRect(x: 0, y: 0, width: 170, height: 80),
      styleMask: [.titled],
      backing: .buffered,
      defer: true
    )
    sheetHostWin.orderFrontRegardless()

    DispatchQueue.main.asyncAfter(deadline: .now() + 0.35) {
      self.tallWin.setFrame(tallFrame, display: true)
      self.customWin.setFrame(customFrame, display: true)
      self.sheetHostWin.setFrame(sheetHostFrame, display: true)
      if self.attachedSheet.sheetParent == nil {
        self.sheetHostWin.beginSheet(self.attachedSheet, completionHandler: nil)
      }
      self.customWin.orderFrontRegardless()
    }

    Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
      if !NSScreen.screens.contains(screen) {
        exit(0)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 75) { exit(0) }
  }
}
final class Delegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
  var window: NSWindow!
  var sheet: NSWindow!
  var opens = 0
  var sheetOpens = 0
  var sheetMenu = 0
  var acts = 0
  let pop = NSPopUpButton(frame: NSRect(x: 30, y: 380, width: 210, height: 28), pullsDown: false)
  let openSheetBtn = NSButton(frame: NSRect(x: 250, y: 380, width: 95, height: 28))
  let sheetPop = NSPopUpButton(frame: NSRect(x: 20, y: 80, width: 200, height: 28), pullsDown: false)
  let closeSheetBtn = NSButton(frame: NSRect(x: 20, y: 24, width: 80, height: 28))
  let combo = SlowCombo(frame: NSRect(x: 30, y: 320, width: 220, height: 28))
  let locked = Unfocusable(frame: NSRect(x: 30, y: 260, width: 220, height: 28), pullsDown: false)
  // `acts` counts its actions, so a script can tell when they ran.
  let jump = Jumpy(frame: NSRect(x: 30, y: 200, width: 180, height: 28), pullsDown: false)
  let note = NSTextField(frame: NSRect(x: 220, y: 200, width: 120, height: 24))
  // A WebKit page's select: Agent Screen types into none.
  let web = WKWebView(frame: NSRect(x: 20, y: 20, width: 320, height: 160))
  func applicationDidFinishLaunching(_ notification: Notification) {
    guard let screen = NSScreen.screens.first(where: { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.stringValue == ProcessInfo.processInfo.environment["MENU_DISPLAY_ID"] }) else { exit(3) }
    window = NSWindow(contentRect: NSRect(x: 50, y: 100, width: 360, height: 430), styleMask: [.titled], backing: .buffered, defer: true, screen: screen)
    window.setFrameOrigin(NSPoint(x: screen.frame.minX + 50, y: screen.frame.minY + 100))
    guard screen.frame.contains(window.frame) else { exit(4) }
    pop.addItems(withTitles: ["Apple", "Banana", "Cherry", "Dragon fruit", "Dragon", "1", "10", "11", "中文"])
    pop.setAccessibilityLabel("Fruit")
    pop.target = self; pop.action = #selector(changed); pop.menu?.delegate = self
    openSheetBtn.title = "Open Sheet"; openSheetBtn.bezelStyle = .rounded
    openSheetBtn.setAccessibilityLabel("Open Sheet")
    openSheetBtn.target = self; openSheetBtn.action = #selector(openSheet)
    sheet = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 260, height: 130), styleMask: [.titled], backing: .buffered, defer: true)
    sheetPop.addItems(withTitles: ["Hourly", "Daily", "Weekly", "Manually"])
    sheetPop.setAccessibilityLabel("Frequency")
    sheetPop.target = self; sheetPop.action = #selector(changed); sheetPop.menu?.delegate = self
    closeSheetBtn.title = "Done"; closeSheetBtn.bezelStyle = .rounded
    closeSheetBtn.setAccessibilityLabel("Done")
    closeSheetBtn.target = self; closeSheetBtn.action = #selector(closeSheet)
    for view in [sheetPop, closeSheetBtn] as [NSView] { sheet.contentView!.addSubview(view) }
    combo.addItems(withObjectValues: ["Red", "Blue", "Green", "Reject", "Slow"])
    combo.setAccessibilityLabel("Color")
    combo.target = self; combo.action = #selector(changed)
    locked.addItems(withTitles: ["Apple", "Kiwi"])
    locked.setAccessibilityLabel("Locked")
    locked.target = self; locked.action = #selector(changed); locked.menu?.delegate = self
    jump.addItems(withTitles: ["Apple", "Banana", "Blueberry"])
    jump.next = note
    jump.took = { [unowned self] in self.changed() }
    jump.setAccessibilityLabel("Jump")
    jump.target = self; jump.action = #selector(jumped); jump.menu?.delegate = self
    note.setAccessibilityLabel("Note")
    NotificationCenter.default.addObserver(forName: NSControl.textDidChangeNotification, object: note, queue: .main) { _ in self.changed() }
    web.loadHTMLString("<select aria-label='Web fruit'><option>Apple</option><option>Kiwi</option></select>", baseURL: nil)
    for view in [pop, openSheetBtn, combo, locked, jump, note, web] as [NSView] { window.contentView!.addSubview(view) }
    window.orderFrontRegardless(); changed()
    Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
      guard screen.frame.contains(self.window.frame), NSScreen.screens.contains(screen) else { exit(5) }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 75) { exit(0) }
  }
  @objc func openSheet() {
    guard sheet.sheetParent == nil else { return }
    window.beginSheet(sheet, completionHandler: nil)
    changed()
  }
  @objc func closeSheet() {
    guard sheet.sheetParent != nil else { return }
    window.endSheet(sheet)
    changed()
  }
  @objc func changed() {
    if combo.stringValue == "Reject" { combo.stringValue = "Blue" }
    window.title = "fruit=\(pop.titleOfSelectedItem ?? "") color=\(combo.stringValue) locked=\(locked.titleOfSelectedItem ?? "") jump=\(jump.titleOfSelectedItem ?? "") acts=\(acts) keys=\(jump.keys) note=\(note.stringValue) opens=\(opens) sheet=\(sheetPop.titleOfSelectedItem ?? "") sheetOpens=\(sheetOpens) sheetMenu=\(sheetMenu)"
  }
  @objc func jumped() { acts += 1; changed() }
  func menuWillOpen(_ menu: NSMenu) {
    if menu === sheetPop.menu {
      sheetOpens += 1; sheetMenu = 1; changed()
      DispatchQueue.main.asyncAfter(deadline: .now() + 8.0) { menu.cancelTrackingWithoutAnimation() }
    } else {
      opens += 1; changed()
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { menu.cancelTrackingWithoutAnimation() }
    }
  }
  func menuDidClose(_ menu: NSMenu) {
    if menu === sheetPop.menu { sheetMenu = 0; changed() }
  }
}
let app = NSApplication.shared
app.setActivationPolicy(.regular)
if ProcessInfo.processInfo.environment["MENU_CUSTOM_DRAG"] == "1" {
  let delegate = DragDelegate()
  app.delegate = delegate
  app.run()
} else {
  let delegate = Delegate()
  app.delegate = delegate
  app.run()
}
