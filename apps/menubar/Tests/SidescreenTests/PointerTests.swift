import AppKit
import Testing
@testable import Sidescreen

/// The Agent Pointer lands where the agent acted, scaled into the frame.
@MainActor
struct PointerTests {
    /// Render a 720×450 view of a 1440×900 display and return the color
    /// just inside the arrow's tip for a pointer at `point`.
    private func tipColor(pointer point: CGPoint, probe: CGPoint) -> NSColor? {
        let view = DisplayView(frame: CGRect(x: 0, y: 0, width: 720, height: 450))
        view.displaySize = CGSize(width: 1440, height: 900)
        view.layout()
        view.showPointer(AgentPointer(point: point, click: false, seq: 1))
        view.layout()
        view.layer?.removeAllAnimationsRecursively()
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 720, pixelsHigh: 450, bitsPerSample: 8,
                                   samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        let context = NSGraphicsContext(bitmapImageRep: rep)!
        view.layer?.render(in: context.cgContext)
        // Bitmap rows run top-down, like display points.
        return rep.colorAt(x: Int(probe.x), y: Int(probe.y))
    }

    @Test func arrowSitsAtTheAgentsPoint() {
        // Display point (1000, 300) maps to view point (500, 150) from the top.
        let inside = tipColor(pointer: CGPoint(x: 1000, y: 300), probe: CGPoint(x: 503, y: 160))
        let elsewhere = tipColor(pointer: CGPoint(x: 1000, y: 300), probe: CGPoint(x: 200, y: 400))
        #expect((inside?.alphaComponent ?? 0) > 0.9)
        // Just above and left of the tip is outside the arrow.
        let beside = tipColor(pointer: CGPoint(x: 1000, y: 300), probe: CGPoint(x: 494, y: 144))
        #expect((elsewhere?.alphaComponent ?? 1) < 0.9)
        #expect((beside?.alphaComponent ?? 1) < 0.9)
    }
}

private extension CALayer {
    func removeAllAnimationsRecursively() {
        removeAllAnimations()
        sublayers?.forEach { $0.removeAllAnimationsRecursively() }
    }
}
