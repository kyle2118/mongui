use crate::{
    conn::{self, AuthKind, CaSource, ClientCert, Profile},
    shell,
    theme::{self, font, icon, pal, radius, size, space},
    widgets::{self as w, Kind, Tone},
    worker::{self, Cmd, CollStats, DbInfo, DbStats, Event},
};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Rect, Response, RichText, Sense, Stroke, StrokeKind,
    TextEdit, TextStyle, Ui, vec2,
};
use mongodb::bson::{Bson, Document};
use std::{
    collections::HashSet,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

const PAGE_SIZES: [u64; 3] = [20, 50, 100];

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
    /// `Err` when the server refused collStats (e.g. the login lacks the privilege).
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
    /// Databases expanded in the sidebar tree.
    open_dbs: HashSet<String>,
    #[cfg(feature = "demo")]
    demo_stage: u8,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let dark = true;
        theme::install(&cc.egui_ctx, dark);
        let ctx = cc.egui_ctx.clone();
        #[cfg(feature = "demo")]
        let (worker, events) = if crate::demo::enabled() { crate::demo::spawn(move || ctx.request_repaint()) } else { worker::spawn(move || ctx.request_repaint()) };
        #[cfg(not(feature = "demo"))]
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
            open_dbs: HashSet::new(),
            #[cfg(feature = "demo")]
            demo_stage: 0,
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
        self.open_dbs.insert(db.clone());
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
                    self.open_dbs.clear();
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

fn value_color(v: &Bson, p: &theme::Palette) -> Color32 {
    match v {
        Bson::String(_) => p.syn_string,
        Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_) | Bson::Decimal128(_) => p.syn_number,
        Bson::Boolean(_) => p.syn_bool,
        Bson::Null | Bson::Undefined => p.syn_null,
        Bson::ObjectId(_) => p.syn_id,
        Bson::DateTime(_) | Bson::Timestamp(_) => p.syn_date,
        _ => p.syn_other,
    }
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

/// One line of text cut with "…" to fit `width`.
fn truncated(ui: &Ui, text: &str, font_id: egui::FontId, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font_id, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(10.0));
    ui.painter().layout_job(job)
}

/// Horizontal offset of a scalar field's key, matching the text of a collapsible field.
const DOC_KEY_INDENT: f32 = 16.0;

fn render_fields(ui: &mut Ui, doc: &Document, id: egui::Id, p: &'static theme::Palette, expand: Option<bool>) {
    for (k, v) in doc {
        render_value(ui, &shell::format_key(k), v, id.with(k), p, expand);
    }
}

fn nested_header(key: &str, kind: &str, n: usize, p: &theme::Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mono = font::mono(font::MONO);
    job.append(key, 0.0, egui::TextFormat::simple(mono.clone(), p.syn_key));
    job.append(":", 0.0, egui::TextFormat::simple(mono.clone(), p.text_faint));
    job.append(&format!("{kind} ({n})"), 6.0, egui::TextFormat::simple(mono, p.text_muted));
    job
}

fn render_value(ui: &mut Ui, key: &str, v: &Bson, id: egui::Id, p: &'static theme::Palette, expand: Option<bool>) {
    match v {
        Bson::Document(d) if !d.is_empty() => {
            egui::CollapsingHeader::new(nested_header(key, "Object", d.len(), p))
                .id_salt(id)
                .open(expand)
                .icon(w::chevron_icon)
                .show(ui, |ui| render_fields(ui, d, id, p, expand));
        }
        Bson::Array(a) if !a.is_empty() => {
            let n = a.len();
            egui::CollapsingHeader::new(nested_header(key, "Array", n, p))
                .id_salt(id)
                .open(expand)
                .icon(w::chevron_icon)
                .show(ui, |ui| {
                    // Huge arrays are capped so one document cannot stall the UI.
                    for (i, item) in a.iter().take(200).enumerate() {
                        render_value(ui, &i.to_string(), item, id.with(i), p, expand);
                    }
                    if n > 200 {
                        w::small_muted(ui, format!("… {} more items. Open the editor to see all.", n - 200));
                    }
                });
        }
        _ => {
            let text = match v {
                Bson::Document(_) => "{}".to_string(),
                Bson::Array(_) => "[]".to_string(),
                other => scalar_text(other),
            };
            let mono = font::mono(font::MONO);
            let mut job = egui::text::LayoutJob::default();
            job.append(key, 0.0, egui::TextFormat::simple(mono.clone(), p.syn_key));
            job.append(":", 0.0, egui::TextFormat::simple(mono.clone(), p.text_faint));
            job.append(&text, 8.0, egui::TextFormat::simple(mono, value_color(v, p)));
            ui.horizontal_wrapped(|ui| {
                // Line up with the keys of collapsible siblings, which sit after their chevron.
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.add_space(DOC_KEY_INDENT);
                ui.add(egui::Label::new(job).wrap().selectable(true));
            });
        }
    }
}

fn path_row(ui: &mut Ui, label: &str, path: &mut String, hint: &str, filter: (&str, &[&str])) -> bool {
    let mut changed = false;
    w::form_label(ui, label);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let wdt = (ui.available_width() - size::ICON_BUTTON - 6.0).max(120.0);
        changed |= ui.add(w::input(path).desired_width(wdt).hint_text(hint)).changed();
        if w::icon_button_kind(ui, icon::FOLDER, "Choose file", Kind::Secondary).clicked() {
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

fn password_row(ui: &mut Ui, label: &str, value: &mut String, show: bool, hint: &str) -> bool {
    w::form_label(ui, label);
    let r = ui.add(w::input(value).password(!show).desired_width(f32::INFINITY).hint_text(hint));
    ui.end_row();
    r.changed()
}

fn text_row(ui: &mut Ui, label: &str, value: &mut String, hint: &str) -> bool {
    w::form_label(ui, label);
    let r = ui.add(w::input(value).desired_width(f32::INFINITY).hint_text(hint));
    ui.end_row();
    r.changed()
}

/// Rounded tile in a connection's color, with a server glyph. Used wherever a connection is named.
fn conn_tile(ui: &mut Ui, color: usize, side: f32) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((side * 0.28) as u8), theme::conn_color(color));
    painter.text(rect.center(), Align2::CENTER_CENTER, icon::SERVER, font::icon(side * 0.52), Color32::from_black_alpha(170));
    r
}

/// Full-width sidebar row with an icon and a label (sidebar footer actions).
fn nav_item(ui: &mut Ui, ic: &str, label: &str, enabled: bool) -> Response {
    let p = pal(ui);
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), size::CONTROL), if enabled { Sense::click() } else { Sense::hover() });
    let lbl = label.to_string();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &lbl));
    let painter = ui.painter();
    if enabled && r.hovered() {
        painter.rect_filled(rect, CornerRadius::same(radius::MD), p.control);
    }
    if r.has_focus() {
        painter.rect_stroke(rect, CornerRadius::same(radius::MD), Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    let color = if !enabled { p.text_faint } else if r.hovered() { p.text_strong } else { p.text_muted };
    painter.text(egui::pos2(rect.left() + 18.0, rect.center().y), Align2::CENTER_CENTER, ic, font::icon(14.0), color);
    painter.text(egui::pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, label, font::regular(font::BODY), color);
    r
}

/// One row of the database tree.
struct TreeRow<'a> {
    depth: u8,
    open: Option<bool>,
    icon: &'a str,
    label: &'a str,
    trailing: Option<String>,
    selected: bool,
}

fn tree_row(ui: &mut Ui, row: TreeRow<'_>) -> Response {
    let p = pal(ui);
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), size::TREE_ROW), Sense::click());
    let lbl = row.label.to_string();
    let selected = row.selected;
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &lbl));
    let painter = ui.painter();
    let bg = if row.selected {
        p.accent_soft
    } else if r.hovered() {
        p.control
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(radius::MD), bg);
    if r.has_focus() {
        painter.rect_stroke(rect, CornerRadius::same(radius::MD), Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    let mut x = rect.left() + 6.0 + row.depth as f32 * 18.0;
    if let Some(open) = row.open {
        let c = egui::pos2(x + 6.0, rect.center().y);
        let rot = egui::emath::Rot2::from_angle(if open { std::f32::consts::FRAC_PI_2 } else { 0.0 });
        let s = 3.0;
        let pts = [vec2(-s * 0.5, -s), vec2(s * 0.5, 0.0), vec2(-s * 0.5, s)].map(|v| c + rot * v);
        painter.line(pts.to_vec(), Stroke::new(1.4, if r.hovered() { p.text } else { p.text_faint }));
    }
    x += 16.0;
    let (icon_color, text_color, f) = if row.selected {
        (p.accent_text, p.text_strong, font::medium(font::BODY))
    } else {
        (p.text_faint, if r.hovered() { p.text_strong } else { p.text }, font::regular(font::BODY))
    };
    painter.text(egui::pos2(x + 7.0, rect.center().y), Align2::CENTER_CENTER, row.icon, font::icon(13.5), icon_color);
    x += 20.0;
    let mut right = rect.right() - 8.0;
    if let Some(t) = &row.trailing {
        let g = ui.painter().layout_no_wrap(t.clone(), font::regular(font::TINY), p.text_faint);
        right -= g.size().x;
        ui.painter().galley(egui::pos2(right, rect.center().y - g.size().y / 2.0), g, p.text_faint);
        right -= 8.0;
    }
    let g = truncated(ui, row.label, f, text_color, right - x);
    ui.painter().galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, text_color);
    r
}

/// Thin colored line across the top of the window in the active connection's color,
/// so production and test connections are hard to confuse.
fn env_strip(ui: &Ui, color: Color32) {
    let r = ui.max_rect();
    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 3.0)), CornerRadius::ZERO, color);
}

fn sidebar_frame(p: &theme::Palette) -> Frame {
    Frame::new().fill(p.bg_sidebar).inner_margin(Margin::symmetric(10, 0))
}

// ---------------------------------------------------------------- screens

impl App {
    fn theme_toggle(&mut self, ui: &mut Ui) {
        let (ic, tip) = if self.dark { (icon::SUN, "Switch to light theme") } else { (icon::MOON, "Switch to dark theme") };
        if w::icon_button(ui, ic, tip).clicked() {
            self.dark = !self.dark;
            theme::apply(ui.ctx(), self.dark);
        }
    }

    fn connections_screen(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        egui::Panel::left("saved")
            .resizable(true)
            .default_size(size::SIDEBAR)
            .size_range(size::SIDEBAR_MIN..=size::SIDEBAR_MAX)
            .frame(sidebar_frame(p))
            .show(ui, |ui| {
                // Brand row, same height as the browser's top bar.
                ui.allocate_ui_with_layout(vec2(ui.available_width(), size::TOPBAR), Layout::left_to_right(Align::Center), |ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(6), p.accent);
                    ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon::DATABASE, font::icon(13.0), p.on_accent);
                    ui.label(RichText::new("mongui").font(font::semibold(font::TITLE)).color(p.text_strong));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.theme_toggle(ui));
                });
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    w::section_label(ui, &format!("Connections · {}", self.profiles.len()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::icon_button(ui, icon::PLUS, "New connection").clicked() {
                            self.select_profile(None);
                        }
                    });
                });
                ui.add_space(space::XXS);

                let mut clicked = None;
                let mut double = None;
                egui::Panel::bottom("conn_footer").frame(Frame::new().inner_margin(Margin { left: 0, right: 0, top: 0, bottom: 10 })).show_separator_line(false).show(ui, |ui| {
                    w::divider(ui);
                    ui.add_space(space::XS);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if nav_item(ui, icon::IMPORT, "Import from Compass", true).clicked() {
                        self.modal = Modal::CompassImport { read_keychain: false };
                    }
                    if nav_item(ui, icon::FOLDER, "Import file…", true).on_hover_text("Load connections a teammate exported").clicked() {
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
                    if nav_item(ui, icon::EXPORT, "Export…", !self.profiles.is_empty()).on_hover_text("Save all connections to a file you can share").clicked() {
                        self.modal = Modal::Export { include_shared: true };
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("conn_list")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if self.profiles.is_empty() {
                            ui.add_space(space::SM);
                            w::small_muted(ui, "No saved connections yet. Fill in the form, or import the ones you already have in Compass.");
                        }
                        let new_selected = self.selected.is_none();
                        for (i, prof) in self.profiles.iter().enumerate() {
                            let selected = self.selected == Some(i);
                            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                            let name = prof.display_name();
                            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, &name));
                            let bg = if selected {
                                p.accent_soft
                            } else if resp.hovered() {
                                p.control
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(radius::LG), bg);
                            if resp.has_focus() {
                                ui.painter().rect_stroke(rect, CornerRadius::same(radius::LG), Stroke::new(1.5, p.accent), StrokeKind::Inside);
                            }
                            let tile = Rect::from_center_size(egui::pos2(rect.left() + 22.0, rect.center().y), vec2(28.0, 28.0));
                            ui.painter().rect_filled(tile, CornerRadius::same(8), theme::conn_color(prof.color));
                            ui.painter().text(tile.center(), Align2::CENTER_CENTER, icon::SERVER, font::icon(14.0), Color32::from_black_alpha(170));
                            let tx = rect.left() + 44.0;
                            let tw = rect.right() - 8.0 - tx;
                            let g = truncated(ui, &prof.display_name(), font::medium(font::BODY), p.text_strong, tw);
                            ui.painter().galley(egui::pos2(tx, rect.top() + 7.0), g, p.text_strong);
                            let sub = if prof.hosts.trim().is_empty() { "No hosts" } else { prof.hosts.as_str() };
                            let g = truncated(ui, sub, font::regular(font::SMALL), p.text_muted, tw);
                            ui.painter().galley(egui::pos2(tx, rect.top() + 26.0), g, p.text_muted);
                            if resp.clicked() {
                                clicked = Some(i);
                            }
                            if resp.double_clicked() {
                                double = Some(i);
                            }
                            resp.on_hover_text("Double-click to connect");
                        }
                        if new_selected && !self.profiles.is_empty() {
                            ui.add_space(2.0);
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
                            ui.painter().rect_filled(rect, CornerRadius::same(radius::LG), p.accent_soft);
                            ui.painter().text(egui::pos2(rect.left() + 22.0, rect.center().y), Align2::CENTER_CENTER, icon::PLUS, font::icon(14.0), p.accent_text);
                            ui.painter().text(egui::pos2(rect.left() + 44.0, rect.center().y), Align2::LEFT_CENTER, "New connection", font::medium(font::BODY), p.text_strong);
                        }
                    });
                if let Some(i) = clicked {
                    self.select_profile(Some(i));
                }
                if let Some(i) = double {
                    self.select_profile(Some(i));
                    self.connect();
                }

            });

        // Actions stay pinned to the bottom so Connect is always in reach, whatever the form's length.
        egui::Panel::bottom("form_actions")
            .frame(Frame::new().fill(p.bg).inner_margin(Margin::symmetric(space::XL as i8, space::MD as i8)))
            .show(ui, |ui| {
                form_column(ui, |ui| self.form_actions(ui));
            });

        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("form").auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(space::XL + 4.0);
                form_column(ui, |ui| self.connection_form(ui));
                ui.add_space(space::XL);
            });
        });
    }

    fn connection_form(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let editing = self.selected.is_some();

        // Header
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::MD;
            conn_tile(ui, self.form.color, 40.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let name = if editing { self.form.display_name() } else { "New connection".to_string() };
                ui.label(RichText::new(name).font(font::semibold(font::HEADING)).color(p.text_strong));
                w::muted(ui, if editing { "Saved connection. Changes are kept when you connect or save." } else { "Fill in the details, or paste a connection string." });
            });
        });
        ui.add_space(space::XL);

        let mut dirty = false;
        let show_pw = &mut self.show_passwords;
        let f = &mut self.form;

        // Connection string
        w::card(ui, |ui| {
            w::card_header(ui, icon::PLUG, "Connection string", "Optional. Paste a mongodb:// URI to fill in the form below.");
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                let wdt = (ui.available_width() - 64.0).max(160.0);
                let r = ui.add(w::input(&mut self.uri_paste).font(TextStyle::Monospace).desired_width(wdt).hint_text("mongodb://user@host1:27017,host2:27017/?tls=true"));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let can_fill = !self.uri_paste.trim().is_empty();
                if (w::button_enabled(ui, can_fill, Kind::Secondary, None, "Fill").clicked() || (enter && can_fill)) && can_fill {
                    match conn::profile_from_uri(&self.uri_paste) {
                        Ok(mut np) => {
                            np.id = f.id.clone();
                            np.name = f.name.clone();
                            np.color = f.color;
                            *f = np;
                            dirty = true;
                            self.uri_paste.clear();
                            self.connect_error = None;
                        }
                        Err(e) => self.connect_error = Some(e.to_string()),
                    }
                }
            });
        });
        ui.add_space(space::MD);

        // General
        w::card(ui, |ui| {
            w::card_header(ui, icon::SERVER, "Server", "");
            w::form_grid(ui, "general", |ui| {
                dirty |= text_row(ui, "Name", &mut f.name, "e.g. rx-notifications QA");
                w::form_label(ui, "Color");
                dirty |= w::color_picker(ui, &mut f.color);
                ui.end_row();
                w::form_label(ui, "Hosts");
                dirty |= ui
                    .add(
                        TextEdit::multiline(&mut f.hosts)
                            .margin(Margin::symmetric(10, 6))
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("host1:27017,host2:27017"),
                    )
                    .changed();
                ui.end_row();
            });
        });
        ui.add_space(space::MD);

        // Authentication
        w::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| w::card_header(ui, icon::KEY, "Sign in", ""));
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let (ic, tip) = if *show_pw { (icon::EYE_OFF, "Hide passwords") } else { (icon::EYE, "Show passwords") };
                    if w::icon_button(ui, ic, tip).clicked() {
                        *show_pw = !*show_pw;
                    }
                });
            });
            dirty |= w::segmented(
                ui,
                &mut f.auth,
                &[
                    (AuthKind::Password, "Username / password", ""),
                    (AuthKind::Ldap, "LDAP (personal)", "Your own LDAP login. Its password is never included when you export connections."),
                    (AuthKind::None, "None", ""),
                ],
            );
            if f.auth != AuthKind::None {
                ui.add_space(space::MD);
                w::form_grid(ui, "auth", |ui| {
                    dirty |= text_row(ui, "Username", &mut f.username, "");
                    dirty |= password_row(ui, "Password", &mut f.password, *show_pw, "");
                    if f.auth == AuthKind::Password {
                        dirty |= text_row(ui, "Auth database", &mut f.auth_db, "admin");
                    }
                });
            }
        });
        ui.add_space(space::MD);

        // TLS
        w::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| w::card_header(ui, icon::SHIELD, "TLS / SSL", "Client certificate and the CA that signs the server's certificate."));
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    let label = if f.tls { "On" } else { "Off" };
                    dirty |= w::toggle(ui, &mut f.tls, label).changed();
                });
            });
            if f.tls {
                ui.add_space(space::XS);
                w::form_grid(ui, "tls", |ui| {
                    w::form_label(ui, "Client certificate");
                    dirty |= w::segmented(
                        ui,
                        &mut f.client_cert,
                        &[
                            (ClientCert::Jks, "JKS keystore", "The same .jks file your Spring Boot apps use"),
                            (ClientCert::Pem, "PEM file", "Certificate and key in one .pem, as in Compass"),
                            (ClientCert::None, "None", ""),
                        ],
                    );
                    ui.end_row();
                    if f.client_cert != ClientCert::None {
                        let (label, hint, filter): (&str, &str, (&str, &[&str])) = if f.client_cert == ClientCert::Jks {
                            ("Keystore file", "/path/to/app.jks", ("Keystores", &["jks", "p12", "pfx", "keystore"]))
                        } else {
                            ("Certificate + key", "/path/to/client.pem", ("PEM", &["pem", "crt", "key"]))
                        };
                        dirty |= path_row(ui, label, &mut f.client_cert_path, hint, filter);
                        let (plabel, phint) = if f.client_cert == ClientCert::Jks { ("Keystore password", "") } else { ("Key password", "only if the key is encrypted") };
                        dirty |= password_row(ui, plabel, &mut f.client_cert_password, *show_pw, phint);
                    }
                    if f.client_cert != ClientCert::Jks && f.ca == CaSource::Keystore {
                        f.ca = CaSource::System;
                    }
                    w::form_label(ui, "Server CA");
                    let mut opts: Vec<(CaSource, &str, &str)> = vec![];
                    if f.client_cert == ClientCert::Jks {
                        opts.push((CaSource::Keystore, "From keystore", "Trust the CA certificates stored inside the JKS above"));
                    }
                    opts.push((CaSource::TrustStore, "cacerts", "A Java trust store: the cacerts file you copy into each new JDK"));
                    opts.push((CaSource::Pem, "CA .pem", "The Certificate Authority file used in Compass"));
                    opts.push((CaSource::System, "System", "Certificates trusted by this computer"));
                    dirty |= w::segmented(ui, &mut f.ca, &opts);
                    ui.end_row();
                    if f.ca == CaSource::TrustStore {
                        dirty |= path_row(ui, "Trust store file", &mut f.ca_path, "/path/to/cacerts", ("", &[]));
                        dirty |= password_row(ui, "Trust store password", &mut f.ca_password, *show_pw, "optional");
                    } else if f.ca == CaSource::Pem {
                        dirty |= path_row(ui, "CA file", &mut f.ca_path, "/path/to/ca.pem", ("PEM", &["pem", "crt", "cer"]));
                    }
                });
            }
        });
        ui.add_space(space::MD);

        // Advanced
        w::card(ui, |ui| {
            let id = ui.make_persistent_id("more");
            let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            let open = state.is_open();
            let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::DisclosureTriangle, true, "More options"));
            if r.clicked() {
                state.toggle(ui);
            }
            let color = if r.hovered() { p.text_strong } else { p.text };
            ui.painter().text(egui::pos2(rect.left() + 7.0, rect.center().y), Align2::CENTER_CENTER, icon::SETTINGS, font::icon(14.0), p.text_muted);
            ui.painter().text(egui::pos2(rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER, "More options", font::semibold(font::BODY), color);
            ui.painter().text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                Align2::CENTER_CENTER,
                if open { icon::CHEVRON_DOWN } else { icon::NEXT },
                font::icon(14.0),
                p.text_muted,
            );
            if r.has_focus() {
                ui.painter().rect_stroke(rect.expand(3.0), CornerRadius::same(radius::MD), Stroke::new(1.5, p.accent), StrokeKind::Inside);
            }
            state.show_body_unindented(ui, |ui| {
                ui.add_space(space::MD);
                w::form_grid(ui, "adv", |ui| {
                    dirty |= text_row(ui, "Replica set name", &mut f.replica_set, "optional");
                    w::form_label(ui, "Read preference");
                    egui::ComboBox::from_id_salt("rp")
                        .width(200.0)
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
                    w::form_label(ui, "Default database");
                    dirty |= ui
                        .add(w::input(&mut f.default_db).desired_width(f32::INFINITY).hint_text("optional"))
                        .on_hover_text("Shown when your login may not list all databases")
                        .changed();
                    ui.end_row();
                    ui.label("");
                    ui.vertical(|ui| {
                        dirty |= w::checkbox(ui, &mut f.srv, "Use mongodb+srv (DNS seed list)").changed();
                        if f.tls {
                            dirty |= w::checkbox(ui, &mut f.insecure, "Skip server certificate checks (insecure)")
                                .on_hover_text("Only for troubleshooting. The server's identity is not verified.")
                                .changed();
                        }
                    });
                    ui.end_row();
                });
            });
        });
        self.form_dirty |= dirty;
    }

    fn form_actions(&mut self, ui: &mut Ui) {
        if let Some(err) = &self.connect_error {
            w::callout(ui, Tone::Error, Some("Could not connect"), err);
            ui.add_space(space::MD);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = space::SM;
            if w::button_enabled(ui, !self.connecting, Kind::Primary, Some(icon::PLUG), "Connect").clicked() {
                self.connect();
            }
            if w::button_enabled(ui, self.form_dirty || self.selected.is_none(), Kind::Secondary, None, "Save").clicked() && self.save_form() {
                self.toast("Connection saved", false);
            }
            if self.connecting {
                ui.add_space(space::XS);
                ui.spinner();
                w::muted(ui, "Connecting…");
                if w::button(ui, Kind::Ghost, Some(icon::CLOSE), "Cancel").on_hover_text("Stop trying to connect").clicked() {
                    self.worker.send(Cmd::CancelConnect);
                    self.connecting = false;
                }
            }
            if let Some(i) = self.selected {
                // Kept at the far edge, away from Connect and Save.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    w::more_menu(ui, "More", |ui| {
                        if w::menu_item(ui, Kind::Ghost, icon::CLONE, "Duplicate").clicked() {
                            let mut copy = self.form.clone();
                            copy.id = conn::new_id();
                            copy.name = format!("{} copy", copy.display_name());
                            self.profiles.push(copy);
                            self.persist();
                            self.select_profile(Some(self.profiles.len() - 1));
                            ui.close();
                        }
                        if w::menu_item(ui, Kind::GhostDanger, icon::TRASH, "Remove this connection…").clicked() {
                            self.modal = Modal::ConfirmDeleteProfile { index: i };
                            ui.close();
                        }
                    });
                });
            }
        });
    }

    fn browser_screen(&mut self, ui: &mut Ui) {
        let mut actions: Vec<Action> = vec![];
        let active = self.active.clone().unwrap_or_default();
        let p = pal(ui);

        egui::Panel::top("top")
            .exact_size(size::TOPBAR)
            .frame(Frame::new().fill(p.bg_sidebar).inner_margin(Margin::symmetric(14, 0)))
            .show(ui, |ui| {
                env_strip(ui, theme::conn_color(active.color));
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = space::SM;
                    conn_tile(ui, active.color, 22.0).on_hover_text("Connection color");
                    ui.label(RichText::new(active.display_name()).font(font::semibold(font::BODY)).color(p.text_strong));
                    if active.tls {
                        ui.label(RichText::new(icon::LOCK).font(font::icon(12.5)).color(p.text_faint)).on_hover_text("TLS");
                    }
                    // Leave room for the buttons on the right.
                    let room = (ui.available_width() - 190.0).max(0.0);
                    if room > 60.0 {
                        let g = truncated(ui, &active.hosts, font::regular(font::SMALL), p.text_muted, room);
                        let (rect, r) = ui.allocate_exact_size(g.size(), Sense::hover());
                        ui.painter().galley(rect.min, g, p.text_muted);
                        r.on_hover_text(active.hosts.replace(',', "\n"));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if w::button(ui, Kind::Secondary, Some(icon::DISCONNECT), "Disconnect").clicked() {
                            actions.push(Action::Disconnect);
                        }
                        self.theme_toggle(ui);
                    });
                });
            });

        egui::Panel::left("tree")
            .resizable(true)
            .default_size(size::SIDEBAR)
            .size_range(size::SIDEBAR_MIN..=size::SIDEBAR_MAX)
            .frame(sidebar_frame(p))
            .show(ui, |ui| {
                ui.add_space(space::MD);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let wdt = ui.available_width() - size::ICON_BUTTON - 6.0;
                    w::icon_input(ui, icon::SEARCH, TextEdit::singleline(&mut self.tree_filter).desired_width(wdt).hint_text("Find database or collection"));
                    if w::icon_button(ui, icon::REFRESH, "Reload databases").clicked() {
                        self.worker.send(Cmd::ListDatabases);
                    }
                });
                ui.add_space(space::MD);
                w::section_label(ui, &format!("Databases · {}", self.dbs.len()));
                ui.add_space(space::XS);
                self.tree(ui, &mut actions);
            });

        if self.view.is_some() {
            egui::Panel::bottom("pager")
                .exact_size(size::PAGER)
                .frame(Frame::new().fill(p.bg_sidebar).inner_margin(Margin::symmetric(space::LG as i8, 0)))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| self.pager(ui, &mut actions));
                });
        }

        egui::CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| {
            if self.view.is_none() {
                ui.add_space((ui.available_height() * 0.32).max(space::XL));
                w::empty_state(ui, icon::COLLECTION, "Pick a collection", "Choose a collection on the left to browse, filter and edit its documents.");
                return;
            }
            self.collection_view(ui, &mut actions);
        });

        for a in actions {
            self.apply(a);
        }
    }

    fn tree(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let p = pal(ui);
        let needle = self.tree_filter.trim().to_lowercase();
        let current = self.view.as_ref().map(|v| (v.db.clone(), v.coll.clone()));
        egui::ScrollArea::vertical().id_salt("tree").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if self.dbs.is_empty() {
                ui.add_space(space::SM);
                w::small_muted(ui, "No databases to show.");
            }
            for node in self.dbs.iter_mut() {
                let name = node.info.name.clone();
                let db_matches = needle.is_empty() || name.to_lowercase().contains(&needle);
                let any_coll = !needle.is_empty() && node.collections.as_ref().is_some_and(|cs| cs.iter().any(|c| c.to_lowercase().contains(&needle)));
                if !db_matches && !any_coll {
                    continue;
                }
                let open = self.open_dbs.contains(&name) || any_coll;
                let trailing = node.collections.as_ref().map(|c| c.len().to_string());
                let r = tree_row(ui, TreeRow { depth: 0, open: Some(open), icon: icon::DATABASE, label: &name, trailing, selected: false });
                let r = if node.info.size_on_disk > 0 { r.on_hover_text(format!("{} on disk", bytes(node.info.size_on_disk as i64))) } else { r };
                let toggled = r.clicked() || (r.has_focus() && ui.input(|i| i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::ArrowLeft)));
                if toggled {
                    if self.open_dbs.contains(&name) {
                        self.open_dbs.remove(&name);
                    } else {
                        self.open_dbs.insert(name.clone());
                    }
                }
                if !open {
                    continue;
                }
                if !node.requested {
                    node.requested = true;
                    self.worker.send(Cmd::ListCollections { db: name.clone() });
                    self.worker.send(Cmd::DbStats { db: name.clone() });
                }
                if let Some(s) = &node.stats {
                    ui.horizontal(|ui| {
                        ui.add_space(42.0);
                        ui.label(
                            RichText::new(format!("{} docs · {}", thousands(s.objects.max(0) as u64), bytes(s.data_size)))
                                .font(font::regular(font::TINY))
                                .color(p.text_faint),
                        )
                        .on_hover_text(format!(
                            "{} collections\nData size: {}\nStorage size: {}\nIndexes: {} ({})",
                            thousands(s.collections.max(0) as u64),
                            bytes(s.data_size),
                            bytes(s.storage_size),
                            s.indexes,
                            bytes(s.index_size)
                        ));
                    });
                }
                match &node.collections {
                    None => {
                        ui.horizontal(|ui| {
                            ui.add_space(42.0);
                            ui.spinner();
                        });
                    }
                    Some(cs) if cs.is_empty() => {
                        ui.horizontal(|ui| {
                            ui.add_space(42.0);
                            ui.label(RichText::new("No collections").font(font::regular(font::SMALL)).color(p.text_faint));
                        });
                    }
                    Some(cs) => {
                        for c in cs.iter().filter(|c| needle.is_empty() || db_matches || c.to_lowercase().contains(&needle)) {
                            let selected = current.as_ref().is_some_and(|(d, cc)| d == &name && cc == c);
                            let r = tree_row(ui, TreeRow { depth: 1, open: None, icon: icon::COLLECTION, label: c, trailing: None, selected });
                            if r.clicked() && !selected {
                                actions.push(Action::OpenCollection(name.clone(), c.clone()));
                            }
                        }
                    }
                }
                ui.add_space(space::XS);
            }
        });
    }

    fn collection_view(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let p = pal(ui);
        let Some(v) = self.view.as_mut() else { return };
        let pad = space::XL as i8 - 4;

        // Header: title, stats, query bar and toolbar.
        Frame::new().inner_margin(Margin { left: pad, right: pad, top: 16, bottom: 12 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(&v.db).font(font::regular(font::HEADING)).color(p.text_muted));
                ui.label(RichText::new("/").font(font::regular(font::HEADING)).color(p.text_faint));
                ui.label(RichText::new(&v.coll).font(font::semibold(font::HEADING)).color(p.text_strong));
            });
            ui.add_space(space::SM);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                match &v.stats {
                    Some(Ok(s)) => {
                        w::stat(ui, "Documents", &thousands(s.count.max(0) as u64), None);
                        w::stat(ui, "Data", &bytes(s.size), Some("Uncompressed size of all documents"));
                        w::stat(ui, "Avg doc", &bytes(s.avg_obj_size), None);
                        w::stat(ui, "On disk", &bytes(s.storage_size), Some("Storage used, after compression"));
                        w::stat(ui, "Indexes", &format!("{} · {}", s.index_count, bytes(s.total_index_size)), None);
                    }
                    Some(Err(e)) => {
                        w::stat(ui, "Stats", "unavailable", Some(e.as_str()));
                    }
                    None => {
                        w::stat(ui, "Documents", "…", None);
                    }
                }
            });
            ui.add_space(space::MD);

            // Query bar. Stacks on narrow windows.
            let narrow = ui.available_width() < 620.0;
            let buttons_w = 84.0 + size::ICON_BUTTON + 2.0 * space::SM;
            let mut enter = false;
            let query_inputs = |ui: &mut Ui, fw: f32, sw: f32, v: &mut CollView| {
                let fr = w::icon_input(ui, icon::FILTER, TextEdit::singleline(&mut v.filter_text).font(TextStyle::Monospace).desired_width(fw).hint_text("Filter  { field: 'value' }"));
                let fr = fr.on_hover_text("Filter in shell syntax. Press Enter to run.");
                let sr = w::icon_input(ui, icon::SORT, TextEdit::singleline(&mut v.sort_text).font(TextStyle::Monospace).desired_width(sw).hint_text("Sort  { field: -1 }"));
                let sr = sr.on_hover_text("Sort in shell syntax, e.g. { createdAt: -1 }");
                (fr.lost_focus() || sr.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter))
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = space::SM;
                if narrow {
                    let wdt = ui.available_width();
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = space::SM;
                        let fr = w::icon_input(ui, icon::FILTER, TextEdit::singleline(&mut v.filter_text).font(TextStyle::Monospace).desired_width(wdt).hint_text("Filter  { field: 'value' }"));
                        enter |= fr.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = space::SM;
                            let sw = (wdt - buttons_w - 30.0).max(120.0);
                            let sr = w::icon_input(ui, icon::SORT, TextEdit::singleline(&mut v.sort_text).font(TextStyle::Monospace).desired_width(sw).hint_text("Sort  { field: -1 }"));
                            enter |= sr.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            if w::button(ui, Kind::Primary, Some(icon::SEARCH), "Find").on_hover_text("Run the filter (Enter)").clicked() {
                                actions.push(Action::RunQuery);
                            }
                            if w::icon_button_kind(ui, icon::CLOSE, "Clear filter and sort", Kind::Secondary).clicked() {
                                actions.push(Action::ResetQuery);
                            }
                        });
                    });
                } else {
                    let total = ui.available_width() - buttons_w - 30.0;
                    enter |= query_inputs(ui, (total * 0.64).max(160.0), (total * 0.36).max(110.0), v);
                    if w::button(ui, Kind::Primary, Some(icon::SEARCH), "Find").on_hover_text("Run the filter (Enter)").clicked() {
                        actions.push(Action::RunQuery);
                    }
                    if w::icon_button_kind(ui, icon::CLOSE, "Clear filter and sort", Kind::Secondary).clicked() {
                        actions.push(Action::ResetQuery);
                    }
                }
            });
            if enter {
                actions.push(Action::RunQuery);
            }
            if let Some(e) = &v.query_error {
                ui.add_space(space::XS);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(icon::ALERT).font(font::icon(13.0)).color(p.danger));
                    ui.label(RichText::new(e).font(font::regular(font::SMALL)).color(p.danger));
                });
            }
            ui.add_space(space::SM);

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if w::button(ui, Kind::Secondary, Some(icon::PLUS), "Insert").on_hover_text("Insert a new document").clicked() {
                    actions.push(Action::InsertDoc);
                }
                ui.add_space(space::XS);
                if w::icon_button(ui, icon::REFRESH, "Reload this page").clicked() {
                    actions.push(Action::Reload);
                }
                if w::icon_button(ui, icon::EXPAND, "Expand all nested fields").clicked() {
                    v.expand_gen = 1;
                }
                if w::icon_button(ui, icon::COLLAPSE, "Collapse all nested fields").clicked() {
                    v.expand_gen = 2;
                }
                if v.loading {
                    ui.add_space(space::SM);
                    ui.spinner();
                    w::small_muted(ui, "Loading…");
                }
            });
        });
        w::divider(ui);

        let expand = match v.expand_gen {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
        v.expand_gen = 0;

        let body = Margin { left: pad, right: pad, top: 14, bottom: 14 };
        if let Some(e) = &v.error {
            Frame::new().inner_margin(body).show(ui, |ui| w::callout(ui, Tone::Error, Some("The query failed"), e));
            return;
        }
        if v.docs.is_empty() {
            ui.add_space((ui.available_height() * 0.25).max(space::XL));
            if v.loading {
                ui.vertical_centered(|ui| {
                    ui.spinner();
                    w::muted(ui, "Loading documents…");
                });
            } else if v.filter.is_empty() && v.page == 0 {
                w::empty_state(ui, icon::INBOX, "This collection is empty", "Use Insert to add the first document.");
            } else {
                w::empty_state(ui, icon::FILTER, "No documents match", "Try a broader filter, or clear it to see every document.");
            }
            return;
        }

        let first_row = v.page * v.page_size;
        egui::ScrollArea::vertical().id_salt((&v.db, &v.coll, v.page)).auto_shrink([false, false]).show(ui, |ui| {
            Frame::new().inner_margin(body).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = space::SM;
                for (i, doc) in v.docs.iter().enumerate() {
                    Frame::new()
                        .fill(p.surface)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(radius::XL))
                        .inner_margin(Margin { left: 10, right: 8, top: 8, bottom: 8 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // Dense rows: documents are read line by line, like code.
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.spacing_mut().interact_size.y = 20.0;
                            ui.spacing_mut().button_padding.y = 1.0;
                            render_fields(ui, doc, egui::Id::unique(("doc", &v.db, &v.coll, first_row + i as u64)), p, expand);
                            // Actions and the row number float over the card's corners, so they take no line of their own.
                            let content = ui.min_rect();
                            let overlay = |ui: &mut Ui, layout: Layout, add: &mut dyn FnMut(&mut Ui)| {
                                let bg = ui.painter().add(egui::Shape::Noop);
                                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(layout));
                                child.spacing_mut().item_spacing.x = 2.0;
                                add(&mut child);
                                ui.painter().set(bg, egui::Shape::rect_filled(child.min_rect().expand(2.0), CornerRadius::same(radius::MD), p.surface));
                            };
                            let number = format!("#{}", thousands(first_row + i as u64 + 1));
                            overlay(ui, Layout::right_to_left(Align::Max), &mut |ui| {
                                ui.label(RichText::new(&number).font(font::medium(font::TINY)).color(p.text_faint));
                            });
                            overlay(ui, Layout::right_to_left(Align::Min), &mut |ui| {
                                // Delete sits inside a menu so a stray click can never reach it.
                                w::more_menu(ui, "More", |ui| {
                                    if w::menu_item(ui, Kind::GhostDanger, icon::TRASH, "Delete document…").clicked() {
                                        actions.push(Action::DeleteDoc(i));
                                        ui.close();
                                    }
                                });
                                if w::row_icon_button(ui, icon::CLONE, "Insert a copy").clicked() {
                                    actions.push(Action::CloneDoc(i));
                                }
                                if w::row_icon_button(ui, icon::COPY, "Copy document").clicked() {
                                    ui.ctx().copy_text(shell::doc_to_shell(doc));
                                }
                                if w::row_icon_button(ui, icon::EDIT, "Edit").clicked() {
                                    actions.push(Action::EditDoc(i));
                                }
                            });
                        });
                }
            });
        });
    }

    /// Rows-per-page, previous/next arrows and the page list from `page_items`.
    fn pager(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let p = pal(ui);
        let Some(v) = self.view.as_mut() else { return };
        let wide = ui.available_width() > 760.0;
        ui.spacing_mut().item_spacing.x = 4.0;
        if wide {
            w::small_muted(ui, "Rows");
        }
        egui::ComboBox::from_id_salt("page_size").width(64.0).selected_text(v.page_size.to_string()).show_ui(ui, |ui| {
            for n in PAGE_SIZES {
                if ui.selectable_label(v.page_size == n, format!("{n} per page")).clicked() && v.page_size != n {
                    actions.push(Action::SetPageSize(n));
                }
            }
        });
        ui.add_space(space::MD);

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
        if ui.add_enabled_ui(has_prev, |ui| w::icon_button(ui, icon::PREV, "Previous page")).inner.clicked() {
            actions.push(Action::GoToPage(cur - 1));
        }
        let mut page_button = |ui: &mut Ui, pg: u64| {
            if w::page_button(ui, &thousands(pg + 1), pg == cur).clicked() && pg != cur {
                actions.push(Action::GoToPage(pg));
            }
        };
        let jump = |ui: &mut Ui, v: &mut CollView, max: Option<u64>| {
            let r = ui
                .add(w::input(&mut v.jump_text).desired_width(56.0).hint_text("Go to").align(Align2::CENTER_CENTER))
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
                        PageItem::Page(pg) => page_button(ui, pg),
                        PageItem::Gap => {
                            ui.label(RichText::new("…").color(p.text_faint));
                        }
                        PageItem::Jump => jump_to = jump(ui, v, Some(tp)),
                    }
                }
            }
            None => {
                w::small_muted(ui, format!("Page {}", thousands(cur + 1)));
                jump_to = jump(ui, v, None);
            }
        }
        if let Some(pg) = jump_to {
            actions.push(Action::GoToPage(pg));
        }
        if ui.add_enabled_ui(has_next, |ui| w::icon_button(ui, icon::NEXT, "Next page")).inner.clicked() {
            actions.push(Action::GoToPage(cur + 1));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if wide {
                if let Some(e) = v.elapsed {
                    ui.label(RichText::new(format!("{} ms", e.as_millis())).font(font::regular(font::SMALL)).color(p.text_faint))
                        .on_hover_text("Time the last page took");
                    ui.label(RichText::new("·").color(p.text_faint));
                }
            }
            let strong = |t: String| RichText::new(t).font(font::medium(font::SMALL)).color(p.text);
            let weak = |t: &str| RichText::new(t).font(font::regular(font::SMALL)).color(p.text_muted);
            // Right-to-left: add the pieces in reverse.
            match v.count {
                CountState::Known { total, estimated } => {
                    let r = ui.label(strong(thousands(total)));
                    if estimated {
                        r.on_hover_text("Estimate from collection metadata");
                    }
                    ui.label(weak("total"));
                }
                CountState::Loading => {
                    ui.spinner();
                    ui.label(weak("total"));
                }
                CountState::Unknown => {
                    ui.label(weak("total not counted")).on_hover_text("Counting every match took too long. Paging still works.");
                }
            }
        });
    }

    fn modals(&mut self, ctx: &egui::Context) {
        let mut close = false;
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut modal = std::mem::replace(&mut self.modal, Modal::None);
        let p = theme::palette(self.dark);
        match &mut modal {
            Modal::None => {}
            Modal::Editor { title, text, original_id, error, focus } => {
                let mut save = false;
                let screen = ctx.content_rect();
                let width = (screen.width() - 160.0).clamp(440.0, 900.0);
                let where_ = self.view.as_ref().map(|v| format!("{}.{}", v.db, v.coll)).unwrap_or_default();
                w::dialog(ctx, "editor", width, title.as_str(), Some(&where_), |ui| {
                    Frame::new()
                        .fill(p.input)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(radius::LG))
                        .inner_margin(Margin::same(2))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height((screen.height() - 300.0).max(180.0)).auto_shrink([false, false]).show(ui, |ui| {
                                let r = ui.add(
                                    TextEdit::multiline(text)
                                        .code_editor()
                                        .frame(Frame::new().inner_margin(Margin::symmetric(12, 10)))
                                        .desired_width(f32::INFINITY)
                                        .desired_rows(24),
                                );
                                if *focus {
                                    r.request_focus();
                                    *focus = false;
                                }
                                if r.changed() {
                                    *error = None;
                                }
                            });
                        });
                    ui.add_space(space::SM);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        ui.label(RichText::new(icon::INFO).font(font::icon(12.5)).color(p.text_faint));
                        ui.label(
                            RichText::new("Types are kept as written: 42 is Int32, NumberLong(42) is Int64, 42.0 is Double, ISODate(\"…\") is a date.")
                                .font(font::regular(font::SMALL))
                                .color(p.text_muted),
                        );
                    });
                    if let Some(e) = error {
                        ui.add_space(space::SM);
                        w::callout(ui, Tone::Error, None, e.as_str());
                    }
                    w::dialog_footer(ui, |ui| {
                        let label = if original_id.is_some() { "Update" } else { "Insert" };
                        let hotkey = ui.input(|i| i.modifiers.command && i.key_pressed(Key::Enter));
                        if w::button(ui, Kind::Primary, None, label).on_hover_text("Cmd+Enter").clicked() || hotkey {
                            save = true;
                        }
                        if w::button(ui, Kind::Secondary, None, "Cancel").clicked() {
                            close = true;
                        }
                        ui.add_space(space::XS);
                        w::kbd(ui, "⌘ ⏎");
                        ui.label(RichText::new("to save").font(font::regular(font::SMALL)).color(p.text_faint));
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
                w::dialog(ctx, "confirm_delete", 520.0, "Delete this document?", Some("This cannot be undone."), |ui| {
                    Frame::new()
                        .fill(p.input)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(radius::LG))
                        .inner_margin(Margin::symmetric(12, 10))
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(egui::Label::new(RichText::new(preview.as_str()).monospace().color(p.text)).wrap());
                            });
                        });
                    w::dialog_footer(ui, |ui| {
                        if w::button(ui, Kind::Danger, Some(icon::TRASH), "Delete").clicked() {
                            confirmed = true;
                        }
                        if w::button(ui, Kind::Secondary, None, "Cancel").clicked() {
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
                w::dialog(ctx, "confirm_remove", 440.0, "Remove connection?", None, |ui| {
                    ui.label(RichText::new(format!("\"{name}\" will be removed from this app. Nothing changes on the server.")).color(p.text));
                    w::dialog_footer(ui, |ui| {
                        if w::button(ui, Kind::Danger, None, "Remove").clicked() {
                            confirmed = true;
                        }
                        if w::button(ui, Kind::Secondary, None, "Cancel").clicked() {
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
                w::dialog(
                    ctx,
                    "compass_import",
                    460.0,
                    "Import from Compass",
                    Some("Copies every saved (favorite) connection from MongoDB Compass on this computer, including hosts, user names and certificate paths."),
                    |ui| {
                        w::checkbox(ui, read_keychain, "Also copy the saved passwords");
                        ui.horizontal(|ui| {
                            ui.add_space(24.0 - ui.spacing().item_spacing.x);
                            ui.add(
                                egui::Label::new(
                                    RichText::new("Compass keeps passwords in the macOS Keychain, so macOS will ask you to allow access once per connection. Leave this off to type passwords in yourself.")
                                        .font(font::regular(font::SMALL))
                                        .color(p.text_muted),
                                )
                                .wrap(),
                            );
                        });
                        w::dialog_footer(ui, |ui| {
                            if w::button(ui, Kind::Primary, Some(icon::IMPORT), "Import").clicked() {
                                go = true;
                            }
                            if w::button(ui, Kind::Secondary, None, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    },
                );
                if go {
                    match conn::import_from_compass(*read_keychain) {
                        Ok(result) => {
                            let mut added = 0;
                            for prof in result.profiles {
                                // Skip ones already imported earlier (same name and hosts).
                                if !self.profiles.iter().any(|e| e.name == prof.name && e.hosts == prof.hosts) {
                                    self.profiles.push(prof);
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
                let n = self.profiles.len();
                w::dialog(ctx, "export", 460.0, "Export connections", Some(&format!("Saves all {n} connections to one file a teammate can import.")), |ui| {
                    w::callout(ui, Tone::Info, None, &format!("LDAP passwords are always left out ({ldap} LDAP connections)."));
                    ui.add_space(space::MD);
                    w::checkbox(ui, include_shared, "Include shared passwords (service accounts and keystores)");
                    ui.add_space(space::XS);
                    ui.add(
                        egui::Label::new(
                            RichText::new("Certificate files are not copied, only their paths. Teammates point those at their own copies.")
                                .font(font::regular(font::SMALL))
                                .color(p.text_muted),
                        )
                        .wrap(),
                    );
                    w::dialog_footer(ui, |ui| {
                        if w::button(ui, Kind::Primary, Some(icon::EXPORT), "Choose file and export").clicked() {
                            go = true;
                        }
                        if w::button(ui, Kind::Secondary, None, "Cancel").clicked() {
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
        let left = ttl.saturating_sub(at.elapsed());
        if left.is_zero() {
            self.toast = None;
            return;
        }
        // Wake up once when the toast should disappear, instead of polling.
        ctx.request_repaint_after(left);
        let p = theme::palette(self.dark);
        let mut dismiss = false;
        let bottom = if self.view.is_some() { size::PAGER + 12.0 } else { 72.0 };
        egui::Area::new(egui::Id::unique("toast")).anchor(Align2::RIGHT_BOTTOM, [-16.0, -bottom]).order(egui::Order::Foreground).show(ctx, |ui| {
            let (ic, color) = if is_error { (icon::ALERT, p.danger) } else { (icon::CHECK_CIRCLE, p.success) };
            Frame::new()
                .fill(p.elevated)
                .stroke(Stroke::new(1.0, p.border_strong))
                .corner_radius(CornerRadius::same(radius::LG))
                .inner_margin(Margin { left: 12, right: 6, top: 6, bottom: 6 })
                .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: p.shadow })
                .show(ui, |ui| {
                    ui.set_max_width(440.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ic).font(font::icon(15.0)).color(color));
                        ui.add(egui::Label::new(RichText::new(msg.as_str()).color(p.text_strong)).wrap().selectable(true));
                        if w::row_icon_button(ui, icon::CLOSE, "Dismiss").clicked() {
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

/// Centers a column no wider than the form width inside the available space.
fn form_column<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_width();
    let side = space::XL;
    let width = (avail - side * 2.0).min(size::FORM_MAX).max(200.0);
    let margin = ((avail - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui)
        })
        .inner
    })
    .inner
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events();
        #[cfg(feature = "demo")]
        self.demo_step(&ctx);
        if self.active.is_some() {
            self.browser_screen(ui);
        } else {
            self.connections_screen(ui);
        }
        self.modals(&ctx);
        self.toasts(&ctx);
    }
}

#[cfg(feature = "demo")]
impl App {
    /// Walks the UI into the screen named by MONGUI_DEMO_OPEN / MONGUI_DEMO_MODAL for screenshots.
    fn demo_step(&mut self, ctx: &egui::Context) {
        let open = std::env::var("MONGUI_DEMO_OPEN").ok();
        let modal = std::env::var("MONGUI_DEMO_MODAL").unwrap_or_default();
        if self.demo_stage == 0 && self.dark && std::env::var_os("MONGUI_DEMO_LIGHT").is_some() {
            self.dark = false;
            theme::apply(ctx, false);
        }
        match self.demo_stage {
            0 if self.active.is_none() && !modal.is_empty() && !modal.starts_with("edit") && !modal.starts_with("delete") => {
                self.modal = match modal.as_str() {
                    "compass" => Modal::CompassImport { read_keychain: false },
                    "export" => Modal::Export { include_shared: true },
                    _ => Modal::ConfirmDeleteProfile { index: 0 },
                };
                self.demo_stage = 9;
            }
            0 if !self.dbs.is_empty() => {
                if let Some((db, coll)) = open.as_deref().and_then(|o| o.split_once('.')) {
                    for n in self.dbs.iter_mut() {
                        n.requested = true;
                        self.worker.send(Cmd::ListCollections { db: n.info.name.clone() });
                        self.worker.send(Cmd::DbStats { db: n.info.name.clone() });
                    }
                    self.open_collection(db.into(), coll.into());
                    if let Ok(f) = std::env::var("MONGUI_DEMO_FILTER") {
                        if let Some(v) = self.view.as_mut() {
                            v.filter_text = f;
                        }
                        self.apply_query_text();
                    }
                }
                self.demo_stage = 1;
            }
            1 if self.view.as_ref().is_some_and(|v| !v.docs.is_empty()) => {
                match modal.as_str() {
                    "edit" => self.apply(Action::EditDoc(0)),
                    "delete" => self.apply(Action::DeleteDoc(0)),
                    _ => {}
                }
                self.demo_stage = 2;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PageItem::*, page_items};

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
