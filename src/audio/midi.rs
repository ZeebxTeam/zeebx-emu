//! MIDI: the music of the arcade ports, synthesized here.
//!
//! Eleven games deliver their soundtrack as MIDI — Double Dragon, Bad Dudes, Caveman Ninja, Dark
//! Seal, Heavy Barrel, Karnov's Revenge, Magical Drop 3, Spin Master, Street Hoop, Super BurgerTime
//! and Wizard Fire. Each one carries **one** `.wav`, which is the effect, and the music arrives
//! through `IMedia` as `audio/mid`.
//!
//! **MIDI is not sound, it is a score.** There is no sample inside the file: there is "play note
//! 69 on instrument 25 with velocity 100". What turned that into sound was the console firmware's
//! synthesizer, with its instrument bank — and that bank is not in the partition we have. So what
//! this module does is a **declared approximation**: it reads the right score and plays it with
//! timbres of our own.
//!
//! Saying so up front matters more than usual, because here the error is silent: a wrongly
//! synthesized song *plays*, and sounds as if it were meant that way. That is why what this module
//! holds itself to is what can be checked without listening — **the right note, at the right time,
//! for the right duration**:
//!
//! - the score is read the way the specification says, running status included;
//! - the conversion from tick to second honors the header's division and the tempo changes;
//! - note 69 comes out at 440 Hz, measured by correlation — not by sounding right.
//!
//! The timbre is what **cannot** be checked that way, and it is precisely the part that is a
//! guess. It is recorded as a hypothesis in use in the report.

/// Rate at which the music is synthesized.
///
/// 22,050 Hz is the rate of the games' own tracks and half the device's: the mixer resamples
/// anyway, and synthesizing at 44,100 would double the cost and the memory for treble that does
/// not exist in the score.
///
/// It is the **same** rate for the wavetable and for the sample bank: the mixer resamples to the
/// device's rate anyway, and keeping both at the same number makes the comparison between them
/// one of timbre, not of rate.
pub const RATE: u32 = 22_050;

/// Cap on the synthesized duration, in seconds.
///
/// A game track is one or two minutes long and repeats — the mixer does the repeating, through
/// `MM_PARM_PLAY_REPEAT`. The cap exists so that a broken file, whose last event may land hours
/// away, does not turn into a request for gigabytes of memory.
const MAX_SECONDS: f64 = 300.0;

/// How many notes can sound at the same time.
///
/// Going past that is not music, it is a broken file or a note that was never released. The cut
/// keeps a `Note On` without a `Note Off` from piling up voices until the end of the synthesis.
const MAX_VOICES: usize = 48;

/// The percussion channel in General MIDI. It is the tenth, counting from zero.
const PERCUSSION_CHANNEL: u8 = 9;

/// Microseconds per beat before any `Set Tempo`: 120 beats per minute, the specification's
/// default.
const DEFAULT_TEMPO: u32 = 500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    Program {
        channel: u8,
        program: u8,
    },
    /// The channel's volume (7), and expression (11), which is a second factor on top of it. The
    /// rest of `Control Change` is ignored.
    Volume {
        channel: u8,
        value: u8,
    },
    Expression {
        channel: u8,
        value: u8,
    },
    /// All of the channel's notes released at once — `All Notes Off` and `All Sound Off`.
    AllNotesOff {
        channel: u8,
    },
    Tempo {
        us_per_beat: u32,
    },
}

/// The parsed score, in ticks.
#[derive(Debug, Clone)]
struct Score {
    /// The header's `division` field, as is: positive is ticks per beat, negative is SMPTE.
    division: i16,
    /// The events of all tracks, together and in tick order.
    events: Vec<(u64, Event)>,
}

/// Reads a variable-length number: seven bits per byte, the high bit saying it continues.
fn varlen(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let mut value = 0u32;
    // Four bytes is the maximum the specification allows, and the limit keeps garbage with the
    // high bit always set from dragging the read to the end of the file.
    for _ in 0..4 {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        value = (value << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Reads a whole `MTrk` track, returning the events with each one's absolute tick.
///
/// **Running status is mandatory, not an optimization.** A real file omits the status byte when
/// it repeats the previous one, and a track of consecutive notes is almost entirely like that:
/// ignoring it does not lose one event, it loses the whole track from the first one on.
fn read_track(bytes: &[u8]) -> Vec<(u64, Event)> {
    let mut events = Vec::new();
    let mut pos = 0usize;
    let mut tick = 0u64;
    let mut status = 0u8;
    while pos < bytes.len() {
        let Some(delta) = varlen(bytes, &mut pos) else {
            break;
        };
        tick += u64::from(delta);
        let Some(&first) = bytes.get(pos) else {
            break;
        };
        // A data byte in place of the status: the previous status applies, and the byte is the
        // first data byte.
        if first & 0x80 != 0 {
            status = first;
            pos += 1;
        }
        let channel = status & 0x0f;
        let data_byte = |pos: &mut usize| -> Option<u8> {
            let byte = *bytes.get(*pos)?;
            *pos += 1;
            Some(byte & 0x7f)
        };
        match status & 0xf0 {
            // `Note On` with zero velocity **is** a `Note Off`, and that is how most sequencers
            // release a note. Treating it as a strike would leave the note stuck forever.
            0x90 => {
                let (Some(note), Some(velocity)) = (data_byte(&mut pos), data_byte(&mut pos)) else {
                    break;
                };
                events.push((
                    tick,
                    match velocity {
                        0 => Event::NoteOff { channel, note },
                        velocity => Event::NoteOn { channel, note, velocity },
                    },
                ));
            }
            0x80 => {
                let (Some(note), Some(_)) = (data_byte(&mut pos), data_byte(&mut pos)) else {
                    break;
                };
                events.push((tick, Event::NoteOff { channel, note }));
            }
            0xb0 => {
                let (Some(controller), Some(value)) = (data_byte(&mut pos), data_byte(&mut pos)) else {
                    break;
                };
                match controller {
                    // Volume and expression **multiply**, they do not overwrite each other:
                    // treating both as the same thing makes an expression `crescendo` erase the
                    // channel's volume, and the instrument comes back at twice what it should.
                    7 => events.push((tick, Event::Volume { channel, value })),
                    11 => events.push((tick, Event::Expression { channel, value })),
                    120 | 123 => events.push((tick, Event::AllNotesOff { channel })),
                    _ => {}
                }
            }
            0xc0 => match data_byte(&mut pos) {
                Some(program) => events.push((tick, Event::Program { channel, program })),
                None => break,
            },
            // Channel pressure: one byte. Note pressure, pitch bend and control: two.
            0xd0 => {
                if data_byte(&mut pos).is_none() {
                    break;
                }
            }
            0xa0 | 0xe0 => {
                if data_byte(&mut pos).is_none() || data_byte(&mut pos).is_none() {
                    break;
                }
            }
            0xf0 => match status {
                // Meta event: type, size, data.
                0xff => {
                    let Some(&kind) = bytes.get(pos) else { break };
                    pos += 1;
                    let Some(size) = varlen(bytes, &mut pos) else {
                        break;
                    };
                    let end = pos + size as usize;
                    if kind == 0x51
                        && size == 3
                        && let Some(t) = bytes.get(pos..pos + 3)
                    {
                        let us = u32::from_be_bytes([0, t[0], t[1], t[2]]);
                        if us > 0 {
                            events.push((tick, Event::Tempo { us_per_beat: us }));
                        }
                    }
                    // End of track (0x2f) and the rest of the metas: skip by the declared size.
                    if kind == 0x2f {
                        break;
                    }
                    pos = end;
                }
                // SysEx, with a declared size. Running status does **not** apply after it.
                0xf0 | 0xf7 => {
                    let Some(size) = varlen(bytes, &mut pos) else {
                        break;
                    };
                    pos += size as usize;
                    status = 0;
                }
                _ => break,
            },
            // A status that does not exist: without knowing the size, there is no safe way to
            // skip.
            _ => break,
        }
    }
    events
}

/// Reads a Standard MIDI File. `None` when it is not one.
fn read(data: &[u8]) -> Option<Score> {
    if data.get(..4)? != b"MThd" {
        return None;
    }
    let header = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?) as usize;
    let body = data.get(8..8 + header.max(6))?;
    let format = u16::from_be_bytes(body.get(..2)?.try_into().ok()?);
    let division = i16::from_be_bytes(body.get(4..6)?.try_into().ok()?);
    // Format 2 is a collection of independent patterns, not a song; playing its tracks together
    // would give an overlay nobody composed.
    if format > 1 || division == 0 {
        return None;
    }

    // **The number of tracks is what the header declares, and the read stops there.** Without
    // this, two SMFs glued in the same buffer become one song: measured, a member extracted from a
    // badly cut `.ggz` yielded 168 s of music where the game plays 47.5 s, because the next file's
    // `MTrk`s came in as if they were tracks of this one. The game delivers one SMF per `Play`, so
    // the defect does not show up in a game — but it would in any file with two songs
    // concatenated, and the wrong duration is the symptom that is least noticed.
    let tracks = u16::from_be_bytes(body.get(2..4)?.try_into().ok()?) as usize;
    let mut events = Vec::new();
    let mut pos = 8 + header;
    let mut read_count = 0usize;
    while pos + 8 <= data.len() && read_count < tracks.max(1) {
        let size = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = data.get(pos + 8..(pos + 8 + size).min(data.len()))?;
        // A chunk that is not `MTrk` is skipped by its size: the specification says to ignore
        // unknown chunks, and there are files with an authoring chunk before the tracks.
        if &data[pos..pos + 4] == b"MTrk" {
            events.extend(read_track(body));
            read_count += 1;
        }
        pos += 8 + size;
    }
    if events.is_empty() {
        return None;
    }
    // Stable order by tick: events on the same tick keep the order in which they appeared, which
    // is what makes a `Program` before the `NoteOn` at the same instant still apply to it.
    events.sort_by_key(|(tick, _)| *tick);
    Some(Score { division, events })
}

impl Score {
    /// The events with their instant in seconds, applying the division and the tempo changes.
    ///
    /// The calculation cannot be done once for the whole file: `Set Tempo` changes the tick's
    /// length in the middle of the song, and an `accelerando` recalculated from zero would put
    /// everything after it in the wrong place. So the instant is accumulated stretch by stretch.
    fn in_seconds(&self) -> Vec<(f64, Event)> {
        let smpte = self.division < 0;
        // SMPTE: the high byte is frames per second (with 29 meaning 29.97) and the low one is
        // ticks per frame. Here time does not depend on the beat, and `Set Tempo` does not apply.
        let per_smpte_tick = match smpte {
            false => 0.0,
            true => {
                // The high byte is the number of frames **in two's complement** — 0xe7 is -25 —,
                // not the whole field negated: negating all sixteen bits together gives 24 where
                // it should give 25, and the music plays 4% off tempo.
                let bits = self.division as u16;
                let frames = match -((bits >> 8) as u8 as i8) as i32 {
                    // 29 frames means 29.97: it is NTSC video, and the specification says so.
                    29 => 29.97,
                    other => f64::from(other),
                };
                let subframes = f64::from(bits & 0xff);
                1.0 / (frames.max(1.0) * subframes.max(1.0))
            }
        };
        let per_beat = f64::from(self.division.max(1) as u32);
        let mut us_per_beat = f64::from(DEFAULT_TEMPO);
        let mut seconds = 0.0;
        let mut last = 0u64;
        let mut output = Vec::with_capacity(self.events.len());
        for &(tick, event) in &self.events {
            let step = (tick - last) as f64;
            seconds += match smpte {
                true => step * per_smpte_tick,
                false => step * us_per_beat / 1_000_000.0 / per_beat,
            };
            last = tick;
            if let Event::Tempo { us_per_beat: us } = event {
                us_per_beat = f64::from(us);
                continue;
            }
            output.push((seconds, event));
        }
        output
    }
}

/// A voice's waveform.
///
/// The "all harmonics" shapes are **parameterized by the rolloff exponent** — `1/k^e` — instead
/// of being fixed cases. The reason is measured: with four fixed shapes (sawtooth = `1/k`, and the
/// other three steps) the soundfont targets fell **between** them, and the centroid error was
/// 1379 Hz for guitar, 1706 Hz for strings, 1926 Hz for brass and 2195 Hz for distortion — four
/// instruments that ended up with the same timbre. With the continuous exponent, each family gets
/// the point the measurement asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Waveform {
    Sine,
    /// Odd harmonics only, falling off as `1/k`.
    Square,
    /// **All** harmonics, falling off as `1/k^exponent`.
    ///
    /// The large exponent is what gives the strong even harmonic that none of the odd-harmonic
    /// shapes has — and that is exactly what Double Dragon's bass needed: the real sample has
    /// H2/H1 = 1.1, and the triangle that was there gave **zero**.
    Partial {
        exponent: f32,
    },
    /// Percussion: noise with decay, with no definite pitch.
    Noise,
}

impl Waveform {
    /// The shape's sample at phase `phase` (one cycle is 1.0).
    ///
    /// The harmonic shapes are **summed**, not generated from the raw shape. A raw square at
    /// 22 kHz **aliases**: the harmonics above half the rate fold back as frequencies that do not
    /// exist in the score, and the result is a note accompanied by a whistle that rises when the
    /// note falls.
    fn sample(self, phase: f32, harmonics: u32, noise: &mut u32) -> f32 {
        use std::f32::consts::TAU;
        match self {
            Self::Sine => (phase * TAU).sin(),
            Self::Square => {
                let mut sum = 0.0;
                let mut k = 1;
                while k <= harmonics.min(9) {
                    sum += (phase * TAU * k as f32).sin() / k as f32;
                    k += 2;
                }
                sum * 4.0 / std::f32::consts::PI
            }
            Self::Partial { exponent } => {
                // The cap is Nyquist's, not a fixed number: with `1/k^e` small enough the high
                // harmonics **are** the timbre, and cutting them at 24 left the sawtooth lead at
                // 2823 Hz when the real sample measures 3497 Hz. At large exponents the high
                // harmonics no longer weigh in, so the extra cost only shows up where it is the
                // sound.
                let mut sum = 0.0;
                for k in 1..=harmonics.min(48) {
                    sum += (phase * TAU * k as f32).sin() / (k as f32).powf(exponent);
                }
                sum
            }
            Self::Noise => {
                // Linear congruential: cheap, deterministic and enough for percussion. Noise that
                // changes from run to run would make the mixer test flaky.
                *noise = noise.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*noise >> 8) as f32 / 8_388_608.0 - 1.0
            }
        }
    }
}

/// One cycle of the harmonic series `1/k^exponent`, precomputed.
///
/// **It exists because of a measurement, not for elegance.** Summing `sin()` per harmonic **for
/// every sample** is expensive: measured in `--release`, a 47.5 s song took **6.25 s** to
/// synthesize — and that work happens inside API dispatch, in the game's `Play` call, so every
/// song that started froze the game for six seconds on this machine. The device is much slower.
///
/// The trick is the classic one from additive synthesis: one sine cycle is computed **once**, and
/// each harmonic is read from that same cycle at `k` times the speed. That way each harmonic costs
/// one read, not one `sin()`.
struct Wavetable {
    samples: Vec<f32>,
}

/// Size of the precomputed cycle, in samples.
///
/// It has to accommodate the highest harmonic: with 48 harmonics, 1024 points give 21 samples per
/// cycle of the highest one, which with linear interpolation reproduces the wave with no audible
/// stepping.
const TABLE_POINTS: usize = 1024;

impl Wavetable {
    /// Builds the cycle of `1/k^exponent` with `harmonics` terms, with the peak at 1.
    ///
    /// Normalizing here **replaces** per-note normalization: the series' peak depends on the
    /// exponent, and one table per exponent already solves that without sweeping phases for every
    /// note.
    fn create(exponent: f32, harmonics: u32) -> Self {
        use std::f32::consts::TAU;
        let points = TABLE_POINTS;
        let base: Vec<f32> = (0..points)
            .map(|i| (TAU * i as f32 / points as f32).sin())
            .collect();
        let mut sum = vec![0.0f32; points];
        for k in 1..=harmonics.clamp(1, 48) {
            let weight = 1.0 / (k as f32).powf(exponent);
            for (i, value) in sum.iter_mut().enumerate() {
                *value += base[(i * k as usize) % points] * weight;
            }
        }
        let largest = sum.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        if largest > 0.0 {
            for value in &mut sum {
                *value /= largest;
            }
        }
        Self { samples: sum }
    }

    /// The sample at phase `phase` (one cycle is 1.0), interpolated between the two neighboring
    /// points.
    ///
    /// The interpolation is not decoration: without it, a high note read at 1024 points per cycle
    /// hisses, which is the same defect the per-harmonic sum avoided.
    fn sample(&self, phase: f32) -> f32 {
        let points = self.samples.len();
        let x = phase * points as f32;
        let i = x as usize % points;
        let fraction = x - x.floor();
        let a = self.samples[i];
        let b = self.samples[(i + 1) % points];
        a + (b - a) * fraction
    }
}

/// How an instrument sounds: shape, envelope and gain.
#[derive(Debug, Clone, Copy)]
struct Timbre {
    shape: Waveform,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    gain: f32,
}

/// The timbre of a General MIDI program.
///
/// **This is still an approximation**, for lack of data, not out of laziness: the console's
/// instrument bank is in the firmware we have not read yet. But the approximation used to be **per
/// family of eight**, and that cost dearly in a measured way: 29 (*overdrive* guitar), 30
/// (distortion), 42 (cello) and 48 (strings) came out with the **same wave**, and 81 (sawtooth
/// *lead*) came out square. In Double Dragon those first three families are 23.2% of the notes.
///
/// Now each program the score uses is handled separately when it differs from its neighbor, and
/// the families cover the rest. The target of the correction is what the comparison with the
/// soundfont measured, not taste:
///
/// | program | measured in the soundfont | what the table did |
/// |---|---|---|
/// | 0 piano | H2/H1 = 0.59, decays to 0.49 in 1.2 s | triangle (H2 = 0) and stopped at 0.29 |
/// | 33 bass | H2/H1 = 1.10, decays to 0.42 in 1.2 s | H2 = 0 and **flat** at 0.65 |
/// | 29 guitar | H2/H1 = 0.82, sustains at 0.75 | decayed to 0.36 and stuck |
/// | 30 distortion | H2/H1 = 4.58, centroid 2195 Hz | identical to 29 |
/// | 48 strings | H2/H1 = 8.37, centroid 1706 Hz | identical to 29 |
/// | 42 cello | H2/H1 = 0.69, rises and sustains | identical to 29 |
///
/// Sustain and decay have to be read together with the note's duration: in a bass line the notes
/// are short, and a high `sustain` turns into a flat buzz instead of a note.
fn timbre(program: u8) -> Timbre {
    let (shape, attack, decay, sustain, release, gain) = match program {
        // Piano: all harmonics, dry attack and long tail. The long decay, with low sustain, is
        // what makes the note **keep falling** instead of getting stuck.
        0..=3 => (Waveform::Partial { exponent: 2.00 }, 0.002, 1.60, 0.22, 0.18, 0.9),
        // Electric piano and harpsichord: brighter and shorter.
        4..=7 => (Waveform::Partial { exponent: 1.70 }, 0.002, 0.90, 0.20, 0.15, 0.85),
        // Chromatic percussion (bell, marimba, xylophone): brightness with a medium tail.
        8..=15 => (Waveform::Partial { exponent: 1.00 }, 0.002, 0.80, 0.15, 0.14, 0.8),
        // Organ and accordion: they sound while the key is held.
        16..=20 => (Waveform::Square, 0.010, 0.05, 0.90, 0.08, 0.6),
        // Reed organ and harmonica: square with body on top.
        21..=23 => (Waveform::Partial { exponent: 1.20 }, 0.015, 0.08, 0.88, 0.09, 0.55),
        // Nylon and steel-string acoustic guitar: plucked, decays.
        24 | 25 => (Waveform::Partial { exponent: 1.80 }, 0.004, 0.90, 0.20, 0.12, 0.75),
        // Clean and *jazz* electric guitar: soft attack, sustains.
        26..=28 => (Waveform::Partial { exponent: 1.50 }, 0.008, 0.35, 0.62, 0.14, 0.7),
        // *Overdrive*: sustains, and it is 10.8% of Double Dragon.
        29 => (Waveform::Partial { exponent: 1.45 }, 0.006, 0.30, 0.76, 0.16, 0.68),
        // Distortion and harmonics: brighter than *overdrive* — measured in the soundfont,
        // H2/H1 = 4.6 and a centroid of 2195 Hz against 1379 Hz for 29. It was the **same wave**.
        30..=31 => (Waveform::Partial { exponent: 1.10 }, 0.005, 0.30, 0.78, 0.16, 0.6),
        // Bass: strong even harmonic (the real sample has H2 above the fundamental) and long tail.
        32..=35 => (Waveform::Partial { exponent: 2.45 }, 0.005, 2.00, 0.30, 0.12, 1.0),
        // *Pick*, *slap* and synth bass: shorter and brighter.
        36..=39 => (Waveform::Partial { exponent: 1.80 }, 0.004, 0.80, 0.32, 0.10, 0.95),
        // Violin, viola and cello: bowed, medium attack, sustains with undulation.
        40..=42 => (Waveform::Partial { exponent: 1.40 }, 0.090, 0.30, 0.80, 0.22, 0.7),
        // Ensemble contrabass: lower and darker.
        43 => (Waveform::Partial { exponent: 2.20 }, 0.080, 0.40, 0.72, 0.22, 0.85),
        // *Tremolo*, *pizzicato* and harp: short plucks.
        44 | 45 | 46 => (Waveform::Partial { exponent: 1.60 }, 0.004, 0.60, 0.25, 0.14, 0.75),
        // Timpani: noise with a tail, treated as percussion with no definite pitch.
        47 => (Waveform::Partial { exponent: 1.20 }, 0.003, 0.70, 0.20, 0.20, 0.9),
        // String section: the brightest family in the soundfont (H2/H1 = 8.4), slow attack.
        48..=55 => (Waveform::Partial { exponent: 1.30 }, 0.075, 0.35, 0.78, 0.28, 0.62),
        // Brass: rich, with medium attack and good sustain.
        56..=60 => (Waveform::Partial { exponent: 1.19 }, 0.035, 0.25, 0.78, 0.14, 0.62),
        // Muted and synth brass: darker.
        61..=63 => (Waveform::Partial { exponent: 1.19 }, 0.030, 0.30, 0.75, 0.13, 0.62),
        // Reed (sax, clarinet): square with body.
        64..=71 => (Waveform::Partial { exponent: 1.50 }, 0.025, 0.25, 0.78, 0.12, 0.7),
        // Flute and pipe: almost a sine, medium attack.
        72..=79 => (Waveform::Sine, 0.035, 0.20, 0.82, 0.14, 0.8),
        // *Lead*: 80 is square and **81 is sawtooth** — it was the table's most audible defect.
        80 => (Waveform::Square, 0.006, 0.20, 0.78, 0.11, 0.6),
        81 => (Waveform::Partial { exponent: 0.70 }, 0.006, 0.20, 0.78, 0.11, 0.55),
        82 | 83 => (Waveform::Partial { exponent: 1.80 }, 0.010, 0.25, 0.75, 0.12, 0.7),
        84..=87 => (Waveform::Partial { exponent: 1.40 }, 0.008, 0.25, 0.76, 0.12, 0.62),
        // Pad and effect: long fade in and fade out.
        88..=95 => (Waveform::Partial { exponent: 1.20 }, 0.150, 0.40, 0.75, 0.40, 0.6),
        96..=103 => (Waveform::Partial { exponent: 1.60 }, 0.100, 0.40, 0.70, 0.40, 0.6),
        // Ethnic: plucked, like the acoustic guitar.
        104..=111 => (Waveform::Partial { exponent: 1.80 }, 0.006, 0.70, 0.25, 0.14, 0.75),
        // Pitched percussive (bell, tuned drum): decays fast.
        112..=119 => (Waveform::Partial { exponent: 1.00 }, 0.002, 0.40, 0.10, 0.12, 0.7),
        // Sound effects.
        _ => (Waveform::Partial { exponent: 1.60 }, 0.005, 0.50, 0.30, 0.15, 0.7),
    };
    Timbre {
        shape,
        attack,
        decay,
        sustain,
        release,
        gain,
    }
}

/// What to do with the percussion noise: nothing, cut the treble, or cut the bass.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Filter {
    Off,
    /// Low-pass at the given frequency. It is what makes the kick and the floor tom.
    LowPass(f32),
    /// High-pass at the given frequency. It is what makes the hi-hat and the cymbal.
    HighPass(f32),
}

/// The timbre of a percussion note, by its number.
///
/// There is no pitch: what changes between a kick and a cymbal is how long the noise lasts and
/// **in which band it lives**.
///
/// The band is what was wrong before, and the symptom was heard before it was explained: in Double
/// Dragon the hi-hat disappeared precisely in the stretches of continuous bass. The cause is
/// **masking**: hi-hat and cymbal got a low-pass, kept the low end and lost the brightness, and
/// ended up living in the same band as the bass — two sounds in the same band, and the louder one
/// eats the quieter one. The hi-hat is treble: its filter is a **high**-pass, and then it sits
/// above the bass and can be heard without needing any extra volume.
fn percussion(note: u8) -> (Timbre, Filter) {
    let (decay, filter, gain) = match note {
        // Kick and floor tom: only the low end.
        35 | 36 | 41 | 43 | 45 | 47 | 48 | 50 => (0.18, Filter::LowPass(120.0), 1.0),
        // Snare and claps: body in the middle, with the crack on top. It is the loudest drum in
        // the kit, measured in the soundfont (0.90 of the peak, against 0.32 for the hi-hat).
        37..=40 => (0.14, Filter::HighPass(700.0), 0.9),
        // Ride and crash cymbals: long and bright, and **louder** than the hi-hat — it used to be
        // the reverse, and the measured effect was the ride vanishing behind the closed hi-hat.
        49 | 51 | 52 | 53 | 55 | 57 | 59 => (0.45, Filter::HighPass(4_000.0), 0.75),
        // Closed hi-hat and the rest: short, bright and **quiet**. In the soundfont it is 0.32 of
        // the peak; here it was 0.60, and that is what made the drums sound "tss-tss" over
        // everything.
        _ => (0.07, Filter::HighPass(5_000.0), 0.35),
    };
    (
        Timbre {
            shape: Waveform::Noise,
            attack: 0.001,
            decay,
            sustain: 0.0,
            release: 0.02,
            gain,
        },
        filter,
    )
}

/// A sounding note, from `Note On` until the envelope reaches zero.
#[derive(Debug, Clone, Copy)]
struct Voice {
    channel: u8,
    note: u8,
    /// Index of this voice's precomputed wave, when it uses one. See [`Wavetable`].
    ///
    /// It is an index, not an `Arc`, so that `Voice` stays `Copy`: the mixing loop copies voices
    /// when releasing them, and switching that to ownership would spread changes all over the file.
    table: Option<usize>,
    /// Sample at which the note started and at which it was released.
    start: usize,
    released_at: Option<usize>,
    frequency: f32,
    timbre: Timbre,
    /// The percussion filter. A pitched note gets no filter.
    filter: Filter,
    amplitude: f32,
}

impl Voice {
    /// The envelope at `t` seconds since the start, with the note released at `released_at`
    /// seconds.
    fn envelope(&self, t: f32, released_at: Option<f32>) -> f32 {
        let Timbre {
            attack,
            decay,
            sustain,
            release,
            ..
        } = self.timbre;
        if t < 0.0 {
            return 0.0;
        }
        let held = match t < attack {
            true => t / attack.max(1e-6),
            false => {
                let since = t - attack;
                match since < decay {
                    true => 1.0 - (1.0 - sustain) * (since / decay.max(1e-6)),
                    false => sustain,
                }
            }
        };
        match released_at {
            Some(released_at) if t > released_at => {
                let since = t - released_at;
                match since < release {
                    true => held * (1.0 - since / release.max(1e-6)),
                    false => 0.0,
                }
            }
            _ => held,
        }
    }

    /// How many samples this voice still produces sound for after being released.
    fn tail(&self) -> usize {
        (self.timbre.release * RATE as f32) as usize + 1
    }
}

/// Synthesizes a MIDI. `None` when it is not a readable MIDI.
///
/// Returns the same [`Sound`](crate::audio::wav::Sound) that WAVE and MP3 produce — the mixer does
/// not need to know that there was no sound here at all, only a score.
pub fn decode(data: &[u8]) -> Option<crate::audio::wav::Sound> {
    let part = prepare(data)?;
    synthesize_blocking(part)
}

/// The same synthesis, without holding up the caller when the song is long.
///
/// On the Vita the work for a track tens of seconds long lands on the frame's thread and
/// the picture stops until it finishes. Below two seconds it stays right here: an
/// effect is not worth a thread, and whoever reads the sound next already finds the sample.
pub fn decode_nonblocking(data: &[u8]) -> Option<crate::audio::wav::Sound> {
    let part = prepare(data)?;
    if part.total <= (RATE as usize) * 2 {
        return synthesize_blocking(part);
    }
    let live = match alloc_synth_buffer(part.total) {
        SynthBuffer::Wide(samples) => std::sync::Arc::new(crate::audio::wav::LiveBuffer::from_f32(samples)),
        SynthBuffer::Short(pcm) => {
            if pcm.is_empty() {
                return None;
            }
            if pcm.len() < part.total {
                crate::logging!(
                    crate::logging::Level::Warning,
                    "midi",
                    "the {:.1}s song did not fit whole; synthesizing {:.1}s",
                    part.total as f64 / f64::from(RATE),
                    pcm.len() as f64 / f64::from(RATE)
                );
            }
            std::sync::Arc::new(crate::audio::wav::LiveBuffer::from_i16(pcm))
        }
    };
    let sound = crate::audio::wav::Sound::synthesizing(RATE, 1, std::sync::Arc::clone(&live));
    let job = std::sync::Arc::new(std::sync::Mutex::new(Some(part)));
    let live_for_thread = std::sync::Arc::clone(&live);
    let job_for_thread = std::sync::Arc::clone(&job);
    let created = std::thread::Builder::new()
        .name("zeebx-midi".into())
        .stack_size(128 * 1024)
        .spawn(move || {
            // The thread is born with the priority of whoever created it. On the device that is
            // the game's, and synthesizing a long track takes a whole core at the
            // same level as the interpreter and the filling — the frame loses a
            // third. Low priority only runs when those two are not using
            // the core.
            #[cfg(target_os = "vita")]
            vita_low_priority();
            if let Some(part) = job_for_thread.lock().ok().and_then(|mut g| g.take()) {
                fill_live(&live_for_thread, &part.released, &part.tables);
            }
        });
    if created.is_err() {
        if let Some(part) = job.lock().ok().and_then(|mut g| g.take()) {
            fill_live(&live, &part.released, &part.tables);
        }
    }
    Some(sound)
}

/// The Vita's user range goes from 64 (the most urgent) to 191. The game and the
/// filling stay at the top; the music waits for the leftovers.
#[cfg(target_os = "vita")]
fn vita_low_priority() {
    unsafe extern "C" {
        fn sceKernelGetThreadId() -> i32;
        fn sceKernelChangeThreadPriority(thid: i32, priority: i32) -> i32;
    }
    unsafe {
        let id = sceKernelGetThreadId();
        if id > 0 {
            let _ = sceKernelChangeThreadPriority(id, 191);
        }
    }
}

struct Part {
    released: Vec<Voice>,
    tables: Vec<Wavetable>,
    total: usize,
}

fn prepare(data: &[u8]) -> Option<Part> {
    let score = read(data)?;
    let events = score.in_seconds();
    let end = events.last()?.0.min(MAX_SECONDS);
    // Half a second of slack for the last note to finish releasing.
    let total = ((end + 0.5) * f64::from(RATE)) as usize + 1;

    let mut program = [0u8; 16];
    let mut volume = [1.0f32; 16];
    let mut expression = [1.0f32; 16];
    // The precomputed waves, one per (exponent, number of harmonics). Several voices
    // share the same one: in a song of 1788 notes the distinct pairs are a few hundred, and
    // each table costs one sine cycle plus one read per harmonic.
    let mut tables: Vec<Wavetable> = Vec::new();
    let mut table_index: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    let mut sounding: Vec<Voice> = Vec::new();
    let mut released: Vec<Voice> = Vec::new();

    let sample_at = |seconds: f64| (seconds * f64::from(RATE)) as usize;
    for &(seconds, event) in &events {
        if seconds > MAX_SECONDS {
            break;
        }
        let now = sample_at(seconds);
        match event {
            Event::Program { channel, program: p } => program[channel as usize & 15] = p,
            Event::Volume { channel, value } => {
                volume[channel as usize & 15] = f32::from(value) / 127.0
            }
            Event::Expression { channel, value } => {
                expression[channel as usize & 15] = f32::from(value) / 127.0
            }
            Event::Tempo { .. } => {}
            Event::AllNotesOff { channel } => {
                for voice in sounding.iter_mut().filter(|v| v.channel == channel) {
                    voice.released_at.get_or_insert(now);
                }
                released.extend(sounding.iter().copied().filter(|v| v.channel == channel));
                sounding.retain(|v| v.channel != channel);
            }
            Event::NoteOff { channel, note } => {
                // Only the oldest note is released, not all the equal ones: the same note played
                // twice before being released is two voices, and releasing both at once cuts the
                // second one too early.
                if let Some(i) = sounding
                    .iter()
                    .position(|v| v.channel == channel && v.note == note)
                {
                    let mut voice = sounding.remove(i);
                    voice.released_at = Some(now);
                    released.push(voice);
                }
            }
            Event::NoteOn { channel, note, velocity } => {
                // Full: **release the oldest voice** instead of refusing the new one. Refusing
                // looks harmless and is a disguised priority rule — and the worst possible one:
                // what stays is the long note already sounding, what loses is the short note that
                // just arrived. In a song with continuous bass, that swallows exactly the
                // percussion, which is what carries the rhythm.
                if sounding.len() >= MAX_VOICES {
                    let mut voice = sounding.remove(0);
                    voice.released_at = Some(now);
                    released.push(voice);
                }
                let channel_idx = channel as usize & 15;
                let (timbre, filter, frequency, table) = match channel == PERCUSSION_CHANNEL {
                    true => {
                        let (timbre, filter) = percussion(note);
                        (timbre, filter, 0.0, None)
                    }
                    false => {
                        let timbre = timbre(program[channel_idx]);
                        // MIDI tuning: note 69 is the 440 Hz A, and each semitone is the twelfth
                        // root of two.
                        let frequency = 440.0 * 2.0f32.powf((f32::from(note) - 69.0) / 12.0);
                        // How many harmonics fit below Nyquist at this pitch. It is the same
                        // number the synthesis will use, and it is what the normalization needs.
                        let harmonics = match frequency > 0.0 {
                            true => (RATE as f32 / 2.0 / frequency) as u32,
                            false => 1,
                        };
                        let mut index = None;
                        if let Waveform::Partial { exponent } = timbre.shape {
                            let key_id = (exponent.to_bits(), harmonics.max(1));
                            index = match table_index.get(&key_id) {
                                Some(&i) => Some(i),
                                None => {
                                    let i = tables.len();
                                    tables.push(Wavetable::create(exponent, harmonics.max(1)));
                                    table_index.insert(key_id, i);
                                    Some(i)
                                }
                            };
                        }
                        (timbre, Filter::Off, frequency, index)
                    }
                };
                sounding.push(Voice {
                    channel,
                    note,
                    table,
                    start: now,
                    released_at: None,
                    frequency,
                    timbre,
                    filter,
                    amplitude: f32::from(velocity) / 127.0
                        * volume[channel_idx]
                        * expression[channel_idx]
                        * timbre.gain,
                });
            }
        }
    }
    // A note the song never released: released at the end, so it does not sustain forever.
    let end_sample = sample_at(end);
    for mut voice in sounding {
        voice.released_at = Some(end_sample);
        released.push(voice);
    }

    Some(Part {
        released,
        tables,
        total,
    })
}

fn synthesize_blocking(part: Part) -> Option<crate::audio::wav::Sound> {
    let Part {
        released,
        tables,
        total,
    } = part;
    // The wide buffer is the usual path. On the Vita the 86.67 s track asks for
    // 7,688,424 contiguous bytes and `malloc` fails with the heap already fragmented; then the
    // same sum goes to 16 bits, half the size, and if even that does not fit
    // what remains is the prefix that fits — the music restarts sooner, and the process does not
    // crash.
    match alloc_synth_buffer(total) {
        SynthBuffer::Wide(mut samples) => {
            for voice in &released {
                render_voice(voice, &mut samples, &tables);
            }
            if samples.iter().all(|s| *s == 0.0) {
                return None;
            }
            apply_fixed_gain(&mut samples);
            Some(crate::audio::wav::Sound::create(RATE, 1, samples))
        }
        SynthBuffer::Short(mut pcm) => {
            if pcm.len() < total {
                crate::logging!(
                    crate::logging::Level::Warning,
                    "midi",
                    "the {:.1}s song did not fit whole; synthesizing {:.1}s",
                    total as f64 / f64::from(RATE),
                    pcm.len() as f64 / f64::from(RATE)
                );
            }
            fill_compact(&released, &tables, &mut pcm);
            if pcm.iter().all(|s| *s == 0) {
                return None;
            }
            Some(crate::audio::wav::Sound::from_i16(RATE, 1, pcm))
        }
    }
}

enum SynthBuffer {
    Wide(Vec<f32>),
    Short(Vec<i16>),
}

/// The synthesis buffer. `try_reserve_exact` returns an error instead of aborting the process.
fn alloc_synth_buffer(total: usize) -> SynthBuffer {
    let mut wide = Vec::new();
    if wide.try_reserve_exact(total).is_ok() {
        wide.resize(total, 0.0);
        return SynthBuffer::Wide(wide);
    }
    let mut n = total;
    loop {
        let mut short = Vec::new();
        if short.try_reserve_exact(n).is_ok() {
            short.resize(n, 0);
            return SynthBuffer::Short(short);
        }
        if n <= 1024 {
            // Not even this fit. `resize` would abort the same way the old `vec!`
            // did; empty makes `decode` give up without bringing the process down.
            return SynthBuffer::Short(Vec::new());
        }
        n /= 2;
    }
}

/// Sums the voices in 16 bits, one window at a time.
///
/// The wide buffer did not fit. The percussion (filtered, short) is computed separately;
/// the rest accumulates in a 4096-sample window that is reused. The fixed
/// gain is applied when writing, and the peak limiter runs a second time,
/// as in [`apply_fixed_gain`].
fn fill_compact(voices: &[Voice], tables: &[Wavetable], pcm: &mut [i16]) {
    let filtered: Vec<(usize, Vec<f32>)> = voices
        .iter()
        .filter_map(|voice| filtered_contribution(voice, tables))
        .collect();
    let mut cursors: Vec<Cursor> = voices
        .iter()
        .filter(|voice| matches!(voice.filter, Filter::Off))
        .map(Cursor::create)
        .collect();
    let mut window = vec![0.0f32; WINDOW.min(pcm.len()).max(1)];
    let mut peak = 0.0f32;
    let mut pos = 0usize;
    while pos < pcm.len() {
        let n = WINDOW.min(pcm.len() - pos);
        window[..n].fill(0.0);
        for cursor in &mut cursors {
            cursor.sum(&mut window[..n], pos, tables);
        }
        for (base, buf) in &filtered {
            if *base >= pos + n || base + buf.len() <= pos {
                continue;
            }
            let src_start = pos.saturating_sub(*base);
            let src_end = (pos + n - *base).min(buf.len());
            for (i, value) in buf[src_start..src_end].iter().enumerate() {
                let dest = *base + src_start + i - pos;
                if let Some(slot) = window.get_mut(dest) {
                    *slot += *value;
                }
            }
        }
        for (i, sample) in window[..n].iter().enumerate() {
            let gain = sample * TABLE_GAIN;
            peak = peak.max(gain.abs());
            pcm[pos + i] = (gain * 32767.0).clamp(-32767.0, 32767.0) as i16;
        }
        pos += n;
    }
    if peak > CAP {
        let factor = CAP / peak;
        for sample in pcm.iter_mut() {
            *sample = (f32::from(*sample) * factor) as i16;
        }
    }
}

/// The same window sum, publishing each piece as soon as it exists.
///
/// Whoever plays holds the voice while the sample has not arrived, so the music
/// starts from the beginning and the game's frame does not wait for the whole track.
fn fill_live(live: &crate::audio::wav::LiveBuffer, voices: &[Voice], tables: &[Wavetable]) {
    let mut cursors: Vec<Cursor> = voices
        .iter()
        .filter(|voice| matches!(voice.filter, Filter::Off))
        .map(Cursor::create)
        .collect();
    let mut deferred_filters: Vec<DeferredFilter> = voices
        .iter()
        .filter(|voice| !matches!(voice.filter, Filter::Off))
        .copied()
        .map(DeferredFilter::create)
        .collect();
    let total = live.total();
    if total == 0 {
        live.publish(0);
        return;
    }
    let mut window = vec![0.0f32; WINDOW.min(total).max(1)];
    let mut pos = 0usize;
    while pos < total {
        let n = WINDOW.min(total - pos);
        let chunk = &mut window[..n];
        chunk.fill(0.0);
        for cursor in &mut cursors {
            cursor.sum(chunk, pos, tables);
        }
        for deferred in &mut deferred_filters {
            if !deferred.overlaps(pos, n) {
                continue;
            }
            let Some((base, buf)) = deferred.samples(tables) else {
                continue;
            };
            if base >= pos + n || base + buf.len() <= pos {
                continue;
            }
            let from = pos.saturating_sub(base);
            let end = (pos + n - base).min(buf.len());
            for (i, value) in buf[from..end].iter().enumerate() {
                let dest = base + from + i - pos;
                if let Some(slot) = chunk.get_mut(dest) {
                    *slot += *value;
                }
            }
        }
        let mut peak = 0.0f32;
        for sample in chunk.iter_mut() {
            *sample *= TABLE_GAIN;
            peak = peak.max(sample.abs());
        }
        if peak > CAP {
            let factor = CAP / peak;
            for sample in chunk.iter_mut() {
                *sample *= factor;
            }
        }
        if live.is_wide() {
            live.write_f32(pos, chunk);
        } else {
            let mut pcm = vec![0i16; n];
            for (i, sample) in chunk.iter().enumerate() {
                pcm[i] = (sample * 32767.0).clamp(-32767.0, 32767.0) as i16;
            }
            live.write_i16(pos, &pcm);
        }
        pos += n;
        live.publish(pos);
    }
}

struct DeferredFilter {
    voice: Voice,
    ready: Option<(usize, Vec<f32>)>,
}

impl DeferredFilter {
    fn create(voice: Voice) -> Self {
        Self { voice, ready: None }
    }

    fn overlaps(&self, pos: usize, n: usize) -> bool {
        let end = self
            .voice
            .released_at
            .map(|s| s + self.voice.tail())
            .unwrap_or(self.voice.start);
        self.voice.start < pos + n && end > pos
    }

    fn samples(&mut self, tables: &[Wavetable]) -> Option<(usize, &[f32])> {
        if self.ready.is_none() {
            self.ready = filtered_contribution(&self.voice, tables);
        }
        self.ready.as_ref().map(|(base, buf)| (*base, buf.as_slice()))
    }
}

const WINDOW: usize = 4096;

struct Cursor {
    voice: Voice,
    phase: f32,
    step: f32,
    noise: u32,
    harmonics: u32,
    /// Next absolute sample this voice will still write.
    next: usize,
    end: usize,
}

impl Cursor {
    fn create(voice: &Voice) -> Self {
        let rate = RATE as f32;
        let harmonics = match voice.frequency > 0.0 {
            true => (rate / 2.0 / voice.frequency) as u32,
            false => 1,
        };
        let end = match voice.released_at {
            Some(s) => s + voice.tail(),
            None => usize::MAX,
        };
        Self {
            voice: *voice,
            phase: 0.0,
            step: voice.frequency / rate,
            noise: 0x1234_5678u32 ^ ((voice.note as u32) << 16) ^ voice.start as u32,
            harmonics,
            next: voice.start,
            end,
        }
    }

    fn sum(&mut self, window: &mut [f32], pos: usize, tables: &[Wavetable]) {
        let rate = RATE as f32;
        let released_at = self
            .voice
            .released_at
            .map(|s| (s.saturating_sub(self.voice.start)) as f32 / rate);
        let window_end = pos + window.len();
        while self.next < window_end && self.next < self.end {
            let i = self.next - self.voice.start;
            let t = i as f32 / rate;
            let envelope = self.voice.envelope(t, released_at);
            if envelope <= 0.0 && released_at.is_some_and(|s| t > s) {
                self.next = self.end;
                break;
            }
            let raw = match self.voice.table.and_then(|i| tables.get(i)) {
                Some(table) => table.sample(self.phase),
                None => self
                    .voice
                    .timbre
                    .shape
                    .sample(self.phase, self.harmonics.max(1), &mut self.noise),
            };
            self.phase = (self.phase + self.step).fract();
            let dest = self.next - pos;
            window[dest] += raw * envelope * self.voice.amplitude;
            self.next += 1;
        }
    }
}

/// The filtered percussion, already on the energy scale, ready to be summed.
///
/// It is short (the longest in the table is just over half a second). It fits in a `Vec`
/// of its own; the melodic note, which lasts the whole song, does not.
fn filtered_contribution(voice: &Voice, tables: &[Wavetable]) -> Option<(usize, Vec<f32>)> {
    if matches!(voice.filter, Filter::Off) {
        return None;
    }
    let base = voice.start;
    let end = match voice.released_at {
        Some(s) => s + voice.tail(),
        None => return None,
    };
    let len = end.saturating_sub(base);
    if len == 0 {
        return None;
    }
    let mut buf = vec![0.0f32; len];
    let mut shifted = *voice;
    shifted.start = 0;
    shifted.released_at = shifted.released_at.map(|s| s.saturating_sub(base));
    play_voice_into(&shifted, &mut buf, tables, voice.start);
    Some((base, buf))
}

/// Sums one voice into the buffer, from its start until the envelope reaches zero.
fn render_voice(voice: &Voice, samples: &mut [f32], tables: &[Wavetable]) {
    play_voice_into(voice, samples, tables, voice.start);
}

/// `seed` is the note's original position. The short path restarts the buffer at
/// zero so as not to allocate the whole song, and without this the percussion noise changed.
fn play_voice_into(voice: &Voice, samples: &mut [f32], tables: &[Wavetable], seed: usize) {
    let rate = RATE as f32;
    let released_at = voice
        .released_at
        .map(|s| (s.saturating_sub(voice.start)) as f32 / rate);
    let end = match voice.released_at {
        Some(s) => (s + voice.tail()).min(samples.len()),
        None => samples.len(),
    };
    // How many harmonics fit below Nyquist. One more and the note gains a whistle that is not in
    // the score.
    let harmonics = match voice.frequency > 0.0 {
        true => (rate / 2.0 / voice.frequency) as u32,
        false => 1,
    };
    let mut noise = 0x1234_5678u32 ^ ((voice.note as u32) << 16) ^ seed as u32;
    // One-pole RC filter: `alpha = fc / (fc + fs/2π)`. The high-pass uses **two** in cascade,
    // because a single one falls 6 dB per octave and lets too much bass through — with one pole
    // the hi-hat still fought over the bass's band, which is the defect that was to be fixed.
    let cutoff = match voice.filter {
        Filter::Off => 0.0,
        Filter::LowPass(hz) | Filter::HighPass(hz) => hz,
    };
    let alpha = match cutoff > 0.0 {
        true => (cutoff / (cutoff + rate / std::f32::consts::TAU)).clamp(0.01, 1.0),
        false => 1.0,
    };
    let mut pole = [0.0f32; 2];
    let mut phase = 0.0f32;
    let step = voice.frequency / rate;
    let Some(chunk) = samples.get_mut(voice.start..end) else {
        return;
    };

    // An unfiltered voice goes straight into the buffer. A filtered voice is assembled separately
    // and then scaled, because filtering removes energy — a low-pass at 120 Hz returns a twentieth
    // of the noise — and without correcting for that the number in the table means nothing.
    // Percussion is short, so the extra buffer is small.
    let mut filtered_voice = match voice.filter {
        Filter::Off => Vec::new(),
        _ => Vec::with_capacity(chunk.len()),
    };
    for (i, dest) in chunk.iter_mut().enumerate() {
        let t = i as f32 / rate;
        let envelope = voice.envelope(t, released_at);
        if envelope <= 0.0 && released_at.is_some_and(|s| t > s) {
            break;
        }
        let raw = match voice.table.and_then(|i| tables.get(i)) {
            // Precomputed wave: one interpolated read per sample, instead of a sum of
            // harmonics. It is what removed the six seconds of freezing per song.
            Some(table) => table.sample(phase),
            // The shapes without a precomputed wave are the ones with few harmonics (sine,
            // square, noise), and for them summing per sample is cheap.
            None => voice.timbre.shape.sample(phase, harmonics.max(1), &mut noise),
        };
        phase = (phase + step).fract();
        pole[0] += alpha * (raw - pole[0]);
        let value = match voice.filter {
            Filter::Off => raw,
            Filter::LowPass(_) => pole[0],
            // What is left after removing the low end is the treble, and the second pole removes
            // what is left of it: it is the complementary high-pass, built from the same state.
            Filter::HighPass(_) => {
                let first = raw - pole[0];
                pole[1] += alpha * (first - pole[1]);
                first - pole[1]
            }
        };
        match voice.filter {
            Filter::Off => *dest += value * envelope * voice.amplitude,
            _ => filtered_voice.push(value * envelope),
        }
    }
    if filtered_voice.is_empty() {
        return;
    }
    // **Match energy, not peak.** Matching peaks was the first attempt and it fails for a reason
    // you can hear: a noise burst has a high peak and low energy, while a sustained bass has both
    // about the same. With peaks matched, the percussion ends up many times quieter than a melodic
    // note with the same gain — and in an arrangement like Double Dragon's, with nine channels
    // sounding, the hi-hat simply cannot be heard.
    //
    // The target is the energy of a melodic note of the same amplitude, which is about 0.7 of it.
    let energy = (filtered_voice.iter().map(|s| f64::from(s * s)).sum::<f64>()
        / filtered_voice.len() as f64)
        .sqrt() as f32;
    if energy <= 0.0 {
        return;
    }
    let factor = voice.amplitude * 0.7 / energy;
    for (dest, value) in chunk.iter_mut().zip(&filtered_voice) {
        *dest += value * factor;
    }
}

/// The wavetable's fixed gain, applied to the raw sum of the voices.
///
/// The function that applies it used to be called `normalize`, and the name started lying when
/// the policy changed: it no longer normalizes anything. Now it is `apply_fixed_gain`, which is
/// what it does.
///
/// **It is the number the measurement asked for, and it replaces peak normalization.** The raw
/// sum of this table's voices comes out well above 1.0 — measured on Double Dragon's twelve real
/// tracks, the raw peak goes from 3.09 (the short jingle) to 8.75 (the densest song). Scaling each
/// song by its **own** peak, as was done before, put them all at 0.8 and thereby erased exactly
/// that difference: the four-second jingle came out of the mixer as loud as the combat track, and
/// louder than it in average energy (RMS 0.220 against 0.158).
///
/// With a fixed gain the peaks go to 0.28–0.79 and the difference comes back. The number is 0.09
/// because it is what matches the **average energy** of the two paths: at 0.09 the table measures
/// an average RMS of 0.101 over the twelve tracks, and the sample bank measures 0.101 over the
/// same twelve. So switching the synthesizer in the menu changes the timbre, not the volume.
const TABLE_GAIN: f32 = 0.09;

/// The peak ceiling after the sum, the same as the bank path's.
const CAP: f32 = 0.95;

/// Applies the fixed gain and, only if it still goes over the ceiling, brings the excess down.
///
/// **Fixed gain, not normalization — that is the fix.** Normalizing matches every song's peak and
/// erases the difference in intensity between them; here a quiet song stays quiet. The limiter
/// exists only for the extreme case: what goes past 1.0 **clips**, and clipping is distortion in
/// the mixer — the defect easiest to mistake for "the synthesizer is bad". When it kicks in, it
/// scales the whole song, which preserves the proportion between its voices.
pub(crate) fn apply_fixed_gain(samples: &mut [f32]) {
    for sample in samples.iter_mut() {
        *sample *= TABLE_GAIN;
    }
    let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if peak > CAP {
        let factor = CAP / peak;
        for sample in samples.iter_mut() {
            *sample *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a format 0 SMF with a single track, so the tests do not depend on a file.
    fn smf(division: i16, track: &[u8]) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend(0u16.to_be_bytes()); // format 0
        out.extend(1u16.to_be_bytes()); // one track
        out.extend(division.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    /// A delta time in variable length, as the file writes it.
    ///
    /// Writing 250 in a single byte is the mistake one can make here: the high bit set means
    /// "continues", and the reader would swallow the next status byte as part of the number.
    fn delta(mut value: u32) -> Vec<u8> {
        let mut bytes = vec![(value & 0x7f) as u8];
        value >>= 7;
        while value > 0 {
            bytes.push((value & 0x7f) as u8 | 0x80);
            value >>= 7;
        }
        bytes.reverse();
        bytes
    }

    /// A note that starts at tick zero and is released after `duration` ticks.
    fn one_note(note: u8, duration: u32) -> Vec<u8> {
        let mut out = vec![0x00, 0x90, note, 100];
        out.extend(delta(duration));
        out.extend([0x80, note, 0x40]);
        out.extend([0x00, 0xff, 0x2f, 0x00]);
        out
    }

    /// A long note on a program, to measure the timbre's behavior.
    fn program_note(program: u8, note: u8, duration: u32) -> Vec<u8> {
        let mut track = vec![0x00, 0xc0, program];
        track.extend([0x00, 0x90, note, 100]);
        track.extend(delta(duration));
        track.extend([0x80, note, 0x40]);
        track.extend([0x00, 0xff, 0x2f, 0x00]);
        smf(96, &track)
    }

    /// The energy of a 200 ms window of the sound, by window index.
    fn window_energy(sound: &crate::audio::wav::Sound, window: usize) -> f32 {
        let samples = &sound.samples[window * 4410..(window + 1) * 4410];
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }

    /// The amplitude of a harmonic, in **quadrature**.
    ///
    /// The first version of this correlated with a fixed-phase sine, and the result said nothing:
    /// the sound's phase does not start at zero at the chosen sample, so the correlation could fall
    /// near zero for a harmonic that was there. Measured: distortion and *overdrive* gave 0.123 and
    /// 0.122 — indistinguishable. With sine **and** cosine, the magnitude does not depend on phase,
    /// and it is the same calculation the analysis outside the repository does by FFT.
    fn harmonic(sound: &crate::audio::wav::Sound, f0: f32, k: u32) -> f32 {
        let chunk = &sound.samples[4410..4410 * 3];
        let rate = RATE as f32;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (i, sample) in chunk.iter().enumerate() {
            let angle = (i as f32 / rate) * f0 * k as f32 * std::f32::consts::TAU;
            re += sample * angle.cos();
            im += sample * angle.sin();
        }
        (re * re + im * im).sqrt() * 2.0 / chunk.len() as f32
    }

    /// The ratio between a harmonic and the fundamental.
    fn harmonic_over_fundamental(sound: &crate::audio::wav::Sound, f0: f32, k: u32) -> f32 {
        let base = harmonic(sound, f0, 1).max(1e-6);
        harmonic(sound, f0, k) / base
    }

    /// **The distortion guitar cannot be the same wave as *overdrive*.**
    ///
    /// It was: both fell in the same `24..=31` range of the table, with the same shape and the
    /// same envelope. Measured in Double Dragon, they are 11.6% and 10.8% of the notes — and they
    /// sounded like a single instrument. The soundfont tells them apart by 816 Hz of centroid and
    /// by an H2/H1 of 4.58 against 0.82.
    #[test]
    fn distortion_is_not_the_same_wave_as_overdrive() {
        let overdrive = decode(&program_note(29, 60, 192)).expect("overdrive");
        let distortion = decode(&program_note(30, 60, 192)).expect("distortion");
        let a = harmonic_over_fundamental(&overdrive, 261.63, 2);
        let b = harmonic_over_fundamental(&distortion, 261.63, 2);
        assert!(
            b > a * 1.15,
            "distortion had to be richer in harmonics than overdrive: {b:.3} versus {a:.3}"
        );
    }

    /// **The sawtooth lead has even harmonics; the square does not.**
    ///
    /// Program 81 (Lead 1, *square*) is square, and 80 is the sawtooth one. The table gave a
    /// square for both — measured in the soundfont, 81 has H2/H1 = 1.02, and a square has **zero**
    /// even harmonic. It was the table's most audible defect.
    #[test]
    fn the_sawtooth_lead_is_not_square() {
        let square = decode(&program_note(80, 60, 192)).expect("80");
        let saw = decode(&program_note(81, 60, 192)).expect("81");
        let h2_square = harmonic_over_fundamental(&square, 261.63, 2);
        let h2_saw = harmonic_over_fundamental(&saw, 261.63, 2);
        assert!(
            h2_square < 0.05,
            "a square has no even harmonic: {h2_square:.3}"
        );
        assert!(
            h2_saw > 0.3,
            "the sawtooth has a strong even harmonic: {h2_saw:.3}"
        );
    }

    /// **The bass decays; it does not stay flat like an organ.**
    ///
    /// Measured in the soundfont: the bass note falls to 0.42 of its initial energy in 1.2 s. The
    /// old table had a `sustain` of 0.55 with a 0.3 s decay and **got stuck** there — the bass
    /// sustained like an organ, and in a game with continuous bass that is the whole background
    /// wrong.
    #[test]
    fn the_bass_decays_instead_of_staying_flat() {
        let sound = decode(&program_note(33, 48, 384)).expect("bass");
        let second = window_energy(&sound, 1);
        let last = window_energy(&sound, 4);
        assert!(
            last < second * 0.85,
            "the bass had to decay: window 2 is {second:.4} and window 5 is {last:.4}"
        );
    }

    /// **Two joined songs do not become one.** The header declares how many tracks the file has,
    /// and the read stops there: without that, the next file's `MTrk`s came in as tracks of this
    /// one.
    ///
    /// Measured on a member extracted from a `.ggz`: 168 s of music where the game plays 47.5 s.
    /// The game delivers one SMF per `Play`, so the defect does not show up in a game — but a file
    /// with two concatenated songs gave the wrong duration, which is the symptom least noticed.
    #[test]
    fn two_joined_songs_do_not_become_one() {
        let first = smf(96, &one_note(69, 96));
        let second = smf(96, &one_note(60, 384));
        let mut joined = first.clone();
        joined.extend(second);
        let alone = decode(&first).expect("one song");
        let together = decode(&joined).expect("two joined");
        assert_eq!(
            together.samples.len(),
            alone.samples.len(),
            "the second song must not get into the first"
        );
    }

    #[test]
    fn the_variable_length_round_trips() {
        for value in [0, 1, 127, 128, 250, 8192, 0x0fff_ffff] {
            let bytes = delta(value);
            let mut pos = 0;
            assert_eq!(varlen(&bytes, &mut pos), Some(value), "{value}");
            assert_eq!(pos, bytes.len());
        }
    }

    #[test]
    fn the_header_and_track_come_out_with_tick_and_event() {
        let score = read(&smf(96, &one_note(69, 96))).expect("it is an SMF");
        assert_eq!(score.division, 96);
        assert_eq!(
            score.events,
            [
                (
                    0,
                    Event::NoteOn {
                        channel: 0,
                        note: 69,
                        velocity: 100
                    }
                ),
                (96, Event::NoteOff { channel: 0, note: 69 }),
            ]
        );
    }

    /// 96 ticks per beat at 120 per minute is half a second per beat.
    #[test]
    fn ticks_become_seconds_by_division_and_tempo() {
        let score = read(&smf(96, &one_note(69, 96))).unwrap();
        let events = score.in_seconds();
        assert_eq!(events[0].0, 0.0);
        assert!((events[1].0 - 0.5).abs() < 1e-9, "{:?}", events[1].0);
    }

    /// A `Set Tempo` in the middle applies only from its point onward.
    ///
    /// Recalculating the whole song with the new tempo is the easy mistake here, and it puts
    /// everything after an `accelerando` in the wrong place without anything looking broken.
    #[test]
    fn a_tempo_change_applies_from_that_point_on() {
        // One beat at 120, then double speed, then another beat.
        let track = [
            &[0x00u8, 0x90, 60, 100][..],
            &[96, 0x80, 60, 0x40][..],
            // Set Tempo = 250,000 us per beat, that is 240 per minute.
            &[0x00, 0xff, 0x51, 0x03, 0x03, 0xd0, 0x90][..],
            &[0x00, 0x90, 62, 100][..],
            &[96, 0x80, 62, 0x40][..],
            &[0x00, 0xff, 0x2f, 0x00][..],
        ]
        .concat();
        let events = read(&smf(96, &track)).unwrap().in_seconds();
        let instants: Vec<f64> = events.iter().map(|(t, _)| (t * 1000.0).round()).collect();
        // 0 ms, 500 ms (one beat at 120), and the second note lasts 250 ms.
        assert_eq!(instants, [0.0, 500.0, 500.0, 750.0]);
    }

    /// Running status: the second event comes without the status byte.
    #[test]
    fn running_status_applies_to_the_next_event() {
        let track = [
            0x00, 0x90, 60, 100, // play 60
            0x10, 62, 100, // and here only the data: it is another Note On
            0x10, 60, 0, // zero velocity is a release
            0x00, 0xff, 0x2f, 0x00,
        ];
        let score = read(&smf(96, &track)).unwrap();
        assert_eq!(
            score.events,
            [
                (
                    0,
                    Event::NoteOn {
                        channel: 0,
                        note: 60,
                        velocity: 100
                    }
                ),
                (
                    16,
                    Event::NoteOn {
                        channel: 0,
                        note: 62,
                        velocity: 100
                    }
                ),
                (32, Event::NoteOff { channel: 0, note: 60 }),
            ]
        );
    }

    /// Meta events and SysEx are skipped by their declared size, without losing what comes after.
    #[test]
    fn meta_and_sysex_do_not_swallow_the_track() {
        let track = [
            0x00, 0xff, 0x03, 0x04, b'n', b'o', b'm', b'e', // track name
            0x00, 0xf0, 0x03, 0x41, 0x42, 0xf7, // SysEx
            0x00, 0x90, 69, 100, //
            0x30, 0x80, 69, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let score = read(&smf(96, &track)).unwrap();
        assert_eq!(score.events.len(), 2);
        assert_eq!(
            score.events[0].1,
            Event::NoteOn {
                channel: 0,
                note: 69,
                velocity: 100
            }
        );
    }

    /// **Note 69 comes out at 440 Hz.**
    ///
    /// Measured by correlation with two sines, the right one and the neighboring semitone:
    /// counting zero crossings depends on the waveform, correlation does not. Getting the tuning
    /// wrong is the defect that most easily passes for "the synthesizer is like that" — the song
    /// plays in full, in the wrong key.
    #[test]
    fn note_69_sounds_at_440_hz() {
        // Program 64 is reed, which in our table is a sine: a simple shape to measure.
        let track = [
            0x00, 0xc0, 64, // program
            0x00, 0x90, 69, 127, //
            96, 0x80, 69, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let sound = decode(&smf(96, &track)).expect("one note is music enough");
        assert_eq!((sound.rate, sound.channels), (RATE, 1));

        let correlation = |hz: f32| -> f32 {
            // From the middle of the sustain, far from the attack and the release.
            let start = (0.1 * RATE as f32) as usize;
            let n = (0.2 * RATE as f32) as usize;
            let mut sum = 0.0;
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                sum += sound.samples[start + i] * (t * hz * std::f32::consts::TAU).sin();
            }
            (sum / n as f32).abs()
        };
        let ok = correlation(440.0);
        // The threshold follows [`TABLE_GAIN`]: with a fixed gain a lone note is no longer
        // stretched up to the ceiling, so what is demanded here is that it **exists**, and the
        // proof that it is the right note is still the comparison with the neighboring semitones,
        // just below.
        assert!(ok > 0.01, "note almost silent: {ok}");
        assert!(
            ok > correlation(466.16) * 3.0,
            "440 Hz ({ok}) did not stand out from the neighboring semitone ({})",
            correlation(466.16)
        );
        assert!(
            ok > correlation(415.30) * 3.0,
            "440 Hz did not stand out from the semitone below"
        );
    }

    /// One octave up doubles the frequency — it is the calculation that hurts most to get wrong,
    /// because it detunes the whole song by half a step without anything looking broken.
    #[test]
    fn each_octave_doubles_the_frequency() {
        let freq_of = |note: u8| 440.0 * 2.0f32.powf((f32::from(note) - 69.0) / 12.0);
        assert!((freq_of(69) - 440.0).abs() < 0.01);
        assert!((freq_of(81) - 880.0).abs() < 0.01);
        assert!((freq_of(57) - 220.0).abs() < 0.01);
        // Middle C, the tabulated value.
        assert!((freq_of(60) - 261.63).abs() < 0.01, "{}", freq_of(60));
    }

    /// Quiet music stays quiet: the gain is fixed, and the limiter raises **nothing**.
    ///
    /// It is the proof of the defect the old policy had. Normalizing by peak, two songs of
    /// different intensities came out with the same peak, and the game lost the difference
    /// between them — measured on Double Dragon's real tracks, the short jingle came out with more
    /// average energy than the combat music. Here the velocity 40 one has to stay quieter than the
    /// velocity 127 one.
    #[test]
    fn the_gain_is_fixed_and_does_not_level_songs_of_different_loudness() {
        let song = |velocity: u8| {
            let track = [
                0x00, 0x90, 60, velocity, //
                96, 0x80, 60, 0x40, //
                0x00, 0xff, 0x2f, 0x00,
            ];
            let sound = decode(&smf(96, &track)).expect("it is music");
            sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()))
        };
        let weak = song(40);
        let strong = song(127);
        assert!(weak < strong * 0.6, "weak {weak} did not stay below strong {strong}");
        assert!(weak > 0.0, "the weak one vanished: {weak}");
        assert!(strong <= CAP, "the strong one clipped: {strong}");
    }

    /// Percussion comes out as noise: real sound, and with no definite pitch.
    #[test]
    fn percussion_sounds_without_pitch() {
        // Channel 9, note 36: kick.
        let track = [
            0x00, 0x99, 36, 127, //
            96, 0x89, 36, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let sound = decode(&smf(96, &track)).expect("percussion is music");
        let peak = sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        // The threshold follows [`TABLE_GAIN`]: since the gain is fixed, not peak normalization,
        // a lone hit does **not** rise to the ceiling — it is worth what it is worth.
        assert!(peak > 0.03, "percussion almost silent: {peak}");
    }

    /// A note with no `Note Off` does not keep sounding forever.
    #[test]
    fn a_note_nobody_releases_ends_with_the_song() {
        let track = [
            0x00, 0x90, 69, 100, // play and never release
            96, 0x90, 71, 100, // another note, so there is an end
            0x00, 0xff, 0x2f, 0x00,
        ];
        let sound = decode(&smf(96, &track)).expect("it is music");
        // The end is the last event plus the release slack: a second and a half covers it all.
        assert!(
            sound.frames() < (1.5 * RATE as f32) as usize,
            "{}",
            sound.frames()
        );
        // And the tail of the file is silence, not a stuck note.
        let tail = &sound.samples[sound.samples.len() - 100..];
        assert!(tail.iter().all(|s| s.abs() < 1e-6));
    }

    /// The peak does not go past the ceiling: above 1.0 the mixer would clip, and clipping sounds
    /// like a bad synthesizer without being one.
    ///
    /// **What is demanded here changed together with the gain policy.** Before, the peak was
    /// *exactly* 0.8 because each song was normalized by its own peak; now the gain is fixed
    /// ([`TABLE_GAIN`]) and the limiter only kicks in when the sum still goes past [`CAP`].
    /// Thirty notes at full velocity are exactly that extreme case.
    #[test]
    fn many_voices_together_do_not_clip() {
        let mut track = Vec::new();
        for note in 40..70 {
            track.extend([0x00, 0x90, note, 127]);
        }
        track.extend([96, 0xff, 0x2f, 0x00]);
        let sound = decode(&smf(96, &track)).expect("it is music");
        let peak = sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak <= CAP, "clipped: {peak}");
        assert!(peak > 0.1, "it came out almost silent: {peak}");
    }

    /// Energy of a band, through a one-pole low-pass — the low end when `high` is false, and what
    /// is left over when it is true.
    fn energy(samples: &[f32], hz: f32, high: bool) -> f32 {
        let alpha = hz / (hz + RATE as f32 / std::f32::consts::TAU);
        let mut low = 0.0f32;
        let mut sum = 0.0f64;
        for &sample in samples {
            low += alpha * (sample - low);
            let strip = match high {
                true => sample - low,
                false => low,
            };
            sum += f64::from(strip * strip);
        }
        (sum / samples.len().max(1) as f64).sqrt() as f32
    }

    fn percussion_alone(note: u8) -> crate::audio::wav::Sound {
        let mut track = vec![0x00, 0x99, note, 127];
        track.extend(delta(96));
        track.extend([0x89, note, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        decode(&smf(96, &track)).expect("percussion is music")
    }

    /// **The hi-hat lives above the bass, and the kick below.**
    ///
    /// It is the test for the defect that was heard before it was understood: in Double Dragon the
    /// hi-hat disappeared in the stretches of continuous bass. It was not volume, it was band —
    /// hi-hat and cymbal got a low-pass, lost their brightness and ended up fighting over the
    /// bass's band. Two sounds in the same band and the louder one eats the quieter one, which is
    /// masking, not mixing.
    #[test]
    fn the_hihat_is_high_and_the_kick_is_low() {
        let hihat = percussion_alone(42);
        let kick = percussion_alone(36);
        // On the hi-hat, the treble has to dominate the low end — and on the kick, the opposite.
        let (treble_hihat, bass_hihat) = (
            energy(&hihat.samples, 2_000.0, true),
            energy(&hihat.samples, 2_000.0, false),
        );
        let brightness_hihat = treble_hihat / bass_hihat;
        let (treble_kick, bass_kick) = (
            energy(&kick.samples, 2_000.0, true),
            energy(&kick.samples, 2_000.0, false),
        );
        let brightness_kick = treble_kick / bass_kick;
        // Measured: 2.6 on the hi-hat and 0.19 on the kick. What the test demands is the
        // separation, with margin — the measurement's band splitter is one-pole and leaks on
        // purpose, so the number does not depend on a better filter than the synthesizer's.
        assert!(brightness_hihat > 2.0, "hi-hat without brightness: {brightness_hihat}");
        assert!(brightness_kick < 0.5, "kick without body: {brightness_kick}");
        assert!(
            brightness_hihat > brightness_kick * 8.0,
            "hi-hat and kick in the same band: {brightness_hihat} versus {brightness_kick}"
        );
    }

    /// The continuous bass does not muffle the hi-hat: they occupy different bands.
    ///
    /// The calculation is the one the ear makes: how much treble energy there is while the bass
    /// sounds. If the hi-hat were in the bass's band, this number would drop to the bass's alone.
    #[test]
    fn a_continuous_bass_does_not_eat_the_hihat() {
        // Program 33 is electric bass. Note 40 held, and a hi-hat on top.
        let mut track = vec![
            0x00, 0xc0, 33, // bass on channel 0
            0x00, 0x90, 40, 127, // and it holds the note
            0x00, 0x99, 42, 100, // hi-hat, on the percussion channel
        ];
        track.extend(delta(96));
        track.extend([0x89, 42, 0x40]);
        track.extend(delta(96));
        track.extend([0x80, 40, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        let with_hihat = decode(&smf(96, &track)).unwrap();

        // The same stretch, only the bass.
        let mut bass_only = vec![0x00, 0xc0, 33, 0x00, 0x90, 40, 127];
        bass_only.extend(delta(192));
        bass_only.extend([0x80, 40, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        let without_hihat = decode(&smf(96, &bass_only)).unwrap();

        // The first 50 ms, where the hi-hat is sounding.
        let amount = (0.05 * RATE as f32) as usize;
        let treble_with = energy(&with_hihat.samples[..amount], 3_000.0, true);
        let treble_without = energy(&without_hihat.samples[..amount], 3_000.0, true);
        assert!(
            treble_with > treble_without * 4.0,
            "the hi-hat did not show up over the bass: {treble_with} versus {treble_without}"
        );
    }

    /// Past the voice cap, what goes is the oldest — not the one that just arrived.
    ///
    /// Discarding the new note is a disguised priority rule, and the worst one: what stays is the
    /// long note already sounding, what loses is the short one that arrived. In a song with
    /// continuous bass that swallows the percussion, which is what carries the rhythm.
    #[test]
    fn past_the_voice_cap_the_oldest_goes() {
        let mut track = Vec::new();
        // More notes than the cap, all held, on channel 0.
        for i in 0..(MAX_VOICES + 8) {
            track.extend([0x00, 0x90, 36 + i as u8, 100]);
        }
        // And a hi-hat after everything, which is the note that must not be refused.
        track.extend(delta(96));
        track.extend([0x99, 42, 127]);
        track.extend(delta(96));
        track.extend([0x89, 42, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        let sound = decode(&smf(96, &track)).unwrap();
        // At the hi-hat's instant (half a second) there has to be treble, which none of the held
        // notes produces.
        let start = (0.5 * RATE as f32) as usize;
        let end = (start + (0.05 * RATE as f32) as usize).min(sound.samples.len());
        let treble = energy(&sound.samples[start..end], 3_000.0, true);
        assert!(treble > 0.01, "the hi-hat was refused by the cap: {treble}");
    }

    #[test]
    fn what_is_not_midi_does_not_decode() {
        assert!(decode(b"RIFF\x00\x00\x00\x00WAVEfmt ").is_none());
        assert!(decode(&[]).is_none());
        assert!(decode(b"MThd").is_none());
        // Format 2 is a collection of independent patterns, not a song.
        let mut two = smf(96, &one_note(69, 96));
        two[9] = 2;
        assert!(decode(&two).is_none());
        // Division zero does not say how long a tick lasts.
        assert!(decode(&smf(0, &one_note(69, 96))).is_none());
    }

    /// A track truncated in the middle of an event neither panics nor makes up a note.
    #[test]
    fn truncated_file_stops_where_it_can() {
        let complete = smf(96, &one_note(69, 96));
        for cutoff in 1..complete.len() {
            let _ = decode(&complete[..cutoff]);
        }
    }

    /// SMPTE division: time comes from frames per second, and `Set Tempo` does not apply.
    #[test]
    fn smpte_division_counts_per_frame() {
        // 0xe7 is -25 in two's complement: 25 frames per second, 40 ticks per frame,
        // that is a thousand ticks per second. That is how the field is written in the file.
        let division = i16::from_be_bytes([0xe7, 40]);
        let events = read(&smf(division, &one_note(69, 250))).unwrap().in_seconds();
        assert!((events[1].0 - 0.25).abs() < 1e-6, "{:?}", events[1].0);
    }

    /// The 16-bit path exists for when the wide buffer does not fit. It has to
    /// sound like the wide one: the difference fits within the quantization.
    #[test]
    fn the_compact_path_matches_the_wide_one() {
        let timbre = timbre(0);
        let frequency = 440.0f32;
        let harmonics = (RATE as f32 / 2.0 / frequency) as u32;
        let (tables, index) = match timbre.shape {
            Waveform::Partial { exponent } => {
                (vec![Wavetable::create(exponent, harmonics.max(1))], Some(0))
            }
            _ => (Vec::new(), None),
        };
        let voice = Voice {
            channel: 0,
            note: 69,
            table: index,
            start: 100,
            released_at: Some(8_000),
            frequency,
            timbre,
            filter: Filter::Off,
            amplitude: 0.8,
        };
        let total = 9_000;
        let mut wide = vec![0.0f32; total];
        render_voice(&voice, &mut wide, &tables);
        apply_fixed_gain(&mut wide);
        let mut short = vec![0i16; total];
        fill_compact(std::slice::from_ref(&voice), &tables, &mut short);
        let error = wide
            .iter()
            .zip(short.iter())
            .map(|(a, b)| (a - f32::from(*b) / 32767.0).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 0.002, "error {error}");
        assert!(short.iter().any(|s| *s != 0));
    }

    /// The filtered percussion also goes through the short path, and the filter changes the energy.
    #[test]
    fn the_compact_path_matches_the_percussion() {
        let (timbre, filter) = percussion(42);
        let voice = Voice {
            channel: 9,
            note: 42,
            table: None,
            start: 50,
            released_at: Some(2_000),
            frequency: 0.0,
            timbre,
            filter,
            amplitude: 0.5,
        };
        let total = 4_000;
        let mut wide = vec![0.0f32; total];
        render_voice(&voice, &mut wide, &[]);
        apply_fixed_gain(&mut wide);
        let mut short = vec![0i16; total];
        fill_compact(std::slice::from_ref(&voice), &[], &mut short);
        let error = wide
            .iter()
            .zip(short.iter())
            .map(|(a, b)| (a - f32::from(*b) / 32767.0).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 0.002, "error {error}");
        assert!(short.iter().any(|s| *s != 0));
    }

    /// A short effect stays on the caller's thread. A long track returns the sound
    /// and finishes the work afterwards: it is what removes the game's pause on the Vita.
    #[test]
    fn long_synthesis_does_not_block_the_caller() {
        let short_track = program_note(0, 69, 48);
        let a = decode(&short_track).unwrap();
        let b = decode_nonblocking(&short_track).unwrap();
        assert_eq!(a.samples, b.samples);

        let long_track = program_note(0, 69, 384);
        let full = decode(&long_track).unwrap();
        let live = decode_nonblocking(&long_track).unwrap();
        assert_eq!(live.n_samples(), full.n_samples());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !live.reaches(live.n_samples().saturating_sub(1)) {
            assert!(
                std::time::Instant::now() < deadline,
                "the synthesis did not finish"
            );
            std::thread::yield_now();
        }
        let error = (0..full.n_samples())
            .map(|i| (full.read(i) - live.read(i)).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 0.002, "error {error}");
    }
}
