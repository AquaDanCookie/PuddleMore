# PuddleMore

PuddleMore is a spreadsheet-style audio and video metadata editor written in
Rust. It keeps the familiar puddletag layout — editable tag columns, Tag Panel,
Artwork, Filesystem, Filter and Logs docks, Actions, Functions and the original
menus and toolbar — without a Python runtime.

## Build and run

Requirements:

- Rust and Cargo
- TagLib C (`libtag_c.so`, version 2.x; `libtagc0-dev` on Debian/Ubuntu)

```sh
cd "PuddleTag Source Code"
./PuddleMore
```

The launcher builds `rust/target/release/puddlemore` on first use and then runs
it. To build manually:

```sh
cd rust
cargo build --release
./target/release/puddlemore
```

Open files or folders directly:

```sh
./PuddleMore song.mkv Album/
```

## Features

- Editable spreadsheet with sorting, filtering, multi-selection, Tag Panel,
  Extended Tags, column visibility, row numbers and status statistics.
- High-contrast System, Light and Dark themes in Preferences → Appearance.
- Undo/Redo, Preview Mode, Write Previews and Clear Previews.
- Actions and Functions using `%field%` and `$function(...)` patterns.
- File→Tag, Tag→File, Format and Rename Directories.
- Artwork: add, replace, remove and save embedded cover art.
- Tag Tools: remove ID3v1, ID3v2, all ID3 and APEv2 tags.
- Playlist load/save (M3U), tag export (CSV, HTML, RTF, TXT, M3U), Logs,
  Stored Tags and file deletion.
- Matroska Segment Info `Title`, global tags, arbitrary tag names, targeted
  tags, cover art and non-image attachments.
- Native AIFF/AIFC chunks, TTA APEv2, WMA/ASF via TagLib, and the usual
  MP3/FLAC/Ogg/Opus/MP4/AAC/WAV/WavPack/APE/MPC families.

## Formats

| Container | Extensions |
| --- | --- |
| ID3v1/ID3v2 | `.mp1`, `.mp2`, `.mp3`, `.wav`, `.wave`, `.aif`, `.aifc`, `.aiff`, `.tta` |
| MP4 atoms | `.mp4`, `.m4a`, `.m4b`, `.m4v` |
| Vorbis comments | `.ogg`, `.oga`, `.opus`, `.spx`, `.flac` |
| APEv2 | `.ape`, `.wv`, `.mpc`, `.tta`, `.tak`, `.ofr` |
| Windows Media | `.wma`, `.wmv`, `.asf` |
| Matroska | `.mka`, `.mkv`, `.webm` |

## Command line

```sh
puddlemore --print song.mkv
puddlemore --print --json Album/
puddlemore --set 'title=New title' --set year=2026 song.mkv
puddlemore --set 'comment=' song.flac
```

## Development

```sh
cd rust
cargo fmt --check
cargo test
cargo build --release
```

The Rust Matroska reader memory-maps the file and parses only the metadata
regions. Matroska saves rebuild the segment atomically when tags outgrow their
reserved space.

## Known limits

- WMA/WMV/ASF use TagLib's ASF property map; arbitrary ASF attributes that
  TagLib does not expose are not editable.
- Matroska track/chapter-targeted tags and non-image attachments are preserved
  but are not shown as editable table columns.
- The Rust Matroska writer rebuilds the segment on save; it does not yet stream
  unchanged clusters to a temporary file like the previous Python backend.
- Raw AC3/DTS bitstreams are not scanned because they have no supported tag
  container.

PuddleMore is based on puddletag by concentricpuddle and is licensed under the
GPLv3. See `copyright` and `LICENSE`.
