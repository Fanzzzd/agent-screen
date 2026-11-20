---
default: patch
---

Moved stderr log writes onto a dedicated FIFO writer thread so log pipe backpressure never stalls the main UI thread while preserving line order and flushing on panic or exit.
