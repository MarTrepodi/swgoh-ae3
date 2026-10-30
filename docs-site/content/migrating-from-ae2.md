# Migrating from swgoh-ae2

AE3 replaces swgoh-ae2, swgoh-asset-extractor and Unity Bundle Extractor.
It needs no .NET runtime, and its HTTP API answers swgoh-ae2's routes.

## Command line

| swgoh-ae2 | AE3 |
|---|---|
| `-downloadManifest` | `ae3 manifest -o DIR` |
| `-getAssetNames` | `ae3 list --json` |
| `-getAssetPrefixes` | `ae3 list --categories --json` |
| `-single <name>` | `ae3 extract <bundle>` |
| `-prefix <prefix>` | `ae3 extract --category <prefix>` |
| `-all` | `ae3 extract --all` |
| `-audio` | `ae3 extract --audio` |
| `-dd <old> new\|changed\|all` | `ae3 diff <old> <current> --extract added\|changed\|all` |
| `-dm <old.data> <new.data>` | `ae3 diff <old> <new>` (by version number) |
| `-v <version>` | `--asset-version <version>` |
| `-t <folder>` | `--output <folder>` |
| `-w <folder>` | Not needed: downloads go to a managed [cache](reference/configuration.md#cache) |

swgoh-ae2's **prefix** is AE3's **category**.

## Folder layout

AE3 writes to `{output}/{category}/{bundle}/` by default. For swgoh-ae2's
`{output}/{prefix}/` layout, pass `--layout ae2`, or set it once:

```toml
layout = "ae2"
```

Meshes are written as OBJ oriented the way swgoh-ae2 wrote them.

## HTTP API

Start `ae3 serve` where you ran swgoh-ae2's server. The routes, parameters
and `assetOS` numbers are the same:

| Route | Parameters |
|---|---|
| `/Asset/list` | `version`, `assetOS` |
| `/Asset/listDiff` | `version`, `diffVersion`, `diffType`, `prefix`, `assetOS` |
| `/Asset/single` | `assetName`, `version`, `forceReDownload`, `assetOS` |

What to check in your client:

- **Errors** now come back as JSON `{"error": "…"}` with a 400, 404, 500 or
  502 status, instead of an empty body. Check the status code.
- **`listDiff`** now honours `assetOS` and the real environment; swgoh-ae2
  always diffed Android on PROD.
- **`assetOS`** also accepts `windows`, `android` and `ios`.
- **The default version** is resolved when the server starts, not per
  request.

Details: [HTTP API reference](reference/http-api.md).

## What's new

Things swgoh-ae2 couldn't do:

- [Search by display name](guides/identify-assets.md) ("Bossk", "Zeta")
  and by asset name across every bundle.
- Export [one asset by path id](guides/extract-assets.md#extract-one-asset-by-path-id).
- Preview everything in the [desktop GUI](reference/gui.md).
- glTF export with textures, WebP images, and combined structured JSON.
- A download cache shared between runs and versions.
