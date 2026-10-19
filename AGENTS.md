# Repository Guidelines

`CLAUDE.md` is a symlink to this file. Edit only this one.

This file is for agents working on Agent Screen's code. `docs/agents/AGENTS.md` is a different file: the snippet users paste into their own projects so agents use Agent Screen's tools.

## Read before the task

| Task                                                      | Read                                                |
| --------------------------------------------------------- | --------------------------------------------------- |
| Anything that names a domain concept (display, Takeover…) | `CONTEXT.md`                                        |
| Input, display lifecycle, window ownership, AeroSpace     | the matching ADR in `docs/adr/`                     |
| Building, tests, debug control methods, releases          | `docs/DEVELOPMENT.md`                               |
| User-visible behavior                                     | `README.md`, which describes it and changes with it |

## Constraints

- Agent Screen exists so agents never take the user's pointer, keyboard or focus. Agent input goes to the target process through SkyLight (ADR 0001). Nothing in the app warps the cursor, activates an app, or raises a window on the user's screen to get a job done.
- Displays are addressed by the handle `create_display` returns (ADR 0002). The control socket's NDJSON protocol is the contract between `Agent Screen mcp` and the app (ADR 0005).
- `rust/agent-screen-model` holds domain rules with no OS calls. Put a rule there when it can be tested without macOS.
- A display ends in review, and a Takeover ends only when the user hands control back (ADR 0004). A new window belongs to the agent only under ADR 0007's rule.
- Agent Screen ignores AeroSpace's closet display (vendor 0x4145, model 1) everywhere it lists displays (`mac::cg::is_closet`).
- UI copy: `t("English", "中文")` or `tf!(…)` at the call site, both languages every time. Text sent to agents and log lines are English only.
- Test-only control methods (`debugShow`, `debugTakeover`, …) sit behind `debug_methods()`. Give every new UI surface a `debugShow` entry so scripts can drive it without a real click.

## Testing on this Mac

The developer's Agent Screen, AeroSpace and apps are running while you work.

- Run scripts beside the user's app with `AGENTSCREEN_INSTANCE=e2e`. It is a separate instance with its own socket and displays.
- `cargo e2e run` moves the real pointer. Run it only when the user is away or asks for it.
- TextEdit, Safari and Preview may be the user's. The scripts skip when they are running; don't work around that. Quit Calculator after a script that opens it.
- Don't move the user's windows and don't switch their AeroSpace workspace. If you do, put it back and say so.
- Create virtual displays sparingly, one at a time, and never from several agents at once. Each new display makes WindowServer and ColorSync reconfigure; dozens in a few hours once froze the machine.
- `rust/agent-screen/scripts/install.sh` replaces the installed app and ends its Agent Displays, including other agents' displays. Check before running it.

## Checks

Before committing, run what CI runs:

```sh
(cd rust && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace)
```

The test scripts are Rust too, in `rust/tests` (see `docs/DEVELOPMENT.md`, Test). Clippy covers them; CI doesn't run them, since they need a Mac with the app's grants.

## Delivery

- A change users will notice gets a changeset in `.changeset/`, written for users (see `docs/DEVELOPMENT.md`, Release).
- Commit messages are plain sentences. knope ignores conventional commits here.
- A new test script gets a line in `docs/DEVELOPMENT.md`'s test list. A new or renamed debug surface goes in its `debugShow` list.

## Agent skills

### Issue tracker

Issues live in GitHub Issues of the public repo `Fanzzzd/agent-screen`, via the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five default labels, unchanged. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.
