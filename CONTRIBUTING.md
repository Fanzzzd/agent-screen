# Contributing to Agent Screen

Agent Screen gives each AI agent a virtual display of its own on a Mac. The agent clicks and types there through an MCP server, while the user's pointer, keyboard and windows stay with the user.

## Where to talk

Use [GitHub Issues](https://github.com/Fanzzzd/agent-screen/issues) for bugs, feature ideas and questions. The repo has no Discussions. Open an issue before a large change, so we agree on the behavior before you write it. Report a vulnerability privately, as [SECURITY.md](SECURITY.md) says, and not in an issue.

## Build and check

Agent Screen builds on a Mac with macOS 26's SDK. The toolchain is pinned in `rust-toolchain.toml`. [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) has the layout, the build and every test script.

```sh
pnpm install
pnpm check                          # what CI runs: cargo fmt, clippy -D warnings, cargo test
apps/macos/scripts/bundle.sh        # target/Agent Screen Dev.app, a debug bundle
```

Without Node, run the same checks with cargo:

```sh
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

The debug bundle is a separate app, Agent Screen Dev, with its own privacy grants and control socket, so it runs beside an installed Agent Screen.

The end-to-end scripts in `tests/e2e` drive the real app. They need a Mac where the debug app has Screen Recording and Accessibility, so CI doesn't run them. Run them beside your own Agent Screen with a separate instance:

```sh
AGENTSCREEN_INSTANCE=e2e cargo e2e robust
```

Every script shows a countdown sign on your main screen before it starts. Orange with "don't touch the mouse or keyboard" means the script uses your pointer, keyboard or screen, so keep your hands off until it ends. Blue means it stays on its own displays. `cargo e2e run` moves the real pointer and restarts the installed app, so run it when you can step away. Most scripts create virtual displays, and every new display makes macOS reconfigure all of them. Run scripts one at a time and don't loop them.

## Rules the code keeps

- Agent Screen exists so agents never take the user's pointer, keyboard or focus. Agent input goes to the target process through SkyLight ([ADR 0001](docs/adr/0001-background-input-through-skylight.md)). Nothing may warp the cursor, activate an app or raise a window on the user's screen to get a job done.
- Decisions about input, the display lifecycle, window ownership and AeroSpace live in [docs/adr](docs/adr). Read the one that touches your change. A change that goes against an ADR updates it, or adds a new ADR, in the same pull request.
- `crates/model` holds domain rules with no OS calls. Put a rule there when it can be tested without macOS, and test it there.
- UI copy is in both languages at the call site: `t("English", "中文")` or `tf!(…)`. Text sent to agents and log lines are English only.
- Every new UI surface gets a `debugShow` entry behind `debug_methods()`, so scripts can drive it without a real click. List it in docs/DEVELOPMENT.md. A new test script gets a line there too.
- [CONTEXT.md](CONTEXT.md) names the domain concepts: display, Takeover, Handover and the rest. Use those names in code, issues and pull requests.
- [README.md](README.md) describes what users see. A change to that behavior updates it.

## Commits and pull requests

Pull requests merge by squash. The pull request's title becomes the commit's subject on `main`, and its description becomes the commit's body. Both follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```
fix(mcp): keep a placed window on the display

place_window answered before the app moved the window back, so the
agent's next screenshot showed an empty display. It now waits until the
window has stayed on the display.

Fixes #123
```

- The subject reads `type(scope): summary`, at most 72 characters. Write the summary in the imperative and start it in lowercase, unless it starts with the name Agent Screen. It has no period.
- Types: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`, `build`, `ci`, `chore`, `revert`.
- Scopes, optional: `app` (apps/macos), `mcp` (crates/mcp), `model` (crates/model), `e2e` (tests/e2e), `website` (apps/website), `docs`, `release`.
- The body wraps at 72 characters. It says what changed for users and why.
- A footer `Fixes #N` closes the issue.
- A breaking change to the control socket's protocol ([ADR 0005](docs/adr/0005-rust-app-and-mcp.md)) or to an MCP tool's schema gets `!` after the type or scope, as in `feat(mcp)!: rename place_window's frame arguments`, and a `BREAKING CHANGE:` footer that says what callers must change.

CI checks the title on every pull request. Commits inside a branch can read however you like, since the squash replaces them.

The pull request template asks what changed and why, how you checked it, and whether it has a changeset. For a bug fix, show the repro failing before the fix and passing after it, with the command and the end of its output. Name the e2e scripts you ran. Add screenshots for a UI change.

## Changesets

A change users will notice comes with a changeset. Run `knope document-change`, pick patch, minor or major, and write the summary for users: what they saw before and what happens now. [knope](https://knope.tech) builds the version and `CHANGELOG.md` from changesets only and reads no commit messages. Tests, refactors and docs need none. CI warns when the app changes without one. Release steps are in docs/DEVELOPMENT.md, Release.

## Review and merge

The maintainer reviews every pull request. CI must pass: formatting, clippy, unit tests, the macOS 15 start check and the title check. The maintainer squash merges, and the branch is deleted after the merge.

## Pull requests written with AI

Pull requests written with an AI agent are welcome. They meet the same bar as any other: you have read the diff, the checks pass, and the repro or e2e output in the description is from a real run. `AGENTS.md` holds the rules agents follow in this repo.

## License

Agent Screen is MIT licensed. Your contribution is licensed the same way.
