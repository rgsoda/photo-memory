#!/usr/bin/env bash
#
# Regenerate the tray PNGs from photomem.svg. Run it after editing the SVG;
# the PNGs are committed because the build embeds them and a build machine
# should not need a renderer.
#
#   ./src-tauri/icons/tray/render.sh
#
# Needs rsvg-convert (librsvg).

set -euo pipefail
cd "$(dirname "$0")"

command -v rsvg-convert >/dev/null || {
    echo "need rsvg-convert (pacman -S librsvg / apt install librsvg2-bin)" >&2
    exit 1
}

# Black for macOS, which treats it as a template image and recolours it to
# match the menu bar — light text on a dark bar and the reverse, without two
# assets. 32px is the 16pt slot at 2x, which is every Mac made in a decade.
sed 's/currentColor/black/g' photomem.svg | rsvg-convert -w 32 -h 32 -o tray-template.png

# White for Linux, where nothing recolours anything: a status bar hands the
# pixmap straight through, so the icon has to already be the right colour for
# the bar it lands on. 44px covers a HiDPI bar scaling down.
sed 's/currentColor/white/g' photomem.svg | rsvg-convert -w 44 -h 44 -o tray-white.png

ls -l tray-template.png tray-white.png
