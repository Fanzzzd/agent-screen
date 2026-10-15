---
default: minor
---

# Smooth Display Changes: a helper keeps macOS's display records short

The setup window has a new optional step, Smooth Display Changes. It asks for your administrator password once and installs a small tool that Sidescreen runs as root through `sudo`. It runs after a display is created or ended, and every 30 minutes. It removes macOS's color records of displays that aren't connected, and keeps display names in English and your languages. macOS reads every record, once per display online, on each display change, so with many records the whole Mac stuttered. Profiles you chose stay. `Sidescreen colorsync remove` takes the tool away. `Sidescreen colorsync reclaim` runs it at once.

When the reads would take longer than macOS waits for them, `create_display` now says so and asks the agent to use or end a display it has, instead of slowing the Mac down.
