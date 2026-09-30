import QuartzCore

/// Animation vocabulary shared by both panels. Layers of layer-backed NSViews
/// use a (0, 0) anchor point, so every transform here is built for that.
enum Motion {
    /// The open morph: settles in about half a second with a slight overshoot.
    static func openSpring(_ keyPath: String, from: Any?, to: Any?) -> CASpringAnimation {
        spring(keyPath, from: from, to: to, duration: 0.5, bounce: 0.16)
    }

    /// Small UI reactions such as hover lift and the preview drop-in.
    static func snappySpring(_ keyPath: String, from: Any?, to: Any?) -> CASpringAnimation {
        spring(keyPath, from: from, to: to, duration: 0.32, bounce: 0.12)
    }

    static func spring(_ keyPath: String, from: Any?, to: Any?, duration: CGFloat, bounce: CGFloat)
        -> CASpringAnimation
    {
        let animation = CASpringAnimation(perceptualDuration: duration, bounce: bounce)
        animation.keyPath = keyPath
        animation.fromValue = from
        animation.toValue = to
        animation.duration = animation.settlingDuration
        animation.fillMode = .both
        return animation
    }

    static func fade(from: Float, to: Float, duration: CFTimeInterval, delay: CFTimeInterval = 0)
        -> CABasicAnimation
    {
        let animation = CABasicAnimation(keyPath: "opacity")
        animation.fromValue = from
        animation.toValue = to
        animation.duration = duration
        animation.beginTime = delay
        animation.fillMode = .both
        animation.timingFunction = CAMediaTimingFunction(name: .easeOut)
        return animation
    }

    /// Transform that shrinks a layer occupying `frame` so it covers `source`
    /// (both in the superlayer's space), keeping its aspect ratio and centering it.
    static func collapse(_ frame: CGRect, into source: CGRect) -> CATransform3D {
        let scale = max(source.width / frame.width, source.height / frame.height)
        let tx = source.midX - scale * frame.width / 2 - frame.minX
        let ty = source.midY - scale * frame.height / 2 - frame.minY
        return CATransform3DConcat(
            CATransform3DMakeScale(scale, scale, 1),
            CATransform3DMakeTranslation(tx, ty, 0)
        )
    }

    /// Scale around a point given in the layer's own bounds.
    static func scale(_ factor: CGFloat, around point: CGPoint, offsetY: CGFloat = 0) -> CATransform3D {
        var transform = CATransform3DMakeTranslation(-point.x, -point.y, 0)
        transform = CATransform3DConcat(transform, CATransform3DMakeScale(factor, factor, 1))
        return CATransform3DConcat(transform, CATransform3DMakeTranslation(point.x, point.y + offsetY, 0))
    }
}
