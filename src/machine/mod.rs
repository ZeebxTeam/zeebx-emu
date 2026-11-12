//! The execution loop: runs the module, serves the API calls and carries on.
//!
//! When the guest calls a method, the core stops with [`StopReason::ApiCall`]. Here we decide
//! what that call means, write the return value to `r0` and resume execution at `lr` — which is
//! where the original `bl`/`blx` left the return address.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::brew::aee::{self, Interface};
use crate::brew::aee_helpers;
use crate::brew::cformat::{self, ArgSource};
use crate::brew::crypto;
use crate::brew::fmath;
use crate::brew::heap::Heap;
use crate::brew::objects::ObjectStore;
use crate::brew::vfs::Vfs;
use crate::cpu::RETURN_MAGIC;
use crate::cpu::{CpuBackend, CpuError, Reg, StopReason};
use crate::input::{self, Pad};
use crate::loader::{self, LoadedModule};
use crate::bridge;
use crate::net as host_net;
use crate::video::atc;
use crate::video::display::{Framebuffer, Rect, Rgb};
use crate::video::gles;
use crate::video::paltex;
use crate::video::rasterizer::{self, GlState, Rasterizer, Vertex};

mod save;
mod bitmap;
mod cipher;
mod diagnostics;
mod display;
mod misc;
mod egl;
mod file;
mod font;
mod gl;
mod guest_dib;
mod helper;
mod hid;
mod image;
mod media;

/// The cap on the decoded-sound cache, in bytes — re-exported because the one who tunes it is the
/// frontend, and the module that holds it is internal.
pub use media::set_sound_cache_cap;
mod net;
mod probe;
mod shell;
mod signal;
mod sound;
mod sql;
mod thread;
mod time;
mod widget;
mod zip;

/// Cap on an SQL statement coming from the guest. The Z-Wheel's largest are 110 bytes.
const MAX_SQL: usize = 4096;

/// Cap on reading a string from the guest, against a corrupted pointer.
///
/// **One MiB, not four KB.** The cap only protects against a pointer with no terminator: the read
/// goes in blocks up to the zero, and a short string costs the same under any cap. But the game
/// has no cap at all, and with four KB `strlen` lied. The levels of Alice in Wonderland are Lua
/// code with the tile map in a single-line base64 string — 15,426 characters in level 2, and
/// 202,806 in `stage1-30` —, and the game decoded only the first 4,096. The tiles come in row
/// order: everything below row 29 of level 2 vanished, drawing and collision. That was the black
/// strip at the bottom of the screen, and the gap room with no floor, where Alice fell forever.
const MAX_STRING: usize = 1 << 20;

/// Cap for the number conversions (`strtoul`, `atoi`, `strtod`).
///
/// They read from a pointer that usually points into the middle of a large text, and every byte
/// past the number is read for nothing. No decimal, hexadecimal or floating-point representation
/// that fits in 32 or 64 bits comes close to this, counting leading whitespace and the sign.
const MAX_NUMBER: usize = 64;
/// `AEE_ENC_ISOLATIN1`, from `AEEShell.h`. It is the encoding BREW uses for `char *` on most
/// handsets; the Zeebo is Brazilian and uses accented text, so it is the right bet until some
/// game shows otherwise.
const AEE_ENC_ISOLATIN1: u16 = 3;

/// How many color items `AEEClrItem` defines, plus index zero, which is unused.
const CLR_COUNT: usize = 17;
/// `RGB_NONE`: "use the current color", not "don't paint".
///
/// The difference decides whether the screen gets cleared: the SDK's `IDISPLAY_ClearScreen` is
/// written as `DrawRect(NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)`, that is, fill the whole surface
/// with the current background color. While `RGB_NONE` meant "don't paint", that call did nothing
/// — and Tekken 2, which clears the screen once per frame, kept the previous screen's text under
/// the menu.
const RGB_NONE: u32 = 0xffff_ffff;

/// `CLR_USER_BACKGROUND`, the color item the fill uses when `RGB_NONE` comes in.
const CLR_USER_BACKGROUND: usize = 2;

/// `CLR_USER_LINE`, the color item of the frame.
const CLR_USER_LINE: usize = 3;

/// `IDF_RECT_FRAME` and `IDF_RECT_FILL`, from `AEEDisp.h`: what the call wants drawn.
const IDF_RECT_FRAME: u32 = 1;
const IDF_RECT_FILL: u32 = 2;
/// Values of the `AEERasterOp` enum, from `inc/AEERasterOp.h`: `OR`, `XOR`, `COPY`, `NOT`,
/// `OLDMASK`, `MERGENOT`, `ANDNOT`, `TRANSPARENT`, `AND`, `BLEND`.
const AEE_RO_XOR: u32 = 1;
#[cfg_attr(not(test), allow(dead_code))]
const AEE_RO_COPY: u32 = 2;
const AEE_RO_TRANSPARENT: u32 = 7;

/// Initial colors: black text on a white background, like a standard BREW handset.
fn default_colors() -> [Rgb; CLR_COUNT] {
    let mut colors = [Rgb::WHITE; CLR_COUNT];
    // CLR_USER_TEXT = 1, CLR_USER_LINE = 3.
    colors[1] = Rgb::BLACK;
    colors[3] = Rgb::BLACK;
    colors
}

/// Translates a comparison into the integer the `str*cmp` functions return.
fn cmp_to_int(ordering: std::cmp::Ordering) -> u32 {
    match ordering {
        std::cmp::Ordering::Less => (-1i32) as u32,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Reads the longest prefix of `text` that forms a number, like C's `strtod`.
///
/// Returns the value and how many bytes were consumed — `strtod` needs the second to fill in
/// `char **ppszEnd`.
fn parse_leading_double(text: &str) -> (f64, usize) {
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut best = None;
    // Leading whitespace is skipped, and from there on the longest prefix Rust's parser accepts
    // wins: it is the same result as `strtod`, without reimplementing the grammar.
    let start = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    for cut in start + 1..=bytes.len() {
        if let Ok(value) = text[start..cut].parse::<f64>() {
            best = Some(value);
            end = cut;
        }
    }
    match best {
        Some(value) => (value, end),
        None => (0.0, 0),
    }
}

/// Looks for `needle` inside `hay` and returns the index where it starts.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Lowers a wide string to ASCII lowercase, truncated at `take` characters.
///
/// It is what the `wstricmp`/`wstrnicmp` comparisons need: BREW only folds the case of ASCII.
fn fold_case(units: &[u16], take: usize) -> Vec<u16> {
    units
        .iter()
        .take(take)
        .map(|&u| match u8::try_from(u) {
            Ok(b) => b.to_ascii_lowercase() as u16,
            Err(_) => u,
        })
        .collect()
}

/// Reads the longest prefix of `text` that forms an unsigned integer in the given base.
///
/// Base 0 means deducing it from the prefix, as in C's `strtoul`: `0x` is hexadecimal, `0` is
/// octal and the rest is decimal. Returns the value and how many bytes were consumed.
fn parse_unsigned(text: &str, base: u32) -> (u32, usize) {
    // C's `isspace` whitespace, not Unicode's: read as Latin-1, `0xA0` would become a space.
    let start = text.len()
        - text
            .trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
            .len();
    // **The sign counts.** C's `strtoul` accepts `+` and `-`, and with `-` it returns the negated
    // number. Alice in Wonderland reads lines like `-2 12 16 73 ...`: stopped at the `-`, its loop
    // counted the field without moving and repeated the line until it exhausted the 18 MB pool.
    let (negative, after_sign) = match text[start..].as_bytes().first() {
        Some(b'-') => (true, start + 1),
        Some(b'+') => (false, start + 1),
        _ => (false, start),
    };
    let rest = &text[after_sign..];
    let hex = rest.starts_with("0x") || rest.starts_with("0X");
    let (digits, base) = match base {
        0 | 16 if hex => (&rest[2..], 16),
        // The `0` of the octal prefix is also a digit: `"0 1"` consumes the zero.
        0 if rest.starts_with('0') => (rest, 8),
        0 => (rest, 10),
        base => (rest, base),
    };
    let taken = digits.chars().take_while(|c| c.is_digit(base)).count();
    let value = u32::from_str_radix(&digits[..taken], base).unwrap_or(u32::MAX);
    let consumed = if taken == 0 {
        0
    } else {
        text.len() - digits.len() + taken
    };
    let value = match (taken, negative) {
        (0, _) => 0,
        (_, true) => value.wrapping_neg(),
        (_, false) => value,
    };
    (value, consumed)
}

/// Size in bytes of a vector component.
fn component_size(kind: u32) -> u32 {
    match kind {
        gles::GL_BYTE | gles::GL_UNSIGNED_BYTE => 1,
        gles::GL_SHORT | gles::GL_UNSIGNED_SHORT => 2,
        _ => 4,
    }
}

/// Rounds `value` up to the next multiple of `alignment`.
///
/// It is the arithmetic `glPixelStorei` calls for: each row of a texture starts at a multiple of
/// the alignment, and whatever is left between the end of one row and the start of the next is
/// padding. GL only accepts 1, 2, 4 and 8, and the default is 4.
pub(super) fn round_up_to(value: u32, alignment: u32) -> u32 {
    let step = alignment.max(1);
    value.div_ceil(step) * step
}

/// Size in bytes of a texel, according to the format/type pair of `TexImage2D`.
fn bytes_per_texel(format: u32, kind: u32) -> u32 {
    match kind {
        gles::GL_UNSIGNED_BYTE => match format {
            gles::GL_RGB => 3,
            gles::GL_RGBA => 4,
            gles::GL_LUMINANCE_ALPHA => 2,
            _ => 1,
        },
        // The three packed types of OpenGL ES are all 16-bit.
        _ => 2,
    }
}

/// Converts the texels to 8-bit RGBA, the rasterizer's only format.
fn decode_texels(bytes: &[u8], format: u32, kind: u32, count: usize) -> Vec<[u8; 4]> {
    // Repeating the five high bits in the three low ones spreads the value across the whole
    // range: it is what makes 0b11111 become 255 and not 248.
    let expand = |value: u16, bits: u32| -> u8 {
        let max = (1u16 << bits) - 1;
        ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
    };
    let size = bytes_per_texel(format, kind) as usize;
    (0..count)
        .map(|i| {
            let at = i * size;
            if at + size > bytes.len() {
                return [255; 4];
            }
            match kind {
                gles::GL_UNSIGNED_SHORT_5_6_5 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 11, 5),
                        expand((v >> 5) & 0x3f, 6),
                        expand(v & 0x1f, 5),
                        255,
                    ]
                }
                gles::GL_UNSIGNED_SHORT_4_4_4_4 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 12, 4),
                        expand((v >> 8) & 0xf, 4),
                        expand((v >> 4) & 0xf, 4),
                        expand(v & 0xf, 4),
                    ]
                }
                gles::GL_UNSIGNED_SHORT_5_5_5_1 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 11, 5),
                        expand((v >> 6) & 0x1f, 5),
                        expand((v >> 1) & 0x1f, 5),
                        if v & 1 != 0 { 255 } else { 0 },
                    ]
                }
                _ => match format {
                    gles::GL_RGB => [bytes[at], bytes[at + 1], bytes[at + 2], 255],
                    gles::GL_RGBA => [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]],
                    gles::GL_LUMINANCE_ALPHA => {
                        let l = bytes[at];
                        [l, l, l, bytes[at + 1]]
                    }
                    gles::GL_ALPHA => [255, 255, 255, bytes[at]],
                    gles::GL_LUMINANCE => [bytes[at], bytes[at], bytes[at], 255],
                    // A format we don't know: a single channel, opaque. It comes out gray, and
                    // gray is visible — getting it wrong this way shows on screen instead of
                    // becoming a black hole.
                    _ => [bytes[at], bytes[at], bytes[at], 255],
                },
            }
        })
        .collect()
}

/// The class that knows how to open a MIME type, for `ISHELL_GetHandler`.
///
/// On the console this comes from the BREW registry, built from the installed `.mif` files. Here
/// the table is fixed and covers what the games ask for; zero means "nobody knows how to open
/// this", which is the answer the documentation provides for.
fn handler_for(mime: &str) -> u32 {
    match mime {
        "image/png" => AEECLSID_PNG,
        "image/jpeg" => AEECLSID_JPEG,
        "image/gif" => AEECLSID_GIF,
        "image/bmp" | "image/x-ms-bmp" => AEECLSID_WINBMP,
        "audio/mid" | "audio/midi" => AEECLSID_MEDIAMIDI,
        "audio/mpeg" | "audio/mp3" => AEECLSID_MEDIAMP3,
        // **WAV is `+ 0xa`, not `0x5511`.** Bejeweled Twist checks the answer: it only creates
        // the media if the class is `+1`, `+2` or `+0xa` (0x67bdc), and its own table says that
        // type 3, the one for the 56 PCM WAV effects in `resources.dat`, is `0x0100550a`
        // (0x1435c). With `0x5511` it skipped them all: the sound manager, which indexes by
        // `id - 3000`, was left with only the 10 songs, and the first effect played in a match
        // read past the end of the list — the game stopped at 0x1c714, reading 0xff53906e.
        "audio/wav" | "audio/x-wav" => AEECLSID_MEDIAADPCM,
        "audio/vnd.qcelp" => AEECLSID_MEDIAADPCM,
        _ => 0,
    }
}

/// Figures out the MIME type of some content, from the first bytes or from the name's extension.
///
/// It is what `ISHELL_DetectType` answers. Signature recognition comes first because that is
/// what the documentation says: "pBuf takes precedence over pszName".
fn detect_mime(bytes: &[u8], name: &str) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if starts(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"BM") {
        return Some("image/bmp");
    }
    if starts(b"MThd") {
        return Some("audio/mid");
    }
    if starts(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0) {
        return Some("audio/mpeg");
    }
    if starts(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    if starts(b"#!AMR") {
        return Some("audio/amr");
    }

    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "mid" | "midi" => "audio/mid",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "amr" => "audio/amr",
        "qcp" => "audio/vnd.qcelp",
        "txt" => "text/plain",
        _ => return None,
    })
}

/// Packs a color into the `RGBVAL` the guest expects back.
fn to_rgbval(color: Rgb) -> u32 {
    (color.r as u32) << 8 | (color.g as u32) << 16 | (color.b as u32) << 24
}

/// Gamepad identifiers, from the USB descriptor captured from the console
/// (`docs/vendor/tripleoxygen/hardware/peripheral/joystick_descriptor.txt`): "My Power / Usb
/// Game Pad".
const GAMEPAD_VENDOR_ID: u16 = 0x1eaa;
const GAMEPAD_PRODUCT_ID: u16 = 0x0135;
/// The Z-Pad: the "Zeebo Game Controller" entry of `hid_devices.cfg`.
const ZPAD_VENDOR_ID: u16 = 0x1a5c;
const ZPAD_PRODUCT_ID: u16 = 0x3033;
/// How often the Boomerang receiver sends a report: 100 per second, alternating between the two
/// players.
const BOOMERANG_PERIOD_US: u64 = 10_000;
/// The Boomerang receiver: the "Zeebo Accelerometer Controller" entry of `hid_devices.cfg`.
/// The Boomerang Sports games only treat the device as a Boomerang with this pair.
const BOOMERANG_VENDOR_ID: u16 = 0x15a2;
const BOOMERANG_PRODUCT_ID: u16 = 0x0003;
/// Identifier of a port in the enumeration: `1` and `2`, in port order.
///
/// `CreateDevice` gets this number back, and it is how we know which port the device the game
/// has just created will read from.
const fn port_handle(port: usize) -> u32 {
    port as u32 + 1
}
const HID_STATUS_CONNECTED: u32 = 1;
/// `AEEUID_HID_Joystick_Device`, from `AEEHIDDevice_Joystick.h`: the device type the games ask
/// for in `GetConnectedDevices`.
const UID_JOYSTICK_DEVICE: u32 = 0x0106_c3fd;
/// The keyboard type, one off from the joystick.
///
/// It did not come from a header: it came from the Z-Wheel. It calls `GetConnectedDevices`
/// **twice**, once asking for `0x0106c3fd` and once asking for this; when the second comes back
/// empty, `Joystick.c:183` prints `No keyboard reported`. In other words, the console enumerates
/// USB keyboards, and the message we saw in the log was the right answer to "none plugged in".
const UID_KEYBOARD_DEVICE: u32 = 0x0106_c3fc;
/// BREW's `EBADPARM`.
const EBADPARM: u32 = 2;
/// Consecutive clock reads, with no other work in between, after which the game is considered
/// to be busy-waiting. The threshold lets through for free the reads that are part of a frame's
/// normal work.
const SPIN_THRESHOLD: u32 = 64;

/// How far the clock advances on each read, once the wait has been recognized. It is also how far
/// it may overshoot the deadline the game is waiting for.
const SPIN_STEP_US: u64 = 250;

/// `AEE_EUNSUPPORTED`, from `AEEStdErr.h`: the API exists, but not for this item.
const EUNSUPPORTED: u32 = 20;
/// `AEE_EBUFFERTOOSMALL`, from `AEEStdErr.h`.
const EBUFFERTOOSMALL: u32 = 38;
/// `AEE_DEVICEITEM_IMEI`, from `AEEDeviceItems.h`: the device identifier in ASCII.
const DEVICEITEM_IMEI: u32 = 28;
/// The IMEI we answer with. It is synthetic — we don't know any Zeebo's —, but it has the right
/// 15 digits and the right Luhn check digit, because whoever asks for an IMEI usually checks it.
const IMEI: &[u8] = b"350000000000006\0";
/// `IFILEMGR_OpenFile` modes, from `AEEFile.h`.
const OFM_READWRITE: u32 = 0x0002;
const OFM_CREATE: u32 = 0x0004;
const OFM_APPEND: u32 = 0x0008;
/// `IFILE_Seek` types, from `AEEFile.h`.
const SEEK_END: u32 = 1;
const SEEK_CURRENT: u32 = 2;
/// File attributes.
const FA_NORMAL: u32 = 0x00;
const FA_DIR: u32 = 0x02;
/// BREW's `FALSE` — its `boolean` is a `uint8`.
const FALSE: u32 = 0;
/// `AEECLSID_PNG`, from `sdk/inc/AEEPNG.bid` — the PNG decoder, exposed as `IImage`.
const AEECLSID_PNG: u32 = 0x0100_4004;
/// `AEECLSID_BMP` = `0x01004001`, the same family as one at `0x01004000`.
///
/// Action Hero 3D asks for this one, and the identification is not a guess: its resources are 67
/// BMPs in a `.res` whose first entry name is `66.bmp`, and it feeds the object an `IMemAStream`
/// just like the one Bejeweled Twist uses with `0x01004004`. Both are `IImage`.
const AEECLSID_BMP: u32 = 0x0100_4001;
/// `AEECLSID_PNGDecoder` and `AEECLSID_PNGDecoderBREW`, from `inc/AEEPNGDecoder*.bid`. The
/// default interface of both is `IImageDecoder`.
const AEECLSID_PNGDECODER: u32 = 0x0102_6e23;
const AEECLSID_PNGDECODER_BREW: u32 = 0x0103_0766;
/// `AEECLSID_JPEGDECODER_BREW` (`AEECLSID_JPEGDecoderBREW` in the toolset's ClassID table).
///
/// **It is what Zuma's Revenge was missing**: it asks for this class, got refused, carried on
/// with the null pointer and crashed — the scan caught it under "crashed in the frame loop" with
/// the reason `invalid access at 0x0`. The decoder it wants is the same one that already serves
/// PNG, BMP, JPEG and GIF: ours looks at the byte signature and picks the format by itself.
const AEECLSID_JPEGDECODER_BREW: u32 = 0x0102_fd92;
/// `IPARM_*` from `inc/AEEIImage.h`.
///
/// `SIZE`, `OFFSET` and `ROP` came out of actual use: Action Hero 3D writes each letter of the
/// menu with `SetParm(0, cx, 12)`, `SetParm(1, x, 0|12|24)` and `Draw` over the 201x37 font sheet
/// — three rows of 12 pixels —, and prepares the image with `SetParm(3, AEE_RO_TRANSPARENT)`.
const IPARM_SIZE: u32 = 0;
const IPARM_OFFSET: u32 = 1;
const IPARM_CXFRAME: u32 = 2;
const IPARM_ROP: u32 = 3;
const IPARM_NFRAMES: u32 = 4;
const IPARM_GETBITMAP: u32 = 10;
/// `AEECLSID_MEMASTREAM` = `AEECLSID_CORE + 12`, from `sdk/inc/AEEClassIDs.h`.
const AEECLSID_MEMASTREAM: u32 = 0x0100_100c;
/// `AEECLSID_LICENSE` = `AEECLSID_CORE + 15`, from `sdk/inc/AEEClassIDs.h`.
const AEECLSID_LICENSE: u32 = 0x0100_100f;
/// `LT_NONE` and `PT_PURCHASE`, from `inc/AEELicenseInfo.h`: a module with no expiry, purchased.
const LT_NONE: u32 = 0;
const PT_PURCHASE: u32 = 2;
/// `BV_UNLIMITED`, from `inc/AEELicenseInfo.h`: the value that means "never expires".
const BV_UNLIMITED: u32 = 0xffff_ffff;
/// Media classes, from `AEECLSID_MULTIMEDIA` (`QVERSION + 0x5500`) in `sdk/inc/AEEClassIDs.h`.
const AEECLSID_MEDIA: u32 = 0x0100_5500;
const AEECLSID_MEDIAMIDI: u32 = 0x0100_5501;
const AEECLSID_MEDIAMP3: u32 = 0x0100_5502;
/// `AEECLSID_MEDIAMIDIOUTMSG` = `AEECLSID_MULTIMEDIA + 5`, per the SDK list. Need for Speed creates
/// it when skipping the opening scene and, without checking the return, calls `SetMediaParm` on
/// the pointer — refusing it meant jumping to address zero. For a long time it carried the name
/// MPEG4, which is `+ 7`.
const AEECLSID_MEDIAMIDIOUTMSG: u32 = 0x0100_5505;
/// The other classes of the family, from the SDK's `AEECLSID_MULTIMEDIA`: QCP (`+3`), PMD (`+4`),
/// MIDIOUTQCP (`+6`), MPEG4 (`+7`), MMF (`+8`), PHR (`+9`), AAC (`+b`), IMELODY (`+c`), AMR
/// (`+e`), XMF (`+0x12`) and DLS (`+0x13`). They all become the same media object: the content
/// goes through the decoders we have, and what none of them reads ends right away, like any sound
/// we don't know how to play. Refusing the class brought down whoever creates it without checking.
const AEECLSID_MEDIA_FAMILY: [u32; 11] = [
    0x0100_5503,
    0x0100_5504,
    0x0100_5506,
    0x0100_5507,
    0x0100_5508,
    0x0100_5509,
    0x0100_550b,
    0x0100_550c,
    0x0100_550e,
    0x0100_5512,
    0x0100_5513,
];
const AEECLSID_MEDIAADPCM: u32 = 0x0100_550a;
/// `AEECLSID_MEDIAUTIL` = `AEECLSID_MULTIMEDIA + 13` — the factory of media objects.
const AEECLSID_MEDIAUTIL: u32 = 0x0100_550d;
const AEECLSID_MEDIAPCM: u32 = 0x0100_5511;
/// Image viewers, from `AEECLSID_VIEW` (`QVERSION + 0x4000`).
const AEECLSID_WINBMP: u32 = 0x0100_4001;
const AEECLSID_GIF: u32 = 0x0100_4003;
const AEECLSID_JPEG: u32 = 0x0100_4005;
/// `IMEDIA_SetMediaParm` parameters, in the order of the `MM_PARM_XXX` table in the BREW
/// documentation — the SDK 4.0.2 headers don't carry the values, only the table gives the order.
/// Two of them are confirmed by what the games do: 1 receives a pointer to an `AEEMediaData`, and
/// 4 receives numbers between 0 and 100.
const MM_PARM_MEDIA_DATA: u32 = 1;
const MM_PARM_VOLUME: u32 = 4;
const MM_PARM_MUTE: u32 = 5;
const MM_PARM_PLAY_REPEAT: u32 = 11;

/// `AEE_MAX_VOLUME`. The value is in none of the headers we have, and it came from what the games
/// use: Double Dragon sends 0 and 100, Quake sends 70 and 80, and nothing in any ROM goes past 100.
const MAX_VOLUME: u32 = 100;

/// `MMD_FILE_NAME` and `MMD_BUFFER`, the `clsData` of an `AEEMediaData`: a file name or memory.
/// The memory one came from observation — it is what the first three games that played sound
/// pass; the file one is what Galaxy on Fire uses for its music.
const MMD_FILE_NAME: u32 = 0;
/// The largest sound we read in one go. An absurd size is a wrong pointer, not music.
const MAX_MEDIA_BUFFER: u32 = 64 * 1024 * 1024;
const MMD_BUFFER: u32 = 1;
/// `MMD_ISOURCE`, the `clsData` of an `AEEMediaDataEx` whose data comes from an `ISource` the game
/// itself implements. It is BREW's `AEECLSID_SOURCE`, and it is what the Data East arcade ports
/// pass, with `bRaw` set and an `AEEMediaWaveSpec` giving the sample format.
const MMD_ISOURCE: u32 = 0x0100_1012;
/// The `Read` slot in the `ISource` vtable: `AddRef`, `Release`, `QueryInterface`, `Read`,
/// `Readable`.
const ISOURCE_READ_SLOT: u32 = 3;
/// The largest chunk of PCM requested from the `ISource` at once, in bytes.
const MAX_PCM_READ: u32 = 16 * 1024;

/// A sound the game generates while it plays: the `ISource` the samples come from and their format.
#[derive(Debug, Clone, Copy)]
struct PcmStream {
    source: u32,
    rate: u32,
    channels: u16,
    bits: u16,
    unsigned: bool,
    /// The `dwBufferSize` of the `AEEMediaDataEx`: the size of each `Read`, in bytes, a multiple of
    /// the frame. Zero when the game didn't say, or said something that doesn't fit the read
    /// buffer.
    block: u32,
    /// When `Play` started, on the virtual clock, and how many frames have been requested since.
    /// The difference between what the clock calls for and what has already come is what is left
    /// to request.
    start_us: u64,
    frames_read: u64,
    playing: bool,
    /// Whether we have already logged that the game **stopped supplying samples** on this stream.
    ///
    /// It is the measurement issue #43 asks for: a voice line that dies early dies here, when the
    /// game's `Read` returns zero, and the line says after how many seconds of audio that happened.
    /// Once per stream, or the log becomes the metric.
    logged_end: bool,
}

/// A memory-delivered WAVE that plays **by reading the game's buffer while it plays**. See
/// [`Machine::pump_live_buffers`].
///
/// Positions are in bytes, counted from the start of the PCM.
#[derive(Debug, Clone)]
struct LiveBuffer {
    /// Where the PCM starts, in the guest.
    pcm: u32,
    rate: u32,
    channels: u16,
    bits: u16,
    /// How far the sound goes, for now: it starts at the end of the `RIFF` and grows with what the
    /// game writes.
    end: u32,
    /// How far the `data` chunk goes. The sound does not grow past this.
    limit: u32,
    /// What was in the buffer from the end of the `RIFF` up to `limit`, at the time of `Play`. It
    /// is the yardstick for growth: only what the game wrote **after** `Play` counts.
    before: Vec<u8>,
    /// Where `before` starts.
    base: u32,
    start_us: u64,
    /// How many bytes have already been handed to the mixer.
    sent: u32,
    /// When the buffer was last compared against `before`.
    scanned_us: u64,
}

impl LiveBuffer {
    /// Bytes per frame: one sample from each channel.
    fn frame(&self) -> u32 {
        (u32::from(self.channels) * u32::from(self.bits / 8)).max(1)
    }

    /// How long `bytes` of PCM play for.
    fn duration_us(&self, bytes: u32) -> u64 {
        u64::from(bytes / self.frame()) * 1_000_000 / u64::from(self.rate.max(1))
    }
}

/// How long a live buffer waits, past the end of what the game wrote, before deeming the sound
/// finished.
///
/// The Tremor in Turma da Mônica writes about 0.3 s ahead of what is playing, in chunks of tenths
/// of a second; the slack covers a late chunk without making `DONE` visibly late.
const LIVE_BUFFER_GRACE_US: u64 = 300_000;
/// How far ahead of the clock a live buffer's samples go to the mixer — the same tenth of a second
/// as the `ISource` streams, so the sound card doesn't run dry between two passes.
const LIVE_BUFFER_LEAD_US: u64 = 100_000;
/// How often, in virtual time, the buffer is compared with what was there at `Play`.
const SCAN_INTERVAL_US: u64 = 20_000;

/// `IMedia` commands and statuses, from `inc/AEEIMedia.h` in the BREW 4.0.2 SDK.
const MM_CMD_PLAY: u32 = 4;
const MM_STATUS_START: u32 = 1;
const MM_STATUS_DONE: u32 = 2;

/// Size of `AEEMediaCmdNotify`: `clsMedia`, `pIMedia`, `nCmd`, `nSubCmd`, `nStatus`, `pCmdData`
/// and `dwSize`, seven words.
const MEDIA_NOTIFY_LEN: u32 = 28;

/// `IMEDIA_GetState` states, from `inc/AEEIMedia.h`.
const MM_STATE_READY: u32 = 2;
const MM_STATE_PLAY: u32 = 3;
const MM_STATE_PLAY_PAUSE: u32 = 5;

/// Firmware classes we don't have and that the game uses **without checking** that they exist.
///
/// An object that answers success to everything lets the game carry on, and whatever it calls on
/// it shows up in the probe report. Powerboat Challenge's `0x01001039` left this list when its use
/// showed what it is: see [`AEECLSID_MD5CTX`].
const CLASSES_UNDER_OBSERVATION: &[u32] = &[];

/// `0x01001039`: MD5 in the `IHashCTX` format, with the context in the game's memory.
///
/// **The number did not come from the SDK, it came from use.** Powerboat Challenge passes a
/// context of 0x58 bytes — the exact size of an `MD5_CTX`: four state words, eight bytes of count
/// and a 64-byte block — and compares 16 bytes of result with the end of the options file. Served
/// by the object that answers success to everything, it never computed anything: the stored
/// digest was garbage, and the game deleted the save on every launch as "corrupted".
const AEECLSID_MD5CTX: u32 = 0x0100_1039;

/// `AEECLSID_QEGL`, from the SDK's `AEECLSID_QEGL.bid`: the object that gives access to EGL and
/// OpenGL ES through BREW's newer interfaces. It is the one Quake tries first.
const AEECLSID_QEGL: u32 = 0x0103_d8ec;
/// `AEECLSID_EGL` and `AEECLSID_GL`, from `sdk/inc/AEEGL.h` — the old interfaces, which Crash
/// Nitro Kart asks for directly.
const AEECLSID_EGL: u32 = 0x0101_4bc4;
/// `AEECLSID_WEB`, from the SDK's `BMPIds.csv`: BREW's HTTP client.
const AEECLSID_WEB: u32 = 0x0100_5000;
/// `AEECLSID_DOWNLOAD`, from SDK 4.0.2 — and the answer to the `0x01000000` puzzle.
///
/// `AEE_CLSIDs.h` says, in three consecutive lines: `QVERSION` is `0x01000000`, `AEECLSID_PRIV` is
/// `QVERSION` and **`AEECLSID_DOWNLOAD` is `AEECLSID_PRIV`**. The earlier reading stopped at the
/// first of the three and recorded the class as "just `QVERSION`", with no interface. Measured: it
/// is `IDownload`, and it is what the Z-Wheel asks for in `ShopAction_Init` — refused, the wheel's
/// game library is not built.
const AEECLSID_DOWNLOAD: u32 = 0x0100_0000;
/// How many plaintext blocks the log keeps, and how much of each.
const PLAINTEXT_MAX: usize = 8;
const PLAINTEXT_BYTES: usize = 512;

/// A game word as a number: 16.16 fixed point in the `x` forms, `float` in the `f` forms.
fn to_scalar(word: u32, fixed_point: bool) -> f32 {
    match fixed_point {
        true => gles::fixed(word),
        false => f32::from_bits(word),
    }
}

const SILENTLY_HANDLED: &[&str] = &[
    "DepthFunc",
    "Hint",
    "LineWidth",
    "LineWidthx",
    "PolygonOffset",
    "PolygonOffsetx",
    "SampleCoverage",
    "SampleCoveragex",
    "Flush",
    "Finish",
];

/// The "stream ended" mark, which `ConnectionManager` sets on receiving an empty chunk.
const END_OF_STREAM: u32 = 0x1a;

/// Where the response object keeps the text it will split, as seen in the parser at `0x85d18`.
const RESPONSE_TEXT: u32 = 0x20;

/// Instruction cap for a call through the bridge. The allocator is a short function.
const BRIDGE_BUDGET: u64 = 5_000_000;
/// The line that goes with the file name in the allocator's tracking. It only keeps it for its own
/// reports, so any value will do; a distinct one helps recognize what came from here.
const ORIGIN_LINE: u32 = 0;

/// The connection states, read from the `switch` of Zeeboids' sync screen at `0x95bb8`.
///
/// It reads the field on every pass: `0` and `2` keep "Connecting", `9` leads to "ReceivingData"
/// and `11` to the failure branch, next to the `Connection_Failed` string.
const STATE_WORKING: u32 = 0;
const STATE_WORKING_2: u32 = 2;
const STATE_RECEIVING: u32 = 9;
const STATE_FAILED: u32 = 11;

/// Where the state sits, relative to the URL: twelve bytes before it, in the same object.
const STATE_OFFSET_BEFORE_URL: u32 = 0xc;

/// How far to search for the `{url, body, size}` triple in the object of whoever asked for the
/// send.
///
/// Zeeboids' is at `+0x244`; a thousand bytes comfortably cover objects of that size without
/// sweeping through the game's memory.
const MAX_OBJECT_FIELDS: u32 = 256;

/// Cap on a request body. The Zeeboids sender allocates 2 KB, which is the size it gives itself;
/// this cap is generous enough not to cut anything real.
const MAX_REQUEST_BODY: u32 = 1 << 16;

/// How many presses the log keeps.
///
/// Forty covered a short reproduction and do not cover a long one: navigating to the import and
/// typing a ZID and a password goes well past that, and what was left was the tail — useless for
/// reproducing, because the beginning is exactly what brings the game to the right state. With
/// this limit, a log becomes a complete `--keys` script, and one test session by the user yields
/// as many repetitions as I need here.
const PAD_LOG_MAX: usize = 400;
/// How many button events wait for `GetNextButtonEvent`, per port.
const PAD_EVENTS_MAX: usize = 64;
/// How many lines of [`Machine::media_log`] are kept.
const MEDIA_LOG_MAX: usize = 600;

/// The state of an `IPeek`: the source bytes, where the read is and where the line is assembled.
struct Peek {
    bytes: Vec<u8>,
    position: usize,
    /// Address, in guest memory, of the one-line buffer. See [`Machine::source_call`].
    buffer: u32,
}

impl Peek {
    /// The next line, without the `\n` and without the `\r` that goes with it in the console's
    /// files.
    ///
    /// Returns `None` when it's over. An empty line is a line: what separates "empty line" from
    /// "end of file" is the `Option`, not the length — `tectoy.cfg` has blank lines between the
    /// sections, and confusing the two would stop the read at the first of them.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        if self.position >= self.bytes.len() {
            return None;
        }
        let rest = &self.bytes[self.position..];
        let end = rest.iter().position(|&b| b == b'\n');
        let line = match end {
            Some(n) => {
                self.position += n + 1;
                &rest[..n]
            }
            None => {
                self.position = self.bytes.len();
                rest
            }
        };
        Some(match line.last() {
            Some(b'\r') => line[..line.len() - 1].to_vec(),
            _ => line.to_vec(),
        })
    }
}

/// Every how many API calls the cost profile reads the clock.
///
/// This machine's clock costs 1318 ns per read; one call in every 64 keeps the instrument below 1%
/// of the clock and still yields thousands of samples per method in any real game.
const PROFILE_SAMPLE_EVERY: u64 = 64;

/// `AEECLSID_SOURCEUTIL`, the `ISource` factory. See [`Interface::SourceUtil`] for how the number
/// was identified — for a long time it sat here under the wrong name.
const AEECLSID_SOURCEUTIL: u32 = 0x0100_1011;

/// `AEECLSID_MD5`: the MD5 digest, exposed as `IHash`.
const AEECLSID_MD5: u32 = 0x0100_1015;
/// `AEECLSID_CipherFactory`, from `inc/AEECipherFactory.bid`.
const AEECLSID_CIPHER_FACTORY: u32 = 0x0102_cce1;
const AEECLSID_GL: u32 = 0x0101_4bc3;
/// IIDs the EGL object's `QueryInterface` serves, from each interface's headers.
const AEEIID_GLES10: u32 = 0x0103_d8dd;
const AEEIID_GLES11: u32 = 0x0103_d8ea;
/// `AEEIID_EGLSURFACEMANIP_V1` and `AEEIID_EGLSURFACEMANIP`, from `sdk/inc/AEEEGLSurfaceManip.h`.
const AEEIID_EGL_SURFACE_MANIP_V1: u32 = 0x0104_34cc;
const AEEIID_EGL_SURFACE_MANIP: u32 = 0x0105_1834;
/// `AEEIID_GLESIMAGEONEXT_V1` and `AEEIID_GLESIMAGEONEXT`, from `sdk/inc/AEEGLESImageonEXT.h`.
const AEEIID_GLES_IMAGEON_EXT_V1: u32 = 0x0104_59b1;
const AEEIID_GLES_IMAGEON_EXT: u32 = 0x0105_8546;
/// `AEEIID_GLES11EXT`, from `sdk/inc/AEEGLES11Ext.h` — the OES extensions of OpenGL ES 1.1.
///
/// **Prey Evil asks for this class through `ISHELL_CreateInstance`** and gives up on the drawing
/// path when it gets null: without it, the survey catches it with eleven GL methods and no drawing.
const AEECLSID_GLES11EXT: u32 = 0x0103_d8eb;

/// `AEEIID_GLES10EXT`, from `sdk/inc/AEEGLES10Ext.h`. Prey Evil asks for it on the EGL object.
const AEEIID_GLES10EXT: u32 = 0x0103_d8de;
/// `AEEIID_EGLGETPOWERLEVEL`, from `sdk/inc/AEEEGLGetPowerLevel.h`.
const AEEIID_EGLGETPOWERLEVEL: u32 = 0x0103_d8f0;
/// `AEEIID_EGLOESSWAPINTERVAL`, from `sdk/inc/AEEEGLOESSwapInterval.h`.
const AEEIID_EGLOESSWAPINTERVAL: u32 = 0x0104_26e3;
/// `AEEIID_EGLGETCOLORBUFFER`, from `sdk/inc/AEEEGLGetColorBuffer.h`.
const AEEIID_EGLGETCOLORBUFFER: u32 = 0x0103_d8ef;
/// `AEEIID_GLES11EXTPAK`, from `sdk/inc/AEEGLES11ExtPak.h`.
const AEEIID_GLES11EXTPAK: u32 = 0x0103_def1;
/// `AEECLSID_IJOYSTICK1` and `AEECLSID_IJOYSTICK2`, from `sdk/inc/AEEJoystick.h`.
///
/// **Prey Evil creates this one and keeps the result.** If it is null, Qualcomm's joystick manager
/// carries on with the empty pointer and falls over on the first `Read` — which is the failure the
/// survey caught.
const AEECLSID_IJOYSTICK1: u32 = 0x0102_1c2b;
const AEECLSID_IJOYSTICK2: u32 = 0x0102_1dac;

const AEEIID_EGL10: u32 = 0x0103_d8ed;
const AEEIID_EGL11: u32 = 0x0103_d8ee;
/// The EGL display identifier. There is only one, and the value is arbitrary — what it must not be
/// is zero, which is `EGL_NO_DISPLAY`.
const EGL_DISPLAY: u32 = 1;
/// The only `EGLConfig` we offer, matching the `EGL_CONFIG_ID` of [`gles::config_attrib`].
const EGL_CONFIG: u32 = 1;
/// First surface and context identifier. It stays far from the display and the config so that
/// mixing one up with the other shows right away.
const EGL_HANDLE_BASE: u32 = 0x100;

/// `AEECLSID_THREAD` = `AEECLSID_CORE + 23`, from `sdk/inc/AEEClassIDs.h`.
const AEECLSID_THREAD: u32 = 0x0100_1017;
/// `EALREADY`: the thread has already been started once, and `IThread` is not reusable.
const EALREADY: u32 = 8;
/// Registers that make up the context of a cooperative thread.
///
/// `lr` is left out on purpose: to resume a thread what matters is where it stopped, and we keep
/// that separately, in `resume_pc`.
const THREAD_REGS: [Reg; 14] = [
    Reg::R0,
    Reg::R1,
    Reg::R2,
    Reg::R3,
    Reg::R4,
    Reg::R5,
    Reg::R6,
    Reg::R7,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::Sp,
];
/// Size of BREW's `AEECallback` (`inc/AEECallback.h`), in bytes.
const CALLBACK_SIZE: u32 = 28;
/// Floor for a thread's stack, in case the game asks for a value that is too small.
const THREAD_MIN_STACK: u32 = 64 * 1024;

/// `AEECLSID_SOUND` = `AEECLSID_CORE + 86`, confirmed by the `COMPILE_ASSERT` in `AEEClassIDs.h`.
const AEECLSID_SOUND: u32 = 0x0100_1056;
const AEECLSID_HEAP: u32 = 0x0100_1002;
/// `AEECLSID_UNZIPSTREAM`, from `sdk/inc/AEEClassIDs.h`.
const AEECLSID_UNZIPSTREAM: u32 = 0x0100_1014;

/// What an image decoder has gathered so far.
#[derive(Default)]
struct DecoderState {
    /// The file, assembled chunk by chunk through `IForceFeed::Write`.
    fed: Vec<u8>,
    /// The finished bitmap, created on the first `GetBitmap`.
    bitmap: Option<u32>,
    /// Whether the image has transparency — decides `GetRop`.
    transparent: bool,
}

/// Cap on a wait. An absurd value — a game asking for hours by mistake — must not drag the clock
/// along with it.
const MAX_SLEEP_MS: u32 = 10_000;

/// In which argument each surface-manipulation method keeps the `AEEEGLBoolean *ret`.
/// The count includes `this`, so the method's first parameter is index 1.
const EXTENSION_RESULT_SLOT: [(&str, usize); 11] = [
    ("SurfaceScaleEnable", 4),
    ("SurfaceRotateEnable", 4),
    ("SetSurfaceRotate", 6),
    ("SurfaceTransparencyEnable", 4),
    ("SetSurfaceTransparency", 4),
    ("SetSurfaceTransparencyMap", 4),
    ("SurfaceColorKeyEnable", 4),
    ("SetSurfaceColorKey", 6),
    ("SurfaceOverlayEnable", 4),
    ("SurfaceOverlayLayerEnable", 5),
    ("SurfaceOverlayBind", 5),
];

/// Cap on what a decoder accepts, so that a game that keeps writing doesn't eat up the host's
/// memory.
const MAX_DECODED_INPUT: usize = 16 * 1024 * 1024;

/// `AEEIID_FORCEFEED`. The value is not in the headers we have; it was identified by use — it is
/// the interface the `.bid` of `AEECLSID_PNGDecoderBREW` declares it supports, and it is the one
/// Heavy Weapon asks the decoder for before fetching the bitmap.
const AEEIID_FORCEFEED: u32 = 0x0101_eb0b;

/// Cap on an image converted to the native format, so that a broken header doesn't ask for too
/// much memory.
const MAX_NATIVE_IMAGE: usize = 8 * 1024 * 1024;

/// Cap on a preferences record, so that an absurd size doesn't ask for too much memory.
const MAX_PREFS: usize = 64 * 1024;

/// How far each character advances horizontally, in pixels.
///
/// We don't draw text, so this number only serves the game to position what it draws itself. It
/// follows the height declared in [`FONT_ASCENT`]: a small screen font, consistent with itself.
const FONT_ADVANCE: i32 = 7;
/// Size at which text is drawn when there is a font. The console had named sizes
/// (`AEE_FONT_NORMAL` and friends) and a `fontsize.map` that said how many pixels each one is;
/// that file did not come in the package, so for now there is a single size.
const FONT_SIZE: f32 = 16.0;
/// `CLR_USER_TEXT`, the color item `IDISPLAY_SetColor` uses for text.
const CLR_USER_TEXT: usize = 1;

/// Font metrics that `GetFontMetrics` reports. We don't draw text yet; what the games need from
/// here is consistent numbers to measure and position with.
const FONT_ASCENT: u32 = 12;
const FONT_DESCENT: u32 = 4;
/// `AEE_MAX_VOLUME`, from `inc/AEEISound.h`.
const AEE_MAX_VOLUME: u16 = 100;
/// `AEE_SOUND_SUCCESS` and `AEE_SOUND_PLAY_DONE`, from the `AEESoundStatus` enum.
const AEE_SOUND_SUCCESS: u32 = 1;
const AEE_SOUND_PLAY_DONE: u32 = 2;
/// `AEE_SOUND_STATUS_CB` and `AEE_SOUND_VOLUME_CB`, from the `AEESoundCmd` enum.
const AEE_SOUND_STATUS_CB: u32 = 0;
const AEE_SOUND_VOLUME_CB: u32 = 1;
/// `AEE_SOUNDPLAYER_VOLUME_CB` and `AEE_SOUNDPLAYER_SUCCESS`, from `inc/AEESoundPlayer.h`. Not the
/// same numbers as `ISound`'s: the player's callback types start with a status and a play one.
const AEE_SOUNDPLAYER_VOLUME_CB: u32 = 4;
const AEE_SOUNDPLAYER_SUCCESS: u32 = 1;
/// The `AEESoundPlayerInput` values, from `inc/AEESoundPlayer.h`. `SDT_NONE` (0) is reserved.
const SDT_FILE: u8 = 1;
const SDT_BUFFER: u8 = 2;
const SDT_VOICEPROMPT: u8 = 3;
/// Where an `ISoundPlayer` keeps its `AEESoundPlayerInfo`, inside its own object slot — see
/// `SetInfo` in `sound.rs`.
const SOUND_PLAYER_SOURCE: u32 = 16;
/// `AEEIID_DIB_20`, from `inc/AEEIDIB.h`: the IID `IDIB` had in BREW 2.0.
const AEEIID_DIB_20: u32 = 0x0100_102c;
/// `AEEIID_TRANSFORM`, from `AEETransform.h`: scaling and rotating one bitmap onto another.
///
/// It sat here as "the third IID of `IDIB`", deduced from being a neighbor of `AEEIID_DIB_20`, and
/// answered with the bitmap itself. Usage proves it wrong: Zenonia asks the **screen** bitmap for
/// this IID, keeps the pointer and calls its slot 4 with `(x, y, pSrc, xSrc, ySrc, dx, dy,
/// pMatrix, nComposite)` — the signature of `TransformBltComplex`. Answered as a bitmap, slot 4
/// landed on `NativeToRGB`, the frame never reached the screen and the game stayed black while
/// drawing all the time onto a 320x240 canvas.
const AEEIID_TRANSFORM: u32 = 0x0100_1029;
/// The canvas interface the Z-Wheel asks a bitmap for. See [`Interface::Canvas`].
const AEEIID_CANVAS: u32 = 0x0101_e443;
/// `IDIB_COLORSCHEME_565`, from `inc/AEEIDIB.h`: 5 bits of red, 6 of green, 5 of blue.
const IDIB_COLORSCHEME_565: u8 = 16;
/// `IDIB_COLORSCHEME_888`: 8 bits per channel.
const IDIB_COLORSCHEME_888: u8 = 24;
/// `AEECLSID_DIB` = `AEECLSID_CORE + 69`. It is the bitmap with direct access to the pixels.
const AEECLSID_DIB: u32 = 0x0100_1045;
/// `AEEIID_IBitmap`, from `inc/AEEIBitmap.h`.
const AEEIID_IBITMAP: u32 = 0x0100_1021;
/// Cap on vertices per polygon, against a corrupted pointer.
const MAX_POLYGON_POINTS: usize = 4096;
/// `AEE_MAX_FILE_NAME`.
const MAX_FILE_NAME: usize = 64;
/// How many lines of drawn text the report keeps.
const MAX_DRAWN_TEXTS: usize = 64;
/// How many file calls the report keeps. A 6 s game makes hundreds of `Test` calls; what matters
/// is the start of the run, and that is what the queue preserves.
const MAX_FS_LOG: usize = 96;
/// Space we report on the card. The Zeebo has 1 GB of NAND; we announce something of that order
/// so that no game refuses to save for lack of space.
const FS_TOTAL_BYTES: u32 = 512 * 1024 * 1024;
const FS_FREE_BYTES: u32 = 256 * 1024 * 1024;

/// BREW's `EFAILED`: generic failure. Used for "no pending event".
const EFAILED: u32 = 1;

/// `ALLOC_NO_ZMEM`, from `AEEStdLib.h`: asks for memory without zeroing, embedded in the size
/// itself.
const ALLOC_NO_ZMEM: u32 = 0x8000_0000;

/// Cap on trace lines, so that a loop doesn't fill up memory.
const MAX_TRACE: usize = 2000;

/// How many distinct texts to keep from `IDisplay::DrawText`.
///
/// The list exists only so the report can say what the game wrote on a screen we don't know how
/// to draw yet, and a game redraws the same text every frame: with no cap it grew forever, one
/// `String` per call, and the emulator got slower the longer it stayed on a screen with text.
const MAX_TEXT: usize = 64;

/// Cap on API calls in a single stretch — one entry into [`Machine::execute`], without yielding.
/// The instruction budget doesn't hold back a loop that calls the API on every pass, because it is
/// reset on every call served. The cap exists so that a repeat loop doesn't freeze the emulator,
/// and it is generous because a 3D game calls the API hundreds of thousands of times per second:
/// Quake alone makes more than three hundred OpenGL calls per frame.
///
/// **It used to be per run, and that brought down healthy games.** The counter never reset: at
/// about 330 thousand calls per second, the 200 million ran out in ten minutes, and the game
/// stopped with "probable repeat loop" in the middle of the race — issue #70, in Ridge Racer. What
/// separates a loop from a game is not yielding, not the accumulated total.
const MAX_CALLS: u64 = 200_000_000;

/// Instructions per microsecond of the virtual clock: the MSM7201A's ARM11 runs at 528 MHz, and
/// the console's ARM11 core is not superscalar, so one instruction per cycle is the right count.
const INSTRUCTIONS_PER_US: u64 = 528;

/// Vertical retrace period of the console's screen, in microseconds — 60 Hz.
///
/// The config we offer declares `EGL_MIN_SWAP_INTERVAL` and `EGL_MAX_SWAP_INTERVAL` equal to 1,
/// which is what the console does: `eglSwapBuffers` waits for the retrace. Without that wait Quake
/// drew four hundred frames per second of virtual time, something no device would do, and its
/// logic ran at the same rate.
const VSYNC_PERIOD_US: u64 = 1_000_000 / 60;

/// BREW's `SUCCESS` (`AEEError.h`).
const SUCCESS: u32 = 0;
/// `ECLASSNOTSUPPORT`: the requested ClassID does not exist on this platform.
const ECLASSNOTSUPPORT: u32 = 20;
/// `ENOMEMORY`: out of memory.
const ENOMEMORY: u32 = 3;
/// `ENOTYPE`: there is no type associated with this content (`AEEError.h`).
const ENOTYPE: u32 = 34;
/// `ENEEDMORE`: more data is needed to decide (`AEEError.h`).
const ENEEDMORE: u32 = 35;
/// How many bytes are enough to recognize the formats we know — the largest is `RIFF`, which needs
/// the first twelve to confirm `WAVE`.
const DETECT_TYPE_BYTES: u32 = 16;
/// BREW's `TRUE` — the helpers that return `boolean` use 1/0.
const TRUE: u32 = 1;

/// `GAV_LATIN1` from `AEEStdLib.h`: asks for the version as a single-byte string, not as `AECHAR`.
const GAV_LATIN1: u32 = 0x0001;
/// The BREW version we answer with, in the `GETAEEVERSION` format: the high byte of the high word
/// is the major version, then minor, sub-version and build. It is the 4.0.2 of the SDK the games
/// use.
const AEE_VERSION: u32 = 0x0400_0200;
const AEE_VERSION_TEXT: &str = "4.0.2.0";
/// Free space `GetFSFree` reports. The console has an optional memory card and a data partition;
/// what matters to the games is that the number isn't tight.
const FS_TOTAL: u32 = 64 * 1024 * 1024;

/// ClassIDs we know how to instantiate.
///
/// The values come from `AEEClassIDs.h`: `AEECLSID_CORE = QVERSION + 0x1000` with
/// `QVERSION = 0x01000000`, so `AEECLSID_DISPLAY = AEECLSID_CORE + 1`.
const AEECLSID_DISPLAY: u32 = 0x0100_1001;

/// `AEECLSID_DISPLAY1`, from `sdk/inc/AEEDisp.h`. The `DISPLAY1` to `DISPLAY4` classes are the
/// numbered screens of a device with more than one, and they expose the **same `IDisplay`** as the
/// default screen — the header is explicit about it. The Zeebo has a single screen, so the first
/// is the one that exists.
///
/// Three games ask for this class: Magical Drop 3, Peggle and Pac-Mania. Magical Drop stored the
/// refusal in a field and wrote to it right after, through a null pointer.
const AEECLSID_DISPLAY1: u32 = 0x0101_27d4;
/// `AEECLSID_FILEMGR`, from the SDK's `AEECLSID_FILEMGR.bid`. In `AEEClassIDs.h` it only appears
/// commented out, which has already made me get this value wrong once.
const AEECLSID_FILEMGR: u32 = 0x0100_1003;
/// The state of a widget: its children, its properties and its size.
///
/// Keeping the children is what makes the accessor consistent with itself — asking twice for child
/// `0x5000` must return the same object, or the game ends up with two.
#[derive(Default)]
struct Widget {
    children: HashMap<u32, u32>,
    properties: HashMap<u32, u32>,
    /// Models attached through slot 17. The Z-Wheel's roller pins the font here under id
    /// `0x8000`; the caller drops its temporary reference right after, so the widget is the one
    /// that needs to keep it alive while the roller exists.
    models: HashMap<u32, u32>,
    /// Width and height, from slot 7. The Z-Wheel sends `640 × 480` — the whole screen.
    size: (u32, u32),
    /// Where the parent hung this widget, in the parent's coordinates. See `AdicionarFilho`.
    position: (i32, i32),
    /// The class it was created with. Five classes of the family share the same slot table, and the
    /// same number means different things in each — see slot 6.
    class_id: u32,
    /// The text slot 6 put on it, when the class is one of those that take text.
    text: String,
    /// Creation order. See [`Machine::current_form`].
    serial: u64,
    /// The children that came in through slot 5, which doesn't identify them by number.
    attached: Vec<u32>,
    /// Whether the widget should be shown. Slot 6 is the one that says.
    visible: bool,
    /// Who hung it, from slot 5. See `PegarPai`.
    parent: u32,
    /// The handler slot 4 registered, already read: `(function, context)`.
    ///
    /// We keep the **values**, not the address of the struct, because `DefinirTratador` returns
    /// the previous handler by writing it back into that same struct — after the call it no longer
    /// describes whoever just registered.
    handler: (u32, u32),

    /// The drawing callback slot 16 registered: `(function, context)`.
    ///
    /// It comes from a `{function, context, free_fn}` triple the game builds in its own struct and
    /// passes by pointer; the slot returns the previous one by writing it back into the first two
    /// words, the same way slot 4 does with the handler. That is why the registered function can
    /// call "the one below" without keeping anything: it reads from the place where it wrote.
    drawing: (u32, u32),
    /// The releasers of the two triples, the third word of each: `(of handler, of drawing)`.
    /// See the widget's `Release`.
    releasers: (u32, u32),
    /// Whether the start notice has already been delivered. See [`Machine::start_animation`].
    start_notified: bool,
}

/// The number a missing-file line carries in parentheses, if it carries one.
///
/// It serves [`Machine::missing_files`]: resource lines have the form `path (recurso 5035)`, and
/// actual file lines have no part in parentheses at all.
fn resource_id(remaining: &str) -> Option<u16> {
    remaining
        .rsplit_once("(recurso ")
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .and_then(|number| number.parse().ok())
}

/// The classes of the interface extension that answer to the same accessor as
/// [`Interface::Widget`].
///
/// The first, `0x01028e51`, was read from the Z-Wheel's code. The others came in by measurement:
/// the game asks for them in sequence — `0x01028e19` and `0x01028e2a` are the "frame widget" of
/// `AnimationVideo_Form.c:93`, `0x01028e47` is the video form itself —, and serving them with this
/// accessor makes each error message go away and the next one appear.
///
/// They are numbering neighbors and appear together in the same firmware table, at `0x1035cf24`.
/// None is in the class table, so there is no vtable to check: what backs the list is the game
/// moving on, and that is why it lives here, with the why written down, instead of becoming a `|`
/// in the middle of the dispatch.
/// The class of a **form**: it has an event handler and hangs its content on item `0x5000`.
///
/// Read from the tree: the opening and the z-pad form are two objects of this class, children of
/// the applet root (`0x01028e51`), each with its container hung on item `0x5000`.
const WIDGET_FORM: u32 = 0x0102_8e47;
/// The applet's root form, the Z-Wheel's `[app+0x24]`. On it slot 6 is `IROOTFORM_RemoveForm`.
/// See the `DefinirVisivel` arm.
const WIDGET_ROOT: u32 = 0x0102_8e51;

/// The class of the family in which slot 6 **sets text**, instead of hiding or showing.
///
/// Measured from both sides. On the Z-Wheel side, following what `ISHELL_LoadResString` loads up
/// to where it stops: the text becomes the first argument of slot 6, with the length in the
/// second, and the object that receives it was created with this class. On the firmware side,
/// `0x01028e2a` is the most used class of the family — forty occurrences.
///
/// In the other classes the same slot is still visibility, which is how it was first read. It is
/// not a contradiction: the slot table is the same and the implementation behind it is not.
const WIDGET_TEXT: u32 = 0x0102_8e2a;

/// The property that holds the widget's color, with alpha in the low byte.
///
/// The Z-Wheel writes `0x444444ff` to it — the gray of the welcome screen's text. The firmware
/// writes values of the same shape through the setter at `0x1035f222`.
const PROP_COLOR: u32 = 0x140;
/// A widget's background color, `RRGGBBAA`, through the same accessor as [`PROP_COLOR`].
const PROP_BACKGROUND_COLOR: u32 = 0x130;

/// The class of the HTML widget, which the help screen (`FAQ_Form.c`) creates to show the pages.
///
/// We don't render HTML: the widget is served like the others of the family and shows the text of
/// a markup file that lives **outside** the ROM — see `Machine::html_text`. Without the class, the
/// screen logged `Unable to create HTML Widget, ERROR(20)` and confirming on "Ajuda" opened
/// nothing.
const WIDGET_HTML: u32 = 0x0102_dd32;

const WIDGET_FAMILY: [u32; 10] = [
    WIDGET_HTML,
    AEECLSID_WIDGET,
    // `0x01028e05` is the last one the stage asks for. After setting up the pbuffer —
    // `ChooseConfig`, `CreatePbufferSurface`, `CreateContext`, `MakeCurrent` — the Z-Wheel creates
    // `0x01028e14` and right after this one; refused, `CreateStageWidget` gives up and takes the
    // game wheel down with it. In the firmware it appears in the same literal pools as
    // `0x01028e19` and `0x01028e4b`, next to selector `0x801` and a color, which is how the
    // others of the family are used.
    0x0102_8e05,
    // `0x01028e14` is the `OwnerDrawWidget`: `CreateTectoyRollerWidget` creates it and, refused,
    // logs `Failure in call to CreateOwnerDrawWidget` and gives up on the whole game wheel.
    0x0102_8e14,
    0x0102_8e19,
    // `0x01028e26` also came in through the probe: `slot3(0x801, 0x186, 0xff0000ff)` — the
    // accessor, with a color — and the same slot 14 as the others.
    0x0102_8e26,
    0x0102_8e2a,
    // `0x01028e36` came in through the probe, not through numbering proximity: served by
    // observation, the game called `slot3(0x801, 0x156, …)` on it — the widget accessor, with the
    // write selector and a property id from the same range as the others. Without it,
    // `ZPad_Keyboard_Instructions_Form.c` failed with `ECLASSNOTSUPPORT`, which is the 20 in
    // `Couldn't create z-pad instruction form (20)`.
    0x0102_8e36,
    0x0102_8e3f,
    0x0102_8e47,
];

/// `0x01035156`, the console's TrueType font. See [`Interface::Typeface`].
const AEECLSID_TYPEFACE: u32 = 0x0103_5156;
/// Concrete font class used by the Z-Wheel's roller.
/// The font the Z-Wheel draws the cover roller with. **It is a system font**: `0x0102f67c` is
/// `AEECLSID_FONT_STANDARD18B`, served by [`crate::machine::font`]. The name stays because that is
/// what the Z-Wheel calls it.
#[allow(dead_code)]
const AEECLSID_ROLLER_FONT: u32 = 0x0102_f67c;

/// `0x01006c01`, the SIM card control. See [`Interface::SimCardCtl`].
const AEECLSID_SIMCARDCTL: u32 = 0x0100_6c01;

/// `0x01006c02`, the system control. See [`Interface::SystemCtl`].
const AEECLSID_SYSTEMCTL: u32 = 0x0100_6c02;

/// `0x01011810`, `ICM`. See [`Interface::Cm`].
const AEECLSID_CM: u32 = 0x0101_1810;

/// `0x01028e3c`, the third extension the Z-Wheel asks for. See [`Interface::Class28e3c`].
const AEECLSID_28E3C: u32 = 0x0102_8e3c;

/// `0x01028e35`, the Z-Wheel's generic list. See [`Interface::Vector`].
const AEECLSID_VECTOR: u32 = 0x0102_8e35;

/// `0x01001027`, `IConfig`. See [`Interface::Config`].
const AEECLSID_CONFIG: u32 = 0x0100_1027;

/// `0x01006c05`, ZEEBOMCP. See [`Interface::ZeeboMcp`].
const AEECLSID_ZEEBOMCP: u32 = 0x0100_6c05;

/// `0x01028e51`, the Z-Wheel's interface widget. See [`Interface::Widget`].
///
/// It is not in any header either, nor in the firmware class table we have. The value came from
/// the module itself: it is the literal at `0x7c69c`, loaded by `tectoymain.c:1001` and handed to
/// the `ISHELL_CreateInstance` whose failure prints `Could not create root form`.
const AEECLSID_WIDGET: u32 = 0x0102_8e51;
/// `0x01003109`, the text control Zenonia uses (`CWBLText`).
const AEECLSID_CONTROL: u32 = 0x0100_3109;

/// The generic collection the Z-Wheel's interface uses.
///
/// There is no header. What identifies the class is the module's literal pool: the constant
/// appears twenty bytes before `Could not create root form`, and also in the `Tectoy_Start` that
/// instantiates the "app history" and in `Tectoy_LaunchMainMenu` — three places with nothing in
/// common besides holding items.
const AEECLSID_COLLECTION: u32 = 0x0100_104f;

/// `AEECLSID_SQLMGR` — the console's database manager.
///
/// The value did not come from any header: it came from the Z-Wheel's own log, which prints
/// `No SQLMGR: 20` from `tectoy_prefsDB.c` every time `ISHELL_CreateInstance` of this class is
/// refused — 21,845 times in a row, until the stack overflowed.
const AEECLSID_SQLMGR: u32 = 0x0102_c4e8;
/// `AEECLSID_HID` — the Zeebo's gamepad.
///
/// It is in no header we have. It was identified like this: the value appears three times inside
/// `IHID.dll` from the Zeebo SDK (whose installer only contained the HID extension, with the
/// strings `"Failed to open HID device"` and `fs:/sys/hid_devices.cfg`), the SDK's `conftest`
/// calls `ISHELL_CreateInstance(shell, AEECLSID_HID, &pIHID)` and `conftest.elf` contains the same
/// constant — and `AEEIID_IHID` is `0x0106c38d`, a neighbor in the range.
const AEECLSID_HID: u32 = 0x0106_c411;
/// `AEECLSID_SignalCBFactory`, from `AEESignalCBFactory.bid` in BREW SDK 4.0.2. It is how the app
/// creates the signals the system fires to notify it of events — in the games' case, gamepad
/// button events.
const AEECLSID_SIGNAL_CB_FACTORY: u32 = 0x0104_1207;
/// `AEECLSID_GRAPHICS` = `AEECLSID_CORE + 0x1001`, resolved from `AEEClassIDs.h`.
const AEECLSID_GRAPHICS: u32 = 0x0100_2001;
/// `AEECLSID_SOUNDPLAYER` = `AEECLSID_ENH`, the one right before `AEECLSID_GRAPHICS`
/// (`AEECLSID_ENH + 1`) in `inc/AEEClassIDs.h`.
const AEECLSID_SOUNDPLAYER: u32 = 0x0100_2000;

/// `EVT_APP_START`, from `inc/AEEEvent.h`. It is zero — the first event an applet receives.
const EVT_APP_START: u32 = 0;
/// `EVT_APP_STOP`, from `inc/AEEEvent.h`: the notice that the applet is about to be closed.
const EVT_APP_STOP: u32 = 1;

/// The Zeebo's screen: VGA 640×480, composite output.
const SCREEN_WIDTH: u16 = 640;
const SCREEN_HEIGHT: u16 = 480;
/// Color depth in bits. The console's framebuffer is RGB565.
const COLOR_DEPTH: u16 = 16;

/// Why the module's execution ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `AEEMod_Load` returned. `code` is the value of `r0`.
    Returned { code: u32 },
    /// The guest called an API we haven't implemented yet. `caller` is the return address kept in
    /// `lr` — that is, right after the instruction that made the call, which makes it possible to
    /// find the responsible code when disassembling the module.
    Unimplemented {
        addr: u32,
        args: [u32; 4],
        caller: u32,
    },
    /// Memory access outside the map.
    Fault {
        addr: u32,
        pc: u32,
        /// `lr` at the moment of the fault: says where the failing function was called from.
        lr: u32,
    },
    /// Core exception — invalid instruction, SWI, etc.
    Exception { pc: u32 },
    /// The instruction budget ran out without getting anywhere.
    Budget,
    /// The guest went past the cap on API calls — almost always a repeat loop caused by some API
    /// that returns an error and the game retrying it indefinitely.
    CallLimit { calls: u64 },
}

/// Outcome of [`Machine::create_applet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppletResult {
    /// `AEEMod_Load` did not leave an `IModule*` — there is nothing to call.
    NoModule,
    /// `CreateInstance` returned. `code` is the BREW error, `applet` the pointer created.
    Called { code: u32, applet: u32 },
    /// Execution stopped before returning.
    Stopped(Outcome),
}

/// `IGraphics` drawing state.
#[derive(Debug, Clone, Copy)]
struct GraphicsState {
    stroke: Rgb,
    fill: Rgb,
    background: Rgb,
    fill_mode: bool,
    point_size: u8,
    /// Offset applied to every coordinate, set by `Translate`.
    origin: (i32, i32),
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            stroke: Rgb::BLACK,
            fill: Rgb::WHITE,
            background: Rgb::WHITE,
            fill_mode: false,
            point_size: 1,
            origin: (0, 0),
        }
    }
}

/// A file opened by the game.
#[derive(Debug)]
struct OpenFile {
    file: std::fs::File,
    /// Path as the game asked for it, to return in `GetInfo`.
    guest_path: String,
    /// Where the open file lives on the host. It is not always what `guest_path` resolves to: the
    /// `tectoy.cfg` without end of life is a copy in the device profile.
    path: std::path::PathBuf,
    /// Whether it was opened for writing. It is what keeps rewind from marking a point in the
    /// middle of a write: see [`Machine::has_writable_file`].
    for_writing: bool,
}

/// A guest callback: the function and the context it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Callback {
    pub function: u32,
    pub context: u32,
}

/// An `IValueModel`: the value and whoever wants to know when it changes.
#[derive(Debug, Default, Clone)]
pub(crate) struct ValueModel {
    value: u32,
    size: u32,
    /// Each listener is the address of the game's `ModelListener`, with the function and the
    /// context that were in it when it was registered. Checking both before calling is what avoids
    /// calling a listener whose memory the game has already freed and reused.
    listeners: Vec<(u32, u32, u32)>,
}

/// A timer armed by `ISHELL_SetTimer`.
///
/// BREW timers are one-shot: whoever wants periodicity re-arms inside the callback itself. That is
/// exactly how a game builds its frame loop.
#[derive(Debug, Clone, Copy)]
struct Timer {
    /// Virtual clock value at which it expires.
    deadline_ms: u32,
    callback: Callback,
}

/// An already decoded image, ready to draw.
#[derive(Debug, Clone)]
struct DecodedImage {
    width: u32,
    height: u32,
    /// Pixels in RGB565, in reading order.
    pixels: Vec<u16>,
    /// Whether the pixel should be drawn. PNG carries an alpha channel and the games count on it.
    opaque: Vec<bool>,
    /// The alpha of each pixel, **only when the image has partial transparency**; empty when every
    /// pixel is opaque or transparent, which is the common case. Whoever draws the image on screen
    /// blends with what is underneath. The Z-Wheel's selection frame (`214×47`, in `tectoyli.brf`)
    /// is an opaque stroke around a blue center with alpha 51: without blending, only the stroke
    /// was left.
    alpha: Vec<u8>,
    /// Width of each frame, when the game splits the image into strips (`IPARM_CXFRAME`).
    frame_width: u16,
}

/// Puts the frames of a GIF side by side, in a single image.
///
/// It is the shape the rest of the emulator already understands: an image with `frame_width`
/// smaller than the width is a sequence, and `IIMAGE_DrawFrame` picks the column. Giving GIF
/// animation its own path would repeat what `IPARM_CXFRAME` already does.
fn frame_strip(gif: &crate::video::gif::Gif) -> DecodedImage {
    let (gif_width, gif_height) = (gif.width as usize, gif.height as usize);
    let frames = gif.frames.len();
    let total = gif_width * frames * gif_height;
    let mut pixels = vec![0u16; total];
    let mut opaque = vec![false; total];
    for (n, frame) in gif.frames.iter().enumerate() {
        for y in 0..gif_height {
            for x in 0..gif_width {
                let color = frame[y * gif_width + x];
                let location = y * gif_width * frames + n * gif_width + x;
                pixels[location] = Rgb {
                    r: color[0],
                    g: color[1],
                    b: color[2],
                }
                .to_rgb565();
                // GIF alpha is binary: either the color is the palette's transparent one, or not.
                opaque[location] = color[3] != 0;
            }
        }
    }
    DecodedImage {
        width: (gif_width * frames) as u32,
        height: gif_height as u32,
        pixels,
        opaque,
        alpha: Vec::new(),
        // A single-frame GIF is not a sequence: saying it is would make `GetInfo` announce a
        // frame width the game didn't ask for.
        frame_width: match frames > 1 {
            true => gif_width as u16,
            false => 0,
        },
    }
}

/// The largest byte request that can come from the guest without becoming a disproportionate host
/// allocation.
///
/// **The size is the game's, and the allocator is ours.** `IFILE_Read(pBuffer, 0x7fffffff)` is a
/// line that fits in the guest and would ask for 2 GiB here; the corresponding `vec![0u8; n]`
/// doesn't return an error — it aborts the process. The cap is 128 MiB: eight times the largest
/// legitimate read ever measured (Iron Sight's 16 MB package, read in large chunks) and twice the
/// game's heap. Nothing larger than that can be handed to the guest anyway.
pub(super) const READ_CAP: u32 = 128 * 1024 * 1024;

/// **A size that came from the guest, checked before it becomes an allocation.**
///
/// The size of a read or a write is the game's argument, and the allocator is ours:
/// `IFILE_Read(pBuffer, 0x7fffffff)` is a line that fits in the guest and would ask for 2 GiB
/// here, and a `vec![0u8; n]` of that size doesn't return an error — the process dies. See
/// [`READ_CAP`]: an invalid argument has to become an API error.
pub(super) fn guest_size(len: usize) -> Result<usize, CpuError> {
    if len > READ_CAP as usize {
        return Err(CpuError(format!(
            "the guest asked for {len} bytes, above the cap of {READ_CAP}"
        )));
    }
    Ok(len)
}

/// Decompresses a deflate block.
///
/// The `IUnzipAStream` documentation speaks of the "deflate algorithm, the one used by gzip", and
/// both forms show up in practice: the raw stream and the same stream inside a gzip or zlib
/// envelope. We try all three, from most to least likely, because telling them apart by header
/// fails precisely in the raw case, which has no header at all.
fn inflate(compressed: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    // **Capped, and one at a time.** Without a cap, a block of a few KiB that decompresses to
    // gigabytes — deflate's amplification has no limit by construction — fills the host's memory;
    // and trying gzip, zlib and raw **together** materialized three outputs before choosing one.
    // `take` cuts the decoder off at the cap plus one byte: if the output goes past that, it is a
    // bomb, not data, and the request is refused instead of bringing down the process.
    let cap = u64::from(INFLATE_CAP);
    let try_decode = |source: &[u8], which: u8| -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut reader: Box<dyn Read> = match which {
            0 => Box::new(flate2::read::GzDecoder::new(source)),
            1 => Box::new(flate2::read::ZlibDecoder::new(source)),
            _ => Box::new(flate2::read::DeflateDecoder::new(source)),
        };
        let got = reader.by_ref().take(cap + 1).read_to_end(&mut out).ok()?;
        (got > 0 && got as u64 <= cap).then_some(out)
    };
    (0..3).find_map(|which| try_decode(compressed, which))
}

/// The cap on a decompression, in bytes.
///
/// See [`inflate`]: the output of a deflate block can be orders of magnitude larger than the input,
/// and the cap is the guest's memory — nothing larger than that can be handed to the game anyway.
const INFLATE_CAP: u32 = 128 * 1024 * 1024;

/// Decodes an image to RGB565, whatever its format.
///
/// BREW's `IImage` is not "the PNG object": it is an interface, and the console has one class per
/// format — `AEECLSID_PNG`, `AEECLSID_BMP`, `AEECLSID_JPEG`. All of them are fed the same way, with
/// an `IAStream`, and that is why looking at the magic bytes is better than trusting the ClassID:
/// the game may ask for one class and hand over something else, and the format is written in the
/// file.
///
/// Action Hero 3D is what demanded this: it asks for `0x01004001` and its resources are 67 BMPs
/// inside a `.res`. Refusing the class, it dereferenced null right after — and with it served but
/// the decoder PNG-only, the image would come out empty.
fn decode_image_bytes(bytes: &[u8]) -> Option<DecodedImage> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => decode_png(bytes),
        // BMP and JPEG go through the decoder that already serves the titles' icons. It returns
        // RGBA, and a BMP has no alpha channel: every pixel is opaque, and whoever wants
        // transparency uses the surface's reserved color, as BREW does.
        _ => {
            let image = crate::video::icon::decode(bytes).ok()?;
            let total = image.width * image.height;
            let mut pixels = Vec::with_capacity(total);
            let mut opaque = Vec::with_capacity(total);
            let mut alpha = Vec::with_capacity(total);
            for pixel in image.rgba.chunks_exact(4).take(total) {
                alpha.push(pixel[3]);
                pixels.push(
                    Rgb {
                        r: pixel[0],
                        g: pixel[1],
                        b: pixel[2],
                    }
                    .to_rgb565(),
                );
                opaque.push(pixel[3] >= 128);
            }
            Some(DecodedImage {
                width: image.width as u32,
                height: image.height as u32,
                pixels,
                opaque,
                alpha: alpha_if_halftone(alpha),
                frame_width: 0,
            })
        }
    }
}

/// Decodes an image **by its signature**, not by a single format.
///
/// The PNG path is still the first, because it has its own handling: palette with `tRNS`, depths
/// below eight bits and partial alpha, which `decode_png` below resolves with the `png` crate's
/// transformations and the rest of the engine cannot repeat.
///
/// **What was missing was the rest.** Zuma's Revenge asks for the **JPEG** decoder, feeds it a
/// sixteen-kilobyte JPEG and got `EFAILED` from `GetBitmap` — the engine only tried PNG, and the
/// recorded hypothesis was "a decoder received data that is not a PNG". The signature dispatcher
/// already existed in [`crate::video::icon::decode`], used by the modules' icons; here it now also
/// serves the guest's decoder.
fn decode_fed_image(bytes: &[u8]) -> Option<DecodedImage> {
    if let Some(image) = decode_png(bytes) {
        return Some(image);
    }
    let image = crate::video::icon::decode(bytes).ok()?;
    let count = image.pixels();
    let mut pixels = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    let mut alpha = Vec::with_capacity(count);
    for pixel in image.rgba.chunks_exact(4) {
        pixels.push(
            Rgb {
                r: pixel[0],
                g: pixel[1],
                b: pixel[2],
            }
            .to_rgb565(),
        );
        let a = pixel[3];
        opaque.push(a == 255);
        alpha.push(a);
    }
    Some(DecodedImage {
        width: image.width as u32,
        height: image.height as u32,
        pixels,
        opaque,
        alpha,
        // No `IPARM_CXFRAME`: the one that splits the image into strips is the decoder's PNG, and
        // the path for the other formats doesn't receive that parameter.
        frame_width: 0,
    })
}

/// Decodes a PNG to RGB565, returning `None` if it is not a PNG we know how to read.
fn decode_png(bytes: &[u8]) -> Option<DecodedImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Bejeweled Twist's PNGs use a palette with `tRNS`. Asking for expansion here avoids having to
    // reimplement palettes, indexed transparency and depths below 8 bits.
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::normalize_to_color8(),
    );
    let mut reader = decoder.read_info().ok()?;
    let mut raw = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut raw).ok()?;
    let data = &raw[..info.buffer_size()];

    let channels = info.color_type.samples();
    let has_alpha = matches!(
        info.color_type,
        png::ColorType::Rgba | png::ColorType::GrayscaleAlpha
    );
    let count = (info.width * info.height) as usize;
    let mut pixels = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    let mut alpha = Vec::with_capacity(count);
    for chunk in data.chunks_exact(channels) {
        let (r, g, b) = if channels >= 3 {
            (chunk[0], chunk[1], chunk[2])
        } else {
            (chunk[0], chunk[0], chunk[0])
        };
        pixels.push(Rgb { r, g, b }.to_rgb565());
        // Partial alpha doesn't exist on a surface without an alpha channel: either the pixel goes
        // in, or it doesn't. On a surface without an alpha channel, partial alpha becomes opaque or
        // transparent; it is `alpha` that preserves it for whoever draws the image over something
        // else.
        let a = if has_alpha {
            chunk[channels - 1]
        } else {
            u8::MAX
        };
        opaque.push(a >= 128);
        alpha.push(a);
    }

    Some(DecodedImage {
        width: info.width,
        height: info.height,
        pixels,
        opaque,
        alpha: alpha_if_halftone(alpha),
        frame_width: 0,
    })
}

/// The DIB bytes BREW's decoder hands over: PNG through its own path, and the rest (JPEG, BMP)
/// through the signature dispatcher, in RGB when there is no alpha.
///
/// **JPEG also comes out in 24 bits.** Zuma's Revenge decodes its backgrounds in JPEG and converts
/// the DIB to 565 on its own, reading three bytes per pixel. Without this path the JPEG got the
/// 16-bit DIB of the generic bitmap, and every line of the background came out as stripes.
fn decode_dib_bytes(bytes: &[u8]) -> Option<(u32, u32, usize, Vec<u8>)> {
    if let Some(png) = decode_png_bytes(bytes) {
        return Some(png);
    }
    let image = crate::video::icon::decode(bytes).ok()?;
    let (width, height) = (image.width as u32, image.height as u32);
    if image.rgba.chunks_exact(4).all(|p| p[3] == u8::MAX) {
        let rgb = image.rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
        return Some((width, height, 3, rgb));
    }
    Some((width, height, 4, image.rgba))
}

/// A PNG in 8-bit-per-channel bytes: RGB, or RGBA when the image has alpha.
///
/// It is the format in which BREW's PNG decoder delivers the `IDIB`. Grayscale becomes RGB, with
/// or without alpha, and the palette is expanded.
fn decode_png_bytes(bytes: &[u8]) -> Option<(u32, u32, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::normalize_to_color8(),
    );
    let mut reader = decoder.read_info().ok()?;
    let mut raw = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut raw).ok()?;
    let data = &raw[..info.buffer_size()];
    let (channels, output) = match info.color_type {
        png::ColorType::Rgba => (4, data.to_vec()),
        png::ColorType::Rgb => (3, data.to_vec()),
        png::ColorType::GrayscaleAlpha => (
            4,
            data.chunks_exact(2)
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
        ),
        _ => (3, data.iter().flat_map(|&c| [c, c, c]).collect()),
    };
    Some((info.width, info.height, channels, output))
}

/// The alpha of an image, or nothing when it has no partial transparency — then `opaque` already
/// says it all, and drawing goes down the path without blending.
fn alpha_if_halftone(alpha: Vec<u8>) -> Vec<u8> {
    match alpha.iter().any(|&a| a != 0 && a != u8::MAX) {
        true => alpha,
        false => Vec::new(),
    }
}

/// Blends `color` over `background`, both in RGB565, with `alpha` from 0 to 255.
fn blend_rgb565(background: u16, color: u16, alpha: u8) -> u16 {
    let a = u32::from(alpha);
    let channel = |offset: u32, mask: u32| {
        let f = (u32::from(background) >> offset) & mask;
        let c = (u32::from(color) >> offset) & mask;
        ((c * a + f * (255 - a) + 127) / 255) << offset
    };
    (channel(11, 0x1f) | channel(5, 0x3f) | channel(0, 0x1f)) as u16
}

/// State of an `IUnzipAStream`: where the compressed bytes come from and what has already come out
/// of them.
#[derive(Debug, Default, Clone)]
struct UnzipState {
    /// The input `IAStream`, as the game handed it over in `SetStream`.
    source: u32,
    /// The result of decompression, produced all at once on the first read.
    output: Vec<u8>,
    position: usize,
    /// Whether we have already tried to decompress. Input that doesn't decompress is not tried
    /// again.
    expanded: bool,
}

/// How an `AEEMediaData` arrived.
pub(super) enum Delivery {
    /// Already read and stored, by key.
    Ready(u64),
    /// A memory buffer, to be read on the next pass of the loop.
    Buffer(u32, u32),
    Nothing,
}

/// A sound handed to an `IMedia`, already read.
///
/// The key is the **content**: the old cache, by address and size, played the previous sound when
/// a new one landed in the same place. When the bytes are read is in [`Machine::resolve_media`].
#[derive(Debug, Clone, Default)]
struct MediaPayload {
    sound: Option<std::sync::Arc<crate::audio::wav::Sound>>,
    /// The duration of a sound we don't decode but know how to time (today, MP3).
    silence_us: Option<u64>,
}

/// What is known about an `IMedia` object.
#[derive(Debug, Clone, Copy)]
struct MediaState {
    state: u32,
    /// The sound delivered by the `AEEMediaData`, by its key in [`Machine::media_payloads`]. Zero
    /// is "nothing delivered".
    payload: u64,
    /// A buffer delivered and not yet read: `(address, size)`. See
    /// [`Machine::resolve_media`].
    pending: (u32, u32),
    /// The buffer of the last memory delivery, `(address, size)`. Every `Play` rereads it: see
    /// [`Machine::media_play`].
    buffer: (u32, u32),
    /// The game asked to play before the buffer was read.
    play_on_load: bool,
    /// From 0 to [`MAX_VOLUME`].
    volume: u32,
    /// How many times to play. Zero is forever, which is what `MM_PARM_PLAY_REPEAT` sets.
    repeat: u32,
    muted: bool,
    /// `PFNMEDIANOTIFY` registered by `RegisterNotify`.
    notify: Callback,
    /// When the sound ends, on the virtual clock. Zero is "not playing", and `u64::MAX` is
    /// infinite `repeat`.
    ends_us: u64,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            state: MM_STATE_READY,
            payload: 0,
            pending: (0, 0),
            buffer: (0, 0),
            play_on_load: false,
            volume: MAX_VOLUME,
            repeat: 1,
            muted: false,
            notify: Callback {
                function: 0,
                context: 0,
            },
            ends_us: 0,
        }
    }
}

impl MediaState {
    /// The volume the way the mixer wants it, with mute already applied.
    fn gain(&self) -> f32 {
        match self.muted {
            true => 0.0,
            false => self.volume.min(MAX_VOLUME) as f32 / MAX_VOLUME as f32,
        }
    }
}

/// A block of guest memory served as a stream by an `IMemAStream`.
#[derive(Debug, Clone, Copy)]
struct MemStream {
    buffer: u32,
    size: u32,
    position: u32,
    /// Whether the buffer came through `Set` and it is the stream's job to free it. See
    /// [`Machine::stream_call`].
    owner: bool,
}

/// State of an `ISound` object.
#[derive(Debug, Clone, Copy)]
struct SoundState {
    /// `PFNSOUNDSTATUS` registered by `RegisterNotify`.
    notify: Callback,
    /// The five `int8`s of `AEESoundInfo`, kept as the game handed them over.
    info: [u8; 5],
    volume: u16,
}

impl Default for SoundState {
    fn default() -> Self {
        Self {
            notify: Callback {
                function: 0,
                context: 0,
            },
            info: [0; 5],
            volume: AEE_MAX_VOLUME,
        }
    }
}

/// Context that a call into the guest made by us has to give back intact: everything the ARM
/// calling convention says to preserve, plus the argument registers, because the API that was just
/// served has already left its result in `r0`.
const SAVED_REGS: [Reg; 15] = [
    Reg::R0,
    Reg::R1,
    Reg::R2,
    Reg::R3,
    Reg::R4,
    Reg::R5,
    Reg::R6,
    Reg::R7,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::Sp,
    Reg::Lr,
];
/// `QueryInterface` slot in the `IBitmap` vtable.
const BITMAP_QUERY_INTERFACE_SLOT: u32 = 2;
/// `BltIn` slot in the `IBitmap` vtable.
const BITMAP_BLT_IN_SLOT: u32 = 10;
/// Color used as transparent when handing an image with alpha to a surface without alpha.
/// Pure magenta is the traditional choice, precisely because it doesn't show up in real art.
const TRANSPARENT_KEY: u16 = 0xf81f;
/// Size of `AEEDeviceInfo` with the extended fields: up to `dwPlatformID`, in
/// `sdk/inc/AEEShell.h`.
const DEVICE_INFO_SIZE: u32 = 64;
/// `AEE_MAX_FILE_NAME`, from `inc/AEEFile.h`: the cap on a path in BREW's file system.
const AEE_MAX_FILE_NAME: u32 = 64;
/// How many return addresses to collect from the stack on a fault.
const STACK_DEPTH: usize = 24;
/// Maximum depth of reentry into the guest.
const MAX_NESTING: u32 = 4;

/// A call observed by `--probe`: class, object, slot, arguments and the text of each argument that
/// pointed to text.
pub type ProbeCall = (u32, u32, u32, [u32; 4], [Option<String>; 4], u64);

/// Instruction cap per `qsort` comparison. A comparator is a short function; this limit exists so
/// that a broken comparator doesn't hang the whole sort.
const QSORT_BUDGET: u64 = 10_000_000;

/// Instruction cap per row handed to an SQL query's callback. Same idea as [`QSORT_BUDGET`]: the
/// callback is a short function that copies fields.
const SQL_CALLBACK_BUDGET: u64 = 10_000_000;

/// How many rounds of callbacks to deliver before giving up — a callback can queue another, and
/// with no cap a cycle would trap the emulator.
const CALLBACK_ROUNDS: usize = 64;

/// How many slice budgets a stretch of execution may spend before yielding back to the frame
/// loop. See [`Machine::execute`].
const SEGMENT_CAP: u64 = 4;

/// `ICipher1` parameter ids, from `inc/AEEICipher1.h`.
const CIPHER_PARAM_DIRECTION: u32 = 0;
const CIPHER_PARAM_KEY: u32 = 1;
const CIPHER_PARAM_KEY_SIZE: u32 = 2;
const CIPHER_PARAM_IV: u32 = 3;
const CIPHER_PARAM_IV_SIZE: u32 = 4;
const CIPHER_PARAM_PADDING: u32 = 5;
const CIPHER_PARAM_BLOCKSIZE: u32 = 6;
const CIPHER_PARAM_MODE: u32 = 8;
/// `CIPHER_PADDING_NONE`, from `inc/AEEICipher1.h`: the only one that changes what we do — the
/// others pad the block, and we pad with zeros.
const CIPHER_PADDING_NONE: u32 = 0;
/// Block and key size of AES-128, in bytes.
const AES_BLOCK: usize = 16;

/// State of an `IHash` object.
///
/// Only the digest in progress. `GetDigest` writes into the buffer the game provides, so there is
/// nothing to keep in its memory — the field that existed for that came from the wrong signature.
#[derive(Debug, Default)]
struct HashState {
    md5: crate::brew::crypto::Md5,
}

/// State of an `ICipher1`: the configuration that came through `SetParam` and what was left over
/// from one `Process` to the next.
#[derive(Debug, Default)]
struct CipherState {
    key: Option<[u8; AES_BLOCK]>,
    iv: [u8; AES_BLOCK],
    padding: u32,
    /// Bytes that haven't completed a block yet. `ICipher1` is streaming: the game can hand over
    /// any amount and only `ProcessLast` closes whatever is missing.
    pending: Vec<u8>,
}

/// The piece of an `IImage` that `Draw` draws, and how.
#[derive(Debug, Clone, Copy, Default)]
struct ImageClip {
    /// `IPARM_OFFSET`: the corner of the piece, inside the image (or the frame).
    x: i32,
    y: i32,
    /// `IPARM_SIZE`: the size of the piece. Without it, up to the edge of the image.
    size: Option<(i32, i32)>,
    /// `IPARM_ROP`: with `AEE_RO_TRANSPARENT`, the reserved color is not drawn.
    transparent: bool,
}

/// A drawing onto one of the game's own surfaces, waiting for the call boundary.
#[derive(Debug, Clone, Copy)]
struct PendingBlit {
    image: u32,
    target: u32,
    x: i32,
    y: i32,
    src_x: i32,
    src_y: i32,
    width: u32,
    height: u32,
    rop: u32,
    frame: Option<u32>,
}

/// One of our surfaces, already drawn, that needs to be composited by the game's `BltIn`.
#[derive(Debug, Clone, Copy)]
struct PendingSurfaceBlit {
    source: u32,
    target: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rop: u32,
}

/// Whether the method can touch more than one pixel of the surface.
///
/// When the game asks for the `IDIB`, it starts seeing the pixels directly in its own memory, and
/// the surface has to be copied both ways around every call that reads or writes them — 1.2 MB
/// there and back. But most of `IDisplay`, `IGraphics` and `IBitmap` doesn't touch any pixel: they
/// are state settings (color, font, clipping) and queries.
///
/// Pac-Mania showed both extremes. It draws **pixel by pixel** through the API — 1.16 million
/// `DrawPixel` calls in ten frames — and queries the clip rectangle 112 thousand times every
/// fifteen seconds. Charging the whole surface to each of those calls cost tens of gigabytes of
/// copying per second.
///
/// The list below is one of exclusion, not inclusion, on purpose: forgetting a method that draws
/// here would give a wrong pixel, which is hard to notice; leaving out one that doesn't draw only
/// costs the copy, which is visible in the measurement. When in doubt, copy.
fn touches_whole_surface(name: &str) -> bool {
    !matches!(
        name,
        // Common to all of them.
        "AddRef" | "Release" | "QueryInterface"
        // `IBitmap`: these two handle their pixel directly in the game's buffer; the rest are
        // queries.
        | "DrawPixel" | "GetPixel"
        | "RGBToNative" | "NativeToRGB"
        | "GetInfo" | "CreateCompatibleBitmap"
        | "SetTransparencyColor" | "GetTransparencyColor"
        // `IDisplay`: state and measurement.
        | "GetFontMetrics" | "MeasureTextEx" | "SetFont"
        | "SetClipRect" | "GetClipRect"
        | "SetColor" | "GetSymbol"
        | "SetDestination" | "GetDestination" | "GetDeviceBitmap"
        | "SetAnnunciators" | "Backlight" | "MakeDefault" | "IsEnabled" | "NotifyEnable"
        | "SetPrefs"
        // `IGraphics`: only the setter/getter pairs, never the `Draw*` or the `Clear*`.
        | "SetBackground" | "GetBackground" | "GetColor"
        | "SetFillMode" | "GetFillMode" | "SetFillColor" | "GetFillColor"
        | "SetPointSize" | "GetPointSize"
        | "SetClip" | "GetClip" | "SetViewport" | "GetViewport"
        | "SetPaintMode" | "GetPaintMode" | "GetColorDepth"
        | "EnableDoubleBuffer" | "Translate"
        | "SetAlgorithmHint" | "GetAlgorithmHint"
        | "SetStrokeStyle" | "GetStrokeStyle"
        // `IImage`: only `Draw`, `DrawFrame` and `Start` put pixels on the surface.
        | "SetParm" | "Notify" | "Stop" | "HandleEvent" | "SetStream"
    )
}

/// Breaks BREW clock seconds into a `JulianType`.
///
/// The struct is `{ wYear, wMonth, wDay, wHour, wMinute, wSecond, wWeekDay }`, seven `uint16`s,
/// from `AEEStdLib.h`. Month and day start at 1; the weekday starts with **Sunday as 0**, which is
/// BREW's convention.
///
/// The epoch is January 6, 1980, GMT — GPS's, not Unix's. Getting this wrong shifts everything by
/// ten years and the game shows a date that doesn't exist.
fn julian_date(seconds: u32) -> [u16; 7] {
    const BREW_EPOCH_IN_UNIX_DAYS: i64 = 3657; // 1980-01-06 minus 1970-01-01
    let days = seconds as i64 / 86_400;
    let rest = seconds as i64 % 86_400;
    // Day of the week: January 6, 1980 was a Sunday, which is BREW's zero.
    let weekday = (days % 7) as u16;

    // Civil count from the days since the Unix epoch, using Howard Hinnant's algorithm: it shifts
    // the year to start in March, which puts February and the leap day at the end.
    let z = days + BREW_EPOCH_IN_UNIX_DAYS + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u16;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u16;
    let year = (year + i64::from(month <= 2)) as u16;

    [
        year,
        month,
        day,
        (rest / 3600) as u16,
        (rest % 3600 / 60) as u16,
        (rest % 60) as u16,
        weekday,
    ]
}

/// The font the game itself packaged, if there is one.
///
/// The console's came from the firmware, which we don't have. Several modules bring their own —
/// the Z-Wheel packages `tectoy.ttf`, which is the font the store was designed with. Using the
/// game's is more faithful than picking one ourselves, and when there is none the text still
/// doesn't come out, which the report states.
fn module_font(root: &std::path::Path) -> Option<crate::video::font::Font> {
    let mut fonts: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|font_path| {
            font_path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf"))
        })
        .collect();
    // Stable order: two `.ttf` files in the same directory must not give different results between
    // runs because of the order in which the file system lists them.
    fonts.sort();
    let font_path = fonts.first()?;
    let name = font_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::video::font::Font::load(std::fs::read(font_path).ok()?, name)
}

/// The MIDI sample bank, looked up in the device folder respecting the policy.
///
/// If `policy` is `MidiBackend::Wavetable`, returns `None` immediately without touching the disk
/// or loading the SoundFont.
#[cfg(feature = "soundfont")]
fn device_sound_bank(
    device: &std::path::Path,
    policy: crate::audio::MidiBackend,
) -> Option<std::sync::Arc<crate::audio::soundfont::Bank>> {
    if policy == crate::audio::MidiBackend::Wavetable {
        crate::logging!(
            crate::logging::Level::Info,
            "midi",
            "backend pinned to the wavetable; the device's bank is ignored"
        );
        return None;
    }
    let sf2_path = crate::audio::soundfont::first_bank(device)?;
    let bank = crate::audio::soundfont::open(&sf2_path);
    if bank.is_none() && policy == crate::audio::MidiBackend::SoundFont {
        crate::logging!(
            crate::logging::Level::Warning,
            "midi",
            "requested backend is SoundFont, but {} did not open; falling back to the wavetable",
            sf2_path.display()
        );
    }
    bank
}

/// The device's font, for whoever didn't bring their own.
///
/// Without it, every `DrawText` of a game without a `.ttf` went only to the report: the Kingdom
/// Hearts menu drew the five boxes and none of the words inside them.
fn console_font(device: &std::path::Path) -> Option<crate::video::font::Font> {
    // **The frontend's device first.** The previous version only looked at the desktop path, so
    // the font the core installs in its profile — `<device root>/shared/fonts/tectoy.ttf` — was
    // never found: the game went on without drawing text, and the frame came out blank when the
    // screen is a white background plus the words.
    let from_device = device.join("shared").join("fonts").join("tectoy.ttf");
    let font_path = match from_device.is_file() {
        true => from_device,
        false => crate::loader::archive::system_font()?,
    };
    crate::video::font::Font::load(std::fs::read(font_path).ok()?, "tectoy.ttf".into())
}

/// Cuts `rect` by the clip. `None` when nothing is left to draw.
fn clip_rect(clip: Option<Rect>, rect: Rect) -> Option<Rect> {
    // **With no clip set, the clip is the whole screen** — not "nothing gets through". That is
    // what was written here, and the effect is silent: every `IDISPLAY_DrawRect` of a game that
    // doesn't set a clip was discarded. Tekken 2 clears the screen once per frame with a
    // `ClearScreen`, which is exactly a `DrawRect`; the clearing never happened, and its menu
    // appeared over the previous screen's text. `clip_blit`, just below, always handled the same
    // case the right way.
    let Some(clip) = clip else {
        return Some(rect);
    };
    let (x0, y0) = (rect.x.max(clip.x) as i32, rect.y.max(clip.y) as i32);
    let x1 = (rect.x as i32 + rect.width as i32).min(clip.x as i32 + clip.width as i32);
    let y1 = (rect.y as i32 + rect.height as i32).min(clip.y as i32 + clip.height as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0 as i16,
        y: y0 as i16,
        width: (x1 - x0) as i16,
        height: (y1 - y0) as i16,
    })
}

/// An already resolved blit: destination, size and origin in the source.
type Blit = ((i32, i32), (i32, i32), (i32, i32));

/// Cuts a blit by the clip, moving the origin in the source along with it.
///
/// Correcting the origin is what makes the clip show **the right piece** of the source: a cut that
/// only shrank the destination would show the wrong corner of the image, and that is how a font
/// atlas turns into swapped letters.
fn clip_blit(
    clip: Option<Rect>,
    dst: (i32, i32),
    size: (i32, i32),
    src: (i32, i32),
) -> Option<Blit> {
    let Some(clip) = clip else {
        return Some((dst, size, src));
    };
    let (cx0, cy0) = (clip.x as i32, clip.y as i32);
    let (cx1, cy1) = (cx0 + clip.width as i32, cy0 + clip.height as i32);
    let (x0, y0) = (dst.0.max(cx0), dst.1.max(cy0));
    let (x1, y1) = ((dst.0 + size.0).min(cx1), (dst.1 + size.1).min(cy1));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((
        (x0, y0),
        (x1 - x0, y1 - y0),
        (src.0 + (x0 - dst.0), src.1 + (y0 - dst.1)),
    ))
}

/// A call into the guest waiting in the queue.
#[derive(Debug, Clone, Copy)]
struct GuestCall {
    function: u32,
    args: [u32; 4],
}

/// A BREW cooperative thread.
///
/// "Cooperative" is what makes this viable in a single-core emulator: the guest only loses control
/// when it calls `Suspend`, and that is exactly where we save the registers. Resuming is restoring
/// them and continuing from where `Suspend` would return.
#[derive(Default)]
struct ThreadState {
    /// Block of the guest heap that serves as this thread's stack.
    stack: u32,
    /// The `AEECallback` that `GetResumeCBK` returns — it is through it that the game asks to
    /// resume.
    resume_cb: u32,
    /// Saved registers, in the order of [`THREAD_REGS`].
    context: [u32; 14],
    /// Address where execution continues on the next resume.
    resume_pc: u32,
    started: bool,
    /// True between a `Suspend` and the following resume. It is what distinguishes "the thread
    /// yielded control" from "the entry function returned".
    suspended: bool,
    finished: bool,
    exit_code: u32,
    /// Callbacks registered by `Join`, fired when the thread finishes.
    joiners: Vec<(Callback, u32)>,
}

/// A client attribute array, as `glVertexPointer` describes it.
///
/// The data stays in guest memory and is only read at draw time: that is how OpenGL ES works, and
/// it is what lets the game change the contents between two calls without telling anyone.
#[derive(Debug, Default, Clone, Copy)]
struct ArrayPointer {
    /// How many components per element (2, 3 or 4).
    size: u32,
    /// Component type (`GL_FIXED`, `GL_FLOAT`, …).
    kind: u32,
    /// Distance in bytes between two elements; zero means "tightly packed".
    stride: u32,
    address: u32,
    enabled: bool,
    /// Name of the buffer object bound to `GL_ARRAY_BUFFER` **when the pointer was given**, or
    /// zero if there was none.
    ///
    /// The specification is explicit about this, and the difference is observable: what decides
    /// where the array comes from is the binding at the moment of `glVertexPointer`, not at
    /// `glDrawElements`. A game that uploads three meshes binds each buffer, gives its pointers and
    /// only then draws — if we read the current binding at draw time, all three would come from the
    /// last buffer.
    buffer: u32,
}

impl ArrayPointer {
    /// Whether the array is enabled and points somewhere.
    ///
    /// **Address zero only means "no array" with no buffer bound.** With a buffer, the pointer is
    /// an offset inside it, and zero is the most common offset: the SDK's QX engine issues
    /// `glVertexPointer(3, GL_FIXED, 0, 0)` for every mesh. Checking only the address discarded
    /// every buffer draw with the vertices at the start — the scenery and characters of Dragon Vs
    /// Chicken vanished entirely.
    fn in_use(&self) -> bool {
        self.enabled && (self.address != 0 || self.buffer != 0)
    }
}

/// Adapts the guest's registers and stack to the `printf` formatter.
struct GuestArgs<'a, C: CpuBackend> {
    words: Vec<u32>,
    index: usize,
    stack: u32,
    cpu: &'a C,
}

impl<C: CpuBackend> ArgSource for GuestArgs<'_, C> {
    fn next_word(&mut self) -> u32 {
        let value = match self.words.get(self.index) {
            Some(&word) => word,
            // Once r1..r3 are used up, the rest comes from the stack, in order.
            None => {
                let offset = (self.index - self.words.len()) as u32 * 4;
                self.cpu.read_u32(self.stack + offset).unwrap_or(0)
            }
        };
        self.index += 1;
        value
    }

    fn read_cstring(&mut self, addr: u32) -> String {
        self.cpu.read_cstring(addr, MAX_STRING)
    }

    fn read_wide_string(&mut self, addr: u32) -> String {
        let mut units = Vec::new();
        for i in 0..MAX_STRING as u32 {
            let Ok(bytes) = self.cpu.read_u32(addr.wrapping_add(i * 2) & !3) else {
                break;
            };
            let unit = match (addr.wrapping_add(i * 2)) & 2 {
                0 => bytes as u16,
                _ => (bytes >> 16) as u16,
            };
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        String::from_utf16_lossy(&units)
    }
}

pub struct Machine<C: CpuBackend> {
    cpu: C,
    module: LoadedModule,
    heap: Heap,
    /// What the emulator allocates for itself in the guest's space: see [`loader::INTERNAL_BASE`].
    internal_heap: Heap,
    objects: ObjectStore,
    /// What the game asked `IFileMgr`, with the path and the return value.
    fs_log: std::collections::VecDeque<String>,
    /// How many times the game asked for each class, known or not.
    requested_classes: BTreeMap<u32, u32>,
    /// ClassIDs the game asked for that we don't know how to create — the list of what is missing.
    unknown_classes: BTreeSet<u32>,
    /// The URLs the game requested through `IWeb`, for the report.
    web_requests: BTreeSet<String>,
    /// The live collections, each with its items and where the cursor is.
    collections: HashMap<u32, (Vec<u32>, usize)>,
    /// The text the game asked to draw, with the moment and the position, for the report.
    ///
    /// It is what answers "what is written on the screen right now?": the *pending* text list only
    /// exists when the font is missing, and a game that draws several warning screens over the run
    /// shows up in the report as a soup of messages with no owner.
    drawn_texts: std::collections::VecDeque<(u32, i32, i32, String)>,
    /// Allocations the heap refused, by requested size and by who asked.
    ///
    /// BREW's `malloc` returns zero when it doesn't fit, and a game that doesn't distinguish
    /// "doesn't fit" from any other failure shows a generic message — that is how Double Dragon
    /// ended up on the "Memory is insufficient" screen with nothing in the report pointing to the
    /// refused request.
    refused_allocs: BTreeSet<(u32, u32)>,
    /// `IHeap::CheckAvail` questions answered with "doesn't fit", for the same reason.
    refused_checks: BTreeSet<(u32, u32)>,
    /// The live system fonts, by `IFont` object, with the metrics of the class that created them.
    fonts: std::collections::HashMap<u32, crate::machine::font::Metrics>,
    /// The open SQLite databases, by `ISQLDatabase` object.
    databases: HashMap<u32, crate::brew::sql::Database>,
    /// ClassIDs that `--probe` says to serve with an observation object.
    probe_classes: BTreeSet<u32>,
    /// Canned answers for probe slots: `(class_id, slot) -> value`.
    probe_answers: HashMap<(u32, u32), u32>,
    /// What time it was when the machine was created, in seconds of the BREW epoch.
    epoch_seconds: u32,
    /// Which class each live probe object belongs to.
    probe_objects: HashMap<u32, u32>,
    /// What was called on each probe: `(class_id, object, slot)` and the arguments of the first
    /// time. The object is part of the key because it is what reveals the **family**: the manager
    /// returns a database, the database returns a query, and without telling the three apart the
    /// sequence becomes a list of slots with no owner.
    probe_log: Vec<ProbeCall>,
    /// Calls in which the `this` pointer was not the expected object.
    suspicious_objects: BTreeSet<(u32, u32)>,
    /// Calls in which a guest pointer did not point to mapped memory.
    bad_pointers: BTreeSet<String>,
    /// APIs we serve based on a hypothesis, not on documentation. It goes into the report so nobody
    /// mistakes a guess for known behavior.
    assumptions: BTreeSet<&'static str>,
    /// `IGraphics` state.
    graphics: GraphicsState,
    /// The game's file system.
    vfs: Vfs,
    /// Open files, indexed by the guest's `IFile` pointer.
    open_files: HashMap<u32, OpenFile>,
    /// Last file error, returned by `IFILEMGR_GetLastError`.
    file_error: u32,
    /// The bytes already handed to each image decoder, and the result when it closes.
    decoders: HashMap<u32, DecoderState>,
    /// Which decoder each `IForceFeed` belongs to — `QueryInterface` returns a separate object,
    /// because the two interfaces have different methods in the same slot.
    feeds: HashMap<u32, u32>,
    /// The objects of the console's graphics extensions, created the first time they are asked for.
    surface_manip: u32,
    imageon_ext: u32,
    /// The `IGLES11Ext` object, created the first time it is asked for. See slot 2 of the EGL
    /// vtable.
    gles11_ext: u32,
    /// The `IGLES10Ext` object, for the same reason.
    gles10_ext: u32,
    /// The `IEGLGetPowerLevel` object.
    egl_get_power_level: u32,
    /// The `IEGLOESSwapInterval` object.
    egl_oes_swap_interval: u32,
    /// The `IEGLGetColorBuffer` object.
    egl_get_color_buffer: u32,
    /// The `IGLES11ExtPak` object.
    gles11_ext_pak: u32,
    /// The rectangle the game draws in, when it declares it through `SetSurfaceScale`. It is worth
    /// more than deducing it from the viewport: here the game **says** the size.
    scale_source: Option<(i32, i32)>,
    /// The preferences stored by `ISHELL_SetPrefs`, by class and version.
    prefs: HashMap<(u32, u16), Vec<u8>>,
    /// The listing in progress for each `IFileMgr`. BREW keeps that state inside the manager
    /// itself, and two managers enumerate different directories at the same time.
    enumerations: HashMap<u32, std::collections::VecDeque<String>>,
    /// Files the game tried to open that don't exist — a good sign of a missing asset.
    missing_files: BTreeSet<String>,
    /// Signals the game registered for input events, by event type **and port**.
    ///
    /// The port is part of the key because registration is done on the device object, one per
    /// connected controller: with two, the second registration overwrote the first and every event
    /// woke up controller two's callback. The game then asked the wrong device, found no event at
    /// all and controller one did nothing — that was Treino Cerebral stuck on "press button 1" with
    /// both ports connected.
    input_signals: BTreeMap<(&'static str, usize), u32>,
    /// Callback of each live signal, indexed by the guest's object pointer.
    signals: HashMap<u32, Callback>,
    /// Signals fired and not yet delivered to the guest.
    pending_signals: Vec<Callback>,
    /// Current state of each port. See [`crate::input::PORTS`].
    pads: [Pad; input::PORTS],
    /// Presses and releases not yet read by the game, per port, in the order they happened.
    pad_events: [std::collections::VecDeque<(usize, bool)>; input::PORTS],
    /// Which device the console sees on each port, and whether it is connected.
    ///
    /// It is what `GetConnectedDevices` answers. A disconnected port is not enumerated — and that
    /// is how you test a game that behaves differently with two controllers.
    ports: [Option<crate::input::bindings::Device>; input::PORTS],
    /// The port of each `IHIDDevice` the game created, by object address.
    device_ports: HashMap<u32, usize>,
    /// The acceleration of each port, in g, in the Boomerang's frame of reference. Only ports with
    /// a Boomerang use it.
    motion: [[f32; 3]; input::PORTS],
    /// The Boomerang receiver's packet counter. See [`Machine::boomerang_packet`].
    boomerang_sequence: u8,
    last_boomerang_report_us: u64,
    last_boomerang_packet_us: u64,
    /// How many times the game started and finished a motion calibration, as far as can be seen
    /// from outside. See [`Machine::calibration`].
    calibrations: (u32, u32),
    /// Keys pressed and not yet delivered, as `(AVK code, pressed)`.
    key_events: std::collections::VecDeque<(u32, bool)>,
    /// The last presses delivered, for the report.
    ///
    /// The queue above is consumed by the game and disappears; this one stays. It exists because an
    /// input problem is indistinguishable from an interpretation problem without seeing what
    /// arrived: a menu that moves two places per press may be the game counting two channels, or
    /// the emulator sending two events, and only the log separates the two cases.
    pad_log: std::collections::VecDeque<(u32, usize, usize, bool)>,
    /// What the game did with sound, in order: `(instant, object, call)`.
    ///
    /// It exists for the same reason as [`Machine::pad_log`]: "there's no sound in the match" shows
    /// up in no report when every call answers success. Identical consecutive calls become one line
    /// with the count, or a `GetState` per frame would push everything else out.
    media_log: std::collections::VecDeque<(u32, u32, String, u32)>,
    /// The current applet, returned by `GetAppInstance`.
    current_applet: u32,
    /// Seed of the pseudorandom generator — fixed, so the same session repeats identically.
    random_state: u32,
    /// When execution started, for the time helpers.
    /// The guest's virtual clock, in milliseconds.
    ///
    /// Deliberately not the host's clock: the game asks for a 33 ms timer expecting a frame, and
    /// the frame loop is what decides how much time has passed. That way the game's pace doesn't
    /// depend on how fast the emulator manages to interpret the instructions, and the same run
    /// always gives the same result.
    clock_us: u64,
    spin_polls: u32,
    /// Moment of the next vertical retrace, in microseconds.
    next_vsync_us: u64,

    /// Record of every call in the order it happened, when tracing is on.
    trace: Vec<String>,
    tracing: bool,
    /// When present, only calls whose name contains this piece go into the trace.
    trace_filter: Option<String>,
    /// What the game wrote via `DBGPRINTF`, in the order it appeared and with how many times each
    /// message repeated — a loop can generate thousands of identical lines.
    debug_output: Vec<(String, u64)>,
    /// Where each line is in `debug_output`, to group without scanning the list on every call.
    debug_index: HashMap<String, usize>,
    /// Drawing surfaces, indexed by the guest's `IBitmap` pointer.
    bitmaps: HashMap<u32, Framebuffer>,
    /// `r0..r11` at the moment of the last memory fault, for the report.
    fault_regs: [u32; 12],
    /// Return addresses seen on the stack of the last fault.
    fault_stack: Vec<u32>,
    /// Timers armed by the game with `ISHELL_SetTimer`.
    timers: Vec<Timer>,
    /// ClassID of the applet the module instantiated, to answer `ISHELL_GetClassItemID`.
    applet_class: u32,
    installed_applets: HashSet<u32>,
    /// The module id of each installed applet — the name of its `.mif`, without extension. It is
    /// what `ISHELL_EnumNextApplet` delivers in `pszMIF`.
    installed_modules: Vec<(u32, String)>,
    /// Where `EnumNextApplet` is in the list, and the `pszMIF` strings already placed in the guest.
    applet_enumeration: usize,
    mif_in_guest: HashMap<u32, u32>,
    /// Instruction budget of the stretch being executed.
    ///
    /// Kept because the dispatch of an API call doesn't receive it, and there is one case in which
    /// it needs to **reenter the guest**: the class provided by an extension module, which only the
    /// extension's `IModule::CreateInstance` knows how to create. See
    /// [`Machine::create_via_extension`].
    budget: u64,
    /// The `IModule*` of each extension. `None` = not loaded yet; `Some(0)` = we tried and it
    /// didn't work, and it isn't tried again on every request.
    ext_modules: Vec<Option<u32>>,
    pending_launch: Option<u32>,
    /// The screen writes right after the last GL frame. See [`Machine::gpu_frame`].
    gl_frame_writes: Option<u64>,
    wheel_boot_skipped: bool,
    /// How the Z-Wheel reads `tectoy.cfg`. See [`Machine::configure_z_wheel`].
    z_wheel: crate::config::ZWheel,
    /// The applet asked to close with `ISHELL_CloseApplet`. See [`Machine::close_requested`].
    applet_closed: bool,
    /// `(root, form)` waiting for the active notice. See [`Machine::deliver_activation`].
    pending_activation: Option<(u32, u32)>,
    /// The screens presented by `IDISPLAY_Update` within the same pass of the loop. See
    /// [`Machine::take_update_frame`].
    update_frames: std::collections::VecDeque<Framebuffer>,
    updates_in_pass: usize,
    /// Current depth of reentry into the guest.
    nesting: u32,
    /// The guest stretch the instruction cap interrupted, when it was the outermost stretch: where
    /// to continue and the registers at that time. See [`Machine::resume_segment`].
    interrupted_segment: Option<(u32, [u32; 15])>,
    /// Game surfaces waiting to be queried about where their pixels live.
    pending_probes: Vec<u32>,
    /// Drawings that need to go through the `BltIn` of a game surface.
    pending_blits: Vec<PendingBlit>,
    /// 2D primitives drawn on temporary bitmaps for the game's `BltIn`.
    pending_surface_blits: Vec<PendingSurfaceBlit>,
    /// Surfaces already queried — the answer doesn't change, and asking again would cost an entry
    /// into the guest on every `SetDestination`.
    probed: HashSet<u32>,
    /// Decoded images, by `IImage` object.
    /// The loaded images, under `Rc` because drawing is the hot path: Pac-Mania calls
    /// `IIMAGE_Draw` tens of thousands of times per frame.
    images: HashMap<u32, std::rc::Rc<DecodedImage>>,
    /// The already materialized surface of each image, for `IPARM_GETBITMAP`.
    image_bitmaps: HashMap<u32, u32>,
    /// How much **real** time each API method cost, when diagnostics ask for the measurement.
    ///
    /// The guest profile says where the game spends its time; this one says where the emulator
    /// spends ours. Without it, a method that costs half a millisecond per call hides behind an
    /// average: what shows up is "8 µs per API call", not "`IIMAGE_Draw` alone is two thirds of the
    /// dispatch".
    /// Cost per API method, **sampled**: `(summed nanoseconds, samples)`.
    ///
    /// See [`Machine::enable_api_profile`] for why the measurement is sampled: this machine's clock
    /// costs more than a microsecond per read, and the profile read the clock twice per call —
    /// which made the instrument charge more than the method being measured and make the run 42%
    /// more expensive. With one call in every `PROFILE_SAMPLE_EVERY`, the same amount is estimated
    /// for 1/64 of the price, and the call count stays exact in [`Machine::call_log`].
    api_time: HashMap<(u32, u32), (u64, u64)>,
    /// How many API calls have gone through here, to pick the ones that get timed.
    api_calls: u64,
    /// How many times a game read the position and found some axis **off center**.
    ///
    /// The `GetPositionState` count doesn't say what the game read: there are arcade ports that
    /// poll the axis every frame (2,300 times in forty seconds) and never the button channel, and
    /// reading the axis centered is what it does 100% of the time without pressing anything. This
    /// counter is what separates "the game reads the axis" from "the axis **reached** the game",
    /// and it is what can be measured without looking at the screen — the d-pad mirror depends on
    /// it.
    off_center_axes: u64,
    /// **Which** axes were seen off center, one bit per axis. See the counter above.
    ///
    /// It is what answers "did the d-pad reach the **left** stick?", which is the question of issue
    /// #39: `X` and `Y` are the left stick's, `Z` and `RZ` the right one's, and a game that only
    /// reads the left one has to show up with the first two bits, and only those.
    off_center_axes_mask: u32,
    /// Cost of one clock read on this machine, to subtract it from the cost profile.
    clock_ns: u64,
    profiling_api: bool,
    /// `IIMAGE_Notify` callback, per object.
    image_notify: HashMap<u32, Callback>,
    /// The `AEEImageInfo` each image's `PFNIMAGEINFO` receives, in the game's heap.
    image_info: HashMap<u32, u32>,
    /// The rectangle and operation `IIMAGE_SetParm` left for the next `Draw` calls.
    image_clips: HashMap<u32, ImageClip>,
    /// Memory blocks presented as a stream.
    streams: HashMap<u32, MemStream>,
    /// State of each live `ISound`.
    sounds: HashMap<u32, SoundState>,
    /// State of each live `ICipher1`.
    ciphers: HashMap<u32, CipherState>,
    /// What the generic collection's `Definir` received: `(object, id) -> bytes`.
    collection_params: HashMap<(u32, u32), Vec<u8>>,
    /// The items and the releaser of each live list. See [`Interface::Vector`].
    vectors: HashMap<u32, (Vec<u32>, u32)>,
    /// The bytes of each live `ISource`.
    sources: HashMap<u32, Vec<u8>>,
    /// The page handed to each HTML widget, raw as it came from the `ISource`. See the
    /// `AdicionarFilho` of [`Machine::widget_call`].
    html_pages: HashMap<u32, Vec<u8>>,
    /// How many lines each HTML widget scrolled, and how far it can scroll (measured when
    /// painting).
    html_scroll: HashMap<u32, usize>,
    html_max_scroll: HashMap<u32, usize>,
    /// Arrows whose press scrolled an HTML panel: their release doesn't go to the game either.
    scroll_keys: std::collections::HashSet<u32>,
    /// The state of each live `IPeek`.
    peeks: HashMap<u32, Peek>,
    /// The items of each live `IConfig`, by object: item number -> bytes.
    config_items: HashMap<u32, HashMap<u32, Vec<u8>>>,
    /// The state of each live widget. See [`Widget`].
    widgets: HashMap<u32, Widget>,
    /// How many times each `(widget class_id, selector)` pair went through the accessor. See the
    /// `Acessador` arm of `widget_call`: it is what answers "what behavior does this proprietary
    /// class expect" from usage, without a probe and without disassembly. Turned on by
    /// [`Machine::enable_widget_census`].
    selectors_per_class: std::collections::BTreeMap<(u32, u32), u32>,
    /// Whether the per-class accessor census is on. Off, so as not to disturb the baseline.
    widget_census: bool,
    /// The APIs that were missing, with whoever called them. See the dispatch's `None`.
    missing_apis: BTreeSet<String>,
    /// Invalid accesses that happened inside a callback and didn't stop the game.
    swallowed_failures: BTreeSet<String>,
    /// GL calls answered with success without doing anything.
    ignored_gl: BTreeSet<&'static str>,
    /// `glPixelStorei(GL_UNPACK_ALIGNMENT, n)`: with how many bytes each texture row starts
    /// aligned in guest memory. The OpenGL default is 4.
    ///
    /// It lives in `Machine`, not in the rasterizer state, because it is a property **of the
    /// game's memory** — the one that reads the texels is `machine/gl.rs`, before handing them to
    /// any rasterizer.
    unpack_alignment: u32,
    /// What the game handed to `ICipher1`, in the clear, before it was encrypted.
    plaintexts: std::collections::VecDeque<Vec<u8>>,
    /// Whether the module bridge may deliver the response to the game. See [`crate::bridge`].
    bridge: bool,
    /// A response waiting for the call boundary: `(object, anchor, state)`.
    pending_response: Option<(u32, u32, u32)>,
    /// The object that still needs to learn the stream has ended, at the next boundary.
    pending_end: Option<u32>,
    /// The responses the bridge managed to deposit in the game's memory.
    delivered: Vec<String>,
    /// Where to redirect connections, when you want a server other than the one in the address.
    network_to: Option<String>,
    /// Whether the emulator may talk to the network.
    ///
    /// Giving network access to a binary of external origin is a design decision, so it is
    /// explicit and shows up in the report. It stays on because that is what the stack exists for,
    /// and `--no-network` turns it off.
    network: bool,
    /// The body of the last response received.
    web_response: Vec<u8>,
    /// State of each live `IHash`.
    hashes: HashMap<u32, HashState>,
    resources: crate::loader::resfile::ResCache,
    unzips: HashMap<u32, UnzipState>,
    /// Guest callbacks already fired and not yet delivered.
    ///
    /// A single queue because they all have the same shape — a function address and up to four
    /// arguments — and because none of them may run in the middle of a call's dispatch.
    pending_calls: Vec<GuestCall>,
    /// The `IMedia` notices not yet delivered — object, command, status and the handler from when
    /// the notice was born. They go out on the loop's pass, not on the call's exit: see
    /// [`Machine::notify_media`].
    media_notify_queue: Vec<(u32, u32, u32, Callback)>,
    /// The images with `PFNIMAGEINFO` not yet delivered. They also go out on the loop's pass: see
    /// [`Machine::notify_image`].
    image_notify_queue: Vec<u32>,
    /// The `IMedia`s that play PCM generated by the game, by object. See [`PcmStream`].
    pcm_streams: HashMap<u32, PcmStream>,
    /// The `IMedia`s that play a WAVE by reading the game's buffer. See [`LiveBuffer`].
    live_buffers: HashMap<u32, LiveBuffer>,
    /// The buffer in the guest where `ISource::Read` writes the samples.
    stream_buffer: u32,
    /// The block where each `AEEMediaCmdNotify` is assembled at delivery time. One is enough: the
    /// notices go out one at a time, and the handler only reads it while it runs.
    media_notify_block: u32,
    /// Resources that **some** file provided. See [`Machine::missing_files`].
    resources_read: BTreeSet<u16>,
    /// Whether the widget tree has already been dumped to the serial.
    widgets_dumped: bool,
    /// Widget creation counter. See [`Machine::current_form`].
    next_serial: u64,
    /// The form that is painted on the surface right now. See [`Machine::paint_widgets`].
    painted_form: u32,
    /// When the widgets were last drawn, on the virtual clock.
    ///
    /// The one that draws an `OwnerDrawWidget` is the game, and in the Z-Wheel that callback is a
    /// software compositor — the loop at `0x1fc70`, which alone accounts for 63% of all guest
    /// instructions. The console asks for a redraw when the screen needs it; we asked on every
    /// pass of the event loop, which is ~128 per virtual second for ~67 frames presented.
    last_draw_us: u64,
    /// Serial capture, when on. See [`Machine::enable_serial`].
    serial: Option<std::io::BufWriter<std::fs::File>>,
    /// Last EGL error, returned by `eglGetError`.
    egl_error: u32,
    /// Live EGL surfaces, with the dimensions of each.
    egl_surfaces: HashMap<u32, (u32, u32)>,
    /// Whether the initial viewport has already been set to the pbuffer size. See
    /// `eglMakeCurrent`.
    egl_initial_viewport: bool,
    /// Where the color buffer's pixels are visible to the game, and at what size.
    ///
    /// Reserved the first time `eglGetColorBufferQUALCOMM` is called, and reused afterwards: the
    /// surface region has no way to give back what it already handed out, and the Z-Wheel asks for
    /// the buffer once per frame.
    egl_color_buffer: (u32, usize),
    /// The already converted bytes, reused from one call to the next.
    egl_color_bytes: Vec<u8>,
    /// Dimensions and working memory of the buffer exposed to the guest. It is writable:
    /// the Z-Wheel copies the background to this address before drawing the stage.
    egl_color_dimensions: Option<(usize, usize)>,
    egl_color_readback: Vec<u8>,
    /// Next free surface or context identifier.
    egl_next_handle: u32,
    /// How many times the game presented a frame with `eglSwapBuffers`.
    egl_swaps: u32,
    /// How many `glClear` calls cleared the color. See [`Machine::gl_swaps`].
    gl_clears: u32,
    /// The texture and buffer names in use in OpenGL ES. See [`gl::GlNames`].
    texture_names: gl::GlNames,
    buffer_names: gl::GlNames,
    /// The `IGLES11` object, created on demand by EGL's `QueryInterface`.
    gles_object: u32,
    /// Current EGL surface and context.
    egl_surface: u32,
    egl_context: u32,
    /// Playback state of each live `IMedia`.
    media: HashMap<u32, MediaState>,
    /// The sounds handed to the `IMedia`s, already read, by content key. See
    /// [`MediaPayload`].
    media_payloads: HashMap<u64, MediaPayload>,
    /// Where the sound goes, when there is somewhere for it to go.
    audio: Option<crate::audio::Mixer>,
    /// The last frame the game presented, in RGB565 words and at screen size.
    ///
    /// Kept at `eglSwapBuffers` because that is where the frame is ready: reading the buffer at the
    /// end of the run catches the drawing halfway, almost always right after the `Clear`. Words
    /// avoid converting RGB565 bytes back to `u16` just to update the screen.
    gl_last_frame_words: Vec<u16>,
    /// Whether there is an OpenGL frame waiting to be brought to the CPU screen.
    ///
    /// See [`Machine::present_gl`]: painting the queue is rasterizing, and reading the frame back
    /// is something else — that only happens when 2D drawing on top or a pixel read needs it.
    gl_frame_pending: bool,
    /// A state was just restored, and the card's texture doesn't have its frame. See
    /// [`Machine::gpu_frame`].
    gpu_without_frame: bool,
    /// How many times the pending frame was brought to the CPU screen. See
    /// [`Machine::gl_frame_materializations`].
    gl_materializations: u32,
    /// OpenGL ES state and buffers.
    ///
    /// Dynamic dispatch because the rasterizer is swappable: the whole boundary is in
    /// [`Rasterizer`], and only this module touches it. The per-call indirection is noise next to
    /// what each call does — the Z-Wheel issues nineteen thousand GL calls in thirteen virtual
    /// seconds.
    gl: Box<dyn Rasterizer>,
    /// Client arrays: position, color and texture coordinate.
    gl_vertices: ArrayPointer,
    gl_colors: ArrayPointer,
    gl_texcoords: ArrayPointer,
    /// The coordinate array of texture unit 1.
    gl_texcoords1: ArrayPointer,
    /// The normal array of `glNormalPointer`. Always three components — the function doesn't even
    /// take a size.
    gl_normals: ArrayPointer,
    /// Contents of each live buffer object, by the name `glGenBuffers` handed out.
    ///
    /// It stays on the host, not in the game's memory, because that is where a real driver keeps
    /// it: the game only sees the buffer by name, and after `glBufferData` it has the right to
    /// reuse the pointer it passed. Keeping our own copy is what makes that right hold.
    gl_buffers: HashMap<u32, Vec<u8>>,
    /// Name bound to `GL_ARRAY_BUFFER`, or zero for "the pointers are in the game's memory".
    gl_array_buffer: u32,
    /// Name bound to `GL_ELEMENT_ARRAY_BUFFER`. Here the current binding **is** the one that
    /// counts: `glDrawElements` decides at draw time whether the last argument is a pointer or an
    /// offset inside the buffer.
    gl_element_buffer: u32,
    /// The normal of `glNormal3x`, used when there is no array. The OpenGL default is `(0, 0, 1)`.
    gl_current_normal: [f32; 3],
    /// Constant strings already copied into guest memory, indexed by text.
    interned: HashMap<&'static str, u32>,
    /// State of each live `IThread`.
    threads: HashMap<u32, ThreadState>,
    /// Resume callback of each thread, indexed by the address of the `AEECallback`.
    resume_callbacks: HashMap<u32, u32>,
    /// Threads ready to run at the next boundary between API calls.
    pending_threads: Vec<u32>,
    /// Outcome that interrupted a callback or a thread, for the frame loop to report.
    stalled: Option<Outcome>,
    /// The running thread, if any — resuming a thread from inside itself would be reentrancy, not
    /// concurrency.
    current_thread: Option<u32>,
    /// The heap block where `sp` was last time, as `(start, size)`: the stack the game allocated
    /// for itself. Kept because finding it walks the whole heap, and `sp` almost never changes
    /// block. See [`Machine::spend_stack`].
    stack_in_heap: Option<(u32, u32)>,
    /// Guest pixel buffer of each surface exposed as `IDIB`.
    dib_buffers: HashMap<u32, u32>,
    /// How many bytes the published buffer of each `IDIB` has.
    ///
    /// It exists because an object's address **gets reused**: once the previous one is freed, the
    /// next bitmap is born in the same place, and with another size. Without the capacity there is
    /// no way to decide between reusing the buffer and reserving another.
    dib_capacity: HashMap<u32, u32>,
    /// The `IDIB`s whose buffer still holds the pixels **of the previous object** at that address.
    ///
    /// The buffer is reused when a new bitmap is born where another died, but the bytes in it
    /// belong to the dead one. While the address is here, what counts is the host surface, and
    /// importing the buffer would bring the old image over the new one. It leaves here when the
    /// host publishes its own pixels.
    ///
    /// Before, this went unnoticed because the write watch said "the game didn't touch it" and the
    /// import didn't happen. Dynarmic has no watch and always answers "dirty", which is only safe
    /// if the buffer is never behind the host — and here it was: the letters of Tekken 2 and
    /// Kingdom Hearts turned into blocks, with the glyph sheet replaced by the image decoded before
    /// it.
    inherited_dibs: HashSet<u32>,
    /// PNG decoder bitmaps whose `IDIB` shows the pixels in the PNG's own format (24-bit RGB or
    /// 32-bit RGBA), not in RGB565: the buffer and its capacity. They stay out of synchronization —
    /// our RGB565 copy serves the blits, and the buffer is read-only for the game.
    decoder_dib: HashMap<u32, (u32, u32)>,
    /// DIBs from `IDISPLAY_CreateDIBitmap`, whose header and pixels belong to the game, and the
    /// pixel buffer we allocated for each (zero when it was created without a size). They stay
    /// out of the RGB565 synchronization: our copy is rebuilt from their header on every blit —
    /// see [`guest_dib`].
    guest_dibs: HashMap<u32, u32>,
    /// The [`Framebuffer::generation`] of each surface the last time the game's buffer and our copy
    /// were equal — from then on, only its dirty box needs to go to the game.
    ///
    /// Without this, **every** call that draws rewrote all the exposed surfaces whole into the
    /// game's memory, changed or not. Pac-Mania makes 168 thousand `IIMAGE_Draw` calls in five
    /// virtual seconds: 95% of the API time — 30 seconds of wall clock — was that copy.
    published_dib: HashMap<u32, u64>,
    /// The surface region, with the same allocator as the game's heap. See
    /// [`Machine::reserve_surface`].
    surfaces: Heap,
    /// Widgets whose handler is receiving a notice right now. See [`Machine::notify_widget`].
    notifying_widgets: std::collections::HashSet<u32>,
    /// Color treated as transparent on each surface.
    transparency: HashMap<u32, u16>,
    /// The target bitmap of each `ITransform` the game asked for through `QueryInterface`.
    transforms: HashMap<u32, u32>,
    /// The state of each `IValueModel` (`0x01028e3c`).
    value_models: HashMap<u32, ValueModel>,
    /// The mode written to the system control (`0x01006c02`) by slot 3.
    system_mode: u32,
    /// The bitmap of each canvas asked for through `QueryInterface`.
    canvases: HashMap<u32, u32>,
    /// The screen surface — BREW's "device bitmap". Zero while nobody has asked for it.
    device_bitmap: u32,
    /// Where `IDisplay` draws. Normally it is the screen.
    display_target: u32,
    /// The `IDisplay` clip rectangle. `None` is the whole surface, which is BREW's default and what
    /// holds before the first `SetClipRect`.
    clip: Option<Rect>,
    /// The `IGraphics` clip, which is another object and has its own. It applies in `clip` only
    /// during an `IGraphics` call — see [`Machine::graphics_call`].
    ///
    /// Left out of the save state on purpose: the game that uses `IGraphics` redoes the clip every
    /// frame, and storing it would change the format for a one-frame difference.
    graphics_clip: Option<Rect>,
    /// The screen, for when there is no device bitmap yet.
    screen: Framebuffer,
    /// Active `IDisplay` colors, indexed by `AEEClrItem` (`CLR_USER_TEXT` = 1 onwards).
    colors: [Rgb; CLR_COUNT],
    /// Texts the game asked to draw. We don't have a font to rasterize them yet, so they are
    /// recorded here instead of vanishing.
    pending_text: Vec<String>,
    /// The game's own font, when it packages one.
    font: Option<crate::video::font::Font>,
    /// The sample bank MIDI is played with, when the device has one.
    ///
    /// `None` is the common case: the bank doesn't come built in (measured: 32,319,396 B and
    /// +64 MiB of RSS at load), so whoever hasn't installed an `.sf2` keeps hearing the wavetable.
    #[cfg(feature = "soundfont")]
    sound_bank: Option<std::sync::Arc<crate::audio::soundfont::Bank>>,
    /// How many times each method was called — the picture of what the game uses.
    calls: BTreeMap<(u32, u32), u64>,
    /// Total calls served since the game opened. The cap looks at the difference within a stretch
    /// ([`MAX_CALLS`]); the total serves the statistics.
    calls_total: u64,
    /// Whether the **current** frame should skip drawing — 3D and screen clearing, not the logic.
    ///
    /// **The frontend decides, frame by frame, not the engine.** The policy (fixed/automatic, and
    /// what "automatic" measures) depends on things only the frontend knows: whether audio is about
    /// to run short, or the frame count Libretro asked for. The engine only receives the decision
    /// already made, and applies it in the only place that matters: the two points that touch the
    /// rasterizer — `gles_draw` and `IGL`'s `Clear` — without touching anything the game sees.
    /// A real game doesn't know whether its pixel reached the screen; no hardware reports that.
    skip_draw: bool,
    /// The game has already called `glReadPixels` in this session.
    ///
    /// Frameskip is not safe after that: skipping a draw and returning the previous screen to the
    /// guest's memory stops being just "losing an image" and can change its logic (Crash uses the
    /// read to check the scene). See `gles_read_pixels`.
    gl_pixels_read: bool,
}

/// Whether the on-card rasterizer was requested.
///
/// `fallback` is what the configuration says. The environment variable has the final word because
/// it is how the measurement picks the backend on the command line, where there is no settings
/// screen.
///
/// An empty value counts as off: a `ZEEBX_GPU=` must not turn on the card without it being asked
/// for — that is how a battery of four measurements came out entirely on the card when half
/// should have been in software.
fn gpu_requested(fallback: bool) -> bool {
    match std::env::var("ZEEBX_GPU") {
        Ok(value) => !value.is_empty() && value != "0",
        Err(_) => fallback,
    }
}

/// The rasterizer the construction adopts. Whoever has a configuration swaps it later, with
/// `use_gpu`.
///
/// There is no context to borrow here: whoever builds the machine directly is the command line,
/// which has no window. The windowed path swaps later, with its own context.
fn rasterizer(width: usize, height: usize) -> Box<dyn Rasterizer> {
    match gpu_requested(false) {
        true => on_gpu(width, height, None),
        false => Box::new(GlState::new(width, height)),
    }
}

/// The card when it opens, software when it doesn't.
///
/// The fallback **is not error handling**, it is a normal path: on a terminal with no reachable
/// EGL there is no card to use, and the emulator has to run anyway. The reason is stated once,
/// because an emulator that silently runs differently from what was asked is worse than a slow
/// one.
fn on_gpu(
    width: usize,
    height: usize,
    context: Option<std::sync::Arc<glow::Context>>,
) -> Box<dyn Rasterizer> {
    // With the `gl` feature, the card rasterizer exists — and it does **not** create a context:
    // the caller hands over its own. Without it, software is the only route, which is the core's
    // case when the frontend offers no context.
    #[cfg(feature = "gl")]
    {
        match crate::video::gpu::GpuState::create(width, height, context) {
            Ok(gpu) => {
                crate::logging!(
                    crate::logging::Level::Info,
                    "gl",
                    "card rasterizer created at {width}x{height}"
                );
                return Box::new(gpu);
            }
            // **A warning, not info.** Falling back to software is not a configuration detail: it
            // is drawing getting slower and different, and it is the first thing to look at when
            // someone says the handheld is slow.
            Err(reason) => {
                crate::logging!(
                    crate::logging::Level::Warning,
                    "gl",
                    "the card rasterizer did not come up ({reason}); continuing on the processor"
                );
            }
        }
    }
    let _ = context;
    Box::new(GlState::new(width, height))
}

impl<C: CpuBackend> Machine<C> {
    /// `root` is the module's directory — the root of the file system the game sees.
    /// Swaps the rasterizer, **before the game starts**.
    ///
    /// The session calls this right after building the machine, with what the configuration asks
    /// for. Swapping after the game has drawn would lose the accumulated GL state — matrices,
    /// textures, lighting —, so this is the only moment the swap is safe.
    /// `context` is the window's, when there is one. See [`crate::video::gpu::GpuState::create`].
    pub fn use_gpu(
        &mut self,
        enabled: bool,
        context: Option<std::sync::Arc<glow::Context>>,
    ) {
        let (width, height) = self.gl.frame_size();
        self.gl = match gpu_requested(enabled) {
            true => on_gpu(width, height, context),
            false => Box::new(GlState::new(width, height)),
        };
    }

    /// Starts counting the `IDISPLAY_Update` calls of one pass of the loop.
    pub fn begin_pass(&mut self) {
        self.updates_in_pass = 0;
    }

    /// Closes the pass. With a single `Update`, the final screen is enough and nothing is kept.
    pub fn end_pass(&mut self) {
        if self.updates_in_pass <= 1 {
            self.update_frames.clear();
        }
    }

    /// Whether there are intermediate screens from a previous pass waiting to be shown.
    pub fn has_update_frames(&self) -> bool {
        !self.update_frames.is_empty()
    }

    /// The next intermediate screen, in the order it was presented.
    ///
    /// **An animation that runs entirely inside a single callback only shows up this way.** The
    /// Z-Wheel's transition (`0x74258`) slides the screen in a synchronous loop: it copies a piece,
    /// calls `IDISPLAY_Update`, moves five pixels, and repeats some two hundred times before
    /// returning. On the console each `Update` goes to the screen; here the window only saw the end
    /// of the callback, and the transition became a hard cut.
    pub fn take_update_frame(&mut self) -> Option<Framebuffer> {
        self.update_frames.pop_front()
    }

    /// Keeps the screen at `IDISPLAY_Update`, when the target is the screen.
    pub(super) fn store_update_frame(&mut self) -> Result<(), CpuError> {
        /// Cap on kept screens: each one is 600 KB. Past it, one in every two is kept.
        const CAP: usize = 96;
        let target = self.target()?;
        if target != self.device_bitmap {
            return Ok(());
        }
        self.updates_in_pass += 1;
        if self.update_frames.len() >= CAP {
            let mut index = 0;
            self.update_frames.retain(|_| {
                index += 1;
                index % 2 == 0
            });
        }
        let screen = self.screen();
        let mut copy = Framebuffer::new(screen.width(), screen.height());
        copy.load_rgb565_bytes(&screen.to_rgb565_bytes());
        self.update_frames.push_back(copy);
        Ok(())
    }

    /// Whether the applet asked to close with `ISHELL_CloseApplet`.
    pub fn close_requested(&self) -> bool {
        self.applet_closed
    }

    /// Delivers `EVT_APP_STOP` to the applet, which is the last thing it receives before exiting.
    ///
    /// It is where many games save their progress. What it answers changes nothing: closing was
    /// requested by the applet itself.
    pub fn stop_applet(&mut self) -> Result<(), CpuError> {
        self.send_applet_event(self.applet_class, EVT_APP_STOP, 0, 0)?;
        Ok(())
    }

    /// Chooses how the Z-Wheel reads `tectoy.cfg`, **before the applet is created**.
    ///
    /// Outside the factory default, it gets a copy of the cfg in the device profile with values
    /// changed; the package is not touched.
    ///
    /// - **End of life.** The factory one has `EOL=1` and `zeebomenu_hide=1`. The read at `0x7fe80`
    ///   sets with them bits `0x2000` and `0x4000` of `app+0x3614`, and the assembly of the lower
    ///   wheel at `0x4f620` ends up with "Jogar" and "Ajuda". With both at zero it goes back to how
    ///   it was before: "Jogar", the zeebo logo, "Comprar" and "Configurar".
    /// - **Transitions.** `0x798c4` decides whether the screen change slides: with the screen's
    ///   type in `SlideOnceToForm`, only if it isn't yet in the preferences' `HasSlidToForm`, which
    ///   it marks the first time; outside it, always, unless it is in `NoSlideToForm`. The cfg has
    ///   `SlideOnceToForm=31` and the dump already comes with `HasSlidToForm=14` — Jogar,
    ///   Configurar and zeebo seen —, so nothing slid. With `SlideOnceToForm=0`, everything slides.
    pub fn configure_z_wheel(&mut self, options: crate::config::ZWheel) {
        self.z_wheel = options;
    }

    pub fn new(cpu: C, module: LoadedModule, root: impl Into<std::path::PathBuf>) -> Self {
        let storage = crate::storage::StoragePaths::from_root(crate::config::config_dir());
        Self::new_with_storage(cpu, module, root, &storage, None)
    }

    /// Like [`Machine::new`], but receives the frontend's persistent roots.
    ///
    /// The old constructor preserves the desktop UI. Isolated frontends, like Libretro, cannot
    /// depend on `ui::settings::config_dir()` and use this form with their authorized root: the
    /// shared NAND and, when the profile asks for it, the title's writable overlay.
    pub fn new_with_storage(
        cpu: C,
        module: LoadedModule,
        root: impl Into<std::path::PathBuf>,
        storage: &crate::storage::StoragePaths,
        save_root: Option<std::path::PathBuf>,
    ) -> Self {
        Self::new_with_storage_policy(cpu, module, root, storage, save_root, crate::audio::MidiBackend::Auto)
    }

    /// Like [`Machine::new_with_storage`], but explicitly receives the MIDI synthesizer policy.
    ///
    /// The `allow` is conditional and has a reason: without the `soundfont` feature there is no
    /// sample bank to choose, so nobody reads the policy — and the alternative, a parameter with
    /// `_` in its name, would lie about the public signature in **every** build because of a
    /// single one.
    #[cfg_attr(not(feature = "soundfont"), allow(unused_variables))]
    pub fn new_with_storage_policy(
        cpu: C,
        module: LoadedModule,
        root: impl Into<std::path::PathBuf>,
        storage: &crate::storage::StoragePaths,
        save_root: Option<std::path::PathBuf>,
        midi_policy: crate::audio::MidiBackend,
    ) -> Self {
        let root_dir: std::path::PathBuf = root.into();
        let device: std::path::PathBuf = storage.device.clone();
        let heap = Heap::new(loader::HEAP_BASE, loader::HEAP_SIZE);
        // The objects go after the pointers the loader has already reserved at the start of the
        // region, so as not to overwrite them.
        let reserved = module.objects_reserved();
        let extensions = module.extensions.len();
        let mut objects = ObjectStore::new(
            loader::OBJECT_BASE + reserved,
            loader::OBJECT_SIZE - reserved as usize,
        );
        objects.adopt(module.shell, Interface::Shell);
        Self {
            cpu,
            module,
            heap,
            internal_heap: Heap::new(loader::INTERNAL_BASE, loader::INTERNAL_SIZE),
            objects,
            fs_log: std::collections::VecDeque::new(),
            requested_classes: BTreeMap::new(),
            unknown_classes: BTreeSet::new(),
            web_requests: BTreeSet::new(),
            collections: HashMap::new(),
            drawn_texts: std::collections::VecDeque::new(),
            refused_allocs: BTreeSet::new(),
            refused_checks: BTreeSet::new(),
            fonts: std::collections::HashMap::new(),
            databases: HashMap::new(),
            probe_classes: BTreeSet::new(),
            probe_answers: HashMap::new(),
            // January 6, 1980 is BREW's epoch; Unix's is ten years and six days earlier.
            epoch_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().saturating_sub(315_964_800) as u32)
                .unwrap_or(0),
            probe_objects: HashMap::new(),
            probe_log: Vec::new(),
            suspicious_objects: BTreeSet::new(),
            assumptions: BTreeSet::new(),
            bad_pointers: BTreeSet::new(),
            graphics: GraphicsState::default(),
            vfs: {
                let mut vfs = Vfs::new(root_dir.clone());
                // All games share the same `fs:/`, as on the console.
                vfs.set_device_root(device.clone());
                // And the same save store: the writable `/mod/` is a single one.
                vfs.set_save_root(save_root.unwrap_or_else(|| storage.mod_saves()));
                vfs
            },
            open_files: HashMap::new(),
            file_error: SUCCESS,
            decoders: HashMap::new(),
            feeds: HashMap::new(),
            surface_manip: 0,
            imageon_ext: 0,
            gles11_ext: 0,
            gles10_ext: 0,
            egl_get_power_level: 0,
            egl_oes_swap_interval: 0,
            egl_get_color_buffer: 0,
            gles11_ext_pak: 0,
            scale_source: None,
            prefs: HashMap::new(),
            enumerations: HashMap::new(),
            missing_files: BTreeSet::new(),
            input_signals: BTreeMap::new(),
            signals: HashMap::new(),
            pending_signals: Vec::new(),
            pads: [Pad::default(); input::PORTS],
            pad_events: std::array::from_fn(|_| std::collections::VecDeque::new()),
            motion: [[0.0, 0.0, 1.0]; input::PORTS],
            boomerang_sequence: 0,
            last_boomerang_report_us: 0,
            last_boomerang_packet_us: 0,
            calibrations: (0, 0),
            // One port with a controller is what there always was; the interface changes this when
            // applying the settings, and headless mode never touches it.
            ports: std::array::from_fn(|n| {
                (n == 0).then_some(crate::input::bindings::Device::Controller)
            }),
            device_ports: HashMap::new(),
            media_log: std::collections::VecDeque::new(),
            key_events: std::collections::VecDeque::new(),
            pad_log: std::collections::VecDeque::new(),
            current_applet: 0,
            random_state: 0x1234_5678,
            clock_us: 0,
            spin_polls: 0,
            next_vsync_us: 0,
            trace: Vec::new(),
            tracing: false,
            trace_filter: None,
            debug_output: Vec::new(),
            debug_index: HashMap::new(),
            bitmaps: HashMap::new(),
            fault_regs: [0; 12],
            fault_stack: Vec::new(),
            timers: Vec::new(),
            applet_class: 0,
            installed_applets: HashSet::new(),
            installed_modules: Vec::new(),
            applet_enumeration: 0,
            mif_in_guest: HashMap::new(),
            budget: 0,
            ext_modules: vec![None; extensions],
            pending_launch: None,
            wheel_boot_skipped: false,
            gl_frame_writes: None,
            z_wheel: crate::config::ZWheel {
                end_of_life: true,
                always_transitions: false,
            },
            applet_closed: false,
            pending_activation: None,
            update_frames: Default::default(),
            updates_in_pass: 0,
            nesting: 0,
            interrupted_segment: None,
            pending_probes: Vec::new(),
            pending_blits: Vec::new(),
            pending_surface_blits: Vec::new(),
            probed: HashSet::new(),
            ciphers: HashMap::new(),
            hashes: HashMap::new(),
            resources: crate::loader::resfile::ResCache::default(),
            unzips: HashMap::new(),
            images: HashMap::new(),
            image_bitmaps: HashMap::new(),
            api_time: HashMap::new(),
            api_calls: 0,
            off_center_axes: 0,
            off_center_axes_mask: 0,
            clock_ns: 0,
            profiling_api: false,
            image_notify: HashMap::new(),
            image_info: HashMap::new(),
            image_clips: HashMap::new(),
            collection_params: HashMap::new(),
            vectors: HashMap::new(),
            sources: HashMap::new(),
            html_pages: HashMap::new(),
            html_scroll: HashMap::new(),
            html_max_scroll: HashMap::new(),
            scroll_keys: Default::default(),
            peeks: HashMap::new(),
            widgets: HashMap::new(),
            selectors_per_class: std::collections::BTreeMap::new(),
            widget_census: false,
            config_items: HashMap::new(),
            network: true,
            // For the same reason, the server redirect also comes from the environment:
            // `ZEEBX_SERVER=127.0.0.1:8080`. Without it, pointing a game at a test server would
            // require port 80, which needs privileges.
            network_to: crate::config::env_var("ZEEBX_SERVER").ok(),
            // The bridge comes on. It brought down the game while delivering from inside the
            // dispatch, and so it stayed off for a while; with delivery at the call boundary that
            // no longer happens, and leaving it off only created a case where the game looks broken
            // for lack of an environment variable. `ZEEBX_NO_BRIDGE` turns it off.
            bridge: crate::config::env_var_os("ZEEBX_NO_BRIDGE").is_none(),
            pending_response: None,
            pending_end: None,
            delivered: Vec::new(),
            plaintexts: std::collections::VecDeque::new(),
            missing_apis: BTreeSet::new(),
            swallowed_failures: BTreeSet::new(),
            ignored_gl: BTreeSet::new(),
            // **One, not the specification's 4.** Peggle uploads the 2-pixel-wide strips of its
            // panels in `GL_RGB` with tightly packed rows (6 bytes), without calling
            // `PixelStorei`, and runs that way on the device: there the effect is that of
            // alignment 1. With 4, each row slipped 2 bytes and the stretched strips turned into
            // colored stripes in the PopCap games. Whoever needs another value asks for it:
            // Powerboat Challenge asks for 1, and it is that call that `PixelStorei` honors.
            unpack_alignment: 1,
            web_response: Vec::new(),
            streams: HashMap::new(),
            sounds: HashMap::new(),
            pending_calls: Vec::new(),
            media_notify_queue: Vec::new(),
            image_notify_queue: Vec::new(),
            pcm_streams: HashMap::new(),
            live_buffers: HashMap::new(),
            stream_buffer: 0,
            media_notify_block: 0,
            resources_read: BTreeSet::new(),
            widgets_dumped: false,
            next_serial: 0,
            painted_form: 0,
            last_draw_us: 0,
            serial: None,
            egl_error: gles::EGL_SUCCESS,
            egl_surfaces: HashMap::new(),
            egl_initial_viewport: false,
            egl_color_buffer: (0, 0),
            egl_color_bytes: Vec::new(),
            egl_color_dimensions: None,
            egl_color_readback: Vec::new(),
            egl_next_handle: EGL_HANDLE_BASE,
            egl_swaps: 0,
            gl_clears: 0,
            texture_names: gl::GlNames::default(),
            buffer_names: gl::GlNames::default(),
            gles_object: 0,
            egl_surface: 0,
            egl_context: 0,
            media: HashMap::new(),
            media_payloads: HashMap::new(),
            audio: None,
            gl_last_frame_words: Vec::new(),
            gl_frame_pending: false,
            gpu_without_frame: false,
            gl_materializations: 0,
            gl: rasterizer(SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize),
            gl_vertices: ArrayPointer::default(),
            gl_colors: ArrayPointer::default(),
            gl_texcoords: ArrayPointer::default(),
            gl_texcoords1: ArrayPointer::default(),
            gl_normals: ArrayPointer::default(),
            gl_buffers: HashMap::new(),
            gl_array_buffer: 0,
            gl_element_buffer: 0,
            gl_current_normal: [0.0, 0.0, 1.0],
            interned: HashMap::new(),
            threads: HashMap::new(),
            resume_callbacks: HashMap::new(),
            pending_threads: Vec::new(),
            stalled: None,
            current_thread: None,
            stack_in_heap: None,
            dib_buffers: HashMap::new(),
            dib_capacity: HashMap::new(),
            inherited_dibs: HashSet::new(),
            decoder_dib: HashMap::new(),
            guest_dibs: HashMap::new(),
            published_dib: HashMap::new(),
            surfaces: Heap::new(loader::SURFACE_BASE, loader::SURFACE_SIZE),
            notifying_widgets: std::collections::HashSet::new(),
            transparency: HashMap::new(),
            transforms: HashMap::new(),
            value_models: HashMap::new(),
            system_mode: 0,
            canvases: HashMap::new(),
            device_bitmap: 0,
            display_target: 0,
            clip: None,
            graphics_clip: None,
            screen: Framebuffer::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32),
            colors: default_colors(),
            pending_text: Vec::new(),
            // The device font comes from the root this engine received, not from the desktop
            // configuration: that is what makes the font installed by the frontend be found.
            font: module_font(&root_dir).or_else(|| console_font(&device)),
            // The MIDI sample bank, when the device has one. It is optional on purpose: the bank
            // doesn't come built in, and without it the music goes back to the wavetable.
            #[cfg(feature = "soundfont")]
            sound_bank: device_sound_bank(&device, midi_policy),
            calls: BTreeMap::new(),
            calls_total: 0,
            skip_draw: false,
            gl_pixels_read: false,
        }
    }

    /// Calls a guest function and waits for it to return.
    ///
    /// It is the reverse path of API dispatch: here we are the ones calling the module. We set up
    /// the arguments per the AAPCS, point `lr` at the sentinel and run the same loop — if the guest
    /// calls some API along the way, it is served normally.
    ///
    /// Only the first four arguments are supported; no BREW API we need today passes more than
    /// that through the stack.
    pub fn call_guest(
        &mut self,
        func: u32,
        args: [u32; 4],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        self.cpu.write_reg(Reg::R0, args[0]);
        self.cpu.write_reg(Reg::R1, args[1]);
        self.cpu.write_reg(Reg::R2, args[2]);
        self.cpu.write_reg(Reg::R3, args[3]);
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        self.execute(func, budget)
    }

    /// Calls the guest from **inside** the dispatch of an API call.
    ///
    /// The normal [`Self::call_guest`] doesn't work here: it writes to `r0..r3` and to `lr`, and
    /// whoever is dispatching will read `lr` afterwards to know where to resume the game. Losing it
    /// sends execution back to the wrong place — usually to address zero.
    ///
    /// That is why the exception to the rule comes with a rule: **the whole context is saved and
    /// restored**. The stack needs no care, because the nested call pushes below the current `sp`,
    /// which is space nobody is using — it is the same guarantee an interrupt has.
    ///
    /// It is what the console does: `ISHELL_CreateInstance` of an extension class enters the
    /// extension's module and comes back with the object, without the game noticing.
    fn call_guest_nested(
        &mut self,
        func: u32,
        args: [u32; 4],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        const CONTEXT_REGS: [Reg; 15] = [
            Reg::R0,
            Reg::R1,
            Reg::R2,
            Reg::R3,
            Reg::R4,
            Reg::R5,
            Reg::R6,
            Reg::R7,
            Reg::R8,
            Reg::R9,
            Reg::R10,
            Reg::R11,
            Reg::R12,
            Reg::Sp,
            Reg::Lr,
        ];
        let saved = CONTEXT_REGS.map(|reg| self.cpu.read_reg(reg));
        let outcome = self.call_guest(func, args, budget);
        for (reg, value) in CONTEXT_REGS.into_iter().zip(saved) {
            self.cpu.write_reg(reg, value);
        }
        outcome
    }

    /// Calls a guest function with more than four arguments.
    ///
    /// The AAPCS puts the first four in `r0..r3` and the rest on the stack, in increasing order
    /// from `sp`, which must be 8-byte aligned on function entry. That is how `IBITMAP_BltIn`,
    /// which has nine, is called.
    fn call_guest_with_stack(
        &mut self,
        func: u32,
        regs: [u32; 4],
        extra: &[u32],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        let sp = self.cpu.read_reg(Reg::Sp);
        let bytes: Vec<u8> = extra.iter().flat_map(|word| word.to_le_bytes()).collect();
        // If the stack has no room, calling would mean writing outside it.
        if (sp as usize) < bytes.len() + 8 {
            return Ok(Outcome::Returned { code: EFAILED });
        }
        let new_sp = (sp - bytes.len() as u32) & !7;
        self.cpu.write_mem(new_sp, &bytes)?;
        self.cpu.write_reg(Reg::Sp, new_sp);
        let outcome = self.call_guest(func, regs, budget);
        self.cpu.write_reg(Reg::Sp, sp);
        outcome
    }

    /// Loads the module into the core and runs `AEEMod_Load` until an outcome.
    ///
    /// `budget` is the instruction cap **per slice**, between two API calls; it exists so that an
    /// infinite loop in the guest doesn't hang the emulator.
    pub fn run(&mut self, budget: u64) -> Result<Outcome, CpuError> {
        self.cpu.reset(&self.module.mem)?;
        self.cpu.write_reg(Reg::R0, self.module.shell);
        self.cpu.write_reg(Reg::R1, self.module.helpers);
        self.cpu.write_reg(Reg::R2, self.module.out_module);
        self.cpu
            .write_reg(Reg::Sp, loader::STACK_BASE + loader::STACK_SIZE as u32 - 16);
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);

        self.execute(self.module.entry, budget)
    }

    /// The loop proper: runs, serves API calls and carries on until an outcome.
    fn execute(&mut self, entry: u32, budget: u64) -> Result<Outcome, CpuError> {
        let mut pc = entry;
        // **There is a cap for the whole stretch, besides the one for each slice.** The budget was
        // passed to each `cpu.run` and started from zero again after every API call, so a game
        // that called one API per pass ran forever inside **one** pass of the frame loop — and
        // meanwhile neither the player's input arrived nor was the real-time cap checked, because
        // both live in the outer loop.
        //
        // That is what trapped the Z-Wheel in attract mode: with nobody touching it, it repeats
        // the opening, and each repetition is an API call. The game was right; the ones not
        // yielding were us.
        //
        // The cap has measured headroom. Equal to a slice's budget it breaks games that do a lot
        // of work in a single frame — Zeeboids stopped halfway, with two and a half seconds
        // instead of ten. At four times, Zeeboids runs identically (one billion four hundred and
        // fifty million instructions in 12,978 passes, the same numbers as before) and the Z-Wheel
        // yields seven times sooner, which is the difference between the interface responding and
        // freezing.
        //
        // And blowing the cap **is not game over**: it is a request to yield. The caller treats
        // `Outcome::Budget` as a normal pass — treating it as a bad outcome stopped the Z-Wheel on
        // the first pass, because it repeats the opening while nobody touches it and each
        // repetition spends budget.
        let begin = self.cpu.instructions();
        // Local, not a counter that gets reset: a callback served in here enters `execute` again,
        // and resetting on its entry would hide the outer stretch's loop.
        let calls_at_start = self.calls_total;
        let cap = budget.saturating_mul(SEGMENT_CAP);
        self.budget = budget;
        loop {
            let spent = self.cpu.instructions().saturating_sub(begin);
            let slice = cap.saturating_sub(spent).min(budget);
            if slice == 0 {
                self.note_interrupted_segment(pc);
                return Ok(Outcome::Budget);
            }
            match self.cpu.run(pc, slice)? {
                StopReason::ApiCall { addr }
                    if self.calls_total - calls_at_start >= MAX_CALLS =>
                {
                    let _ = addr;
                    return Ok(Outcome::CallLimit {
                        calls: self.calls_total - calls_at_start,
                    });
                }
                StopReason::ApiCall { addr } => match self.dispatch(addr)? {
                    Some(result) => {
                        self.cpu.write_reg(Reg::R0, result);
                        // `lr` holds the return address, with bit 0 indicating Thumb — the core
                        // understands that convention, so we pass it on as is.
                        pc = self.cpu.read_reg(Reg::Lr);
                        // This is the boundary between two API calls — the only point where we
                        // can enter the guest without interrupting anything halfway.
                        self.run_pending_callbacks(budget)?;
                    }
                    None => {
                        // Record it here, not only where the outcome is read, because not every
                        // outcome is read: an API missing **inside a callback** aborts that call
                        // and execution carries on, leaving no trace. That is how the widget's
                        // slot 12 went unnoticed — the Z-Wheel built the whole screen, died
                        // silently in the image callback, and the report came out clean.
                        let caller = self.cpu.read_reg(Reg::Lr);
                        self.missing_apis
                            .insert(format!("{} (from {caller:#010x})", aee::describe(addr)));
                        return Ok(Outcome::Unimplemented {
                            addr,
                            args: self.args(),
                            caller,
                        });
                    }
                },
                StopReason::Returned => {
                    return Ok(Outcome::Returned {
                        code: self.cpu.read_reg(Reg::R0),
                    });
                }
                StopReason::MemoryFault { addr, pc } => {
                    let lr = self.cpu.read_reg(Reg::Lr);
                    self.fault_stack = self.scan_stack();
                    self.fault_regs = [
                        Reg::R0,
                        Reg::R1,
                        Reg::R2,
                        Reg::R3,
                        Reg::R4,
                        Reg::R5,
                        Reg::R6,
                        Reg::R7,
                        Reg::R8,
                        Reg::R9,
                        Reg::R10,
                        Reg::R11,
                    ]
                    .map(|reg| self.cpu.read_reg(reg));
                    // Record it here, not only where the outcome is read, for the same reason as
                    // the missing APIs: an invalid access **inside a callback** aborts that call
                    // and execution carries on, and whoever sent the event reads only "nobody
                    // handled it". That is how the Z-Wheel's `SendEvent` at `0x885d8` vanished
                    // from the report while it brought down the z-pad form with error 6.
                    self.swallowed_failures
                        .insert(format!("invalid access at {addr:#010x} at pc {pc:#010x}"));
                    return Ok(Outcome::Fault { addr, pc, lr });
                }
                StopReason::Exception { pc } => return Ok(Outcome::Exception { pc }),
                StopReason::Budget => {
                    // Bit 0 says the mode, as in every ARM resume.
                    let stopped = self.cpu.read_reg(Reg::Pc) & !1;
                    let mode = u32::from(self.cpu.in_thumb());
                    self.note_interrupted_segment(stopped | mode);
                    return Ok(Outcome::Budget);
                }
            }
        }
    }

    /// Keeps where to continue the stretch the instruction cap interrupted.
    ///
    /// **Only the outermost stretch.** The guest keeps its state in its registers and on its stack,
    /// so resuming is continuing from `pc` — but a nested stretch (a callback called from inside an
    /// API's dispatch) has someone waiting for it on our side, and that frame is already gone when
    /// the pass ends. Nested, the cap is still just a request to yield.
    fn note_interrupted_segment(&mut self, pc: u32) {
        if self.nesting != 0 {
            return;
        }
        // **The registers go along.** Between the cut and the resume the emulator still delivers
        // this pass's signals and callbacks, and entering the guest for that overwrites `r0`-`r3`,
        // `lr` and whatever else the handler uses. Without saving the context, the resume carried
        // on with the registers of something else — and Rolima jumped to address zero.
        let mut state = [0u32; 15];
        for (slot, reg) in state.iter_mut().zip(THREAD_REGS) {
            *slot = self.cpu.read_reg(reg);
        }
        state[14] = self.cpu.read_reg(Reg::Lr);
        self.interrupted_segment = Some((pc, state));
    }

    /// Continues the interrupted stretch, if there is one.
    ///
    /// **A game can run its whole loop inside `EVT_APP_START`.** Zeebo Extreme Rolima does that: it
    /// never yields with `IThread::Suspend`, as its siblings do, and the instruction cap cut the
    /// loading off halfway. Without resuming, the stretch vanished — the event loop found no timer
    /// or callback at all, and the session ended by itself at 3.8 seconds, as if the game had
    /// finished.
    fn resume_segment(&mut self, budget: u64) -> Result<Option<Outcome>, CpuError> {
        let Some((pc, state)) = self.interrupted_segment.take() else {
            return Ok(None);
        };
        for (value, reg) in state.iter().zip(THREAD_REGS) {
            self.cpu.write_reg(reg, *value);
        }
        self.cpu.write_reg(Reg::Lr, state[14]);
        self.execute(pc, budget).map(Some)
    }

    /// Serves a call. `None` means "not implemented yet".
    fn dispatch(&mut self, addr: u32) -> Result<Option<u32>, CpuError> {
        let Some((iface, slot)) = aee::decode(addr) else {
            return Ok(None);
        };
        *self.calls.entry((iface as u32, slot)).or_insert(0) += 1;
        self.calls_total += 1;
        self.note_spin(iface, slot);
        let traced = self.tracing
            && match &self.trace_filter {
                Some(part) => aee::describe(addr).contains(part.as_str()),
                None => true,
            };
        if traced {
            let args = self.args();
            // Keeps the last calls: in a game that makes tens of thousands of them, what matters
            // is the end, not the beginning.
            if self.trace.len() >= MAX_TRACE {
                self.trace.remove(0);
            }
            self.trace.push(format!(
                "{} (r0={:#x} r1={:#x} r2={:#x} r3={:#x}) returns to {:#010x}",
                aee::describe(addr),
                args[0],
                args[1],
                args[2],
                args[3],
                self.cpu.read_reg(Reg::Lr)
            ));
        }

        let entry = if traced {
            self.trace.len().checked_sub(1)
        } else {
            None
        };
        // A bad pointer coming from the guest must not bring down the emulator: we turn it into
        // `EBADPARM`, which is what BREW answers in that case, and record it to show up in the
        // report.
        // **Sampled, and out of necessity.** This machine's `Instant::now` is a system call —
        // measured at 1318 ns per read, with the proof in
        // [`crate::scan::tests::how_much_the_clock_costs`] —, and the profile reads the clock twice
        // per call. Timing all of them made the instrument charge 2.65 µs per call, more than the
        // method measured, and make the run 42% more expensive. One in every
        // `PROFILE_SAMPLE_EVERY` estimates the same and brings the instrument back to normal use.
        self.api_calls = self.api_calls.wrapping_add(1);
        let should_sample = self.api_calls % PROFILE_SAMPLE_EVERY == 0;
        let started = (self.profiling_api && should_sample).then(std::time::Instant::now);
        let result = match self.dispatch_inner(iface, slot) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(None),
            Err(err) => {
                self.note_bad_pointer(format!("{} ({err})", aee::describe(addr)));
                EBADPARM
            }
        };
        if let Some(started) = started {
            let cost = self.api_time.entry((iface as u32, slot)).or_insert((0, 0));
            cost.0 += started.elapsed().as_nanos() as u64;
            cost.1 += 1;
        }
        // Appends the return value to the trace line: without it you can't see which call returned
        // the error that made the game give up.
        if let Some(line) = entry.and_then(|i| self.trace.get_mut(i)) {
            line.push_str(&format!(" -> {result:#x}"));
        }
        self.spend_stack();
        Ok(Some(result))
    }

    /// Dirties the stack below `sp`, as the real implementation of the call would have dirtied it.
    ///
    /// **On the device, BREW runs on the game's stack; here, it doesn't.** Every Zeebo `MALLOC`,
    /// `FREE` or `OpenFile` pushes frames below the caller's `sp`, and wipes whatever was left
    /// there. Our servicing is Rust and doesn't touch the guest's stack, so the leftovers of a
    /// previous call survive — and some games, without knowing it, depend on them going away.
    ///
    /// Iron Sight is the measured case. The level's building loop asks for `building0.pof` up to
    /// `building49.pof`, and none of them exists in `dataTall.bar`: the game counts on the loader
    /// returning 0. The chunk reader (`0x9b860`) is built on the stack, doesn't find the file and
    /// zeroes the stream, but its user (`0x8bae8`) only checks whether the current chunk is `3DOB`
    /// — a field the constructor doesn't initialize. The `3DOB` of the previous `.pof`, read at the
    /// same stack address, was left there, and the game carried on with the null stream until it
    /// jumped to address zero. The `MALLOC` called just before, `0x20` bytes from the top of the
    /// frame, reaches the field with `0x4c` bytes of stack; the 256 here cover that with room to
    /// spare.
    ///
    /// Only inside a stack whose start we know — the main one or a live heap block —, because
    /// below the floor lives something else. Iron Sight doesn't use `IThread`: it allocates 64 KB
    /// with `MALLOC` and points `sp` there on its own, and an `IThread`'s stack is a block of the
    /// internal region, so the same criterion serves both.
    fn spend_stack(&mut self) {
        /// How much stack the real call would have used. Not measured on the device: it is headroom
        /// over the `0x4c` bytes Iron Sight needs.
        const STACK_SPENT: u32 = 256;
        let sp = self.cpu.read_reg(Reg::Sp);
        let main = loader::STACK_BASE..=loader::STACK_BASE + loader::STACK_SIZE as u32;
        let floor = if main.contains(&sp) {
            Some(loader::STACK_BASE)
        } else {
            // The stored answer only holds if the block is still alive, the same size, and `sp` is
            // still in it.
            let stored = self.stack_in_heap.filter(|&(start, size)| {
                self.block_size(start) == Some(size)
                    && (start..=start + size).contains(&sp)
            });
            if stored.is_none() {
                // The top is exclusive in the block, but it is the initial `sp` of a stack: we
                // search by the last byte below it.
                let background = sp.wrapping_sub(1);
                self.stack_in_heap = if loader::is_internal(background) {
                    self.internal_heap.block_containing(background)
                } else {
                    self.heap.block_containing(background)
                };
            }
            self.stack_in_heap.map(|(start, _)| start)
        };
        if let Some(floor) = floor
            && sp.wrapping_sub(STACK_SPENT) >= floor
        {
            // Without invalidating the JIT: the stack doesn't become code (see
            // `CpuBackend::mark_stack_dirty`).
            let _ = self.cpu.mark_stack_dirty(sp - STACK_SPENT, STACK_SPENT);
        }
    }

    /// How many nanoseconds one `Instant::now()` read costs on this machine.
    ///
    /// Measured, not assumed: on Linux with `vDSO` it is tens of nanoseconds, and without it it
    /// goes past a microsecond. The API profile reads the clock twice per sampled call, and
    /// subtracting that is the difference between measuring the method and measuring the
    /// instrument. The proof of the value is in [`crate::scan::tests::how_much_the_clock_costs`].
    fn measure_clock() -> u64 {
        /// Sample reads: high enough for the loop to vanish into the noise, low enough not to
        /// weigh on a game's startup.
        const N: u64 = 4096;

        let _ = std::time::Instant::now();
        let begin = std::time::Instant::now();
        let mut last = begin;
        for _ in 0..N {
            last = std::time::Instant::now();
        }
        let _ = last;
        begin.elapsed().as_nanos() as u64 / N
    }

    /// The dispatch proper, separated so that guest memory access faults can be handled in
    /// [`Machine::dispatch`].
    fn dispatch_inner(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let result = match (iface, slot) {
            (Interface::Helpers, _) => match self.helper_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::MediaUtil, _) | (Interface::Media, _) => {
                match self.media_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Egl, _) | (Interface::EglLegacy, _) => match self.egl_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Gles, _) | (Interface::GlLegacy, _) => {
                match self.gles_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Thread, _) => match self.thread_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Shell, 2) => self.shell_create_instance()?,
            // `int ISHELL_CloseApplet(IShell *, boolean bReturnToIdle)`. It is how a game exits
            // through its own menu. BREW doesn't close inside the call: it schedules it, and the
            // applet receives `EVT_APP_STOP` after returning. Here the request is noted down, and
            // the session ends on the loop's pass (see [`Machine::stop_applet`]).
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("CloseApplet") => {
                self.applet_closed = true;
                SUCCESS
            }
            (Interface::Shell, 4) => self.shell_get_device_info()?,
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("StartApplet" | "CanStartApplet")
                ) =>
            {
                let cls = self.cpu.read_reg(Reg::R1);
                let installed = self.installed_applets.contains(&cls);
                match Interface::Shell.method(slot) {
                    // `boolean ISHELL_CanStartApplet(IShell *, AEECLSID)`: **true means it can**.
                    // It answered `SUCCESS`, which is zero — "can't" for every installed game. The
                    // Z-Wheel asks this at `0x7e58c` before launching and, with zero, gives up and
                    // logs "Failure attempting to launch game".
                    Some("CanStartApplet") => u32::from(installed),
                    _ if installed => {
                        self.pending_launch = Some(cls);
                        SUCCESS
                    }
                    _ => ECLASSNOTSUPPORT,
                }
            }
            // `int ISHELL_EnumAppletInit(IShell *)` and `boolean ISHELL_EnumNextApplet(IShell *,
            // AEEAppInfo *)`. The Z-Wheel walks the list before launching a game (`0x794c8`): it
            // finds the chosen class, reads `pszMIF`, cuts up to the last slash and strips the
            // extension — what it wants is the **module id**, with which it builds the game's
            // directory.
            //
            // `AEEAppInfo` is `{ AEECLSID cls; char *pszMIF; uint16 wIDBase; uint16 wAppType;
            // uint32 dwFlags; }`.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("EnumAppletInit") => {
                self.applet_enumeration = 0;
                SUCCESS
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("EnumNextApplet") => {
                let output = self.cpu.read_reg(Reg::R1);
                match self
                    .installed_modules
                    .get(self.applet_enumeration)
                    .cloned()
                {
                    Some((class_id, id)) if output != 0 => {
                        self.applet_enumeration += 1;
                        let mif = match self.mif_in_guest.get(&class_id) {
                            Some(&address) => address,
                            None => {
                                let text = format!("fs:/mif/{id}.mif");
                                let address = self.alloc_internal(text.len() as u32 + 1)?;
                                if address != 0 {
                                    self.write_cstring_limited(address, &text, text.len() + 1)?;
                                    self.mif_in_guest.insert(class_id, address);
                                }
                                address
                            }
                        };
                        self.cpu.write_mem(output, &[0u8; 16])?;
                        self.cpu.write_u32(output, class_id)?;
                        self.cpu.write_u32(output + 4, mif)?;
                        1
                    }
                    _ => 0,
                }
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("ActiveApplet") => {
                self.applet_class
            }

            (Interface::Shell, slot)
                if Interface::Shell.method(slot) == Some("GetDeviceInfoEx") =>
            {
                self.shell_get_device_info_ex()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("LoadResObject") => {
                self.shell_load_res_object()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("LoadResString") => {
                self.shell_load_res_string()?
            }
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("LoadResData" | "LoadResDataEx")
                ) =>
            {
                let with_type = Interface::Shell.method(slot) == Some("LoadResDataEx");
                self.shell_load_res_data(with_type)?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("FreeResData") => {
                self.free(self.cpu.read_reg(Reg::R1));
                SUCCESS
            }
            // Delivers an event to an applet. The Z-Wheel sends one to **itself** — its string
            // says why: "SendEvent to get PrefsDB failed", that is, it is how one part of the app
            // asks another for the pointer to the preferences database.
            //
            // The observed call has the shape of `PostEventEx`: `r1` is flags (zero), `r2` is the
            // ClassID and `r3` the event. Since the five-argument form of `SendEvent` puts the
            // ClassID in `r1`, we read both: what decides is which of the two registers carries
            // the ClassID of the running applet.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("SendEvent") => {
                let (a1, a2, a3) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let (cls, evt, w) = match a2 == self.applet_class {
                    true => (a2, a3, self.stack_arg(0)? as u16),
                    false => (a1, a2, a3 as u16),
                };
                let dw = self.stack_arg(1)?;
                self.send_applet_event(cls, evt, w, dw)?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("GetHandler") => {
                let mime = self
                    .cpu
                    .read_cstring(self.cpu.read_reg(Reg::R2), MAX_STRING);
                handler_for(&mime)
            }
            // int ISHELL_GetPrefs(IShell *, AEECLSID cls, uint16 wVer, void *pCfg, uint16 nSize)
            // int ISHELL_SetPrefs(IShell *, AEECLSID cls, uint16 wVer, void *pCfg, uint16 nSize)
            //
            // The preferences live only while the emulator runs. Keeping them on disk would mean
            // inventing a format: the console had one, and we don't know which. What matters is
            // that a game that writes and rereads at the same moment finds what it wrote.
            (Interface::Shell, slot)
                if matches!(Interface::Shell.method(slot), Some("GetPrefs" | "SetPrefs")) =>
            {
                let (cls, version) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let (buffer, size) = (self.cpu.read_reg(Reg::R3), self.stack_arg(0)? as usize);
                let key = (cls, version as u16);
                match Interface::Shell.method(slot) == Some("SetPrefs") {
                    true => {
                        let mut bytes = vec![0u8; size.min(MAX_PREFS)];
                        if buffer != 0 {
                            self.cpu.read_mem(buffer, &mut bytes)?;
                        }
                        self.prefs.insert(key, bytes);
                        SUCCESS
                    }
                    false => match self.prefs.get(&key) {
                        // With no room, or no destination, BREW returns the record's size.
                        Some(bytes) if buffer == 0 || size < bytes.len() => bytes.len() as u32,
                        Some(bytes) => {
                            let bytes = bytes.clone();
                            self.cpu.write_mem(buffer, &bytes)?;
                            SUCCESS
                        }
                        None => EFAILED,
                    },
                }
            }
            // boolean ISHELL_Prompt(IShell *, AEEPromptInfo *pi)
            //
            // False is "I didn't create the dialog", which is the truth: we have no dialog
            // interface. BREW provides for that answer, and the game goes down the path of one
            // that couldn't ask instead of stopping here.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("Prompt") => FALSE,
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("DetectType") => {
                self.shell_detect_type()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("Resume") => {
                self.shell_resume()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("GetClassItemID") => {
                self.shell_get_class_item_id()
            }
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("SetTimer" | "CancelTimer" | "GetTimerExpiration")
                ) =>
            {
                self.shell_timer_call(Interface::Shell.method(slot).unwrap_or(""))?
            }
            (Interface::Signal, _)
            | (Interface::SignalCtl, _)
            | (Interface::SignalCbFactory, _) => match self.signal_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Graphics, _)
            | (Interface::Display, _)
            | (Interface::Bitmap, _)
            | (Interface::Transform, _)
            | (Interface::Canvas, _) => {
                let whole = iface.method(slot).is_none_or(touches_whole_surface);
                if whole {
                    self.sync_surfaces_in()?;
                }
                let handled = match iface {
                    Interface::Graphics => self.graphics_call(slot)?,
                    Interface::Display => self.display_call(slot)?,
                    Interface::Transform => self.transform_call(slot)?,
                    Interface::Canvas => self.canvas_call(slot)?,
                    _ => self.bitmap_call(slot)?,
                };
                let Some(result) = handled else {
                    return Ok(None);
                };
                if whole {
                    self.sync_surfaces_out()?;
                }
                result
            }
            (Interface::FileMgr, _) | (Interface::File, _) => match self.file_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Sound, _) => match self.sound_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SoundPlayer, _) => match self.sound_player_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Heap, _) => match self.heap_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::EglSurfaceManip, _)
            | (Interface::GlesImageonExt, _)
            | (Interface::Gles11Ext, _)
            | (Interface::Gles10Ext, _)
            | (Interface::EglGetPowerLevel, _)
            | (Interface::EglOesSwapInterval, _)
            | (Interface::EglGetColorBuffer, _)
            | (Interface::Gles11ExtPak, _)
            | (Interface::Joystick, _) => {
                match self.extension_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::ImageDecoder, _) | (Interface::ForceFeed, _) => {
                match self.decoder_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::UnzipStream, _) => match self.unzip_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::License, _) => match self.license_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Web, _)
            | (Interface::Hash, _)
            | (Interface::HashCtx, _)
            | (Interface::CipherFactory, _)
            | (Interface::Cipher, _) => match self.crypto_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::MemAStream, _) => match self.stream_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Image, _) => {
                // Only the three that draw pay for the copy. Pac-Mania makes 168 thousand `SetParm`
                // and 168 thousand `Draw` calls in five virtual seconds, one pair per sprite;
                // charging the whole surface to each `SetParm` — which doesn't put a single pixel
                // on screen — was 52 seconds of wall clock, half of everything the emulator spent
                // serving the game.
                let whole = iface.method(slot).is_none_or(touches_whole_surface);
                if whole {
                    self.sync_surfaces_in()?;
                }
                let Some(result) = self.image_call(slot)? else {
                    return Ok(None);
                };
                if whole {
                    self.sync_surfaces_out()?;
                }
                result
            }
            (Interface::Config, _) => match self.config_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::ZeeboMcp, _) => match self.zeebo_mcp_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Widget, _) => match self.widget_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Control, _) => match self.control_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SimCardCtl, _) => match self.sim_card_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SystemCtl, _) => match self.system_ctl_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Cm, _) => match self.cm_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Class28e3c, _) => match self.value_model_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Font, _) => match self.font_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Typeface, _) => match self.typeface_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Vector, _) => match self.vector_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Source, _) | (Interface::Peek, _) => {
                match self.source_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::SourceUtil, _) => match self.source_util_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Collection, _) => match self.collection_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SqlMgr, _) | (Interface::SqlDatabase, _) => {
                match self.sql_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Probe, _) => self.probe_call(slot)?,
            (Interface::Hid, _) | (Interface::HidDevice, _) => match self.hid_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            // Slots 0 and 1 of every interface are AddRef and Release, inherited from IBase.
            (_, 0) => {
                let obj = self.cpu.read_reg(Reg::R0);
                self.check_object(obj, iface);
                self.objects.add_ref(obj)
            }
            (_, 1) => {
                let obj = self.cpu.read_reg(Reg::R0);
                self.check_object(obj, iface);
                self.objects.release(obj)
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Runs one pass of the event loop: skips idle time, fires what is due and lets the guest
    /// run.
    ///
    /// There is no fixed time step. The clock advances with the work the guest does — the
    /// instruction count — and idle time is skipped up to the next event, as any emulator's
    /// scheduler does. With a fixed step per pass of the loop, virtual time ran much faster than
    /// the game: Crash spent fifteen passes drawing one frame, saw half a second go by between one
    /// and the next, and integrated its physics with that.
    ///
    /// It must run outside the dispatch of a call, like the signal queue: the callbacks execute
    /// in the guest.
    pub fn advance(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        // The stretch the cap cut off continues before anything else: it is the game in the
        // middle of a frame, and a timer or callback delivered on top of it would arrive at the
        // wrong time.
        if let Some(outcome) = self.resume_segment(budget)? {
            return Ok(vec![outcome]);
        }
        self.skip_idle_time();

        // The due ones leave the list *before* running, because the callback typically re-arms
        // the timer — and the re-armed one must not fire already in this same frame.
        let now = self.now_ms();
        let mut due = Vec::new();
        self.timers.retain(|timer| {
            if timer.deadline_ms <= now {
                due.push(*timer);
                false
            } else {
                true
            }
        });

        let mut outcomes = Vec::new();
        for timer in due {
            let call = self.resolve_notify(timer.callback)?;
            if call.function == 0 {
                continue;
            }
            outcomes.push(self.call_guest(call.function, [call.context, 0, 0, 0], budget)?);
        }
        self.run_pending_callbacks(budget)?;
        self.run_pending_threads(budget)?;

        // Whatever interrupted a callback or a thread is also an outcome of the frame.
        outcomes.extend(self.stalled.take());
        Ok(outcomes)
    }

    /// Creates a new object and writes its vtable pointer into guest memory.
    fn new_object(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let Some(addr) = self.objects.create(iface) else {
            return Ok(0);
        };
        if self.dib_buffers.contains_key(&addr) {
            self.inherited_dibs.insert(addr);
        }
        // The transparent color belongs to the bitmap that died here, not to the object being
        // born.
        self.transparency.remove(&addr);
        self.cpu.write_u32(addr, loader::vtable_addr(iface))?;
        Ok(addr)
    }

    /// Replaces the trampoline `GetAppInstance` with three words of ARM code.
    ///
    /// The function always returns the same pointer once the applet exists, and it is called
    /// millions of times: in BREW games the globals live inside the applet, so every access to a
    /// global goes through it. Serving it through the trampoline costs stopping and restarting the
    /// core on every call; as code, it doesn't even leave the CPU.
    ///
    /// ```asm
    /// ldr r0, [pc]   ; the pointer is right after the `bx`
    /// bx  lr
    /// .word <applet>
    /// ```
    fn install_app_instance_stub(&mut self, applet: u32) -> Result<(), CpuError> {
        const LDR_R0_PC: u32 = 0xe59f_0000;
        const BX_LR: u32 = 0xe12f_ff1e;
        let Some(slot) = aee_helpers::HELPERS
            .iter()
            .position(|&name| name == "GetAppInstance")
        else {
            return Ok(());
        };
        self.cpu.write_u32(loader::STUB_BASE, LDR_R0_PC)?;
        self.cpu.write_u32(loader::STUB_BASE + 4, BX_LR)?;
        self.cpu.write_u32(loader::STUB_BASE + 8, applet)?;
        self.cpu
            .write_u32(loader::HELPERS_BASE + slot as u32 * 4, loader::STUB_BASE)
    }

    /// Copies a constant string into guest memory, only once.
    ///
    /// `eglQueryString` and `glGetString` return pointers the game may keep and reread; the
    /// contract is that they remain valid, so each text becomes a permanent block.
    fn intern(&mut self, text: &'static str) -> Result<u32, CpuError> {
        if let Some(&addr) = self.interned.get(text) {
            return Ok(addr);
        }
        let addr = self.alloc_internal(text.len() as u32 + 1)?;
        if addr != 0 {
            self.write_cstring(addr, text)?;
            self.interned.insert(text, addr);
        }
        Ok(addr)
    }

    /// Checks that the pointer received really is one of our objects of the expected interface.
    ///
    /// A mismatch here usually means a misaligned vtable — a slot out of order makes the guest call
    /// the wrong method, and the symptom shows up far from the cause. Recording it early saves
    /// hours.
    fn check_object(&mut self, addr: u32, expected: Interface) {
        if self.objects.kind_of(addr) != Some(expected) {
            self.suspicious_objects.insert((addr, expected as u32));
        }
    }

    fn args(&self) -> [u32; 4] {
        [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ]
    }

    pub fn cpu_mut(&mut self) -> &mut C {
        &mut self.cpu
    }

    pub fn cpu(&self) -> &C {
        &self.cpu
    }

    pub fn module(&self) -> &LoadedModule {
        &self.module
    }
}

#[cfg(test)]
mod tests;
