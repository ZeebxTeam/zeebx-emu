//! MP3: the games' music, decoded — and timed when it can't be decoded.
//!
//! **Zeebo music is MP3.** Nine games ship it that way: Quake, Quake 2, Galaxy on Fire,
//! Rally Master Pro, Powerboat Challenge, Action Hero 3D, Need For Speed, zeetris and the
//! Z-Wheel. Sound effects are RIFF/WAVE and always played; music played in no game at all, and
//! this was the reason — the whole format was missing.
//!
//! Two things live here, and it helps to know why:
//!
//! - [`decode`] returns the sound. Layer III is Huffman, requantization, joint stereo, IMDCT and
//!   a synthesis filterbank; writing that without a reference to compare against gives sound
//!   that plays and comes out at the wrong pitch. Decoding is done by `symphonia`, pure Rust —
//!   the same decision as SQLite for `ISQLMgr` and `ab_glyph` for `DrawText`.
//! - [`probe`] reads only the header and the encoder tag, and says **how long it lasts**. It
//!   still matters: an MP3 the decoder rejects can still be timed, and timing is what keeps the
//!   game from hanging.
//!
//! The second path was born from Tekken 2. Its music arrives through `IMEDIA_SetMediaParm` and
//! we could only read RIFF/WAVE. With no readable sound, `Play` answered "this sound has already
//! ended", the game polled the state, saw "ready" and asked to play again — **766 thousand times
//! in four virtual seconds**. It wasn't a heavy game: it was a game pushing back against an
//! answer of ours. With the duration, the sound "plays" in silence for the right amount of
//! virtual clock time and the game moves on. Today Tekken really plays, and the duration path
//! stayed as a safety net.

/// An MP3 we know enough about to time it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp3 {
    /// Samples per second.
    pub rate: u32,
    /// How many audio frames the file has.
    pub frames: u64,
    /// Samples per frame: 1152 in MPEG-1 and 576 in MPEG-2 and MPEG-2.5.
    pub samples_per_frame: u32,
}

impl Mp3 {
    /// The duration in microseconds.
    pub fn duration_us(&self) -> u64 {
        match self.rate {
            0 => 0,
            rate => self.frames * u64::from(self.samples_per_frame) * 1_000_000 / u64::from(rate),
        }
    }
}

/// Layer III bitrate table, in kbps, indexed by the header field.
///
/// The MPEG-1 row and the MPEG-2 row differ. Index 0 (free) and 15 (invalid) have no rate, and
/// go in as zero: the reader treats zero as "can't estimate from the size".
const BITRATE_V1: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BITRATE_V2: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

/// Sample rates per version, in header field order.
const RATES_V1: [u32; 3] = [44100, 48000, 32000];
const RATES_V2: [u32; 3] = [22050, 24000, 16000];
const RATES_V25: [u32; 3] = [11025, 12000, 8000];

/// How many bytes of data the `ID3v2` at the front of the file takes up, if there is one.
///
/// The size comes in four bytes of seven bits each — the format stores it that way so it never
/// forms a `0xFF` that looks like a frame sync.
fn id3_len(data: &[u8]) -> usize {
    if data.len() < 10 || &data[..3] != b"ID3" {
        return 0;
    }
    let size = data[6..10]
        .iter()
        .fold(0usize, |acc, byte| (acc << 7) | (*byte as usize & 0x7f));
    10 + size
}

/// Reads what can be known about an MP3 without decoding it. `None` when it isn't one.
pub fn probe(data: &[u8]) -> Option<Mp3> {
    let start = id3_len(data);
    let head = data.get(start..start + 4)?;
    // Sync: eleven bits set.
    if head[0] != 0xff || head[1] & 0xe0 != 0xe0 {
        return None;
    }
    // Only Layer III matters: it's what "MP3" means, and it's what the games ship.
    if (head[1] >> 1) & 0b11 != 0b01 {
        return None;
    }
    let (rates, bitrates, samples_per_frame) = match (head[1] >> 3) & 0b11 {
        0b11 => (RATES_V1, BITRATE_V1, 1152),
        0b10 => (RATES_V2, BITRATE_V2, 576),
        0b00 => (RATES_V25, BITRATE_V2, 576),
        // 0b01 is reserved.
        _ => return None,
    };
    let rate = *rates.get(((head[2] >> 2) & 0b11) as usize)?;
    let bitrate = bitrates[((head[2] >> 4) & 0b1111) as usize] * 1000;
    let mono = (head[3] >> 6) & 0b11 == 0b11;

    // The encoder tag lives in the first frame, at an offset that depends on the version and
    // the channel count — it's where the side info would be, which the tag frame doesn't use.
    // When it carries the frame count, the duration is exact even with a variable bitrate.
    let tag_at = start
        + 4
        + match (samples_per_frame, mono) {
            (1152, true) => 17,
            (1152, false) => 32,
            (_, true) => 9,
            (_, false) => 17,
        };
    let tag = data
        .get(tag_at..tag_at + 12)
        .filter(|tag| (&tag[..4] == b"Xing" || &tag[..4] == b"Info") && tag[7] & 1 == 1);
    if let Some(tag) = tag {
        let frames = u32::from_be_bytes([tag[8], tag[9], tag[10], tag[11]]);
        return Some(Mp3 {
            rate,
            frames: u64::from(frames),
            samples_per_frame,
        });
    }

    // Without a tag, the math is the constant-bitrate one: the size divided by the bitrate. It
    // is wrong for variable-bitrate files, and it's the only estimate possible without scanning
    // every frame.
    if bitrate == 0 {
        return None;
    }
    let bytes = (data.len() - start) as u64;
    let seconds_by_bits = bytes * 8 * u64::from(rate) / u64::from(bitrate);
    Some(Mp3 {
        rate,
        frames: seconds_by_bits / u64::from(samples_per_frame),
        samples_per_frame,
    })
}

/// Decodes the whole MP3 into samples. `None` when it isn't an MP3 that can be read.
///
/// Returns the same [`Sound`](crate::audio::wav::Sound) that RIFF/WAVE produces, so the mixer
/// doesn't need to know where the sound came from: resampling, volume and looping already work
/// the same.
///
/// Decoding is done **all at once**, not streamed. A thirty-six-second track at 22 kHz mono is
/// three million samples, six megabytes of `f32` — it fits, and the result stays in the sound
/// cache in `machine.rs`, decoded once per track and not per `Play`. Streaming would be right
/// for a game that changes music all the time; none of ours does that, and streaming would add
/// state and one more thread for nothing.
pub fn decode(data: &[u8]) -> Option<crate::audio::wav::Sound> {
    decode_detailed(data).ok()
}

/// Like [`decode`], but **saying why** when it refuses.
///
/// The reason matters: without it the report showed only the WAV error ("not a RIFF/WAVE"),
/// which is true and doesn't help at all — the question is what `symphonia`'s probe didn't
/// accept. That's how we found out that Turma da Mônica ships Ogg and Peggle ships MP3 with an
/// ID3v2 tag.
/// Where the first MPEG frame starts.
///
/// The ID3v2 tag declares **its own size** in the header, and that's what a decoder uses to skip
/// to the audio. Peggle ships MP3s whose tag says zero bytes and has thirty thousand of content:
/// whoever trusts the field looks for the frame in the wrong place, doesn't find it, and rejects
/// the whole sound — which is what happened to seven of the game's tracks. Here the search is for
/// the frame itself, `0xFF` followed by three high bits, which is what the format really
/// guarantees.
fn audio_start(bytes: &[u8]) -> usize {
    if bytes.len() < 3 || &bytes[0..3] != b"ID3" {
        return 0;
    }
    for i in 3..bytes.len().saturating_sub(1) {
        if bytes[i] == 0xFF && (bytes[i + 1] & 0xE0) == 0xE0 {
            return i;
        }
    }
    0
}

pub fn decode_detailed(data: &[u8]) -> Result<crate::audio::wav::Sound, String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // `MediaSourceStream` wants ownership of what it reads, and the buffer belongs to the guest:
    // copying is the price of not leaving the decoder looking at the game's memory while the
    // game rewrites it.
    // **Two containers, the same decoder.** Most games' music arrives as MP3; Turma da Mônica's
    // arrives as Ogg/Vorbis — 86 sounds that were rejected one by one and left the game in
    // **absolute silence** (measured: peak 0.000 in the report, with eight thousand Ogg samples
    // in the rejected list). The extension hint helps the probe and doesn't get in the way: with
    // the wrong one it fails, and then it's worth trying the other. Before this line, the
    // report's `rejected (unknown format, 4f 67 67 53 ...)` was the only clue, and it stayed in
    // plain sight for an afternoon.
    // **The frame search comes before the probe.** An ID3 tag that lies about its size makes the
    // probe fail on the whole file; starting at the first frame, it sees what the format
    // promises. For a file with no tag, or with an honest tag, the offset is zero and nothing
    // changes.
    let start = audio_start(data);
    let audio = &data[start..];
    let mut probed = None;
    for extension in ["mp3", "ogg"] {
        let source = std::io::Cursor::new(audio.to_vec());
        let stream = MediaSourceStream::new(Box::new(source), Default::default());
        let mut hint = Hint::new();
        hint.with_extension(extension);
        if let Ok(found) = symphonia::default::get_probe().format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        ) {
            probed = Some(found);
            break;
        }
    }
    let probed = probed.ok_or_else(|| "the probe did not recognize the container".to_string())?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "the container declares no audio track".to_string())?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| format!("no decoder for {:#?}: {error}", track.codec_params.codec))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut rate = 0;
    let mut channels = 0;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let package = match format.next_packet() {
            Ok(package) => package,
            // End of file arrives as a read error, and it's the normal outcome.
            Err(_) => break,
        };
        if package.track_id() != id {
            continue;
        }
        let frame = match decoder.decode(&package) {
            Ok(frame) => frame,
            // A broken frame in the middle doesn't invalidate the music: the period encoder
            // leaves garbage at the start of some files, and stopping there would return silence.
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *frame.spec();
        rate = spec.rate;
        channels = spec.channels.count() as u16;
        let buffer =
            buffer.get_or_insert_with(|| SampleBuffer::new(frame.capacity() as u64, spec));
        buffer.copy_interleaved_ref(frame);
        samples.extend_from_slice(buffer.samples());
    }
    if samples.is_empty() || rate == 0 || channels == 0 {
        return Err(format!(
            "decoded {} sample(s), rate {rate}, {channels} channel(s)",
            samples.len()
        ));
    }
    Ok(crate::audio::wav::Sound::create(rate, channels, samples))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The tag that lies about its size.**
    ///
    /// `ID3` + version + flags + four size bytes. Peggle ships a tag saying zero bytes; looking
    /// for the frame by the declared field led to no decoder at all. Searching for the frame
    /// pattern finds the audio either way.
    #[test]
    fn finds_the_frame_after_a_lying_tag() {
        let mut data = b"ID3\x03\x00\x00\x00\x00\x00\x00".to_vec();
        data.extend_from_slice(&[0u8; 32]); // tag content, with no frame at all
        let location = data.len();
        data.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00]); // MPEG-1 Layer III frame sync
        assert_eq!(audio_start(&data), location);
    }

    /// Without a tag, the audio starts at the start — and a stray `0xFF` doesn't become a false
    /// positive on the first byte, which is the case for every WAV and every Ogg.
    #[test]
    fn without_a_tag_the_audio_starts_at_zero() {
        assert_eq!(audio_start(&[0xFF, 0xFB, 0x90, 0x00]), 0);
        assert_eq!(audio_start(b"OggS\x00\x02\x00\x00"), 0);
        assert_eq!(audio_start(b"RIFF\x24\x00\x00\x00WAVE"), 0);
        assert_eq!(audio_start(b""), 0);
    }

    /// A quarter second of 440 Hz, mono, 22,050 Hz, generated with `lame`.
    ///
    /// A real fixture instead of hand-assembled bytes: what we want to prove is that a real MP3
    /// goes in and comes out at the right pitch, and no synthetic header proves that. It only
    /// goes into the test binary.
    const TONE: &[u8] = include_bytes!("../../assets/teste/tom-440hz.mp3");

    /// The music comes out with the right rate, channels and **pitch**.
    ///
    /// The pitch is checked by counting zero crossings: a sound that plays, looks fine and is at
    /// the wrong pitch would pass any other check — it's the same care the mixer already takes
    /// with resampling.
    #[test]
    fn a_real_mp3_decodes_at_the_right_pitch() {
        let sound = decode(TONE).expect("the 440 Hz tone is a readable MP3");
        assert_eq!((sound.rate, sound.channels), (22050, 1));
        // The encoder adds silence at the start and at the end, so the duration isn't exact.
        let seconds = sound.frames() as f32 / sound.rate as f32;
        assert!((0.2..0.4).contains(&seconds), "{seconds} s");
        let peak = sound.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.3, "sound almost mute: peak {peak}");
        // Only the middle: the edges have the encoder's silence, and zero doesn't cross zero.
        let middle = &sound.samples[sound.samples.len() / 4..sound.samples.len() * 3 / 4];
        let crossings = middle
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        let hz = crossings as f32 / 2.0 / (middle.len() as f32 / sound.rate as f32);
        assert!((430.0..450.0).contains(&hz), "measured pitch: {hz} Hz");
    }

    /// What isn't MP3 doesn't become sound — and doesn't panic.
    #[test]
    fn what_is_not_mp3_does_not_decode() {
        assert!(decode(b"RIFF\x00\x00\x00\x00WAVEfmt ").is_none());
        assert!(decode(&[]).is_none());
        assert!(decode(&[0xff; 512]).is_none());
    }

    /// The real header of the first frame of Tekken 2's music, with the `Info` tag its encoder
    /// left: MPEG-2, Layer III, 64 kbps, 22,050 Hz, mono.
    fn tekken_header(frames: u32) -> Vec<u8> {
        let mut out = vec![0xff, 0xf3, 0x80, 0xc4];
        out.extend(std::iter::repeat_n(0u8, 9));
        out.extend(b"Info");
        out.extend([0, 0, 0, 1]); // flags: only the frame count
        out.extend(frames.to_be_bytes());
        out.resize(140642, 0);
        out
    }

    #[test]
    fn the_encoder_tag_gives_the_exact_duration() {
        let mp3 = probe(&tekken_header(1350)).expect("it is an MP3");
        assert_eq!((mp3.rate, mp3.samples_per_frame), (22050, 576));
        assert_eq!(mp3.frames, 1350);
        // 1350 frames of 576 samples at 22,050 Hz are a little under 36 seconds.
        assert_eq!(mp3.duration_us() / 1000, 35265);
    }

    #[test]
    fn without_a_tag_the_duration_comes_from_the_bitrate() {
        let mut data = tekken_header(1350);
        // Break the tag: what's left is the size-by-bitrate math.
        data[13..17].copy_from_slice(b"xxxx");
        let mp3 = probe(&data).expect("it is an MP3");
        // 140,642 bytes at 64 kbps are 17.5 seconds.
        assert_eq!(mp3.duration_us() / 1_000_000, 17);
    }

    #[test]
    fn a_leading_id3_does_not_get_in_the_way() {
        let mut data = b"ID3\x04\x00\x00\x00\x00\x02\x01".to_vec();
        data.resize(10 + 257, 0);
        data.extend(tekken_header(100));
        let mp3 = probe(&data).expect("it is an MP3");
        assert_eq!(mp3.frames, 100);
    }

    #[test]
    fn a_riff_is_not_mp3() {
        assert_eq!(probe(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
        assert_eq!(probe(&[]), None);
        // Right sync, wrong layer: Layer II is not MP3.
        assert_eq!(probe(&[0xff, 0xf5, 0x80, 0xc4, 0, 0, 0, 0]), None);
    }
}
