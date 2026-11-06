//! Dev only (`--features review-shots`): with `TLS450_SHOTS=<folder>` set, the
//! app steps through set UI states and saves a PNG of each, then closes. Used
//! for UI reviews without driving the mouse on someone's desktop. Not in the
//! normal build.

use std::path::PathBuf;

use eframe::egui;

use crate::{App, Message, Preset};

type Setup = fn(&mut App, &egui::Context);

/// (file name, what to set up). Each runs on the state left by the one before.
const STATES: &[(&str, Setup)] = &[
    ("01-loaded", |_, _| {}),
    ("02-last-7-days-search-leak", |a, _| {
        a.quick_range(Preset::Week);
        a.filters.search = "leak".into();
    }),
    ("03-device-tank-1", |a, _| {
        a.filters.search.clear();
        a.refilter();
        a.filters.device = a.devices.iter().find(|d| d.starts_with("Tank 1")).cloned().unwrap_or_default();
    }),
    ("04-automatic-tests-on-oldest-first", |a, _| {
        a.filters.device.clear();
        a.quick_range(Preset::All);
        a.filters.auto_tests = true;
        a.filters.sort = crate::Sort { col: 0, desc: false };
    }),
    ("05-invalid-from-time", |a, _| {
        a.filters.auto_tests = false;
        a.filters.sort = crate::Sort::default();
        a.filters.from = "2026-13-45 25:00".into();
        a.filters.preset = Preset::Custom;
    }),
    ("06-from-after-to", |a, _| {
        a.filters.from = "2026-11-05 12:00".into();
        a.filters.to = "2026-11-04 12:00".into();
    }),
    ("07-no-results", |a, _| {
        a.quick_range(Preset::All);
        a.filters.search = "zzzz".into();
    }),
    ("08-after-save", |a, _| {
        a.filters.search.clear();
        a.message = Message::Good(r"Saved 1046 rows to C:\Snapshots\snapshot-A00000000000000-261105-1558 audit log.txt".into());
    }),
    ("09-open-error", |a, _| {
        a.message = Message::Error(
            "Could not open notes.tar: This file is not a snapshot .tar.".into(),
            "Could not open notes.tar: This file is not a snapshot .tar.\nStill showing snapshot-A00000000000000-261105-1558.tar.".into(),
        );
    }),
    ("10-rows-selected", |a, _| {
        a.message = Message::None;
        a.refilter();
        a.click_row(1, false, false);
        a.click_row(3, true, false);
        a.click_row(4, true, false);
    }),
    ("11-show-around-tank-1-test-start", |a, _| {
        let row = a.data.as_ref().and_then(|d| {
            d.rows
                .iter()
                .position(|r| r.event == "Manual static leak test started" && r.device.starts_with("Tank 1"))
        });
        if let Some(r) = row {
            a.show_around(r);
        }
    }),
    ("11b-select-all-within-15-min", |a, _| {
        let row = a.data.as_ref().and_then(|d| {
            d.rows
                .iter()
                .position(|r| r.event == "Manual static leak test started" && r.device.starts_with("Tank 1"))
        });
        if let Some(r) = row {
            a.select_around(r);
        }
    }),
    ("11c-sorted-by-user", |a, _| {
        a.selected.clear();
        a.quick_range(Preset::Week);
        a.filters.sort = crate::Sort { col: 3, desc: false };
    }),
    ("12-narrow-window", |a, ctx| {
        a.filters.sort = crate::Sort::default();
        a.selected.clear();
        a.quick_range(Preset::All);
        a.message = Message::None;
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(860.0, 560.0)));
    }),
    ("12b-light-mode-selected", |a, ctx| {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1280.0, 780.0)));
        ctx.set_theme(egui::Theme::Light);
        a.refilter();
        a.click_row(0, false, false);
        a.click_row(5, true, false);
    }),
    ("13-no-snapshot-open", |a, ctx| {
        ctx.set_theme(egui::Theme::Dark);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1280.0, 780.0)));
        a.data = None;
        a.shown.clear();
        a.shown_for = None;
        a.filters = Default::default();
        a.message = Message::None;
    }),
];

pub struct Shots {
    dir: PathBuf,
    index: usize,
    frames: u32,
    waiting: bool,
}

impl Shots {
    pub fn from_env() -> Option<Shots> {
        let dir = PathBuf::from(std::env::var_os("TLS450_SHOTS")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Shots {
            dir,
            index: 0,
            frames: 0,
            waiting: false,
        })
    }
}

/// Called at the start of every frame.
pub fn drive(app: &mut App, ctx: &egui::Context) {
    let Some(mut s) = app.shots.take() else { return };
    ctx.request_repaint();
    // Wait for the snapshot to finish loading before the first capture.
    if app.loading.is_some() || (s.index == 0 && app.data.is_none()) {
        app.shots = Some(s);
        return;
    }

    // A screenshot that arrived: save it and move on.
    let shot = ctx.input(|i| {
        i.raw.events.iter().find_map(|e| match e {
            egui::Event::Screenshot { image, .. } => Some(image.clone()),
            _ => None,
        })
    });
    if let Some(img) = shot {
        let path = s.dir.join(format!("{}.png", STATES[s.index].0));
        save_png(&path, &img);
        s.index += 1;
        s.frames = 0;
        s.waiting = false;
    }
    if s.index >= STATES.len() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        return;
    }
    if !s.waiting {
        if s.frames == 0 {
            (STATES[s.index].1)(app, ctx);
        }
        s.frames += 1;
        // Let layout, resizes and the filter settle before the capture.
        if s.frames > 12 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            s.waiting = true;
        }
    }
    app.shots = Some(s);
}

fn save_png(path: &std::path::Path, img: &egui::ColorImage) {
    let Ok(f) = std::fs::File::create(path) else { return };
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.size[0] as u32, img.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let Ok(mut w) = enc.write_header() else { return };
    let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
    let _ = w.write_image_data(&bytes);
}
