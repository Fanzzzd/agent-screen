# Reporting bugs

Open a [bug report](https://github.com/Fanzzzd/agent-screen/issues/new?template=bug_report.yml) for a reproducible Agent Screen problem. Reports are public. For a vulnerability, use the private route in [SECURITY.md](../SECURITY.md).

## Collect the failing session first

1. In the MCP session where the problem occurred, call **`get_diagnostics`** with `{}`. Paste its report into the issue. This identifies that MCP process and the app that answers its control endpoint. It works before `create_display` and leaves ongoing sessions alone.
2. Include the MCP client's name and version, the MCP registration used, your macOS version from About This Mac, and whether AeroSpace is running. Include the tool calls, error text and steps that reproduce the problem. State what you expected and what happened.
3. If you also need to identify the installed CLI, run:

   ```sh
   "/Applications/Agent Screen.app/Contents/MacOS/AgentScreen" diagnostics --json
   ```

   `AgentScreen diagnostics` gives a text summary. `AgentScreen diagnostics --help` explains the command. Use the path you actually configured if the app is elsewhere. For the development bundle, use `target/Agent Screen Dev.app/Contents/MacOS/AgentScreen`; its default instance is `dev`. An explicit `AGENTSCREEN_INSTANCE` selects that instance's endpoint. A report identifies only the app responding there.

If an older build lacks `get_diagnostics` or `diagnostics`, write **unavailable** for that report and paste:

```sh
"/Applications/Agent Screen.app/Contents/MacOS/AgentScreen" --version
```

Say which app or MCP identities remain unknown. An old `--version` may print only a version number. It identifies the executable you just ran, not a previously running app or MCP process. You can report the original failure without upgrading first.

Diagnostics opens a separate read-only control connection with a two-second connection-and-reply deadline. It does not start the app, create a display, register or end a session, send input, change settings, restart clients, enumerate other sessions or upload anything. A missing app remains stopped. The caller receives an explicit unavailable, timeout, unsupported, malformed or refused status instead of a guessed app identity.

## Shared-browser reports

The current `place_window` schema has `x`, `y`, `width` and `height` in screenshot pixels. If a client does not expose them, retain its same-session `get_diagnostics` output or record the tool as unavailable. A newly started CLI cannot prove which tool schema that failing session loaded. Do not restart an active session just to collect this evidence.

An explicit frame request can move or shrink a zoomed window below the reserved menu strip. Read the returned actual frame and any constraint note. This does not automatically correct every zoom operation. For a window already on the target Agent Display, a call without frame arguments leaves its frame unchanged. Initial placement still moves or fits a window onto that display as before.

For a shared browser's refused keys, record whether the error reports a different focused window or an unknown focused window. Neither error proves that the user was typing. Keep the refusal intact; user idleness does not establish safe keyboard routing. For small scrolls in a virtualized list, include the requested amounts and observed movement. Event metadata alone does not prove how the browser received or applied a scroll. A browser without a private DevTools pipe still cannot receive a conflict-free page drag. A fresh profile has separate sign-ins.

## Read the identity correctly

- **`collector: "mcp"`** identifies the MCP process serving this tool call. This is the report to collect from the failing session.
- **`collector: "cli"`** identifies only the CLI process you just invoked. Its `mcp_status` is `not_collected`. Starting a new CLI cannot prove what an older MCP has loaded.
- **`app.identity`** comes from compiled metadata in the app that answered the read-only request. A legacy app that rejects the method has no verified identity in the report.
- **`package_version`** is the Cargo package version compiled into that process. `git_revision` is its full source revision, when available. `dirty: true` means the build had local changes. Missing git metadata is `null`, including source archives. The report records the Cargo profile and target too. It contains no build timestamp or executable-file checksum.
- **`bundle_version`** is `null`, with `bundle_version_status: "not_collected"`. A bundle's on-disk Info.plist can change after the process starts. It cannot reliably identify the loaded process. Historical bundle versions may also differ from Cargo package versions.
- **`comparison`** compares the collector with the responding app. `different` means their known metadata differs. `unknown` includes missing metadata and dirty builds whose equality cannot be proven. `same_source_metadata` means complete clean source metadata agrees. It does not assert byte-identical executables or prove that a fix works.

`report_schema_version` versions this report. `control_protocol_version` versions the additive diagnostics control contract, not the negotiated MCP protocol. OS name and process architecture are included. Add the macOS product version yourself. Instances are `default`, `dev`, `e2e`, or `custom_redacted`; custom names stay private.

The checkout's current Git HEAD describes source on disk. It does not identify a running app or MCP. Likewise, a hash of an executable at `current_exe` describes the file now at that path. An MCP process may still have an older, replaced executable mapped in memory. Diagnostics uses that process's compiled constants and does not label an on-disk hash as a loaded MCP hash.

## Stale clients and unverified fixes

Updating or replacing Agent Screen does not update an MCP process that is already running. Collect `get_diagnostics` in the failing session before restarting it. If it lacks the tool, record that fact and the identity as unknown. A new CLI report cannot fill that gap.

When the session's work is safe to end, use your MCP client's controls to restart that connection or start a new client session. Reconnect to the intended Agent Screen registration, then collect `get_diagnostics` again. Ending a session can leave its display waiting for review. Diagnostics never restarts the client for you and never reconnects or stops unrelated sessions.

To verify a proposed fix, keep the original reproduction and its failing result. Record the build actually executing the retest, then run the same reproduction and attach its result. For a GUI bug, include separate GUI evidence from that build. If you only checked source, ran unit tests, or could not reproduce the original failure, say **unverified**. Matching versions alone do not verify a fix.

## Privacy and sample

The report permits compiled build fields, collector, app status, instance category, OS/architecture, comparison and fixed guidance. It does not collect executable paths, host names, account names, environment dumps, window titles, prompts, documents, screenshots or logs. Extra control-response fields and raw error messages are discarded. Review any logs, screenshots or recordings you choose to attach separately. Remove secrets and unrelated user content.

This is a schema-1 excerpt with unknown build information. It is an illustration, not output from a real installation:

```json
{
  "report_schema_version": 1,
  "collector": "cli",
  "mcp_status": "not_collected",
  "app": { "status": "unsupported", "identity": null },
  "instance": "default",
  "os": { "name": "macos", "arch": "aarch64" },
  "comparison": "unknown"
}
```

Attach the full report from your command or tool call. Keep `null`, dirty and unavailable fields intact; they explain what was not verified.
