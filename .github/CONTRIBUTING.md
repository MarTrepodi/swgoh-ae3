# Contributing to AE3

Thanks for helping improve AE3. This guide covers how to report problems, propose changes and get a pull request merged. By taking part you agree to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## Reporting bugs and requesting features

- **Bugs:** open a [bug report](https://github.com/swgoh-utils/swgoh-ae3/issues/new?template=bug_report.yml). Include the AE3 version (`ae3 --version`), your platform, the Asset Version and Platform you were working with, the exact command or GUI steps, and what you expected instead.
- **Features:** open a [feature request](https://github.com/swgoh-utils/swgoh-ae3/issues/new?template=feature_request.yml) describing the problem it solves before writing code, so we can agree on the approach.
- **Security problems:** do **not** open a public issue. Follow the [security policy](SECURITY.md).

Never attach extracted game assets, Bundles or Audio Packages to issues or pull requests. Name the Bundle and Asset Version instead; anyone can fetch the same file from the CDN.

## Development setup

1. Install [rustup](https://rustup.rs). The toolchain pinned in `rust-toolchain.toml` installs automatically on first build.
2. On Linux, install the ALSA headers the GUI's sound playback links against: `libasound2-dev` (Debian, Ubuntu) or `alsa-lib-devel` (Fedora).
3. Fetch the test fixtures, a pinned and checksummed set of real game Packages:

   ```bash
   scripts/fetch-fixtures.sh
   ```

   They land in `fixtures/` (or `$AE3_FIXTURES`), which git ignores. Fixture tests skip whatever is missing.

The workspace is split into crates under `crates/`:

| Crate | Role |
|---|---|
| `ae3-core` | Everything the binaries do: version resolution, Manifests, downloads and cache, Diff, Asset Index, search, export |
| `ae3-unity` | UnityFS Bundles, SerializedFiles and TypeTree reading |
| `ae3-image` | Texture decoding and PNG/WebP encoding |
| `ae3-audio` | Wwise Audio Packages and sound conversion |
| `ae3` | The command-line tool |
| `ae3-gui` | The desktop app (egui) |
| `ae3-oracle` | Development-only cross-checks; never shipped |

The binaries only translate input and present results. New behaviour belongs in `ae3-core` (or the crate below it) so the CLI and GUI stay in step.

## Vocabulary

Code, docs and issues use these terms consistently:

- **Asset Version**: a numbered content release, such as `100054`.
- **Platform**: Windows, Android or iOS asset set.
- **Manifest**: the list of Packages in one Asset Version for one Platform.
- **Package**: a **Bundle** (Unity Assets) or an **Audio Package** (Wwise sound data).
- **Asset**: one Unity object inside a Bundle.
- **Category**: the part of a Bundle name before its first underscore, such as `charui`.
- **Atlas** and **Sprite**: a packed texture and the named regions cut from it (**Atlas Cutting**).
- **Asset Index**: which Assets each Bundle contains; **Name Index**: in-game display names (**Game Entities**) and their Bundles.
- **Diff**: Packages added, changed or removed between two Asset Versions; an **Engine Rebuild** is a Diff where at least 80% of shared Bundles changed.

## Project rules

- **Pure Rust.** No C libraries in shipped binaries (the dev-only `ae3-oracle` is the exception) and no async runtime: network work is synchronous on thread pools.
- **Small binaries.** CI enforces size budgets: `ae3` 5 MiB everywhere; `ae3-gui` 8 MiB (macOS), 11 MiB (Windows, Linux aarch64) and 14 MiB (Linux x86_64). Add dependencies with minimal features and say in the PR what they cost.
- **No unsafe code.** It is forbidden workspace-wide, except for OpenGL calls in `ae3-gui`'s mesh view.
- **Never panic on bad data.** Malformed game files must produce errors, not crashes; decoders run behind a per-Asset panic guard.
- **Game data stays out of git.** Never commit fixtures, extracted assets or files derived from them.

## Checks

Run these before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Tests that need the network are `#[ignore]`d. Run them when your change touches downloads, the CDN or the gamedata mirror:

```bash
cargo test --workspace --all-features -- --ignored
```

Write tests for new behaviour and for every bug fix: a test that fails before the fix and passes after it. GUI logic lives in a model separate from rendering so it can be tested headless.

### What CI runs

- **Every pull request** runs the fast gate (`ci.yml`): commit messages, `rustfmt`, and clippy plus the tests on Linux x86_64. Pull requests that only change documentation skip the Rust jobs.
- **The full matrix** (`full.yml`) builds and tests on Windows, macOS and both Linux architectures, makes release builds, enforces the binary size budgets and runs the texture oracle. It runs nightly when `main` has changed, on release-plz's release pull request, and on any pull request with the `full-ci` label. Add that label when your change is platform-specific or adds a dependency.

### Optional git hooks

The repository ships hooks that catch the two most common CI failures before you push: unformatted code (`pre-commit`) and a commit message that breaks the conventions (`commit-msg`, which needs `cargo install --locked committed`). Enable them once per clone:

```bash
git config core.hooksPath .githooks
```

## Commits and pull requests

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): summary`, for example `fix(gui): keep preview cells from overlapping`.

- **Types:** `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.
- **Scopes:** a crate (`core`, `unity`, `image`, `audio`, `cli`, `gui`) or an area (`ci`, `release`, `readme`).

release-plz builds the changelog and the next version number from these messages, so they matter: `feat` and `fix` commits appear in release notes, `chore`, `ci`, `build` and `test` do not.

`main` is protected: every change, including maintainers', lands through a pull request whose checks pass. Commits are linted with [`committed`](https://github.com/crate-ci/committed) using the rules in `committed.toml`: a subject of at most 72 characters in the imperative mood, no `fixup!` or WIP commits, and no merge commits inside a pull request (rebase on `main` instead). Pull requests merge with a merge commit or a rebase, so each linted commit reaches `main` unchanged.

For pull requests:

1. Branch from `main` and keep each pull request to one change.
2. Fill in the pull request template, including how you tested it.
3. Update `README.md` when behaviour, flags or settings change. Don't edit `CHANGELOG.md`; release-plz writes it.
4. A maintainer reviews and merges once CI is green.

## License

By contributing you agree that your contributions are licensed under the [MIT License](../LICENSE), the same as the project.
