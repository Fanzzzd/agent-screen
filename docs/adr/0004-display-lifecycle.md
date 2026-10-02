# A display ends in review, and control returns only when handed back

When an Agent Session stops using its display, by `end_display` or a closed connection, the display closes by itself if no window is on it, and otherwise becomes a Review Display that waits for the user. We do not keep every ended display: they piled up in Peek as dimmed "已结束" tiles that users did not understand. We do not close every ended display either: returning windows unasked would move them into the user's workspace mid-task, and the user may still want to look at the result. Neither reason is a claim that the task succeeded, because `end_display` and a dropped connection look the same here; the copy says the agent stopped, never that it finished.

A Takeover ends only when the user presses 让 … 继续. Collapsing the Viewer, switching displays or clicking elsewhere used to hand control back, so an agent could resume while the user had only glanced away. Opening the Viewer is watching, not operating: the Viewer takes the keyboard only after a click into the display.

A window counts as on a display if it touches the display and its center is on no other display, so a window hanging off an edge is never left behind when the display closes.
