//! The Boomerang calibration notice: the corner of the game window that shows the controller
//! tilting while the game calibrates motion — see `docs/implementation/20-boomerang-and-wii-remote.md`.
//!
//! It came out of the egui `App` so the Qt window shows the same notice, at the same moment. What
//! lives here is when it opens, whether the controller is still, when it completes and goes away,
//! and how much the model rotates; drawing it is each window's job.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The open notice: the recent readings and where it stands.
struct Warning {
    opened_at: Instant,
    /// The latest readings, to tell whether the controller is still.
    recent: VecDeque<[f32; 3]>,
    still_since: Option<Instant>,
    completed_at: Option<Instant>,
}

impl Warning {
    /// How much the reading may vary with the controller still counting as still, in g.
    const TOLERANCE: f32 = 0.05;
    const READINGS: usize = 20;
    /// The animation until it is level, and how long the notice stays after it.
    const SETTLE: Duration = Duration::from_millis(400);
    const HOLD: Duration = Duration::from_millis(1200);
    /// A notice that never completes does not stay on screen forever.
    const MAX_OPEN: Duration = Duration::from_secs(30);

    fn create(now: Instant) -> Self {
        Self {
            opened_at: now,
            recent: VecDeque::new(),
            still_since: None,
            completed_at: None,
        }
    }

    /// Stores a reading and says whether the controller is still.
    fn sample(&mut self, reading: [f32; 3], now: Instant) -> bool {
        if self.recent.len() == Self::READINGS {
            self.recent.pop_front();
        }
        self.recent.push_back(reading);
        let stopped = self.recent.len() == Self::READINGS
            && (0..3).all(|axis| {
                let (smallest, largest) = self.recent.iter().fold((f32::MAX, f32::MIN), |(a, b), l| {
                    (a.min(l[axis]), b.max(l[axis]))
                });
                largest - smallest < Self::TOLERANCE
            });
        match (stopped, self.still_since) {
            (true, None) => self.still_since = Some(now),
            (false, _) => self.still_since = None,
            _ => {}
        }
        stopped
    }

    fn complete(&mut self, now: Instant) {
        self.completed_at.get_or_insert(now);
    }

    fn completion_progress(&self, now: Instant) -> f32 {
        self.completed_at.map_or(0.0, |at| {
            ((now - at).as_secs_f32() / Self::SETTLE.as_secs_f32()).clamp(0.0, 1.0)
        })
    }

    fn expired(&self, now: Instant) -> bool {
        now - self.opened_at > Self::MAX_OPEN
            || self
                .completed_at
                .is_some_and(|at| now - at > Self::SETTLE + Self::HOLD)
    }
}

/// What the port's sensor is doing, for the bottom line of the notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The port has no sensor reading: the window says which one and why.
    NoSensor,
    Still,
    Moving,
}

/// The notice as the window draws it in this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The game finished calibrating: the title changes and the bottom line goes away.
    pub done: bool,
    /// `None` when completed.
    pub status: Option<Status>,
    /// How much the Boomerang model rotates, in radians: the wheel's tilt, which goes to zero in a
    /// short animation when calibration completes — the model "settles".
    pub rotation: f32,
}

/// Tracking of the calibrations of a running game.
#[derive(Default)]
pub struct Calibration {
    warning: Option<Warning>,
    /// The started and finished calibration counts already seen. They belong to the session: a
    /// new game starts from zero.
    seen: (u32, u32),
}

impl Calibration {
    /// A new game opened.
    pub fn restart(&mut self) {
        *self = Self::default();
    }

    /// The notice in this frame, or `None` if there is no notice to show.
    ///
    /// `calibration` is the session's count, [`crate::session::Session::calibration`]. `reading` is
    /// the motion of the Boomerang port and whether its sensor is reading — `None` when no
    /// connected port is a Boomerang. `enabled` is the `motion.calibration_notice` option.
    pub fn frame(
        &mut self,
        calibration: (u32, u32),
        reading: Option<([f32; 3], bool)>,
        enabled: bool,
        now: Instant,
    ) -> Option<View> {
        let (started, finished) = calibration;
        let fresh = (started > self.seen.0, finished > self.seen.1);
        self.seen = calibration;
        let Some((motion, has_sensor)) = reading else {
            self.warning = None;
            return None;
        };
        if fresh.0 && enabled {
            self.warning = Some(Warning::create(now));
        }
        let warning = self.warning.as_mut()?;
        if fresh.1 {
            warning.complete(now);
        }
        // Still is only information: the one that says it calibrated is the game. Completing on
        // being still said "calibrated" while Crash Nitro Kart was still rejecting the readings.
        let stopped = warning.sample(motion, now);
        if warning.expired(now) {
            self.warning = None;
            return None;
        }
        let [x, y, _] = motion;
        let in_plane = (x * x + y * y).sqrt();
        let wheel_angle = x.atan2(y) * ((in_plane - 0.3) / 0.4).clamp(0.0, 1.0);
        let done = warning.completed_at.is_some();
        let status = match (has_sensor, stopped) {
            _ if done => None,
            (false, _) => Some(Status::NoSensor),
            (true, true) => Some(Status::Still),
            (true, false) => Some(Status::Moving),
        };
        Some(View {
            done,
            status,
            rotation: wheel_angle * (1.0 - warning.completion_progress(now)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REST: [f32; 3] = [0.0, 0.0, 1.0];

    /// The notice only opens when the game starts a calibration, and only with the option on.
    #[test]
    fn the_notice_opens_when_calibration_starts() {
        let now = Instant::now();
        let mut calibration = Calibration::default();
        assert_eq!(calibration.frame((0, 0), Some((REST, true)), true, now), None);
        assert!(calibration.frame((1, 0), Some((REST, true)), true, now).is_some());

        let mut off = Calibration::default();
        assert_eq!(off.frame((1, 0), Some((REST, true)), false, now), None);
    }

    /// Still only after twenty equal readings; completed is for the game to say, and the notice
    /// goes away after settling and holding.
    #[test]
    fn still_and_completed_are_different_things() {
        let start = Instant::now();
        let mut calibration = Calibration::default();
        let mut view = None;
        for i in 0..Warning::READINGS {
            let now = start + Duration::from_millis(i as u64 * 10);
            view = calibration.frame((1, 0), Some((REST, true)), true, now);
        }
        let view = view.expect("the notice is open");
        assert_eq!(view.status, Some(Status::Still));
        assert!(!view.done, "still is not calibrated");

        let completed = start + Duration::from_secs(1);
        let view = calibration.frame((1, 1), Some((REST, true)), true, completed);
        assert_eq!(view.map(|v| (v.done, v.status)), Some((true, None)));
        let after = completed + Warning::SETTLE + Warning::HOLD + Duration::from_millis(1);
        assert_eq!(calibration.frame((1, 1), Some((REST, true)), true, after), None);
    }

    /// Without a Boomerang port there is no notice, even with a calibration running.
    #[test]
    fn without_boomerang_there_is_no_notice() {
        let mut calibration = Calibration::default();
        assert_eq!(calibration.frame((1, 0), None, true, Instant::now()), None);
    }
}
