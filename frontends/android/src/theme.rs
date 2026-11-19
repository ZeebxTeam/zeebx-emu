//! The size of things on a screen held in the hands.
//!
//! egui comes tuned for a monitor and a mouse: 12-point text, 18-point targets, 4-point spacing.
//! That is what you can hit with the tip of an arrow; with a thumb, you can't. Reading changes
//! too: a handheld sits forty or fifty centimeters from the face, not thirty, so the same
//! apparent size needs more points.
//!
//! The numbers here come from what Android asks of itself: 48 as the minimum side of a touch
//! target, body text at 16, and spacing between rows that lets the finger miss by a few points
//! without hitting the neighbor.

/// How long the shorter side of anything that gets touched is.
pub const TARGET_SIZE: f32 = 48.0;

/// Sizes the interface for a handheld screen.
pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(26.0, Proportional)),
        (TextStyle::Body, FontId::new(17.0, Proportional)),
        (TextStyle::Button, FontId::new(17.0, Proportional)),
        // "Small" is the hint and the secondary label. At 14 it is still readable at arm's
        // length; at the factory size, 9, it is decoration.
        (TextStyle::Small, FontId::new(14.0, Proportional)),
        (TextStyle::Monospace, FontId::new(15.0, Monospace)),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(10.0, 10.0);
    spacing.button_padding = egui::vec2(14.0, 10.0);
    spacing.interact_size = egui::vec2(TARGET_SIZE, TARGET_SIZE);
    spacing.icon_width = 22.0;
    spacing.icon_width_inner = 12.0;
    spacing.slider_rail_height = 10.0;
    spacing.scroll.bar_width = 12.0;
    spacing.window_margin = egui::Margin::same(16);
    spacing.menu_margin = egui::Margin::same(12);

    // Rounder corners and a bigger difference between idle and touched: with no pointer
    // hovering, the only feedback the finger gets is at the moment of the touch.
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(10);
    }
    style.visuals.window_corner_radius = egui::CornerRadius::same(14);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(0, 110, 190);

    ctx.set_style(style);
}
