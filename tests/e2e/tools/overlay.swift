// The sign every test script shows on your screens, driven by lines on
// stdin:
//
//   count N TEXT   a big count from N in the middle, one a second, with
//                  TEXT below, in orange: hands off the mouse and keyboard
//   badge NAME     a small badge in the bottom-right corner showing NAME
//   step TEXT      what the script does now, on the badge
//   hide           take the sign off the screen, then print "hidden" once
//                  the window list no longer shows it
//   show           put it back
//   done KIND HEAD<TAB>DETAIL
//                  the result: KIND pass (green), fail (red) or skip
//                  (grey), with "可以操作了 / you can use the Mac" below;
//                  quits 3 s later, 6 s after a failure
//
// The count shows on every real screen, so you see it on the one you work
// on, the built-in one or not. The badge and the result show only on your
// screen: the one with the front app's window when the sign starts, or
// else the one with the pointer. Agent Screen's virtual displays and
// AeroSpace's closet don't count.
//
// It prints "ready" once it shows. It never becomes active or takes a
// click. It quits when stdin closes, except after `done`, which keeps it on
// screen for its few seconds.
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.accessory)

/// Not one of Agent Screen's displays (vendor 0x5353, as
/// `mac::cg::is_agent_screen`) or AeroSpace's closet (vendor 0x4145,
/// model 1).
func real(_ screen: NSScreen) -> Bool {
  let key = NSDeviceDescriptionKey("NSScreenNumber")
  guard let number = screen.deviceDescription[key] as? NSNumber else { return true }
  let id = CGDirectDisplayID(number.uint32Value)
  let vendor = CGDisplayVendorNumber(id)
  return vendor != 0x5353 && !(vendor == 0x4145 && CGDisplayModelNumber(id) == 1)
}

/// One screen's copy of the sign.
final class Copy {
  let screen: NSScreen
  let panel = NSPanel(
    contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
    defer: false)
  let box = NSView()
  let big = NSTextField(labelWithString: "")
  let line = NSTextField(wrappingLabelWithString: "")
  let foot = NSTextField(labelWithString: "")

  init(_ screen: NSScreen) {
    self.screen = screen
    panel.level = .screenSaver
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.ignoresMouseEvents = true
    panel.hidesOnDeactivate = false
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
    box.wantsLayer = true
    box.layer?.backgroundColor = NSColor(white: 0, alpha: 0.78).cgColor
    big.alignment = .center
    line.textColor = .white
    line.alignment = .center
    foot.font = .systemFont(ofSize: 13, weight: .semibold)
    foot.textColor = .systemGreen
    box.addSubview(big)
    box.addSubview(line)
    box.addSubview(foot)
    panel.contentView = box
  }

  /// The big sign in the middle of the screen.
  func large() {
    let size = NSSize(width: 560, height: 300)
    let f = screen.frame
    panel.setFrame(
      NSRect(x: f.midX - size.width / 2, y: f.midY - size.height / 2,
             width: size.width, height: size.height), display: true)
    box.layer?.cornerRadius = 28
    big.frame = NSRect(x: 0, y: 100, width: size.width, height: 170)
    line.frame = NSRect(x: 20, y: 20, width: size.width - 40, height: 80)
    line.font = .systemFont(ofSize: 22, weight: .medium)
    line.alignment = .center
    line.maximumNumberOfLines = 3
    line.lineBreakMode = .byWordWrapping
  }

  /// The badge in the bottom-right corner, clear of the menu bar, Peek, the
  /// cards under the menu bar icon and the middle of the screen, where
  /// alerts and windows open.
  func small(tall: Bool) {
    let size = NSSize(width: 360, height: tall ? 88 : 64)
    let f = screen.visibleFrame
    panel.setFrame(
      NSRect(x: f.maxX - size.width - 16, y: f.minY + 16, width: size.width, height: size.height),
      display: true)
    box.layer?.cornerRadius = 14
    let lift: CGFloat = tall ? 24 : 0
    foot.isHidden = !tall
    foot.frame = NSRect(x: 14, y: 8, width: size.width - 28, height: 20)
    big.frame = NSRect(x: 14, y: 34 + lift, width: size.width - 28, height: 22)
    big.font = .systemFont(ofSize: 15, weight: .bold)
    big.alignment = .left
    line.frame = NSRect(x: 14, y: 8 + lift, width: size.width - 28, height: 24)
    line.font = .systemFont(ofSize: 12, weight: .regular)
    line.alignment = .left
    line.maximumNumberOfLines = 1
    line.lineBreakMode = .byTruncatingTail
  }
}

let shown = NSScreen.screens.filter(real)
let copies = (shown.isEmpty ? Array(NSScreen.screens.prefix(1)) : shown).map(Copy.init)

func displayID(_ screen: NSScreen) -> CGDirectDisplayID? {
  let key = NSDeviceDescriptionKey("NSScreenNumber")
  return (screen.deviceDescription[key] as? NSNumber).map { CGDirectDisplayID($0.uint32Value) }
}

/// The copy on your screen: the one under the front app's top window, else
/// the one under the pointer, else the first.
func yours() -> Copy {
  let at = { (point: CGPoint) in
    copies.first { c in displayID(c.screen).map { CGDisplayBounds($0).contains(point) } ?? false }
  }
  if let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements],
      kCGNullWindowID) as? [[String: Any]] ?? []
    for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid
      && (w[kCGWindowLayer as String] as? Int) == 0 {
      if let b = w[kCGWindowBounds as String] as? [String: Double],
        let copy = at(CGPoint(x: (b["X"] ?? 0) + (b["Width"] ?? 0) / 2,
                              y: (b["Y"] ?? 0) + (b["Height"] ?? 0) / 2)) {
        return copy
      }
    }
  }
  return CGEvent(source: nil).flatMap { at($0.location) } ?? copies[0]
}

var home = yours()
/// The count shows on every copy; the badge only on `home`.
var wide = true

var name = ""
var hidden = false
var finished = false

func large() {
  wide = true
  copies.forEach { $0.large() }
  if !hidden { show() }
}
func small(tall: Bool = false) {
  wide = false
  copies.forEach { $0.small(tall: tall) }
  if !hidden { show() }
}

/// The big line, the line below it and the foot, on every copy.
func say(big: String, _ color: NSColor, line: String, foot: String? = nil) {
  for c in copies {
    c.big.textColor = color
    c.big.stringValue = big
    c.line.stringValue = line
    if let foot { c.foot.stringValue = foot }
  }
}

var counting = 0
func count(_ n: Int, _ color: NSColor, _ text: String) {
  counting += 1
  let mine = counting
  large()
  for c in copies { c.foot.isHidden = true }
  func tick(_ left: Int) {
    guard mine == counting, left > 0 else { return }
    for c in copies {
      c.big.font = .monospacedDigitSystemFont(ofSize: 140, weight: .bold)
      c.big.alignment = .center
    }
    say(big: "\(left)", color, line: text)
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { tick(left - 1) }
  }
  tick(n)
}

func badge(_ head: String, _ color: NSColor, _ text: String) {
  counting += 1
  small()
  say(big: head, color, line: text)
}

/// Off the screen, then "hidden" on stdout once WindowServer stops listing
/// any copy, so a capture taken next can't show it.
func hide() {
  hidden = true
  copies.forEach { $0.panel.orderOut(nil) }
  let numbers = Set(copies.map { $0.panel.windowNumber })
  func check(_ tries: Int) {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
      as? [[String: Any]] ?? []
    let shown = list.contains { numbers.contains(($0[kCGWindowNumber as String] as? Int) ?? -1) }
    if shown && tries > 0 {
      DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { check(tries - 1) }
      return
    }
    // One more frame for the compositor.
    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
      print("hidden")
      fflush(stdout)
    }
  }
  check(100)
}

func show() {
  hidden = false
  for c in copies {
    if wide || c === home { c.panel.orderFrontRegardless() } else { c.panel.orderOut(nil) }
  }
}

func perform(_ raw: Substring) {
  let parts = raw.split(separator: " ", maxSplits: 2).map(String.init)
  let rest = { (from: Int) in parts.count > from ? parts[from] : "" }
  switch parts.first {
  case "count": count(Int(rest(1)) ?? 3, .systemOrange, rest(2))
  case "badge":
    name = String(raw.dropFirst(6))
    badge("测试中 " + name, .systemOrange, "preparing, you can keep working / 准备中，可以继续操作")
  case "step": badge("测试中 " + name, .systemOrange, String(raw.dropFirst(5)))
  case "hide": hide()
  case "show": show()
  case "done":
    finished = true
    let kind = rest(1)
    let words = rest(2).split(separator: "\t", maxSplits: 1).map(String.init)
    let color: NSColor =
      kind == "pass" ? .systemGreen : kind == "fail" ? .systemRed : .systemGray
    counting += 1
    home = yours()
    small(tall: true)
    say(
      big: words.first ?? "", color, line: words.count > 1 ? words[1] : "",
      foot: "可以操作了 / you can use the Mac")
    show()
    DispatchQueue.main.asyncAfter(deadline: .now() + (kind == "fail" ? 6 : 3)) { exit(0) }
  default: break
  }
}

// Lines can arrive split across reads: only whole ones run.
var pending = ""
FileHandle.standardInput.readabilityHandler = { input in
  let data = input.availableData
  if data.isEmpty { input.readabilityHandler = nil }
  let text = String(decoding: data, as: UTF8.self)
  DispatchQueue.main.async {
    if data.isEmpty {
      if !finished { exit(0) }
      return
    }
    pending += text
    while let end = pending.firstIndex(of: "\n") {
      let raw = pending[..<end]
      pending = String(pending[pending.index(after: end)...])
      perform(raw)
    }
  }
}
// The badge first: nothing in the middle until a count.
small()
// "ready" once the run loop turns, so the count starts on screen.
DispatchQueue.main.async {
  print("ready")
  fflush(stdout)
}
app.run()
