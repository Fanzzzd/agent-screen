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
  var dragged = false
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
  override func mouseDown(with event: NSEvent) {
    dragged = false
  }
  override func mouseDragged(with event: NSEvent) {
    dragged = true
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
  override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
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

func spawnAppexPane(
  screen: NSScreen,
  frame: NSRect,
  statePath: String
) -> (NSWindow, Process)? {
  guard let execPath = Bundle.main.executablePath else { return nil }
  let appexExec = (Bundle.main.bundlePath as NSString)
    .appendingPathComponent("Contents/PlugIns/MenuPane.appex/Contents/MacOS/MenuPane")
  let appexDir = (appexExec as NSString).deletingLastPathComponent
  try? FileManager.default.createDirectory(atPath: appexDir, withIntermediateDirectories: true)
  if FileManager.default.fileExists(atPath: appexExec) {
    try? FileManager.default.removeItem(atPath: appexExec)
  }
  try? FileManager.default.copyItem(atPath: execPath, toPath: appexExec)

  let host = NSWindow(
    contentRect: frame,
    styleMask: [.borderless],
    backing: .buffered,
    defer: false,
    screen: screen
  )
  host.setFrame(frame, display: true)
  host.level = .floating
  host.isOpaque = false
  host.backgroundColor = NSColor(white: 1.0, alpha: 0.02)
  host.title = "PaneHost"
  host.orderFrontRegardless()

  let proc = Process()
  proc.executableURL = URL(fileURLWithPath: appexExec)
  var env = ProcessInfo.processInfo.environment
  env["MENU_PANE_MODE"] = "1"
  env["MENU_HOST_WID"] = String(host.windowNumber)
  env["MENU_PANE_X"] = String(Double(frame.origin.x))
  env["MENU_PANE_Y"] = String(Double(frame.origin.y))
  env["MENU_PANE_W"] = String(Double(frame.size.width))
  env["MENU_PANE_H"] = String(Double(frame.size.height))
  env["MENU_PANE_STATE"] = statePath
  proc.environment = env
  proc.terminationHandler = { _ in
    DispatchQueue.main.async {
      host.orderOut(nil)
    }
  }
  try? proc.run()
  return (host, proc)
}

final class PaneDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
  var paneWin: NSWindow!
  var opens = 0
  var menuOpen = 0
  let pop = NSPopUpButton(frame: NSRect(x: 20, y: 32, width: 190, height: 28), pullsDown: false)
  let ppid = getppid()

  func applicationDidFinishLaunching(_ notification: Notification) {
    let env = ProcessInfo.processInfo.environment
    guard let screen = findScreen("MENU_DISPLAY_ID"),
          let x = env["MENU_PANE_X"].flatMap(Double.init),
          let y = env["MENU_PANE_Y"].flatMap(Double.init),
          let w = env["MENU_PANE_W"].flatMap(Double.init),
          let h = env["MENU_PANE_H"].flatMap(Double.init) else { exit(3) }
    let frame = NSRect(x: x, y: y, width: w, height: h)
    let panel = NSPanel(
      contentRect: NSRect(x: x, y: y, width: w, height: max(60, h - 28)),
      styleMask: [.titled, .nonactivatingPanel],
      backing: .buffered,
      defer: false
    )
    panel.becomesKeyOnlyIfNeeded = false
    panel.level = .floating
    panel.setFrame(frame, display: true)
    paneWin = panel
    guard screen.frame.contains(paneWin.frame) else { exit(4) }
    pop.addItems(withTitles: ["Hourly", "Daily", "Weekly", "Manually"])
    pop.setAccessibilityLabel("PaneFreq")
    pop.target = self
    pop.action = #selector(changed)
    pop.menu?.delegate = self
    paneWin.contentView!.addSubview(pop)
    let hostWid = env["MENU_HOST_WID"].flatMap(Int.init) ?? 0
    if hostWid > 0 {
      paneWin.order(.below, relativeTo: hostWid)
    } else {
      paneWin.orderFrontRegardless()
    }
    changed()
    Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
      if getppid() == 1
        || kill(self.ppid, 0) != 0
        || !NSScreen.screens.contains(screen)
        || !screen.frame.contains(self.paneWin.frame)
      {
        exit(0)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 75) { exit(0) }
  }

  @objc func changed() {
    let state = "pane=\(pop.titleOfSelectedItem ?? "") paneOpens=\(opens) paneMenu=\(menuOpen)"
    paneWin.title = state
    if let path = ProcessInfo.processInfo.environment["MENU_PANE_STATE"] {
      try? state.write(toFile: path, atomically: true, encoding: .utf8)
    }
  }

  func menuWillOpen(_ menu: NSMenu) {
    opens += 1
    menuOpen = 1
    changed()
    DispatchQueue.main.asyncAfter(deadline: .now() + 8.0) {
      menu.cancelTrackingWithoutAnimation()
    }
  }

  func menuDidClose(_ menu: NSMenu) {
    menuOpen = 0
    changed()
  }
}

final class DragDelegate: NSObject, NSApplicationDelegate {
  var customWin: NSWindow!
  var tallWin: NSWindow!
  var sheetHostWin: NSWindow!
  var attachedSheet: NSWindow!
  var paneHost: NSWindow?
  var paneProc: Process?

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
      styleMask: [.titled, .closable, .fullSizeContentView],
      backing: .buffered,
      defer: false,
      screen: screen
    )
    tallWin.titlebarAppearsTransparent = true
    tallWin.titleVisibility = .hidden
    tallWin.minSize = NSSize(width: 220, height: 1300)
    tallWin.title = "TallDrag"
    tallWin.setFrame(tallFrame, display: true)
    let tallBar = TallToolbarView(frame: NSRect(x: 0, y: 1248, width: 220, height: 52))
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

    for delay in [0.35, 0.95] {
      DispatchQueue.main.asyncAfter(deadline: .now() + delay) {
        self.tallWin.setFrame(tallFrame, display: true)
        self.customWin.setFrame(customFrame, display: true)
        self.sheetHostWin.setFrame(sheetHostFrame, display: true)
        if self.attachedSheet.sheetParent == nil {
          self.sheetHostWin.beginSheet(self.attachedSheet, completionHandler: nil)
        }
        self.customWin.orderFrontRegardless()
      }
    }

    if let statePath = ProcessInfo.processInfo.environment["MENU_PANE_STATE"] {
      let paneFrame = NSRect(
        x: screen.frame.minX + 610,
        y: screen.frame.maxY - 60 - 120,
        width: 240,
        height: 120
      )
      if let (host, proc) = spawnAppexPane(screen: screen, frame: paneFrame, statePath: statePath) {
        paneHost = host
        paneProc = proc
      }
    }

    Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
      if !NSScreen.screens.contains(screen) {
        self.paneProc?.terminate()
        exit(0)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 75) {
      self.paneProc?.terminate()
      exit(0)
    }
  }
}
final class Delegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
  var window: NSWindow!
  var sheet: NSWindow!
  var paneHost: NSWindow?
  var paneProc: Process?
  var opens = 0
  var sheetOpens = 0
  var sheetMenu = 0
  var acts = 0
  let pop = NSPopUpButton(frame: NSRect(x: 30, y: 380, width: 210, height: 28), pullsDown: false)
  let openSheetBtn = NSButton(frame: NSRect(x: 250, y: 380, width: 95, height: 28))
  let openPaneBtn = NSButton(frame: NSRect(x: 250, y: 320, width: 95, height: 28))
  let sheetPop = NSPopUpButton(frame: NSRect(x: 20, y: 80, width: 200, height: 28), pullsDown: false)
  let closeSheetBtn = NSButton(frame: NSRect(x: 20, y: 24, width: 80, height: 28))
  let combo = SlowCombo(frame: NSRect(x: 30, y: 320, width: 210, height: 28))
  let locked = Unfocusable(frame: NSRect(x: 30, y: 260, width: 220, height: 28), pullsDown: false)
  // `acts` counts its actions, so a script can tell when they ran.
  let jump = Jumpy(frame: NSRect(x: 30, y: 200, width: 180, height: 28), pullsDown: false)
  let note = NSTextField(frame: NSRect(x: 220, y: 200, width: 120, height: 24))
  // A WebKit page's select: Agent Screen types into none.
  let web = WKWebView(frame: NSRect(x: 20, y: 20, width: 320, height: 160))
  var hostScreen: NSScreen!
  func applicationDidFinishLaunching(_ notification: Notification) {
    guard let screen = findScreen("MENU_DISPLAY_ID") else { exit(3) }
    hostScreen = screen
    window = NSWindow(contentRect: NSRect(x: 50, y: 100, width: 360, height: 430), styleMask: [.titled], backing: .buffered, defer: true, screen: screen)
    window.setFrameOrigin(NSPoint(x: screen.frame.minX + 50, y: screen.frame.minY + 100))
    guard screen.frame.contains(window.frame) else { exit(4) }
    pop.addItems(withTitles: ["Apple", "Banana", "Cherry", "Dragon fruit", "Dragon", "1", "10", "11", "中文"])
    pop.setAccessibilityLabel("Fruit")
    pop.target = self; pop.action = #selector(changed); pop.menu?.delegate = self
    openSheetBtn.title = "Open Sheet"; openSheetBtn.bezelStyle = .rounded
    openSheetBtn.setAccessibilityLabel("Open Sheet")
    openSheetBtn.target = self; openSheetBtn.action = #selector(openSheet)
    openPaneBtn.title = "Open Pane"; openPaneBtn.bezelStyle = .rounded
    openPaneBtn.setAccessibilityLabel("Open Pane")
    openPaneBtn.target = self; openPaneBtn.action = #selector(openPane)
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
    for view in [pop, openSheetBtn, openPaneBtn, combo, locked, jump, note, web] as [NSView] { window.contentView!.addSubview(view) }
    window.orderFrontRegardless(); changed()
    Timer.scheduledTimer(withTimeInterval: 0.02, repeats: true) { _ in
      guard screen.frame.contains(self.window.frame), NSScreen.screens.contains(screen) else {
        self.paneProc?.terminate()
        exit(5)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 75) {
      self.paneProc?.terminate()
      exit(0)
    }
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
  @objc func openPane() {
    guard paneHost == nil,
          let statePath = ProcessInfo.processInfo.environment["MENU_PANE_STATE"] else { return }
    let paneFrame = NSRect(
      x: hostScreen.frame.minX + 520,
      y: hostScreen.frame.maxY - 120 - 120,
      width: 240,
      height: 120
    )
    if let (host, proc) = spawnAppexPane(screen: hostScreen, frame: paneFrame, statePath: statePath) {
      paneHost = host
      paneProc = proc
    }
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
let env = ProcessInfo.processInfo.environment
if env["MENU_PANE_MODE"] == "1" {
  app.setActivationPolicy(.accessory)
  let delegate = PaneDelegate()
  app.delegate = delegate
  app.run()
} else if env["MENU_CUSTOM_DRAG"] == "1" {
  app.setActivationPolicy(.regular)
  let delegate = DragDelegate()
  app.delegate = delegate
  app.run()
} else {
  app.setActivationPolicy(.regular)
  let delegate = Delegate()
  app.delegate = delegate
  app.run()
}
