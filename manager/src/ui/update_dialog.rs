//! The updater window: pops up over everything when a newer manager version is found (at start-up
//! or via "Check for updates"). One click downloads and installs it, then the manager closes and
//! reopens on the new version by itself. Running Roblox windows are picked up again after the
//! restart, so nothing has to be closed. "Later" hides it until the next start (or the top bar's
//! Update button).
use super::about::markdown;
use super::theme::*;
use super::widgets::*;
use crate::app::App;
use crate::update::{Job, VERSION};
use eframe::egui::{self, Align2, Color32, Margin, Order, RichText, Rounding, Stroke};

pub fn show(app: &mut App, ctx: &egui::Context) {
    let u = app.engine.updates.lock().unwrap().clone();
    let Some(rel) = u.app_latest.clone().filter(|r| r.is_newer()) else { return };
    let working = u.app.busy() || u.restart_now;
    if !working && app.update_dismissed.as_deref() == Some(rel.tag.as_str()) {
        return;
    }

    // Dim everything behind the window and swallow clicks on it.
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("update-dim")).order(Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
        ui.painter().rect_filled(r, Rounding::ZERO, Color32::from_black_alpha(150));
    });

    let width = (screen.width() - 48.0).clamp(300.0, 480.0);
    egui::Window::new("update")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(Order::Foreground)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .default_width(width)
        .frame(
            egui::Frame::none()
                .fill(CARD)
                .stroke(Stroke::new(1.0, BORDER))
                .rounding(Rounding::same(16.0))
                .inner_margin(Margin::same(24.0))
                .shadow(egui::epaint::Shadow { offset: [0.0, 12.0].into(), blur: 40.0, spread: 0.0, color: Color32::from_black_alpha(140) }),
        )
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 20.0, ACCENT.gamma_multiply(0.18));
                // Arrow-down "download" glyph, drawn so it doesn't depend on font coverage.
                let c = r.center();
                let s = Stroke::new(2.2, ACCENT);
                ui.painter().line_segment([c + egui::vec2(0.0, -9.0), c + egui::vec2(0.0, 6.0)], s);
                ui.painter().line_segment([c + egui::vec2(-6.0, 0.0), c + egui::vec2(0.0, 6.0)], s);
                ui.painter().line_segment([c + egui::vec2(6.0, 0.0), c + egui::vec2(0.0, 6.0)], s);
                ui.painter().line_segment([c + egui::vec2(-8.0, 10.0), c + egui::vec2(8.0, 10.0)], s);
                ui.vertical(|ui| {
                    let title = if u.app_installed { "Update installed" } else { "Update available" };
                    ui.label(RichText::new(title).size(20.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new(format!("{}  ·  you have v{VERSION}", rel.tag)).color(WEAK));
                });
            });
            ui.add_space(12.0);

            if !rel.notes.is_empty() && !u.app_installed {
                egui::Frame::none().fill(FIELD).rounding(10.0).inner_margin(Margin::same(12.0)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().min_scrolled_height(140.0).max_height(220.0).show(ui, |ui| markdown(ui, &rel.notes));
                });
                ui.add_space(14.0);
            }

            match &u.app {
                Job::Working(t) => {
                    let mut bar = egui::ProgressBar::new(u.app_progress.unwrap_or(0.0)).desired_height(10.0).rounding(5.0);
                    if u.app_progress.is_none() {
                        bar = bar.animate(true);
                    }
                    ui.add(bar.fill(ACCENT));
                    ui.add_space(4.0);
                    ui.label(RichText::new(t).color(WEAK));
                }
                Job::Failed(er) => {
                    ui.label(RichText::new(er).color(RED));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if u.app_installed {
                            if ui.add(primary("Close and reopen").min_size(egui::vec2(170.0, 40.0))).clicked() {
                                app.restart(ctx);
                            }
                        } else if ui.add(primary("Try again").min_size(egui::vec2(140.0, 40.0))).clicked() {
                            app.engine.install_app_update(true);
                        }
                        if ui.add(subtle("Later").min_size(egui::vec2(90.0, 40.0))).clicked() {
                            app.update_dismissed = Some(rel.tag.clone());
                        }
                    });
                }
                _ if u.app_installed => {
                    ui.label(RichText::new("✔  Installed — reopening the manager…").color(GREEN));
                }
                _ => {
                    ui.horizontal(|ui| {
                        if ui.add(primary("Update now").min_size(egui::vec2(150.0, 40.0))).clicked() {
                            app.engine.install_app_update(true);
                        }
                        if ui.add(subtle("Later").min_size(egui::vec2(90.0, 40.0))).clicked() {
                            app.update_dismissed = Some(rel.tag.clone());
                        }
                    });
                }
            }
            ui.add_space(10.0);
            help(ui, "The manager closes and reopens by itself when it's done. Your Roblox windows stay open and keep being watched.");
        });
}
