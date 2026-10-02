import AppKit

/// Dismissing an Agent Display: every window on it is either returned to the
/// user or closed, then the display goes away.
///
/// The agent's Disposition decides each window by default. Without one, the
/// windows the agent opened close and everything else returns. A Handover
/// always returns, whatever the agent said. The user sees the plan and can
/// change any window before confirming.
@MainActor
enum Dismissal {
    enum Fate { case giveBack, close }

    struct Item {
        let target: WindowTarget
        let app: String
        let title: String
        var fate: Fate
        /// A Handover: the user's own window, never closed.
        let locked: Bool
        let home: WindowHome?
        /// Why the default is what it is, shown under the window's name.
        var reason = ""

        /// "Safari · Flights", or just the app when the title adds nothing.
        var name: String { title.isEmpty || title == app ? app : "\(app) · \(title)" }
    }

    static func plan(for display: VirtualDisplay) -> [Item] {
        let launchedApps = Set(display.records.values.filter { $0.origin == .launched }.map(\.target.pid))
        return WindowCatalog.windows(on: display).map { window in
            let record = display.records[window.target.windowID]
            let ofLaunchedApp = launchedApps.contains(window.target.pid)
            return Item(target: window.target, app: window.app, title: window.title,
                        fate: fate(origin: record?.origin, disposition: record?.disposition, ofLaunchedApp: ofLaunchedApp),
                        locked: record?.origin == .handedOver, home: record?.home,
                        reason: reason(origin: record?.origin, disposition: record?.disposition,
                                       ofLaunchedApp: ofLaunchedApp))
        }
    }

    /// The default for one window. `origin` is nil for a window Sidescreen
    /// never tracked.
    nonisolated static func fate(origin: WindowOrigin?, disposition: Disposition?, ofLaunchedApp: Bool) -> Fate {
        switch (origin, disposition) {
        case (.handedOver, _): .giveBack
        case (_, .keep): .giveBack
        case (_, .close): .close
        case (.launched, nil): .close
        case (.placed, nil), (.found, nil): .giveBack
        // A further window of an app the agent launched is the agent's;
        // anything else may be the user's.
        case (nil, nil): ofLaunchedApp ? .close : .giveBack
        }
    }

    private static func reason(origin: WindowOrigin?, disposition: Disposition?, ofLaunchedApp: Bool) -> String {
        switch (origin, disposition) {
        case (.handedOver, _): "你交给 agent 的窗口"
        case (_, .keep): "agent 建议保留"
        case (_, .close): "agent 建议关掉"
        case (.launched, nil): "agent 打开的，没给意见"
        case (.placed, nil), (.found, nil): "agent 移进来的，没给意见"
        case (nil, nil): ofLaunchedApp ? "agent 打开的 app 的窗口" : "不是 agent 放进来的"
        }
    }

    /// Whether to ask before dismissing. An empty display nobody uses
    /// just goes.
    static func needsConfirmation(_ items: [Item], for display: VirtualDisplay) -> Bool {
        !items.isEmpty || isLive(display)
    }

    static func isLive(_ display: VirtualDisplay) -> Bool { display.owner != nil && !display.ended }

    /// Close and return windows. Returns the windows that refused to close,
    /// for example because they ask about unsaved changes.
    @discardableResult
    static func execute(_ items: [Item]) async -> [Item] {
        var stubborn: [Item] = []
        for item in items where item.fate == .close {
            do { try WindowMover.close(item.target) } catch { stubborn.append(item) }
        }
        for item in items where item.fate == .giveBack {
            await giveBack(item)
        }
        if !stubborn.isEmpty {
            // Leave them to macOS, which moves them to the main display
            // when the virtual one disappears.
            NSLog("Sidescreen: \(stubborn.count) window(s) did not close on dismissal")
        }
        return stubborn
    }

    /// A window returns to where it came from when we know that, otherwise
    /// to the main display.
    static func giveBack(_ item: Item) async {
        if let home = item.home {
            if let workspace = home.workspace, let aerospace = AeroSpace.current,
               (try? await aerospace.moveWindow(item.target.windowID, toWorkspace: workspace)) != nil {
                return
            }
            if (try? WindowMover.setFrame(item.target, to: home.frame)) != nil { return }
        }
        do { try await Placement.place(item.target, onto: CGMainDisplayID()) } catch {
            NSLog("Sidescreen: could not return window \(item.target.windowID): \(error)")
        }
    }
}
