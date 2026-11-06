//! The controls drawn on screen: the game overlay and the editor that moves the pieces around.
//!
//! The math of where each piece sits and what the finger presses belongs to the core, in
//! [`zeebx::input::touch`]; what lives here is the drawing and reading the touch.
//!
//! **In-game touch does not go through egui.** egui gets the finger as a single pointer, and
//! playing needs two at once: one thumb holding the D-pad and the other pressing 1. That is why
//! the [`Overlay`] reads the whole `MotionEvent`, with every finger.

use android_activity::input::{Axis, MotionAction, MotionEvent, ToolType};

use zeebx::input::touch::{self, Element, TouchControl};
use zeebx::ui::settings::{OnScreenControls, OnScreenControlsMode};

use crate::theme::TARGET_SIZE;
use crate::{Emulator, View};

/// What the overlay knows between one frame and the next.
#[derive(Default)]
pub struct Overlay {
    /// The buttons the fingers are pressing now, one bit per [`zeebx::input::Pad`] index. It is
    /// added to the physical controller, not written over it: lifting a finger off the screen
    /// must not release a button that is held on the controller.
    pub buttons: u32,
    /// The shortcut pieces the fingers are holding: turbo, fast-forward and rewind.
    pub shortcuts: touch::Shortcuts,
    /// The last press came from a physical controller. In automatic mode, this hides the pieces.
    pub physical_controller: bool,
    /// The finger holding each stick, by the id Android gives it, in the order of [`STICKS`].
    ///
    /// Buttons keep no state — what is pressed is purely a function of where the fingers are.
    /// The stick does: the finger that touched it owns it until it lifts, see
    /// [`touch::grab_stick`]. And it is the id, not the position in the list, because a finger's
    /// position changes when another one lifts.
    sticks: [Option<i32>; 2],
    /// Where the sticks are, from `-1` to `1`, in the order of [`zeebx::input::Pad::axes`].
    pub axes: [f32; 4],
    /// The pieces as they were at the last draw.
    ///
    /// Touch uses these, not a fresh calculation: the event arrives before the frame is drawn, and
    /// only at draw time is the screen size known.
    pub elements: Vec<Element>,
}

impl Overlay {
    /// Whether the pieces are on screen.
    pub fn visible(&self, settings_page: &OnScreenControls) -> bool {
        match settings_page.mode {
            OnScreenControlsMode::Always => true,
            OnScreenControlsMode::Auto => !self.physical_controller,
            OnScreenControlsMode::Never => false,
        }
    }

    /// A physical controller press arrived: in automatic mode, the pieces get out of the way.
    pub fn saw_controller(&mut self) {
        self.physical_controller = true;
        self.release();
    }

    /// Releases everything. For when touch stops going to the game — the "back" question opened,
    /// the app lost focus — with a finger still on a piece.
    pub fn release(&mut self) {
        self.buttons = 0;
        self.shortcuts = touch::Shortcuts::default();
        self.sticks = [None; 2];
        self.axes = [0.0; 4];
    }

    /// A touch event, with every finger that is on the screen.
    pub fn touch(&mut self, motion: &MotionEvent, pixels_per_point: f32) {
        // Any touch brings the pieces back in automatic mode: it is how someone who put the
        // controller down says they want to play on the screen.
        self.physical_controller = false;

        let action = motion.action();
        if matches!(action, MotionAction::Cancel) {
            self.release();
            return;
        }
        let action_index = motion.pointer_index();

        // A finger that just touched down can grab a stick, if nobody holds it yet.
        if matches!(action, MotionAction::Down | MotionAction::PointerDown) {
            let finger = motion.pointer_at_index(action_index);
            let point = [
                finger.axis_value(Axis::X) / pixels_per_point,
                finger.axis_value(Axis::Y) / pixels_per_point,
            ];
            if let Some(touch_control) = touch::grab_stick(&self.elements, point)
                && let Some(slot) = STICKS.iter().position(|&m| m == touch_control)
                && self.sticks[slot].is_none()
            {
                self.sticks[slot] = Some(finger.pointer_id());
            }
        }

        // The finger that is lifting still comes in this event's list. It is left out of the
        // count, otherwise the button would only release on another finger's next move, and it
        // releases the stick it was holding.
        let lifting = matches!(action, MotionAction::Up | MotionAction::PointerUp).then_some(action_index);
        self.axes = [0.0; 4];
        let mut free_slots = Vec::new();
        for finger in motion.pointers() {
            let id = finger.pointer_id();
            if Some(finger.pointer_index()) == lifting {
                for stick in &mut self.sticks {
                    if *stick == Some(id) {
                        *stick = None;
                    }
                }
                continue;
            }
            if finger.tool_type() == ToolType::Palm {
                continue;
            }
            let point = [
                finger.axis_value(Axis::X) / pixels_per_point,
                finger.axis_value(Axis::Y) / pixels_per_point,
            ];
            let Some(slot) = self.sticks.iter().position(|&m| m == Some(id)) else {
                free_slots.push(point);
                continue;
            };
            let touch_control = STICKS[slot];
            if let (Some(element), Some([horizontal, vertical])) = (
                self.elements.iter().find(|e| e.touch_control == touch_control),
                touch_control.axes(),
            ) {
                let [x, y] = touch::stick(element, point);
                self.axes[horizontal] = x;
                self.axes[vertical] = y;
            }
        }
        // A stick's finger does not press buttons, not even when it passes over one.
        self.shortcuts = touch::shortcuts(&self.elements, free_slots.iter().copied());
        self.buttons = touch::buttons(&self.elements, free_slots);
    }
}

/// The sticks, in the order of the [`Overlay::sticks`] slots.
const STICKS: [TouchControl; 2] = [TouchControl::LeftStick, TouchControl::RightStick];

/// The screen size in points, in the core's format.
fn size(ctx: &egui::Context) -> [f32; 2] {
    let screen = ctx.content_rect();
    [screen.width(), screen.height()]
}

/// Draws the pieces. `pressed_buttons` lights the ones that are pressed, `axes` puts each stick's
/// knob in place; `highlight` is the one the editor is dragging.
fn draw(
    painter: &egui::Painter,
    elements: &[Element],
    pressed_buttons: u32,
    shortcuts: touch::Shortcuts,
    axes: [f32; 4],
    opacity: u8,
    highlight: Option<TouchControl>,
) {
    let alpha = (f32::from(opacity.min(100)) / 100.0 * 255.0) as u8;
    let background = egui::Color32::from_rgba_unmultiplied(20, 20, 24, alpha / 2 + alpha / 4);
    let lit = egui::Color32::from_rgba_unmultiplied(0, 110, 190, alpha);
    let ink = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alpha);
    let border = egui::Stroke::new(2.0_f32, ink);

    for element in elements {
        let center = egui::pos2(element.center[0], element.center[1]);
        let [mx, my] = element.middle;
        let container = egui::Rect::from_center_size(center, egui::vec2(2.0 * mx, 2.0 * my));
        // The highlight applies to the whole group: choosing 3 lights all four.
        let highlight_stroke = match highlight.is_some_and(|d| d.key_id() == element.touch_control.key_id()) {
            true => egui::Stroke::new(3.0_f32, egui::Color32::from_rgb(255, 200, 0)),
            false => border,
        };

        match element.touch_control {
            TouchControl::LeftStick | TouchControl::RightStick => {
                // The base and the knob. Pushed against the edge, the knob still sits entirely
                // inside the base: the maximum offset is the base's radius minus its own.
                let [horizontal, vertical] = element.touch_control.axes().unwrap_or([0, 1]);
                let offset = egui::vec2(axes[horizontal], axes[vertical]);
                let knob = mx * 0.45;
                let in_use = offset != egui::Vec2::ZERO;
                painter.circle_filled(center, mx, background);
                painter.circle_stroke(center, mx, highlight_stroke);
                let color = match in_use {
                    true => lit,
                    false => egui::Color32::from_rgba_unmultiplied(200, 200, 210, alpha / 2),
                };
                let location = center + offset * (mx - knob);
                painter.circle_filled(location, knob, color);
                painter.circle_stroke(location, knob, border);
            }
            TouchControl::Dpad => {
                // A single cross, like the controller's, with each arm lit on its own: the
                // diagonals light two.
                //
                // **The center and the arms do not overlap.** It used to be four rectangles
                // crossing in the middle, and with the semi-transparent color the crossing came
                // out darker, with each arm's rounded corners showing through. And the fill goes
                // in a mesh without edge anti-aliasing: neighboring pieces drawn as egui shapes
                // get a light line at the seam, which is each one's anti-aliased edge.
                let arm = mx * 0.34;
                let tip = mx * 0.95;
                let [up, down, left, right] = zeebx::input::DPAD;
                let ret = |x0: f32, y0: f32, x1: f32, y1: f32| {
                    egui::Rect::from_min_max(
                        center + egui::vec2(x0, y0),
                        center + egui::vec2(x1, y1),
                    )
                };
                let color_of = |index: usize| match pressed_buttons & (1 << index) != 0 {
                    true => lit,
                    false => background,
                };
                let mut mesh = egui::Mesh::default();
                mesh.add_colored_rect(ret(-arm, -arm, arm, arm), background);
                mesh.add_colored_rect(ret(-arm, -tip, arm, -arm), color_of(up));
                mesh.add_colored_rect(ret(-arm, arm, arm, tip), color_of(down));
                mesh.add_colored_rect(ret(-tip, -arm, -arm, arm), color_of(left));
                mesh.add_colored_rect(ret(arm, -arm, tip, arm), color_of(right));
                painter.add(mesh);
                // The outline is the whole cross's, in a single line.
                let outline = [
                    (-arm, -tip),
                    (arm, -tip),
                    (arm, -arm),
                    (tip, -arm),
                    (tip, arm),
                    (arm, arm),
                    (arm, tip),
                    (-arm, tip),
                    (-arm, arm),
                    (-tip, arm),
                    (-tip, -arm),
                    (-arm, -arm),
                ]
                .map(|(x, y)| center + egui::vec2(x, y));
                painter.add(egui::Shape::closed_line(outline.to_vec(), highlight_stroke));
                // The arrows at the tips, so the cross reads as a D-pad and not as a "+".
                for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
                    let axis = egui::vec2(dx, dy);
                    let side = egui::vec2(-dy, dx);
                    let apex = center + axis * tip * 0.82;
                    let base = center + axis * tip * 0.55;
                    painter.add(egui::Shape::convex_polygon(
                        vec![apex, base + side * arm * 0.55, base - side * arm * 0.55],
                        ink,
                        egui::Stroke::NONE,
                    ));
                }
            }
            touch_control => {
                let pressed = match touch_control {
                    TouchControl::Turbo => shortcuts.turbo,
                    TouchControl::FastForward => shortcuts.fast_forward,
                    TouchControl::Back => shortcuts.back,
                    _ => touch_control
                        .index()
                        .is_some_and(|index| pressed_buttons & (1 << index) != 0),
                };
                let color = match pressed {
                    true => lit,
                    false => background,
                };
                match touch_control.is_round() && mx == my {
                    true => {
                        painter.circle_filled(center, mx, color);
                        painter.circle_stroke(center, mx, highlight_stroke);
                    }
                    false => {
                        painter.rect_filled(container, my, color);
                        painter.rect_stroke(container, my, highlight_stroke, egui::StrokeKind::Inside);
                    }
                }
                let letter = match touch_control {
                    TouchControl::Home => my * 0.75,
                    _ => my * 0.85,
                };
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    touch_control.label(),
                    egui::FontId::proportional(letter),
                    ink,
                );
            }
        }
    }
}

impl Emulator {
    /// The pieces over the game, when visible. Called by the game screen after the frame, so as
    /// to sit on top of it.
    pub(crate) fn draw_overlay(&mut self, ctx: &egui::Context) {
        let settings_page = &self.settings.on_screen_controls;
        // With the "back" question open the game is paused, and the pieces, on the front layer,
        // would sit over the dialog and eat the touches on its buttons.
        if self.confirming || !self.overlay.visible(settings_page) {
            self.overlay.elements.clear();
            return;
        }
        let mut elements =
            touch::build(size(ctx), settings_page.scale, &settings_page.positions, &settings_page.sizes);
        // Shortcut pieces only when the feature is in use: see `OnScreenControls::show`.
        elements.retain(|element| settings_page.show(element.touch_control, &self.settings));
        self.overlay.elements = elements;
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("controles-na-tela"),
        ));
        draw(
            &painter,
            &self.overlay.elements,
            self.overlay.buttons,
            self.overlay.shortcuts,
            self.overlay.axes,
            settings_page.opacity,
            None,
        );
    }

    /// The screen for moving and resizing the pieces.
    ///
    /// Dragging a piece moves it; tapping it, or dragging it, selects it, and the middle bar grows
    /// and shrinks the selected one. Saves on release and at every size step.
    ///
    /// Size is set with buttons, not by pinching: egui gets the finger as a single pointer (see
    /// [`crate::host_input`]), and a pinch would need a separate touch path just for this
    /// screen.
    pub(crate) fn touch_editor(&mut self, ctx: &egui::Context) {
        let screen = size(ctx);
        let background = egui::Frame::NONE.fill(egui::Color32::from_rgb(16, 16, 20));
        let mut needs_save = false;
        egui::CentralPanel::default().frame(background).show(ctx, |ui| {
            let area = ctx.content_rect();
            // The outline of the game frame, 4:3 and centered, as it appears in "Fit": it is the
            // reference for how much each piece will cover.
            let height = area.height();
            let frame =
                egui::Rect::from_center_size(area.center(), egui::vec2(height * 4.0 / 3.0, height));
            ui.painter().rect_filled(frame, 0.0, egui::Color32::from_rgb(34, 38, 48));
            ui.painter().text(
                frame.center() - egui::vec2(0.0, 70.0),
                egui::Align2::CENTER_CENTER,
                self.catalog.get("touch.editor.hint"),
                egui::FontId::proportional(18.0),
                egui::Color32::from_gray(150),
            );

            // In the editor only the pieces the game shows: arranging a button that does not
            // appear helps nobody. See `OnScreenControls::show`.
            let visible_items: Vec<TouchControl> = TouchControl::ALL
                .into_iter()
                .filter(|&touch_control| self.settings.on_screen_controls.show(touch_control, &self.settings))
                .collect();
            let settings_page = &mut self.settings.on_screen_controls;
            let response =
                ui.interact(area, egui::Id::new("editor-de-toque"), egui::Sense::click_and_drag());
            let mut elements =
                touch::build(screen, settings_page.scale, &settings_page.positions, &settings_page.sizes);
            elements.retain(|element| visible_items.contains(&element.touch_control));

            // A tap selects the piece; on empty space, it clears the selection.
            if response.clicked()
                && let Some(finger) = response.interact_pointer_pos()
            {
                self.chosen = touch::under(&elements, [finger.x, finger.y]);
            }
            if response.drag_started() {
                // The piece is the one that was under the finger **when it touched down**: the
                // drag only starts after a few points of movement, and by then the finger may
                // already have left it.
                let origin = ctx.input(|i| i.pointer.press_origin());
                self.dragging = origin.and_then(|origin| {
                    let touch_control = touch::under(&elements, [origin.x, origin.y])?;
                    // The center is the group's: grabbing 3 drags the whole diamond.
                    let [x, y] = touch::group_center(&elements, touch_control)?;
                    // Keeps where, inside the piece, the finger grabbed it: without this it would
                    // jump to center on the finger.
                    Some((touch_control, egui::pos2(x, y) - origin))
                });
                if let Some((touch_control, _)) = self.dragging {
                    self.chosen = Some(touch_control);
                }
            }
            if response.dragged()
                && let (Some((touch_control, delta)), Some(finger)) =
                    (self.dragging, response.interact_pointer_pos())
            {
                let center = finger + delta;
                settings_page
                    .positions
                    .insert(touch_control.key_id().to_string(), touch::fraction([center.x, center.y], screen));
            }
            if response.drag_stopped() && self.dragging.take().is_some() {
                needs_save = true;
            }

            // In the editor the pieces show up even with opacity turned all the way down: nobody
            // drags what they can't see.
            let mut elements =
                touch::build(screen, settings_page.scale, &settings_page.positions, &settings_page.sizes);
            elements.retain(|element| visible_items.contains(&element.touch_control));
            draw(
                ui.painter(),
                &elements,
                0,
                touch::Shortcuts::default(),
                [0.0; 4],
                settings_page.opacity.max(70),
                self.chosen,
            );
        });

        // The bar sits in the middle, in an area above the panel: that way its buttons win over
        // the drag. In the middle because it is the only place no piece takes by default — at
        // the top it covered HOME, and in the editor HOME could not be grabbed.
        egui::Area::new(egui::Id::new("barra-do-editor"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    if let Some(touch_control) = self.chosen {
                        needs_save |= self.size_bar(ui, touch_control);
                        ui.add_space(8.0);
                    }
                    ui.horizontal(|ui| {
                        let restore = egui::Button::new(self.catalog.get("touch.editor.reset"))
                            .min_size(egui::vec2(0.0, TARGET_SIZE));
                        if ui.add(restore).clicked() {
                            let settings_page = &mut self.settings.on_screen_controls;
                            settings_page.positions.clear();
                            settings_page.sizes.clear();
                            needs_save = true;
                        }
                        let ready = egui::Button::new(self.catalog.get("touch.editor.done"))
                            .min_size(egui::vec2(0.0, TARGET_SIZE));
                        if ui.add(ready).clicked() {
                            self.chosen = None;
                            self.location = View::Settings;
                        }
                    });
                });
            });

        if needs_save {
            self.save();
        }
    }

    /// "−  D-pad 120%  +": the size of the selected piece, in 10% steps. Says whether it changed.
    fn size_bar(&mut self, ui: &mut egui::Ui, touch_control: TouchControl) -> bool {
        const STEP: u8 = 10;
        let name = match touch_control {
            TouchControl::Dpad => self.catalog.get("touch.piece.dpad"),
            TouchControl::LeftStick => self.catalog.get("touch.piece.lstick"),
            TouchControl::RightStick => self.catalog.get("touch.piece.rstick"),
            TouchControl::B1 | TouchControl::B2 | TouchControl::B3 | TouchControl::B4 => self.catalog.get("touch.piece.buttons"),
            other => other.label(),
        }
        .to_string();
        let sizes = &mut self.settings.on_screen_controls.sizes;
        let current = sizes.get(touch_control.key_id()).copied().unwrap_or(100);
        let mut create = current;
        ui.horizontal(|ui| {
            let button = |text| egui::Button::new(text).min_size(egui::vec2(TARGET_SIZE, TARGET_SIZE));
            if ui
                .add_enabled(current > touch::MIN_SIZE, button("−"))
                .clicked()
            {
                create = current.saturating_sub(STEP).max(touch::MIN_SIZE);
            }
            ui.label(egui::RichText::new(format!("{name}  {current}%")).strong());
            if ui
                .add_enabled(current < touch::MAX_SIZE, button("+"))
                .clicked()
            {
                create = current.saturating_add(STEP).min(touch::MAX_SIZE);
            }
        });
        if create == current {
            return false;
        }
        // 100 is not saved: it is what absence already says, and the file keeps only what changed.
        match create {
            100 => sizes.remove(touch_control.key_id()),
            _ => sizes.insert(touch_control.key_id().to_string(), create),
        };
        true
    }
}
