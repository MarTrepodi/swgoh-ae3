# swgoh-ae3

**AE3** is the third-generation asset extractor for *Star Wars: Galaxy of Heroes*. It downloads, previews, searches and exports the game's published assets. It comes as a scriptable command-line tool and a desktop GUI, both small native binaries for Windows, macOS and Linux.

AE3 replaces three earlier tools: [swgoh-ae2](https://github.com/swgoh-utils/swgoh-ae2), swgoh-asset-extractor and Unity Bundle Extractor. It is a ground-up rewrite in Rust, with no .NET, Python or other runtime to install.

> **Status: early development.** The features and commands below describe the target for version 1 and may change before the first release.

## Features

- **Automatic asset version detection.** Finds the latest asset version and content location on every run, and falls back to cached and probed values when the usual source is unavailable.
- **All three platforms' assets.** Windows, Android and iOS asset sets.
- **Asset search.** Finds every bundle that contains an asset whose name matches your search, using an index AE3 builds in the background. Building the index reads only the start of each bundle, and later game releases re-check only the bundles that changed.
- **Live preview.** Select a bundle to see its contents right away:
  - textures, with zoom and per-channel views
  - sprite atlases, with each sprite outlined
  - structured data as a browsable tree
  - text, with syntax highlighting
  - audio playback
  - 3D meshes
- **Sprite separation.** Cuts sprite atlases into individual named images, with metadata about each sprite.
- **Version diffs.** Compares two asset versions and reports added, changed and removed bundles and audio packages, with download sizes. It flags releases where the game moved to a new engine version, and can extract only what changed.
- **Exports:**

  | Asset | Format |
  |---|---|
  | Images | PNG, or lossless WebP |
  | Meshes | OBJ |
  | Audio | Ogg Vorbis, or the original Wwise data |
  | Structured data | JSON |
  | Text | Original bytes |

- **Download cache.** A bundle unchanged between game releases is downloaded only once.

## Installation

Pre-built binaries will be published on the [Releases](https://github.com/swgoh-utils/swgoh-ae3/releases) page. Each release includes:

| Platform | CLI | GUI |
|---|---|---|
| Windows x86_64 | `ae3.exe` | `ae3-gui.exe` |
| macOS (Apple silicon) | `ae3` | `AE3.app` |
| Linux x86_64 / aarch64 | `ae3` | `ae3-gui` (plus a `.desktop` file) |

Early releases are not code-signed:

- **macOS:** right-click `AE3.app` and choose **Open** the first time, or run:

  ```bash
  xattr -d com.apple.quarantine AE3.app
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
| `ae3 diff <old> <new>` | Compare two asset versions |
| `ae3 cache` | Inspect or prune the download cache |
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
ae3 extract charui_bossk --path-id 4712 --image-format webp
```

```bash
ae3 diff 100052 --category charui --summary
```

```bash
ae3 cache prune --max 2GB
```

`ae3 search` updates the asset index first, which reads only the start of each new or changed bundle. `--names-only` matches bundle names without the index, and `--no-update` searches the index as it stands. `ae3 extract` also takes `--layout`, `--image-format png|webp`, `--audio-format ogg|raw`, `--skip-existing` and `--no-atlas-cutting`. With a single bundle, `--path-id` exports only those assets; with a single audio package, `--sound` exports only those sounds.

Exit codes: `0` success, `1` error, `2` invalid arguments or settings, `3` partial failure: some packages or assets in a batch failed, or the batch was cancelled.

`ae3 serve --bind 127.0.0.1:8080` answers swgoh-ae2's routes: `/Asset/list`, `/Asset/listDiff` and `/Asset/single`, with the same parameters (`assetOS` 0 = Windows, 1 = Android, 2 = iOS). Errors come back as JSON with a matching HTTP status.

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

Settings live in a single TOML file in your OS configuration directory, shared by the CLI and the GUI. When the same setting is given more than once, command-line flags win over `AE3_*` environment variables, which win over the file.

## Building from source

You need the Rust toolchain pinned in `rust-toolchain.toml` (rustup installs it automatically).

```bash
cargo build --release
```

The binaries are written to `target/release/`. To include the HTTP API:

```bash
cargo build --release -p ae3 --features serve
```

## License

AE3 is released under the [MIT License](LICENSE). Third-party components keep their own licenses, which are reproduced in each release. These include the BSD-3-Clause Wwise Vorbis codebooks from ww2ogg.

*Star Wars: Galaxy of Heroes* and its assets are the property of their respective owners. AE3 is an unofficial community tool, not affiliated with or endorsed by Electronic Arts, Capital Games or Lucasfilm. Use extracted assets in line with the game's terms and applicable law.
