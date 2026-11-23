//! mongui's design system: colors, type, spacing, radii and the global egui style.
//!
//! Screens never pick raw colors or sizes. They ask this module (`pal(ui)`, `space::*`,
//! `font::*`, `radius::*`) or use the components in `widgets.rs`, so the whole app changes
//! together when a token changes.

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle,
    epaint::text::FontTweak,
};
use std::sync::Arc;

// ---------------------------------------------------------------- colors

/// Every color the UI uses. Two instances exist: dark (default) and light.
pub struct Palette {
    /// Main workspace background.
    pub bg: Color32,
    /// Sidebars, top bar and pager: one step darker than the workspace in dark mode.
    pub bg_sidebar: Color32,
    /// Cards and grouped sections that sit on `bg`.
    pub surface: Color32,
    /// Popups, menus, dialogs and toasts.
    pub elevated: Color32,
    /// Text inputs.
    pub input: Color32,
    /// Neutral button fills: rest, hover, pressed.
    pub control: Color32,
    pub control_hover: Color32,
    pub control_active: Color32,

    pub border: Color32,
    pub border_strong: Color32,
    /// Hairlines between rows, quieter than `border`.
    pub divider: Color32,

    pub text: Color32,
    pub text_strong: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,

    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_active: Color32,
    /// Text and icons placed on `accent`.
    pub on_accent: Color32,
    /// Selected rows, selected segments, text selection.
    pub accent_soft: Color32,
    /// Accent text on `accent_soft`.
    pub accent_text: Color32,

    pub danger: Color32,
    pub danger_hover: Color32,
    pub danger_soft: Color32,
    pub success: Color32,
    pub warning: Color32,

    /// Document value colors.
    pub syn_key: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_bool: Color32,
    pub syn_null: Color32,
    pub syn_id: Color32,
    pub syn_date: Color32,
    pub syn_other: Color32,

    pub backdrop: Color32,
    pub shadow: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub static DARK: Palette = Palette {
    bg: rgb(0x141519),
    bg_sidebar: rgb(0x0F1013),
    surface: rgb(0x191A1F),
    elevated: rgb(0x1D1F24),
    input: rgb(0x0F1013),
    control: rgb(0x23252B),
    control_hover: rgb(0x2A2C33),
    control_active: rgb(0x31343C),

    border: rgb(0x2A2C33),
    border_strong: rgb(0x3A3D46),
    divider: rgb(0x212328),

    text: rgb(0xD9DBE1),
    text_strong: rgb(0xF4F5F7),
    text_muted: rgb(0x9095A1),
    text_faint: rgb(0x60646F),

    accent: rgb(0x6C78F0),
    accent_hover: rgb(0x7E89F4),
    accent_active: rgb(0x5D69E2),
    on_accent: rgb(0xFFFFFF),
    accent_soft: rgb(0x242842),
    accent_text: rgb(0xB2B9FF),

    danger: rgb(0xE5534B),
    danger_hover: rgb(0xEE6A62),
    danger_soft: rgb(0x2A1719),
    success: rgb(0x4CC38A),
    warning: rgb(0xE2B340),

    syn_key: rgb(0xA9AEBB),
    syn_string: rgb(0xE6B673),
    syn_number: rgb(0x7DC4E4),
    syn_bool: rgb(0xC0A2F2),
    syn_null: rgb(0x6E7380),
    syn_id: rgb(0xEE9A9A),
    syn_date: rgb(0x89CF9C),
    syn_other: rgb(0xC9CCD4),

    backdrop: Color32::from_rgba_premultiplied(0, 0, 0, 150),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 110),
};

pub static LIGHT: Palette = Palette {
    bg: rgb(0xFFFFFF),
    bg_sidebar: rgb(0xF6F6F8),
    surface: rgb(0xFFFFFF),
    elevated: rgb(0xFFFFFF),
    input: rgb(0xFFFFFF),
    control: rgb(0xF1F1F4),
    control_hover: rgb(0xE8E8EC),
    control_active: rgb(0xDEDEE3),

    border: rgb(0xE3E3E8),
    border_strong: rgb(0xCDCDD4),
    divider: rgb(0xEDEDF0),

    text: rgb(0x2A2B31),
    text_strong: rgb(0x111216),
    text_muted: rgb(0x676B76),
    text_faint: rgb(0x9A9DA6),

    accent: rgb(0x5462E6),
    accent_hover: rgb(0x4655DB),
    accent_active: rgb(0x3B49CC),
    on_accent: rgb(0xFFFFFF),
    accent_soft: rgb(0xECEEFD),
    accent_text: rgb(0x3D4AC9),

    danger: rgb(0xD1392F),
    danger_hover: rgb(0xBC2F26),
    danger_soft: rgb(0xFCEDEC),
    success: rgb(0x1F9D62),
    warning: rgb(0xB7860B),

    syn_key: rgb(0x4B505C),
    syn_string: rgb(0x9A5B00),
    syn_number: rgb(0x0A6F93),
    syn_bool: rgb(0x6B3FB5),
    syn_null: rgb(0x9A9DA6),
    syn_id: rgb(0xA8353A),
    syn_date: rgb(0x2C7A3E),
    syn_other: rgb(0x3D404A),

    backdrop: Color32::from_rgba_premultiplied(10, 12, 20, 70),
    shadow: Color32::from_rgba_premultiplied(0, 0, 0, 28),
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// The palette for the theme `ui` is drawn in.
pub fn pal(ui: &egui::Ui) -> &'static Palette {
    palette(ui.visuals().dark_mode)
}

/// Colors a user can give a saved connection. The index is what gets saved, so the order is fixed.
pub const CONN_COLORS: [Color32; 8] = [
    rgb(0x5B8DEF), // blue
    rgb(0x45B97C), // green
    rgb(0xE2B340), // yellow
    rgb(0xE8833A), // orange
    rgb(0xE5534B), // red
    rgb(0xA77BEA), // purple
    rgb(0x3DBDB4), // teal
    rgb(0x8B919D), // gray
];

pub fn conn_color(i: usize) -> Color32 {
    CONN_COLORS[i % CONN_COLORS.len()]
}

// ---------------------------------------------------------------- layout tokens

pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
}

pub mod radius {
    pub const SM: u8 = 4;
    pub const MD: u8 = 6;
    pub const LG: u8 = 8;
    pub const XL: u8 = 12;
}

pub mod size {
    /// Height of buttons, inputs and combo boxes.
    pub const CONTROL: f32 = 28.0;
    /// Square icon-only buttons.
    pub const ICON_BUTTON: f32 = 28.0;
    pub const TOPBAR: f32 = 44.0;
    pub const PAGER: f32 = 44.0;
    pub const TREE_ROW: f32 = 26.0;
    pub const SIDEBAR: f32 = 264.0;
    pub const SIDEBAR_MIN: f32 = 200.0;
    pub const SIDEBAR_MAX: f32 = 420.0;
    /// Label column of forms.
    pub const FORM_LABEL: f32 = 136.0;
    pub const FORM_MAX: f32 = 680.0;
}

// ---------------------------------------------------------------- type

pub mod font {
    use eframe::egui::{FontFamily, FontId};

    pub const BODY: f32 = 13.5;
    pub const SMALL: f32 = 12.0;
    pub const TINY: f32 = 11.0;
    pub const MONO: f32 = 12.5;
    pub const TITLE: f32 = 15.0;
    pub const HEADING: f32 = 19.0;
    pub const ICON: f32 = 15.0;

    pub fn regular(size: f32) -> FontId {
        FontId::proportional(size)
    }
    pub fn medium(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("medium".into()))
    }
    pub fn semibold(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("semibold".into()))
    }
    pub fn mono(size: f32) -> FontId {
        FontId::monospace(size)
    }
    pub fn icon(size: f32) -> FontId {
        FontId::new(size, FontFamily::Name("icons".into()))
    }
}

/// Lucide icons (ISC license), subset into `assets/fonts/lucide.ttf`.
/// They are also a fallback of every text font, so they can sit inside button labels.
pub mod icon {
    pub const PLUS: &str = "\u{e13d}";
    pub const EDIT: &str = "\u{e1f9}";
    pub const TRASH: &str = "\u{e18e}";
    pub const COPY: &str = "\u{e09e}";
    pub const CLONE: &str = "\u{e3fd}";
    pub const REFRESH: &str = "\u{e145}";
    pub const SEARCH: &str = "\u{e151}";
    pub const CLOSE: &str = "\u{e1b2}";
    pub const PREV: &str = "\u{e06e}";
    pub const NEXT: &str = "\u{e06f}";
    pub const CHEVRON_DOWN: &str = "\u{e06d}";
    pub const FOLDER: &str = "\u{e247}";
    pub const DISCONNECT: &str = "\u{e45d}";
    pub const PLUG: &str = "\u{e37f}";
    pub const DATABASE: &str = "\u{e0ad}";
    pub const COLLECTION: &str = "\u{e529}";
    pub const MORE: &str = "\u{e0b6}";
    pub const SUN: &str = "\u{e178}";
    pub const MOON: &str = "\u{e11e}";
    pub const EXPAND: &str = "\u{e43e}";
    pub const COLLAPSE: &str = "\u{e43c}";
    pub const EXPORT: &str = "\u{e19e}";
    pub const IMPORT: &str = "\u{e0b2}";
    pub const SERVER: &str = "\u{e153}";
    pub const LOCK: &str = "\u{e10b}";
    pub const SHIELD: &str = "\u{e1ff}";
    pub const KEY: &str = "\u{e4a3}";
    pub const ALERT: &str = "\u{e077}";
    pub const CHECK_CIRCLE: &str = "\u{e226}";
    pub const INBOX: &str = "\u{e0f7}";
    pub const FILTER: &str = "\u{e0dc}";
    pub const SORT: &str = "\u{e37d}";
    pub const EYE: &str = "\u{e0ba}";
    pub const EYE_OFF: &str = "\u{e0bb}";
    pub const SETTINGS: &str = "\u{e29a}";
    pub const INFO: &str = "\u{e0f9}";
}

// ---------------------------------------------------------------- install

fn font_data(bytes: &'static [u8], tweak: FontTweak) -> Arc<FontData> {
    Arc::new(FontData::from_static(bytes).tweak(tweak))
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let icon_tweak = FontTweak { scale: 1.0, y_offset_factor: 0.08, ..Default::default() };
    for (name, bytes, tweak) in [
        ("Inter", &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..], FontTweak::default()),
        ("Inter-Medium", &include_bytes!("../assets/fonts/Inter-Medium.ttf")[..], FontTweak::default()),
        ("Inter-SemiBold", &include_bytes!("../assets/fonts/Inter-SemiBold.ttf")[..], FontTweak::default()),
        ("JetBrainsMono", &include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")[..], FontTweak::default()),
        ("lucide", &include_bytes!("../assets/fonts/lucide.ttf")[..], icon_tweak),
    ] {
        fonts.font_data.insert(name.into(), font_data(bytes, tweak));
    }
    // egui's bundled fonts stay as fallbacks for scripts Inter's subset lacks (Cyrillic, Greek, emoji).
    let with = |first: &str| {
        let mut v = vec![first.to_string(), "lucide".to_string()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("Inter"));
    fonts.families.insert(FontFamily::Name("medium".into()), with("Inter-Medium"));
    fonts.families.insert(FontFamily::Name("semibold".into()), with("Inter-SemiBold"));
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["lucide".into(), "Inter".into()]);
    let mut mono = vec!["JetBrainsMono".to_string(), "lucide".to_string()];
    mono.extend(fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default());
    fonts.families.insert(FontFamily::Monospace, mono);
    ctx.set_fonts(fonts);
}

/// Called once at startup.
pub fn install(ctx: &egui::Context, dark: bool) {
    install_fonts(ctx);
    apply(ctx, dark);
}

/// Applies the dark or light theme. Cheap enough to call when the user toggles it.
pub fn apply(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let p = palette(dark);
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let r = CornerRadius::same(radius::MD);

    v.panel_fill = p.bg;
    v.window_fill = p.elevated;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(radius::XL);
    v.window_shadow = Shadow { offset: [0, 12], blur: 32, spread: 0, color: p.shadow };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: p.shadow };
    v.menu_corner_radius = CornerRadius::same(radius::LG);
    v.extreme_bg_color = p.input;
    v.text_edit_bg_color = Some(p.input);
    v.faint_bg_color = p.surface;
    v.code_bg_color = p.surface;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.danger;
    v.weak_text_color = Some(p.text_muted);
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = true;
    v.striped = false;
    v.button_frame = true;
    v.disabled_alpha = 0.45;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.divider);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.input;
    w.inactive.weak_bg_fill = p.control;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;
    w.inactive.expansion = 0.0;

    w.hovered.bg_fill = p.input;
    w.hovered.weak_bg_fill = p.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.input;
    w.active.weak_bg_fill = p.control_active;
    w.active.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.active.fg_stroke = Stroke::new(1.0, p.text_strong);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open = w.hovered;
    w.open.weak_bg_fill = p.control_active;

    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        let sp = &mut s.spacing;
        sp.item_spacing = egui::vec2(space::SM, 6.0);
        sp.button_padding = egui::vec2(10.0, 4.0);
        sp.interact_size = egui::vec2(24.0, size::CONTROL);
        sp.menu_margin = Margin::same(6);
        sp.window_margin = Margin::same(20);
        sp.indent = 16.0;
        sp.icon_width = 16.0;
        sp.icon_width_inner = 8.0;
        sp.icon_spacing = 6.0;
        sp.combo_height = 280.0;
        sp.menu_spacing = 2.0;
        sp.scroll = egui::style::ScrollStyle::thin();
        sp.scroll.bar_width = 8.0;
        sp.scroll.floating = true;
        sp.scroll.bar_inner_margin = 2.0;
        sp.tooltip_width = 360.0;
        s.interaction.tooltip_delay = 0.35;
        s.interaction.selectable_labels = false;
        for (style, f) in s.text_styles.iter_mut() {
            *f = match style {
                TextStyle::Heading => font::semibold(font::HEADING),
                TextStyle::Small => FontId::proportional(font::SMALL),
                TextStyle::Monospace => FontId::monospace(font::MONO),
                TextStyle::Button => FontId::proportional(font::BODY),
                _ => FontId::proportional(font::BODY),
            };
        }
    });
}
