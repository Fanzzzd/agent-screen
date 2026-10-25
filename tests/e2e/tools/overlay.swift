// A sign in the middle of the main screen for a test that moves the user's
// pointer or takes the keyboard, driven by lines on stdin:
//
//   count N TEXT   count down from N, one a second, with TEXT below
//   step TEXT      what the test is doing now
//   done TEXT      green: the user has the machine back; quits 2 s later
//
// It never becomes active or takes a click. It quits when stdin closes.
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let screen = NSScreen.screens.first!.frame
let size = NSSize(width: 520, height: 300)
let panel = NSPanel(
  contentRect: NSRect(x: screen.midX - size.width / 2, y: screen.midY - size.height / 2,
                      width: size.width, height: size.height),
  styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
panel.level = .screenSaver
panel.isOpaque = false
panel.backgroundColor = .clear
panel.ignoresMouseEvents = true
panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
let box = NSView(frame: NSRect(origin: .zero, size: size))
box.wantsLayer = true
box.layer?.backgroundColor = NSColor(white: 0, alpha: 0.78).cgColor
box.layer?.cornerRadius = 28
let big = NSTextField(labelWithString: "")
big.font = .monospacedDigitSystemFont(ofSize: 140, weight: .bold)
big.alignment = .center
big.frame = NSRect(x: 0, y: 100, width: size.width, height: 170)
let line = NSTextField(wrappingLabelWithString: "")
line.font = .systemFont(ofSize: 22, weight: .medium)
line.textColor = .white
line.alignment = .center
line.frame = NSRect(x: 20, y: 20, width: size.width - 40, height: 80)
box.addSubview(big)
box.addSubview(line)
panel.contentView = box

var counting = 0
func count(_ n: Int, _ text: String) {
  counting += 1
  let mine = counting
  func tick(_ left: Int) {
    guard mine == counting, left > 0 else { return }
    big.font = .monospacedDigitSystemFont(ofSize: 140, weight: .bold)
    big.textColor = .systemOrange
    big.stringValue = "\(left)"
    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { tick(left - 1) }
  }
  line.stringValue = text
  tick(n)
}
func show(_ head: String, _ color: NSColor, _ text: String) {
  counting += 1
  big.font = .systemFont(ofSize: 64, weight: .bold)
  big.textColor = color
  big.stringValue = head
  line.stringValue = text
}

FileHandle.standardInput.readabilityHandler = { handle in
  let data = handle.availableData
  let text = String(decoding: data, as: UTF8.self)
  DispatchQueue.main.async {
    if data.isEmpty { exit(0) }
    for raw in text.split(separator: "\n") {
      let parts = raw.split(separator: " ", maxSplits: 2).map(String.init)
      switch parts.first {
      case "count": count(Int(parts.count > 1 ? parts[1] : "3") ?? 3, parts.count > 2 ? parts[2] : "")
      case "step": show("测试中", .systemOrange, raw.dropFirst(5).description)
      case "done":
        show("可以操作了", .systemGreen, raw.dropFirst(5).description)
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { exit(0) }
      default: break
      }
    }
  }
}
panel.orderFrontRegardless()
app.run()
