# Output files

What an extraction writes, and where.

## Folder layout

=== "ae3 layout (default)"

    One folder per bundle, inside a folder per category:

    ```text
    {output}/{category}/{bundle}/{asset}.{ext}
    {output}/{category}/{bundle}/{bundle}.structured.json
    {output}/{category}/{bundle}/sprites/{atlas}/{sprite}.png
    {output}/{category}/{bundle}/sprites/{atlas}.json
    {output}/{category}/{audio package}/{wwise id}.ogg
    ```

    ```text
    ae3-output/
    ├── charui/
    │   └── charui_bossk/
    │       └── tex.charui_bossk.png
    └── char/
        └── char_bossk_pre/
            ├── bossk_mesh.obj
            ├── bossk_tex.png
            └── char_bossk_pre.structured.json
    ```

=== "ae2 layout"

    swgoh-ae2's layout: every bundle of a category shares one folder.

    ```text
    {output}/{category}/{asset}.{ext}
    {output}/{category}/sprites/{atlas}/{sprite}.png
    {output}/{category}/sprites/{atlas}.json
    {output}/{category}/{audio package}/{wwise id}.ogg
    ```

    ```text
    ae3-output/
    └── charui/
        ├── tex.charui_4lom.png
        ├── tex.charui_50rt.png
        └── tex.charui_bossk.png
    ```

    When two bundles of a category hold assets with the same name, the
    first keeps the plain name and the others get a `_{path id}` suffix,
    such as `standard_atlas_ref_-3984845052272470908.json`.

Audio packages always get their own folder.

## File names

- An asset's file is named after the asset (`tex.charui_bossk.png`,
  `bossk_tex.png`), not the bundle.
- Names are made safe on every OS: path separators, control characters and
  `<>:"|?*` become `_`, trailing dots and spaces are trimmed, Windows
  device names (`CON`, `NUL`, …) get a leading `_`, and names are cut to
  160 bytes.
- When two assets in one bundle would get the same file name (compared
  without case), the one that sorts first by kind and path id keeps the
  plain name and the others get `_{path id}`. Names are therefore stable
  from run to run.
- Files appear atomically: each is written under a temporary name and then
  renamed, so a reader never sees half a file.

## By asset kind

### Textures

PNG, or lossless WebP with `--image-format webp`. Every texture encoding
the game uses on each platform (ETC, ASTC, crunched and uncompressed
formats) is decoded to the same RGBA image.

### Sprite atlases

With atlas cutting (the default), each sprite becomes
`sprites/{atlas}/{sprite}.png` and the atlas is described by
`sprites/{atlas}.json`:

```json
{
  "name": "standard_atlas",
  "asset": { "package": "shared_uicontainer", "file": "CAB-5c7f…", "path_id": -3876598707914989183 },
  "pixel_size": 1.0,
  "texture": {
    "asset": { "package": "shared_uicontainer", "file": "CAB-5c7f…", "path_id": -8340471856654006876 },
    "name": "standard_atlas",
    "width": 2048, "height": 2048,
    "format_id": 4, "format": "RGBA32"
  },
  "padding_restored": true,
  "sprites": [
    {
      "name": "add_crew",
      "x": 1670, "y": 1046, "width": 128, "height": 128,
      "border_left": 0, "border_right": 0, "border_top": 0, "border_bottom": 0,
      "padding_left": 0, "padding_right": 0, "padding_top": 0, "padding_bottom": 0,
      "file": "standard_atlas/add_crew.png"
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `sprites[].x`, `y`, `width`, `height` | The sprite's rectangle in the atlas texture |
| `sprites[].border_*` | Nine-slice borders, for sprites that stretch |
| `sprites[].padding_*` | Transparent margin NGUI trimmed; restored in the file when `padding_restored` is true |
| `sprites[].file` | The sprite's image, relative to this JSON file |
| `replacement` | Present when the atlas is a reference to another atlas |

Many bundles carry a reference copy of an atlas whose texture lives in a
shared bundle (`shared_uicontainer` holds the game's `standard_atlas`, 435
sprites). AE3 cuts the sprites once, into the shared bundle's folder; the
referring bundle's JSON points there.

With `--no-atlas-cutting`, the atlas is written as JSON only.

### Meshes

| `--mesh-format` | Files |
|---|---|
| `obj` (default) | `{mesh}.obj`: positions, normals, UVs and one group per submesh, mirrored on X as swgoh-ae2 wrote them |
| `glb` | `{mesh}.glb`: binary glTF with the same geometry in glTF's coordinate space, and each submesh's main texture embedded as PNG |
| `both` | Both, side by side |

AE3 finds a mesh's texture through the renderer in the same bundle that
draws it. When none does (shared resource bundles), or its material has no
texture, the `.glb` holds geometry only and the export report has a `note`
saying why.

### Audio

| `--audio-format` | Files |
|---|---|
| `ogg` (default) | `{wwise id}.ogg` for Wwise Vorbis; `{wwise id}.wav` for PCM and PTADPCM |
| `raw` | `{wwise id}.wem`, the original Wwise data |

A sound that can't be converted is written as raw `.wem`, with a `note` in
the export report.

### Structured data

"Structured" assets are the game's data objects: materials, animation
clips and controllers, MonoBehaviours and so on.

| `--structured` | Files |
|---|---|
| `combined` (default) | One `{bundle}.structured.json` per bundle |
| `separate` | One `{asset}.json` per asset |
| `none` | Nothing |

The combined file is a JSON object with one member per asset, keyed by the
file name the asset would otherwise get:

```json
{
  "hmn_rfl_bossk_idle": {
    "path_id": -8272699188967797282,
    "name": "hmn_rfl_bossk_idle",
    "class_name": "AnimationClip",
    "data": { "m_Name": "hmn_rfl_bossk_idle", "m_Legacy": false, "…": "…" }
  },
  "UnitComponent_-3399267987560439297": { "…": "…" }
}
```

Each member has `path_id`, `name`, `class_name`, and, when known,
`script_class` and `container_path`, plus `data`. Assets picked with
`--path-id` are always written as their own files.

### Text

Text assets keep their original bytes, with an extension guessed from the
content: `.json`, `.xml`, `.yaml` or `.txt`, and `.bytes` for binary data.

### Scene objects

GameObjects, Transforms, MonoScripts and similar plumbing are counted
(`ignored` in the JSON report) but not written, unless picked by
`--path-id`, when they're written as JSON.
