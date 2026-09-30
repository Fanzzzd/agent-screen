import AppKit

/// Keeps the user's AeroSpace workspaces off Agent Displays.
///
/// Stock AeroSpace manages every display, virtual ones included. When a new
/// display appears it shows some workspace there, sometimes one that holds the
/// user's windows. After every display change the guard looks at the
/// workspace shown on each Agent Display. If it holds windows Sidescreen did
/// not put there, the guard moves that workspace back to the main monitor and
/// refocuses the workspace the user was on.
///
/// Workspaces that the user's config force-assigns to a monitor number cannot
/// be moved; a virtual display takes over that number just by existing. Those
/// are reported as conflicts so the menu can explain the fix.
@MainActor
final class AeroSpaceGuard {
    struct Conflict: Equatable {
        let workspace: String
        let display: String
    }

    private(set) var conflicts: [Conflict] = []
    private let displays: DisplayManager
    private var pending: Task<Void, Never>?

    init(displays: DisplayManager) {
        self.displays = displays
        displays.observe { [weak self] in self?.scheduleCheck() }
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.scheduleCheck() }
        }
    }

    /// AeroSpace reacts to display changes on its own schedule; wait for it
    /// to settle, then check once.
    func scheduleCheck() {
        pending?.cancel()
        pending = Task {
            try? await Task.sleep(for: .milliseconds(900))
            guard !Task.isCancelled else { return }
            await checkAll()
        }
    }

    private func checkAll() async {
        guard let aerospace = AeroSpace.current else {
            conflicts = []
            return
        }
        var found: [Conflict] = []
        for display in displays.displays {
            if let conflict = await check(display, with: aerospace) { found.append(conflict) }
        }
        if found != conflicts {
            conflicts = found
            for conflict in found {
                NSLog("Sidescreen: AeroSpace pins workspace \(conflict.workspace) to \(conflict.display)")
            }
        }
    }

    private func check(_ display: VirtualDisplay, with aerospace: AeroSpace) async -> Conflict? {
        guard let name = NSScreen.screens.first(where: { $0.displayID == display.id })?.localizedName,
              let monitor = try? await aerospace.monitorID(named: name)
        else { return nil }

        // Each eviction makes AeroSpace pick another workspace; stop after a few.
        for _ in 0..<3 {
            guard let workspace = try? await aerospace.visibleWorkspace(onMonitor: monitor),
                  let windows = try? await aerospace.windowIDs(inWorkspace: workspace)
            else { return nil }
            if windows.allSatisfy(display.windows.contains) { return nil }

            let focused = try? await aerospace.focusedWorkspace()
            do {
                try await aerospace.moveWorkspace(workspace, toMonitor: "main")
            } catch {
                return Conflict(workspace: workspace, display: name)
            }
            // The moved workspace is now showing on the main monitor.
            if let focused, focused != workspace { try? await aerospace.focus(workspace: focused) }
        }
        return nil
    }
}
