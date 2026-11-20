---
default: patch
---

Released the `SCShareableContent` cache mutex before calling `SCShareableContent::get()` so concurrent captures across Agent Displays do not serialize on one listing, and reused a single WindowServer window scan inside `screenshot::filter_counting`.
