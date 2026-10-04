#!/bin/bash
# Two headless sway compositors for one user and one $HOME:
#   user  = the user's desktop (WAYLAND_DISPLAY=wayland-1, its own D-Bus)
#   agent = an Agent Display   (WAYLAND_DISPLAY=wayland-2, a private D-Bus)
# Each app starts on "user" first, then again on "agent". The result says
# which compositor got the second window.
set -u
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
cfg=/tmp/sway.cfg
printf 'xwayland enable\noutput HEADLESS-1 resolution 1440x900\n' > $cfg

start() { # name socket -> exports <NAME>_DBUS, <NAME>_X
  local name=$1 sock=$2
  eval "$(dbus-launch --sh-syntax)"
  local bus=$DBUS_SESSION_BUS_ADDRESS
  WAYLAND_DISPLAY= DBUS_SESSION_BUS_ADDRESS=$bus sway -c $cfg >/tmp/sway-$name.log 2>&1 &
  for i in $(seq 1 50); do [ -S "$XDG_RUNTIME_DIR/$sock" ] && break; sleep 0.2; done
  # sway names its socket wayland-N in start order.
  local ipc=$(ls -t $XDG_RUNTIME_DIR/sway-ipc.*.sock | head -1)
  # Xwayland's socket appears with sway; the newest one is this sway's.
  for i in $(seq 1 25); do [ -n "$(ls /tmp/.X11-unix 2>/dev/null | grep -v -x -F -f /tmp/xseen 2>/dev/null)" ] && break; sleep 0.2; done
  local x=$(ls /tmp/.X11-unix | grep -v -x -F -f /tmp/xseen 2>/dev/null | head -1 | tr X :)
  ls /tmp/.X11-unix >> /tmp/xseen
  eval "${name}_BUS='$bus'; ${name}_IPC='$ipc'; ${name}_WL='$sock'; ${name}_X='$x'"
}
touch /tmp/xseen
start user wayland-1
start agent wayland-2
sleep 1
xdisp() { eval "echo \$${1}_X"; }

run() { # where app...
  local where=$1; shift
  local bus ipc wl
  eval "bus=\$${where}_BUS; ipc=\$${where}_IPC; wl=\$${where}_WL"
  SWAYSOCK=$ipc WAYLAND_DISPLAY=$wl DBUS_SESSION_BUS_ADDRESS=$bus QT_QPA_PLATFORM=wayland \
    DISPLAY=$(xdisp $where) "$@" >/tmp/app-$where-$(basename $1).log 2>&1 &
}
windows() { # where -> one line per app window: app_id or class, title
  local ipc; eval "ipc=\$${1}_IPC"
  SWAYSOCK=$ipc swaymsg -t get_tree | jq -r '.. | select(.type? == "con" or .type? == "floating_con") | select(.pid? != null) | "\(.app_id // .window_properties.class) \"\(.name)\""'
}
count() { windows $1 | grep -c . ; }

test_app() { # label command...
  local label=$1; shift
  run user "$@"; sleep 6
  local u1=$(count user) a1=$(count agent)
  run agent "$@"; sleep 6
  local u2=$(count user) a2=$(count agent)
  local verdict
  if [ $a2 -gt $a1 ]; then verdict="agent (isolated)"
  elif [ $u2 -gt $u1 ]; then verdict="user (handed to the running instance)"
  else verdict="nowhere (refused or exited)"; fi
  printf '%-22s user %s->%s  agent %s->%s  second window: %s\n' "$label" $u1 $u2 $a1 $a2 "$verdict"
  [ $a2 -gt $a1 ] && windows agent | sed 's/^/      agent: /'
  [ $u2 -gt $u1 ] && windows user | sed 's/^/      user:  /'
  [ $a2 -eq $a1 ] && [ $u2 -eq $u1 ] && tail -3 /tmp/app-agent-$(basename $1).log | sed 's/^/      log: /'
  pkill -f "$(basename $1)" ; sleep 1
}

[ "${1:-}" = "--setup-only" ] && return 0
echo "user Xwayland $(xdisp user), agent Xwayland $(xdisp agent)"
test_app "foot (Wayland)"      foot
test_app "xterm (X11)"         xterm
test_app "gnome-text-editor"   gnome-text-editor
test_app "kwrite (Qt)"         kwrite
test_app "firefox-esr"         firefox-esr --new-window about:blank
test_app "chromium"            chromium --no-sandbox --ozone-platform=wayland --new-window about:blank
SWAYSOCK=$agent_IPC WAYLAND_DISPLAY=$agent_WL grim /tmp/agent.png && echo "capture: grim on agent ok ($(stat -c %s /tmp/agent.png) bytes)"

echo
echo "Mitigation A: the agent's copy gets its own profile; the user's keeps the default"
test_pair() { # label user-command ... -- agent-command ...
  local label=$1; shift; local u=() 
  while [ "$1" != "--" ]; do u+=("$1"); shift; done; shift
  run user "${u[@]}"; sleep 6
  local u1=$(count user) a1=$(count agent)
  run agent "$@"; sleep 6
  local u2=$(count user) a2=$(count agent)
  printf '%-22s user %s->%s  agent %s->%s\n' "$label" $u1 $u2 $a1 $a2
  windows agent | sed 's/^/      agent: /'
  pkill -f "$(basename $1)"; sleep 1
}
mkdir -p /tmp/agent-firefox
test_pair "firefox --profile" firefox-esr --new-window about:blank -- firefox-esr --profile /tmp/agent-firefox --new-window about:blank
test_pair "chromium --user-data" chromium --no-sandbox --ozone-platform=wayland --new-window about:blank -- chromium --no-sandbox --ozone-platform=wayland --user-data-dir=/tmp/agent-chromium --new-window about:blank

echo
echo "Mitigation B: a separate HOME for every agent app"
mkdir -p /tmp/agent-home
run_home() { local where=$1; shift; HOME=/tmp/agent-home run "$where" "$@"; }
test_home() { # label command...: user side keeps the real HOME
  local label=$1; shift
  run user "$@"; sleep 6
  local u1=$(count user) a1=$(count agent)
  run_home agent "$@"; sleep 6
  local u2=$(count user) a2=$(count agent)
  printf '%-22s user %s->%s  agent %s->%s\n' "$label" $u1 $u2 $a1 $a2
  windows agent | sed 's/^/      agent: /'
  pkill -f "$(basename $1)"; sleep 1
}
test_home "firefox HOME"        firefox-esr --new-window about:blank
test_home "chromium HOME"       chromium --no-sandbox --ozone-platform=wayland --new-window about:blank
test_home "kwrite HOME"         kwrite
