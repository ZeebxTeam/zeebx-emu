//! RIFF/WAVE reading.
//!
//! It's the format in which games hand sound to BREW: they pass an `AEEMediaData` of
//! `MMD_BUFFER` pointing to a RIFF in memory. Quake, Zeebo Sports Peteca and Double Dragon use
//! uncompressed PCM, mono, 8 or 16 bits, at rates from 11025 to 44100 Hz; Pac-Mania uses IMA
//! ADPCM, which compresses each sample into four bits.
//!
//! Written by hand instead of coming from a dependency because it's a twelve-byte header and a
//! list of chunks. A format we can't read yet is **rejected by name**, not decoded by chance:
//! the name is what tells us what to implement next.

/// `WAVE_FORMAT_PCM` and `WAVE_FORMAT_IMA_ADPCM`, from the RIFF format header.
const FORMAT_PCM: u16 = 1;
const FORMAT_IMA_ADPCM: u16 = 17;

/// How far the step table index moves, per nibble. It's the IMA ADPCM table.
const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// The 89 IMA ADPCM quantization steps.
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// The state of one IMA ADPCM channel: the previous sample and where we are in the step table.
#[derive(Debug, Clone, Copy)]
struct AdpcmChannel {
    predictor: i32,
    index: i32,
}

impl AdpcmChannel {
    /// Decodes one nibble and advances the state.
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEP_TABLE[self.index.clamp(0, 88) as usize];
        // The nibble carries three magnitude bits and one sign bit, and the sum is a fraction of
        // the step: `step/8` is the constant term that exists even when the three bits are zero.
        let magnitude = i32::from(nibble & 7);
        let mut delta = step >> 3;
        if magnitude & 4 != 0 {
            delta += step;
        }
        if magnitude & 2 != 0 {
            delta += step >> 1;
        }
        if magnitude & 1 != 0 {
            delta += step >> 2;
        }
        if nibble & 8 != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self
            .predictor
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        self.index = (self.index + i32::from(INDEX_TABLE[(nibble & 0xf) as usize])).clamp(0, 88);
        self.predictor as i16
    }
}

/// Decodes the IMA ADPCM of a WAVE.
///
/// The data comes in blocks of `block_align` bytes. Each block starts with four bytes per channel
/// — the first sample and the table index — and continues with the nibbles. In stereo they
/// alternate four bytes at a time per channel; in mono, straight through.
fn decode_ima_adpcm(payload: &[u8], channels: usize, block_align: usize) -> Vec<f32> {
    if channels == 0 || block_align < 4 * channels {
        return Vec::new();
    }
    let mut out = Vec::new();
    for block in payload.chunks(block_align) {
        if block.len() < 4 * channels {
            break;
        }
        let mut state: Vec<AdpcmChannel> = (0..channels)
            .map(|channel| {
                let at = channel * 4;
                AdpcmChannel {
                    predictor: i32::from(i16::from_le_bytes([block[at], block[at + 1]])),
                    index: i32::from(block[at + 2]),
                }
            })
            .collect();
        // The header sample is the first one of each channel.
        for channel in &state {
            out.push(channel.predictor as f32 / 32768.0);
        }

        let body = &block[4 * channels..];
        // Each group carries four bytes — eight samples — of each channel, in order.
        for group in body.chunks(4 * channels) {
            let mut decoded = vec![Vec::with_capacity(8); channels];
            for (channel, lane) in group.chunks(4).enumerate().take(channels) {
                for &byte in lane {
                    decoded[channel].push(state[channel].decode(byte & 0xf));
                    decoded[channel].push(state[channel].decode(byte >> 4));
                }
            }
            let frames = decoded.iter().map(Vec::len).min().unwrap_or(0);
            for frame in 0..frames {
                for lane in decoded.iter().take(channels) {
                    out.push(f32::from(lane[frame]) / 32768.0);
                }
            }
        }
    }
    out
}

/// A sound ready to play: samples normalized to `-1.0..=1.0`, interleaved by channel.
#[derive(Debug, Clone)]
pub struct Sound {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
    /// The same waveform in 16 bits, when the `f32` buffer didn't fit.
    ///
    /// Empty on the normal path. Double Dragon's 86.67 s track asks for 7,688,424
    /// bytes in `f32` (`(duration + 0.5) × 22050 + 1`, times 4). On the Vita that
    /// `malloc` fails — the 232 MB heap is already fragmented by the game's heap — and
    /// the process aborts. In 16 bits the same track takes half.
    pub compact: Vec<i16>,
    /// Filled by another thread. `None` when the sound arrived ready.
    live: Option<std::sync::Arc<LiveBuffer>>,
}

impl PartialEq for Sound {
    fn eq(&self, other: &Self) -> bool {
        self.rate == other.rate
            && self.channels == other.channels
            && self.samples == other.samples
            && self.compact == other.compact
            && self.live.is_none()
            && other.live.is_none()
    }
}

impl Sound {
    pub fn create(rate: u32, channels: u16, samples: Vec<f32>) -> Self {
        Self {
            rate,
            channels,
            samples,
            compact: Vec::new(),
            live: None,
        }
    }

    pub fn from_i16(rate: u32, channels: u16, compact: Vec<i16>) -> Self {
        Self {
            rate,
            channels,
            samples: Vec::new(),
            compact,
            live: None,
        }
    }

    pub(crate) fn synthesizing(rate: u32, channels: u16, live: std::sync::Arc<LiveBuffer>) -> Self {
        Self {
            rate,
            channels,
            samples: Vec::new(),
            compact: Vec::new(),
            live: Some(live),
        }
    }

    /// How many samples there are, whether in the wide buffer or the compact one.
    pub fn n_samples(&self) -> usize {
        if let Some(live) = &self.live {
            return live.total;
        }
        if self.samples.is_empty() {
            self.compact.len()
        } else {
            self.samples.len()
        }
    }

    /// Bytes this sound takes up in the cache.
    pub fn bytes_pcm(&self) -> usize {
        if let Some(live) = &self.live {
            return live.bytes;
        }
        self.samples.len() * std::mem::size_of::<f32>()
            + self.compact.len() * std::mem::size_of::<i16>()
    }

    /// Sample `index` has already been written. A ready sound always answers yes.
    pub fn reaches(&self, index: usize) -> bool {
        let Some(live) = &self.live else {
            return true;
        };
        let ready = live.ready.load(std::sync::atomic::Ordering::Acquire);
        ready >= live.total || index < ready
    }

    /// Sample `i`, in `-1.0..=1.0`.
    pub fn read(&self, i: usize) -> f32 {
        if let Some(live) = &self.live {
            return live.read(i);
        }
        if let Some(s) = self.samples.get(i) {
            return *s;
        }
        self.compact
            .get(i)
            .map(|s| f32::from(*s) / 32767.0)
            .unwrap_or(0.0)
    }

    /// How many audio frames the sound has — one sample per channel counts as one.
    pub fn frames(&self) -> usize {
        match self.channels {
            0 => 0,
            channels => self.n_samples() / channels as usize,
        }
    }
}

/// A buffer that a thread fills while the game carries on.
///
/// Synthesizing a long track takes seconds on the Vita. Done on the frame thread,
/// the picture stops and only comes back when the work is over. Here the game thread
/// gets the sound right away, with the right size, and the voice waits for the sample
/// that doesn't exist yet instead of jumping to the middle of the music.
pub(crate) struct LiveBuffer {
    samples: std::cell::UnsafeCell<Vec<f32>>,
    compact: std::cell::UnsafeCell<Vec<i16>>,
    ready: std::sync::atomic::AtomicUsize,
    total: usize,
    bytes: usize,
}

impl std::fmt::Debug for LiveBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveBuffer")
            .field("total", &self.total)
            .field(
                "ready",
                &self.ready.load(std::sync::atomic::Ordering::Relaxed),
            )
            .finish()
    }
}

// The reader only sees indices already published. The writer publishes the end of
// the window after writing it, with `Release`. No sample is read and written
// at the same time.
unsafe impl Send for LiveBuffer {}
unsafe impl Sync for LiveBuffer {}

impl LiveBuffer {
    pub(crate) fn from_f32(samples: Vec<f32>) -> Self {
        let total = samples.len();
        Self {
            bytes: total * std::mem::size_of::<f32>(),
            samples: std::cell::UnsafeCell::new(samples),
            compact: std::cell::UnsafeCell::new(Vec::new()),
            ready: std::sync::atomic::AtomicUsize::new(0),
            total,
        }
    }

    pub(crate) fn from_i16(compact: Vec<i16>) -> Self {
        let total = compact.len();
        Self {
            bytes: total * std::mem::size_of::<i16>(),
            samples: std::cell::UnsafeCell::new(Vec::new()),
            compact: std::cell::UnsafeCell::new(compact),
            ready: std::sync::atomic::AtomicUsize::new(0),
            total,
        }
    }

    pub(crate) fn total(&self) -> usize {
        self.total
    }

    pub(crate) fn is_wide(&self) -> bool {
        !unsafe { &*self.samples.get() }.is_empty()
    }

    pub(crate) fn write_f32(&self, pos: usize, data: &[f32]) {
        let buf = unsafe { &mut *self.samples.get() };
        buf[pos..pos + data.len()].copy_from_slice(data);
    }

    pub(crate) fn write_i16(&self, pos: usize, data: &[i16]) {
        let buf = unsafe { &mut *self.compact.get() };
        buf[pos..pos + data.len()].copy_from_slice(data);
    }

    pub(crate) fn publish(&self, end: usize) {
        self.ready
            .store(end, std::sync::atomic::Ordering::Release);
    }

    fn read(&self, i: usize) -> f32 {
        if i >= self.ready.load(std::sync::atomic::Ordering::Acquire) {
            return 0.0;
        }
        unsafe {
            let samples = &*self.samples.get();
            if !samples.is_empty() {
                return samples.get(i).copied().unwrap_or(0.0);
            }
            let compact = &*self.compact.get();
            compact
                .get(i)
                .map(|s| f32::from(*s) / 32767.0)
                .unwrap_or(0.0)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// Not a RIFF/WAVE.
    NotWave,
    /// The format chunk or the data chunk is missing.
    Incomplete,
    /// The format exists, but it isn't PCM. The number is the `wFormatTag`, which says which it is.
    NotPcm(u16),
    /// A bit depth we can't read.
    BadDepth(u16),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWave => write!(f, "not a RIFF/WAVE"),
            Self::Incomplete => write!(f, "the WAVE is missing required chunks"),
            Self::NotPcm(tag) => write!(f, "the WAVE is not PCM (format {tag})"),
            Self::BadDepth(bits) => write!(f, "the WAVE has {bits} bits per sample"),
        }
    }
}

/// Reads a PCM RIFF/WAVE.
///
/// How far the `data` chunk may go past the end the `RIFF` declares before the `RIFF` wins.
const RIFF_SLACK: usize = 4096;

/// A PCM WAVE in which the `data` chunk goes well past the end the `RIFF` declares, with the
/// positions in bytes counted from the start of the buffer.
///
/// It's the case where the header alone doesn't decide where the sound ends. In Zeebo F.C. Super
/// League the `RIFF` is right and what comes after is garbage; in Turma da Mônica the `RIFF`
/// belongs to a template and the speech continues past it, **written while it plays**. What tells
/// the two apart is what the game does with the buffer after `Play` — see
/// `Machine::pump_live_buffers`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenPcm {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    /// Where the PCM starts.
    pub start: usize,
    /// Where the `RIFF` says the sound ends.
    pub riff_end: usize,
    /// Where the `data` chunk ends, clamped to the buffer.
    pub data_end: usize,
}

/// The [`OpenPcm`] of a buffer, or `None` when the header is enough — another format, or a `data`
/// that doesn't go past the `RIFF` by more than the slack.
pub fn open_pcm(data: &[u8]) -> Option<OpenPcm> {
    let layout = layout(data).ok()?;
    let (tag, channels, rate, _, bits) = layout.format?;
    let (start, data_end) = layout.data?;
    let past_riff =
        layout.riff_end >= 44 && data_end > layout.riff_end + RIFF_SLACK;
    (tag == FORMAT_PCM && matches!(bits, 8 | 16) && channels > 0 && rate > 0 && past_riff)
        .then_some(OpenPcm {
            channels,
            rate,
            bits,
            start,
            riff_end: layout.riff_end.max(start),
            data_end,
        })
}

/// The chunks of a RIFF/WAVE, without interpreting the PCM.
struct WavLayout {
    /// `(format, channels, rate, alignment from block, bits)`.
    format: Option<(u16, u16, u32, u16, u16)>,
    /// Where the `data` chunk starts and ends, clamped to the buffer.
    data: Option<(usize, usize)>,
    riff_end: usize,
}

fn layout(data: &[u8]) -> Result<WavLayout, WavError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let riff_end = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize + 8;
    let mut layout = WavLayout {
        format: None,
        data: None,
        riff_end,
    };
    // The chunks come in sequence, each with an identifier and a size. An odd-sized chunk is
    // followed by a padding byte that doesn't count in the size.
    let mut at = 12usize;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
        let body = at + 8;
        let end = body.saturating_add(len as usize).min(data.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let read16 = |i: usize| u16::from_le_bytes([data[body + i], data[body + i + 1]]);
                let rate = u32::from_le_bytes([
                    data[body + 4],
                    data[body + 5],
                    data[body + 6],
                    data[body + 7],
                ]);
                layout.format = Some((read16(0), read16(2), rate, read16(12), read16(14)));
            }
            b"data" => layout.data = Some((body, end)),
            _ => {}
        }
        at = body + len as usize + (len as usize & 1);
    }
    Ok(layout)
}

pub fn parse(data: &[u8]) -> Result<Sound, WavError> {
    // **The file ends where the RIFF says, not where the buffer ends.** Zeebo F.C. Super League
    // builds each sound in a 500 KB scratch buffer and writes the size of the whole buffer into
    // the `data` chunk, but the real size (14 KB) into the `RIFF`. Following `data` played eleven
    // seconds: the effect and then memory garbage, loud, over the music. A `RIFF` that doesn't fit
    // a header — zero, or larger than the buffer, as from a streaming writer — limits nothing.
    //
    // It only applies when `data` goes **well** past the declared end: a file edited with one
    // more chunk and a stale `RIFF` is off by a few bytes, and cutting it would lose the end of
    // the sound.
    //
    // This is a snapshot read. When the game keeps writing past the `RIFF` while the sound plays
    // — Turma da Mônica's speech —, what extends the sound is playback, not the parser: see
    // [`open_pcm`].
    let layout = layout(data)?;
    let (Some((tag, channels, rate, align, bits)), Some((body, end))) =
        (layout.format, layout.data)
    else {
        return Err(WavError::Incomplete);
    };
    let riff_end = layout.riff_end;
    let end = match riff_end >= 44 && end > riff_end + RIFF_SLACK {
        true => riff_end.max(body),
        false => end,
    };
    let payload = &data[body..end];
    if tag == FORMAT_IMA_ADPCM {
        return Ok(Sound::create(
            rate.max(1),
            channels.max(1),
            decode_ima_adpcm(payload, channels.max(1) as usize, align as usize),
        ));
    }
    if tag != FORMAT_PCM {
        return Err(WavError::NotPcm(tag));
    }
    let samples = match bits {
        // 8-bit PCM is **unsigned**, with silence at 128; 16-bit is signed.
        8 => payload
            .iter()
            .map(|&b| (b as f32 - 128.0) / 128.0)
            .collect(),
        16 => payload
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
            .collect(),
        other => return Err(WavError::BadDepth(other)),
    };
    Ok(Sound::create(rate.max(1), channels.max(1), samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a WAVE with the given format and data bytes. The block alignment comes from the
    /// depth, as in a PCM.
    fn build(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        build_aligned(tag, channels, rate, bits, channels * bits / 8, data)
    }

    /// The same thing, with the block alignment given separately — which is what ADPCM needs,
    /// because there the block has no relation to the depth.
    fn build_aligned(
        tag: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        align: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());

        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        let total = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&total.to_le_bytes());
        out
    }

    /// The sound takes up the start of a larger buffer, `data` gives the buffer's size and `RIFF`
    /// gives the sound's: the `RIFF` wins, and the garbage after it doesn't play. It's the scratch
    /// buffer of Zeebo F.C. Super League.
    ///
    /// **Turma da Mônica's `sfx_bal_all.wav` does not confirm this rule**, although it has been
    /// cited as proof. Its 882,000-byte buffer is a template with that effect's header (`RIFF` of
    /// 56,352), and the speech lines pass through it, up to five seconds long, decoded while they
    /// play. The template's `RIFF` never changes — measured over thirty seconds of play. What
    /// extends the sound past it is playback: see [`open_pcm`].
    #[test]
    fn the_riff_caps_a_data_chunk_larger_than_the_sound() {
        assert_eq!(parse(&scratch_buffer()).unwrap().frames(), 2);
    }

    /// Two frames of sound, the `RIFF` saying so and `data` saying 10,000 bytes of buffer.
    fn scratch_buffer() -> Vec<u8> {
        let sound = build(FORMAT_PCM, 1, 22050, 16, &[0x10, 0x00, 0x20, 0x00]);
        let mut buffer = sound.clone();
        buffer.extend(std::iter::repeat(0x7f).take(9996));
        let data_at = sound.len() - 4 - 4;
        buffer[data_at..data_at + 4].copy_from_slice(&10_000u32.to_le_bytes());
        buffer
    }

    /// The buffer where the header alone doesn't decide is recognized, with the three positions.
    #[test]
    fn open_pcm_says_where_each_header_ends() {
        let buffer = scratch_buffer();
        let opened = open_pcm(&buffer).expect("the data goes past the RIFF");
        assert_eq!((opened.rate, opened.channels, opened.bits), (22050, 1, 16));
        assert_eq!(opened.start, 44);
        assert_eq!(opened.riff_end, 48);
        assert_eq!(opened.data_end, buffer.len());
    }

    /// A plain WAVE, and one with the `RIFF` only a little behind `data`, are not opened: the
    /// header is enough.
    #[test]
    fn a_plain_wave_is_not_opened() {
        let sound = build(FORMAT_PCM, 1, 22050, 16, &[0; 4000]);
        assert_eq!(open_pcm(&sound), None);
        let mut edited = sound.clone();
        edited[4..8].copy_from_slice(&((sound.len() - 8 - 100) as u32).to_le_bytes());
        assert_eq!(open_pcm(&edited), None);
        assert_eq!(open_pcm(b"RIFF\0\0\0\0WAVE"), None);
    }

    #[test]
    fn reads_16_bit_pcm() {
        let mut data = Vec::new();
        for value in [0i16, 16384, -16384, i16::MIN] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let sound = parse(&build(FORMAT_PCM, 1, 22050, 16, &data)).unwrap();
        assert_eq!(sound.rate, 22050);
        assert_eq!(sound.channels, 1);
        assert_eq!(sound.frames(), 4);
        assert_eq!(sound.samples[0], 0.0);
        assert_eq!(sound.samples[1], 0.5);
        assert_eq!(sound.samples[3], -1.0);
    }

    #[test]
    fn eight_bit_pcm_is_unsigned_with_silence_at_128() {
        // Reading 8 bits as if signed turns silence into clipping; it's the classic mistake with
        // this format.
        let sound = parse(&build(FORMAT_PCM, 1, 11025, 8, &[128, 255, 0, 192])).unwrap();
        assert_eq!(sound.samples[0], 0.0);
        assert_eq!(sound.samples[1], (255.0 - 128.0) / 128.0);
        assert_eq!(sound.samples[2], -1.0);
        assert_eq!(sound.samples[3], 0.5);
    }

    #[test]
    fn unknown_chunks_in_the_middle_are_skipped() {
        // Double Dragon sends a `bext` before the `fmt `; stopping at the first unfamiliar chunk
        // would lose the whole sound.
        let mut wave = build(FORMAT_PCM, 1, 8000, 8, &[128, 128]);
        let mut with_extra = wave[..12].to_vec();
        with_extra.extend_from_slice(b"bext");
        with_extra.extend_from_slice(&3u32.to_le_bytes());
        with_extra.extend_from_slice(&[1, 2, 3, 0]); // three bytes plus the padding
        with_extra.extend_from_slice(&wave[12..]);
        wave = with_extra;
        let sound = parse(&wave).unwrap();
        assert_eq!(sound.frames(), 2);
    }

    #[test]
    fn an_unknown_format_is_rejected_by_name() {
        // Rejecting while saying which format it is is what lets us know what to implement next:
        // that's how Pac-Mania's ADPCM showed up.
        let err = parse(&build(85, 1, 8000, 4, &[0, 1, 2, 3]));
        assert_eq!(err, Err(WavError::NotPcm(85)));
    }

    #[test]
    fn ima_adpcm_follows_the_step_table() {
        // Values checked by hand against the IMA specification. A block with the first sample at
        // zero and the index at zero, followed by nibbles 4 and 4:
        //   nibble 4: step 7, delta = 7>>3 + 7 = 7  -> sample 7,  index goes to 2
        //   nibble 4: step 9, delta = 9>>3 + 9 = 10 -> sample 17, index goes to 4
        // The nibbles come low one first, so both fit in byte 0x44.
        let mut data = vec![0u8, 0, 0, 0]; // predictor = 0, index = 0, reserved
        data.extend_from_slice(&[0x44, 0, 0, 0]);
        let sound = parse(&build_aligned(FORMAT_IMA_ADPCM, 1, 8000, 4, 8, &data)).unwrap();

        assert_eq!(sound.samples[0], 0.0, "the header sample comes first");
        assert_eq!(sound.samples[1], 7.0 / 32768.0);
        assert_eq!(sound.samples[2], 17.0 / 32768.0);
    }

    #[test]
    fn adpcm_with_a_block_larger_than_the_data_does_not_overflow() {
        // A block announced as 1024 bytes with a handful of real data has to come out short,
        // not bring down the emulator.
        let sound = parse(&build_aligned(
            FORMAT_IMA_ADPCM,
            1,
            8000,
            4,
            1024,
            &[0, 0, 0, 0, 0x11],
        ))
        .unwrap();
        assert!(!sound.samples.is_empty());
    }

    #[test]
    fn what_is_not_wave_is_rejected() {
        assert_eq!(parse(b"nao sou um wave"), Err(WavError::NotWave));
        assert_eq!(parse(&[]), Err(WavError::NotWave));
    }

    #[test]
    fn a_wave_without_data_does_not_become_sound() {
        let mut wave = build(FORMAT_PCM, 1, 8000, 16, &[]);
        // Cut the `data` chunk off.
        wave.truncate(wave.len() - 8);
        assert_eq!(parse(&wave), Err(WavError::Incomplete));
    }
}
