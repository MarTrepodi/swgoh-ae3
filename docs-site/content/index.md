# swgoh-ae3

**swgoh-ae3** (AE3) finds, previews and exports the game assets that
*Star Wars: Galaxy of Heroes* (SWGOH) publishes: unit portraits, ability icons,
gear and material icons, sprite atlases, 3D models, sounds and the game's
data objects.

It comes in three forms. Use whichever suits the job:

| Interface | Program | Best for |
|---|---|---|
| **Command line** | `ae3` (`ae3.exe` on Windows) | Scripts, batch exports, CI jobs, anything repeatable |
| **Desktop GUI** | `ae3-gui` (`swgoh-ae3.app` on macOS) | Browsing, previewing and hand-picking assets |
| **HTTP API** | `ae3 serve` | Other programs and websites that need asset lists or images on demand; drop-in for swgoh-ae2's API |

All three read the same [configuration file](reference/configuration.md) and
share one download cache, so a bundle downloaded by one is reused by the
others.

## What do you want to do?

<div class="grid cards" markdown>

-   :material-account-search: **Find the image for something in the game**

    ---

    Start from a name players know ("Bossk", "Zeta", "Carbonite Circuit
    Board") and get the bundles that hold its pictures.

    [:octicons-arrow-right-24: Identify assets from game data](guides/identify-assets.md)

-   :material-folder-search: **See what is inside a bundle**

    ---

    List bundles by category, search asset names and get the path id of a
    single texture, mesh or sprite.

    [:octicons-arrow-right-24: Locate assets in bundles](guides/locate-assets.md)

-   :material-export: **Export files**

    ---

    Whole bundles, whole categories, or one asset at a time, as PNG/WebP,
    OBJ/glTF, Ogg/WAV or JSON.

    [:octicons-arrow-right-24: Extract assets](guides/extract-assets.md)

-   :material-account-box-multiple: **Get every character portrait**

    ---

    One unit's thumbnail, or all of them, from each interface.

    [:octicons-arrow-right-24: Unit thumbnails](guides/unit-thumbnails.md)

-   :material-diamond-stone: **Get upgrade material icons**

    ---

    Ability materials (Mk I–III, Omega, Zeta, Omicron), relic materials
    (Signal Data, Carbonite Circuit Boards…) and gear.

    [:octicons-arrow-right-24: Ability and relic materials](guides/upgrade-materials.md)

-   :material-source-branch: **See what a game update added**

    ---

    Compare two asset versions and export only what is new or changed.

    [:octicons-arrow-right-24: Track new assets](guides/track-new-assets.md)

</div>

## Sixty-second tour

```bash
ae3 search bossk                     # (1)!
ae3 extract charui_bossk             # (2)!
ae3 extract skill_zeta scavenger_carbonitecicuitboard   # (3)!
```

1.  Lists the **game entities** called "Bossk" (the unit, its player
    portrait, an ability, a gear piece) with the bundles each one uses, then
    every bundle whose name or contents match, with each asset's path id.
2.  Writes `ae3-output/charui/charui_bossk/tex.charui_bossk.png`: Bossk's
    128×128 character portrait.
3.  Writes the Zeta ability material icon and the Carbonite Circuit Board
    relic material icon.

New here? Read [Install](getting-started/install.md), then the
[Quick start](getting-started/quick-start.md). The [Concepts](concepts.md)
page explains the words this documentation uses, such as *bundle*,
*category*, *path id* and *asset version*.

!!! note "Unofficial tool"
    *Star Wars: Galaxy of Heroes* and its assets are the property of their
    respective owners. AE3 is a community tool, not affiliated with or
    endorsed by Electronic Arts, Capital Games or Lucasfilm. Use extracted
    assets in line with the game's terms and applicable law.
