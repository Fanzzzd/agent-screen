---
default: patch
---

Unsupported web drags now create an approval request in a queue shared by agent sessions, without sending a mouse-down first. The card names the agent that wants your mouse, shows each request, spells out what approval covers, and fits its content. During approval it shows a live thumbnail, and lets you stop and take control. New requests share an active approval until 30 seconds of queue inactivity. Agents can inspect, wait for or withdraw their own requests with `input_queue`; disconnecting or ending a display clears its work. This build provides approval and scheduling only. It has no real-input executor, and approval never reports an unexecuted drag as completed.

A drag can also use a slider's accessibility value when its range, orientation and current thumb position determine the direction. Ambiguous controls stay untouched. A value write must be confirmed before it is reported as successful; an uncertain result does not trigger a second drag. Successful drags identify their background route.
