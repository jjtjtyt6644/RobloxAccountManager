//! One place for colours, sizes and spacing so every screen looks like the same app.
use eframe::egui::{self, Color32, FontId, Rounding, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(13, 15, 19);
pub const PANEL: Color32 = Color32::from_rgb(19, 22, 27);
pub const CARD: Color32 = Color32::from_rgb(22, 26, 32);
pub const BORDER: Color32 = Color32::from_rgb(35, 39, 47);
pub const FIELD: Color32 = Color32::from_rgb(15, 18, 22);
pub const TEXT: Color32 = Color32::from_rgb(230, 232, 238);
pub const WEAK: Color32 = Color32::from_rgb(140, 147, 161);

pub const ACCENT: Color32 = Color32::from_rgb(61, 123, 245);
pub const GREEN: Color32 = Color32::from_rgb(64, 196, 120);
pub const AMBER: Color32 = Color32::from_rgb(235, 184, 64);
pub const ORANGE: Color32 = Color32::from_rgb(242, 140, 64);
pub const RED: Color32 = Color32::from_rgb(232, 88, 88);
pub const PURPLE: Color32 = Color32::from_rgb(168, 124, 240);
pub const GREY: Color32 = Color32::from_rgb(116, 122, 136);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.extreme_bg_color = FIELD; // text-edit background
    v.faint_bg_color = Color32::from_rgb(24, 27, 32); // striped rows
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_rgb(38, 64, 120);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_rounding = Rounding::same(12.0);
    v.window_stroke = Stroke::new(1.0, BORDER);

    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    // Visible edges for checkboxes, text fields and slider rails on the dark cards.
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(62, 69, 82));
    v.slider_trailing_fill = true;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    let base = Color32::from_rgb(28, 32, 39);
    let hover = Color32::from_rgb(38, 43, 52);
    v.widgets.inactive.bg_fill = base;
    v.widgets.inactive.weak_bg_fill = base;
    v.widgets.hovered.bg_fill = hover;
    v.widgets.hovered.weak_bg_fill = hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 76, 90));
    v.widgets.active.bg_fill = Color32::from_rgb(58, 64, 78);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(58, 64, 78);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        // Small radius so checkboxes stay square; buttons set their own larger radius (widgets.rs).
        w.rounding = Rounding::same(5.0);
    }
    // Always dark, whatever the Windows light/dark setting. egui follows the system theme by default
    // and keeps a separate style per theme, so on a PC in light mode it used its light style (dark
    // text, light buttons) on top of our dark panels — black-on-black text. Pin the theme to dark
    // and give both theme slots the same visuals so nothing can switch underneath us.
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Light, v.clone());
    ctx.set_visuals_of(egui::Theme::Dark, v);

    ctx.all_styles_mut(|s| {
        s.animation_time = 0.0; // no animations => no extra repaints while idle
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(14.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.combo_width = 200.0;
        s.spacing.slider_width = 200.0;
        s.spacing.text_edit_width = 260.0;
        s.text_styles = [
            (TextStyle::Heading, FontId::proportional(22.0)),
            (TextStyle::Body, FontId::proportional(14.5)),
            (TextStyle::Button, FontId::proportional(14.5)),
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}
