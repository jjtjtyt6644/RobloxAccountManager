//! Accounts screen: searchable list with one-click Play/Stop and bulk actions on the left, the
//! selected account's settings as cards on the right. First-run shows a guided empty state.
use super::settings_view::{preset_summary, profile_fields};
use super::theme::*;
use super::widgets::*;
use crate::app::{App, GroupFilter, Nav};
use crate::runtime::{mem, trim::MemNote};
use crate::storage::db::Account;
use eframe::egui::{self, text::LayoutJob, Align, FontId, Layout, Margin, RichText, TextFormat};
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

/// Accepts "920587237", "https://www.roblox.com/games/920587237/Name", or a URL with placeId=…
pub fn parse_place(input: &str) -> Result<String, &'static str> {
    let s = input.trim();
    let digits_after = |marker: &str| -> Option<String> {
        let lower = s.to_ascii_lowercase();
        let i = lower.find(marker)? + marker.len();
        let d: String = s[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
        (!d.is_empty()).then_some(d)
    };
    let id = if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        Some(s.to_string())
    } else {
        digits_after("/games/").or_else(|| digits_after("placeid="))
    };
    match id {
        Some(d) if d.len() <= 19 => Ok(d),
        Some(_) => Err("That number is too long to be a Place ID."),
        None => Err("Couldn't find a Place ID — paste a roblox.com/games/… link or just the number."),
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if app.accounts.is_empty() {
        egui::CentralPanel::default().frame(super::page_frame()).show(ctx, |ui| empty_state(app, ui));
        return;
    }
    egui::SidePanel::left("account-list")
        .resizable(true)
        .default_width(350.0)
        .width_range(250.0..=540.0)
        .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::same(14.0)))
        .show(ctx, |ui| list(app, ui));
    egui::CentralPanel::default().frame(super::page_frame()).show(ctx, |ui| detail(app, ui));
}

fn groups(app: &App) -> Vec<String> {
    let set: BTreeSet<String> = app.accounts.iter().filter(|a| !a.group_tag.is_empty()).map(|a| a.group_tag.clone()).collect();
    set.into_iter().collect()
}

fn list(app: &mut App, ui: &mut egui::Ui) {
    ui.add(
        egui::TextEdit::singleline(&mut app.search)
            .hint_text("🔍  Search name or @username")
            .desired_width(f32::INFINITY),
    );
    let gs = groups(app);
    if !gs.is_empty() {
        let label = match &app.group_filter {
            GroupFilter::All => "All groups".to_string(),
            GroupFilter::Ungrouped => "No group".to_string(),
            GroupFilter::Named(g) => g.clone(),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new("Show").color(WEAK));
            egui::ComboBox::from_id_salt("group-filter").selected_text(label).width(ui.available_width() - 8.0).show_ui(
                ui,
                |ui| {
                    ui.selectable_value(&mut app.group_filter, GroupFilter::All, "All groups");
                    for g in &gs {
                        ui.selectable_value(&mut app.group_filter, GroupFilter::Named(g.clone()), g);
                    }
                    ui.selectable_value(&mut app.group_filter, GroupFilter::Ungrouped, "No group");
                },
            );
        });
    }

    let q = app.search.trim().to_lowercase();
    let shown: Vec<Account> = app
        .accounts
        .iter()
        .filter(|a| match &app.group_filter {
            GroupFilter::All => true,
            GroupFilter::Ungrouped => a.group_tag.is_empty(),
            GroupFilter::Named(g) => &a.group_tag == g,
        })
        .filter(|a| q.is_empty() || a.label.to_lowercase().contains(&q) || a.username.to_lowercase().contains(&q))
        .cloned()
        .collect();

    // ---- bulk actions ----
    ui.horizontal(|ui| {
        let all = !shown.is_empty() && shown.iter().all(|a| app.checked.contains(&a.id));
        let mut v = all;
        if ui.checkbox(&mut v, RichText::new(format!("Select all ({})", shown.len())).color(WEAK)).changed() {
            for a in &shown {
                if v {
                    app.checked.insert(a.id);
                } else {
                    app.checked.remove(&a.id);
                }
            }
        }
    });
    if !app.checked.is_empty() {
        let sel: Vec<&Account> = app.accounts.iter().filter(|a| app.checked.contains(&a.id)).collect();
        let launchable: Vec<i64> = sel
            .iter()
            .filter(|a| !is_running(&a.status) && !a.place_id.is_empty() && a.status != "needs_relogin")
            .map(|a| a.id)
            .collect();
        let stoppable: Vec<i64> = sel.iter().filter(|a| is_running(&a.status)).map(|a| a.id).collect();
        let no_game = sel.iter().filter(|a| a.place_id.is_empty()).count();
        let n_sel = sel.len();
        egui::Frame::none().fill(CARD).rounding(8.0).inner_margin(Margin::same(10.0)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(format!("{n_sel} selected")).strong());
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!launchable.is_empty(), primary(&format!("▶  Launch {}", launchable.len())))
                    .on_hover_text("Clients start one after another; each waits for the previous one's window.")
                    .on_disabled_hover_text("All selected accounts are running, have no game set, or need to sign in again.")
                    .clicked()
                {
                    app.engine.launch_many(launchable.clone());
                }
                if ui.add_enabled(!stoppable.is_empty(), subtle(&format!("⏹  Stop {}", stoppable.len()))).clicked() {
                    app.engine.kill_many(stoppable.clone());
                }
                if ui.add(subtle("Clear")).clicked() {
                    app.checked.clear();
                }
            });
            if no_game > 0 {
                ui.label(RichText::new(format!("{no_game} selected without a game will be skipped.")).color(AMBER).size(12.0));
            }
        });
    }
    ui.add_space(2.0);
    ui.separator();

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if shown.is_empty() {
            ui.add_space(8.0);
            help(ui, "No accounts match this search.");
        }
        ui.spacing_mut().item_spacing.y = 4.0;
        for a in &shown {
            row(app, ui, a);
        }
    });
}

fn row(app: &mut App, ui: &mut egui::Ui, a: &Account) {
    let (color, text) = status_info(&a.status);
    let running = is_running(&a.status);
    let selected = app.selected == Some(a.id);
    let ram = app.engine.tr().get(&a.id).filter(|t| t.pid.is_some() && t.mem_ws > 0).map(|t| (t.mem_ws, t.mem_note));
    let why = if a.place_id.is_empty() {
        Some("Set a game for this account first")
    } else if a.status == "needs_relogin" {
        Some("Sign in again first (+ Add account)")
    } else {
        None
    };

    // The whole row is one clickable card (click = select, double-click = Play). Widgets placed on top
    // of it (checkbox, button) take their own clicks.
    let (rect, bg) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::click());
    let (fill, stroke) = if selected {
        (egui::Color32::from_rgb(27, 34, 48), egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.55)))
    } else if bg.hovered() {
        (CARD, egui::Stroke::new(1.0, BORDER))
    } else {
        (egui::Color32::TRANSPARENT, egui::Stroke::NONE)
    };
    ui.painter().rect(rect, egui::Rounding::same(10.0), fill, stroke);
    if bg.clicked() {
        app.selected = Some(a.id);
    }
    if bg.double_clicked() && !running && why.is_none() {
        app.engine.launch(a.id);
    }

    let mut ui = ui.new_child(
        egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(10.0, 0.0))).layout(Layout::left_to_right(Align::Center)),
    );
    ui.spacing_mut().item_spacing.x = 8.0;
    let mut c = app.checked.contains(&a.id);
    if ui.checkbox(&mut c, "").on_hover_text("Select for bulk Launch / Stop").changed() {
        if c {
            app.checked.insert(a.id);
        } else {
            app.checked.remove(&a.id);
        }
    }
    dot(&mut ui, color);
    // Right-to-left: the button is placed first so it always fits; the memory chip only when there's
    // room; the name gets whatever is left and is cut with "…".
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let narrow = ui.available_width() < 220.0;
        let size = egui::vec2(if narrow { 36.0 } else { 72.0 }, 32.0);
        if running {
            if ui.add(subtle(if narrow { "⏹" } else { "⏹ Stop" }).min_size(size)).on_hover_text("Stop").clicked() {
                app.engine.kill(a.id);
            }
        } else if ui
            .add_enabled(why.is_none(), primary(if narrow { "▶" } else { "▶ Play" }).min_size(size))
            .on_hover_text("Play (or double-click the row)")
            .on_disabled_hover_text(why.unwrap_or(""))
            .clicked()
        {
            app.engine.launch(a.id);
        }
        if let Some((ws, note)) = ram.filter(|_| ui.available_width() > 180.0) {
            let c = if note == MemNote::Focused || ws > 400 * 1024 * 1024 { WEAK } else { GREEN };
            let tip = if note.text().is_empty() { "Memory in use".to_string() } else { format!("Memory in use · {}", note.text()) };
            chip(ui, &format!("{:.0} MB", ws as f64 / 1048576.0), c).on_hover_text(tip);
        }
        let w = ui.available_width().max(30.0);
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            ui.set_width(w);
            ui.add_space(9.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let one_line = |t: String, size: f32, c: egui::Color32| {
                let mut job = LayoutJob::single_section(t, TextFormat { font_id: FontId::proportional(size), color: c, ..Default::default() });
                job.wrap = egui::text::TextWrapping::truncate_at_width(w);
                egui::Label::new(job).selectable(false)
            };
            ui.add(one_line(a.label.clone(), 14.5, if selected { egui::Color32::WHITE } else { TEXT }));
            let mut job = LayoutJob::default();
            job.wrap = egui::text::TextWrapping::truncate_at_width(w);
            let mut sub = format!("@{}", a.username);
            if !a.group_tag.is_empty() {
                sub.push_str(&format!("  ·  {}", a.group_tag));
            }
            job.append(&sub, 0.0, TextFormat { font_id: FontId::proportional(12.0), color: WEAK, ..Default::default() });
            job.append(&format!("  ·  {text}"), 0.0, TextFormat { font_id: FontId::proportional(12.0), color, ..Default::default() });
            ui.add(egui::Label::new(job).selectable(false));
        });
    });
}

fn empty_state(app: &mut App, ui: &mut egui::Ui) {
    let busy = app.engine.login_busy.load(Ordering::SeqCst);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.16).max(20.0));
        ui.label(RichText::new("No accounts yet").size(28.0).strong());
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Add a Roblox account, give it a game, press Play.\nYou sign in on the real roblox.com page in its own window; \
                 the manager keeps only the sign-in, encrypted for your Windows user.",
            )
            .color(WEAK),
        );
        ui.add_space(16.0);
        let label = if busy { "Waiting for sign-in…" } else { "+  Add your first account" };
        if ui.add_enabled(!busy, primary(label).min_size(egui::vec2(260.0, 42.0))).clicked() {
            app.engine.start_login();
        }
        ui.add_space(8.0);
        if ui.link("Already have a .ROBLOSECURITY cookie? Add it in Settings").clicked() {
            app.nav = Nav::Settings;
            app.settings_tab = 4;
        }
        ui.add_space(28.0);
        ui.allocate_ui(egui::vec2(560.0, 0.0), |ui| {
            card(ui, "How it works", |ui| {
                for (n, t) in [
                    ("1", "Add each account with + Add account."),
                    ("2", "Paste a game link (or Place ID) into the account's Game box."),
                    ("3", "Press ▶ Play — or tick several accounts and Launch them together."),
                    ("4", "If a client crashes or disconnects, it's restarted automatically (Settings > Games)."),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(n).color(ACCENT).strong());
                        ui.label(t);
                    });
                }
            });
        });
    });
}

fn load_bufs(app: &mut App, acc: &Account) {
    app.edit.for_id = Some(acc.id);
    app.edit.label = acc.label.clone();
    app.edit.group = acc.group_tag.clone();
    app.edit.place_input = acc.place_id.clone();
    app.edit.place_msg = None;
    app.edit.show_preset = false;
    app.confirm_remove = None;
}

fn detail(app: &mut App, ui: &mut egui::Ui) {
    let Some(acc) = app.selected.and_then(|id| app.accounts.iter().find(|a| a.id == id).cloned()) else {
        help(ui, "Select an account on the left.");
        return;
    };
    if app.edit.for_id != Some(acc.id) {
        load_bufs(app, &acc);
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.set_max_width(ui.available_width().min(780.0)); // set_max_width SETS the width; never exceed the panel
        header(app, ui, &acc);
        ui.add_space(14.0);
        if acc.status == "needs_relogin" {
            card(ui, "", |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("⚠").color(PURPLE).size(18.0));
                    ui.label(format!(
                        "This account's sign-in has expired. Click + Add account and sign in as @{} to refresh it — \
                         its settings are kept.",
                        acc.username
                    ));
                });
                if ui.add(primary("Sign in again")).clicked() {
                    app.engine.start_login();
                }
            });
            ui.add_space(10.0);
        }
        game_card(app, ui, &acc);
        ui.add_space(10.0);
        setup_card(app, ui, &acc);
        ui.add_space(10.0);
        session_card(app, ui, &acc);
        ui.add_space(10.0);
        remove_card(app, ui, &acc);
        ui.add_space(20.0);
    });
}

fn header(app: &mut App, ui: &mut egui::Ui, acc: &Account) {
    let (color, base) = status_info(&acc.status);
    let attempts = app.engine.tr().get(&acc.id).map(|t| t.attempts).unwrap_or(0);
    let max = app.engine.settings.read().unwrap().max_attempts;
    let text: String = if acc.status == "reconnecting" && attempts > 0 {
        format!("Reconnecting… ({attempts} of {max})")
    } else {
        base.to_string()
    };
    // Narrow windows: stack the button under the name instead of squeezing it off the edge.
    let narrow = ui.available_width() < 520.0;
    let title = |ui: &mut egui::Ui| {
        ui.label(RichText::new(&acc.label).size(26.0).strong());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("@{}", acc.username)).color(WEAK));
            pill(ui, &text, color);
        });
    };
    if narrow {
        title(ui);
        ui.add_space(4.0);
        play_stop_big(app, ui, acc, ui.available_width().min(320.0));
    } else {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width(ui.available_width() - 170.0);
                title(ui);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| play_stop_big(app, ui, acc, 150.0));
        });
    }
}

fn play_stop_big(app: &mut App, ui: &mut egui::Ui, acc: &Account, width: f32) {
    let size = egui::vec2(width, 42.0);
    if is_running(&acc.status) {
        if ui.add(danger("⏹  Stop").min_size(size)).clicked() {
            app.engine.kill(acc.id);
        }
    } else {
        let why = if acc.place_id.is_empty() {
            Some("Add a game below first")
        } else if acc.status == "needs_relogin" {
            Some("Sign in again first")
        } else {
            None
        };
        let r = ui.add_enabled(why.is_none(), primary("▶  Play").min_size(size));
        if r.on_disabled_hover_text(why.unwrap_or("")).clicked() {
            app.engine.launch(acc.id);
        }
    }
}

fn game_card(app: &mut App, ui: &mut egui::Ui, acc: &Account) {
    card(ui, "Game", |ui| {
        help(ui, "Paste a roblox.com game link or a Place ID. Play joins this game.");
        let r = ui.add(
            egui::TextEdit::singleline(&mut app.edit.place_input)
                .hint_text("https://www.roblox.com/games/920587237/…   or   920587237")
                .desired_width(f32::INFINITY),
        );
        if r.changed() {
            if app.edit.place_input.trim().is_empty() {
                let _ = app.db.set_place(acc.id, "");
                app.edit.place_msg = None;
                app.reload();
            } else {
                match parse_place(&app.edit.place_input) {
                    Ok(p) => {
                        if p != acc.place_id {
                            let _ = app.db.set_place(acc.id, &p);
                            app.reload();
                        }
                        app.edit.place_msg = Some((true, format!("✔  Saved — Place {p}")));
                    }
                    Err(m) => app.edit.place_msg = Some((false, m.to_string())),
                }
            }
        }
        match &app.edit.place_msg {
            Some((ok, m)) => {
                ui.label(RichText::new(m).color(if *ok { GREEN } else { AMBER }).size(12.5));
            }
            None if acc.place_id.is_empty() => {
                ui.label(RichText::new("No game set yet — Play stays disabled until you add one.").color(AMBER).size(12.5));
            }
            None => {}
        }
    });
}

fn setup_card(app: &mut App, ui: &mut egui::Ui, acc: &Account) {
    let gs = groups(app);
    card(ui, "Setup", |ui| {
        form_row(ui, "Nickname", |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.edit.label).desired_width(ui.available_width().min(300.0)));
            let v = app.edit.label.trim().to_string();
            if r.changed() && !v.is_empty() {
                let _ = app.db.set_label(acc.id, &v); // saved as you type; list re-sorts when you leave the field
            }
            if r.lost_focus() {
                if v.is_empty() {
                    app.edit.label = acc.label.clone();
                }
                app.reload();
            }
        });
        form_row(ui, "Group", |ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut app.edit.group).hint_text("none").desired_width((ui.available_width() - 110.0).clamp(80.0, 190.0)));
            if r.changed() {
                let _ = app.db.set_group(acc.id, app.edit.group.trim());
            }
            if r.lost_focus() {
                app.reload();
            }
            if !gs.is_empty() {
                egui::ComboBox::from_id_salt("pick-group").selected_text("Choose…").width(100.0).show_ui(ui, |ui| {
                    for g in gs.iter().map(String::as_str).chain(std::iter::once("")) {
                        let shown = if g.is_empty() { "(no group)" } else { g };
                        if ui.selectable_label(acc.group_tag == g, shown).clicked() {
                            app.edit.group = g.to_string();
                            let _ = app.db.set_group(acc.id, g);
                            app.reload();
                        }
                    }
                });
            }
        });
        form_row(ui, "Performance profile", |ui| {
            let cur = app.presets.iter().find(|p| p.id == acc.preset_id).map(|p| p.name.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("acc-preset").selected_text(cur).width(190.0).show_ui(ui, |ui| {
                for p in app.presets.clone() {
                    if ui.selectable_label(p.id == acc.preset_id, &p.name).on_hover_text(preset_summary(&p)).clicked() {
                        let _ = app.db.set_preset(acc.id, p.id);
                        app.reload();
                    }
                }
            });
        });
        if let Some(mut p) = app.presets.iter().find(|p| p.id == acc.preset_id).cloned() {
            ui.horizontal(|ui| {
                ui.add_space(158.0);
                help(ui, &preset_summary(&p));
            });
            ui.horizontal(|ui| {
                ui.add_space(158.0);
                let t = if app.edit.show_preset { "Hide profile settings" } else { "Edit this profile…" };
                if ui.link(t).clicked() {
                    app.edit.show_preset = !app.edit.show_preset;
                }
            });
            if app.edit.show_preset {
                ui.add_space(4.0);
                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(Margin::same(12.0)).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    help(ui, &format!("Changes apply to every account using “{}”, from their next launch.", p.name));
                    if profile_fields(ui, &mut p) {
                        let _ = app.db.save_preset(&p);
                        app.reload();
                    }
                });
            }
        }
    });
}

fn session_card(app: &mut App, ui: &mut egui::Ui, acc: &Account) {
    let live = app.engine.tr().get(&acc.id).map(|t| (t.pid, t.attempts, t.live_since_ts, t.mem_ws, t.mem_note));
    let s = app.engine.settings.read().unwrap().clone();
    let verified = app.engine.rejoin_available();
    card(ui, "Session", |ui| {
        egui::Grid::new("session-grid").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            match live {
                Some((pid, attempts, since, ws, note)) => {
                    ui.label(RichText::new("Process").color(WEAK));
                    ui.label(pid.map(|p| format!("PID {p}")).unwrap_or_else(|| "starting…".into()));
                    ui.end_row();
                    if pid.is_some() && ws > 0 {
                        ui.label(RichText::new("Memory now").color(WEAK));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(mem::mb(ws)).monospace());
                            if !note.text().is_empty() {
                                help(ui, note.text());
                            }
                        });
                        ui.end_row();
                    }
                    if since > 0 {
                        ui.label(RichText::new("Running since").color(WEAK));
                        ui.label(local_time(since, false));
                        ui.end_row();
                    }
                    ui.label(RichText::new("Reconnects").color(WEAK));
                    ui.label(format!("{attempts} of {} allowed", s.max_attempts));
                    ui.end_row();
                }
                None => {
                    ui.label(RichText::new("Process").color(WEAK));
                    ui.label("not running");
                    ui.end_row();
                }
            }
            ui.label(RichText::new("If it disconnects").color(WEAK));
            let txt = if !s.auto_reconnect {
                "Nothing — auto-reconnect is off"
            } else if verified && s.same_server_rejoin {
                "Rejoins the previous server if possible (best-effort), else a new one"
            } else {
                "Relaunches into a new server"
            };
            ui.label(txt);
            ui.end_row();
        });
        if s.auto_reconnect && !(verified && s.same_server_rejoin) {
            ui.horizontal(|ui| {
                help(ui, if verified { "Same-server rejoin is verified but turned off." } else { "Same-server rejoin is locked until verified." });
                if ui.link("Settings").clicked() {
                    app.nav = Nav::Settings;
                    app.settings_tab = 0;
                }
            });
        }
    });
}

fn remove_card(app: &mut App, ui: &mut egui::Ui, acc: &Account) {
    card(ui, "", |ui| {
        if app.confirm_remove == Some(acc.id) {
            ui.label(format!("Remove @{} and its saved sign-in from this PC? Your Roblox account itself is not affected.", acc.username));
            ui.horizontal(|ui| {
                if ui.add(danger("Remove")).clicked() {
                    app.engine.remove_account(acc.id);
                    app.confirm_remove = None;
                }
                if ui.add(subtle("Cancel")).clicked() {
                    app.confirm_remove = None;
                }
            });
        } else {
            ui.horizontal(|ui| {
                if ui.add(subtle("Remove account…")).clicked() {
                    app.confirm_remove = Some(acc.id);
                }
                help(ui, "Deletes the saved sign-in from this PC only.");
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::parse_place;
    #[test]
    fn place_inputs() {
        assert_eq!(parse_place("920587237").unwrap(), "920587237");
        assert_eq!(parse_place(" https://www.roblox.com/games/920587237/Adopt-Me ").unwrap(), "920587237");
        assert_eq!(parse_place("https://www.roblox.com/games/start?placeId=123&launchData=x").unwrap(), "123");
        assert!(parse_place("hello").is_err());
        assert!(parse_place("https://www.roblox.com/games/").is_err());
    }
}
