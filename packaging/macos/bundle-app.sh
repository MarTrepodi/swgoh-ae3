#!/usr/bin/env bash
# Wraps a release ae3-gui binary in AE3.app, signs the bundle ad hoc and zips
# it with the same license files as the other archives.
#
#   packaging/macos/bundle-app.sh <ae3-gui binary> <version> <output dir>
#
# Writes <output dir>/ae3-gui-aarch64-apple-darwin.zip and its .sha256.
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

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
app="$stage/AE3.app"

mkdir -p "$app/Contents/MacOS"
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
printf 'APPL????' > "$app/Contents/PkgInfo"
install -m 755 "$binary" "$app/Contents/MacOS/ae3-gui"

# Ad hoc: no identity, so Gatekeeper still asks on first launch, but the
# bundle has a valid seal that binds Info.plist to the executable.
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"

cp LICENSE README.md CHANGELOG.md THIRD-PARTY-LICENSES.md \
  crates/ae3-audio/THIRD-PARTY-NOTICES.md "$stage/"

# ditto keeps the bundle's symlinks, permissions and signature intact.
rm -f "$out/$name.zip"
(cd "$stage" && ditto -c -k --norsrc . "$out/$name.zip")
(cd "$out" && shasum -a 256 -b "$name.zip" > "$name.zip.sha256")
echo "wrote $out/$name.zip"
