// A display's mode, for `holdmode` (issue #33).
//
//   mode ID           the mode display ID runs, "w h pixel-width" in points
//   mode ID W H       switch display ID to a W × H mode, as WindowServer
//                     does when it drops a display's mode; prints the
//                     sizes it offers when it has none of that size
import CoreGraphics
import Foundation

let args = CommandLine.arguments
let id = CGDirectDisplayID(UInt32(args[1])!)
if args.count == 2 {
  guard let mode = CGDisplayCopyDisplayMode(id) else { exit(1) }
  print(mode.width, mode.height, mode.pixelWidth)
  exit(0)
}
let (w, h) = (Int(args[2])!, Int(args[3])!)
let options = [kCGDisplayShowDuplicateLowResolutionModes: true] as CFDictionary
let modes = (CGDisplayCopyAllDisplayModes(id, options) as? [CGDisplayMode]) ?? []
guard let mode = modes.first(where: { $0.width == w && $0.height == h }) else {
  print("no \(w)x\(h) mode among", modes.map { "\($0.width)x\($0.height)@\($0.pixelWidth)" }.joined(separator: " "))
  exit(2)
}
var config: CGDisplayConfigRef?
CGBeginDisplayConfiguration(&config)
CGConfigureDisplayWithDisplayMode(config, id, mode, nil)
// For this session only: macOS doesn't keep it for the serial.
print(CGCompleteDisplayConfiguration(config, .forSession).rawValue)
