import AppKit

/// Keeps the user's cursor off Agent Displays. They sit outside anything
/// the user can see, so a cursor there is lost. A window manager can warp it
/// there: AeroSpace's `move-mouse monitor-lazy-center` does when focus moves
/// to a window on an Agent Display.
@MainActor
final class CursorGuard {
    private let displays: DisplayManager
    private var timer: Timer?
    private var lastGood: CGPoint?

    init(displays: DisplayManager) {
        self.displays = displays
        displays.observe { [weak self] in self?.update() }
        update()
    }

    private func update() {
        if displays.displays.isEmpty {
            timer?.invalidate()
            timer = nil
        } else if timer == nil {
            let timer = Timer(timeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.check() }
            }
            RunLoop.main.add(timer, forMode: .common)
            self.timer = timer
        }
    }

    private func check() {
        guard let location = CGEvent(source: nil)?.location else { return }
        guard displays.displays.contains(where: { $0.bounds.contains(location) }) else {
            lastGood = location
            return
        }
        let main = CGDisplayBounds(CGMainDisplayID())
        let back = lastGood ?? CGPoint(x: main.midX, y: main.midY)
        #if DEBUG
        NSLog("Sidescreen cursor: pulled back from \(location) to \(back)")
        #endif
        CGWarpMouseCursorPosition(back)
        CGAssociateMouseAndMouseCursorPosition(1)
    }
}
