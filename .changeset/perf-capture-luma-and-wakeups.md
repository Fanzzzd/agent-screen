---
default: patch
---

Stopped CPU IOSurface luma scanning between agent settles (keeping `"agent-warm"` at 30 fps and seeding the luma baseline from the warm frame when counting resumes), reused the luma scratch buffer, cached `display_bounds` per frame in `Capture::start` with a live re-read on mismatch, coalesced and UI-rate-limited `Streams` main-thread wakeups with a trailing wake for throttled bursts, reset repair backoff from verified frames even when UI wakes are suppressed, and moved `wait_until_still` change polling off the main thread.
