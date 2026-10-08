// Real pointer input for the end-to-end tests, posted at the HID level so it
// travels the same path as the user's. Every command puts the cursor back.
//
//   hid hover X Y [PNG X,Y,W,H] glide there, stay 0.5 s, capture the region while there
//   hid click X Y              left click
//   hid scroll X Y STEPS       a trackpad gesture: began, STEPS changes, ended
//   hid wheel X Y TICKS        mouse wheel ticks, no phases
//   hid replay LOG FROM TO X Y replay scroll events recorded by `probe tap`
//   hid key CODE               press and release a key, the pointer untouched
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let saved = CGEvent(source: nil)!.location

func post(_ type: CGEventType, _ p: CGPoint) {
  CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: p, mouseButton: .left)!.post(tap: .cghidEventTap)
}

/// Glide in small steps, as a hand would, so each window on the way sees the pointer.
func glide(to p: CGPoint) {
  for i in 1...20 {
    let t = Double(i) / 20
    post(.mouseMoved, CGPoint(x: saved.x + (p.x - saved.x) * t, y: saved.y + (p.y - saved.y) * t))
    usleep(10_000)
  }
  usleep(250_000)
}

func scrollEvent(_ p: CGPoint, continuous: Bool, phase: Int64, momentum: Int64, lines: Int32, points: Double) {
  let e = CGEvent(scrollWheelEvent2Source: nil, units: continuous ? .pixel : .line, wheelCount: 1, wheel1: lines, wheel2: 0, wheel3: 0)!
  e.location = p
  e.setIntegerValueField(.scrollWheelEventIsContinuous, value: continuous ? 1 : 0)
  e.setIntegerValueField(.scrollWheelEventScrollPhase, value: phase)
  e.setIntegerValueField(.scrollWheelEventMomentumPhase, value: momentum)
  if continuous { e.setDoubleValueField(.scrollWheelEventPointDeltaAxis1, value: points) }
  e.post(tap: .cghidEventTap)
}

func field(_ line: Substring, _ key: String) -> String {
  for part in line.split(separator: " ") where part.hasPrefix(key + "=") { return String(part.dropFirst(key.count + 1)) }
  return "0"
}

if args[1] == "key" {
  let code = CGKeyCode(Int(args[2])!)
  CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)!.post(tap: .cghidEventTap)
  usleep(30_000)
  CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)!.post(tap: .cghidEventTap)
  exit(0)
}

let p = CGPoint(x: Double(args[args[1] == "replay" ? 5 : 2])!, y: Double(args[args[1] == "replay" ? 6 : 3])!)
glide(to: p)
switch args[1] {
case "hover":
  usleep(500_000)
  if args.count > 5 {
    let capture = Process()
    capture.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
    capture.arguments = ["-x", "-R", args[5], args[4]]
    try! capture.run(); capture.waitUntilExit()
  }
case "click":
  post(.leftMouseDown, p); usleep(40_000); post(.leftMouseUp, p); usleep(200_000)
case "scroll":
  let steps = Int(args[4])!
  scrollEvent(p, continuous: true, phase: 128, momentum: 0, lines: 0, points: 0)
  for i in 0..<steps {
    scrollEvent(p, continuous: true, phase: i == 0 ? 1 : 2, momentum: 0, lines: -3, points: -30)
    usleep(16_000)
  }
  scrollEvent(p, continuous: true, phase: 4, momentum: 0, lines: 0, points: 0)
  usleep(400_000)
case "wheel":
  for _ in 0..<Int(args[4])! {
    scrollEvent(p, continuous: false, phase: 0, momentum: 0, lines: -3, points: 0)
    usleep(60_000)
  }
  usleep(400_000)
case "replay":
  let from = Double(args[3])!, to = Double(args[4])!
  var last: Double? = nil
  for line in try! String(contentsOfFile: args[2], encoding: .utf8).split(separator: "\n") where line.contains("type=22") {
    let t = Double(line.split(separator: " ")[0])!
    if t < from || t > to { continue }
    if let l = last { usleep(useconds_t(max(0, t - l) * 1_000_000)) }
    last = t
    scrollEvent(p, continuous: field(line, "cont") == "1", phase: Int64(field(line, "phase"))!,
                momentum: Int64(field(line, "mom"))!, lines: Int32(field(line, "d1"))!, points: Double(field(line, "pd1"))!)
  }
  usleep(400_000)
default:
  print("unknown command \(args[1])"); exit(2)
}
post(.mouseMoved, saved)
CGWarpMouseCursorPosition(saved)
