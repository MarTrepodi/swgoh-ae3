# Unit thumbnails

Every character and ship has a square portrait (the "thumbnail" shown in
the roster) in its own bundle in the `charui` category: Bossk's is
`charui_bossk`, holding one 128×128 texture, `tex.charui_bossk`.

## One unit

=== "CLI"

    Find the bundle from the unit's name, then extract it:

    ```bash
    ae3 search "jedi master kenobi" --names-only
    ```

    ```text
    Game Entities
      Unit             Jedi Master Kenobi: charui_globiwan, char_obiwan_jedimaster_pre, relic_item_globiwan
      Player Portrait  Jedi Master Kenobi: vanity_globiwan
      ...
    ```

    ```bash
    ae3 extract charui_globiwan
    ```

    The portrait is `ae3-output/charui/charui_globiwan/tex.charui_globiwan.png`.

=== "GUI"

    1. **Fetch latest**, then search for the unit's name.
    2. Click its **Unit** chip; the table shows only its bundles.
    3. Click the `charui_…` row. The preview shows the portrait.
    4. Press **Extract selected (1)**.

=== "HTTP API"

    ```bash
    curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=charui_globiwan"
    ```

    Saves `tex.charui_globiwan.png`.

!!! holocron "Don't guess bundle names"
    The bundle is named after the game's internal id, which often differs
    from the display name: Jedi Master Kenobi is `charui_globiwan`, Echo is
    `charui_bb_echo`, The Mandalorian and Grogu is `charui_glmandogrogu`.
    Use the display-name search to look them up.

## The other pictures of a unit

A unit's game entity links to more than its portrait. In the order AE3
lists them:

| Bundle | What it holds | Size |
|---|---|---|
| `charui_<id>` | Roster portrait | 128×128 |
| `char_<id>_pre` / `ship_<id>_pre` | 3D model, its textures and animations | — |
| `relic_<…>` | The unit's relic item image | 256×256 |
| `charui_large_<id>` | Large portrait, for about 90 units | 256×256 |
| `charui_<id>_turnorder` | Turn-order portrait (ships) | 256×256 |
| `vanity_<id>` | The matching player portrait (a separate game entity) | 128×128 |

To get every picture of one unit at once, pass all its bundles. In bash,
straight from the search:

```bash
ae3 extract $(ae3 search "the mandalorian and grogu" --names-only --json \
  | jq -r '.entities[] | select(.kind == "unit") | .bundles[] | select(endswith("_pre") | not)')
```

(The `select` leaves out the 3D model bundle, which is much larger.)

## Every unit

=== "CLI"

    Extract the whole `charui` category: about 775 bundles, 6.5 MB of
    downloads.

    ```bash
    ae3 extract --category charui -o portraits
    ```

    With the default layout each portrait is in its own folder. To get them
    all in one folder, use swgoh-ae2's layout:

    ```bash
    ae3 extract --category charui --layout ae2 -o portraits
    ```

    ```text
    portraits/charui/tex.charui_4lom.png
    portraits/charui/tex.charui_50rt.png
    portraits/charui/tex.charui_aayla.png
    ...
    ```

    The category also holds a few portraits that are not playable units
    (event tiles, placeholders). Use the name list below if you need only
    units.

=== "GUI"

    1. **Fetch latest**.
    2. Type `charui_` in the search box.
    3. With no row selected, press **Extract all (N)**.

    To get them all in one folder, choose **Edit settings → Layout →
    ae2** first.

=== "HTTP API"

    List the bundles, keep the `charui_` ones and fetch each:

    === "bash"

        ```bash
        mkdir -p portraits && cd portraits
        curl -s http://127.0.0.1:8080/Asset/list \
          | jq -r '.[] | select(startswith("charui_"))' \
          | while read -r name; do
              curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=$name"
            done
        ```

    === "PowerShell"

        ```powershell
        New-Item -ItemType Directory -Force portraits | Out-Null
        $names = Invoke-RestMethod http://127.0.0.1:8080/Asset/list |
          Where-Object { $_ -like 'charui_*' }
        foreach ($name in $names) {
          Invoke-WebRequest "http://127.0.0.1:8080/Asset/single?assetName=$name" `
            -OutFile "portraits/tex.$name.png"
        }
        ```

    The server handles four requests at once by default (`ae3 serve
    --threads N` changes it) and keeps recent PNGs in memory, so repeat
    requests are instant.

## Name files after units

Bundle names are internal ids. To save each portrait as
`<Display name>.png`, map units to their first bundle, which is the
portrait. The CLI's search needs a query, so the simplest complete source
of every unit is the **name index file** AE3 keeps in its cache:

| OS | Default location |
|---|---|
| Windows | `%LOCALAPPDATA%\ae3\names\ENG_US.json` |
| macOS | `~/Library/Caches/ae3/names/ENG_US.json` |
| Linux | `~/.cache/ae3/names/ENG_US.json` |

`ae3 cache` prints the cache folder if you've moved it. The file exists
after any display-name search, and is rebuilt when the game data changes.

```json
{
  "format": 1,
  "language": "ENG_US",
  "versions": { "gamedata": "…", "localization": "…" },
  "entities": [
    { "kind": "unit", "id": "BADBATCHECHO", "name": "Echo",
      "bundles": ["charui_bb_echo", "char_bb_echo_pre", "relic_item_bbecho"] },
    …
  ]
}
```

!!! warning
    This file is AE3's cache, not a published interface; its `format`
    number changes if its layout does. Check `format == 1` in scripts.

This Python script (standard library only) exports every unit's portrait
and copies it to `units/<Display name>.png`:

```python title="unit_portraits.py"
"""Export every unit's portrait, named after the unit."""

import json
import re
import shutil
import subprocess
import sys
from pathlib import Path


def name_index_path() -> Path:
    stats = subprocess.run(
        ["ae3", "cache", "--json"], check=True, capture_output=True, text=True
    )
    return Path(json.loads(stats.stdout)["root"]) / "names" / "ENG_US.json"


def unit_portraits(index_file: Path) -> dict[str, str]:
    """Map each unit's portrait bundle to its display name."""
    index = json.loads(index_file.read_text(encoding="utf-8"))
    if index["format"] != 1:
        sys.exit(f"unknown name index format {index['format']}")
    return {
        e["bundles"][0]: e["name"]
        for e in index["entities"]
        if e["kind"] == "unit" and e["bundles"] and e["bundles"][0].startswith("charui_")
    }


def safe(name: str) -> str:
    return re.sub(r'[<>:"/\\|?*]', "_", name).strip()


def main() -> None:
    # A display-name search makes sure the name index exists and is current.
    subprocess.run(["ae3", "search", "bossk", "--names-only", "-q"], check=True,
                   stdout=subprocess.DEVNULL)
    portraits = unit_portraits(name_index_path())
    result = subprocess.run(
        ["ae3", "extract", *portraits, "--json", "-o", "ae3-output"],
        capture_output=True, text=True,
    )
    if result.returncode not in (0, 3):  # 3: some packages failed
        sys.exit(result.stderr)
    out = Path("units")
    out.mkdir(exist_ok=True)
    for written in json.loads(result.stdout)["written"]:
        name = portraits[written["package"]]
        shutil.copyfile(written["path"], out / f"{safe(name)}.png")
    print(f"{len(portraits)} unit portraits in {out}/")


if __name__ == "__main__":
    main()
```

```bash
python3 unit_portraits.py
```

If two units ever share a display name, the later one overwrites the
earlier; add `e["id"]` to the file name to keep both.
