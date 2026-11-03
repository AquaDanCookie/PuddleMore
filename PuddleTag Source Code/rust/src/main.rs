//! PuddleMore — audio tag editor.
//!
//! This binary is both the desktop application and a small command line tool:
//!
//! ```text
//! puddlemore                       # open the GUI
//! puddlemore song.mkv album/       # open these paths in the GUI
//! puddlemore --print song.mkv      # print every tag
//! puddlemore --set title=New ...   # edit tags without the GUI
//! ```

mod actions;
mod aiff;
mod apev2;
mod config;
mod export;
mod matroska;
mod metadata;
mod pattern;
mod playlist;
mod taglib;
mod tagtools;
mod theme;

use anyhow::{anyhow, Result};
use eframe::egui;
use metadata::{default_columns, load_track, save_track, scan_paths, Track};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "mp2", "mp1", "flac", "ogg", "oga", "opus", "spx", "m4a", "mp4", "m4v", "m4b", "aac",
    "wav", "wave", "aif", "aiff", "aifc", "wv", "ape", "mpc", "mpp", "tta", "tak", "dsf", "dff",
    "ofr", "ofs", "mka", "mkv", "webm", "wma", "wmv", "asf",
];

fn main() {
    // Behave like a normal Unix filter when a pager/head closes the pipe.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    if let Err(err) = run() {
        eprintln!("puddlemore: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse(std::env::args().skip(1))?;
    if cli.help {
        print_help();
        return Ok(());
    }
    if cli.version {
        println!("PuddleMore {VERSION}");
        return Ok(());
    }

    if cli.print || !cli.sets.is_empty() {
        return run_cli(&cli);
    }

    run_gui(cli.paths)
}

// ---------------------------------------------------------------------------
// Command line
// ---------------------------------------------------------------------------

struct Cli {
    paths: Vec<PathBuf>,
    sets: Vec<(String, String)>,
    print: bool,
    json: bool,
    help: bool,
    version: bool,
}

impl Cli {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut cli = Cli {
            paths: Vec::new(),
            sets: Vec::new(),
            print: false,
            json: false,
            help: false,
            version: false,
        };
        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => cli.help = true,
                "-V" | "--version" => cli.version = true,
                "-p" | "--print" => cli.print = true,
                "--json" => cli.json = true,
                "-s" | "--set" => {
                    let assignment = args
                        .next()
                        .ok_or_else(|| anyhow!("--set needs FIELD=VALUE"))?;
                    let (field, value) = assignment.split_once('=').ok_or_else(|| {
                        anyhow!("invalid --set value {assignment:?}; expected FIELD=VALUE")
                    })?;
                    if field.trim().is_empty() {
                        return Err(anyhow!("empty field name in --set"));
                    }
                    cli.sets
                        .push((field.trim().to_ascii_lowercase(), value.to_string()));
                }
                "--gui" => {}
                other if other.starts_with('-') => {
                    return Err(anyhow!("unknown option {other:?} (try --help)"));
                }
                other => cli.paths.push(PathBuf::from(other)),
            }
        }
        if cli.json && !cli.print {
            return Err(anyhow!("--json only makes sense together with --print"));
        }
        Ok(cli)
    }
}

fn run_cli(cli: &Cli) -> Result<()> {
    if cli.paths.is_empty() {
        return Err(anyhow!("no files or folders given"));
    }
    let files = scan_paths(&cli.paths);
    if files.is_empty() {
        return Err(anyhow!("no supported audio files found"));
    }

    let mut failures = 0usize;
    let mut json_rows = Vec::new();
    for path in &files {
        let mut track = load_track(path);
        if !cli.sets.is_empty() {
            match apply_sets(&mut track, &cli.sets).and_then(|()| save_track(&mut track)) {
                Ok(()) => {
                    if !cli.print {
                        println!("{}: saved", path.display());
                    }
                }
                Err(err) => {
                    failures += 1;
                    eprintln!("{}: {err:#}", path.display());
                }
            }
        } else if let Some(error) = &track.error {
            failures += 1;
            eprintln!("{}: {error}", path.display());
        }
        if cli.print {
            if cli.json {
                json_rows.push(track_json(&track));
            } else {
                print_track(&track);
            }
        }
    }
    if cli.json {
        println!("[");
        for (index, row) in json_rows.iter().enumerate() {
            let comma = if index + 1 == json_rows.len() {
                ""
            } else {
                ","
            };
            println!("  {row}{comma}");
        }
        println!("]");
    }
    if failures > 0 {
        return Err(anyhow!("{failures} file(s) failed"));
    }
    Ok(())
}

fn apply_sets(track: &mut Track, sets: &[(String, String)]) -> Result<()> {
    for (field, value) in sets {
        if Track::is_read_only(field) {
            return Err(anyhow!("field {field:?} is read-only"));
        }
        track.set_display(field, value.clone());
    }
    Ok(())
}

fn print_track(track: &Track) {
    println!("{}", track.path.display());
    for (field, values) in &track.fields {
        let value = values.join("\\\\");
        println!("  {field}={}", value.replace('\n', "\\n"));
    }
}

fn track_json(track: &Track) -> String {
    let mut object = String::from("{\"path\":");
    object.push_str(&json_string(&track.path.display().to_string()));
    object.push_str(",\"format\":");
    object.push_str(&json_string(&track.format));
    if let Some(error) = &track.error {
        object.push_str(",\"error\":");
        object.push_str(&json_string(error));
    }
    object.push_str(",\"tags\":{");
    let mut first = true;
    for (field, values) in &track.fields {
        if !first {
            object.push(',');
        }
        first = false;
        object.push_str(&json_string(field));
        object.push(':');
        object.push_str(&json_string(&values.join("\\\\")));
    }
    object.push_str("}}");
    object
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn print_help() {
    println!(
        "PuddleMore {VERSION} — audio tag editor

USAGE:
    puddlemore [OPTIONS] [PATH...]

With paths and no action option PuddleMore opens its GUI. Folders are scanned
recursively for supported audio files.

OPTIONS:
    -p, --print           Print all tags (and file information) as field=value.
        --json            Print tags as JSON; use together with --print.
    -s, --set FIELD=VALUE Set a tag. Use an empty VALUE to remove a field.
                          May be repeated. FIELD names are canonical names such
                          as title, artist, album, albumartist, track,
                          totaltracks, disc, year, genre, comment, composer.
        --gui             Force the GUI even when other options are absent.
    -h, --help            Show this help.
    -V, --version         Show the version.

EXAMPLES:
    puddlemore --print video.mkv
    puddlemore --set title=\"My film\" --set year=2024 video.mkv
    puddlemore --print --json Music/
"
    );
}

// ---------------------------------------------------------------------------
// GUI
// ---------------------------------------------------------------------------

const TABLE_ROW_HEIGHT: f32 = 20.0;
const TABLE_HEADER_HEIGHT: f32 = 23.0;
const TABLE_INDEX_WIDTH: f32 = 16.0;
const TABLE_FONT_SIZE: f32 = 12.0;
const LEFT_DOCK_WIDTH: f32 = 289.0;

const TAG_FIELDS: [(&str, &str); 7] = [
    ("Artist", "artist"),
    ("Title", "title"),
    ("Album", "album"),
    ("Track", "track"),
    ("Year", "year"),
    ("Genre", "genre"),
    ("Comment", "comment"),
];

fn run_gui(paths: Vec<PathBuf>) -> Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("PuddleMore")
        .with_app_id("PuddleMore")
        .with_inner_size([680.0, 600.0])
        .with_min_inner_size([640.0, 480.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "PuddleMore",
        options,
        Box::new(move |cc| Ok(Box::new(PuddleMoreApp::new(cc, paths)))),
    )
    .map_err(|err| anyhow!("{err}"))
}

fn install_system_fonts(ctx: &egui::Context) {
    let candidates = [
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/opentype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ];
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = candidates.iter().find_map(|path| fs::read(path).ok()) {
        fonts.font_data.insert(
            "puddlemore-sans".to_string(),
            Arc::new(egui::FontData::from_owned(bytes)),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "puddlemore-sans".to_string());
    }
    ctx.set_fonts(fonts);
}

fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/PuddleMore.png"))
        .ok()?
        .into_rgba8();
    let image = image::imageops::resize(&image, 64, 64, image::imageops::FilterType::Triangle);
    Some(egui::IconData {
        rgba: image.into_raw(),
        width: 64,
        height: 64,
    })
}

struct Icons {
    folder: egui::TextureHandle,
    folder_new: egui::TextureHandle,
    refresh: egui::TextureHandle,
    save: egui::TextureHandle,
    undo: egui::TextureHandle,
    file_to_tag: egui::TextureHandle,
    tag_to_file: egui::TextureHandle,
    format: egui::TextureHandle,
    rename_dirs: egui::TextureHandle,
    parent: egui::TextureHandle,
    actions: egui::TextureHandle,
    quick_actions: egui::TextureHandle,
}

impl Icons {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            folder: load_texture(ctx, "icon-folder", include_bytes!("../assets/folder.png")),
            folder_new: load_texture(
                ctx,
                "icon-folder-new",
                include_bytes!("../assets/folder-new.png"),
            ),
            refresh: load_texture(
                ctx,
                "icon-refresh",
                include_bytes!("../assets/view-refresh.png"),
            ),
            save: load_texture(
                ctx,
                "icon-save",
                include_bytes!("../assets/document-save.png"),
            ),
            undo: load_texture(ctx, "icon-undo", include_bytes!("../assets/edit-undo.png")),
            file_to_tag: load_texture(
                ctx,
                "icon-file-to-tag",
                include_bytes!("../assets/puddletag.filetotag.png"),
            ),
            tag_to_file: load_texture(
                ctx,
                "icon-tag-to-file",
                include_bytes!("../assets/puddletag.tagtofile.png"),
            ),
            format: load_texture(
                ctx,
                "icon-format",
                include_bytes!("../assets/document-edit.png"),
            ),
            rename_dirs: load_texture(
                ctx,
                "icon-rename-dirs",
                include_bytes!("../assets/puddletag.renamedirs.png"),
            ),
            parent: load_texture(
                ctx,
                "icon-parent-folder",
                include_bytes!("../assets/view-sidetree.png"),
            ),
            actions: load_texture(
                ctx,
                "icon-actions",
                include_bytes!("../assets/quickopen.png"),
            ),
            quick_actions: load_texture(
                ctx,
                "icon-quick-actions",
                include_bytes!("../assets/quickopen-function.png"),
            ),
        }
    }
}

fn load_texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> egui::TextureHandle {
    let mut image = image::load_from_memory(bytes)
        .unwrap_or_else(|err| panic!("failed to decode embedded icon {name}: {err}"))
        .into_rgba8();
    recolor_icon(&mut image, theme::palette().dark);
    let size = [image.width() as usize, image.height() as usize];
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR)
}

fn recolor_icon(image: &mut image::RgbaImage, dark: bool) {
    for pixel in image.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        if a == 0 {
            continue;
        }
        let max = r.max(g).max(b) as u32;
        let min = r.min(g).min(b) as u32;
        let saturation = max - min;
        let luminance = (299 * r as u32 + 587 * g as u32 + 114 * b as u32) / 1000;
        if saturation <= 28 {
            let value = if dark {
                // Invert neutral artwork for dark backgrounds: white pages
                // become dark grey and black outlines become light.
                (60.0 + (255.0 - luminance as f32) / 255.0 * 160.0) as u32
            } else {
                (luminance as f32 * 0.28 + 8.0) as u32
            }
            .clamp(0, 255) as u8;
            pixel.0 = [value, value, value, a];
        } else if dark && luminance < 70 {
            let blend = |channel: u8| (channel as f32 + (255.0 - channel as f32) * 0.55) as u8;
            pixel.0 = [blend(r), blend(g), blend(b), a];
        } else if !dark && luminance > 205 {
            let darken = |channel: u8| (channel as f32 * 0.55) as u8;
            pixel.0 = [darken(r), darken(g), darken(b), a];
        }
    }
}

#[derive(Clone)]
struct Column {
    title: String,
    field: String,
    visible: bool,
    builtin: bool,
}

struct ScanResult {
    tracks: Vec<Track>,
    truncated: bool,
}

struct CellEdit {
    row: usize,
    field: String,
    buffer: String,
    request_focus: bool,
}

#[derive(Clone)]
struct FieldChange {
    row: usize,
    field: String,
    before: Option<Vec<String>>,
    after: Option<Vec<String>>,
}

#[derive(Clone)]
struct UndoBatch {
    label: String,
    changes: Vec<FieldChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvertKind {
    FileToTag,
    TagToFile,
    Format,
    RenameDirs,
}

struct ConvertDialog {
    kind: ConvertKind,
    pattern: String,
    field: String,
}

struct ExportDialog {
    template: String,
    preset: usize,
}

enum UiAction {
    OpenFolder,
    OpenFiles,
    AddFolder,
    Save,
    Refresh,
    RefreshSelected,
    Unload,
    Exit,
    SelectAll,
    InvertSelection,
    ExtendedTags,
    ToggleTagPanel,
    ToggleFilesystem,
    ToggleFilter,
    Columns,
    GoToParent,
    About,
    Preferences,
    Undo,
    Redo,
    TogglePreview,
    WritePreviews,
    ClearPreview,
    ClearPreviewCells,
    RemoveApev2,
    RemoveId3v2,
    RemoveId3v1,
    RemoveId3,
    ShowActions,
    ShowFunctions,
    ConvertFileToTag,
    ConvertTagToFile,
    ConvertFormat,
    ConvertRenameDirs,
    LoadPlaylist,
    SavePlaylist,
    ExportTags,
    DeleteFiles,
    ToggleLogs,
    ToggleStoredTags,
    ToggleArtwork,
}

struct PuddleMoreApp {
    tracks: Vec<Track>,
    selected: BTreeSet<usize>,
    selection_anchor: Option<usize>,
    columns: Vec<Column>,
    sort_field: String,
    sort_ascending: bool,
    filter_input: String,
    filter: String,
    filter_history: Vec<String>,
    status: String,
    receiver: Option<Receiver<ScanResult>>,
    scanning: bool,
    scan_replace: bool,
    show_columns: bool,
    show_extended: bool,
    show_about: bool,
    show_tag_panel: bool,
    show_filesystem: bool,
    show_filter: bool,
    tag_panel_open: bool,
    filesystem_open: bool,
    filter_open: bool,
    new_field: String,
    edit_cell: Option<CellEdit>,
    icons: Icons,
    pattern: String,
    history: std::collections::BTreeMap<String, BTreeSet<String>>,
    dir_cache: std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    expanded_dirs: std::collections::HashSet<PathBuf>,
    current_dir: Option<PathBuf>,
    subfolders: bool,
    fs_reveal_pending: bool,
    config: config::Config,
    theme_choice: theme::ThemeChoice,
    resolved_theme: theme::ResolvedTheme,
    show_preferences: bool,
    preferences_theme_before: theme::ThemeChoice,
    preview_mode: bool,
    undo_stack: Vec<UndoBatch>,
    redo_stack: Vec<UndoBatch>,
    show_actions: bool,
    show_functions: bool,
    macros: Vec<actions::Macro>,
    checked_actions: BTreeSet<String>,
    function_name: String,
    function_fields: String,
    function_args: String,
    convert_dialog: Option<ConvertDialog>,
    export_dialog: Option<ExportDialog>,
    confirm_delete: bool,
    show_logs: bool,
    logs: Vec<String>,
    show_stored_tags: bool,
    show_artwork: bool,
    artwork_cache: Option<(u64, egui::TextureHandle, [usize; 2])>,
}

impl PuddleMoreApp {
    fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        let config = config::Config::load();
        let theme_choice = theme::ThemeChoice::from_config(&config.theme());
        let selection = config
            .value("table", "selected_color")
            .is_some()
            .then(|| config.selected_color([0x2f, 0x7e, 0xc7]));
        let preview = config
            .value("table", "preview_color")
            .is_some()
            .then(|| config.preview_color([0xc0, 0xff, 0xc0]));
        let resolved_theme = theme::apply(&cc.egui_ctx, theme_choice, selection, preview);
        install_system_fonts(&cc.egui_ctx);

        let mut columns: Vec<Column> = default_columns()
            .into_iter()
            .map(|(title, field)| Column {
                title,
                field,
                visible: true,
                builtin: true,
            })
            .collect();
        for extra in [
            ("Album Artist", "albumartist"),
            ("Track Total", "totaltracks"),
            ("Disc", "disc"),
            ("Disc Total", "totaldiscs"),
            ("Composer", "composer"),
            ("Description", "description"),
            ("Lyrics", "lyrics"),
            ("Copyright", "copyright"),
            ("BPM", "bpm"),
            ("ISRC", "isrc"),
            ("Mood", "mood"),
            ("Language", "language"),
            ("Label", "organization"),
        ] {
            if !columns.iter().any(|column| column.field == extra.1) {
                columns.push(Column {
                    title: extra.0.to_string(),
                    field: extra.1.to_string(),
                    visible: false,
                    builtin: true,
                });
            }
        }

        let mut app = PuddleMoreApp {
            tracks: Vec::new(),
            selected: BTreeSet::new(),
            selection_anchor: None,
            columns,
            sort_field: "__filename".to_string(),
            sort_ascending: true,
            filter_input: String::new(),
            filter: String::new(),
            filter_history: Vec::new(),
            status: "Ready".to_string(),
            receiver: None,
            scanning: false,
            scan_replace: true,
            show_columns: false,
            show_extended: false,
            show_about: false,
            show_tag_panel: true,
            show_filesystem: true,
            show_filter: true,
            tag_panel_open: true,
            filesystem_open: true,
            filter_open: true,
            new_field: String::new(),
            edit_cell: None,
            icons: Icons::new(&cc.egui_ctx),
            pattern: "%title%".to_string(),
            history: std::collections::BTreeMap::new(),
            dir_cache: std::collections::HashMap::new(),
            expanded_dirs: std::collections::HashSet::new(),
            current_dir: None,
            subfolders: config.subfolders(),
            fs_reveal_pending: false,
            config,
            theme_choice,
            resolved_theme,
            show_preferences: false,
            preferences_theme_before: theme_choice,
            preview_mode: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            show_actions: false,
            show_functions: false,
            macros: actions::load_macros(),
            checked_actions: BTreeSet::new(),
            function_name: "format".to_string(),
            function_fields: "title".to_string(),
            function_args: "%artist% - %title%".to_string(),
            convert_dialog: None,
            export_dialog: None,
            confirm_delete: false,
            show_logs: false,
            logs: Vec::new(),
            show_stored_tags: false,
            show_artwork: false,
            artwork_cache: None,
        };

        if let Some(folder) = paths.iter().find(|path| path.is_dir()) {
            app.current_dir = Some(folder.clone());
            app.reveal_in_filesystem(folder);
        } else if let Some(parent) = paths
            .iter()
            .find(|path| path.is_file())
            .and_then(|path| path.parent())
        {
            app.current_dir = Some(parent.to_path_buf());
            app.reveal_in_filesystem(parent);
        }

        if !paths.is_empty() {
            app.start_scan(cc.egui_ctx.clone(), paths, true);
        }
        app
    }

    fn start_scan(&mut self, ctx: egui::Context, paths: Vec<PathBuf>, replace: bool) {
        if self.scanning {
            self.status = "Already scanning…".to_string();
            return;
        }
        if let Some(folder) = paths.iter().find(|path| path.is_dir()) {
            self.current_dir = Some(folder.clone());
            self.reveal_in_filesystem(folder);
        } else if let Some(parent) = paths
            .iter()
            .find(|path| path.is_file())
            .and_then(|path| path.parent())
        {
            self.current_dir = Some(parent.to_path_buf());
            self.reveal_in_filesystem(parent);
        }

        let (sender, receiver) = channel();
        self.receiver = Some(receiver);
        self.scanning = true;
        self.scan_replace = replace;
        self.status = format!("Scanning {} path(s)…", paths.len());
        let recursive = self.subfolders;
        std::thread::spawn(move || {
            let files = metadata::scan_paths_in(&paths, recursive);
            let total = files.len();
            let mut tracks = Vec::with_capacity(total);
            let mut truncated = false;
            for path in files {
                if tracks.len() >= 50_000 {
                    truncated = true;
                    break;
                }
                tracks.push(load_track(&path));
            }
            let _ = sender.send(ScanResult { tracks, truncated });
            ctx.request_repaint();
        });
    }

    fn poll_scan(&mut self, ctx: &egui::Context) {
        let mut finished = None;
        let mut disconnected = false;
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(result) => finished = Some(result),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => disconnected = true,
            }
        }
        if let Some(result) = finished {
            self.receiver = None;
            self.scanning = false;
            if self.scan_replace {
                self.tracks.clear();
                self.selected.clear();
                self.selection_anchor = None;
                self.history.clear();
                self.edit_cell = None;
                self.undo_stack.clear();
                self.redo_stack.clear();
            }
            let added = self.add_tracks(result.tracks);
            let errors = self
                .tracks
                .iter()
                .filter(|track| track.error.is_some())
                .count();
            if self.tracks.is_empty() {
                self.status = "No supported audio files found.".to_string();
            } else if result.truncated {
                self.status = format!("Showing the first {added} files (50,000 file limit).");
            } else {
                self.status = format!(
                    "Loaded {added} file(s); {} in library{}.",
                    self.tracks.len(),
                    if errors > 0 {
                        format!(", {errors} with errors")
                    } else {
                        String::new()
                    }
                );
            }
            if self.current_dir.is_none() {
                if let Some(parent) = self
                    .tracks
                    .first()
                    .and_then(|track| track.path.parent())
                    .map(Path::to_path_buf)
                {
                    self.current_dir = Some(parent.clone());
                    self.reveal_in_filesystem(&parent);
                }
            }
        }
        if disconnected {
            self.receiver = None;
            self.scanning = false;
            self.status = "Scan failed unexpectedly.".to_string();
        }
        if self.scanning {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn add_tracks(&mut self, tracks: Vec<Track>) -> usize {
        let mut existing: BTreeSet<PathBuf> =
            self.tracks.iter().map(|track| track.path.clone()).collect();
        let before = self.tracks.len();
        let mut first_added = None;
        for track in tracks {
            if existing.insert(track.path.clone()) {
                if first_added.is_none() {
                    first_added = Some(self.tracks.len());
                }
                self.merge_columns(&track);
                self.record_history(&track);
                self.tracks.push(track);
            }
        }
        if let Some(index) = first_added {
            if self.selected.is_empty() {
                self.selected.insert(index);
                self.selection_anchor = Some(index);
            }
        }
        self.tracks.len() - before
    }

    fn merge_columns(&mut self, track: &Track) {
        for field in track.fields.keys() {
            if field.starts_with("__") || self.columns.iter().any(|c| &c.field == field) {
                continue;
            }
            self.columns.push(Column {
                title: field_title(field),
                field: field.clone(),
                visible: false,
                builtin: false,
            });
        }
    }

    fn record_history(&mut self, track: &Track) {
        for (_, field) in TAG_FIELDS {
            let value = ui_display(track, field);
            if !value.is_empty() {
                self.history
                    .entry(field.to_string())
                    .or_default()
                    .insert(value);
            }
        }
    }

    fn remove_tag_kind(&mut self, kind: tagtools::TagKind) {
        if self.preview_mode {
            self.status = "Disable Preview Mode first to enable tag deletion.".to_string();
            return;
        }
        if self.selected.is_empty() {
            self.status = "Select files first.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let mut removed = 0usize;
        let mut failed = 0usize;
        for index in indices {
            let Some(path) = self.tracks.get(index).map(|track| track.path.clone()) else {
                continue;
            };
            match tagtools::remove(&path, kind) {
                Ok(true) => {
                    self.tracks[index] = load_track(&path);
                    removed += 1;
                }
                Ok(false) => {}
                Err(err) => {
                    failed += 1;
                    if let Some(track) = self.tracks.get_mut(index) {
                        track.error = Some(format!("{err:#}"));
                    }
                }
            }
        }
        self.status = if failed > 0 {
            format!("Removed {removed} tag(s); {failed} failed.")
        } else {
            format!("Removed {removed} tag(s).")
        };
    }

    fn push_undo(&mut self, label: impl Into<String>, mut changes: Vec<FieldChange>) {
        changes.retain(|change| change.before != change.after);
        if changes.is_empty() {
            return;
        }
        self.undo_stack.push(UndoBatch {
            label: label.into(),
            changes,
        });
        self.redo_stack.clear();
    }

    fn apply_change(&mut self, change: &FieldChange, use_after: bool) {
        let Some(track) = self.tracks.get_mut(change.row) else {
            return;
        };
        let value = if use_after {
            &change.after
        } else {
            &change.before
        };
        match value {
            Some(values) if !values.is_empty() => {
                track.fields.insert(change.field.clone(), values.clone());
            }
            _ => {
                track.fields.remove(&change.field);
            }
        }
        track.dirty = track.fields != track.original_fields;
    }

    fn undo(&mut self) {
        let Some(batch) = self.undo_stack.pop() else {
            self.status = "Nothing to undo.".to_string();
            return;
        };
        for change in batch.changes.iter().rev() {
            self.apply_change(change, false);
        }
        self.status = format!("Undid {}.", batch.label);
        self.redo_stack.push(batch);
    }

    fn redo(&mut self) {
        let Some(batch) = self.redo_stack.pop() else {
            self.status = "Nothing to redo.".to_string();
            return;
        };
        for change in &batch.changes {
            self.apply_change(change, true);
        }
        self.status = format!("Redid {}.", batch.label);
        self.undo_stack.push(batch);
    }

    fn toggle_preview(&mut self) {
        self.preview_mode = !self.preview_mode;
        if !self.preview_mode {
            for track in &mut self.tracks {
                track.fields = track.original_fields.clone();
                track.dirty = false;
            }
            self.status = "Preview Mode: Off".to_string();
        } else {
            self.status = "Preview Mode: On".to_string();
        }
    }

    fn write_previews(&mut self) {
        let indices: Vec<usize> = (0..self.tracks.len())
            .filter(|index| self.tracks[*index].fields != self.tracks[*index].original_fields)
            .collect();
        if indices.is_empty() {
            self.status = "No previews to write.".to_string();
            return;
        }
        let mut saved = 0usize;
        let mut failed = 0usize;
        for index in indices {
            let Some(track) = self.tracks.get_mut(index) else {
                continue;
            };
            match save_track(track) {
                Ok(()) => saved += 1,
                Err(err) => {
                    failed += 1;
                    track.error = Some(format!("{err:#}"));
                }
            }
        }
        if failed == 0 {
            self.preview_mode = false;
        }
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.status = match (saved, failed) {
            (saved, 0) => format!("Wrote {saved} preview(s)."),
            (saved, failed) => format!("Wrote {saved} preview(s); {failed} failed."),
        };
    }

    fn clear_previews(&mut self, cells_only: bool) {
        if !self.preview_mode {
            self.status = "Preview Mode is off.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let fields: Vec<String> = self
            .columns
            .iter()
            .map(|column| column.field.clone())
            .collect();
        let mut changes = Vec::new();
        for index in indices {
            let Some(track) = self.tracks.get(index) else {
                continue;
            };
            if cells_only {
                for field in &fields {
                    let before = track.fields.get(field).cloned();
                    let after = track.original_fields.get(field).cloned();
                    changes.push(FieldChange {
                        row: index,
                        field: field.clone(),
                        before,
                        after,
                    });
                }
            } else {
                for (field, before) in &track.fields {
                    let after = track.original_fields.get(field).cloned();
                    changes.push(FieldChange {
                        row: index,
                        field: field.clone(),
                        before: Some(before.clone()),
                        after,
                    });
                }
            }
        }
        for change in &changes {
            self.apply_change(change, true);
        }
        self.status = if cells_only {
            "Cleared selected preview cells.".to_string()
        } else {
            "Cleared selected preview files.".to_string()
        };
    }

    fn save_selected(&mut self) {
        if self.scanning {
            self.status = "Wait for the scan to finish before saving.".to_string();
            return;
        }
        if self.selected.is_empty() {
            self.status = "Select files to save.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let mut saved = 0usize;
        let mut failed = 0usize;
        for index in indices {
            let Some(track) = self.tracks.get_mut(index) else {
                continue;
            };
            if !track.dirty {
                continue;
            }
            match save_track(track) {
                Ok(()) => saved += 1,
                Err(err) => {
                    failed += 1;
                    track.error = Some(format!("{err:#}"));
                }
            }
        }
        self.update_status_after_save(saved, failed);
    }

    fn update_status_after_save(&mut self, saved: usize, failed: usize) {
        self.status = match (saved, failed) {
            (0, 0) => "Nothing to save.".to_string(),
            (saved, 0) => format!("Saved {saved} file(s)."),
            (0, failed) => format!("{failed} file(s) failed to save."),
            (saved, failed) => format!("Saved {saved} file(s); {failed} failed."),
        };
    }

    fn open_files(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Audio files", AUDIO_EXTENSIONS)
            .pick_files()
        {
            self.start_scan(ctx.clone(), files, true);
        }
    }

    fn open_folder(&mut self, ctx: &egui::Context, append: bool) {
        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
            self.start_scan(ctx.clone(), vec![folder], append);
        }
    }

    fn refresh_all(&mut self, ctx: &egui::Context) {
        if self.tracks.is_empty() {
            self.status = "Nothing to refresh.".to_string();
            return;
        }
        let paths: Vec<PathBuf> = self.tracks.iter().map(|track| track.path.clone()).collect();
        self.start_scan(ctx.clone(), paths, true);
    }

    fn refresh_selected(&mut self) {
        if self.selected.is_empty() {
            self.status = "Select files to refresh.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let mut refreshed = 0usize;
        for index in indices {
            let Some(path) = self.tracks.get(index).map(|track| track.path.clone()) else {
                continue;
            };
            let track = load_track(&path);
            self.merge_columns(&track);
            self.record_history(&track);
            if index < self.tracks.len() {
                self.tracks[index] = track;
                refreshed += 1;
            }
        }
        self.status = format!("Reloaded {refreshed} file(s).");
    }

    fn unload_all(&mut self) {
        self.tracks.clear();
        self.selected.clear();
        self.selection_anchor = None;
        self.history.clear();
        self.edit_cell = None;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.filter.clear();
        self.filter_input.clear();
        self.status = "Unloaded everything.".to_string();
    }

    fn focus_parent_folder(&mut self) {
        let Some(parent) = self
            .selected
            .iter()
            .next()
            .and_then(|index| self.tracks.get(*index))
            .and_then(|track| track.path.parent())
            .map(Path::to_path_buf)
        else {
            self.status = "Select a file first.".to_string();
            return;
        };
        self.current_dir = Some(parent.clone());
        self.reveal_in_filesystem(&parent);
        self.show_filesystem = true;
        self.filesystem_open = true;
        self.status = format!("Focusing {}.", parent.display());
    }

    fn reveal_in_filesystem(&mut self, path: &Path) {
        self.expanded_dirs.clear();
        for ancestor in path.ancestors().collect::<Vec<_>>().into_iter().rev() {
            if ancestor != Path::new("/") {
                self.expanded_dirs.insert(ancestor.to_path_buf());
            }
        }
        self.expanded_dirs.insert(path.to_path_buf());
        self.fs_reveal_pending = true;
    }

    fn tag_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = self.tag_panel_open;
        let mut visible = self.show_tag_panel;
        dock_header(ui, "Tag Panel", &mut open, &mut visible);
        self.tag_panel_open = open;
        if !visible {
            self.show_tag_panel = false;
            return;
        }
        if !open {
            return;
        }

        let selected: Vec<usize> = self.selected.iter().copied().collect();
        egui::Frame::NONE
            .fill(theme::palette().window_bg)
            .stroke(egui::Stroke::new(1.0, theme::palette().border))
            .inner_margin(egui::Margin::symmetric(4, 4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 2.0);
                self.tag_panel_field(ui, "Artist", "artist", &selected);
                self.tag_panel_field(ui, "Title", "title", &selected);
                self.tag_panel_field(ui, "Album", "album", &selected);
                ui.columns(3, |columns| {
                    self.tag_panel_field(&mut columns[0], "Track", "track", &selected);
                    self.tag_panel_field(&mut columns[1], "Year", "year", &selected);
                    self.tag_panel_field(&mut columns[2], "Genre", "genre", &selected);
                });
                self.tag_panel_field(ui, "Comment", "comment", &selected);
            });
    }

    fn tag_panel_field(&mut self, ui: &mut egui::Ui, label: &str, field: &str, selected: &[usize]) {
        let common = self.common_display(selected, field);
        let mixed = !selected.is_empty() && common.is_none();
        let mut value = common.unwrap_or_default();
        let history = self
            .history
            .get(field)
            .map(|values| values.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let enabled = !selected.is_empty();

        ui.label(egui::RichText::new(label).underline());
        let mut edited = false;
        let mut chosen = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(1.0, 0.0);
            let input_width = (ui.available_width() - 19.0).max(24.0);
            let response = ui
                .add_enabled_ui(enabled, |ui| {
                    ui.add_sized(
                        [input_width, 33.0],
                        egui::TextEdit::singleline(&mut value)
                            .hint_text(if mixed { "<multiple values>" } else { "" })
                            .margin(egui::Margin::symmetric(3, 2)),
                    )
                })
                .inner;
            if response.changed() {
                edited = true;
            }
            let arrow = dropdown_button_height(
                ui,
                enabled,
                "Choose a value from the library or clear this field.",
                33.0,
            );
            if enabled {
                egui::Popup::menu(&arrow).show(|ui| {
                    if ui.button("<keep>").clicked() {
                        ui.close();
                    }
                    if ui.button("<blank>").clicked() {
                        chosen = Some(String::new());
                        ui.close();
                    }
                    if !history.is_empty() {
                        ui.separator();
                        for entry in history.iter().take(100) {
                            if ui.button(entry).clicked() {
                                chosen = Some(entry.clone());
                                ui.close();
                            }
                        }
                    }
                });
            }
        });

        if edited {
            self.apply_field_to_selected(selected, field, value);
        } else if let Some(value) = chosen {
            self.apply_field_to_selected(selected, field, value);
        }
    }

    fn common_display(&self, selected: &[usize], field: &str) -> Option<String> {
        let mut tracks = selected.iter().filter_map(|index| self.tracks.get(*index));
        let first = tracks.next()?;
        let value = ui_display(first, field);
        if tracks.all(|track| ui_display(track, field) == value) {
            Some(value)
        } else {
            None
        }
    }

    fn apply_field_to_selected(&mut self, selected: &[usize], field: &str, value: String) {
        let mut changed = 0usize;
        let mut changes = Vec::new();
        for index in selected {
            let Some(track) = self.tracks.get_mut(*index) else {
                continue;
            };
            if ui_display(track, field) == value {
                continue;
            }
            let before = track.fields.get(field).cloned();
            set_ui_field(track, field, value.clone());
            let after = track.fields.get(field).cloned();
            changes.push(FieldChange {
                row: *index,
                field: field.to_string(),
                before,
                after,
            });
            changed += 1;
        }
        if changed > 0 {
            if !value.is_empty() {
                self.history
                    .entry(field.to_string())
                    .or_default()
                    .insert(value.clone());
            }
            self.push_undo(format!("edit {}", field_title(field)), changes);
            self.status = format!("Modified {changed} file(s).");
        }
    }

    fn filesystem_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = self.filesystem_open;
        let mut visible = self.show_filesystem;
        dock_header(ui, "Filesystem", &mut open, &mut visible);
        self.filesystem_open = open;
        if !visible {
            self.show_filesystem = false;
            return;
        }
        if !open {
            return;
        }

        let checkbox_height = 25.0;
        let tree_height = (ui.available_height() - checkbox_height).max(40.0);
        let current = self.current_dir.clone();
        let mut clicked = None;
        let mut reveal_pending = self.fs_reveal_pending;
        let icons = &self.icons;
        {
            let cache = &mut self.dir_cache;
            let expanded = &mut self.expanded_dirs;
            egui::Frame::NONE
                .fill(theme::palette().table_bg)
                .stroke(egui::Stroke::new(1.0, theme::palette().border))
                .inner_margin(egui::Margin::same(2))
                .show(ui, |ui| {
                    ui.set_min_height(tree_height);
                    set_scrollbar_style(ui);
                    egui::ScrollArea::both()
                        .id_salt("filesystem_tree")
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .max_height(tree_height)
                        .show(ui, |ui| {
                            let roots = dir_children_cached(cache, Path::new("/"));
                            for root in roots {
                                render_dir_tree(
                                    ui,
                                    &root,
                                    0,
                                    cache,
                                    expanded,
                                    current.as_deref(),
                                    icons,
                                    &mut clicked,
                                    &mut reveal_pending,
                                );
                            }
                        });
                });
        }
        self.fs_reveal_pending = reveal_pending;
        if let Some(path) = clicked {
            self.current_dir = Some(path.clone());
            self.reveal_in_filesystem(&path);
            self.start_scan(ui.ctx().clone(), vec![path], true);
        }
        ui.checkbox(&mut self.subfolders, "Subfolders");
    }

    fn filter_panel(&mut self, ui: &mut egui::Ui) {
        let mut open = self.filter_open;
        let mut visible = self.show_filter;
        dock_header(ui, "Filter", &mut open, &mut visible);
        self.filter_open = open;
        if !visible {
            self.show_filter = false;
            return;
        }
        if !open {
            return;
        }

        let mut apply = false;
        let mut clear = false;
        let mut chosen_filter = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(3.0, 0.0);
            ui.label("Filter:");
            let width = (ui.available_width() - 110.0).max(24.0);
            let response = ui.add_sized(
                [width, 29.0],
                egui::TextEdit::singleline(&mut self.filter_input)
                    .hint_text("filename or tag")
                    .margin(egui::Margin::symmetric(3, 2)),
            );
            if response.has_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                apply = true;
            }
            let history = self.filter_history.clone();
            let arrow = dropdown_button_height(ui, true, "Recent filters", 29.0);
            egui::Popup::menu(&arrow).show(|ui| {
                if history.is_empty() {
                    ui.label(egui::RichText::new("No recent filters").weak());
                }
                for entry in history.iter().take(40) {
                    if ui.button(entry).clicked() {
                        chosen_filter = Some(entry.clone());
                        ui.close();
                    }
                }
            });
            let (clear_rect, clear_response) =
                ui.allocate_exact_size(egui::vec2(22.0, 29.0), egui::Sense::click());
            let painter = ui.painter();
            painter.rect_filled(
                clear_rect,
                2.0,
                if clear_response.hovered() {
                    theme::palette().button_hover_bg
                } else {
                    theme::palette().button_bg
                },
            );
            painter.rect_stroke(
                clear_rect,
                2.0,
                egui::Stroke::new(1.0, theme::palette().border),
                egui::StrokeKind::Inside,
            );
            paint_close_icon(
                painter,
                clear_rect.center(),
                3.5,
                theme::palette().error_red,
            );
            if clear_response.clicked() {
                clear = true;
            }
            if ui
                .add_sized([62.0, 29.0], egui::Button::new("Go"))
                .clicked()
            {
                apply = true;
            }
        });
        if let Some(value) = chosen_filter {
            self.filter_input = value;
            apply = true;
        }
        if clear {
            self.filter_input.clear();
            self.filter.clear();
            self.status = "Filter cleared.".to_string();
        } else if apply {
            let filter = self.filter_input.trim().to_string();
            self.filter = filter.clone();
            if !filter.is_empty() && !self.filter_history.iter().any(|entry| entry == &filter) {
                self.filter_history.push(filter.clone());
            }
            self.status = if filter.is_empty() {
                "Filter cleared.".to_string()
            } else {
                format!("Filtering by {filter:?}.")
            };
        }
    }

    fn selection_stats(&self, indices: impl Iterator<Item = usize>) -> (usize, u64, u64) {
        let mut count = 0usize;
        let mut duration = 0u64;
        let mut size = 0u64;
        for index in indices {
            let Some(track) = self.tracks.get(index) else {
                continue;
            };
            count += 1;
            duration += track
                .fields
                .get("__length")
                .and_then(|values| values.first())
                .map(|value| parse_duration(value))
                .unwrap_or(0);
            size += track
                .fields
                .get("__size")
                .and_then(|values| values.first())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
        }
        (count, duration, size)
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let selected = self.selection_stats(self.selected.iter().copied());
        let total = self.selection_stats(0..self.tracks.len());
        ui.horizontal(|ui| {
            if self.scanning {
                ui.spinner();
            }
            let status = if self.scanning {
                self.status.clone()
            } else {
                self.status.clone()
            };
            ui.add(egui::Label::new(status).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{} ({})", total.0, stats_summary(total.1, total.2)));
                ui.add_space(8.0);
                ui.label(format!(
                    "{} ({})",
                    selected.0,
                    stats_summary(selected.1, selected.2)
                ));
                ui.add_space(8.0);
                ui.label(if self.preview_mode {
                    "Preview Mode: On"
                } else {
                    "Preview Mode: Off"
                });
            });
        });
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let needle = self.filter.trim().to_lowercase();
        let mut indices: Vec<usize> = self
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, track)| {
                if needle.is_empty() {
                    return true;
                }
                if track
                    .path
                    .display()
                    .to_string()
                    .to_lowercase()
                    .contains(&needle)
                {
                    return true;
                }
                track.fields.values().any(|values| {
                    values
                        .iter()
                        .any(|value| value.to_lowercase().contains(&needle))
                })
            })
            .map(|(index, _)| index)
            .collect();

        let field = self.sort_field.as_str();
        indices.sort_by(|left, right| {
            let left_value = ui_display(&self.tracks[*left], field).to_lowercase();
            let right_value = ui_display(&self.tracks[*right], field).to_lowercase();
            left_value
                .cmp(&right_value)
                .then_with(|| self.tracks[*left].path.cmp(&self.tracks[*right].path))
        });
        if !self.sort_ascending {
            indices.reverse();
        }
        indices
    }

    fn select_row(&mut self, row: usize, ctrl: bool, shift: bool) {
        if shift {
            if let Some(anchor) = self.selection_anchor {
                if !ctrl {
                    self.selected.clear();
                }
                let (start, end) = if anchor <= row {
                    (anchor, row)
                } else {
                    (row, anchor)
                };
                for index in start..=end {
                    self.selected.insert(index);
                }
            } else {
                self.selected.clear();
                self.selected.insert(row);
                self.selection_anchor = Some(row);
            }
        } else if ctrl {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
            self.selection_anchor = Some(row);
        } else {
            self.selected.clear();
            self.selected.insert(row);
            self.selection_anchor = Some(row);
        }
    }

    fn select_all_visible(&mut self) {
        let indices = self.filtered_indices();
        self.selected = indices.into_iter().collect();
        self.selection_anchor = self.selected.iter().next().copied();
        self.status = format!("Selected {} file(s).", self.selected.len());
    }

    fn invert_selection(&mut self) {
        let indices = self.filtered_indices();
        let previous = self.selected.clone();
        self.selected.clear();
        for index in indices {
            if !previous.contains(&index) {
                self.selected.insert(index);
            }
        }
        self.selection_anchor = self.selected.iter().next().copied();
        self.status = format!("Selected {} file(s).", self.selected.len());
    }

    fn commit_cell_edit(&mut self, edit: CellEdit) {
        let Some(track) = self.tracks.get_mut(edit.row) else {
            return;
        };
        if ui_display(track, &edit.field) == edit.buffer {
            return;
        }
        let before = track.fields.get(&edit.field).cloned();
        set_ui_field(track, &edit.field, edit.buffer.clone());
        let after = track.fields.get(&edit.field).cloned();
        if !edit.buffer.is_empty() {
            self.history
                .entry(edit.field.clone())
                .or_default()
                .insert(edit.buffer);
        }
        self.push_undo(
            format!("edit {}", field_title(&edit.field)),
            vec![FieldChange {
                row: edit.row,
                field: edit.field.clone(),
                before,
                after,
            }],
        );
        self.status = format!("Modified 1 file ({}).", field_title(&edit.field));
    }

    fn table(&mut self, ui: &mut egui::Ui) {
        let columns: Vec<Column> = self
            .columns
            .iter()
            .filter(|column| column.visible)
            .cloned()
            .collect();
        let indices = self.filtered_indices();
        let widths: Vec<f32> = columns
            .iter()
            .map(|column| column_width(&column.field))
            .collect();
        let total_width = TABLE_INDEX_WIDTH + widths.iter().sum::<f32>();
        let lead_row = self
            .selection_anchor
            .filter(|index| self.selected.contains(index))
            .or_else(|| self.selected.iter().next().copied());
        let table_rect = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(table_rect, 0.0, theme::palette().table_bg);

        if columns.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(
                    "No columns selected. Right-click a column header or use Windows > Columns….",
                );
            });
            return;
        }

        let (header_rect, _) = ui.allocate_exact_size(
            egui::vec2(table_rect.width(), TABLE_HEADER_HEIGHT),
            egui::Sense::hover(),
        );
        let body_rect = egui::Rect::from_min_max(
            egui::pos2(table_rect.left(), header_rect.bottom()),
            table_rect.max,
        );

        let mut edit = self.edit_cell.take();
        let mut pending_edit = None;
        let mut pending_selection = None;
        let mut commit_edit = false;
        let mut cancel_edit = false;
        let mut column_clicked = None;
        let mut open_columns = false;
        let body_left = body_rect.left();

        let scroll_output = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(body_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
                |ui| {
                    set_scrollbar_style(ui);
                    egui::ScrollArea::both()
                        .id_salt("tag_table_scroll")
                        .auto_shrink([false, false])
                        .scroll_bar_visibility(
                            egui::scroll_area::ScrollBarVisibility::AlwaysVisible,
                        )
                        .show_rows(ui, TABLE_ROW_HEIGHT, indices.len(), |ui, row_range| {
                            ui.set_min_width(total_width);
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                            for row_position in row_range.clone() {
                                let index = indices[row_position];
                                let (row_rect, row_response) = ui.allocate_exact_size(
                                    egui::vec2(total_width, TABLE_ROW_HEIGHT),
                                    egui::Sense::click(),
                                );
                                let selected = self.selected.contains(&index);
                                let hovered = row_response.hovered();
                                let has_preview = self.preview_mode
                                    && self.tracks[index].fields
                                        != self.tracks[index].original_fields;
                                let background = if selected {
                                    theme::palette().table_selected_bg
                                } else if hovered {
                                    theme::palette().table_hover_bg
                                } else if has_preview {
                                    theme::palette().preview_bg
                                } else if row_position % 2 == 1 {
                                    theme::palette().table_alt_bg
                                } else {
                                    theme::palette().table_bg
                                };
                                let painter = ui.painter().clone();
                                painter.rect_filled(row_rect, 0.0, background);
                                painter.hline(
                                    row_rect.x_range(),
                                    row_rect.bottom() - 0.5,
                                    egui::Stroke::new(1.0, theme::palette().grid_line),
                                );

                                let track = &self.tracks[index];
                                let mut cell_left = row_rect.left() + TABLE_INDEX_WIDTH;
                                for (column, width) in columns.iter().zip(widths.iter()) {
                                    let cell_rect = egui::Rect::from_min_size(
                                        egui::pos2(cell_left, row_rect.top()),
                                        egui::vec2(*width, row_rect.height()),
                                    );
                                    let current_cell = selected
                                        && lead_row == Some(index)
                                        && column.field == "__filename";
                                    if current_cell {
                                        painter.rect_filled(
                                            cell_rect,
                                            0.0,
                                            theme::palette().table_current_cell_bg,
                                        );
                                    }
                                    painter.vline(
                                        cell_left,
                                        row_rect.y_range(),
                                        egui::Stroke::new(1.0, theme::palette().grid_line),
                                    );
                                    let editing = edit.as_ref().is_some_and(|edit| {
                                        edit.row == index && edit.field == column.field
                                    });
                                    if editing {
                                        let active = edit.as_mut().expect("checked above");
                                        let edit_rect = cell_rect.shrink(1.0);
                                        let response = ui.put(
                                            edit_rect,
                                            egui::TextEdit::singleline(&mut active.buffer)
                                                .id_salt((
                                                    "cell-editor",
                                                    index,
                                                    column.field.clone(),
                                                ))
                                                .margin(egui::Margin::symmetric(2, 0))
                                                .font(egui::FontId::proportional(TABLE_FONT_SIZE))
                                                .desired_width(edit_rect.width() - 4.0),
                                        );
                                        if active.request_focus {
                                            response.request_focus();
                                            active.request_focus = false;
                                        }
                                        if response.has_focus()
                                            && ui
                                                .input(|input| input.key_pressed(egui::Key::Escape))
                                        {
                                            cancel_edit = true;
                                        } else if response.lost_focus()
                                            || (response.has_focus()
                                                && ui.input(|input| {
                                                    input.key_pressed(egui::Key::Enter)
                                                }))
                                        {
                                            commit_edit = true;
                                        }
                                    } else {
                                        let value = ui_display(track, &column.field);
                                        let color = if column.field == "__filename"
                                            && track.error.is_some()
                                        {
                                            theme::palette().error_red
                                        } else if current_cell {
                                            egui::Color32::WHITE
                                        } else if has_preview {
                                            theme::palette().preview_text
                                        } else {
                                            theme::palette().text
                                        };
                                        let font = egui::FontId::proportional(TABLE_FONT_SIZE);
                                        let value = elide_text(
                                            ui,
                                            &value,
                                            (cell_rect.width() - 8.0).max(0.0),
                                            &font,
                                        );
                                        let clip = cell_rect.intersect(ui.clip_rect());
                                        painter.with_clip_rect(clip).text(
                                            egui::pos2(
                                                cell_rect.left() + 4.0,
                                                cell_rect.center().y,
                                            ),
                                            egui::Align2::LEFT_CENTER,
                                            value,
                                            font,
                                            color,
                                        );
                                    }
                                    cell_left += *width;
                                }
                                painter.vline(
                                    cell_left,
                                    row_rect.y_range(),
                                    egui::Stroke::new(1.0, theme::palette().border),
                                );

                                if row_response.clicked() {
                                    let modifiers = ui.input(|input| input.modifiers);
                                    pending_selection = Some((
                                        index,
                                        modifiers.command || modifiers.ctrl,
                                        modifiers.shift,
                                    ));
                                }
                                if row_response.double_clicked()
                                    && !(edit.as_ref().is_some_and(|edit| edit.row == index))
                                {
                                    if let Some(pointer) =
                                        ui.input(|input| input.pointer.interact_pos())
                                    {
                                        if pointer.x >= body_left + TABLE_INDEX_WIDTH {
                                            let local =
                                                pointer.x - row_rect.left() - TABLE_INDEX_WIDTH;
                                            if let Some((column, _)) =
                                                column_at(local, &columns, &widths)
                                            {
                                                if !Track::is_read_only(&column.field) {
                                                    pending_edit = Some(CellEdit {
                                                        row: index,
                                                        field: column.field.clone(),
                                                        buffer: ui_display(track, &column.field),
                                                        request_focus: true,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            (row_range.start, row_range.end)
                        })
                },
            )
            .inner;

        let range = scroll_output.inner;
        let offset = scroll_output.state.clamped_offset();
        let inner_rect = scroll_output.inner_rect;

        if let Some((row, ctrl, shift)) = pending_selection {
            self.select_row(row, ctrl, shift);
        }
        if cancel_edit {
            edit = None;
        } else if commit_edit {
            if let Some(active) = edit.take() {
                self.commit_cell_edit(active);
            }
        }
        if let Some(new_edit) = pending_edit {
            if let Some(active) = edit.take() {
                self.commit_cell_edit(active);
            }
            edit = Some(new_edit);
        }
        self.edit_cell = edit;

        if indices.is_empty() {
            let message = if self.tracks.is_empty() {
                "Open audio files or a folder to begin."
            } else {
                "No files match the filter."
            };
            ui.painter().text(
                body_rect.center(),
                egui::Align2::CENTER_CENTER,
                message,
                egui::FontId::proportional(13.0),
                theme::palette().weak_text,
            );
        }

        let row_header_right = inner_rect.left() + TABLE_INDEX_WIDTH;
        let row_header_rect = egui::Rect::from_min_max(
            egui::pos2(inner_rect.left(), inner_rect.top()),
            egui::pos2(row_header_right, inner_rect.bottom()),
        );
        {
            let painter = ui.painter().with_clip_rect(inner_rect);
            painter.rect_filled(row_header_rect, 0.0, theme::palette().table_index_bg);
            for row_position in range.0..range.1 {
                let index = indices[row_position];
                let y = inner_rect.top() + row_position as f32 * TABLE_ROW_HEIGHT - offset.y;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(inner_rect.left(), y),
                    egui::vec2(TABLE_INDEX_WIDTH, TABLE_ROW_HEIGHT),
                );
                if !rect.intersects(inner_rect) {
                    continue;
                }
                let selected = self.selected.contains(&index);
                let background = if selected {
                    theme::palette().table_index_selected_bg
                } else {
                    theme::palette().table_index_bg
                };
                painter.rect_filled(rect, 0.0, background);
                painter.hline(
                    rect.x_range(),
                    rect.bottom() - 0.5,
                    egui::Stroke::new(1.0, theme::palette().grid_line),
                );
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    egui::FontId::proportional(TABLE_FONT_SIZE),
                    theme::palette().text,
                );
            }
            painter.vline(
                row_header_right,
                inner_rect.y_range(),
                egui::Stroke::new(1.0, theme::palette().border),
            );
        }

        {
            let painter = ui.painter_at(header_rect);
            painter.rect_filled(header_rect, 0.0, theme::palette().header_bg);
            painter.hline(
                header_rect.x_range(),
                header_rect.bottom() - 0.5,
                egui::Stroke::new(1.0, theme::palette().border),
            );
            let row_header_right = inner_rect.left() + TABLE_INDEX_WIDTH;
            painter.vline(
                row_header_right,
                header_rect.y_range(),
                egui::Stroke::new(1.0, theme::palette().border),
            );

            let mut cumulative = 0.0f32;
            for (column, width) in columns.iter().zip(widths.iter()) {
                let left = row_header_right - offset.x + cumulative;
                let rect = egui::Rect::from_min_size(
                    egui::pos2(left, header_rect.top()),
                    egui::vec2(*width, header_rect.height()),
                );
                cumulative += *width;
                if rect.right() <= row_header_right || rect.left() >= header_rect.right() {
                    continue;
                }
                let visible = egui::Rect::from_min_max(
                    egui::pos2(rect.left().max(row_header_right), header_rect.top()),
                    egui::pos2(rect.right().min(header_rect.right()), header_rect.bottom()),
                );
                painter.vline(
                    rect.right(),
                    header_rect.y_range(),
                    egui::Stroke::new(1.0, theme::palette().grid_line),
                );
                let font = egui::FontId::proportional(TABLE_FONT_SIZE);
                let sorted = self.sort_field == column.field;
                let title_limit =
                    (visible.width() - 8.0 - if sorted { 14.0 } else { 0.0 }).max(0.0);
                let title = elide_text(ui, &column.title, title_limit, &font);
                painter.with_clip_rect(visible).text(
                    egui::pos2(rect.left() + 5.0, header_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &title,
                    font.clone(),
                    theme::palette().text,
                );
                if sorted {
                    let title_width = ui.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(title, font, theme::palette().text)
                            .size()
                            .x
                    });
                    let marker_x = (rect.left() + 7.0 + title_width + 2.0)
                        .min(visible.right() - 9.0)
                        .max(visible.left() + 7.0);
                    let marker_painter = painter.with_clip_rect(visible);
                    paint_sort_arrow(
                        &marker_painter,
                        egui::pos2(marker_x, header_rect.center().y),
                        self.sort_ascending,
                        theme::palette().text,
                    );
                }
                let response = ui.interact(
                    visible,
                    ui.make_persistent_id(("table-header", column.field.clone())),
                    egui::Sense::click(),
                );
                if response.clicked() {
                    column_clicked = Some(column.field.clone());
                }
                if response.secondary_clicked() {
                    open_columns = true;
                }
            }
        }

        if let Some(field) = column_clicked {
            if self.sort_field == field {
                self.sort_ascending = !self.sort_ascending;
            } else {
                self.sort_field = field;
                self.sort_ascending = true;
            }
        }
        if open_columns {
            self.show_columns = true;
        }
    }

    fn columns_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_columns;
        egui::Window::new("Columns")
            .open(&mut open)
            .default_width(390.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("Checked columns appear in the table. Right-click a column header to reopen this window.");
                ui.label("All tag fields, including fields hidden from the table, can be edited in Edit > Extended Tags.");
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Show all").clicked() {
                        for column in &mut self.columns {
                            column.visible = true;
                        }
                    }
                    if ui.button("Hide all").clicked() {
                        for column in &mut self.columns {
                            column.visible = false;
                        }
                    }
                    if ui.button("Reset").clicked() {
                        let defaults = default_columns();
                        for column in &mut self.columns {
                            column.visible = defaults
                                .iter()
                                .any(|(_, field)| field == &column.field);
                        }
                    }
                });
                ui.separator();
                let mut move_up = None;
                let mut move_down = None;
                egui::ScrollArea::vertical()
                    .id_salt("columns_scroll")
                    .max_height(430.0)
                    .show(ui, |ui| {
                        for index in 0..self.columns.len() {
                            ui.horizontal(|ui| {
                                let column = &mut self.columns[index];
                                ui.checkbox(&mut column.visible, &column.title);
                                if !column.builtin {
                                    ui.label(
                                        egui::RichText::new(&column.field).small().weak(),
                                    );
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if index + 1 < self.columns.len()
                                            && ui.small_button("↓").clicked()
                                        {
                                            move_down = Some(index);
                                        }
                                        if index > 0 && ui.small_button("↑").clicked() {
                                            move_up = Some(index);
                                        }
                                    },
                                );
                            });
                        }
                    });
                if let Some(index) = move_up {
                    self.columns.swap(index, index - 1);
                }
                if let Some(index) = move_down {
                    self.columns.swap(index, index + 1);
                }
            });
        self.show_columns = open;
    }

    fn extended_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_extended;
        egui::Window::new("Extended Tags")
            .open(&mut open)
            .default_width(520.0)
            .default_height(500.0)
            .resizable(true)
            .show(ctx, |ui| {
                let Some(index) = self.selected.iter().next().copied() else {
                    ui.label("Select a file to edit its extended tags.");
                    return;
                };
                let Some(track) = self.tracks.get(index) else {
                    return;
                };
                let path = track.path.clone();
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string();
                let error = track.error.clone();
                let fields: Vec<(String, Vec<String>)> = track
                    .fields
                    .iter()
                    .map(|(field, values)| (field.clone(), values.clone()))
                    .collect();
                ui.heading(name);
                ui.label(egui::RichText::new(path.display().to_string()).small().weak());
                if self.selected.len() > 1 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} files selected; this window edits the first selected file. Use the Tag Panel to edit all selected files.",
                            self.selected.len()
                        ))
                        .small()
                        .weak(),
                    );
                }
                if let Some(error) = error {
                    ui.colored_label(theme::palette().error_red, error);
                }
                ui.separator();

                let mut change = None;
                egui::ScrollArea::vertical()
                    .id_salt("extended_scroll")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 66.0)
                    .show(ui, |ui| {
                        egui::Grid::new("extended_grid")
                            .num_columns(2)
                            .striped(true)
                            .spacing([8.0, 3.0])
                            .show(ui, |ui| {
                                for (field, values) in fields {
                                    ui.label(field_title(&field));
                                    let mut value = values.join("\\\\");
                                    if Track::is_read_only(&field) {
                                        ui.add(
                                            egui::Label::new(if value.is_empty() {
                                                "—"
                                            } else {
                                                &value
                                            })
                                            .truncate()
                                            .sense(egui::Sense::hover()),
                                        );
                                    } else {
                                        let response = ui.add(
                                            egui::TextEdit::singleline(&mut value)
                                                .id_salt(format!("extended-{field}"))
                                                .desired_width(360.0),
                                        );
                                        if response.changed() {
                                            change = Some((field.clone(), value));
                                        }
                                    }
                                    ui.end_row();
                                }
                            });
                    });

                let mut add_field = false;
                let mut remove_empty = false;
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_field)
                            .hint_text("new field name")
                            .desired_width(160.0),
                    );
                    if ui.button("Add field").clicked() {
                        add_field = true;
                    }
                    if ui.button("Remove empty fields").clicked() {
                        remove_empty = true;
                    }
                });
                ui.label(
                    egui::RichText::new("Separate multiple values with \\\\.")
                        .small()
                        .weak(),
                );

                if let Some((field, value)) = change {
                    if let Some(track) = self.tracks.get_mut(index) {
                        track.set_display(&field, value);
                    }
                }
                if add_field {
                    let field = self.new_field.trim().to_ascii_lowercase();
                    if !field.is_empty() && !Track::is_read_only(&field) {
                        if let Some(track) = self.tracks.get_mut(index) {
                            track.fields.entry(field.clone()).or_default();
                            track.dirty = true;
                        }
                        if !self.columns.iter().any(|column| column.field == field) {
                            self.columns.push(Column {
                                title: field_title(&field),
                                field: field.clone(),
                                visible: false,
                                builtin: false,
                            });
                        }
                        self.new_field.clear();
                    }
                }
                if remove_empty {
                    if let Some(track) = self.tracks.get_mut(index) {
                        track.fields.retain(|field, values| {
                            field.starts_with("__") || values.iter().any(|v| !v.is_empty())
                        });
                        track.dirty = true;
                    }
                }
            });
        self.show_extended = open;
    }

    fn selected_macro_names(&self) -> Vec<String> {
        self.checked_actions.iter().cloned().collect()
    }

    fn run_selected_actions(&mut self) {
        let names = self.selected_macro_names();
        if names.is_empty() {
            self.status = "Check at least one action.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let macros: Vec<actions::Macro> = self
            .macros
            .iter()
            .filter(|macro_| names.contains(&macro_.name))
            .cloned()
            .collect();
        let mut changes = Vec::new();
        for index in indices {
            let Some(track) = self.tracks.get_mut(index) else {
                continue;
            };
            let before: BTreeMap<String, Option<Vec<String>>> = track
                .fields
                .iter()
                .map(|(field, values)| (field.clone(), Some(values.clone())))
                .collect();
            for macro_ in &macros {
                actions::apply_macro(macro_, track, None);
            }
            for (field, old) in before.clone() {
                let new = track.fields.get(&field).cloned();
                if old != new {
                    changes.push(FieldChange {
                        row: index,
                        field,
                        before: old,
                        after: new,
                    });
                }
            }
            for field in track.fields.keys().cloned().collect::<Vec<_>>() {
                if !before.contains_key(&field) {
                    let after = track.fields.get(&field).cloned();
                    changes.push(FieldChange {
                        row: index,
                        field,
                        before: None,
                        after,
                    });
                }
            }
        }
        self.push_undo("actions", changes);
        if !self.preview_mode {
            self.save_selected();
        } else {
            self.status = "Actions applied to preview.".to_string();
        }
    }

    fn apply_function_editor(&mut self) {
        let name = self.function_name.clone();
        let fields: Vec<String> = self
            .function_fields
            .split(',')
            .map(|field| field.trim().to_string())
            .filter(|field| !field.is_empty())
            .collect();
        let args: Vec<String> = self
            .function_args
            .split(',')
            .map(|arg| arg.trim().to_string())
            .collect();
        if fields.is_empty() {
            self.status = "Enter at least one field.".to_string();
            return;
        }
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        let mut changes = Vec::new();
        for index in indices {
            let Some(track) = self.tracks.get_mut(index) else {
                continue;
            };
            for field in &fields {
                let before = track.fields.get(field).cloned();
                let value = track.display(field);
                if let Ok(result) = pattern::apply_field_function(&name, &args, &value, track) {
                    if result.is_empty() {
                        track.fields.remove(field);
                    } else {
                        track.fields.insert(field.clone(), vec![result]);
                    }
                    let after = track.fields.get(field).cloned();
                    changes.push(FieldChange {
                        row: index,
                        field: field.clone(),
                        before,
                        after,
                    });
                }
            }
            track.dirty = track.fields != track.original_fields;
        }
        self.push_undo(format!("function {name}"), changes);
        if !self.preview_mode {
            self.save_selected();
        } else {
            self.status = "Function applied to preview.".to_string();
        }
    }

    fn actions_window(&mut self, ctx: &egui::Context) {
        if !self.show_actions {
            return;
        }
        let mut open = true;
        egui::Window::new("Actions")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                if self.macros.is_empty() {
                    ui.label("No action files found in ~/.local/share/puddletag/actions.");
                }
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for macro_ in &self.macros {
                            let mut checked = self.checked_actions.contains(&macro_.name);
                            let mut response = ui.checkbox(&mut checked, &macro_.name);
                            if let Some(filename) = &macro_.filename {
                                response = response.on_hover_text(filename);
                            }
                            if response.changed() {
                                if checked {
                                    self.checked_actions.insert(macro_.name.clone());
                                } else {
                                    self.checked_actions.remove(&macro_.name);
                                }
                            }
                            ui.label(egui::RichText::new(macro_.summary()).small().weak());
                            ui.separator();
                        }
                    });
                ui.add_space(4.0);
                if ui.button("Apply").clicked() {
                    self.run_selected_actions();
                }
            });
        if !open {
            self.show_actions = false;
        }
    }

    fn functions_window(&mut self, ctx: &egui::Context) {
        if !self.show_functions {
            return;
        }
        let mut open = true;
        egui::Window::new("Functions")
            .open(&mut open)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Function:");
                    egui::ComboBox::from_id_salt("function-name")
                        .selected_text(&self.function_name)
                        .show_ui(ui, |ui| {
                            for name in actions::function_names() {
                                ui.selectable_value(
                                    &mut self.function_name,
                                    name.to_string(),
                                    *name,
                                );
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Fields:");
                    ui.text_edit_singleline(&mut self.function_fields);
                });
                ui.horizontal(|ui| {
                    ui.label("Arguments:");
                    ui.text_edit_singleline(&mut self.function_args);
                });
                ui.label(
                    egui::RichText::new(
                        "Arguments are comma-separated. Use %field% patterns for format().",
                    )
                    .small()
                    .weak(),
                );
                ui.add_space(4.0);
                if ui.button("Apply").clicked() {
                    self.apply_function_editor();
                }
            });
        if !open {
            self.show_functions = false;
        }
    }

    fn open_convert(&mut self, kind: ConvertKind) {
        let pattern = match kind {
            ConvertKind::RenameDirs => "%album%".to_string(),
            _ => "%artist% - %title%".to_string(),
        };
        self.convert_dialog = Some(ConvertDialog {
            kind,
            pattern,
            field: "title".to_string(),
        });
    }

    fn run_convert(&mut self) {
        let Some(dialog) = self.convert_dialog.take() else {
            return;
        };
        let indices: Vec<usize> = self.selected.iter().copied().collect();
        if indices.is_empty() {
            self.status = "Select files first.".to_string();
            return;
        }
        match dialog.kind {
            ConvertKind::FileToTag => {
                let mut changes = Vec::new();
                for index in indices {
                    let Some(track) = self.tracks.get_mut(index) else {
                        continue;
                    };
                    let filename = track.path.display().to_string();
                    let parsed = pattern::filename_to_tag(&dialog.pattern, &filename);
                    for (field, values) in parsed {
                        let before = track.fields.get(&field).cloned();
                        if let Some(value) = values.first() {
                            track.fields.insert(field.clone(), vec![value.clone()]);
                        }
                        let after = track.fields.get(&field).cloned();
                        changes.push(FieldChange {
                            row: index,
                            field,
                            before,
                            after,
                        });
                    }
                    track.dirty = track.fields != track.original_fields;
                }
                self.push_undo("File->Tag", changes);
                if !self.preview_mode {
                    self.save_selected();
                } else {
                    self.status = "File->Tag applied to preview.".to_string();
                }
            }
            ConvertKind::TagToFile => {
                let mut renamed = 0usize;
                let mut failed = 0usize;
                for index in indices {
                    let Some(track) = self.tracks.get(index) else {
                        continue;
                    };
                    match pattern::tag_to_filename(&dialog.pattern, track, true, false) {
                        Ok(new_path) if new_path != track.path => {
                            if new_path.exists() {
                                failed += 1;
                                continue;
                            }
                            if let Some(parent) = new_path.parent() {
                                let _ = fs::create_dir_all(parent);
                            }
                            match fs::rename(&track.path, &new_path) {
                                Ok(()) => {
                                    self.tracks[index] = load_track(&new_path);
                                    renamed += 1;
                                }
                                Err(_) => failed += 1,
                            }
                        }
                        Ok(_) => {}
                        Err(_) => failed += 1,
                    }
                }
                self.status = if failed > 0 {
                    format!("Renamed {renamed} file(s); {failed} failed.")
                } else {
                    format!("Renamed {renamed} file(s).")
                };
            }
            ConvertKind::Format => {
                let mut changes = Vec::new();
                for index in indices {
                    let Some(track) = self.tracks.get_mut(index) else {
                        continue;
                    };
                    let Ok(rendered) = pattern::render(&dialog.pattern, track) else {
                        continue;
                    };
                    let before = track.fields.get(&dialog.field).cloned();
                    if rendered.is_empty() {
                        track.fields.remove(&dialog.field);
                    } else {
                        track.fields.insert(dialog.field.clone(), vec![rendered]);
                    }
                    changes.push(FieldChange {
                        row: index,
                        field: dialog.field.clone(),
                        before,
                        after: track.fields.get(&dialog.field).cloned(),
                    });
                    track.dirty = track.fields != track.original_fields;
                }
                self.push_undo("Format", changes);
                if !self.preview_mode {
                    self.save_selected();
                } else {
                    self.status = "Format applied to preview.".to_string();
                }
            }
            ConvertKind::RenameDirs => {
                let mut renames = Vec::new();
                for index in &indices {
                    let Some(track) = self.tracks.get(*index) else {
                        continue;
                    };
                    if let Ok(new_dir) =
                        pattern::tag_to_filename(&dialog.pattern, track, false, true)
                    {
                        if let Some(old_dir) = track.path.parent() {
                            if new_dir != old_dir {
                                renames.push((old_dir.to_path_buf(), new_dir));
                            }
                        }
                    }
                }
                renames.sort();
                renames.dedup();
                let mut renamed = 0usize;
                for (old_dir, new_dir) in renames {
                    if new_dir.exists() {
                        continue;
                    }
                    if let Some(parent) = new_dir.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::rename(&old_dir, &new_dir).is_ok() {
                        renamed += 1;
                        for track in &mut self.tracks {
                            if let Ok(rest) = track.path.strip_prefix(&old_dir) {
                                let new_path = new_dir.join(rest);
                                *track = load_track(&new_path);
                            }
                        }
                    }
                }
                self.status = format!("Renamed {renamed} directory(ies).");
            }
        }
    }

    fn convert_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.convert_dialog.as_mut() else {
            return;
        };
        let mut open = true;
        let mut apply = false;
        let title = match dialog.kind {
            ConvertKind::FileToTag => "File->Tag",
            ConvertKind::TagToFile => "Tag->File",
            ConvertKind::Format => "Format",
            ConvertKind::RenameDirs => "Rename Directories",
        };
        egui::Window::new(title)
            .open(&mut open)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.label("Pattern:");
                ui.text_edit_singleline(&mut dialog.pattern);
                if dialog.kind == ConvertKind::Format {
                    ui.horizontal(|ui| {
                        ui.label("Field:");
                        ui.text_edit_singleline(&mut dialog.field);
                    });
                }
                ui.add_space(4.0);
                if ui.button("Apply").clicked() {
                    apply = true;
                }
            });
        if apply {
            self.run_convert();
        } else if !open {
            self.convert_dialog = None;
        }
    }

    fn log(&mut self, message: impl Into<String>) {
        self.logs.push(message.into());
        if self.logs.len() > 5000 {
            self.logs.remove(0);
        }
    }

    fn load_playlist(&mut self, ctx: &egui::Context) {
        let file = rfd::FileDialog::new()
            .add_filter("M3U playlist", &["m3u", "m3u8"])
            .pick_file();
        let Some(file) = file else {
            return;
        };
        match playlist::read(&file) {
            Ok(files) => {
                let count = files.len();
                self.log(format!("Loading playlist {}", file.display()));
                self.start_scan(ctx.clone(), files, true);
                self.status = format!("Loading {count} playlist entries…");
            }
            Err(err) => {
                self.status = format!("Playlist error: {err:#}");
                self.log(format!("Playlist error: {err:#}"));
            }
        }
    }

    fn save_playlist(&mut self) {
        let indices: Vec<usize> = if self.selected.is_empty() {
            (0..self.tracks.len()).collect()
        } else {
            self.selected.iter().copied().collect()
        };
        if indices.is_empty() {
            self.status = "No files to save.".to_string();
            return;
        }
        let default_name = self
            .config
            .string("playlist", "filepattern", "PuddleMore.m3u");
        let file = rfd::FileDialog::new()
            .add_filter("M3U playlist", &["m3u"])
            .set_file_name(default_name)
            .save_file();
        let Some(file) = file else {
            return;
        };
        let pattern = self
            .config
            .string("playlist", "extpattern", "%artist% - %title%");
        let options = playlist::PlaylistOptions {
            extended: self.config.bool("playlist", "extinfo", true),
            relative: self.config.bool("playlist", "reldir", false),
            windows_separator: self.config.bool("playlist", "windows_separator", false),
        };
        let tracks: Vec<(PathBuf, u64, String)> = indices
            .iter()
            .filter_map(|index| self.tracks.get(*index))
            .map(|track| {
                let seconds = parse_duration(&track.display("__length"));
                let title =
                    pattern::render(&pattern, track).unwrap_or_else(|_| track.display("title"));
                (track.path.clone(), seconds, title)
            })
            .collect();
        match playlist::write(&file, &tracks, &options) {
            Ok(()) => {
                self.status = format!("Saved playlist {}.", file.display());
                self.log(format!("Saved playlist {}", file.display()));
            }
            Err(err) => {
                self.status = format!("Playlist error: {err:#}");
                self.log(format!("Playlist error: {err:#}"));
            }
        }
    }

    fn open_export(&mut self) {
        let template = export::load_template();
        let preset = match template.as_str() {
            value if value == export::Preset::Html.template() => 1,
            value if value == export::Preset::Rtf.template() => 2,
            value if value == export::Preset::Txt.template() => 3,
            value if value == export::Preset::M3u.template() => 4,
            _ => 0,
        };
        self.export_dialog = Some(ExportDialog { template, preset });
    }

    fn export_window(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.export_dialog.as_mut() else {
            return;
        };
        let mut open = true;
        let mut save = false;
        egui::Window::new("Export")
            .open(&mut open)
            .default_width(720.0)
            .default_height(500.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Preset:");
                    let before = dialog.preset;
                    egui::ComboBox::from_id_salt("export-preset")
                        .selected_text(export::Preset::Csv.label())
                        .show_ui(ui, |ui| {
                            for (index, preset) in [
                                export::Preset::Csv,
                                export::Preset::Html,
                                export::Preset::Rtf,
                                export::Preset::Txt,
                                export::Preset::M3u,
                            ]
                            .iter()
                            .enumerate()
                            {
                                ui.selectable_value(&mut dialog.preset, index, preset.label());
                            }
                        });
                    if before != dialog.preset {
                        dialog.template = [
                            export::Preset::Csv,
                            export::Preset::Html,
                            export::Preset::Rtf,
                            export::Preset::Txt,
                            export::Preset::M3u,
                        ][dialog.preset]
                            .template()
                            .to_string();
                    }
                });
                ui.label("Format:");
                ui.add(
                    egui::TextEdit::multiline(&mut dialog.template)
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                ui.separator();
                ui.label("Preview:");
                let preview_tracks: Vec<Track> = self.tracks.iter().take(20).cloned().collect();
                let preview = export::render(&dialog.template, &preview_tracks)
                    .unwrap_or_else(|err| format!("Error: {err:#}"));
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut preview.as_str())
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                ui.add_space(4.0);
                if ui.button("Save export file…").clicked() {
                    save = true;
                }
            });
        if save {
            let template = dialog.template.clone();
            let _ = export::save_template(&template);
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("All files", &["*"])
                .set_file_name("export.txt")
                .save_file()
            {
                match export::render(&template, &self.tracks) {
                    Ok(contents) => match export::save(&file, &contents) {
                        Ok(()) => {
                            self.status = format!("Exported {}.", file.display());
                            self.log(format!("Exported {}", file.display()));
                        }
                        Err(err) => {
                            self.status = format!("Export error: {err:#}");
                        }
                    },
                    Err(err) => self.status = format!("Export error: {err:#}"),
                }
            }
            self.export_dialog = None;
        } else if !open {
            let template = dialog.template.clone();
            let _ = export::save_template(&template);
            self.export_dialog = None;
        }
    }

    fn logs_window(&mut self, ctx: &egui::Context) {
        if !self.show_logs {
            return;
        }
        let mut open = true;
        egui::Window::new("Logs")
            .open(&mut open)
            .default_width(640.0)
            .default_height(300.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Copy").clicked() {
                        ctx.copy_text(self.logs.join("\n"));
                    }
                    if ui.button("Clear").clicked() {
                        self.logs.clear();
                    }
                });
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.label(line);
                        }
                    });
            });
        if !open {
            self.show_logs = false;
        }
    }

    fn stored_tags_window(&mut self, ctx: &egui::Context) {
        if !self.show_stored_tags {
            return;
        }
        let mut open = true;
        egui::Window::new("Stored Tags")
            .open(&mut open)
            .default_width(460.0)
            .default_height(360.0)
            .show(ctx, |ui| {
                let Some(track) = self
                    .selected
                    .iter()
                    .next()
                    .and_then(|index| self.tracks.get(*index))
                else {
                    ui.label("Select a file first.");
                    return;
                };
                ui.heading(
                    track
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                );
                ui.label(
                    egui::RichText::new(track.path.display().to_string())
                        .small()
                        .weak(),
                );
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (field, values) in &track.fields {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(field_title(field)).strong());
                            ui.label(values.join("\\"));
                        });
                    }
                });
            });
        if !open {
            self.show_stored_tags = false;
        }
    }

    fn delete_window(&mut self, ctx: &egui::Context) {
        if !self.confirm_delete {
            return;
        }
        let mut open = true;
        let mut delete = false;
        let count = self.selected.len();
        egui::Window::new("Delete files")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(format!("Delete {count} selected file(s)?"));
                ui.label("This cannot be undone.");
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        delete = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_delete = false;
                    }
                });
            });
        if !open {
            self.confirm_delete = false;
        }
        if delete {
            let paths: Vec<PathBuf> = self
                .selected
                .iter()
                .filter_map(|index| self.tracks.get(*index).map(|track| track.path.clone()))
                .collect();
            let mut deleted = 0usize;
            for path in &paths {
                if fs::remove_file(path).is_ok() {
                    deleted += 1;
                }
            }
            let set: BTreeSet<PathBuf> = paths.into_iter().collect();
            self.tracks.retain(|track| !set.contains(&track.path));
            self.selected.clear();
            self.confirm_delete = false;
            self.status = format!("Deleted {deleted} file(s).");
            self.log(format!("Deleted {deleted} file(s)."));
        }
    }

    fn artwork_window(&mut self, ctx: &egui::Context) {
        if !self.show_artwork {
            return;
        }
        let mut open = true;
        egui::Window::new("Artwork")
            .open(&mut open)
            .default_width(360.0)
            .default_height(420.0)
            .show(ctx, |ui| {
                let Some(index) = self.selected.iter().next().copied() else {
                    ui.label("Select a file first.");
                    return;
                };
                let Some(track) = self.tracks.get_mut(index) else {
                    return;
                };
                let image = track.images.first().cloned();
                if let Some(image) = &image {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    image.data.hash(&mut hasher);
                    let key = hasher.finish();
                    let refresh = self
                        .artwork_cache
                        .as_ref()
                        .map(|(cached, _, _)| *cached != key)
                        .unwrap_or(true);
                    if refresh {
                        if let Ok(decoded) = image::load_from_memory(&image.data) {
                            let rgba = decoded.to_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            let color =
                                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                            let texture = ctx.load_texture(
                                format!("artwork-{key}"),
                                color,
                                egui::TextureOptions::LINEAR,
                            );
                            self.artwork_cache = Some((key, texture, size));
                        }
                    }
                    if let Some((_, texture, size)) = &self.artwork_cache {
                        let max_width = ui.available_width().min(320.0);
                        let scale = (max_width / size[0] as f32).min(1.0);
                        ui.image((
                            texture.id(),
                            egui::vec2(size[0] as f32 * scale, size[1] as f32 * scale),
                        ));
                    }
                    ui.label(format!(
                        "{} × {} · {} · {}",
                        size_label(&image.data),
                        image.mime,
                        type_name(image.image_type),
                        image.data.len()
                    ));
                    ui.label(&image.description);
                } else {
                    ui.label("No embedded artwork.");
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("Add / Replace…").clicked() {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter(
                                "Images",
                                &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff"],
                            )
                            .pick_file()
                        {
                            if let Ok(data) = fs::read(&file) {
                                let mime = crate::metadata::sniff_image_mime(&data);
                                track.images = vec![crate::metadata::ImageData {
                                    data,
                                    mime,
                                    description: String::new(),
                                    image_type: 3,
                                }];
                                track.dirty = true;
                                self.artwork_cache = None;
                            }
                        }
                    }
                    if image.is_some() && ui.button("Remove").clicked() {
                        track.images.clear();
                        track.dirty = true;
                        self.artwork_cache = None;
                    }
                    if let Some(image) = &image {
                        if ui.button("Save Image…").clicked() {
                            if let Some(file) =
                                rfd::FileDialog::new().set_file_name("cover").save_file()
                            {
                                let _ = fs::write(file, &image.data);
                            }
                        }
                    }
                });
            });
        if !open {
            self.show_artwork = false;
        }
    }

    fn apply_current_theme(&mut self, ctx: &egui::Context) {
        let selection = self
            .config
            .value("table", "selected_color")
            .is_some()
            .then(|| self.config.selected_color([0x2f, 0x7e, 0xc7]));
        let preview = self
            .config
            .value("table", "preview_color")
            .is_some()
            .then(|| self.config.preview_color([0xc0, 0xff, 0xc0]));
        self.resolved_theme = theme::apply(ctx, self.theme_choice, selection, preview);
    }

    fn preferences_window(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }
        let mut open = true;
        let mut accepted = false;
        let mut cancelled = false;
        egui::Window::new("PuddleMore settings")
            .open(&mut open)
            .resizable(true)
            .default_width(640.0)
            .default_height(430.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(150.0);
                        ui.heading("Preferences");
                        ui.add_space(4.0);
                        let _ = ui.selectable_label(true, "General");
                        ui.add_space(2.0);
                        let _ = ui.selectable_label(false, "Appearance");
                        ui.add_space(2.0);
                        let _ = ui.selectable_label(false, "Playlist");
                        ui.add_space(2.0);
                        let _ = ui.selectable_label(false, "Tags");
                        ui.add_space(2.0);
                        let _ = ui.selectable_label(false, "Columns");
                        ui.add_space(2.0);
                        let _ = ui.selectable_label(false, "Tag Panel");
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.heading("Appearance");
                        ui.label("Choose a high-contrast palette. System follows your desktop's light or dark preference.");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Colour theme:");
                            let before = self.theme_choice;
                            egui::ComboBox::from_id_salt("theme-choice")
                                .selected_text(self.theme_choice.label())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.theme_choice,
                                        theme::ThemeChoice::System,
                                        "System",
                                    );
                                    ui.selectable_value(
                                        &mut self.theme_choice,
                                        theme::ThemeChoice::Light,
                                        "Light",
                                    );
                                    ui.selectable_value(
                                        &mut self.theme_choice,
                                        theme::ThemeChoice::Dark,
                                        "Dark",
                                    );
                                });
                            if before != self.theme_choice {
                                self.apply_current_theme(ctx);
                            }
                        });
                        ui.add_space(8.0);
                        let palette = theme::palette();
                        egui::Frame::NONE
                            .fill(palette.window_bg)
                            .stroke(egui::Stroke::new(1.0, palette.border))
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("PuddleMore preview")
                                        .strong()
                                        .color(palette.text),
                                );
                                ui.label(
                                    egui::RichText::new(
                                        "Tags, menus and dialogs use the same palette.",
                                    )
                                    .color(palette.text),
                                );
                                ui.add_space(4.0);
                                egui::Frame::NONE
                                    .fill(palette.table_selected_bg)
                                    .inner_margin(egui::Margin::symmetric(6, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new("Selected row")
                                                .color(palette.selected_text),
                                        );
                                    });
                            });
                        ui.add_space(8.0);
                        ui.label("Theme changes are applied immediately.");
                        ui.add_space(10.0);
                        ui.separator();
                        ui.heading("General");
                        let mut subfolders = self.config.subfolders();
                        let mut grid = self.config.show_gridlines();
                        let mut tips = self.config.show_tooltips();
                        let mut rows = self.config.show_row_numbers();
                        let mut resize = self.config.autoresize_columns();
                        let mut mtime = self.config.preserve_mtime();
                        let mut last = self.config.load_last_folder();
                        if ui.checkbox(&mut subfolders, "Subfolders").changed() {
                            let _ = self.config.set_general("Su&bfolders", serde_json::json!(subfolders));
                        }
                        if ui.checkbox(&mut grid, "Show gridlines").changed() {
                            let _ = self.config.set_general("Show &gridlines", serde_json::json!(grid));
                        }
                        if ui.checkbox(&mut tips, "Show tooltips in file view").changed() {
                            let _ = self.config.set_general("Show tooltips in file-view:", serde_json::json!(tips));
                        }
                        if ui.checkbox(&mut rows, "Show row numbers").changed() {
                            let _ = self.config.set_general("Show &row numbers", serde_json::json!(rows));
                        }
                        if ui
                            .checkbox(&mut resize, "Automatically resize columns to contents")
                            .changed()
                        {
                            let _ = self.config.set_general(
                                "Automatically resize columns to contents",
                                serde_json::json!(resize),
                            );
                        }
                        if ui
                            .checkbox(&mut mtime, "Preserve file modification times")
                            .changed()
                        {
                            let _ = self.config.set_general(
                                "&Preserve file modification times (if supported)",
                                serde_json::json!(mtime),
                            );
                        }
                        if ui.checkbox(&mut last, "Load last folder at startup").changed() {
                            let _ = self
                                .config
                                .set_general("&Load last folder at startup", serde_json::json!(last));
                        }
                        self.subfolders = subfolders;
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        accepted = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });
        if accepted {
            let _ = self.config.set_theme(self.theme_choice.config_value());
            self.preferences_theme_before = self.theme_choice;
            self.show_preferences = false;
            self.apply_current_theme(ctx);
            self.status = "Settings saved.".to_string();
        } else if cancelled || !open {
            self.theme_choice = self.preferences_theme_before;
            self.config = config::Config::load();
            self.subfolders = self.config.subfolders();
            self.apply_current_theme(ctx);
            self.show_preferences = false;
        }
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("About PuddleMore")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.heading("PuddleMore");
                ui.label(format!("Version {VERSION}"));
                ui.label("Audio tag editor");
                ui.add_space(6.0);
                ui.label(
                    "Read and write tags for MP3, FLAC, Ogg, Opus, MP4/M4A, WAV, AIFF, \
                     WavPack, TrueAudio, WMA/ASF, Matroska and related formats.",
                );
                ui.add_space(6.0);
                ui.label("License: GPL-3.0-or-later");
            });
        self.show_about = open;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) -> Option<UiAction> {
        let mut action = None;
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if menu_item(ui, "Open Folder…", "Ctrl+O", true) {
                    action = Some(UiAction::OpenFolder);
                    ui.close();
                }
                if menu_item(ui, "Open Files…", "Ctrl+Shift+O", true) {
                    action = Some(UiAction::OpenFiles);
                    ui.close();
                }
                if menu_item(ui, "Add Folder…", "", true) {
                    action = Some(UiAction::AddFolder);
                    ui.close();
                }
                ui.separator();
                if menu_item(ui, "Load playlist…", "", true) {
                    action = Some(UiAction::LoadPlaylist);
                }
                if menu_item(ui, "Save playlist…", "", !self.tracks.is_empty()) {
                    action = Some(UiAction::SavePlaylist);
                }
                ui.separator();
                if menu_item(
                    ui,
                    "Save",
                    "Ctrl+S",
                    !self.selected.is_empty() && !self.scanning,
                ) {
                    action = Some(UiAction::Save);
                    ui.close();
                }
                if menu_item(ui, "Export…", "", !self.tracks.is_empty()) {
                    action = Some(UiAction::ExportTags);
                }
                ui.separator();
                if menu_item(ui, "Delete", "Del", !self.selected.is_empty()) {
                    action = Some(UiAction::DeleteFiles);
                }
                ui.separator();
                if menu_item(ui, "Refresh", "F5", !self.tracks.is_empty()) {
                    action = Some(UiAction::Refresh);
                    ui.close();
                }
                if menu_item(ui, "Refresh Selected", "", !self.selected.is_empty()) {
                    action = Some(UiAction::RefreshSelected);
                    ui.close();
                }
                if menu_item(ui, "Unload Everything", "", !self.tracks.is_empty()) {
                    action = Some(UiAction::Unload);
                    ui.close();
                }
                ui.separator();
                if menu_item(ui, "Exit", "", true) {
                    action = Some(UiAction::Exit);
                    ui.close();
                }
            });

            ui.menu_button("Edit", |ui| {
                if menu_item(ui, "Undo", "Ctrl+Z", !self.undo_stack.is_empty()) {
                    action = Some(UiAction::Undo);
                }
                if menu_item(ui, "Redo", "Ctrl+Shift+Z", !self.redo_stack.is_empty()) {
                    action = Some(UiAction::Redo);
                }
                if menu_item(ui, "Extended Tags", "", !self.selected.is_empty()) {
                    action = Some(UiAction::ExtendedTags);
                    ui.close();
                }
                ui.separator();
                if menu_item(ui, "Invert Selection", "", !self.tracks.is_empty()) {
                    action = Some(UiAction::InvertSelection);
                    ui.close();
                }
                if menu_item(ui, "Select All", "Ctrl+A", !self.tracks.is_empty()) {
                    action = Some(UiAction::SelectAll);
                    ui.close();
                }
                ui.separator();
                if menu_item(ui, "Preferences", "", true) {
                    action = Some(UiAction::Preferences);
                }
            });

            ui.menu_button("Convert", |ui| {
                let enabled = !self.selected.is_empty();
                if menu_item(ui, "File->Tag", "Ctrl+T", enabled) {
                    action = Some(UiAction::ConvertFileToTag);
                }
                if menu_item(ui, "Tag->File", "", enabled) {
                    action = Some(UiAction::ConvertTagToFile);
                }
                if menu_item(ui, "Format", "", enabled) {
                    action = Some(UiAction::ConvertFormat);
                }
                ui.separator();
                if menu_item(ui, "Rename Directories", "", enabled) {
                    action = Some(UiAction::ConvertRenameDirs);
                }
            });

            ui.menu_button("Actions", |ui| {
                if menu_item(ui, "Go to parent folder", "", !self.selected.is_empty()) {
                    action = Some(UiAction::GoToParent);
                    ui.close();
                }
                let enabled = !self.selected.is_empty();
                if menu_item(ui, "Actions", "", enabled) {
                    action = Some(UiAction::ShowActions);
                }
                if menu_item(ui, "Functions", "", enabled) {
                    action = Some(UiAction::ShowFunctions);
                }
            });

            ui.menu_button("Preview Mode", |ui| {
                let toggle_label = if self.preview_mode {
                    "Disable Preview Mode"
                } else {
                    "Enable Preview Mode"
                };
                if menu_item(ui, toggle_label, "Ctrl+Shift+P", true) {
                    action = Some(UiAction::TogglePreview);
                }
                if menu_item(ui, "Write Previews", "Ctrl+W", self.preview_mode) {
                    action = Some(UiAction::WritePreviews);
                }
                if menu_item(ui, "Clear Selected Files", "", self.preview_mode) {
                    action = Some(UiAction::ClearPreview);
                }
                if menu_item(ui, "Clear Selected Cells", "", self.preview_mode) {
                    action = Some(UiAction::ClearPreviewCells);
                }
            });

            ui.menu_button("Tag Tools", |ui| {
                let enabled = !self.selected.is_empty() && !self.preview_mode;
                if menu_item(ui, "Remove APEv2 Tag", "", enabled) {
                    action = Some(UiAction::RemoveApev2);
                }
                if menu_item(ui, "Remove All ID3 Tags", "", enabled) {
                    action = Some(UiAction::RemoveId3);
                }
                if menu_item(ui, "Remove ID3v2 Tag", "", enabled) {
                    action = Some(UiAction::RemoveId3v2);
                }
                if menu_item(ui, "Remove ID3v1 Tag", "", enabled) {
                    action = Some(UiAction::RemoveId3v1);
                }
            });

            ui.menu_button("Windows", |ui| {
                if toggle_menu_item(ui, "Filter", self.show_filter) {
                    action = Some(UiAction::ToggleFilter);
                    ui.close();
                }
                if toggle_menu_item(ui, "Tag Panel", self.show_tag_panel) {
                    action = Some(UiAction::ToggleTagPanel);
                    ui.close();
                }
                if toggle_menu_item(ui, "Artwork", self.show_artwork) {
                    action = Some(UiAction::ToggleArtwork);
                }
                if toggle_menu_item(ui, "Filesystem", self.show_filesystem) {
                    action = Some(UiAction::ToggleFilesystem);
                    ui.close();
                }
                if toggle_menu_item(ui, "Stored Tags", self.show_stored_tags) {
                    action = Some(UiAction::ToggleStoredTags);
                }
                if toggle_menu_item(ui, "Logs", self.show_logs) {
                    action = Some(UiAction::ToggleLogs);
                }
                ui.separator();
                if menu_item(ui, "Columns…", "", true) {
                    action = Some(UiAction::Columns);
                    ui.close();
                }
            });

            ui.menu_button("Help", |ui| {
                if menu_item(ui, "About PuddleMore", "", true) {
                    action = Some(UiAction::About);
                    ui.close();
                }
            });
        });
        action
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) -> Option<UiAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
            ui.add_space(3.0);
            if toolbar_icon(ui, &self.icons.folder, "Open Folder (Ctrl+O)", true) {
                action = Some(UiAction::OpenFolder);
            }
            if toolbar_icon(ui, &self.icons.folder_new, "Add Folder", true) {
                action = Some(UiAction::AddFolder);
            }
            if toolbar_icon(
                ui,
                &self.icons.refresh,
                "Refresh (F5)",
                !self.tracks.is_empty() && !self.scanning,
            ) {
                action = Some(UiAction::Refresh);
            }
            ui.separator();
            if toolbar_icon(
                ui,
                &self.icons.save,
                "Save selected (Ctrl+S)",
                !self.selected.is_empty() && !self.scanning,
            ) {
                action = Some(UiAction::Save);
            }
            if toolbar_icon(
                ui,
                &self.icons.undo,
                "Undo (Ctrl+Z)",
                !self.undo_stack.is_empty(),
            ) {
                action = Some(UiAction::Undo);
            }
            ui.separator();

            let _ = ui.add_sized(
                [88.0, 21.0],
                egui::TextEdit::singleline(&mut self.pattern)
                    .hint_text("%title%")
                    .margin(egui::Margin::symmetric(3, 1)),
            );
            let patterns = [
                "%title%",
                "%artist% - %title%",
                "%track% - %title%",
                "%album% - %title%",
                "%artist%/%album%/%track% %title%",
            ];
            let arrow = dropdown_button(ui, true, "Pattern presets");
            let mut chosen_pattern = None;
            egui::Popup::menu(&arrow).show(|ui| {
                for pattern in patterns {
                    if ui.button(pattern).clicked() {
                        chosen_pattern = Some(pattern.to_string());
                        ui.close();
                    }
                }
            });
            if let Some(pattern) = chosen_pattern {
                self.pattern = pattern;
            }

            let enabled = !self.selected.is_empty();
            if toolbar_icon(
                ui,
                &self.icons.file_to_tag,
                "File->Tag (using the pattern)",
                enabled,
            ) {
                self.open_convert(ConvertKind::FileToTag);
                if let Some(dialog) = self.convert_dialog.as_mut() {
                    dialog.pattern = self.pattern.clone();
                }
            }
            if toolbar_icon(
                ui,
                &self.icons.tag_to_file,
                "Tag->File (using the pattern)",
                enabled,
            ) {
                self.open_convert(ConvertKind::TagToFile);
                if let Some(dialog) = self.convert_dialog.as_mut() {
                    dialog.pattern = self.pattern.clone();
                }
            }
            if toolbar_icon_text(
                ui,
                Some(&self.icons.format),
                "Format",
                "Format the selected fields using the pattern",
                enabled,
            ) {
                self.open_convert(ConvertKind::Format);
                if let Some(dialog) = self.convert_dialog.as_mut() {
                    dialog.pattern = self.pattern.clone();
                }
            }
            if toolbar_icon(ui, &self.icons.rename_dirs, "Rename Directories", enabled) {
                self.open_convert(ConvertKind::RenameDirs);
            }
            ui.separator();
            if toolbar_icon(
                ui,
                &self.icons.parent,
                "Go to parent folder",
                !self.selected.is_empty(),
            ) {
                action = Some(UiAction::GoToParent);
            }
            if toolbar_icon(ui, &self.icons.actions, "Actions", enabled) {
                self.macros = actions::load_macros();
                self.show_actions = true;
            }
            if toolbar_icon(ui, &self.icons.quick_actions, "QuickActions", enabled) {
                self.macros = actions::load_macros();
                self.show_actions = true;
            }
            if toolbar_text_button(ui, "Functions", "Run a function", enabled) {
                self.show_functions = true;
            }
        });
        action
    }

    fn left_docks(&mut self, ui: &mut egui::Ui) {
        if self.show_tag_panel {
            egui::Panel::top("tag_dock")
                .resizable(false)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::palette().window_bg)
                        .inner_margin(egui::Margin::symmetric(4, 2)),
                )
                .show(ui, |ui| self.tag_panel(ui));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(theme::palette().window_bg)
                    .inner_margin(egui::Margin::symmetric(4, 2)),
            )
            .show(ui, |ui| {
                if self.show_filesystem {
                    self.filesystem_panel(ui);
                }
            });
    }

    fn handle_action(&mut self, action: UiAction, ctx: &egui::Context) {
        match action {
            UiAction::OpenFolder => self.open_folder(ctx, false),
            UiAction::OpenFiles => self.open_files(ctx),
            UiAction::AddFolder => self.open_folder(ctx, true),
            UiAction::Save => {
                if self.preview_mode {
                    self.write_previews();
                } else {
                    self.save_selected();
                }
            }
            UiAction::Refresh => self.refresh_all(ctx),
            UiAction::RefreshSelected => self.refresh_selected(),
            UiAction::Unload => self.unload_all(),
            UiAction::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            UiAction::SelectAll => self.select_all_visible(),
            UiAction::InvertSelection => self.invert_selection(),
            UiAction::ExtendedTags => {
                if self.selected.is_empty() {
                    self.status = "Select a file first.".to_string();
                } else {
                    self.show_extended = true;
                }
            }
            UiAction::ToggleTagPanel => self.show_tag_panel = !self.show_tag_panel,
            UiAction::ToggleFilesystem => self.show_filesystem = !self.show_filesystem,
            UiAction::ToggleFilter => self.show_filter = !self.show_filter,
            UiAction::Columns => self.show_columns = true,
            UiAction::GoToParent => self.focus_parent_folder(),
            UiAction::About => self.show_about = true,
            UiAction::Preferences => {
                self.preferences_theme_before = self.theme_choice;
                self.show_preferences = true;
            }
            UiAction::Undo => self.undo(),
            UiAction::Redo => self.redo(),
            UiAction::TogglePreview => self.toggle_preview(),
            UiAction::WritePreviews => self.write_previews(),
            UiAction::ClearPreview => self.clear_previews(false),
            UiAction::ClearPreviewCells => self.clear_previews(true),
            UiAction::RemoveApev2 => self.remove_tag_kind(tagtools::TagKind::Apev2),
            UiAction::RemoveId3v2 => self.remove_tag_kind(tagtools::TagKind::Id3v2),
            UiAction::RemoveId3v1 => self.remove_tag_kind(tagtools::TagKind::Id3v1),
            UiAction::RemoveId3 => self.remove_tag_kind(tagtools::TagKind::Id3),
            UiAction::ShowActions => {
                self.macros = actions::load_macros();
                self.show_actions = true;
            }
            UiAction::ShowFunctions => self.show_functions = true,
            UiAction::ConvertFileToTag => self.open_convert(ConvertKind::FileToTag),
            UiAction::ConvertTagToFile => self.open_convert(ConvertKind::TagToFile),
            UiAction::ConvertFormat => self.open_convert(ConvertKind::Format),
            UiAction::ConvertRenameDirs => self.open_convert(ConvertKind::RenameDirs),
            UiAction::LoadPlaylist => self.load_playlist(ctx),
            UiAction::SavePlaylist => self.save_playlist(),
            UiAction::ExportTags => self.open_export(),
            UiAction::DeleteFiles => self.confirm_delete = true,
            UiAction::ToggleLogs => self.show_logs = !self.show_logs,
            UiAction::ToggleStoredTags => self.show_stored_tags = !self.show_stored_tags,
            UiAction::ToggleArtwork => self.show_artwork = !self.show_artwork,
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let wants_keyboard = ctx.egui_wants_keyboard_input();
        let (command, shift) = ctx.input(|input| (input.modifiers.command, input.modifiers.shift));
        if command && ctx.input(|input| input.key_pressed(egui::Key::O)) {
            if shift {
                self.open_files(ctx);
            } else {
                self.open_folder(ctx, false);
            }
            return;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::S)) {
            if self.preview_mode {
                self.write_previews();
            } else {
                self.save_selected();
            }
            return;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::Z)) {
            if shift {
                self.redo();
            } else {
                self.undo();
            }
            return;
        }
        if command && shift && ctx.input(|input| input.key_pressed(egui::Key::P)) {
            self.toggle_preview();
            return;
        }
        if command && ctx.input(|input| input.key_pressed(egui::Key::W)) {
            self.write_previews();
            return;
        }
        if wants_keyboard {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::F5)) {
            self.refresh_all(ctx);
        } else if ctx.input(|input| input.key_pressed(egui::Key::F3)) {
            self.show_filter = !self.show_filter;
        } else if command && ctx.input(|input| input.key_pressed(egui::Key::A)) {
            self.select_all_visible();
        } else if command && shift && ctx.input(|input| input.key_pressed(egui::Key::I)) {
            self.invert_selection();
        } else if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.edit_cell = None;
        }
    }
}

impl eframe::App for PuddleMoreApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.theme_choice == theme::ThemeChoice::System
            && theme::resolve(&ctx, self.theme_choice) != self.resolved_theme
        {
            self.apply_current_theme(&ctx);
        }
        self.poll_scan(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| Some(file.path().to_path_buf()))
                .collect()
        });
        if !dropped.is_empty() {
            self.start_scan(ctx.clone(), dropped, false);
        }

        self.handle_shortcuts(&ctx);

        let mut action = None;
        egui::Panel::top("top_chrome")
            .frame(
                egui::Frame::NONE
                    .fill(theme::palette().window_bg)
                    .inner_margin(egui::Margin::symmetric(2, 1)),
            )
            .show(ui, |ui| {
                if let Some(menu_action) = self.menu_bar(ui) {
                    action = Some(menu_action);
                }
                ui.add_space(1.0);
                ui.separator();
                ui.add_space(1.0);
                if let Some(toolbar_action) = self.toolbar(ui) {
                    action = Some(toolbar_action);
                }
            });

        egui::Panel::bottom("status_bar")
            .exact_size(24.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(theme::palette().window_bg)
                    .inner_margin(egui::Margin::symmetric(4, 2)),
            )
            .show(ui, |ui| self.status_bar(ui));

        if self.show_filter {
            let height = if self.filter_open { 67.0 } else { 23.0 };
            egui::Panel::bottom("filter_dock")
                .exact_size(height)
                .resizable(false)
                .frame(
                    egui::Frame::NONE
                        .fill(theme::palette().window_bg)
                        .inner_margin(egui::Margin::symmetric(4, 2)),
                )
                .show(ui, |ui| self.filter_panel(ui));
        }

        if self.show_tag_panel || self.show_filesystem {
            egui::Panel::left("left_docks")
                .resizable(true)
                .default_size(LEFT_DOCK_WIDTH)
                .min_size(220.0)
                .max_size(520.0)
                .frame(egui::Frame::NONE.fill(theme::palette().window_bg))
                .show(ui, |ui| self.left_docks(ui));
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::palette().table_bg))
            .show(ui, |ui| self.table(ui));

        if let Some(action) = action {
            self.handle_action(action, &ctx);
        }

        self.columns_window(&ctx);
        self.extended_window(&ctx);
        self.about_window(&ctx);
        self.preferences_window(&ctx);
        self.actions_window(&ctx);
        self.functions_window(&ctx);
        self.convert_window(&ctx);
        self.export_window(&ctx);
        self.logs_window(&ctx);
        self.stored_tags_window(&ctx);
        self.delete_window(&ctx);
        self.artwork_window(&ctx);
    }
}

fn dock_header(ui: &mut egui::Ui, title: &str, open: &mut bool, visible: &mut bool) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, 0.0, theme::palette().dock_header_bg);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        egui::Stroke::new(1.0, theme::palette().border),
    );
    painter.text(
        egui::pos2(rect.left() + 5.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.0),
        theme::palette().text,
    );

    let toggle_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 55.0, rect.top() + 3.0),
        egui::vec2(16.0, 16.0),
    );
    let toggle = ui.interact(
        toggle_rect,
        ui.make_persistent_id((title, "dock-toggle")),
        egui::Sense::click(),
    );
    if toggle.hovered() {
        painter.rect_filled(toggle_rect, 2.0, theme::palette().button_hover_bg);
    }
    paint_chevron(
        &painter,
        toggle_rect.center(),
        4.0,
        *open,
        theme::palette().text,
        1.3,
    );
    if toggle.clicked() {
        *open = !*open;
    }

    let float_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 36.0, rect.top() + 4.0),
        egui::vec2(15.0, 14.0),
    );
    paint_float_icon(
        &painter,
        float_rect.center(),
        4.0,
        theme::palette().weak_text,
    );

    let close_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 19.0, rect.top() + 3.0),
        egui::vec2(15.0, 16.0),
    );
    let close = ui.interact(
        close_rect,
        ui.make_persistent_id((title, "dock-close")),
        egui::Sense::click(),
    );
    if close.hovered() {
        painter.rect_filled(close_rect, 2.0, theme::palette().button_hover_bg);
    }
    paint_close_icon(&painter, close_rect.center(), 3.2, theme::palette().text);
    if close.clicked() {
        *visible = false;
    }
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str, enabled: bool) -> bool {
    let mut button = egui::Button::new(label);
    if !shortcut.is_empty() {
        button = button.shortcut_text(shortcut);
    }
    ui.add_enabled(enabled, button).clicked()
}

fn toggle_menu_item(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    ui.add(egui::Button::new(label).selected(selected))
        .clicked()
}

fn toolbar_icon(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    tooltip: &str,
    enabled: bool,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(27.0, 30.0), egui::Sense::click());
    let painter = ui.painter();
    if enabled && response.hovered() {
        painter.rect_filled(rect, 2.0, theme::palette().button_hover_bg);
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, theme::palette().border),
            egui::StrokeKind::Inside,
        );
    }
    let image_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0));
    let image = egui::Image::new((texture.id(), egui::vec2(20.0, 20.0)));
    if enabled {
        image.paint_at(ui, image_rect);
    } else {
        image
            .tint(egui::Color32::from_rgba_unmultiplied(120, 120, 120, 110))
            .paint_at(ui, image_rect);
    }
    let clicked = enabled && response.clicked();
    if enabled {
        response.on_hover_text(tooltip);
    } else {
        response.on_hover_text(format!("{tooltip} (not available in this build)"));
    }
    clicked
}

fn toolbar_icon_text(
    ui: &mut egui::Ui,
    texture: Option<&egui::TextureHandle>,
    text: &str,
    tooltip: &str,
    enabled: bool,
) -> bool {
    let font = egui::FontId::proportional(13.0);
    let text_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_string(), font.clone(), theme::palette().text)
            .size()
            .x
    });
    let width = text_width + if texture.is_some() { 29.0 } else { 14.0 };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), egui::Sense::click());
    let painter = ui.painter();
    if enabled && response.hovered() {
        painter.rect_filled(rect, 2.0, theme::palette().button_hover_bg);
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, theme::palette().border),
            egui::StrokeKind::Inside,
        );
    }
    let text_left = if let Some(texture) = texture {
        let image_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::vec2(19.0, 19.0),
        );
        let image = egui::Image::new((texture.id(), egui::vec2(19.0, 19.0)));
        if enabled {
            image.paint_at(ui, image_rect);
        } else {
            image
                .tint(egui::Color32::from_rgba_unmultiplied(120, 120, 120, 110))
                .paint_at(ui, image_rect);
        }
        rect.left() + 28.0
    } else {
        rect.left() + 7.0
    };
    painter.text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        if enabled {
            theme::palette().text
        } else {
            theme::palette().weak_text
        },
    );
    let clicked = enabled && response.clicked();
    if enabled {
        response.on_hover_text(tooltip);
    } else {
        response.on_hover_text(format!("{tooltip} (not available in this build)"));
    }
    clicked
}

fn toolbar_text_button(ui: &mut egui::Ui, text: &str, tooltip: &str, enabled: bool) -> bool {
    toolbar_icon_text(ui, None, text, tooltip, enabled)
}

fn set_scrollbar_style(ui: &mut egui::Ui) {
    let palette = theme::palette();
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.bg_fill = palette.border;
    widgets.inactive.corner_radius = egui::CornerRadius::same(2);
    widgets.hovered.bg_fill = palette.weak_text;
    widgets.hovered.corner_radius = egui::CornerRadius::same(2);
    widgets.active.bg_fill = palette.text;
    widgets.active.corner_radius = egui::CornerRadius::same(2);
}

fn dropdown_button(ui: &mut egui::Ui, enabled: bool, tooltip: &str) -> egui::Response {
    dropdown_button_height(ui, enabled, tooltip, 21.0)
}

fn dropdown_button_height(
    ui: &mut egui::Ui,
    enabled: bool,
    tooltip: &str,
    height: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(19.0, height), egui::Sense::click());
    let background = if !enabled {
        theme::palette().table_alt_bg
    } else if response.hovered() {
        theme::palette().button_hover_bg
    } else {
        theme::palette().button_bg
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, background);
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, theme::palette().border),
        egui::StrokeKind::Inside,
    );
    paint_chevron(
        painter,
        egui::pos2(rect.center().x, rect.center().y + 1.0),
        3.2,
        true,
        if enabled {
            theme::palette().text
        } else {
            theme::palette().weak_text
        },
        1.3,
    );
    if enabled {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

fn paint_chevron(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    down: bool,
    color: egui::Color32,
    width: f32,
) {
    let points = if down {
        vec![
            egui::pos2(center.x - size, center.y - size * 0.45),
            egui::pos2(center.x, center.y + size * 0.55),
            egui::pos2(center.x + size, center.y - size * 0.45),
        ]
    } else {
        vec![
            egui::pos2(center.x - size * 0.45, center.y - size),
            egui::pos2(center.x + size * 0.55, center.y),
            egui::pos2(center.x - size * 0.45, center.y + size),
        ]
    };
    painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
}

fn paint_close_icon(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    painter.line_segment(
        [
            egui::pos2(center.x - size, center.y - size),
            egui::pos2(center.x + size, center.y + size),
        ],
        egui::Stroke::new(1.2, color),
    );
    painter.line_segment(
        [
            egui::pos2(center.x + size, center.y - size),
            egui::pos2(center.x - size, center.y + size),
        ],
        egui::Stroke::new(1.2, color),
    );
}

fn paint_float_icon(painter: &egui::Painter, center: egui::Pos2, size: f32, color: egui::Color32) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(size * 2.0, size * 1.6));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    painter.hline(
        rect.x_range(),
        rect.top() + size * 0.5,
        egui::Stroke::new(1.0, color),
    );
}

fn paint_sort_arrow(painter: &egui::Painter, center: egui::Pos2, down: bool, color: egui::Color32) {
    let points = if down {
        vec![
            egui::pos2(center.x - 4.0, center.y - 2.0),
            egui::pos2(center.x + 4.0, center.y - 2.0),
            egui::pos2(center.x, center.y + 3.0),
        ]
    } else {
        vec![
            egui::pos2(center.x - 4.0, center.y + 2.0),
            egui::pos2(center.x + 4.0, center.y + 2.0),
            egui::pos2(center.x, center.y - 3.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        points,
        color,
        egui::Stroke::NONE,
    ));
}

fn column_width(field: &str) -> f32 {
    match field {
        "__filename" => 96.0,
        "__dirpath" => 170.0,
        "comment" | "description" | "lyrics" => 150.0,
        "__filetype" => 95.0,
        "__ext" => 75.0,
        _ => 100.0,
    }
}

fn field_title(field: &str) -> String {
    let mut name = field;
    let mut prefix = "";
    if let Some(stripped) = name.strip_prefix("__") {
        name = stripped;
        prefix = "File ";
    }
    let mut out = String::from(prefix);
    let mut capitalize = true;
    for ch in name.chars() {
        match ch {
            '_' => {
                out.push(' ');
                capitalize = true;
            }
            _ => {
                if capitalize {
                    out.extend(ch.to_uppercase());
                    capitalize = false;
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out
}

fn ui_display(track: &Track, field: &str) -> String {
    if field == "track" {
        let track_number = track.display("track");
        let total = track.display("totaltracks");
        if !track_number.is_empty() && !total.is_empty() && !track_number.contains('/') {
            return format!("{track_number}/{total}");
        }
        return track_number;
    }
    track.display(field)
}

fn set_ui_field(track: &mut Track, field: &str, value: String) {
    if field == "track" {
        let value = value.trim();
        if value.is_empty() {
            track.set_display("track", String::new());
            track.set_display("totaltracks", String::new());
        } else if let Some((number, total)) = value.split_once('/') {
            track.set_display("track", number.trim().to_string());
            track.set_display("totaltracks", total.trim().to_string());
        } else {
            track.set_display("track", value.to_string());
        }
    } else {
        track.set_display(field, value);
    }
}

fn column_at<'a>(x: f32, columns: &'a [Column], widths: &[f32]) -> Option<(&'a Column, f32)> {
    let mut left = 0.0f32;
    for (column, width) in columns.iter().zip(widths.iter()) {
        if x >= left && x < left + *width {
            return Some((column, left));
        }
        left += *width;
    }
    None
}

fn elide_text(ui: &egui::Ui, text: &str, max_width: f32, font: &egui::FontId) -> String {
    if text.is_empty() || max_width <= 0.0 {
        return String::new();
    }
    let full_width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_string(), font.clone(), theme::palette().text)
            .size()
            .x
    });
    if full_width <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let chars: Vec<char> = text.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();
    while low < high {
        let middle = (low + high + 1) / 2;
        let candidate = format!("{}{}", chars[..middle].iter().collect::<String>(), ellipsis);
        let width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap(candidate, font.clone(), theme::palette().text)
                .size()
                .x
        });
        if width <= max_width {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    let visible: String = chars.into_iter().take(low).collect();
    format!("{visible}{ellipsis}")
}

fn size_label(data: &[u8]) -> String {
    match image::load_from_memory(data) {
        Ok(image) => format!("{}×{}", image.width(), image.height()),
        Err(_) => "image".to_string(),
    }
}

fn type_name(image_type: u8) -> &'static str {
    let names = [
        "Other",
        "File Icon",
        "Other File Icon",
        "Front Cover",
        "Back Cover",
        "Leaflet Page",
        "Media",
        "Lead Artist",
        "Artist",
        "Conductor",
        "Band",
        "Composer",
        "Lyricist",
        "Recording Location",
        "During Recording",
        "During Performance",
        "Movie Screen Capture",
        "Coloured Fish",
        "Illustration",
        "Band Logo",
        "Publisher Logo",
    ];
    names
        .get(image_type as usize)
        .copied()
        .unwrap_or("Front Cover")
}

fn parse_duration(value: &str) -> u64 {
    let mut total = 0u64;
    for part in value.split(':') {
        if let Ok(number) = part.trim().parse::<u64>() {
            total = total.saturating_mul(60).saturating_add(number);
        }
    }
    total
}

fn format_duration(total: u64) -> String {
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

fn stats_summary(duration: u64, size: u64) -> String {
    format!("{} | {}", format_duration(duration), format_size(size))
}

fn dir_children_cached(
    cache: &mut std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    dir: &Path,
) -> Vec<PathBuf> {
    if let Some(children) = cache.get(dir) {
        return children.clone();
    }
    let mut children = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name();
                    if name.to_string_lossy().starts_with('.') {
                        continue;
                    }
                    children.push(entry.path());
                }
            }
        }
    }
    children.sort_by(|left, right| {
        left.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .cmp(
                &right
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase(),
            )
    });
    cache.insert(dir.to_path_buf(), children.clone());
    children
}

#[allow(clippy::too_many_arguments)]
fn render_dir_tree(
    ui: &mut egui::Ui,
    path: &Path,
    depth: usize,
    cache: &mut std::collections::HashMap<PathBuf, Vec<PathBuf>>,
    expanded: &mut std::collections::HashSet<PathBuf>,
    current: Option<&Path>,
    icons: &Icons,
    clicked: &mut Option<PathBuf>,
    reveal_pending: &mut bool,
) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let is_expanded = expanded.contains(path);
    let is_current = current == Some(path);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);
        ui.add_space(depth as f32 * 12.0);
        let (arrow_rect, arrow) =
            ui.allocate_exact_size(egui::vec2(12.0, 16.0), egui::Sense::click());
        if arrow.hovered() {
            ui.painter()
                .rect_filled(arrow_rect, 2.0, theme::palette().button_hover_bg);
        }
        paint_chevron(
            ui.painter(),
            arrow_rect.center(),
            3.0,
            is_expanded,
            theme::palette().text,
            1.2,
        );
        if arrow.clicked() {
            if is_expanded {
                expanded.remove(path);
            } else {
                expanded.insert(path.to_path_buf());
            }
        }
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        egui::Image::new((icons.folder.id(), egui::vec2(14.0, 14.0))).paint_at(ui, icon_rect);
        let label = ui.add(egui::Button::new(name).frame(false).selected(is_current));
        if label.clicked() {
            *clicked = Some(path.to_path_buf());
        }
        if is_current && *reveal_pending {
            let clip = ui.clip_rect();
            let target = egui::Rect::from_x_y_ranges(clip.x_range(), label.rect.y_range());
            ui.scroll_to_rect(target, Some(egui::Align::TOP));
            *reveal_pending = false;
        }
    });
    if expanded.contains(path) {
        let children = dir_children_cached(cache, path);
        for child in children {
            render_dir_tree(
                ui,
                &child,
                depth + 1,
                cache,
                expanded,
                current,
                icons,
                clicked,
                reveal_pending,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escapes_quotes_and_backslashes() {
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn field_titles_are_human_readable() {
        assert_eq!(field_title("albumartist"), "Albumartist");
        assert_eq!(field_title("__filename"), "File Filename");
        assert_eq!(field_title("original_year"), "Original Year");
    }

    #[test]
    fn duration_and_size_formatting() {
        assert_eq!(parse_duration("00:01"), 1);
        assert_eq!(parse_duration("01:02:03"), 3723);
        assert_eq!(format_duration(4), "00:04");
        assert_eq!(format_duration(3723), "1:02:03");
        assert_eq!(format_size(23833), "23 KB");
    }
}
