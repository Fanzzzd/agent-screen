#!/bin/bash
# Step 3b: each D-Bus session has its own AT-SPI bus, so the agent sees only its apps.
set -u
source /usr/local/bin/isolation.sh --setup-only
export GTK_A11Y=atspi NO_AT_BRIDGE=0
run user gnome-text-editor; run agent foot; sleep 1; run agent gnome-text-editor; sleep 6
apps() { local bus; eval "bus=\$${1}_BUS"; DBUS_SESSION_BUS_ADDRESS=$bus python3 - <<'PY'
import pyatspi
d = pyatspi.Registry.getDesktop(0)
for app in d:
    if app is None: continue
    names = [c.name for c in app if c is not None][:2]
    print(f"  {app.name}: {names}")
PY
}
echo "user bus:";  apps user
echo "agent bus:"; apps agent
