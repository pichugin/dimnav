#!/usr/bin/env bash
#
# Records the website demo video against the demo tree.
#
#   npm run dev                       # vite on :1420, in another shell
#   ./scripts/demo-tree.sh --force
#   ./scripts/record-demo.sh
#
# Produces scripts/.demo-out/dimnav-demo.mp4 (gitignored) plus the poster and
# the caption track, which DO go in the repo. The video is published as a
# GitHub release asset rather than committed:
#
#   gh release upload media scripts/.demo-out/dimnav-demo.mp4 --clobber
#
# because it is re-recorded whenever the UI moves and git would keep every
# superseded copy for ever.
#
# Captions are a WebVTT sidecar, not burned into the frames: the video has no
# audio, so a viewer cannot otherwise tell which key produced what. A track file
# stays sharp at any scale, can be corrected without re-encoding, and can be
# switched off. (This Homebrew ffmpeg has neither libass nor drawtext, so
# burning them in was not on the table anyway.)

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
# shellcheck source=scripts/demo-lib.sh
source "$HERE/demo-lib.sh"

DEMO="${DEMO_ROOT:-/Users/Shared/dimnav-demo}"
OUTDIR="$HERE/.demo-out"
RAW="$OUTDIR/raw.mov"
MP4="$OUTDIR/dimnav-demo.mp4"
SITE="$REPO_ROOT/site/assets"
CUES="$OUTDIR/cues.tsv"

command -v ffmpeg >/dev/null || die "ffmpeg not found. brew install ffmpeg"
command -v cwebp  >/dev/null || die "cwebp not found. brew install webp"
[[ -d "$DEMO" ]] || die "demo tree missing. Run ./scripts/demo-tree.sh --force"
mkdir -p "$OUTDIR" "$SITE"

now() { perl -MTime::HiRes -e 'printf "%.3f", Time::HiRes::time()'; }
T0=""
: > "$CUES"

# cue <seconds-to-show> <text>  - caption from now until now+seconds
cue() {
  local t; t=$(now)
  printf '%s\t%s\t%s\n' "$t" "$1" "$2" >> "$CUES"
}

PHOTOS="$DEMO/Media/Photos/2026-06 Lisbon"

echo "==> preparing"
launch_with "$DEMO/Projects/aurora-cms" "$PHOTOS"

echo "==> recording"
# screencapture will NOT overwrite an existing file: it exits immediately and,
# because it is backgrounded, the script sails on, re-encodes the stale capture
# and reports success. Every "re-record" then silently produces the same video.
rm -f "$RAW"
screencapture -v -x -R"$WIN_X,$WIN_Y,$WIN_W,$WIN_H" "$RAW" &
REC_PID=$!
sleep 1.5
kill -0 "$REC_PID" 2>/dev/null || die "screencapture exited immediately (is $RAW writable?)"
REC_START=$(now)
sleep 3                       # lead-in: a still first frame makes a clean poster
T0=$(now)

# --- Beat design -----------------------------------------------------------
#
# One rule governs the whole sequence: whenever an Enter is sent, the ACTIVE
# panel's cursor must be on a directory, on `..`, or a dialog must own the
# keyboard. The demo images are sparse placeholders, so an Enter that reaches a
# panel with a file under the cursor launches Preview, and its "could not be
# opened" sheet then sits on top and swallows every remaining beat - the whole
# back half of the take is lost, silently.
#
# That is also why navigation here is arrow keys rather than `cd` typed into the
# built-in terminal: if a Cmd+T fails to take focus, the typed letters land
# harmlessly in a panel but the trailing Enter does not. Arrows cannot misfire
# that way, and they show better on camera.

# --- 1. the panels themselves ---------------------------------------------
cue 2.80 "Two panels — source and destination at once"
sleep 2.80

# --- 2. navigation ---------------------------------------------------------
cue 6.30 "Arrows move the cursor; Enter opens, Backspace comes back to it"
k down 0.39; k down 0.55; k down 0.55         # assets / docs / migrations
k down 0.49                                    # src/  (a directory: Enter is safe)
k return 1.26
k down 0.35; k down 0.45; k down 0.6
k backspace 1.40                               # back out, cursor lands on src/
sleep 0.70

# --- 3. detailed view ------------------------------------------------------
cue 4.20 "⌃4 — a detailed view with size, date and permissions"
chord 4 "control down" 2.10
sleep 1.40
chord 2 "control down" 0.84

# --- 4. quick search -------------------------------------------------------
cue 4.90 "⌘F jumps to a name as you type it"
chord f "command down" 0.84
type_text "doc" 2.0
sleep 1.26
k escape 0.84

# --- 5. viewer -------------------------------------------------------------
# The cursor is on docs/ courtesy of the search, so this Enter opens a folder.
cue 5.60 "F3 opens a file in place — text, hex or image"
k return 1.12                                  # into docs/
k down 0.35; k down 0.6                        # architecture.md
k f3 2.38
sleep 1.68
k escape 0.98
k backspace 1.12                               # out of docs/
k home 0.42                                    # park on `..`

# --- 6. select and copy ----------------------------------------------------
cue 3.50 "Space selects — the footer counts what you picked"
k tab 0.56                                     # the photo panel
k home 0.35
k down 0.35
k space 0.39; k space 0.55; k space 1.0

# F5 acts on the SELECTION, not on the cursor - so the cursor can be parked on
# `..` first. That keeps the invariant: if the F5 prompt is slow and the Enter
# below reaches the panel instead, it merely steps up a directory rather than
# opening a placeholder image in Preview and losing the rest of the take.
k home 0.56

cue 6.30 "F5 copies — asynchronous, cancellable, with real progress"
k f5 1.54                                      # editable destination prompt
sleep 1.26
k return 2.80                                  # run it
sleep 1.05
k tab 0.56                                     # back to the project panel
k home 0.42                                    # and park there too

# --- 7. terminal -----------------------------------------------------------
# Worst case, if Cmd+T misses, the letters do nothing in a panel and the Enter
# lands on `..` - which just steps up a directory. Nothing modal, nothing lost.
cue 7.00 "⌘T runs commands where the panel is pointing"
chord t "command down" 0.70
type_text "cat Cargo.toml" 0.8
k return 1.68
chord t "shift down, command down" 1.82        # expand the output pane
sleep 1.54
chord t "shift down, command down" 0.84        # collapse

# --- 8. the watcher --------------------------------------------------------
cue 5.60 "Changes made outside the app appear on their own"
sleep 0.84
touch "$DEMO/Projects/aurora-cms/NOTES-from-the-build.md"
sleep 3.50
rm -f "$DEMO/Projects/aurora-cms/NOTES-from-the-build.md"
sleep 1.26

# --- 9. settings and themes ------------------------------------------------
cue 6.30 "F2 — themes and settings, applied live"
dismiss_intruders
k f2 1.68
k down 0.70
k right 1.40
sleep 1.54
k left 1.12
k escape 0.84

# --- 10. help --------------------------------------------------------------
cue 4.90 "F1 lists every binding, generated from the live keymap"
dismiss_intruders
k f1 1.82
sleep 2.38
k escape 0.98

sleep 1.5
echo "==> stopping"
kill -INT "$REC_PID" 2>/dev/null || true
for _ in $(seq 1 40); do kill -0 "$REC_PID" 2>/dev/null || break; sleep 0.25; done
sleep 1.5
[[ -s "$RAW" ]] || die "no recording was produced"
# Belt and braces: a capture older than this run means the stale-file trap above
# bit again, and everything downstream would describe the wrong video.
find "$RAW" -newermt '-5 minutes' | grep -q . || die "$RAW is stale - recording did not run"

echo "==> writing captions"
python3 - "$CUES" "$REC_START" "$SITE/dimnav-demo.vtt" <<'PY'
import sys
cues_path, t0, out = sys.argv[1], float(sys.argv[2]), sys.argv[3]

def ts(sec):
    sec = max(sec, 0.0)
    h, rem = divmod(sec, 3600)
    m, s = divmod(rem, 60)
    return f"{int(h):02d}:{int(m):02d}:{s:06.3f}"

# Offsets are measured from when the RECORDER started, not from the first
# beat: the take opens with a still lead-in, and timing the cues from the beat
# clock puts every caption on screen that many seconds too early.
cues = []
for raw in open(cues_path):
    at, dur, text = raw.rstrip("\n").split("\t", 2)
    start = float(at) - t0
    cues.append([start, start + float(dur), text])

# Clamp each cue to the start of the next one. Overlapping cues stack up in the
# player and cover the thing they are describing.
for i in range(len(cues) - 1):
    cues[i][1] = min(cues[i][1], cues[i + 1][0] - 0.05)

lines = ["WEBVTT", ""]
for start, end, text in cues:
    if end <= start:
        continue
    lines += [f"{ts(start)} --> {ts(end)}", text, ""]
open(out, "w").write("\n".join(lines))
print(f"  {out}  ({len(cues)} cues)")
PY

echo "==> encoding"
# One Retina master (2560x1600 @120fps) down to 1280x800 @30. CRF 20 keeps the
# UI text crisp; a screen recording of a file manager is mostly static, so the
# bitrate stays low regardless.
ffmpeg -y -v error -i "$RAW" \
  -vf "scale=1280:800:flags=lanczos,fps=30" \
  -c:v libx264 -profile:v high -pix_fmt yuv420p -crf 20 -preset slow \
  -movflags +faststart -an "$MP4"

ffmpeg -y -v error -ss 1.0 -i "$MP4" -frames:v 1 "$OUTDIR/poster.png"
cwebp -quiet -q 82 "$OUTDIR/poster.png" -o "$SITE/demo-poster.webp"

printf '  %-22s %s\n' "dimnav-demo.mp4" "$(du -h "$MP4" | cut -f1 | tr -d ' ')"
printf '  %-22s %s\n' "demo-poster.webp" "$(du -h "$SITE/demo-poster.webp" | cut -f1 | tr -d ' ')"
ffprobe -v error -select_streams v:0 -show_entries stream=width,height,duration -of default=nw=1 "$MP4" | sed 's/^/  /'

echo
echo "Publish with:"
echo "  gh release upload media $MP4 --clobber"
