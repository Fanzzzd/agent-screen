import AppKit

/// Draws the Agent Pointer over a display frame: an arrow that glides to
/// each point the agent acts on, with a ripple on clicks. It lives in the
/// Viewer and Peek only, never on the display, so agent screenshots never
/// contain it.
final class PointerLayer: CALayer {
    private let arrow = CAShapeLayer()
    private var shown: AgentPointer?

    override init() {
        super.init()
        actions = ["position": NSNull(), "bounds": NSNull(), "transform": NSNull(), "opacity": NSNull()]
        // The tip of the arrow is the layer's anchor.
        bounds = CGRect(x: 0, y: 0, width: 16, height: 22)
        anchorPoint = CGPoint(x: 0, y: 1)
        opacity = 0

        let path = CGMutablePath()
        path.move(to: CGPoint(x: 0, y: 22))
        path.addLine(to: CGPoint(x: 0, y: 4))
        path.addLine(to: CGPoint(x: 4.6, y: 8.4))
        path.addLine(to: CGPoint(x: 7.8, y: 1))
        path.addLine(to: CGPoint(x: 10.6, y: 2.2))
        path.addLine(to: CGPoint(x: 7.5, y: 9.4))
        path.addLine(to: CGPoint(x: 13.5, y: 9.4))
        path.closeSubpath()
        arrow.path = path
        arrow.fillColor = NSColor.controlAccentColor.cgColor
        arrow.strokeColor = NSColor.white.cgColor
        arrow.lineWidth = 1.5
        arrow.lineJoin = .round
        arrow.shadowColor = NSColor.black.cgColor
        arrow.shadowOpacity = 0.35
        arrow.shadowRadius = 2
        arrow.shadowOffset = CGSize(width: 0, height: -1)
        arrow.frame = bounds
        addSublayer(arrow)
    }

    override init(layer: Any) { super.init(layer: layer) }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Move to `pointer` inside `frame`, the display image's rect in the
    /// parent layer. `scale` shrinks the arrow for small thumbnails.
    func update(_ pointer: AgentPointer?, displaySize: CGSize, in frame: CGRect, scale: CGFloat) {
        guard let pointer, displaySize.width > 0, displaySize.height > 0 else {
            shown = nil
            fade(to: 0)
            return
        }
        let target = CGPoint(x: frame.minX + pointer.point.x / displaySize.width * frame.width,
                             y: frame.maxY - pointer.point.y / displaySize.height * frame.height)
        let isNew = pointer.seq != shown?.seq
        let wasHidden = shown == nil
        shown = pointer
        transform = CATransform3DMakeScale(scale, scale, 1)

        if wasHidden || !isNew {
            position = target
        } else {
            let glide = CASpringAnimation(perceptualDuration: 0.32, bounce: 0.08)
            glide.keyPath = "position"
            glide.fromValue = NSValue(point: presentation()?.position ?? position)
            glide.toValue = NSValue(point: target)
            position = target
            add(glide, forKey: "glide")
        }
        fade(to: 1)
        if isNew && pointer.click { ripple(at: target, scale: scale) }
    }

    private func fade(to value: Float) {
        guard opacity != value else { return }
        let animation = CABasicAnimation(keyPath: "opacity")
        animation.fromValue = presentation()?.opacity ?? opacity
        animation.duration = 0.2
        opacity = value
        add(animation, forKey: "fade")
    }

    /// A ring that spreads from the tip once the arrow has arrived.
    private func ripple(at point: CGPoint, scale: CGFloat) {
        guard let parent = superlayer else { return }
        let size = 34 * scale
        let ring = CAShapeLayer()
        ring.frame = CGRect(x: point.x - size / 2, y: point.y - size / 2, width: size, height: size)
        ring.path = CGPath(ellipseIn: ring.bounds, transform: nil)
        ring.fillColor = NSColor.controlAccentColor.withAlphaComponent(0.18).cgColor
        ring.strokeColor = NSColor.controlAccentColor.cgColor
        ring.lineWidth = 2
        ring.opacity = 0
        parent.insertSublayer(ring, below: self)

        let start = ring.convertTime(CACurrentMediaTime(), from: nil) + 0.18
        let grow = CABasicAnimation(keyPath: "transform.scale")
        grow.fromValue = 0.2
        grow.toValue = 1
        let fade = CAKeyframeAnimation(keyPath: "opacity")
        fade.values = [1, 1, 0]
        fade.keyTimes = [0, 0.4, 1]
        let group = CAAnimationGroup()
        group.animations = [grow, fade]
        group.beginTime = start
        group.duration = 0.5
        group.timingFunction = CAMediaTimingFunction(name: .easeOut)
        CATransaction.begin()
        CATransaction.setCompletionBlock { ring.removeFromSuperlayer() }
        ring.add(group, forKey: "ripple")
        CATransaction.commit()
    }
}
