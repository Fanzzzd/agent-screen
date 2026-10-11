---
default: patch
---

# launch_app says why there is no window

When `launch_app` returns no window it now gives a `reason`: `still_starting`, `existing_window` (an app you were running opened the file in a window it had) or `no_new_window` (an app you were running made none, such as Calculator), with the next step for each. For an app you were running, Sidescreen stops waiting 2 to 3 seconds after its request, so a window you open in that app afterwards always stays on your screen.
