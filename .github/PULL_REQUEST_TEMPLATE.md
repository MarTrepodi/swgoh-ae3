## What and why

<!-- What this changes and the problem it solves. Link the issue: "Fixes #123". -->

## How it was tested

<!-- Commands run, new or updated tests, and any manual checks (for GUI changes, what you clicked). -->

## Checklist

- [ ] The PR title and commits follow Conventional Commits: `type(scope): summary`
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features` pass
- [ ] New behaviour and bug fixes have tests
- [ ] `README.md` is updated if commands, flags or settings changed (`CHANGELOG.md` is generated; don't edit it)
- [ ] New dependencies use minimal features, and their size cost is noted below
- [ ] No game data (fixtures, Bundles or extracted assets) is committed

## Notes for reviewers

<!-- Anything else: trade-offs, follow-ups, binary size change. -->
