import AppKit

/// A plain-layer stand-in for the control panel card, used only while the
/// card grows or shrinks.
///
/// Transforming the real card does not work: the live IOSurface frame inside
/// it keeps rendering at full size and only gets clipped. The proxy is a
/// layer-hosting view holding bitmaps only (a header snapshot and a still of
/// the latest display frame), so everything in it follows the transform.
@MainActor
final class MorphProxy {
    let view = NSView()
    let screenLayer = CALayer()

    private let root = CALayer()
    private let surface = CALayer()
    private let headerLayer = CALayer()

    init(cornerRadius: CGFloat, screenCornerRadius: CGFloat) {
        view.layer = root
        view.wantsLayer = true
        view.isHidden = true

        root.shadowColor = NSColor.black.cgColor
        root.shadowOpacity = 0.28
        root.shadowRadius = 24
        root.shadowOffset = CGSize(width: 0, height: -10)

        surface.cornerRadius = cornerRadius
        surface.cornerCurve = .continuous
        surface.masksToBounds = true
        surface.borderWidth = 0.5

        screenLayer.contentsGravity = .resize
        screenLayer.cornerRadius = screenCornerRadius
        screenLayer.cornerCurve = .continuous
        screenLayer.masksToBounds = true
        screenLayer.backgroundColor = NSColor.black.withAlphaComponent(0.35).cgColor

        root.addSublayer(surface)
        surface.addSublayer(headerLayer)
        surface.addSublayer(screenLayer)
        for layer in [root, surface, headerLayer, screenLayer] {
            layer.actions = ["contents": NSNull(), "bounds": NSNull(), "position": NSNull(),
                             "frame": NSNull(), "shadowPath": NSNull()]
        }
    }

    /// Match the card's current geometry and look.
    func mirror(card: NSView, header: NSView, screen: DisplayView) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        view.frame = card.frame
        let bounds = CGRect(origin: .zero, size: card.frame.size)
        surface.frame = bounds
        root.shadowPath = CGPath(roundedRect: bounds, cornerWidth: surface.cornerRadius,
                                 cornerHeight: surface.cornerRadius, transform: nil)

        card.effectiveAppearance.performAsCurrentDrawingAppearance {
            surface.backgroundColor = NSColor.windowBackgroundColor.withAlphaComponent(0.92).cgColor
            surface.borderColor = NSColor.separatorColor.cgColor
        }

        header.layoutSubtreeIfNeeded()
        headerLayer.frame = header.convert(header.bounds, to: card)
        headerLayer.contents = snapshot(of: header)
        headerLayer.contentsScale = card.window?.backingScaleFactor ?? 2

        screenLayer.frame = screen.convert(screen.contentRect, to: card)
        CATransaction.commit()
    }

    private func snapshot(of view: NSView) -> NSImage? {
        guard let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { return nil }
        view.cacheDisplay(in: view.bounds, to: rep)
        let image = NSImage(size: view.bounds.size)
        image.addRepresentation(rep)
        return image
    }
}
