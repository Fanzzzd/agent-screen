---
default: patch
---

Agent Screen retries an AeroSpace claim-file write after it fails, even when the next request repeats the same claim. It keeps the intended displays and windows separate from the last published file, so a failed display registration does not prevent later window claims.
