# Agents report Agent Screen bugs themselves

Accepted, 2026-11-20, revised the same day after review: every form report waits for a click, nothing is read from GitHub before the user's consent, and redaction covers more patterns. Replaces the server instructions' rule "Do not auto-submit or upload user content" from the diagnostics work ([Reporting bugs](../REPORTING_BUGS.md)). Issue #237.

When an agent hit an Agent Screen bug, the report rarely reached us. The instructions told the agent to collect `get_diagnostics` and attach it to a GitHub issue by hand, and not to submit anything itself. An agent without `gh`, or in a client with no network, could not file anything, so a report depended on the user noticing and doing the work. Reports that did arrive often lacked the exact build, so triage could not tell a bug fixed in a later release from one that still happens.

## Decision

Agents report Agent Screen bugs with a new MCP tool, `report_issue`. The server instructions now say so; `get_diagnostics` stays for reports a person files by hand.

The agent gives a title, its steps as sentences, its tool calls as `{tool, arguments, result}` in a separate list, what it expected, what happened, whether it can reproduce it, and optionally the failing tool and its exact error. Steps and calls are two plain lists, with no `anyOf`, which some clients' schema converters refuse. The host adds what the agent can't know: the app's version and build revision (the app's, not the client's process's, which may be older), the macOS version, the client's name and version from MCP `initialize` (or the request's `_meta` on MCP 2026-07-28), whether AeroSpace runs, the session's `get_diagnostics` report, and the display log's lines about the session's displays. Every field is plain JSON with no client-specific part, so every client gets the same tool. When the client's `AgentScreen mcp` process is an older build than the app, every answer says to restart the client, not to update Agent Screen.

The report fills the fields of the repository's bug report form (`.github/ISSUE_TEMPLATE/bug_report.yml`), since the repository accepts no blank issues. Its fingerprint, `agentscreen-` and 12 hex digits, hashes the tool and its error with numbers folded, so the same failure from two agents or two runs has the same one. It is written into the report, where GitHub's search finds it.

### Privacy

Reports are public. Redaction (`agent_screen_model::report::Redactor`, no OS calls, unit tested) replaces what it recognizes before the report reaches the card:

- Window titles on the session's displays become `[window title]`. The app reads them when the report is made. The Display Labels' task titles, the project folder, the account name, the home folder and the Mac's names (`ComputerName`, `LocalHostName`, `hostname`) become `[private]`, and so does the user's full name, whole or as any word of three letters or more of it, in any case.
- Quoted text becomes `[text]`, which is how agents write what they typed or what a window said. Agent Screen's own words stay: tool names, actions, error codes.
- Links become `[link]`. Host names, any dotted name ending in letters that isn't a file name (`acme.internal`, `Printer.local`), `localhost` and IPv4 and IPv6 addresses become `[host]`. Paths (`/…`, `~…`, `./…`, `C:\…`, and any word with a slash between words, such as `Documents/taxes`) become `[path]`. A dotted name with a common file extension (`Q3-salaries.xlsx`) becomes `[file]`, and email addresses `[email]`. Paths and links are replaced before known names, so a folder name inside a path can't split the path.
- Keys and tokens of any length become `[secret]`: GitHub's `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_` and `github_pat_`, `sk-` and `sk_`, Slack's `xox…-`, GitLab's `glpat-`, npm's `npm_`, Google's `AIza`, AWS access key ids (`AKIA…`, `ASIA…`), JSON Web Tokens (`eyJ…`), the word after `Bearer` or `Basic`, and the value of `password=`, `token:`, `api_key=` and similar keys.
- Data URLs, runs of 64 or more base64 or hex characters, and runs of 32 or more that mix letters and digits become `[data]`. No screenshot, recording or image is ever part of a report.
- Tool-call arguments keep their keys and only the values of keys that carry Agent Screen's own data: handles, coordinates, actions, sizes, flags. `text`, `title`, `url`, `user_instruction`, element text and every other value become `[removed]`. A tool that isn't Agent Screen's becomes `[another tool]`.
- The diagnostics report already holds only compiled build fields.

Chinese, Japanese and Korean prose has no spaces, so a token also ends at full-width punctuation and where that text meets Latin text: "连接git.acme.internal失败" has the token `git.acme.internal`. A path stays one token across a slash, so a folder named in Chinese goes with it. A key counts wherever it starts after a character that isn't a letter or digit, and a name of two Chinese characters, such as 张伟, is matched anywhere in the text.

This is a denylist over free text, and it can't recognize every private word: unquoted words the user typed, a window title it was never told about, a name or a place in prose. So every claim says "redacts known patterns; review before sending", the tool's description tells the agent to write only about Agent Screen's behavior, and the user reads the whole report on the card before anything leaves the Mac.

The tool's result carries the report exactly as it would leave the Mac.

### Duplicates and fixes

Nothing leaves the Mac before the user's consent, so the first report is not checked. Once the user has opened a report from the card, a later report first searches GitHub's public issues for its fingerprint, unauthenticated (`GET /search/issues`, 10 searches a minute), and reads the latest release (`GET /repos/…/releases/latest`). Both run at once, through the system's `curl` with a 6-second limit and a generic user agent, `agent-screen/<version>`, and stop when the client cancels the call.

- An open issue with the fingerprint: the tool returns `duplicate` with its number, and nothing waits on the card.
- An issue closed as fixed (`state_reason` `completed`), when the app is older than the latest release: `fixed_in_newer_release`. The agent asks the user to update, and nothing waits.
- An issue closed as fixed, when the app runs the latest release or newer: the report waits on the card, noting a possible regression of that issue. An issue closed as not planned or as a duplicate counts as no match.
- No answer from GitHub: the report waits on the card, and the result says the check was unavailable.

### Sending

The host hands the report to a `Sender` (`crates/mcp/src/report.rs`). Both adapters first give it to the app with the control method `fileReport`, which puts it on the Bug Report Card under the menu bar icon, in the Help Card's family. The card names the agent and the title, shows the whole report on Show Report (the title and the agent's fields first, then the app's facts), and says what Open on GitHub does. This build has two adapters:

- `Form`, the one in use. Every report waits on the card. Open on GitHub opens the prefilled new-issue form of that one report, in front, since the user just clicked; one click never opens several tabs. The form is at most 8000 characters long, shortening the log, then the diagnostics, then the longest field. Its content reaches github.com when the page loads, as the URL's query, so the click is the consent to send it; the issue exists only once the user presses Submit, under their own account. An agent's call never opens a browser window: opening one would put a tab or a window on the user's screen, which AeroSpace's rules can then focus or move.
- `Relay`, off: it would POST the redacted report to a service we run. No service exists, so its endpoint, `RELAY`, is unset and the form is used. Even when set, it posts only when `fileReport` answers `send`, which the app does only after the user opened a report from a card that said "later reports without asking". That card shows only in a build with `RELAY` set; a click on the form-only card never agrees to the relay. Otherwise the report waits on the card like a form report.

The user stays in charge. Not Now drops a report. Turn Off, on the card, in the menu's Agent Bug Reports or in Settings, stops reports: the tool then returns `off` with the report for the agent to show the user, and reads nothing from GitHub. The menu lists the reports the user opened as "Reported", newest first, and opens each again. The same fingerprint waits or goes out at most once a day, at most five reports wait, and the relay would file at most three an hour. The setting, the two consents and the last five reports are kept per instance in `reports-<instance>.json` in the state folder.

The app's control methods are `reportContext`, which returns the setting, the consent, the app's version, revision and dirty flag, the macOS version, whether AeroSpace runs, the titles and names to redact, the user's full name and the session's log lines (from the last 256 KB of the display log), and `fileReport`, which takes the redacted title, fingerprint, report text and a form URL that starts with `https://github.com/Fanzzzd/agent-screen/issues/new?template=bug_report.yml&`. Both work without the privacy permissions.

## The owner's options for filing without the user

The issue asks for reports filed with no `gh` and no user token on the Mac. The form needs the user's GitHub account; filing as a service needs credentials we hold, which can't ship in the app, where anyone could extract them. That leaves two shapes, both needing a service we run:

1. **A relay with a GitHub App behind it. Recommended.** Create a GitHub App, say `agent-screen-reports`, with the repository permissions Issues: Read and write and Metadata: Read, and nothing else. Install it on `Fanzzzd/agent-screen` only. Run a small HTTPS service, for example a Cloudflare Worker, holding the App's id, private key and installation id as secrets. It mints an installation token per request (valid 1 hour, limited to this repository). Its `POST /report` takes `{title, body, fingerprint, version}` and:
   - rejects a title over 120 characters, a body over 16 KB and a malformed fingerprint;
   - runs the same redaction check again and rejects a body with a link, path or email outside the markers;
   - searches the fingerprint, comments on an open match ("also seen on 0.1.x, Codex 0.50") instead of filing, at most one comment per version, and files a new issue with the labels `bug`, `needs-triage` and `agent-report` otherwise;
   - limits each IP address to 5 reports an hour and 20 a day, and everyone to 100 a day;
   - has a switch that stops all filing, for an abuse wave.

   Then set `RELAY` to its endpoint and release. The relay sender already asks the app with `fileReport` before posting, and the card then says later reports go out without asking. Issues appear as `agent-screen-reports[bot]`, the key can be rotated without an app update, and GitHub allows an installation 5000 API requests an hour.
2. **A relay with a fine-grained personal access token**, limited to this repository and Issues: Read and write. Same service and limits. Simpler to set up, but issues appear as the owner, the token expires (at most a year), and it is tied to one person's account. Not recommended.

A GitHub App's user authorization (device flow) would file as the user without a relay, but it puts a user token on the Mac, which the issue rules out.

Until the owner chooses, duplicates come back as the existing issue's number instead of a comment, and the follow-up the issue describes, telling the agent at its next `create_display` that its issue was fixed, waits for the relay too.

## Consequences

- An agent files a report with one tool call in any MCP client, with no `gh` or token on the Mac. The user reviews it on the card, opens it with a click and submits it in their browser.
- Every report carries the app's version and build revision, the macOS version and the client's name and version, so triage can tell a fixed bug from a live one.
- A report never includes a screenshot. Redaction replaces the patterns listed above; what it can't recognize, the user sees on the card before sending.
- Nothing leaves the Mac before the user's click: not the report, and not a GitHub search. Reports stop when the user turns them off.
- The first report on a Mac is never checked for duplicates; later ones are.
