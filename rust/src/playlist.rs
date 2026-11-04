//! M3U playlists, matching the original application's dialect.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn read(path: &Path) -> Result<Vec<PathBuf>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut files = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = line.replace(',', "");
        let entry = PathBuf::from(entry);
        let resolved = if entry.is_absolute() {
            entry
        } else {
            base.join(entry)
        };
        files.push(resolved);
    }
    Ok(files)
}

#[derive(Clone, Debug)]
pub struct PlaylistOptions {
    pub extended: bool,
    pub relative: bool,
    pub windows_separator: bool,
}

pub fn write(
    path: &Path,
    tracks: &[(PathBuf, u64, String)],
    options: &PlaylistOptions,
) -> Result<()> {
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut output = String::from("#EXTM3U\n");
    let mut lines = Vec::new();
    for (track_path, seconds, title) in tracks {
        let mut entry = if options.relative {
            pathdiff::diff_paths(track_path, base)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| track_path.display().to_string())
        } else {
            track_path.display().to_string()
        };
        if options.windows_separator {
            entry = entry.replace('/', "\\");
        }
        if options.extended {
            lines.push(format!("#EXTINF: {seconds}, {title}"));
        }
        lines.push(entry);
    }
    output.push_str(&lines.join("\n"));
    fs::write(path, output).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_m3u() {
        let path =
            std::env::temp_dir().join(format!("puddlemore-playlist-{}.m3u", std::process::id()));
        let tracks = vec![(
            PathBuf::from("/music/Artist/song.mp3"),
            65,
            "Artist - Song".to_string(),
        )];
        let options = PlaylistOptions {
            extended: true,
            relative: false,
            windows_separator: false,
        };
        write(&path, &tracks, &options).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#EXTM3U\n#EXTINF: 65, Artist - Song\n"));
        let files = read(&path).unwrap();
        assert_eq!(files, vec![PathBuf::from("/music/Artist/song.mp3")]);
        let _ = fs::remove_file(path);
    }
}
