# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

<!--
Hand-written summary of the work since the 0.1.0 section. release-plz
writes the next version's section from the Conventional Commits and keeps
whatever sits here below it, so delete this block in the release PR (or
move it into 0.1.0 if v0.1.0 is tagged on a commit that already has it).
-->

### Added

- *(core)* Display-name search: a Name Index built from the community gamedata mirror (units, abilities, gear, materials, player portraits) finds Bundles by in-game names such as "Bossk", with a `language` setting (`AE3_LANGUAGE`, default `ENG_US`)
- *(cli)* `ae3 search` lists matching Game Entities above the Bundles, adds them to `--json`, and gains `--no-display-names` and `--language`
- *(gui)* Search lists matching Game Entities under the search box; picking one shows only its Bundles
- *(core)* Meshes export as binary glTF (`.glb`) with their main textures embedded, as well as or instead of OBJ: `mesh-format` setting (`AE3_MESH_FORMAT`) and `--mesh-format obj|glb|both` on `extract` and `diff`
- *(core)* Structured Assets are combined into one `{bundle}.structured.json` per Bundle by default; the `structured` setting (`AE3_STRUCTURED`, `--structured separate|combined|none`) brings back one file per Asset or skips them
- *(audio)* PTADPCM sounds convert to 16-bit WAV, and every Vorbis, PCM and PTADPCM sound decodes to PCM
- *(gui)* Play and stop sounds in the Audio Package preview and the detail viewer
- *(gui)* The window opens the last Asset Version from the cache straight away and moves to the latest one when it is found
- *(gui)* Choose the Structured Asset and Mesh export formats in the Output section
- *(gui)* A link says when a newer AE3 release is out, checked at most once a day; the `check-for-updates` setting (`AE3_CHECK_FOR_UPDATES`) turns it off

### Changed

- *(core)* Whole-Bundle exports now write Structured Assets combined per Bundle; pass `--structured separate` (or set `structured = "separate"`) for one JSON file per Asset as before
- *(core)* The Asset Index file moves to format version 2; version 1 files are read and rewritten, while older AE3 builds rebuild a version 2 file from scratch
- *(gui)* On Linux the GUI links ALSA for sound, so it needs `libasound.so.2` at run time and `libasound2-dev` to build

### Fixed

- *(cli)* `ae3 diff --category` scopes every count, size and the Engine Rebuild figure to that Category
- *(audio)* A corrupt Vorbis sound fails that sound instead of panicking in ww2ogg or lewton
- *(gui)* Dependency Bundles are released when the preview moves to another Bundle or an extraction ends

### Performance

- *(core)* A first Asset Index build fetches less and reuses connections after Range requests: about a minute and 585 MB for 100054 Windows, down from about three minutes and 680 MB
- *(unity)* Category prefix probes read only the names search needs and estimate the missing compressed bytes more closely

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

### Changed

- *(core)* Release builds unwind on panic, and a decoder panic is contained to the one Asset, sound or Package it hit

### Fixed

- *(core)* Export failures on real game data found by per-category smoke tests
- *(core)* Native Sprites with fractional rects are cut instead of failing
- *(cli)* `ae3 manifest` Environment output and export summary plurals
- *(gui)* The window app id matches the Linux `.desktop` entry
