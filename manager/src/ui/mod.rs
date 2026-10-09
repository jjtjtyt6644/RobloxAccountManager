pub mod accounts;
pub mod activity;
pub mod settings_view;
pub mod theme;
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
    match app.nav {
        Nav::Accounts => accounts::show(app, ctx),
        Nav::Activity => {
            egui::CentralPanel::default().frame(page_frame()).show(ctx, |ui| activity::show(app, ui));
        }
        Nav::Settings => {
            egui::CentralPanel::default().frame(page_frame()).show(ctx, |ui| settings_view::show(app, ui));
        }
    }
}

pub fn page_frame() -> egui::Frame {
    egui::Frame::none().fill(BG).inner_margin(Margin::symmetric(24.0, 18.0))
}

fn nav_tab(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let rt = RichText::new(text).size(14.5);
    let rt = if selected { rt.color(egui::Color32::WHITE).strong() } else { rt.color(WEAK) };
    ui.add(
        egui::Button::new(rt)
            .frame(selected)
            .fill(egui::Color32::from_rgb(34, 39, 49))
            .stroke(egui::Stroke::NONE)
            .min_size(egui::vec2(0.0, 30.0)),
    )
}

fn top_bar(app: &mut App, ctx: &egui::Context) {
    let running: Vec<i64> = app.accounts.iter().filter(|a| is_running(&a.status)).map(|a| a.id).collect();
    egui::TopBottomPanel::top("top")
        .exact_height(58.0)
        .frame(egui::Frame::none().fill(PANEL).inner_margin(Margin::symmetric(18.0, 0.0)).stroke(egui::Stroke::new(1.0, BORDER)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new("Roblox Account Manager").size(16.0).strong().color(egui::Color32::WHITE));
                ui.add_space(16.0);
                egui::Frame::none()
                    .fill(BG)
                    .stroke(egui::Stroke::new(1.0, BORDER))
                    .rounding(10.0)
                    .inner_margin(Margin::same(3.0))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (n, l) in [(Nav::Accounts, "Accounts"), (Nav::Activity, "Activity"), (Nav::Settings, "Settings")] {
                            if nav_tab(ui, app.nav == n, l).clicked() && app.nav != n {
                                app.nav = n;
                                app.reload();
                            }
                        }
                    });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let busy = app.engine.login_busy.load(Ordering::SeqCst);
                    let label = if busy { "Waiting for sign-in…" } else { "+  Add account" };
                    if ui
                        .add_enabled(!busy, primary(label))
                        .on_hover_text("Opens the official Roblox sign-in page in a separate window.")
                        .clicked()
                    {
                        app.engine.start_login();
                    }
                    if !running.is_empty() && ui.add(subtle(&format!("⏹  Stop all ({})", running.len()))).clicked() {
                        app.engine.kill_many(running.clone());
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("{} running  ·  {} accounts", running.len(), app.accounts.len()))
                            .color(WEAK),
                    );
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
