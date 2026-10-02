import AppKit

@main
struct SidescreenMain {
    @MainActor static func main() {
        let args = CommandLine.arguments.dropFirst()
        if args.first == "--probe" {
            exit(Probe.run(Array(args.dropFirst())))
        }

        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)
        app.run()
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let displays = DisplayManager()
    private let hub = StreamHub()
    private let forwarders = ForwarderPool()
    private var control: ControlServer?
    private var status: StatusItemController?
    private var aerospace: AeroSpaceGuard?
    private var notifier: HelpNotifier?
    private var cursorGuard: CursorGuard?

    func applicationDidFinishLaunching(_ notification: Notification) {
        Permissions.requestMissing()

        let preview = PreviewPanelController(hub: hub, displays: displays)
        let viewer = ControlPanelController(hub: hub, forwarders: forwarders, displays: displays)
        let aerospace = AeroSpaceGuard(displays: displays)
        self.aerospace = aerospace
        cursorGuard = CursorGuard(displays: displays)
        status = StatusItemController(displays: displays, hub: hub, forwarders: forwarders,
                                      preview: preview, control: viewer, aerospace: aerospace)

        let notifier = HelpNotifier(displays: displays)
        notifier.onOpen = { [weak self] id in self?.status?.openForHelp(id) }
        self.notifier = notifier

        let agents = AgentControl(displays: displays, forwarders: forwarders)
        let server = ControlServer(
            handle: { connection, method, params in
                try await agents.handle(connection: connection, method: method, params: Params(raw: params))
            },
            onDisconnect: { agents.connectionClosed($0) }
        )
        do {
            try server.start()
            control = server
        } catch {
            NSLog("Sidescreen: control socket failed: \(error)")
        }

        displays.observe { [hub, displays] in
            displays.displays.forEach { hub.keepWarm($0.id) }
        }

        // Development: SIDESCREEN_DEMO=panel|preview opens UI without a click.
        if let demo = ProcessInfo.processInfo.environment["SIDESCREEN_DEMO"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                MainActor.assumeIsolated { self.status?.demo(demo) }
            }
        }
    }

    /// A normal quit tidies up as dismissal would, without asking: each
    /// window follows its Disposition. A crash leaves the windows to macOS.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let plans = displays.displays.map(Dismissal.plan(for:))
        guard plans.contains(where: { !$0.isEmpty }) else { return .terminateNow }
        Task {
            for items in plans { await Dismissal.execute(items) }
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}
