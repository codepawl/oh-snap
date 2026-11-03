#!/bin/bash
# Builds oh-snap.app and oh-snap-macos.zip from two single-architecture binaries.
# usage: make-app.sh <arm64 binary> <x86_64 binary> <output folder>
# The signature is ad hoc; the Developer ID signature and notarization happen outside CI.
set -euo pipefail

arm_binary="$1"
intel_binary="$2"
output="$3"
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"

version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)"
build="${GITHUB_RUN_NUMBER:-1}"

app="$output/oh-snap.app"
rm -rf "$app" "$output/oh-snap-macos.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

lipo -create "$arm_binary" "$intel_binary" -output "$app/Contents/MacOS/oh-snap"
sed -e "s/@VERSION@/$version/" -e "s/@BUILD@/$build/" "$here/Info.plist" > "$app/Contents/Info.plist"

iconset="$output/oh-snap.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256; do
    sips -z "$size" "$size" "$root/assets/oh-snap.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$root/assets/oh-snap.png" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
cp "$root/assets/oh-snap.png" "$iconset/icon_512x512.png"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/oh-snap.icns"
rm -rf "$iconset"

codesign -s - --deep --force --options runtime --entitlements "$here/entitlements.plist" "$app"
codesign --verify --deep --strict "$app"
"$app/Contents/MacOS/oh-snap" --version

(cd "$output" && ditto -c -k --keepParent oh-snap.app oh-snap-macos.zip)
echo "built $output/oh-snap-macos.zip ($version)"
