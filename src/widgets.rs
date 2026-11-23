//! Reusable UI components built on the tokens in `theme.rs`.
//! Screens compose these instead of styling egui widgets one by one.

use crate::theme::{self, font, icon, pal, radius, size, space};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Response, RichText, Sense, Stroke, StrokeKind,
    TextEdit, Ui, Vec2, WidgetInfo, WidgetText, vec2,
};
use eframe::egui::Role as WidgetType;

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Filled with the accent color: the one main action of a view.
    Primary,
    /// Neutral fill with a hairline border.
    Secondary,
    /// No background until hovered: toolbars, icon buttons, row actions, menu items.
    Ghost,
    /// Red, only for confirming destructive actions inside a dialog.
    Danger,
    /// Ghost with red text (destructive menu items).
    GhostDanger,
}

/// Fill, border and text color of a button in a given state.
fn button_colors(p: &theme::Palette, kind: Kind, hovered: bool, pressed: bool, selected: bool) -> (Color32, Color32, Color32) {
    match kind {
        Kind::Primary => (if pressed { p.accent_active } else if hovered { p.accent_hover } else { p.accent }, Color32::TRANSPARENT, p.on_accent),
        Kind::Danger => (if pressed { p.danger } else if hovered { p.danger_hover } else { p.danger }, Color32::TRANSPARENT, p.on_accent),
        Kind::Secondary => (
            if pressed { p.control_active } else if hovered { p.control_hover } else { p.control },
            if hovered { p.border_strong } else { p.border },
            if hovered { p.text_strong } else { p.text },
        ),
        Kind::Ghost | Kind::GhostDanger => {
            let fill = if selected {
                p.accent_soft
            } else if pressed {
                p.control_hover
            } else if hovered {
                p.control
            } else {
                Color32::TRANSPARENT
            };
            let text = if kind == Kind::GhostDanger {
                p.danger
            } else if selected {
                p.accent_text
            } else if hovered {
                p.text_strong
            } else {
                p.text_muted
            };
            (fill, Color32::TRANSPARENT, text)
        }
    }
}

/// Everything about one button. Built by the helpers below.
pub struct Btn<'a> {
    pub kind: Kind,
    pub icon: Option<&'a str>,
    pub icon_size: f32,
    pub label: &'a str,
    pub font: Option<egui::FontId>,
    pub selected: bool,
    pub min_size: Vec2,
    /// Stretch to the available width and left-align the content (menu items).
    pub full_width: bool,
    /// Accessible name when there is no label.
    pub a11y: &'a str,
}

impl<'a> Btn<'a> {
    pub fn new(kind: Kind, icon: Option<&'a str>, label: &'a str) -> Self {
        Self { kind, icon, icon_size: 14.0, label, font: None, selected: false, min_size: vec2(0.0, size::CONTROL), full_width: false, a11y: label }
    }

    /// Custom-painted so every state (rest, hover, pressed, focused, disabled, selected) follows the
    /// palette exactly. Keyboard: focusable with Tab, activated with Space or Enter.
    pub fn show(self, ui: &mut Ui) -> Response {
        let p = pal(ui);
        let font_id = self.font.clone().unwrap_or_else(|| {
            if matches!(self.kind, Kind::Primary | Kind::Danger) { font::medium(font::BODY) } else { font::regular(font::BODY) }
        });
        let icon_g = self.icon.map(|i| ui.painter().layout_no_wrap(i.to_string(), font::icon(self.icon_size), Color32::PLACEHOLDER));
        let label_g = (!self.label.is_empty()).then(|| ui.painter().layout_no_wrap(self.label.to_string(), font_id, Color32::PLACEHOLDER));
        let gap = if icon_g.is_some() && label_g.is_some() { 7.0 } else { 0.0 };
        let content_w = icon_g.as_ref().map_or(0.0, |g| g.size().x) + gap + label_g.as_ref().map_or(0.0, |g| g.size().x);
        let pad = if label_g.is_some() { 12.0 } else { 6.0 };
        let mut desired = vec2(content_w + pad * 2.0, self.min_size.y).max(self.min_size);
        if self.full_width {
            desired.x = desired.x.max(ui.available_width());
        }
        let (rect, r) = ui.allocate_exact_size(desired, Sense::click());
        let (label, selected, a11y) = (self.label.to_string(), self.selected, self.a11y.to_string());
        r.widget_info(|| {
            let name = if label.is_empty() { &a11y } else { &label };
            if selected { WidgetInfo::selected(WidgetType::Button, true, true, name) } else { WidgetInfo::labeled(WidgetType::Button, true, name) }
        });
        if ui.is_rect_visible(rect) {
            let enabled = ui.is_enabled();
            let hovered = enabled && r.hovered();
            let pressed = enabled && r.is_pointer_button_down_on();
            let (fill, stroke, text) = button_colors(p, self.kind, hovered, pressed, self.selected);
            let cr = CornerRadius::same(radius::MD);
            let painter = ui.painter();
            painter.rect(rect, cr, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
            if r.has_focus() {
                painter.rect_stroke(rect.expand(1.5), CornerRadius::same(radius::MD + 1), Stroke::new(1.5, p.accent), StrokeKind::Outside);
            }
            let mut x = if self.full_width { rect.left() + pad } else { rect.center().x - content_w / 2.0 };
            if let Some(g) = icon_g {
                let y = rect.center().y - g.size().y / 2.0;
                let wdt = g.size().x;
                painter.galley(egui::pos2(x, y), g, text);
                x += wdt + gap;
            }
            if let Some(g) = label_g {
                painter.galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, text);
            }
        }
        r
    }
}

/// Text button, optionally with a leading icon.
pub fn button(ui: &mut Ui, kind: Kind, icon: Option<&str>, label: &str) -> Response {
    Btn::new(kind, icon, label).show(ui)
}

pub fn button_enabled(ui: &mut Ui, enabled: bool, kind: Kind, icon: Option<&str>, label: &str) -> Response {
    ui.add_enabled_ui(enabled, |ui| button(ui, kind, icon, label)).inner
}

/// Square, icon-only ghost button with a tooltip. The tooltip doubles as the accessible label.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    icon_button_kind(ui, icon, tip, Kind::Ghost)
}

pub fn icon_button_kind(ui: &mut Ui, icon: &str, tip: &str, kind: Kind) -> Response {
    let mut b = Btn::new(kind, Some(icon), "");
    b.icon_size = font::ICON;
    b.a11y = tip;
    b.min_size = Vec2::splat(size::ICON_BUTTON);
    b.show(ui).on_hover_text(tip)
}

/// Small square ghost button used inside dense rows (document actions).
pub fn row_icon_button(ui: &mut Ui, icon: &str, tip: &str) -> Response {
    let mut b = Btn::new(Kind::Ghost, Some(icon), "");
    b.a11y = tip;
    b.min_size = Vec2::splat(24.0);
    b.show(ui).on_hover_text(tip)
}

/// Compact page-number button for the pager.
pub fn page_button(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let mut b = Btn::new(Kind::Ghost, None, label);
    b.font = Some(font::medium(font::SMALL));
    b.selected = selected;
    b.min_size = vec2(28.0, 28.0);
    b.show(ui)
}

/// A ghost "⋯" button that opens a menu.
pub fn more_menu(ui: &mut Ui, tip: &str, add: impl FnOnce(&mut Ui)) {
    let r = row_icon_button(ui, icon::MORE, tip);
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(210.0);
        add(ui)
    });
}

/// A full-width item in a popup menu.
pub fn menu_item(ui: &mut Ui, kind: Kind, icon: &str, label: &str) -> Response {
    let mut b = Btn::new(kind, Some(icon), label);
    b.full_width = true;
    b.show(ui)
}

// ---------------------------------------------------------------- inputs

/// Single-line text input with the standard height and padding.
pub fn input<'t>(text: &'t mut dyn egui::TextBuffer) -> TextEdit<'t> {
    TextEdit::singleline(text).margin(Margin::symmetric(10, 4)).align(Align2::LEFT_CENTER).min_size(vec2(0.0, size::CONTROL))
}

/// Text input with an icon painted at its left edge (search and query fields).
pub fn icon_input(ui: &mut Ui, icon: &str, edit: TextEdit<'_>) -> Response {
    let edit = edit.margin(Margin { left: 30, right: 10, top: 4, bottom: 4 }).align(Align2::LEFT_CENTER).min_size(vec2(0.0, size::CONTROL));
    let r = ui.add(edit);
    let p = pal(ui);
    let color = if r.has_focus() { p.text_muted } else { p.text_faint };
    ui.painter().text(
        egui::pos2(r.rect.left() + 15.0, r.rect.center().y),
        Align2::CENTER_CENTER,
        icon,
        font::icon(14.0),
        color,
    );
    r
}

/// iOS/macOS style switch. Keyboard: Space/Enter toggles when focused.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let galley = WidgetText::from(RichText::new(label).font(font::medium(font::BODY))).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    let track = vec2(30.0, 18.0);
    let desired = vec2(track.x + space::SM + galley.size().x, size::CONTROL);
    let (rect, mut r) = ui.allocate_exact_size(desired, Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let label_owned = label.to_string();
    let value = *on;
    r.widget_info(|| WidgetInfo::selected(WidgetType::CheckBox, true, value, &label_owned));
    let t = ui.ctx().animate_bool_responsive(r.id, *on);
    let track_rect = Rect::from_center_size(egui::pos2(rect.left() + track.x / 2.0, rect.center().y), track);
    let off_fill = if r.hovered() { p.control_hover } else { p.control_active };
    let fill = lerp_color(off_fill, p.accent, t);
    let painter = ui.painter();
    painter.rect_filled(track_rect, CornerRadius::same(9), fill);
    if r.has_focus() {
        painter.rect_stroke(track_rect.expand(2.0), CornerRadius::same(11), Stroke::new(1.5, p.accent), StrokeKind::Outside);
    }
    let knob_x = egui::lerp(track_rect.left() + 9.0..=track_rect.right() - 9.0, t);
    painter.circle_filled(egui::pos2(knob_x, track_rect.center().y), 7.0, Color32::WHITE);
    painter.galley(egui::pos2(track_rect.right() + space::SM, rect.center().y - galley.size().y / 2.0), galley, p.text_strong);
    r
}

/// Checkbox with an accent fill when checked. Keyboard: Space/Enter toggles when focused.
pub fn checkbox(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let p = pal(ui);
    let galley = ui.painter().layout(label.to_string(), font::regular(font::BODY), Color32::PLACEHOLDER, (ui.available_width() - 24.0).max(80.0));
    let desired = vec2(24.0 + galley.size().x, galley.size().y.max(22.0));
    let (rect, mut r) = ui.allocate_exact_size(desired, Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let (lbl, value) = (label.to_string(), *on);
    r.widget_info(|| WidgetInfo::selected(WidgetType::CheckBox, true, value, &lbl));
    let bx = Rect::from_min_size(egui::pos2(rect.left(), rect.top() + (22.0 - 16.0) / 2.0), Vec2::splat(16.0));
    let painter = ui.painter();
    if *on {
        painter.rect_filled(bx, CornerRadius::same(radius::SM), if r.hovered() { p.accent_hover } else { p.accent });
        let pts = vec![bx.left_top() + vec2(4.0, 8.2), bx.left_top() + vec2(7.0, 11.0), bx.left_top() + vec2(12.2, 5.2)];
        painter.line(pts, Stroke::new(1.8, p.on_accent));
    } else {
        let stroke = if r.hovered() { p.text_faint } else { p.border_strong };
        painter.rect(bx, CornerRadius::same(radius::SM), p.input, Stroke::new(1.0, stroke), StrokeKind::Inside);
    }
    if r.has_focus() {
        painter.rect_stroke(bx.expand(2.0), CornerRadius::same(radius::SM + 2), Stroke::new(1.5, p.accent), StrokeKind::Outside);
    }
    let color = if r.hovered() { p.text_strong } else { p.text };
    painter.galley(egui::pos2(rect.left() + 24.0, rect.top() + (22.0 - galley.rows[0].height()) / 2.0), galley, color);
    r
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A row of mutually exclusive options in one rounded container (auth type, certificate type…).
/// Each option is `(value, label, tooltip)`. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str, &str)]) -> bool {
    let p = pal(ui);
    let mut changed = false;
    let pad = 3.0;
    let font_id = font::regular(font::BODY - 0.5);
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, label, _)| ui.painter().layout_no_wrap(label.to_string(), font_id.clone(), Color32::PLACEHOLDER))
        .collect();
    let seg_w: Vec<f32> = galleys.iter().map(|g| g.size().x + 22.0).collect();
    let total = vec2(seg_w.iter().sum::<f32>() + pad * 2.0, size::CONTROL);
    let (outer, _) = ui.allocate_exact_size(total, Sense::hover());
    ui.painter().rect(outer, CornerRadius::same(radius::LG), p.input, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let mut x = outer.left() + pad;
    for (i, ((val, label, tip), galley)) in options.iter().zip(galleys).enumerate() {
        let rect = Rect::from_min_size(egui::pos2(x, outer.top() + pad), vec2(seg_w[i], outer.height() - pad * 2.0));
        x += seg_w[i];
        let id = ui.scope_id().with(("seg", label));
        let r = ui.interact(rect, id, Sense::click());
        let selected = *value == *val;
        let lbl = label.to_string();
        r.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, selected, &lbl));
        let r = if tip.is_empty() { r } else { r.on_hover_text(*tip) };
        if r.clicked() && !selected {
            *value = *val;
            changed = true;
        }
        let (fill, color) = if selected {
            (p.control_active, p.text_strong)
        } else if r.hovered() {
            (p.control, p.text)
        } else {
            (Color32::TRANSPARENT, p.text_muted)
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(radius::MD), fill);
        if r.has_focus() {
            painter.rect_stroke(rect, CornerRadius::same(radius::MD), Stroke::new(1.5, p.accent), StrokeKind::Inside);
        }
        painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    changed
}

/// Row of color swatches for a saved connection.
pub fn color_picker(ui: &mut Ui, value: &mut usize) -> bool {
    let p = pal(ui);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, c) in theme::CONN_COLORS.iter().enumerate() {
            let (rect, r) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
            let names = ["Blue", "Green", "Yellow", "Orange", "Red", "Purple", "Teal", "Gray"];
            let selected = *value == i;
            r.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, selected, names[i]));
            let painter = ui.painter();
            if selected {
                painter.circle_stroke(rect.center(), 10.0, Stroke::new(2.0, *c));
                painter.circle_filled(rect.center(), 6.5, *c);
            } else {
                painter.circle_filled(rect.center(), if r.hovered() { 8.0 } else { 7.0 }, *c);
            }
            if r.has_focus() {
                painter.circle_stroke(rect.center(), 11.5, Stroke::new(1.0, p.accent));
            }
            if r.clicked() {
                *value = i;
                changed = true;
            }
        }
    });
    changed
}

// ---------------------------------------------------------------- text helpers

pub fn section_label(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.label(RichText::new(text.to_uppercase()).font(font::medium(font::TINY)).color(p.text_faint).extra_letter_spacing(0.6));
}

pub fn muted(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    ui.label(RichText::new(text.into()).color(p.text_muted))
}

pub fn small_muted(ui: &mut Ui, text: impl Into<String>) -> Response {
    let p = pal(ui);
    ui.label(RichText::new(text.into()).font(font::regular(font::SMALL)).color(p.text_muted))
}

/// A keyboard hint like `⏎` or `⌘⏎`, drawn as a small keycap.
pub fn kbd(ui: &mut Ui, keys: &str) {
    let p = pal(ui);
    Frame::new()
        .fill(p.control)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::SM))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(keys).font(font::regular(font::TINY)).color(p.text_muted));
        });
}

// ---------------------------------------------------------------- containers

/// Bordered card on the workspace background.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let p = pal(ui);
    Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::XL))
        .inner_margin(Margin::same(space::LG as i8 + 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Title and one-line description at the top of a card.
pub fn card_header(ui: &mut Ui, icon_str: &str, title_text: &str, description: &str) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon_str).font(font::icon(14.0)).color(p.text_muted));
        ui.label(RichText::new(title_text).font(font::semibold(font::BODY)).color(p.text_strong));
    });
    if !description.is_empty() {
        ui.add_space(-2.0);
        ui.label(RichText::new(description).font(font::regular(font::SMALL)).color(p.text_muted));
    }
    ui.add_space(space::SM);
}

/// Two-column form row: muted label on the left, control on the right.
/// Use inside `form_grid`.
pub fn form_label(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.allocate_ui_with_layout(vec2(size::FORM_LABEL, size::CONTROL), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(vec2(size::FORM_LABEL, size::CONTROL));
        ui.label(RichText::new(text).color(p.text_muted));
    });
}

pub fn form_grid<R>(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Grid::new(id).num_columns(2).spacing([space::MD, space::SM]).min_col_width(size::FORM_LABEL).show(ui, add).inner
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Error,
    Info,
}

/// Tinted message box with an icon. The text is selectable so errors can be copied.
pub fn callout(ui: &mut Ui, tone: Tone, title_text: Option<&str>, body: &str) {
    let p = pal(ui);
    let (bg, fg, ic) = match tone {
        Tone::Error => (p.danger_soft, p.danger, icon::ALERT),
        Tone::Info => (p.accent_soft, p.accent_text, icon::INFO),
    };
    Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0, fg.gamma_multiply(0.35)))
        .corner_radius(CornerRadius::same(radius::LG))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.label(RichText::new(ic).font(font::icon(15.0)).color(fg));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if let Some(t) = title_text {
                        ui.label(RichText::new(t).font(font::medium(font::BODY)).color(fg));
                    }
                    ui.add(egui::Label::new(RichText::new(body).color(p.text)).wrap().selectable(true));
                });
            });
        });
}

/// Centered icon, title and hint for empty or idle views.
pub fn empty_state(ui: &mut Ui, ic: &str, title_text: &str, body: &str) {
    let p = pal(ui);
    ui.vertical_centered(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
        ui.painter().rect(rect, CornerRadius::same(radius::XL), p.surface, Stroke::new(1.0, p.border), StrokeKind::Inside);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, ic, font::icon(20.0), p.text_muted);
        ui.add_space(space::MD);
        ui.label(RichText::new(title_text).font(font::semibold(font::TITLE)).color(p.text_strong));
        ui.add_space(space::XXS);
        ui.set_max_width(360.0);
        ui.label(RichText::new(body).color(p.text_muted));
    });
}

/// Compact "label value" statistic, laid out as one unit so wrapping rows never split it.
pub fn stat(ui: &mut Ui, label: &str, value: &str, tip: Option<&str>) {
    let p = pal(ui);
    let lg = ui.painter().layout_no_wrap(label.to_string(), font::regular(font::SMALL), p.text_muted);
    let vg = ui.painter().layout_no_wrap(value.to_string(), font::medium(font::SMALL), p.text);
    let pad = vec2(8.0, 0.0);
    let gap = 5.0;
    let size = vec2(pad.x * 2.0 + lg.size().x + gap + vg.size().x, 22.0);
    let (rect, r) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(radius::MD), p.surface, Stroke::new(1.0, p.border), StrokeKind::Inside);
    let y = rect.center().y - lg.size().y / 2.0;
    let lx = rect.left() + pad.x;
    let vx = lx + lg.size().x + gap;
    painter.galley(egui::pos2(lx, y), lg, p.text_muted);
    painter.galley(egui::pos2(vx, y), vg, p.text);
    if let Some(t) = tip {
        r.on_hover_text(t);
    }
}

/// Horizontal hairline that spans the available width.
pub fn divider(ui: &mut Ui) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, p.divider));
}

/// Standard dialog frame: title, optional description, body and a right-aligned footer.
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    title_text: &str,
    description: Option<&str>,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let p = theme::palette(ctx.global_style().visuals.dark_mode);
    let frame = Frame::new()
        .fill(p.elevated)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(radius::XL))
        .inner_margin(Margin::same(space::XL as i8))
        .shadow(egui::Shadow { offset: [0, 16], blur: 40, spread: 0, color: p.shadow });
    egui::Modal::new(egui::Id::unique(id))
        .frame(frame)
        .backdrop_color(p.backdrop)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.label(RichText::new(title_text).font(font::semibold(17.0)).color(p.text_strong));
            if let Some(d) = description {
                ui.add_space(space::XXS);
                ui.label(RichText::new(d).color(p.text_muted));
            }
            ui.add_space(space::LG);
            body(ui)
        })
        .inner
}

/// Footer row of a dialog: content is laid out right-to-left, so add the primary button first.
pub fn dialog_footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(space::LG);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = space::SM;
        add(ui)
    });
}

/// Paints a rotating chevron for collapsing headers (replaces egui's triangle).
pub fn chevron_icon(ui: &mut Ui, openness: f32, response: &Response) {
    let p = pal(ui);
    let color = if response.hovered() { p.text } else { p.text_faint };
    let c = response.rect.center();
    let s = 3.2;
    let rot = egui::emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
    let pts = [vec2(-s * 0.5, -s), vec2(s * 0.5, 0.0), vec2(-s * 0.5, s)].map(|v| c + rot * v);
    ui.painter().line(pts.to_vec(), Stroke::new(1.4, color));
}
