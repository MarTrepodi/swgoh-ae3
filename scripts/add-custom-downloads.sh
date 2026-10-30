#!/usr/bin/env bash
# Adds the downloads our own release jobs build to the Download tables dist
# writes into the release notes. dist lists only the archives it built, so
# without this the macOS app and disk image, the macOS CLI archive and the
# Windows installer are attached to the release but missing from its notes.
# The builds of ae3 with the HTTP API (ae3-serve-<target>) get a table of
# their own, "Download ae3 <version> with the HTTP API", after ae3's.
#
#   scripts/add-custom-downloads.sh <notes file> <tag> <artifacts dir>
#
# Edits <notes file> in place. A row is added only when its file and the
# file's .sha256 are in <artifacts dir>, and only once.

set -euo pipefail

notes=$1 tag=$2 artifacts=$3
base="https://github.com/${GITHUB_REPOSITORY:?}/releases/download/$tag"

# table | file | platform | the row it goes before ("top": first row).
# A table is the app its "## Download" heading names; ae3+serve is the
# HTTP API table.
rows='
ae3-gui|swgoh-ae3-macos-arm64.dmg|Apple Silicon macOS (disk image)|top
ae3-gui|ae3-gui-aarch64-apple-darwin.zip|Apple Silicon macOS (app, zipped)|top
ae3-gui|swgoh-ae3-windows-x86_64.msi|x64 Windows (installer)|ae3-gui-x86_64-pc-windows-msvc.zip
ae3|ae3-aarch64-apple-darwin.tar.xz|Apple Silicon macOS|top
ae3+serve|ae3-serve-aarch64-apple-darwin.tar.xz|Apple Silicon macOS|top
ae3+serve|ae3-serve-x86_64-pc-windows-msvc.zip|x64 Windows|top
ae3+serve|ae3-serve-aarch64-unknown-linux-gnu.tar.xz|ARM64 Linux|top
ae3+serve|ae3-serve-x86_64-unknown-linux-gnu.tar.xz|x64 Linux|top
'

# Only files that are attached and not yet listed, as tab-separated
# app, anchor and table row.
add=""
while IFS='|' read -r app file platform anchor; do
  [[ -n "$app" ]] || continue
  [[ -f "$artifacts/$file" && -f "$artifacts/$file.sha256" ]] || continue
  grep -qF "[$file](" "$notes" && continue
  add+="$app"$'\t'"$anchor"$'\t'"| [$file]($base/$file) | $platform | [checksum]($base/$file.sha256) |"$'\n'
done <<< "$rows"
[[ -n "$add" ]] || exit 0

# The HTTP API table starts empty, right after ae3's table, the first time
# it gets a row.
if [[ "$add" == *$'ae3+serve\t'* ]] && ! grep -q '^## Download ae3 .* with the HTTP API$' "$notes"; then
  awk '
    function section() {
      print ""
      print "## Download ae3 " version " with the HTTP API"
      print ""
      print "The same `ae3`, with the HTTP API (`ae3 serve`) built in."
      print ""
      print "|  File  | Platform | Checksum |"
      print "|--------|----------|----------|"
      done = 1
    }
    /^## Download ae3 [^ ]+$/ { in_ae3 = 1; version = $4 }
    in_ae3 && !done && seen_table && $0 !~ /^\|/ { section(); in_ae3 = 0 }
    in_ae3 && $0 ~ /^\|/ { seen_table = 1 }
    { print }
    END { if (in_ae3 && seen_table && !done) section() }
  ' "$notes" > "$notes.new"
  mv "$notes.new" "$notes"
fi

# Through the environment: not every awk takes newlines in -v.
ADD=$add awk '
  BEGIN {
    n = split(ENVIRON["ADD"], lines, "\n")
    for (i = 1; i <= n; i++) {
      if (lines[i] == "") continue
      split(lines[i], f, "\t")
      key = f[1] SUBSEP f[2]
      pending[key] = pending[key] f[3] "\n"
    }
  }
  # "## Download <app> <version>" starts that app'"'"'s table; the same with
  # "with the HTTP API" after it starts <app>+serve'"'"'s.
  /^## Download / { app = $3; if ($0 ~ / with the HTTP API$/) app = app "+serve" }
  {
    if (app != "" && match($0, /^\| \[[^]]+\]\(/)) {
      file = substr($0, 4, RLENGTH - 5)
      key = app SUBSEP file
      if (key in pending) { printf "%s", pending[key]; delete pending[key] }
    }
    print
    if (app != "" && $0 ~ /^\|-+\|/) {
      key = app SUBSEP "top"
      if (key in pending) { printf "%s", pending[key]; delete pending[key] }
    }
  }
' "$notes" > "$notes.new"
mv "$notes.new" "$notes"
