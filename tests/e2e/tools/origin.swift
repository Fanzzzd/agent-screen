// A display's place, for `sparerepark` (#286).
//
//   origin ID          display ID's bounds, "x y w h" in points
//   origin main        the main display's bounds
//   origin ID X Y      move display ID's top-left corner to X, Y, for this
//                      session only, as macOS does when it reattaches a
//                      display after a change
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let id = args[1] == "main" ? CGMainDisplayID() : CGDirectDisplayID(UInt32(args[1])!)
if args.count == 2 {
  let b = CGDisplayBounds(id)
  print(Int(b.origin.x), Int(b.origin.y), Int(b.width), Int(b.height))
  exit(0)
}
var config: CGDisplayConfigRef?
CGBeginDisplayConfiguration(&config)
CGConfigureDisplayOrigin(config, id, Int32(args[2])!, Int32(args[3])!)
print(CGCompleteDisplayConfiguration(config, .forSession).rawValue)
