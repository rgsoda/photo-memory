#!/usr/bin/env bash
#
# Regenerate every icon from the two SVGs. Run it after editing either one;
# the PNGs are committed because the build embeds them and a build machine
# should not need a renderer.
#
#   ./src-tauri/icons/render.sh
#
# Needs rsvg-convert (librsvg) and python3.

set -euo pipefail
cd "$(dirname "$0")"

command -v rsvg-convert >/dev/null || {
    echo "need rsvg-convert (pacman -S librsvg / apt install librsvg2-bin)" >&2
    exit 1
}

# ── the tray glyph ───────────────────────────────────────────────────────────
#
# Black for macOS, which treats it as a template image and recolours it to match
# the menu bar — light text on a dark bar and the reverse, from one asset. 32px
# is the 16pt slot at 2x, which is every Mac made in a decade.
sed 's/currentColor/black/g' tray/photomem.svg | rsvg-convert -w 32 -h 32 -o tray/tray-template.png

# White for Linux, where nothing recolours anything: the pixmap goes to the bar
# as it is, so it has to arrive the right colour. 44px covers a HiDPI bar.
sed 's/currentColor/white/g' tray/photomem.svg | rsvg-convert -w 44 -h 44 -o tray/tray-white.png

# ── the app icon ─────────────────────────────────────────────────────────────

rsvg-convert -w 32   -h 32   photomem.svg -o 32x32.png
rsvg-convert -w 128  -h 128  photomem.svg -o 128x128.png
rsvg-convert -w 256  -h 256  photomem.svg -o 128x128@2x.png
rsvg-convert -w 512  -h 512  photomem.svg -o icon.png

# The .icns the macOS bundle wants. Written here rather than with iconutil so
# this runs on the machine the work happens on; iconutil is macOS-only, and
# ImageMagick on Arch has no ICNS writer. Modern .icns entries are just PNGs
# with a four-byte type in front, which is what this assembles.
python3 - <<'PY'
import struct, subprocess

# type -> pixel size. The @2x types repeat a size on purpose: macOS picks by
# type, not by measuring, and a missing type means a blurry upscale.
ENTRIES = [
    (b"icp4",   16), (b"icp5",   32), (b"ic11",   32), (b"ic12",   64),
    (b"ic07",  128), (b"ic13",  256), (b"ic08",  256), (b"ic14",  512),
    (b"ic09",  512), (b"ic10", 1024),
]

chunks = []
for kind, size in ENTRIES:
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size), "photomem.svg"],
        check=True, capture_output=True).stdout
    chunks.append(kind + struct.pack(">I", len(png) + 8) + png)

body = b"".join(chunks)
open("icon.icns", "wb").write(b"icns" + struct.pack(">I", len(body) + 8) + body)
PY

ls -l tray/tray-template.png tray/tray-white.png 32x32.png 128x128.png 128x128@2x.png icon.png icon.icns
