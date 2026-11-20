---
default: patch
---

Reduced CPU usage during `launch_app` 20 ms polling by skipping pre-existing unrelated PIDs from the pre-launch scan while re-checking every newly spawned PID on each 20 ms tick (catching `xpcproxy` to `exec` transitions immediately), checking single-PID liveness directly, reusing the initial PID scan, and filtering new windows without string allocations.
