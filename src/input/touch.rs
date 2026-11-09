//! The controls drawn on a phone's screen: where each piece sits and what the finger presses.
//!
//! It lives in the core, and not in the Android frontend, for two reasons. The Android package
//! only compiles for Android, and the computation of which button the finger hit is the part worth
//! testing; and iOS will need the same computation. Drawing and reading the touch event stay with
//! each frontend.
//!
//! Everything here is in interface **points**, with the origin at the top-left corner and `y`
//! growing downward — the same space as egui's.

use std::collections::BTreeMap;

use super::DPAD;

/// A piece of the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchControl {
    Dpad,
    /// The left stick, on the `X` and `Y` axes.
    LeftStick,
    /// The right stick, on the `Z` and `RZ` axes.
    RightStick,
    B1,
    B2,
    B3,
    B4,
    Zl,
    Zr,
    Home,
    /// Player 1's turbo key. See [`crate::speed::turbo`].
    Turbo,
    /// Fast-forward. See [`crate::speed::FastForward`].
    FastForward,
    /// Rewind. See [`crate::speed::rewind`].
    Rewind,
}

impl TouchControl {
    pub const ALL: [Self; 13] = [
        Self::Dpad,
        Self::LeftStick,
        Self::RightStick,
        Self::B1,
        Self::B2,
        Self::B3,
        Self::B4,
        Self::Zl,
        Self::Zr,
        Self::Home,
        Self::Turbo,
        Self::FastForward,
        Self::Rewind,
    ];

    /// The piece's key in the saved positions and sizes. It never changes: it is what is in
    /// `settings.json`.
    ///
    /// **The four face buttons share a single key**, `botoes`: in the editor they are a group,
    /// dragged and resized as a whole. Separate, tidying up the diamond took four drags lined up
    /// by hand, and it never became a diamond again.
    pub fn key_id(self) -> &'static str {
        match self {
            Self::Dpad => "dpad",
            Self::LeftStick => "lstick",
            Self::RightStick => "rstick",
            Self::B1 | Self::B2 | Self::B3 | Self::B4 => "botoes",
            Self::Zl => "zl",
            Self::Zr => "zr",
            Self::Home => "home",
            Self::Turbo => "turbo",
            Self::FastForward => "avancar",
            Self::Rewind => "voltar",
        }
    }

    /// Where the button sits in the diamond, in radii from its center. The position is the
    /// controller's: 1 at the bottom, 2 on the left, 3 at the top and 4 on the right (issue #41).
    fn in_diamond(self) -> Option<[f32; 2]> {
        match self {
            Self::B1 => Some([0.0, 1.0]),
            Self::B2 => Some([-1.0, 0.0]),
            Self::B3 => Some([0.0, -1.0]),
            Self::B4 => Some([1.0, 0.0]),
            _ => None,
        }
    }

    /// What is written on the piece — the same as what is printed on the Zeebo controller.
    pub fn label(self) -> &'static str {
        match self {
            Self::Dpad | Self::LeftStick | Self::RightStick => "",
            Self::B1 => "1",
            Self::B2 => "2",
            Self::B3 => "3",
            Self::B4 => "4",
            Self::Zl => "ZL",
            Self::Zr => "ZR",
            Self::Home => "HOME",
            Self::Turbo => "TURBO",
            Self::FastForward => "▶▶",
            Self::Rewind => "◀◀",
        }
    }

    /// The [`super::Pad`] button the piece presses. The D-pad presses four, and so it has no
    /// single one; the sticks press no button at all.
    ///
    /// The indices are the same ones the Android frontend gives the physical controller: ZL and
    /// ZR are the "upper" ones on each side, and HOME takes `Back` — see [`super::BUTTON_UIDS`].
    pub fn index(self) -> Option<usize> {
        match self {
            // Turbo, fast-forward and rewind are not console buttons: see [`shortcuts`].
            Self::Dpad
            | Self::LeftStick
            | Self::RightStick
            | Self::Turbo
            | Self::FastForward
            | Self::Rewind => None,
            Self::B1 => Some(0),
            Self::B2 => Some(1),
            Self::B3 => Some(2),
            Self::B4 => Some(3),
            Self::Zl => Some(6),
            Self::Zr => Some(4),
            Self::Home => Some(9),
        }
    }

    /// The two [`super::Pad`] axes the stick moves, horizontal and vertical. In the order of
    /// [`super::Pad::axes`]: `X`, `Y`, `Z`, `RZ`.
    pub fn axes(self) -> Option<[usize; 2]> {
        match self {
            Self::LeftStick => Some([0, 1]),
            Self::RightStick => Some([2, 3]),
            _ => None,
        }
    }

    /// Round or pill-shaped. The triggers are pills, as on the controller, and so are the
    /// shortcut pieces: they are not Zeebo buttons, and should not look like them.
    pub fn is_round(self) -> bool {
        !matches!(self, Self::Zl | Self::Zr | Self::Turbo | Self::FastForward | Self::Rewind)
    }

    /// Half the width and the height, in points, at the factory size.
    ///
    /// The face buttons are 60 points in diameter, above the 48 Android asks for a touch target:
    /// during play the thumb does not look where it is going.
    fn middle(self) -> [f32; 2] {
        match self {
            Self::Dpad => [72.0, 72.0],
            Self::LeftStick | Self::RightStick => [50.0, 50.0],
            Self::B1 | Self::B2 | Self::B3 | Self::B4 => [30.0, 30.0],
            Self::Zl | Self::Zr => [44.0, 22.0],
            Self::Home | Self::FastForward | Self::Rewind => [30.0, 18.0],
            Self::Turbo => [34.0, 18.0],
        }
    }
}

/// How much of the edge is left free around the pieces, in the factory layout.
const MARGIN: f32 = 28.0;

/// The distance from the center of the button diamond to the center of each button.
const DIAMOND_RADIUS: f32 = 62.0;

/// The gap between neighboring pieces in the factory layout.
const GAP: f32 = 16.0;

/// How far beyond the drawing a touch still counts. The thumb slides during play, and a button
/// that only obeys inside the painted circle seems to fail.
const TOLERANCE: f32 = 1.15;

/// On the D-pad the slack is larger: it is the piece the finger holds while dragging.
const DPAD_TOLERANCE: f32 = 1.35;

/// Near the center of the D-pad no direction counts. Without this, a thumb resting on it would
/// change direction with every tremor.
const DEAD_CENTER: f32 = 0.2;

/// Below this, as a fraction of the radius, the stick is at the center. It is the same dead zone
/// as the physical stick's: a thumb resting on it is never truly still.
const STICK_DEAD_ZONE: f32 = 0.12;

/// `sin(22.5°)`: with it each direction counts within a 135° sector, and the four together cut
/// the circle into eight 45° slices — four straight and four diagonal, as on a real D-pad.
const DIAGONAL_THRESHOLD: f32 = 0.382_683_43;

/// A piece placed on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Element {
    pub touch_control: TouchControl,
    pub center: [f32; 2],
    /// Half the width and the height, already at the chosen scale.
    pub middle: [f32; 2],
}

/// Where each piece sits by default, in points, on a screen of the given size.
///
/// The factory layout is expressed **from the edges**, not as a fraction of the screen: the
/// button diamond has to stay a diamond on a long phone and on an almost square tablet.
fn factory_position(touch_control: TouchControl, screen: [f32; 2], scale: f32) -> [f32; 2] {
    let [width, height] = screen;
    let middle = touch_control.middle().map(|v| v * scale);
    let diamond = [
        width - MARGIN - (DIAMOND_RADIUS + 30.0) * scale,
        height - MARGIN - (DIAMOND_RADIUS + 30.0) * scale,
    ];
    let step = DIAMOND_RADIUS * scale;
    let dpad = TouchControl::Dpad.middle()[0] * scale;
    let button = TouchControl::B2.middle()[0] * scale;
    match touch_control {
        TouchControl::Dpad => [MARGIN + middle[0], height - MARGIN - middle[1]],
        // The sticks at the bottom, inward of the D-pad and the diamond, as on the controller:
        // the D-pad and the buttons sit within the thumb's natural reach, and the stick a little
        // below and toward the center.
        TouchControl::LeftStick => [
            MARGIN + 2.0 * dpad + GAP * scale + middle[0],
            height - MARGIN - middle[1],
        ],
        TouchControl::RightStick => [
            diamond[0] - step - button - GAP * scale - middle[0],
            height - MARGIN - middle[1],
        ],
        // For the buttons, the factory position is the center of the diamond; each one is placed
        // from there in [`build`], which is what knows the group's size.
        TouchControl::B1 | TouchControl::B2 | TouchControl::B3 | TouchControl::B4 => diamond,
        TouchControl::Zl => [MARGIN + middle[0], MARGIN + middle[1]],
        TouchControl::Zr => [width - MARGIN - middle[0], MARGIN + middle[1]],
        // At the top, in the middle: the bottom is where the sticks live.
        TouchControl::Home => [width / 2.0, MARGIN / 2.0 + middle[1]],
        // Rewind and fast-forward flank HOME, away from the thumbs: they are session shortcuts,
        // and an accidental touch in the middle of a fight would rewind or speed up the game.
        TouchControl::Rewind | TouchControl::FastForward => {
            let home = TouchControl::Home.middle()[0] * scale;
            let side = match touch_control {
                TouchControl::Rewind => -1.0,
                _ => 1.0,
            };
            [width / 2.0 + side * (home + GAP * scale + middle[0]), MARGIN / 2.0 + middle[1]]
        }
        // Turbo sits above the diamond, within reach of the thumb that presses the buttons.
        TouchControl::Turbo => [diamond[0], diamond[1] - step - button - GAP * scale - middle[1]],
    }
}

/// The smallest and largest size of a piece in the editor, in percent. Below half the button
/// becomes smaller than the touch target Android asks for; above double, the D-pad alone exceeds
/// a third of the height of a phone in landscape.
pub const MIN_SIZE: u8 = 50;
pub const MAX_SIZE: u8 = 200;

/// Places the pieces on a screen of the given size.
///
/// `scale` is in percent of the factory size, and applies to all of them. `sizes` is each piece's,
/// by [`TouchControl::key_id`], in percent, on top of `scale`; a piece not in there stays at 100.
/// `positions` is what the user dragged, as a fraction of the screen; a piece not in there stays
/// in its factory position. Whatever the saved position, the piece ends up entirely inside the
/// screen: a file made on a tablet and opened on a phone must not leave a button outside.
pub fn build(
    screen: [f32; 2],
    scale: u8,
    positions: &BTreeMap<String, [f32; 2]>,
    sizes: &BTreeMap<String, u8>,
) -> Vec<Element> {
    let scale = f32::from(scale.max(1)) / 100.0;
    TouchControl::ALL
        .iter()
        .map(|&touch_control| {
            // The factory position is computed with the overall scale, not the piece's: growing
            // the D-pad must not push the sticks out of place.
            let own = sizes
                .get(touch_control.key_id())
                .map_or(100, |t| (*t).clamp(MIN_SIZE, MAX_SIZE));
            let size = scale * f32::from(own) / 100.0;
            let middle = touch_control.middle().map(|v| v * size);
            let center = match positions.get(touch_control.key_id()) {
                Some(&[fx, fy]) => [fx * screen[0], fy * screen[1]],
                None => factory_position(touch_control, screen, scale),
            };
            let center = match touch_control.in_diamond() {
                // For a button, the saved center is the diamond's, and what has to fit is the
                // whole diamond: clamping each button on its own would distort it against the
                // edge.
                Some([dx, dy]) => {
                    let step = DIAMOND_RADIUS * size;
                    let reach = [step + middle[0], step + middle[1]];
                    [
                        clamp_inside(center[0], reach[0], screen[0]) + dx * step,
                        clamp_inside(center[1], reach[1], screen[1]) + dy * step,
                    ]
                }
                None => [
                    clamp_inside(center[0], middle[0], screen[0]),
                    clamp_inside(center[1], middle[1], screen[1]),
                ],
            };
            Element { touch_control, center, middle }
        })
        .collect()
}

/// Clamps a coordinate so the piece fits; if the screen is smaller than the piece, it goes in the
/// middle.
fn clamp_inside(value: f32, middle: f32, limit: f32) -> f32 {
    match limit > 2.0 * middle {
        true => value.clamp(middle, limit - middle),
        false => limit / 2.0,
    }
}

/// The inverse of [`build`] for one piece: from a center in points to the fraction that is saved.
pub fn fraction(center: [f32; 2], screen: [f32; 2]) -> [f32; 2] {
    [
        (center[0] / screen[0].max(1.0)).clamp(0.0, 1.0),
        (center[1] / screen[1].max(1.0)).clamp(0.0, 1.0),
    ]
}

/// The buttons the fingers press, one bit per [`super::Pad`] index.
///
/// It keeps no state between calls: Android sends, in every touch event, the position of every
/// finger on the screen, and what is pressed is purely a function of where they are. That way a
/// finger that slides from one button to its neighbor switches the press without having to lift.
pub fn buttons(elements: &[Element], fingers: impl IntoIterator<Item = [f32; 2]>) -> u32 {
    let mut bits = 0;
    for finger in fingers {
        for element in elements {
            bits |= pressed_by(element, finger);
        }
    }
    bits
}

/// A piece and a finger: what it presses on it.
fn pressed_by(element: &Element, finger: [f32; 2]) -> u32 {
    let dx = finger[0] - element.center[0];
    let dy = finger[1] - element.center[1];
    let [mx, my] = element.middle;
    match element.touch_control {
        TouchControl::LeftStick | TouchControl::RightStick => 0,
        TouchControl::Dpad => {
            let distance = dx.hypot(dy);
            if distance > mx * DPAD_TOLERANCE || distance < mx * DEAD_CENTER {
                return 0;
            }
            let limit = distance * DIAGONAL_THRESHOLD;
            let [up, down, left, right] = DPAD;
            [
                (dy < -limit, up),
                (dy > limit, down),
                (dx < -limit, left),
                (dx > limit, right),
            ]
            .into_iter()
            .filter(|(applies, _)| *applies)
            .fold(0, |bits, (_, index)| bits | 1 << index)
        }
        touch_control => {
            let inside = match touch_control.is_round() {
                // An ellipse, which is the circle when both halves are equal — HOME is wider than
                // it is tall.
                true => (dx / (mx * TOLERANCE)).powi(2) + (dy / (my * TOLERANCE)).powi(2) <= 1.0,
                false => dx.abs() <= mx * TOLERANCE && dy.abs() <= my * TOLERANCE,
            };
            match (inside, touch_control.index()) {
                (true, Some(index)) => 1 << index,
                _ => 0,
            }
        }
    }
}

/// The shortcut pieces the fingers hold: turbo, fast-forward and rewind. Like [`buttons`], it is
/// purely a function of where the fingers are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shortcuts {
    pub turbo: bool,
    pub fast_forward: bool,
    pub rewind: bool,
}

pub fn shortcuts(elements: &[Element], fingers: impl IntoIterator<Item = [f32; 2]>) -> Shortcuts {
    let mut shortcuts = Shortcuts::default();
    for finger in fingers {
        for element in elements {
            let dx = finger[0] - element.center[0];
            let dy = finger[1] - element.center[1];
            let [mx, my] = element.middle;
            let inside = dx.abs() <= mx * TOLERANCE && dy.abs() <= my * TOLERANCE;
            match element.touch_control {
                TouchControl::Turbo => shortcuts.turbo |= inside,
                TouchControl::FastForward => shortcuts.fast_forward |= inside,
                TouchControl::Rewind => shortcuts.rewind |= inside,
                _ => {}
            }
        }
    }
    shortcuts
}

/// The stick a finger that has just touched down grabbed, if it grabbed one.
///
/// The question is only asked on touch-down: from then on the finger belongs to the stick until
/// it lifts, whether it leaves the circle or not. Without that, pushing hard to the edge would make
/// the finger leave the piece and the stick return to center in the middle of a turn.
pub fn grab_stick(elements: &[Element], finger: [f32; 2]) -> Option<TouchControl> {
    elements.iter().find_map(|element| {
        element.touch_control.axes()?;
        let dx = finger[0] - element.center[0];
        let dy = finger[1] - element.center[1];
        (dx.hypot(dy) <= element.middle[0] * TOLERANCE).then_some(element.touch_control)
    })
}

/// Where the finger puts the stick: horizontal and vertical, from `-1` to `1`, with `y` growing
/// downward — which is also the direction of [`super::Pad`]'s internal axis, where up is negative.
///
/// Outside the circle, the stick stays at the edge, in the finger's direction.
pub fn stick(element: &Element, finger: [f32; 2]) -> [f32; 2] {
    let radius = element.middle[0].max(1.0);
    let x = (finger[0] - element.center[0]) / radius;
    let y = (finger[1] - element.center[1]) / radius;
    let size = x.hypot(y);
    if size < STICK_DEAD_ZONE {
        return [0.0, 0.0];
    }
    let cutoff = size.max(1.0);
    [x / cutoff, y / cutoff]
}

/// The center of what the editor drags when it grabs the piece: the diamond's, for a face button,
/// and the piece's own for the rest. It is the point saved through [`fraction`].
pub fn group_center(elements: &[Element], touch_control: TouchControl) -> Option<[f32; 2]> {
    let group: Vec<_> = elements
        .iter()
        .filter(|e| e.touch_control.key_id() == touch_control.key_id())
        .collect();
    if group.is_empty() {
        return None;
    }
    let n = group.len() as f32;
    Some([
        group.iter().map(|e| e.center[0]).sum::<f32>() / n,
        group.iter().map(|e| e.center[1]).sum::<f32>() / n,
    ])
}

/// The piece under a point, for the editor to know which one the finger grabbed. No slack: in the
/// editor nobody is playing, and the slack would grab the neighboring piece.
pub fn under(elements: &[Element], point: [f32; 2]) -> Option<TouchControl> {
    // Back to front: whatever was drawn last is on top.
    elements.iter().rev().find_map(|element| {
        let dx = (point[0] - element.center[0]).abs();
        let dy = (point[1] - element.center[1]).abs();
        (dx <= element.middle[0] && dy <= element.middle[1]).then_some(element.touch_control)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ordinary phone in landscape: 20:9, in points.
    const SCREEN: [f32; 2] = [915.0, 412.0];

    fn factory() -> Vec<Element> {
        build(SCREEN, 100, &BTreeMap::new(), &BTreeMap::new())
    }

    fn center(elements: &[Element], touch_control: TouchControl) -> [f32; 2] {
        elements.iter().find(|e| e.touch_control == touch_control).unwrap().center
    }

    fn bit(index: usize) -> u32 {
        1 << index
    }

    #[test]
    fn each_button_presses_its_own_index() {
        let elements = factory();
        for touch_control in TouchControl::ALL {
            let Some(index) = touch_control.index() else { continue };
            assert_eq!(
                buttons(&elements, [center(&elements, touch_control)]),
                bit(index),
                "{touch_control:?}"
            );
        }
    }

    #[test]
    fn factory_touch_controls_fit_and_do_not_overlap() {
        for screen in [SCREEN, [1024.0, 768.0], [640.0, 360.0]] {
            let elements = build(screen, 100, &BTreeMap::new(), &BTreeMap::new());
            for (i, a) in elements.iter().enumerate() {
                assert!(a.center[0] - a.middle[0] >= 0.0 && a.center[0] + a.middle[0] <= screen[0]);
                assert!(a.center[1] - a.middle[1] >= 0.0 && a.center[1] + a.middle[1] <= screen[1]);
                for b in &elements[i + 1..] {
                    let apart = (a.center[0] - b.center[0]).abs() >= a.middle[0] + b.middle[0]
                        || (a.center[1] - b.center[1]).abs() >= a.middle[1] + b.middle[1];
                    assert!(apart, "{:?} and {:?} on {screen:?}", a.touch_control, b.touch_control);
                }
            }
        }
    }

    /// The shortcut pieces press no console button, and a finger on them is read separately.
    #[test]
    fn shortcut_touch_controls_press_no_button() {
        let elements = factory();
        for touch_control in [TouchControl::Turbo, TouchControl::FastForward, TouchControl::Rewind] {
            let point = center(&elements, touch_control);
            assert_eq!(buttons(&elements, [point]), 0, "{touch_control:?}");
        }
        let got = shortcuts(&elements, [center(&elements, TouchControl::FastForward)]);
        assert_eq!(got, Shortcuts { fast_forward: true, ..Shortcuts::default() });
        let got = shortcuts(
            &elements,
            [center(&elements, TouchControl::Turbo), center(&elements, TouchControl::Rewind)],
        );
        assert_eq!(got, Shortcuts { turbo: true, rewind: true, fast_forward: false });
        assert_eq!(shortcuts(&elements, [center(&elements, TouchControl::B1)]), Shortcuts::default());
    }

    #[test]
    fn the_dpad_has_eight_directions_and_a_dead_center() {
        let elements = factory();
        let [x, y] = center(&elements, TouchControl::Dpad);
        let [up, down, left, right] = DPAD;
        assert_eq!(buttons(&elements, [[x, y]]), 0);
        assert_eq!(buttons(&elements, [[x, y - 50.0]]), bit(up));
        assert_eq!(buttons(&elements, [[x - 50.0, y]]), bit(left));
        assert_eq!(buttons(&elements, [[x + 35.0, y + 35.0]]), bit(down) | bit(right));
        // At 20° from the horizontal it is still straight: the diagonal only starts at 22.5°.
        let (s, c) = 20f32.to_radians().sin_cos();
        assert_eq!(buttons(&elements, [[x + 50.0 * c, y - 50.0 * s]]), bit(right));
        // Outside the drawing, but within the slack, it still counts.
        assert_eq!(buttons(&elements, [[x, y + 90.0]]), bit(down));
        assert_eq!(buttons(&elements, [[x, y + 110.0]]), 0);
    }

    #[test]
    fn two_fingers_add_up() {
        let elements = factory();
        let [x, y] = center(&elements, TouchControl::Dpad);
        let b1 = center(&elements, TouchControl::B1);
        assert_eq!(
            buttons(&elements, [[x, y - 50.0], b1]),
            bit(DPAD[0]) | bit(0)
        );
    }

    #[test]
    fn the_stick_follows_the_finger_and_stops_at_the_edge() {
        let elements = factory();
        let left = *elements.iter().find(|e| e.touch_control == TouchControl::LeftStick).unwrap();
        let [x, y] = left.center;
        assert_eq!(stick(&left, [x, y]), [0.0, 0.0]);
        assert_eq!(stick(&left, [x + 25.0, y]), [0.5, 0.0]);
        // Up is negative, as on the console's axis.
        assert_eq!(stick(&left, [x, y - 50.0]), [0.0, -1.0]);
        assert_eq!(stick(&left, [x, y + 300.0]), [0.0, 1.0]);
        let [dx, dy] = stick(&left, [x + 100.0, y - 100.0]);
        assert!((dx.hypot(dy) - 1.0).abs() < 1e-5 && dx > 0.0 && dy < 0.0);
        // A tremor at the center moves nothing.
        assert_eq!(stick(&left, [x + 4.0, y]), [0.0, 0.0]);
    }

    #[test]
    fn only_the_stick_grabs_the_finger_and_it_presses_no_button() {
        let elements = factory();
        let right = center(&elements, TouchControl::RightStick);
        assert_eq!(grab_stick(&elements, right), Some(TouchControl::RightStick));
        assert_eq!(grab_stick(&elements, center(&elements, TouchControl::B1)), None);
        assert_eq!(buttons(&elements, [right]), 0);
        assert_eq!(TouchControl::RightStick.axes(), Some([2, 3]));
    }

    #[test]
    fn far_from_everything_presses_nothing() {
        assert_eq!(buttons(&factory(), [[SCREEN[0] / 2.0, SCREEN[1] / 3.0]]), 0);
    }

    #[test]
    fn the_saved_position_is_a_fraction_and_the_touch_control_stays_inside() {
        let mut positions = BTreeMap::new();
        positions.insert("botoes".to_string(), [0.5, 0.5]);
        positions.insert("zl".to_string(), [0.0, 0.0]);
        let elements = build(SCREEN, 100, &positions, &BTreeMap::new());
        // The whole group went to the middle: 1 sits one radius below the center.
        assert_eq!(center(&elements, TouchControl::B1), [457.5, 268.0]);
        assert_eq!(group_center(&elements, TouchControl::B3), Some([457.5, 206.0]));
        // Saved in the corner, drawn against it.
        assert_eq!(center(&elements, TouchControl::Zl), [44.0, 22.0]);
        assert_eq!(fraction([457.5, 206.0], SCREEN), [0.5, 0.5]);
    }

    #[test]
    fn the_scale_grows_the_touch_control_and_its_reach() {
        let normal = factory();
        let [x, y] = center(&normal, TouchControl::B1);
        assert_eq!(buttons(&normal, [[x + 40.0, y]]), 0);
        let largest = build(SCREEN, 150, &BTreeMap::new(), &BTreeMap::new());
        let [x, y] = center(&largest, TouchControl::B1);
        assert_eq!(buttons(&largest, [[x + 40.0, y]]), bit(0));
    }

    #[test]
    fn one_touch_control_size_leaves_the_others_alone_and_is_limited() {
        let mut sizes = BTreeMap::new();
        sizes.insert("botoes".to_string(), 150);
        sizes.insert("dpad".to_string(), 10);
        let elements = build(SCREEN, 100, &BTreeMap::new(), &sizes);
        let middle = |touch_control| elements.iter().find(|e| e.touch_control == touch_control).unwrap().middle;
        assert_eq!(middle(TouchControl::B1), [45.0, 45.0]);
        assert_eq!(middle(TouchControl::B2), [45.0, 45.0]);
        assert_eq!(middle(TouchControl::Zl), [44.0, 22.0]);
        // Saved below the minimum, drawn at the minimum.
        assert_eq!(middle(TouchControl::Dpad), [36.0, 36.0]);
        // The diamond grows along with it, but the D-pad, which has another key, did not move.
        let [x, _] = center(&elements, TouchControl::B4);
        let [cx, _] = group_center(&elements, TouchControl::B4).unwrap();
        assert_eq!(x - cx, 93.0);
        assert_eq!(
            center(&elements, TouchControl::Dpad),
            center(&factory(), TouchControl::Dpad)
        );
        // And the overall scale multiplies the piece's.
        let largest = build(SCREEN, 150, &BTreeMap::new(), &sizes);
        assert_eq!(largest.iter().find(|e| e.touch_control == TouchControl::B1).unwrap().middle, [67.5, 67.5]);
    }

    #[test]
    fn the_whole_diamond_fits_even_when_saved_in_the_corner() {
        let mut positions = BTreeMap::new();
        positions.insert("botoes".to_string(), [1.0, 1.0]);
        let elements = build(SCREEN, 100, &positions, &BTreeMap::new());
        // 4 touches the right edge and 1 the bottom one; the diamond is still a diamond.
        assert_eq!(center(&elements, TouchControl::B4), [SCREEN[0] - 30.0, SCREEN[1] - 92.0]);
        assert_eq!(center(&elements, TouchControl::B1), [SCREEN[0] - 92.0, SCREEN[1] - 30.0]);
    }

    #[test]
    fn the_editor_picks_the_touch_control_under_the_finger() {
        let elements = factory();
        assert_eq!(under(&elements, center(&elements, TouchControl::Zr)), Some(TouchControl::Zr));
        assert_eq!(under(&elements, [SCREEN[0] / 2.0, SCREEN[1] / 3.0]), None);
    }
}
