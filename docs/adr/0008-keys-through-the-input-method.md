# Keys through the input method borrow the user's keyboard, on request

ADR 0001 posts keys to the target process and keeps it in the background. An input method (pinyin, Japanese, Korean) composes only for the app with keyboard focus, so it never sees those keys. An Agent Session that has to reproduce an input-method bug, such as composition breaking on blur, can't do it with `type`, which inserts finished text.

`type` and `key` take `via_input_method`. With it, Agent Screen gives the target app keyboard focus for the call, posts the keys at the session level as a keyboard would, then gives focus back to the app the user had in front. `input_source` names a source to use for the call; the user's comes back after. `type` turns the text into the keys a US keyboard presses for it, so the agent types the letters the input method composes from.

This breaks ADR 0001's rule on purpose. For the call, the user's keys reach the agent's app and the agent's keys reach whichever app has focus. So:

- The agent asks for it on each call. Nothing turns it on for a session.
- It runs only when the user hasn't touched the keyboard or pointer for 2 s, and stops at the next touch.
- It runs only when the target app has keyboard focus, checked before every key. A modal system prompt, such as a keychain password request, keeps focus; Agent Screen then sends nothing. It also refuses while secure input is on, which a password field turns on.
- It takes at most 250 keys, about 10 s, a call.
- The Viewer and Peek say the agent is using the user's keyboard while it lasts.

We tried a session-level post without the focus check while a keychain prompt was in front. The keys went into its password field. The checks above came from that.

A Takeover doesn't cover this case: the user's input method composes in the Viewer, and Agent Screen passes the target only finished text.
