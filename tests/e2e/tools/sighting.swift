// Watches the user's displays while a test script runs and records every
// window a process of the test shows there. Samples the window list every
// INTERVAL ms (default 10) until stdin closes, then prints one JSON line per
// window seen: owner, pid, executable, title, frame, first and last seen and
// how long it showed, in ms since the start.
//
//     sighting <script pid> <since, s since 1970> <mark> [<skip path>...]
//
// The user's displays are the active ones that are not Agent Displays
// (vendor 0x5353) or AeroSpace's closet (vendor 0x4145, model 1). A process
// is the test's when it descends from the script, when its executable or
// command line holds <mark> (the checkout: its target/e2e tools, fixtures
// and throwaway browser profiles), or when launchd started it after <since>,
// as the apps launch_app opens are, unless it lives in /System/Library or /usr,
// where macOS keeps the services that show alerts and notifications, or in
// another checkout's target folder, where another script runs. A
// process whose executable starts with a <skip path> is left out: the
// countdown sign, and the test app, whose cards and Viewer are for the user.
import CoreGraphics
import Darwin
import Foundation

let args = CommandLine.arguments
let scriptPid = pid_t(args[1])!
let since = Double(args[2])!
let mark = args[3]
let skipPaths = Array(args.dropFirst(4))
let interval = UInt32(ProcessInfo.processInfo.environment["INTERVAL"].flatMap(Int.init) ?? 10)
let me = getpid()

struct Proc {
  let parent: pid_t
  let started: Double
  let path: String
  let commandLine: String
}

func kinfo(_ pid: pid_t) -> kinfo_proc? {
  var info = kinfo_proc()
  var size = MemoryLayout<kinfo_proc>.stride
  var mib: [Int32] = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid]
  guard sysctl(&mib, 4, &info, &size, nil, 0) == 0, size > 0 else { return nil }
  return info
}

func commandLine(_ pid: pid_t) -> String {
  var mib: [Int32] = [CTL_KERN, KERN_PROCARGS2, pid]
  var size = 0
  guard sysctl(&mib, 3, nil, &size, nil, 0) == 0, size > 0 else { return "" }
  var buffer = [UInt8](repeating: 0, count: size)
  guard sysctl(&mib, 3, &buffer, &size, nil, 0) == 0 else { return "" }
  // argc, then the executable path, then the arguments, NUL separated.
  return String(decoding: buffer[4..<size].map { $0 == 0 ? 32 : $0 }, as: UTF8.self)
}

func path(_ pid: pid_t) -> String {
  var buffer = [CChar](repeating: 0, count: 4096)
  guard proc_pidpath(pid, &buffer, 4096) > 0 else { return "" }
  return String(cString: buffer)
}

var procs: [pid_t: Proc] = [:]
func proc(_ pid: pid_t) -> Proc? {
  if let p = procs[pid] { return p }
  guard let info = kinfo(pid) else { return nil }
  let t = info.kp_proc.p_un.__p_starttime
  let p = Proc(
    parent: info.kp_eproc.e_ppid, started: Double(t.tv_sec) + Double(t.tv_usec) / 1e6,
    path: path(pid), commandLine: commandLine(pid))
  procs[pid] = p
  return p
}

func descends(_ pid: pid_t) -> Bool {
  var p = pid
  for _ in 0..<64 {
    if p == scriptPid { return true }
    guard p > 1, let parent = proc(p)?.parent else { return false }
    p = parent
  }
  return false
}

var verdicts: [pid_t: Bool] = [:]
func isTests(_ pid: pid_t) -> Bool {
  if let v = verdicts[pid] { return v }
  var v = false
  if pid != me, let p = proc(pid), !skipPaths.contains(where: { p.path.hasPrefix($0) }) {
    let system = p.path.hasPrefix("/System/Library/") || p.path.hasPrefix("/usr/")
    let ours = p.path.contains(mark) || p.commandLine.contains(mark)
    // Another checkout's test tools and apps belong to the script running there.
    let others =
      !ours && ["/target/e2e/", "/target/Agent Screen Dev.app/"].contains { p.commandLine.contains($0) }
    // A late app is the test's when launchd started it, as `open` and
    // launch_app do; another tool's child, such as Playwright's browser
    // under node, is not.
    let opened = p.parent == 1
    v = descends(pid) || ours || (p.started >= since && opened && !system && !others)
  }
  verdicts[pid] = v
  return v
}

func userDisplays() -> [CGRect] {
  var ids = [CGDirectDisplayID](repeating: 0, count: 32)
  var n: UInt32 = 0
  CGGetActiveDisplayList(32, &ids, &n)
  return ids[0..<Int(n)].filter { id in
    let vendor = CGDisplayVendorNumber(id)
    return vendor != 0x5353 && !(vendor == 0x4145 && CGDisplayModelNumber(id) == 1)
  }.map { CGDisplayBounds($0) }
}

struct Seen {
  var owner: String
  var pid: pid_t
  var title: String
  var layer: Int
  var frame: CGRect
  var first: Double
  var last: Double
  var ms: Double
}

var seen: [Int: Seen] = [:]
var order: [Int] = []
var done = false
Thread {
  while readLine() != nil {}
  done = true
}.start()

let start = Date()
var displays = userDisplays()
var lastDisplays = Date()
var previous = start
while !done {
  let now = Date()
  if now.timeIntervalSince(lastDisplays) > 1 {
    displays = userDisplays()
    lastDisplays = now
  }
  let elapsed = now.timeIntervalSince(start) * 1000
  let step = now.timeIntervalSince(previous) * 1000
  previous = now
  let list =
    CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
  for w in list {
    guard let pid = w[kCGWindowOwnerPID as String] as? pid_t,
      let b = w[kCGWindowBounds as String],
      let r = CGRect(dictionaryRepresentation: b as! CFDictionary),
      (w[kCGWindowAlpha as String] as? Double ?? 1) > 0
    else { continue }
    // A sliver, as of a window parked past a screen's edge, shows nothing.
    let visible = displays.contains { d in
      let i = r.intersection(d)
      return !i.isNull && i.width > 2 && i.height > 2
    }
    guard visible, isTests(pid) else { continue }
    let id = w[kCGWindowNumber as String] as? Int ?? 0
    if var s = seen[id] {
      s.last = elapsed
      s.ms += step
      s.frame = r
      if let t = w[kCGWindowName as String] as? String, !t.isEmpty { s.title = t }
      seen[id] = s
    } else {
      seen[id] = Seen(
        owner: w[kCGWindowOwnerName as String] as? String ?? "", pid: pid,
        title: w[kCGWindowName as String] as? String ?? "",
        layer: w[kCGWindowLayer as String] as? Int ?? 0, frame: r, first: elapsed, last: elapsed,
        ms: interval > 0 ? Double(interval) : 1)
      order.append(id)
    }
  }
  usleep(interval * 1000)
}

for id in order {
  let s = seen[id]!
  let out: [String: Any] = [
    "window": id, "owner": s.owner, "pid": s.pid, "path": procs[s.pid]?.path ?? "",
    "title": s.title, "layer": s.layer,
    "frame": [s.frame.minX, s.frame.minY, s.frame.width, s.frame.height],
    "first": Int(s.first), "last": Int(s.last), "ms": Int(s.ms.rounded()),
  ]
  print(String(data: try! JSONSerialization.data(withJSONObject: out), encoding: .utf8)!)
}
