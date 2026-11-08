// Lists on-screen windows whose top-left lies on a display, one JSON per line.
//
//     onscreen main          the user's main display
//     onscreen <displayId>   any display
//     onscreen closet        AeroSpace's closet (vendor 0x4145, model 1); none without one
import CoreGraphics
import Foundation

let arg = CommandLine.arguments.dropFirst().first ?? "main"
func closet() -> CGDirectDisplayID {
  var ids = [CGDirectDisplayID](repeating: 0, count: 32)
  var count: UInt32 = 0
  CGGetActiveDisplayList(32, &ids, &count)
  guard let id = ids[..<Int(count)].first(where: {
    CGDisplayVendorNumber($0) == 0x4145 && CGDisplayModelNumber($0) == 1
  }) else { exit(0) }
  return id
}
let id = arg == "main" ? CGMainDisplayID() : arg == "closet" ? closet() : CGDirectDisplayID(UInt32(arg)!)
let area = CGDisplayBounds(id)
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list {
  guard let b = w[kCGWindowBounds as String], let r = CGRect(dictionaryRepresentation: b as! CFDictionary),
        r.intersects(area) else { continue }
  let out: [String: Any] = [
    "owner": w[kCGWindowOwnerName as String] ?? "", "name": w[kCGWindowName as String] ?? "",
    "layer": w[kCGWindowLayer as String] ?? 0, "x": r.minX, "y": r.minY, "w": r.width, "h": r.height,
  ]
  print(String(data: try! JSONSerialization.data(withJSONObject: out), encoding: .utf8)!)
}
