---
default: patch
---

Batched Accessibility attribute reads in `press_new_window`, `belongs`, `item_name`, and `dock_app_at` using `AXUIElementCopyMultipleAttributeValues`, and replaced `wait_for_change`'s per-poll window presence check with a single zero-string-allocation WindowServer lookup.
