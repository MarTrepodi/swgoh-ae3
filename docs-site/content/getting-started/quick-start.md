# Quick start

This page takes you from nothing to a saved PNG of Bossk's character
portrait, in each interface. Pick a tab; the choice carries over to every
other tabbed example on the site.

## 1. Find what you want

You know the name players see ("Bossk"). You need the name of the **bundle**
that holds the picture (`charui_bossk`).

=== "CLI"

    ```bash
    ae3 search bossk --names-only
    ```

    ```text
    Game Entities
      Unit             Bossk: charui_bossk, char_bossk_pre, relic_rfl_bossk
      Player Portrait  Bossk: vanity_bossk
      Ability          Bossk's Payout (Bossk): abilityui_passive_contract
      Gear             Power Cell Injector (Fusion) - Bossk: equip_powercellinjector

    Bundles
    ability_bossk_basic
    ability_bossk_special01
    ...
    charui_bossk
    ...
    ```

    The **Game Entities** section lists the things in the game named
    "Bossk", each with the bundles it uses, most specific first. For a unit
    the first bundle is its character portrait, `charui_bossk`.

    The first search downloads the community game data (about 25 MB, once
    per game data update) to learn the display names; later searches take a
    fraction of a second. `--names-only` skips building the
    [asset index](../concepts.md#asset-index), which you don't need yet.

=== "GUI"

    1. Start the app (`ae3-gui`, or **swgoh-ae3** from the Start menu or
       Applications).
    2. In **Game data** on the left, leave **Asset Version** empty and press
       **Fetch latest**. The bundle table fills in.
    3. Type `bossk` in the search box above the table.

    ![The GUI after searching: Game Entities chips above the bundle table](../assets/gui/search_game_entities.png)

    *Screenshot from AE3's test data; with real game data the chips read
    "Bossk · 3", "Bossk · 1" and so on.*

    The **Game Entities** chips are the things named "Bossk". Click the
    **Unit** chip (the person icon) and the table shows only that unit's
    bundles: `charui_bossk`, `char_bossk_pre` and `relic_rfl_bossk`.

=== "HTTP API"

    Start the server (see [Install](install.md#get-the-http-api) for how to
    get a build that has it):

    ```bash
    ae3 serve
    ```

    ```text
    Listening on http://127.0.0.1:8080 (default Asset Version 100056, Windows)
    ```

    The HTTP API speaks swgoh-ae2's protocol, which knows bundle names but
    not display names. Get the full bundle list and filter it:

    ```bash
    curl -s http://127.0.0.1:8080/Asset/list | jq -r '.[] | select(contains("bossk"))'
    ```

    ```text
    ability_bossk_basic
    ability_bossk_special01
    ability_bossk_special02
    char_bossk_pre
    charui_bossk
    ...
    ```

    To go from a display name to a bundle, use `ae3 search` (CLI) once,
    then call the API with the bundle name.

## 2. Export it

=== "CLI"

    ```bash
    ae3 extract charui_bossk
    ```

    ```text
    Exporting 1 Package to ae3-output
    Wrote 1 file (23.2 KB) from 1 of 1 Package to ae3-output
    ```

    The file is `ae3-output/charui/charui_bossk/tex.charui_bossk.png`, a
    128×128 PNG. Use `--output` (or `-o`) to write somewhere else:

    ```bash
    ae3 extract charui_bossk -o ./portraits
    ```

=== "GUI"

    1. Click `charui_bossk` in the table. Its assets appear in the preview
       on the right.
    2. Press **Extract selected (1)** in the bar at the bottom.
    3. When it finishes, press **Reveal in folder**.

    By default files go to `ae3-output` (shown at the bottom left as
    **Output: ae3-output**). Change it with **Edit settings → Output
    folder**.

=== "HTTP API"

    ```bash
    curl -s -o tex.charui_bossk.png "http://127.0.0.1:8080/Asset/single?assetName=charui_bossk"
    ```

    `/Asset/single` returns the bundle's main texture as `image/png`. Nothing
    is written on the server; the PNG comes back in the response.

## 3. Where next

- Get [every unit's portrait](../guides/unit-thumbnails.md) at once.
- Get [ability and relic material icons](../guides/upgrade-materials.md).
- Learn what [path ids](../concepts.md#path-id) are, to export one asset out
  of a large bundle: [Locate assets in bundles](../guides/locate-assets.md).
