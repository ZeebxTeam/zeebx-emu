//! IMedia: the sound and video playback the game asks for.

use super::*;

/// How many decoded sounds the cache keeps before forgetting the ones nobody uses. See
/// [`Machine::drop_orphan_sounds`].
///
/// On the Switch, 64 entries throw away a track nobody is playing and the next playback
/// synthesizes it again. The byte cap is still the memory limit.
///
/// On the Vita the count stays at 64. The console has 512 MB for the whole system, and the
/// interpreter no longer reserves the JIT code block (32 MiB on the Switch, 64 on the
/// desktop). Raising the count, as on the Switch, fits less well here.
#[cfg(zeebx_switch)]
const MAX_STORED_SOUNDS: usize = 512;
#[cfg(not(zeebx_switch))]
const MAX_STORED_SOUNDS: usize = 64;

/// How much decoded PCM the cache keeps, in bytes, before forgetting the ones nobody uses.
///
/// **Counting entries is not counting memory, and the gap has grown.** A short effect takes a
/// few kilobytes and a long track takes tens of megabytes: 64 entries can be half a megabyte of
/// effects or over a hundred megabytes of music, and only the second case matters. The longest
/// track measured in Double Dragon is 109 s, which at 44,100 Hz in `f32` comes to 19.3 MB — four
/// of them stored go past 77 MB.
///
/// The number grew on purpose along with [`crate::audio::soundfont::BANK_RATE`]: synthesizing
/// the bank at 44,100 instead of 22,050 **doubles** the PCM of every song. On a handheld there is
/// little RAM to spare, and the cost of forgetting is a re-synthesis; the cost of not forgetting
/// is the system killing the process.
const MAX_SOUND_BYTES: usize = 24 * 1024 * 1024;

/// The currently chosen cap, which starts at [`MAX_SOUND_BYTES`] and the frontend can change.
///
/// Global for the same reason as the bank rate: the value applies to the next eviction, not to
/// the machine's creation. See `crate::audio::soundfont::set_rate`.
static CHOSEN_CAP: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(MAX_SOUND_BYTES);

/// Changes how much decoded PCM the cache may keep, in bytes.
///
/// The 1 MiB floor exists so that a value that is too small does not turn the cache into "forget
/// everything on every sound", which is worse than having no cache: it would cost a re-synthesis
/// per effect played.
pub fn set_sound_cache_cap(bytes: usize) {
    CHOSEN_CAP.store(bytes.max(1024 * 1024), std::sync::atomic::Ordering::Relaxed);
}

impl<C: CpuBackend> Machine<C> {
    /// `ISound` (`AEECLSID_SOUND` = `0x01001056`), from `inc/AEEISound.h`.
    ///
    /// It is BREW's *basic* sound API: tones from the device's generator, vibration and volume —
    /// it does not play files, that is `ISoundPlayer`. Here it is silent: it accepts everything,
    /// keeps the state the game can read back and tells the callback that playback has finished.
    /// Without that, games that check the return of `Set` give up on initialization.
    /// `IMediaUtil` and `IMedia`, from `sdk/inc/AEEMediaUtil.h` and `inc/AEEIMedia.h`.
    ///
    /// Mute, like `ISound`: they keep the playback state and answer what the game expects, but no
    /// sound comes out. What matters here is that they exist — Quake creates the music player
    /// during audio initialization and, if that fails, it carries on and later calls `Play` on a
    /// null pointer, without checking.
    pub(super) fn media_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.arg(0);
        let call = self.describe_media_call(iface, name)?;
        let result = self.media_call_inner(iface, name, this)?;
        if let Some(result) = result {
            self.note_media(this, format!("{call} -> {result:#x}"));
        } else {
            self.note_media(this, format!("{call} -> NOT IMPLEMENTED"));
        }
        Ok(result)
    }

    /// The call as text, with the arguments that say something about the sound.
    fn describe_media_call(
        &mut self,
        iface: Interface,
        name: &str,
    ) -> Result<String, CpuError> {
        let (a1, a2, a3) = (self.arg(1), self.arg(2), self.arg(3));
        Ok(match (iface, name) {
            (Interface::Media, "SetMediaParm") => {
                let extra = match a1 {
                    MM_PARM_MEDIA_DATA if a2 != 0 => {
                        let class_id = self.cpu.read_u32(a2)?;
                        let data = self.cpu.read_u32(a2 + 4)?;
                        let size = self.cpu.read_u32(a2 + 8)?;
                        let mut head = [0u8; 4];
                        if class_id == MMD_BUFFER && data != 0 {
                            self.cpu.read_mem(data, &mut head).ok();
                        }
                        match class_id {
                            MMD_FILE_NAME if data != 0 => format!(
                                " file{{{:?}}}",
                                self.cpu.read_cstring(data, MAX_STRING)
                            ),
                            _ => format!(
                                " data{{class {class_id:#x}, {size} bytes at {data:#x}, {:?}}}",
                                String::from_utf8_lossy(&head)
                            ),
                        }
                    }
                    _ => String::new(),
                };
                format!("SetMediaParm(parm {a1}, {a2:#x}, {a3:#x}){extra}")
            }
            (Interface::MediaUtil, _) => format!("IMediaUtil::{name}({a1:#x}, {a2:#x})"),
            _ => format!("{name}({a1:#x}, {a2:#x})"),
        })
    }

    /// Stores a line in [`Machine::media_log`], merging consecutive repeats.
    fn note_media(&mut self, object: u32, line: String) {
        let now = self.elapsed_ms();
        if let Some(last) = self.media_log.back_mut() {
            if last.1 == object && last.2 == line {
                last.3 += 1;
                return;
            }
        }
        if self.media_log.len() == MEDIA_LOG_MAX {
            self.media_log.pop_front();
        }
        self.media_log.push_back((now, object, line, 1));
    }

    /// The media log, for the report: `(instant, object, call, consecutive times)`.
    pub fn media_log(&self) -> Vec<(u32, u32, String, u32)> {
        self.media_log.iter().cloned().collect()
    }

    fn media_call_inner(
        &mut self,
        iface: Interface,
        name: &str,
        this: u32,
    ) -> Result<Option<u32>, CpuError> {
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.media.remove(&this);
                    self.pcm_streams.remove(&this);
                    self.live_buffers.remove(&this);
                    // A looping song kept playing after the object was gone.
                    if let Some(mixer) = &self.audio {
                        mixer.stop(this);
                    }
                }
                remaining
            }
            (_, "QueryInterface") => {
                let out = self.arg(2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateMedia(IMediaUtil *, AEEMediaData *, IMedia **ppm)
            (Interface::MediaUtil, "CreateMedia" | "CreateMediaEx") => {
                let out = self.arg(2);
                let media = self.new_object(Interface::Media)?;
                if media == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let mut state = MediaState::default();
                match self.read_media_data(self.arg(1))? {
                    Delivery::Ready(payload) => state.payload = payload,
                    Delivery::Buffer(location, size) => {
                        (state.pending, state.buffer) = ((location, size), (location, size))
                    }
                    Delivery::Nothing => {}
                }
                self.media.insert(media, state);
                if out != 0 {
                    self.cpu.write_u32(out, media)?;
                }
                SUCCESS
            }
            // int SetMediaParm(IMedia *, int16 nParmID, int32 p1, int32 p2)
            (Interface::Media, "SetMediaParm") => {
                let (parm, p1) = (self.arg(1), self.arg(2));
                let mut state = self
                    .media
                    .get(&this)
                    .copied()
                    .unwrap_or(MediaState::default());
                let mut param_result = SUCCESS;
                match parm {
                    MM_PARM_MEDIA_DATA if self.read_pcm_stream(this, p1)? => {}
                    MM_PARM_MEDIA_DATA => match self.read_media_data(p1)? {
                        Delivery::Ready(payload) => {
                            (state.payload, state.pending, state.buffer) = (payload, (0, 0), (0, 0))
                        }
                        Delivery::Buffer(location, size) => {
                            (state.pending, state.buffer) = ((location, size), (location, size))
                        }
                        Delivery::Nothing => param_result = EFAILED,
                    },
                    MM_PARM_VOLUME => state.volume = p1.min(MAX_VOLUME),
                    MM_PARM_MUTE => state.muted = p1 != 0,
                    MM_PARM_PLAY_REPEAT => state.repeat = p1,
                    // The rest of `MM_PARM_XXX` is accepted and ignored: refusing would make
                    // games give up on playing because of a setting that does not change the
                    // sound.
                    _ => {}
                }
                self.media.insert(this, state);
                if let (Some(mixer), MM_PARM_VOLUME | MM_PARM_MUTE) = (&self.audio, parm) {
                    mixer.set_volume(this, state.gain());
                }
                param_result
            }
            // void RegisterNotify(IMedia *, PFNMEDIANOTIFY pfn, void *pUser)
            (Interface::Media, "RegisterNotify") => {
                let notify = Callback {
                    function: self.arg(1),
                    context: self.arg(2),
                };
                self.media.entry(this).or_default().notify = notify;
                SUCCESS
            }
            (Interface::Media, "Play") => self.media_play(this)?,
            (Interface::Media, "Resume") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, false);
                }
                SUCCESS
            }
            (Interface::Media, "Pause") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY_PAUSE;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, true);
                }
                SUCCESS
            }
            // `Stop` tells the callback that playback has finished, **with `MM_STATUS_DONE`**.
            // Crash Nitro Kart counts the active sounds and only plays the track music when the
            // count reaches zero; its handler (`0x11a80`) decrements on `DONE` and on status 9
            // and ignores `ABORT`. With no notification — and later with `ABORT` —, the menu
            // songs stopped at the start of the race never left the count, and the whole race was
            // silent. Double Dragon treats both statuses the same way.
            (Interface::Media, "Stop") => {
                let playing = self.is_playing(this);
                // **Who silences the speech.** In Turma da Mônica, it is the game that ends the
                // speech, through `Stop`, when its decoder reaches the end — see
                // [`Machine::open_live_buffer`]. This line says whose order it was: the game's,
                // through `Stop`, or ours, by end of sound — and how many seconds actually
                // played.
                if playing
                    && let Some(decoded) = self.media_sound(this)?
                {
                    let dur_s = decoded.n_samples() as f64
                        / f64::from(decoded.channels.max(1))
                        / f64::from(decoded.rate.max(1));
                    // **A looping sound has no end, and so no "how much is left".** Its
                    // `ends_us` is `u64::MAX`, and subtracting the clock gave a twelve-digit
                    // number that looked like a bug of ours in the measurements. The right
                    // sentence is one that can be read.
                    let looping = self
                        .media
                        .get(&this)
                        .is_some_and(|playback| playback.ends_us == u64::MAX);
                    let expected = self
                        .media
                        .get(&this)
                        .map(|playback| playback.ends_us.saturating_sub(self.now_us()) as f64 / 1e6)
                        .unwrap_or(0.0);
                    if looping {
                        crate::logging!(
                            crate::logging::Level::Info,
                            "midia",
                            "Stop {}: the game stopped a {:.2}s sound that was looping",
                            this,
                            dur_s
                        );
                    } else {
                        crate::logging!(
                            crate::logging::Level::Info,
                            "midia",
                            "Stop {}: the game stopped a {:.2}s sound with {:.2}s still left to play",
                            this,
                            dur_s,
                            expected.max(0.0)
                        );
                    }
                }
                if let Some(stream) = self.pcm_streams.get_mut(&this) {
                    stream.playing = false;
                }
                self.live_buffers.remove(&this);
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_READY;
                    state.ends_us = 0;
                    state.play_on_load = false;
                }
                if let Some(mixer) = &self.audio {
                    mixer.stop(this);
                }
                if playing {
                    self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
                }
                SUCCESS
            }
            // int GetState(IMedia *, boolean *pbStateChanging) — the state is the return value,
            // and the pointer says whether it is in transition. Here it never is: the change is
            // immediate.
            (Interface::Media, "GetState") => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_mem(out, &[0])?;
                }
                // A sound that has finished returns the object to "ready": that is what the
                // game polls to know the sound is over. What says it is over is the **virtual
                // clock**, not the mixer — for the same reason as in `media_play`: the emulator
                // runs muted without stopping counting time, and some sounds play in silence
                // because we cannot decode them, only time them.
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                if state.state == MM_STATE_PLAY && now >= state.ends_us {
                    state.state = MM_STATE_READY;
                }
                state.state
            }
            // int32 GetTotalTime(IMedia *) — the sound's duration, in milliseconds.
            //
            // A sound that plays in silence answers its duration like any other. Zero here is a
            // divisor waiting to happen: whoever builds a progress bar divides by the total, and
            // a zero total brings the game down because of an answer of ours.
            (Interface::Media, "GetTotalTime") => match {
                // The duration needs the sound: whoever asks cannot wait for the next turn.
                self.resolve_media(this)?;
                self.media_sound(this)?
            } {
                Some(sound) => (sound.frames() as u64 * 1000 / u64::from(sound.rate.max(1))) as u32,
                None => (self.media_silent_length(this)?.unwrap_or(0) / 1000) as u32,
            },
            // Volume and mute return what was stored: a game that reads the volume and writes it
            // back used to mute itself with the zero from before. The rest is still zeroed.
            (Interface::Media, "GetMediaParm") => {
                let state = self.media.get(&this).copied().unwrap_or_default();
                let value = match self.arg(1) {
                    MM_PARM_VOLUME => state.volume,
                    MM_PARM_MUTE => u32::from(state.muted),
                    _ => 0,
                };
                for (index, v) in [(2, value), (3, 0)] {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, v)?;
                    }
                }
                SUCCESS
            }
            // The rest of `IMedia` answers success without doing anything — and now shows up in
            // the log by name, which is what was missing to know whether a silent match depends
            // on it.
            (Interface::Media, _) => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Reads an `AEEMediaData` from the guest and returns the sound's key in
    /// [`Machine::media_payloads`].
    ///
    /// The struct is `{ AEECLSID clsData; void *pData; uint32 dwSize; }`. `MMD_BUFFER` points to
    /// the bytes; `MMD_FILE_NAME` points to the name of a file in the package. The bytes are read
    /// and decoded **now** — see [`MediaPayload`]. `None` when there is no sound to store: a file
    /// that does not exist or a delivery form we don't know.
    pub(super) fn read_media_data(&mut self, pointer: u32) -> Result<Delivery, CpuError> {
        if pointer == 0 {
            return Ok(Delivery::Nothing);
        }
        let class = self.cpu.read_u32(pointer)?;
        let data = self.cpu.read_u32(pointer + 4)?;
        let size = self.cpu.read_u32(pointer + 8)?;
        let bytes = match class {
            MMD_BUFFER if data != 0 && size != 0 && size <= MAX_MEDIA_BUFFER => {
                return Ok(Delivery::Buffer(data, size));
            }
            MMD_BUFFER => return Ok(Delivery::Nothing),
            MMD_FILE_NAME if data != 0 => {
                let name = self.cpu.read_cstring(data, MAX_STRING);
                match self.vfs.resolve(&name).and_then(|p| std::fs::read(p).ok()) {
                    Some(bytes) => bytes,
                    None => {
                        self.bad_pointers
                            .insert(format!("sound requested by name, and the file does not exist: {name}"));
                        return Ok(Delivery::Nothing);
                    }
                }
            }
            _ => {
                self.note_bad_pointer(format!(
                    "a media item was delivered as {class:#010x}, and we can only read memory and files"
                ));
                return Ok(Delivery::Nothing);
            }
        };
        Ok(Delivery::Ready(self.store_sound(bytes)))
    }

    /// Reads now the buffer an object received that has not been read yet.
    ///
    /// **The buffer is read on the next turn of the loop, not on delivery nor on `Play`.** The two
    /// extremes break different games. Read on `Play`, a game that loads several sounds through
    /// the same scratch buffer has already reused it by the time it plays, and the wrong sound
    /// comes out. Read on delivery, Zeebo F.C. Super League delivers the menu navigation sound with
    /// only the header written — it says play and copies the samples right after, in the same
    /// handler —, and out came a hiss with the buffer's old contents (there was even the header of
    /// an ATC texture in there). On the device `Play` also only reads the buffer later, in a
    /// separate task.
    pub(super) fn resolve_media(&mut self, this: u32) -> Result<(), CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(());
        };
        let (location, size) = state.pending;
        if size == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes(location, size)?;
        let payload = self.store_sound(bytes);
        let should_play = match self.media.get_mut(&this) {
            Some(state) => {
                state.payload = payload;
                state.pending = (0, 0);
                std::mem::take(&mut state.play_on_load)
            }
            None => false,
        };
        if should_play {
            // The start notification already went out in `Play`.
            self.start_playback(this, false)?;
        }
        Ok(())
    }

    /// Reads the buffers left for this turn of the loop. See [`Machine::resolve_media`].
    pub(super) fn resolve_pending_media(&mut self) -> Result<(), CpuError> {
        let pending: Vec<u32> = self
            .media
            .iter()
            .filter(|(_, state)| state.pending.1 != 0)
            .map(|(this, _)| *this)
            .collect();
        for this in pending {
            self.resolve_media(this)?;
        }
        Ok(())
    }

    /// Decodes and stores a sound by its bytes, and returns its key.
    fn store_sound(&mut self, bytes: Vec<u8>) -> u64 {
        // Double Dragon's `sound.ggz` stores the sounds compressed: with the gzip envelope, the
        // real format is inside it.
        let bytes = match bytes.starts_with(&[0x1f, 0x8b]) {
            true => inflate(&bytes).unwrap_or(bytes),
            false => bytes,
        };
        let key_id = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            // Zero is "nothing delivered" in the object's state.
            hasher.finish().max(1)
        };
        if !self.media_payloads.contains_key(&key_id) {
            let payload = self.decode_sound(&bytes);
            // **The accounting issue #43 asks for, in one line per sound.** The format that
            // arrived (by its first bytes), how many bytes, and how many seconds we actually
            // decoded. If a long speech line shows up here with one second, the cut is here and not
            // in the game -- and that is what separates "does not decode" from "decodes and gets
            // cut".
            crate::logging!(
                crate::logging::Level::Info,
                "midia",
                "sound: {} bytes, signature {:?}, RIFF says {} and data says {}, decoded to {}",
                bytes.len(),
                String::from_utf8_lossy(&bytes[..bytes.len().min(4)]),
                // **The two sizes the parser has to choose between.** One game writes the whole
                // buffer in `data` and the truth in `RIFF`; Turma da Mônica seems to do the
                // opposite, with `RIFF` carrying only what has already been filled. Without these
                // two numbers side by side, the parser's choice is indistinguishable from the
                // bug.
                if bytes.len() >= 8 {
                    u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize + 8
                } else {
                    0
                },
                if bytes.len() >= 44 {
                    (0..bytes.len().saturating_sub(8))
                        .find(|&i| &bytes[i..i + 4] == b"data")
                        .and_then(|i| bytes.get(i + 4..i + 8))
                        .map(|t| u32::from_le_bytes([t[0], t[1], t[2], t[3]]) as usize)
                        .unwrap_or(0)
                } else {
                    0
                },
                match &payload.sound {
                    // **Divide by the channels, or stereo comes out doubled.** The samples come
                    // interleaved: a two-channel mp3 has twice as many samples as frames. Without
                    // this division, `menu_music.mp3` (15.67 s) showed up as 31.19 s.
                    Some(sound) => format!(
                        "{:.2}s at {} Hz, {} channel(s)",
                        sound.n_samples() as f64
                            / f64::from(sound.channels.max(1))
                            / f64::from(sound.rate.max(1)),
                        sound.rate,
                        sound.channels
                    ),
                    None => format!(
                        "nothing (timed only: {} ms)",
                        payload.silence_us.map(|us| us / 1000).unwrap_or(0)
                    ),
                },
            );
            self.media_payloads.insert(key_id, payload);
            self.drop_orphan_sounds(key_id);
        }
        key_id
    }

    /// Forgets the decoded sounds no `IMedia` uses, once past
    /// [`MAX_STORED_SOUNDS`].
    ///
    /// The cache exists so as not to decode again a sound the game delivers once more, but with no
    /// cap it kept **every** content that ever went through a `Play`: Zeebo F.C. writes different
    /// effects into the same 500 KB buffer, and each one became an entry forever.
    /// A playing voice loses nothing: its PCM is in an `Arc` the mixer also holds.
    fn drop_orphan_sounds(&mut self, create: u64) {
        // **Both caps, not just the entry one.** See [`MAX_SOUND_BYTES`]: sixty-four short
        // effects fit with room to spare, and four long songs synthesized by the bank no longer
        // fit. Whichever overflows first rules.
        let stored_bytes: usize = self
            .media_payloads
            .values()
            .filter_map(|payload| payload.sound.as_ref())
            .map(|sound| sound.bytes_pcm())
            .sum();
        let cap = CHOSEN_CAP.load(std::sync::atomic::Ordering::Relaxed);
        if self.media_payloads.len() <= MAX_STORED_SOUNDS && stored_bytes <= cap
        {
            return;
        }
        let in_use: std::collections::HashSet<u64> =
            self.media.values().map(|state| state.payload).collect();
        self.media_payloads
            .retain(|key_id, _| *key_id == create || in_use.contains(key_id));
    }


    /// Plays a score with the sample bank, when the device has one.
    ///
    /// It only tries when the bytes start with `MThd`: recognizing the format **before** opening
    /// the bank avoids spending 32 MB of loading on a sound that is not a score, and keeps the bank
    /// from changing the outcome for a format the table already handled.
    #[cfg(feature = "soundfont")]
    fn play_with_bank(&self, bytes: &[u8]) -> Option<crate::audio::wav::Sound> {
        if !bytes.starts_with(b"MThd") {
            return None;
        }
        let bank = self.sound_bank.as_ref()?;
        // **The bank's rate is not the table's.** See [`crate::audio::soundfont::BANK_RATE`]: the
        // `.sf2` samples are recorded at 44.1 kHz, and synthesizing at 22.05 cut their brightness
        // before the mixer had any chance to resample back.
        crate::audio::soundfont::play(bank, bytes, crate::audio::soundfont::rate())
    }

    /// Without the feature, the path is always the timbre table.
    #[cfg(not(feature = "soundfont"))]
    fn play_with_bank(&self, _bytes: &[u8]) -> Option<crate::audio::wav::Sound> {
        None
    }

    /// Decodes a sound by what it is, not by its name.
    fn decode_sound(&mut self, bytes: &[u8]) -> MediaPayload {
        // RIFF/WAVE first because it is what almost every sound is, and the cheapest to
        // recognize. MP3 next: it is the format of the **music**, and while it did not exist here,
        // effects played and soundtracks didn't, in any game.
        let decoded = match crate::audio::wav::parse(bytes) {
            Ok(sound) => Some(sound),
            Err(wav_error) => match crate::audio::mp3::decode_detailed(bytes) {
                Ok(sound) => Some(sound),
                // **The sample bank comes before the timbre table.** When it exists, the score is
                // played with the real samples, and what the table cannot reach (the harmonic
                // ratio of strings and distortion, the resonance of drums) comes from the bank
                // instead. Without a bank, the path is the usual one.
                Err(reason) => match self.play_with_bank(bytes) {
                    Some(sound) => {
                        self.assumptions.insert(concat!(
                            "MIDI music is played with the device's sample bank, not with ",
                            "the timbre table; the console's bank is in the firmware we have not read yet"
                        ));
                        Some(sound)
                    }
                    None => match {
                        let t_midi = std::time::Instant::now();
                        // On the Vita the long synthesis runs on a thread. On the frame's
                        // thread it holds the picture until the whole song exists.
                        let dec = if cfg!(target_os = "vita") {
                            crate::audio::midi::decode_nonblocking(bytes)
                        } else {
                            crate::audio::midi::decode(bytes)
                        };
                        if let Some(ref sound) = dec {
                            let dur_s = sound.n_samples() as f64 / sound.rate.max(1) as f64;
                            crate::logging!(
                                crate::logging::Level::Info,
                                "midi",
                                "timbre table: {} bytes of SMF -> {:.1}s of audio synthesized in {:.1}ms",
                                bytes.len(),
                                dur_s,
                                t_midi.elapsed().as_secs_f64() * 1000.0
                            );
                            self.assumptions.insert(concat!(
                                "MIDI music is synthesized here, with approximate timbre — ",
                                "the console's instrument bank is in the firmware we have not read yet"
                            ));
                        }
                        dec
                    } {
                        Some(sound) => Some(sound),
                        None => {
                    // Saying *which* format arrived is what lets us know what to implement
                    // next — and "not a RIFF/WAVE" doesn't say. What does is the block's own
                    // signature: that is how we learned Tekken 2's soundtrack is MP3 without
                    // opening the game, and that the arcade ports' is MIDI.
                    let format = detect_mime(bytes, "").unwrap_or("unknown format");
                    // The first bytes go into the report next to the format name: that is what
                    // **identifies** what arrived without opening the game. `FF FB` is an MP3
                    // frame, `ftyp` is an MP4 box, `OggS` is Ogg — and the difference between them
                    // decides what to implement. Without this, the line only said "rejected", and
                    // the next investigation started from scratch, inside the game.
                    let signature: String = bytes
                        .iter()
                        .take(16)
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                        // **Both reasons**, not just the WAV one: "not a RIFF/WAVE" is true
                        // and doesn't help — the question is what the music decoder rejected.
                        // Seeing both is how Turma da Mônica's Ogg was discovered.
                        self.note_bad_pointer(format!(
                            "sound rejected ({format}, {} bytes, {signature}): {wav_error} / {reason}",
                            bytes.len()
                        ));
                        None
                    }
                    },
                },
            },
        };
        let silence_us = match decoded {
            Some(_) => None,
            None => crate::audio::mp3::probe(bytes).map(|mp3| mp3.duration_us()),
        };
        MediaPayload {
            sound: decoded.map(std::sync::Arc::new),
            silence_us,
        }
    }

    /// The sound of an `IMedia` object, already read at delivery.
    pub(super) fn media_sound(
        &mut self,
        this: u32,
    ) -> Result<Option<std::sync::Arc<crate::audio::wav::Sound>>, CpuError> {
        Ok(self.payload_of(this).and_then(|payload| payload.sound.clone()))
    }

    /// How long a sound we cannot decode lasts, when it can be found out without decoding.
    ///
    /// Today only MP3 lands here, through the first frame's header and the encoder tag
    /// — see [`crate::audio::mp3`].
    pub(super) fn media_silent_length(&mut self, this: u32) -> Result<Option<u64>, CpuError> {
        Ok(self.payload_of(this).and_then(|payload| payload.silence_us))
    }

    fn payload_of(&self, this: u32) -> Option<&MediaPayload> {
        let payload = self.media.get(&this)?.payload;
        self.media_payloads.get(&payload)
    }

    /// Whether the object has a sound playing right now.
    fn is_playing(&self, this: u32) -> bool {
        self.media
            .get(&this)
            .is_some_and(|state| state.state == MM_STATE_PLAY && self.now_us() < state.ends_us)
    }

    /// `int Play(IMedia *)`.
    ///
    /// **A sound delivered through memory is reread on every `Play`.** The device's `IMedia` does
    /// not copy the buffer: it plays whatever is there. Zeebo F.C. Super League has a single
    /// object for the match effects, with a 500 KB buffer; it writes the kick, or the footstep,
    /// and says play again, without another `SetMediaParm`. Kept from the first read, every effect
    /// came out as the menu selection sound, which was the first to go through there.
    pub(super) fn media_play(&mut self, this: u32) -> Result<u32, CpuError> {
        if self.pcm_streams.contains_key(&this) {
            return self.start_stream(this);
        }
        // **A `Play` on music that is already looping does not restart it.** The Zeebo Extreme
        // games' sound manager says to play the track music again every time an effect ends —
        // the turbo, the drift —, and here the voice restarted from the beginning: the music
        // restarted on every effect. Refusing with `EBADSTATE` doesn't work either: it takes it
        // that the music stopped and repeats `Play` every frame. What it expects is `START`, which
        // moves it to "playing"; the voice carries on from where it is. The rule is limited to the
        // infinite loop: an effect played again over itself is what Zeebo F.C. does, and it needs
        // to restart.
        if self.is_playing(this) && self.media.get(&this).is_some_and(|state| state.repeat == 0)
        {
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
            return Ok(SUCCESS);
        }
        if let Some(state) = self.media.get_mut(&this)
            && state.buffer.1 != 0
        {
            state.pending = state.buffer;
        }
        // A sound not read yet starts when it is read, on the next turn of the loop. For the game
        // it is already playing.
        if let Some(state) = self.media.get_mut(&this)
            && state.pending.1 != 0
        {
            state.play_on_load = true;
            state.state = MM_STATE_PLAY;
            state.ends_us = u64::MAX;
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
            return Ok(SUCCESS);
        }
        self.start_playback(this, true)
    }

    /// Plays an object's sound **reading the game's buffer while it plays**, when the header alone
    /// does not decide where it ends. Returns `false` to go down the usual path.
    ///
    /// **Turma da Mônica's speech is decoded while it plays.** The game brings its own Vorbis
    /// decoder (Tremor) and writes each speech line, up to five seconds long, into an 882,000-byte
    /// buffer whose header comes from a template: `RIFF` of 56,352 (0.638 s) and `data` the size of
    /// the buffer. At `Play` only about 0.2 s is decoded; the rest arrives about 0.3 s ahead of
    /// what is playing, and `RIFF` never changes — measured over thirty seconds of play. Read all
    /// at once and cut at `RIFF`, the sound lasted 0.638 s, `DONE` arrived, and the game **cleared
    /// the buffer and stopped decoding**: the speech died at the start.
    ///
    /// Cutting at `data` doesn't work: in Zeebo F.C. Super League `RIFF` is right and what comes
    /// after it is memory garbage, loud. What separates the two is what the game does **after
    /// `Play`**: the sound only grows past `RIFF` with what is written there after it starts. The
    /// garbage that was already there does not count.
    fn open_live_buffer(&mut self, this: u32) -> Result<bool, CpuError> {
        self.live_buffers.remove(&this);
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(false);
        };
        let (location, size) = state.buffer;
        // A looping sound goes back to the start, and then there is no "what was written after":
        // it stays on the usual path.
        if location == 0 || size == 0 || state.repeat != 1 {
            return Ok(false);
        }
        let bytes = self.read_bytes(location, size)?;
        let Some(opened) = crate::audio::wav::open_pcm(&bytes) else {
            return Ok(false);
        };
        let frame = u32::from(opened.channels) * u32::from(opened.bits / 8);
        let relative = |n: usize| (n - opened.start) as u32 / frame * frame;
        let (end, limit) = (relative(opened.riff_end), relative(opened.data_end));
        let before = opened.start + end as usize..opened.start + limit as usize;
        let now = self.now_us();
        let live = LiveBuffer {
            pcm: location + opened.start as u32,
            rate: opened.rate,
            channels: opened.channels,
            bits: opened.bits,
            end,
            limit,
            before: bytes[before].to_vec(),
            base: end,
            start_us: now,
            sent: 0,
            scanned_us: 0,
        };
        // With no growth, it ends where `RIFF` says, as before: that is Super League.
        let ends_us = now + live.duration_us(end);
        let gain = state.gain();
        if let Some(state) = self.media.get_mut(&this) {
            state.state = MM_STATE_PLAY;
            state.ends_us = ends_us;
        }
        if let Some(mixer) = &self.audio {
            mixer.open_stream(this, live.rate, live.channels, gain);
        }
        self.live_buffers.insert(this, live);
        Ok(true)
    }

    /// Extends the live buffers with what the game wrote and hands the mixer what the clock
    /// already owes. See [`Machine::open_live_buffer`].
    ///
    /// **The end of the sound is projected, not "when the game stops".** On every growth `ends_us`
    /// becomes the end of what has been written so far plus [`LIVE_BUFFER_GRACE_US`]: that is what
    /// lets `GetState` and `DONE` work without a path of their own, and a save state loaded in the
    /// middle of speech — which does not store this state — still gets `DONE` on time.
    pub(super) fn pump_live_buffers(&mut self) -> Result<(), CpuError> {
        if self.live_buffers.is_empty() {
            return Ok(());
        }
        let now = self.now_us();
        let ids: Vec<u32> = self.live_buffers.keys().copied().collect();
        for this in ids {
            match self.media.get(&this).map(|state| state.state) {
                Some(MM_STATE_PLAY) => {}
                Some(MM_STATE_PLAY_PAUSE) => continue,
                _ => {
                    self.live_buffers.remove(&this);
                    continue;
                }
            }
            let Some(mut live) = self.live_buffers.remove(&this) else {
                continue;
            };
            let frame = live.frame();
            // Scanning on every turn of the loop was expensive: the game does thousands of turns
            // per second, and Tremor writes in chunks of tenths of a second.
            if now >= live.scanned_us + SCAN_INTERVAL_US && live.end < live.limit {
                live.scanned_us = now;
                let window = (live.rate / 2 * frame).max(frame);
                let end = live.limit.min(live.end.saturating_add(window));
                let fresh_bytes = self.read_bytes(live.pcm + live.end, end - live.end)?;
                let offset = (live.end - live.base) as usize;
                if let Some(written) = written_end(&fresh_bytes, &live.before[offset..offset + fresh_bytes.len()]) {
                    live.end = live
                        .limit
                        .min(live.end + (written as u32).div_ceil(frame) * frame);
                    let ends_us =
                        live.start_us + live.duration_us(live.end) + LIVE_BUFFER_GRACE_US;
                    if let Some(state) = self.media.get_mut(&this) {
                        state.ends_us = state.ends_us.max(ends_us);
                    }
                }
            }
            let due = (now.saturating_sub(live.start_us) + LIVE_BUFFER_LEAD_US)
                * u64::from(live.rate)
                / 1_000_000
                * u64::from(frame);
            let target = u64::from(live.end).min(due) as u32;
            if target > live.sent {
                let bytes = self.read_bytes(live.pcm + live.sent, target - live.sent)?;
                if let Some(mixer) = &self.audio {
                    // 8-bit PCM in WAVE is unsigned; 16-bit is signed.
                    mixer.feed_stream(this, &pcm_to_f32(&bytes, live.bits, live.bits == 8));
                }
                live.sent = target;
            }
            self.live_buffers.insert(this, live);
        }
        Ok(())
    }

    /// Starts playing an object's already read sound.
    fn start_playback(&mut self, this: u32, warn: bool) -> Result<u32, CpuError> {
        if self.open_live_buffer(this)? {
            if warn {
                self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
            }
            return Ok(SUCCESS);
        }
        // **A `Play` on a sound that is still playing does not notify.** Notifying `DONE` here
        // created a cycle in games that play again inside the notification handler: the new
        // `Play` landed on the sound that had just started, generated another notification, and
        // the sound restarted every frame — Zeebo F.C. Super League's audio came out clipped and
        // choppy. The end notification stays only in `Stop` and in the natural end.
        let Some(sound) = self.media_sound(this)? else {
            // A sound we cannot read but can **time** plays in silence for the right amount of
            // time. Without that Tekken 2 got stuck: its music is MP3, `Play` answered "this
            // sound has already ended", the game polled the state, saw "ready" and said play
            // again — 766 thousand times in four virtual seconds, which put it on the "far too
            // slow" list without having any work to do.
            if let Some(length_us) = self.media_silent_length(this)? {
                self.assumptions
                    .insert("a sound in a format we do not decode plays in silence, only with the right duration");
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                state.state = MM_STATE_PLAY;
                state.ends_us = match state.repeat {
                    0 => u64::MAX,
                    times => now + length_us * u64::from(times),
                };
                if warn {
                    self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
                }
                return Ok(SUCCESS);
            }
            // With no readable sound there is nothing to play, but refusing would make the game
            // treat it as a serious error; for the game, the sound simply ended right away.
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
            return Ok(SUCCESS);
        };
        // When the sound ends comes from the **virtual clock**, not from the mixer: a game that
        // waits for the end notification to play the next one needs to get it even with sound
        // off, or it goes silent for good after the first effect.
        let length_us = sound.frames() as u64 * 1_000_000 / u64::from(sound.rate.max(1));
        let now = self.now_us();
        let state = self.media.entry(this).or_default();
        state.state = MM_STATE_PLAY;
        state.ends_us = match state.repeat {
            0 => u64::MAX,
            times => now + length_us * u64::from(times),
        };
        let (gain, repeat) = (state.gain(), state.repeat);
        if let Some(mixer) = &self.audio {
            // The Vita mixes by wall-clock time, which runs ahead of virtual time.
            // A play-once track would end in the mixer and stay silent until the
            // game asked again, on DONE. Looping in the mixer holds the music;
            // `poll_media` cuts it when virtual time reaches the end. A short effect
            // still plays once: repeating a punch sounds like another punch.
            let mixer_repeat = if cfg!(target_os = "vita") && repeat != 0 && length_us >= 3_000_000
            {
                0
            } else {
                repeat
            };
            mixer.play(this, sound, gain, mixer_repeat);
        }
        if warn {
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        }
        Ok(SUCCESS)
    }

    /// Tells the game that a sound has started or reached its end.
    ///
    /// The notification is what closes the loop for games that play one thing at a time: without
    /// it the game waits forever for the previous effect to finish, and sound stops after the
    /// first one.
    ///
    /// **Every notification goes out on the next turn of the event loop, not on return from the
    /// call** — as in BREW, which delivers through the loop after the game's handler returns
    /// control. Three games measured the two choices:
    ///
    /// - The **Zeebo Extreme** games mark the sound as "requested" (2) *after* `Play`, and `START`
    ///   moves it to "playing" (1). Delivered on return from `Play`, `START` arrived before the
    ///   mark and the sound stayed at 2 forever; their `Stop` only acts on 1, the menu music never
    ///   stopped, and with the track music queued the player refused every effect — Bóia Cross
    ///   raced with the soundtrack only.
    /// - **Double Dragon** repeats the music through `DONE`: if the object is still marked, it
    ///   schedules a `Play` 100 ms later. It unmarks and releases the object right after `Stop`.
    ///   With `DONE` on return from `Stop`, the handler saw the object still marked, and the
    ///   scheduled `Play` landed on an already freed `IMedia` — address zero, on pressing back.
    /// - **Zeebo F.C. Super League** also stops and releases, and waits for that sound's `DONE`.
    ///   That is why the notification does **not** vanish with `Release`: the handler is stored
    ///   when the notification is born. Discarded along with the object, the intro stopped at the
    ///   notice screen.
    pub(super) fn notify_media(
        &mut self,
        this: u32,
        cmd: u32,
        status: u32,
    ) -> Result<(), CpuError> {
        if let Some(state) = self.media.get(&this)
            && state.notify.function != 0
        {
            self.media_notify_queue.push((this, cmd, status, state.notify));
        }
        Ok(())
    }

    /// Delivers the queued notifications, in order.
    ///
    /// The `AEEMediaCmdNotify` is written **at the time of each call**, in a single block: a
    /// `START` and a `DONE` in the same turn, written at enqueue time, would both say `DONE`.
    pub fn deliver_media_notifies(&mut self, budget: u64) -> Result<(), CpuError> {
        if self.media_notify_queue.is_empty() {
            return Ok(());
        }
        if self.media_notify_block == 0 {
            self.media_notify_block = self.alloc_internal(MEDIA_NOTIFY_LEN)?;
        }
        let block = self.media_notify_block;
        if block == 0 {
            self.media_notify_queue.clear();
            return Ok(());
        }
        for (this, cmd, status, notify) in std::mem::take(&mut self.media_notify_queue) {
            // `AEEMediaCmdNotify`: clsMedia, pIMedia, nCmd, nSubCmd, nStatus, pCmdData, dwSize.
            // The class goes zeroed — the game identifies the sound by the pointer, not by it.
            for (index, value) in [0, this, cmd, 0, status, 0, 0].into_iter().enumerate() {
                self.cpu.write_u32(block + index as u32 * 4, value)?;
            }
            self.call_guest(notify.function, [notify.context, block, 0, 0], budget)?;
        }
        Ok(())
    }

    /// Recognizes the delivery of a sound the game generates while it plays, and stores its
    /// format.
    ///
    /// The `AEEMediaDataEx` is `{ clsData, pData, dwSize, dwStructSize, dwCaps, bRaw, pSpec,
    /// dwSpecSize, dwBufferSize }`, and the `AEEMediaWaveSpec` pointed to by `pSpec` is `{ uint16
    /// wSize; AEECLSID clsMedia; uint16 wChannels; uint32 dwSamplesPerSec; uint16
    /// wBitsPerSample; boolean bUnsigned; uint32 dwAvgBytesPerSec; uint16 wBlockAlign }`.
    /// Caveman Ninja builds exactly that: 11025 Hz, mono, 16 bits.
    ///
    /// Returns `false` when the delivery is not that one, to go down the memory and file path.
    pub(super) fn read_pcm_stream(&mut self, this: u32, pointer: u32) -> Result<bool, CpuError> {
        if pointer == 0 || self.cpu.read_u32(pointer)? != MMD_ISOURCE {
            return Ok(false);
        }
        let source = self.cpu.read_u32(pointer + 4)?;
        let raw = self.cpu.read_u32(pointer + 20)? & 0xff != 0;
        let spec = self.cpu.read_u32(pointer + 24)?;
        if source == 0 || spec == 0 || !raw {
            self.note_bad_pointer("a sound came from an ISource without being raw PCM, and we can only play PCM".into());
            return Ok(false);
        }
        let mut bytes = [0u8; 24];
        self.cpu.read_mem(spec, &mut bytes)?;
        let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let channels = u16_at(8);
        let rate = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let bits = u16_at(16);
        let unsigned = bytes[18] != 0;
        if channels == 0 || rate == 0 || !matches!(bits, 8 | 16) {
            self.note_bad_pointer(format!(
                "PCM with {channels} channel(s), {rate} Hz and {bits} bits, which we cannot play"
            ));
            return Ok(false);
        }
        let per_frame = u32::from(channels) * u32::from(bits / 8);
        let block = match self.cpu.read_u32(pointer + 32)? {
            b if b >= per_frame && b <= MAX_PCM_READ => b - b % per_frame,
            _ => 0,
        };
        self.pcm_streams.insert(
            this,
            PcmStream {
                source,
                rate,
                channels,
                bits,
                unsigned,
                block,
                start_us: 0,
                frames_read: 0,
                playing: false,
                logged_end: false,
            },
        );
        Ok(true)
    }

    /// `Play` of a stream: from here on the samples are requested from the game on every turn of
    /// the loop.
    fn start_stream(&mut self, this: u32) -> Result<u32, CpuError> {
        let now = self.now_us();
        let state = self.media.entry(this).or_default();
        state.state = MM_STATE_PLAY;
        // A stream has no known end: it ends at `Stop`.
        state.ends_us = u64::MAX;
        let gain = state.gain();
        let Some(stream) = self.pcm_streams.get_mut(&this) else {
            return Ok(SUCCESS);
        };
        stream.start_us = now;
        stream.frames_read = 0;
        stream.playing = true;
        let (rate, channels) = (stream.rate, stream.channels);
        if let Some(mixer) = &self.audio {
            mixer.open_stream(this, rate, channels, gain);
        }
        self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        Ok(SUCCESS)
    }

    /// Asks the game for the samples the virtual clock already owes, and sends them to the mixer.
    ///
    /// What sets the pace is **virtual time**, as in the rest of the sound: with no sound card the
    /// emulator keeps asking, and the game, which usually emulates the sound chip inside `Read`,
    /// moves the same way. A tenth of a second goes ahead, so the card does not run dry between two
    /// turns.
    pub(super) fn pump_pcm_streams(&mut self, budget: u64) -> Result<(), CpuError> {
        let playing: Vec<u32> = self
            .pcm_streams
            .iter()
            .filter(|(this, stream)| {
                stream.playing
                    && self
                        .media
                        .get(this)
                        .is_some_and(|state| state.state == MM_STATE_PLAY)
            })
            .map(|(this, _)| *this)
            .collect();
        if playing.is_empty() {
            return Ok(());
        }
        if self.stream_buffer == 0 {
            self.stream_buffer = self.alloc_internal(MAX_PCM_READ)?;
            if self.stream_buffer == 0 {
                return Ok(());
            }
        }
        let buffer = self.stream_buffer;
        let now = self.now_us();
        for this in playing {
            let Some(stream) = self.pcm_streams.get(&this).copied() else {
                continue;
            };
            let per_frame = u32::from(stream.channels) * u32::from(stream.bits / 8);
            let due = (now.saturating_sub(stream.start_us) + 100_000) * u64::from(stream.rate)
                / 1_000_000;
            let mut owed = due.saturating_sub(stream.frames_read);
            let Ok(vtable) = self.cpu.read_u32(stream.source) else {
                continue;
            };
            let read = self.cpu.read_u32(vtable + ISOURCE_READ_SLOT * 4)?;
            // A few reads per turn are enough; a `Read` that returns less is the game with no
            // samples ready, and insisting in the same turn only wastes time.
            for _ in 0..8 {
                if owed == 0 {
                    break;
                }
                // **In `dwBufferSize` blocks, as BREW asks.** Prey 2 Evil's mixer asks for `n/4`
                // bytes of ADPCM per voice and treats `4 × got < n` as the end of the sound: with
                // the 734 bytes of 1/60 s at 22050 Hz, every odd read out of the pairs rewound the
                // voice and a 33 ms stretch played in a loop. Caveman Ninja declares 732 bytes,
                // almost what was already being requested; Prey declares 6144.
                let request = match stream.block {
                    0 => (owed * u64::from(per_frame)).min(u64::from(MAX_PCM_READ)) as u32,
                    block => block,
                };
                let request = request - request % per_frame;
                if request == 0 {
                    break;
                }
                let outcome = self.call_guest(read, [stream.source, buffer, request, 0], budget)?;
                let Outcome::Returned { code } = outcome else {
                    break;
                };
                let got = code as i32;
                if got <= 0 {
                    // **The point where a speech line dies.** The stream has no known end: the
                    // game ends it by no longer supplying samples. The line says after how many
                    // seconds of audio that happened and how the stream was declared.
                    if let Some(stream) = self.pcm_streams.get_mut(&this)
                        && !stream.logged_end
                    {
                        stream.logged_end = true;
                        crate::logging!(
                            crate::logging::Level::Info,
                            "midia",
                            "stream {}: the game stopped supplying samples at {:.2}s of audio ({} Hz, {} channel(s), {} bits)",
                            this,
                            stream.frames_read as f64 / f64::from(stream.rate.max(1)),
                            stream.rate,
                            stream.channels,
                            stream.bits
                        );
                    }
                    break;
                }
                let got = (got as u32).min(request);
                let bytes = self.read_bytes(buffer, got)?;
                let samples = pcm_to_f32(&bytes, stream.bits, stream.unsigned);
                if let Some(mixer) = &self.audio {
                    mixer.feed_stream(this, &samples);
                }
                let frames = u64::from(got / per_frame);
                if let Some(stored) = self.pcm_streams.get_mut(&this) {
                    stored.frames_read += frames;
                }
                owed = owed.saturating_sub(frames);
                if got < request {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Brings the mixer in line with a freshly restored save state.
    ///
    /// **The mixer is not part of the save state**, and what it was playing belonged to the moment
    /// before `Load`. Without this, going back with rewind left the next scene's music playing on
    /// top of the restored one — and forever, when it was a loop: the restored game does not know
    /// that voice exists, and never tells it to stop. Measured in Action Hero 3D.
    ///
    /// What the state says is playing plays again: an effect, from the point where the virtual
    /// clock left it; a loop, from the start, because the moment it started is not stored; a
    /// stream, empty, which [`Machine::pump_pcm_streams`] fills again. A sound whose cache is no
    /// longer in memory — the state came from another session — stays silent until the game plays
    /// it again, as it already did. Live buffers do not come back: see
    /// [`Machine::pump_live_buffers`].
    pub(super) fn rebuild_sound(&mut self) {
        self.live_buffers.clear();
        let Some(mixer) = self.audio.clone() else {
            return;
        };
        mixer.stop_all();
        let now = self.now_us();
        let playing: Vec<(u32, MediaState)> = self
            .media
            .iter()
            .filter(|(_, state)| state.state == MM_STATE_PLAY && now < state.ends_us)
            .map(|(&this, &state)| (this, state))
            .collect();
        for (this, state) in playing {
            if let Some(stream) = self.pcm_streams.get(&this) {
                if stream.playing {
                    mixer.open_stream(this, stream.rate, stream.channels, state.gain());
                }
                continue;
            }
            let Some(sound) = self.payload_of(this).and_then(|payload| payload.sound.clone()) else {
                continue;
            };
            let from_us = match state.repeat {
                0 => 0,
                times => {
                    let duration_us = sound.frames() as u64 * 1_000_000 / u64::from(sound.rate.max(1));
                    (duration_us * u64::from(times)).saturating_sub(state.ends_us - now)
                }
            };
            mixer.play_from(this, sound, state.gain(), state.repeat, from_us);
        }
    }

    /// Queues the end notification for the sounds that have already finished.
    pub(super) fn poll_media(&mut self) -> Result<(), CpuError> {
        let now = self.now_us();
        let finished: Vec<u32> = self
            .media
            .iter()
            .filter(|(_, state)| state.state == MM_STATE_PLAY && now >= state.ends_us)
            .map(|(this, _)| *this)
            .collect();
        for this in finished {
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
                state.ends_us = 0;
            }
            // The loop `start_playback` put on the long track does not end on its own.
            // Without this `stop`, the music would carry on after DONE.
            #[cfg(target_os = "vita")]
            if let Some(mixer) = &self.audio {
                mixer.stop(this);
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
        }
        Ok(())
    }
}

/// 8- or 16-bit little-endian PCM samples, to `f32` in [-1, 1].
pub(super) fn pcm_to_f32(bytes: &[u8], bits: u16, unsigned: bool) -> Vec<f32> {
    match bits {
        8 => bytes
            .iter()
            .map(|&b| match unsigned {
                true => (f32::from(b) - 128.0) / 128.0,
                false => f32::from(b as i8) / 128.0,
            })
            .collect(),
        _ => bytes
            .chunks_exact(2)
            .map(|c| {
                let value = u16::from_le_bytes([c[0], c[1]]);
                match unsigned {
                    true => (f32::from(value) - 32768.0) / 32768.0,
                    false => f32::from(value as i16) / 32768.0,
                }
            })
            .collect(),
    }
}

/// How far `now` differs from `before`: the index right after the last changed byte, or `None`
/// if nothing changed.
///
/// It is the **last** difference, not the first changed run: speech has pauses, and the silence
/// the decoder writes over a zeroed buffer changes nothing. Stopping at the first match would
/// hold the sound in the middle of the pause.
fn written_end(now: &[u8], before: &[u8]) -> Option<usize> {
    now
        .iter()
        .zip(before)
        .rposition(|(a, b)| a != b)
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::written_end;

    #[test]
    fn nothing_written_does_not_grow() {
        assert_eq!(written_end(&[0, 7, 0], &[0, 7, 0]), None);
    }

    /// The garbage that was already there is `before`, and so it does not count — that is Super
    /// League.
    #[test]
    fn only_counts_what_changed() {
        assert_eq!(written_end(&[9, 9, 9, 9], &[9, 9, 9, 9]), None);
        assert_eq!(written_end(&[9, 1, 9, 9], &[9, 9, 9, 9]), Some(2));
    }

    /// A pause in speech written over zeros does not stop the growth.
    #[test]
    fn a_pause_in_the_middle_does_not_hold_back_the_end() {
        let before = [0u8; 8];
        let now = [5, 5, 0, 0, 0, 5, 0, 0];
        assert_eq!(written_end(&now, &before), Some(6));
    }
}
