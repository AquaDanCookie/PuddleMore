//! High-contrast System/Light/Dark palettes shared by the whole UI.

use eframe::egui;
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn from_config(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolvedTheme {
    Light,
    Dark,
}

impl ResolvedTheme {
    pub fn is_dark(self) -> bool {
        self == Self::Dark
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub window_bg: egui::Color32,
    pub dock_header_bg: egui::Color32,
    pub header_bg: egui::Color32,
    pub table_bg: egui::Color32,
    pub table_alt_bg: egui::Color32,
    pub table_hover_bg: egui::Color32,
    pub table_selected_bg: egui::Color32,
    pub table_index_bg: egui::Color32,
    pub table_index_selected_bg: egui::Color32,
    pub table_current_cell_bg: egui::Color32,
    pub border: egui::Color32,
    pub grid_line: egui::Color32,
    pub text: egui::Color32,
    pub weak_text: egui::Color32,
    pub error_red: egui::Color32,
    pub selected_text: egui::Color32,
    pub preview_bg: egui::Color32,
    pub preview_text: egui::Color32,
    pub button_bg: egui::Color32,
    pub button_hover_bg: egui::Color32,
    pub button_active_bg: egui::Color32,
    pub tooltip_bg: egui::Color32,
    pub tooltip_text: egui::Color32,
    pub link: egui::Color32,
    pub disabled_text: egui::Color32,
    pub accent: egui::Color32,
    pub accent_text: egui::Color32,
}

fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

impl Palette {
    pub fn light() -> Self {
        Self {
            dark: false,
            window_bg: rgb(0xf2f4f7),
            dock_header_bg: rgb(0xe6e9ee),
            header_bg: rgb(0xeef1f5),
            table_bg: rgb(0xffffff),
            table_alt_bg: rgb(0xf7f9fb),
            table_hover_bg: rgb(0xe8f0fd),
            table_selected_bg: rgb(0x2f7ec7),
            table_index_bg: rgb(0xe6e9ee),
            table_index_selected_bg: rgb(0xd3dce6),
            table_current_cell_bg: rgb(0x2f7ec7),
            border: rgb(0xc2c9d1),
            grid_line: rgb(0xdfe4ea),
            text: rgb(0x171a1e),
            weak_text: rgb(0x66707a),
            error_red: rgb(0xb91e1e),
            selected_text: rgb(0xffffff),
            preview_bg: rgb(0xc0ffc0),
            preview_text: rgb(0x171a1e),
            button_bg: rgb(0xe6e9ee),
            button_hover_bg: rgb(0xdce6f5),
            button_active_bg: rgb(0xcbd9ec),
            tooltip_bg: rgb(0xffffe1),
            tooltip_text: rgb(0x171a1e),
            link: rgb(0x155d9e),
            disabled_text: rgb(0x737b85),
            accent: rgb(0x2f7ec7),
            accent_text: rgb(0xffffff),
        }
    }

    pub fn dark() -> Self {
        Self {
            dark: true,
            window_bg: rgb(0x20242a),
            dock_header_bg: rgb(0x2a3038),
            header_bg: rgb(0x1d2228),
            table_bg: rgb(0x15191e),
            table_alt_bg: rgb(0x1d2228),
            table_hover_bg: rgb(0x2b3139),
            table_selected_bg: rgb(0x2f81c7),
            table_index_bg: rgb(0x242a31),
            table_index_selected_bg: rgb(0x2a3038),
            table_current_cell_bg: rgb(0x2f81c7),
            border: rgb(0x3c454f),
            grid_line: rgb(0x2b3139),
            text: rgb(0xeef2f6),
            weak_text: rgb(0x9ba6b2),
            error_red: rgb(0xff8a8a),
            selected_text: rgb(0xffffff),
            preview_bg: rgb(0x2d5b3e),
            preview_text: rgb(0xffffff),
            button_bg: rgb(0x2a3038),
            button_hover_bg: rgb(0x333b44),
            button_active_bg: rgb(0x3c454f),
            tooltip_bg: rgb(0x2b3139),
            tooltip_text: rgb(0xeef2f6),
            link: rgb(0x74b9ff),
            disabled_text: rgb(0x8a949f),
            accent: rgb(0x2f81c7),
            accent_text: rgb(0xffffff),
        }
    }
}

static PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

fn palette_cell() -> &'static RwLock<Palette> {
    PALETTE.get_or_init(|| RwLock::new(Palette::light()))
}

pub fn palette() -> Palette {
    *palette_cell().read().expect("palette lock")
}

pub fn set_palette(palette: Palette) {
    *palette_cell().write().expect("palette lock") = palette;
}

pub fn resolve(ctx: &egui::Context, choice: ThemeChoice) -> ResolvedTheme {
    match choice {
        ThemeChoice::Light => ResolvedTheme::Light,
        ThemeChoice::Dark => ResolvedTheme::Dark,
        ThemeChoice::System => match ctx.system_theme() {
            Some(egui::Theme::Dark) => ResolvedTheme::Dark,
            Some(egui::Theme::Light) => ResolvedTheme::Light,
            None => {
                if ctx.style_of(ctx.theme()).visuals.dark_mode {
                    ResolvedTheme::Dark
                } else {
                    ResolvedTheme::Light
                }
            }
        },
    }
}

pub fn apply(
    ctx: &egui::Context,
    choice: ThemeChoice,
    selection_override: Option<[u8; 3]>,
    preview_override: Option<[u8; 3]>,
) -> ResolvedTheme {
    ctx.set_theme(match choice {
        ThemeChoice::System => egui::ThemePreference::System,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::Dark => egui::ThemePreference::Dark,
    });

    let resolved = resolve(ctx, choice);
    let mut palette = if resolved.is_dark() {
        Palette::dark()
    } else {
        Palette::light()
    };
    if let Some(color) = selection_override {
        palette.table_selected_bg = egui::Color32::from_rgb(color[0], color[1], color[2]);
        palette.table_current_cell_bg = palette.table_selected_bg;
        palette.accent = palette.table_selected_bg;
    }
    if let Some(color) = preview_override {
        palette.preview_bg = egui::Color32::from_rgb(color[0], color[1], color[2]);
        palette.preview_text = readable_text(palette.preview_bg);
    }
    set_palette(palette);

    let style = make_style(&palette);
    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_global_style(style);
    resolved
}

pub fn readable_text(background: egui::Color32) -> egui::Color32 {
    let luminance =
        (299 * background.r() as u32 + 587 * background.g() as u32 + 114 * background.b() as u32)
            / 1000;
    if luminance < 128 {
        egui::Color32::WHITE
    } else {
        egui::Color32::BLACK
    }
}

fn make_style(palette: &Palette) -> egui::Style {
    let mut style = if palette.dark {
        (*egui::Context::default().style_of(egui::Theme::Dark)).clone()
    } else {
        (*egui::Context::default().style_of(egui::Theme::Light)).clone()
    };
    let mut visuals = if palette.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.dark_mode = palette.dark;
    visuals.override_text_color = Some(palette.text);
    visuals.panel_fill = palette.window_bg;
    visuals.window_fill = palette.window_bg;
    visuals.extreme_bg_color = palette.table_bg;
    visuals.text_edit_bg_color = Some(palette.table_bg);
    visuals.faint_bg_color = palette.table_alt_bg;
    visuals.selection.bg_fill = palette.table_selected_bg;
    visuals.selection.stroke = egui::Stroke::new(1.0, palette.selected_text);
    visuals.widgets.noninteractive.bg_fill = palette.window_bg;
    visuals.widgets.noninteractive.weak_bg_fill = palette.window_bg;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.bg_fill = palette.button_bg;
    visuals.widgets.inactive.weak_bg_fill = palette.button_bg;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.hovered.bg_fill = palette.button_hover_bg;
    visuals.widgets.hovered.weak_bg_fill = palette.button_hover_bg;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.active.bg_fill = palette.button_active_bg;
    visuals.widgets.active.weak_bg_fill = palette.button_active_bg;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, palette.accent);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.open.bg_fill = palette.button_bg;
    visuals.widgets.open.weak_bg_fill = palette.button_bg;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, palette.border);
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, palette.text);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(2);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(2);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(2);
    visuals.window_corner_radius = egui::CornerRadius::ZERO;
    visuals.menu_corner_radius = egui::CornerRadius::same(2);
    visuals.window_shadow = egui::Shadow::NONE;
    visuals.popup_shadow = egui::Shadow::NONE;
    visuals.button_frame = true;
    visuals.collapsing_header_frame = false;
    visuals.striped = true;
    visuals.interact_cursor = None;
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(4.0, 3.0);
    style.spacing.button_padding = egui::vec2(5.0, 2.0);
    style.spacing.menu_margin = egui::Margin::symmetric(4, 3);
    style.spacing.window_margin = egui::Margin::symmetric(6, 4);
    style.spacing.interact_size = egui::vec2(20.0, 20.0);
    style.spacing.combo_width = 80.0;
    style.spacing.scroll.floating = false;
    style.spacing.scroll.foreground_color = false;
    style.spacing.scroll.bar_width = 12.0;
    style.spacing.scroll.bar_inner_margin = 2.0;
    style.spacing.scroll.bar_outer_margin = 0.0;
    style.spacing.scroll.content_margin = egui::Margin::ZERO;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(10.5));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    style
}
