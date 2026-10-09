//! Small reusable pieces: status pill, cards, buttons, friendly names for statuses and events.
use super::theme::*;
use eframe::egui::{self, Color32, Margin, RichText, Rounding, Stroke};

pub fn status_info(status: &str) -> (Color32, &'static str) {
    match status {
        "live" => (GREEN, "Running"),
        "queued" => (AMBER, "Queued"),
        "launching" => (AMBER, "Starting…"),
        "reconnecting" => (ORANGE, "Reconnecting…"),
        "crashed" => (RED, "Stopped (crashed)"),
        "needs_relogin" => (PURPLE, "Sign-in expired"),
        _ => (GREY, "Not running"),
    }
}

pub fn is_running(status: &str) -> bool {
    matches!(status, "live" | "queued" | "launching" | "reconnecting")
}

/// (friendly label, colour, counts as a problem)
pub fn event_info(event: &str) -> (&'static str, Color32, bool) {
    match event {
        "launch" => ("Launched", GREEN, false),
        "kill" => ("Stopped", GREY, false),
        "account_added" => ("Account added", GREEN, false),
        "failure" => ("Problem detected", ORANGE, true),
        "auth_expired" => ("Sign-in expired", PURPLE, true),
        "cookie_error" => ("Sign-in missing", PURPLE, true),
        "ticket_error" => ("Couldn't get launch ticket", RED, true),
        "flags_error" => ("Couldn't apply graphics settings", RED, true),
        "spawn_error" => ("Couldn't start Roblox", RED, true),
        "add_failed" => ("Couldn't add account", RED, true),
        "login_cancelled" => ("Sign-in cancelled", GREY, false),
        "blocked" => ("Launch blocked", AMBER, true),
        "probe" => ("Rejoin check", ACCENT, false),
        "diagnostic" => ("Diagnostic", ACCENT, false),
        "multi_instance" => ("Several accounts", ACCENT, false),
        "update" => ("Manager updated", GREEN, false),
        "adopted" => ("Picked up running game", GREEN, false),
        "detected" => ("Found running game", GREEN, false),
        "roblox_update" => ("Roblox updated", GREEN, false),
        "update_failed" => ("Update failed", RED, true),
        _ => ("Event", GREY, false),
    }
}

pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.18))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(9.0, 2.0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).color(color).size(12.5).strong());
        });
}

pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), 4.5, color);
}

/// A titled card that fills the available width.
pub fn card<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::symmetric(20.0, 18.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if !title.is_empty() {
                ui.label(RichText::new(title).size(16.0).strong().color(Color32::WHITE));
                ui.add_space(2.0);
            }
            add(ui)
        })
        .inner
}

pub fn help(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(WEAK).size(12.5));
}

pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE).strong())
        .fill(ACCENT)
        .rounding(8.0)
        .min_size(egui::vec2(0.0, 32.0))
}

pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned()).color(Color32::WHITE).strong())
        .fill(Color32::from_rgb(170, 52, 52))
        .rounding(8.0)
        .min_size(egui::vec2(0.0, 30.0))
}

pub fn subtle(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_owned())).rounding(8.0).min_size(egui::vec2(0.0, 30.0))
}

/// Two-column "label | control" row with a fixed label width, so forms line up.
pub fn form_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(150.0, 28.0), egui::Sense::hover());
        ui.painter().text(
            r.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            WEAK,
        );
        add(ui)
    })
    .inner
}

pub fn local_time(ts: i64, with_date: bool) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_opt(ts, 0).single() {
        Some(t) if with_date => t.format("%b %d  %H:%M:%S").to_string(),
        Some(t) => t.format("%H:%M").to_string(),
        None => "—".into(),
    }
}

/// Small monospace-ish chip for a number, e.g. a client's memory.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::none()
        .fill(color.gamma_multiply(0.14))
        .rounding(Rounding::same(6.0))
        .inner_margin(Margin::symmetric(7.0, 2.0))
        .show(ui, |ui| ui.label(RichText::new(text).monospace().size(11.5).color(color)))
        .response
}

/// Row of equal-width choice tiles (title + one-line tag). Returns true when the choice changed.
pub fn choice_tiles(ui: &mut egui::Ui, selected: &mut usize, options: &[(&str, &str)]) -> bool {
    let mut changed = false;
    let gap = 8.0;
    let n = options.len().max(1) as f32;
    let w = ((ui.available_width() - gap * (n - 1.0)) / n).max(110.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, (title, tag)) in options.iter().enumerate() {
            let on = *selected == i;
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 58.0), egui::Sense::click());
            let fill = if on {
                ACCENT.gamma_multiply(0.16)
            } else if resp.hovered() {
                CARD
            } else {
                FIELD
            };
            let stroke = if on { Stroke::new(1.5, ACCENT) } else { Stroke::new(1.0, BORDER) };
            let p = ui.painter();
            p.rect(rect, Rounding::same(10.0), fill, stroke);
            let x = rect.left() + 14.0;
            p.text(egui::pos2(x, rect.top() + 19.0), egui::Align2::LEFT_CENTER, *title, egui::FontId::proportional(14.5), TEXT);
            p.text(egui::pos2(x, rect.top() + 39.0), egui::Align2::LEFT_CENTER, *tag, egui::FontId::proportional(12.0), if on { ACCENT } else { WEAK });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
                *selected = i;
                changed = true;
            }
        }
    });
    changed
}

/// Explanation box under a choice: what it does, and what it costs.
pub fn explain(ui: &mut egui::Ui, title: &str, body: &str, cost: &str) {
    egui::Frame::none()
        .fill(FIELD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(10.0))
        .inner_margin(Margin::symmetric(14.0, 12.0))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new(body).color(Color32::from_rgb(184, 189, 200)));
            if !cost.is_empty() {
                ui.label(RichText::new(cost).color(WEAK).size(12.5));
            }
        });
}

/// Segmented tab switcher, painted by hand so it stays exactly 36 px tall whatever layout it's in.
/// Returns the index clicked, if any.
pub fn segmented(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let font = egui::FontId::proportional(14.0);
    let pad = egui::vec2(14.0, 0.0);
    let galleys: Vec<_> = items
        .iter()
        .map(|t| ui.painter().layout_no_wrap(t.to_string(), font.clone(), TEXT))
        .collect();
    let inner = 3.0;
    let w: f32 = galleys.iter().map(|g| g.size().x + pad.x * 2.0).sum::<f32>() + inner * 2.0;
    let (rect, whole) = ui.allocate_exact_size(egui::vec2(w, 36.0), egui::Sense::hover());
    let p = ui.painter().clone();
    p.rect(rect, Rounding::same(10.0), BG, Stroke::new(1.0, BORDER));
    let mut x = rect.left() + inner;
    let mut clicked = None;
    for (i, g) in galleys.into_iter().enumerate() {
        let tw = g.size().x + pad.x * 2.0;
        let r = egui::Rect::from_min_size(egui::pos2(x, rect.top() + inner), egui::vec2(tw, rect.height() - inner * 2.0));
        // Ids derive from this control's own allocation, so several switchers can share a parent.
        let resp = ui.interact(r, whole.id.with(("seg", i)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let on = i == selected;
        if on {
            p.rect_filled(r, Rounding::same(7.0), Color32::from_rgb(34, 39, 49));
        } else if resp.hovered() {
            p.rect_filled(r, Rounding::same(7.0), Color32::from_rgb(24, 28, 35));
        }
        let color = if on { Color32::WHITE } else if resp.hovered() { TEXT } else { WEAK };
        p.galley_with_override_text_color(r.center() - g.size() / 2.0, g, color);
        if resp.clicked() && !on {
            clicked = Some(i);
        }
        x += tw;
    }
    clicked
}
