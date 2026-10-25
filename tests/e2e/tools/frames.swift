// frames SECONDS: a small window at the bottom left with two squares, one
// turned by a Core Animation animation, one moved by a main-thread timer.
// ScreenCaptureKit samples it at 60 fps, and every time a square's pixels
// stood still longer than 80 ms prints "gap <ca|main> <start ms> <ms>",
// start in Unix milliseconds. Used by changecost.
import AppKit
import ScreenCaptureKit
import QuartzCore

let seconds = Double(CommandLine.arguments.dropFirst().first ?? "40") ?? 40
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let panel = NSPanel(contentRect: NSRect(x: 40, y: 40, width: 240, height: 120),
                    styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
panel.level = .floating
panel.backgroundColor = .black
panel.contentView!.wantsLayer = true
let root = panel.contentView!.layer!
let a = CALayer(); a.frame = CGRect(x: 20, y: 20, width: 80, height: 80); a.backgroundColor = NSColor.systemRed.cgColor
let b = CALayer(); b.frame = CGRect(x: 140, y: 20, width: 20, height: 80); b.backgroundColor = NSColor.systemGreen.cgColor
root.addSublayer(a); root.addSublayer(b)
let spin = CABasicAnimation(keyPath: "transform.rotation.z")
spin.fromValue = 0; spin.toValue = Double.pi * 2; spin.duration = 1; spin.repeatCount = .infinity
a.add(spin, forKey: "spin")
panel.orderFrontRegardless()
var t0 = CACurrentMediaTime()
Timer.scheduledTimer(withTimeInterval: 1.0/60, repeats: true) { _ in
    let p = (CACurrentMediaTime() - t0).truncatingRemainder(dividingBy: 1)
    CATransaction.begin(); CATransaction.setDisableActions(true)
    b.frame.origin.x = 120 + 90 * p
    CATransaction.commit()
}
final class Out: NSObject, SCStreamOutput {
    var last: [UInt64] = [0, 0]; var changed: [Double] = [0, 0]; let start = Date()
    func stream(_ s: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .screen, let px = CMSampleBufferGetImageBuffer(sb) else { return }
        CVPixelBufferLockBaseAddress(px, .readOnly); defer { CVPixelBufferUnlockBaseAddress(px, .readOnly) }
        let base = CVPixelBufferGetBaseAddress(px)!.assumingMemoryBound(to: UInt8.self)
        let row = CVPixelBufferGetBytesPerRow(px); let w = CVPixelBufferGetWidth(px); let h = CVPixelBufferGetHeight(px)
        let now = Date().timeIntervalSince1970
        for (i, (x0, x1)) in [(0, w/2), (w/2, w)].enumerated() {
            var hash: UInt64 = 1469598103934665603
            for y in stride(from: 0, to: h, by: 2) { for x in stride(from: x0, to: x1, by: 2) {
                hash = (hash ^ UInt64(base[y*row + x*4 + 2])) &* 1099511628211 } }
            if hash != last[i] {
                if changed[i] > 0 && now - changed[i] > 0.08 {
                    print(String(format: "gap %@ %.0f %.0f", i == 0 ? "ca" : "main", changed[i] * 1000, (now - changed[i]) * 1000))
                    fflush(stdout)
                }
                last[i] = hash; changed[i] = now
            }
        }
    }
}
let out = Out()
var stream: SCStream?
Task {
    try await Task.sleep(nanoseconds: 700_000_000)
    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    guard let w = content.windows.first(where: { $0.windowID == CGWindowID(panel.windowNumber) }) else { print("no window"); exit(1) }
    let cfg = SCStreamConfiguration(); cfg.width = 240; cfg.height = 120; cfg.minimumFrameInterval = CMTime(value: 1, timescale: 60)
    cfg.pixelFormat = kCVPixelFormatType_32BGRA
    stream = SCStream(filter: SCContentFilter(desktopIndependentWindow: w), configuration: cfg, delegate: nil)
    try stream!.addStreamOutput(out, type: .screen, sampleHandlerQueue: DispatchQueue(label: "cap"))
    try await stream!.startCapture()
    print("capturing"); fflush(stdout)
}
DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
app.run()
