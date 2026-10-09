//! About: version, update controls and the changelog (CHANGES.md, built into the exe).
use super::theme::*;
use super::widgets::*;
use crate::app::App;
use crate::update::{self, Job, REPO, VERSION};
use eframe::egui::{self, Color32, RichText};

const CHANGELOG: &str = include_str!("../../../CHANGES.md");

fn job_line(ui: &mut egui::Ui, j: &Job) {
    let (t, c) = match j {
        Job::Idle => return,
        Job::Working(t) => (t.as_str(), ACCENT),
        Job::Done(t) => (t.as_str(), GREEN),
        Job::Failed(t) => (t.as_str(), RED),
    };
    ui.horizontal_wrapped(|ui| {
        if j.busy() {
            ui.add(egui::Spinner::new().size(14.0));
        }
        ui.label(RichText::new(t).color(c).size(13.0));
    });
}

/// "Check for updates" / "Install" / "Restart now", plus the status line. Used here and in Settings.
pub fn app_update_controls(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let u = app.engine.updates.lock().unwrap().clone();
    let newer = u.app_latest.as_ref().filter(|r| r.is_newer()).cloned();
    ui.horizontal_wrapped(|ui| {
        if u.app_installed {
            if ui
                .add(primary("Restart now"))
                .on_hover_text("Roblox windows stay open; the manager starts watching them again once you relaunch them.")
                .clicked()
            {
                app.restart(ctx);
            }
        } else if let Some(r) = &newer {
            if ui.add_enabled(!u.app.busy(), primary(&format!("Install {}", r.tag))).clicked() {
                app.update_dismissed = None; // show the updater window with progress
                app.engine.install_app_update(true);
            }
        }
        if !u.app_installed && ui.add_enabled(!u.app.busy(), subtle("Check for updates")).clicked() {
            app.update_dismissed = None;
            app.engine.check_app_update(false);
        }
    });
    job_line(ui, &u.app);
    if let Some(r) = newer.filter(|_| !u.app_installed) {
        if !r.notes.is_empty() {
            ui.add_space(4.0);
            egui::Frame::none().fill(FIELD).rounding(10.0).inner_margin(egui::Margin::same(12.0)).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(format!("What's new in {}", r.tag)).strong());
                if !r.published.is_empty() {
                    help(ui, &format!("Released {}", r.published));
                }
                markdown(ui, &r.notes);
            });
        }
    }
}

/// Roblox player: installed vs latest, "Check" and "Update Roblox".
pub fn roblox_update_controls(app: &mut App, ui: &mut egui::Ui) {
    let u = app.engine.updates.lock().unwrap().clone();
    let running = app.engine.tr().values().filter(|t| t.pid.is_some()).count();
    egui::Grid::new("roblox-ver").num_columns(2).spacing([20.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Installed").color(WEAK));
        ui.label(RichText::new(u.roblox_installed.clone().unwrap_or_else(|| "—  (press Check)".into())).monospace());
        ui.end_row();
        ui.label(RichText::new("Latest").color(WEAK));
        ui.label(
            RichText::new(match &u.roblox_latest {
                Some(l) => format!("{}  ({})", l.upload, l.version),
                None => "—".into(),
            })
            .monospace(),
        );
        ui.end_row();
    });
    let outdated = matches!((&u.roblox_installed, &u.roblox_latest), (Some(h), Some(l)) if *h != l.upload)
        || (u.roblox_installed.is_none() && u.roblox_latest.is_some());
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!u.roblox.busy(), subtle("Check")).clicked() {
            app.engine.check_roblox_update();
        }
        let label = if outdated { "Update Roblox" } else { "Reinstall Roblox" };
        let btn = if outdated { primary(label) } else { subtle(label) };
        if ui
            .add_enabled(!u.roblox.busy() && running == 0, btn)
            .on_hover_text("Downloads Roblox's official installer from Roblox's servers and runs it.")
            .on_disabled_hover_text("Stop every Roblox window first — the installer closes them.")
            .clicked()
        {
            app.engine.update_roblox();
        }
    });
    if running > 0 {
        help(ui, &format!("{running} Roblox window(s) running. Stop them before updating."));
    }
    job_line(ui, &u.roblox);
}

/// Tiny Markdown subset: #/##/### headings, "- " bullets, `code` stripped, blank lines.
pub fn markdown(ui: &mut egui::Ui, text: &str) {
    for line in text.lines() {
        let l = line.trim_end();
        let clean = |s: &str| s.replace('`', "").replace("**", "");
        if let Some(h) = l.strip_prefix("# ") {
            ui.add_space(10.0);
            ui.label(RichText::new(clean(h)).size(19.0).strong().color(Color32::WHITE));
        } else if let Some(h) = l.strip_prefix("## ") {
            ui.add_space(6.0);
            ui.label(RichText::new(clean(h)).size(15.0).strong());
        } else if let Some(h) = l.strip_prefix("### ") {
            ui.label(RichText::new(clean(h)).strong());
        } else if let Some(b) = l.trim_start().strip_prefix("- ") {
            let indent = (l.len() - l.trim_start().len()) as f32 * 6.0;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(4.0 + indent);
                ui.label(RichText::new("•").color(ACCENT));
                ui.label(RichText::new(clean(b)).color(Color32::from_rgb(200, 204, 214)));
            });
        } else if l.is_empty() {
            ui.add_space(2.0);
        } else {
            ui.label(RichText::new(clean(l)).color(Color32::from_rgb(200, 204, 214)));
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(780.0)); // set_max_width SETS the width; never exceed the panel
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Roblox Account Manager").size(24.0).strong().color(Color32::WHITE));
            pill(ui, &format!("v{VERSION}"), ACCENT);
        });
        help(ui, "Run several Roblox accounts side by side, with auto-reconnect and a memory saver.");
        ui.horizontal_wrapped(|ui| {
            if ui.link("GitHub").clicked() {
                update::open_url(format!("https://github.com/{REPO}"));
            }
            ui.label(RichText::new("·").color(WEAK));
            if ui.link("All releases").clicked() {
                update::open_url(format!("https://github.com/{REPO}/releases"));
            }
        });
        ui.add_space(12.0);

        card(ui, "Updates", |ui| {
            help(ui, "New versions are downloaded from this project's GitHub releases.");
            ui.add_space(4.0);
            app_update_controls(app, ui, ctx);
        });
        ui.add_space(14.0);

        card(ui, "Changelog", |ui| {
            markdown(ui, CHANGELOG);
        });
        ui.add_space(20.0);
    });
}
