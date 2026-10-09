//! Settings, split into tabs by what you're trying to do. Every card is: a title, one plain sentence,
//! then the controls.
//!
//!   Games        several accounts at once, what happens on a crash/disconnect, same-server rejoin
//!   Performance  performance profiles (formerly "presets"), memory saver, CPU saver, what they apply to
//!   Updates      the manager and Roblox
//!   This app     the manager's own footprint
//!   Security     backup, sign out everywhere, add from cookie
use super::theme::*;
use super::widgets::*;
use crate::app::App;
use crate::runtime::mem;
use crate::storage::db::{CpuMode, Preset, Settings, TrimMode};
use eframe::egui::{self, Color32, Margin, RichText, Rounding, Stroke};
use zeroize::Zeroizing;

pub const TABS: [&str; 5] = ["Games", "Performance", "Updates", "This app", "Security"];

const PRIORITY: [&str; 3] = ["Normal", "Lower", "Lowest"];
/// skip_mips 0..=4 as words. Values above 4 (set under Advanced) show as "Lowest".
const TEXTURES: [&str; 5] = ["Full", "Half", "Quarter", "Low", "Lowest"];

/// One line describing a profile, e.g. "60 fps · full textures · shadows · effects · normal priority".
pub fn preset_summary(p: &Preset) -> String {
    let g = &p.gfx;
    let tex = TEXTURES[(g.skip_mips as usize).min(4)].to_lowercase();
    let mut parts = vec![format!("{} fps", g.fps), format!("{tex} textures")];
    parts.push(if g.shadows_off { "no shadows" } else { "shadows" }.into());
    parts.push(if g.post_fx_off { "no effects" } else { "effects" }.into());
    parts.push(format!("{} priority", PRIORITY[p.priority.clamp(0, 2) as usize].to_lowercase()));
    parts.join("  ·  ")
}

fn field(ui: &mut egui::Ui, label: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.label(RichText::new(label).strong());
    if !hint.is_empty() {
        help(ui, hint);
    }
    add(ui);
}

/// The settings a profile controls (everything but its name). Used here and on the account page.
pub fn profile_fields(ui: &mut egui::Ui, p: &mut Preset) -> bool {
    let mut c = false;
    let g = &mut p.gfx;
    field(ui, "Frame rate", "Lower = much less CPU and GPU. 10–15 is plenty for an AFK alt.", |ui| {
        ui.horizontal_wrapped(|ui| {
            c |= ui.add(egui::Slider::new(&mut g.fps, 5..=240).suffix(" fps")).changed();
            for v in [10u32, 15, 30, 60, 144] {
                if ui.selectable_label(g.fps == v, v.to_string()).clicked() {
                    g.fps = v;
                    c = true;
                }
            }
        });
    });
    field(ui, "Texture quality", "Lower = less video memory.", |ui| {
        if let Some(i) = segmented(ui, &TEXTURES, (g.skip_mips as usize).min(4)) {
            g.skip_mips = i as u32;
            c = true;
        }
    });
    field(ui, "Extras", "", |ui| {
        let mut shadows = !g.shadows_off;
        if ui.checkbox(&mut shadows, "Shadows").changed() {
            g.shadows_off = !shadows;
            c = true;
        }
        let mut fx = !g.post_fx_off;
        if ui.checkbox(&mut fx, "Visual effects (bloom, blur, colour grading)").changed() {
            g.post_fx_off = !fx;
            c = true;
        }
    });
    field(ui, "Priority", "How much CPU time Windows gives it compared with your other programs.", |ui| {
        if let Some(i) = segmented(ui, &PRIORITY, p.priority.clamp(0, 2) as usize) {
            p.priority = i as i32;
            c = true;
        }
    });
    let g = &mut p.gfx;
    ui.add_space(4.0);
    egui::CollapsingHeader::new(RichText::new("Advanced").color(WEAK)).id_salt(("adv", p.id)).show(ui, |ui| {
        egui::Grid::new(("adv-grid", p.id)).num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
            ui.label("Exact texture level");
            c |= ui
                .add(egui::Slider::new(&mut g.skip_mips, 0..=8).custom_formatter(|v, _| {
                    if v < 0.5 { "full".into() } else { format!("1/{}", 1u32 << (v as u32)) }
                }))
                .changed();
            ui.end_row();
            ui.label("Frame buffer cap");
            c |= ui
                .add(egui::Slider::new(&mut g.fb_cap, 0..=1024).custom_formatter(|v, _| if v < 0.5 { "not set".into() } else { format!("{v:.0}") }))
                .changed();
            ui.end_row();
        });
        help(ui, "Roblox only honours an allow-list of these settings; anything it doesn't allow is ignored silently.");
    });
    c
}

/// A settings card: title (+ optional status pill on the right), one plain sentence, then controls.
fn section(ui: &mut egui::Ui, title: &str, blurb: &str, status: Option<(&str, Color32)>, add: impl FnOnce(&mut egui::Ui)) {
    card(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(16.0).strong().color(Color32::WHITE));
            if let Some((t, c)) = status {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| pill(ui, t, c));
            }
        });
        if !blurb.is_empty() {
            help(ui, blurb);
        }
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(14.0);
}

/// (level, tile title, tile tag, what it does, what it costs)
const TRIM_LEVELS: [(TrimMode, &str, &str, &str, &str); 4] = [
    (TrimMode::Off, "Off", "No trimming", "Roblox manages its own memory. Each window typically uses 500 MB – 1.5 GB.", "No extra CPU use."),
    (
        TrimMode::Balanced,
        "Balanced",
        "Recommended",
        "Every 15 seconds, windows above your target are trimmed, and Windows is told to take memory from them first if the PC runs low.",
        "Small CPU cost. You may see a short hitch when you switch into a window.",
    ),
    (
        TrimMode::Strong,
        "Strong",
        "Every 5 s",
        "Same as Balanced, but checks every 5 seconds so windows stay close to the target.",
        "More CPU and disk use. Works best with a light performance profile.",
    ),
    (
        TrimMode::Max,
        "Max",
        "Hard limit",
        "Windows are not allowed above the target at all. Lowest RAM, but they can run slowly while limited.",
        "Highest CPU and disk use.",
    ),
];

const CPU_LEVELS: [(CpuMode, &str, &str, &str, &str); 3] = [
    (CpuMode::Off, "Off", "No throttling", "Every Roblox window runs at full speed.", "No change."),
    (
        CpuMode::Efficiency,
        "Efficiency",
        "Recommended",
        "Windows run in Windows Efficiency mode (like Task Manager's leaf icon) at lower priority.",
        "They may run at a lower frame rate. Usually unnoticeable for idle alts.",
    ),
    (
        CpuMode::Strong,
        "Strong",
        "2 cores max",
        "Efficiency mode, lowest priority, and each window is limited to 2 CPU cores.",
        "Busy games can lag or load slowly. Best for alts that just stand AFK.",
    ),
];

pub fn show(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut s = app.engine.settings.read().unwrap().clone();
    let before_multi = s.multi_instance;
    let mut changed = false;

    ui.set_max_width(ui.available_width().min(820.0)); // set_max_width SETS the width; never exceed the panel
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Settings").size(24.0).strong());
        ui.add_space(12.0);
        if let Some(i) = segmented(ui, &TABS, app.settings_tab.min(TABS.len() - 1)) {
            app.settings_tab = i;
        }
    });
    ui.add_space(10.0);

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        match app.settings_tab {
            0 => games(app, ui, &mut s, &mut changed),
            1 => performance(app, ui, &mut s, &mut changed),
            2 => updates(app, ui, ctx, &mut s, &mut changed),
            3 => this_app(app, ui, &mut s, &mut changed),
            _ => security(app, ui),
        }
    });

    if changed {
        if s.multi_instance != before_multi {
            app.engine.set_multi_instance(s.multi_instance);
        }
        *app.engine.settings.write().unwrap() = s.clone();
        let _ = app.db.save_settings(&s);
    }
}

// ============================== Games ==============================
fn games(app: &mut App, ui: &mut egui::Ui, s: &mut Settings, changed: &mut bool) {
    let multi = app.engine.multi_instance_state();
    let status = match multi {
        Some(true) => ("Working", GREEN),
        Some(false) => ("Needs attention", AMBER),
        None => ("Off", GREY),
    };
    section(ui, "Run several accounts at once", "Without this, opening a second Roblox window closes the first one.", Some(status), |ui| {
        *changed |= ui.checkbox(&mut s.multi_instance, "Allow more than one Roblox window").changed();
        if multi == Some(false) {
            ui.label(
                RichText::new(
                    "A Roblox window was already open before this could switch on, so new windows would close it. \
                     Close every Roblox window once; after that, launch as many as you like.",
                )
                .color(AMBER)
                .size(12.5),
            );
            if ui.add(primary("Close all Roblox windows and fix")).clicked() {
                app.engine.close_all_roblox_and_fix();
            }
        }
        *changed |= ui.checkbox(&mut s.kill_on_exit, "Close all Roblox windows when I close the manager").changed();
    });

    section(ui, "When a window crashes or disconnects", "Covers crashes, error pop-ups and the in-game \"Disconnected\" screen.", None, |ui| {
        *changed |= ui.checkbox(&mut s.auto_reconnect, "Reopen it automatically").changed();
        ui.add_enabled_ui(s.auto_reconnect, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(26.0);
                ui.label("Try");
                *changed |= ui.add(egui::DragValue::new(&mut s.max_attempts).range(1..=20)).changed();
                ui.label("times, waiting");
                *changed |= ui.add(egui::DragValue::new(&mut s.delay_secs).range(1..=300)).changed();
                ui.label("seconds between tries");
            });
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                help(ui, "If a window then stays open for a minute, its count starts over.");
            });
        });
        *changed |= ui
            .checkbox(&mut s.log_tail, "Spot the in-game \"Disconnected\" screen (recommended)")
            .on_hover_text("That screen is drawn inside the game, not as a separate window, so the manager reads Roblox's log file to notice it.")
            .changed();
        *changed |= ui
            .checkbox(&mut s.retry_on_auth_fail, "Keep trying if Roblox's servers refuse the launch")
            .on_hover_text("Doesn't apply to expired sign-ins; those always need signing in again.")
            .changed();
    });

    let schema = app.engine.schema.read().unwrap().clone();
    let note = app.engine.probe_note.lock().unwrap().clone();
    let status = if schema.is_some() { ("Verified", GREEN) } else { ("Locked", GREY) };
    section(ui, "Rejoin the same server", "Best-effort: on reconnect, aim for the server you were in.", Some(status), |ui| match schema {
        None => {
            help(
                ui,
                "The manager won't guess Roblox's server-join link; it first needs to see a real one in Roblox's log. \
                 Until then, reconnects start a new server.",
            );
            help(ui, "To verify: on roblox.com open any game > Servers > Join a server. Then press Check.");
            ui.horizontal_wrapped(|ui| {
                if ui.add(subtle("Check Roblox logs")).clicked() {
                    app.engine.probe_rejoin();
                }
                if !note.is_empty() {
                    ui.label(RichText::new(note).color(WEAK).size(12.5));
                }
            });
        }
        Some(sc) => {
            help(ui, &format!("Verified from {} on {}.", sc.source_file, local_time(sc.captured_at, true)));
            *changed |= ui
                .add_enabled(s.auto_reconnect, egui::Checkbox::new(&mut s.same_server_rejoin, "On reconnect, try the previous server first"))
                .changed();
            help(ui, "If that server is full or closed, Roblox picks another. After 2 failed tries the manager stops aiming for it.");
            if ui.link("Forget verification").clicked() {
                app.engine.forget_schema();
                *s = app.engine.settings.read().unwrap().clone();
            }
        }
    });
}

// ============================== Performance ==============================
fn performance(app: &mut App, ui: &mut egui::Ui, s: &mut Settings, changed: &mut bool) {
    section(
        ui,
        "Performance profiles",
        "A profile is the graphics quality an account opens Roblox with. Give your main a nice-looking profile and your \
         alts a light one so they use far less CPU, GPU and memory. Changes apply the next time an account is launched.",
        None,
        |ui| {
            for p in app.presets.clone() {
                profile_card(app, ui, p);
                ui.add_space(8.0);
            }
            if ui.add(subtle("+  New profile")).clicked() {
                let base = app.presets.iter().find(|p| Some(p.id) == app.open_profile).or(app.presets.first()).cloned().unwrap_or_default();
                if let Ok(id) = app.db.new_preset("New profile", &base.gfx, base.priority) {
                    app.open_profile = Some(id);
                }
                app.reload();
            }
        },
    );

    let scope = if s.saver_all_windows { "every Roblox window" } else { "Roblox windows you aren't playing in" };
    section(ui, "Memory saver", &format!("Shrinks the RAM used by {scope}."), None, |ui| {
        let mut idx = TRIM_LEVELS.iter().position(|l| l.0 == s.trim_mode).unwrap_or(0);
        let tiles: Vec<(&str, &str)> = TRIM_LEVELS.iter().map(|l| (l.1, l.2)).collect();
        if choice_tiles(ui, &mut idx, &tiles) {
            s.trim_mode = TRIM_LEVELS[idx].0;
            *changed = true;
        }
        let (_, name, _, body, cost) = TRIM_LEVELS[idx];
        explain(ui, name, body, cost);
        ui.add_enabled_ui(s.trim_mode != TrimMode::Off, |ui| {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Aim for each window to use");
                *changed |= ui.add(egui::Slider::new(&mut s.trim_target_mb, 60..=1000).suffix(" MB").logarithmic(true)).changed();
            });
            help(ui, "100 MB is reachable on a light profile. Heavier games bounce back between checks unless you use Max.");
        });
        let live: Vec<u64> = app.engine.tr().values().filter(|e| e.pid.is_some()).map(|e| e.mem_ws).collect();
        if !live.is_empty() {
            ui.label(
                RichText::new(format!("Right now: {} window(s) using {} in total.", live.len(), mem::mb(live.iter().sum())))
                    .color(WEAK)
                    .size(12.5),
            );
        }
    });

    section(ui, "CPU saver", &format!("Lowers the CPU used by {scope}."), None, |ui| {
        let mut idx = CPU_LEVELS.iter().position(|l| l.0 == s.cpu_saver).unwrap_or(0);
        let tiles: Vec<(&str, &str)> = CPU_LEVELS.iter().map(|l| (l.1, l.2)).collect();
        if choice_tiles(ui, &mut idx, &tiles) {
            s.cpu_saver = CPU_LEVELS[idx].0;
            *changed = true;
        }
        let (_, name, _, body, cost) = CPU_LEVELS[idx];
        explain(ui, name, body, cost);
    });

    section(ui, "Which windows the savers apply to", "Also covers Roblox windows the manager didn't open itself.", None, |ui| {
        *changed |= ui.radio_value(&mut s.saver_all_windows, false, "Only windows I'm not playing in (recommended)").changed();
        help(ui, "The window you click into gets full speed and memory back within a second.");
        *changed |= ui.radio_value(&mut s.saver_all_windows, true, "Every Roblox window, including the one I'm playing").changed();
        if s.saver_all_windows {
            ui.label(
                RichText::new("The game you're playing will also be trimmed and slowed — expect hitches, especially with Max or Strong.")
                    .color(AMBER)
                    .size(12.5),
            );
        }
    });
}

fn profile_card(app: &mut App, ui: &mut egui::Ui, mut p: Preset) {
    let open = app.open_profile == Some(p.id);
    let users: Vec<(i64, String)> = app.accounts.iter().filter(|a| a.preset_id == p.id).map(|a| (a.id, a.label.clone())).collect();
    egui::Frame::none()
        .fill(if open { Color32::from_rgb(26, 31, 40) } else { FIELD })
        .stroke(Stroke::new(1.0, if open { ACCENT.gamma_multiply(0.5) } else { BORDER }))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width(ui.available_width() - 90.0);
                    ui.label(RichText::new(&p.name).size(15.0).strong().color(Color32::WHITE));
                    help(ui, &preset_summary(&p));
                    let used = if users.is_empty() {
                        "Not used by any account".to_string()
                    } else {
                        format!("Used by {}", users.iter().map(|u| u.1.as_str()).collect::<Vec<_>>().join(", "))
                    };
                    ui.label(RichText::new(used).size(12.5).color(if users.is_empty() { GREY } else { ACCENT }));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.add(subtle(if open { "Done" } else { "Edit" })).clicked() {
                        app.open_profile = if open { None } else { Some(p.id) };
                    }
                });
            });
            if !open {
                return;
            }
            ui.add_space(6.0);
            ui.separator();
            let mut save = false;
            field(ui, "Name", "", |ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(ui.available_width().min(260.0)));
                save |= r.changed() && !p.name.trim().is_empty();
            });
            save |= profile_fields(ui, &mut p);
            if save {
                let _ = app.db.save_preset(&p);
                app.reload();
            }
            field(ui, "Accounts using this profile", "Click an account to switch it to this profile.", |ui| {
                ui.horizontal_wrapped(|ui| {
                    for a in app.accounts.clone() {
                        let on = a.preset_id == p.id;
                        if ui.selectable_label(on, &a.label).clicked() && !on {
                            let _ = app.db.set_preset(a.id, p.id);
                            app.reload();
                        }
                    }
                    if app.accounts.is_empty() {
                        help(ui, "No accounts yet.");
                    }
                });
            });
            ui.add_space(6.0);
            if ui.add(subtle("Duplicate")).clicked() {
                if let Ok(id) = app.db.new_preset(&format!("{} copy", p.name), &p.gfx, p.priority) {
                    app.open_profile = Some(id);
                }
                app.reload();
            }
        });
}

// ============================== Updates ==============================
fn updates(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context, s: &mut Settings, changed: &mut bool) {
    section(ui, "Account Manager", &format!("You have v{}.", crate::update::VERSION), None, |ui| {
        *changed |= ui.checkbox(&mut s.check_updates_on_start, "Check for a new version when the manager starts").changed();
        ui.add_enabled_ui(s.check_updates_on_start, |ui| {
            *changed |= ui.checkbox(&mut s.auto_install_updates, "Install it automatically (the manager reopens by itself)").changed();
        });
        ui.add_space(4.0);
        super::about::app_update_controls(app, ui, ctx);
    });
    section(
        ui,
        "Roblox",
        "Roblox normally updates itself when it starts. Use this if a launch fails because Roblox is out of date.",
        None,
        |ui| super::about::roblox_update_controls(app, ui),
    );
}

// ============================== This app ==============================
fn this_app(app: &mut App, ui: &mut egui::Ui, s: &mut Settings, changed: &mut bool) {
    section(ui, "Keep the manager light", "How much the manager itself uses.", None, |ui| {
        *changed |= ui.checkbox(&mut s.manager_light, "Keep the manager light in the background (recommended)").changed();
        help(
            ui,
            "When the manager isn't the active window it runs in Efficiency mode. When it's minimized it stops drawing \
             completely (0% GPU) and releases most of its memory. Launching, reconnecting and the savers keep working.",
        );
    });
    section(ui, "Diagnostics", "Live figures for the manager itself (not the Roblox windows).", None, |ui| {
        if let Some(m) = app.mem {
            egui::Grid::new("diag").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("Working set").color(WEAK));
                ui.label(mem::mb(m.working_set));
                ui.end_row();
                ui.label(RichText::new("Private bytes").color(WEAK));
                ui.label(mem::mb(m.private));
                ui.end_row();
                ui.label(RichText::new("Peak working set").color(WEAK));
                ui.label(mem::mb(m.peak_working_set));
                ui.end_row();
                ui.label(RichText::new("Roblox windows tracked").color(WEAK));
                ui.label(app.engine.tr().len().to_string());
                ui.end_row();
                ui.label(RichText::new("Other Roblox windows").color(WEAK));
                ui.label(app.engine.others.lock().unwrap().len().to_string());
                ui.end_row();
            });
        }
        help(ui, "Most of the manager's memory while its window is open is the graphics driver; minimized it drops to a few MB.");
    });
}

// ============================== Security ==============================
fn security(app: &mut App, ui: &mut egui::Ui) {
    section(
        ui,
        "Saved sign-ins",
        "Encrypted with Windows DPAPI for your Windows user only. The database can't be opened on another PC or by another Windows user.",
        None,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add(subtle("Export encrypted backup")).clicked() {
                    app.notice = Some(match app.db.export_backup() {
                        Ok(p) => (true, format!("Saved to {}", p.display())),
                        Err(e) => (false, format!("Export failed: {e}")),
                    });
                }
                if !app.confirm_clear && ui.add(subtle("Sign out all accounts…")).clicked() {
                    app.confirm_clear = true;
                }
            });
            if app.confirm_clear {
                ui.label("Erase every saved sign-in? Accounts stay listed and will need signing in again.");
                ui.horizontal(|ui| {
                    if ui.add(danger("Erase")).clicked() {
                        let _ = app.db.clear_all_cookies();
                        app.confirm_clear = false;
                        app.reload();
                    }
                    if ui.add(subtle("Cancel")).clicked() {
                        app.confirm_clear = false;
                    }
                });
            }
            if let Some((ok, m)) = &app.notice {
                ui.label(RichText::new(m).color(if *ok { GREEN } else { RED }).size(12.5));
            }
        },
    );
    section(ui, "Add an account from a cookie", "For power users. Paste a .ROBLOSECURITY value; it's checked with Roblox, then encrypted.", None, |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.paste_buf)
                    .password(true)
                    .hint_text(".ROBLOSECURITY value")
                    .desired_width(ui.available_width().min(360.0) - 70.0),
            );
            if ui.add_enabled(!app.paste_buf.trim().is_empty(), subtle("Add")).clicked() {
                let c = Zeroizing::new(std::mem::take(&mut app.paste_buf).trim().to_string());
                app.engine.add_cookie(c);
            }
        });
    });
}
