//! TLS-450 Plus Snapshot Audit Log Tool: open a TLS-450 Plus snapshot .tar,
//! pick a time range, filter and search its audit trail, alarm and leak test
//! events, and save or copy what is shown (or selected) as plain text.

#![windows_subsystem = "windows"]

#[cfg(feature = "review-shots")]
mod shots;
mod snapshot;

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui::{self, Align, Color32, Key, Label, Layout, Modifiers, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use snapshot::{Loaded, Row, Source, parse_local};

const APP_NAME: &str = "TLS-450 Plus Snapshot Audit Log Tool";
/// The one version number: Cargo.toml. build.rs stamps the same value into the
/// exe's file properties.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const RED: Color32 = Color32::from_rgb(235, 90, 90);
const AMBER: Color32 = Color32::from_rgb(235, 175, 70);
const BLUE: Color32 = Color32::from_rgb(110, 170, 240);
const GREEN: Color32 = Color32::from_rgb(110, 200, 120);

/// Width of the toolbar row labels (Time / Show / Filter), so the rows line up.
const ROW_LABEL_W: f32 = 46.0;
/// "Show everything around this time": this many seconds either side.
const AROUND_SECS: i64 = 15 * 60;

fn main() -> eframe::Result {
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("{APP_NAME} {APP_VERSION}"))
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([720.0, 420.0])
        .with_drag_and_drop(true);
    // Window and taskbar icon (the exe's own icon comes from build.rs).
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon-256.png")) {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            cc.egui_ctx.all_styles_mut(|s| {
                s.spacing.item_spacing.y = 6.0;
                // Always-visible scroll bars, so it's clear the table scrolls.
                s.spacing.scroll = egui::style::ScrollStyle::solid();
                s.visuals.faint_bg_color = if s.visuals.dark_mode {
                    Color32::from_gray(40)
                } else {
                    Color32::from_gray(238)
                };
            });
            Ok(Box::new(App::new(arg)))
        }),
    )
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Preset {
    All,
    Day,
    Week,
    Custom,
}

#[derive(Clone, PartialEq)]
struct Filters {
    from: String,
    to: String,
    preset: Preset,
    audit: bool,
    alarms: bool,
    manual_tests: bool,
    auto_tests: bool,
    user: String,
    device: String,
    search: String,
    /// The sort: a column (0 Time .. 6 Previous, as `Row::columns`) and its
    /// direction. Default: Time, newest first.
    sort: Sort,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Sort {
    col: usize,
    desc: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort { col: 0, desc: true }
    }
}

const COLUMN_NAMES: [&str; 7] = ["Time", "Source", "Device", "User", "Event", "Value", "Previous"];

impl Default for Filters {
    fn default() -> Self {
        Filters {
            from: String::new(),
            to: String::new(),
            preset: Preset::All,
            audit: true,
            alarms: true,
            manual_tests: true,
            auto_tests: false,
            user: String::new(),
            device: String::new(),
            search: String::new(),
            sort: Sort::default(),
        }
    }
}

enum Message {
    None,
    Good(String),
    /// Short text for the status bar, longer text for its hover.
    Error(String, String),
}

fn error(short: impl Into<String>) -> Message {
    let s = short.into();
    Message::Error(s.clone(), s)
}

/// What a time box is showing, for its outline.
#[derive(PartialEq)]
enum BoxState {
    Ok,
    Bad,
}

struct App {
    data: Option<Loaded>,
    /// Lower-case search text of each row, same order as `data.rows`.
    hay: Vec<String>,
    /// User and Device choices for the rows the tick boxes allow.
    users: Vec<String>,
    devices: Vec<String>,
    filters: Filters,
    /// Indexes (into `data.rows`) of the rows that pass the filters, in display order.
    shown: Vec<usize>,
    shown_for: Option<Filters>,
    /// Why the time range isn't applied (invalid box, From after To).
    range_error: Option<String>,
    from_bad: bool,
    to_bad: bool,
    /// Selected rows (indexes into `data.rows`); kept while they stay shown.
    selected: BTreeSet<usize>,
    /// Display position of the last plain click, for Shift+click ranges.
    anchor: Option<usize>,
    /// Row to bring into view on the next frame (index into `data.rows`).
    scroll_to: Option<usize>,
    message: Message,
    loading: Option<(PathBuf, Receiver<Result<Loaded, String>>)>,
    /// Opened at start because it was the newest snapshot next to the program.
    auto_opened: bool,
    focus_search: bool,
    #[cfg(feature = "review-shots")]
    shots: Option<shots::Shots>,
}

impl App {
    fn new(arg: Option<PathBuf>) -> App {
        let mut app = App {
            data: None,
            hay: vec![],
            users: vec![],
            devices: vec![],
            filters: Filters::default(),
            shown: vec![],
            shown_for: None,
            range_error: None,
            from_bad: false,
            to_bad: false,
            selected: BTreeSet::new(),
            anchor: None,
            scroll_to: None,
            message: Message::None,
            loading: None,
            auto_opened: false,
            focus_search: false,
            #[cfg(feature = "review-shots")]
            shots: shots::Shots::from_env(),
        };
        match arg {
            Some(p) => app.open(&p),
            None => {
                if let Some(p) = std::env::current_exe().ok().and_then(|e| e.parent().and_then(newest_snapshot)) {
                    app.auto_opened = true;
                    app.open(&p);
                }
            }
        }
        app
    }

    /// Starts loading a snapshot on a background thread.
    fn open(&mut self, path: &Path) {
        let path = if path.is_dir() {
            match newest_snapshot(path) {
                Some(p) => p,
                None => {
                    self.message = error(format!("No snapshot-*.tar in {}", path.display()));
                    return;
                }
            }
        } else {
            path.to_path_buf()
        };
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("tar")) {
            self.message = error(format!("Not a snapshot .tar: {}", file_name(&path)));
            return;
        }
        let (tx, rx) = channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(snapshot::load(&p));
        });
        self.loading = Some((path, rx));
    }

    /// Takes a finished load, if there is one.
    fn poll_loading(&mut self) {
        let Some((path, rx)) = &self.loading else { return };
        let Ok(result) = rx.try_recv() else { return };
        let path = path.clone();
        self.loading = None;
        match result {
            Ok(d) => {
                self.hay = d.rows.iter().map(Row::haystack).collect();
                self.filters = Filters::default();
                if let (Some(first), Some(last)) = (d.rows.first(), d.rows.last()) {
                    self.filters.from = first.time.clone();
                    self.filters.to = last.time.clone();
                }
                self.message = Message::None;
                self.selected.clear();
                self.anchor = None;
                self.data = Some(d);
                self.shown_for = None;
            }
            Err(e) => {
                let short = format!("Could not open {}: {e}", file_name(&path));
                let long = match &self.data {
                    Some(d) => format!("{short}\nStill showing {}.", file_name(&d.path)),
                    None => short.clone(),
                };
                self.message = Message::Error(short, long);
                self.auto_opened &= self.data.is_some();
            }
        }
    }

    fn refilter(&mut self) {
        if self.shown_for.as_ref() == Some(&self.filters) {
            return;
        }
        let Some(d) = &self.data else {
            self.shown.clear();
            self.shown_for = Some(self.filters.clone());
            return;
        };
        let f = &self.filters;
        let source_on = |r: &Row| match r.source {
            Source::Audit => f.audit,
            Source::Alarm => f.alarms,
            Source::Test if r.manual => f.manual_tests,
            Source::Test => f.auto_tests,
        };

        // User and Device lists follow the tick boxes.
        let mut users: Vec<String> = d
            .rows
            .iter()
            .filter(|r| source_on(r))
            .map(|r| r.user.clone())
            .filter(|u| !u.is_empty())
            .collect();
        users.sort();
        users.dedup();
        let mut devices: Vec<String> = d
            .rows
            .iter()
            .filter(|r| source_on(r))
            .map(|r| r.device.clone())
            .filter(|u| !u.is_empty())
            .collect();
        devices.sort_by_key(|s| natural_key(s));
        devices.dedup();

        // The time range: a box that isn't valid stops the range being applied,
        // and then nothing is shown (so nothing unintended gets saved).
        let from_txt = f.from.trim();
        let to_txt = f.to.trim();
        let from = parse_local(from_txt, false).map(|l| d.clock.to_utc(l));
        // To takes the later instant in the repeated fall-back hour.
        let to = parse_local(to_txt, true).map(|l| d.clock.to_utc_latest(l));
        let from_bad = !from_txt.is_empty() && from.is_none();
        let to_bad = !to_txt.is_empty() && to.is_none();
        let range_error = if from_bad {
            Some("From: use YYYY-MM-DD HH:MM".to_string())
        } else if to_bad {
            Some("To: use YYYY-MM-DD HH:MM".to_string())
        } else if let (Some(a), Some(b)) = (from, to)
            && a > b
        {
            Some("From is after To".to_string())
        } else {
            None
        };
        let after = range_error.as_deref() == Some("From is after To");

        let words: Vec<String> = f.search.to_lowercase().split_whitespace().map(str::to_string).collect();
        let mut shown: Vec<usize> = if range_error.is_some() {
            vec![]
        } else {
            d.rows
                .iter()
                .enumerate()
                .filter(|(i, r)| {
                    source_on(r)
                        && from.is_none_or(|t| r.utc >= t)
                        && to.is_none_or(|t| r.utc <= t)
                        && (f.user.is_empty() || r.user == f.user)
                        && (f.device.is_empty() || r.device == f.device)
                        && words.iter().all(|w| self.hay[*i].contains(w.as_str()))
                })
                .map(|(i, _)| i)
                .collect()
        };
        sort_rows(&mut shown, &d.rows, f.sort);

        // A User or Device that the tick boxes have hidden is dropped.
        let mut f2 = f.clone();
        if !f2.user.is_empty() && !users.contains(&f2.user) {
            f2.user.clear();
        }
        if !f2.device.is_empty() && !devices.contains(&f2.device) {
            f2.device.clear();
        }
        self.users = users;
        self.devices = devices;
        self.range_error = range_error;
        self.from_bad = from_bad || after;
        self.to_bad = to_bad || after;
        let visible: HashSet<usize> = shown.iter().copied().collect();
        self.selected.retain(|i| visible.contains(i));
        // A pending scroll to a row that is no longer shown is dropped.
        if self.scroll_to.is_some_and(|r| !visible.contains(&r)) {
            self.scroll_to = None;
        }
        self.anchor = None;
        self.shown = shown;
        if f2 != self.filters {
            self.filters = f2;
            self.shown_for = None;
            self.refilter();
        } else {
            self.shown_for = Some(self.filters.clone());
        }
    }

    /// The last day / week before the newest event, or everything.
    fn quick_range(&mut self, preset: Preset) {
        let Some(d) = &self.data else { return };
        let (Some(first), Some(last)) = (d.rows.first(), d.rows.last()) else {
            return;
        };
        self.filters.to = last.time.clone();
        self.filters.from = match preset {
            Preset::Day => d.clock.local(last.utc - 86400),
            Preset::Week => d.clock.local(last.utc - 7 * 86400),
            _ => first.time.clone(),
        };
        self.filters.preset = preset;
    }

    fn reset_filters(&mut self) {
        self.filters = Filters {
            sort: self.filters.sort,
            ..Filters::default()
        };
        self.quick_range(Preset::All);
    }

    /// Every event within 15 minutes of a row: every kind of event shown, User,
    /// Device and Search cleared; the row stays selected and in view.
    fn show_around(&mut self, row: usize) {
        let Some(d) = &self.data else { return };
        let utc = d.rows[row].utc;
        self.filters.from = d.clock.local(utc - AROUND_SECS);
        self.filters.to = d.clock.local(utc + AROUND_SECS);
        self.filters.preset = Preset::Custom;
        self.filters.audit = true;
        self.filters.alarms = true;
        self.filters.manual_tests = true;
        self.filters.auto_tests = true;
        self.filters.user.clear();
        self.filters.device.clear();
        self.filters.search.clear();
        self.refilter();
        self.selected = BTreeSet::from([row]);
        self.anchor = self.shown.iter().position(|&i| i == row);
        self.scroll_to = Some(row);
    }

    /// Ctrl+A: every row shown.
    fn select_all(&mut self) {
        self.selected = self.shown.iter().copied().collect();
        // Shift+click after Ctrl+A extends from the top, rather than starting over.
        self.anchor = (!self.shown.is_empty()).then_some(0);
    }

    /// Right-click "Select all events within 15 min": the same window as
    /// `show_around`, with every row in it selected, ready to save.
    fn select_around(&mut self, row: usize) {
        self.show_around(row);
        self.select_all();
    }

    /// The rows Save and Copy use: the selected ones if any, else all shown.
    fn output_rows(&self) -> Vec<&Row> {
        let Some(d) = &self.data else { return vec![] };
        if self.selected.is_empty() {
            self.shown.iter().map(|&i| &d.rows[i]).collect()
        } else {
            self.shown
                .iter()
                .filter(|i| self.selected.contains(i))
                .map(|&i| &d.rows[i])
                .collect()
        }
    }

    fn output_what(&self) -> String {
        let n = self.output_rows().len();
        let base = if self.selected.is_empty() {
            format!("the {n} rows shown")
        } else {
            format!("the {n} selected rows")
        };
        match self.sort_note() {
            Some(note) => format!("{base}, in this order ({note})"),
            None => base,
        }
    }

    /// "sorted by User, A to Z" when the sort isn't the default (Time, newest first).
    fn sort_note(&self) -> Option<String> {
        self.sort_parts().map(|(col, dir)| format!("sorted by {col}, {dir}"))
    }

    /// The column name and direction of a non-default sort.
    fn sort_parts(&self) -> Option<(&'static str, &'static str)> {
        let s = self.filters.sort;
        (s != Sort::default()).then(|| (COLUMN_NAMES[s.col], sort_words(s.col, s.desc)))
    }

    fn zone_label(&self) -> String {
        self.data.as_ref().map(|d| d.clock.name.clone()).unwrap_or_default()
    }

    fn nothing_reason(&self) -> Option<String> {
        if self.data.is_none() {
            Some("Nothing to save: no snapshot is open".into())
        } else if let Some(e) = &self.range_error {
            Some(format!("Nothing to save: {e}"))
        } else if self.shown.is_empty() {
            Some("Nothing to save: no events match the filters".into())
        } else {
            None
        }
    }

    fn save(&mut self) {
        self.refilter();
        if let Some(r) = self.nothing_reason() {
            self.message = error(r);
            return;
        }
        let Some(d) = &self.data else { return };
        let stem = d.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        // The suggested name carries the span of the rows saved, so two saves
        // from one snapshot don't overwrite each other.
        let span = {
            let rows = self.output_rows();
            let first = rows.iter().map(|r| r.time.as_str()).min().unwrap_or_default();
            let last = rows.iter().map(|r| r.time.as_str()).max().unwrap_or_default();
            file_span(first, last)
        };
        let mut dlg = rfd::FileDialog::new()
            .add_filter("Text file", &["txt"])
            .set_file_name(format!("{stem} audit log {span}.txt"));
        if let Some(dir) = d.path.parent() {
            dlg = dlg.set_directory(dir);
        }
        let Some(out) = dlg.save_file() else { return };
        let rows = self.output_rows();
        let n = rows.len();
        let kind = if self.selected.is_empty() { "rows" } else { "selected rows" };
        match std::fs::write(&out, to_text(&rows, &self.zone_label())) {
            Ok(()) => self.message = Message::Good(format!("Saved {n} {kind} to {}", out.display())),
            Err(e) => self.message = error(format!("Could not save {}: {e}", out.display())),
        }
    }

    fn copy(&mut self, ctx: &egui::Context) {
        self.refilter();
        if let Some(r) = self.nothing_reason() {
            self.message = error(r.replace("Nothing to save", "Nothing to copy"));
            return;
        }
        let rows = self.output_rows();
        let n = rows.len();
        let kind = if self.selected.is_empty() { "rows" } else { "selected rows" };
        ctx.copy_text(to_text(&rows, &self.zone_label()));
        self.message = Message::Good(format!("Copied {n} {kind}"));
    }

    fn pick_and_open(&mut self) {
        let mut dlg = rfd::FileDialog::new().add_filter("TLS-450 Plus snapshot", &["tar"]);
        if let Some(dir) = self.data.as_ref().and_then(|d| d.path.parent()) {
            dlg = dlg.set_directory(dir);
        }
        if let Some(p) = dlg.pick_file() {
            self.auto_opened = false;
            self.open(&p);
        }
    }

    /// A click on the row at display position `pos`.
    fn click_row(&mut self, pos: usize, ctrl: bool, shift: bool) {
        let row = self.shown[pos];
        if shift && let Some(a) = self.anchor {
            let (lo, hi) = (a.min(pos), a.max(pos));
            if !ctrl {
                self.selected.clear();
            }
            self.selected.extend(self.shown[lo..=hi].iter().copied());
            return;
        }
        if ctrl {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
        } else if self.selected.len() == 1 && self.selected.contains(&row) {
            self.selected.clear();
        } else {
            self.selected = BTreeSet::from([row]);
        }
        self.anchor = Some(pos);
    }
}

/// Fixed-width columns, a header line, Windows line endings. Value and
/// Previous can be long; they run past their column rather than widening it
/// for every line.
fn to_text(rows: &[&Row], zone: &str) -> String {
    const CAP: usize = 48;
    let time_head = if zone.is_empty() {
        "TIME".to_string()
    } else {
        format!("TIME ({zone})")
    };
    let heads = [time_head.as_str(), "SOURCE", "DEVICE", "USER", "EVENT", "VALUE", "PREVIOUS"];
    let mut w: Vec<usize> = heads.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.columns().iter().enumerate() {
            let n = c.chars().count();
            w[i] = w[i].max(if i >= 5 { n.min(CAP) } else { n });
        }
    }
    let line = |c: &[&str]| -> String {
        let mut s = String::new();
        for (i, v) in c.iter().enumerate() {
            s.push_str(v);
            if i + 1 < c.len() {
                let pad = w[i].saturating_sub(v.chars().count());
                s.push_str(&" ".repeat(pad + 2));
            }
        }
        s.trim_end().to_string() + "\r\n"
    };
    let mut out = line(&heads);
    for r in rows {
        out.push_str(&line(&r.columns()));
    }
    out
}

/// "2026-11-04 12:03:58" .. "2026-11-04 12:07:49" -> "2026-11-04 1203-1207";
/// across days "2026-11-04 1203 to 2026-11-05 0939". No colons (not allowed in
/// Windows file names).
fn file_span(first: &str, last: &str) -> String {
    let part = |t: &str| -> (String, String) {
        let (d, time) = t.split_once(' ').unwrap_or((t, ""));
        (d.to_string(), time.get(..5).unwrap_or("").replace(':', ""))
    };
    let (d1, t1) = part(first);
    let (d2, t2) = part(last);
    if d1.is_empty() {
        String::new()
    } else if d1 == d2 {
        format!("{d1} {t1}-{t2}")
    } else {
        format!("{d1} {t1} to {d2} {t2}")
    }
}

/// How one cell sorts: a cell that is a whole number (e.g. "-166.2") sorts
/// numerically, before any text; text sorts with numbers inside it compared
/// as numbers (Tank 2 before Tank 10).
#[derive(PartialEq)]
enum SortKey {
    Num(f64),
    Text(Vec<(String, String)>),
}

fn sort_key(cell: &str) -> SortKey {
    match cell.trim().parse::<f64>() {
        Ok(v) if v.is_finite() => SortKey::Num(v),
        _ => SortKey::Text(natural_key_str(&cell.to_lowercase())),
    }
}

fn cmp_keys(a: &SortKey, b: &SortKey) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (SortKey::Num(x), SortKey::Num(y)) => x.total_cmp(y),
        (SortKey::Num(_), SortKey::Text(_)) => Ordering::Less,
        (SortKey::Text(_), SortKey::Num(_)) => Ordering::Greater,
        (SortKey::Text(x), SortKey::Text(y)) => x.cmp(y),
    }
}

/// Text split into (letters, digits) pieces; the digits are kept as text with
/// leading zeros dropped and compared by length first, so any length of
/// number compares correctly.
fn natural_key_str(s: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let mut text = String::new();
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((std::mem::take(&mut text), digits_key(&num)));
                num.clear();
            }
            text.push(c);
        }
    }
    out.push((text, digits_key(&num)));
    out
}

/// "007" -> "001007"-style key: length (6 wide) then the digits, so 9 < 10.
fn digits_key(d: &str) -> String {
    let d = d.trim_start_matches('0');
    format!("{:06}{d}", d.len())
}

/// Sorts row indexes (given in time order). Time: newest or oldest first.
/// Another column: numbers numerically, then text (numbers inside compared as
/// numbers), ties kept newest first; rows with that cell blank always go last.
fn sort_rows(shown: &mut Vec<usize>, rows: &[Row], sort: Sort) {
    if sort.col == 0 {
        if sort.desc {
            shown.reverse();
        }
        return;
    }
    let c = sort.col;
    shown.reverse(); // ties: newest first
    let (filled, blank): (Vec<usize>, Vec<usize>) = shown.iter().partition(|&&i| !rows[i].columns()[c].is_empty());
    let mut keyed: Vec<(SortKey, usize)> = filled.into_iter().map(|i| (sort_key(rows[i].columns()[c]), i)).collect();
    keyed.sort_by(|a, b| cmp_keys(&a.0, &b.0)); // stable: ties stay newest first
    let mut out: Vec<usize> = if sort.desc {
        // Reverse the order of the keys but keep ties newest first.
        let mut groups: Vec<(SortKey, Vec<usize>)> = vec![];
        for (k, i) in keyed {
            match groups.last_mut() {
                Some((gk, g)) if cmp_keys(gk, &k).is_eq() => g.push(i),
                _ => groups.push((k, vec![i])),
            }
        }
        groups.into_iter().rev().flat_map(|(_, g)| g).collect()
    } else {
        keyed.into_iter().map(|(_, i)| i).collect()
    };
    out.extend(blank);
    *shown = out;
}

/// How a sort reads in words, for the header's hover text.
fn sort_words(col: usize, desc: bool) -> &'static str {
    match (col, desc) {
        (0, true) => "newest first",
        (0, false) => "oldest first",
        (_, false) => "A to Z",
        (_, true) => "Z to A",
    }
}

/// One row, tab-separated (pastes into a spreadsheet or an email table).
fn row_tsv(r: &Row) -> String {
    r.columns().join("\t")
}

const TSV_HEADER: &str = "Time\tSource\tDevice\tUser\tEvent\tValue\tPrevious";

fn newest_snapshot(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            n.starts_with("snapshot-") && n.ends_with(".tar")
        })
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

/// Sort key so "Tank 2" comes before "Tank 10" (case ignored); the same rule
/// the table's column sort uses for text.
fn natural_key(s: &str) -> Vec<(String, String)> {
    natural_key_str(&s.to_lowercase())
}

fn row_label(ui: &mut egui::Ui, text: &str) {
    let h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(egui::vec2(ROW_LABEL_W, h), Layout::left_to_right(Align::Center), |ui| {
        ui.set_width(ROW_LABEL_W);
        ui.label(RichText::new(text).strong());
    });
}

fn time_box(ui: &mut egui::Ui, value: &mut String, state: BoxState) -> bool {
    let mut edit = TextEdit::singleline(value).desired_width(150.0).hint_text("YYYY-MM-DD HH:MM");
    if state == BoxState::Bad {
        edit = edit.text_color(RED);
    }
    let r = ui
        .add(edit)
        .on_hover_text("Console local time: YYYY-MM-DD, YYYY-MM-DD HH:MM or YYYY-MM-DD HH:MM:SS");
    if state == BoxState::Bad {
        ui.painter()
            .rect_stroke(r.rect, 2.0, egui::Stroke::new(1.5, RED), egui::StrokeKind::Outside);
    }
    r.changed()
}

fn source_color(s: Source) -> Option<Color32> {
    match s {
        Source::Audit => None,
        Source::Alarm => Some(AMBER),
        Source::Test => Some(BLUE),
    }
}

impl App {
    fn toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let has_data = self.data.is_some();
        let can = self.nothing_reason().is_none();
        let what = self.output_what();
        // The buttons say exactly what they will save: the selection, or every row shown.
        let n_out = self.output_rows().len();
        let picked = !self.selected.is_empty();
        // One label pattern always: the count (0 when there is nothing to save).
        let (save_txt, copy_txt) = if picked && can {
            (format!("Save {n_out} selected to .txt..."), format!("Copy {n_out} selected"))
        } else {
            let n = if can { n_out } else { 0 };
            (format!("Save {n} rows to .txt..."), format!("Copy {n} rows"))
        };
        // A selection is marked with an outline in the selection colour, painted
        // over the button so its size doesn't change; the blue fill is kept for
        // the selected rows themselves.
        let sel_stroke = egui::Stroke::new(1.5, ui.visuals().selection.bg_fill);
        let mark = |ui: &egui::Ui, r: &egui::Response| {
            if picked {
                ui.painter().rect_stroke(r.rect, 3.0, sel_stroke, egui::StrokeKind::Inside);
            }
        };
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open snapshot...").on_hover_text("Ctrl+O").clicked() {
                self.pick_and_open();
            }
            let r = ui.add_enabled(can, egui::Button::new(save_txt));
            mark(ui, &r);
            if r.on_hover_text(format!("Saves {what}. Ctrl+S")).clicked() {
                self.save();
            }
            let r = ui.add_enabled(can, egui::Button::new(copy_txt));
            mark(ui, &r);
            if r.on_hover_text(format!("Copies {what}, as in the .txt")).clicked() {
                self.copy(ctx);
            }
            if picked && ui.button("Clear selection").on_hover_text("Esc").clicked() {
                self.selected.clear();
                self.anchor = None;
                ctx.request_repaint();
            }
            ui.add_enabled_ui(has_data, |ui| {
                if ui.button("Reset filters").clicked() {
                    self.reset_filters();
                }
            });
        });
        ui.add_enabled_ui(has_data, |ui| {
            ui.horizontal_wrapped(|ui| {
                row_label(ui, "Time");
                ui.label("From");
                let s = if self.from_bad { BoxState::Bad } else { BoxState::Ok };
                let mut changed = time_box(ui, &mut self.filters.from, s);
                ui.label("To");
                let s = if self.to_bad { BoxState::Bad } else { BoxState::Ok };
                changed |= time_box(ui, &mut self.filters.to, s);
                if changed {
                    self.filters.preset = Preset::Custom;
                }
                ui.add_space(6.0);
                // The quick ranges as one segmented control, the same height as the boxes.
                egui::Frame::new()
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                    .corner_radius(4)
                    .inner_margin(egui::Margin::symmetric(2, 0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.spacing_mut().button_padding.y = 1.0;
                        ui.horizontal(|ui| {
                            for (p, label, tip) in [
                                (Preset::All, "All", "Every event in the snapshot"),
                                (Preset::Day, "Last 24 h", "The 24 hours before the newest event in the snapshot"),
                                (Preset::Week, "Last 7 days", "The 7 days before the newest event in the snapshot"),
                            ] {
                                if ui.selectable_label(self.filters.preset == p, label).on_hover_text(tip).clicked() {
                                    self.quick_range(p);
                                }
                            }
                        });
                    });
                // After the quick ranges, so they don't move when it appears.
                if let Some(e) = &self.range_error {
                    ui.label(RichText::new(e).color(RED));
                }
            });
            ui.horizontal_wrapped(|ui| {
                row_label(ui, "Show");
                let tests_tip = "Leak tests appear when they end. The console does not record who starts them.";
                ui.checkbox(&mut self.filters.audit, "Audit trail");
                ui.checkbox(&mut self.filters.alarms, "Alarms");
                ui.checkbox(&mut self.filters.manual_tests, "Manual leak tests")
                    .on_hover_text(tests_tip);
                ui.checkbox(&mut self.filters.auto_tests, "Automatic leak tests")
                    .on_hover_text(tests_tip);
            });
            ui.horizontal_wrapped(|ui| {
                row_label(ui, "Filter");
                ui.horizontal(|ui| {
                    ui.label("User");
                    let user_txt = if self.filters.user.is_empty() {
                        "All".to_string()
                    } else {
                        self.filters.user.clone()
                    };
                    egui::ComboBox::from_id_salt("user")
                        .width(170.0)
                        .selected_text(user_txt)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.filters.user, String::new(), "All");
                            for u in &self.users {
                                ui.selectable_value(&mut self.filters.user, u.clone(), u.as_str());
                            }
                        })
                        .response
                        .on_hover_text("Only audit trail rows have a user");
                });
                ui.horizontal(|ui| {
                    ui.label("Device");
                    let dev_txt = if self.filters.device.is_empty() {
                        "All".to_string()
                    } else {
                        self.filters.device.clone()
                    };
                    egui::ComboBox::from_id_salt("device")
                        .width(190.0)
                        .selected_text(dev_txt)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.filters.device, String::new(), "All");
                            for d in &self.devices {
                                ui.selectable_value(&mut self.filters.device, d.clone(), d.as_str());
                            }
                        });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Search");
                    let r = ui.add(
                        TextEdit::singleline(&mut self.filters.search)
                            .id(egui::Id::new("search"))
                            .desired_width((ui.available_width() - 40.0).clamp(200.0, 420.0))
                            .hint_text("words, all must match"),
                    );
                    let r = r.on_hover_text("Ctrl+F. Every word must appear somewhere in the row. Esc clears.");
                    if self.focus_search {
                        r.request_focus();
                        self.focus_search = false;
                    }
                    if !self.filters.search.is_empty() && ui.small_button("x").on_hover_text("Clear search").clicked() {
                        self.filters.search.clear();
                    }
                });
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some((p, _)) = &self.loading {
                ui.spinner();
                ui.label(format!("Loading {}...", file_name(p)));
                ui.separator();
            }
            match &self.data {
                Some(d) => {
                    let mut tip = d.path.display().to_string();
                    if self.auto_opened {
                        tip.push_str("\n(opened at start: the newest snapshot next to the program)");
                    }
                    ui.label(file_name(&d.path)).on_hover_text(tip);
                    ui.separator();
                    ui.label(format!("Showing {} of {}", self.shown.len(), d.rows.len()));
                    if !self.selected.is_empty() {
                        ui.label(format!("({} selected)", self.selected.len()));
                    }
                    if let Some((col, dir)) = self.sort_parts() {
                        ui.separator();
                        ui.label(RichText::new(format!("Sorted by {col}, {dir}")).strong());
                    }
                    ui.separator();
                    ui.label(format!("Times: {}", d.clock.zone()));
                    if !d.missing.is_empty() {
                        ui.separator();
                        ui.label(RichText::new(format!("Not in this snapshot: {}", d.missing.join(", "))).color(RED));
                    }
                    if d.skipped > 0 {
                        ui.separator();
                        ui.label(RichText::new(format!("{} unreadable rows skipped", d.skipped)).color(RED));
                    }
                }
                None if self.loading.is_none() => {
                    ui.label("No snapshot open");
                }
                None => {}
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &self.message {
                Message::None => {}
                Message::Good(m) => {
                    ui.add(Label::new(RichText::new(m).color(GREEN)).truncate()).on_hover_text(m);
                    ui.separator();
                }
                Message::Error(short, long) => {
                    ui.add(Label::new(RichText::new(short).color(RED)).truncate()).on_hover_text(long);
                    ui.separator();
                }
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "review-shots")]
        shots::drive(self, ui.ctx());

        let ctx = ui.ctx().clone();
        self.poll_loading();
        if self.loading.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Files dropped on, or dragged over, the window.
        let (dropped, hovering) = ctx.input(|i| {
            (
                i.raw.dropped_files.first().map(|f| f.path().to_path_buf()),
                !i.raw.hovered_files.is_empty(),
            )
        });
        if let Some(p) = dropped {
            self.auto_opened = false;
            self.open(&p);
        }

        // Keyboard: Ctrl+O open, Ctrl+S save, Ctrl+F search, Esc clears the
        // search (in the search box) or the selection (when no box has focus).
        let (k_open, k_save, k_find, k_esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::F),
                i.key_pressed(Key::Escape),
            )
        });
        if k_open {
            self.pick_and_open();
        }
        if k_save {
            self.save();
        }
        if k_find {
            self.focus_search = true;
        }
        let focused = ctx.memory(|m| m.focused());
        if k_esc {
            if focused == Some(egui::Id::new("search")) {
                self.filters.search.clear();
            } else if focused.is_none() && !ctx.any_popup_open() {
                self.selected.clear();
                self.anchor = None;
            }
        }
        // Ctrl+A selects every row shown, unless a text box has focus (then it
        // selects that box's text as usual).
        if focused.is_none() && self.data.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.refilter();
            self.select_all();
        }

        self.refilter();
        egui::Panel::top("controls").show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
                self.toolbar(ui, &ctx);
            });
        });
        // Changes made in the toolbar this frame are shown this frame.
        self.refilter();
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        let mut reset = false;
        let mut open_clicked = false;
        let mut clicked: Option<(usize, bool, bool)> = None;
        // (row, select every row in the window too)
        let mut around: Option<(usize, bool)> = None;
        let mut copied_row: Option<bool> = None;
        let mut copy_selection = false;
        let mut sort_click: Option<usize> = None;
        let sort = self.filters.sort;
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(d) = &self.data else {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    ui.heading("No snapshot open");
                    ui.add_space(8.0);
                    ui.label("Open a TLS-450 Plus snapshot (.tar), or drag one onto this window.");
                    ui.add_space(8.0);
                    if ui.button("Open snapshot...").clicked() {
                        open_clicked = true;
                    }
                });
                return;
            };
            let shown = &self.shown;
            if shown.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);
                    if self.range_error.is_some() {
                        ui.label(RichText::new("No rows: the time range isn't valid").size(16.0));
                        ui.add_space(6.0);
                        ui.label("Fix the boxes outlined in red above, or reset the filters.");
                    } else {
                        ui.label(RichText::new("No events match the filters.").size(16.0));
                    }
                    ui.add_space(8.0);
                    if ui.button("Reset filters").clicked() {
                        reset = true;
                    }
                });
                return;
            }
            let strong = ui.visuals().strong_text_color();
            let selected = &self.selected;
            let scroll_pos = self.scroll_to.and_then(|r| shown.iter().position(|&i| i == r));
            let mods = ctx.input(|i| i.modifiers);
            let mut copy: Option<(usize, bool)> = None;
            // Table min width: fixed columns + Event's minimum. Narrower windows scroll sideways.
            const MIN_TABLE: f32 = 156.0 + 62.0 + 80.0 + 70.0 + 160.0 + 70.0 + 60.0 + 7.0 * 8.0;
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_min_width(MIN_TABLE.max(ui.available_width()));
                // Row hover: a blue-grey that stands apart from the grey stripes
                // (set for the table only; buttons keep their own hover colour).
                ui.visuals_mut().widgets.hovered.bg_fill = if ui.visuals().dark_mode {
                    Color32::from_rgb(52, 66, 86)
                } else {
                    Color32::from_rgb(214, 226, 242)
                };
                let row_h = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
                // Column widths (Device, User, Value, Previous). Narrow windows get
                // their own remembered set, so Value still fits at about 860 pt
                // after the window has been wider.
                let narrow = ui.available_width() < 1100.0;
                let [w_dev, w_user, w_val, w_prev] = if narrow {
                    [105.0, 80.0, 100.0, 60.0]
                } else {
                    [140.0, 120.0, 150.0, 100.0]
                };
                let mut table = TableBuilder::new(ui)
                    .id_salt(if narrow { "rows-narrow" } else { "rows-wide" })
                    .striped(true)
                    .resizable(true)
                    .sense(egui::Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::exact(156.0))
                    .column(Column::exact(62.0).clip(true))
                    .column(Column::initial(w_dev).at_least(80.0).clip(true))
                    .column(Column::initial(w_user).at_least(70.0).clip(true))
                    .column(Column::remainder().at_least(160.0).clip(true))
                    .column(Column::initial(w_val).at_least(70.0).clip(true))
                    .column(Column::initial(w_prev).at_least(60.0).clip(true))
                    .min_scrolled_height(0.0);
                if let Some(p) = scroll_pos {
                    table = table.scroll_to_row(p, Some(Align::Center));
                }
                table
                    .header(row_h + 2.0, |mut h| {
                        // Click a header to sort by it; click again to reverse.
                        for (c, name) in COLUMN_NAMES.iter().enumerate() {
                            h.col(|ui| {
                                let on = sort.col == c;
                                let text = if on {
                                    format!("{name} {}", if sort.desc { "⏷" } else { "⏶" })
                                } else {
                                    name.to_string()
                                };
                                let mut rt = RichText::new(text).strong();
                                if on {
                                    rt = rt.color(ui.visuals().selection.stroke.color);
                                }
                                let then = if c == 0 { "" } else { ", then newest first" };
                                let tip = if on {
                                    format!("Sorted by {name}, {}{then}. Click to reverse.", sort_words(c, sort.desc))
                                } else {
                                    format!("Click to sort by {name}; click again to reverse.")
                                };
                                let r = ui
                                    .add(Label::new(rt).sense(egui::Sense::click()).selectable(false))
                                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                                    .on_hover_text(tip);
                                if r.clicked() {
                                    sort_click = Some(c);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(row_h, shown.len(), |mut row| {
                            let pos = row.index();
                            let idx = shown[pos];
                            let r = &d.rows[idx];
                            let is_sel = selected.contains(&idx);
                            row.set_selected(is_sel);
                            for (i, v) in r.columns().iter().enumerate() {
                                row.col(|ui| {
                                    // ALARM and TEST are a small rounded tag on every row:
                                    // a tinted tag with coloured text normally, a solid tag
                                    // with dark text on a selected row (readable on the
                                    // selection blue, and the same shape either way).
                                    // AUDIT goes through the same frame (no fill) so all three
                                    // line up.
                                    if i == 1 {
                                        let dark = ui.visuals().dark_mode;
                                        let (fill, text) = match source_color(r.source) {
                                            Some(c) if is_sel => (c, Color32::BLACK),
                                            // Text brighter (dark mode) or darker (light mode)
                                            // than the tint, for contrast.
                                            Some(c) if dark => (c.gamma_multiply(0.18), c.lerp_to_gamma(Color32::WHITE, 0.25)),
                                            Some(c) => (c.gamma_multiply(0.25), c.lerp_to_gamma(Color32::BLACK, 0.45)),
                                            None => (Color32::TRANSPARENT, strong),
                                        };
                                        let bold = r.source != Source::Audit;
                                        let mut t = RichText::new(*v).color(text);
                                        if bold {
                                            t = t.strong();
                                        }
                                        egui::Frame::new()
                                            .fill(fill)
                                            .corner_radius(3)
                                            .inner_margin(egui::Margin::symmetric(4, 0))
                                            .show(ui, |ui| {
                                                ui.add(Label::new(t).truncate().selectable(false));
                                            });
                                        return;
                                    }
                                    let t = RichText::new(*v).color(strong);
                                    let resp = ui.add(Label::new(t).truncate().selectable(false));
                                    if !v.is_empty() {
                                        resp.on_hover_text(*v);
                                    }
                                });
                            }
                            let resp = row.response();
                            if resp.clicked() {
                                clicked = Some((pos, mods.command, mods.shift));
                            }
                            resp.context_menu(|ui| {
                                if ui
                                    .button("Show all events within 15 min")
                                    .on_hover_text("Every kind of event, any user and device, 15 minutes either side of this row")
                                    .clicked()
                                {
                                    around = Some((idx, false));
                                    ui.close();
                                }
                                if ui
                                    .button("Select all events within 15 min")
                                    .on_hover_text("The same, with every row in it selected, ready to save or copy")
                                    .clicked()
                                {
                                    around = Some((idx, true));
                                    ui.close();
                                }
                                ui.separator();
                                if is_sel && selected.len() > 1 && ui.button(format!("Copy {} selected rows", selected.len())).clicked() {
                                    copy_selection = true;
                                    ui.close();
                                }
                                if ui.button("Copy row").clicked() {
                                    copy = Some((idx, false));
                                    ui.close();
                                }
                                if ui.button("Copy row with header").clicked() {
                                    copy = Some((idx, true));
                                    ui.close();
                                }
                            });
                        });
                    });
            });
            if let Some((idx, header)) = copy {
                let line = row_tsv(&d.rows[idx]);
                ctx.copy_text(if header { format!("{TSV_HEADER}\r\n{line}") } else { line });
                copied_row = Some(header);
            }
        });
        if let Some(header) = copied_row {
            self.message = Message::Good(if header { "Copied 1 row with header" } else { "Copied 1 row" }.into());
        }
        if scroll_done(self) {
            self.scroll_to = None;
        }
        if let Some((pos, ctrl, shift)) = clicked {
            self.click_row(pos, ctrl, shift);
            // A click in the table takes keyboard focus from any toolbar widget,
            // so Esc and Ctrl+A act on the rows next.
            if let Some(f) = ctx.memory(|m| m.focused()) {
                ctx.memory_mut(|m| m.surrender_focus(f));
            }
        }
        match around {
            Some((idx, false)) => self.show_around(idx),
            Some((idx, true)) => self.select_around(idx),
            None => {}
        }
        if let Some(c) = sort_click {
            // Same column: reverse. New column: Time starts newest first, the
            // others A to Z.
            self.filters.sort = if self.filters.sort.col == c {
                Sort {
                    col: c,
                    desc: !self.filters.sort.desc,
                }
            } else {
                Sort { col: c, desc: c == 0 }
            };
            // Bring the selection (or the top) into view in the new order.
            self.refilter();
            self.scroll_to = self
                .shown
                .iter()
                .find(|i| self.selected.contains(i))
                .or(self.shown.first())
                .copied();
            ctx.request_repaint();
        }
        if copy_selection {
            self.copy(&ctx);
        }
        // Anything that changed the rows or the selection this frame is drawn
        // again at once, so the toolbar labels never lag a frame behind.
        if clicked.is_some()
            || around.is_some()
            || reset
            || copied_row.is_some()
            || copy_selection
            || self.shown_for.as_ref() != Some(&self.filters)
        {
            ctx.request_repaint();
        }
        if reset {
            self.reset_filters();
        }
        if open_clicked {
            self.pick_and_open();
        }

        // Drag-over highlight.
        if hovering {
            let rect = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a snapshot .tar to open it",
                egui::FontId::proportional(22.0),
                Color32::WHITE,
            );
        }
    }
}

/// The scroll request is used once it has been drawn with its row present.
fn scroll_done(app: &App) -> bool {
    app.scroll_to.is_some_and(|r| app.shown.contains(&r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(time: &str, user: &str, event: &str, value: &str) -> Row {
        Row {
            utc: 0,
            time: time.into(),
            source: Source::Audit,
            manual: false,
            device: String::new(),
            user: user.into(),
            event: event.into(),
            value: value.into(),
            previous: String::new(),
        }
    }

    #[test]
    fn text_output_lines_up() {
        let r = row("2026-11-05 09:01:11", "nologinuser", "TLS Security: Login", "GUI");
        let t = to_text(&[&r], "US/Mountain");
        let lines: Vec<&str> = t.split("\r\n").collect();
        assert!(lines[0].starts_with("TIME (US/Mountain)   SOURCE  DEVICE  USER"));
        assert!(lines[1].starts_with("2026-11-05 09:01:11  AUDIT           nologinuser  TLS Security: Login  GUI"));
        assert_eq!(lines.len(), 3); // header, row, and the empty piece after the last CRLF
    }

    #[test]
    fn long_values_do_not_widen_every_line() {
        let long = "x".repeat(200);
        let a = row("2026-11-05 09:01:11", "u", "E", &long);
        let b = row("2026-11-05 09:01:12", "u", "E", "short");
        let t = to_text(&[&a, &b], "");
        let second = t.split("\r\n").nth(2).unwrap();
        assert!(second.len() < 120, "{second}");
    }

    #[test]
    fn long_events_keep_value_aligned() {
        let a = row("2026-11-05 09:01:11", "u", &"E".repeat(70), "v1");
        let b = row("2026-11-05 09:01:12", "u", "E", "v2");
        let t = to_text(&[&a, &b], "");
        let lines: Vec<&str> = t.split("\r\n").collect();
        assert_eq!(lines[1].find("v1"), lines[2].find("v2"));
    }

    #[test]
    fn save_name_span() {
        assert_eq!(file_span("2026-11-04 12:03:58", "2026-11-04 12:07:49"), "2026-11-04 1203-1207");
        assert_eq!(
            file_span("2026-11-04 12:03:58", "2026-11-05 09:39:17"),
            "2026-11-04 1203 to 2026-11-05 0939"
        );
        assert!(!file_span("2026-11-04 12:03:58", "2026-11-04 12:07:49").contains(':'));
    }

    #[test]
    fn numbers_sort_as_numbers() {
        let vals = [
            "1.9",
            "-84.1",
            "start volume 7085 gal",
            "1.10",
            "-166.2",
            "1.0",
            "Tank 10",
            "Tank 2",
            "GUI",
        ];
        let rows: Vec<Row> = vals
            .iter()
            .enumerate()
            .map(|(i, v)| row(&format!("2026-11-05 09:00:{i:02}"), "u", "E", v))
            .collect();
        let mut v: Vec<usize> = (0..rows.len()).collect();
        sort_rows(&mut v, &rows, Sort { col: 5, desc: false });
        let got: Vec<&str> = v.iter().map(|&i| rows[i].value.as_str()).collect();
        assert_eq!(
            got,
            [
                "-166.2",
                "-84.1",
                "1.0",
                "1.10",
                "1.9",
                "GUI",
                "start volume 7085 gal",
                "Tank 2",
                "Tank 10"
            ]
        );
        // A long digit run doesn't collapse to 0.
        assert!(natural_key_str("x99999999999999999999999") > natural_key_str("x5"));
    }

    #[test]
    fn column_sort_keeps_blanks_last_and_ties_newest_first() {
        // Rows 0..5 in time order; users: b, "", a, b, "".
        let rows: Vec<Row> = ["b", "", "a", "b", ""]
            .iter()
            .enumerate()
            .map(|(i, u)| row(&format!("2026-11-05 09:00:0{i}"), u, "E", ""))
            .collect();
        let by_user = |desc| {
            let mut v: Vec<usize> = (0..rows.len()).collect();
            sort_rows(&mut v, &rows, Sort { col: 3, desc });
            v
        };
        assert_eq!(by_user(false), [2, 3, 0, 4, 1]); // a, b (newest first), blanks
        assert_eq!(by_user(true), [3, 0, 2, 4, 1]); // b (newest first), a, blanks
        let mut t: Vec<usize> = (0..rows.len()).collect();
        sort_rows(&mut t, &rows, Sort { col: 0, desc: true });
        assert_eq!(t, [4, 3, 2, 1, 0]);
    }

    #[test]
    fn devices_sort_by_number() {
        let mut v = vec!["Tank 10", "Tank 2", "Tank 1 REGULAR"];
        v.sort_by_key(|s| natural_key(s));
        assert_eq!(v, ["Tank 1 REGULAR", "Tank 2", "Tank 10"]);
    }
}
