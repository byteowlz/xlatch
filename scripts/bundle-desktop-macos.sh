#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
  echo 'This bundle recipe requires macOS.' >&2
  exit 1
fi
cargo build --locked -p xlatch-desktop --features tray
version=$(cargo pkgid -p xlatch-desktop)
version=${version##*@}
revision=$(git rev-parse HEAD)
build=$(git rev-list --count HEAD)
if [[ -n "$(git status --porcelain)" ]]; then revision="$revision+dirty"; fi
bundle=target/desktop/xlatch.app
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
cp target/debug/xlatch-desktop "$bundle/Contents/MacOS/"
cp logo/apple/macos/xlatch_white_on_black.icns "$bundle/Contents/Resources/xlatch.icns"
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>xlatch</string>
<key>CFBundleDisplayName</key><string>xlatch</string>
<key>CFBundleIdentifier</key><string>com.byteowlz.xlatch.desktop</string>
<key>CFBundleExecutable</key><string>xlatch-desktop</string>
<key>CFBundleIconFile</key><string>xlatch</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$build</string>
<key>XlatchSourceRevision</key><string>$revision</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$bundle"
codesign --verify --deep --strict "$bundle"
printf 'Local development bundle: %s/%s\n' "$PWD" "$bundle"
