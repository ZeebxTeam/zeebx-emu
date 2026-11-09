//! Host sound output, and the mixing of the voices that play at the same time.
//!
//! The console mixes several sounds at once, and each one comes at the rate it was recorded at —
//! Peteca has sounds at 11025, 22050 and 44100 Hz in the same session. Whoever plays cannot care
//! about that, so the mixer resamples each voice to the device's rate and sums everything.
//!
//! The state sits behind a [`Mutex`] shared with the audio thread, which is what consumes it. It
//! runs in real time: nothing here allocates or blocks inside the mixing loop.

pub mod midi;
pub mod mp3;
pub mod wav;

/// MIDI synthesis backend requested by the user or the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiBackend {
    /// Default behavior: uses a SoundFont (.sf2) if available and valid; falls back to the
    /// wavetable if there is none.
    #[default]
    Auto,
    /// Forces the built-in wavetable synthesizer (fast start, without the heavy loading or
    /// rendering of a .sf2).
    Wavetable,
    /// Requires a SoundFont (.sf2); if there is none or it fails, warns and falls back with an
    /// explicit report.
    SoundFont,
}

impl std::str::FromStr for MidiBackend {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "automático" | "padrao" | "padrão" => Ok(Self::Auto),
            "timbres" | "tabela" | "tabela de timbres" | "synth" => Ok(Self::Wavetable),
            "soundfont" | "sf2" | "banco" => Ok(Self::SoundFont),
            _ => Err(()),
        }
    }
}

/// Sample bank synthesis. See the `soundfont` feature and the module header.
#[cfg(feature = "soundfont")]
pub mod soundfont;

/// Where the sample bank should go, when the build does not have the bank synthesizer.
///
/// The message exists in both cases on purpose: a build without the feature has to say that it
/// **does not have** the capability, and not stay silent — silence here turns into "the bank does
/// not work".
#[cfg(not(feature = "soundfont"))]
pub mod soundfont {
    use std::path::Path;

    /// The rate the bank would use, so the frontend has the same name in both cases.
    pub const BANK_RATE: u32 = 44_100;

    /// Without a bank there is nothing to configure; it exists so the frontend needs no `cfg`.
    pub fn set_rate(_rate: u32) {}

    /// Without a bank there is nothing to configure; it exists so the frontend needs no `cfg`.
    pub fn set_voices(_voices: usize) {}

    /// Without a bank there is nothing to configure; it exists so the frontend needs no `cfg`.
    pub fn set_soundfont(_path: Option<std::path::PathBuf>) {}

    /// Without a bank there is nothing to configure; it exists so the frontend needs no `cfg`.
    pub fn set_effects(_enabled: bool) {}

    pub fn report(device: &Path) -> String {
        let dir = device.join("soundfonts");
        format!(
            "Zeebx: this build does not have the sample bank synthesizer; MIDI plays with the \
             wavetable (the .sf2 would go in {})",
            dir.display()
        )
    }
}

use std::sync::{Arc, Mutex};

#[cfg(feature = "audio")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::wav::Sound;

/// A playing voice: the sound, where it is and how it comes out.
#[derive(Debug)]
struct Voice {
    sound: Arc<Sound>,
    /// Position in the sound's frames, fractional because its rate is rarely the device's.
    position: f64,
    /// How much to advance per device frame: the ratio between the two rates.
    step: f64,
    volume: f32,
    /// How many more times to play. `None` is forever, which is `MM_PARM_PLAY_REPEAT` zero.
    remaining: Option<u32>,
    paused: bool,
    done: bool,
    /// **Source** frames the voice has already consumed, summed since the start.
    ///
    /// It is the honest measure of how much the voice played: `position` wraps around when the
    /// sound repeats, and a device frame counter would lie about the duration. It answers "did
    /// this voice play everything the game sent?" — the question that separates a mixer defect
    /// from a defect in what comes after it.
    played: f64,
    /// Frames left for the voice to fade away, once the sound has reached its end.
    ///
    /// **A voice that vanishes at once pops.** The sound ends, the level may be at 0.947 — that was
    /// measured in Peteca — and the next sample is zero: a drop of almost the full range in one
    /// sample is an audible pop, not a property of the game's sound. The fade takes
    /// [`FADE_OUT_FRAMES`] frames, which at 44.1 kHz is less than two milliseconds: it vanishes
    /// with no step and without delaying the next effect.
    fade_out: u32,
}

/// How many frames the voice takes to fade away when the sound ends.
///
/// 64 frames at 44.1 kHz are 1.5 ms — below what the ear tells apart as a separate silence, and
/// enough for the step to stop existing.
const FADE_OUT_FRAMES: u32 = 64;

impl Voice {
    /// The sample of channel `channel` at the current position, interpolated between the two
    /// neighboring frames. Without the interpolation, resampling 11025 to 48000 hisses.
    fn sample(&self, channel: usize) -> f32 {
        let channels = self.sound.channels as usize;
        let frames = self.sound.frames();
        if frames == 0 {
            return 0.0;
        }
        let index = self.position.floor() as usize;
        let fraction = (self.position - self.position.floor()) as f32;
        // Mono sound feeds both channels; stereo sound uses the requested channel.
        let lane = channel.min(channels - 1);
        let needed = index.saturating_add(1) * channels.max(1) + lane;
        if !self.sound.reaches(needed) {
            return 0.0;
        }
        let at = |frame: usize| -> f32 { self.sound.read(frame.min(frames - 1) * channels + lane) };
        at(index) + (at(index + 1) - at(index)) * fraction
    }

    /// Advances one device frame, handling the end of the sound and the repetition. `pace` is the
    /// game's speed: see [`Mixer::set_fast_forward`].
    fn advance(&mut self, pace: f64) {
        let step = self.step * pace;
        self.position += step;
        self.played += step;
        if (self.position as usize) < self.sound.frames() {
            return;
        }
        match &mut self.remaining {
            None => self.position = 0.0,
            // The end of the sound does **not** cut: it holds the last value and fades out over
            // `FADE_OUT_FRAMES` frames. Repeating is not an end — the loop starts over from the
            // beginning and does not need this.
            Some(0) | Some(1) => {
                if self.fade_out == 0 {
                    self.fade_out = FADE_OUT_FRAMES;
                }
            }
            Some(left) => {
                *left -= 1;
                self.position = 0.0;
            }
        }
    }

    /// Moves one device frame: either the sound advances, or the fade consumes what is left of it.
    ///
    /// **Both mixing paths use this step** — the audible one and the muted one. The muted one also
    /// has to consume the fade: without it the voice would never reach its end there, and the test
    /// that demands "the sound ran to the end" caught exactly that.
    fn step(&mut self, pace: f64) {
        if self.fade_out == 0 {
            // A sample the synthesis has not written yet: stay put here. Advancing would play
            // silence and, when the buffer showed up, the music would start in the middle.
            let frame = self.position.floor() as usize;
            let channels = (self.sound.channels as usize).max(1);
            if !self.sound.reaches(frame.saturating_add(1) * channels) {
                return;
            }
        }
        match self.fade_out {
            0 => self.advance(pace),
            1 => {
                self.done = true;
                // **A voice that dies before the end of the sound is a cut**, and it is the exact
                // symptom being looked for: speech that stops in the middle. The warning only goes
                // out when the voice was not a repeating one and the difference exceeds one frame
                // — in the normal case it dies at the end and matches the total. A looping sound
                // never "ends", so there is nothing to warn about there.
                let (played, total) = (
                    self.played / f64::from(self.sound.rate.max(1)),
                    self.sound.frames() as f64 / f64::from(self.sound.rate.max(1)),
                );
                if self.remaining.is_some() && played + 0.02 < total {
                    crate::logging!(
                        crate::logging::Level::Warning,
                        "mixer",
                        "cut: the {:.2}s voice stopped after {:.2}s, {:.2}s short",
                        total,
                        played,
                        total - played
                    );
                }
            }
            remaining => self.fade_out = remaining - 1,
        }
    }

    /// How much this voice still counts, from 1.0 during the sound to 0.0 at the end of the fade.
    fn decay(&self) -> f32 {
        match self.fade_out {
            0 => 1.0,
            remaining => remaining as f32 / FADE_OUT_FRAMES as f32,
        }
    }
}

/// A voice the game feeds bit by bit: samples arrive while it plays.
///
/// It is the sound of Data East's arcade ports, which generate their emulator's audio frame by
/// frame and deliver it through an `ISource`. There is no known end or duration: what is missing
/// becomes silence until the next batch.
#[derive(Debug)]
struct Stream {
    /// Samples interleaved by channel, not played yet.
    samples: std::collections::VecDeque<f32>,
    channels: usize,
    /// How much to advance in the stream per device frame.
    step: f64,
    /// Where the device is between `previous` and `current`, from 0 to 1.
    fraction: f64,
    previous: [f32; 2],
    current: [f32; 2],
    volume: f32,
    paused: bool,
    /// The maximum of stored samples: if the game delivers faster than the device plays, the
    /// oldest ones go, instead of the delay growing without end.
    capacity: usize,
    /// How much a remaining sample drops per output frame when the stream runs dry.
    ///
    /// Holding the last sample during a short gap turns an underrun into DC voltage and sounds
    /// like a buzz/pop. A very short fade keeps continuity without making up audio.
    underrun_decay: f32,
}

impl Stream {
    /// Advances one device frame and returns the stereo frame of that instant. `pace` is the
    /// game's speed: at 3x the game delivers three times the samples, and the device consumes them
    /// at the same pace, without the queue growing to the cap and discarding.
    fn next_frame(&mut self, pace: f64) -> [f32; 2] {
        self.fraction += self.step * pace;
        while self.fraction >= 1.0 {
            self.fraction -= 1.0;
            self.previous = self.current;
            if self.samples.len() < self.channels {
                // Ran out of samples: do not hold the last value indefinitely. That turns into an
                // audible DC component (mainly in the ports that feed PCM in blocks).
                // It falls smoothly to zero in a few milliseconds and does not accumulate delay.
                self.fraction = 0.0;
                let out = self.current;
                for sample in &mut self.current {
                    *sample *= self.underrun_decay;
                    if sample.abs() < 0.0001 {
                        *sample = 0.0;
                    }
                }
                self.previous = self.current;
                return out;
            }
            let left = self.samples.pop_front().unwrap_or(0.0);
            let right = match self.channels {
                1 => left,
                _ => {
                    let right = self.samples.pop_front().unwrap_or(0.0);
                    for _ in 2..self.channels {
                        self.samples.pop_front();
                    }
                    right
                }
            };
            self.current = [left, right];
        }
        let f = self.fraction as f32;
        [
            self.previous[0] + (self.current[0] - self.previous[0]) * f,
            self.previous[1] + (self.current[1] - self.previous[1]) * f,
        ]
    }
}

#[derive(Debug, Default)]
struct State {
    /// The voices, indexed by the guest `IMedia` object that created them.
    voices: std::collections::HashMap<u32, Voice>,
    /// The voices fed bit by bit, by the same index.
    streams: std::collections::HashMap<u32, Stream>,
    /// Master volume, from 0 to 1.
    master: f32,
    muted: bool,
    rate: u32,
    /// The game's speed relative to the console: 1 outside fast-forward. See
    /// [`Mixer::set_fast_forward`].
    pace: f64,
    /// Silenced by fast-forward, separate from the user's `muted`: releasing fast-forward must not
    /// turn on a sound the user turned off.
    silenced: bool,
    /// Stopped by the window's pause. See [`Mixer::freeze`].
    frozen: bool,
    /// How much of the sound comes out, in frames of [`FADE_OUT_FRAMES`]: full outside the pause,
    /// zero inside it, and in between while entering or leaving it.
    audible: u32,
}

impl State {
    /// Runs `out` through the pause ramp, one frame at a time.
    fn pause_ramp(&mut self, out: &mut [f32], channels: usize) {
        if !self.frozen && self.audible == FADE_OUT_FRAMES {
            return;
        }
        for frame in out.chunks_mut(channels) {
            self.audible = match self.frozen {
                true => self.audible.saturating_sub(1),
                false => (self.audible + 1).min(FADE_OUT_FRAMES),
            };
            let gain = self.audible as f32 / FADE_OUT_FRAMES as f32;
            frame.iter_mut().for_each(|sample| *sample *= gain);
        }
    }
}

/// The mixer, shared between the emulator and the audio thread.
#[derive(Debug, Clone)]
pub struct Mixer {
    state: Arc<Mutex<State>>,
}

impl Mixer {
    fn new(rate: u32, master: f32, muted: bool) -> Self {
        crate::logging!(
            crate::logging::Level::Info,
            "mixer",
            "mixer created at {rate} Hz, master {master:.2}, muted {muted}"
        );
        Self {
            state: Arc::new(Mutex::new(State {
                rate,
                master,
                muted,
                pace: 1.0,
                audible: FADE_OUT_FRAMES,
                ..State::default()
            })),
        }
    }

    /// Starts playing `sound` on voice `id`, replacing whatever was on it.
    ///
    /// `repeat` follows BREW's `MM_PARM_PLAY_REPEAT`: 1 plays once, 0 plays forever.
    pub fn play(&self, id: u32, sound: Arc<Sound>, volume: f32, repeat: u32) {
        self.play_from(id, sound, volume, repeat, 0);
    }

    /// [`Mixer::play`] with `from_us` microseconds of the sound already played, counting the
    /// repetitions: it is what a save state restored in the middle of an effect asks for. See
    /// [`crate::machine::Machine::restore_state`].
    pub fn play_from(&self, id: u32, sound: Arc<Sound>, volume: f32, repeat: u32, from_us: u64) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        // **The measurement that separates "cuts off" from "plays fast".** The voice's step is the
        // ratio between the sound's rate and the mixer's, and it decides speed and pitch: step 2
        // plays one sample out of every two and the sound comes out high-pitched and in half the
        // time, which is the reported symptom. If this number is not 1.0 for a 44100 sound in a
        // 44100 mixer, the defect is here.
        crate::logging!(
            crate::logging::Level::Info,
            "mixer",
            "voice {:#x}: sound {} Hz x{} channel(s), mixer {} Hz, step {:.4}, volume {:.2}, repeats {}",
            id,
            sound.rate,
            sound.channels,
            state.rate,
            f64::from(sound.rate) / f64::from(state.rate.max(1)),
            volume,
            match repeat {
                0 => "forever".to_string(),
                n => format!("{n}x"),
            }
        );
        let step = f64::from(sound.rate) / f64::from(state.rate.max(1));
        // **A voice replaced while it was playing is a cut.** A game's `IMedia` object is usually
        // reused for the next sound, and then the replacement is what the device would do; but if
        // the volume is zero the whole sound would have gone unnoticed, and that is what this
        // warning tells apart.
        let before = state.voices.get(&id);
        if let Some(voice) = before.filter(|voice| !voice.done && !voice.paused) {
            crate::logging!(
                crate::logging::Level::Info,
                "mixer",
                "the voice of object {:#x} was replaced with {:.2}s played of {:.2}s (new voice: {:.2}s, volume {:.2})",
                id,
                voice.played / f64::from(state.rate.max(1)),
                voice.sound.frames() as f64 / f64::from(voice.sound.rate.max(1)),
                sound.frames() as f64 / f64::from(sound.rate.max(1)),
                volume
            );
        }
        // What has already played, in the sound's frames: the whole loops come off the
        // repetitions, and the rest is the position inside the current loop.
        let frames = sound.frames().max(1) as u64;
        let played = from_us.saturating_mul(u64::from(sound.rate)) / 1_000_000;
        let laps = u32::try_from(played / frames).unwrap_or(u32::MAX);
        state.voices.insert(
            id,
            Voice {
                played: played as f64,
                fade_out: 0,
                position: (played % frames) as f64,
                sound,
                step,
                volume,
                remaining: match repeat {
                    0 => None,
                    times => Some(times.saturating_sub(laps).max(1)),
                },
                paused: false,
                done: false,
            },
        );
    }

    /// Silences every voice and closes every stream, at once.
    ///
    /// It is what a restored save state asks for: what the mixer was playing belonged to the
    /// instant before the `Load`, and the restored game does not know it exists — a looping sound
    /// would keep playing forever, because nobody would tell it to stop anymore.
    pub fn stop_all(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.voices.clear();
            state.streams.clear();
        }
    }

    /// Opens on voice `id` an empty stream of `rate` Hz and `channels` channels.
    pub fn open_stream(&self, id: u32, rate: u32, channels: u16, volume: f32) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let channels = usize::from(channels.max(1));
        let step = f64::from(rate) / f64::from(state.rate.max(1));
        // The decay is applied when we advance one source frame, not one output frame.
        // Using the device rate here lengthens the fade when a game delivers 11/22 kHz.
        // Roughly 5 ms down to -60 dB, regardless of the two rates.
        let fade_frames = (rate.max(1) as f32 * 0.005).max(1.0);
        let underrun_decay = 0.001_f32.powf(1.0 / fade_frames);
        state.voices.remove(&id);
        state.streams.insert(
            id,
            Stream {
                samples: std::collections::VecDeque::with_capacity(rate as usize * channels),
                channels,
                step,
                fraction: 0.0,
                previous: [0.0; 2],
                current: [0.0; 2],
                volume: volume.clamp(0.0, 1.0),
                paused: false,
                // Half a second of slack.
                capacity: rate as usize * channels / 2,
                underrun_decay,
            },
        );
    }

    /// Delivers interleaved samples to the stream of voice `id`.
    pub fn feed_stream(&self, id: u32, samples: &[f32]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(stream) = state.streams.get_mut(&id) else {
            return;
        };
        stream.samples.extend(samples.iter().copied());
        let excess = stream.samples.len().saturating_sub(stream.capacity);
        let excess = excess - excess % stream.channels;
        stream.samples.drain(..excess);
    }

    pub fn stop(&self, id: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.voices.remove(&id);
            state.streams.remove(&id);
        }
    }

    pub fn set_paused(&self, id: u32, paused: bool) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.paused = paused;
            }
            if let Some(stream) = state.streams.get_mut(&id) {
                stream.paused = paused;
            }
        }
    }

    pub fn set_volume(&self, id: u32, volume: f32) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.volume = volume.clamp(0.0, 1.0);
            }
            if let Some(stream) = state.streams.get_mut(&id) {
                stream.volume = volume.clamp(0.0, 1.0);
            }
        }
    }

    /// Whether voice `id` is still playing.
    ///
    /// What decides the end of a sound for the game is the virtual clock, not the mixer — the
    /// emulator runs muted without stopping counting time. This is the tests' window into the
    /// voices' lifetime, and that is all it is for.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_playing(&self, id: u32) -> bool {
        self.state
            .lock()
            .map(|state| state.voices.contains_key(&id) || state.streams.contains_key(&id))
            .unwrap_or(false)
    }

    pub fn set_master(&self, volume: f32, muted: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.master = volume.clamp(0.0, 1.0);
            state.muted = muted;
        }
    }

    /// The game's speed, so the sound moves along with it, and whether fast-forward silences it.
    ///
    /// **The sound is pulled by the device's clock, not by the game's.** The device asks for
    /// samples at the pace of the real world; at 3x the game would play the music at 1x and
    /// deliver the effects three times closer together, and the end of each sound — which the game
    /// measures by the virtual clock — would arrive before the mixer played it in full.
    /// Multiplying the step of each voice and each stream by the speed makes the sound keep up
    /// with the game, higher-pitched, like a sped-up tape. Outside fast-forward the pace is exactly
    /// 1, and nothing changes. See `docs/implementation/24-speed.md`.
    pub fn set_fast_forward(&self, pace: f32, silenced: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.pace = f64::from(pace.clamp(0.1, 16.0));
            state.silenced = silenced;
        }
    }

    /// Stops all sound where it is, or releases it: it is the window's pause.
    ///
    /// **Pausing is not muting.** Mute lets the sound run, and a voice's `set_paused` is the game
    /// asking; here it is the whole game that stops. The game measures the end of each sound by
    /// the virtual clock, which the pause holds: with the mixer running, the music kept playing
    /// with the picture frozen, reached its end during the pause, and on resuming the game waited
    /// for a sound that had already finished. The streams already went silent on their own,
    /// because a stopped game does not feed them — which is why the defect only showed up in some
    /// games.
    ///
    /// Entering and leaving go through [`FADE_OUT_FRAMES`] frames of ramp, for the same reason as
    /// a voice's fade: stopping in the middle of the wave pops. On entering, the sound runs those
    /// extra frames; less than 2 ms, below any difference the game measures.
    pub fn freeze(&self, frozen: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.frozen = frozen;
        }
    }

    /// A mixer with no device at all, to write to a file what would come out of the speaker.
    ///
    /// It exists so the sound can be **checked**: without this, the only way to know whether the
    /// audio is right is to listen to it, and that does not fit in a test or an automated run.
    pub fn silent(rate: u32) -> Self {
        Self::new(rate, 1.0, false)
    }

    /// Mixes `frames` stereo frames and returns the interleaved samples.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        self.render_into(frames, &mut out);
        out
    }

    /// The same as [`render`](Self::render), writing into a buffer the caller keeps.
    ///
    /// The core asks for this every frame. Allocating and freeing the `Vec` every time fragments
    /// the Vita's heap, and the next `malloc` — the long track — does not find a contiguous block.
    pub fn render_into(&self, frames: usize, out: &mut Vec<f32>) {
        out.resize(frames * 2, 0.0);
        self.fill(out, 2);
    }

    /// Fills `out` with the mix of the voices. `channels` is how many channels the device wants.
    fn fill(&self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let master = match state.muted || state.silenced {
            true => 0.0,
            false => state.master,
        };
        let pace = state.pace;
        // Frozen, only the frames left in the ramp move; the rest stays where it is.
        let live = match state.frozen {
            true => frames.min(state.audible as usize),
            false => frames,
        };
        if master == 0.0 {
            // Muted still consumes the voices: the sound keeps running, it just does not come out.
            for voice in state.voices.values_mut() {
                if !voice.paused && !voice.done {
                    for _ in 0..live {
                        voice.step(pace);
                    }
                }
            }
            state.voices.retain(|_, voice| !voice.done);
            for stream in state.streams.values_mut() {
                if !stream.paused {
                    for _ in 0..live {
                        stream.next_frame(pace);
                    }
                }
            }
            state.pause_ramp(out, channels);
            return;
        }
        for voice in state.voices.values_mut() {
            if voice.paused || voice.done {
                continue;
            }
            // Mono feeds both channels with the same sample. Computing it twice was the general
            // path, and in the track (one channel, 22 kHz) that is half the loop.
            let mono = voice.sound.channels <= 1;
            for frame in out[..live * channels].chunks_mut(channels) {
                if voice.done {
                    break;
                }
                // **The gain stays inside the loop.** `decay()` changes every frame while the
                // voice fades; hoisting it out would freeze the first value and erase the fade.
                let gain = voice.volume * master * voice.decay();
                if mono {
                    let mono_sample = voice.sample(0) * gain;
                    for slot in frame.iter_mut() {
                        *slot += mono_sample;
                    }
                } else {
                    for (channel, slot) in frame.iter_mut().enumerate() {
                        *slot += voice.sample(channel) * gain;
                    }
                }
                // Once the fade starts, the sound no longer advances: what you hear is the last
                // value, smaller and smaller. When the fade ends, the voice goes.
                voice.step(pace);
            }
        }
        for stream in state.streams.values_mut() {
            if stream.paused {
                continue;
            }
            let gain = stream.volume * master;
            for frame in out[..live * channels].chunks_mut(channels) {
                let stereo = stream.next_frame(pace);
                for (channel, slot) in frame.iter_mut().enumerate() {
                    *slot += stereo[channel.min(1)] * gain;
                }
            }
        }
        state.pause_ramp(out, channels);
        // Summing voices overflows the range; clipping is what a device would do anyway, and it
        // is better than letting the value wrap around and turn into a pop.
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
        // The voice that finished goes here: it is what makes `is_playing` tell the truth, and it
        // is the only cleanup needed — whoever starts a new sound on the same object replaces the
        // voice.
        state.voices.retain(|_, voice| !voice.done);
    }
}

/// The audio output. While it exists, the sound plays; dropping it closes the stream.
///
/// It only exists with the `desktop` feature: a Libretro frontend receives what the mixer
/// generates through a callback and has no device of its own to open.
#[cfg(feature = "audio")]
pub struct Output {
    _stream: cpal::Stream,
    mixer: Mixer,
}

/// How many frames the **device** asked for, and when we last said so.
///
/// **It is the measure of "nothing plays".** The mixer may render the right sound and still the
/// stream stops being fed — the device stops asking, and what you hear is only what was already
/// in its buffer: one piece, once. If this counter stops growing, the defect is in the stream, not
/// in the engine.
static OUTPUT_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OUTPUT_LAST_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static OUTPUT_CLOCK: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

#[cfg(feature = "audio")]
impl Output {
    /// Opens the system's default device.
    ///
    /// Returns the reason as text when it fails: a host without audio must not stop the game from
    /// running, so the caller treats it as "no sound", not as a fatal error.
    pub fn open(volume: f32, muted: bool) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output available")?;
        // CPAL 0.15's Android backend picks 44.1 kHz through the generic heuristic even on
        // devices whose native output is 48 kHz. That makes our mixer resample to 44.1 and
        // Android resample again to 48. Preferring 48 kHz/F32 avoids that second conversion.
        let config = if cfg!(target_os = "android") {
            let rate = cpal::SampleRate(48_000);
            let ranges: Vec<_> = device
                .supported_output_configs()
                .map(|configs| configs.collect())
                .unwrap_or_default();
            let preferred = ranges
                .iter()
                .copied()
                .filter(|range| {
                    range.channels() == 2 && range.sample_format() == cpal::SampleFormat::F32
                })
                .find_map(|range| range.try_with_sample_rate(rate))
                .or_else(|| {
                    ranges
                        .iter()
                        .copied()
                        .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
                        .find_map(|range| range.try_with_sample_rate(rate))
                });
            match preferred {
                Some(config) => config,
                None => device
                    .default_output_config()
                    .map_err(|err| err.to_string())?,
            }
        } else {
            device
                .default_output_config()
                .map_err(|err| err.to_string())?
        };
        let channels = config.channels() as usize;
        // **The rate the device actually uses.** The mixer is created with it, and this is the
        // last mile the 5-why of #43 points to: a device at a different rate than expected makes
        // the sound come out high-pitched and fast, and nothing inside the engine shows it.
        crate::logging!(
            crate::logging::Level::Info,
            "audio",
            "device at {} Hz, {} channel(s), format {:?}",
            config.sample_rate().0,
            channels,
            config.sample_format()
        );
        let mixer = Mixer::new(config.sample_rate().0, volume, muted);
        let mut stream_config = config.config();
        // The emulator can have heavy CPU/GPU spikes. Giving Oboe ~21 ms of capacity keeps a bad
        // slice from turning into crackle, without pushing latency to noticeably high values.
        //
        // **There is no fallback to the driver's default, and none is missing:** cpal 0.15.3's
        // Oboe backend translates `BufferSize::Fixed(n)` into `set_buffer_capacity_in_frames(n)`
        // and has no refusal path. A retry `match` here would be a branch that never runs.
        if cfg!(target_os = "android") {
            stream_config.buffer_size = cpal::BufferSize::Fixed(1024);
        }

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mixer = mixer.clone();
                device.build_output_stream(
                    &stream_config,
                    move |out: &mut [f32], _| {
                        // The device asked: count the frames and, every ten seconds, say it is
                        // still alive. See [`OUTPUT_FRAMES`]. It used to be once per second, and in
                        // a recorded 7 min session that made 438 of the file's 544 lines; ten
                        // seconds still show when the stream stops, and match the `saude:` line.
                        {
                            use std::sync::atomic::Ordering;
                            let frames = (out.len() / channels.max(1)) as u64;
                            let total =
                                OUTPUT_FRAMES.fetch_add(frames, Ordering::Relaxed) + frames;
                            let start = *OUTPUT_CLOCK.get_or_init(std::time::Instant::now);
                            let elapsed_ms = start.elapsed().as_millis() as u64;
                            if elapsed_ms >= OUTPUT_LAST_MS.load(Ordering::Relaxed) + 10_000 {
                                OUTPUT_LAST_MS.store(elapsed_ms, Ordering::Relaxed);
                                crate::logging!(
                                    crate::logging::Level::Info,
                                    "audio",
                                    "device: {total} frames requested in {elapsed_ms} ms ({:.0} per second)",
                                    total as f64 * 1000.0 / elapsed_ms.max(1) as f64
                                );
                            }
                        }
                        mixer.fill(out, channels)
                    },
                    |err| {
                        crate::logging!(
                            crate::logging::Level::Error,
                            "audio",
                            "audio output error: {err}"
                        )
                    },
                    None,
                )
            }
            other => return Err(format!("unsupported audio format: {other}")),
        }
        .map_err(|err| err.to_string())?;
        stream.play().map_err(|err| err.to_string())?;
        Ok(Self {
            _stream: stream,
            mixer,
        })
    }

    pub fn mixer(&self) -> Mixer {
        self.mixer.clone()
    }
}

#[cfg(feature = "audio")]
impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output").finish_non_exhaustive()
    }
}

/// Writes stereo samples as a 16-bit WAVE.
///
/// It is the counterpart of the reader: the emulator reads RIFF from the game and, when you want
/// to check what came out, writes RIFF back.
pub fn to_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let payload: Vec<u8> = samples
        .iter()
        .flat_map(|&s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&2u16.to_le_bytes()); // stereo
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes per second
    fmt.extend_from_slice(&4u16.to_le_bytes()); // frame alignment
    fmt.extend_from_slice(&16u16.to_le_bytes());

    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((36 + payload.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize) -> Arc<Sound> {
        Arc::new(Sound::create(
            rate,
            1,
            (0..frames).map(|_| 1.0).collect(),
        ))
    }

    /// A mixer with no device, to test mixing without depending on audio on the host.
    fn mixer(rate: u32) -> Mixer {
        Mixer::new(rate, 1.0, false)
    }

    /// **The voice has to last as long as the sound, not half of it.**
    ///
    /// This is the easiest calculation to get wrong in all of the audio, and the mistake has an
    /// exact symptom: a mono sound consumed as if it were stereo moves twice as fast through the
    /// file and ends in half the time — to the listener, "the speech cut off". `zeebo-lle` measured
    /// the same trap on its side (ratio `channels × rate / 2 × device rate`), and Turma da Mônica
    /// has 3.77 s mono voices: with the wrong factor they become 0.94 s, which is the reported
    /// "cuts at ~1 s".
    ///
    /// The test pins the duration for mono **and** for stereo, so that the difference between the
    /// two paths can no longer be written.
    #[test]
    fn a_voice_plays_the_whole_sound_not_half_of_it() {
        for channels in [1u16, 2] {
            let frames = 8_000usize;
            let samples: Vec<f32> = (0..frames * usize::from(channels))
                .map(|i| ((i % 97) as f32 / 97.0) * 0.5)
                .collect();
            let sound = Arc::new(Sound::create(8_000, channels, samples));
            let mixer = mixer(8_000);
            mixer.play(7, sound, 1.0, 1);
            let mut total = 0usize;
            // The cap avoids an infinite loop if the voice never ends; the expected value is much
            // smaller.
            while mixer.is_playing(7) && total < 40_000 {
                let _ = mixer.render(1_000);
                total += 1_000;
            }
            let expected = frames + FADE_OUT_FRAMES as usize;
            assert!(
                total.abs_diff(expected) <= 1_000,
                "channels {channels}: played {total} frames, expected {expected} (sound of {frames})"
            );
        }
    }

    /// A voice that starts midway plays only what is left, counting the repetitions already
    /// played. And "stop everything" takes voices and streams together.
    #[test]
    fn a_voice_starts_midway_and_stop_all_silences_everything() {
        let mixer = mixer(1_000);
        // A one-second sound, three times, with 2.5 s already played: half a second is left.
        mixer.play_from(1, tone(1_000, 1_000), 1.0, 3, 2_500_000);
        let mut total = 0usize;
        while mixer.is_playing(1) && total < 10_000 {
            let _ = mixer.render(50);
            total += 50;
        }
        let expected = 500 + FADE_OUT_FRAMES as usize;
        assert!(total.abs_diff(expected) <= 50, "played {total} frames, expected {expected}");

        mixer.play(2, tone(1_000, 1_000), 1.0, 0);
        mixer.open_stream(3, 1_000, 1, 1.0);
        mixer.stop_all();
        assert!(!mixer.is_playing(2) && !mixer.is_playing(3));
    }

    /// At 3x the voice ends in a third of the time, which is when the game — by the virtual clock
    /// — thinks it ended. And the fast-forward muting silences without stopping the voice.
    #[test]
    fn in_fast_forward_a_voice_moves_at_game_speed() {
        let frames = 9_000usize;
        let sound = Arc::new(Sound::create(9_000, 1, vec![0.5; frames]));
        let mixer = mixer(9_000);
        mixer.set_fast_forward(3.0, true);
        mixer.play(4, sound, 1.0, 1);
        assert!(mixer.render(10).iter().all(|&s| s == 0.0), "muted makes no sound");
        let mut total = 10usize;
        while mixer.is_playing(4) && total < 40_000 {
            let _ = mixer.render(100);
            total += 100;
        }
        let expected = frames / 3 + FADE_OUT_FRAMES as usize;
        assert!(
            total.abs_diff(expected) <= 100,
            "played {total} frames, expected {expected}"
        );

        // Outside fast-forward, back to 1 and audible.
        mixer.set_fast_forward(1.0, false);
        mixer.play(5, tone(9_000, 100), 1.0, 1);
        assert!(mixer.render(4).iter().all(|&s| s > 0.9));
    }

    #[test]
    fn the_mixer_output_turns_back_into_a_wave() {
        // Dumping to a file is how the sound is checked without listening to it, so the round
        // trip has to match.
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 8), 1.0, 1);
        let samples = mixer.render(4);
        let wave = to_wav(&samples, 8000);
        let loaded = crate::audio::wav::parse(&wave).unwrap();
        assert_eq!(loaded.rate, 8000);
        assert_eq!(loaded.channels, 2);
        assert_eq!(loaded.frames(), 4);
        assert!(loaded.samples.iter().all(|&s| s > 0.9));
    }

    #[test]
    fn a_voice_plays_at_the_requested_volume() {
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 100), 0.5, 1);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn two_voices_add_up_and_the_result_is_clipped() {
        // Three full voices would add up to 3.0; the device receives 1.0, not a value that wraps
        // around.
        let mixer = mixer(8000);
        for id in 1..=3 {
            mixer.play(id, tone(8000, 100), 1.0, 1);
        }
        let mut out = [0.0f32; 4];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn a_finished_voice_leaves_the_list() {
        // Two frames of sound on a device at the same rate: the sound ends, and the voice **fades**
        // before leaving — it vanishes on its own, otherwise the voice list would grow with every
        // effect played.
        let mixer = mixer(8000);
        mixer.play(7, tone(8000, 2), 1.0, 1);
        let mut out = [0.0f32; (FADE_OUT_FRAMES as usize + 16) * 2];
        mixer.fill(&mut out, 2);
        assert!(!mixer.is_playing(7), "the voice leaves after the fade");
    }

    /// **The fade exists, and it is monotonic.**
    ///
    /// The sound ends in the middle of the wave, the level may be at almost the full range, and
    /// cutting there is a pop. The voice holds the last value and fades over [`FADE_OUT_FRAMES`]
    /// frames; since the position does not move in that stretch, the output is exactly a ramp —
    /// and that is what the test demands.
    #[test]
    fn a_voice_fades_out_instead_of_vanishing_at_once() {
        let mixer = mixer(8000);
        mixer.play(7, tone(8000, 2), 1.0, 1);
        let mut out = vec![0.0f32; (FADE_OUT_FRAMES as usize + 4) * 2];
        mixer.fill(&mut out, 2);
        let left: Vec<f32> = out.chunks_exact(2).map(|pair| pair[0]).collect();
        let last = left[..FADE_OUT_FRAMES as usize].to_vec();
        assert!(
            last.windows(2).all(|pair| pair[1] <= pair[0] + 1e-6),
            "the fade has to be monotonic: {last:?}"
        );
        assert!(
            last.first().copied().unwrap_or(0.0) > 0.0,
            "the fade starts where the sound stopped"
        );
    }

    #[test]
    fn infinite_repeat_never_ends() {
        // `MM_PARM_PLAY_REPEAT` zero is "plays forever".
        let mixer = mixer(8000);
        mixer.play(3, tone(8000, 2), 1.0, 0);
        let mut out = [0.0f32; 64];
        mixer.fill(&mut out, 2);
        assert!(mixer.is_playing(3));
    }

    #[test]
    fn a_paused_voice_neither_plays_nor_advances() {
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 4), 1.0, 1);
        mixer.set_paused(1, true);
        let mut out = [0.0f32; 16];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0));
        // And it continues from where it stopped when resumed.
        mixer.set_paused(1, false);
        mixer.fill(&mut out, 2);
        assert!(out[0] > 0.0);
    }

    /// The window's pause stops the music where it is, with no pop on entering or leaving, and on
    /// resuming the music continues from the same point. Before, it kept playing with the picture
    /// frozen.
    #[test]
    fn frozen_the_sound_fades_stops_and_resumes_where_it_left_off() {
        let frames = 1_000usize;
        let mixer = mixer(1_000);
        mixer.play(1, tone(1_000, frames), 1.0, 1);
        let _ = mixer.render(100);
        mixer.freeze(true);
        let fade_out = mixer.render(FADE_OUT_FRAMES as usize);
        let left: Vec<f32> = fade_out.iter().step_by(2).copied().collect();
        assert!(left[0] > 0.9 && left.windows(2).all(|pair| pair[1] <= pair[0]));
        assert_eq!(left.last().copied(), Some(0.0));
        // Stopped, it neither comes out nor moves: a whole minute does not reach the end of the
        // one-second sound.
        assert!(mixer.render(60_000).iter().all(|&s| s == 0.0));
        assert!(mixer.is_playing(1), "the music ended during the pause");

        mixer.freeze(false);
        let fade_in: Vec<f32> = mixer
            .render(FADE_OUT_FRAMES as usize)
            .into_iter()
            .step_by(2)
            .collect();
        assert!(fade_in[0] > 0.0 && fade_in[0] < 0.1, "{fade_in:?}");
        assert!(fade_in.windows(2).all(|pair| pair[1] >= pair[0]));
        // On resuming the rest of the sound comes out: the total minus the 100 frames before the
        // pause. The entry ramp moved FADE_OUT_FRAMES frames, and the fade at the end of the sound
        // adds as many again.
        let mut total = FADE_OUT_FRAMES as usize;
        while mixer.is_playing(1) && total < 10_000 {
            let _ = mixer.render(10);
            total += 10;
        }
        let expected = frames - 100;
        assert!(total.abs_diff(expected) <= 10, "{total} versus {expected}");
    }

    /// Muted and paused at the same time, the sound also stays put: mute lets it run, the pause
    /// does not.
    #[test]
    fn frozen_and_muted_the_sound_does_not_advance() {
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 1_000), 1.0, 1);
        mixer.set_master(1.0, true);
        mixer.freeze(true);
        let _ = mixer.render(10_000);
        assert!(mixer.is_playing(1));
    }

    #[test]
    fn a_stream_that_runs_dry_falls_silent_instead_of_buzzing() {
        // A short block must not leave the last sample stuck in the output. Before, a stream that
        // delayed the next batch sustained 1.0 indefinitely — DC voltage audible as a pop/buzz.
        // The last sample comes out whole once and then disappears in ~5 ms.
        let mixer = mixer(1000);
        mixer.open_stream(9, 1000, 1, 1.0);
        mixer.feed_stream(9, &[1.0]);
        let samples = mixer.render(12);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        assert!(left.iter().any(|sample| *sample > 0.9), "{left:?}");
        assert!(left.last().copied().unwrap_or(1.0).abs() < 0.001, "{left:?}");
    }

    #[test]
    fn underrun_fade_does_not_depend_on_the_output_rate() {
        // The source is 1 kHz and the device 4 kHz. The decay happens when consuming source
        // frames; after ~10 ms it must already be effectively silent even with the device 4x
        // faster.
        let mixer = mixer(4000);
        mixer.open_stream(10, 1000, 1, 1.0);
        mixer.feed_stream(10, &[1.0]);
        let samples = mixer.render(48);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        assert!(left.iter().any(|sample| *sample > 0.9), "{left:?}");
        assert!(left.last().copied().unwrap_or(1.0).abs() < 0.001, "{left:?}");
    }

    #[test]
    fn the_sound_rate_is_converted_to_the_output_rate() {
        // A 4000 Hz sound on an 8000 device: each sound frame yields two device frames, so
        // four sound frames become eight before it ends.
        let mixer = mixer(8000);
        mixer.play(1, tone(4000, 4), 1.0, 1);
        let mut out = [0.0f32; 12]; // six stereo frames
        mixer.fill(&mut out, 2);
        assert!(mixer.is_playing(1), "it could not have ended yet");
        let mut rest = [0.0f32; (FADE_OUT_FRAMES as usize + 16) * 2];
        mixer.fill(&mut rest, 2);
        assert!(!mixer.is_playing(1), "it leaves after the fade");
    }

    #[test]
    fn resampling_preserves_the_pitch() {
        // A 1000 Hz sine recorded at 8000 and played on a 32000 device has to stay 1000 Hz.
        // Inverting the ratio between the rates sounds equally "right" in a volume test, and only
        // the frequency gives it away — hence counting the zero crossings.
        let rate = 8000;
        let hertz = 1000.0;
        let sound = Arc::new(Sound::create(
            rate,
            1,
            (0..rate)
                .map(|i| (i as f32 / rate as f32 * hertz * std::f32::consts::TAU).sin())
                .collect(),
        ));

        let device = 32000;
        let mixer = mixer(device);
        mixer.play(1, sound, 1.0, 1);
        // Half a second of the device, in stereo frames.
        let samples = mixer.render(device as usize / 2);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        // Half a second of 1000 Hz has a thousand zero crossings.
        assert!(
            (900..=1100).contains(&crossings),
            "expected about 1000 zero crossings, got {crossings}"
        );
    }

    #[test]
    fn the_stream_plays_what_the_game_feeds_and_holds_the_last_value_when_starved() {
        // An 8000 Hz stream on an 8000 device: a delivered frame is a played frame.
        let mixer = mixer(8000);
        mixer.open_stream(7, 8000, 1, 1.0);
        mixer.feed_stream(7, &[0.5, -0.5, 0.25]);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        // Both channels receive the same. The stream starts from silence: the first delivered
        // sample comes out on the next frame, and what is missing holds the last value instead of
        // popping.
        assert_eq!(out[0], out[1]);
        assert_eq!(&out[..8], &[0.0, 0.0, 0.5, 0.5, -0.5, -0.5, 0.25, 0.25]);
        assert!(mixer.is_playing(7));
        mixer.stop(7);
        assert!(!mixer.is_playing(7));
    }

    #[test]
    fn when_muted_the_sound_runs_but_is_not_output() {
        // Muting is not pausing: whoever turns the volume back up in the middle of a song expects
        // to find it further along, not stopped.
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 4), 1.0, 1);
        mixer.set_master(1.0, true);
        let mut out = [0.0f32; (FADE_OUT_FRAMES as usize + 8) * 2];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0), "nothing comes out when muted");
        assert!(!mixer.is_playing(1), "the sound ran to the end, fade included");
    }
}

#[cfg(test)]
mod report_tests {
    /// **The build without the synthesizer also answers**, and says it does not have it — instead
    /// of staying silent. Silence here would lead to the same wrong conclusion as on the other
    /// side: "the bank does not work".
    #[cfg(not(feature = "soundfont"))]
    #[test]
    fn the_report_warns_that_the_build_has_no_bank() {
        let device = std::env::temp_dir().join("zeebx-aparelho-sem-feature");
        let text = super::soundfont::report(&device);
        assert!(
            text.contains("does not have the sample bank synthesizer"),
            "the report has to say the build lacks the capability: {text}"
        );
        assert!(text.contains("soundfonts"), "and where the file would go: {text}");
    }
}
