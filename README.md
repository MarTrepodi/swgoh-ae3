# swgoh-ae3

**AE3** is the third-generation asset extractor for *Star Wars: Galaxy of Heroes*. It downloads, previews, searches and exports the game's published assets. It comes as a scriptable command-line tool and a desktop GUI, both small native binaries for Windows, macOS and Linux.

AE3 replaces three earlier tools: [swgoh-ae2](https://github.com/swgoh-utils/swgoh-ae2), swgoh-asset-extractor and Unity Bundle Extractor. It is a ground-up rewrite in Rust, with no .NET, Python or other runtime to install.

> **Status: early development.** The features and commands below describe the target for version 1 and may change before the first release.

## Features

- **Automatic asset version detection.** Finds the latest asset version and content location on every run, and falls back to cached and probed values when the usual source is unavailable.
- **All three platforms' assets.** Windows, Android and iOS asset sets.
- **Asset search.** Finds every bundle that contains an asset whose name matches your search, using an index AE3 builds in the background. Building the index reads only as much of each bundle as it takes to list its assets, and later game releases re-check only the bundles that changed. The first build for a platform takes a few minutes.
- **Live preview.** Select a bundle to see its contents right away:
  - textures, with zoom and per-channel views
  - sprite atlases, with each sprite outlined
  - structured data as a browsable tree
  - text, with syntax highlighting
  - audio packages as a list of sounds with codec, channels, sample rate and duration (playback is planned)
  - 3D meshes, shaded, with an orbiting camera: drag to rotate, right-drag to pan, scroll to zoom
- **Sprite separation.** Cuts sprite atlases into individual named images, with metadata about each sprite. Many bundles carry a copy of an atlas whose texture lives in one shared bundle; those sprites are cut once, into the shared bundle's folder, and each copy's metadata points there.
- **Version diffs.** Compares two asset versions and reports added, changed and removed bundles and audio packages, with download sizes. It flags releases where the game moved to a new engine version, and can extract only what changed.
- **Exports:**

  | Asset | Format |
  |---|---|
  | Images | PNG, or lossless WebP |
  | Sprite atlases | One image per sprite, plus a JSON file describing each sprite |
  | Meshes | Wavefront OBJ, oriented as swgoh-ae2 wrote them, with normals, UVs and one group per submesh |
  | Audio | Ogg Vorbis, or the original Wwise data |
  | Structured data | JSON |
  | Text | Original bytes |

- **Download cache.** A bundle unchanged between game releases is downloaded only once. The cache keeps up to 5 GB by default and drops the least recently used packages first.

## Installation

Pre-built binaries are published on the [Releases](https://github.com/swgoh-utils/swgoh-ae3/releases) page. The CLI and the GUI come as separate archives:

| Platform | CLI archive (contains) | GUI archive (contains) |
|---|---|---|
| Windows x86_64 | `ae3-x86_64-pc-windows-msvc.zip` (`ae3.exe`) | `ae3-gui-x86_64-pc-windows-msvc.zip` (`ae3-gui.exe`) |
| macOS (Apple silicon) | `ae3-aarch64-apple-darwin.tar.xz` (`ae3`) | `ae3-gui-aarch64-apple-darwin.zip` (`AE3.app`) |
| Linux x86_64 | `ae3-x86_64-unknown-linux-gnu.tar.xz` (`ae3`) | `ae3-gui-x86_64-unknown-linux-gnu.tar.xz` (`ae3-gui`, `ae3-gui.desktop`) |
| Linux aarch64 | `ae3-aarch64-unknown-linux-gnu.tar.xz` (`ae3`) | `ae3-gui-aarch64-unknown-linux-gnu.tar.xz` (`ae3-gui`, `ae3-gui.desktop`) |

Every archive also contains `LICENSE`, `README.md`, `CHANGELOG.md`, `THIRD-PARTY-NOTICES.md` (the Wwise Vorbis codebooks) and `THIRD-PARTY-LICENSES.md` (the Rust crates built in). Each archive has a `.sha256` checksum file next to it on the release page. The binaries need nothing else installed. The Linux builds need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later).

To add the GUI to a Linux desktop's application menu, run this from the extracted folder:

```bash
install -Dm755 ae3-gui ~/.local/bin/ae3-gui && install -Dm644 ae3-gui.desktop ~/.local/share/applications/ae3-gui.desktop
```

The `.desktop` file starts `ae3-gui` from your `PATH`, so `~/.local/bin` must be on it.

Early releases are not code-signed. `AE3.app` is signed ad hoc only:

- **macOS:** the first launch of `AE3.app` or `ae3` is blocked. Open **System Settings** → **Privacy & Security** and choose **Open Anyway**, or clear the download quarantine first:

  ```bash
  xattr -dr com.apple.quarantine AE3.app ae3
  ```

- **Windows:** SmartScreen may warn on first launch. Choose **More info** → **Run anyway**.

## Command-line usage

```bash
ae3 version
```

```bash
ae3 search jarjar
```

```bash
ae3 extract charui_bossk --output ./assets
```

```bash
ae3 diff 100052 100054 --extract added
```

| Command | Purpose |
|---|---|
| `ae3 version` | Show the latest asset version and where content is served from |
| `ae3 manifest` | Download and summarize the manifest for an asset version |
| `ae3 list` | List bundles and audio packages |
| `ae3 search <text>` | Find bundles containing assets whose names match |
| `ae3 index` | Build or update the asset index used by search |
| `ae3 extract` | Export assets from bundles and audio packages |
| `ae3 diff <old> [new]` | Compare two asset versions (`new` defaults to the latest) |
| `ae3 cache` | Inspect, prune or clear the download cache |
| `ae3 serve` | Run the HTTP API (optional build feature) |

Global options include `--platform`, `--asset-version`, `--output`, `--json`, `-v`/`-q`, `--parallel`, `--cache-dir` and `--no-cache`. `--json` prints one JSON document on stdout, with snake_case keys; when a command fails, that document is `{"error": {...}}`. Progress is written to stderr, so stdout stays clean for pipes. Ctrl-C stops a running batch cleanly, keeping what is already written and indexed. Press it again to quit at once.

More examples:

```bash
ae3 list --categories
```

```bash
ae3 search bossk --names-only
```

```bash
ae3 extract charui_bossk --path-id -6516795281392349143 --image-format webp
```

```bash
ae3 diff 100052 --category charui --summary
```

```bash
ae3 cache prune --max 2GB
```

`ae3 search` updates the asset index first, which reads only the start of each new or changed bundle, and prints each matching bundle with the matching assets and their path ids. `--names-only` matches bundle names without the index, and `--no-update` searches the index as it stands. `ae3 extract` takes package names, or `--category <name>`, `--all` (every bundle) or `--audio` (every audio package). It also takes `--layout ae3|ae2`, `--image-format png|webp`, `--audio-format ogg|raw`, `--skip-existing`, `--no-atlas-cutting` and `--no-padding-restoration`. With a single bundle, `--path-id` exports only those assets; with a single audio package, `--sound` exports only those sounds.

`ae3 diff --category <name>` compares only that category's packages: the counts, sizes, changed share and engine-rebuild flag all describe that category, in text and `--json` output alike.

Exit codes: `0` success, `1` error, `2` invalid arguments or settings, `3` partial failure: some packages or assets in a batch failed, or the batch was cancelled.

`ae3 serve` (listening on `127.0.0.1:8080` unless `--bind` says otherwise) answers swgoh-ae2's routes: `/Asset/list`, `/Asset/listDiff` and `/Asset/single`, with the same parameters (`assetOS` 0 = Windows, 1 = Android, 2 = iOS). Errors come back as JSON with a matching HTTP status.

### Migrating from swgoh-ae2

| swgoh-ae2 | AE3 |
|---|---|
| `-downloadManifest` | `ae3 manifest` |
| `-getAssetNames` | `ae3 list --json` |
| `-getAssetPrefixes` | `ae3 list --categories --json` |
| `-single <name>` | `ae3 extract <bundle>` |
| `-prefix <prefix>` | `ae3 extract --category <prefix>` |
| `-all` | `ae3 extract --all` |
| `-audio` | `ae3 extract --audio` |
| `-dd <old> new\|changed\|all` | `ae3 diff <old> <current> --extract added\|changed\|all` |
| `-dm <old.data> <new.data>` | `ae3 diff <old> <new>` |
| `-v <version>` | `--asset-version <version>` |
| `-t <folder>` | `--output <folder>` |
| `-w <folder>` | Not needed. Downloads go to a managed cache (`ae3 cache`) |

AE3 writes to `{output}/{category}/{bundle}/` by default. Pass `--layout ae2` to get swgoh-ae2's `{output}/{prefix}/` layout. ae2's "prefix" is called a **category** in AE3.

## Configuration

Settings live in a single TOML file, `config.toml`, shared by the CLI and the GUI:

| OS | Location |
|---|---|
| Windows | `%APPDATA%\ae3\config.toml` |
| macOS | `~/Library/Application Support/ae3/config.toml` |
| Linux | `$XDG_CONFIG_HOME/ae3/config.toml` (default `~/.config/ae3/config.toml`) |

Set `AE3_CONFIG` to use another file. Every key is optional:

```toml
platform = "windows"          # windows, android or ios
output-dir = "ae3-output"
cache-dir = "/path/to/cache"  # default: the OS cache directory
cache-cap = "5GB"
parallel-downloads = 8
image-format = "png"          # png or webp
audio-format = "ogg"          # ogg or raw
layout = "ae3"                # ae3 or ae2
atlas-cutting = true
padding-restoration = true
skip-existing = false
no-cache = false
```

Each key also has an environment variable: `AE3_PLATFORM`, `AE3_OUTPUT_DIR`, `AE3_CACHE_DIR`, `AE3_CACHE_CAP`, `AE3_PARALLEL_DOWNLOADS`, `AE3_IMAGE_FORMAT`, `AE3_AUDIO_FORMAT`, `AE3_LAYOUT`, `AE3_ATLAS_CUTTING`, `AE3_PADDING_RESTORATION`, `AE3_SKIP_EXISTING` and `AE3_NO_CACHE`. When the same setting is given more than once, command-line flags win over `AE3_*` environment variables, which win over the file.

## Building from source

You need the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it automatically).

```bash
cargo build --release
```

The binaries are written to `target/release/`. To include the HTTP API:

```bash
cargo build --release -p ae3 --features serve
```

The release archives are built without `serve`.

### Releasing

Releases are cut from conventional commits (`feat:`, `fix:`, `perf:` and so on). On every push to `main`, [release-plz](https://release-plz.dev) opens or updates a release PR that bumps the shared workspace version and adds the new section to `CHANGELOG.md`. Merging that PR tags `vX.Y.Z`, and the tag starts [cargo-dist](https://axodotdev.github.io/cargo-dist/) in `.github/workflows/release.yml`, which builds the archives above and publishes the GitHub Release. Nothing is published to crates.io.

The first release is the exception: release-plz opens no PR while `CHANGELOG.md` already has the hand-written 0.1.0 section, so push its tag yourself:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/), `type(scope): summary`, with a crate (`core`, `unity`, `image`, `audio`, `cli`, `gui`) or area (`ci`, `release`, `readme`) as the scope.

`release.yml` is generated: change `[workspace.metadata.dist]` in `Cargo.toml`, then run `dist generate`. Pull requests run `dist plan` to check the release config.

## License

AE3 is released under the [MIT License](LICENSE). Third-party components keep their own licenses, which are reproduced in each release. These include the BSD-3-Clause Wwise Vorbis codebooks from ww2ogg.

*Star Wars: Galaxy of Heroes* and its assets are the property of their respective owners. AE3 is an unofficial community tool, not affiliated with or endorsed by Electronic Arts, Capital Games or Lucasfilm. Use extracted assets in line with the game's terms and applicable law.
