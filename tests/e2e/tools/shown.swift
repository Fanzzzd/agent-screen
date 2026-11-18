// Records what one process shows on the user's displays while a test acts,
// as pixels rather than the window list: a minimize or restore animation
// draws a window where the list doesn't say it is. Until stdin closes,
// ScreenCaptureKit captures each user display, and any extra display the
// arguments name, with only that process's windows in the picture, at 60
// fps. Then it prints one JSON line per display: the frames it got, idle
// ones included, how many held any of the process's pixels, the first
// and last of those in ms since the capture began, and where it saved the
// first such frame.
//
//     shown <pid> <png prefix> [<display id>...]
//
// The user's displays are the active ones that are not Agent Displays
// (vendor 0x5353) or AeroSpace's closet (vendor 0x4145, model 1). Prints
// "ready" on stderr once every capture runs.
import AppKit
import CoreGraphics
import ScreenCaptureKit

let args = CommandLine.arguments
let pid = pid_t(args[1])!
let prefix = args[2]
let extra = args.dropFirst(3).compactMap { CGDirectDisplayID($0) }

func isUser(_ id: CGDirectDisplayID) -> Bool {
  let vendor = CGDisplayVendorNumber(id)
  if vendor == 0x5353 { return false }
  if vendor == 0x4145 && CGDisplayModelNumber(id) == 1 { return false }
  return true
}

final class Watch: NSObject, SCStreamOutput {
  let display: CGDirectDisplayID
  let start = Date()
  var frames = 0
  var shown = 0
  var first: Int?
  var last: Int?
  var saved: String?
  init(_ display: CGDirectDisplayID) { self.display = display }

  func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
    guard type == .screen, sb.isValid,
      let attachments = CMSampleBufferGetSampleAttachmentsArray(sb, createIfNecessary: false)
        as? [[SCStreamFrameInfo: Any]],
      let raw = attachments.first?[.status] as? Int
    else { return }
    // Every frame the stream sends counts, idle ones too, so a live
    // capture of a display the process never drew on still shows frames.
    frames += 1
    guard SCFrameStatus(rawValue: raw) == .complete,
      let px = CMSampleBufferGetImageBuffer(sb)
    else { return }
    CVPixelBufferLockBaseAddress(px, .readOnly)
    defer { CVPixelBufferUnlockBaseAddress(px, .readOnly) }
    let base = CVPixelBufferGetBaseAddress(px)!.assumingMemoryBound(to: UInt8.self)
    let row = CVPixelBufferGetBytesPerRow(px)
    let w = CVPixelBufferGetWidth(px)
    let h = CVPixelBufferGetHeight(px)
    // The background is black. A window's own pixels are brighter than its
    // shadow's, which stays near black.
    var any = false
    outer: for y in stride(from: 0, to: h, by: 2) {
      for x in stride(from: 0, to: w, by: 2) {
        let p = y * row + x * 4
        if max(base[p], base[p + 1], base[p + 2]) > 40 { any = true; break outer }
      }
    }
    let ms = Int(Date().timeIntervalSince(start) * 1000)
    if any {
      shown += 1
      if first == nil {
        first = ms
        save(px)
      }
      last = ms
    }
  }

  func save(_ px: CVPixelBuffer) {
    let image = CIImage(cvPixelBuffer: px)
    let context = CIContext()
    guard let cg = context.createCGImage(image, from: image.extent) else { return }
    let file = "\(prefix)-\(display).png"
    let rep = NSBitmapImageRep(cgImage: cg)
    if let data = rep.representation(using: .png, properties: [:]),
      (try? data.write(to: URL(fileURLWithPath: file))) != nil
    {
      saved = file
    }
  }

  func json() -> String {
    let path = saved.map { "\"\($0)\"" } ?? "null"
    return "{\"display\":\(display),\"frames\":\(frames),\"shown\":\(shown),\"first\":\(first.map(String.init) ?? "null"),\"last\":\(last.map(String.init) ?? "null"),\"saved\":\(path)}"
  }
}

var streams: [SCStream] = []
var watches: [Watch] = []
let queue = DispatchQueue(label: "shown")

Task {
  do {
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
    guard let app = content.applications.first(where: { $0.processID == pid }) else {
      FileHandle.standardError.write("no app with pid \(pid)\n".data(using: .utf8)!)
      exit(2)
    }
    for display in content.displays where isUser(display.displayID) || extra.contains(display.displayID) {
      let filter = SCContentFilter(display: display, including: [app], exceptingWindows: [])
      let cfg = SCStreamConfiguration()
      // A quarter of the points: a window shows over far more than that.
      cfg.width = max(display.width / 4, 64)
      cfg.height = max(display.height / 4, 64)
      cfg.minimumFrameInterval = CMTime(value: 1, timescale: 60)
      cfg.showsCursor = false
      cfg.backgroundColor = .black
      cfg.pixelFormat = kCVPixelFormatType_32BGRA
      let watch = Watch(display.displayID)
      let stream = SCStream(filter: filter, configuration: cfg, delegate: nil)
      try stream.addStreamOutput(watch, type: .screen, sampleHandlerQueue: queue)
      try await stream.startCapture()
      streams.append(stream)
      watches.append(watch)
    }
    // Nothing captured would prove nothing.
    guard !watches.isEmpty else {
      let seen = content.displays.map { "\($0.displayID)" }.joined(separator: " ")
      FileHandle.standardError.write("no display to capture; ScreenCaptureKit listed [\(seen)]\n".data(using: .utf8)!)
      exit(4)
    }
    FileHandle.standardError.write("ready\n".data(using: .utf8)!)
  } catch {
    FileHandle.standardError.write("capture failed: \(error)\n".data(using: .utf8)!)
    exit(3)
  }
}

Thread {
  while readLine() != nil {}
  Task {
    for s in streams { try? await s.stopCapture() }
    queue.sync {
      for w in watches { print(w.json()) }
    }
    fflush(stdout)
    exit(0)
  }
}.start()

RunLoop.main.run()
