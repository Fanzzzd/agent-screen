---
default: patch
---

# A development build no longer interferes with the installed app

A debug build of Sidescreen is now its own app, "Sidescreen Dev", with its own bundle id, privacy grants, control socket and agent registration (`sidescreen-dev`), and a purple menu bar icon. Before, it shared the installed app's bundle id, which could cost the installed app its Screen Recording and Accessibility grants, and Connect in a dev build replaced the installed app's registration with Claude Code and Codex.
