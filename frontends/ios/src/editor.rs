//! The screen for moving and resizing the on-screen controls.
//!
//! Dragging a piece moves it; touching it selects it, and the bar in the middle grows and shrinks
//! the selected one. Saves on release and at every size step. The rule is Android's, and so is
//! the math, which is the core's [`zeebx::input::touch`]: Swift only says where the finger is.

use zeebx::input::touch::{self, TouchControl};

use crate::App;

/// How much one press of `−` or `+` changes a piece, in percent.
const STEP: i32 = 10;

impl App {
    pub(crate) fn set_editing(&mut self, editing: bool) {
        self.editing = editing;
        self.dragging = None;
        self.chosen = None;
        self.overlay.release();
    }

    /// The finger touched down: the piece under it is selected, and grabbed.
    ///
    /// The piece is the one under the finger **when it touched down**. On empty space, the
    /// selection clears.
    pub(crate) fn editor_grab(&mut self, point: [f32; 2]) {
        let elements = &self.overlay.elements;
        self.dragging = touch::under(elements, point).and_then(|control| {
            // The center is the group's: grabbing 3 drags the whole diamond.
            let [x, y] = touch::group_center(elements, control)?;
            // Keeps where, inside the piece, the finger grabbed it: without this it would jump
            // to center on the finger.
            Some((control, [x - point[0], y - point[1]]))
        });
        self.chosen = self.dragging.map(|(control, _)| control);
    }

    pub(crate) fn editor_drag(&mut self, point: [f32; 2]) {
        let Some((control, [dx, dy])) = self.dragging else {
            return;
        };
        let [_, _, width, height] = self.safe;
        let center = [point[0] + dx, point[1] + dy];
        self.settings
            .on_screen_controls
            .positions
            .insert(control.key_id().to_string(), touch::fraction(center, [width, height]));
        // Placed again, but not saved: a drag is sixty of these a second, and the file is
        // written once, on release.
        self.place_elements();
    }

    pub(crate) fn editor_drop(&mut self) {
        if self.dragging.take().is_some() {
            self.changed();
        }
    }

    /// Grows or shrinks the selected piece by `steps` of ten percent.
    pub(crate) fn editor_resize(&mut self, steps: i32) {
        let Some(control) = self.chosen else {
            return;
        };
        let sizes = &mut self.settings.on_screen_controls.sizes;
        let current = i32::from(sizes.get(control.key_id()).copied().unwrap_or(100));
        let resized = (current + steps * STEP)
            .clamp(i32::from(touch::MIN_SIZE), i32::from(touch::MAX_SIZE));
        if resized == current {
            return;
        }
        // 100 is not saved: it is what absence already says, and the file keeps only what
        // changed.
        match resized {
            100 => sizes.remove(control.key_id()),
            _ => sizes.insert(control.key_id().to_string(), resized as u8),
        };
        self.changed();
    }

    /// Back to the factory layout: "restore" is emptying the two maps.
    pub(crate) fn editor_reset(&mut self) {
        let controls = &mut self.settings.on_screen_controls;
        controls.positions.clear();
        controls.sizes.clear();
        self.changed();
    }

    /// What the size bar says, like "D-pad  120%". Nothing when no piece is selected.
    pub(crate) fn editor_selection(&self) -> Option<String> {
        let control = self.chosen?;
        let name = match control {
            TouchControl::Dpad => self.catalog.get("touch.piece.dpad"),
            TouchControl::LeftStick => self.catalog.get("touch.piece.lstick"),
            TouchControl::RightStick => self.catalog.get("touch.piece.rstick"),
            TouchControl::B1 | TouchControl::B2 | TouchControl::B3 | TouchControl::B4 => {
                self.catalog.get("touch.piece.buttons")
            }
            other => other.label(),
        };
        let sizes = &self.settings.on_screen_controls.sizes;
        let size = sizes.get(control.key_id()).copied().unwrap_or(100);
        Some(format!("{name}  {size}%"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::test_app;

    fn editing(name: &str) -> (std::path::PathBuf, App) {
        let (root, mut app) = test_app(name);
        app.screen = [844.0, 390.0];
        app.safe = [0.0, 0.0, 844.0, 390.0];
        app.place_elements();
        app.set_editing(true);
        (root, app)
    }

    fn center(app: &App, control: TouchControl) -> [f32; 2] {
        touch::group_center(&app.overlay.elements, control).unwrap()
    }

    #[test]
    fn dragging_a_face_button_moves_the_whole_diamond_and_saves_on_release() {
        let (root, mut app) = editing("arrastar");
        let before = center(&app, TouchControl::B1);
        let grab = app
            .overlay
            .elements
            .iter()
            .find(|element| element.touch_control == TouchControl::B3)
            .unwrap()
            .center;
        app.editor_grab(grab);
        assert_eq!(app.chosen, Some(TouchControl::B3));
        app.editor_drag([grab[0] - 100.0, grab[1] - 40.0]);
        let after = center(&app, TouchControl::B1);
        assert!((after[0] - (before[0] - 100.0)).abs() < 0.01);
        assert!((after[1] - (before[1] - 40.0)).abs() < 0.01);
        assert!(!app.file.exists(), "nothing is written in the middle of a drag");
        app.editor_drop();
        assert!(app.file.exists());
        assert!(app.settings.on_screen_controls.positions.contains_key("botoes"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_size_stops_at_the_limits_and_a_hundred_is_not_stored() {
        let (root, mut app) = editing("tamanho");
        app.editor_grab(center(&app, TouchControl::Dpad));
        app.editor_drop();
        assert_eq!(app.editor_selection().as_deref(), Some("D-pad  100%"));
        app.editor_resize(2);
        assert_eq!(app.settings.on_screen_controls.sizes.get("dpad"), Some(&120));
        app.editor_resize(99);
        assert_eq!(app.settings.on_screen_controls.sizes.get("dpad"), Some(&touch::MAX_SIZE));
        app.editor_resize(-10);
        assert_eq!(app.settings.on_screen_controls.sizes.get("dpad"), None);
        app.editor_reset();
        assert!(app.settings.on_screen_controls.positions.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_touch_on_empty_space_clears_the_selection() {
        let (root, mut app) = editing("vazio");
        app.editor_grab(center(&app, TouchControl::Home));
        assert!(app.chosen.is_some());
        app.editor_drop();
        app.editor_grab([422.0, 195.0]);
        assert_eq!(app.chosen, None);
        assert_eq!(app.editor_selection(), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
