#!/usr/bin/env python3
"""Turn the dimnav emblem into a macOS-shaped app icon master.

macOS does not mask app icons for you — every native app bakes its own rounded
shape into the artwork, inset inside a transparent margin. This script does that:
centre-crop the emblem to tighten it in frame, scale it to the Big Sur icon grid,
punch it through a superellipse mask, and centre the result on a 1024x1024
transparent canvas.

    python3 scripts/make-icon.py                    # write the masters
    python3 scripts/make-icon.py --preview          # also write a contact sheet
    python3 scripts/make-icon.py --crop 960         # loosen the crop

Then regenerate the bundle icon set:

    npm run tauri icon src-tauri/icon-master.png
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw

ROOT = Path(__file__).resolve().parent.parent

# The Big Sur icon grid: a 1024 canvas whose content occupies the middle 824,
# leaving a 100px margin on every side for the system's shadow.
CANVAS = 1024
CONTENT = 824

# Superellipse exponent. |x|^n + |y|^n = 1 with n=5 is the usual approximation of
# Apple's continuous-curvature corner — visibly rounder at the corner apex than a
# plain rounded rectangle of equivalent radius.
SQUIRCLE_N = 5.0


def superellipse_mask(size: int, n: float = SQUIRCLE_N, supersample: int = 4) -> Image.Image:
    """An antialiased superellipse mask, drawn a row at a time.

    Solving |x|^n + |y|^n = 1 for x gives the half-width at each row, so the shape
    is one filled rectangle per scanline. Drawing it supersampled and downscaling
    is what antialiases the edge — Pillow has no native superellipse.
    """
    upscaled = size * supersample
    mask = Image.new("L", (upscaled, upscaled), 0)
    draw = ImageDraw.Draw(mask)
    half = upscaled / 2

    for row in range(upscaled):
        y = abs((row + 0.5 - half) / half)
        if y >= 1.0:
            continue
        x = (1.0 - y**n) ** (1.0 / n)
        draw.rectangle([half - x * half, row, half + x * half, row], fill=255)

    return mask.resize((size, size), Image.LANCZOS)


def build_master(source: Path, crop: int) -> Image.Image:
    src = Image.open(source).convert("RGBA")
    if src.width != src.height:
        raise SystemExit(f"source must be square, got {src.width}x{src.height}")
    if crop > src.width:
        raise SystemExit(f"--crop {crop} exceeds source width {src.width}")

    # Centre-crop to tighten the emblem in frame, then scale to the grid.
    inset = (src.width - crop) // 2
    plate = src.crop((inset, inset, inset + crop, inset + crop))
    plate = plate.resize((CONTENT, CONTENT), Image.LANCZOS)

    # Multiply rather than replace, so any transparency already in the source
    # survives the mask instead of being painted over.
    plate.putalpha(ImageChops.multiply(plate.getchannel("A"), superellipse_mask(CONTENT)))

    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    offset = (CANVAS - CONTENT) // 2
    canvas.paste(plate, (offset, offset), plate)
    return canvas


def write_preview(master: Image.Image, out: Path) -> None:
    """A contact sheet of the sizes macOS actually renders.

    Small sizes are nearest-neighbour upscaled after downsampling, so the sheet
    shows the real pixel result rather than a flattering smooth scale — 32px and
    16px are where icon detail goes to die.
    """
    sizes = [512, 256, 128, 64, 32, 16]
    cell, pad = 280, 16
    sheet = Image.new("RGBA", (len(sizes) * (cell + pad) + pad, cell + 2 * pad), (32, 32, 36, 255))

    for i, size in enumerate(sizes):
        small = master.resize((size, size), Image.LANCZOS)
        shown = small.resize((cell, cell), Image.LANCZOS if size >= 128 else Image.NEAREST)
        sheet.paste(shown, (pad + i * (cell + pad), pad), shown)

    sheet.save(out)
    print(f"  {out.relative_to(ROOT)}  (sizes: {', '.join(map(str, sizes))})")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--source",
        type=Path,
        default=Path.home() / "Desktop/dimnav-logo-design/dimnav-logo-light-1024.png",
    )
    # Chosen by eye against 960/896/820/750 at 128/64/32/16px. 820 is where the
    # strokes stay distinct at 64px without the outer circle colliding with the
    # squircle corners — 750 is visibly cramped, 896 goes mushy a size sooner.
    ap.add_argument("--crop", type=int, default=820, help="centre-crop size before scaling")
    ap.add_argument("--preview", action="store_true", help="also write a contact sheet")
    args = ap.parse_args()

    if not args.source.exists():
        raise SystemExit(f"source not found: {args.source}")

    master = build_master(args.source, args.crop)

    outputs = {
        ROOT / "src-tauri/icon-master.png": CANVAS,
        ROOT / "src/assets/dimnav-icon.png": 256,
    }
    print(f"crop={args.crop} content={CONTENT} canvas={CANVAS} n={SQUIRCLE_N}")
    for path, size in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        image = master if size == CANVAS else master.resize((size, size), Image.LANCZOS)
        image.save(path)
        print(f"  {path.relative_to(ROOT)}  {size}x{size}")

    if args.preview:
        write_preview(master, ROOT / "scripts/.icon-preview.png")


if __name__ == "__main__":
    main()
