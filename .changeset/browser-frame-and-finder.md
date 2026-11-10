---
default: patch
---

`place_window` reports the frame a window ended with when the app answers a resize with an error but takes it anyway, as some apps such as Dia do. It used to fail with "the app rejected the change" for a window that had moved and resized. It fails only when the window kept its frame, and then says which frame that is.
