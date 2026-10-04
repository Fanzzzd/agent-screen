#!/bin/bash
# Step 3: input into the agent compositor must not touch the user's.
set -u
source /usr/local/bin/isolation.sh --setup-only
run user foot; run agent foot -e sh -c 'cat > /tmp/typed.txt'; sleep 3
seat() { local ipc; eval "ipc=\$${1}_IPC"; SWAYSOCK=$ipc swaymsg -t get_tree | jq -r '.. | select(.focused? == true) | .name'; }
cursor() { local ipc; eval "ipc=\$${1}_IPC"; SWAYSOCK=$ipc swaymsg -t get_seats | jq -c '.[0] | {focus}'; }
echo "user focus before: $(seat user) $(cursor user)"
WAYLAND_DISPLAY=$agent_WL wtype "hello from the agent"; WAYLAND_DISPLAY=$agent_WL wtype -k Return
WAYLAND_DISPLAY=$agent_WL wlrctl pointer move 200 150; WAYLAND_DISPLAY=$agent_WL wlrctl pointer click left
sleep 1; pkill -f "cat > /tmp/typed.txt"; sleep 0.5
echo "user focus after:  $(seat user) $(cursor user)"
echo "typed on agent: $(cat /tmp/typed.txt 2>/dev/null)"
