# PuddleMore Project Summary

## Status

PuddleMore is now a single Rust/egui desktop application plus a Rust CLI. The
old PyQt/puddletag source tree, Python launcher path, virtualenv and bridge
script have been removed. The launcher `PuddleTag Source Code/PuddleMore`
builds `rust/target/release/puddlemore` on first use and runs it.

## Architecture

- **UI** (`rust/src/main.rs`): spreadsheet table, Tag Panel, Filesystem, Filter,
  Artwork, Logs and Stored Tags, Extended Tags, Columns, Preferences, status
  bar, menus, toolbar, preview mode and undo/redo.
- **Configuration** (`config.rs`): reads and writes the existing
  `~/.config/puddletag/puddletag.conf` JSON and `gensettings`, so existing
  settings and column/theme choices survive the migration.
- **Themes** (`theme.rs`): exact high-contrast System/Light/Dark palettes and
  egui style mapping.
- **Patterns and actions** (`pattern.rs`, `actions.rs`): `%field%`,
  `$function(...)`, filename parsing, tag-to-filename, and JSON/legacy INI
  action files.
- **Convert/export/playlists** (`main.rs`, `export.rs`, `playlist.rs`):
  File→Tag, Tag→File, Format, Rename Directories, M3U load/save and CSV/HTML/
  RTF/TXT/M3U export.
- **Metadata engines**:
  - `taglib.rs`: TagLib C property map for the broad format set, plus embedded
    pictures through TagLib complex properties.
  - `matroska.rs`: native EBML/Matroska reader and writer, cover art, targeted
    tags and non-image attachment preservation.
  - `aiff.rs`: native AIFF/AIFC text chunks merged with ID3.
  - `apev2.rs`: native APEv2 reader/writer and deletion.
  - `tagtools.rs`: direct ID3v1/ID3v2 removal.
  - Lofty remains a secondary fallback when TagLib cannot parse a file.
- **Assets**: icons and the application icon are embedded from `rust/assets/`;
  there are no references into the deleted Python tree.

## Supported formats

- ID3v1/ID3v2: MP1, MP2, MP3, WAV/WAVE, AIFF/AIFC, TTA.
- MP4 atoms: MP4, M4A, M4B, M4V.
- Vorbis comments: OGG, OGA, OPUS, SPX, FLAC.
- APEv2: APE, WV, MPC, TTA, TAK, OFR.
- Microsoft: WMA, WMV, ASF (TagLib property map).
- Matroska: MKV, MKA, WEBM.

## Verification

- `cargo fmt --check` passes.
- `cargo test` passes (15 tests).
- `cargo build --release` passes.
- GUI launches with MKV and MP3 fixtures.
- TagLib picture round-trip checked on an MP3 with cover art.
- Matroska cover replacement preserves a non-image font attachment.
- CLI `--print`, `--json` and `--set` round-trip MKV and MP3 tags.
- Themes verified in System, Light and Dark modes.
- Launcher tested after deleting the Python tree.

## Known limits

- WMA/WMV/ASF use TagLib's ASF property map. Arbitrary ASF attributes that
  TagLib does not expose are not editable.
- Matroska track/chapter-targeted tags and non-image attachments are preserved
  but are not shown as editable table columns.
- The Rust Matroska writer rebuilds the segment on save; it does not stream
  unchanged clusters to a temporary file yet.
- The rewrite focuses on the core editor. The old puddletag music library,
  duplicate finder, mass-tagging profiles, plugin loader, shortcut editor and
  text-file/clipboard-to-tag tools were not carried over.
- Raw AC3/DTS bitstreams are not scanned because they have no supported tag
  container.

## Screenshots

- `screenshots/PuddleMore-light.png`
- `screenshots/PuddleMore-dark.png`

## Relevant files

- `PuddleTag Source Code/PuddleMore` — primary launcher.
- `PuddleTag Source Code/PuddleMore.desktop` — desktop entry.
- `PuddleTag Source Code/rust/src/main.rs` — UI, menus, dialogs and CLI.
- `PuddleTag Source Code/rust/src/metadata.rs` — format-neutral tag model.
- `PuddleTag Source Code/rust/src/taglib.rs` — TagLib engine and pictures.
- `PuddleTag Source Code/rust/src/matroska.rs` — native Matroska engine.
- `PuddleTag Source Code/rust/src/pattern.rs` — pattern evaluator.
- `PuddleTag Source Code/rust/src/actions.rs` — saved actions.
- `PuddleTag Source Code/rust/src/export.rs` — tag export.
- `PuddleTag Source Code/rust/src/playlist.rs` — M3U playlists.
- `PuddleTag Source Code/rust/src/config.rs` — settings.
- `PuddleTag Source Code/rust/src/theme.rs` — palettes.
