#!/usr/bin/env bash
# Decides whether a full CI run has anything new to test.
#
#   scripts/full-ci-needed.sh [--passed] [<commit>]   (default: HEAD)
#
# Prints "true" when <commit>'s build inputs (BUILD_INPUTS below) differ from
# every commit an earlier Full CI run already built, and "false" when some
# run built the same code, whichever event started it (a nightly, a full-ci
# or release pull request, a manual run). A docs or workflow-only change, or
# a release merge whose code its release pull request already built, needs
# no new build. Version numbers do not count as code: a release pull request
# is main plus a version bump, so it and the nightly build the code once.
#
# A run that failed counts as having built the code, unless --passed is
# given: then only a run that passed does (release pull requests, so that
# nothing ships on a failed build).
#
# Needs gh (GH_TOKEN in CI) and the commits' history: fetch-depth 0.

set -euo pipefail

BUILD_INPUTS=(crates Cargo.toml Cargo.lock rust-toolchain.toml)

passed=false
if [[ "${1:-}" == --passed ]]; then
  passed=true
  shift
fi
commit=$(git rev-parse "${1:-HEAD}^{commit}")
repo=${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}

# GitHub's API now and then answers with a 5xx error or times out, and a
# failed nightly gate opens a nightly-failure issue, so each call gets four
# tries, 5, 10 and 20 seconds apart (GH_RETRY_DELAY sets the first wait).
gh_retry() {
  local attempt delay=${GH_RETRY_DELAY:-5}
  for attempt in 1 2 3 4; do
    if gh "$@"; then
      return 0
    fi
    (( attempt < 4 )) || return 1
    echo "gh $1 failed (try $attempt of 4); retrying in ${delay}s." >&2
    sleep "$delay"
    delay=$((delay * 2))
  done
}

# Completed runs, newest first. Cancelled runs may not have built at all.
if $passed; then counted='"success"'; else counted='"success", "failure"'; fi
runs=$(gh_retry run list --repo "$repo" --workflow full.yml --status completed --limit 50 \
  --json databaseId,headSha,conclusion \
  --jq ".[] | select(.conclusion | IN($counted)) | \"\\(.databaseId) \\(.headSha)\"")

# The build inputs' differences, less lines that only set a version. A real
# dependency change in Cargo.lock also changes that package's checksum or
# source line; only the workspace's own crates have a version line alone.
code_changes() {
  git diff -U0 "$1" "$2" -- "${BUILD_INPUTS[@]}" |
    grep -E '^[-+]' | grep -vE '^(--- a/|\+\+\+ b/|--- /dev/null|\+\+\+ /dev/null)' |
    grep -vE '^[-+]version = "[^"]*"$' || true
}

while read -r id sha; do
  [[ -n "$id" ]] || continue
  # A pull request head that was never merged may not be fetched.
  git cat-file -e "$sha^{commit}" 2>/dev/null || continue
  [[ -z "$(code_changes "$sha" "$commit")" ]] || continue
  # Same code; did that run build it, or did its gate skip the build?
  built=$(gh_retry api "repos/$repo/actions/runs/$id/jobs?per_page=100" \
    --jq '[.jobs[] | select(.name | startswith("Full (")) | select(.conclusion == "success" or .conclusion == "failure")] | length')
  if (( built > 0 )); then
    echo "Run $id already built this code (at ${sha:0:7})." >&2
    echo false
    exit 0
  fi
done <<< "$runs"

echo "No earlier full CI run built this code." >&2
echo true
