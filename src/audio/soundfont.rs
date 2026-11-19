//! Sample-bank (SoundFont) synthesis for MIDI.
//!
//! **Why it exists.** The comparison with Zeebulator measured what the wavetable can and can't
//! do: after it was redone against the measurement, its centroid error dropped from 7635 to
//! 701 Hz, but three things stayed out of reach, and all three are **sample material**:
//!
//! | | real sample | wavetable |
//! |---|---|---|
//! | string harmonic ratio | 8.37 | 0.60 |
//! | distortion harmonic ratio | 4.58 | 0.84 |
//! | drum centroid | 7092 Hz | 6661 Hz |
//!
//! Here the score is played with the bank, and those three come from the bank itself.
//!
//! **Why `rustysynth` and not Zeebulator's code.** Even with a compatible license, `rustysynth`
//! is **MIT** and **pure Rust**, so it goes into the Libretro core without bringing in any host
//! library, which is the project's rule.
//!
//! **Why the bank isn't embedded.** It's 32 MB (measured: GeneralUser GS, 32,319,396 B) and
//! loading it costs +64 MiB of RSS, because the samples become `float`. Embedded, the core's `.so` would go from
//! 15.6 MB to ~48 MB on six CI targets — and a core that downloads 48 MB to run a 1 MB game
//! doesn't pay for itself. The bank is **optional**, looked up in the device folder, and without
//! it MIDI falls back to the wavetable, which is what already plays today.
//!
//! The search path is the same as for the system font: the device folder the frontend handed
//! over first, and then the desktop profile. `ZEEBX_SOUNDFONT` points straight at a file, for
//! trying things out without installing anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::audio::wav::Sound;

/// Duration cap for a synthesized song, in seconds.
///
/// The same as the wavetable synthesizer's, and for the same reason: a broken score can't
/// reserve endless memory.
///
/// **It isn't a cap of convenience: it's the same number for both paths.** The first version had
/// 36 s here, and Double Dragon's music — 47.5 s — came out cut, which only showed up because the
/// measurement compared the duration with the reference's. Two paths that claim to play the same
/// score can't have different caps.
const MAX_SECONDS: f64 = 300.0;

/// How many frames per block when rendering. The `rustysynth` sequencer works in blocks.
const BLOCK: usize = 1024;

/// The rate the bank is synthesized at, in Hz.
///
/// **It isn't the wavetable's rate, and the difference is measured.** The wavetable synthesizes at
/// 22,050 Hz because what it produces is a sum of harmonics it picks itself, and there is nothing
/// there above 11 kHz. The bank is the opposite: of the 920 samples in `GeneralUser-GS.sf2`, 373
/// are recorded at 44,100 Hz and there are samples at 48,000 — synthesizing at 22,050 throws away
/// **all** content above 11 kHz, and the mixer, which only interpolates linearly, has no way to
/// bring back what was cut. The result is brightness that disappears: cymbals, hi-hats and metal
/// attacks come out muffled.
///
/// 44,100 is the rate of the core's own mixer (`SAMPLE_RATE` in the Libretro frontend), so there
/// is no resampling at all on this path — and it's the same choice the reference emulator `zeemu`
/// makes on its SoundFont path.
pub const BANK_RATE: u32 = 44_100;

/// The rate currently chosen, which starts at [`BANK_RATE`] and the frontend can change.
///
/// **It's a global, and that's a deliberate choice.** The alternative would be to carry the rate
/// as a parameter from `Session::start_*` to `Machine` and from there to here, as was done with
/// the synthesizer policy — and in that case it was worth it, because the choice changes what the
/// machine **is** when it's created. This one doesn't: it applies to the next synthesized song,
/// and a song already synthesized doesn't change rate. One more parameter in five public
/// signatures for a value nobody needs at creation is cost with no return. This module already
/// keeps a global for the same reason — the cache of open banks.
static CHOSEN_RATE: AtomicU32 = AtomicU32::new(BANK_RATE);

/// Changes the rate the bank will be synthesized at from now on.
///
/// Values outside 8,000–48,000 are ignored: `rustysynth` refuses outside 16,000–192,000, and an
/// absurd rate coming from a hand-edited `.opt` can't bring the sound down.
pub fn set_rate(rate: u32) {
    if (8_000..=48_000).contains(&rate) {
        CHOSEN_RATE.store(rate, Ordering::Relaxed);
    }
}

/// The rate the bank is synthesized at now.
pub fn rate() -> u32 {
    CHOSEN_RATE.load(Ordering::Relaxed)
}

/// The voices currently chosen. See [`VOICES`] for why the default is what it is.
static CHOSEN_VOICES: AtomicUsize = AtomicUsize::new(VOICES);

/// Changes the cap on simultaneous voices from now on.
///
/// `rustysynth` accepts 8 to 256 and refuses anything outside that, so the value is clamped to
/// the range instead of rejected: whoever asked for 512 wants the maximum, and failing the whole
/// synthesis because of that would be worse.
pub fn set_voices(voices: usize) {
    CHOSEN_VOICES.store(voices.clamp(8, 256), Ordering::Relaxed);
}

/// Whether the synthesizer's reverb and chorus run. On by default.
///
/// **The score doses the effect, not the synthesizer.** MIDI says per channel how much reverb
/// (CC91) and chorus (CC93) each instrument gets; `rustysynth` only provides the effect unit.
/// With it off, those controls go nowhere and every note ends dry, which sounds like a cut-off
/// release. The option to turn it off stays for those who prefer a dry sound.
static CHOSEN_EFFECTS: AtomicBool = AtomicBool::new(true);

/// Turns MIDI music reverb and chorus on or off from now on. The music is synthesized when the
/// game loads it, so the change applies from the next game on.
pub fn set_effects(enabled: bool) {
    CHOSEN_EFFECTS.store(enabled, Ordering::Relaxed);
}

/// The bank the user chose in the settings or on the command line. `None` is the automatic
/// search.
///
/// Global for the same reason as the rate: the bank is opened when the machine is created, and the
/// frontend sets it before opening the game. A change applies from the next game on.
static CHOSEN_BANK: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Chooses the bank for the next games, or goes back to the automatic search with `None`. An
/// empty path counts as `None`: it's what a cleared settings field delivers.
pub fn set_soundfont(path: Option<PathBuf>) {
    let path = path.filter(|c| !c.as_os_str().is_empty());
    if let Ok(mut chosen) = CHOSEN_BANK.lock() {
        *chosen = path;
    }
}

/// The bank currently chosen, if any.
pub fn chosen_bank() -> Option<PathBuf> {
    CHOSEN_BANK.lock().ok().and_then(|chosen| chosen.clone())
}

/// How many voices the bank can play at once.
///
/// The `rustysynth` default is 64, and Double Dragon's soundtrack uses up to nine simultaneous
/// channels with chords: 64 voices steal notes in dense passages, and voice stealing sounds like a
/// note that vanishes. 128 is what `zeemu` uses, and the `rustysynth` cap is 256.
const VOICES: usize = 128;

/// The synthesizer's master volume, before any summing.
///
/// **It's fixed gain, not normalization — that is the fix.** The previous version normalized each
/// song's peak to 0.8, and that's the wrong measure by construction: a calm, sparse track was
/// raised until it touched the same ceiling as a dense, full one, so the game lost the
/// difference in intensity between them, and the balance with the effects (which are WAVE and are
/// **not** normalized) changed with every song that came in.
///
/// Both reference engines use fixed gain for the same reason: `zeebulator` applies -16 dB and
/// `zeemu` applies -8 dB, both **before** the synthesizer's internal summing, which is where
/// clipping would happen. Here the number is `rustysynth`'s own default (0.5, or -6 dB), which is
/// the point at which Double Dragon's soundtrack was measured with a peak of 0.53 — close to the
/// ceiling, without touching it.
///
/// It stays a constant so it can be calibrated by ear, which is how both reference engines
/// arrived at their numbers.
const MASTER_VOLUME: f32 = 0.5;

/// The peak ceiling after summing.
///
/// Above this the waveform **clips** in the mixer, and clipping is distortion. Below it, nothing
/// is touched: it's a limiter, not a normalizer — it only lowers what went over the ceiling, and
/// never raises what is low.
const CAP: f32 = 0.95;

/// Where the bank is looked for, in order of preference.
///
/// `ZEEBX_SOUNDFONT` first because it's the path of someone experimenting; then the device folder,
/// which is what the frontend controls; and last the desktop profile.
pub fn candidates(device: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    if let Some(path) = std::env::var_os("ZEEBX_SOUNDFONT") {
        output.push(PathBuf::from(path));
    }
    output.extend(candidates_in(&default_dirs(device)));
    output
}

/// The folders where the bank is looked for, in order: the device one, which the frontend
/// controls, and the desktop profile one.
///
/// Split from [`candidates`] so the tests can assert without depending on the profile of whoever
/// runs them. **A `.sf2` in `~/.config/zeebx` — which is where the README says to put it — turned
/// the suite red**, because two tests asked "is there no bank at all" with the developer's disk
/// full of them.
pub fn default_dirs(device: &Path) -> Vec<PathBuf> {
    vec![
        device.join("soundfonts"),
        crate::config::config_dir().join("aparelho").join("soundfonts"),
    ]
}

/// The banks in the given folders, in order. Without the profile and without `ZEEBX_SOUNDFONT`.
pub fn candidates_in(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut output = Vec::new();
    for base in dirs {
        for name in banks_in(base) {
            output.push(name);
        }
    }
    output
}

/// The `.sf2` files in a folder, in stable order.
///
/// Stable because two banks in the same directory can't give different results between runs
/// because of the order in which the filesystem lists them — the same reason as for the font.
fn banks_in(base: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut soundfonts: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sf2"))
        })
        .collect();
    soundfonts.sort();
    soundfonts
}

/// The bank to use: the chosen one, then the one from `ZEEBX_SOUNDFONT`, then the first `.sf2` in
/// the search folders.
///
/// **A chosen bank that vanished doesn't mute the music.** The file may have moved since it was
/// chosen; the log says which, and the search goes on as if nothing had been chosen. Silence here
/// would be "the bank doesn't work" without saying why.
///
/// `ZEEBX_SOUNDFONT` only went into [`candidates`], which nobody called outside the tests: the
/// variable that [`report`] tells people to use was ignored.
pub fn first_bank(device: &Path) -> Option<PathBuf> {
    let env_var = std::env::var_os("ZEEBX_SOUNDFONT").map(PathBuf::from);
    for (origin, path) in [("chosen", chosen_bank()), ("from ZEEBX_SOUNDFONT", env_var)] {
        let Some(path) = path else {
            continue;
        };
        if path.is_file() {
            return Some(path);
        }
        crate::logging!(
            crate::logging::Level::Warning,
            "soundfont",
            "the {} bank does not exist: {}; falling back to the folder search",
            origin,
            path.display()
        );
    }
    first_bank_in(&default_dirs(device))
}

/// The first real bank **only** in the given folders: the search without the runner's profile.
pub fn first_bank_in(dirs: &[PathBuf]) -> Option<PathBuf> {
    candidates_in(dirs).into_iter().find(|c| c.is_file())
}

/// The declared size of the `sdta`'s `smpl` chunk, in bytes, if the file declares it.
///
/// Walks the RIFF chain the way `rustysynth` walks it — `RIFF`/`sfbk` and, inside the
/// `LIST`/`sdta`, the sub-chunks — because it's **the `sdta`'s `smpl`** that the dependency reads
/// with `slice::from_raw_parts_mut` (`binary_reader.rs`, `read_wave_data`). Returns `None` for
/// anything that doesn't get that far: the library is what judges a strange file, not this scan,
/// and the normal path (the user's `.sf2`) has to keep reaching the synthesizer intact.
fn smpl_size(bytes: &[u8]) -> Option<u32> {
    /// The four-letter identifier at position `pos`, if there are four bytes there.
    fn id(bytes: &[u8], pos: usize) -> Option<&[u8]> {
        bytes.get(pos..pos.checked_add(4)?)
    }

    /// The size declared in the head of the chunk that starts at `pos`, in bytes.
    fn size(bytes: &[u8], pos: usize) -> Option<usize> {
        let field: [u8; 4] = bytes.get(pos.checked_add(4)?..pos.checked_add(8)?)?.try_into().ok()?;
        Some(u32::from_le_bytes(field) as usize)
    }

    if id(bytes, 0)? != b"RIFF" || id(bytes, 8)? != b"sfbk" {
        return None;
    }
    // The top-level chunks, from the end of the RIFF header on. The step is the declared size
    // plus the RIFF even padding, as in the tests' `block`.
    let mut pos = 12usize;
    while pos.checked_add(8).is_some_and(|end| end <= bytes.len()) {
        let Some(block) = id(bytes, pos) else { return None };
        let Some(declared) = size(bytes, pos) else { return None };
        let body = pos + 8;
        if block == b"LIST" && id(bytes, body) == Some(b"sdta".as_slice()) {
            // Inside the `sdta`: the `smpl` with the samples and the `sm24` with the eight extra bits.
            let end = body.saturating_add(declared).min(bytes.len());
            let mut sub = body + 4;
            while sub.checked_add(8).is_some_and(|f| f <= end) {
                let (Some(name), Some(chunk_len)) = (id(bytes, sub), size(bytes, sub)) else {
                    break;
                };
                if name == b"smpl" {
                    return Some(chunk_len as u32);
                }
                let Some(next) = sub.checked_add(8 + chunk_len + (chunk_len & 1)) else {
                    break;
                };
                sub = next;
            }
        }
        let Some(next) = body.checked_add(declared) else {
            return None;
        };
        pos = next + (declared & 1);
    }
    None
}

/// A loaded bank.
///
/// Expensive to build (32 MB of samples converted to `float`) and cheap to reuse, so it's kept
/// by path: a game that plays twelve songs loads the bank once.
pub struct Bank {
    /// `Arc` because `rustysynth`'s `Synthesizer` takes the font by `Arc`: a new voice per
    /// song, and the 32 MB bank isn't copied along.
    source: Arc<rustysynth::SoundFont>,
}

impl Bank {
    fn load(path: &Path) -> Option<Self> {
        let t0 = Instant::now();
        let bytes = std::fs::read(path).ok()?;
        let read_elapsed = t0.elapsed();
        // **The guard against writing one byte past the allocation.** `rustysynth` 1.3.6
        // (`binary_reader.rs`, `read_wave_data`) reserves a `Vec<i16>` of `size / 2` elements and
        // creates, with `slice::from_raw_parts_mut`, a slice of `size` **bytes**: with an odd-sized
        // `smpl` the slice is one byte larger than the allocation, and the read writes outside it.
        // It's UB triggered by a user file — a truncated bank or one built by another tool —, not
        // by a game, so the refusal is ours and comes before the parse. The whole bank is
        // rejected, and `None` makes MIDI fall back to the wavetable, as with any bank that won't
        // open.
        if let Some(size) = smpl_size(&bytes)
            && size % 2 == 1
        {
            crate::logging!(
                crate::logging::Level::Error,
                "soundfont",
                "bank {} rejected: the `smpl` chunk has {size} bytes (odd size) and \
                 `rustysynth` would write one byte past the allocation; the `smpl` chunk must \
                 have an even size, and the bank must be regenerated",
                path.display()
            );
            return None;
        }
        let t_parse = Instant::now();
        let mut reader = std::io::Cursor::new(bytes);
        let source = Arc::new(rustysynth::SoundFont::new(&mut reader).ok()?);
        let parse_elapsed = t_parse.elapsed();
        crate::logging!(
            crate::logging::Level::Debug,
            "midi",
            "bank {} loaded in {:.1}ms (read: {:.1}ms, parse/samples: {:.1}ms, presets: {})",
            path.display(),
            t0.elapsed().as_secs_f64() * 1000.0,
            read_elapsed.as_secs_f64() * 1000.0,
            parse_elapsed.as_secs_f64() * 1000.0,
            source.get_presets().len()
        );
        Some(Self { source })
    }

    /// How many presets the bank has, so the report can say it's a real bank.
    pub fn presets(&self) -> usize {
        self.source.get_presets().len()
    }
}

/// The bank already loaded, by path. Only one in practice; the map tolerates the path changing.
static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Bank>>>> = OnceLock::new();

/// Loads the bank at `path`, reusing whatever is already in memory.
pub fn open(path: &Path) -> Option<Arc<Bank>> {
    let guard = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = guard.lock().ok()?;
    if let Some(soundfont) = map.get(path) {
        crate::logging!(
            crate::logging::Level::Debug,
            "soundfont",
            "bank {} reused from the cache",
            path.display()
        );
        return Some(soundfont.clone());
    }
    let begin = Instant::now();
    let soundfont = Arc::new(Bank::load(path)?);
    // The bank load time is what explains the delay of the first song on the handheld; it lives
    // here, and not in the mixer, because this is where the work happens.
    crate::logging!(
        crate::logging::Level::Info,
        "soundfont",
        "bank {} with {} preset(s) opened in {} ms",
        path.display(),
        soundfont.presets(),
        begin.elapsed().as_millis()
    );
    map.insert(path.to_path_buf(), soundfont.clone());
    Some(soundfont)
}

/// Plays an SMF score with the bank, returning mono PCM at the requested rate.
///
/// `None` when the bank doesn't open, when the bytes aren't an SMF that `rustysynth` accepts, or
/// when no sample is left — and then the caller moves on to the wavetable.
pub fn play(soundfont: &Bank, bytes: &[u8], rate: u32) -> Option<Sound> {
    let t0 = Instant::now();
    let mut reader = std::io::Cursor::new(bytes);
    let midi = rustysynth::MidiFile::new(&mut reader).ok()?;
    let length = midi.get_length().min(MAX_SECONDS);
    let mut settings_page = rustysynth::SynthesizerSettings::new(rate as i32);
    // 1. `block_size = 1024`: cuts the sequencer's block and sync overhead by ~1.9x.
    // 2. Reverb and chorus: see [`CHOSEN_EFFECTS`].
    settings_page.block_size = BLOCK;
    settings_page.enable_reverb_and_chorus = CHOSEN_EFFECTS.load(Ordering::Relaxed);
    // 3. `maximum_polyphony`: see [`VOICES`] — the default of 64 steals notes in dense passages.
    settings_page.maximum_polyphony = CHOSEN_VOICES.load(Ordering::Relaxed);
    let mut synthesizer = rustysynth::Synthesizer::new(&soundfont.source, &settings_page).ok()?;
    // 4. The master volume is fixed, and doesn't come from normalization afterwards. See
    // [`MASTER_VOLUME`].
    synthesizer.set_master_volume(MASTER_VOLUME);
    let mut sequence = rustysynth::MidiFileSequencer::new(synthesizer);
    sequence.play(&Arc::new(midi), false);

    let total = ((length + 0.5) * f64::from(rate)) as usize + 1;
    let mut samples = Vec::with_capacity(total);
    let (mut left, mut right) = (vec![0.0f32; BLOCK], vec![0.0f32; BLOCK]);
    while samples.len() < total {
        sequence.render(&mut left, &mut right);
        for i in 0..BLOCK {
            if samples.len() >= total {
                break;
            }
            // Mono: the average of both channels. The console has a single speaker, and the mixer
            // duplicates it later.
            samples.push(0.5 * (left[i] + right[i]));
        }
        if sequence.end_of_sequence() {
            break;
        }
    }
    // The tail: the bank may finish before the declared length, and a sound shorter than the
    // score would make the game think the music ended early.
    samples.resize(total, 0.0);
    // **Limit, don't normalize.** The gain was already applied once, fixed, in the synthesizer's
    // master volume (see [`MASTER_VOLUME`]): all that's left here is keeping a dense sum from
    // going over the ceiling and clipping. A song that came out quiet **stays** quiet, because
    // that's how it is.
    let peak = limit(&mut samples);
    let elapsed = t0.elapsed();
    crate::logging!(
        crate::logging::Level::Info,
        "midi",
        "bank: {} bytes of SMF -> {:.1}s of audio ({} samples @ {}Hz, raw peak {:.3}) synthesized in {:.1}ms ({:.2}x real time)",
        bytes.len(),
        length,
        samples.len(),
        rate,
        peak,
        elapsed.as_secs_f64() * 1000.0,
        if elapsed.as_secs_f64() > 0.0 { length / elapsed.as_secs_f64() } else { 0.0 }
    );
    Some(Sound::create(rate, 1, samples))
}

/// Lowers the volume only when the peak went over [`CAP`]. Returns the peak **before** touching it.
///
/// It's the opposite of normalizing: normalizing evens out every song's peak, and in doing so
/// erases the difference in intensity between a calm track and a full one. Here, quiet music stays
/// quiet, and only the one that would touch the ceiling comes down — by a factor for the whole
/// song, which preserves the proportion between its voices.
///
/// Returning the raw peak is what lets [`MASTER_VOLUME`] be calibrated by ear with a number in
/// hand: without it, knowing whether the fixed gain is close to the ceiling requires recording the
/// audio and measuring it elsewhere.
fn limit(samples: &mut [f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if peak > CAP {
        let factor = CAP / peak;
        for sample in samples.iter_mut() {
            *sample *= factor;
        }
    }
    peak
}


/// Where the bank should go, and whether there already is one — in one line, for the frontend to
/// show.
///
/// It exists because the search is by directory and the folder isn't obvious: in the Libretro
/// core it comes from the system root the frontend handed over, and whoever installs the core has
/// no way to guess the path. The alternative — inventing one more search directory — trades a
/// clear notice for a guess, and a guess at a file path is the defect you pay for with "it doesn't
/// work and doesn't say why".
pub fn report(device: &Path) -> String {
    report_for(device, first_bank(device))
}

/// The same report, with the bank already resolved.
///
/// The `Option` comes in as a parameter so the test for the "no bank" side doesn't depend on the
/// profile of whoever runs it — see [`default_dirs`].
fn report_for(device: &Path, soundfont: Option<PathBuf>) -> String {
    match soundfont {
        Some(path) => format!(
            "Zeebx: MIDI sample bank at {}; the soundtrack plays with the samples",
            path.display()
        ),
        None => {
            let dir = device.join("soundfonts");
            format!(
                "Zeebx: no MIDI sample bank; the soundtrack plays with the wavetable. \
                 To hear it with samples, put a .sf2 in {} (or point ZEEBX_SOUNDFONT at one)",
                dir.display()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rate and voices chosen by the frontend stop at what the synthesizer accepts.
    ///
    /// **What is checked is that an absurd value doesn't get through.** Both come from a file the
    /// user edits by hand, and `rustysynth` refuses to create the synthesizer outside its ranges
    /// — a refusal that would reach the game as music that simply doesn't play, without saying
    /// why.
    ///
    /// The test restores the defaults at the end because the state is global and shared with the
    /// other tests in this module (see [`CHOSEN_RATE`]); leaving it dirty would make the test
    /// order change their results.
    #[test]
    fn the_chosen_rate_and_voices_stay_within_the_synthesizer_range() {
        set_rate(22_050);
        assert_eq!(rate(), 22_050);
        // Out of range is **ignored**, not clamped: an absurd rate is usually a broken file, and
        // keeping the previous one is safer than making up a number.
        set_rate(1);
        assert_eq!(rate(), 22_050, "an absurd rate should not have gotten through");
        set_rate(999_999);
        assert_eq!(rate(), 22_050, "an absurd rate should not have gotten through");
        set_rate(BANK_RATE);
        assert_eq!(rate(), BANK_RATE);

        // Voices are **clamped** to the range, not ignored: whoever asks for 512 wants the
        // maximum, and the maximum is a request that can be met.
        set_voices(4);
        assert_eq!(CHOSEN_VOICES.load(Ordering::Relaxed), 8);
        set_voices(512);
        assert_eq!(CHOSEN_VOICES.load(Ordering::Relaxed), 256);
        set_voices(48);
        assert_eq!(CHOSEN_VOICES.load(Ordering::Relaxed), 48);
        set_voices(VOICES);
    }

    /// The test bank, from the comparison harness that lives outside the repository.
    fn test_bank() -> Option<PathBuf> {
        let candidates = [
            std::env::var_os("ZEEBX_SOUNDFONT").map(PathBuf::from),
            Some(PathBuf::from(
                "/home/rafaelfrequiao/zeebx-midi-harness/zbfont/GeneralUser-GS.sf2",
            )),
        ];
        candidates.into_iter().flatten().find(|c| c.is_file())
    }

    /// A one-note SMF, so as not to depend on a file.
    fn one_note(program: u8, note: u8, _ticks: u32) -> Vec<u8> {
        let mut track = vec![0x00, 0xc0, program, 0x00, 0x90, note, 100];
        track.extend([0x81, 0x70]);
        track.extend([0x80, note, 0x40, 0x00, 0xff, 0x2f, 0x00]);
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend(0u16.to_be_bytes());
        out.extend(1u16.to_be_bytes());
        out.extend(96i16.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    /// **The bank loads and plays.** The test declares itself skipped when there's no bank on
    /// disk, and says so: a test that passes without proving anything is worse than a missing one.
    #[test]
    fn the_bank_plays_the_score() {
        let Some(path) = test_bank() else {
            eprintln!("no .sf2 bank on disk: test skipped (use ZEEBX_SOUNDFONT)");
            return;
        };
        let soundfont = open(&path).expect("the bank opens");
        assert!(
            soundfont.presets() > 100,
            "a General MIDI bank has hundreds of presets, this one has {}",
            soundfont.presets()
        );
        let sound = play(&soundfont, &one_note(0, 69, 240), 22_050).expect("plays");
        assert_eq!(sound.rate, 22_050);
        assert_eq!(sound.channels, 1);
        let peak = sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.01, "the piano should have sounded, peak {peak}");
    }


    /// A minimal SoundFont2, built byte by byte, so the test doesn't depend on a file on disk.
    ///
    /// **Why this exists.** The tests that use the real bank skip themselves when there's no bank
    /// on the machine — and on CI there isn't. A test that passes without proving anything is worse
    /// than a missing test, so the bank path needs a case that **always** runs.
    ///
    /// The structure wasn't guessed: it was read from `rustysynth`'s parser, because the format
    /// has two traps that cost two attempts — the zone record is **4 bytes** (`wGenNdx`,
    /// `wModNdx`) and not four numbers, and `SAMPLE_ID` (53) has to be the **last** generator of
    /// the zone, otherwise the parser treats the first zone as a global zone and no region is left
    /// to play.
    fn minimal_bank() -> Vec<u8> {
        let rate = 22_050u32;
        let frames = 64usize;
        let raw = minimal_bank_samples(rate, frames);
        minimal_bank_with_smpl(&raw, rate, frames)
    }

    /// The minimal bank's samples as raw bytes: 64 frames of a 440 Hz sine plus the 46 zeros the
    /// `smpl` requires at the end.
    fn minimal_bank_samples(rate: u32, frames: usize) -> Vec<u8> {
        let mut raw = Vec::new();
        for i in 0..frames {
            let angle = std::f64::consts::TAU * 440.0 * i as f64 / f64::from(rate);
            raw.extend(((angle.sin() * 20_000.0) as i16).to_le_bytes());
        }
        raw.extend(std::iter::repeat_n(0u8, 46 * 2));
        raw
    }

    /// The same bank, with the raw `smpl` bytes supplied by the caller.
    ///
    /// **Why the raw bytes, and not the samples.** It's what allows building an **odd**-sized
    /// `smpl`, the case for the guard against writing past the allocation. The rest of the
    /// structure stays that of a bank the parser accepts — without that the test would prove that
    /// a broken file is rejected, and not that the odd chunk is stopped **before** it reaches the
    /// dependency.
    fn minimal_bank_with_smpl(raw: &[u8], rate: u32, frames: usize) -> Vec<u8> {
        fn block(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = name.to_vec();
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
            if body.len() % 2 == 1 {
                out.push(0);
            }
            out
        }

        fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
            let mut body = kind.to_vec();
            for part in parts {
                body.extend(part);
            }
            block(b"LIST", &body)
        }

        fn fixed_name(name: &str) -> Vec<u8> {
            let mut out = name.as_bytes().to_vec();
            out.resize(20, 0);
            out
        }

        /// A zone record: 4 bytes, `wGenNdx` and `wModNdx`.
        fn zone(generators: u16, modulators: u16) -> Vec<u8> {
            [generators.to_le_bytes(), modulators.to_le_bytes()].concat()
        }

        /// The generators of a zone, with the terminator the format requires.
        fn generators(items: &[(u16, u16)]) -> Vec<u8> {
            let mut body = Vec::new();
            for (kind, value) in items {
                body.extend(kind.to_le_bytes());
                body.extend(value.to_le_bytes());
            }
            body.extend([0u8; 4]);
            body
        }

        // --- INFO: just the version and the name ---
        let info = list(
            b"INFO",
            &[
                block(b"ifil", &[2u16.to_le_bytes(), 1u16.to_le_bytes()].concat()),
                // **`INAM` in uppercase, not `inam`**: the parser only accepts the format's exact
                // four letters, and the lowercase one brings down the whole load with
                // `ListContainsUnknownId`. It cost one attempt.
                block(b"INAM", &{
                    let mut n = b"Banco minimo do teste".to_vec();
                    n.push(0);
                    n
                }),
            ],
        );

        // --- sdta: the samples ---
        // **Without the RIFF even padding, and not through `block`.** It's the file's only
        // artificiality, and it belongs to the test, not to the format: `rustysynth` reads the
        // sub-chunks up to the `LIST` size and does **not** skip the padding. With an even-sized
        // `smpl` (the `minimal_bank` case) there is no padding at all, and the `LIST` comes out
        // byte for byte the same as `block`'s; with the other test's odd `smpl`, the padding byte
        // would sit between the samples and the `pdta` `LIST`, and the parser would read it as the
        // start of the identifier — the file would be rejected for misalignment, and not by the
        // guard, which is what is measured here.
        let smpl = {
            let mut out = b"smpl".to_vec();
            out.extend((raw.len() as u32).to_le_bytes());
            out.extend(raw);
            out
        };
        let sdta = {
            let mut body = b"sdta".to_vec();
            body.extend(smpl);
            let mut out = b"LIST".to_vec();
            out.extend((body.len() as u32).to_le_bytes());
            out.extend(body);
            out
        };

        // --- pdta ---
        // phdr: preset 0 and the terminator. The terminator's `wPresetBagNdx` is the zone count.
        let mut phdr = fixed_name("Piano do teste");
        phdr.extend(0u16.to_le_bytes()); // wPreset
        phdr.extend(0u16.to_le_bytes()); // wBank
        phdr.extend(0u16.to_le_bytes()); // wPresetBagNdx
        phdr.extend(0u32.to_le_bytes()); // wLibrary
        phdr.extend(0u32.to_le_bytes()); // wGenre
        phdr.extend(0u32.to_le_bytes()); // wMorphology
        let mut phdr_end = fixed_name("EOP");
        phdr_end.extend(0u16.to_le_bytes()); // wPreset
        phdr_end.extend(0u16.to_le_bytes()); // wBank
        phdr_end.extend(1u16.to_le_bytes()); // wPresetBagNdx: one preset zone
        phdr_end.extend([0u8; 12]); // wLibrary, wGenre, wMorphology
        let phdr = block(b"phdr", &[phdr, phdr_end].concat());

        // One preset zone, with the key range and instrument 0. **`INSTRUMENT` (41) last**, for
        // the same reason as `SAMPLE_ID` on the instrument side: with it in the middle, the parser
        // treats this zone as global and the preset is left with no region — and the symptom is
        // silence, not an error.
        let pbag = block(b"pbag", &[zone(0, 0), zone(2, 0)].concat());
        let pmod = block(b"pmod", &[0u8; 10]);
        let pgen = block(b"pgen", &generators(&[(43, 0x7f00), (41, 0)]));

        // inst: instrument 0 and the terminator.
        let mut inst0 = fixed_name("Inst do teste");
        inst0.extend(0u16.to_le_bytes());
        let mut inst_end = fixed_name("EOI");
        inst_end.extend(1u16.to_le_bytes());
        let inst = block(b"inst", &[inst0, inst_end].concat());

        // One instrument zone. **`SAMPLE_ID` last**: with it in the middle, the parser treats this
        // zone as global and the instrument is left with no region.
        let ibag = block(b"ibag", &[zone(0, 0), zone(4, 0)].concat());
        let imod = block(b"imod", &[0u8; 10]);
        let igen = block(b"igen", &generators(&[(43, 0x7f00), (54, 1), (58, 69), (53, 0)]));

        // shdr: sample 0 and the terminator.
        let mut shdr = fixed_name("Amostra do teste");
        shdr.extend(0u32.to_le_bytes()); // start
        shdr.extend((frames as u32 - 1).to_le_bytes()); // end
        shdr.extend(0u32.to_le_bytes()); // loop start
        shdr.extend((frames as u32 - 1).to_le_bytes()); // loop end
        shdr.extend(rate.to_le_bytes());
        shdr.push(60); // original pitch
        shdr.push(0); // pitch correction
        shdr.extend(0u16.to_le_bytes()); // link
        shdr.extend(1u16.to_le_bytes()); // type: mono
        let mut eos = fixed_name("EOS");
        eos.extend([0u8; 26]);
        let shdr = block(b"shdr", &[shdr, eos].concat());

        let pdta = list(b"pdta", &[phdr, pbag, pmod, pgen, inst, ibag, imod, igen, shdr]);

        let mut sfbk = b"sfbk".to_vec();
        sfbk.extend(info);
        sfbk.extend(sdta);
        sfbk.extend(pdta);
        block(b"RIFF", &sfbk)
    }


    /// **A corrupted bank can't bring the game down.** It's a file the user downloads and copies
    /// by hand: a truncated download or a `.sf2` in another format is a real scenario, not a
    /// hypothesis. The engine has to carry on with the wavetable and not say anything wrong.
    #[test]
    fn a_broken_bank_brings_nothing_down() {
        let dir = std::env::temp_dir().join("zeebx-banco-estragado.sf2");
        // Valid RIFF header and the rest garbage: it passes the most obvious check and fails when
        // reading the lists, which is where a really truncated file fails.
        let mut garbage = b"RIFF".to_vec();
        garbage.extend(1_000u32.to_le_bytes());
        garbage.extend(b"sfbk".to_vec());
        garbage.extend(vec![0x7f; 4_000]);
        std::fs::write(&dir, &garbage).expect("writes the garbage");

        assert!(
            open(&dir).is_none(),
            "a broken bank has to be rejected, not half accepted"
        );
        // With the bank rejected, MIDI goes through the wavetable — the production path
        // (`machine::media`) asks for `sound_bank`, which is `None`, and falls to the synthesizer.
        let sound = crate::audio::midi::decode(&one_note(0, 69, 240)).expect("the wavetable handles it");
        assert!(sound.samples.iter().any(|s| *s != 0.0), "the wavetable should have sounded");
        let _ = std::fs::remove_file(&dir);
    }

    /// **An odd-sized `smpl` is rejected before it reaches the dependency.**
    ///
    /// `rustysynth` 1.3.6 (`binary_reader.rs`, `read_wave_data`) reserves a `Vec<i16>` of
    /// `size / 2` elements and creates, with `slice::from_raw_parts_mut`, a slice of `size`
    /// **bytes**: with an odd-sized `smpl` the slice is one byte larger than the allocation, and the
    /// read writes outside it. It's UB on the user's bank path — a file they copy by hand, and the
    /// defect is in the library —, so the guard is ours and comes before the parse.
    ///
    /// **Without the guard this test fails, and that's what it measures:** the bank built here is
    /// valid in every other way (the same as `minimal_bank`), so `rustysynth` accepts it — writing
    /// one byte past the allocation — and `open` returns `Some` instead of `None`.
    #[test]
    fn a_bank_with_an_odd_smpl_is_rejected() {
        let dir = std::env::temp_dir().join("zeebx-banco-smpl-impar.sf2");
        let (rate, frames) = (22_050u32, 64usize);
        let mut raw = minimal_bank_samples(rate, frames);
        // The extra byte: with it the `smpl` declares an odd size, and `rustysynth`'s `Vec<i16>`
        // ends up one byte smaller than the slice the library creates over it.
        raw.push(0);
        assert_eq!(raw.len() % 2, 1, "the case is the odd chunk");
        std::fs::write(&dir, minimal_bank_with_smpl(&raw, rate, frames)).expect("writes the bank");

        assert!(
            open(&dir).is_none(),
            "an odd-sized `smpl` has to be rejected before the parse"
        );
        let _ = std::fs::remove_file(&dir);
    }

    /// **A path that doesn't exist is also rejected silently**, and without spending a load.
    #[test]
    fn a_missing_bank_is_not_an_error() {
        let path = std::env::temp_dir().join("zeebx-banco-que-nao-existe.sf2");
        let _ = std::fs::remove_file(&path);
        assert!(open(&path).is_none());
        // Through the given folders, not the profile: with a bank in `~/.config/zeebx` this
        // assertion failed — and that's where the README says to copy the `.sf2`.
        assert!(first_bank_in(&[std::env::temp_dir().join("zeebx-sem-pasta")]).is_none());
    }


    /// **The report says where to put the bank**, otherwise "the bank doesn't work" is
    /// indistinguishable from "the file is in the wrong place". This is the side of the build
    /// **with** the synthesizer; the side without it is checked in `audio::mod`, and both exist
    /// because silence in one of the builds leads to the wrong conclusion.
    #[cfg(feature = "soundfont")]
    #[test]
    fn the_report_says_where_to_put_the_bank() {
        let device = std::env::temp_dir().join("zeebx-aparelho-sem-banco");
        let _ = std::fs::remove_dir_all(&device);
        let text = report_for(&device, None);
        assert!(
            text.contains("soundfonts"),
            "the report has to name the folder: {text}"
        );
        assert!(
            text.contains("wavetable"),
            "without a bank, the report has to say what the soundtrack plays with: {text}"
        );
    }

    /// **The bank path is always exercised**, with a bank built here.
    ///
    /// The module's other tests use a real `.sf2` and skip themselves when there isn't one — and on
    /// CI there isn't. This one doesn't skip: it builds the minimal bank, loads it and plays.
    #[test]
    fn the_bank_path_plays_a_minimal_bank() {
        let dir = std::env::temp_dir().join("zeebx-banco-minimo.sf2");
        std::fs::write(&dir, minimal_bank()).expect("writes the bank");
        let soundfont = open(&dir).expect("the minimal bank opens");
        assert!(soundfont.presets() >= 1, "the bank has to declare the preset");
        let sound = play(&soundfont, &one_note(0, 69, 240), 22_050).expect("plays");
        assert_eq!(sound.channels, 1);
        let peak = sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.01, "the minimal bank should have sounded, peak {peak}");
        // The duration follows the score, as on the wavetable path: a sound that's too short
        // would make the game think the music ended early.
        let expected = 22_050 * 2; // a 240-tick note at 96 per beat and 120 per minute
        assert!(
            sound.samples.len() >= expected / 2,
            "sound too short: {} samples",
            sound.samples.len()
        );
        let _ = std::fs::remove_file(&dir);
    }

    /// **The bank tells the bow from the pick.** It was the wavetable's most audible defect:
    /// 29 (*overdrive*), 30 (distortion), 42 (cello) and 48 (strings) came out with the **same
    /// waveform**.
    ///
    /// The measure is the **attack**, not the centroid: on a single note both have a similar
    /// centroid (measured: 435 Hz and 426 Hz), so the centroid doesn't separate them. What does is
    /// the behavior — a pick attacks at full force, a bow starts soft and swells. In the reference
    /// soundfont the guitar starts at 1.00 of its own peak and the cello at 0.46.
    #[test]
    fn the_bank_tells_the_bow_from_the_pick() {
        let Some(path) = test_bank() else {
            eprintln!("no .sf2 bank on disk: test skipped (use ZEEBX_SOUNDFONT)");
            return;
        };
        let soundfont = open(&path).expect("the bank opens");
        let envelope = |program: u8| -> Vec<f32> {
            let sound = play(&soundfont, &one_note(program, 60, 480), 22_050).expect("plays");
            let window = sound.rate as usize / 5;
            let mut output: Vec<f32> = (0..5)
                .map(|i| {
                    let chunk = &sound.samples[i * window..(i + 1) * window];
                    (chunk.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt()
                })
                .collect();
            let peak = output.iter().fold(0.0f32, |a, s| a.max(*s));
            if peak > 0.0 {
                for value in &mut output {
                    *value /= peak;
                }
            }
            output
        };
        let pick = envelope(29);
        let bow = envelope(42);
        assert!(
            bow[0] < pick[0] * 0.75,
            "the bow should have attacked softer than the pick: {:.2} vs {:.2}",
            bow[0],
            pick[0]
        );
    }

    /// The chosen bank beats the search, and a chosen one that vanished doesn't mute: the search
    /// goes on.
    ///
    /// Touches the global bank and resets it to `None` at the end; no other test calls
    /// [`first_bank`], which is what reads it — they use [`first_bank_in`].
    #[test]
    fn the_chosen_bank_beats_the_search_and_a_vanished_one_does_not_mute() {
        let dir = std::env::temp_dir().join("zeebx-banco-escolhido");
        std::fs::create_dir_all(&dir).unwrap();
        let chosen = dir.join("firmware.sf2");
        std::fs::write(&chosen, b"RIFF").unwrap();
        let device = dir.join("aparelho-sem-bancos");

        set_soundfont(Some(chosen.clone()));
        assert_eq!(chosen_bank(), Some(chosen.clone()));
        assert_eq!(first_bank(&device), Some(chosen.clone()));

        let vanished = dir.join("nao-existe.sf2");
        set_soundfont(Some(vanished.clone()));
        assert_ne!(first_bank(&device), Some(vanished));

        // A cleared settings field arrives as an empty path, and the automatic search applies.
        set_soundfont(Some(PathBuf::new()));
        assert_eq!(chosen_bank(), None);
        set_soundfont(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_midi_backend_policy_parses() {
        use crate::audio::MidiBackend;
        use std::str::FromStr;

        assert_eq!(MidiBackend::from_str("Auto"), Ok(MidiBackend::Auto));
        assert_eq!(MidiBackend::from_str("automático"), Ok(MidiBackend::Auto));
        assert_eq!(MidiBackend::from_str("Tabela de timbres"), Ok(MidiBackend::Wavetable));
        assert_eq!(MidiBackend::from_str("timbres"), Ok(MidiBackend::Wavetable));
        assert_eq!(MidiBackend::from_str("SoundFont"), Ok(MidiBackend::SoundFont));
        assert_eq!(MidiBackend::from_str("sf2"), Ok(MidiBackend::SoundFont));
        assert_eq!(MidiBackend::from_str("invalido"), Err(()));
    }
}
