# PuddleMore Rust application

This directory contains PuddleMore's Rust metadata engine, command-line tool
and standalone egui interface. The primary desktop UI is the puddletag PyQt6
front end, which uses this binary as its adapter for Matroska, TTA/APEv2,
native AIFF and WAV.

## Build

Requirements:

- Rust toolchain (`cargo`, `rustc`). If Cargo is not on `PATH`, use
  `~/.cargo/bin/cargo`.
- TagLib C library and development headers (`libtag_c.so`, version 2.x).
  On Debian/Ubuntu the package is `libtagc0-dev`; on Fedora it is
  `taglib-devel`.

```sh
cd rust
~/.cargo/bin/cargo build --release
```

The binary is `target/release/puddlemore`. The `../PuddleMore` launcher builds
it on first use and the Python UI invokes it for the extra formats.

## Usage

```sh
# Open the GUI (paths may be files or folders)
./target/release/puddlemore
./target/release/puddlemore song.mkv Album/

# Print every field, including read-only file information
./target/release/puddlemore --print song.mkv

# Print as JSON
./target/release/puddlemore --print --json Album/

# Edit tags without the GUI. Repeat --set as needed.
./target/release/puddlemore --set 'title=New title' --set year=2026 song.mkv

# Use an empty value to remove a field
./target/release/puddlemore --set 'comment=' song.flac
```

Folders are scanned recursively. The interface includes the menu bar and icon
toolbar, Tag Panel and Filesystem docks, a full-width Filter dock, artwork,
logs, stored tags, extended tags, columns, preview mode, undo/redo and
Preferences.

Double-click a cell to edit it; Enter commits and Escape cancels. Files can
also be dropped onto the window. `Ctrl+S` saves (or writes previews while
Preview Mode is on). `Ctrl+Z`/`Ctrl+Shift+Z` undo and redo, and `Ctrl+Shift+P`
toggles Preview Mode.

## Metadata engines

| Engine | Used for | Notes |
| --- | --- | --- |
| Native Matroska | `.mkv`, `.mka`, `.webm` | Segment Info `Title`, global tags, arbitrary names, targeted tags, cover art and non-image attachments. The reader memory-maps the file and parses metadata regions. |
| Native AIFF chunks | `.aif`, `.aiff`, `.aifc` with `NAME`/`AUTH`/`(c) `/`ANNO` | Merges native text chunks with ID3 and preserves other chunks including `SSND`. |
| TagLib property map | MP3/ID3, FLAC, Ogg Vorbis/Opus/Speex, MP4/M4A/ALAC, WAV, AIFF, WavPack, Monkey's Audio, Musepack, DSF/DFF, WMA/ASF and other TagLib formats | Preserves arbitrary text properties and cover art. UTF-8 ID3 is enabled. |
| Native APEv2 | TrueAudio (`.tta`) with an APEv2 tag | Binary APE items such as cover art are preserved; Tag Tools can delete the tag. |
| Lofty | Secondary fallback when TagLib cannot parse a file | Fixed key model; used only when no better engine works. |

## Known limits

- WMA/WMV/ASF use TagLib's ASF property map; arbitrary ASF attributes that
  TagLib does not expose are not editable.
- Matroska targeted tags and non-image attachments are preserved but are not
  editable table columns.
- Matroska saves rebuild the segment instead of streaming unchanged clusters.
- DSF/DFF, TAK, Monkey's Audio, Musepack and OptimFROG have no local encoder in
  the test environment, so they rely on TagLib without an end-to-end fixture.

## Tests

```sh
~/.cargo/bin/cargo test
~/.cargo/bin/cargo build --release
```

The tests cover column labels, JSON escaping, APEv2 key/footer handling, AIFF
chunk mapping, action application, pattern rendering, filename parsing,
playlists, export, ID3 removal and the existing format engines.
