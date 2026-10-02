// Stand-in agent app for the spike: a window on display <id> whose content
// changes every display refresh, so capture has 60 fps to deliver. AppKit,
// not GPUI: agent apps are ordinary Mac apps, and GPUI paces windows on a
// virtual display at ~24 Hz.
import AppKit

let target = CGDirectDisplayID(CommandLine.arguments[1])!
let app = NSApplication.shared
app.setActivationPolicy(.accessory)

guard let screen = NSScreen.screens.first(where: {
    ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == target
}) else { fatalError("display \(target) not found") }

// Borderless, so AppKit does not constrain the frame onto the main screen.
let frame = NSRect(x: screen.frame.minX + 60, y: screen.frame.minY + 60, width: 1200, height: 760)
let window = NSWindow(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)
window.backgroundColor = NSColor(red: 0.13, green: 0.19, blue: 0.29, alpha: 1)
let square = CALayer()
square.backgroundColor = NSColor.orange.cgColor
square.cornerRadius = 24
square.bounds = CGRect(x: 0, y: 0, width: 140, height: 140)
window.contentView!.wantsLayer = true
window.contentView!.layer!.addSublayer(square)
window.orderFrontRegardless()
window.setFrame(frame, display: true)
guard window.screen == screen else { fatalError("busy window landed off display \(target)") }

final class Ticker: NSObject {
    let start = CACurrentMediaTime()
    var frames = 0
    @objc func tick(_ link: CADisplayLink) {
        let t = CACurrentMediaTime() - start
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        square.position = CGPoint(x: 110 + (sin(t * 1.3) * 0.5 + 0.5) * 900, y: 110 + (cos(t * 0.9) * 0.5 + 0.5) * 500)
        CATransaction.commit()
        frames += 1
    }
}
let ticker = Ticker()
let link = window.contentView!.displayLink(target: ticker, selector: #selector(Ticker.tick(_:)))
link.add(to: .main, forMode: .common)
Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
    FileHandle.standardError.write("busy fps=\(ticker.frames)\n".data(using: .utf8)!)
    ticker.frames = 0
}
app.run()
