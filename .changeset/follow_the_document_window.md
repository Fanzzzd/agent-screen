---
default: patch
---

# launch_app follows an app to its document window

Pages, given a text file, closes the window it opened first and shows the document in another. launch_app kept trying to place the closed one and failed. It now places the app's new window instead.
