pub mod about;
pub mod accounts;
pub mod activity;
pub mod settings_view;
pub mod theme;
pub mod update_dialog;
pub mod widgets;

use crate::app::{App, Nav};
use crate::runtime::mem;
use eframe::egui::{self, Align, Layout, Margin, RichText};
use std::sync::atomic::Ordering;
use theme::*;
use widgets::*;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    top_bar(app, ctx);
    status_bar(app, ctx);
    update_dialog::show(app, ctx);
    match app.nav {
        Nav::Accounts => accounts::show(app, ctx),
        Nav::Activity => {
            egui::CentralPanel::default().frame(page_frame()).show(ctx, |ui| activity::show(app, ui));
        }
        Nav::Settings => {
            egui::CentralPanel::default().frame(page_frame()).show(ctx, |ui| settings_view::show(app, ui, ctx));
        }
        Nav::About => {
            egui::CentralPanel::default().frame(page_frame()).show(ctx, |ui| about::show(app, ui, ctx));
        }
    }
}

pub fn page_frame() -> egui::Frame {
    egui::Frame::none().fill(BG).inner_margin(Margin::symmetric(24.0, 18.0))
}

fn top_bar(app: &mut App, ctx: &egui::Context) {
    let running: Vec<i64> = app.accounts.iter().filter(|a| is_running(&a.status)).map(|a| a.id).collect();
    // Drop the less important pieces as the window narrows so the buttons never get pushed off.
    let w = ctx.screen_rect().width();
    let show_title = w >= 980.0;
    let show_counts = w >= 1120.0;
    let compact = w < 860.0;
    let update = {
        let u = app.engine.updates.lock().unwrap();
        u.app_latest.as_ref().filter(|r| r.is_newer() && !u.app_installed).map(|r| r.tag.clone())
    };
    egui::TopBottomPanel::top("top")
        .exact_height(58.0)
        .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::symmetric(16.0, 0.0)).stroke(egui::Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                if show_title {
                    ui.label(RichText::new("Roblox Account Manager").size(16.0).strong().color(egui::Color32::WHITE));
                    ui.add_space(12.0);
                }
                const TABS: [(Nav, &str); 4] =
                    [(Nav::Accounts, "Accounts"), (Nav::Activity, "Activity"), (Nav::Settings, "Settings"), (Nav::About, "About")];
                let cur = TABS.iter().position(|t| t.0 == app.nav).unwrap_or(0);
                let labels: Vec<&str> = TABS.iter().map(|t| t.1).collect();
                if let Some(i) = segmented(ui, &labels, cur) {
                    app.nav = TABS[i].0;
                    app.reload();
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let busy = app.engine.login_busy.load(Ordering::SeqCst);
                    let label = match (busy, compact) {
                        (true, _) => "Signing in…",
                        (false, true) => "+  Add",
                        (false, false) => "+  Add account",
                    };
                    if ui
                        .add_enabled(!busy, primary(label))
                        .on_hover_text("Opens the official Roblox sign-in page in a separate window.")
                        .clicked()
                    {
                        app.engine.start_login();
                    }
                    if !running.is_empty() {
                        let t = if compact { "⏹  Stop all".to_string() } else { format!("⏹  Stop all ({})", running.len()) };
                        if ui.add(subtle(&t)).clicked() {
                            app.engine.kill_many(running.clone());
                        }
                    }
                    if let Some(tag) = &update {
                        let t = if compact { "Update".to_string() } else { format!("Update {tag}") };
                        if ui
                            .add(egui::Button::new(RichText::new(t).color(GREEN).strong()).fill(GREEN.gamma_multiply(0.14)))
                            .on_hover_text("A new version of the manager is available")
                            .clicked()
                        {
                            app.update_dismissed = None;
                        }
                    }
                    if show_counts {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!("{} running  ·  {} accounts", running.len(), app.accounts.len()))
                                .color(WEAK),
                        );
                    }
                });
            });
        });
}

fn status_bar(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::bottom("status")
        .exact_height(30.0)
        .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::symmetric(18.0, 0.0)).stroke(egui::Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                match app.last_event.clone() {
                    Some(e) => {
                        let (label, color, _) = event_info(&e.event);
                        dot(ui, color);
                        let who = if e.account_id == 0 { String::new() } else { format!("{} — ", e.label) };
                        let mut detail: String = e.detail.chars().take(110).collect();
                        if e.detail.chars().count() > 110 {
                            detail.push('…');
                        }
                        let r = ui.add(
                            egui::Label::new(
                                RichText::new(format!("{}  {who}{label}: {detail}", local_time(e.ts, false))).color(WEAK).size(12.5),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if r.on_hover_text("Open Activity").clicked() {
                            app.nav = Nav::Activity;
                            app.reload();
                        }
                    }
                    None => {
                        ui.label(RichText::new("Ready").color(WEAK).size(12.5));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let clients: Vec<u64> = app.engine.tr().values().filter(|t| t.pid.is_some()).map(|t| t.mem_ws).collect();
                    if let Some(m) = app.mem {
                        ui.label(RichText::new(format!("Manager {}", mem::mb(m.working_set))).color(WEAK).size(12.5))
                            .on_hover_text(format!(
                                "Working set {}\nPrivate {}\nPeak {}",
                                mem::mb(m.working_set),
                                mem::mb(m.private),
                                mem::mb(m.peak_working_set)
                            ));
                    }
                    if !clients.is_empty() {
                        ui.label(RichText::new("·").color(WEAK).size(12.5));
                        ui.label(
                            RichText::new(format!("Roblox windows {}", mem::mb(clients.iter().sum())))
                                .color(WEAK)
                                .size(12.5),
                        );
                    }
                });
            });
        });
}
