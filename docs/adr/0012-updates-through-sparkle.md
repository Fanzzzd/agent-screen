# Updates through Sparkle

Agent Screen had no way to update itself: users ran `brew upgrade --cask agent-screen` or downloaded a DMG when they happened to look. Fixes reached few of them.

Release builds embed Sparkle 2, the update framework most Mac apps outside the App Store use. `scripts/sparkle.sh` fetches a pinned version and checks its hash; `bundle.sh` puts `Sparkle.framework` in the release bundle and signs its parts under the hardened runtime. Agent Screen isn't sandboxed, so Sparkle's XPC services stay out. The development bundle carries no Sparkle and never updates.

## The feed

- `SUFeedURL` is `appcast.xml` of the latest GitHub release. `release.sh` writes it beside the DMG, and `release.yml` uploads it with the release. It names the DMG users download, its length, its EdDSA signature and the release's minimum macOS.
- The DMG is signed with an EdDSA key. Its public half is `SUPublicEDKey` in `Info.plist`, and the private half is the `SPARKLE_ED_PRIVATE_KEY` secret. Sparkle refuses an update whose signature doesn't match, and one whose code signature doesn't match the running app's.

## When it installs

Installing replaces the app and restarts it. A restart ends every Agent Display and the agents' work on them (ADR 0004), so an update never installs in the middle of that work.

- Sparkle checks once a day and downloads in the background (`SUAutomaticallyUpdate`), showing nothing.
- Agent Screen takes the install from Sparkle. It runs it once no Agent Display has been open for two checks 30 s apart. Agents' MCP servers reconnect after the restart.
- Meanwhile the status menu offers **Restart to Update to x.y.z…**. With Agent Displays open, it says how many the restart ends and asks first.
- Quitting installs it too.
- **Check for Updates…** in the status menu shows Sparkle's own window. Its Install and Relaunch asks the same question when Agent Displays are open.

Only the user's own Agent Screen updates. A test or development instance doesn't, unless a script gives it a test feed with `AGENTSCREEN_DEBUG=1` and `AGENTSCREEN_UPDATE_FEED`.

## Consequences

- Homebrew's cask says `auto_updates true`, so `brew upgrade` leaves the app to update itself.
- 0.1.17 and earlier have no Sparkle: users update to the first version with it by hand, once.
- A release run that has to be finished after the DMG went up keeps the release's own `appcast.xml`, since a rebuilt DMG has another signature.
