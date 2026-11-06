//! The sound the screen pulls and the speaker thread consumes.
//!
//! `sceAudioOutOutput` blocks until the device accepts the block. Doing it on the game thread
//! holds the emulation to the speaker's clock. The ring is the go-between: the game pushes, the
//! thread pulls a fixed grain and blocks there.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

/// Samples per call on the MAIN port. Must be a multiple of 64, which is `SCE_AUDIO_MIN_LEN`.
pub const GRAIN: usize = 256;

/// A stereo grain aligned to 64 bytes.
///
/// `sceAudioOutOutput` reads this address by DMA. Without the alignment the kernel kills the
/// process, and that does not go through Rust's panic: the log ends at the last line there was
/// time to write.
#[repr(C, align(64))]
pub struct Block {
    pub samples: [i16; GRAIN * 2],
}

/// The MAIN port only accepts 48 kHz. The mixer delivers at this rate, and the device does not
/// resample.
pub const RATE: u32 = 48_000;

/// How many stereo frames to deliver in this step.
///
/// The speaker consumes the full rate on the wall clock. Delivering only the game's clock leaves
/// a gap when the frame runs late, and the music starts moving at the emulation's pace. The cap
/// of 150 ms is the core's on the Vita: it fits in a short ring, and a bigger spike would be
/// dropped at the far end.
pub fn samples_due(real_ms: u64, virtual_ms: u64, rate: u32) -> usize {
    let real = real_ms.min(150) * u64::from(rate) / 1000;
    let virt = virtual_ms.min(1_000) * u64::from(rate) / 1000;
    real.max(virt) as usize
}

pub struct Ring {
    internal: Mutex<VecDeque<i16>>,
    /// Interleaved samples, not frames. Always even.
    cap: usize,
}

impl Ring {
    pub fn create(frames: usize) -> Self {
        let frames = frames.max(1);
        Self {
            internal: Mutex::new(VecDeque::with_capacity(frames * 2)),
            cap: frames * 2,
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<i16>> {
        self.internal.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Pushes interleaved stereo PCM. What doesn't fit is left out: the ring holds what is still
    /// to play, and growing without a cap here is the soundtrack filling RAM when the thread
    /// falls behind.
    pub fn push(&self, pcm: &[i16]) -> usize {
        let mut queue = self.lock();
        let pairs = pcm.len() / 2 * 2;
        let free_slots = self.cap.saturating_sub(queue.len());
        let n = pairs.min(free_slots);
        queue.extend(pcm[..n].iter().copied());
        n / 2
    }

    /// Pulls into `dest` and pads with silence. The return value is in stereo frames.
    pub fn pull(&self, dest: &mut [i16]) -> usize {
        let mut queue = self.lock();
        let n = dest.len().min(queue.len());
        for (i, sample) in queue.drain(..n).enumerate() {
            dest[i] = sample;
        }
        dest[n..].fill(0);
        n / 2
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_clock_rules_when_the_game_lags() {
        assert_eq!(samples_due(10, 10, RATE), 480);
        assert_eq!(samples_due(1_000, 0, RATE), 7_200);
        assert_eq!(samples_due(0, 2_000, RATE), 48_000);
        assert_eq!(samples_due(0, 0, RATE), 0);
    }

    #[test]
    fn the_ring_keeps_pairs_and_pads_with_silence() {
        let ring = Ring::create(2);
        assert_eq!(ring.push(&[1]), 0, "a lone sample is not a frame");
        assert_eq!(ring.push(&[1, 2, 3, 4, 5, 6]), 2);
        let mut dest = [9i16; 6];
        assert_eq!(ring.pull(&mut dest), 2);
        assert_eq!(&dest[..4], &[1, 2, 3, 4]);
        assert_eq!(&dest[4..], &[0, 0]);
        ring.clear();
        assert_eq!(ring.pull(&mut dest), 0);
    }

    #[test]
    fn the_grain_is_a_multiple_of_the_port_minimum() {
        assert_eq!(GRAIN % 64, 0);
        assert!(GRAIN >= 64);
        assert!(std::mem::align_of::<Block>() >= 64);
        assert_eq!(std::mem::size_of::<Block>(), GRAIN * 2 * 2);
    }
}
