#!/usr/bin/env bash
#
# Captures the four website screenshots against the demo tree, then writes them
# out at the size the page actually displays them.
#
#   npm run tauri dev            # in another shell
#   ./scripts/demo-tree.sh --force
#   ./scripts/demo-shots.sh
#
# Output goes straight into site/assets/ as WebP. They are shown ~500px wide in
# a two-up grid, so 1000px is 2x for a Retina audience and the sensible ceiling;
# a larger file buys nothing a reader can see.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
# shellcheck source=scripts/demo-lib.sh
source "$HERE/demo-lib.sh"

DEMO="${DEMO_ROOT:-/Users/Shared/dimnav-demo}"
RAW="$HERE/.demo-out/shots"
OUT="$REPO_ROOT/site/assets"
WIDTH=1000

command -v cwebp >/dev/null || die "cwebp not found. brew install webp"
[[ -d "$DEMO" ]] || die "demo tree missing. Run ./scripts/demo-tree.sh --force"
mkdir -p "$RAW" "$OUT"

# Each shot writes the config it wants and relaunches, so panel directories,
# sort, view mode, theme and the terminal pane size are all facts rather than
# the residue of whatever the previous shot did. The user's own config is
# restored by the EXIT trap in demo-lib.sh.
PHOTOS="$DEMO/Media/Photos/2026-06 Lisbon"

echo "==> 1/4 panels"
launch_with "$DEMO/Projects/aurora-cms" "$PHOTOS"
k home 0.25
for _ in $(seq 1 13); do k down 0.07; done      # onto CONTRIBUTING.md
sleep 0.7
shot "$RAW/shot-panels.png"

echo "==> 2/4 viewer"
# Shows the viewer on a document rather than in hex mode. F4 (the hex toggle) is
# reliably swallowed when the viewer is driven by an injected keystroke, while
# F3/F5/Cmd+T all land - so hex is left to the video, and this frame shows
# something a reader can actually read.
launch_with "$DEMO/Projects/aurora-cms/docs" "$DEMO/Projects/aurora-cms/src"
k home 0.25
k down 0.3                                      # api-reference.pdf
k down 0.3                                      # architecture.md
k f3 3.0
sleep 1.0
shot "$RAW/shot-viewer.png"

echo "==> 3/4 copy with a collision"
# F5 copies from the ACTIVE panel to the other one, and the left panel holds the
# keyboard on launch - so the trip folder goes left and the folder that already
# has three of its names goes right.
launch_with "$PHOTOS" "$DEMO/Media/Photos/Selects"
k home 0.25
k down 0.25
k space 0.2; k space 0.2; k space 0.35          # IMG_4821..4823
k f5 1.3                                        # editable destination prompt
k return 1.9                                    # accept -> collision dialog
sleep 1.0
shot "$RAW/shot-copy.png"

echo "==> 4/4 terminal"
launch_with "$DEMO/Projects/aurora-cms" "$PHOTOS" half
chord t "command down" 0.5
# Deliberately not `ls -la`: its owner column prints the account name on every
# line, and these frames are published. `cat` shows real content, proves the
# terminal is running in the panel's directory, and names nobody.
type_text "cat Cargo.toml" 0.35
k return 2.0
sleep 0.6
shot "$RAW/shot-terminal.png"

echo "==> encoding"
for n in panels copy terminal viewer; do
  cwebp -quiet -q 80 -resize "$WIDTH" 0 "$RAW/shot-$n.png" -o "$OUT/shot-$n.webp"
  printf '  shot-%s.webp  %s\n' "$n" "$(du -h "$OUT/shot-$n.webp" | cut -f1 | tr -d ' ')"
done

# Social card: 1200x630 from the panels shot. JPEG, because link scrapers are
# unreliable with WebP.
#
# Crop VERTICALLY ONLY, and with ffmpeg's explicit crop=w:h:x:y rather than
# `sips -c`. sips crops from the CENTRE (--cropOffset shifts that centred
# window, it does not set a top-left origin), and this frame's content runs
# edge to edge - so any horizontal crop slices the first characters off the
# left panel's filenames. 2560 / (1200/630) = 1344; y=56 clears the title bar
# while keeping both path bars.
ffmpeg -y -v error -i "$RAW/shot-panels.png" \
  -vf "crop=2560:1344:0:56,scale=1200:630:flags=lanczos" -q:v 3 "$OUT/og-panels.jpg"
printf '  og-panels.jpg  %s\n' "$(du -h "$OUT/og-panels.jpg" | cut -f1 | tr -d ' ')"

echo "done. Raw captures kept in $RAW"
