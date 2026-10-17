// How late an ordinary app's main thread runs, as a stand-in for the apps
// the user is in: an AppKit app with screens, so it gets every display change
// the way they do. A timer asks for a tick every 10 ms for SECONDS; each tick
// that comes more than 30 ms late is a stall, printed as one JSON line when
// done: {"max":…, "over50":…, "over100":…, "stalled":…, "stalls":[[at, ms], …]}
// with `at` in seconds since the Unix epoch and `stalled` the ms spent late.
//
//   lag SECONDS
import AppKit

let seconds = Double(CommandLine.arguments[1])!
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
// AppKit watches the screens once something has asked for them.
_ = NSScreen.screens

var last = Date()
var stalls: [(Double, Int)] = []
let every = 0.010
let timer = Timer(timeInterval: every, repeats: true) { _ in
  let now = Date()
  let late = Int((now.timeIntervalSince(last) - every) * 1000)
  if late > 30 { stalls.append((now.timeIntervalSince1970, late)) }
  last = now
}
RunLoop.main.add(timer, forMode: .common)
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
  let ms = stalls.map { $0.1 }
  let list = stalls.map { String(format: "[%.3f,%d]", $0.0, $0.1) }.joined(separator: ",")
  print("{\"max\":\(ms.max() ?? 0),\"over50\":\(ms.filter { $0 > 50 }.count),\"over100\":\(ms.filter { $0 > 100 }.count),\"stalled\":\(ms.reduce(0, +)),\"stalls\":[\(list)]}")
  exit(0)
}
app.run()
