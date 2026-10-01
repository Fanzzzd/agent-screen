import AppKit
// Regenerate Resources/AppIcon.icns:
//   swiftc -o /tmp/make-icon scripts/make-icon.swift && mkdir -p /tmp/AppIcon.iconset
//   /tmp/make-icon /tmp/AppIcon.iconset && iconutil -c icns /tmp/AppIcon.iconset -o Resources/AppIcon.icns
//
// Sidescreen app icon: a rounded square with a deep blue gradient and two
// offset screens, the back one dimmed, the front one with a pointer.
func render(_ size: CGFloat) -> Data {
    let image = NSImage(size: NSSize(width: size, height: size), flipped: false) { rect in
        let s = size / 1024
        let tile = NSBezierPath(roundedRect: rect.insetBy(dx: 100 * s, dy: 100 * s), xRadius: 185 * s, yRadius: 185 * s)
        NSGradient(starting: NSColor(calibratedRed: 0.20, green: 0.42, blue: 0.98, alpha: 1),
                   ending: NSColor(calibratedRed: 0.10, green: 0.16, blue: 0.48, alpha: 1))!.draw(in: tile, angle: -90)
        func screen(_ r: CGRect, alpha: CGFloat) {
            let p = NSBezierPath(roundedRect: r, xRadius: 34 * s, yRadius: 34 * s)
            NSColor.white.withAlphaComponent(alpha).setFill(); p.fill()
        }
        screen(CGRect(x: 250 * s, y: 430 * s, width: 470 * s, height: 320 * s), alpha: 0.35)
        screen(CGRect(x: 310 * s, y: 290 * s, width: 470 * s, height: 320 * s), alpha: 0.95)
        // pointer on the front screen
        let arrow = NSBezierPath()
        let o = CGPoint(x: 520 * s, y: 520 * s)
        let pts: [(CGFloat, CGFloat)] = [(0, 0), (0, -150), (38, -113), (66, -175), (92, -164), (65, -103), (118, -103)]
        arrow.move(to: CGPoint(x: o.x + pts[0].0 * s, y: o.y + pts[0].1 * s))
        for p in pts.dropFirst() { arrow.line(to: CGPoint(x: o.x + p.0 * s, y: o.y + p.1 * s)) }
        arrow.close()
        NSColor(calibratedRed: 0.16, green: 0.36, blue: 0.95, alpha: 1).setFill(); arrow.fill()
        return true
    }
    let rep = NSBitmapImageRep(data: image.tiffRepresentation!)!
    return rep.representation(using: .png, properties: [:])!
}
let dir = CommandLine.arguments[1]
for (name, px) in [("16x16", 16), ("16x16@2x", 32), ("32x32", 32), ("32x32@2x", 64), ("128x128", 128),
                   ("128x128@2x", 256), ("256x256", 256), ("256x256@2x", 512), ("512x512", 512), ("512x512@2x", 1024)] {
    try! render(CGFloat(px)).write(to: URL(fileURLWithPath: "\(dir)/icon_\(name).png"))
}
