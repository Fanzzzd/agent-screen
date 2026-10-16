---
default: patch
---

# Typing through the input method works call after call

Several problems with `type` and `key` with `via_input_method` are fixed:

- After a call, an app that ignores the Accessibility request to come to the front, such as an Electron app, stayed behind the agent's app. Switching your input source back then committed the agent's composition as plain letters. Sidescreen now waits until your app is in front, activates it another way when it doesn't come, and only then switches your input source back.
- A second call sent its keys nowhere: the agent's window had lost key status while your app was in front. Each call now makes the window key with a click on its title bar first.
- A call right after another was refused as if you were typing, because the keys Sidescreen had just posted reset the idle timer. Sidescreen now tells its own events apart from yours.
- The agent's input source takes over before the first key, so the first letter is composed too.
- Sidescreen could crash when an input source changed during a call. It now makes input source calls on the main thread.

The tool description now says to compose and commit in one call, such as "nihao " for 你好: what the input method holds is dropped when your app comes back to the front.
