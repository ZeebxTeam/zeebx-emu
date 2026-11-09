//! The rewind: return points marked from time to time, and stepping back from point to point.
//!
//! **Points, not a smooth rewind.** A game state is 15 to 66 MB and takes 5 to 37 ms to save
//! (measured on 2026-10-10 on the CPU rasterizer: Double Dragon 14.8 MB, Crash 14–19 MB, Zeebo
//! Extreme Rolima 38–66 MB). Saving every frame, as RetroArch's rewind does, does not fit in
//! 16 ms. One point per second of game costs a hiccup of that size once per second, and that is
//! why the rewind ships turned off.
//!
//! **Only the copy runs in the game loop.** Compression — 26 ms on Double Dragon, 218 ms on Rolima
//! with fast deflate — goes to a separate thread, which stores the compressed point in the ring.
//! The ring has a memory ceiling, not a count ceiling: the same ceiling holds some 75 points of
//! Double Dragon and some 13 of Rolima. See `docs/implementation/24-speed.md`.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::session::Session;

/// The rewind as the user chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RewindSettings {
    #[serde(rename = "ligado")]
    pub enabled: bool,
    /// How much game time between marked points.
    #[serde(rename = "intervalo_ms")]
    pub interval_ms: u32,
    /// The ring's ceiling, in megabytes of compressed points.
    #[serde(rename = "memoria_mb")]
    pub memory_mb: u32,
}

impl RewindSettings {
    pub const INTERVALS_MS: [u32; 4] = [500, 1000, 2000, 5000];
    pub const MEMORY_SIZES_MB: [u32; 5] = [64, 128, 256, 512, 1024];

    /// Half on Android: a 3 GB device does not have the desktop's 256 MB to spare.
    #[cfg(target_os = "android")]
    pub const DEFAULT_MEMORY_MB: u32 = 128;
    #[cfg(not(target_os = "android"))]
    pub const DEFAULT_MEMORY_MB: u32 = 256;

    fn cap_in_bytes(&self) -> usize {
        self.memory_mb.clamp(16, 4096) as usize * 1024 * 1024
    }
}

impl Default for RewindSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_ms: 1000,
            memory_mb: Self::DEFAULT_MEMORY_MB,
        }
    }
}

/// A return point: the game clock at which it was marked and the compressed state.
struct Snapshot {
    clock_ms: u32,
    compressed: Vec<u8>,
}

/// The points, from oldest to newest, and how much they take up.
#[derive(Default)]
struct Ring {
    points: VecDeque<Snapshot>,
    bytes: usize,
    cap: usize,
    /// Changes on every [`Rewind::clear`] and on every point taken out: a point the thread
    /// finished compressing after that belongs to a timeline that no longer exists, and does not
    /// go in.
    ///
    /// **Taking a point out counts too.** What is in the thread when the game goes back was marked
    /// before the rewind, and for the restored point it is the future. Going into the ring
    /// afterwards, it became the newest, and the next rewind step took the game **forward** —
    /// measured on Magical Drop III: 30,023 ms, back to 29,000, and the next step back to 30,023.
    generation: u64,
}

impl Ring {
    fn guard(&mut self, point: Snapshot) {
        self.bytes += point.compressed.len();
        self.points.push_back(point);
        // The oldest goes until it fits. The newest stays even alone above the ceiling: one point
        // is better than none, and the ceiling is about what accumulates.
        while self.bytes > self.cap && self.points.len() > 1 {
            if let Some(old) = self.points.pop_front() {
                self.bytes -= old.compressed.len();
            }
        }
    }

    fn pop_newest(&mut self) -> Option<Snapshot> {
        let point = self.points.pop_back()?;
        self.bytes -= point.compressed.len();
        self.generation = self.generation.wrapping_add(1);
        Some(point)
    }
}

/// What goes to the compression thread: the ring's generation, the clock and the raw state.
type Request = (u64, u32, Vec<u8>);

/// The rewind of a running game.
pub struct Rewind {
    ring: Arc<Mutex<Ring>>,
    sender: Option<SyncSender<Request>>,
    /// The thread is compressing: a new point now would be saved only to wait in the queue, and
    /// the saving is what costs in the game loop. Better to skip the mark and try on the next pass.
    busy: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    settings_page: RewindSettings,
    /// The game clock at the last marked or restored point.
    last_ms: Option<u32>,
}

impl Rewind {
    pub fn create(settings_page: RewindSettings) -> Self {
        let ring = Arc::new(Mutex::new(Ring {
            cap: settings_page.cap_in_bytes(),
            ..Ring::default()
        }));
        let busy = Arc::new(AtomicBool::new(false));
        // One request in the queue, at most: the thread compresses one while the loop can already
        // mark the next, and more than that would be raw memory (up to 66 MB each) waiting.
        let (sender, receive) = std::sync::mpsc::sync_channel::<Request>(1);
        let worker = {
            let ring = Arc::clone(&ring);
            let busy = Arc::clone(&busy);
            std::thread::Builder::new()
                .name("zeebx-rewind".to_string())
                .spawn(move || compress_forever(receive, ring, busy))
                .ok()
        };
        Self {
            ring,
            sender: worker.is_some().then_some(sender),
            busy,
            worker,
            settings_page,
            last_ms: None,
        }
    }

    /// Changes the settings of an existing rewind: the interval applies on the next point, and
    /// the new ceiling evicts whatever exceeds it on the next one that comes in.
    pub fn apply_settings(&mut self, settings_page: RewindSettings) {
        self.settings_page = settings_page;
        if let Ok(mut ring) = self.ring.lock() {
            ring.cap = settings_page.cap_in_bytes();
        }
    }

    /// Marks a point if the interval has passed and the session allows it. Called after each game
    /// pass; does nothing on most of them.
    pub fn track(&mut self, session: &mut Session) {
        let Some(sender) = &self.sender else {
            return;
        };
        let now = session.clock_ms();
        // `wrapping_sub` because the clock of a restored point may be ahead of the current one
        // — and then the difference wraps around and marks right away, which is correct.
        let due = self
            .last_ms
            .is_none_or(|last| now.wrapping_sub(last) >= self.settings_page.interval_ms);
        if !due || self.busy.load(Ordering::Acquire) {
            return;
        }
        // A drawing in progress or a file open for writing: try on the next pass, without
        // restarting the interval.
        if session.can_mark_point().is_err() {
            return;
        }
        let generation = self.ring.lock().map(|ring| ring.generation).unwrap_or_default();
        // **The point carries the image of the instant.** Through the GPU, the last frame stays in
        // the texture until someone asks for it, and the texture does not go into the state:
        // without bringing it to the screen, the point came back with the screen of an old frame
        // — or black, in a game that only draws through GL. The game's context is current here:
        // the point is marked right after it advances.
        session.materialize_gl_frame();
        let state = session.save_state();
        self.busy.store(true, Ordering::Release);
        match sender.try_send((generation, now, state)) {
            Ok(()) => self.last_ms = Some(now),
            Err(TrySendError::Full(_)) => self.busy.store(false, Ordering::Release),
            Err(TrySendError::Disconnected(_)) => {
                self.busy.store(false, Ordering::Release);
                self.sender = None;
            }
        }
    }

    /// Goes back to the newest point, and takes it out of the ring. `false` when there is no point
    /// at all.
    ///
    /// A point the engine refuses — a file the game deleted afterwards, for example — leaves the
    /// ring and the previous one is tried: the player sees the rewind skip a point, and nothing
    /// breaks.
    pub fn step_back(&mut self, session: &mut Session) -> bool {
        loop {
            let Some(point) = self.ring.lock().ok().and_then(|mut ring| ring.pop_newest()) else {
                return false;
            };
            let state = match decompress(&point.compressed) {
                Ok(state) => state,
                Err(error) => {
                    crate::logging!(
                        crate::logging::Level::Warning,
                        "rewind",
                        "the {} ms point did not decompress: {error}",
                        point.clock_ms
                    );
                    continue;
                }
            };
            match session.restore_state(&state) {
                Ok(()) => {
                    self.last_ms = Some(point.clock_ms);
                    return true;
                }
                Err(error) => crate::logging!(
                    crate::logging::Level::Warning,
                    "rewind",
                    "the {} ms point was refused and left the ring: {error}",
                    point.clock_ms
                ),
            }
        }
    }

    /// Forgets all the points. It is what loading a save state asks: the points belong to another
    /// timeline.
    pub fn clear(&mut self) {
        if let Ok(mut ring) = self.ring.lock() {
            ring.points.clear();
            ring.bytes = 0;
            ring.generation = ring.generation.wrapping_add(1);
        }
        self.last_ms = None;
    }

    /// How many points there are now, and how much they take up.
    pub fn size(&self) -> (usize, usize) {
        self.ring
            .lock()
            .map(|ring| (ring.points.len(), ring.bytes))
            .unwrap_or_default()
    }

    /// Waits for the thread to finish the point in progress. Only for the tests, which need the
    /// point in the ring before going back to it.
    #[cfg(test)]
    fn wait_for_compression(&self) {
        while self.busy.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    }
}

impl Drop for Rewind {
    fn drop(&mut self) {
        // Closing the channel ends the thread's loop; waiting for it is waiting at most one point.
        self.sender = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// How much real time between points the held rewind steps back. With one point per second of
/// game, it is going back four times faster than it was played.
pub const REWIND_STEP: std::time::Duration = std::time::Duration::from_millis(250);

/// The rewind as a window uses it: the held shortcut, the step, the muted sound and the clock.
///
/// It lives here, and not in each frontend, because the desktop (through `ui::running_game`) and
/// Android do the same thing with the same shortcut.
#[derive(Default)]
pub struct RewindController {
    rewind: Option<Rewind>,
    /// The shortcut was pressed on the previous pass.
    before: bool,
    /// The game is going back: it does not advance, and fast-forward does not apply.
    rewinding: bool,
    /// When the next point can be restored, with the shortcut held.
    next: Option<std::time::Instant>,
    /// The last request to go back found no point at all.
    no_points: bool,
}

/// What a reading of the shortcut asks of the caller, beyond what was already done in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reading {
    /// Nothing new.
    Nothing,
    /// The shortcut was released: the real time spent going back is not for the game to chase,
    /// and whoever measures that time from outside (the window's slice) has to restart too.
    Released,
}

impl RewindController {
    /// Reads this pass's shortcut. It comes **before** fast-forward and the game step: while going
    /// back, the game does not advance.
    ///
    /// Held, it goes back one point every [`REWIND_STEP`] and shows its frame, with the sound
    /// muted; released, the game continues from the last point shown, and the newer ones have
    /// already left the ring. Paused, each press goes back one point and the game stays paused —
    /// it is the way to find the right point before releasing.
    pub fn read(
        &mut self,
        session: &mut Session,
        pressed: bool,
        paused: bool,
        settings_page: &RewindSettings,
    ) -> Reading {
        if !settings_page.enabled {
            // Turning it off midway releases the game and the sound, and drops the ring's memory.
            let released = self.rewinding;
            if released {
                session.mute(false);
                session.restart_clock();
            }
            *self = Self::default();
            return match released {
                true => Reading::Released,
                false => Reading::Nothing,
            };
        }
        let rewind = self.rewind.get_or_insert_with(|| Rewind::create(*settings_page));
        rewind.apply_settings(*settings_page);
        let went_down = pressed && !self.before;
        let released = !pressed && self.before;
        self.before = pressed;
        let now = std::time::Instant::now();
        let due = self.next.is_none_or(|next| now >= next);
        if pressed && (went_down || (!paused && due)) {
            self.no_points = !rewind.step_back(session);
            self.next = Some(now + REWIND_STEP);
        }
        if pressed && !self.rewinding {
            session.mute(true);
        }
        self.rewinding = pressed;
        if !released {
            return Reading::Nothing;
        }
        self.no_points = false;
        session.mute(false);
        session.restart_clock();
        Reading::Released
    }

    /// Marks a point, if it is time. Called after each pass in which the game advanced.
    pub fn track(&mut self, session: &mut Session) {
        if let Some(rewind) = &mut self.rewind {
            rewind.track(session);
        }
    }

    pub fn rewinding(&self) -> bool {
        self.rewinding
    }

    /// What the window writes over the game while it goes back: `Some(true)` when there are no
    /// points left.
    pub fn indicator(&self) -> Option<bool> {
        self.rewinding.then_some(self.no_points)
    }

    /// Forgets the points: a loaded save state is another timeline.
    pub fn forget_points(&mut self) {
        if let Some(rewind) = &mut self.rewind {
            rewind.clear();
        }
    }

    /// Forgets the shortcut, without dropping the points: the window's input restarted.
    pub fn release(&mut self) {
        self.before = false;
        self.rewinding = false;
        self.no_points = false;
    }
}

fn compress_forever(receive: Receiver<Request>, ring: Arc<Mutex<Ring>>, busy: Arc<AtomicBool>) {
    while let Ok((generation, clock_ms, state)) = receive.recv() {
        let compressed = compress(&state);
        drop(state);
        if let (Ok(compressed), Ok(mut ring)) = (compressed, ring.lock())
            && ring.generation == generation
        {
            ring.guard(Snapshot {
                clock_ms,
                compressed,
            });
        }
        busy.store(false, Ordering::Release);
    }
}

fn compress(state: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(state)?;
    encoder.finish()
}

fn decompress(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut state = Vec::new();
    flate2::read::DeflateDecoder::new(compressed).read_to_end(&mut state)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(clock_ms: u32, bytes: usize) -> Snapshot {
        Snapshot {
            clock_ms,
            compressed: vec![0; bytes],
        }
    }

    /// The ceiling evicts the oldest, and the newest stays even alone above it.
    #[test]
    fn the_ring_respects_the_cap_by_dropping_the_oldest() {
        let mut ring = Ring {
            cap: 100,
            ..Ring::default()
        };
        ring.guard(point(1, 40));
        ring.guard(point(2, 40));
        ring.guard(point(3, 40));
        let clocks: Vec<u32> = ring.points.iter().map(|p| p.clock_ms).collect();
        assert_eq!(clocks, [2, 3]);
        assert_eq!(ring.bytes, 80);

        ring.guard(point(4, 500));
        assert_eq!(ring.points.len(), 1, "the big one stays alone");
        assert_eq!(ring.pop_newest().map(|p| p.clock_ms), Some(4));
        assert_eq!(ring.bytes, 0);
        assert!(ring.pop_newest().is_none());
    }

    /// The point the thread was compressing when the game went back does not go in afterwards:
    /// for the restored point, it is the future, and going in it would be the next rewind step —
    /// forward.
    #[test]
    fn popping_a_snapshot_discards_the_one_in_the_thread() {
        let mut ring = Ring {
            cap: 1000,
            ..Ring::default()
        };
        ring.guard(point(29_000, 10));
        // The thread took this instant's generation for the 30,023 ms point.
        let requested_generation = ring.generation;
        assert_eq!(ring.pop_newest().map(|p| p.clock_ms), Some(29_000));
        assert_ne!(ring.generation, requested_generation, "the in-flight point would go in");
    }

    #[test]
    fn compress_and_decompress_round_trip() {
        let state: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let compressed = compress(&state).unwrap();
        assert!(compressed.len() < state.len());
        assert_eq!(decompress(&compressed).unwrap(), state);
    }

    #[test]
    fn the_factory_settings() {
        let settings_page = RewindSettings::default();
        assert!(!settings_page.enabled, "the rewind costs a hiccup per point: it ships off");
        assert_eq!(settings_page.interval_ms, 1000);
        assert!(RewindSettings::MEMORY_SIZES_MB.contains(&settings_page.memory_mb));
        let loaded: RewindSettings = serde_json::from_str(r#"{"ligado": true}"#).unwrap();
        assert!(loaded.enabled);
        assert_eq!(loaded.interval_ms, 1000);
    }

    /// With a real session: mark, the thread compresses, and going back returns the point's clock.
    #[test]
    fn mark_and_rewind_in_a_session() {
        let mut session = crate::session::tests::minimal_session_for_save_state();
        let mut rewind = Rewind::create(RewindSettings {
            enabled: true,
            ..RewindSettings::default()
        });
        rewind.track(&mut session);
        rewind.wait_for_compression();
        assert_eq!(rewind.size().0, 1);
        assert!(rewind.step_back(&mut session));
        assert_eq!(rewind.size().0, 0);
        assert!(!rewind.step_back(&mut session), "with no points, it does not go back");

        // Clearing also discards whatever the thread is still compressing.
        rewind.last_ms = None;
        rewind.track(&mut session);
        rewind.clear();
        rewind.wait_for_compression();
        assert_eq!(rewind.size().0, 0);
    }
}
