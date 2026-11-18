// The user's side of the demo, drawn on a stage display the real user
// never sees: a notes panel and a pointer. Commands come on stdin, one per
// line, and each prints "done" when finished:
//
//     type TEXT      type into the notes, a character at a time
//     move X Y       glide the pointer's tip there, global top-left points
//
// A utility panel, so AeroSpace floats it instead of tiling it.
//
//     notes X Y W H      the panel's top-left corner and size in global points
import AppKit

let a = CommandLine.arguments.dropFirst().compactMap(Double.init)
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let primary = NSScreen.screens[0].frame.height
/// Global top-left points to Cocoa's bottom-left.
func cocoa(_ x: Double, _ y: Double, _ h: Double) -> NSPoint { NSPoint(x: x, y: primary - y - h) }

let panel = NSPanel(contentRect: NSRect(origin: cocoa(a[0], a[1], a[3]), size: NSSize(width: a[2], height: a[3])),
                    styleMask: [.titled, .utilityWindow], backing: .buffered, defer: false)
panel.title = "Your notes"
// Above the Viewer, which floats too.
panel.level = NSWindow.Level(rawValue: NSWindow.Level.floating.rawValue + 1)
panel.hidesOnDeactivate = false
panel.setFrame(NSRect(origin: cocoa(a[0], a[1], a[3]), size: NSSize(width: a[2], height: a[3])), display: true)
let text = NSTextView(frame: panel.contentView!.bounds.insetBy(dx: 14, dy: 10))
text.font = .systemFont(ofSize: 20)
text.autoresizingMask = [.width, .height]
text.drawsBackground = false
panel.contentView!.addSubview(text)
panel.orderFrontRegardless()

let arrow = NSCursor.arrow.image
let tip = NSCursor.arrow.hotSpot
let pointer = NSPanel(contentRect: NSRect(origin: .zero, size: arrow.size), styleMask: [.borderless, .nonactivatingPanel],
                      backing: .buffered, defer: false)
pointer.level = .screenSaver
pointer.isOpaque = false
pointer.backgroundColor = .clear
pointer.hasShadow = false
pointer.ignoresMouseEvents = true
pointer.contentView = NSImageView(image: arrow)
var at = NSPoint(x: a[0] + a[2] / 2, y: a[1] + a[3] / 2)
func place(_ p: NSPoint) {
  pointer.setFrameOrigin(cocoa(p.x - tip.x, p.y - tip.y, arrow.size.height))
}
place(at)
pointer.orderFrontRegardless()

func done() { print("done"); fflush(stdout) }

func run(_ line: String) {
  let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
  switch parts.first {
  case "type":
    let chars = Array(parts.count > 1 ? parts[1] : "")
    var i = 0
    Timer.scheduledTimer(withTimeInterval: 0.07, repeats: true) { timer in
      guard i < chars.count else { timer.invalidate(); done(); return }
      text.textStorage?.append(NSAttributedString(string: String(chars[i]),
                                                  attributes: [.font: text.font!, .foregroundColor: NSColor.labelColor]))
      i += 1
    }
  case "move":
    let xy = (parts.count > 1 ? parts[1] : "").split(separator: " ").compactMap { Double($0) }
    let from = at, to = NSPoint(x: xy[0], y: xy[1])
    var step = 0
    Timer.scheduledTimer(withTimeInterval: 1.0 / 60, repeats: true) { timer in
      step += 1
      let t = Double(step) / 30, e = 1 - pow(1 - t, 3)
      at = NSPoint(x: from.x + (to.x - from.x) * e, y: from.y + (to.y - from.y) * e)
      place(at)
      if step >= 30 { timer.invalidate(); done() }
    }
  default:
    done()
  }
}

Thread {
  while let line = readLine() {
    DispatchQueue.main.async { run(line) }
  }
  DispatchQueue.main.async { exit(0) }
}.start()
print("ready"); fflush(stdout)
app.run()
