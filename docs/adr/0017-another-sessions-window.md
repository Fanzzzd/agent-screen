# A window another session holds goes to a new session only on the user's word

`place_window` used to refuse a window on another Agent Session's display, saying other agent sessions held it. One session needed WeChat while another session's display had it. It could only message the other session and ask it to let go, and neither agent could tell whether the user wanted that. Moving the window without the user is ruled out: from the window alone, Agent Screen can't tell whether the holder is still working in it.

So a request for such a window goes to the user. `place_window` for a window held by another session opens a Window Request, the same card and notification as for the user's own windows (ADR 0007). The card names the asking agent, the window and the holder: "Codex asks for WeChat. Claude Code · task B is using it." Hand Over moves the window to the asker's display as a Placement. Keep leaves it where it is, and the asker's call fails with a reply naming the holder. The call waits up to 120 s by default, and a request nobody answers is withdrawn, as before.

A window is held by another session when it sits on that session's live Agent Display, or when it sits on the user's screen and that session's display records it or started its app. A window on a Review Display goes through the same request, whichever session stopped using it, and Hand Over takes it out of review. A display no session uses, such as one the user made from Peek, holds nothing this way; its windows still need the user to bring them out.

A Takeover of the holding display doesn't stop the request. The user is the one operating that display, so they can answer, and Hand Over moves the window even while the Takeover goes on.

A user's instruction in the conversation can stand for the card (ADR 0013), but only when it names the window or its app. The session relays it as `user_instruction`. Agent Screen checks that the words contain the app's name, its owner name, its bundle id or the window's title, ignoring case. Words that name neither, such as "go ahead", open the card instead. A Window Authorization never covers another session's window: it is permission to take the user's windows, and this window is in another agent's work. Only the user's answer or their explicit words move it.

The holder learns what happened through the calls it already makes, without a push channel. Its display drops the window's record and remembers that the user gave it away. A call that names the window on that display answers that the user gave the app to another agent session, rather than that no such window exists. The holder's next `list_windows`, screenshot or window state on that display says it once. The window no longer counts as the holder's stray, and its record no longer keeps it from the asker, even though the holder's session started the app.

A window the user handed over to the holder stays a Handover for the asker, with its original return location. When the asker's display ends, the window goes back to the user, never to the holder: the holder's claim on it was the user's gift, and the user now gave it elsewhere. Any other window becomes a Placement on the asker's display and stays for review when that display ends.

The same rule answers the reverse case. A holder that wants the window back calls `place_window` for it, which asks the user in the same way.
