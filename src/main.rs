//! TLS-450 Plus Snapshot Audit Log Tool: open a TLS-450 Plus snapshot .tar,
//! pick a time range and the kinds of event to show (audit trail, alarms, leak
//! tests, deliveries, the console's logs, ...), filter and search them, and
//! save or copy what is shown (or selected) as plain text.

#![windows_subsystem = "windows"]

mod kind;
#[cfg(feature = "review-shots")]
mod shots;
mod snapshot;
mod update;

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Align, Color32, Key, Label, Layout, Modifiers, RichText, TextEdit};
use egui_extras::{Column, TableBuilder};
use kind::{GROUPS, KindSet};
use snapshot::{Kind, Loaded, Row, parse_local};

const APP_NAME: &str = "TLS-450 Plus Snapshot Audit Log Tool";
/// The one version number: Cargo.toml. build.rs stamps the same value into the
/// exe's file properties.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

const RED: Color32 = Color32::from_rgb(235, 90, 90);
const AMBER: Color32 = Color32::from_rgb(235, 175, 70);
const BLUE: Color32 = Color32::from_rgb(110, 170, 240);
const GREEN: Color32 = Color32::from_rgb(110, 200, 120);

/// Width of the toolbar row labels (Time / Filter), so the rows line up.
const ROW_LABEL_W: f32 = 46.0;
/// Below this window width the Show panel folds away on its own.
const NARROW: f32 = 1000.0;
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
    /// The kinds of event ticked under Show, by `Kind::index`.
    kinds: KindSet,
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
            kinds: kind::defaults(),
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

/// The empty table's buttons.
enum EmptyAction {
    Defaults,
    All,
    Matching,
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
    /// Text typed in the User and Device lists to narrow them.
    user_find: String,
    device_find: String,
    /// Rows of each kind in the snapshot, and of those, the ones the time
    /// range, User, Device and Search let through (by `Kind::index`).
    totals: [usize; Kind::COUNT],
    counts: [usize; Kind::COUNT],
    /// Log Management rows that repeat an audit row (hidden while audit is shown).
    repeats: usize,
    /// The Show panel is open (it can be folded away on a narrow window).
    show_panel: bool,
    /// The panel was folded because the window got narrow (it opens again
    /// when the window is wide, unless the user changed it meanwhile).
    auto_folded: bool,
    /// Window width last frame, to notice it crossing `NARROW`.
    last_width: Option<f32>,
    /// The filters before "Show all events within 15 min", for "Back".
    before_around: Option<Box<Filters>>,
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
    update: UpdateState,
    /// The "What's new" window (release notes of the offered update) is open.
    show_notes: bool,
    #[cfg(feature = "review-shots")]
    shots: Option<shots::Shots>,
}

/// The update check and install, shown as a strip above the toolbar.
enum UpdateState {
    Idle,
    /// `manual`: started from the button, so "up to date" and errors are reported.
    Checking {
        manual: bool,
        rx: Receiver<Result<update::Check, String>>,
    },
    /// A newer version: the strip offers it until Later.
    Available(update::Release),
    Downloading {
        release: update::Release,
        rx: Receiver<Download>,
        cancel: Arc<AtomicBool>,
        done: u64,
        total: u64,
    },
    /// The update did not go through; the installed program is unchanged.
    Failed {
        release: update::Release,
        error: String,
    },
}

enum Download {
    Progress(u64, u64),
    Done(Result<PathBuf, String>),
}

impl App {
    fn new(arg: Option<PathBuf>) -> App {
        let mut app = App {
            data: None,
            hay: vec![],
            users: vec![],
            devices: vec![],
            user_find: String::new(),
            device_find: String::new(),
            totals: [0; Kind::COUNT],
            counts: [0; Kind::COUNT],
            repeats: 0,
            show_panel: true,
            auto_folded: false,
            last_width: None,
            before_around: None,
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
            update: UpdateState::Idle,
            show_notes: false,
            #[cfg(feature = "review-shots")]
            shots: shots::Shots::from_env(),
        };
        if let Ok(exe) = std::env::current_exe() {
            update::clean_up_old(&exe);
        }
        // A quiet check at start; screenshot runs stay offline.
        #[cfg(feature = "review-shots")]
        let check_now = app.shots.is_none();
        #[cfg(not(feature = "review-shots"))]
        let check_now = true;
        if check_now {
            app.start_check(false);
        }
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

    fn start_check(&mut self, manual: bool) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(update::check(APP_VERSION));
        });
        self.update = UpdateState::Checking { manual, rx };
    }

    fn update_busy(&self) -> bool {
        matches!(self.update, UpdateState::Checking { .. } | UpdateState::Downloading { .. })
    }

    /// The user said yes: check the folder can be written, then download on a thread.
    fn start_download(&mut self, release: update::Release) {
        let Some(asset) = release.asset.clone() else { return };
        let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
        let Some(dir) = dir.filter(|d| update::can_write_to(d)) else {
            self.update = UpdateState::Failed {
                release,
                error: "this program's folder can't be written to, so it can't replace itself. \
                        Download the new version from the releases page instead."
                    .into(),
            };
            return;
        };
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let progress = |done, total| {
                let _ = tx.send(Download::Progress(done, total));
            };
            let r = update::download(&asset, APP_VERSION, &dir, &progress, &stop);
            let _ = tx.send(Download::Done(r));
        });
        let total = release.asset.as_ref().map_or(0, |a| a.size);
        self.update = UpdateState::Downloading {
            release,
            rx,
            cancel,
            done: 0,
            total,
        };
    }

    /// Takes finished checks and download progress.
    fn poll_update(&mut self, ctx: &egui::Context) {
        match &mut self.update {
            UpdateState::Checking { manual, rx } => {
                let manual = *manual;
                let r = match rx.try_recv() {
                    Ok(r) => r,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => Err("the check stopped unexpectedly".to_string()),
                };
                self.update = UpdateState::Idle;
                match r {
                    Ok(update::Check::Available(rel)) => self.update = UpdateState::Available(rel),
                    Ok(update::Check::UpToDate(_)) if manual => {
                        self.message = Message::Good(format!("You have the latest version ({APP_VERSION})."));
                    }
                    Err(e) if manual => self.message = error(format!("Update check failed: {e}")),
                    _ => {}
                }
            }
            UpdateState::Downloading {
                release, rx, done, total, ..
            } => {
                let mut finished = None;
                loop {
                    match rx.try_recv() {
                        Ok(Download::Progress(d, t)) => (*done, *total) = (d, t),
                        Ok(Download::Done(r)) => finished = Some(r),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            finished.get_or_insert(Err("the download stopped unexpectedly".to_string()));
                            break;
                        }
                    }
                }
                let Some(r) = finished else { return };
                let release = release.clone();
                match r {
                    Err(e) if e == "canceled" => self.update = UpdateState::Available(release),
                    Err(e) => self.update = UpdateState::Failed { release, error: e },
                    Ok(tmp) => self.install(ctx, release, &tmp),
                }
            }
            _ => {}
        }
    }

    /// Swap the verified download in and start it, reopening the same snapshot.
    fn install(&mut self, ctx: &egui::Context, release: update::Release, tmp: &Path) {
        let swapped = std::env::current_exe()
            .map_err(|e| format!("could not find this program ({e})"))
            .and_then(|exe| update::swap_in(&exe, tmp).map(|_| exe));
        let exe = match swapped {
            Ok(exe) => exe,
            Err(e) => {
                let _ = std::fs::remove_file(tmp);
                self.update = UpdateState::Failed { release, error: e };
                return;
            }
        };
        let mut cmd = std::process::Command::new(&exe);
        if let Some(d) = &self.data {
            cmd.arg(&d.path);
        }
        match cmd.spawn() {
            Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => {
                self.update = UpdateState::Idle;
                self.message = error(format!(
                    "Updated to {}, but it could not be started ({e}). Close this window and start the program again.",
                    release.version
                ));
            }
        }
    }

    /// The release notes of the offered update, in a window of their own.
    fn notes_window(&mut self, ctx: &egui::Context) {
        let UpdateState::Available(r) = &self.update else {
            self.show_notes = false;
            return;
        };
        if !self.show_notes {
            return;
        }
        let notes = r.notes.trim();
        let mut open = true;
        egui::Window::new(format!("What's new in {}", r.version))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .default_height(380.0)
            // Below the update strip and toolbar, so the strip's buttons stay in view.
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, 140.0))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 32.0)
                    .show(ui, |ui| {
                        if notes.is_empty() {
                            ui.weak("This release has no notes.");
                        } else {
                            notes_view(ui, notes);
                        }
                    });
                ui.separator();
                ui.hyperlink_to("Open the release page in a browser", update::RELEASES_PAGE);
            });
        if !open {
            self.show_notes = false;
        }
    }

    /// The strip above the toolbar while an update is offered or running.
    fn update_strip(&mut self, ui: &mut egui::Ui) {
        let mut next: Option<UpdateState> = None;
        let mut install: Option<update::Release> = None;
        ui.horizontal_wrapped(|ui| match &self.update {
            UpdateState::Available(r) => {
                ui.label(RichText::new(format!("Version {} is available", r.version)).strong().color(BLUE));
                ui.label(format!("(you have {APP_VERSION})."));
                if r.asset.is_some() {
                    if ui
                        .button("Update and restart")
                        .on_hover_text("Downloads the new version from GitHub, checks it, replaces this program and starts it again")
                        .clicked()
                    {
                        install = Some(r.clone());
                    }
                } else if ui.button("Open download page").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(update::RELEASES_PAGE));
                }
                if ui
                    .selectable_label(self.show_notes, "What's new")
                    .on_hover_text("Show the release notes of this version")
                    .clicked()
                {
                    self.show_notes = !self.show_notes;
                }
                if ui.button("Later").clicked() {
                    self.show_notes = false;
                    next = Some(UpdateState::Idle);
                }
            }
            UpdateState::Downloading {
                release,
                cancel,
                done,
                total,
                ..
            } => {
                ui.spinner();
                let pct = if *total > 0 { done * 100 / total } else { 0 };
                ui.label(format!("Downloading version {}... {pct}%", release.version));
                if ui.button("Cancel").clicked() {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            UpdateState::Failed { release, error } => {
                ui.label(RichText::new(format!("Update to {} failed: {error}", release.version)).color(RED));
                ui.label("This version is unchanged.");
                if ui.button("Open download page").clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(update::RELEASES_PAGE));
                }
                if ui.button("Close").clicked() {
                    next = Some(UpdateState::Idle);
                }
            }
            _ => {}
        });
        if let Some(n) = next {
            self.update = n;
        }
        if let Some(r) = install {
            self.start_download(r);
        }
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
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            // The loader stopped without an answer (it hit a bug): say so
            // rather than showing "Loading..." for ever.
            Err(TryRecvError::Disconnected) => Err("reading it stopped unexpectedly.".to_string()),
        };
        let path = path.clone();
        self.loading = None;
        match result {
            Ok(d) => {
                self.hay = d.rows.iter().map(Row::haystack).collect();
                self.totals = [0; Kind::COUNT];
                for r in &d.rows {
                    self.totals[r.kind.index()] += 1;
                }
                self.repeats = d.rows.iter().filter(|r| r.repeat).count();
                // The kinds ticked carry over to the next snapshot (the ones from
                // before "Show all events within 15 min", if that is on); the rest
                // starts afresh.
                let kinds = match self.before_around.take() {
                    Some(f) => f.kinds,
                    None => self.filters.kinds,
                };
                self.filters = Filters {
                    kinds,
                    ..Filters::default()
                };
                if let (Some(first), Some(last)) = (d.rows.first(), d.rows.last()) {
                    self.filters.from = first.time.clone();
                    self.filters.to = last.time.clone();
                }
                self.message = if kinds == kind::defaults() {
                    Message::None
                } else {
                    let on = kinds.iter().filter(|k| **k).count();
                    Message::Good(format!(
                        "Showing the {on} kinds of event ticked before. Defaults in the Show panel puts them back."
                    ))
                };
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
        let source_on = |r: &Row| kind_shown(&f.kinds, r);

        // User and Device lists follow the tick boxes.
        let mut users: Vec<String> = d
            .rows
            .iter()
            .filter(|r| source_on(r))
            .map(|r| r.user.clone())
            .filter(|u| !u.is_empty())
            .collect();
        users.sort_unstable();
        users.dedup();
        // A choice the ticks no longer cover stays chosen (the list marks it),
        // rather than being dropped without a word.
        let mut devices: Vec<String> = d
            .rows
            .iter()
            .filter(|r| source_on(r))
            .map(|r| r.device.clone())
            .filter(|u| !u.is_empty())
            .collect();
        // Dedup first (cheap), then the natural sort on the few names left.
        devices.sort_unstable();
        devices.dedup();
        devices.sort_by_cached_key(|s| natural_key(s));

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

        let search = Search::parse(&f.search);
        // Every filter but the kinds: what each kind's count in the Show panel counts.
        let others = |i: usize, r: &Row| {
            from.is_none_or(|t| r.utc >= t)
                && to.is_none_or(|t| r.utc <= t)
                && (f.user.is_empty() || r.user == f.user)
                && (f.device.is_empty() || r.device == f.device)
                && search.matches(&self.hay[i])
        };
        let mut counts = [0; Kind::COUNT];
        let mut shown: Vec<usize> = vec![];
        if range_error.is_none() {
            let audit_on = f.kinds[Kind::Audit.index()];
            for (i, r) in d.rows.iter().enumerate() {
                if (r.repeat && audit_on) || !others(i, r) {
                    continue;
                }
                counts[r.kind.index()] += 1;
                if f.kinds[r.kind.index()] {
                    shown.push(i);
                }
            }
        }
        sort_rows(&mut shown, &d.rows, f.sort);

        self.users = users;
        self.devices = devices;
        self.counts = counts;
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
        self.shown_for = Some(self.filters.clone());
    }

    /// Rows that match every filter but sit in kinds not ticked.
    fn hidden_matches(&self) -> usize {
        Kind::ALL
            .iter()
            .filter(|k| !self.filters.kinds[k.index()])
            .map(|k| self.counts[k.index()])
            .sum()
    }

    /// Ticks every kind that has rows matching the other filters.
    fn tick_matching_kinds(&mut self) {
        for k in Kind::ALL {
            if self.counts[k.index()] > 0 {
                self.filters.kinds[k.index()] = true;
            }
        }
    }

    /// The filters as lines, for the top of a saved file and the status bar tip.
    fn filter_lines(&self) -> Vec<String> {
        let f = &self.filters;
        let mut v = vec![];
        let kinds: Vec<&str> = Kind::ALL.iter().filter(|k| f.kinds[k.index()]).map(|k| k.name()).collect();
        v.push(if kinds.len() == Kind::COUNT {
            "Kinds: all".to_string()
        } else {
            format!("Kinds: {}", kinds.join(", "))
        });
        v.push(format!(
            "From: {}  To: {}",
            if f.from.trim().is_empty() { "start" } else { f.from.trim() },
            if f.to.trim().is_empty() { "end" } else { f.to.trim() }
        ));
        if !f.user.is_empty() {
            v.push(format!("User: {}", f.user));
        }
        if !f.device.is_empty() {
            v.push(format!("Device: {}", f.device));
        }
        if !f.search.trim().is_empty() {
            v.push(format!("Search: {}", f.search.trim()));
        }
        v
    }

    /// Back to the filters from before "Show all events within 15 min".
    fn back_from_around(&mut self) {
        if let Some(f) = self.before_around.take() {
            self.filters = *f;
            self.refilter();
            self.scroll_to = self.selected.iter().next().copied();
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
        self.before_around = None;
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
        if self.before_around.is_none() {
            self.before_around = Some(Box::new(self.filters.clone()));
        }
        self.filters.from = d.clock.local(utc - AROUND_SECS);
        self.filters.to = d.clock.local(utc + AROUND_SECS);
        self.filters.preset = Preset::Custom;
        self.filters.kinds = [true; Kind::COUNT];
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
        } else if !self.filters.kinds.contains(&true) {
            Some("Nothing to save: no kind of event is ticked under Show".into())
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
        // What the rows are: the snapshot and the filters that picked them.
        let mut head = String::new();
        for l in d.site.text_lines() {
            head.push_str(&l);
            head.push_str("\r\n");
        }
        head.push_str(&format!("Snapshot: {}\r\n", file_name(&d.path)));
        head.push_str(&format!("Times: console local, {}\r\n", d.clock.zone()));
        for l in self.filter_lines() {
            head.push_str(&l);
            head.push_str("\r\n");
        }
        head.push_str(&format!("Rows: {n} ({})\r\n\r\n", self.output_what()));
        match std::fs::write(&out, head + &to_text(&rows, &self.zone_label())) {
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

/// The store the snapshot is from, on two lines beside the "Store" label: the
/// console's report header (name in bold, then the address), and under it the
/// console's serial number and software, with a Copy button.
/// True when its Copy button was clicked.
fn store_row(ui: &mut egui::Ui, site: &snapshot::Site) -> bool {
    let mut copied = false;
    // Lighter than body text, still readable in both themes.
    let soft = ui.visuals().text_color().gamma_multiply(0.85);
    ui.horizontal_top(|ui| {
        row_label(ui, "Store");
        ui.vertical(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                match site.header.split_first() {
                    None => {
                        ui.label(RichText::new("No store header set up on this console").italics().color(soft));
                    }
                    Some((first, rest)) => {
                        ui.label(RichText::new(first).strong().size(15.0));
                        if !rest.is_empty() {
                            ui.label(rest.join(", "));
                        }
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(site.console()).color(soft));
                if ui
                    .small_button("Copy store info")
                    .on_hover_text("Copies these lines, as at the top of a saved .txt")
                    .clicked()
                {
                    ui.ctx().copy_text(site.text_lines().join("\r\n"));
                    copied = true;
                }
            });
        });
    });
    ui.add_space(6.0);
    copied
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

/// Release notes (GitHub Markdown) as plain text: headings bold, bullets as
/// dots, emphasis and code marks dropped, links shown by their text.
fn notes_view(ui: &mut egui::Ui, notes: &str) {
    for line in notes.lines() {
        let t = line.trim_end();
        let indent = (t.len() - t.trim_start().len()) as f32 * 4.0;
        let t = t.trim_start();
        if t.is_empty() {
            ui.add_space(4.0);
        } else if let Some(h) = t.strip_prefix('#') {
            ui.add_space(4.0);
            ui.label(RichText::new(md_plain(h.trim_start_matches('#').trim())).strong().size(15.0));
        } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label("\u{2022}");
                ui.label(md_plain(b));
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent);
                ui.label(md_plain(t));
            });
        }
    }
}

/// Markdown inline marks removed: `**`, `__`, backticks, and `[text](url)`
/// shown as its text.
fn md_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find("](").and_then(|j| after[j + 2..].find(')').map(|k| (j, j + 2 + k))) {
            Some((j, end)) => {
                out.push_str(&after[..j]);
                rest = &after[end + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.replace("**", "").replace("__", "").replace('`', "")
}

/// The Source tag's color, one per kind (hues close within a group). None:
/// plain text, for the audit rows people read most (and Log Management,
/// which gets an outline).
fn kind_color(k: Kind) -> Option<Color32> {
    let rgb = |r, g, b| Some(Color32::from_rgb(r, g, b));
    match k {
        Kind::Audit | Kind::LogManagement => None,
        Kind::Alarm => Some(AMBER),
        Kind::AlarmPeriod => rgb(215, 140, 90),
        Kind::ManualTest => Some(BLUE),
        Kind::AutoTest => rgb(90, 195, 235),
        Kind::Csld => rgb(145, 140, 240),
        Kind::LineTest => rgb(70, 200, 190),
        Kind::LineDiag => rgb(120, 190, 145),
        Kind::Compliance => rgb(140, 205, 85),
        Kind::Delivery => rgb(180, 140, 240),
        Kind::Inventory => rgb(215, 130, 215),
        Kind::Power => rgb(235, 105, 130),
        Kind::AppLog => rgb(135, 165, 205),
        Kind::AutoEvent => rgb(185, 180, 95),
        Kind::ProgramLog => rgb(160, 160, 160),
        Kind::Isd => rgb(190, 145, 110),
    }
}

/// A tag's fill and text color: tinted with brighter text in dark mode,
/// tinted with much darker text in light mode (readable at small size), solid
/// with black text on a selected row.
fn tag_colors(c: Color32, dark: bool, selected: bool) -> (Color32, Color32) {
    if selected {
        (c, Color32::BLACK)
    } else if dark {
        (c.gamma_multiply(0.18), c.lerp_to_gamma(Color32::WHITE, 0.25))
    } else {
        (c.gamma_multiply(0.22), c.lerp_to_gamma(Color32::BLACK, 0.62))
    }
}

/// A row passes the Show ticks: its kind is ticked, and it is not a Log
/// Management entry repeating an audit row that is shown anyway.
fn kind_shown(kinds: &KindSet, r: &Row) -> bool {
    kinds[r.kind.index()] && !(r.repeat && kinds[Kind::Audit.index()])
}

/// The Search box: words that must all appear, "quoted phrases" that must
/// appear as typed, and -words (or -"phrases") that must not appear. Case is
/// ignored. A lone "-" is an ordinary word.
#[derive(Debug, Default, PartialEq)]
struct Search {
    want: Vec<String>,
    not: Vec<String>,
}

impl Search {
    fn parse(s: &str) -> Search {
        let s = s.to_lowercase();
        let mut out = Search::default();
        let mut chars = s.chars().peekable();
        loop {
            while chars.next_if(|c| c.is_whitespace()).is_some() {}
            let Some(&c) = chars.peek() else { break };
            let mut neg = false;
            if c == '-' {
                chars.next();
                match chars.peek() {
                    None => {
                        out.want.push("-".into());
                        break;
                    }
                    Some(n) if n.is_whitespace() => {
                        out.want.push("-".into());
                        continue;
                    }
                    _ => neg = true,
                }
            }
            let mut term = String::new();
            if chars.next_if_eq(&'"').is_some() {
                // A phrase runs to the closing quote (or the end of the box).
                for c in chars.by_ref() {
                    if c == '"' {
                        break;
                    }
                    term.push(c);
                }
            } else {
                while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
                    term.push(c);
                }
            }
            let term = term.trim().to_string();
            if term.is_empty() {
                continue;
            }
            if neg {
                out.not.push(term);
            } else {
                out.want.push(term);
            }
        }
        out
    }

    fn matches(&self, hay: &str) -> bool {
        self.want.iter().all(|w| hay.contains(w.as_str())) && !self.not.iter().any(|w| hay.contains(w.as_str()))
    }
}

/// A drop-down list with "All" first and a box at the top that narrows the
/// list as you type (Enter picks the first match). The box takes the keyboard
/// once, when the list opens, so Tab and the arrow keys still move on. A
/// choice no longer in `items` (its kinds were unticked) stays chosen and is
/// marked, so it is never dropped without a word.
fn pick_list(
    ui: &mut egui::Ui,
    id: &str,
    width: f32,
    value: &mut String,
    items: &[String],
    find: &mut String,
    noun: &str,
) -> egui::Response {
    let stale = !value.is_empty() && !items.contains(value);
    let text = if value.is_empty() {
        "All".to_string()
    } else if stale {
        format!("{value} (none in kinds shown)")
    } else {
        value.clone()
    };
    let opened_id = egui::Id::new((id, "opened"));
    let r = egui::ComboBox::from_id_salt(id)
        .width(width)
        .height(420.0)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .selected_text(text)
        .show_ui(ui, |ui| {
            let te = ui.add(
                TextEdit::singleline(find)
                    .id(egui::Id::new((id, "find")))
                    .hint_text("Type to narrow the list")
                    .desired_width(width.clamp(160.0, 260.0)),
            );
            let first_frame = !ui.data(|d| d.get_temp::<bool>(opened_id).unwrap_or(false));
            if first_frame {
                ui.data_mut(|d| d.insert_temp(opened_id, true));
                te.request_focus();
            }
            ui.separator();
            let needle = find.trim().to_lowercase();
            let matching: Vec<&String> = items
                .iter()
                .filter(|it| needle.is_empty() || it.to_lowercase().contains(&needle))
                .collect();
            if te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                // An empty box keeps the choice as it is; text picks the first match.
                if !needle.is_empty()
                    && let Some(first) = matching.first()
                {
                    *value = (*first).clone();
                }
                ui.close();
            }
            if ui.selectable_label(value.is_empty(), "All").clicked() {
                value.clear();
                ui.close();
            }
            if stale && ui.selectable_label(true, format!("{value} (none in kinds shown)")).clicked() {
                ui.close();
            }
            for it in &matching {
                if ui.selectable_label(*value == **it, it.as_str()).clicked() {
                    *value = (*it).clone();
                    ui.close();
                }
            }
            if matching.is_empty() {
                ui.weak(if items.is_empty() {
                    format!("No {noun} in the kinds shown")
                } else {
                    "No match".to_string()
                });
            }
        });
    if r.inner.is_none() {
        find.clear();
        ui.data_mut(|d| d.remove::<bool>(opened_id));
    }
    r.response
}

impl App {
    /// The Show panel: a tick box for each kind of event, in groups, with how
    /// many rows of it match the other filters.
    fn show_panel_ui(&mut self, ui: &mut egui::Ui) {
        let has_data = self.data.is_some();
        // Secondary text: lighter than body text but still readable (the theme's
        // own "weak" color is too faint at small sizes).
        let soft = ui.visuals().text_color().gamma_multiply(0.72);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show").strong().size(15.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("Hide")
                    .on_hover_text("Fold this panel away (the Filter row then has a Show button)")
                    .clicked()
                {
                    self.show_panel = false;
                    self.auto_folded = false;
                }
            });
        });
        ui.add_enabled_ui(has_data, |ui| {
            ui.horizontal(|ui| {
                let names: Vec<&str> = Kind::ALL.iter().filter(|k| k.default_on()).map(|k| k.name()).collect();
                if ui
                    .small_button("Defaults")
                    .on_hover_text(format!("Tick only: {}", names.join(", ")))
                    .clicked()
                {
                    self.filters.kinds = kind::defaults();
                }
                if ui.small_button("All").on_hover_text("Tick every kind").clicked() {
                    self.filters.kinds = [true; Kind::COUNT];
                }
                if ui.small_button("None").on_hover_text("Untick every kind").clicked() {
                    self.filters.kinds = [false; Kind::COUNT];
                }
            });
            ui.label(
                RichText::new("Right-click or Alt+click a kind to show only it.")
                    .small()
                    .color(soft),
            );
            if has_data && !self.filters.user.is_empty() {
                ui.label(
                    RichText::new("A User is chosen: only audit trail and log management rows have a user.")
                        .small()
                        .color(AMBER),
                );
            }
            ui.separator();
            let range_bad = self.range_error.is_some();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // Room on the right so the scroll bar never covers the counts.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        right: 12,
                        ..Default::default()
                    })
                    .show(ui, |ui| {
                        for (group, kinds) in GROUPS {
                            ui.add_space(4.0);
                            let all_on = kinds.iter().all(|k| self.filters.kinds[k.index()]);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(group.to_uppercase()).small().strong().color(soft));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    // A small link that says what it does to the group.
                                    let r = ui
                                        .add(egui::Link::new(
                                            RichText::new(if all_on { "Untick all" } else { "Tick all" }).small(),
                                        ))
                                        .on_hover_text(if all_on {
                                            format!("Untick every kind in {group}")
                                        } else {
                                            format!("Tick every kind in {group}")
                                        });
                                    if r.clicked() {
                                        for k in kinds.iter() {
                                            self.filters.kinds[k.index()] = !all_on;
                                        }
                                    }
                                });
                            });
                            for &k in kinds.iter() {
                                self.kind_row(ui, k, has_data, range_bad, soft);
                            }
                        }
                    });
            });
        });
    }

    /// One kind in the Show panel: swatch, tick box, name and count.
    fn kind_row(&mut self, ui: &mut egui::Ui, k: Kind, has_data: bool, range_bad: bool, soft: Color32) {
        let total = self.totals[k.index()];
        let count = self.counts[k.index()];
        let empty = has_data && total == 0;
        let repeats = if k == Kind::LogManagement && self.filters.kinds[Kind::Audit.index()] {
            self.repeats
        } else {
            0
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
            match kind_color(k) {
                Some(c) => {
                    ui.painter().rect_filled(rect, 2.0, c);
                }
                None => {
                    ui.painter()
                        .rect_stroke(rect, 2.0, egui::Stroke::new(1.0, soft), egui::StrokeKind::Inside);
                }
            }
            let mut tip = k.about().to_string();
            if empty {
                tip.push_str("\n\nThis snapshot has none.");
            } else if has_data {
                tip.push_str(&format!(
                    "\n\n{count} of the {total} in this snapshot match the time range, User, Device and Search."
                ));
                if repeats > 0 {
                    tip.push_str(&format!(" The {repeats} that repeat audit trail rows are not counted."));
                }
            }
            tip.push_str("\nRight-click or Alt+click: show only this kind.");
            // A kind the snapshot doesn't have shows an empty, disabled box (its
            // tick is kept for the next snapshot).
            let r = if empty {
                let mut off = false;
                ui.add_enabled(false, egui::Checkbox::new(&mut off, ""));
                ui.add(Label::new(RichText::new(k.name()).color(soft)).selectable(false))
                    .on_hover_text(tip)
            } else {
                let mut on = self.filters.kinds[k.index()];
                let r = ui.checkbox(&mut on, k.name()).on_hover_text(tip);
                if r.clicked() && ui.input(|i| i.modifiers.alt) {
                    self.filters.kinds = [false; Kind::COUNT];
                    self.filters.kinds[k.index()] = true;
                } else if r.changed() {
                    self.filters.kinds[k.index()] = on;
                }
                r
            };
            r.context_menu(|ui| {
                if ui.button(format!("Show only {}", k.name())).clicked() {
                    self.filters.kinds = [false; Kind::COUNT];
                    self.filters.kinds[k.index()] = true;
                    ui.close();
                }
            });
            if has_data {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let on = self.filters.kinds[k.index()] && !empty;
                    let n = if empty {
                        "none".to_string()
                    } else if range_bad {
                        "-".to_string()
                    } else {
                        count.to_string()
                    };
                    let color = if on { ui.visuals().strong_text_color() } else { soft };
                    ui.add(Label::new(RichText::new(n).monospace().color(color)).selectable(false));
                });
            }
        });
    }

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
        // A selection is marked with an outline in the selection color, painted
        // over the button so its size doesn't change; the blue fill is kept for
        // the selected rows themselves.
        let sel_stroke = egui::Stroke::new(1.5, ui.visuals().selection.bg_fill);
        let mark = |ui: &egui::Ui, r: &egui::Response| {
            if picked {
                ui.painter().rect_stroke(r.rect, 3.0, sel_stroke, egui::StrokeKind::Inside);
            }
        };
        if let Some(d) = &self.data
            && store_row(ui, &d.site)
        {
            self.message = Message::Good("Copied the store info".into());
        }
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
            if r.on_hover_text(format!(
                "Copies {what}, as in the .txt (the rows only, without the filter lines at its top)"
            ))
            .clicked()
            {
                self.copy(ctx);
            }
            if picked && ui.button("Clear selection").on_hover_text("Esc").clicked() {
                self.selected.clear();
                self.anchor = None;
                ctx.request_repaint();
            }
            ui.add_enabled_ui(has_data, |ui| {
                if ui
                    .button("Reset filters")
                    .on_hover_text("Every filter back to its start, the Show ticks to Defaults")
                    .clicked()
                {
                    self.reset_filters();
                }
            });
            if self.before_around.is_some()
                && ui
                    .button("Back to previous filters")
                    .on_hover_text(
                        "Undo \"Show all events within 15 min\": the time range, kinds, User, Device and Search from before \
                         (changes made since are dropped)",
                    )
                    .clicked()
            {
                self.back_from_around();
            }
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
                row_label(ui, "Filter");
                if !self.show_panel {
                    let on = self.filters.kinds.iter().filter(|k| **k).count();
                    let hidden = self.hidden_matches();
                    let tip = if hidden > 0 {
                        format!("Open the Show panel to pick the kinds of event. {hidden} more rows match in kinds not ticked.")
                    } else {
                        "Open the Show panel to pick the kinds of event".to_string()
                    };
                    if ui
                        .button(format!("Show: {on} of {} kinds", Kind::COUNT))
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.show_panel = true;
                        self.auto_folded = false;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("User");
                    pick_list(ui, "user", 170.0, &mut self.filters.user, &self.users, &mut self.user_find, "users")
                        .on_hover_text("Only audit trail and log management rows have a user: choosing one hides every other kind");
                });
                ui.horizontal(|ui| {
                    ui.label("Device");
                    pick_list(
                        ui,
                        "device",
                        190.0,
                        &mut self.filters.device,
                        &self.devices,
                        &mut self.device_find,
                        "devices",
                    )
                    .on_hover_text("The devices of the kinds of event ticked under Show");
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Search");
                    let r = ui.add(
                        TextEdit::singleline(&mut self.filters.search)
                            .id(egui::Id::new("search"))
                            .desired_width((ui.available_width() - 40.0).clamp(200.0, 420.0))
                            .hint_text("words, \"a phrase\", -not"),
                    );
                    let r = r.on_hover_text(
                        "Ctrl+F. Every word must appear somewhere in the row. \"Quotes\" match a phrase as typed; \
                         -word or -\"phrase\" leaves out rows that have it. Case is ignored. Esc clears.",
                    );
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
        let mut check_clicked = false;
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
                    let hidden = self.hidden_matches();
                    let mut tip = format!("{} rows in this snapshot.\n{}", d.rows.len(), self.filter_lines().join("\n"));
                    if hidden > 0 {
                        tip.push_str(&format!("\n{hidden} more rows match but are in kinds not ticked under Show."));
                    }
                    let showing = if hidden > 0 && self.range_error.is_none() {
                        format!("Showing {} ({hidden} more in kinds not ticked)", self.shown.len())
                    } else {
                        format!("Showing {}", self.shown.len())
                    };
                    ui.label(showing).on_hover_text(tip);
                    if !self.selected.is_empty() {
                        ui.label(format!("({} selected)", self.selected.len()));
                    }
                    if let Some((col, dir)) = self.sort_parts() {
                        ui.separator();
                        ui.label(RichText::new(format!("Sorted by {col}, {dir}")).strong());
                    }
                    ui.separator();
                    ui.label(format!("Times: {}", d.clock.name)).on_hover_text(if d.clock.fixed {
                        format!(
                            "Console local time: {}. The console's time zone is not one this program knows, so this \
                                 fixed offset (from its status log, or UTC if that has none) is used all year, without daylight saving.",
                            d.clock.zone()
                        )
                    } else {
                        format!("Console local time: {}", d.clock.zone())
                    });
                    // Anything about how the snapshot was read: one item, details on hover.
                    let mut notes: Vec<(String, bool)> = vec![];
                    if !d.missing.is_empty() {
                        notes.push((format!("Not in this snapshot: {}.", d.missing.join(", ")), true));
                    }
                    if !d.unreadable.is_empty() {
                        notes.push((
                            format!(
                                "These tables are in the snapshot but not laid out as expected, so they were not read: {}.",
                                d.unreadable.join(", ")
                            ),
                            true,
                        ));
                    }
                    if d.skipped > 0 {
                        notes.push((
                            format!("{} rows could not be read (a damaged time or value) and were skipped.", d.skipped),
                            true,
                        ));
                    }
                    let no_time: usize = d.no_time.iter().sum();
                    if no_time > 0 {
                        let list: Vec<String> = Kind::ALL
                            .iter()
                            .filter(|k| d.no_time[k.index()] > 0)
                            .map(|k| format!("{} {}", d.no_time[k.index()], k.name()))
                            .collect();
                        notes.push((
                            format!(
                                "{no_time} rows have no date and time (the console saved them that way, for example at power up \
                                 before its clock ran), so they can't be placed in the list: {}.",
                                list.join(", ")
                            ),
                            false,
                        ));
                    }
                    if !notes.is_empty() {
                        let bad = notes.iter().any(|n| n.1);
                        let short = if notes.len() == 1 && !bad {
                            format!("{no_time} without a time")
                        } else {
                            format!("Notices ({})", notes.len())
                        };
                        let t = if bad {
                            RichText::new(short).color(RED)
                        } else {
                            RichText::new(short)
                        };
                        ui.separator();
                        ui.label(t)
                            .on_hover_text(notes.iter().map(|n| n.0.as_str()).collect::<Vec<_>>().join("\n\n"));
                    }
                }
                None if self.loading.is_none() => {
                    ui.label("No snapshot open");
                }
                None => {}
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let busy = self.update_busy();
                let checking = matches!(self.update, UpdateState::Checking { manual: true, .. });
                if ui
                    .add_enabled(!busy, egui::Button::new("Check for updates").small())
                    .on_hover_text(format!("Version {APP_VERSION}. Looks for a newer release on GitHub."))
                    .clicked()
                {
                    check_clicked = true;
                }
                if checking {
                    ui.spinner();
                }
                ui.separator();
                match &self.message {
                    Message::None => {}
                    Message::Good(m) => {
                        ui.add(Label::new(RichText::new(m).color(GREEN)).truncate()).on_hover_text(m);
                        ui.separator();
                    }
                    Message::Error(short, long) => {
                        ui.add(Label::new(RichText::new(short).color(RED)).truncate()).on_hover_text(long);
                        ui.separator();
                    }
                }
            });
        });
        if check_clicked {
            self.message = Message::None;
            self.start_check(true);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "review-shots")]
        shots::drive(self, ui.ctx());

        let ctx = ui.ctx().clone();
        self.poll_loading();
        self.poll_update(&ctx);
        if self.loading.is_some() || self.update_busy() {
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
        // Typing goes to a text box only; a focused tick box or button doesn't count.
        let typing = ctx.egui_wants_keyboard_input();
        if k_esc {
            if focused == Some(egui::Id::new("search")) {
                self.filters.search.clear();
            } else if !typing && !ctx.any_popup_open() {
                self.selected.clear();
                self.anchor = None;
            }
        }
        // Ctrl+A selects every row shown, unless a text box has focus (then it
        // selects that box's text as usual).
        if !typing && self.data.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            self.refilter();
            self.select_all();
        }

        self.refilter();
        if matches!(
            self.update,
            UpdateState::Available(_) | UpdateState::Downloading { .. } | UpdateState::Failed { .. }
        ) {
            egui::Panel::top("update").show(ui, |ui| {
                egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    self.update_strip(ui);
                });
            });
        }
        self.notes_window(&ctx);
        egui::Panel::top("controls").show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)).show(ui, |ui| {
                self.toolbar(ui, &ctx);
            });
        });
        // Changes made in the toolbar this frame are shown this frame.
        self.refilter();
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        // The Show panel folds itself away when the window gets narrow, and comes
        // back when it is wide again (if it was folded that way).
        let width = ctx.content_rect().width();
        let was = self.last_width.unwrap_or(f32::MAX);
        if width < NARROW && was >= NARROW && self.show_panel {
            self.show_panel = false;
            self.auto_folded = true;
        } else if width >= NARROW && was < NARROW && self.auto_folded {
            self.show_panel = true;
            self.auto_folded = false;
        }
        self.last_width = Some(width);
        if self.show_panel {
            egui::Panel::left("show")
                .resizable(true)
                .default_size(236.0)
                .size_range(210.0..=360.0)
                .show(ui, |ui| {
                    ui.add_space(6.0);
                    self.show_panel_ui(ui);
                });
            // Ticks changed in the panel are shown this frame.
            self.refilter();
        }

        let mut reset = false;
        let mut empty_action: Option<EmptyAction> = None;
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
                    } else if !self.filters.kinds.contains(&true) {
                        ui.label(RichText::new("No kind of event is ticked under Show.").size(16.0));
                        ui.add_space(6.0);
                        ui.label("Tick one in the Show panel, or:");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            // Centered: the row's own width is unknown until drawn.
                            ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                            if ui.button("Show the defaults").clicked() {
                                empty_action = Some(EmptyAction::Defaults);
                            }
                            if ui.button("Show every kind").clicked() {
                                empty_action = Some(EmptyAction::All);
                            }
                        });
                    } else {
                        ui.label(RichText::new("No events match the filters.").size(16.0));
                        ui.add_space(6.0);
                        let hidden = self.hidden_matches();
                        if hidden > 0 {
                            ui.label(format!("{hidden} rows match in kinds that are not ticked under Show."));
                            ui.add_space(8.0);
                            if ui.button("Tick those kinds").clicked() {
                                empty_action = Some(EmptyAction::Matching);
                            }
                        } else {
                            ui.label(format!("Filtering by: {}", self.filter_lines().join("; ")));
                        }
                    }
                    ui.add_space(8.0);
                    if ui
                        .button("Reset filters")
                        .on_hover_text("Every filter back to its start, the Show ticks to Defaults")
                        .clicked()
                    {
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
            const MIN_TABLE: f32 = 156.0 + 88.0 + 80.0 + 70.0 + 160.0 + 70.0 + 60.0 + 7.0 * 8.0;
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                ui.set_min_width(MIN_TABLE.max(ui.available_width()));
                // Row hover: a blue-gray that stands apart from the gray stripes
                // (set for the table only; buttons keep their own hover color).
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
                    .column(Column::exact(88.0).clip(true))
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
                                    // Each kind's Source is a small rounded tag on every row:
                                    // a tinted tag with colored text normally, a solid tag
                                    // with dark text on a selected row (readable on the
                                    // selection blue, and the same shape either way).
                                    // AUDIT and LOG MGMT go through the same frame (no fill)
                                    // so every tag lines up.
                                    if i == 1 {
                                        let dark = ui.visuals().dark_mode;
                                        let (fill, text) = match kind_color(r.kind) {
                                            Some(c) => tag_colors(c, dark, is_sel),
                                            None => (Color32::TRANSPARENT, strong),
                                        };
                                        let outline = if r.kind == Kind::LogManagement {
                                            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.fg_stroke.color.gamma_multiply(0.55))
                                        } else {
                                            egui::Stroke::NONE
                                        };
                                        let bold = kind_color(r.kind).is_some();
                                        let mut t = RichText::new(*v).color(text);
                                        if bold {
                                            t = t.strong();
                                        }
                                        egui::Frame::new()
                                            .fill(fill)
                                            .stroke(outline)
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
        match empty_action {
            Some(EmptyAction::Defaults) => self.filters.kinds = kind::defaults(),
            Some(EmptyAction::All) => self.filters.kinds = [true; Kind::COUNT],
            Some(EmptyAction::Matching) => self.tick_matching_kinds(),
            None => {}
        }
        if empty_action.is_some() {
            ctx.request_repaint();
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
    #[test]
    fn release_notes_lose_markdown_marks() {
        assert_eq!(super::md_plain("**Bold** and `code`"), "Bold and code");
        assert_eq!(super::md_plain("See [the page](https://x.y/z) now"), "See the page now");
        assert_eq!(super::md_plain("a [b c"), "a [b c");
        assert_eq!(super::md_plain("[x](u"), "[x](u");
    }

    use super::*;

    fn row(time: &str, user: &str, event: &str, value: &str) -> Row {
        Row {
            utc: 0,
            time: time.into(),
            user: user.into(),
            event: event.into(),
            value: value.into(),
            ..Row::default()
        }
    }

    #[test]
    fn search_words_phrases_and_exclusions() {
        let s = Search::parse(r#"Leak  "tank 1" -alarm -"no result" - "#);
        assert_eq!(s.want, ["leak", "tank 1", "-"]);
        assert_eq!(s.not, ["alarm", "no result"]);
        assert!(s.matches("manual static leak test tank 1 passed -"));
        assert!(!s.matches("manual static leak test tank 1 passed"));
        assert!(!s.matches("leak alarm tank 1 -"));
        assert!(!s.matches("leak tank 1 no result -"));
        // An unclosed quote runs to the end; empty terms are ignored.
        let s = Search::parse(r#"-"" "" "open phrase"#);
        assert_eq!(
            s,
            Search {
                want: vec!["open phrase".into()],
                not: vec![]
            }
        );
        assert!(Search::parse("   ").matches("anything"));
        assert_eq!(Search::parse("a-b -c-d").want, ["a-b"]);
        assert_eq!(Search::parse("a-b -c-d").not, ["c-d"]);
    }

    #[test]
    fn log_management_repeats_hide_only_with_audit() {
        let mut r = row("2026-01-01 00:00:00", "u", "e", "v");
        r.kind = Kind::LogManagement;
        r.repeat = true;
        let mut k = [false; Kind::COUNT];
        k[Kind::LogManagement.index()] = true;
        assert!(kind_shown(&k, &r));
        k[Kind::Audit.index()] = true;
        assert!(!kind_shown(&k, &r));
        r.repeat = false;
        assert!(kind_shown(&k, &r));
        k[Kind::LogManagement.index()] = false;
        assert!(!kind_shown(&k, &r));
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
