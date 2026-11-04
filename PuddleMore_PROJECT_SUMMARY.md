# PuddleMore Project Summary

## Status

PuddleMore is a working desktop audio/video tag editor built on the puddletag
PyQt6 interface, with PuddleMore branding, themes and extra format support. A
Rust metadata engine provides Matroska, APEv2 TTA, native AIFF and WAV handling
through the normal puddletag Tag API.

The launcher `PuddleMore` runs `puddlemore_main.py` with `.venv/bin/python` (or
`python3`) and builds the Rust helper on first use if needed. PuddleMore reports
its own version (`0.2.0`) rather than the upstream puddletag version.

## Architecture

- **PyQt6 UI** (`puddlestuff/`): the full puddletag spreadsheet, Tag Panel,
  Artwork, Filesystem, Filter, Logs, Stored Tags, Actions, Functions, Preview
  Mode, Extended Tags and Preferences.
- **Themes** (`puddlestuff/theme.py`): exact high-contrast System/Light/Dark
  palettes, Fusion styling, icon recolouring and a live Appearance preview.
- **Rust bridge** (`puddlestuff/audioinfo/rustbridge.py`): routes Matroska,
  TTA, AIFF/AIFC and WAV through `rust/target/release/puddlemore`, preserving
  targeted Matroska tags and non-image attachments.
- **Rust engine** (`rust/src/`): Matroska EBML reader/writer, native AIFF,
  APEv2, TagLib format map, CLI and standalone egui interface.
- **Extra routing**: MP1/MP2, WAV/WAVE, M4B, OGA and ASF extensions mapped to
  the existing mutagen/puddletag classes.

## Supported formats

- ID3v1/ID3v2: MP1, MP2, MP3, WAV/WAVE, AIFF/AIFC, TTA.
- MP4 atoms: MP4, M4A, M4B, M4V.
- Vorbis comments: OGG, OGA, OPUS, SPX, FLAC.
- APEv2: APE, WV, MPC, TTA, TAK, OFR.
- Microsoft: WMA, WMV, ASF.
- Matroska: MKV, MKA, WEBM.

## Verified

- Matroska read/write through the bridge, including arbitrary custom fields.
- TTA, AIFF/AIFC and WAV read/write through the Rust bridge.
- PyQt6 UI launches, loads MKV/MP3/WAV and displays tags.
- System, Light and Dark palettes apply through Preferences → Appearance.
- Rust `cargo fmt --check`, `cargo test` (15 tests) and release build pass.
- Launcher and desktop entry run the PyQt application and build the Rust helper
  when missing.

## Known limits

- Matroska cover art and targeted tags are preserved by the Rust engine but
  cover art is not exposed through the bridge's Artwork panel yet.
- Matroska saves rebuild the segment instead of streaming unchanged clusters.
- WMA/WMV/ASF use mutagen through puddletag; arbitrary ASF attributes that
  mutagen does not expose are not editable.
- Raw AC3/DTS bitstreams are not scanned.

## Relevant files

- `PuddleMore` — primary launcher.
- `puddlemore_main.py` — PyQt6 entry point.
- `puddlestuff/` — puddletag UI and metadata layer.
- `puddlestuff/theme.py` — PuddleMore themes.
- `puddlestuff/audioinfo/rustbridge.py` — Rust format adapter.
- `rust/` — Rust engine, CLI and standalone egui UI.
