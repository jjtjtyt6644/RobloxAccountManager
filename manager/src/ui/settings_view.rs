//! Settings, grouped by what the user is trying to do, each with a one-line explanation.
use super::theme::*;
use super::widgets::*;
use crate::app::App;
use crate::runtime::mem;
use crate::storage::db::{Gfx, Preset, TrimMode};
use eframe::egui::{self, RichText};
use zeroize::Zeroizing;

const PRIORITY: [&str; 3] = ["Normal", "Below normal", "Idle"];

pub fn preset_summary(p: &Preset) -> String {
    let g = &p.gfx;
    let mut parts = vec![format!("{} fps cap", g.fps)];
    if g.skip_mips > 0 {
        parts.push(format!("textures ÷{}", 1u32 << g.skip_mips.min(8)));
    }
    if g.shadows_off {
        parts.push("no shadows".into());
    }
    if g.post_fx_off {
        parts.push("no post-effects".into());
    }
    parts.push(format!("{} CPU priority", PRIORITY[p.priority.clamp(0, 2) as usize].to_lowercase()));
    parts.join("  ·  ")
}

pub fn preset_editor(ui: &mut egui::Ui, g: &mut Gfx) -> bool {
    let mut c = false;
    egui::Grid::new("preset-editor").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label("Frame-rate cap");
        c |= ui.add(egui::Slider::new(&mut g.fps, 5..=240).suffix(" fps")).changed();
        ui.end_row();
        ui.label("Texture quality");
        c |= ui
            .add(egui::Slider::new(&mut g.skip_mips, 0..=8).custom_formatter(|v, _| {
                if v < 0.5 {
                    "full".into()
                } else {
                    format!("1/{}", 1u32 << (v as u32))
                }
            }))
            .on_hover_text("Lower = less video memory. 1/4 is a good saving for background accounts.")
            .changed();
        ui.end_row();
        ui.label("Shadows");
        c |= ui.checkbox(&mut g.shadows_off, "Turn off").changed();
        ui.end_row();
        ui.label("Post-processing");
        c |= ui.checkbox(&mut g.post_fx_off, "Turn off (bloom, blur, colour effects)").changed();
        ui.end_row();
        ui.label("Frame buffer cap");
        c |= ui
            .add(egui::Slider::new(&mut g.fb_cap, 0..=1024).custom_formatter(|v, _| if v < 0.5 { "not set".into() } else { format!("{v:.0}") }))
            .changed();
        ui.end_row();
    });
    help(ui, "Roblox only honours an allow-list of these flags; anything it doesn't allow is ignored silently.");
    c
}

fn section(ui: &mut egui::Ui, title: &str, blurb: &str, add: impl FnOnce(&mut egui::Ui)) {
    section_with(ui, title, blurb, None, add);
}

/// A settings card: title (+ optional status pill on the right), one plain sentence, then controls.
fn section_with(
    ui: &mut egui::Ui,
    title: &str,
    blurb: &str,
    status: Option<(&str, egui::Color32)>,
    add: impl FnOnce(&mut egui::Ui),
) {
    card(ui, "", |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).size(16.0).strong().color(egui::Color32::WHITE));
            if let Some((t, c)) = status {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| pill(ui, t, c));
            }
        });
        help(ui, blurb);
        ui.add_space(6.0);
        add(ui);
    });
    ui.add_space(14.0);
}

/// (level, tile title, tile tag, what it does, what it costs)
const TRIM_LEVELS: [(TrimMode, &str, &str, &str, &str); 4] = [
    (
        TrimMode::Off,
        "Off",
        "No trimming",
        "Roblox manages its own memory. Each window typically uses 500 MB – 1.5 GB.",
        "No extra CPU use.",
    ),
    (
        TrimMode::Balanced,
        "Balanced",
        "Recommended",
        "Every 15 seconds, background windows above your target are trimmed, and Windows is told to take \
         memory from them first if the PC runs low.",
        "Small CPU cost. You may see a short hitch when you switch into a window.",
    ),
    (
        TrimMode::Strong,
        "Strong",
        "Checks every 5 s",
        "Same as Balanced, but checks every 5 seconds so background windows stay close to the target.",
        "More CPU and disk use. Works best with a light performance preset.",
    ),
    (
        TrimMode::Max,
        "Max",
        "Hard limit",
        "Background windows are not allowed above the target at all. Lowest RAM, but they can run slowly \
         while limited.",
        "Highest CPU and disk use. The limit lifts the moment you click into the window.",
    ),
];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let mut s = app.engine.settings.read().unwrap().clone();
    let before_multi = s.multi_instance;
    let mut changed = false;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(780.0);
        ui.label(RichText::new("Settings").size(24.0).strong());
        ui.add_space(8.0);

        // ---------------- multiple clients ----------------
        let multi = app.engine.multi_instance_state();
        let status = match multi {
            Some(true) => ("Working", GREEN),
            Some(false) => ("Needs attention", AMBER),
            None => ("Off", GREY),
        };
        section_with(
            ui,
            "Run several accounts at once",
            "Without this, opening a second Roblox window closes the first one.",
            Some(status),
            |ui| {
                changed |= ui.checkbox(&mut s.multi_instance, "Allow more than one Roblox window").changed();
                if multi == Some(false) {
                    ui.label(
                        RichText::new(
                            "A Roblox window was already open before this could switch on, so new windows would \
                             close it. Close every Roblox window once; after that, launch as many as you like.",
                        )
                        .color(AMBER)
                        .size(12.5),
                    );
                    if ui.add(primary("Close all Roblox windows and fix")).clicked() {
                        app.engine.close_all_roblox_and_fix();
                    }
                }
                changed |= ui.checkbox(&mut s.kill_on_exit, "Close all Roblox windows when I close the manager").changed();
            },
        );

        // ---------------- reconnect ----------------
        section(
            ui,
            "When a window crashes or disconnects",
            "Covers crashes, error pop-ups and the in-game \"Disconnected\" screen.",
            |ui| {
                changed |= ui.checkbox(&mut s.auto_reconnect, "Reopen it automatically").changed();
                ui.add_enabled_ui(s.auto_reconnect, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(26.0);
                        ui.label("Try");
                        changed |= ui.add(egui::DragValue::new(&mut s.max_attempts).range(1..=20)).changed();
                        ui.label("times, waiting");
                        changed |= ui.add(egui::DragValue::new(&mut s.delay_secs).range(1..=300)).changed();
                        ui.label("seconds between tries");
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(26.0);
                        help(ui, "If a window then stays open for a minute, its count starts over.");
                    });
                });
                changed |= ui
                    .checkbox(&mut s.log_tail, "Spot the in-game \"Disconnected\" screen (recommended)")
                    .on_hover_text(
                        "That screen is drawn inside the game, not as a separate window, so the manager reads \
                         Roblox's log file to notice it.",
                    )
                    .changed();
                changed |= ui
                    .checkbox(&mut s.retry_on_auth_fail, "Keep trying if Roblox's servers refuse the launch")
                    .on_hover_text("Doesn't apply to expired sign-ins; those always need signing in again.")
                    .changed();

                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Rejoin the same server").strong());
                    pill(ui, "best-effort", AMBER);
                });
                let schema = app.engine.schema.read().unwrap().clone();
                let note = app.engine.probe_note.lock().unwrap().clone();
                match schema {
                    None => {
                        help(
                            ui,
                            "Locked until verified. The manager won't guess Roblox's server-join link format; it first \
                             needs to see a real one in Roblox's log. Reconnects start a new server until then.",
                        );
                        help(ui, "To verify: on roblox.com open any game → Servers → Join a server. Then press Check.");
                        ui.horizontal(|ui| {
                            if ui.add(subtle("Check Roblox logs")).clicked() {
                                app.engine.probe_rejoin();
                            }
                            if !note.is_empty() {
                                ui.label(RichText::new(note).color(WEAK).size(12.5));
                            }
                        });
                    }
                    Some(sc) => {
                        ui.label(
                            RichText::new(format!(
                                "✔  Verified from {} on {} (launchtime in {}).",
                                sc.source_file,
                                local_time(sc.captured_at, true),
                                if sc.launchtime_ms { "ms" } else { "s" }
                            ))
                            .color(GREEN)
                            .size(12.5),
                        );
                        changed |= ui
                            .add_enabled(
                                s.auto_reconnect,
                                egui::Checkbox::new(&mut s.same_server_rejoin, "On reconnect, try the previous server first"),
                            )
                            .changed();
                        help(
                            ui,
                            "Best-effort: if that server is full or has closed, Roblox puts you in another one. After 2 \
                             failed tries the manager stops aiming for it. Fresh launches always pick a new server.",
                        );
                        if ui.link("Forget verification").clicked() {
                            app.engine.forget_schema();
                            s = app.engine.settings.read().unwrap().clone();
                        }
                    }
                }
            },
        );

        // ---------------- presets ----------------
        section(
            ui,
            "Performance presets",
            "Graphics settings each account opens with. Give alts a light preset so they use less memory and CPU.",
            |ui| {
                let names: Vec<(i64, String)> = app.presets.iter().map(|p| (p.id, p.name.clone())).collect();
                let cur = names.iter().find(|x| x.0 == app.sel_preset).map(|x| x.1.clone()).unwrap_or_default();
                form_row(ui, "Preset", |ui| {
                    egui::ComboBox::from_id_salt("preset").selected_text(cur).width(200.0).show_ui(ui, |ui| {
                        for (pid, n) in &names {
                            ui.selectable_value(&mut app.sel_preset, *pid, n);
                        }
                    });
                });
                if let Some(mut p) = app.presets.iter().find(|p| p.id == app.sel_preset).cloned() {
                    let used = app.accounts.iter().filter(|a| a.preset_id == p.id).count();
                    help(ui, &format!("Used by {used} account(s)."));
                    form_row(ui, "Name", |ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(200.0));
                        if r.changed() && !p.name.trim().is_empty() {
                            let _ = app.db.save_preset(&p);
                        }
                        if r.lost_focus() {
                            app.reload();
                        }
                    });
                    form_row(ui, "CPU priority", |ui| {
                        let mut sel = p.priority.clamp(0, 2);
                        egui::ComboBox::from_id_salt(("prio", p.id))
                            .selected_text(PRIORITY[sel as usize])
                            .width(200.0)
                            .show_ui(ui, |ui| {
                                for (i, l) in PRIORITY.iter().enumerate() {
                                    ui.selectable_value(&mut sel, i as i32, *l);
                                }
                            });
                        if sel != p.priority {
                            p.priority = sel;
                            let _ = app.db.save_preset(&p);
                            app.reload();
                        }
                    });
                    if preset_editor(ui, &mut p.gfx) {
                        let _ = app.db.save_preset(&p);
                        app.reload();
                    }
                    if ui.add(subtle("Duplicate as new preset")).clicked() {
                        let name = format!("{} copy", p.name);
                        if let Ok(id) = app.db.new_preset(&name, &p.gfx, p.priority) {
                            app.sel_preset = id;
                        }
                        app.reload();
                    }
                }
            },
        );

        // ---------------- memory ----------------
        section(
            ui,
            "Memory saver",
            "Shrinks the RAM used by Roblox windows you aren't playing in. The window you're playing in is never touched.",
            |ui| {
                let mut idx = TRIM_LEVELS.iter().position(|l| l.0 == s.trim_mode).unwrap_or(0);
                let tiles: Vec<(&str, &str)> = TRIM_LEVELS.iter().map(|l| (l.1, l.2)).collect();
                if choice_tiles(ui, &mut idx, &tiles) {
                    s.trim_mode = TRIM_LEVELS[idx].0;
                    changed = true;
                }
                let (_, name, _, body, cost) = TRIM_LEVELS[idx];
                explain(ui, name, body, cost);
                ui.add_enabled_ui(s.trim_mode != TrimMode::Off, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Aim for each background window to use");
                        changed |= ui
                            .add(egui::Slider::new(&mut s.trim_target_mb, 60..=1000).suffix(" MB").logarithmic(true))
                            .changed();
                    });
                    help(
                        ui,
                        "100 MB is reachable for alts on a light preset (Alt-Low / Alt-Minimal). Heavier games bounce \
                         back between checks unless you use Max.",
                    );
                });
                let live: Vec<u64> = app.engine.tr().values().filter(|e| e.pid.is_some()).map(|e| e.mem_ws).collect();
                if !live.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(format!(
                            "Right now: {} Roblox window(s) using {} in total.",
                            live.len(),
                            mem::mb(live.iter().sum())
                        ))
                        .color(WEAK)
                        .size(12.5),
                    );
                }
            },
        );

        // ---------------- security ----------------
        section(
            ui,
            "Security & backup",
            "Sign-ins are encrypted with Windows DPAPI for your Windows user only. The database can't be opened on \
             another PC or by another Windows user.",
            |ui| {
                ui.horizontal(|ui| {
                    if ui.add(subtle("Export encrypted backup")).clicked() {
                        app.notice = Some(match app.db.export_backup() {
                            Ok(p) => (true, format!("Saved to {}", p.display())),
                            Err(e) => (false, format!("Export failed: {e}")),
                        });
                    }
                    if !app.confirm_clear {
                        if ui.add(subtle("Sign out all accounts…")).clicked() {
                            app.confirm_clear = true;
                        }
                    }
                });
                if app.confirm_clear {
                    ui.horizontal(|ui| {
                        ui.label("Erase every saved sign-in? Accounts stay listed and will need signing in again.");
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

        // ---------------- advanced ----------------
        section(
            ui,
            "Add an account from a cookie (advanced)",
            "For power users. Paste a .ROBLOSECURITY value; it's checked with Roblox, then encrypted.",
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.paste_buf)
                            .password(true)
                            .hint_text(".ROBLOSECURITY value")
                            .desired_width(360.0),
                    );
                    if ui.add_enabled(!app.paste_buf.trim().is_empty(), subtle("Add")).clicked() {
                        let c = Zeroizing::new(std::mem::take(&mut app.paste_buf).trim().to_string());
                        app.engine.add_cookie(c);
                    }
                });
            },
        );

        // ---------------- diagnostics ----------------
        section(ui, "Diagnostics", "Live figures for the manager itself (not the Roblox windows).", |ui| {
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
                    ui.label(RichText::new("Clients tracked").color(WEAK));
                    ui.label(app.engine.tr().len().to_string());
                    ui.end_row();
                });
            }
            help(ui, "A 5-minute sample is written to Activity after each start.");
        });
    });

    if changed {
        if s.multi_instance != before_multi {
            app.engine.set_multi_instance(s.multi_instance);
        }
        *app.engine.settings.write().unwrap() = s.clone();
        let _ = app.db.save_settings(&s);
    }
}
