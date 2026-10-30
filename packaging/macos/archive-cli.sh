#!/usr/bin/env bash
# Packs a release ae3 binary as ae3-aarch64-apple-darwin.tar.xz, the archive
# dist makes on the other platforms: a top-level folder holding the binary
# and the license files.
#
#   packaging/macos/archive-cli.sh <ae3 binary> <output dir> [name]
#
# Writes <output dir>/<name>.tar.xz and its .sha256; <name> defaults to
# ae3-aarch64-apple-darwin, and the build with the HTTP API passes
# ae3-serve-aarch64-apple-darwin. The macOS release job builds both beside
# ae3-gui, so the release needs one macOS runner instead of several. Run
# from the repository root, after THIRD-PARTY-LICENSES.md is generated for
# the binary being packed.
set -euo pipefail

if (( $# < 2 || $# > 3 )); then
  echo "usage: $0 <ae3 binary> <output dir> [name]" >&2
  exit 2
fi
binary=$1
mkdir -p "$2"
out=$(cd "$2" && pwd)
name=${3:-ae3-aarch64-apple-darwin}

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
install -m 755 "$binary" "$stage/$name/ae3"
# The files dist's include list (Cargo.toml) puts in every archive.
cp LICENSE README.md CHANGELOG.md THIRD-PARTY-LICENSES.md \
  crates/ae3-audio/THIRD-PARTY-NOTICES.md "$stage/$name/"

rm -f "$out/$name.tar.xz"
# No macOS extended attributes or resource forks, and no builder's user name.
(cd "$stage" && COPYFILE_DISABLE=1 tar --no-xattrs --uid 0 --gid 0 --uname "" --gname "" -cJf "$out/$name.tar.xz" "$name")
(cd "$out" && shasum -a 256 -b "$name.tar.xz" > "$name.tar.xz.sha256")
echo "wrote $out/$name.tar.xz"
