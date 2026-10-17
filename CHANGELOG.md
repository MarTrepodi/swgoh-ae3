# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
