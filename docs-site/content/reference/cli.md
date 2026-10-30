# Command line reference

```text
ae3 [OPTIONS] <COMMAND>
```

| Command | Purpose |
|---|---|
| [`version`](#version) | Show the asset version and where content is served from |
| [`manifest`](#manifest) | Download and summarise the manifest; optionally save it |
| [`list`](#list) | List bundles, audio packages or categories |
| [`search`](#search) | Find bundles by display name, asset name or bundle name |
| [`index`](#index) | Build or update the asset index |
| [`extract`](#extract) | Export assets |
| [`diff`](#diff) | Compare two asset versions; optionally export the difference |
| [`cache`](#cache) | Inspect, prune or clear the download cache |
| [`serve`](#serve) | Run the HTTP API (builds with the `serve` feature only) |

`ae3 help <command>` or `ae3 <command> --help` prints each command's full
help. `ae3 --version` prints AE3's own version.

## Global options

These work with every command, before or after the command name.

| Option | Default | Description |
|---|---|---|
| `--platform PLATFORM` | `windows` | Asset set: `windows`, `android` or `ios` |
| `--asset-version N` | latest | Use this asset version instead of resolving the latest |
| `-o`, `--output DIR` | `ae3-output` | Directory results are written into |
| `--json` | | Print one JSON document on stdout instead of text |
| `-v`, `--verbose` | | Say more on stderr; repeat (`-vv`) for even more |
| `-q`, `--quiet` | | Print no progress or warnings on stderr; errors still print |
| `--cache-dir DIR` | OS cache folder | Download cache directory |
| `--no-cache` | | Don't reuse or keep downloads; use a temporary directory |
| `--parallel N` | `8` | Packages downloaded and exported at once (1–256) |

`-v` and `-q` can't be combined. Most of these can also be set in the
[configuration file](configuration.md) or with `AE3_*` environment
variables; flags win.

## Output conventions

- **stdout** carries the result: text, or one JSON document with
  `--json`. **stderr** carries progress, warnings and errors.
- With `--json`, a failure prints `{"error": {"message": "...", "hint": "..."}}`
  (`hint` when there is one).
- Sizes in text output are decimal (1 KB = 1000 bytes). JSON sizes are bytes.
- Path ids and content hashes are 64-bit integers.

**Exit codes:** `0` success · `1` error · `2` invalid arguments or settings ·
`3` partial failure (some packages or assets failed, or the batch was
cancelled) · `130` interrupted by a second ++ctrl+c++.

++ctrl+c++ once stops a running batch cleanly, keeping what's already written
and indexed; a second press quits at once.

---

## version

Resolve the asset version and content location to use.

```bash
ae3 version
```

```text
Asset Version  100056 (github-meta)
Environment    PROD
Platform       Windows
Manifest       https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100056/Windows/ETC/manifest.data
```

The word in brackets is where the version came from:

| Source | Meaning |
|---|---|
| `explicit` | You passed `--asset-version` |
| `github-meta` | The community metadata mirror on GitHub (the usual case) |
| `probe` | The mirror was unavailable; AE3 started from the last version it saw and probed the CDN for newer ones |

If none of these works, AE3 asks you to pass `--asset-version`.

??? example "JSON"
    ```json
    {
      "asset_version": 100056,
      "environment": "PROD",
      "platform": "Windows",
      "base_url": "https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/",
      "manifest_url": "https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100056/Windows/ETC/manifest.data",
      "source": "github-meta"
    }
    ```

## manifest

Download and summarise the manifest. With `--output`, also save the raw
`manifest.data` there as `manifest-{version}-{platform}.data`.

```bash
ae3 manifest -o manifests
```

```text
Asset Version   100056
Platform        Windows
Environment     PROD
Manifest        https://eaassets-a.akamaihd.net/assetssw.capitalgames.com/PROD/100056/Windows/ETC/manifest.data
Packages        11654 (1.5 GB)
  Bundles       11501 (1.3 GB)
  Audio         153 (154.4 MB)
Categories      292
Saved to        manifests/manifest-100056-Windows.data
```

??? example "JSON"
    ```json
    {
      "asset_version": 100056, "platform": "Windows", "environment": "PROD",
      "manifest_environment": "STAGE",
      "manifest_url": "https://…/manifest.data",
      "tex_format": "ETC", "timestamp": 1793135346, "revision": 24977,
      "packages": 11654, "bundles": 11501, "audio_packages": 153, "other_packages": 0,
      "categories": 292,
      "total_size": 1472172517, "bundle_size": 1317724323, "audio_size": 154448194
    }
    ```

## list

List the packages in the manifest, sorted by name, or the categories.

```text
ae3 list [--categories] [--category CATEGORY] [--kind bundle|audio]
```

| Option | Description |
|---|---|
| `--categories` | List categories with their package counts and sizes |
| `--category CATEGORY` | Only packages of this category (case-insensitive) |
| `--kind bundle\|audio` | Only bundles, or only audio packages |

```bash
ae3 list --category shipskill
```

```text
shipskill_pentagon  bundle     6.7 KB
shipskill_square    bundle     6.6 KB
shipskill_star      bundle     6.7 KB
shipskill_triangle  bundle     6.3 KB
```

??? example "JSON"
    `ae3 list --json`:

    ```json
    {
      "asset_version": 100056,
      "platform": "Windows",
      "packages": [
        { "name": "charui_4lom", "kind": "bundle", "category": "charui",
          "size": 6375, "content_hash": 7238027183850002663 }
      ]
    }
    ```

    `ae3 list --categories --json`:

    ```json
    {
      "asset_version": 100056,
      "platform": "Windows",
      "categories": [
        { "name": "ability", "packages": 2617, "bundles": 2617,
          "audio_packages": 0, "size": 88715583 }
      ]
    }
    ```

## search

Find bundles by display name, asset name or bundle name.

```text
ae3 search <QUERY> [--names-only | --no-update] [--no-display-names] [--language LANG]
```

The query is a case-insensitive substring. Results come in two sections:

1. **Game Entities**: units, abilities, gear, materials and player
   portraits whose display names match, best match first, each with the
   bundles it links to.
2. **Bundles**: packages whose name matches, or that contain assets whose
   names match, each with those assets' kinds, names and path ids.

Before searching, AE3 brings the [asset index](../concepts.md#asset-index)
up to date (reading only new or changed bundles) and checks the community
game data for new display names.

| Option | Description |
|---|---|
| `--names-only` | Match bundle names and display names only; don't build or read the asset index |
| `--no-update` | Search the stored asset index and name index as they are, with no network checks |
| `--no-display-names` | Leave out game entities |
| `--language LANG` | Display-name language, by code or name (`FRE_FR`, `fr`, `french`); default the configured `language`, else English |

```bash
ae3 search zeta --names-only
```

```text
Game Entities
  Material  Ability Material Zeta: skill_zeta

Bundles
pack_galactic_zeta_bundle
...
skill_zeta
skill_zeta_glow
```

??? example "JSON"
    ```json
    {
      "asset_version": 100056,
      "platform": "Windows",
      "names_only": false,
      "name_index": {
        "status": "current",
        "language": "ENG_US",
        "gamedata_version": "0.40.6:W8EftHi2TSizGnD-1QzTFg",
        "localization_version": "NBtg8ikGTz-rnCFHrdUd8A"
      },
      "query": "bossk",
      "entities": [
        { "kind": "unit", "id": "BOSSK", "name": "Bossk", "rank": "exact",
          "bundles": ["charui_bossk", "char_bossk_pre", "relic_rfl_bossk"] },
        { "kind": "player-portrait", "id": "PLAYERPORTRAIT_BOSSK", "name": "Bossk",
          "rank": "exact", "bundles": ["vanity_bossk"] },
        { "kind": "ability", "id": "contractability_bossk01", "name": "Bossk's Payout",
          "context": "Bossk", "rank": "prefix", "bundles": ["abilityui_passive_contract"] },
        { "kind": "gear", "id": "G12Finisher_BOSSK_C",
          "name": "Power Cell Injector (Fusion) - Bossk", "rank": "word-prefix",
          "bundles": ["equip_powercellinjector"] }
      ],
      "hits": [
        { "package": "charui_bossk", "kind": "bundle", "category": "charui",
          "name_matched": true, "indexed": true, "kinds": ["texture"],
          "assets": [
            { "path_id": -6516795281392349143, "kind": "texture", "name": "tex.charui_bossk" }
          ] }
      ],
      "coverage": { "indexed_bundles": 11501, "failed_bundles": 0,
                    "total_bundles": 11501, "audio_packages": 153, "other_packages": 0 }
    }
    ```

    - `name_index` is `null` with `--no-display-names`. Its `status` is
      `current`, `built`, `unchecked`, `stale` or `unavailable`, with a
      `problem` when it is `stale` or `unavailable`.
    - `entities[].kind` is `unit`, `ability`, `gear`, `material` or
      `player-portrait`. `context` appears only on abilities.
    - `entities[].rank` is `exact` (the name is the query), `prefix` (the
      name starts with it), `word-prefix` (a later word starts with it) or
      `substring`. Entities are sorted by rank. Comparison ignores case,
      punctuation and repeated spaces.
    - `hits[].indexed` is `false` for a bundle the asset index couldn't
      read; it can match by name only.

If the index doesn't yet cover every bundle, search notes on stderr how
many bundles it couldn't look inside.

## index

Build or update the asset index for the current asset version and
platform. `ae3 search` does this automatically; run it on its own to
prepare an index ahead of time.

```text
ae3 index [--plan] [--retry-failed] [--reprobe-all]
```

| Option | Description |
|---|---|
| `--plan` | Only show what an update would read |
| `--retry-failed` | Re-read bundles whose earlier read failed for good |
| `--reprobe-all` | Re-read every bundle, ignoring the stored index |

```bash
ae3 index --plan
```

```text
Asset Version   100056
Bundles         11501
Indexed         11501
Unreadable      0
Changed         0
New             0
To probe        0 (reads only the start of their 0 B)
Index file      /Users/you/Library/Caches/ae3/index/Windows.ae3index
```

??? example "JSON"
    `ae3 index --plan --json`:

    ```json
    {
      "asset_version": 100056, "platform": "Windows",
      "path": "/Users/you/Library/Caches/ae3/index/Windows.ae3index",
      "plan": { "asset_version": 100056, "total_bundles": 11501, "audio_packages": 153,
                "indexed": 11501, "failed": 0, "changed": 0, "new": 0,
                "to_probe": 0, "to_probe_size": 0 }
    }
    ```

    Without `--plan`, the document has `coverage` (as in `search`) and a
    `report` of the run.

## extract

Export assets from bundles and sounds from audio packages.

```text
ae3 extract <PACKAGE...|--category CATEGORY|--all|--audio> [OPTIONS]
```

Exactly one selection is required:

| Selection | Exports |
|---|---|
| `PACKAGE...` | The named bundles and/or audio packages |
| `--category CATEGORY` | Every package in the category |
| `--all` | Every bundle |
| `--audio` | Every audio package |

Narrowing a single package:

| Option | Description |
|---|---|
| `--path-id ID` | Only these assets, by path id (repeatable); needs exactly one bundle |
| `--sound ID` | Only these sounds, by Wwise id (repeatable); needs exactly one audio package |

Export options (also accepted by [`diff`](#diff)):

| Option | Values | Default | Description |
|---|---|---|---|
| `--layout` | `ae3`, `ae2` | `ae3` | `{output}/{category}/{bundle}/` or `{output}/{category}/` |
| `--image-format` | `png`, `webp` | `png` | Textures and sprites; WebP is lossless |
| `--audio-format` | `ogg`, `raw` | `ogg` | Ogg Vorbis (PCM and PTADPCM become WAV) or the original Wwise data |
| `--mesh-format` | `obj`, `glb`, `both` | `obj` | Wavefront OBJ, binary glTF with the main texture embedded, or both |
| `--structured` | `combined`, `separate`, `none` | `combined` | Data objects: one `{bundle}.structured.json` per bundle, one `{asset}.json` per asset, or none |
| `--skip-existing` / `--overwrite` | | overwrite | Leave existing files alone, or replace them |
| `--no-atlas-cutting` / `--atlas-cutting` | | cutting on | Write atlases as JSON instead of one image per sprite |
| `--no-padding-restoration` / `--padding-restoration` | | restoration on | Keep sprites trimmed instead of restoring their transparent padding |

Of each pair, the flag given last wins.

```bash
ae3 extract charui_bossk skill_zeta -o ./icons --image-format webp
```

```text
Exporting 2 Packages to ./icons
Wrote 2 files (29.8 KB) from 2 of 2 Packages to ./icons
```

Then, if any: `Skipped N Assets (-v lists them)`, `Failed N Assets or Packages`
(the first 20 failures are listed on stderr; `-v` lists all), and
`Cancelled before finishing`.

??? example "JSON"
    ```json
    {
      "output_dir": "out2",
      "packages": 1,
      "bytes_written": 117484,
      "written": [
        { "path": "out2/char/char_bossk_pre/bossk_tex.png", "package": "char_bossk_pre",
          "asset": "bossk_tex", "bytes": 117484 }
      ],
      "skipped": [],
      "errors": [],
      "ignored": 0,
      "packages_done": 1,
      "cancelled": false
    }
    ```

    - `written[].asset` is the asset's name, `{atlas}/{sprite}` for a
      sprite, or a sound's Wwise id. A `note` explains anything unusual,
      such as a sound kept as raw WEM.
    - `skipped` lists assets deliberately not written, with a reason
      (the file exists and `--skip-existing` is on, or AE3 can't export
      that asset).
    - `errors` lists failures with `package`, `asset` (when the failure was
      one asset's) and `error`.
    - `ignored` counts scene objects (Transforms, GameObjects and the like)
      and, with `--structured none`, data objects.

## diff

Compare two asset versions of the current platform.

```text
ae3 diff <OLD> [NEW] [--category CATEGORY] [--summary] [--extract added|changed|all] [export options]
```

`NEW` defaults to `--asset-version`, or else the latest.

| Option | Description |
|---|---|
| `--category CATEGORY` | Compare only this category; every count, size and the engine-rebuild flag describe it alone |
| `--summary` | Print only the counts, not the package lists |
| `--extract added\|changed\|all` | Also export those packages from the newer version (`all` = added and changed) |

Takes every [extract](#extract) export option.

```bash
ae3 diff 100054 --summary
```

```text
Diff 100054 -> 100056 (Windows)
  added        50 Packages     9.9 MB
  changed      50 Packages   119.2 MB
  removed       0 Packages
  42 of 11454 Bundles in both changed (0.4%)
```

When 80% or more of the shared bundles changed, the output says
`Engine Rebuild` and JSON has `"engine_rebuild": true`. The JSON shape is in
[Track new assets](../guides/track-new-assets.md#diff-output-as-json); with
`--extract`, it also includes the export report.

## cache

Inspect, prune or clear the download cache.

```text
ae3 cache [stats | prune [--max SIZE] | clear [--index]]
```

| Action | Description |
|---|---|
| `stats` (default) | Show where the cache is and what it holds |
| `prune [--max SIZE]` | Remove least recently used packages until the cache is at most `SIZE` (default: the cache cap, 5 GB) |
| `clear [--index]` | Remove every cached package and manifest; `--index` also removes the asset index files |

Sizes take `KB`/`MB`/`GB`/`TB` (powers of 1000) or `KiB`/`MiB`/`GiB`/`TiB`
(powers of 1024): `--max 2GB`, `--max 500MiB`.

```bash
ae3 cache
```

```text
Cache           /Users/you/Library/Caches/ae3
Packages        151 (46.3 MB of 5.0 GB cap)
  Windows       151 (46.3 MB)
Manifests       2 (1.5 MB)
Asset Indexes   1 (5.0 MB)
```

??? example "JSON"
    ```json
    {
      "root": "/Users/you/Library/Caches/ae3",
      "packages": 157, "package_bytes": 47420690, "cap_bytes": 5000000000,
      "by_platform": [ { "platform": "Windows", "packages": 157, "bytes": 47420690 } ],
      "manifests": 2, "manifest_bytes": 1525221,
      "index_files": 1, "index_bytes": 4956389
    }
    ```

## serve

Run the [HTTP API](http-api.md). Only in builds made with
`--features serve`.

```text
ae3 serve [--bind ADDRESS] [--threads N]
```

| Option | Default | Description |
|---|---|---|
| `--bind ADDRESS` | `127.0.0.1:8080` | Address and port to listen on |
| `--threads N` | `4` | Requests handled at once (1–64) |

The global `--platform` and `--asset-version` set the defaults for requests
that don't name them. `-v` logs each request with its status and time.
++ctrl+c++ stops the server.
