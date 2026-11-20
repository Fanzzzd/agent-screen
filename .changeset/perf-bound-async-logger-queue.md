---
default: patch
---

Bounded the asynchronous stderr logger queue to 4096 lines, dropping oldest entries when stderr stalls and writing a warning line with the dropped count once writes resume.
