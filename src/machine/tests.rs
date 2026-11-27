//! The Machine tests: a bare machine, a minimal module and the check of the conversation.

use super::*;

/// **An absurd size coming from the game becomes an API error, not a host allocation.**
///
/// `read_bytes` is the path the reads the game asks for go through, and its ceiling is the same
/// one that guards the other places. Without the guard, this call would ask the host allocator for
/// 2 GiB.
#[test]
fn a_read_with_absurd_size_is_rejected() {
    // The small request goes through — it is the normal path of all the game's reads.
    assert!(super::guest_size(0x10).is_ok());
    // And the absurd one is rejected **before** any allocation: without the guard, this would ask
    // the host allocator for 2 GiB, which does not return an error — the process dies.
    let rejection = super::guest_size(0x7fff_ffff);
    assert!(rejection.is_err(), "2 GiB must be rejected");
    // The exact limit: the ceiling passes, one byte above does not.
    assert!(super::guest_size(super::READ_CAP as usize).is_ok());
    assert!(super::guest_size(super::READ_CAP as usize + 1).is_err());
}

/// **A decompression bomb is rejected, and does not let the host die.**
///
/// Deflate amplification has no limit by construction: a few KiB decompress to gigabytes. Before
/// this guard `inflate` materialized the whole output — and **three times**, one per attempt
/// (gzip, zlib, raw) — before choosing one. The test builds a block that decompresses to 2 GiB and
/// demands `None`; with the ceiling, `take` cuts at 128 MiB plus one byte and the answer is the
/// refusal. It also checks that a legitimate block still works.
#[test]
fn a_decompression_bomb_is_rejected() {
    use std::io::{Read, Write};
    // 256 MiB of zeros, written in chunks: the compressor does not need them in memory, and the
    // decompressed output goes past the 128 MiB ceiling. The size is chosen so that the old
    // behavior is a **wrong answer** (returning the 256 MiB) and not an out-of-memory abort.
    let mut bomb = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    let zeros = vec![0u8; 1 << 20];
    for _ in 0..256 {
        bomb.write_all(&zeros).expect("write");
    }
    let bomb = bomb.finish().expect("finish");
    assert!(bomb.len() < 4 * 1024 * 1024, "the bomb must be small: {} bytes", bomb.len());

    // **The amplification exists, and it is measured here**: without a ceiling, the same block
    // delivers 256 MiB. That is what `inflate` did, three times per call (gzip, zlib and raw),
    // before choosing one.
    let mut uncapped = Vec::new();
    flate2::read::DeflateDecoder::new(&bomb[..])
        .read_to_end(&mut uncapped)
        .expect("the block is valid");
    assert_eq!(uncapped.len(), 256 << 20, "the block delivers 256 MiB from {} bytes", bomb.len());

    // With the ceiling, the answer is the refusal.
    assert_eq!(super::inflate(&bomb), None, "256 MiB go past the ceiling and are rejected");

    // And the legitimate path keeps working.
    let mut valid = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    valid.write_all(b"zeebx!").expect("write");
    let valid = valid.finish().expect("finish");
    assert_eq!(super::inflate(&valid).as_deref(), Some(&b"zeebx!"[..]));
}

/// The GL frame as `u16` RGB565, at the requested size.
fn frame_565<C: CpuBackend>(machine: &mut Machine<C>, width: usize, height: usize) -> Vec<u16> {
    let mut bytes = Vec::new();
    machine.gl.frame_rgb565(width, height, &mut bytes);
    bytes
        .chunks_exact(2)
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect()
}

/// The clip rectangle Pac-Mania uses to show a single letter from the sheet.
const CLIP: Rect = Rect {
    x: 100,
    y: 50,
    width: 20,
    height: 10,
};

#[test]
fn without_a_clip_the_blit_goes_through_whole() {
    // Before the first `SetClipRect` the clip is the whole surface, and nothing changes.
    let same = clip_blit(None, (5, 7), (30, 40), (1, 2));
    assert_eq!(same, Some(((5, 7), (30, 40), (1, 2))));
}

#[test]
fn the_clip_moves_with_the_source_origin() {
    // This is the part that matters: clipping only the destination would show the wrong corner of
    // the image, and that is how a font sheet turns into swapped letters. The destination offset
    // must show up the same in the source.
    let (dst, size, src) = clip_blit(Some(CLIP), (90, 40), (100, 100), (0, 0)).unwrap();
    assert_eq!(dst, (100, 50), "the destination starts at the clip");
    assert_eq!(src, (10, 10), "the source moved the same amount");
    assert_eq!(size, (20, 10), "and the size is the clip's");
}

#[test]
fn a_blit_outside_the_clip_does_not_draw() {
    assert_eq!(clip_blit(Some(CLIP), (0, 0), (10, 10), (0, 0)), None);
    assert_eq!(clip_blit(Some(CLIP), (200, 200), (10, 10), (0, 0)), None);
    // Touching the edge does not draw either: the BREW rectangle does not include the limit.
    assert_eq!(clip_blit(Some(CLIP), (80, 50), (20, 10), (0, 0)), None);
}

#[test]
fn a_blit_smaller_than_the_clip_stays_as_is() {
    let inside = clip_blit(Some(CLIP), (105, 52), (5, 5), (3, 4));
    assert_eq!(inside, Some(((105, 52), (5, 5), (3, 4))));
}

/// `ClearScreen` is `DrawRect(NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)`, and clears the whole
/// screen with the current background color.
///
/// Three readings had to be right at the same time, and all three were wrong: a null pointer is
/// the whole surface and not "no rectangle"; `RGB_NONE` is "the current color" and not "do not
/// paint"; and what decides between frame and fill is `flags`. Tekken 2 clears the screen this
/// way once per frame — without it, its menu showed up on top of the previous screen's text.
/// `strstr` with an empty needle returns the text itself, which is what C says.
///
/// It looks like a specification detail and it is not: Need For Speed registers the game sounds
/// by looking up each one's name in a seventy-entry table, and **one of its five names is the
/// empty string**. Answering "not found", it scanned the whole table, concluded the sound does
/// not exist, printed `ZeeboSnd.cpp:327 BREAKPOINT!` and entered a jump to itself. The game
/// showed up in the report as "instruction budget exhausted" — that is, as if it were heavy,
/// when it was stopped on purpose.
#[test]
fn an_empty_needle_matches_at_the_start_of_the_text() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let text = loader::HEAP_BASE;
    let needle = loader::HEAP_BASE + 0x100;
    machine.cpu.write_mem(text, b"snd/skid/skid.wav\0").unwrap();

    let search = |machine: &mut Machine<UnicornCpu>, word: &[u8]| -> u32 {
        machine.cpu.write_mem(needle, word).unwrap();
        call(
            machine,
            Interface::Helpers,
            slot_of(Interface::Helpers, "strstr"),
            [text, needle, 0, 0],
        )
    };
    assert_eq!(
        search(&mut machine, b"\0"),
        text,
        "an empty needle matches at the start"
    );
    assert_eq!(search(&mut machine, b"skid.wav\0"), text + 9);
    assert_eq!(search(&mut machine, b"snd/skid/skid.wav\0"), text);
    assert_eq!(
        search(&mut machine, b"carbon\0"),
        0,
        "what is not there does not match"
    );
    // A needle longer than the text does not match, and must not overflow.
    assert_eq!(search(&mut machine, b"snd/skid/skid.wav.extra\0"), 0);
}

/// BREW's `snprintf` returns the bytes written **with** the terminator; `sprintf`, without.
///
/// Need For Speed builds each career array with `p += snprintf(p, n, "%s=", name) - 1`. With the
/// C99 length, the next item landed on top of the `=` and of each separator, and the car
/// selection screen opened with no name and no attributes (issue #35). And both count bytes of
/// the game's memory: its text is UTF-8 read as Latin-1, and "ARMAZÉM" is eight bytes, not ten.
/// After every `MALLOC` block there is at least one byte, and it is not zero.
///
/// Um Jogo de Ovos reads the boss scene into a block of the exact size of the file and the parser
/// looks at the byte right after the end: a zero there, and no token is accepted. `b1i.cut` has
/// 1070 bytes and `b1o.cut`, 4392 — a multiple of eight, which without the slack would end flush
/// against the next block.
#[test]
fn every_malloc_block_has_nonzero_padding() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    for size in [1070u32, 4392, 0] {
        let block = machine.malloc(size).unwrap();
        let after = machine.malloc(16).unwrap();
        assert!(block != 0 && after > block + size, "{size}: the next block does not touch");
        let byte = machine.cpu.read_cbytes(block + size, 1);
        assert_eq!(byte, [thread::BLOCK_PADDING], "{size}: the byte after the end");
        assert!(
            machine.cpu.read_cbytes(block, size as usize).is_empty(),
            "{size}: what the game asked for is still zeroed"
        );
    }
}

/// `wstrtoutf8` writes real UTF-8, and the terminator only when it fits.
///
/// FIFA 09 takes the player's initial by converting **one** character into a **one**-byte buffer.
/// Reserving room for the zero, the initial came out empty and the HUD did not show the name of
/// players with a first and last name ("K. Chafni"); only those with a single name, like "Chris".
/// And the output went out in Latin-1: the "ô" in "Chantôme" became a byte that is not UTF-8.
#[test]
fn wstrtoutf8_writes_utf8_and_the_nul_only_if_it_fits() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let origin = loader::HEAP_BASE;
    let dest = loader::HEAP_BASE + 0x100;
    let wide = |text: &str| -> Vec<u8> {
        text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
    };
    let wstrtoutf8 = slot_of(Interface::Helpers, "wstrtoutf8");

    machine.cpu.write_mem(origin, &wide("Kamel")).unwrap();
    machine.cpu.write_mem(dest, &[0xee; 8]).unwrap();
    machine.cpu.write_reg(Reg::R3, 1);
    call(&mut machine, Interface::Helpers, wstrtoutf8, [origin, 1, dest, 1]);
    assert_eq!(machine.cpu.read_cbytes(dest, 2), [b'K', 0xee], "the initial, without a zero");

    machine.cpu.write_mem(origin, &wide("Chantôme")).unwrap();
    machine.cpu.write_reg(Reg::R3, 32);
    call(&mut machine, Interface::Helpers, wstrtoutf8, [origin, u32::MAX, dest, 32]);
    assert_eq!(machine.cpu.read_cbytes(dest, 32), "Chantôme".as_bytes(), "UTF-8 and terminated");
}

#[test]
fn brew_snprintf_counts_the_terminator() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let dest = loader::HEAP_BASE;
    let format = loader::HEAP_BASE + 0x100;
    let name = loader::HEAP_BASE + 0x200;
    machine.cpu.write_mem(format, b"%s=\0").unwrap();
    machine.cpu.write_mem(name, b"aCarNames\0").unwrap();
    let snprintf = slot_of(Interface::Helpers, "snprintf");

    assert_eq!(
        call(&mut machine, Interface::Helpers, snprintf, [dest, 64, format, name]),
        11,
        "ten letters and the zero"
    );
    assert_eq!(machine.cpu.read_cbytes(dest, 64), b"aCarNames=");
    // Without a buffer, the size the result would need, also with the zero.
    assert_eq!(call(&mut machine, Interface::Helpers, snprintf, [0, 0, format, name]), 11);
    // Truncated, whatever fit: four letters and the zero.
    assert_eq!(call(&mut machine, Interface::Helpers, snprintf, [dest, 5, format, name]), 5);
    assert_eq!(machine.cpu.read_cbytes(dest, 64), b"aCar");

    machine.cpu.write_mem(format, b"%s\0").unwrap();
    machine.cpu.write_mem(name, b"ARMAZ\xc3\x89M\0").unwrap();
    let sprintf = slot_of(Interface::Helpers, "sprintf");
    assert_eq!(call(&mut machine, Interface::Helpers, sprintf, [dest, format, name, 0]), 8);
    assert_eq!(machine.cpu.read_cbytes(dest, 64), b"ARMAZ\xc3\x89M");
}

#[test]
fn clearing_the_screen_paints_everything_with_the_background_color() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let display = machine.objects.create(Interface::Display).unwrap();
    let target = machine.device_bitmap().unwrap();
    machine.bitmaps.get_mut(&target).unwrap().fill_rect(
        Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        },
        Rgb::WHITE,
    );

    // Blue background, and the clear the game does: no rectangle, no colors, just the flag.
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "SetColor"),
        [
            display,
            CLR_USER_BACKGROUND as u32,
            to_rgbval(Rgb { r: 0, g: 0, b: 255 }),
            0,
        ],
    );
    machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
    machine
        .cpu
        .write_u32(loader::STACK_BASE + 0x1000, IDF_RECT_FILL)
        .unwrap();
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "DrawRect"),
        [display, 0, RGB_NONE, RGB_NONE],
    );
    let fb = machine.bitmaps.get(&target).unwrap();
    let blue = Rgb { r: 0, g: 0, b: 255 }.to_rgb565();
    assert_eq!(fb.get_pixel(0, 0), blue, "the corner was not cleared");
    assert_eq!(fb.get_pixel(639, 479), blue, "the other corner was not cleared");
}

/// Frame only means **frame only**: the fill color comes along and must not paint.
///
/// Quake calls it exactly like this — `IDF_RECT_FRAME` with black as the fill. Ignoring the flag
/// put down a black rectangle it did not ask for.
#[test]
fn only_the_border_when_the_flag_asks_for_only_the_border() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let display = machine.objects.create(Interface::Display).unwrap();
    let target = machine.device_bitmap().unwrap();
    machine.bitmaps.get_mut(&target).unwrap().fill_rect(
        Rect {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        },
        Rgb::WHITE,
    );

    let rect = loader::HEAP_BASE;
    for (i, value) in [10i16, 10, 40, 30].iter().enumerate() {
        machine
            .cpu
            .write_mem(rect + i as u32 * 2, &value.to_le_bytes())
            .unwrap();
    }
    machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
    machine
        .cpu
        .write_u32(loader::STACK_BASE + 0x1000, IDF_RECT_FRAME)
        .unwrap();
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "DrawRect"),
        [display, rect, to_rgbval(Rgb { r: 255, g: 0, b: 0 }), 0],
    );
    let fb = machine.bitmaps.get(&target).unwrap();
    let red = Rgb { r: 255, g: 0, b: 0 }.to_rgb565();
    assert_eq!(
        fb.get_pixel(10, 10),
        red,
        "the frame was not drawn"
    );
    assert_eq!(
        fb.get_pixel(25, 25),
        Rgb::WHITE.to_rgb565(),
        "the inside was filled without the game asking"
    );
}

#[test]
fn the_rectangle_is_cut_by_the_clip() {
    let cut = clip_rect(
        Some(CLIP),
        Rect {
            x: 90,
            y: 40,
            width: 100,
            height: 100,
        },
    )
    .unwrap();
    assert_eq!((cut.x, cut.y, cut.width, cut.height), (100, 50, 20, 10));
    // With no clip, the rectangle goes through whole. The opposite was written here, and it
    // erased every `DrawRect` from whoever does not set a clip — including the screen clear.
    let loose = Rect {
        x: 1,
        y: 2,
        width: 3,
        height: 4,
    };
    assert_eq!(clip_rect(None, loose), Some(loose));
    assert_eq!(
        clip_rect(
            Some(CLIP),
            Rect {
                x: 0,
                y: 0,
                width: 5,
                height: 5
            }
        ),
        None
    );
}
// The engine's default backend, not unicorn directly: on Windows ARM64 unicorn does not
// build (there is no MASM assembler for ARM64) and the default becomes dynarmic, which upholds
// the same contract. The local name stays `UnicornCpu` because that is what the tests call it.
use crate::cpu::DefaultBackend as UnicornCpu;
use crate::loader::modfile::ModImage;

/// Synthetic module that calls `MALLOC(16)` through the helpers table and returns the pointer.
///
/// ```asm
/// mov  r0, #16
/// ldr  r1, [r2, #0x68]   ; r2 = helpers table
/// mov  lr, pc            ; return = next instruction (bx lr)
/// bx   r1
/// bx   lr                ; goes to the sentinel? no: lr was overwritten
/// ```
///
/// After the call, `lr` points to the next instruction, so we finish by jumping to the
/// sentinel stored in r3.
fn module_calling_malloc() -> ModImage {
    let code = [
        0xe3a0_0010u32.to_le_bytes(), // mov r0, #16
        0xe592_1068u32.to_le_bytes(), // ldr r1, [r2, #0x68]
        0xe1a0_e00fu32.to_le_bytes(), // mov lr, pc  (points to bx r3)
        0xe12f_ff11u32.to_le_bytes(), // bx r1
        0xe12f_ff13u32.to_le_bytes(), // bx r3
    ]
    .concat();
    ModImage::parse(code).unwrap()
}

/// Calls a method directly through the dispatch, as if the guest had called it.
fn call(machine: &mut Machine<UnicornCpu>, iface: Interface, slot: u32, args: [u32; 4]) -> u32 {
    machine.cpu.write_reg(Reg::R0, args[0]);
    machine.cpu.write_reg(Reg::R1, args[1]);
    machine.cpu.write_reg(Reg::R2, args[2]);
    machine.cpu.write_reg(Reg::R3, args[3]);
    machine
        .dispatch(aee::encode(iface, slot))
        .unwrap()
        .expect("method should be implemented")
}

/// A minimal RIFF/WAVE, 8-bit mono PCM, with `frames` silent samples.
fn riff_pcm8(rate: u32, frames: u32) -> Vec<u8> {
    let mut wave = Vec::new();
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + frames).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16u32.to_le_bytes());
    wave.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wave.extend_from_slice(&1u16.to_le_bytes()); // mono
    wave.extend_from_slice(&rate.to_le_bytes());
    wave.extend_from_slice(&rate.to_le_bytes()); // bytes per second
    wave.extend_from_slice(&1u16.to_le_bytes()); // block alignment
    wave.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&frames.to_le_bytes());
    wave.extend(std::iter::repeat_n(128u8, frames as usize));
    wave
}

/// A method's slot by name. The search covers the whole table: the large tables — OpenGL and
/// the helpers — go well past the first slots.
fn slot_of(iface: Interface, name: &str) -> u32 {
    (0..256)
        .find(|&s| iface.method(s) == Some(name))
        .unwrap_or_else(|| panic!("{name} does not exist in {}", iface.name()))
}

#[test]
fn the_type_comes_from_the_signature_and_only_then_from_the_extension() {
    assert_eq!(detect_mime(b"\x89PNG\r\n\x1a\n...", ""), Some("image/png"));
    assert_eq!(detect_mime(b"RIFF\0\0\0\0WAVEfmt ", ""), Some("audio/wav"));
    assert_eq!(detect_mime(b"ID3\x03", ""), Some("audio/mpeg"));
    // With no recognizable signature, the name's extension counts.
    assert_eq!(detect_mime(b"", "musica.MP3"), Some("audio/mpeg"));
    assert_eq!(detect_mime(b"", "sem_extensao"), None);
    // And the signature takes precedence over the name, as the documentation says.
    assert_eq!(
        detect_mime(b"\x89PNG\r\n\x1a\n", "x.mp3"),
        Some("image/png")
    );
}

#[test]
fn the_handler_for_each_type_comes_from_the_sdk_class_ids() {
    assert_eq!(handler_for("image/png"), AEECLSID_PNG);
    assert_eq!(handler_for("audio/mpeg"), AEECLSID_MEDIAMP3);
    // A type nobody knows how to open returns zero, which is the expected answer.
    assert_eq!(handler_for("application/x-nada"), 0);
}

#[test]
fn strtoul_honors_the_base_and_reports_where_it_stopped() {
    assert_eq!(parse_unsigned("  42abc", 10), (42, 4));
    assert_eq!(parse_unsigned("0x1f", 0), (31, 4));
    assert_eq!(parse_unsigned("0x1f", 16), (31, 4));
    assert_eq!(parse_unsigned("017", 0), (15, 3));
    assert_eq!(parse_unsigned("ff", 16), (255, 2));
    // With no digit at all, `strtoul` returns zero and consumes nothing.
    assert_eq!(parse_unsigned("xyz", 10), (0, 0));
}

#[test]
fn strtod_reads_the_longest_numeric_prefix() {
    assert_eq!(parse_leading_double("3.5rest"), (3.5, 3));
    assert_eq!(parse_leading_double("3.5 x"), (3.5, 3));
    assert_eq!(parse_leading_double("  -2e3;"), (-2000.0, 6));
    assert_eq!(parse_leading_double("nada"), (0.0, 0));
}

#[test]
fn subsequence_search_finds_the_start() {
    assert_eq!(find_subslice(b"abcdef", b"cde"), Some(2));
    assert_eq!(find_subslice(b"abcdef", b"xyz"), None);
    assert_eq!(find_subslice(b"abc", b""), Some(0));
}

#[test]
fn wide_comparison_folds_case_only_for_ascii() {
    let upper: Vec<u16> = "ABC".encode_utf16().collect();
    let lower: Vec<u16> = "abc".encode_utf16().collect();
    assert_eq!(fold_case(&upper, usize::MAX), fold_case(&lower, usize::MAX));
    assert_eq!(fold_case(&upper, 1), fold_case(&lower, 1));
    assert_ne!(fold_case(&upper, 2), fold_case(&lower, 1));
}

/// Builds a machine with its own working directory, for the file tests.
fn machine_at(root: &std::path::Path) -> Machine<UnicornCpu> {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, root);
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine
}

/// A 2x1 PNG with alpha: one opaque red pixel and one fully transparent.
fn png_with_alpha() -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 2, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 0])
        .unwrap();
    out
}

#[test]
fn the_surface_scale_tells_the_size_the_game_draws_at() {
    // `EGL_QUALCOMM_surface_scale` is the console scaling up what the game drew small. When the
    // game uses it, it **states** the size — and that is worth more than the deduction from the
    // viewport, which exists only for those who do not state it.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let egl = machine.new_object(Interface::Egl).unwrap();
    let out = loader::HEAP_BASE;
    assert_eq!(
        call(
            &mut machine,
            Interface::Egl,
            slot_of(Interface::Egl, "QueryInterface"),
            [egl, AEEIID_EGL_SURFACE_MANIP, out, 0],
        ),
        SUCCESS
    );
    let manip = machine.cpu.read_u32(out).unwrap();
    assert_ne!(manip, 0);

    // AEEEGLSurfaceScaleRect { x, y, width, height } = 320x224, an arcade screen.
    let rect = loader::HEAP_BASE + 0x100;
    for (index, value) in [0u32, 0, 320, 224].iter().enumerate() {
        machine
            .cpu
            .write_u32(rect + index as u32 * 4, *value)
            .unwrap();
    }
    // `dst` is the fourth argument and `ret` the fifth, counting from `dpy`: with `this` in r0,
    // both go on the stack. `ret` is where the game reads the answer. Writing it one slot early
    // wrote nothing when `dst` was null (the usual case) and clobbered `dst` when it was not,
    // so the game saw its own uninitialised stack and gave up on the extension.
    let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 / 2;
    let ret = loader::HEAP_BASE + 0x200;
    machine.cpu.write_reg(Reg::Sp, sp);
    machine.cpu.write_u32(sp, 0).unwrap();
    machine.cpu.write_u32(sp + 4, ret).unwrap();
    machine.cpu.write_u32(ret, 0).unwrap();
    assert_eq!(
        call(
            &mut machine,
            Interface::EglSurfaceManip,
            slot_of(Interface::EglSurfaceManip, "SetSurfaceScale"),
            [manip, 1, 1, rect],
        ),
        SUCCESS
    );
    assert_eq!(machine.gl.surface(), (320, 224));
    assert_eq!(machine.cpu.read_u32(ret).unwrap(), 1, "ret says EGL_TRUE");
}

#[test]
fn sleeping_advances_the_clock() {
    // On the console the ARM really stops and time passes on its own. Here the clock moves with
    // the instructions, so a `sleep` that does not advance it sleeps forever — that was the loop
    // Magical Drop 3 was stuck in, with a hundred million calls.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let before = machine.now_ms();
    call(
        &mut machine,
        Interface::Helpers,
        slot_of(Interface::Helpers, "sleep"),
        [50, 0, 0, 0],
    );
    assert!(machine.now_ms() >= before + 50, "the clock moved the 50 ms");
}

#[test]
fn the_decoder_takes_the_png_through_force_feed_and_returns_the_bitmap() {
    // The whole path games use: create the decoder, ask for the input interface, write the
    // file in pieces, close with an empty write and fetch the bitmap.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let decoder = machine.new_object(Interface::ImageDecoder).unwrap();
    let out = loader::HEAP_BASE;
    assert_eq!(
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "QueryInterface"),
            [decoder, AEEIID_FORCEFEED, out, 0],
        ),
        SUCCESS
    );
    let feed = machine.cpu.read_u32(out).unwrap();
    // `IForceFeed` is a **separate** object: the two interfaces have different methods in the
    // same slot, and returning the same pointer would make the game call the wrong method.
    assert_ne!(feed, decoder);

    // Delivered in two pieces, as a game that reads the file bit by bit would do.
    let png = png_with_alpha();
    let buffer = loader::HEAP_BASE + 0x100;
    machine.cpu.write_mem(buffer, &png).unwrap();
    let write = slot_of(Interface::ForceFeed, "Write");
    let half = png.len() as u32 / 2;
    call(
        &mut machine,
        Interface::ForceFeed,
        write,
        [feed, buffer, half, 0],
    );
    call(
        &mut machine,
        Interface::ForceFeed,
        write,
        [feed, buffer + half, png.len() as u32 - half, 0],
    );
    call(&mut machine, Interface::ForceFeed, write, [feed, 0, 0, 0]);

    let bitmap_out = loader::HEAP_BASE + 0x800;
    assert_eq!(
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "GetBitmap"),
            [decoder, bitmap_out, 0, 0],
        ),
        SUCCESS
    );
    let bitmap = machine.cpu.read_u32(bitmap_out).unwrap();
    let fb = machine.bitmaps.get(&bitmap).expect("the bitmap exists");
    assert_eq!((fb.width(), fb.height()), (2, 1));
    assert_eq!(fb.get_pixel(0, 0), Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
    // The transparent pixel becomes the reserved color, and `GetRop` tells the game so.
    assert_eq!(fb.get_pixel(1, 0), TRANSPARENT_KEY);
    assert_eq!(
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "GetRop"),
            [decoder, 0, 0, 0],
        ),
        AEE_RO_TRANSPARENT
    );
    // The decoded bitmap already arrives with the public `IDIB` fields filled in, without waiting
    // for a `QueryInterface`: on the console a software `IBitmap` **is** an `IDIB`, and the game
    // reads the size straight from the fields. Peggle is the one that demanded it — without it, it
    // read 0×0, built every sprite as a zero-sided square and 99% of the frame's triangles came
    // out degenerate, with a black screen and the game drawing all the time.
    let mut cx = [0u8; 2];
    machine.cpu.read_mem(bitmap + 20, &mut cx).unwrap();
    assert_eq!(u16::from_le_bytes(cx), 2);
    let mut cy = [0u8; 2];
    machine.cpu.read_mem(bitmap + 22, &mut cy).unwrap();
    assert_eq!(u16::from_le_bytes(cy), 1);
    assert!(machine.cpu.read_u32(bitmap + 8).unwrap() >= loader::SURFACE_BASE);

    // Asking again returns the same bitmap, not a new copy on every call.
    call(
        &mut machine,
        Interface::ImageDecoder,
        slot_of(Interface::ImageDecoder, "GetBitmap"),
        [decoder, bitmap_out, 0, 0],
    );
    assert_eq!(machine.cpu.read_u32(bitmap_out).unwrap(), bitmap);
}

#[test]
fn the_julian_date_counts_from_the_brew_epoch() {
    // Zero is the epoch instant itself: January 6, 1980, a Sunday.
    assert_eq!(julian_date(0), [1980, 1, 6, 0, 0, 0, 0]);
    // One day later, Monday.
    assert_eq!(julian_date(86_400), [1980, 1, 7, 0, 0, 0, 1]);
    // The time of day comes from the remainder, and the day of the week wraps around at seven.
    assert_eq!(
        julian_date(86_400 * 7 + 3600 * 13 + 60 * 24 + 35),
        [1980, 1, 13, 13, 24, 35, 0]
    );
    // End of February in a leap year, which is where a naive count goes wrong.
    assert_eq!(julian_date(86_400 * 54), [1980, 2, 29, 0, 0, 0, 5]);
    assert_eq!(julian_date(86_400 * 55), [1980, 3, 1, 0, 0, 0, 6]);
    // And the turn of the century: 2000 was a leap year, which 1900 would not be.
    assert_eq!(julian_date(86_400 * 7300), [2000, 1, 1, 0, 0, 0, 6]);
    assert_eq!(julian_date(86_400 * 7361), [2000, 3, 2, 0, 0, 0, 4]);
}

#[test]
fn the_decoder_rejects_what_is_not_png() {
    // Refusing with the reason avoids the hunt for an image that vanishes without explanation.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let decoder = machine.new_object(Interface::ImageDecoder).unwrap();
    let out = loader::HEAP_BASE;
    call(
        &mut machine,
        Interface::ImageDecoder,
        slot_of(Interface::ImageDecoder, "QueryInterface"),
        [decoder, AEEIID_FORCEFEED, out, 0],
    );
    let feed = machine.cpu.read_u32(out).unwrap();
    let buffer = loader::HEAP_BASE + 0x100;
    machine.cpu.write_mem(buffer, b"nao e um png").unwrap();
    call(
        &mut machine,
        Interface::ForceFeed,
        slot_of(Interface::ForceFeed, "Write"),
        [feed, buffer, 12, 0],
    );
    let bitmap_out = loader::HEAP_BASE + 0x800;
    assert_eq!(
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "GetBitmap"),
            [decoder, bitmap_out, 0, 0],
        ),
        EFAILED
    );
    assert_eq!(machine.cpu.read_u32(bitmap_out).unwrap(), 0);
}

#[test]
fn a_native_image_becomes_a_bitmap_the_blit_understands() {
    // The returned pointer goes straight to `IDISPLAY_BitBlt`, which takes an `IBitmap *`.
    // Returning a loose block of pixels would force the drawing to have a second path.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // A 24-bit BMP, 2x1: red and blue (BMP stores BGR, bottom to top).
    let mut bmp = vec![0u8; 54];
    bmp[0..2].copy_from_slice(b"BM");
    bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
    bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
    bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
    bmp[22..26].copy_from_slice(&1u32.to_le_bytes());
    bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
    bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]);
    let size = bmp.len() as u32;
    bmp[2..6].copy_from_slice(&size.to_le_bytes());

    let buffer = loader::HEAP_BASE;
    machine.cpu.write_mem(buffer, &bmp).unwrap();
    let info = buffer + 0x200;
    let realloc = buffer + 0x300;

    let bitmap = call(
        &mut machine,
        Interface::Helpers,
        slot_of(Interface::Helpers, "SetupNativeImage"),
        [AEECLSID_WINBMP, buffer, info, realloc],
    );
    assert_ne!(bitmap, 0, "returns the converted bitmap");

    let fb = machine.bitmaps.get(&bitmap).expect("the bitmap exists");
    assert_eq!((fb.width(), fb.height()), (2, 1));
    assert_eq!(fb.get_pixel(0, 0), Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
    assert_eq!(fb.get_pixel(1, 0), Rgb { r: 0, g: 0, b: 255 }.to_rgb565());

    // The `AEEImageInfo` and the notice that the memory is ours.
    let mut cx = [0u8; 2];
    machine.cpu.read_mem(info, &mut cx).unwrap();
    assert_eq!(u16::from_le_bytes(cx), 2);
    let mut flag = [0u8; 1];
    machine.cpu.read_mem(realloc, &mut flag).unwrap();
    assert_eq!(flag[0], 1, "the image came out in an allocation of ours");
}

/// Two images of different sizes at the **same object address**, and the second one's `IDIB`
/// must describe the second one.
///
/// It is the defect that erased Tekken 2's text. It decodes nine images in sequence, freeing each
/// one before the next, so they are all born at the same address. The `IDIB` announced the first
/// one's size for all of them: the game created a 200x112 page for a 360x280 letter sheet, kept
/// only its corner and then asked for each glyph by coordinates on the whole sheet. Whatever fell
/// outside the page became a block, and the menu came out with the words as orange rectangles.
#[test]
fn the_dib_follows_an_image_swap_at_the_same_address() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    /// A gray 24-bit BMP, of the requested size.
    fn bmp(width: u32, height: u32) -> Vec<u8> {
        let mut out = vec![0u8; 54];
        out[0..2].copy_from_slice(b"BM");
        out[10..14].copy_from_slice(&54u32.to_le_bytes());
        out[14..18].copy_from_slice(&40u32.to_le_bytes());
        out[18..22].copy_from_slice(&width.to_le_bytes());
        out[22..26].copy_from_slice(&height.to_le_bytes());
        out[26..28].copy_from_slice(&1u16.to_le_bytes());
        out[28..30].copy_from_slice(&24u16.to_le_bytes());
        let step = (width * 3).div_ceil(4) * 4;
        out.extend(std::iter::repeat_n(0x80u8, (step * height) as usize));
        let size = out.len() as u32;
        out[2..6].copy_from_slice(&size.to_le_bytes());
        out
    }

    let buffer = loader::HEAP_BASE;
    let info = buffer + 0x4000;
    let realloc = info + 0x100;
    let output = realloc + 0x100;

    let convert = |machine: &mut Machine<UnicornCpu>, image: &[u8]| -> u32 {
        machine.cpu.write_mem(buffer, image).unwrap();
        let bitmap = call(
            machine,
            Interface::Helpers,
            slot_of(Interface::Helpers, "SetupNativeImage"),
            [AEECLSID_WINBMP, buffer, info, realloc],
        );
        assert_ne!(bitmap, 0);
        // The game asks for the `IDIB` to read the public fields, which is how it finds out the
        // image size.
        call(
            machine,
            Interface::Bitmap,
            slot_of(Interface::Bitmap, "QueryInterface"),
            [bitmap, AEECLSID_DIB, output, 0],
        );
        bitmap
    };
    let size_in_dib = |machine: &Machine<UnicornCpu>, bitmap: u32| -> (u16, u16, i16) {
        let mut fields = [0u8; 6];
        machine.cpu.read_mem(bitmap + 20, &mut fields).unwrap();
        (
            u16::from_le_bytes([fields[0], fields[1]]),
            u16::from_le_bytes([fields[2], fields[3]]),
            i16::from_le_bytes([fields[4], fields[5]]),
        )
    };

    let first = convert(&mut machine, &bmp(8, 4));
    assert_eq!(size_in_dib(&machine, first), (8, 4, 16));

    // The game frees the image before converting the next one, and the address is reused.
    while machine.objects.release(first) > 0 {}
    let second = convert(&mut machine, &bmp(20, 10));
    assert_eq!(second, first, "the address had to be recycled");
    assert_eq!(
        size_in_dib(&machine, second),
        (20, 10, 40),
        "the IDIB kept describing the previous image"
    );

    // And the published buffer must fit the whole new image: with the old buffer, the pixels at
    // the end would be left out and the sheet would come out cut off.
    let pixels = machine.dib_buffers.get(&second).copied().unwrap();
    let mut last = [0u8; 2];
    machine
        .cpu
        .read_mem(pixels + 20 * 10 * 2 - 2, &mut last)
        .unwrap();
    assert_ne!(
        u16::from_le_bytes(last),
        0,
        "the last pixel was not published"
    );
}

#[test]
fn a_native_image_we_cannot_read_returns_null() {
    // Returning null is what BREW does when it does not convert, and the game has a path for it.
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let buffer = loader::HEAP_BASE;
    machine.cpu.write_mem(buffer, b"nao e um bmp").unwrap();
    let out = call(
        &mut machine,
        Interface::Helpers,
        slot_of(Interface::Helpers, "SetupNativeImage"),
        [AEECLSID_WINBMP, buffer, 0, 0],
    );
    assert_eq!(out, 0);
}

#[test]
fn enumeration_lists_files_and_directories_separately() {
    // The game asks for one thing or the other, never both: the `bDirs` of `EnumInit` decides.
    let root = std::env::temp_dir().join("zeebx-testes-enum");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("pasta")).unwrap();
    std::fs::write(root.join("b.dat"), b"12345").unwrap();
    std::fs::write(root.join("a.dat"), b"1").unwrap();

    let mut machine = machine_at(&root);
    let mgr = machine.new_object(Interface::FileMgr).unwrap();
    let dir = loader::HEAP_BASE;
    machine.cpu.write_mem(dir, b"fs:/~/\0").unwrap();
    let info = loader::HEAP_BASE + 0x100;

    let names = |machine: &mut Machine<UnicornCpu>, dirs: u32| {
        call(
            machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumInit"),
            [mgr, dir, dirs, 0],
        );
        let mut out = Vec::new();
        while call(
            machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumNext"),
            [mgr, info, 0, 0],
        ) == TRUE
        {
            out.push(machine.cpu.read_cstring(info + 12, MAX_FILE_NAME));
        }
        out
    };

    // The path comes back with the prefix the game passed: it is what goes to `OpenFile`.
    assert_eq!(names(&mut machine, FALSE), ["fs:/~/a.dat", "fs:/~/b.dat"]);
    assert_eq!(names(&mut machine, TRUE), ["fs:/~/pasta"]);

    // The size of the last file read comes along.
    call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumInit"),
        [mgr, dir, FALSE, 0],
    );
    call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumNext"),
        [mgr, info, 0, 0],
    );
    assert_eq!(
        machine.cpu.read_u32(info + 8).unwrap(),
        1,
        "a.dat has 1 byte"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// **The save the game wrote joins the list, with its size, and does not end it.** The file only
/// exists in the overlay, and the metadata was looked up in the package: the list ended there,
/// and whatever came after it in the order vanished. It was Ultimate Chess 3D with a saved game,
/// which lost its pieces and crashed on opening.
#[test]
fn an_overlay_only_save_joins_the_list_without_dropping_the_rest() {
    let base = std::env::temp_dir().join(format!("zeebx-testes-enum-overlay-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (root, save) = (base.join("jogo"), base.join("save"));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&save).unwrap();
    std::fs::write(root.join("a.dat"), b"1").unwrap();
    std::fs::write(root.join("z.dat"), b"1").unwrap();
    std::fs::write(save.join("m.sv2"), b"partida").unwrap();

    let mut machine = machine_at(&root);
    machine.vfs.set_save_root(&save);
    let mgr = machine.new_object(Interface::FileMgr).unwrap();
    let dir = loader::HEAP_BASE;
    machine.cpu.write_mem(dir, b"fs:/~/\0").unwrap();
    let info = loader::HEAP_BASE + 0x100;
    call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumInit"),
        [mgr, dir, FALSE, 0],
    );
    let mut seen = Vec::new();
    while call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumNext"),
        [mgr, info, 0, 0],
    ) == TRUE
    {
        let name = machine.cpu.read_cstring(info + 12, MAX_FILE_NAME);
        seen.push((name, machine.cpu.read_u32(info + 8).unwrap()));
    }
    assert_eq!(
        seen,
        [
            ("fs:/~/a.dat".to_string(), 1),
            ("fs:/~/m.sv2".to_string(), 7),
            ("fs:/~/z.dat".to_string(), 1),
        ]
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// **A long string has its own length, not the ceiling's.** The Alice in Wonderland stages keep
/// the map in a single-line base64 string, from 15 thousand to 200 thousand characters, and with
/// the four-KB ceiling `strlen` returned 4,096: the map arrived cut off, without the bottom rows.
#[test]
fn strlen_of_a_long_string_does_not_stop_at_the_old_cap() {
    let root = std::env::temp_dir().join(format!("zeebx-testes-strlen-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut machine = machine_at(&root);
    let text = loader::HEAP_BASE;
    let mut bytes = vec![b'A'; 20_000];
    bytes.push(0);
    machine.cpu.write_mem(text, &bytes).unwrap();
    let size = call(
        &mut machine,
        Interface::Helpers,
        slot_of(Interface::Helpers, "strlen"),
        [text, 0, 0, 0],
    );
    assert_eq!(size, 20_000);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_end_of_enumeration_leaves_the_error_brew_specifies() {
    // After an `EnumNext` that returns false, `GetLastError` returns `EFAILED` even though
    // everything went fine. The documentation calls this compatibility with the 1.0 client, and
    // there are games that check it.
    let root = std::env::temp_dir().join("zeebx-testes-enum-fim");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let mut machine = machine_at(&root);
    let mgr = machine.new_object(Interface::FileMgr).unwrap();
    let dir = loader::HEAP_BASE;
    machine.cpu.write_mem(dir, b"fs:/~/\0").unwrap();

    call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumInit"),
        [mgr, dir, FALSE, 0],
    );
    let more = call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "EnumNext"),
        [mgr, loader::HEAP_BASE + 0x100, 0, 0],
    );
    assert_eq!(more, FALSE, "an empty directory has no next entry");
    let error = call(
        &mut machine,
        Interface::FileMgr,
        slot_of(Interface::FileMgr, "GetLastError"),
        [mgr, 0, 0, 0],
    );
    assert_eq!(error, EFAILED);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn preferences_come_back_as_they_were_written() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let shell = machine.new_object(Interface::Shell).unwrap();
    let buffer = loader::HEAP_BASE;
    machine.cpu.write_mem(buffer, &[1, 2, 3, 4]).unwrap();

    // `nSize` is the fifth argument, and the fifth goes on the stack.
    let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 / 2;
    machine.cpu.write_reg(Reg::Sp, sp);
    machine.cpu.write_u32(sp, 4).unwrap();
    let set = slot_of(Interface::Shell, "SetPrefs");
    assert_eq!(
        call(&mut machine, Interface::Shell, set, [shell, 7, 1, buffer]),
        SUCCESS
    );

    let out = buffer + 0x100;
    let get = slot_of(Interface::Shell, "GetPrefs");
    assert_eq!(
        call(&mut machine, Interface::Shell, get, [shell, 7, 1, out]),
        SUCCESS
    );
    let mut bytes = [0u8; 4];
    machine.cpu.read_mem(out, &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);

    // A class that was never saved fails, which is what BREW answers.
    assert_eq!(
        call(&mut machine, Interface::Shell, get, [shell, 99, 1, out]),
        EFAILED
    );
}

/// The complete cooperative thread: it starts, yields control at `Suspend` and comes back from
/// the exact point where it stopped when `ISHELL_Resume` fires the resume callback.
#[test]
fn a_cooperative_thread_yields_and_resumes_where_it_stopped() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let thread = machine.new_object(Interface::Thread).unwrap();
    let start = slot_of(Interface::Thread, "Start");
    assert_eq!(
        call(
            &mut machine,
            Interface::Thread,
            start,
            [thread, 4096, 0x9000, 7]
        ),
        SUCCESS
    );
    // Starting twice is `EALREADY`: BREW's `IThread` is not reusable.
    assert_eq!(
        call(
            &mut machine,
            Interface::Thread,
            start,
            [thread, 4096, 0x9000, 7]
        ),
        EALREADY
    );
    assert_eq!(machine.pending_threads, vec![thread]);
    machine.pending_threads.clear();

    // `Suspend` freezes the registers and gives control back by swapping `lr` for the return
    // sentinel.
    let suspend = slot_of(Interface::Thread, "Suspend");
    machine.cpu.write_reg(Reg::R4, 0xabc);
    machine.cpu.write_reg(Reg::Lr, 0x1234);
    call(&mut machine, Interface::Thread, suspend, [thread, 0, 0, 0]);
    assert_eq!(machine.cpu.read_reg(Reg::Lr), RETURN_MAGIC);
    let state = &machine.threads[&thread];
    assert!(state.suspended);
    assert_eq!(state.resume_pc, 0x1234);
    assert_eq!(state.context[4], 0xabc);

    // `GetResumeCBK` always returns the same callback, and that is how `ISHELL_Resume`
    // recognizes that the one wanting to come back is the thread.
    let get_cbk = slot_of(Interface::Thread, "GetResumeCBK");
    let cbk = call(&mut machine, Interface::Thread, get_cbk, [thread, 0, 0, 0]);
    assert_ne!(cbk, 0);
    assert_eq!(
        call(&mut machine, Interface::Thread, get_cbk, [thread, 0, 0, 0]),
        cbk
    );

    let shell = machine.module.shell;
    let resume = slot_of(Interface::Shell, "Resume");
    call(&mut machine, Interface::Shell, resume, [shell, cbk, 0, 0]);
    assert_eq!(machine.pending_threads, vec![thread]);
}

/// BREW's newer interfaces deliver the result through an output pointer, and EGL's
/// `QueryInterface` must return a GL object — not itself.
#[test]
fn shell_only_starts_an_installed_applet_and_queues_once() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.set_installed_applets([(0x123456, "279369".to_string())]);
    let start = slot_of(Interface::Shell, "StartApplet");
    let can = slot_of(Interface::Shell, "CanStartApplet");
    // `CanStartApplet` is boolean: true means it can. `SUCCESS` would be zero, "cannot".
    assert_eq!(call(&mut machine, Interface::Shell, can, [0, 0x123456, 0, 0]), 1);
    assert_eq!(call(&mut machine, Interface::Shell, can, [0, 0x654321, 0, 0]), 0);
    assert_eq!(machine.take_launch_request(), None);
    assert_eq!(
        call(&mut machine, Interface::Shell, start, [0, 0x654321, 0, 0]),
        ECLASSNOTSUPPORT
    );
    assert_eq!(machine.take_launch_request(), None);
    assert_eq!(
        call(&mut machine, Interface::Shell, start, [0, 0x123456, 0, 0]),
        SUCCESS
    );
    assert_eq!(machine.take_launch_request(), Some(0x123456));
    assert_eq!(machine.take_launch_request(), None);
}

/// The Z-Wheel walks the installed applets before launching and uses each one's `.mif` name.
#[test]
fn enum_next_applet_yields_class_and_mif_of_each_installed() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.set_installed_applets([(0x123456, "279369".to_string())]);
    let init = slot_of(Interface::Shell, "EnumAppletInit");
    let next = slot_of(Interface::Shell, "EnumNextApplet");
    let info = machine.heap.alloc(16).unwrap();

    assert_eq!(call(&mut machine, Interface::Shell, init, [0, 0, 0, 0]), SUCCESS);
    assert_eq!(call(&mut machine, Interface::Shell, next, [0, info, 0, 0]), 1);
    assert_eq!(machine.cpu.read_u32(info).unwrap(), 0x123456);
    let mif = machine.cpu.read_u32(info + 4).unwrap();
    assert_eq!(machine.cpu.read_cstring(mif, 64), "fs:/mif/279369.mif");
    assert_eq!(call(&mut machine, Interface::Shell, next, [0, info, 0, 0]), 0, "finished");
}

#[test]
fn qualcomm_buffer_imports_the_background_before_drawing() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.gl = Box::new(GlState::new(2, 1));
    machine.egl_surface = 1;
    machine.egl_surfaces.insert(1, (2, 1));
    let get = slot_of(Interface::EglLegacy, "eglGetColorBufferQUALCOMM");
    let buffer = call(&mut machine, Interface::EglLegacy, get, [0; 4]);
    machine
        .cpu
        .write_mem(buffer, &0xf800u16.to_le_bytes())
        .unwrap();
    // Even a call with no vertices must sync the buffer before drawing.
    let draw = slot_of(Interface::GlLegacy, "glDrawArrays");
    call(
        &mut machine,
        Interface::GlLegacy,
        draw,
        [gles::GL_TRIANGLES, 0, 0, 0],
    );
    assert_eq!(frame_565(&mut machine, 2, 1)[0], 0xf800);
    // A later clear must not be undone by an old copy from the guest.
    let clear = slot_of(Interface::GlLegacy, "glClear");
    call(
        &mut machine,
        Interface::GlLegacy,
        clear,
        [gles::GL_COLOR_BUFFER_BIT, 0, 0, 0],
    );
    let again = call(&mut machine, Interface::EglLegacy, get, [0; 4]);
    assert_eq!(machine.cpu.read_u32(again).unwrap(), 0);
}

/// `skip_draw` hides the game's `Clear`, without returning any error to it.
///
/// **The proof is the pixel, not the return value.** The BREW API has no "refused because of a
/// frame skip" — the game always sees success, because real hardware does not report whether the
/// pixel reached the screen either. What changes, and what is checked here, is that the fill
/// **does not happen** while the flag is on, and happens again as soon as it is off — the same
/// call, two outcomes.
#[test]
fn skipping_draw_hides_the_clear_without_an_error_for_the_game() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.gl = Box::new(GlState::new(2, 1));
    machine.egl_surface = 1;
    machine.egl_surfaces.insert(1, (2, 1));

    let clear_color = slot_of(Interface::GlLegacy, "glClearColor");
    let clear = slot_of(Interface::GlLegacy, "glClear");

    // Color A, no skipping: the frame turns white. It is the proof that the normal path works.
    let white = 1.0f32.to_bits();
    let r = call(
        &mut machine,
        Interface::GlLegacy,
        clear_color,
        [white, white, white, white],
    );
    assert_eq!(r, SUCCESS);
    let r = call(
        &mut machine,
        Interface::GlLegacy,
        clear,
        [gles::GL_COLOR_BUFFER_BIT, 0, 0, 0],
    );
    assert_eq!(r, SUCCESS, "the game must not see any error, skipping or not");
    assert_eq!(frame_565(&mut machine, 2, 1), vec![0xffff, 0xffff], "color A did not paint");

    // Color B, skipping: the frame **stays white** — the Clear did not really happen.
    machine.set_skip_draw(true);
    let black = 0.0f32.to_bits();
    call(
        &mut machine,
        Interface::GlLegacy,
        clear_color,
        [black, black, black, black],
    );
    let r = call(
        &mut machine,
        Interface::GlLegacy,
        clear,
        [gles::GL_COLOR_BUFFER_BIT, 0, 0, 0],
    );
    assert_eq!(r, SUCCESS, "skipping a frame must not show up as an error to the game");
    assert_eq!(
        frame_565(&mut machine, 2, 1),
        vec![0xffff, 0xffff],
        "with skip_draw on, color B must not have painted"
    );

    // Turning it off, the same call paints again — the flag is per frame, not permanent.
    machine.set_skip_draw(false);
    let r = call(
        &mut machine,
        Interface::GlLegacy,
        clear,
        [gles::GL_COLOR_BUFFER_BIT, 0, 0, 0],
    );
    assert_eq!(r, SUCCESS);
    assert_eq!(
        frame_565(&mut machine, 2, 1),
        vec![0x0000, 0x0000],
        "with skipping off, color B (black) had to paint"
    );
}

#[test]
fn egl_answers_through_out_pointers_and_keeps_gl_apart() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let egl = machine.new_object(Interface::Egl).unwrap();
    let out = loader::HEAP_BASE + 0x800;

    let get_display = slot_of(Interface::Egl, "GetDisplay");
    assert_eq!(
        call(&mut machine, Interface::Egl, get_display, [egl, 0, out, 0]),
        SUCCESS
    );
    assert_eq!(machine.cpu.read_u32(out).unwrap(), EGL_DISPLAY);

    // `GetConfigAttrib(this, dpy, config, attribute, *value, *ret)`: the last two arguments go
    // on the stack.
    let attrib = slot_of(Interface::Egl, "GetConfigAttrib");
    let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 - 64;
    machine.cpu.write_reg(Reg::Sp, sp);
    machine.cpu.write_u32(sp, out).unwrap();
    machine.cpu.write_u32(sp + 4, out + 4).unwrap();
    call(
        &mut machine,
        Interface::Egl,
        attrib,
        [egl, EGL_DISPLAY, EGL_CONFIG, gles::EGL_GREEN_SIZE],
    );
    assert_eq!(machine.cpu.read_u32(out).unwrap(), 6);
    assert_eq!(machine.cpu.read_u32(out + 4).unwrap(), gles::EGL_TRUE);

    // The GL object is a different one: returning `this` would hand the EGL vtable to whoever
    // asked for OpenGL's, and the first call would land on a slot that does not exist.
    let query = slot_of(Interface::Egl, "QueryInterface");
    assert_eq!(
        call(
            &mut machine,
            Interface::Egl,
            query,
            [egl, AEEIID_GLES11, out, 0]
        ),
        SUCCESS
    );
    let gl = machine.cpu.read_u32(out).unwrap();
    assert_ne!(gl, egl);
    assert_eq!(aee::decode(machine.cpu.read_u32(gl).unwrap()), None);

    // An interface we do not know must become a refusal, not a wrong object.
    assert_eq!(
        call(
            &mut machine,
            Interface::Egl,
            query,
            [egl, 0x1234_5678, out, 0]
        ),
        ECLASSNOTSUPPORT
    );
    assert_eq!(machine.cpu.read_u32(out).unwrap(), 0);
}

#[test]
fn creates_a_signal_and_queues_the_callback() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let factory = machine.new_object(Interface::SignalCbFactory).unwrap();
    // CreateSignal(pfn, pCx, ppiSig, ppiSigCtl): the fourth argument goes on the stack.
    let out_signal = loader::HEAP_BASE;
    let out_control = loader::HEAP_BASE + 4;
    machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
    machine
        .cpu
        .write_u32(loader::STACK_BASE + 0x1000, out_control)
        .unwrap();
    let code = call(
        &mut machine,
        Interface::SignalCbFactory,
        slot_of(Interface::SignalCbFactory, "CreateSignal"),
        [factory, 0xcafe, 0xbeef, out_signal],
    );
    assert_eq!(code, SUCCESS);

    let signal = machine.cpu.read_u32(out_signal).unwrap();
    let control = machine.cpu.read_u32(out_control).unwrap();
    assert_ne!(signal, 0);
    assert_ne!(control, 0);
    assert_ne!(signal, control, "signal and control are distinct objects");

    // Firing the signal queues the callback with the context the app registered.
    call(
        &mut machine,
        Interface::Signal,
        slot_of(Interface::Signal, "Set"),
        [signal, 0, 0, 0],
    );
    assert_eq!(
        machine.pending_signals,
        vec![Callback {
            function: 0xcafe,
            context: 0xbeef
        }]
    );
}

#[test]
fn draws_to_the_screen_through_ibitmap() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // The game takes the screen surface and paints a red rectangle on it.
    let display = machine.new_object(Interface::Display).unwrap();
    let out_bitmap = loader::HEAP_BASE + 64;
    let code = call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "GetDeviceBitmap"),
        [display, out_bitmap, 0, 0],
    );
    assert_eq!(
        code, SUCCESS,
        "GetDeviceBitmap returns an error code, not the pointer"
    );
    let bitmap = machine.cpu.read_u32(out_bitmap).unwrap();
    assert_ne!(bitmap, 0);

    let red = call(
        &mut machine,
        Interface::Bitmap,
        slot_of(Interface::Bitmap, "RGBToNative"),
        [bitmap, 0x0000_ff00, 0, 0],
    );
    assert_eq!(red, Rgb { r: 255, g: 0, b: 0 }.to_rgb565() as u32);

    // AEERect { x: 10, y: 20, width: 30, height: 40 } written to guest memory.
    let rect_addr = loader::HEAP_BASE;
    machine
        .cpu
        .write_mem(rect_addr, &[10, 0, 20, 0, 30, 0, 40, 0])
        .unwrap();
    call(
        &mut machine,
        Interface::Bitmap,
        slot_of(Interface::Bitmap, "FillRect"),
        [bitmap, rect_addr, red, 0],
    );

    assert!(machine.screen().is_dirty());
    assert_eq!(machine.screen().get_pixel(15, 25), red as u16);
    assert_eq!(
        machine.screen().get_pixel(5, 5),
        0,
        "outside the rectangle stays intact"
    );
}

#[test]
fn a_compatible_bitmap_is_created_at_the_requested_size() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let display = machine.new_object(Interface::Display).unwrap();
    let out_screen = loader::HEAP_BASE + 64;
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "GetDeviceBitmap"),
        [display, out_screen, 0, 0],
    );
    let screen = machine.cpu.read_u32(out_screen).unwrap();
    let out = loader::HEAP_BASE;
    call(
        &mut machine,
        Interface::Bitmap,
        slot_of(Interface::Bitmap, "CreateCompatibleBitmap"),
        [screen, out, 64, 32],
    );

    let created = machine.cpu.read_u32(out).unwrap();
    assert_ne!(created, 0);
    assert_ne!(created, screen);
    let info = machine.bitmaps.get(&created).unwrap();
    assert_eq!((info.width(), info.height()), (64, 32));
}

#[test]
fn widget_slot13_returns_a_usable_surface() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let widget = machine.new_object(Interface::Widget).unwrap();
    let out = loader::HEAP_BASE;
    call(&mut machine, Interface::Widget, 13, [widget, out, 640, 100]);
    let bitmap = machine.cpu.read_u32(out).unwrap();
    assert_ne!(bitmap, 0);
    assert_eq!(machine.objects.kind_of(bitmap), Some(Interface::Bitmap));
    let surface = &machine.bitmaps[&bitmap];
    assert_eq!((surface.width(), surface.height()), (640, 100));
}

#[test]
fn widget_slot17_owns_the_roller_model() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let roller = machine.new_object(Interface::Widget).unwrap();
    let source = machine.new_object(Interface::Widget).unwrap();
    machine.widgets.insert(
        roller,
        Widget {
            visible: true,
            ..Widget::default()
        },
    );

    call(
        &mut machine,
        Interface::Widget,
        17,
        [roller, 0x8000, source, 0],
    );
    assert_eq!(machine.widgets[&roller].models.get(&0x8000), Some(&source));

    // The module releases the font after attaching it; the roller still owns it.
    call(&mut machine, Interface::Widget, 1, [source, 0, 0, 0]);
    assert_eq!(machine.objects.kind_of(source), Some(Interface::Widget));

    let other_source = machine.new_object(Interface::Widget).unwrap();
    call(
        &mut machine,
        Interface::Widget,
        17,
        [roller, 0x8000, other_source, 0],
    );
    assert_eq!(
        machine.objects.kind_of(source),
        None,
        "swapping releases the previous model"
    );

    call(&mut machine, Interface::Widget, 1, [other_source, 0, 0, 0]);
    call(&mut machine, Interface::Widget, 1, [roller, 0, 0, 0]);
    assert_eq!(machine.objects.kind_of(other_source), None);
}

/// The body of Zeeboids' `import` comes as raw `deflate`, and that of the other responses comes
/// as text. The bridge must inflate the first without breaking the second.
#[test]
fn the_inflater_recognizes_the_import_and_leaves_text_alone() {
    use std::io::Write;

    let response = b"0;10;32439;202CB962;355800020137588;Ira;1;01/01/1980";
    let mut zip = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
    zip.write_all(response).unwrap();
    let compressed = zip.finish().unwrap();

    assert_eq!(inflate(&compressed).as_deref(), Some(&response[..]));
    // Plain text is not deflate, and trying to inflate it must fail instead of returning garbage.
    assert_eq!(inflate(b"0;2;100002;"), None);
}

/// The Z-Wheel asks the language file for image 5035, does not find it, falls back to
/// `tectoyli.brf` and finds it there. The first attempt is not a missing file.
#[test]
fn a_resource_found_in_another_file_leaves_the_missing_list() {
    assert_eq!(
        resource_id("fs:/~0x01070798/tectoy_pt.brf (recurso 5035)"),
        Some(5035)
    );
    assert_eq!(resource_id("fontsize.map"), None);
    assert_eq!(resource_id("um (recurso não)"), None);
}

#[test]
fn a_timer_fires_once_and_the_callback_can_rearm_it() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let shell = machine.module.shell;
    let set = slot_of(Interface::Shell, "SetTimer");
    assert_eq!(
        call(
            &mut machine,
            Interface::Shell,
            set,
            [shell, 100, 0x1234, 0x5678]
        ),
        SUCCESS
    );
    assert_eq!(machine.armed_timers(), 1);

    // The whole 100 ms are left — that is what `GetTimerExpiration` must answer.
    let expiration = slot_of(Interface::Shell, "GetTimerExpiration");
    assert_eq!(
        call(
            &mut machine,
            Interface::Shell,
            expiration,
            [shell, 0x1234, 0x5678, 0]
        ),
        100
    );

    // With nothing else to do, the loop skips the idle time — one frame per pass, which is the
    // granularity at which a key could arrive. The timer leaves the list before firing: a BREW
    // timer is one-shot.
    for _ in 0..PASSES_UNTIL_TIMER_FIRES {
        machine.advance(1_000).unwrap();
        if machine.armed_timers() == 0 {
            break;
        }
    }
    assert!(machine.clock_ms() >= 100);
    assert_eq!(machine.armed_timers(), 0);

    // Rearming the same (pfn, pUser) pair replaces instead of stacking.
    call(
        &mut machine,
        Interface::Shell,
        set,
        [shell, 10, 0x1234, 0x5678],
    );
    call(
        &mut machine,
        Interface::Shell,
        set,
        [shell, 20, 0x1234, 0x5678],
    );
    assert_eq!(machine.armed_timers(), 1);

    let cancel = slot_of(Interface::Shell, "CancelTimer");
    call(
        &mut machine,
        Interface::Shell,
        cancel,
        [shell, 0x1234, 0x5678, 0],
    );
    assert_eq!(machine.armed_timers(), 0);
}

/// The `IHashCTX` MD5 keeps its state in the game's context, and the digest does not depend on
/// the data arriving in a single piece. That is what Powerboat Challenge was missing: without the
/// digest, the options file was considered corrupt and deleted on every launch.
#[test]
fn md5_with_the_games_context_gives_the_rfc_digest() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let ctx = 0x2000_1000;
    machine.write_md5_ctx(ctx, &crate::brew::crypto::Md5::new()).unwrap();
    for chunk in [&b"a"[..], &b"bc"[..]] {
        let mut md5 = machine.read_md5_ctx(ctx).unwrap();
        md5.update(chunk);
        machine.write_md5_ctx(ctx, &md5).unwrap();
    }
    let summary = machine.read_md5_ctx(ctx).unwrap().finish();
    let hex: String = summary.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "900150983cd24fb0d6963f7d28e17f72");
}

/// A negative deadline is an expired timer: it fires on the next pass, not 49 days from now. That
/// is what kept Powerboat Challenge stuck on the loading screen (`SetTimer` with -2 ms).
#[test]
fn a_timer_with_a_negative_deadline_fires_on_the_next_pass() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let shell = machine.module.shell;
    let set = slot_of(Interface::Shell, "SetTimer");
    let minus_two = (-2i32) as u32;
    call(&mut machine, Interface::Shell, set, [shell, minus_two, 0x1234, 0x5678]);
    let expiration = slot_of(Interface::Shell, "GetTimerExpiration");
    assert_eq!(
        call(&mut machine, Interface::Shell, expiration, [shell, 0x1234, 0x5678, 0]),
        0,
        "a negative deadline has already expired"
    );
    let before = machine.clock_ms();
    machine.advance(1_000).unwrap();
    assert_eq!(machine.armed_timers(), 0, "the expired timer did not fire");
    assert!(machine.clock_ms() - before < 1_000, "the clock should not jump");
}

/// How many loop passes to run before giving up waiting for a timer to expire.
///
/// The idle skip is one frame per pass, so a half-second timer takes about thirty; the number is
/// generous on purpose, so the test fails because the timer did not expire and not because the
/// count was too tight.
const PASSES_UNTIL_TIMER_FIRES: usize = 200;

/// The clock moves with the guest's work, and idle time is skipped one frame at a time up to the
/// next timer — never by a fixed step, which is what made virtual time run ahead of the game.
#[test]
fn the_clock_skips_idle_time_until_the_next_timer() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // With no armed timer there is nowhere to jump: the clock stays where it is.
    let before = machine.clock_ms();
    machine.advance(1_000).unwrap();
    assert_eq!(machine.clock_ms(), before);

    let shell = machine.module.shell;
    let set = slot_of(Interface::Shell, "SetTimer");
    call(&mut machine, Interface::Shell, set, [shell, 500, 0x1234, 0]);
    for _ in 0..PASSES_UNTIL_TIMER_FIRES {
        machine.advance(1_000).unwrap();
        if machine.armed_timers() == 0 {
            break;
        }
    }
    assert!(machine.clock_ms() >= before + 500);
    assert_eq!(machine.armed_timers(), 0);
}

/// A 2×1 PNG with a palette and `tRNS`: the first pixel opaque red, the second transparent.
/// It is the format Bejeweled Twist uses, and the one that broke the decoder before palette
/// expansion.
fn palette_png() -> Vec<u8> {
    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[4..]);
        out.extend_from_slice(&crc.to_be_bytes());
        out
    }
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &byte in data {
            crc ^= byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    // Uncompressed zlib: header, one final block and the row's adler32.
    let row = [0u8, 0, 1];
    let mut zlib = vec![0x78, 0x01, 0x01, 3, 0, 0xfc, 0xff];
    zlib.extend_from_slice(&row);
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &row {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    // 2×1, 8 bits per sample, type 3 (palette).
    let mut ihdr = 2u32.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
    png.extend(chunk(b"IHDR", &ihdr));
    png.extend(chunk(b"PLTE", &[255, 0, 0, 0, 255, 0]));
    png.extend(chunk(b"tRNS", &[255, 0]));
    png.extend(chunk(b"IDAT", &zlib));
    png.extend(chunk(b"IEND", &[]));
    png
}

#[test]
fn decodes_a_png_with_palette_and_transparency() {
    let image = decode_png(&palette_png()).expect("the test PNG should be accepted");
    assert_eq!((image.width, image.height), (2, 1));
    assert_eq!(image.pixels[0], Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
    assert_eq!(image.opaque, vec![true, false]);
}

#[test]
fn an_image_arrives_through_the_stream_and_draws_honoring_alpha() {
    let module_image = module_calling_malloc();
    let module = loader::load(&module_image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // The PNG lives in guest memory, as if it had come from `resources.dat`.
    let png = palette_png();
    let buffer = loader::HEAP_BASE + 0x1000;
    machine.cpu.write_mem(buffer, &png).unwrap();

    let stream = machine.new_object(Interface::MemAStream).unwrap();
    call(
        &mut machine,
        Interface::MemAStream,
        slot_of(Interface::MemAStream, "Set"),
        [stream, buffer, png.len() as u32, 0],
    );

    let image = machine.new_object(Interface::Image).unwrap();
    call(
        &mut machine,
        Interface::Image,
        slot_of(Interface::Image, "SetStream"),
        [image, stream, 0, 0],
    );

    // GetInfo returns the dimensions: cx and cy as uint16.
    let info = loader::HEAP_BASE;
    call(
        &mut machine,
        Interface::Image,
        slot_of(Interface::Image, "GetInfo"),
        [image, info, 0, 0],
    );
    let mut size = [0u8; 4];
    machine.cpu.read_mem(info, &mut size).unwrap();
    assert_eq!(u16::from_le_bytes([size[0], size[1]]), 2);
    assert_eq!(u16::from_le_bytes([size[2], size[3]]), 1);

    // Drawing leaves the opaque pixel and preserves what was under the transparent one.
    let screen = machine.device_bitmap().unwrap();
    machine
        .bitmaps
        .get_mut(&screen)
        .unwrap()
        .set_pixel_native(11, 5, 0x1234);
    call(
        &mut machine,
        Interface::Image,
        slot_of(Interface::Image, "Draw"),
        [image, 10, 5, 0],
    );
    let screen_fb = machine.bitmaps.get(&screen).unwrap();
    assert_eq!(
        screen_fb.get_pixel(10, 5),
        Rgb { r: 255, g: 0, b: 0 }.to_rgb565()
    );
    assert_eq!(
        screen_fb.get_pixel(11, 5),
        0x1234,
        "zero alpha does not draw"
    );
}

#[test]
fn qsort_sorts_by_calling_the_games_comparator() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // An integer comparator, like the one a game would write:
    //   ldr r2, [r0]
    //   ldr r3, [r1]
    //   sub r0, r2, r3
    //   bx  lr
    let compare = loader::MODULE_BASE + 0x900;
    for (i, word) in [0xe590_2000u32, 0xe591_3000, 0xe042_0003, 0xe12f_ff1e]
        .iter()
        .enumerate()
    {
        machine
            .cpu
            .write_mem(compare + i as u32 * 4, &word.to_le_bytes())
            .unwrap();
    }

    let base = loader::HEAP_BASE;
    let values: [u32; 6] = [40, 10, 60, 20, 50, 30];
    for (i, value) in values.iter().enumerate() {
        machine.cpu.write_u32(base + i as u32 * 4, *value).unwrap();
    }

    machine
        .qsort(base, values.len() as u32, 4, compare)
        .unwrap();

    let sorted: Vec<u32> = (0..values.len())
        .map(|i| machine.cpu.read_u32(base + i as u32 * 4).unwrap())
        .collect();
    assert_eq!(sorted, [10, 20, 30, 40, 50, 60]);
}

#[test]
fn qsort_leaves_the_registers_as_it_found_them() {
    // Sorting reenters the guest, and the caller must not notice the detour: when `qsort`
    // returns, the dispatch will still write `r0` and resume at `lr`.
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let compare = loader::MODULE_BASE + 0x900;
    for (i, word) in [0xe590_2000u32, 0xe591_3000, 0xe042_0003, 0xe12f_ff1e]
        .iter()
        .enumerate()
    {
        machine
            .cpu
            .write_mem(compare + i as u32 * 4, &word.to_le_bytes())
            .unwrap();
    }
    let base = loader::HEAP_BASE;
    for (i, value) in [3u32, 1, 2].iter().enumerate() {
        machine.cpu.write_u32(base + i as u32 * 4, *value).unwrap();
    }

    machine.cpu.write_reg(Reg::R4, 0xdead_beef);
    machine.cpu.write_reg(Reg::Lr, 0x0001_2345);
    machine.qsort(base, 3, 4, compare).unwrap();
    assert_eq!(machine.cpu.read_reg(Reg::R4), 0xdead_beef);
    assert_eq!(machine.cpu.read_reg(Reg::Lr), 0x0001_2345);
}

#[test]
fn arguments_past_the_fourth_go_on_the_stack() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // A minimal function that returns the fifth argument — the first one on the stack:
    //   ldr r0, [sp]
    //   bx  lr
    let code = loader::MODULE_BASE + 0x800;
    machine
        .cpu
        .write_mem(code, &0xe59d_0000u32.to_le_bytes())
        .unwrap();
    machine
        .cpu
        .write_mem(code + 4, &0xe12f_ff1eu32.to_le_bytes())
        .unwrap();

    machine
        .cpu
        .write_reg(Reg::Sp, loader::STACK_BASE + loader::STACK_SIZE as u32 - 16);
    let sp_before = machine.cpu.read_reg(Reg::Sp);
    let outcome = machine
        .call_guest_with_stack(code, [1, 2, 3, 4], &[0xabcd, 0x1234], 1_000)
        .unwrap();
    assert_eq!(outcome, Outcome::Returned { code: 0xabcd });
    assert_eq!(
        machine.cpu.read_reg(Reg::Sp),
        sp_before,
        "the stack goes back in place"
    );
}

#[test]
fn fill_rect_honors_the_raster_operation() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let screen = machine.device_bitmap().unwrap();
    let rect = loader::HEAP_BASE;
    for (i, value) in [0i16, 0, 8, 8].iter().enumerate() {
        machine
            .cpu
            .write_mem(rect + i as u32 * 2, &value.to_le_bytes())
            .unwrap();
    }
    machine
        .bitmaps
        .get_mut(&screen)
        .unwrap()
        .set_pixel_native(2, 2, 0x1234);

    // Filling with the transparent color and `AEE_RO_TRANSPARENT` writes nothing.
    let fill = slot_of(Interface::Bitmap, "FillRect");
    machine.transparency.insert(screen, 0);
    call(
        &mut machine,
        Interface::Bitmap,
        fill,
        [screen, rect, 0, AEE_RO_TRANSPARENT],
    );
    assert_eq!(
        machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
        0x1234,
        "the transparent fill must not erase the screen"
    );

    // With `AEE_RO_COPY`, it fills.
    call(
        &mut machine,
        Interface::Bitmap,
        fill,
        [screen, rect, 0x00ff, AEE_RO_COPY],
    );
    assert_eq!(
        machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
        0x00ff
    );

    // `AEE_RO_XOR` inverts.
    call(
        &mut machine,
        Interface::Bitmap,
        fill,
        [screen, rect, 0xff00, AEE_RO_XOR],
    );
    assert_eq!(
        machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
        0xffff
    );
}

#[test]
fn stack_scan_finds_the_return_addresses() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // A `bl` in the module, so the following address looks like a legitimate return.
    let call_site = loader::MODULE_BASE + 0x900;
    machine
        .cpu
        .write_mem(call_site, &0xeb00_0000u32.to_le_bytes())
        .unwrap();
    let ret = call_site + 4;

    let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 - 64;
    machine.cpu.write_reg(Reg::Sp, sp);
    machine.cpu.write_u32(sp, 0xdead_beef).unwrap();
    machine.cpu.write_u32(sp + 4, ret).unwrap();
    // A module address with no `bl` before it does not count as a return.
    machine
        .cpu
        .write_u32(sp + 8, loader::MODULE_BASE + 0x40)
        .unwrap();

    assert_eq!(machine.scan_stack(), vec![ret]);
}

#[test]
fn device_info_ex_returns_the_imei_and_its_length() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let buffer = loader::HEAP_BASE;
    let size = loader::HEAP_BASE + 0x100;

    // An item we do not know: `EUNSUPPORTED`, without touching anything.
    machine.cpu.write_u32(size, 16).unwrap();
    machine.cpu.write_reg(Reg::R1, 1);
    machine.cpu.write_reg(Reg::R2, buffer);
    machine.cpu.write_reg(Reg::R3, size);
    assert_eq!(machine.shell_get_device_info_ex().unwrap(), EUNSUPPORTED);

    // The IMEI fits in the 16 bytes Zeebo Sports Peteca offers, with the terminator.
    machine.cpu.write_reg(Reg::R1, DEVICEITEM_IMEI);
    assert_eq!(machine.shell_get_device_info_ex().unwrap(), SUCCESS);
    assert_eq!(machine.cpu.read_u32(size).unwrap(), IMEI.len() as u32);
    let mut got = [0u8; 16];
    machine.cpu.read_mem(buffer, &mut got).unwrap();
    assert_eq!(&got, IMEI);

    // Without `pnSize` there is no way to return the size, and the call is invalid.
    machine.cpu.write_reg(Reg::R3, 0);
    assert_eq!(machine.shell_get_device_info_ex().unwrap(), EBADPARM);
}

#[test]
fn device_info_fills_the_extended_fields_when_asked() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let info = loader::HEAP_BASE;
    machine.cpu.write_mem(info, &[0xffu8; 64]).unwrap();
    // Without `wStructSize`, the extended part is not touched.
    machine
        .cpu
        .write_mem(info + 44, &0u16.to_le_bytes())
        .unwrap();
    machine.cpu.write_reg(Reg::R1, info);
    machine.shell_get_device_info().unwrap();
    let mut path = [0u8; 2];
    machine.cpu.read_mem(info + 56, &mut path).unwrap();
    assert_eq!(
        u16::from_le_bytes(path),
        0xffff,
        "must not write past dwLang"
    );

    // With `wStructSize = 64` — which is what Bejeweled Twist sends —, the extended fields
    // come filled in.
    machine
        .cpu
        .write_mem(info + 44, &64u16.to_le_bytes())
        .unwrap();
    machine.cpu.write_reg(Reg::R1, info);
    machine.shell_get_device_info().unwrap();

    let mut size = [0u8; 2];
    machine.cpu.read_mem(info + 44, &mut size).unwrap();
    assert_eq!(u16::from_le_bytes(size) as u32, DEVICE_INFO_SIZE);
    machine.cpu.read_mem(info + 56, &mut path).unwrap();
    assert_eq!(u16::from_le_bytes(path) as u32, AEE_MAX_FILE_NAME);
    // And the basic fields are still right.
    let mut screen = [0u8; 4];
    machine.cpu.read_mem(info, &mut screen).unwrap();
    assert_eq!(u16::from_le_bytes([screen[0], screen[1]]), SCREEN_WIDTH);
    assert_eq!(u16::from_le_bytes([screen[2], screen[3]]), SCREEN_HEIGHT);
}

#[test]
fn the_module_license_does_not_expire() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let license = machine.new_object(Interface::License).unwrap();
    assert_eq!(
        call(
            &mut machine,
            Interface::License,
            slot_of(Interface::License, "IsExpired"),
            [license, 0, 0, 0],
        ),
        FALSE
    );

    let expire = loader::HEAP_BASE;
    assert_eq!(
        call(
            &mut machine,
            Interface::License,
            slot_of(Interface::License, "GetInfo"),
            [license, expire, 0, 0],
        ),
        LT_NONE
    );
    assert_eq!(machine.cpu.read_u32(expire).unwrap(), BV_UNLIMITED);

    // The documentation itself says to refuse when the type is not `LT_USES`.
    assert_eq!(
        call(
            &mut machine,
            Interface::License,
            slot_of(Interface::License, "SetUsesRemaining"),
            [license, 1, 0, 0],
        ),
        EFAILED
    );

    let kind = loader::HEAP_BASE + 16;
    let seq = loader::HEAP_BASE + 24;
    assert_eq!(
        call(
            &mut machine,
            Interface::License,
            slot_of(Interface::License, "GetPurchaseInfo"),
            [license, kind, expire, seq],
        ),
        PT_PURCHASE
    );
    let mut license_type = [0u8; 1];
    machine.cpu.read_mem(kind, &mut license_type).unwrap();
    assert_eq!(license_type[0] as u32, LT_NONE);
    assert_eq!(machine.cpu.read_u32(seq).unwrap(), 0);
}

#[test]
fn the_class_item_id_comes_from_the_module_directory() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    // The console's `.mod` lives in a directory named with the BREW store item number.
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, "roms/jogo/mod/277083");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.applet_class = 0x0108_c1e1;

    machine.cpu.write_reg(Reg::R1, 0x0108_c1e1);
    assert_eq!(machine.shell_get_class_item_id(), 277083);

    // A class from another module: the documentation says to return zero.
    machine.cpu.write_reg(Reg::R1, 0x0100_1001);
    assert_eq!(machine.shell_get_class_item_id(), 0);
}

#[test]
fn sound_keeps_its_settings_and_notifies_the_end_of_playback() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let sound = machine.new_object(Interface::Sound).unwrap();
    let info = loader::HEAP_BASE;
    // AEE_SOUND_DEVICE_SPEAKER, AEE_SOUND_METHOD_MIDI, local, no mute.
    machine.cpu.write_mem(info, &[9, 6, 0, 0, 0]).unwrap();
    let set = call(
        &mut machine,
        Interface::Sound,
        slot_of(Interface::Sound, "Set"),
        [sound, info, 0, 0],
    );
    assert_eq!(set, SUCCESS);

    let out = loader::HEAP_BASE + 16;
    call(
        &mut machine,
        Interface::Sound,
        slot_of(Interface::Sound, "Get"),
        [sound, out, 0, 0],
    );
    let mut read_back = [0u8; 5];
    machine.cpu.read_mem(out, &mut read_back).unwrap();
    assert_eq!(read_back, [9, 6, 0, 0, 0]);

    // Without `RegisterNotify` there is nobody to notify.
    call(
        &mut machine,
        Interface::Sound,
        slot_of(Interface::Sound, "PlayTone"),
        [sound, 0, 0, 0],
    );
    assert!(machine.pending_calls.is_empty());

    call(
        &mut machine,
        Interface::Sound,
        slot_of(Interface::Sound, "RegisterNotify"),
        [sound, 0x1234, 0x5678, 0],
    );
    call(
        &mut machine,
        Interface::Sound,
        slot_of(Interface::Sound, "PlayTone"),
        [sound, 0, 0, 0],
    );
    let notify = machine.pending_calls.first().copied().unwrap();
    assert_eq!(notify.function, 0x1234);
    // PFNSOUNDSTATUS(pUser, eCBType, eSPStatus, dwParam)
    assert_eq!(
        notify.args,
        [0x5678, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE, 0]
    );
}

#[test]
fn the_end_of_the_sound_notifies_the_game() {
    // A game that plays one thing at a time waits for the end notice to play the next one.
    // Without it the sound stops after the first effect, and that is what was happening.
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // A half-second RIFF: 8-bit PCM, mono, 8000 Hz.
    let wave = riff_pcm8(8000, 4000);
    let buffer = machine.heap.alloc(wave.len() as u32).unwrap();
    machine.cpu.write_mem(buffer, &wave).unwrap();
    let data = machine.heap.alloc(12).unwrap();
    for (index, value) in [MMD_BUFFER, buffer, wave.len() as u32]
        .into_iter()
        .enumerate()
    {
        machine
            .cpu
            .write_u32(data + index as u32 * 4, value)
            .unwrap();
    }

    let out = machine.heap.alloc(4).unwrap();
    let util = machine.new_object(Interface::MediaUtil).unwrap();
    call(
        &mut machine,
        Interface::MediaUtil,
        slot_of(Interface::MediaUtil, "CreateMedia"),
        [util, data, out, 0],
    );
    let media = machine.cpu.read_u32(out).unwrap();

    call(
        &mut machine,
        Interface::Media,
        slot_of(Interface::Media, "RegisterNotify"),
        [media, 0x4321, 0x8765, 0],
    );
    call(
        &mut machine,
        Interface::Media,
        slot_of(Interface::Media, "Play"),
        [media, 0, 0, 0],
    );

    // Playing notifies that it started — but on the loop's next pass, not on the way out of
    // `Play`: the game still needs to mark the sound as requested before the notice arrives.
    assert!(machine.pending_calls.is_empty(), "the notice does not go out on the call's exit");
    let warnings = |machine: &Machine<UnicornCpu>| -> Vec<(u32, u32, u32)> {
        machine.media_notify_queue.iter().map(|&(o, c, st, _)| (o, c, st)).collect()
    };
    assert_eq!(warnings(&machine), vec![(media, MM_CMD_PLAY, MM_STATUS_START)]);

    // The buffer is read on the loop's next pass, and that is when the sound actually starts.
    machine.resolve_pending_media().unwrap();
    machine.media_notify_queue.clear();

    // Before it is time, nothing: the sound is still playing.
    machine.poll_media().unwrap();
    assert!(machine.media_notify_queue.is_empty());

    // Once the duration has passed, the end notice goes out — and it goes out from the virtual
    // clock, without depending on the sound really coming out of the sound card.
    machine.clock_us += 600_000;
    machine.poll_media().unwrap();
    assert_eq!(warnings(&machine), vec![(media, MM_CMD_PLAY, MM_STATUS_DONE)]);
    // And it does not repeat: the sound has already ended once.
    machine.media_notify_queue.clear();
    machine.poll_media().unwrap();
    assert!(machine.media_notify_queue.is_empty());
}

#[test]
fn each_play_rereads_the_sound_buffer() {
    // Super League writes another effect into the same buffer and tells it to play again,
    // without a new `SetMediaParm`. The sound played must be what is in the buffer now.
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let short = riff_pcm8(8000, 800);
    let long_sound = riff_pcm8(8000, 4000);
    let buffer = machine.heap.alloc(long_sound.len() as u32).unwrap();
    machine.cpu.write_mem(buffer, &short).unwrap();
    let data = machine.heap.alloc(12).unwrap();
    for (index, value) in [MMD_BUFFER, buffer, long_sound.len() as u32].into_iter().enumerate() {
        machine.cpu.write_u32(data + index as u32 * 4, value).unwrap();
    }
    let out = machine.heap.alloc(4).unwrap();
    let util = machine.new_object(Interface::MediaUtil).unwrap();
    call(
        &mut machine,
        Interface::MediaUtil,
        slot_of(Interface::MediaUtil, "CreateMedia"),
        [util, data, out, 0],
    );
    let media = machine.cpu.read_u32(out).unwrap();
    let play = slot_of(Interface::Media, "Play");
    let duration = |machine: &mut Machine<UnicornCpu>| {
        machine.media_sound(media).unwrap().map(|sound| sound.frames())
    };

    call(&mut machine, Interface::Media, play, [media, 0, 0, 0]);
    machine.resolve_pending_media().unwrap();
    assert_eq!(duration(&mut machine), Some(800));

    machine.cpu.write_mem(buffer, &long_sound).unwrap();
    call(&mut machine, Interface::Media, play, [media, 0, 0, 0]);
    machine.resolve_pending_media().unwrap();
    assert_eq!(duration(&mut machine), Some(4000), "the second Play plays the new sound");
}

#[test]
fn the_notice_arrives_after_release_with_its_own_status() {
    // `START` and `DONE` of the same sound on the same pass, with the object already released:
    // both calls go out, and each one reads its own status in the shared block.
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    // The handler records `nStatus` (offset 16 of the block, in r1) in a list at `LIST`:
    // ldr r2,[r1,#16] / ldr r3,=LISTA / ldr r0,[r3] / add r0,r0,#1 / str r0,[r3] /
    // str r2,[r3,r0,lsl #2] / bx lr
    const LIST: u32 = 0x0010_0000;
    let handler = machine.heap.alloc(64).unwrap();
    let code: [u32; 8] = [
        0xE5912010, 0xE59F3010, 0xE5930000, 0xE2800001, 0xE5830000, 0xE7832100, 0xE12FFF1E,
        LIST,
    ];
    for (i, word) in code.iter().enumerate() {
        machine.cpu.write_u32(handler + 4 * i as u32, *word).unwrap();
    }
    machine.cpu.write_u32(LIST, 0).unwrap();

    let util = machine.new_object(Interface::MediaUtil).unwrap();
    let out = machine.heap.alloc(4).unwrap();
    call(
        &mut machine,
        Interface::MediaUtil,
        slot_of(Interface::MediaUtil, "CreateMedia"),
        [util, 0, out, 0],
    );
    let media = machine.cpu.read_u32(out).unwrap();
    call(
        &mut machine,
        Interface::Media,
        slot_of(Interface::Media, "RegisterNotify"),
        [media, handler, 0, 0],
    );
    machine.notify_media(media, MM_CMD_PLAY, MM_STATUS_START).unwrap();
    machine.notify_media(media, MM_CMD_PLAY, MM_STATUS_DONE).unwrap();
    // The game releases the object before the loop's next pass: the notice must still arrive.
    call(
        &mut machine,
        Interface::Media,
        slot_of(Interface::Media, "Release"),
        [media, 0, 0, 0],
    );
    machine.deliver_media_notifies(1_000_000).unwrap();

    assert_eq!(machine.cpu.read_u32(LIST).unwrap(), 2, "both calls");
    assert_eq!(machine.cpu.read_u32(LIST + 4).unwrap(), MM_STATUS_START);
    assert_eq!(machine.cpu.read_u32(LIST + 8).unwrap(), MM_STATUS_DONE);
}

#[test]
fn reading_the_clock_in_a_loop_makes_time_pass() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let clock = slot_of(Interface::Helpers, "aee_GetUpTimeMS");
    let yield_ = slot_of(Interface::Thread, "Suspend");

    // Up to the threshold, reading the clock is normal frame work and costs no time.
    for _ in 0..SPIN_THRESHOLD {
        call(&mut machine, Interface::Helpers, clock, [0; 4]);
    }
    assert_eq!(machine.clock_us, 0);

    // Past the threshold, every read advances the clock — and yielding, which is how the loop
    // goes around, does not interrupt the count.
    for _ in 0..10 {
        call(&mut machine, Interface::Thread, yield_, [0; 4]);
        call(&mut machine, Interface::Helpers, clock, [0; 4]);
    }
    assert_eq!(machine.clock_us, 10 * SPIN_STEP_US);
}

#[test]
fn work_in_the_middle_of_the_loop_breaks_the_busy_wait() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let clock = slot_of(Interface::Helpers, "aee_GetUpTimeMS");
    let work = slot_of(Interface::Helpers, "aee_GetRand");

    // A frame that reads the clock in the middle of what it does still costs what it costs: the
    // count resets on every call that is neither a read nor a yield.
    for _ in 0..(SPIN_THRESHOLD * 4) {
        call(&mut machine, Interface::Helpers, clock, [0; 4]);
        call(&mut machine, Interface::Helpers, work, [0, 0, 0, 0]);
    }
    assert_eq!(machine.clock_us, 0);
}

#[test]
fn dib_query_interface_exposes_the_pixels_to_the_game() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let display = machine.new_object(Interface::Display).unwrap();
    let out_screen = loader::HEAP_BASE + 64;
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "GetDeviceBitmap"),
        [display, out_screen, 0, 0],
    );
    let screen = machine.cpu.read_u32(out_screen).unwrap();

    let out = loader::HEAP_BASE;
    call(
        &mut machine,
        Interface::Bitmap,
        slot_of(Interface::Bitmap, "QueryInterface"),
        [screen, AEECLSID_DIB, out, 0],
    );

    // An `IDIB` is the `IBitmap` itself with the public fields filled in.
    assert_eq!(machine.cpu.read_u32(out).unwrap(), screen);
    let buffer = machine.cpu.read_u32(screen + 8).unwrap();
    assert!(buffer >= loader::SURFACE_BASE);
    let mut cx = [0u8; 2];
    machine.cpu.read_mem(screen + 20, &mut cx).unwrap();
    assert_eq!(u16::from_le_bytes(cx), SCREEN_WIDTH);
    let mut depth = [0u8; 2];
    machine.cpu.read_mem(screen + 28, &mut depth).unwrap();
    assert_eq!(depth, [COLOR_DEPTH as u8, IDIB_COLORSCHEME_565]);

    // Whatever the game writes to the buffer must reach the host framebuffer.
    machine
        .cpu
        .write_mem(buffer, &0xf800u16.to_le_bytes())
        .unwrap();
    // **The write here comes from the host, and the product only syncs when the guest wrote.**
    // Whatever writes through the guest goes through the JIT hooks and sets the flag on its own;
    // from the outside there is no hook at all, and the way to signal is the same one the
    // emulator's helpers use (`memset`, `memmove`): `mark_dirty`. Without this the test passed on
    // unicorn — whose `mem_write` fires the write hook — and failed on dynarmic, which only sees
    // what the recompiled code executes.
    machine.cpu.mark_dirty(buffer, 2);
    machine.sync_from_guest(screen).unwrap();
    assert_eq!(
        machine.bitmaps.get(&screen).unwrap().get_pixel(0, 0),
        0xf800
    );
}

/// An empty machine, to exercise an API without loading any game.
fn bare_machine() -> Machine<UnicornCpu> {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    // Without a stack there is no fifth argument: `GetConnectedDevices` reads `nLenReq` from there.
    machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
    machine
}

/// With both ports set to controller, the enumeration returns both — and the output array gets
/// both identifiers, in port order.
#[test]
fn both_ports_show_up_in_the_enumeration() {
    use crate::input::bindings::Device;
    let mut machine = bare_machine();
    machine.set_ports([Some(Device::Controller), Some(Device::Controller)]);
    let hid = machine.new_object(Interface::Hid).unwrap();
    let output = machine.heap.alloc(16).unwrap();
    let count = machine.heap.alloc(4).unwrap();
    machine
        .cpu
        .write_u32(machine.cpu.read_reg(Reg::Sp), count)
        .unwrap();

    let r = call(
        &mut machine,
        Interface::Hid,
        slot_of(Interface::Hid, "GetConnectedDevices"),
        [hid, UID_JOYSTICK_DEVICE, output, 2],
    );
    assert_eq!(r, SUCCESS);
    assert_eq!(machine.cpu.read_u32(count).unwrap(), 2);
    assert_eq!(machine.cpu.read_u32(output).unwrap(), 1);
    assert_eq!(machine.cpu.read_u32(output + 4).unwrap(), 2);
}

/// A port set to keyboard leaves the joystick list and joins the keyboard list. It is what makes
/// the Z-Wheel replace `No keyboard reported` with a found keyboard.
#[test]
fn the_keyboard_port_is_not_a_joystick() {
    use crate::input::bindings::Device;
    let mut machine = bare_machine();
    machine.set_ports([Some(Device::Controller), Some(Device::Keyboard)]);
    let hid = machine.new_object(Interface::Hid).unwrap();
    let output = machine.heap.alloc(16).unwrap();
    let count = machine.heap.alloc(4).unwrap();
    machine
        .cpu
        .write_u32(machine.cpu.read_reg(Reg::Sp), count)
        .unwrap();

    for (kind, expected, first) in [
        (UID_JOYSTICK_DEVICE, 1u32, 1u32),
        (UID_KEYBOARD_DEVICE, 1, 2),
    ] {
        call(
            &mut machine,
            Interface::Hid,
            slot_of(Interface::Hid, "GetConnectedDevices"),
            [hid, kind, output, 2],
        );
        assert_eq!(machine.cpu.read_u32(count).unwrap(), expected);
        assert_eq!(machine.cpu.read_u32(output).unwrap(), first);
    }
}

/// Each `IHIDDevice` reads the port it came from. Without this both devices would read the same
/// controller, which is the defect player two would notice first.
#[test]
fn each_device_reads_its_own_port() {
    use crate::input::bindings::Device;
    let mut machine = bare_machine();
    machine.set_ports([Some(Device::Controller), Some(Device::Controller)]);
    let hid = machine.new_object(Interface::Hid).unwrap();
    let output = machine.heap.alloc(4).unwrap();

    let mut create_device = |handle: u32| {
        call(
            &mut machine,
            Interface::Hid,
            slot_of(Interface::Hid, "CreateDevice"),
            [hid, handle, output, 0],
        );
        machine.cpu.read_u32(output).unwrap()
    };
    let (one, two) = (create_device(1), create_device(2));
    assert_ne!(one, two);

    let mut pressed = Pad::default();
    pressed.press(input::DPAD[0], true);
    machine.set_port_pad(1, pressed);

    let info = machine.heap.alloc(20).unwrap();
    let mut state = |device: u32| {
        call(
            &mut machine,
            Interface::HidDevice,
            slot_of(Interface::HidDevice, "GetButtonInfo"),
            [device, input::DPAD[0] as u32, info, 0],
        );
        machine.cpu.read_u32(info + 4).unwrap()
    };
    assert_eq!(state(one), 0, "port 1 is idle");
    assert_eq!(state(two), 1, "port 2 has the d-pad up");
}

/// **The `glPixelStorei` row alignment.**
///
/// GL aligns each texture row to a multiple of the requested alignment — 4 by default —, and what
/// is left over is padding. A three-column RGB565 texture has six bytes per row, which become
/// eight; read as if it were tightly packed, the second row starts two bytes early and the image
/// comes out scrambled diagonally. It was the only GL call the scan of the 62 ROMs flagged as
/// "served without doing anything", used by Double Dragon, Galaxy on Fire, Pac-Mania, Powerboat
/// Challenge and Raging Thunder 2.
#[test]
fn pixel_storei_alignment_changes_the_row_stride() {
    use crate::machine::round_up_to;
    assert_eq!(round_up_to(6, 4), 8, "tręs colunas RGB565 ocupam oito bytes");
    assert_eq!(round_up_to(4, 4), 4, "what is already aligned does not grow");
    assert_eq!(round_up_to(6, 1), 6, "alignment 1 is a tightly packed row");
    assert_eq!(round_up_to(6, 2), 6);
    assert_eq!(round_up_to(10, 8), 16);
    assert_eq!(round_up_to(0, 4), 0, "an empty texture does not become a row of padding");
}

#[test]
fn serves_malloc_and_resumes_execution() {
    let image = module_calling_malloc();
    let module = loader::load(&image).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");

    // r2 must point to the helpers table and r3 to the return sentinel; the `AEEMod_Load`
    // convention already puts the helpers in r1, so we adjust after the reset.
    machine.cpu.reset(&machine.module.mem).unwrap();
    let outcome = {
        machine.cpu.write_reg(Reg::R2, loader::HELPERS_BASE);
        machine.cpu.write_reg(Reg::R3, RETURN_MAGIC);
        machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
        let mut pc = machine.module.entry;
        loop {
            match machine.cpu.run(pc, 100).unwrap() {
                StopReason::ApiCall { addr } => {
                    let value = machine
                        .dispatch(addr)
                        .unwrap()
                        .expect("MALLOC implementado");
                    machine.cpu.write_reg(Reg::R0, value);
                    pc = machine.cpu.read_reg(Reg::Lr);
                }
                other => break other,
            }
        }
    };

    assert_eq!(outcome, StopReason::Returned);
    let ptr = machine.cpu.read_reg(Reg::R0);
    assert_eq!(
        ptr,
        loader::HEAP_BASE,
        "the first block sits at the start of the heap"
    );
    assert_eq!(
        machine.cpu.read_u32(ptr).unwrap(),
        0,
        "MALLOC hands out zeroed memory"
    );
}

/// The block read must return exactly what the component-by-component read returned — it exists
/// only to trade 57 ns per crossing for 0.3 ns, not to change values.
///
/// It holds for the types the games really use: `GL_FIXED` in geometry, `GL_FLOAT` where the SDK
/// left it, `GL_SHORT` in the compact models and `GL_UNSIGNED_BYTE` in color, which is the only
/// normalized one. And with a `stride` larger than the element, which is how an interleaved array
/// arrives.
#[test]
fn bulk_array_read_matches_component_by_component_read() {
    let mut machine = bare_machine();
    let base = loader::STACK_BASE + 0x2000;
    let bytes: Vec<u8> = (0..512u32).map(|i| (i * 7 + 3) as u8).collect();
    machine.cpu.write_mem(base, &bytes).unwrap();

    let indices: Vec<u32> = vec![0, 3, 1, 7, 2, 7, 0];
    for kind in [
        gles::GL_FIXED,
        gles::GL_FLOAT,
        gles::GL_SHORT,
        gles::GL_UNSIGNED_SHORT,
        gles::GL_BYTE,
        gles::GL_UNSIGNED_BYTE,
    ] {
        for size in 2..=4u32 {
            for stride in [0, size * component_size(kind) + 4] {
                let pointer = ArrayPointer {
                    size,
                    kind,
                    stride,
                    address: base,
                    enabled: true,
                    buffer: 0,
                };
                let fallback = [1.0, 2.0, 3.0, 4.0];
                let block = machine.read_array(pointer, &indices, fallback).unwrap();
                let loose: Vec<[f32; 4]> = indices
                    .iter()
                    .map(|&i| machine.read_attribute(pointer, i, fallback).unwrap())
                    .collect();
                assert_eq!(block, loose, "kind={kind:#x} size={size} stride={stride}");
            }
        }
    }
}

/// A corrupt array pointer gives an absurd extent. Instead of allocating that, the block read
/// gives up and falls back to the component-by-component path — which fails on its own if the
/// address is not mapped, as it always did.
#[test]
fn an_array_with_absurd_length_does_not_allocate_the_world() {
    let machine = bare_machine();
    let pointer = ArrayPointer {
        size: 3,
        kind: gles::GL_FIXED,
        stride: 0x4000_0000,
        address: loader::STACK_BASE,
        enabled: true,
        buffer: 0,
    };
    assert!(machine.read_array(pointer, &[0, 1], [0.0; 4]).is_err());
}

/// With a buffer object bound, the array's "pointer" is an **offset inside the buffer**, and the
/// content comes from our copy — not from the game's memory.
///
/// The test puts different values in the two places on purpose: if the read fell into the game's
/// memory, it would pass by chance on an empty buffer and fail here.
#[test]
fn a_vector_with_a_bound_buffer_reads_the_buffer_not_game_memory() {
    let mut machine = bare_machine();
    let base = loader::STACK_BASE + 0x2000;
    // In the game's memory, a triangle at 1.0; in the buffer, the same at 2.0.
    let one: Vec<u8> = (0..9).flat_map(|_| 0x0001_0000u32.to_le_bytes()).collect();
    machine.cpu.write_mem(base, &one).unwrap();
    let two: Vec<u8> = (0..9).flat_map(|_| 0x0002_0000u32.to_le_bytes()).collect();
    machine.gl_buffers.insert(7, two);

    let pointer = ArrayPointer {
        size: 3,
        kind: gles::GL_FIXED,
        stride: 0,
        // The offset is zero, which is the most common case — and the one that, without a
        // buffer, would be the null address.
        address: 0,
        enabled: true,
        buffer: 7,
    };
    let loaded = machine.read_array(pointer, &[0, 1, 2], [0.0; 4]).unwrap();
    assert_eq!(loaded, vec![[2.0, 2.0, 2.0, 0.0]; 3], "came from the buffer");

    // And the one-off path — the fallback, when the block does not work — must agree.
    let loose = machine.read_attribute(pointer, 1, [0.0; 4]).unwrap();
    assert_eq!(loose, [2.0, 2.0, 2.0, 0.0]);

    // Without a buffer, the same pointer with `address: base` reads the game's memory.
    let in_memory = ArrayPointer {
        address: base,
        buffer: 0,
        ..pointer
    };
    let loaded = machine.read_array(in_memory, &[0, 1, 2], [0.0; 4]).unwrap();
    assert_eq!(loaded, vec![[1.0, 1.0, 1.0, 0.0]; 3], "came from the game's memory");
}

/// The AECHAR read now comes in blocks, and a block can cross the end of the mapped region: a
/// string that ends flush against the edge of the stack is precisely the case where the whole
/// read fails even though there are valid units before it. That is the fallback path, and it
/// must return the string, not empty.
#[test]
fn aechar_flush_against_the_region_end_is_still_read() {
    let mut machine = bare_machine();
    let end = loader::STACK_BASE + loader::STACK_SIZE as u32;

    // "Zeebo" with the terminator exactly in the last pair of bytes of the stack.
    let text: Vec<u16> = "Zeebo\0".encode_utf16().collect();
    let bytes: Vec<u8> = text.iter().flat_map(|u| u.to_le_bytes()).collect();
    let addr = end - bytes.len() as u32;
    machine.cpu.write_mem(addr, &bytes).unwrap();

    assert_eq!(machine.read_aechar(addr).unwrap(), "Zeebo");
    assert_eq!(
        machine.read_aechar_units(addr).unwrap(),
        "Zeebo".encode_utf16().collect::<Vec<u16>>()
    );

    // And with no terminator at all: it stops at the edge instead of running off the map.
    let unterminated: Vec<u8> = std::iter::repeat_n(b'A', 8).flat_map(|c| [c, 0]).collect();
    let addr = end - unterminated.len() as u32;
    machine.cpu.write_mem(addr, &unterminated).unwrap();
    assert_eq!(machine.read_aechar(addr).unwrap(), "AAAAAAAA");
}

/// The zero address and the empty string still count as empty, not as an error.
#[test]
fn empty_aechar_and_null_pointer_stay_empty() {
    let mut machine = bare_machine();
    let addr = loader::STACK_BASE + 0x3000;
    machine.cpu.write_mem(addr, &[0, 0]).unwrap();
    assert_eq!(machine.read_aechar(0).unwrap(), "");
    assert_eq!(machine.read_aechar(addr).unwrap(), "");
    assert!(machine.read_aechar_units(addr).unwrap().is_empty());
}

#[test]
fn strtoul_accepts_a_sign_like_c_does() {
    // Alice in Wonderland reads `-2 12 16 ...`: without the sign, nothing was consumed.
    assert_eq!(parse_unsigned("-2 12", 10), (2u32.wrapping_neg(), 2));
    assert_eq!(parse_unsigned(" +7", 10), (7, 3));
    assert_eq!(parse_unsigned("\r\r\n0 0", 10), (0, 4));
    assert_eq!(parse_unsigned("0x1f", 0), (0x1f, 4));
    assert_eq!(parse_unsigned("0 1", 0), (0, 1));
    assert_eq!(parse_unsigned("-", 10), (0, 0));
    assert_eq!(parse_unsigned("abc", 10), (0, 0));
}

#[test]
fn raw_pcm_becomes_floating_point_samples() {
    // Signed 16-bit, which is what Data East's arcade ports deliver.
    let bytes = [0x00, 0x00, 0x00, 0x40, 0x00, 0xc0];
    let samples = super::media::pcm_to_f32(&bytes, 16, false);
    assert_eq!(samples, vec![0.0, 0.5, -0.5]);
    // Unsigned 8-bit has silence at 128.
    assert_eq!(super::media::pcm_to_f32(&[128, 255, 0], 8, true), vec![0.0, 0.9921875, -1.0]);
}

/// A module that counts to a hundred in a loop and returns, without calling any API.
fn module_with_long_loop() -> ModImage {
    let code = [
        0xe3a0_0000u32.to_le_bytes(), // mov r0, #0
        0xe280_0001u32.to_le_bytes(), // add r0, r0, #1
        0xe350_0064u32.to_le_bytes(), // cmp r0, #100
        0x1aff_fffcu32.to_le_bytes(), // bne -3
        0xe12f_ff1eu32.to_le_bytes(), // bx lr
    ]
    .concat();
    ModImage::parse(code).unwrap()
}

#[test]
fn a_slice_cut_by_the_budget_resumes_on_the_next_pass() {
    // Zeebo Extreme Rolima runs its whole loop inside `EVT_APP_START`, without yielding.
    // Without resuming, the instruction ceiling discarded the slice and the session ended on its
    // own.
    let module = loader::load(&module_with_long_loop()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    // A short budget: the hundred-iteration loop does not fit.
    let truncated = machine.run(4).unwrap();
    assert_eq!(truncated, Outcome::Budget, "the ceiling should have cut the loop");

    // The next pass continues from where it stopped, and now the loop finishes.
    let mut outcome = None;
    for _ in 0..64 {
        let outcomes = machine.advance(1_000).unwrap();
        if let Some(end) = outcomes
            .iter()
            .find(|o| matches!(o, Outcome::Returned { .. }))
        {
            outcome = Some(end.clone());
            break;
        }
    }
    assert_eq!(
        outcome,
        Some(Outcome::Returned { code: 100 }),
        "the loop should have reached the end, with the counter at a hundred"
    );
}


/// **A DIB the game describes itself is drawn from the game's own memory.**
///
/// Devil May Cry: Dante x Virgil loads its sprites with its own BMP reader (`0x40000` in
/// `dmc_mas.mod`): it calls `IDISPLAY_CreateDIBitmap(p, &dib, 4, 0, 0)` — the depth from
/// `biBitCount`, zero width and height — and then writes the `IDIB` fields by hand: `pBmp` points
/// at the last row of the BMP's pixel data, `nPitch` is the negative row size, `pRGB` is the BMP's
/// own palette (`RGBQUAD`s, so `0x00RRGGBB` read as a word) and `cx`/`cy` come from the header.
/// Nothing of that goes through us: the blit has to read the struct at the time it draws.
#[test]
fn a_dib_described_by_the_game_is_blitted_from_its_own_memory() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let display = machine.new_object(Interface::Display).unwrap();

    let out = loader::HEAP_BASE;
    machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
    machine.cpu.write_u32(loader::STACK_BASE + 0x1000, 0).unwrap(); // h
    let result = call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "CreateDIBitmap"),
        [display, out, 4, 0],
    );
    assert_eq!(result, SUCCESS);
    let dib = machine.cpu.read_u32(out).unwrap();
    assert_ne!(dib, 0);
    assert_eq!(machine.objects.kind_of(dib), Some(Interface::Bitmap));
    let mut depth = [0u8; 1];
    machine.cpu.read_mem(dib + 28, &mut depth).unwrap();
    assert_eq!(depth[0], 4, "nDepth is the depth the game asked for");

    // A 2x2 4-bit BMP, bottom-up, rows padded to four bytes. On screen the top row is the
    // indices 1, 2 and the bottom one 3, 0.
    let pixels = loader::HEAP_BASE + 0x100;
    machine
        .cpu
        .write_mem(pixels, &[0x30, 0, 0, 0, 0x12, 0, 0, 0])
        .unwrap();
    let palette = loader::HEAP_BASE + 0x200;
    for (index, rgb) in [0x0000_0000u32, 0x00ff_0000, 0x0000_ff00, 0x0000_00ff]
        .into_iter()
        .enumerate()
    {
        machine.cpu.write_u32(palette + index as u32 * 4, rgb).unwrap();
    }
    machine.cpu.write_u32(dib + 8, pixels + 4).unwrap(); // pBmp: the top row
    machine.cpu.write_u32(dib + 12, palette).unwrap(); // pRGB
    machine.cpu.write_mem(dib + 20, &2u16.to_le_bytes()).unwrap(); // cx
    machine.cpu.write_mem(dib + 22, &2u16.to_le_bytes()).unwrap(); // cy
    machine.cpu.write_mem(dib + 24, &(-4i16).to_le_bytes()).unwrap(); // nPitch

    // IDISPLAY_BitBlt(p, 10, 20, 2, 2, dib, 0, 0, AEE_RO_COPY)
    let sp = loader::STACK_BASE + 0x1000;
    for (index, word) in [2, dib, 0, 0, AEE_RO_COPY].into_iter().enumerate() {
        machine.cpu.write_u32(sp + index as u32 * 4, word).unwrap();
    }
    call(
        &mut machine,
        Interface::Display,
        slot_of(Interface::Display, "BitBlt"),
        [display, 10, 20, 2],
    );

    let screen = machine.screen();
    let (red, green, blue) = (0xf800, 0x07e0, 0x001f);
    assert_eq!(
        [
            screen.get_pixel(10, 20),
            screen.get_pixel(11, 20),
            screen.get_pixel(10, 21),
            screen.get_pixel(11, 21),
        ],
        [red, green, blue, 0],
    );
}

/// **In a phone game the controller presses phone keys, and the console's own keys stay out.**
///
/// Every frontend already turns button 1 into `AVK_CONFIRM` and button 2 into `AVK_CLR`, which
/// Devil May Cry does not know. The machine is the one place all of them go through, so the phone
/// mapping happens here: the button becomes the game's key, and the frontend's key is dropped
/// instead of arriving twice.
#[test]
fn a_phone_game_gets_phone_keys_from_the_controller() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    machine.applet_class = 0x0107_0e36;

    let mut pad = Pad::default();
    pad.press(Pad::button_by_name("b2").unwrap(), true);
    machine.set_pad(pad);
    machine.set_key(input::avk::CLR, true); // what the frontend sends for the same button
    machine.set_key(input::avk::digit(5).unwrap(), true); // a digit typed on the keyboard

    assert_eq!(
        machine.key_events.iter().copied().collect::<Vec<_>>(),
        [(input::avk::digit(8).unwrap(), true), (input::avk::digit(5).unwrap(), true)],
    );
}

/// **`AEECLSID_SOUNDPLAYER` exists, and `GetVolume` answers the way the SDK says.**
///
/// Devil May Cry: Dante x Virgil creates it next to its `ISound` (`0x350ac` in `dmc_mas.mod`),
/// registers its callback and asks for the volume. Only when the class exists does it write the
/// 100 that its sound manager copies into its "ready" field; refused, the field stayed 0 and the
/// game played no sound at all, with every effect and song already decoded in memory.
///
/// `inc/AEESoundPlayer.h`: `GetVolume` replies through `PFNSOUNDPLAYERSTATUS` with
/// `AEE_SOUNDPLAYER_VOLUME_CB` (4), `AEE_SOUNDPLAYER_SUCCESS` (1) and `dwParam` pointing at an
/// `AEESoundPlayerCmdData` whose `wVolume` holds the volume. The game reads it with a `ldrh` from
/// `dwParam`.
#[test]
fn the_sound_player_reports_its_volume_through_the_callback() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();

    let out = loader::HEAP_BASE;
    let created = call(
        &mut machine,
        Interface::Shell,
        slot_of(Interface::Shell, "CreateInstance"),
        [loader::OBJECT_BASE, 0x0100_2000, out, 0],
    );
    assert_eq!(created, SUCCESS);
    let player = machine.cpu.read_u32(out).unwrap();
    assert_eq!(machine.objects.kind_of(player), Some(Interface::SoundPlayer));

    let slot = |name| slot_of(Interface::SoundPlayer, name);
    call(&mut machine, Interface::SoundPlayer, slot("RegisterNotify"), [player, 0x1234, 0x5678, 0]);
    call(&mut machine, Interface::SoundPlayer, slot("SetVolume"), [player, 60, 0, 0]);
    call(&mut machine, Interface::SoundPlayer, slot("GetVolume"), [player, 0, 0, 0]);

    let notify = machine.pending_calls.last().copied().unwrap();
    assert_eq!(notify.function, 0x1234);
    let [context, kind, status, data] = notify.args;
    assert_eq!((context, kind, status), (0x5678, 4, 1));
    let mut volume = [0u8; 2];
    machine.cpu.read_mem(data, &mut volume).unwrap();
    assert_eq!(u16::from_le_bytes(volume), 60);
}

/// **`ISoundPlayer::SetInfo` takes a source the way the SDK says, and `GetInfo` gives it back.**
///
/// Ultimate Chess 3D hands a buffer to the player right at `EVT_APP_START` (`0x9cd6c` in
/// `chessbots.mod`) and only goes on — to `GetVolume` — when `SetInfo` succeeds. While the class was
/// refused it took another path; once it existed, the missing `SetInfo` stopped the game.
///
/// `inc/AEESoundPlayer.h`: `AEESoundPlayerInfo` is `{eInput, pData, dwSize}`; a null `pInfo`
/// resets the source, a zero `dwSize` is `EBADPARM`, an input type BREW does not know is
/// `EUNSUPPORTED`.
#[test]
fn the_sound_player_takes_a_source_and_gives_it_back() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let player = machine.new_object(Interface::SoundPlayer).unwrap();
    let slot = |name| slot_of(Interface::SoundPlayer, name);
    let info = loader::HEAP_BASE;
    let write_info = |machine: &mut Machine<UnicornCpu>, input: u8, data: u32, size: u32| {
        machine.cpu.write_mem(info, &[input, 0, 0, 0]).unwrap();
        machine.cpu.write_u32(info + 4, data).unwrap();
        machine.cpu.write_u32(info + 8, size).unwrap();
    };

    write_info(&mut machine, 2, 0x1000_1000, 640); // SDT_BUFFER
    assert_eq!(call(&mut machine, Interface::SoundPlayer, slot("SetInfo"), [player, info, 0, 0]), SUCCESS);
    let back = loader::HEAP_BASE + 0x40;
    assert_eq!(call(&mut machine, Interface::SoundPlayer, slot("GetInfo"), [player, back, 0, 0]), SUCCESS);
    let mut bytes = [0u8; 12];
    machine.cpu.read_mem(back, &mut bytes).unwrap();
    assert_eq!(bytes[0], 2);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0x1000_1000);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 640);

    write_info(&mut machine, 2, 0x1000_1000, 0);
    assert_eq!(call(&mut machine, Interface::SoundPlayer, slot("SetInfo"), [player, info, 0, 0]), EBADPARM);
    write_info(&mut machine, 9, 0x1000_1000, 640);
    assert_eq!(call(&mut machine, Interface::SoundPlayer, slot("SetInfo"), [player, info, 0, 0]), EUNSUPPORTED);
    // A null `pInfo` resets the source.
    assert_eq!(call(&mut machine, Interface::SoundPlayer, slot("SetInfo"), [player, 0, 0, 0]), SUCCESS);
    call(&mut machine, Interface::SoundPlayer, slot("GetInfo"), [player, back, 0, 0]);
    machine.cpu.read_mem(back, &mut bytes).unwrap();
    assert_eq!(bytes, [0u8; 12]);
}

/// **A phone game is told the phone's screen; a Zeebo game, the console's.**
///
/// Devil May Cry reads only `cxScreen` and `cyScreen` and lays itself out from them. Told 640×480,
/// it centred a 240-pixel column and painted the rest almost white; told 240×400 — the size its
/// art was made for — it draws at the corner with nothing around it, and the frontends show just
/// that area. See [`Machine::visible_area`].
#[test]
fn a_phone_game_is_told_the_phone_screen() {
    let module = loader::load(&module_calling_malloc()).unwrap();
    let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
    machine.cpu.reset(&machine.module.mem).unwrap();
    let info = loader::HEAP_BASE;
    let screen = |machine: &mut Machine<UnicornCpu>| {
        machine.cpu.write_reg(Reg::R1, info);
        machine.shell_get_device_info().unwrap();
        let mut bytes = [0u8; 4];
        machine.cpu.read_mem(info, &mut bytes).unwrap();
        (
            u16::from_le_bytes([bytes[0], bytes[1]]),
            u16::from_le_bytes([bytes[2], bytes[3]]),
        )
    };

    assert_eq!(screen(&mut machine), (SCREEN_WIDTH, SCREEN_HEIGHT));
    assert_eq!(machine.visible_area(), None);

    machine.applet_class = 0x0107_0e36;
    assert_eq!(screen(&mut machine), (240, 400));
    assert_eq!(
        machine.visible_area(),
        Some(Rect { x: 0, y: 0, width: 240, height: 400 })
    );
}
