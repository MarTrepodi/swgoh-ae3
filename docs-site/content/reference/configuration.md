# Configuration

The CLI, the GUI and the HTTP API share one settings file, `config.toml`,
and one download cache.

## Where settings come from

When the same setting is given more than once, the most specific wins:

1. command-line flags (CLI only),
2. `AE3_*` environment variables,
3. the configuration file,
4. built-in defaults.

The GUI's **Settings** window reads and writes the configuration file.

## The configuration file

| OS | Location |
|---|---|
| Windows | `%APPDATA%\ae3\config.toml` |
| macOS | `~/Library/Application Support/ae3/config.toml` |
| Linux | `$XDG_CONFIG_HOME/ae3/config.toml` (default `~/.config/ae3/config.toml`) |

Set `AE3_CONFIG` to the path of another file to use it instead, for
example a per-project or CI configuration.

Every key is optional. All keys, with their defaults:

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

The GUI also keeps a little state of its own in a `[gui]` table (such as
the last asset version it loaded). Leave it to the GUI.

## Settings

| Key | Environment variable | CLI flag | Values | Default |
|---|---|---|---|---|
| `platform` | `AE3_PLATFORM` | `--platform` | `windows`, `android`, `ios` | `windows` |
| `output-dir` | `AE3_OUTPUT_DIR` | `-o`, `--output` | path | `ae3-output` (relative to the working directory) |
| `cache-dir` | `AE3_CACHE_DIR` | `--cache-dir` | path | see [Cache](#cache) |
| `cache-cap` | `AE3_CACHE_CAP` | — | size, such as `5GB`, `500MiB` | `5GB` |
| `parallel-downloads` | `AE3_PARALLEL_DOWNLOADS` | `--parallel` | 1–256 | `8` |
| `image-format` | `AE3_IMAGE_FORMAT` | `--image-format` | `png`, `webp` | `png` |
| `audio-format` | `AE3_AUDIO_FORMAT` | `--audio-format` | `ogg`, `raw` | `ogg` |
| `layout` | `AE3_LAYOUT` | `--layout` | `ae3`, `ae2` | `ae3` |
| `atlas-cutting` | `AE3_ATLAS_CUTTING` | `--[no-]atlas-cutting` | boolean | `true` |
| `padding-restoration` | `AE3_PADDING_RESTORATION` | `--[no-]padding-restoration` | boolean | `true` |
| `skip-existing` | `AE3_SKIP_EXISTING` | `--skip-existing`, `--overwrite` | boolean | `false` |
| `structured` | `AE3_STRUCTURED` | `--structured` | `combined`, `separate`, `none` | `combined` |
| `mesh-format` | `AE3_MESH_FORMAT` | `--mesh-format` | `obj`, `glb`, `both` | `obj` |
| `no-cache` | `AE3_NO_CACHE` | `--no-cache` | boolean | `false` |
| `language` | `AE3_LANGUAGE` | `search --language` | see [languages](../guides/identify-assets.md#search-in-another-language) | `ENG_US` |
| `check-for-updates` | `AE3_CHECK_FOR_UPDATES` | — | boolean | `true` |

Booleans in environment variables accept `true`/`false`, `1`/`0`,
`yes`/`no` and `on`/`off`. Sizes take `KB`, `MB`, `GB`, `TB` (powers of
1000) or `KiB`, `MiB`, `GiB`, `TiB` (powers of 1024), or a plain number of
bytes.

## Cache

AE3 keeps downloaded bundles and manifests, the asset index and the name
index in one cache folder:

| OS | Default location |
|---|---|
| Windows | `%LOCALAPPDATA%\ae3` |
| macOS | `~/Library/Caches/ae3` |
| Linux | `$XDG_CACHE_HOME/ae3` (default `~/.cache/ae3`) |

| What | Where in the cache | Size |
|---|---|---|
| Bundles and audio packages | per platform | Up to `cache-cap` (5 GB); least recently used dropped first |
| Manifests | per version and platform | About 0.7 MB each |
| Asset index | `index/{Platform}.ae3index` | About 5 MB per platform |
| Name index | `names/{LANGUAGE}.json` | About 0.5 MB per language |

A bundle whose content hash didn't change between asset versions is
downloaded once and reused. Manage the cache with
[`ae3 cache`](cli.md#cache):

```bash
ae3 cache                 # where it is and what it holds
ae3 cache prune --max 2GB # shrink it
ae3 cache clear           # empty it (add --index to drop the asset indexes too)
```

`--no-cache` (or `no-cache = true`) uses a temporary folder instead, so
nothing is reused or kept.

## Update checks

With `check-for-updates` on (the default), the GUI asks GitHub Releases at
most once a day whether a newer AE3 has been published, without slowing
the window down, and shows "swgoh-ae3 vX available" with a link to the
release page. It never downloads or installs anything. **About → Check for
updates** asks straight away, whether or not the setting is on. The CLI
never checks.

## Network access

AE3 contacts:

| Host | Why |
|---|---|
| The game's CDN (`eaassets-a.akamaihd.net`) | Manifests and packages |
| `raw.githubusercontent.com` | The community metadata mirror (latest asset version) and game data (display names) |
| `swgoh-bootstrap.akamaized.net` | Fallback for the game environment when the mirror is unreachable |
| `api.github.com` | The GUI's update check |
