use crate::{
    conn::{self, AuthKind, CaSource, ClientCert, Profile},
    shell,
    worker::{self, Cmd, CollStats, DbInfo, DbStats, Event},
};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, TextEdit, TextStyle};
use mongodb::bson::{Bson, Document};
use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0x4F, 0xA3, 0xF7), // blue
    Color32::from_rgb(0x5C, 0xC9, 0x8B), // green
    Color32::from_rgb(0xF2, 0xC1, 0x4E), // yellow
    Color32::from_rgb(0xF2, 0x8C, 0x38), // orange
    Color32::from_rgb(0xEF, 0x5B, 0x5B), // red
    Color32::from_rgb(0xB0, 0x7C, 0xF0), // purple
    Color32::from_rgb(0x4D, 0xD0, 0xC8), // teal
    Color32::from_rgb(0x9A, 0xA5, 0xB1), // gray
];
const ACCENT: Color32 = Color32::from_rgb(0x4F, 0xA3, 0xF7);
const DANGER: Color32 = Color32::from_rgb(0xEF, 0x5B, 0x5B);
const PAGE_SIZES: [u64; 3] = [20, 50, 100];

// Icons from egui's bundled icon font, so they render the same on every machine.
const I_PLUS: &str = "➕";
const I_EDIT: &str = "✏";
const I_TRASH: &str = "🗑";
const I_COPY: &str = "📋";
const I_CLONE: &str = "⎘";
const I_REFRESH: &str = "🔄";
const I_SEARCH: &str = "🔍";
const I_RESET: &str = "✖";
const I_PREV: &str = "⏴";
const I_NEXT: &str = "⏵";
const I_FOLDER: &str = "🗁";
const I_PLUG: &str = "⏻";
const I_DB: &str = "🗄";

struct DbNode {
    info: DbInfo,
    collections: Option<Vec<String>>,
    requested: bool,
    stats: Option<DbStats>,
}

enum CountState {
    Loading,
    Known { total: u64, estimated: bool },
    /// Counting took too long; paging falls back to previous/next only.
    Unknown,
}

struct CollView {
    db: String,
    coll: String,
    filter_text: String,
    sort_text: String,
    filter: Document,
    sort: Document,
    page: u64,
    page_size: u64,
    docs: Vec<Document>,
    has_more: bool,
    loading: bool,
    error: Option<String>,
    query_error: Option<String>,
    count: CountState,
    req: u64,
    elapsed: Option<Duration>,
    stats: Option<Result<CollStats, String>>,
    jump_text: String,
    expand_gen: u64,
}

enum Modal {
    None,
    Editor { title: String, text: String, original_id: Option<Bson>, error: Option<String>, focus: bool },
    ConfirmDelete { id: Bson, preview: String },
    ConfirmDeleteProfile { index: usize },
    CompassImport { read_keychain: bool },
    Export { include_shared: bool },
}

enum Action {
    OpenCollection(String, String),
    RunQuery,
    ResetQuery,
    GoToPage(u64),
    SetPageSize(u64),
    Reload,
    EditDoc(usize),
    CloneDoc(usize),
    DeleteDoc(usize),
    InsertDoc,
    Disconnect,
}

pub struct App {
    worker: worker::Handle,
    events: Receiver<Event>,
    profiles: Vec<Profile>,
    selected: Option<usize>,
    form: Profile,
    form_dirty: bool,
    uri_paste: String,
    show_passwords: bool,
    connecting: bool,
    connect_error: Option<String>,
    active: Option<Profile>,
    dbs: Vec<DbNode>,
    tree_filter: String,
    view: Option<CollView>,
    modal: Modal,
    toast: Option<(String, Instant, bool)>,
    next_req: u64,
    dark: bool,
}

fn apply_theme(ctx: &egui::Context, dark: bool) {
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    visuals.selection.bg_fill = if dark { Color32::from_rgb(0x1F, 0x4E, 0x7A) } else { Color32::from_rgb(0xBF, 0xDD, 0xFB) };
    visuals.hyperlink_color = ACCENT;
    if dark {
        visuals.panel_fill = Color32::from_rgb(0x1B, 0x1E, 0x24);
        visuals.window_fill = Color32::from_rgb(0x22, 0x26, 0x2E);
        visuals.extreme_bg_color = Color32::from_rgb(0x12, 0x14, 0x18);
        visuals.faint_bg_color = Color32::from_rgb(0x22, 0x26, 0x2E);
    }
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        for (style, font) in s.text_styles.iter_mut() {
            font.size = match style {
                TextStyle::Heading => 20.0,
                TextStyle::Small => 11.5,
                TextStyle::Monospace => 13.0,
                _ => 14.0,
            };
        }
    });
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark = true;
        apply_theme(&cc.egui_ctx, dark);
        let ctx = cc.egui_ctx.clone();
        let (worker, events) = worker::spawn(move || ctx.request_repaint());
        let profiles = conn::load_profiles();
        let selected = if profiles.is_empty() { None } else { Some(0) };
        let form = profiles.first().cloned().unwrap_or_default();
        let mut app = Self {
            worker,
            events,
            profiles,
            selected,
            form,
            form_dirty: false,
            uri_paste: String::new(),
            show_passwords: false,
            connecting: false,
            connect_error: None,
            active: None,
            dbs: vec![],
            tree_filter: String::new(),
            view: None,
            modal: Modal::None,
            toast: None,
            next_req: 1,
            dark,
        };
        // Handy for demos and scripted checks: MONGUI_AUTOCONNECT="<saved connection name>".
        if let Ok(name) = std::env::var("MONGUI_AUTOCONNECT") {
            if let Some(i) = app.profiles.iter().position(|p| p.name == name) {
                app.select_profile(Some(i));
                app.connect();
            }
        }
        app
    }

    fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), Instant::now(), is_error));
    }

    fn select_profile(&mut self, idx: Option<usize>) {
        self.selected = idx;
        self.form = idx.and_then(|i| self.profiles.get(i).cloned()).unwrap_or_default();
        self.form_dirty = false;
        self.connect_error = None;
        self.uri_paste.clear();
    }

    fn save_form(&mut self) -> bool {
        if self.form.name.trim().is_empty() {
            self.form.name = self.form.display_name();
        }
        match self.selected {
            Some(i) if i < self.profiles.len() => self.profiles[i] = self.form.clone(),
            _ => {
                self.profiles.push(self.form.clone());
                self.selected = Some(self.profiles.len() - 1);
            }
        }
        self.form_dirty = false;
        self.persist()
    }

    fn persist(&mut self) -> bool {
        match conn::save_profiles(&self.profiles) {
            Ok(()) => true,
            Err(e) => {
                self.toast(format!("Could not save connections: {e}"), true);
                false
            }
        }
    }

    fn connect(&mut self) {
        if let Err(e) = self.form.validate() {
            self.connect_error = Some(e.to_string());
            return;
        }
        self.save_form();
        self.connecting = true;
        self.connect_error = None;
        self.worker.send(Cmd::Connect(Box::new(self.form.clone())));
    }

    fn alloc_req(&mut self) -> u64 {
        self.next_req += 1;
        self.next_req
    }

    fn run_query(&mut self, recount: bool) {
        let req = self.alloc_req();
        let Some(v) = self.view.as_mut() else { return };
        v.req = req;
        v.loading = true;
        v.error = None;
        // One extra row tells us whether a next page exists even when the total is unknown.
        self.worker.send(Cmd::Query {
            req,
            db: v.db.clone(),
            coll: v.coll.clone(),
            filter: v.filter.clone(),
            sort: v.sort.clone(),
            skip: v.page * v.page_size,
            limit: v.page_size as i64 + 1,
        });
        if recount {
            v.count = CountState::Loading;
            self.worker.send(Cmd::Count { req, db: v.db.clone(), coll: v.coll.clone(), filter: v.filter.clone() });
        }
    }

    fn open_collection(&mut self, db: String, coll: String) {
        self.view = Some(CollView {
            db: db.clone(),
            coll: coll.clone(),
            filter_text: String::new(),
            sort_text: String::new(),
            filter: Document::new(),
            sort: Document::new(),
            page: 0,
            page_size: self.view.as_ref().map(|v| v.page_size).unwrap_or(PAGE_SIZES[0]),
            docs: vec![],
            has_more: false,
            loading: true,
            error: None,
            query_error: None,
            count: CountState::Loading,
            req: 0,
            elapsed: None,
            stats: None,
            jump_text: String::new(),
            expand_gen: 0,
        });
        self.run_query(true);
        self.worker.send(Cmd::CollStats { db, coll });
    }

    fn apply_query_text(&mut self) {
        let Some(v) = self.view.as_mut() else { return };
        let filter = match shell::parse_document(&v.filter_text) {
            Ok(f) => f,
            Err(e) => {
                v.query_error = Some(format!("Filter: {e}"));
                return;
            }
        };
        let sort = match shell::parse_document(&v.sort_text) {
            Ok(s) => s,
            Err(e) => {
                v.query_error = Some(format!("Sort: {e}"));
                return;
            }
        };
        v.query_error = None;
        v.filter = filter;
        v.sort = sort;
        v.page = 0;
        self.run_query(true);
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Connected { profile_id } => {
                    self.connecting = false;
                    self.active = self.profiles.iter().find(|p| p.id == profile_id).cloned().or(Some(self.form.clone()));
                    self.dbs.clear();
                    self.view = None;
                    self.tree_filter.clear();
                }
                Event::ConnectFailed(e) => {
                    if !self.connecting {
                        continue; // cancelled meanwhile
                    }
                    self.connecting = false;
                    self.connect_error = Some(e);
                }
                Event::Disconnected => {
                    self.active = None;
                    self.view = None;
                    self.dbs.clear();
                }
                Event::Databases(list) => {
                    self.dbs = list
                        .into_iter()
                        .map(|info| DbNode { info, collections: None, requested: false, stats: None })
                        .collect();
                }
                Event::Collections { db, names } => {
                    if let Some(n) = self.dbs.iter_mut().find(|n| n.info.name == db) {
                        n.collections = Some(names);
                    }
                }
                Event::DbStats { db, stats } => {
                    if let Some(n) = self.dbs.iter_mut().find(|n| n.info.name == db) {
                        n.stats = Some(stats);
                    }
                }
                Event::Docs { req, mut docs, elapsed } => {
                    if let Some(v) = self.view.as_mut().filter(|v| v.req == req) {
                        v.has_more = docs.len() as u64 > v.page_size;
                        docs.truncate(v.page_size as usize);
                        v.docs = docs;
                        v.loading = false;
                        v.elapsed = Some(elapsed);
                    }
                }
                Event::QueryFailed { req, error } => {
                    if let Some(v) = self.view.as_mut().filter(|v| v.req == req) {
                        v.loading = false;
                        v.docs.clear();
                        v.has_more = false;
                        v.error = Some(error);
                    }
                }
                Event::Count { req, total, estimated } => {
                    if let Some(v) = self.view.as_mut().filter(|v| v.req == req) {
                        v.count = match total {
                            Some(total) => CountState::Known { total, estimated },
                            None => CountState::Unknown,
                        };
                    }
                }
                Event::CollStats { db, coll, stats } => {
                    if let Some(v) = self.view.as_mut().filter(|v| v.db == db && v.coll == coll) {
                        v.stats = Some(stats);
                    }
                }
                Event::Written(msg) => {
                    self.toast(msg, false);
                    self.run_query(true);
                    if let Some(v) = &self.view {
                        self.worker.send(Cmd::CollStats { db: v.db.clone(), coll: v.coll.clone() });
                    }
                }
                Event::Error(e) => self.toast(e, true),
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::OpenCollection(db, coll) => self.open_collection(db, coll),
            Action::RunQuery => self.apply_query_text(),
            Action::ResetQuery => {
                if let Some(v) = self.view.as_mut() {
                    v.filter_text.clear();
                    v.sort_text.clear();
                }
                self.apply_query_text();
            }
            Action::GoToPage(p) => {
                if let Some(v) = self.view.as_mut() {
                    v.page = p;
                    v.jump_text.clear();
                }
                self.run_query(false);
            }
            Action::SetPageSize(n) => {
                if let Some(v) = self.view.as_mut() {
                    // Keep the first visible document on screen when the page size changes.
                    let first = v.page * v.page_size;
                    v.page_size = n;
                    v.page = first / n;
                }
                self.run_query(false);
            }
            Action::Reload => {
                self.run_query(true);
                if let Some(v) = &self.view {
                    self.worker.send(Cmd::CollStats { db: v.db.clone(), coll: v.coll.clone() });
                }
            }
            Action::EditDoc(i) => {
                if let Some(d) = self.view.as_ref().and_then(|v| v.docs.get(i)) {
                    self.modal = Modal::Editor {
                        title: "Edit document".into(),
                        text: shell::doc_to_shell(d),
                        original_id: Some(d.get("_id").cloned().unwrap_or(Bson::Null)),
                        error: None,
                        focus: true,
                    };
                }
            }
            Action::CloneDoc(i) => {
                if let Some(d) = self.view.as_ref().and_then(|v| v.docs.get(i)) {
                    let mut copy = d.clone();
                    copy.remove("_id");
                    self.modal = Modal::Editor {
                        title: "Insert a copy".into(),
                        text: shell::doc_to_shell(&copy),
                        original_id: None,
                        error: None,
                        focus: true,
                    };
                }
            }
            Action::InsertDoc => {
                self.modal = Modal::Editor {
                    title: "Insert document".into(),
                    text: "{\n  \n}".into(),
                    original_id: None,
                    error: None,
                    focus: true,
                };
            }
            Action::DeleteDoc(i) => {
                if let Some(d) = self.view.as_ref().and_then(|v| v.docs.get(i)) {
                    let mut preview = shell::doc_to_shell(d);
                    if preview.len() > 600 {
                        let cut = (0..=600).rev().find(|i| preview.is_char_boundary(*i)).unwrap_or(0);
                        preview.truncate(cut);
                        preview.push_str("\n…");
                    }
                    self.modal = Modal::ConfirmDelete { id: d.get("_id").cloned().unwrap_or(Bson::Null), preview };
                }
            }
            Action::Disconnect => self.worker.send(Cmd::Disconnect),
        }
    }
}

// ---------------------------------------------------------------- helpers

#[derive(Debug, PartialEq)]
enum PageItem {
    Page(u64),
    Gap,
    Jump,
}

/// Pager layout (0-based pages): every page when there are fewer than 10, otherwise
/// `1 2 3 [… current] go-to last`. The gap shows only when pages are hidden before current.
fn page_items(cur: u64, total_pages: u64) -> Vec<PageItem> {
    if total_pages < 10 {
        return (0..total_pages).map(PageItem::Page).collect();
    }
    let last = total_pages - 1;
    let mut items: Vec<PageItem> = (0..3).map(PageItem::Page).collect();
    if cur > 3 {
        items.push(PageItem::Gap);
    }
    if cur > 2 && cur < last {
        items.push(PageItem::Page(cur));
    }
    items.push(PageItem::Jump);
    items.push(PageItem::Page(last));
    items
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{v:.0} {}", UNITS[u]) } else { format!("{v:.1} {}", UNITS[u]) }
}

/// A "label value" chip, painted as one fixed-size block so every chip has the same height and a
/// wrapping row moves it to the next line whole (never wrapping its text letter by letter).
fn stat_chip(ui: &mut egui::Ui, label: &str, value: &str) -> egui::Response {
    let galley = |t: RichText| egui::WidgetText::from(t).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, TextStyle::Body);
    let (l, r) = (galley(RichText::new(label)), galley(RichText::new(value).strong()));
    let (pad_x, gap) = (8.0, 5.0);
    let height = l.size().y.max(r.size().y) + 6.0;
    let size = egui::vec2(pad_x + l.size().x + gap + r.size().x + pad_x, height);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 6.0, ui.visuals().faint_bg_color);
        let lx = rect.left() + pad_x;
        let rx = lx + l.size().x + gap;
        painter.galley(egui::pos2(lx, rect.center().y - l.size().y / 2.0), l, ui.visuals().weak_text_color());
        painter.galley(egui::pos2(rx, rect.center().y - r.size().y / 2.0), r, ui.visuals().strong_text_color());
    }
    response
}

fn icon_button(ui: &mut egui::Ui, icon: &str, tip: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(icon).size(15.0))).on_hover_text(tip).clicked()
}

fn value_color(v: &Bson, dark: bool) -> Color32 {
    let (d, l) = match v {
        Bson::String(_) => (Color32::from_rgb(0xE6, 0xB6, 0x73), Color32::from_rgb(0x9A, 0x5B, 0x00)),
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => {
            (Color32::from_rgb(0x7F, 0xD1, 0xE8), Color32::from_rgb(0x00, 0x6C, 0x8C))
        }
        Bson::Boolean(_) => (Color32::from_rgb(0xC3, 0x9A, 0xF0), Color32::from_rgb(0x6B, 0x3F, 0xB5)),
        Bson::Null | Bson::Undefined => (Color32::from_gray(0x8A), Color32::from_gray(0x70)),
        Bson::ObjectId(_) => (Color32::from_rgb(0xF0, 0x9A, 0x9A), Color32::from_rgb(0xA8, 0x3A, 0x3A)),
        Bson::DateTime(_) | Bson::Timestamp(_) => (Color32::from_rgb(0x8F, 0xD6, 0x94), Color32::from_rgb(0x2C, 0x7A, 0x36)),
        _ => (Color32::from_gray(0xC8), Color32::from_gray(0x40)),
    };
    if dark { d } else { l }
}

fn scalar_text(v: &Bson) -> String {
    let s = shell::format_scalar(v);
    if s.chars().count() > 400 {
        let cut: String = s.chars().take(400).collect();
        format!("{cut}…")
    } else {
        s
    }
}

fn render_fields(ui: &mut egui::Ui, doc: &Document, id: egui::Id, dark: bool, expand: Option<bool>) {
    for (k, v) in doc {
        render_value(ui, &shell::format_key(k), v, id.with(k), dark, expand);
    }
}

fn render_value(ui: &mut egui::Ui, key: &str, v: &Bson, id: egui::Id, dark: bool, expand: Option<bool>) {
    let key_text = RichText::new(format!("{key}:")).monospace().strong();
    match v {
        Bson::Document(d) if !d.is_empty() => {
            let n = d.len();
            egui::CollapsingHeader::new(RichText::new(format!("{key}: Object ({n})")).monospace())
                .id_salt(id)
                .open(expand)
                .show(ui, |ui| render_fields(ui, d, id, dark, expand));
        }
        Bson::Array(a) if !a.is_empty() => {
            let n = a.len();
            egui::CollapsingHeader::new(RichText::new(format!("{key}: Array ({n})")).monospace())
                .id_salt(id)
                .open(expand)
                .show(ui, |ui| {
                    // Huge arrays are capped so one document cannot stall the UI.
                    for (i, item) in a.iter().take(200).enumerate() {
                        render_value(ui, &i.to_string(), item, id.with(i), dark, expand);
                    }
                    if n > 200 {
                        ui.weak(format!("… {} more items. Open the editor to see all.", n - 200));
                    }
                });
        }
        _ => {
            let text = match v {
                Bson::Document(_) => "{}".to_string(),
                Bson::Array(_) => "[]".to_string(),
                other => scalar_text(other),
            };
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(key_text);
                ui.label(RichText::new(text).monospace().color(value_color(v, dark)));
            });
        }
    }
}

fn path_row(ui: &mut egui::Ui, label: &str, path: &mut String, hint: &str, filter: (&str, &[&str])) -> bool {
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 44.0).max(120.0);
        changed |= ui.add(TextEdit::singleline(path).desired_width(w).hint_text(hint)).changed();
        if icon_button(ui, I_FOLDER, "Choose file") {
            let mut dlg = rfd::FileDialog::new();
            if !filter.1.is_empty() {
                dlg = dlg.add_filter(filter.0, filter.1).add_filter("All files", &["*"]);
            }
            if let Some(p) = dlg.pick_file() {
                *path = p.display().to_string();
                changed = true;
            }
        }
    });
    ui.end_row();
    changed
}

fn password_row(ui: &mut egui::Ui, label: &str, value: &mut String, show: bool, hint: &str) -> bool {
    ui.label(label);
    let r = ui.add(TextEdit::singleline(value).password(!show).desired_width(f32::INFINITY).hint_text(hint));
    ui.end_row();
    r.changed()
}

// ---------------------------------------------------------------- screens

impl App {
    fn connections_screen(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("saved").resizable(true).default_size(270.0).show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Connections");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, I_PLUS, "New connection") {
                        self.select_profile(None);
                    }
                });
            });
            ui.add_space(4.0);
            let mut clicked = None;
            let mut double = None;
            egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(ui.available_height() - 90.0).show(ui, |ui| {
                if self.profiles.is_empty() {
                    ui.weak("No saved connections yet.\nFill in the form, or import the ones you already have in Compass.");
                }
                for (i, p) in self.profiles.iter().enumerate() {
                    let selected = self.selected == Some(i);
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::click());
                    let visuals = ui.visuals();
                    let bg = if selected {
                        visuals.selection.bg_fill
                    } else if resp.hovered() {
                        visuals.widgets.hovered.weak_bg_fill
                    } else {
                        Color32::TRANSPARENT
                    };
                    let painter = ui.painter();
                    painter.rect_filled(rect, 6.0, bg);
                    let stripe = egui::Rect::from_min_size(rect.min + egui::vec2(3.0, 4.0), egui::vec2(12.0, rect.height() - 8.0));
                    painter.rect_filled(stripe, 4.0, PALETTE[p.color % PALETTE.len()]);
                    painter.text(
                        rect.min + egui::vec2(24.0, 5.0),
                        egui::Align2::LEFT_TOP,
                        p.display_name(),
                        egui::FontId::proportional(14.5),
                        visuals.strong_text_color(),
                    );
                    let sub: String = p.hosts.chars().take(38).collect();
                    painter.text(
                        rect.min + egui::vec2(24.0, 25.0),
                        egui::Align2::LEFT_TOP,
                        sub,
                        egui::FontId::proportional(11.5),
                        visuals.weak_text_color(),
                    );
                    if resp.clicked() {
                        clicked = Some(i);
                    }
                    if resp.double_clicked() {
                        double = Some(i);
                    }
                }
            });
            if let Some(i) = clicked {
                self.select_profile(Some(i));
            }
            if let Some(i) = double {
                self.select_profile(Some(i));
                self.connect();
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Import from Compass").clicked() {
                    self.modal = Modal::CompassImport { read_keychain: false };
                }
                if ui.button("Import file").on_hover_text("Load connections a teammate exported").clicked() {
                    if let Some(path) = rfd::FileDialog::new().add_filter("mongui connections", &["json"]).pick_file() {
                        match conn::import_profiles(&path) {
                            Ok(list) => {
                                let n = list.len();
                                self.profiles.extend(list);
                                self.persist();
                                self.toast(format!("{n} connections imported"), false);
                            }
                            Err(e) => self.toast(format!("Import failed: {e:#}"), true),
                        }
                    }
                }
                if ui.add_enabled(!self.profiles.is_empty(), egui::Button::new("Export")).on_hover_text("Save all connections to a file you can share").clicked() {
                    self.modal = Modal::Export { include_shared: true };
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(680.0);
                ui.add_space(8.0);
                ui.heading(if self.selected.is_some() { "Edit connection" } else { "New connection" });
                ui.add_space(6.0);
                let mut dirty = false;
                let f = &mut self.form;

                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.uri_paste)
                            .desired_width(ui.available_width() - 70.0)
                            .hint_text("Optional: paste a mongodb:// connection string to fill the form"),
                    );
                    if ui.add_enabled(!self.uri_paste.trim().is_empty(), egui::Button::new("Fill")).clicked() {
                        match conn::profile_from_uri(&self.uri_paste) {
                            Ok(mut p) => {
                                p.id = f.id.clone();
                                p.name = f.name.clone();
                                p.color = f.color;
                                *f = p;
                                dirty = true;
                                self.uri_paste.clear();
                                self.connect_error = None;
                            }
                            Err(e) => self.connect_error = Some(e.to_string()),
                        }
                    }
                });
                ui.add_space(6.0);

                egui::Grid::new("general").num_columns(2).spacing([12.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                    ui.label("Name");
                    dirty |= ui.add(TextEdit::singleline(&mut f.name).desired_width(f32::INFINITY).hint_text("e.g. rx-notifications QA")).changed();
                    ui.end_row();

                    ui.label("Color");
                    ui.horizontal(|ui| {
                        for (i, c) in PALETTE.iter().enumerate() {
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::click());
                            ui.painter().circle_filled(rect.center(), 8.0, *c);
                            if f.color == i {
                                ui.painter().circle_stroke(rect.center(), 10.0, egui::Stroke::new(2.0, ui.visuals().strong_text_color()));
                            }
                            if resp.clicked() {
                                f.color = i;
                                dirty = true;
                            }
                        }
                    });
                    ui.end_row();

                    ui.label("Hosts");
                    dirty |= ui
                        .add(TextEdit::multiline(&mut f.hosts).desired_rows(2).desired_width(f32::INFINITY).hint_text("host1:27017,host2:27017"))
                        .changed();
                    ui.end_row();
                });

                ui.add_space(10.0);
                ui.strong("Sign in");
                ui.horizontal(|ui| {
                    dirty |= ui.selectable_value(&mut f.auth, AuthKind::Password, "Username / password").changed();
                    dirty |= ui
                        .selectable_value(&mut f.auth, AuthKind::Ldap, "LDAP (personal)")
                        .on_hover_text("Your own LDAP login. Its password is never included when you export connections.")
                        .changed();
                    dirty |= ui.selectable_value(&mut f.auth, AuthKind::None, "None").changed();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.checkbox(&mut self.show_passwords, "Show passwords");
                    });
                });
                if f.auth != AuthKind::None {
                    egui::Grid::new("auth").num_columns(2).spacing([12.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                        ui.label("Username");
                        dirty |= ui.add(TextEdit::singleline(&mut f.username).desired_width(f32::INFINITY)).changed();
                        ui.end_row();
                        dirty |= password_row(ui, "Password", &mut f.password, self.show_passwords, "");
                        if f.auth == AuthKind::Password {
                            ui.label("Auth database");
                            dirty |= ui.add(TextEdit::singleline(&mut f.auth_db).desired_width(f32::INFINITY).hint_text("admin")).changed();
                            ui.end_row();
                        }
                    });
                }

                ui.add_space(10.0);
                dirty |= ui.checkbox(&mut f.tls, RichText::new("TLS / SSL").strong()).changed();
                if f.tls {
                    ui.horizontal(|ui| {
                        ui.label("Client certificate");
                        dirty |= ui.selectable_value(&mut f.client_cert, ClientCert::Jks, "JKS keystore").on_hover_text("The same .jks file your Spring Boot apps use").changed();
                        dirty |= ui.selectable_value(&mut f.client_cert, ClientCert::Pem, "PEM file").on_hover_text("Certificate and key in one .pem, as in Compass").changed();
                        dirty |= ui.selectable_value(&mut f.client_cert, ClientCert::None, "None").changed();
                    });
                    if f.client_cert != ClientCert::None {
                        egui::Grid::new("cert").num_columns(2).spacing([12.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                            let (label, hint, filter): (&str, &str, (&str, &[&str])) = if f.client_cert == ClientCert::Jks {
                                ("Keystore file", "/path/to/app.jks", ("Keystores", &["jks", "p12", "pfx", "keystore"]))
                            } else {
                                ("Certificate + key", "/path/to/client.pem", ("PEM", &["pem", "crt", "key"]))
                            };
                            dirty |= path_row(ui, label, &mut f.client_cert_path, hint, filter);
                            let (plabel, phint) = if f.client_cert == ClientCert::Jks { ("Keystore password", "") } else { ("Key password", "only if the key is encrypted") };
                            dirty |= password_row(ui, plabel, &mut f.client_cert_password, self.show_passwords, phint);
                        });
                    }
                    if f.client_cert != ClientCert::Jks && f.ca == CaSource::Keystore {
                        f.ca = CaSource::System;
                    }
                    ui.horizontal(|ui| {
                        ui.label("Server CA");
                        if f.client_cert == ClientCert::Jks {
                            dirty |= ui
                                .selectable_value(&mut f.ca, CaSource::Keystore, "From client keystore")
                                .on_hover_text("Trust the CA certificates stored inside the JKS above")
                                .changed();
                        }
                        dirty |= ui
                            .selectable_value(&mut f.ca, CaSource::TrustStore, "cacerts / trust store")
                            .on_hover_text("The cacerts file you copy into each new JDK")
                            .changed();
                        dirty |= ui.selectable_value(&mut f.ca, CaSource::Pem, "CA .pem").on_hover_text("The Certificate Authority file used in Compass").changed();
                        dirty |= ui.selectable_value(&mut f.ca, CaSource::System, "System").on_hover_text("Certificates trusted by this computer").changed();
                    });
                    if matches!(f.ca, CaSource::TrustStore | CaSource::Pem) {
                        egui::Grid::new("ca").num_columns(2).spacing([12.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                            if f.ca == CaSource::TrustStore {
                                dirty |= path_row(ui, "Trust store file", &mut f.ca_path, "/path/to/cacerts", ("", &[]));
                                dirty |= password_row(ui, "Trust store password", &mut f.ca_password, self.show_passwords, "optional");
                            } else {
                                dirty |= path_row(ui, "CA file", &mut f.ca_path, "/path/to/ca.pem", ("PEM", &["pem", "crt", "cer"]));
                            }
                        });
                    }
                }

                ui.add_space(6.0);
                egui::CollapsingHeader::new("More options").id_salt("more").show(ui, |ui| {
                    egui::Grid::new("adv").num_columns(2).spacing([12.0, 8.0]).min_col_width(130.0).show(ui, |ui| {
                        ui.label("Replica set name");
                        dirty |= ui.add(TextEdit::singleline(&mut f.replica_set).desired_width(f32::INFINITY).hint_text("optional")).changed();
                        ui.end_row();
                        ui.label("Read preference");
                        egui::ComboBox::from_id_salt("rp")
                            .selected_text(if f.read_preference.is_empty() { "Default" } else { f.read_preference.as_str() })
                            .show_ui(ui, |ui| {
                                for (val, label) in [("", "Default"), ("primary", "primary"), ("primaryPreferred", "primaryPreferred"), ("secondary", "secondary"), ("secondaryPreferred", "secondaryPreferred"), ("nearest", "nearest")] {
                                    if ui.selectable_label(f.read_preference == val, label).clicked() {
                                        f.read_preference = val.to_string();
                                        dirty = true;
                                    }
                                }
                            });
                        ui.end_row();
                        ui.label("Default database").on_hover_text("Shown when your login may not list all databases");
                        dirty |= ui.add(TextEdit::singleline(&mut f.default_db).desired_width(f32::INFINITY).hint_text("optional")).changed();
                        ui.end_row();
                    });
                    dirty |= ui.checkbox(&mut f.srv, "Use mongodb+srv (DNS seed list)").changed();
                    if f.tls {
                        dirty |= ui
                            .checkbox(&mut f.insecure, "Skip server certificate checks (insecure)")
                            .on_hover_text("Only for troubleshooting. The server's identity is not verified.")
                            .changed();
                    }
                });
                self.form_dirty |= dirty;

                ui.add_space(12.0);
                if let Some(err) = &self.connect_error {
                    egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.0, DANGER)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(RichText::new(err).color(DANGER)).wrap().selectable(true));
                    });
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    let connect = egui::Button::new(RichText::new("Connect").strong().color(Color32::WHITE)).fill(Color32::from_rgb(0x2C, 0x7B, 0xE5));
                    if ui.add_enabled(!self.connecting, connect).clicked() {
                        self.connect();
                    }
                    if ui.add_enabled(self.form_dirty || self.selected.is_none(), egui::Button::new("Save")).clicked() && self.save_form() {
                        self.toast("Connection saved", false);
                    }
                    if self.connecting {
                        ui.spinner();
                        ui.label("Connecting…");
                        if ui.button(format!("{I_RESET}  Cancel")).on_hover_text("Stop trying to connect").clicked() {
                            self.worker.send(Cmd::CancelConnect);
                            self.connecting = false;
                        }
                    }
                    if let Some(i) = self.selected {
                        // Kept at the far edge, away from Connect and Save.
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.menu_button("⋯", |ui| {
                                if ui.button("Duplicate").clicked() {
                                    let mut copy = self.form.clone();
                                    copy.id = conn::new_id();
                                    copy.name = format!("{} copy", copy.display_name());
                                    self.profiles.push(copy);
                                    self.persist();
                                    self.select_profile(Some(self.profiles.len() - 1));
                                    ui.close();
                                }
                                if ui.button(RichText::new(format!("{I_TRASH}  Remove this connection…")).color(DANGER)).clicked() {
                                    self.modal = Modal::ConfirmDeleteProfile { index: i };
                                    ui.close();
                                }
                            })
                            .response
                            .on_hover_text("More");
                        });
                    }
                });
                ui.add_space(20.0);
            });
        });
    }

    /// Top bar drawn in every state, so the light/dark switch is always available.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        let mut disconnect = false;
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(active) = &self.active {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 18.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 6.0, PALETTE[active.color % PALETTE.len()]);
                    ui.label(RichText::new(active.display_name()).strong().size(15.0));
                    ui.weak(active.hosts.chars().take(80).collect::<String>());
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.active.is_some() && ui.button(format!("{I_PLUG}  Disconnect")).clicked() {
                        disconnect = true;
                    }
                    if icon_button(ui, if self.dark { "☀" } else { "🌙" }, "Switch light / dark") {
                        self.dark = !self.dark;
                        apply_theme(ctx, self.dark);
                    }
                });
            });
            ui.add_space(4.0);
        });
        if disconnect {
            self.apply(Action::Disconnect);
        }
    }

    fn browser_screen(&mut self, ui: &mut egui::Ui) {
        let mut actions: Vec<Action> = vec![];
        let dark = self.dark;

        egui::Panel::left("tree").resizable(true).default_size(260.0).show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut self.tree_filter).desired_width(ui.available_width() - 40.0).hint_text(format!("{I_SEARCH} Find database or collection")));
                if icon_button(ui, I_REFRESH, "Reload databases") {
                    self.worker.send(Cmd::ListDatabases);
                }
            });
            let needle = self.tree_filter.trim().to_lowercase();
            let current = self.view.as_ref().map(|v| (v.db.clone(), v.coll.clone()));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                if self.dbs.is_empty() {
                    ui.weak("No databases to show.");
                }
                for node in self.dbs.iter_mut() {
                    let db_matches = needle.is_empty() || node.info.name.to_lowercase().contains(&needle);
                    let coll_matches = |c: &String| needle.is_empty() || db_matches || c.to_lowercase().contains(&needle);
                    let any_coll = node.collections.as_ref().map(|cs| cs.iter().any(|c| c.to_lowercase().contains(&needle))).unwrap_or(false);
                    if !db_matches && !any_coll {
                        continue;
                    }
                    let mut header = egui::CollapsingHeader::new(RichText::new(format!("{I_DB} {}", node.info.name)).strong()).id_salt(("db", &node.info.name));
                    if !needle.is_empty() && any_coll {
                        header = header.open(Some(true));
                    }
                    let resp = header.show(ui, |ui| {
                        if !node.requested {
                            node.requested = true;
                            self.worker.send(Cmd::ListCollections { db: node.info.name.clone() });
                            self.worker.send(Cmd::DbStats { db: node.info.name.clone() });
                        }
                        if let Some(s) = &node.stats {
                            ui.weak(format!("{} collections · {} docs · {}", thousands(s.collections.max(0) as u64), thousands(s.objects.max(0) as u64), bytes(s.data_size)))
                                .on_hover_text(format!(
                                    "Data size: {}\nStorage size: {}\nIndexes: {} ({})",
                                    bytes(s.data_size), bytes(s.storage_size), s.indexes, bytes(s.index_size)
                                ));
                        }
                        match &node.collections {
                            None => {
                                ui.spinner();
                            }
                            Some(cs) if cs.is_empty() => {
                                ui.weak("No collections");
                            }
                            Some(cs) => {
                                for c in cs.iter().filter(|c| coll_matches(c)) {
                                    let selected = current.as_ref().is_some_and(|(d, cc)| d == &node.info.name && cc == c);
                                    if ui.selectable_label(selected, c).clicked() {
                                        actions.push(Action::OpenCollection(node.info.name.clone(), c.clone()));
                                    }
                                }
                            }
                        }
                    });
                    if node.info.size_on_disk > 0 {
                        resp.header_response.on_hover_text(format!("{} on disk", bytes(node.info.size_on_disk as i64)));
                    }
                }
            });
        });

        if self.view.is_some() {
            egui::Panel::bottom("pager").show(ui, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| self.pager(ui, &mut actions));
                ui.add_space(4.0);
            });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let Some(v) = self.view.as_mut() else {
                ui.centered_and_justified(|ui| {
                    ui.weak("Pick a collection on the left to browse its documents.");
                });
                return;
            };
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{}.{}", v.db, v.coll)).heading());
                match &v.stats {
                    Some(Ok(s)) => {
                        ui.add_space(10.0);
                        for (label, value) in [
                            ("Documents", thousands(s.count.max(0) as u64)),
                            ("Data", bytes(s.size)),
                            ("Avg doc", bytes(s.avg_obj_size)),
                            ("On disk", bytes(s.storage_size)),
                            ("Indexes", format!("{} ({})", s.index_count, bytes(s.total_index_size))),
                        ] {
                            stat_chip(ui, label, &value);
                        }
                    }
                    Some(Err(e)) => {
                        ui.add_space(10.0);
                        stat_chip(ui, "Stats", "unavailable").on_hover_text(e);
                    }
                    None => {}
                }
            });
            ui.add_space(2.0);

            ui.horizontal(|ui| {
                let total_w = ui.available_width() - 250.0;
                let fw = (total_w * 0.68).max(160.0);
                let sw = (total_w * 0.32).max(100.0);
                ui.label("Filter");
                let fr = ui.add(TextEdit::singleline(&mut v.filter_text).font(TextStyle::Monospace).desired_width(fw).hint_text("{ field: 'value' }"));
                ui.label("Sort");
                let sr = ui.add(TextEdit::singleline(&mut v.sort_text).font(TextStyle::Monospace).desired_width(sw).hint_text("{ field: -1 }"));
                let enter = (fr.lost_focus() || sr.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                let find = egui::Button::new(RichText::new(format!("{I_SEARCH} Find")).color(Color32::WHITE)).fill(Color32::from_rgb(0x2C, 0x7B, 0xE5));
                if ui.add(find).on_hover_text("Run the filter (Enter)").clicked() || enter {
                    actions.push(Action::RunQuery);
                }
                if icon_button(ui, I_RESET, "Clear filter and sort") {
                    actions.push(Action::ResetQuery);
                }
            });
            if let Some(e) = &v.query_error {
                ui.colored_label(DANGER, e);
            }

            ui.horizontal(|ui| {
                if ui.button(format!("{I_PLUS}  Insert")).on_hover_text("Insert a new document").clicked() {
                    actions.push(Action::InsertDoc);
                }
                if icon_button(ui, I_REFRESH, "Reload this page") {
                    actions.push(Action::Reload);
                }
                if icon_button(ui, "⊞", "Expand all nested fields") {
                    v.expand_gen = 1;
                }
                if icon_button(ui, "⊟", "Collapse all nested fields") {
                    v.expand_gen = 2;
                }
                if v.loading {
                    ui.spinner();
                }
            });
            ui.separator();

            let expand = match v.expand_gen {
                1 => Some(true),
                2 => Some(false),
                _ => None,
            };
            v.expand_gen = 0;

            if let Some(e) = &v.error {
                egui::Frame::group(ui.style()).stroke(egui::Stroke::new(1.0, DANGER)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(egui::Label::new(RichText::new(e).color(DANGER)).wrap().selectable(true));
                });
                return;
            }
            if v.docs.is_empty() && !v.loading {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.weak(if v.filter.is_empty() && v.page == 0 { "This collection is empty." } else { "No documents match." });
                });
                return;
            }

            let first_row = v.page * v.page_size;
            egui::ScrollArea::vertical().id_salt((&v.db, &v.coll, v.page)).auto_shrink([false, false]).show(ui, |ui| {
                for (i, doc) in v.docs.iter().enumerate() {
                    egui::Frame::group(ui.style()).fill(ui.visuals().faint_bg_color).corner_radius(8.0).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.weak(format!("#{}", thousands(first_row + i as u64 + 1)));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                // Delete sits inside a menu so a stray click can never reach it.
                                ui.menu_button("⋯", |ui| {
                                    if ui.button(RichText::new(format!("{I_TRASH}  Delete document…")).color(DANGER)).clicked() {
                                        actions.push(Action::DeleteDoc(i));
                                        ui.close();
                                    }
                                })
                                .response
                                .on_hover_text("More");
                                if ui.add(egui::Button::new(I_CLONE).frame(false)).on_hover_text("Insert a copy").clicked() {
                                    actions.push(Action::CloneDoc(i));
                                }
                                if ui.add(egui::Button::new(I_COPY).frame(false)).on_hover_text("Copy document").clicked() {
                                    ui.ctx().copy_text(shell::doc_to_shell(doc));
                                }
                                if ui.add(egui::Button::new(I_EDIT).frame(false)).on_hover_text("Edit").clicked() {
                                    actions.push(Action::EditDoc(i));
                                }
                            });
                        });
                        ui.spacing_mut().item_spacing.y = 2.0;
                        render_fields(ui, doc, egui::Id::unique(("doc", first_row, i)), dark, expand);
                    });
                    ui.add_space(2.0);
                }
            });
        });

        for a in actions {
            self.apply(a);
        }
    }

    /// Rows-per-page, previous/next arrows and the page list from `page_items`.
    fn pager(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(v) = self.view.as_mut() else { return };
        egui::ComboBox::from_id_salt("page_size").width(64.0).selected_text(format!("{} / page", v.page_size)).show_ui(ui, |ui| {
            for n in PAGE_SIZES {
                if ui.selectable_label(v.page_size == n, n.to_string()).clicked() && v.page_size != n {
                    actions.push(Action::SetPageSize(n));
                }
            }
        });
        ui.separator();

        let total_pages = match v.count {
            CountState::Known { total, .. } => Some(total.div_ceil(v.page_size).max(1)),
            _ => None,
        };
        let cur = v.page;
        let has_prev = cur > 0;
        let has_next = match total_pages {
            Some(tp) => cur + 1 < tp,
            None => v.has_more,
        };
        if ui.add_enabled(has_prev, egui::Button::new(I_PREV)).on_hover_text("Previous page").clicked() {
            actions.push(Action::GoToPage(cur - 1));
        }
        let mut page_button = |ui: &mut egui::Ui, p: u64| {
            if ui.selectable_label(p == cur, thousands(p + 1)).clicked() && p != cur {
                actions.push(Action::GoToPage(p));
            }
        };
        let jump = |ui: &mut egui::Ui, v: &mut CollView, max: Option<u64>| {
            let hint = "go to";
            let r = ui
                .add(TextEdit::singleline(&mut v.jump_text).desired_width(64.0).hint_text(hint).align(egui::Align2::CENTER_CENTER))
                .on_hover_text("Type a page number and press Enter");
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let cleaned: String = v.jump_text.chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = cleaned.parse::<u64>() {
                    let mut target = n.max(1) - 1;
                    if let Some(m) = max {
                        target = target.min(m - 1);
                    }
                    if target != cur {
                        return Some(target);
                    }
                }
                v.jump_text.clear();
            }
            None
        };
        let mut jump_to = None;
        match total_pages {
            Some(tp) => {
                for item in page_items(cur, tp) {
                    match item {
                        PageItem::Page(p) => page_button(ui, p),
                        PageItem::Gap => {
                            ui.weak("…");
                        }
                        PageItem::Jump => jump_to = jump(ui, v, Some(tp)),
                    }
                }
            }
            None => {
                ui.label(format!("Page {}", thousands(cur + 1)));
                jump_to = jump(ui, v, None);
            }
        }
        if let Some(p) = jump_to {
            actions.push(Action::GoToPage(p));
        }
        if ui.add_enabled(has_next, egui::Button::new(I_NEXT)).on_hover_text("Next page").clicked() {
            actions.push(Action::GoToPage(cur + 1));
        }
        ui.separator();

        match v.count {
            CountState::Known { total, estimated } => {
                let r = ui.label(format!("total {}", thousands(total)));
                if estimated {
                    r.on_hover_text("Estimate from collection metadata");
                }
            }
            CountState::Loading => {
                ui.label("total");
                ui.spinner();
            }
            CountState::Unknown => {
                ui.weak("total not counted").on_hover_text("Counting every match took too long. Paging still works.");
            }
        }
        if let Some(e) = v.elapsed {
            ui.weak(format!("· {} ms", e.as_millis()));
        }
    }

    fn modals(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut modal = std::mem::replace(&mut self.modal, Modal::None);
        match &mut modal {
            Modal::None => {}
            Modal::Editor { title, text, original_id, error, focus } => {
                let mut save = false;
                egui::Modal::new(egui::Id::unique("editor")).show(ctx, |ui| {
                        let screen = ui.ctx().content_rect();
                        ui.set_width((screen.width() - 160.0).clamp(480.0, 900.0));
                        ui.heading(title.as_str());
                        ui.weak("Types are kept as written: 42 is Int32, NumberLong(42) is Int64, 42.0 is Double, ISODate(\"…\") is a date.");
                        egui::ScrollArea::vertical().max_height((screen.height() - 260.0).max(200.0)).auto_shrink([false, false]).show(ui, |ui| {
                            let r = ui.add(TextEdit::multiline(text).code_editor().desired_width(f32::INFINITY).desired_rows(30));
                            if *focus {
                                r.request_focus();
                                *focus = false;
                            }
                            if r.changed() {
                                *error = None;
                            }
                        });
                        if let Some(e) = error {
                            ui.colored_label(DANGER, e.as_str());
                        }
                        ui.horizontal(|ui| {
                            let label = if original_id.is_some() { "Update" } else { "Insert" };
                            let b = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE)).fill(Color32::from_rgb(0x2C, 0x7B, 0xE5));
                            let hotkey = ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
                            if ui.add(b).on_hover_text("Cmd+Enter").clicked() || hotkey {
                                save = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    });
                if save {
                    if let Some(v) = &self.view {
                        let (db, coll) = (v.db.clone(), v.coll.clone());
                        match original_id {
                            Some(id) => match shell::parse_document(text) {
                                Err(e) => *error = Some(e.to_string()),
                                Ok(doc) if doc.is_empty() => *error = Some("The document is empty".into()),
                                Ok(doc) if doc.get("_id").is_some_and(|new| new != &*id) => {
                                    *error = Some("The _id of a document cannot be changed. Use \"Insert a copy\" instead.".into())
                                }
                                Ok(mut doc) => {
                                    if !doc.contains_key("_id") {
                                        let mut with_id = Document::new();
                                        with_id.insert("_id", id.clone());
                                        with_id.extend(doc);
                                        doc = with_id;
                                    }
                                    self.worker.send(Cmd::Replace { db, coll, id: id.clone(), doc });
                                    close = true;
                                }
                            },
                            None => match shell::parse_value(text) {
                                Err(e) => *error = Some(e.to_string()),
                                Ok(Bson::Document(d)) => {
                                    self.worker.send(Cmd::Insert { db, coll, docs: vec![d] });
                                    close = true;
                                }
                                Ok(Bson::Array(items)) => {
                                    let docs: Vec<Document> = items.iter().filter_map(|b| b.as_document().cloned()).collect();
                                    if docs.is_empty() || docs.len() != items.len() {
                                        *error = Some("An array must contain only documents".into());
                                    } else {
                                        self.worker.send(Cmd::Insert { db, coll, docs });
                                        close = true;
                                    }
                                }
                                Ok(_) => *error = Some("Write one document { … } or an array of documents [ {…}, {…} ]".into()),
                            },
                        }
                    }
                }
            }
            Modal::ConfirmDelete { id, preview } => {
                let mut confirmed = false;
                egui::Modal::new(egui::Id::unique("modal")).show(ctx, |ui| {
                    ui.heading("Delete this document?");
                    ui.add_space(4.0);
                    ui.set_max_width(560.0);
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(preview.as_str()).monospace()).wrap());
                    });
                    ui.weak("This cannot be undone.");
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new(format!("{I_TRASH}  Delete")).color(Color32::WHITE)).fill(DANGER)).clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if confirmed {
                    if let Some(v) = &self.view {
                        self.worker.send(Cmd::Delete { db: v.db.clone(), coll: v.coll.clone(), id: id.clone() });
                    }
                    close = true;
                }
            }
            Modal::ConfirmDeleteProfile { index } => {
                let name = self.profiles.get(*index).map(|p| p.display_name()).unwrap_or_default();
                let mut confirmed = false;
                egui::Modal::new(egui::Id::unique("modal")).show(ctx, |ui| {
                    ui.heading("Remove connection?");
                    ui.add_space(4.0);
                    ui.label(format!("\"{name}\" will be removed from this app. Nothing changes on the server."));
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Remove").color(Color32::WHITE)).fill(DANGER)).clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if confirmed {
                    if *index < self.profiles.len() {
                        self.profiles.remove(*index);
                        self.persist();
                    }
                    let next = if self.profiles.is_empty() { None } else { Some(index.saturating_sub(1).min(self.profiles.len() - 1)) };
                    self.select_profile(next);
                    close = true;
                }
            }
            Modal::CompassImport { read_keychain } => {
                let mut go = false;
                egui::Modal::new(egui::Id::unique("modal")).show(ctx, |ui| {
                    ui.heading("Import from Compass");
                    ui.add_space(4.0);
                    ui.set_max_width(480.0);
                    ui.label("Copies every saved (favorite) connection from MongoDB Compass on this computer, including hosts, user names and certificate paths.");
                    ui.add_space(4.0);
                    ui.checkbox(read_keychain, "Also copy the saved passwords");
                    ui.weak("Compass keeps passwords in the macOS Keychain, so macOS will ask you to allow access once per connection. Leave this off to type passwords in yourself.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Import").strong().color(Color32::WHITE)).fill(Color32::from_rgb(0x2C, 0x7B, 0xE5))).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if go {
                    match conn::import_from_compass(*read_keychain) {
                        Ok(result) => {
                            let mut added = 0;
                            for p in result.profiles {
                                // Skip ones already imported earlier (same name and hosts).
                                if !self.profiles.iter().any(|e| e.name == p.name && e.hosts == p.hosts) {
                                    self.profiles.push(p);
                                    added += 1;
                                }
                            }
                            self.persist();
                            if self.selected.is_none() && !self.profiles.is_empty() {
                                self.select_profile(Some(0));
                            }
                            let pw = if *read_keychain { format!(", {} with passwords", result.passwords_found) } else { String::new() };
                            self.toast(format!("{added} connections imported from Compass{pw}"), false);
                        }
                        Err(e) => self.toast(format!("{e:#}"), true),
                    }
                    close = true;
                }
            }
            Modal::Export { include_shared } => {
                let mut go = false;
                let ldap = self.profiles.iter().filter(|p| p.auth == AuthKind::Ldap).count();
                egui::Modal::new(egui::Id::unique("modal")).show(ctx, |ui| {
                    ui.heading("Export connections");
                    ui.add_space(4.0);
                    ui.set_max_width(480.0);
                    ui.label(format!("Saves all {} connections to one file a teammate can import.", self.profiles.len()));
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("LDAP passwords are always left out ({ldap} LDAP connections).")).strong());
                    ui.checkbox(include_shared, "Include shared passwords (service accounts and keystores)");
                    ui.weak("Certificate files are not copied, only their paths. Teammates point those at their own copies.");
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Choose file and export").strong().color(Color32::WHITE)).fill(Color32::from_rgb(0x2C, 0x7B, 0xE5))).clicked() {
                            go = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
                if go {
                    if let Some(path) = rfd::FileDialog::new().set_file_name("mongui-connections.json").save_file() {
                        match conn::export_profiles(&self.profiles, &path, *include_shared) {
                            Ok(()) => self.toast(format!("Exported to {}", path.display()), false),
                            Err(e) => self.toast(format!("Export failed: {e:#}"), true),
                        }
                        close = true;
                    }
                }
            }
        }
        if !(close || escape) && matches!(self.modal, Modal::None) {
            self.modal = modal;
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        let Some((msg, at, is_error)) = self.toast.clone() else { return };
        let ttl = if is_error { Duration::from_secs(12) } else { Duration::from_secs(3) };
        if at.elapsed() > ttl {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(300));
        let mut dismiss = false;
        egui::Area::new(egui::Id::unique("toast")).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -52.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            let color = if is_error { DANGER } else { Color32::from_rgb(0x5C, 0xC9, 0x8B) };
            egui::Frame::popup(ui.style()).stroke(egui::Stroke::new(1.5, color)).show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(msg.as_str()).wrap().selectable(true));
                    if ui.add(egui::Button::new("✖").frame(false)).clicked() {
                        dismiss = true;
                    }
                });
            });
        });
        if dismiss {
            self.toast = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events();
        self.top_bar(ui);
        if self.active.is_some() {
            self.browser_screen(ui);
        } else {
            self.connections_screen(ui);
        }
        self.modals(&ctx);
        self.toasts(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::{page_items, PageItem::*};

    #[test]
    fn pager_layout() {
        assert_eq!(page_items(0, 1), vec![Page(0)]);
        assert_eq!(page_items(2, 4), vec![Page(0), Page(1), Page(2), Page(3)]);
        assert_eq!(page_items(1, 50), vec![Page(0), Page(1), Page(2), Jump, Page(49)]);
        // page 4 follows 3 directly: no gap
        assert_eq!(page_items(3, 50), vec![Page(0), Page(1), Page(2), Page(3), Jump, Page(49)]);
        assert_eq!(page_items(20, 50), vec![Page(0), Page(1), Page(2), Gap, Page(20), Jump, Page(49)]);
        // current is the last page: shown once, at the end
        assert_eq!(page_items(49, 50), vec![Page(0), Page(1), Page(2), Gap, Jump, Page(49)]);
    }
}
