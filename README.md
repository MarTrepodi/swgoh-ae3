# swgoh-ae3

**AE3** is the third-generation asset extractor for *Star Wars: Galaxy of Heroes*. It downloads, previews, searches and exports the game's published assets. It comes as a scriptable command-line tool and a desktop GUI, both small native binaries for Windows, macOS and Linux.

AE3 replaces three earlier tools: [swgoh-ae2](https://github.com/swgoh-utils/swgoh-ae2), swgoh-asset-extractor and Unity Bundle Extractor. It is a ground-up rewrite in Rust, with no .NET, Python or other runtime to install.

> **Status: early development.** The features and commands below describe the target for version 1 and may change before the first release.

## Features

- **Automatic asset version detection.** Finds the latest asset version and content location on every run, and falls back to cached and probed values when the usual source is unavailable.
- **All three platforms' assets.** Windows, Android and iOS asset sets.
- **Asset search.** Finds every bundle that contains an asset whose name matches your search, using an index AE3 builds in the background. Building the index reads only as much of each bundle as it takes to list its assets, and later game releases re-check only the bundles that changed. The first build for a platform takes a minute or two.
- **Display-name search.** `ae3 search` and the GUI's search box also match the names players see, such as "Bossk" or "Hunting Party", and list the matching units, abilities, gear, materials and player portraits above the bundle matches, each with the bundles it uses. In the GUI, click one to show only its bundles; the names load in the background when the window opens, from the cached copy first. The names come from the community [gamedata](https://github.com/swgoh-utils/gamedata) mirror, downloaded once per game data update, in any of the game's languages.
- **Live preview.** Select a bundle to see its contents right away:
  - textures, with zoom and per-channel views
  - sprite atlases, with each sprite outlined
  - structured data as a browsable tree
  - text, with syntax highlighting
  - audio packages as a list of sounds with codec, channels, sample rate and duration, each with a play button (Wwise Vorbis, PCM and PTADPCM sounds; other codecs show "can't play")
  - 3D meshes, shaded, with an orbiting camera: drag to rotate, right-drag to pan, scroll to zoom
- **Sprite separation.** Cuts sprite atlases into individual named images, with metadata about each sprite. Many bundles carry a copy of an atlas whose texture lives in one shared bundle; those sprites are cut once, into the shared bundle's folder, and each copy's metadata points there.
- **Version diffs.** Compares two asset versions and reports added, changed and removed bundles and audio packages, with download sizes. It flags releases where the game moved to a new engine version, and can extract only what changed.
- **Exports:**

  | Asset | Format |
  |---|---|
  | Images | PNG, or lossless WebP |
  | Sprite atlases | One image per sprite, plus a JSON file describing each sprite |
  | Meshes | Wavefront OBJ, oriented as swgoh-ae2 wrote them, with normals, UVs and one group per submesh; or binary glTF (`.glb`) with the main texture embedded; or both |
  | Audio | Ogg Vorbis, WAV (Wwise PCM and PTADPCM), or the original Wwise data |
  | Structured data | JSON: by default one `{bundle}.structured.json` per bundle, or one file per asset |
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

Every archive also contains `LICENSE`, `README.md`, `CHANGELOG.md`, `THIRD-PARTY-NOTICES.md` (the Wwise Vorbis codebooks) and `THIRD-PARTY-LICENSES.md` (the Rust crates built in). Each archive has a `.sha256` checksum file next to it on the release page. On Windows and macOS the binaries need nothing else installed. The Linux builds need glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later). The Linux GUI also needs the ALSA library (`libasound.so.2`, package `libasound2` or `alsa-lib`) to start; desktop distributions install it by default, and PipeWire and PulseAudio play its sound through their ALSA plugins.

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
| `ae3 search <text>` | Find bundles by display name, asset name or bundle name |
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

`ae3 search` updates the asset index first, which reads only the start of each new or changed bundle, and prints each matching bundle with the matching assets and their path ids. Above them it lists the game entities (units, abilities, gear, materials and player portraits) whose display names match, best match first, each with the bundles it links to. The display names are rebuilt from the community gamedata on GitHub when it changes, about 25 MB of downloads that leave a cached index of about 0.5 MB; when GitHub is unreachable the cached names are used. `--names-only` matches bundle names and display names without the asset index, `--no-update` searches both indexes as they stand, `--no-display-names` leaves the game entities out, and `--language` picks the display-name language for one search. `ae3 extract` takes package names, or `--category <name>`, `--all` (every bundle) or `--audio` (every audio package). It also takes `--layout ae3|ae2`, `--image-format png|webp`, `--audio-format ogg|raw`, `--skip-existing`, `--no-atlas-cutting`, `--no-padding-restoration`, `--structured combined|separate|none` and `--mesh-format obj|glb|both`. With a single bundle, `--path-id` exports only those assets; with a single audio package, `--sound` exports only those sounds.

Structured assets are the MonoBehaviours and other Unity objects that are not images, atlases, meshes, text or audio. A category such as `conquest` holds tens of thousands of them, so by default (`--structured combined`) each bundle's structured assets go into one `{bundle}.structured.json` next to its other files. That file is a JSON object with one member per asset, keyed by the file name the asset would otherwise get (its name, with `_{path id}` added when two assets share a name). Each member holds the asset's `path_id`, `name`, `class_name`, `script_class` and `container_path` when known, and its `data`. `--structured separate` writes one `{asset}.json` per asset instead, as earlier releases did, and `--structured none` skips them. Assets picked with `--path-id` are always written as their own files.

Meshes are written as OBJ by default. `--mesh-format glb` writes binary glTF instead, and `--mesh-format both` writes both files side by side. A `.glb` holds the same geometry as the OBJ (positions, normals, UVs and one primitive per submesh, mirrored into glTF's right-handed space), and each submesh's material carries the main texture its renderer uses, embedded as PNG. AE3 finds that texture through the renderer in the same bundle that draws the mesh and that renderer's material. When no renderer in the bundle draws the mesh, as in shared resource containers, or its material has no texture, the `.glb` holds the geometry only and the export report says why.

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
structured = "combined"       # combined, separate or none
mesh-format = "obj"           # obj, glb or both
no-cache = false
language = "ENG_US"           # display-name search: ENG_US, FRE_FR, GER_DE, JPN_JP, ...
check-for-updates = true      # GUI only: look for a new AE3 release once a day
```

Each key also has an environment variable: `AE3_PLATFORM`, `AE3_OUTPUT_DIR`, `AE3_CACHE_DIR`, `AE3_CACHE_CAP`, `AE3_PARALLEL_DOWNLOADS`, `AE3_IMAGE_FORMAT`, `AE3_AUDIO_FORMAT`, `AE3_LAYOUT`, `AE3_ATLAS_CUTTING`, `AE3_PADDING_RESTORATION`, `AE3_SKIP_EXISTING`, `AE3_STRUCTURED`, `AE3_MESH_FORMAT`, `AE3_NO_CACHE`, `AE3_LANGUAGE` and `AE3_CHECK_FOR_UPDATES`. When the same setting is given more than once, command-line flags win over `AE3_*` environment variables, which win over the file.

With `check-for-updates` on, the GUI asks GitHub Releases at most once a day whether a newer AE3 has been published, without slowing the window down, and shows "AE3 vX available" with a link to the release page. It never downloads or installs anything. The CLI never checks.

## Building from source

You need the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it automatically). On Linux the GUI's sound playback also needs the ALSA headers: `libasound2-dev` on Debian and Ubuntu, `alsa-lib-devel` on Fedora.

```bash
cargo build --release
```

The binaries are written to `target/release/`. To include the HTTP API:

```bash
cargo build --release -p ae3 --features serve
```

The release archives are built without `serve`.

### Tests and fixtures

Many tests decode real game data. `scripts/fetch-fixtures.sh` downloads a pinned, checksummed list of Packages from Asset Version 100054 for every Platform into `fixtures/` (or `$AE3_FIXTURES`); fixture tests skip whatever is missing, and CI fetches the whole list on every platform.

```bash
scripts/fetch-fixtures.sh
cargo test --workspace
```

The `ae3-oracle` crate is development-only and never shipped. Its default tests check every texture format SWGOH uses against pinned statistics and across Platforms. With the `oracle` feature, which builds `unity-rs-core` and needs a C compiler for its zstd, it also decodes every fixture texture with that library and compares pixels:

```bash
cargo test -p ae3-oracle --features oracle -- --include-ignored
```

### Releasing

Releases are cut from conventional commits (`feat:`, `fix:`, `perf:` and so on). On every push to `main`, [release-plz](https://release-plz.dev) opens or updates a release PR that bumps the shared workspace version and adds the new section to `CHANGELOG.md`. Merging that PR tags `vX.Y.Z`, and the tag starts [cargo-dist](https://axodotdev.github.io/cargo-dist/) in `.github/workflows/release.yml`, which builds the archives above and publishes the GitHub Release. Nothing is published to crates.io.

release-plz needs a repository secret, `RELEASE_PLZ_TOKEN`: a fine-grained personal access token (or a GitHub App token) with read and write access to Contents and Pull requests on this repository. The default `GITHUB_TOKEN` does not work here, because GitHub starts no workflows for what it pushes: CI would not run on the release PR, and the tag would build no release. The release-plz jobs run only in the `swgoh-utils` organisation; to try them in another repository, set its repository variable `RELEASE_PLZ` to `true`. `release.yml` and CI need no secrets.

The first release is the exception: release-plz opens no PR while `CHANGELOG.md` already has the hand-written 0.1.0 section, so push its tag yourself:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/), `type(scope): summary`, with a crate (`core`, `unity`, `image`, `audio`, `cli`, `gui`) or area (`ci`, `release`, `readme`) as the scope.

`release.yml` is generated: change `[workspace.metadata.dist]` in `Cargo.toml`, then run `dist generate`. Pull requests run `dist plan` to check the release config.

## License

AE3 is released under the [MIT License](LICENSE). Third-party components keep their own licenses, which are reproduced in each release. These include the BSD-3-Clause Wwise Vorbis codebooks from ww2ogg.

*Star Wars: Galaxy of Heroes* and its assets are the property of their respective owners. AE3 is an unofficial community tool, not affiliated with or endorsed by Electronic Arts, Capital Games or Lucasfilm. Use extracted assets in line with the game's terms and applicable law.
