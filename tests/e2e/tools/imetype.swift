// Real keys through the user's own input method, for `viewerwetype`. It
// posts key events at the HID tap, as a keyboard does, so they reach the
// key window through the selected input method, and lists the input
// method's windows on screen every 30 ms meanwhile.
//
//   imetype OUT STEP...
//
// A STEP is  k:CODE  press and release key CODE
//            w:MS    wait MS
//            m:NAME  write a mark line
//
// Lines of OUT are JSON, each with "t", milliseconds since 1970:
//   {"source":ID,"pids":[..],"tap":true}   the input source, its processes,
//                                          and whether it can see your input
//   {"key":CODE}                           a key went down
//   {"mark":NAME}
//   {"windows":[{"id","layer","x","y","w","h"}..]}   those on screen now
//   {"abort":WHY}
//
// When you touch the keyboard, the pointer or the trackpad, it stops,
// presses Escape so a composition under way is cancelled, and exits 3.
import AppKit
import Carbon
import CoreGraphics

let args = CommandLine.arguments
guard args.count > 2 else { print("usage: imetype OUT STEP..."); exit(2) }
FileManager.default.createFile(atPath: args[1], contents: nil)
let out = FileHandle(forWritingAtPath: args[1])!
let outLock = NSLock()
@Sendable func now() -> Int { Int(Date().timeIntervalSince1970 * 1000) }
func line(_ fields: [String: Any]) {
  var fields = fields
  fields["t"] = now()
  guard let data = try? JSONSerialization.data(withJSONObject: fields) else { return }
  outLock.lock()
  out.write(data)
  out.write("\n".data(using: .utf8)!)
  outLock.unlock()
}

let source = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
func property(_ key: CFString) -> String {
  guard let value = TISGetInputSourceProperty(source, key) else { return "" }
  return Unmanaged<CFString>.fromOpaque(value).takeUnretainedValue() as String
}
let bundle = property(kTISPropertyBundleID)
let pids = Set(NSRunningApplication.runningApplications(withBundleIdentifier: bundle).map { $0.processIdentifier })

// Your own input: hardware events carry no process.
var posted = 0
let own = Int64(getpid())
let start = CGEvent(source: nil)!.location
func stop(_ why: String) -> Never {
  line(["abort": why])
  if posted > 0 { press(53) }
  exit(3)
}
let mask: CGEventMask = [CGEventType.keyDown, .keyUp, .flagsChanged, .leftMouseDown, .rightMouseDown,
                         .otherMouseDown, .mouseMoved, .scrollWheel]
  .reduce(0) { $0 | (1 << CGEventMask($1.rawValue)) }
let tap = CGEvent.tapCreate(
  tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly, eventsOfInterest: mask,
  callback: { _, type, event, _ in
    let from = event.getIntegerValueField(.eventSourceUnixProcessID)
    if from == 0 {
      // The pointer resting on a trackpad still reports a point or two.
      let far = abs(event.location.x - start.x) > 4 || abs(event.location.y - start.y) > 4
      if type != .mouseMoved || far { stop("you used the keyboard or pointer (event type \(type.rawValue))") }
    }
    return Unmanaged.passUnretained(event)
  }, userInfo: nil)
if let tap {
  CFRunLoopAddSource(CFRunLoopGetMain(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
  CGEvent.tapEnable(tap: tap, enable: true)
}
line(["source": property(kTISPropertyInputSourceID), "pids": pids.map { Int($0) }, "tap": tap != nil])

func press(_ code: CGKeyCode) {
  CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: true)!.post(tap: .cghidEventTap)
  usleep(12_000)
  CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: false)!.post(tap: .cghidEventTap)
}

/// The input method's windows. Listing every window takes too long to do
/// each time, so one full list finds theirs, then they are asked for by
/// number, and a list of what is on screen finds new ones.
var known = Set<CGWindowID>()
func find(_ option: CGWindowListOption) {
  let list = CGWindowListCopyWindowInfo([option], kCGNullWindowID) as? [[String: Any]] ?? []
  for w in list {
    if let pid = w[kCGWindowOwnerPID as String] as? Int32, pids.contains(pid),
       let id = w[kCGWindowNumber as String] as? Int {
      known.insert(CGWindowID(id))
    }
  }
}
func windows() -> [[String: Any]] {
  // The numbers themselves as the array's values, not objects.
  var ids: [UnsafeRawPointer?] = known.map { UnsafeRawPointer(bitPattern: UInt($0)) }
  let list = CGWindowListCreateDescriptionFromArray(CFArrayCreate(nil, &ids, ids.count, nil)) as? [[String: Any]] ?? []
  return list.compactMap { w in
    guard w[kCGWindowIsOnscreen as String] as? Bool == true,
          let b = w[kCGWindowBounds as String] as? [String: Double],
          (w[kCGWindowAlpha as String] as? Double ?? 1) > 0 else { return nil }
    return ["id": w[kCGWindowNumber as String] as? Int ?? 0,
            "layer": w[kCGWindowLayer as String] as? Int ?? 0,
            "x": b["X"] ?? 0, "y": b["Y"] ?? 0, "w": b["Width"] ?? 0, "h": b["Height"] ?? 0]
  }
}
find(.optionAll)

var done = false
Thread.detachNewThread {
  var round = 0
  while !done {
    if round % 5 == 4 { find(.optionOnScreenOnly) }
    round += 1
    line(["windows": windows()])
    usleep(25_000)
  }
}
Thread.detachNewThread {
  for step in args[2...] {
    let parts = step.split(separator: ":", maxSplits: 1).map(String.init)
    guard parts.count == 2 else { continue }
    switch parts[0] {
    case "k":
      guard let code = UInt16(parts[1]) else { continue }
      line(["key": Int(code)])
      posted += 1
      press(code)
    case "w":
      usleep(useconds_t((Int(parts[1]) ?? 0) * 1000))
    case "m":
      line(["mark": parts[1]])
    default:
      continue
    }
  }
  done = true
  usleep(40_000)
  exit(0)
}
CFRunLoopRun()
