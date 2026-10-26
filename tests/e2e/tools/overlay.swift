// The sign every test script shows on the main screen, driven by lines on
// stdin:
//
//   count N TEXT   a big count from N in the middle, one a second, with
//                  TEXT below, in orange: hands off the mouse and keyboard
//   calm N TEXT    the same in blue, for a script that leaves them alone
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
// It prints "ready" once it shows. It never becomes active or takes a click. It quits when stdin closes,
// except after `done`, which keeps it on screen for its few seconds.
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screen = NSScreen.screens.first!
let panel = NSPanel(
  contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered,
  defer: false)
panel.level = .screenSaver
panel.isOpaque = false
panel.backgroundColor = .clear
panel.ignoresMouseEvents = true
panel.hidesOnDeactivate = false
panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
let box = NSView()
box.wantsLayer = true
box.layer?.backgroundColor = NSColor(white: 0, alpha: 0.78).cgColor
let big = NSTextField(labelWithString: "")
big.alignment = .center
let line = NSTextField(wrappingLabelWithString: "")
line.textColor = .white
line.alignment = .center
let foot = NSTextField(labelWithString: "")
foot.font = .systemFont(ofSize: 13, weight: .semibold)
foot.textColor = .systemGreen
box.addSubview(big)
box.addSubview(line)
box.addSubview(foot)
panel.contentView = box

var name = ""
var hidden = false
var finished = false

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
}

/// The badge in the bottom-right corner, clear of the menu bar, Peek, the
/// cards under the menu bar icon and the middle of the screen, where
/// alerts and windows open.
func small(tall: Bool = false) {
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

var counting = 0
func count(_ n: Int, _ color: NSColor, _ text: String) {
  counting += 1
  let mine = counting
  large()
  foot.isHidden = true
  func tick(_ left: Int) {
    guard mine == counting, left > 0 else { return }
    big.font = .monospacedDigitSystemFont(ofSize: 140, weight: .bold)
    big.alignment = .center
    big.textColor = color
    big.stringValue = "\(left)"
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { tick(left - 1) }
  }
  line.stringValue = text
  tick(n)
}

func badge(_ head: String, _ color: NSColor, _ text: String) {
  counting += 1
  small()
  big.textColor = color
  big.stringValue = head
  line.stringValue = text
}

/// Off the screen, then "hidden" on stdout once WindowServer stops listing
/// the panel, so a capture taken next can't show it.
func hide() {
  hidden = true
  panel.orderOut(nil)
  let number = panel.windowNumber
  func check(_ tries: Int) {
    let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
      as? [[String: Any]] ?? []
    let shown = list.contains { ($0[kCGWindowNumber as String] as? Int) == number }
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
  panel.orderFrontRegardless()
}

func perform(_ raw: Substring) {
  let parts = raw.split(separator: " ", maxSplits: 2).map(String.init)
  let rest = { (from: Int) in parts.count > from ? parts[from] : "" }
  switch parts.first {
  case "count": count(Int(rest(1)) ?? 3, .systemOrange, rest(2))
  case "calm": count(Int(rest(1)) ?? 3, .systemBlue, rest(2))
  case "badge":
    name = String(raw.dropFirst(6))
    badge("测试中 " + name, .systemOrange, "starting / 准备中")
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
    small(tall: true)
    big.textColor = color
    big.stringValue = words.first ?? ""
    line.stringValue = words.count > 1 ? words[1] : ""
    foot.stringValue = "可以操作了 / you can use the Mac"
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
large()
panel.orderFrontRegardless()
// "ready" once the run loop turns, so the count starts on screen.
DispatchQueue.main.async {
  print("ready")
  fflush(stdout)
}
app.run()
