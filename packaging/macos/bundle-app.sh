#!/usr/bin/env bash
# Wraps a release ae3-gui binary in swgoh-ae3.app, signs the bundle ad hoc and
# ships it two ways: zipped with the same license files as the other archives,
# and as a disk image holding the app and an Applications shortcut to drag it
# onto. The app carries the license files too, in Contents/Resources.
#
#   packaging/macos/bundle-app.sh <ae3-gui binary> <version> <output dir>
#
# Writes <output dir>/ae3-gui-aarch64-apple-darwin.zip,
# <output dir>/swgoh-ae3-macos-arm64.dmg and a .sha256 for each.
# Run from the repository root, after THIRD-PARTY-LICENSES.md is generated.
set -euo pipefail

if (( $# != 3 )); then
  echo "usage: $0 <ae3-gui binary> <version> <output dir>" >&2
  exit 2
fi
binary=$1 version=$2
mkdir -p "$3"
out=$(cd "$3" && pwd)
name=ae3-gui-aarch64-apple-darwin
dmg=swgoh-ae3-macos-arm64
notices=(LICENSE README.md CHANGELOG.md THIRD-PARTY-LICENSES.md
  crates/ae3-audio/THIRD-PARTY-NOTICES.md crates/ae3-gui/FONT-NOTICES.md)

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app="$stage/swgoh-ae3.app"

mkdir -p "$app/Contents/MacOS"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
install -m 755 "$binary" "$app/Contents/MacOS/ae3-gui"

# The app icon: an iconset rendered from the 512 px master, then .icns.
mkdir -p "$app/Contents/Resources"
iconset="$stage/swgoh-ae3.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256; do
  sips -z "$size" "$size" packaging/icons/swgoh-ae3-512.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" packaging/icons/swgoh-ae3-512.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
cp packaging/icons/swgoh-ae3-512.png "$iconset/icon_512x512.png"
iconutil -c icns -o "$app/Contents/Resources/swgoh-ae3.icns" "$iconset"
rm -r "$iconset"
cp "${notices[@]}" "$app/Contents/Resources/"

# Ad hoc: no identity, so Gatekeeper still asks on first launch, but the
# bundle has a valid seal that binds Info.plist to the executable.
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"

cp "${notices[@]}" "$stage/"

# ditto keeps the bundle's symlinks, permissions and signature intact.
rm -f "$out/$name.zip"
(cd "$stage" && ditto -c -k --norsrc . "$out/$name.zip")
(cd "$out" && shasum -a 256 -b "$name.zip" > "$name.zip.sha256")
echo "wrote $out/$name.zip"

# The disk image: the app beside a link to /Applications, compressed with
# LZFSE (ULFO; macOS 10.11 and later, so every Apple silicon Mac).
volume="$stage/volume"
mkdir "$volume"
ditto "$app" "$volume/swgoh-ae3.app"
ln -s /Applications "$volume/Applications"
rm -f "$out/$dmg.dmg"
# hdiutil on CI runners now and then fails with "Resource busy"; retry.
for attempt in 1 2 3; do
  if hdiutil create -quiet -volname "swgoh-ae3 $version" -srcfolder "$volume" \
    -fs HFS+ -format ULFO -ov "$out/$dmg.dmg"; then
    break
  fi
  (( attempt < 3 )) || exit 1
  sleep 5
done
hdiutil verify -quiet "$out/$dmg.dmg"
(cd "$out" && shasum -a 256 -b "$dmg.dmg" > "$dmg.dmg.sha256")
echo "wrote $out/$dmg.dmg"
