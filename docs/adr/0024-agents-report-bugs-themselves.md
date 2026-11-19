# Agents report Agent Screen bugs themselves

Accepted, 2026-11-20. Replaces the server instructions' rule "Do not auto-submit or upload user content" from the diagnostics work ([Reporting bugs](../REPORTING_BUGS.md)). Issue #237.

When an agent hit an Agent Screen bug, the report rarely reached us. The instructions told the agent to collect `get_diagnostics` and attach it to a GitHub issue by hand, and not to submit anything itself. An agent without `gh`, or in a client with no network, could not file anything, so a report depended on the user noticing and doing the work. Reports that did arrive often lacked the exact build, so triage could not tell a bug fixed in a later release from one that still happens.

## Decision

Agents report Agent Screen bugs with a new MCP tool, `report_issue`. The server instructions now say so; `get_diagnostics` stays for reports a person files by hand.

The agent gives a title, its steps (tool calls as `{tool, arguments, result}`, or sentences), what it expected, what happened, whether it can reproduce it, and optionally the failing tool and its exact error. The host adds what the agent can't know: the Agent Screen version and build revision, the macOS version, the client's name and version from MCP `initialize` (or the request's `_meta` on MCP 2026-07-28), whether AeroSpace runs, the session's `get_diagnostics` report, and the display log's lines about the session's displays. Every field is plain JSON with no client-specific part, so every client gets the same tool.

The report fills the fields of the repository's bug report form (`.github/ISSUE_TEMPLATE/bug_report.yml`), since the repository accepts no blank issues. Its fingerprint, `agentscreen-` and 12 hex digits, hashes the tool and its error with numbers folded, so the same failure from two agents or two runs has the same one. It is written into the report, where GitHub's search finds it.

### Privacy

Reports are public, and they are redacted before anything leaves the Mac (`agent_screen_model::report::Redactor`, no OS calls, unit tested):

- Window titles on the session's displays become `[window title]`. The app reads them when the report is made. The Display Labels' task titles, the project folder and the account name become `[private]`.
- Quoted text becomes `[text]`, which is how agents write what they typed or what a window said. Agent Screen's own words stay: tool names, actions, error codes.
- Links, bare host names on common top-level domains, file paths (`/…/…`, `~/…`, `C:\…`) and email addresses become `[link]`, `[path]` and `[email]`. Paths and links are replaced before known names, so a folder name inside a path can't split the path.
- Data URLs and runs of 64 or more base64 or hex characters, which is what a screenshot in text looks like, become `[data]`. No screenshot, recording or image is ever part of a report.
- Tool-call arguments keep their keys and only the values of keys that carry Agent Screen's own data: handles, coordinates, actions, sizes, flags. `text`, `title`, `url`, `user_instruction`, element text and every other value become `[removed]`. A tool that isn't Agent Screen's becomes `[another tool]`.
- The diagnostics report already holds only compiled build fields.

Redaction can't know a window title it was never told about, such as one from a window that has left the display. It is caught only when the agent quoted it. The tool's description tells the agent not to put user content in a report, and the user sees the full form before submitting it.

The tool's result carries the report exactly as it would leave the Mac.

### Duplicates and fixes

Before sending, the host searches GitHub's public issues for the fingerprint, unauthenticated (`GET /search/issues`, 10 searches a minute), and reads the latest release (`GET /repos/…/releases/latest`). Both go through the system's `curl` with a 6-second limit and a generic user agent, `agent-screen/<version>`.

- An open issue with the fingerprint: the tool returns `duplicate` with its number and files nothing.
- A closed one, when this Mac runs a release older than the latest: `fixed_in_newer_release`. The agent asks the user to update, and nothing is filed.
- A closed one on the latest release or newer: the report goes out, noting a possible regression of that issue.
- No answer from GitHub: the report goes out, and the result says the check was unavailable.

### Sending

The host hands the report to a `Sender` (`crates/mcp/src/report.rs`). This build has two adapters:

- `Form`, the one in use: the control method `fileReport` asks the app to open the prefilled new-issue form, at most 8000 characters long, shortening the log, then the diagnostics, then the longest field. The first report waits on the Bug Report Card under the menu bar icon, in the Help Card's family, until the user presses Open on GitHub, which is their consent for later reports too. After that, a report opens the form in the background with `open -g`, so the browser never takes the user's focus. The issue exists only once the user presses Submit, under their own account.
- `Relay`, off: it would POST the redacted report to a service we run. No service exists, so its endpoint, `RELAY`, is unset and the form is used.

The user stays in charge. Not Now drops a report. Turn Off, on the card, in the menu's Agent Bug Reports or in Settings, stops reports: the tool then returns `off` with the report for the agent to show the user, and reads nothing from GitHub. The menu lists the reports opened as "Reported", newest first, and opens each again. The same fingerprint opens at most once a day, and at most three forms open an hour. The setting, the consent and the last five reports are kept per instance in `reports-<instance>.json` in the state folder.

The app's control methods are `reportContext`, which returns the setting, the macOS version, whether AeroSpace runs, the titles and names to redact and the session's log lines, and `fileReport`, which takes the redacted title, fingerprint and a `https://github.com/` form URL. Both work without the privacy permissions.

## The owner's options for filing without the user

The issue asks for reports filed with no `gh` and no user token on the Mac. The form needs the user's GitHub account; filing as a service needs credentials we hold, which can't ship in the app, where anyone could extract them. That leaves two shapes, both needing a service we run:

1. **A relay with a GitHub App behind it. Recommended.** Create a GitHub App, say `agent-screen-reports`, with the repository permissions Issues: Read and write and Metadata: Read, and nothing else. Install it on `Fanzzzd/agent-screen` only. Run a small HTTPS service, for example a Cloudflare Worker, holding the App's id, private key and installation id as secrets. It mints an installation token per request (valid 1 hour, limited to this repository). Its `POST /report` takes `{title, body, fingerprint, version}` and:
   - rejects a title over 120 characters, a body over 16 KB and a malformed fingerprint;
   - runs the same redaction check again and rejects a body with a link, path or email outside the markers;
   - searches the fingerprint, comments on an open match ("also seen on 0.1.x, Codex 0.50") instead of filing, at most one comment per version, and files a new issue with the labels `bug`, `needs-triage` and `agent-report` otherwise;
   - limits each IP address to 5 reports an hour and 20 a day, and everyone to 100 a day;
   - has a switch that stops all filing, for an abuse wave.

   Then set `RELAY` to its endpoint, gate the relay sender behind the same consent as the form (ask the app with `fileReport` before posting), and release. Issues appear as `agent-screen-reports[bot]`, the key can be rotated without an app update, and GitHub allows an installation 5000 API requests an hour.
2. **A relay with a fine-grained personal access token**, limited to this repository and Issues: Read and write. Same service and limits. Simpler to set up, but issues appear as the owner, the token expires (at most a year), and it is tied to one person's account. Not recommended.

A GitHub App's user authorization (device flow) would file as the user without a relay, but it puts a user token on the Mac, which the issue rules out.

Until the owner chooses, duplicates come back as the existing issue's number instead of a comment, and the follow-up the issue describes, telling the agent at its next `create_display` that its issue was fixed, waits for the relay too.

## Consequences

- An agent files a report with one tool call in any MCP client, with no `gh` or token on the Mac. The user submits it in their browser.
- Every report carries the Agent Screen version, build revision, macOS version and the client's name and version, so triage can tell a fixed bug from a live one.
- A report never includes a screenshot, and drops what redaction recognizes as the user's. What it can't recognize is listed above.
- The first report needs one click from the user. Reports stop when the user turns them off, and nothing about reports is sent to GitHub then.
