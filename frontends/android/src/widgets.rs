//! The pieces of a touch screen.
//!
//! A settings row here is not a `checkbox` with a caption beside it: it is a full-width strip,
//! with the name large on the left and the control on the right, tall enough for the thumb to hit
//! without aiming. It is the difference between a shrunken desktop window and a screen made for
//! the hand.
//!
//! **Hints stay hidden.** Zeebx's help texts are long — some have five lines — and all of them
//! open at once become a gray wall where you can no longer find what you were looking for. Each
//! row shows a `?`; whoever wants the explanation taps it, and it opens right there. Whoever
//! already knows sees a list of names and values.

use crate::theme::TARGET_SIZE;

/// Which hint is open. It is the row's identifier, and only one stays open at a time.
pub type ExpandedHint = Option<String>;

/// A section title.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .color(ui.visuals().weak_text_color())
            .strong(),
    );
    ui.add_space(4.0);
}

/// A row strip: the background, the control on the right, the `?` and the name with whatever is
/// left.
///
/// The inner order is right to left on purpose. The control is what decides the width — a switch
/// is 52 points, a value badge is whatever it is — and the name gets the rest; the other way
/// around, a long name would push the touch target off the screen.
fn strip<R>(
    ui: &mut egui::Ui,
    key_id: &str,
    title: &str,
    hint: Option<&str>,
    opened: &mut ExpandedHint,
    controller: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let showing = opened.as_deref() == Some(key_id);
    let frame = egui::Frame::NONE
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(14, 8));

    let result = frame
        .show(ui, |ui| {
            // The row height is given, not discovered. In a `right_to_left` inside a scroll
            // area, the available space is the whole screen, and the name's sub-`Ui` stretched
            // the strip to the end — one row took up the screen and the ones below went out of
            // view.
            let container = egui::vec2(ui.available_width(), TARGET_SIZE - 16.0);
            ui.allocate_ui_with_layout(
                container,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                let returned = controller(ui);
                if hint.is_some() {
                    ui.add_space(4.0);
                    let button = egui::Button::new(
                        egui::RichText::new("?").color(ui.visuals().weak_text_color()),
                    )
                    .frame(false)
                    .min_size(egui::vec2(34.0, 34.0));
                    if ui.add(button).clicked() {
                        *opened = match showing {
                            true => None,
                            false => Some(key_id.to_string()),
                        };
                    }
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(title);
                });
                returned
                },
            )
            .inner
        })
        .inner;

    if showing {
        if let Some(hint) = hint {
            ui.add_space(2.0);
            // The width is imposed before the text exists. A `label` in a `horizontal` does not
            // wrap: it grows, pushes the scroll area sideways and misaligns every other strip —
            // the whole screen goes crooked because of one hint.
            let width = (ui.available_width() - 28.0).max(120.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_max_width(width);
                    ui.label(
                        egui::RichText::new(hint)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });
            ui.add_space(4.0);
        }
    }
    result
}

/// A row that switches on and off.
pub fn toggle(
    ui: &mut egui::Ui,
    key_id: &str,
    title: &str,
    hint: Option<&str>,
    opened: &mut ExpandedHint,
    value: &mut bool,
) -> bool {
    strip(ui, key_id, title, hint, opened, |ui| toggle_switch(ui, value))
}

/// A choice between a few options, like the buttons on a car radio.
///
/// It is a group of large buttons side by side, not a column of dots: the `radio_button` dot is
/// 10 points across, and nobody hits 10 points with a thumb.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    key_id: &str,
    title: &str,
    hint: Option<&str>,
    opened: &mut ExpandedHint,
    value: &mut T,
    options: &[(T, String)],
) -> bool {
    let mut changed = false;
    strip(ui, key_id, title, hint, opened, |_| {});
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (option, name) in options {
            let chosen = *value == *option;
            let button = egui::Button::new(name)
                .min_size(egui::vec2(0.0, TARGET_SIZE - 8.0))
                .selected(chosen);
            if ui.add(button).clicked() && !chosen {
                *value = *option;
                changed = true;
            }
        }
    });
    ui.add_space(6.0);
    changed
}

/// A number in a range, with the value in a badge on the right.
pub fn slider(
    ui: &mut egui::Ui,
    key_id: &str,
    title: &str,
    hint: Option<&str>,
    opened: &mut ExpandedHint,
    value: &mut u8,
    range: std::ops::RangeInclusive<u8>,
    label: impl Fn(u8) -> String,
) -> bool {
    let mut changed = false;
    let current = label(*value);
    strip(ui, key_id, title, hint, opened, |ui| {
        ui.label(egui::RichText::new(current).monospace().strong());
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        // `Slider` does not look at the space it gets: it measures by `spacing.slider_width`,
        // which is 100 points out of the box. For a drag target that is far too short — each
        // pixel is worth several steps — so the rail is stated here, and it is the whole row.
        ui.spacing_mut().slider_width = (ui.available_width() - 28.0).max(120.0);
        // No number beside it: it is already in the row's badge, and the `Slider`'s text field
        // is a tiny target that only gets in the way on touch.
        changed = ui
            .add(egui::Slider::new(value, range).show_value(false))
            .changed();
    });
    ui.add_space(6.0);
    changed
}

/// A row that leads somewhere else: the name on top, the value below, and the arrow on the
/// right.
pub fn navigate(ui: &mut egui::Ui, title: &str, value: &str) -> bool {
    let frame = egui::Frame::NONE
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(14, 10));
    let response = frame
        .show(ui, |ui| {
            let container = egui::vec2(ui.available_width(), TARGET_SIZE - 8.0);
            ui.allocate_ui_with_layout(
                container,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(egui::RichText::new("›").size(24.0));
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.label(title);
                        ui.label(
                            egui::RichText::new(value)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                },
            );
        })
        .response;
    ui.interact(
        response.rect,
        egui::Id::new(("navega", title)),
        egui::Sense::click(),
    )
    .clicked()
}

/// The on/off switch, drawn by hand.
///
/// egui's `checkbox` is an 18-point square with a tick inside — from the other side of the screen
/// you can't tell whether it is checked. A light switch tells its state by position and color,
/// which is what reads from afar.
fn toggle_switch(ui: &mut egui::Ui, enabled: &mut bool) -> bool {
    let size = egui::vec2(52.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *enabled = !*enabled;
    }
    // The animation gives the feedback the finger lacks: without it the switch seems not to have
    // responded.
    let amount = ui
        .ctx()
        .animate_bool_responsive(response.id, *enabled);
    let visual = ui.style().interact_selectable(&response, *enabled);
    let rail = egui::Color32::from_rgb(90, 90, 96).lerp_to_gamma(
        ui.visuals().selection.bg_fill,
        amount,
    );
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, rail);
    let radius = rect.height() / 2.0 - 3.0;
    let center = egui::pos2(
        egui::lerp((rect.left() + radius + 3.0)..=(rect.right() - radius - 3.0), amount),
        rect.center().y,
    );
    ui.painter()
        .circle(center, radius, egui::Color32::WHITE, visual.fg_stroke);
    response.clicked()
}
