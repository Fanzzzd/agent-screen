---
agent-screen: patch
---

An agent can keep working in its window of a browser you're using in another window (#110). It used to have its keys refused while that browser was your front app, and its clicks there were sometimes lost. Now Agent Screen sends no keys or clicks to that browser at all, since they would reach your window. Instead, a plain left click presses the button or link under it, or picks the text field under it, through accessibility. Typed text goes into the picked field and is read back. Return, Escape, Backspace and Command-A act on what the click picked. Other keys, such as Tab or shortcuts, still fail, and the error names the window that has focus. Your window keeps its focus and selection throughout.
