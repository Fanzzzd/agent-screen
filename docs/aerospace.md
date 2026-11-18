# Agent Screen with AeroSpace

Only for AeroSpace users. Without a window manager, nothing here applies.

Stock AeroSpace treats agent displays as monitors. It shows one of your workspaces on each new agent display, and it files an agent's new window in your focused workspace, so the window can pass over your screen for a frame or two before Agent Screen moves it. Agent Screen moves your workspaces back to the monitors they were on.

To stop this at the source, AeroSpace needs an `ignored-monitors` option, [proposed upstream](https://github.com/nikitabobko/AeroSpace/pull/2306). Until it lands, install our build of AeroSpace, which is AeroSpace plus that one patch. Your config stays as it is.

```sh
brew tap fanzzzd/agent-screen
brew trust fanzzzd/agent-screen
brew uninstall --cask aerospace
brew install --cask aerospace-agent-screen
tccutil reset Accessibility bobko.aerospace
open -a AeroSpace
```

Then press **Set Up…** on the AeroSpace row in Agent Screen's Settings, or add this line to your AeroSpace config yourself:

```toml
ignored-monitors = ['^Agent Screen']
```

macOS keeps Accessibility permission per app signature, so the old AeroSpace's entry doesn't work for this build. `tccutil reset` clears it, and macOS asks again when AeroSpace starts.

## What you install

The build follows AeroSpace's releases. Each one is built, tested and signed in [CI](https://github.com/Fanzzzd/AeroSpace/actions/workflows/agent-screen-release.yml). [Compare it with upstream](https://github.com/Fanzzzd/AeroSpace/compare/v0.21.3-Beta...agent-screen) to see the patch: 168 lines added and 20 removed across 26 files, most of them tests and docs. Why a fork: [ADR 0006](adr/0006-aerospace-ignores-agent-displays.md).

## Going back

Remove the `ignored-monitors` line from your config first: stock AeroSpace doesn't know the option and refuses the config. Then:

```sh
brew uninstall --cask aerospace-agent-screen
brew install --cask nikitabobko/tap/aerospace
tccutil reset Accessibility bobko.aerospace
open -a AeroSpace
```
