// Read-only lookups for the end-to-end tests.
//
//   probe windows PID         on-screen windows of PID as JSON lines
//   probe displays            how many Sidescreen virtual displays are active
//   probe idle                seconds since the user last touched a key or the pointer
//   probe locked              1 while the screen is locked, else 0
//   probe pixel PNG X Y       the average color of a 5x5 square, "r g b" 0-255
//   probe span PNG Y R G B    first and last x in row Y within 40 of the color, or nothing
//   probe front               the front app's name
//   probe main                the main display's size in points, "w h"
//   probe tap SECONDS OUT     record scroll events at the HID level, listen only
//   probe bounds ID           a display's frame in global points, "x y w h"
//   probe units               every online display's unit number, "id unit" per line
//   probe reconfigurations S  how many display reconfigurations begin within S seconds
//   probe differ PNG PNG      mean RGB difference of two images, 0-255, below the menu bar
import AppKit
import CoreGraphics
import Foundation
import ImageIO

// Top-level globals start in order, so this one comes before any use.
var reconfigurations = Set<Int>()
func reconfigurationBegun(_ slot: Int) { reconfigurations.insert(slot) }

let args = CommandLine.arguments
let tapStart = Date()
var tapOut: FileHandle?
switch args[1] {
case "windows":
  let pid = Int32(args[2])!
  let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as! [[String: Any]]
  for w in list where (w[kCGWindowOwnerPID as String] as? Int32) == pid {
    let b = w[kCGWindowBounds as String] as! [String: Double]
    print("{\"id\":\(w[kCGWindowNumber as String] as! Int),\"layer\":\(w[kCGWindowLayer as String] as! Int),\"x\":\(b["X"]!),\"y\":\(b["Y"]!),\"width\":\(b["Width"]!),\"height\":\(b["Height"]!)}")
  }
case "displays":
  var ids = [CGDirectDisplayID](repeating: 0, count: 32); var n: UInt32 = 0
  CGGetActiveDisplayList(32, &ids, &n)
  print(ids[0..<Int(n)].filter { CGDisplayVendorNumber($0) == 0x5353 }.count)
case "idle":
  print(Int(CGEventSource.secondsSinceLastEventType(.hidSystemState, eventType: CGEventType(rawValue: ~0)!)))
case "locked":
  let session = CGSessionCopyCurrentDictionary() as? [String: Any] ?? [:]
  print((session["CGSSessionScreenIsLocked"] as? Bool ?? false) ? 1 : 0)
case "pixel":
  let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[2]) as CFURL, nil)!
  let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
  let (x, y) = (Int(Double(args[3])!), Int(Double(args[4])!))
  let ctx = CGContext(data: nil, width: 5, height: 5, bitsPerComponent: 8, bytesPerRow: 20, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(image, in: CGRect(x: -x + 2, y: -(image.height - y) + 3, width: image.width, height: image.height))
  let px = ctx.data!.assumingMemoryBound(to: UInt8.self)
  var sum = [0, 0, 0]
  for i in 0..<25 { for c in 0..<3 { sum[c] += Int(px[i * 4 + c]) } }
  print(sum.map { $0 / 25 }.map(String.init).joined(separator: " "))
case "reconfigurations":
  // Display reconfigurations over SECONDS: one per configuration WindowServer
  // begins, whichever displays it touches.
  _ = NSApplication.shared
  CGDisplayRegisterReconfigurationCallback({ _, flags, _ in
    if flags.contains(.beginConfigurationFlag) {
      // Callbacks of one configuration arrive within a few milliseconds.
      reconfigurationBegun(Int(Date().timeIntervalSince1970 * 20))
    }
  }, nil)
  // Callbacks come only through the app's event loop.
  DispatchQueue.main.asyncAfter(deadline: .now() + Double(args[2])!) {
    print(reconfigurations.count)
    exit(0)
  }
  NSApplication.shared.run()
case "units":
  // Every online display, the closet included: "id unit" per line.
  var ids = [CGDirectDisplayID](repeating: 0, count: 32)
  var count: UInt32 = 0
  CGGetOnlineDisplayList(32, &ids, &count)
  for id in ids[0..<Int(count)] { print("\(id) \(CGDisplayUnitNumber(id))") }
case "bounds":
  // A display's frame in global points: x y width height.
  let b = CGDisplayBounds(CGDirectDisplayID(args[2])!)
  print("\(b.origin.x) \(b.origin.y) \(b.size.width) \(b.size.height)")
case "differ":
  // Mean RGB difference, 0 to 255, of two images scaled to 64 × 40,
  // leaving out the top 6%: a menu bar there differs between captures.
  func grid(_ path: String) -> [UInt8] {
    let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL, nil)!
    let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
    let ctx = CGContext(data: nil, width: 64, height: 40, bitsPerComponent: 8, bytesPerRow: 256, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue)!
    ctx.interpolationQuality = .medium
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: 64, height: 40))
    // Rows run top down in memory.
    return Array(UnsafeBufferPointer(start: ctx.data!.assumingMemoryBound(to: UInt8.self), count: 64 * 40 * 4)[(64 * 3 * 4)...])
  }
  let (a, b) = (grid(args[2]), grid(args[3]))
  var total = 0, count = 0
  for i in 0..<a.count where i % 4 != 3 { total += abs(Int(a[i]) - Int(b[i])); count += 1 }
  print(total / count)
case "span":
  let source = CGImageSourceCreateWithURL(URL(fileURLWithPath: args[2]) as CFURL, nil)!
  let image = CGImageSourceCreateImageAtIndex(source, 0, nil)!
  let y = Int(Double(args[3])!)
  let want = args[4...6].map { Int($0)! }
  let ctx = CGContext(data: nil, width: image.width, height: 1, bitsPerComponent: 8, bytesPerRow: image.width * 4, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
  ctx.draw(image, in: CGRect(x: 0, y: -(image.height - y - 1), width: image.width, height: image.height))
  let px = ctx.data!.assumingMemoryBound(to: UInt8.self)
  let hits = (0..<image.width).filter { x in (0..<3).allSatisfy { abs(Int(px[x * 4 + $0]) - want[$0]) <= 40 } }
  if let first = hits.first, let last = hits.last { print(first, last) }
case "main":
  let b = CGDisplayBounds(CGMainDisplayID())
  print(Int(b.width), Int(b.height))
case "front":
  print(NSWorkspace.shared.frontmostApplication?.localizedName ?? "")
case "activate":
  // Bring a process forward, as the user clicking it would.
  let pid = pid_t(args[2])!
  NSRunningApplication(processIdentifier: pid)?.activate()
case "tap":
  FileManager.default.createFile(atPath: args[3], contents: nil)
  tapOut = FileHandle(forWritingAtPath: args[3])
  let mask = CGEventMask(1 << CGEventType.scrollWheel.rawValue) | CGEventMask(1 << CGEventType.leftMouseDown.rawValue)
  let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .tailAppendEventTap, options: .listenOnly, eventsOfInterest: mask, callback: { _, type, e, _ in
    let line = String(format: "%.3f type=%d at=(%.0f,%.0f) cont=%lld phase=%lld mom=%lld d1=%lld pd1=%.2f\n",
      Date().timeIntervalSince(tapStart), type.rawValue, e.location.x, e.location.y,
      e.getIntegerValueField(.scrollWheelEventIsContinuous), e.getIntegerValueField(.scrollWheelEventScrollPhase),
      e.getIntegerValueField(.scrollWheelEventMomentumPhase), e.getIntegerValueField(.scrollWheelEventDeltaAxis1),
      e.getDoubleValueField(.scrollWheelEventPointDeltaAxis1))
    tapOut?.write(line.data(using: .utf8)!)
    return Unmanaged.passUnretained(e)
  }, userInfo: nil)!
  CFRunLoopAddSource(CFRunLoopGetCurrent(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
  CGEvent.tapEnable(tap: tap, enable: true)
  CFRunLoopRunInMode(.defaultMode, Double(args[2])!, false)
default:
  print("unknown command \(args[1])"); exit(2)
}
