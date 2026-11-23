---
default: patch
---

Typing a few characters at a time into a slow web page no longer loses keys. Each `type` call now waits until its own characters show in the field before it returns, so the next call's keys no longer arrive while the page is still handling the last ones. Before, a short call returned as soon as its characters showed anywhere in the field, and a page that drops keys that come too fast lost some without a word.
