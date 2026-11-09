---
default: patch
---

Recover input monitoring after a macOS event-tap timeout. Previously, Allow could appear to do nothing because the app immediately revoked every new approval after that timeout. The app now re-enables and checks the input monitor after ending the previous grant and releasing its inputs. Queued actions still need a fresh Allow; recovery never resumes an old approval.
