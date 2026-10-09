// Lists on-screen windows whose top-left lies on a display, one JSON per line.
//
//     onscreen main          the user's main display
//     onscreen <displayId>   any display
import CoreGraphics
import Foundation

let arg = CommandLine.arguments.dropFirst().first ?? "main"
let id = arg == "main" ? CGMainDisplayID() : CGDirectDisplayID(UInt32(arg)!)
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
