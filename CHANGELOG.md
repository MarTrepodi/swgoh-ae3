# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4](https://github.com/MarTrepodi/swgoh-ae3/compare/v0.1.3...v0.1.4) - 2026-10-21

### Fixed

- *(release)* link releases and update checks to the current repository

## [0.1.2](https://github.com/MarTrepodi/swgoh-ae3/compare/v0.1.1...v0.1.2) - 2026-10-18

### Added

- *(gui)* open the About window from the macOS menu bar
- *(gui)* lay the window out as header, sidebar and cards
- *(gui)* draw icons from Phosphor instead of Unicode glyphs
- *(gui)* set the window in Ubuntu with a palette from the icon

### Fixed

- *(gui)* build the macOS menu module cleanly on every platform

## [0.1.1](https://github.com/MarTrepodi/swgoh-ae3/compare/v0.1.0...v0.1.1) - 2026-10-18

### Added

- *(gui)* rename to swgoh-ae3, add the icon and an About window
- *(gui)* name every input widget for screen readers
- *(gui)* add a scripted smoke run for CI

### Changed

- *(gui)* run the window from a library with seams for tests

### Fixed

- *(gui)* let table clicks reach the row and the preview cells

## [0.1.0](https://github.com/swgoh-utils/swgoh-ae3/releases/tag/v0.1.0)

The first release. This section is written by hand; release-plz generates
every later section from the Conventional Commits since the previous tag.

### Added

- *(core)* Download, cache, version Diff and configuration infrastructure
- *(unity)* UnityFS Bundles, SerializedFiles and category prefix probing
- *(audio)* Audio Package extraction and WEM to Ogg Vorbis conversion
- *(image)* Unity texture decoding and PNG/WebP encoding
- *(core)* Asset access and the export layer
- *(core)* The Asset Index and asset search
- *(cli)* The `ae3` command-line tool, with the optional `serve` HTTP API
- *(gui)* The `ae3-gui` desktop app: manifest, search, Bundles table, preview, detail viewer, Diff and extraction
- *(release)* Release archives for Windows x86_64, macOS arm64 and Linux x86_64/aarch64
- *(core)* Display-name search: a Name Index built from the community gamedata mirror (units, abilities, gear, materials, player portraits) finds Bundles by in-game names such as "Bossk", with a `language` setting (`AE3_LANGUAGE`, default `ENG_US`)
- *(cli)* `ae3 search` lists matching Game Entities above the Bundles, includes them in `--json`, and takes `--no-display-names` and `--language`
- *(gui)* Search lists matching Game Entities under the search box; picking one shows only its Bundles
- *(core)* Mesh export as OBJ and binary glTF (`.glb`) with the main texture embedded: `mesh-format` setting (`AE3_MESH_FORMAT`) and `--mesh-format obj|glb|both`
- *(core)* Structured Assets export combined into one `{bundle}.structured.json` per Bundle by default; the `structured` setting (`AE3_STRUCTURED`, `--structured separate|combined|none`) writes one file per Asset or skips them
- *(audio)* PTADPCM sounds convert to 16-bit WAV, and every Vorbis, PCM and PTADPCM sound decodes to PCM
- *(gui)* Play and stop sounds in the Audio Package preview and the detail viewer (on Linux this needs `libasound.so.2` at run time)
- *(gui)* Orbit-camera mesh preview
- *(gui)* The window opens the last Asset Version from the cache straight away and moves to the latest one when it is found
- *(gui)* Structured Asset and Mesh export formats in the Output section
- *(gui)* A link says when a newer AE3 release is out, checked at most once a day; the `check-for-updates` setting (`AE3_CHECK_FOR_UPDATES`) turns it off

### Changed

- *(core)* Release builds unwind on panic, and a decoder panic is contained to the one Asset, sound or Package it hit

### Fixed

- *(core)* Export failures on real game data found by per-category smoke tests
- *(core)* Native Sprites with fractional rects are cut instead of failing
- *(cli)* `ae3 manifest` Environment output and export summary plurals
- *(gui)* The window app id matches the Linux `.desktop` entry
- *(cli)* `ae3 diff --category` scopes every count, size and the Engine Rebuild figure to that Category
- *(audio)* A corrupt Vorbis sound fails that sound instead of panicking
- *(gui)* Dependency Bundles are released when the preview moves to another Bundle or an extraction ends
- *(gui)* Preview cells no longer overlap, and the audio preview keeps its width

### Performance

- *(core)* A first Asset Index build takes about a minute and fetches about 585 MB for Asset Version 100054 on Windows, reusing connections after Range requests and reading only the names search needs
