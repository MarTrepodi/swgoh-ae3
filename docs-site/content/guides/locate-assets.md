# Locate assets in bundles

Once you know a bundle's name, or roughly what you're looking for, the
next step is finding the exact asset inside it: its name, kind and
[path id](../concepts.md#path-id). With the path id you can export that one
asset instead of the whole bundle.

## Browse categories and bundles

=== "CLI"

    List the categories with their bundle counts and sizes:

    ```bash
    ae3 list --categories
    ```

    ```text
    ability                               2617 bundles     0 audio    88.7 MB
    abilityui                              367 bundles     0 audio     2.2 MB
    ...
    charui                                 775 bundles     0 audio     6.5 MB
    ...
    scavenger                               26 bundles     0 audio   573.4 KB
    ...
    skill                                    9 bundles     0 audio   101.4 KB
    ```

    List the bundles in one category:

    ```bash
    ae3 list --category skill
    ```

    ```text
    skill_abilitymaterial_ultimate  bundle    19.2 KB
    skill_hexagon_white             bundle    33.0 KB
    skill_omicron                   bundle     7.8 KB
    skill_pentagon_gold             bundle     5.9 KB
    skill_pentagon_white            bundle     5.3 KB
    skill_square_white              bundle     5.8 KB
    skill_triangle_white            bundle     5.1 KB
    skill_zeta                      bundle     6.9 KB
    skill_zeta_glow                 bundle    12.4 KB
    ```

    `--kind bundle` or `--kind audio` narrows to one kind of package.

=== "GUI"

    After **Fetch latest**, the table lists every package with its size.
    Click **Name**, **Types** or **Size** to sort. The **Asset types** chips
    above the table (Texture, Atlas, Sprite, Text, Structured, Mesh, Audio)
    show only packages holding that kind of asset; press **All types** to
    clear them. Type filters need the [asset index](../concepts.md#asset-index),
    which builds in the background; its progress is in the sidebar.

    Typing a category name such as `skill_` in the search box narrows the
    table to that category's bundles.

=== "HTTP API"

    `/Asset/list` returns every bundle name as a JSON array. Filter it on
    the client:

    ```bash
    curl -s http://127.0.0.1:8080/Asset/list | jq -r '.[] | select(startswith("skill_"))'
    ```

    The API has no category listing of its own; a category is the part of
    the name before the first `_`.

## Search asset names and get path ids

=== "CLI"

    Without `--names-only`, `ae3 search` also looks *inside* bundles, through
    the asset index, and lists each match with its kind and path id:

    ```bash
    ae3 search bossk
    ```

    ```text
    Game Entities
      Unit             Bossk: charui_bossk, char_bossk_pre, relic_rfl_bossk
      ...

    Bundles
    char_bossk_pre
        mesh       Bossk_weapon  (path id 1474002511046246879)
        texture    bossk_mask  (path id 6676416519307852629)
        structured bossk_mat  (path id 5485236150910287353)
        mesh       bossk_mesh  (path id 780028565564020081)
        texture    bossk_tex  (path id 719922364503921676)
        ...
        texture    rfl_bossk_tex  (path id -3565641646980641963)
    charui_bossk
        texture    tex.charui_bossk  (path id -6516795281392349143)
    relic_rfl_bossk
        texture    tex.relic_rfl_bossk  (path id -7883354041697663554)
    vanity_bossk
        texture    tex.vanity_bossk  (path id -4686596141441493556)
    ```

    A bundle listed with no assets under it matched by its name only.

    The search matches asset names anywhere in the game, so it also finds
    assets shared into other bundles. For example, Bossk's attack animation
    `hmn_rfl_bossk_atk` is also in `char_guildbountyhunter_01_pre`.

    **Search one bundle's contents.** Search for the bundle's exact name
    and every asset whose name contains it is listed. To see *all* of a
    bundle's assets regardless of their names, use the GUI preview, or
    export the bundle and look at what was written.

    !!! note "The first search builds the asset index"
        The first search on a platform reads the start of every bundle,
        which takes a minute or two and about as many requests as there are
        bundles. Later searches only re-read bundles that changed. Run
        `ae3 index --plan` to see what an update would read, and
        `ae3 index` to update on its own, for example on a schedule.

=== "GUI"

    1. Type in the search box. Bundles whose names, display names or asset
       names match stay in the table.
    2. Click a bundle. The preview on the right shows every exportable
       asset as a thumbnail with its name, kind and size.
    3. Double-click a thumbnail to open it in the **detail viewer**: zoom,
       per-channel (R, G, B, A) views, the asset's class and **path id**.

    ![Bundle preview grid](../assets/gui/bundle_preview_grid.png)

    The **Scene objects** checkbox in the preview also lists GameObjects,
    Transforms, MonoScripts and the like, which are hidden by default
    because they aren't useful as files.

    For a sprite atlas, the detail viewer outlines every sprite and lists
    them by name and size:

    ![Atlas detail viewer](../assets/gui/atlas_overlay.png)

=== "HTTP API"

    The HTTP API cannot list the assets inside a bundle. Use the CLI's
    `ae3 search --json` and read the `hits` array (see below), or the GUI.

## Search results as JSON

```bash
ae3 search skill_zeta --json
```

```json
{
  "asset_version": 100056,
  "platform": "Windows",
  "names_only": false,
  "name_index": { "status": "current", "language": "ENG_US", "...": "..." },
  "query": "skill_zeta",
  "entities": [],
  "hits": [
    {
      "package": "skill_zeta",
      "kind": "bundle",
      "category": "skill",
      "name_matched": true,
      "indexed": true,
      "kinds": ["texture"],
      "assets": [
        { "path_id": -1892276161897214758, "kind": "texture", "name": "tex.skill_zeta" }
      ]
    },
    {
      "package": "skill_zeta_glow",
      "...": "..."
    }
  ],
  "coverage": { "indexed_bundles": 11501, "failed_bundles": 0, "total_bundles": 11501, "...": "..." }
}
```

| Field | Meaning |
|---|---|
| `hits[].package` | The bundle (or audio package) name |
| `hits[].name_matched` | The package's own name matched the search |
| `hits[].indexed` | The package is in the asset index; `false` means its assets couldn't be searched |
| `hits[].kinds` | Every kind of asset the bundle holds |
| `hits[].assets[]` | The assets whose names matched, with `path_id`, `kind` and `name` |
| `coverage` | How much of the manifest the asset index covers |

!!! warning "Path ids are 64-bit integers"
    Path ids can exceed the range JavaScript numbers hold exactly
    (±2^53). In JavaScript, parse the JSON with a big-integer-aware parser,
    or read path ids from the CLI's text output. `jq` 1.7+ and Python keep
    them exact.

Every texture and the bundle it is in, one per line:

=== "bash"

    ```bash
    ae3 search bossk --json \
      | jq -r '.hits[] | .package as $b | .assets[] | select(.kind == "texture") | "\($b)\t\(.path_id)\t\(.name)"'
    ```

=== "PowerShell"

    ```powershell
    $r = ae3 search bossk --json | ConvertFrom-Json
    foreach ($hit in $r.hits) {
      foreach ($a in $hit.assets | Where-Object kind -eq 'texture') {
        "{0}`t{1}`t{2}" -f $hit.package, $a.path_id, $a.name
      }
    }
    ```

## Options that change what search does

| Option | Effect |
|---|---|
| `--names-only` | Match bundle names and display names only; don't build or read the asset index |
| `--no-update` | Use the asset index and name index as stored, without checking for new bundles or game data (fast, offline) |
| `--no-display-names` | Leave game entities out |
| `--language LANG` | Display-name language for this search |

`--names-only` and `--no-update` can't be combined.

## Next

[Extract](extract-assets.md) what you found, as whole bundles or by path id.
