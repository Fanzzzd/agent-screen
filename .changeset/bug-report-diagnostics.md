---
default: patch
---

Collect build information for a bug with the MCP tool `get_diagnostics` in the failing session, or `AgentScreen diagnostics --json` for the CLI and responding app. Reports use compiled package versions and source revisions, distinguish stale or unknown builds, and leave the app stopped if it is not running. The bug report form and reporting guide explain older-build fallbacks and what to attach.
