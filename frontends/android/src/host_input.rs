//! From the Android event to the Zeebo controller, and to the finger on the interface.
//!
//! **This file is the reason the Android frontend does not use eframe.** The winit path loses
//! the controller twice: it translates a `Gamepad`'s key codes to `Key::Unidentified`, which egui
//! throws away, and the stick axes never even become keyboard events — they are `MotionEvent`s,
//! and winit only looks at touch ones. Reading the native queue directly, what arrives is what
//! Android sent: `Keycode::ButtonA`, `Axis::X`, the trigger, "back".
//!
//! The button map is the same one desktop gives a modern controller
//! ([`zeebx::input::bindings::Player::with_gamepad`]), and it matches by **position** — on the
//! device 1 is at the bottom, 2 on the left, 3 at the top and 4 on the right (issue #41): south on
//! `b1`, west on `b2`, north on `b3`, east on `b4`, the upper triggers on `zl`/`zr` and Start on
//! `back` — the Zeebo controller has no Start, and HOME takes its place.

use android_activity::input::{
    Axis, InputEvent, KeyAction, Keycode, MotionAction, Source, ToolType,
};

use zeebx::input::{self, AXIS_RANGE, Pad};

use crate::touch::Overlay;

/// Below this the stick is at rest. A handheld's stick never returns exactly to center, and
/// without a dead zone the character walks on its own.
const DEAD_ZONE: f32 = 0.12;

/// The point from which the stick counts as a direction **for the interface**.
///
/// Well above [`DEAD_ZONE`] on purpose: there the goal is to keep the axis from jittering at
/// rest, here it is to keep the cursor from jumping on a bump. Whoever pushes the stick to pick a
/// game pushes it all the way.
const ARROW_THRESHOLD: f32 = 0.6;

/// egui's arrows in the order of [`zeebx::input::DPAD`]: up, down, left, right.
const ARROWS: [egui::Key; 4] = [
    egui::Key::ArrowUp,
    egui::Key::ArrowDown,
    egui::Key::ArrowLeft,
    egui::Key::ArrowRight,
];

/// What the loop collects from one round of events.
#[derive(Default)]
pub struct HostInput {
    /// The events the interface will consume: touches and the keys that navigate menus.
    pub events: Vec<egui::Event>,
    /// "Back" was pressed in this round.
    pub back: bool,
    /// The controller's button 2 was released in this round: it is the "back" for whoever has no
    /// `Back` key.
    ///
    /// Separate from [`back`](Self::back) because **in-game it does not count**: there button 2
    /// belongs to the player, and turning it into "back" would open the close question on every
    /// press. The loop decides, since it knows which screen is up.
    pub ui_back: bool,
    /// The state of the four directions **as the interface sees it**.
    ///
    /// **The D-pad comes in through two doors, and some controllers use both.** Some send
    /// `Keycode::DpadUp`; others, a pair of hat axes in the `MotionEvent`; and the stick is a
    /// third. If each door pushed its own event, the cursor would move two steps per press on a
    /// controller that speaks through both. All of them go through [`HostInput::direction`], and
    /// only a **change** of state becomes an event: one step per press, wherever the press comes
    /// from.
    arrows: [bool; 4],
    /// How many pixels fit in one egui point.
    pixels_per_point: f32,
    /// The physical controller buttons pressed now, by the name desktop gives them (`South`,
    /// `LeftTrigger`): it is how button shortcuts are stored. See [`button_name`].
    pub physical: std::collections::HashSet<&'static str>,
    /// The physical button that went down in this round, for capturing a shortcut. It is a pulse.
    pub last_physical: Option<&'static str>,
}

/// The physical controller buttons: the Android code, the name desktop gives the same button
/// (the `gilrs` one, see `zeebx::input::gamepads`) and the [`Pad`] index it presses.
///
/// **The name is desktop's on purpose**: a shortcut saved as `Source::button("LeftThumb")` works
/// on both frontends, and the same `settings.json` can move from one to the other.
const PHYSICAL_BUTTONS: [(Keycode, &str, usize); 13] = [
    (Keycode::ButtonA, "South", 0),
    (Keycode::ButtonB, "East", 1),
    (Keycode::ButtonX, "West", 2),
    (Keycode::ButtonY, "North", 3),
    (Keycode::ButtonL1, "LeftTrigger", 6),
    (Keycode::ButtonR1, "RightTrigger", 4),
    (Keycode::ButtonL2, "LeftTrigger2", 5),
    (Keycode::ButtonR2, "RightTrigger2", 7),
    (Keycode::ButtonThumbl, "LeftThumb", 10),
    (Keycode::ButtonThumbr, "RightThumb", 8),
    (Keycode::ButtonStart, "Start", 9),
    (Keycode::ButtonSelect, "Select", 9),
    (Keycode::ButtonMode, "Mode", 11),
];

/// The name of a physical button, as desktop calls it.
fn button_name(code: Keycode) -> Option<&'static str> {
    PHYSICAL_BUTTONS.iter().find(|(c, _, _)| *c == code).map(|(_, name, _)| *name)
}

/// The [`Pad`] index the physical button named `name` presses. It is what lets a button that
/// became a shortcut be taken away from the game.
pub fn button_index(name: &str) -> Option<usize> {
    PHYSICAL_BUTTONS.iter().find(|(_, n, _)| *n == name).map(|(_, _, index)| *index)
}

impl HostInput {
    pub fn create(pixels_per_point: f32) -> Self {
        Self {
            pixels_per_point,
            ..Default::default()
        }
    }

    /// Clears what was collected in the previous frame. `back` is a pulse: it lasts one frame.
    pub fn begin_frame(&mut self) {
        self.events.clear();
        self.back = false;
        self.ui_back = false;
        self.last_physical = None;
        // `arrows` is **not** cleared here: it is the state of the directions, not a per-frame
        // pulse. Clearing it would make every round emit the same arrow again, with the D-pad
        // at rest.
    }

    /// Receives an event from the native queue and dispatches it: the controller goes to the
    /// [`Pad`], the finger and the navigation keys go to egui's list.
    ///
    /// With `overlay` present — a game on screen, and on-screen controls turned on — the finger
    /// goes to it, not to egui.
    pub fn receive(
        &mut self,
        event: &InputEvent,
        pad: &mut Pad,
        overlay: Option<&mut Overlay>,
    ) {
        match event {
            InputEvent::KeyEvent(key) => self.key_event(key, pad, overlay),
            InputEvent::MotionEvent(motion) => self.motion(motion, pad, overlay),
            _ => {}
        }
    }

    fn key_event(
        &mut self,
        key_event: &android_activity::input::KeyEvent,
        pad: &mut Pad,
        overlay: Option<&mut Overlay>,
    ) {
        let is_pressed = match key_event.action() {
            KeyAction::Down => true,
            KeyAction::Up => false,
            // `Multiple` is text repetition; it is neither a button press nor a release.
            _ => return,
        };
        let code = key_event.key_code();
        if let Some(name) = button_name(code) {
            match is_pressed {
                true => {
                    if self.physical.insert(name) {
                        self.last_physical = Some(name);
                    }
                }
                false => {
                    self.physical.remove(name);
                }
            }
        }

        if code == Keycode::Back {
            // Only the falling edge: holding the button must not open two dialogs.
            if !is_pressed {
                self.back = true;
            }
            return;
        }

        let [up, down, left, right] = input::DPAD;
        let button = match code {
            Keycode::DpadUp => Some(up),
            Keycode::DpadDown => Some(down),
            Keycode::DpadLeft => Some(left),
            Keycode::DpadRight => Some(right),
            // South, east, west and north — the same order as desktop's controller map.
            Keycode::ButtonA | Keycode::DpadCenter | Keycode::Enter => Some(0),
            Keycode::ButtonB => Some(1),
            Keycode::ButtonX => Some(2),
            Keycode::ButtonY => Some(3),
            // The Zeebo has **one** trigger on each side, ZL and ZR.
            Keycode::ButtonL1 => Some(6),
            Keycode::ButtonR1 => Some(4),
            Keycode::ButtonL2 => Some(5),
            Keycode::ButtonR2 => Some(7),
            Keycode::ButtonThumbl => Some(10),
            Keycode::ButtonThumbr => Some(8),
            // The console has no Start: HOME, which here is `back`, takes its place.
            Keycode::ButtonStart | Keycode::ButtonSelect => Some(9),
            Keycode::ButtonMode => Some(11),
            other => {
                if is_pressed {
                    log::info!("unmapped key: {other:?}");
                }
                None
            }
        };
        if let Some(index) = button {
            pad.press(index, is_pressed);
            if let Some(overlay) = overlay {
                overlay.saw_controller();
            }
        }

        // The same button also navigates the interface: on a handheld the library is browsed
        // with the D-pad, not with the finger.
        // The four directions go through the funnel, which is what reconciles them with the hat
        // and the stick.
        let direction = match code {
            Keycode::DpadUp => Some(0),
            Keycode::DpadDown => Some(1),
            Keycode::DpadLeft => Some(2),
            Keycode::DpadRight => Some(3),
            _ => None,
        };
        if let Some(index) = direction {
            self.direction(index, is_pressed, key_event.repeat_count() > 0);
            return;
        }
        // **Button 2 is the "back" for whoever has no `Back` key.** It used to go to
        // `Key::Escape`, which nobody in this frontend reads -- the only effect was egui dropping
        // focus. Now it raises the pulse, and the loop decides whether it counts: in-game button
        // 2 belongs to the player.
        if code == Keycode::ButtonB {
            if !is_pressed {
                self.ui_back = true;
            }
            return;
        }
        let navigation = match code {
            Keycode::ButtonA | Keycode::DpadCenter | Keycode::Enter => Some(egui::Key::Enter),
            _ => None,
        };
        if let Some(key) = navigation {
            self.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: is_pressed,
                repeat: key_event.repeat_count() > 0,
                modifiers: egui::Modifiers::NONE,
            });
        }
    }

    /// A direction changed state: emits the arrow, and only if it really changed.
    ///
    /// It is the funnel the key, the hat and the stick go through. Android's key repeat is the
    /// only exception that gets past the unchanged state: holding the D-pad has to keep moving.
    fn direction(&mut self, index: usize, is_pressed: bool, is_repeat: bool) {
        if self.arrows[index] == is_pressed && !is_repeat {
            return;
        }
        self.arrows[index] = is_pressed;
        self.events.push(egui::Event::Key {
            key: ARROWS[index],
            physical_key: None,
            pressed: is_pressed,
            repeat: is_repeat,
            modifiers: egui::Modifiers::NONE,
        });
    }

    fn motion(
        &mut self,
        motion: &android_activity::input::MotionEvent,
        pad: &mut Pad,
        overlay: Option<&mut Overlay>,
    ) {
        let source = motion.source();
        if matches!(source, Source::Joystick | Source::Gamepad | Source::Dpad) {
            self.stick(motion, pad);
            // Only a stick that left the center counts as a physical controller: some
            // controllers send a resting axis with nobody touching it, and that would hide the
            // pieces from whoever plays on the screen.
            if let Some(overlay) = overlay
                && (pad.axes.iter().any(|axis| *axis != 0)
                    || input::DPAD.iter().any(|&direction| pad.is_down(direction)))
            {
                overlay.saw_controller();
            }
            return;
        }
        match overlay {
            Some(overlay) => overlay.touch(motion, self.pixels_per_point),
            None => self.touch(motion),
        }
    }

    /// The stick axes and the D-pad that some controllers report as a "hat".
    fn stick(&mut self, motion: &android_activity::input::MotionEvent, pad: &mut Pad) {
        let Some(pointer) = motion.pointers().next() else {
            return;
        };
        for (index, axis) in [Axis::X, Axis::Y, Axis::Z, Axis::Rz].into_iter().enumerate() {
            let raw = pointer.axis_value(axis);
            let value = match raw.abs() < DEAD_ZONE {
                true => 0.0,
                false => raw,
            };
            pad.set_axis(index, (value * AXIS_RANGE as f32) as i32);
        }

        // Not every controller sends the D-pad as keys: some report it as a pair of axes that
        // only take -1, 0 or 1. When it comes that way, this is where it becomes a button.
        let [up, down, left, right] = input::DPAD;
        let x = pointer.axis_value(Axis::HatX);
        let y = pointer.axis_value(Axis::HatY);
        if x != 0.0
            || y != 0.0
            || pad.is_down(up)
            || pad.is_down(down)
            || pad.is_down(left)
            || pad.is_down(right)
        {
            pad.press(left, x < -0.5);
            pad.press(right, x > 0.5);
            pad.press(up, y < -0.5);
            pad.press(down, y > 0.5);
        }

        // **And the interface moves too.** Until now the hat and the stick became console
        // buttons and nothing else: in-game the D-pad worked, and in the grid the cursor did not
        // budge. The game reads the `Pad`, the interface reads egui events, and only the first
        // half had been written.
        //
        // The stick comes in the same way: on a handheld nobody wants to find out that the grid
        // only obeys the D-pad.
        let ex = pointer.axis_value(Axis::X);
        let ey = pointer.axis_value(Axis::Y);
        for (index, enabled) in [
            y < -0.5 || ey < -ARROW_THRESHOLD,
            y > 0.5 || ey > ARROW_THRESHOLD,
            x < -0.5 || ex < -ARROW_THRESHOLD,
            x > 0.5 || ex > ARROW_THRESHOLD,
        ]
        .into_iter()
        .enumerate()
        {
            self.direction(index, enabled, false);
        }
    }

    /// The finger on the screen, translated into an egui pointer.
    fn touch(&mut self, motion: &android_activity::input::MotionEvent) {
        let index = motion.pointer_index();
        let pointer = motion.pointer_at_index(index);
        if pointer.tool_type() == ToolType::Palm {
            return;
        }
        let position = egui::pos2(
            pointer.axis_value(Axis::X) / self.pixels_per_point,
            pointer.axis_value(Axis::Y) / self.pixels_per_point,
        );

        match motion.action() {
            MotionAction::Down | MotionAction::PointerDown => {
                // egui decides the click by where the pointer **was**: without this
                // `PointerMoved` first, the first touch on a new screen hits nothing.
                self.events.push(egui::Event::PointerMoved(position));
                self.events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            MotionAction::Move => {
                self.events.push(egui::Event::PointerMoved(position));
            }
            MotionAction::Up | MotionAction::PointerUp => {
                self.events.push(egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                });
                // Without this egui thinks the finger is still hovering and leaves the button lit.
                self.events.push(egui::Event::PointerGone);
            }
            MotionAction::Cancel => {
                self.events.push(egui::Event::PointerGone);
            }
            _ => {}
        }
    }
}
