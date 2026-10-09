//! Activity: what happened, in plain language, newest first, local time.
use super::theme::*;
use super::widgets::*;
use crate::app::App;
use eframe::egui::{self, RichText};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Activity").size(24.0).strong());
        ui.add_space(16.0);
        let cur = match app.log_account {
            None => "All accounts".to_string(),
            Some(0) => "Manager".to_string(),
            Some(id) => app.accounts.iter().find(|a| a.id == id).map(|a| a.label.clone()).unwrap_or_else(|| "—".into()),
        };
        egui::ComboBox::from_id_salt("log-acc").selected_text(cur).width(180.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.log_account, None, "All accounts");
            ui.selectable_value(&mut app.log_account, Some(0), "Manager");
            for a in &app.accounts {
                ui.selectable_value(&mut app.log_account, Some(a.id), &a.label);
            }
        });
        ui.checkbox(&mut app.log_problems_only, "Problems only");
    });
    help(ui, "Newest first. Times are your local time.");
    ui.add_space(6.0);

    let rows: Vec<_> = app
        .logs
        .iter()
        .rev()
        .filter(|e| app.log_account.map_or(true, |id| e.account_id == id))
        .filter(|e| !app.log_problems_only || event_info(&e.event).2)
        .collect();

    egui::Frame::none().fill(CARD).rounding(10.0).stroke(egui::Stroke::new(1.0, BORDER)).inner_margin(12.0).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        if rows.is_empty() {
            help(ui, "Nothing here yet.");
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("activity").num_columns(4).striped(true).spacing([18.0, 8.0]).show(ui, |ui| {
                for h in ["Time", "Account", "What happened", "Details"] {
                    ui.label(RichText::new(h).color(WEAK).size(12.5).strong());
                }
                ui.end_row();
                for e in rows {
                    let (label, color, _) = event_info(&e.event);
                    ui.label(RichText::new(local_time(e.ts, true)).color(WEAK).monospace());
                    ui.label(if e.account_id == 0 { "Manager".to_string() } else { e.label.clone() });
                    ui.horizontal(|ui| {
                        dot(ui, color);
                        ui.label(RichText::new(label).color(color));
                    });
                    ui.add(egui::Label::new(RichText::new(&e.detail).color(TEXT)).wrap());
                    ui.end_row();
                }
            });
        });
    });
}
