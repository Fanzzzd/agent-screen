import AppKit

/// Moves windows onto a display in whatever way the user's window manager
/// respects. AeroSpace re-tiles any window moved behind its back, so with
/// AeroSpace running we ask it to move the window to the workspace shown on
/// the target display. Otherwise, or when AeroSpace ignores the display, we
/// use the Accessibility API.
@MainActor
enum Placement {
    enum Failure: Error, CustomStringConvertible {
        case aerospace(String)
        case accessibility(WindowMover.Failure)

        var description: String {
            switch self {
            case .aerospace(let message): "AeroSpace refused: \(message)"
            case .accessibility(let failure): "Accessibility refused: \(failure)"
            }
        }
    }

    static func place(_ target: WindowTarget, onto displayID: CGDirectDisplayID) async throws(Failure) {
        // AeroSpace configured with `ignored-monitors` doesn't list the display and leaves windows on it alone.
        if let aerospace = AeroSpace.current, let screen = NSScreen.screens.first(where: { $0.displayID == displayID }),
           let monitor = try? await aerospace.monitorID(named: screen.localizedName) {
            do {
                try await aerospace.move(target.windowID, toMonitor: monitor)
                return
            } catch {
                throw .aerospace("\(error)")
            }
        }
        do {
            try WindowMover.move(target, into: CGDisplayBounds(displayID))
        } catch {
            throw .accessibility(error)
        }
    }
}

/// The `aerospace` command line client.
struct AeroSpace {
    let executable: URL

    /// Non-nil while AeroSpace is running and its CLI is installed.
    @MainActor static var current: AeroSpace? {
        guard NSWorkspace.shared.runningApplications.contains(where: { $0.bundleIdentifier == "bobko.aerospace" })
        else { return nil }
        let candidates = ["/opt/homebrew/bin/aerospace", "/usr/local/bin/aerospace"]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
            .map { AeroSpace(executable: URL(fileURLWithPath: $0)) }
    }

    struct CommandError: Error, CustomStringConvertible {
        let description: String
    }

    func move(_ windowID: CGWindowID, toMonitor monitor: String) async throws {
        guard let workspace = try await visibleWorkspace(onMonitor: monitor) else {
            throw CommandError(description: "no visible workspace on monitor \(monitor)")
        }
        try await moveWindow(windowID, toWorkspace: workspace)
    }

    func moveWindow(_ windowID: CGWindowID, toWorkspace workspace: String) async throws {
        _ = try await run(["move-node-to-workspace", "--window-id", String(windowID), workspace])
    }

    /// The workspace a window belongs to.
    func workspace(of windowID: CGWindowID) async throws -> String? {
        try await run(["list-windows", "--all", "--format", "%{window-id}|%{workspace}"])
            .split(separator: "\n").compactMap { line -> String? in
                let parts = line.split(separator: "|", maxSplits: 1).map(String.init)
                return parts.count == 2 && parts[0] == String(windowID) ? parts[1] : nil
            }.first
    }

    func monitorID(named name: String) async throws -> String? {
        let monitors = try await run(["list-monitors", "--format", "%{monitor-id}|%{monitor-name}"])
        return monitors.split(separator: "\n").compactMap { line -> String? in
            let parts = line.split(separator: "|", maxSplits: 1).map(String.init)
            return parts.count == 2 && parts[1] == name ? parts[0] : nil
        }.first
    }

    func visibleWorkspace(onMonitor monitor: String) async throws -> String? {
        let name = try await run(["list-workspaces", "--monitor", monitor, "--visible"])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? nil : name
    }

    func focusedWorkspace() async throws -> String? {
        let name = try await run(["list-workspaces", "--focused"]).trimmingCharacters(in: .whitespacesAndNewlines)
        return name.isEmpty ? nil : name
    }

    func windowIDs(inWorkspace workspace: String) async throws -> [CGWindowID] {
        try await run(["list-windows", "--workspace", workspace, "--format", "%{window-id}"])
            .split(separator: "\n").compactMap { CGWindowID($0.trimmingCharacters(in: .whitespaces)) }
    }

    func moveWorkspace(_ workspace: String, toMonitor monitor: String) async throws {
        _ = try await run(["move-workspace-to-monitor", "--workspace", workspace, monitor])
    }

    func focus(workspace: String) async throws {
        _ = try await run(["workspace", workspace])
    }

    private func run(_ arguments: [String]) async throws -> String {
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let output = Pipe()
        let errors = Pipe()
        process.standardOutput = output
        process.standardError = errors
        try process.run()
        let data = await Task.detached { output.fileHandleForReading.readDataToEndOfFile() }.value
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(data: errors.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            throw CommandError(description: message.trimmingCharacters(in: .whitespacesAndNewlines))
        }
        return String(data: data, encoding: .utf8) ?? ""
    }
}
