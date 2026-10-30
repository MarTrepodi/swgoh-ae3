# Ability and relic materials

Upgrade materials (the items spent on ability upgrades, relic tiers, gear,
datacrons and mods) each have a small icon in its own bundle. Search finds
them by the name shown in the game, as `Material` or `Gear` game entities:

```bash
ae3 search "signal data" --names-only
```

```text
Game Entities
  Material  Flawed Signal Data: scavenger_data03
  Material  Corrupted Signal Data: scavenger_data04
  Material  Fragmented Signal Data: scavenger_data01
  Material  Incomplete Signal Data: scavenger_data02
  Material  Corrupted Signal Data Surplus: scavenger_data04
```

The tables below were produced the same way, from asset version 100056 and
the game data of the same date. Names and bundles can change in later game
updates; the search always reflects the current ones.

## Ability materials

Spent on character ability upgrades. All are in the `skill` category, one
texture per bundle: 128×128, except Ultimate (256×256) and Omicron
(512×512).

| Material | Gamedata id | Bundle |
|---|---|---|
| Ability Material Mk I | `ability_mat_A` | `skill_triangle_white` |
| Ability Material Mk II | `ability_mat_B` | `skill_square_white` |
| Ability Material Mk III | `ability_mat_C` | `skill_pentagon_white` |
| Ability Material Omega | `ability_mat_D` | `skill_pentagon_gold` |
| Ability Material Zeta | `ability_mat_E` | `skill_zeta` |
| Ability Material Omicron | `ability_mat_F` | `skill_hexagon_white` |
| Ability Material Ultimate | `ability_mat_ULTIMATE` | `skill_abilitymaterial_ultimate` |

The category also has `skill_zeta_glow` (a glowing Zeta icon) and
`skill_omicron` (a second, 128×128 Omicron icon); no material links to
them.

**Ship ability materials** are in `shipskill`:

| Material | Gamedata id | Bundle |
|---|---|---|
| Ship Ability Material Mk I | `shipability_mat_A` | `shipskill_triangle` |
| Ship Ability Material Mk II | `shipability_mat_B` | `shipskill_square` |
| Ship Ability Material Mk III | `shipability_mat_C` | `shipskill_pentagon` |
| Ship Ability Material Omega | `shipability_mat_D` | `shipskill_star` |

## Relic materials

Spent on relic tiers. All are in the `scavenger` category, 128×128.

| Material | Gamedata id | Bundle |
|---|---|---|
| Fragmented Signal Data | `RM_001` | `scavenger_data01` |
| Incomplete Signal Data | `RM_002` | `scavenger_data02` |
| Flawed Signal Data | `RM_003` | `scavenger_data03` |
| Corrupted Signal Data | `RM_004` | `scavenger_data04` |
| Carbonite Circuit Board | `SCV_001` | `scavenger_carbonitecicuitboard` |
| Bronzium Wiring | `SCV_002` | `scavenger_bronsiumwiring` |
| Chromium Transistor | `SCV_003` | `scavenger_chromiumtransisters` |
| Aurodium Heatsink | `SCV_004` | `scavenger_aurodiumheatsink` |
| Electrium Conductor | `SCV_005` | `scavenger_electriumconductors` |
| Zinbiddle Card | `SCV_006` | `scavenger_zinbiddlecard` |
| Impulse Detector | `SCV_007` | `scavenger_impulsedetector` |
| Aeromagnifier | `SCV_008` | `scavenger_aeromagnifier` |
| Gyrda Keypad | `SCV_009` | `scavenger_gyrdakeypad` |
| Droid Brain | `SCV_010` | `scavenger_droidbrain` |
| Coaxial Servomotor | `SCV_011` | `scavenger_servomotor` |

The "Surplus" variants (`SCV_001_SURPLUS` and so on) use the same icon as
the material. Note the game's own spellings in some bundle names
(`cicuitboard`, `bronsium`, `transisters`); search by display name rather
than typing them.

The `scavenger` category also holds `scavenger_data05` to `scavenger_data07`,
`scavenger_capacitor` and `scavenger_skeletonkey`, which no current material
links to, plus the Scavenger screen's UI bundles (`scavenger_view`,
`scavenger_home`, …).

!!! holocron "Relic materials vs. a unit's relic image"
    The `relic` category is something else: each unit's *relic image*, the
    signature item shown on its relic screen (Bossk's is `relic_rfl_bossk`,
    a rifle). Search finds it as the third bundle of the unit.

## Era materials

Spent on era level upgrades. In the `icon` category:

| Material | Bundle |
|---|---|
| Mk I Grek Ability Module | `icon_eralevels_mat_a_1` |
| Mk I Basic Performance Core | `icon_eralevels_mat_b_1` |
| Mk II Forn Ability Module | `icon_eralevels_mat_c_1` |
| Mk II Improved Performance Core | `icon_eralevels_mat_d_1` |
| Mk III Esk Ability Module | `icon_eralevels_mat_a_2` |
| Mk III Advanced Performance Core | `icon_eralevels_mat_b_2` |
| Mk IV Dorn Ability Module | `icon_eralevels_mat_c_2` |
| Mk IV Enhanced Performance Core | `icon_eralevels_mat_d_2` |
| Mk V Cresh Ability Module | `icon_eralevels_mat_a_3` |
| Mk V Augmented Performance Core | `icon_eralevels_mat_b_3` |
| Mk VI Besh Ability Module | `icon_eralevels_mat_c_3` |
| Mk VI Superior Performance Core | `icon_eralevels_mat_d_3` |
| Mk VII Aurek Ability Module | `icon_eralevels_mat_a_4` |
| Mk VII Masterwork Performance Core | `icon_eralevels_mat_b_4` |
| Mk VIII Senth Ability Module | `icon_eralevels_mat_c_4` |
| Mk VIII Legendary Performance Core | `icon_eralevels_mat_d_4` |
| Zerek Ability Module | `icon_eralevels_mat_special` |

## Other materials

| Kind | Example | Bundles |
|---|---|---|
| Datacron upgrade and reroll | Acumen Mk1, Alteration Mk1 | `icon_datacrons_upgrade_{a–d}_{1–3}`, `icon_datacrons_reroll_{a–d}_{1–3}` |
| Character training droids | T1–T7 Training Droid | `consumable_xpbot_star01` … `07` |
| Ship enhancement droids | T1–T7 Enhancement Droid | `consumable_shipxp_star01` … `07` |
| Credit droids | T-I DLC-13 Mining Droid | `consumable_cashbot_dlc13_01` … `05` |
| Mod slicing materials | Mk 1 Fusion Disk, Mk 2 Microprocessor | `equip_mods_*` |
| Fleet commander upgrades | Prestige, Reinforcement Materials | `fleetcommanderskill_chevron`, `shipupgrade_hardware` |

## Gear

Gear pieces are `Gear` game entities with icons in the `equip` category.
Every tier of a piece shares one icon; the game draws the tier colour
around it:

```bash
ae3 search "stun gun" --names-only
```

```text
Game Entities
  Gear  Mk 1 A/KT Stun Gun: equip_stungun
  Gear  Mk 2 A/KT Stun Gun: equip_stungun
  ...
  Gear  Mk 12 ArmaTek Stun Gun: equip_stungun
  Gear  Mk 4 A/KT Stun Gun Salvage: equip_stungun_salvage
  Gear  Mk 5 A/KT Stun Gun Prototype: equip_stungun_prototype
  Gear  Mk 12 ArmaTek Stun Gun Component: equip_stungun_component
  ...
```

Salvage, prototype and component pieces have their own icons
(`_salvage`, `_prototype`, `_component`).

## Get one material icon

=== "CLI"

    ```bash
    ae3 extract skill_zeta
    ```

    Writes `ae3-output/skill/skill_zeta/tex.skill_zeta.png` (128×128).

=== "GUI"

    1. **Fetch latest**, then type `zeta` in the search box.
    2. Click the **Ability Material Zeta** chip (the material icon). The
       table shows `skill_zeta`.
    3. Select it and press **Extract selected (1)**.

=== "HTTP API"

    ```bash
    curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=skill_zeta"
    ```

## Get every ability and relic material

=== "CLI"

    The three categories are small (under 1 MB together):

    ```bash
    ae3 extract --category skill
    ```

    ```bash
    ae3 extract --category shipskill
    ```

    ```bash
    ae3 extract --category scavenger
    ```

    `scavenger` includes its UI bundles; to export only the materials,
    name the bundles:

    === "bash"

        ```bash
        ae3 extract $(ae3 search "signal data" --names-only --json | jq -r '.entities[].bundles[]' | sort -u) \
          scavenger_carbonitecicuitboard scavenger_bronsiumwiring scavenger_chromiumtransisters \
          scavenger_aurodiumheatsink scavenger_electriumconductors scavenger_zinbiddlecard \
          scavenger_impulsedetector scavenger_aeromagnifier scavenger_gyrdakeypad \
          scavenger_droidbrain scavenger_servomotor
        ```

    === "PowerShell"

        ```powershell
        $relic = 'scavenger_data01','scavenger_data02','scavenger_data03','scavenger_data04',
          'scavenger_carbonitecicuitboard','scavenger_bronsiumwiring','scavenger_chromiumtransisters',
          'scavenger_aurodiumheatsink','scavenger_electriumconductors','scavenger_zinbiddlecard',
          'scavenger_impulsedetector','scavenger_aeromagnifier','scavenger_gyrdakeypad',
          'scavenger_droidbrain','scavenger_servomotor'
        ae3 extract @relic
        ```

    To get *every* material the game data knows, with display names, read
    the name index file as in
    [Name files after units](unit-thumbnails.md#name-files-after-units),
    filtering on `kind == "material"` instead of `"unit"`.

=== "GUI"

    1. Type `skill_` in the search box. The table shows the `skill`
       bundles.
    2. Press **Extract all (N)**.
    3. Repeat with `shipskill_` and `scavenger_`.

    Or search `ability material`, click each chip in turn and extract,
    to get exactly the materials.

=== "HTTP API"

    === "bash"

        ```bash
        for name in skill_triangle_white skill_square_white skill_pentagon_white \
                    skill_pentagon_gold skill_zeta skill_hexagon_white \
                    skill_abilitymaterial_ultimate \
                    scavenger_data01 scavenger_data02 scavenger_data03 scavenger_data04 \
                    scavenger_carbonitecicuitboard scavenger_bronsiumwiring \
                    scavenger_chromiumtransisters scavenger_aurodiumheatsink \
                    scavenger_electriumconductors scavenger_zinbiddlecard \
                    scavenger_impulsedetector scavenger_aeromagnifier \
                    scavenger_gyrdakeypad scavenger_droidbrain scavenger_servomotor; do
          curl -s -OJ "http://127.0.0.1:8080/Asset/single?assetName=$name"
        done
        ```

    === "Python"

        ```python
        from pathlib import Path

        import httpx

        MATERIALS: dict[str, str] = {
            "Ability Material Mk I": "skill_triangle_white",
            "Ability Material Mk II": "skill_square_white",
            "Ability Material Mk III": "skill_pentagon_white",
            "Ability Material Omega": "skill_pentagon_gold",
            "Ability Material Zeta": "skill_zeta",
            "Ability Material Omicron": "skill_hexagon_white",
            "Ability Material Ultimate": "skill_abilitymaterial_ultimate",
            "Fragmented Signal Data": "scavenger_data01",
            "Incomplete Signal Data": "scavenger_data02",
            "Flawed Signal Data": "scavenger_data03",
            "Corrupted Signal Data": "scavenger_data04",
            "Carbonite Circuit Board": "scavenger_carbonitecicuitboard",
            "Bronzium Wiring": "scavenger_bronsiumwiring",
            "Chromium Transistor": "scavenger_chromiumtransisters",
            "Aurodium Heatsink": "scavenger_aurodiumheatsink",
            "Electrium Conductor": "scavenger_electriumconductors",
            "Zinbiddle Card": "scavenger_zinbiddlecard",
            "Impulse Detector": "scavenger_impulsedetector",
            "Aeromagnifier": "scavenger_aeromagnifier",
            "Gyrda Keypad": "scavenger_gyrdakeypad",
            "Droid Brain": "scavenger_droidbrain",
            "Coaxial Servomotor": "scavenger_servomotor",
        }


        def main() -> None:
            out = Path("materials")
            out.mkdir(exist_ok=True)
            with httpx.Client(base_url="http://127.0.0.1:8080", timeout=60) as client:
                for name, bundle in MATERIALS.items():
                    response = client.get("/Asset/single", params={"assetName": bundle})
                    response.raise_for_status()
                    (out / f"{name}.png").write_bytes(response.content)


        if __name__ == "__main__":
            main()
        ```

        ```bash
        uv run --with httpx materials.py
        ```
