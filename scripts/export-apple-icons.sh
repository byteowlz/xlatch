#!/bin/bash
# Export the approved SVG artwork; requires librsvg, ImageMagick, and macOS iconutil.
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in rsvg-convert magick iconutil; do
    command -v "$tool" >/dev/null || { echo "Missing dependency: $tool" >&2; exit 1; }
done
out=logo/apple
mkdir -p "$out/ios" "$out/macos"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
for variant in white_on_black black_on_white; do
    background=black
    if [[ "$variant" == black_on_white ]]; then background=white; fi
    rsvg-convert -w 1024 -h 1024 "logo/SVG/${variant}.svg" -o "$work/ios.png"
    magick "$work/ios.png" -background "$background" -alpha remove -alpha off -colorspace sRGB \
        "PNG24:$out/ios/xlatch_${variant}_1024.png"
    # Fit the existing macOS tile proportionally into a centered 896px area.
    # 64px outer padding is our optical choice, not an App Store requirement.
    rsvg-convert --keep-aspect-ratio -w 896 -h 896 "logo/SVG/mac_${variant}.svg" -o "$work/mac.png"
    master="$out/macos/xlatch_${variant}_1024.png"
    magick "$work/mac.png" -background none -gravity center -extent 1024x1024 -colorspace sRGB "PNG32:$master"
    iconset="$out/macos/xlatch_${variant}.iconset"
    mkdir -p "$iconset"
    for size in 16 32 128 256 512; do
        magick "$master" -resize "${size}x${size}" "$iconset/icon_${size}x${size}.png"
        pixels=$((size * 2))
        magick "$master" -resize "${pixels}x${pixels}" "$iconset/icon_${size}x${size}@2x.png"
    done
    iconutil -c icns "$iconset" -o "$out/macos/xlatch_${variant}.icns"
done
cp "$out/ios/xlatch_white_on_black_1024.png" ios/App/Assets.xcassets/AppIcon.appiconset/AppIcon.png
echo "Exported iOS PNGs, macOS PNGs/iconsets/ICNS, and updated the iOS app icon."
