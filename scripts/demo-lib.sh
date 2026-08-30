#!/usr/bin/env bash
#
# Shared helpers for driving dimnav from the outside: window placement, key
# injection and screen capture. Sourced by record-demo.sh and shots-demo.sh.
#
# Requires Accessibility permission for whatever runs it (Terminal, iTerm, ...),
# and Screen Recording permission for the capture helpers.

APP_PROCESS="dimnav"
WIN_X=${WIN_X:-80}
WIN_Y=${WIN_Y:-80}
WIN_W=${WIN_W:-1280}
WIN_H=${WIN_H:-800}

# macOS virtual key codes, as a case rather than an associative array: macOS
# still ships bash 3.2, which predates `declare -A`, and requiring a Homebrew
# bash for a screenshot script is a poor trade.
keycode() {
  case "$1" in
    return)   echo 36  ;; tab)      echo 48  ;; space) echo 49 ;;
    backspace) echo 51 ;; escape)   echo 53  ;;
    left)     echo 123 ;; right)    echo 124 ;;
    down)     echo 125 ;; up)       echo 126 ;;
    pageup)   echo 116 ;; pagedown) echo 121 ;;
    home)     echo 115 ;; end)      echo 119 ;;
    f1) echo 122 ;; f2) echo 120 ;; f3) echo 99  ;; f4) echo 118 ;;
    f5) echo 96  ;; f6) echo 97  ;; f7) echo 98  ;; f8) echo 100 ;;
    *) return 1 ;;
  esac
}

die() { echo "error: $*" >&2; exit 1; }

require_app() {
  pgrep -x "$APP_PROCESS" >/dev/null \
    || die "$APP_PROCESS is not running. Start it with: npm run tauri dev"
}

# Raise and pin the window. `set frontmost` alone does not raise a Tauri
# window; AXRaise is the part that actually brings it forward.
place_window() {
  osascript >/dev/null <<EOF
tell application "System Events" to tell process "$APP_PROCESS"
  set frontmost to true
  perform action "AXRaise" of window 1
  set position of window 1 to {$WIN_X, $WIN_Y}
  set size of window 1 to {$WIN_W, $WIN_H}
end tell
EOF
  sleep 0.6
  local got
  got=$(osascript <<EOF
tell application "System Events" to tell process "$APP_PROCESS"
  set p to position of window 1
  set s to size of window 1
  return ((item 1 of p) as text) & "," & ((item 2 of p) as text) & "," & ((item 1 of s) as text) & "," & ((item 2 of s) as text)
end tell
EOF
)
  [[ "$got" == "$WIN_X,$WIN_Y,$WIN_W,$WIN_H" ]] \
    || die "window would not take the requested geometry (got $got)"
}

# k <name> [delay]  - press a named key
k() {
  local code
  code=$(keycode "$1") || die "unknown key: $1"
  osascript -e "tell application \"System Events\" to key code $code" >/dev/null
  sleep "${2:-0.45}"
}

# kmod <name> <modifiers> [delay]  - e.g. kmod f6 "shift down"
kmod() {
  local code
  code=$(keycode "$1") || die "unknown key: $1"
  osascript -e "tell application \"System Events\" to key code $code using {$2}" >/dev/null
  sleep "${3:-0.45}"
}

# chord <char> <modifiers> [delay]  - e.g. chord t "command down"
#
# Cmd+Shift chords must be spelled with an explicit shift and a lower-case
# letter: with Command held, WebKit reports the unshifted character, so
# `keystroke "T" using command down` arrives indistinguishable from Cmd+T.
chord() {
  osascript -e "tell application \"System Events\" to keystroke \"$1\" using {$2}" >/dev/null
  sleep "${3:-0.45}"
}

# press <char> [delay]  - a bare printable key, no modifiers (e.g. * or -)
press() {
  local esc=${1//\\/\\\\}; esc=${esc//\"/\\\"}
  osascript -e "tell application \"System Events\" to keystroke \"$esc\"" >/dev/null
  sleep "${2:-0.3}"
}

# type <text> [delay]  - literal text into whatever holds the keyboard
type_text() {
  local esc=${1//\\/\\\\}; esc=${esc//\"/\\\"}
  osascript -e "tell application \"System Events\" to keystroke \"$esc\"" >/dev/null
  sleep "${2:-0.4}"
}

# Run a command in dimnav's own terminal. `cd` is a built-in that navigates the
# active panel, which is how each panel is pointed at the demo tree without
# touching config.toml.
run_in_app_terminal() {
  chord t "command down" 0.5
  type_text "$1" 0.35
  k return 0.9
  chord t "command down" 0.5
}

# The demo tree's images and archives are sparse placeholders, not real files,
# so an Enter that reaches a panel launches Preview and lands its "could not be
# opened" sheet on top of whatever was about to be captured. Clearing it before
# each shot keeps one mistimed keystroke from silently ruining a frame.
dismiss_intruders() {
  # `tell application "Preview" to quit` LAUNCHES Preview when it is not already
  # running, and then waits on it - so an unconditional quit is a hang, not a
  # no-op. Only ever address an app that is genuinely up.
  local app
  for app in Preview "QuickTime Player"; do
    if pgrep -x "$app" >/dev/null 2>&1; then
      osascript -e "tell application \"$app\" to quit" >/dev/null 2>&1 || true
    fi
  done
  sleep 0.3
  osascript >/dev/null 2>&1 <<EOF || true
tell application "System Events" to tell process "$APP_PROCESS"
  set frontmost to true
  perform action "AXRaise" of window 1
end tell
EOF
  sleep 0.3
}

# --- deterministic starting state -----------------------------------------
#
# Panel directories, sort, view mode, hidden files, theme AND the terminal pane
# size are all persisted in config.toml, which means they survive a relaunch:
# leaving the output pane open in one run reopens it in the next, and the shot
# after that is quietly wrong. Writing the config before launching is what makes
# a capture reproducible - and it removes the need to drive the panels there
# with `cd` at all.
#
# The user's own config is backed up on first write and restored by an EXIT
# trap, so running this on a real machine does not cost anyone their settings.

CONFIG_DIR="$HOME/Library/Application Support/dimnav"
CONFIG_FILE="$CONFIG_DIR/config.toml"
# The backup lives at a FIXED path, not a mktemp one, and the generated config
# carries a marker. Together they survive a run that is killed before its trap
# fires: without both, the next run backs up the leftover demo config as though
# it were the user's, and the real settings are gone for good. (This is not
# hypothetical - it ate an `appearance` setting during development.)
CONFIG_MARKER="# dimnav-demo-capture: transient, restored by scripts/demo-lib.sh"
CONFIG_BACKUP_FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.demo-out/config.user.toml"
CONFIG_TRAPPED=""

config_backup() {
  mkdir -p "$(dirname "$CONFIG_BACKUP_FILE")"
  if [[ -z "$CONFIG_TRAPPED" ]]; then
    trap config_restore EXIT INT TERM
    CONFIG_TRAPPED=1
  fi
  # An existing backup is always the authoritative copy of the real settings.
  [[ -f "$CONFIG_BACKUP_FILE" ]] && return 0
  if [[ -f "$CONFIG_FILE" ]]; then
    if grep -qF "$CONFIG_MARKER" "$CONFIG_FILE"; then
      echo "warning: $CONFIG_FILE is a leftover demo config and no backup exists;" >&2
      echo "         the original settings were lost by an earlier interrupted run." >&2
      return 0
    fi
    cp "$CONFIG_FILE" "$CONFIG_BACKUP_FILE"
  else
    printf '__none__\n' > "$CONFIG_BACKUP_FILE"
  fi
}

config_restore() {
  [[ -f "$CONFIG_BACKUP_FILE" ]] || return 0
  if [[ "$(head -1 "$CONFIG_BACKUP_FILE")" == "__none__" ]]; then
    rm -f "$CONFIG_FILE"
  else
    mkdir -p "$CONFIG_DIR"
    cp "$CONFIG_BACKUP_FILE" "$CONFIG_FILE"
  fi
  rm -f "$CONFIG_BACKUP_FILE"
}

# write_demo_config <left_dir> <right_dir> [term_size] [columns] [theme]
write_demo_config() {
  local left="$1" right="$2"
  local term="${3:-collapsed}" cols="${4:-2}" theme="${5:-classic}"
  config_backup
  mkdir -p "$CONFIG_DIR"
  cat > "$CONFIG_FILE" <<EOF
$CONFIG_MARKER
trash_default = false
theme = "$theme"
appearance = "dark"
edit_max_bytes = 16777216
associations = []

[left_panel]
start_dir = "$left"
sort_mode = "name_folders_first"
show_hidden = true

[left_panel.view_mode]
kind = "columns"
columns = $cols

[right_panel]
start_dir = "$right"
sort_mode = "name_folders_first"
show_hidden = true

[right_panel.view_mode]
kind = "columns"
columns = $cols

[viewer]
wrap = false
tab_width = 4
hex_bytes_per_row = 16

[terminal]
scrollback_bytes = 1048576
size = "$term"

[watch]
enabled = true
debounce_ms = 200
max_delay_ms = 1000
identity_poll_ms = 2000
follow_moves = true
on_lost = "nearest-ancestor"
poll_non_local_ms = 5000
refresh_on_focus = true
EOF
}

# Put the app into a known state by restarting it.
#
# Driving it back to a clean state with keystrokes does not work: Escape is a
# TOGGLE (it raises and lowers the terminal curtain) as well as a dialog
# dismissal, so firing it N times lands on a state that depends on how many
# modals happened to be open. A copy of three files also raises one collision
# prompt per file, so the count is not knowable in advance. Relaunching costs
# about a second and is exact: no curtain, no dialog, no selection, output pane
# collapsed.
#
# Requires vite on :1420 (npm run dev) because this is the debug binary; the
# release bundle has its frontend baked in but is not necessarily current.
APP_BIN="${APP_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/debug/dimnav}"

# IMPORTANT: quitting is what persists panel state. dimnav writes its panel
# directories, sort and view back to config.toml on exit, so a config written
# BEFORE the old instance is gone gets silently overwritten by it - and the new
# instance then starts wherever the previous one happened to be looking.
# Hence quit -> write -> start, never write -> relaunch.
quit_app() {
  osascript -e "tell application \"System Events\" to tell process \"$APP_PROCESS\" to keystroke \"q\" using {command down}" >/dev/null 2>&1 || true
  local i
  for i in $(seq 1 40); do pgrep -x "$APP_PROCESS" >/dev/null || break; sleep 0.25; done
  pkill -x "$APP_PROCESS" 2>/dev/null || true
  sleep 0.5
}

start_app() {
  [[ -x "$APP_BIN" ]] || die "app binary not found at $APP_BIN (cargo build)"
  curl -sf http://localhost:1420 >/dev/null 2>&1 \
    || die "vite is not serving on :1420 - start it with: npm run dev"

  ( cd "$(dirname "$APP_BIN")" && "$APP_BIN" >/dev/null 2>&1 & )
  local i
  for i in $(seq 1 60); do pgrep -x "$APP_PROCESS" >/dev/null && break; sleep 0.25; done
  pgrep -x "$APP_PROCESS" >/dev/null || die "$APP_PROCESS did not start"
  sleep 2.2                 # let the webview paint before anything is captured
  place_window
}

# launch_with <left> <right> [term_size] [columns] [theme]
launch_with() {
  dismiss_intruders
  quit_app
  write_demo_config "$@"
  start_app
}

# Cheap insurance mid-recording: bring dimnav forward without the full
# geometry check. If anything has stolen focus, every keystroke after it would
# otherwise go to the wrong app - and during a take there is no way to notice.
ensure_front() {
  osascript >/dev/null 2>&1 <<EOF || true
tell application "System Events" to tell process "$APP_PROCESS"
  set frontmost to true
  perform action "AXRaise" of window 1
end tell
EOF
  sleep 0.3
}

# shot <path>  - capture exactly the window rect at native Retina resolution
shot() {
  dismiss_intruders
  screencapture -x -o -R"$WIN_X,$WIN_Y,$WIN_W,$WIN_H" "$1"
}
