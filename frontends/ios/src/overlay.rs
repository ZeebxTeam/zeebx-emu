//! The controls drawn on screen: which fingers are down and what they press.
//!
//! Where each piece sits and what a finger hits is the core's, in [`zeebx::input::touch`] — the
//! same computation Android uses. What lives here is the part UIKit does differently: Android
//! sends every finger in every event, and UIKit sends only the ones that changed. So this keeps
//! the fingers, and what is pressed is recomputed from all of them each time one moves.

use zeebx::input::touch::{self, Element, TouchControl};
use zeebx::ui::settings::{OnScreenControls, OnScreenControlsMode};

/// The sticks, in the order of the [`Overlay::sticks`] slots.
const STICKS: [TouchControl; 2] = [TouchControl::LeftStick, TouchControl::RightStick];

#[derive(Debug, Default)]
pub struct Overlay {
    /// The buttons the fingers are pressing now, one bit per [`zeebx::input::Pad`] index. It is
    /// added to the physical controller, not written over it.
    pub buttons: u32,
    /// The shortcut pieces the fingers are holding: turbo, fast-forward and rewind.
    pub shortcuts: touch::Shortcuts,
    /// Where the sticks are, from `-1` to `1`, in the order of [`zeebx::input::Pad::axes`].
    pub axes: [f32; 4],
    /// The pieces on screen, already filtered by what the settings show.
    pub elements: Vec<Element>,
    /// The last press came from a physical controller. In automatic mode, this hides the pieces.
    physical_controller: bool,
    /// The fingers on the glass, by the id Swift gives each one.
    fingers: Vec<(i64, [f32; 2])>,
    /// The finger holding each stick. The finger that touched it owns it until it lifts, whether
    /// it leaves the circle or not: see [`touch::grab_stick`].
    sticks: [Option<i64>; 2],
    /// Fingers that touched down while the pieces were hidden. They bring the pieces back and
    /// press nothing: nobody aims at a button they cannot see.
    waking: Vec<i64>,
}

impl Overlay {
    /// Whether the pieces are on screen.
    pub fn visible(&self, settings: &OnScreenControls) -> bool {
        match settings.mode {
            OnScreenControlsMode::Always => true,
            OnScreenControlsMode::Auto => !self.physical_controller,
            OnScreenControlsMode::Never => false,
        }
    }

    /// A physical controller press arrived: in automatic mode, the pieces get out of the way.
    pub fn saw_controller(&mut self) {
        if !self.physical_controller {
            self.physical_controller = true;
            self.release();
        }
    }

    /// Releases everything. For when touch stops going to the game — the menu opened, the app
    /// left the screen — with a finger still on a piece.
    pub fn release(&mut self) {
        self.buttons = 0;
        self.shortcuts = touch::Shortcuts::default();
        self.axes = [0.0; 4];
        self.fingers.clear();
        self.sticks = [None; 2];
        self.waking.clear();
    }

    pub fn down(&mut self, id: i64, point: [f32; 2], settings: &OnScreenControls) {
        if settings.mode == OnScreenControlsMode::Never {
            return;
        }
        let hidden = !self.visible(settings);
        // Any touch brings the pieces back in automatic mode: it is how someone who put the
        // controller down says they want to play on the screen.
        self.physical_controller = false;
        if hidden {
            self.waking.push(id);
            return;
        }
        self.fingers.retain(|(known, _)| *known != id);
        self.fingers.push((id, point));
        // A finger that just touched down can grab a stick, if nobody holds it yet.
        if let Some(control) = touch::grab_stick(&self.elements, point)
            && let Some(slot) = STICKS.iter().position(|stick| *stick == control)
            && self.sticks[slot].is_none()
        {
            self.sticks[slot] = Some(id);
        }
        self.recompute();
    }

    pub fn moved(&mut self, id: i64, point: [f32; 2]) {
        let Some(finger) = self.fingers.iter_mut().find(|(known, _)| *known == id) else {
            return;
        };
        finger.1 = point;
        self.recompute();
    }

    pub fn up(&mut self, id: i64) {
        self.waking.retain(|known| *known != id);
        self.fingers.retain(|(known, _)| *known != id);
        for stick in &mut self.sticks {
            if *stick == Some(id) {
                *stick = None;
            }
        }
        self.recompute();
    }

    /// The pieces changed place or size: what the resting fingers press may have changed too.
    pub fn set_elements(&mut self, elements: Vec<Element>) {
        self.elements = elements;
        self.recompute();
    }

    fn recompute(&mut self) {
        self.axes = [0.0; 4];
        let mut free = Vec::new();
        for &(id, point) in &self.fingers {
            let Some(slot) = self.sticks.iter().position(|owner| *owner == Some(id)) else {
                free.push(point);
                continue;
            };
            let control = STICKS[slot];
            if let (Some(element), Some([horizontal, vertical])) = (
                self.elements.iter().find(|e| e.touch_control == control),
                control.axes(),
            ) {
                let [x, y] = touch::stick(element, point);
                self.axes[horizontal] = x;
                self.axes[vertical] = y;
            }
        }
        // A stick's finger does not press buttons, not even when it passes over one.
        self.shortcuts = touch::shortcuts(&self.elements, free.iter().copied());
        self.buttons = touch::buttons(&self.elements, free);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// An ordinary phone in landscape, in points.
    const SCREEN: [f32; 2] = [844.0, 390.0];

    fn overlay() -> Overlay {
        let mut overlay = Overlay::default();
        overlay.set_elements(touch::build(SCREEN, 100, &BTreeMap::new(), &BTreeMap::new()));
        overlay
    }

    fn center(overlay: &Overlay, control: TouchControl) -> [f32; 2] {
        overlay
            .elements
            .iter()
            .find(|element| element.touch_control == control)
            .unwrap()
            .center
    }

    fn always() -> OnScreenControls {
        OnScreenControls {
            mode: OnScreenControlsMode::Always,
            ..Default::default()
        }
    }

    #[test]
    fn two_fingers_press_two_buttons_and_each_lifts_its_own() {
        let mut overlay = overlay();
        let settings = always();
        overlay.down(1, center(&overlay, TouchControl::B1), &settings);
        overlay.down(2, center(&overlay, TouchControl::B4), &settings);
        assert_eq!(overlay.buttons, 0b1001);
        overlay.up(1);
        assert_eq!(overlay.buttons, 0b1000);
        overlay.up(2);
        assert_eq!(overlay.buttons, 0);
    }

    #[test]
    fn a_finger_slides_from_one_button_to_its_neighbour() {
        let mut overlay = overlay();
        let settings = always();
        overlay.down(7, center(&overlay, TouchControl::B1), &settings);
        overlay.moved(7, center(&overlay, TouchControl::B2));
        assert_eq!(overlay.buttons, 0b0010);
    }

    #[test]
    fn the_stick_keeps_its_finger_past_the_edge_and_presses_nothing() {
        let mut overlay = overlay();
        let settings = always();
        let stick = center(&overlay, TouchControl::LeftStick);
        overlay.down(3, stick, &settings);
        overlay.moved(3, [stick[0] + 400.0, stick[1]]);
        assert_eq!(overlay.axes[0], 1.0);
        assert_eq!(overlay.axes[1], 0.0);
        assert_eq!(overlay.buttons, 0, "the stick's finger crossed the screen over the buttons");
        overlay.up(3);
        assert_eq!(overlay.axes, [0.0; 4]);
    }

    #[test]
    fn in_automatic_mode_the_first_touch_only_brings_the_pieces_back() {
        let mut overlay = overlay();
        let settings = OnScreenControls::default();
        assert!(overlay.visible(&settings));
        overlay.saw_controller();
        assert!(!overlay.visible(&settings));

        overlay.down(1, center(&overlay, TouchControl::B1), &settings);
        assert!(overlay.visible(&settings));
        assert_eq!(overlay.buttons, 0, "the finger could not see what it hit");
        overlay.up(1);
        overlay.down(2, center(&overlay, TouchControl::B1), &settings);
        assert_eq!(overlay.buttons, 1);
    }

    #[test]
    fn never_means_the_glass_is_just_glass() {
        let mut overlay = overlay();
        let settings = OnScreenControls {
            mode: OnScreenControlsMode::Never,
            ..Default::default()
        };
        overlay.down(1, center(&overlay, TouchControl::B1), &settings);
        assert_eq!(overlay.buttons, 0);
    }

    #[test]
    fn the_shortcut_pieces_are_not_console_buttons() {
        let mut overlay = overlay();
        let settings = always();
        overlay.down(1, center(&overlay, TouchControl::Rewind), &settings);
        assert!(overlay.shortcuts.rewind);
        assert_eq!(overlay.buttons, 0);
        overlay.release();
        assert!(!overlay.shortcuts.rewind);
    }
}
