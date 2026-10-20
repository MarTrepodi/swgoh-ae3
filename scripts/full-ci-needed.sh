#!/usr/bin/env bash
# Decides whether a scheduled full CI run has anything new to test.
#
#   scripts/full-ci-needed.sh [<commit>]   (default: HEAD)
#
# Prints "true" when <commit>'s build inputs (BUILD_INPUTS below) differ from
# every commit an earlier Full CI run already built, and "false" when some
# run built exactly this code, whether it passed or failed, and whichever
# event started it (a nightly, a full-ci or release pull request, a manual
# run). A docs or workflow-only change, or a release merge whose code its
# release pull request already built, needs no new build.
#
# Needs gh (GH_TOKEN in CI) and the commits' history: fetch-depth 0.

set -euo pipefail

BUILD_INPUTS=(crates Cargo.toml Cargo.lock rust-toolchain.toml)

commit=$(git rev-parse "${1:-HEAD}^{commit}")
repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}

# Completed runs, newest first. Cancelled runs may not have built at all.
runs=$(gh run list --repo "$repo" --workflow full.yml --status completed --limit 50 \
  --json databaseId,headSha,conclusion \
  --jq '.[] | select(.conclusion == "success" or .conclusion == "failure") | "\(.databaseId) \(.headSha)"')

while read -r id sha; do
  [[ -n "$id" ]] || continue
  # A pull request head that was never merged may not be fetched.
  git cat-file -e "$sha^{commit}" 2>/dev/null || continue
  git diff --quiet "$sha" "$commit" -- "${BUILD_INPUTS[@]}" || continue
  # Same code; did that run build it, or did its gate skip the build?
  built=$(gh api "repos/$repo/actions/runs/$id/jobs?per_page=100" \
    --jq '[.jobs[] | select(.name | startswith("Full (")) | select(.conclusion == "success" or .conclusion == "failure")] | length')
  if (( built > 0 )); then
    echo "Run $id already built this code (at ${sha:0:7})." >&2
    echo false
    exit 0
  fi
done <<< "$runs"

echo "No earlier full CI run built this code." >&2
echo true
