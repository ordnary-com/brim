#!/bin/sh
# Renders the file icons that `brim icons` installs, from the SVGs in
# assets/icons. Needs rsvg-convert, python3 and (for .icns) macOS iconutil.
#
#   scripts/icons.sh
set -eu
cd "$(dirname "$0")/../assets/icons"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

icns() { # SVG ICNS
    set="$tmp/$(basename "$2" .icns).iconset"
    mkdir -p "$set"
    for s in 16 32 128 256 512; do
        rsvg-convert -w "$s" "$1" > "$set/icon_${s}x${s}.png"
        rsvg-convert -w $((s * 2)) "$1" > "$set/icon_${s}x${s}@2x.png"
    done
    iconutil -c icns "$set" -o "$2"
}

ico() { # SVG ICO: PNG images in an ICO container
    for s in 16 24 32 48 64 256; do
        rsvg-convert -w "$s" "$1" > "$tmp/$s.png"
    done
    python3 - "$tmp" "$2" <<'PY'
import struct, sys
tmp, out = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 256]
images = [open(f"{tmp}/{s}.png", "rb").read() for s in sizes]
data = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
for s, png in zip(sizes, images):
    data += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(png), offset)
    offset += len(png)
open(out, "wb").write(data + b"".join(images))
PY
}

for name in brim brimproj; do
    icns "$name-document.svg" "$name.icns"
    ico "$name-document.svg" "$name.ico"
done
icns app.svg app.icns
