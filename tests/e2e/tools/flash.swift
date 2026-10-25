// Watch the user's main display for an app's windows, every 5 ms, until
// stdin closes. Prints how many distinct windows showed there and for how
// many milliseconds in all.
//
//     flash TextEdit
import CoreGraphics
import Foundation

let owner = CommandLine.arguments[1]
let main = CGDisplayBounds(CGMainDisplayID())
var seen = Set<Int>()
var visibleMs = 0.0
var done = false
Thread { while readLine() != nil {}; done = true }.start()
var last = Date()
while !done {
  let now = Date()
  let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
  var any = false
  for w in list where (w[kCGWindowOwnerName as String] as? String) == owner && (w[kCGWindowLayer as String] as? Int) == 0 {
    let b = w[kCGWindowBounds as String] as! [String: Double]
    let r = CGRect(x: b["X"]!, y: b["Y"]!, width: b["Width"]!, height: b["Height"]!)
    if r.intersects(main) && r.width > 1 && r.height > 1 {
      seen.insert(w[kCGWindowNumber as String] as! Int)
      any = true
    }
  }
  if any { visibleMs += now.timeIntervalSince(last) * 1000 }
  last = now
  usleep(5_000)
}
print("{\"windows\":\(seen.count),\"ms\":\(Int(visibleMs))}")
