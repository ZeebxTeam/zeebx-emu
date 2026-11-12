//! `IFont`: the system bitmap fonts.
//!
//! BREW does not draw text only with the console's font: it has ready-made **font classes**, and
//! the application asks `ISHELL_CreateInstance` for one of them. There are 29 classes in four
//! families:
//!
//! | Family | Classes |
//! |---|---|
//! | `AEECLSID_FONTSYS*` | `NORMAL`, `LARGE`, `BOLD`, `ITALIC`, `BOLDITALIC`, `LARGEITALIC` |
//! | `AEECLSID_FONT_STANDARD*` | `11`, `11B`, `15`, `15B`, `18`, `18B`, `23`, `23B`, `26`, `26B`, `36` |
//! | `AEECLSID_FONT_BASIC*` | `6`, `9`, `10`, `11`, `11B`, `12`, `12B`, `14`, `15` |
//! | standalone | `AEECLSID_FONT`, `FONT_FIXED4X6`, `BITFONTFOUNDRY` |
//!
//! **It is not `ITypeface`.** `ITypeface` creates fonts from a TTF; `IFont` already **is** the
//! drawable font. The two interfaces have different methods, and treating them as the same thing
//! is what made Double Dragon, Resident Evil 4 and the Data East ports fall into the
//! "Memory is insufficient. Please delete some files." screen — their generic failure message when
//! the font's `CreateInstance` comes back null.
//!
//! The metrics table comes from the reference code that already implements this interface
//! (`zeebo-emulator/research/sources/zeemu/brew/BrewFont.cpp`). The order of the six methods does
//! not: it is `INHERIT_IFont` in `inc/AEEIFont.h`, see [`crate::brew::aee_slots::FONT`]. The
//! method signatures come from the SDK's own examples:
//!
//! ```c
//! IFont_GetInfo(pFont, &info, sizeof(info))
//! IFont_MeasureText(pFont, pszBuf, WSTRLEN(pszBuf), IFONT_MAXWIDTH, &nChars, &extent.width)
//! ```

use super::*;

/// What a system font says about itself, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Metrics {
    pub ascent: u16,
    pub descent: u16,
    pub leading: u16,
    pub max_char_width: u16,
    pub height: u16,
    pub bold: bool,
    pub italic: bool,
}

/// The classes this interface answers for.
///
/// Without this list, `CreateInstance` for these classes returns "not supported" — and the game,
/// which does not tell "does not exist" from "out of memory", shows the out-of-memory screen.
pub const FONT_CLASSES: [u32; 29] = [
    // FONTSYS
    0x0101_2786,
    0x0101_2787,
    0x0101_2788,
    0x0101_402c,
    0x0101_402d,
    0x0101_402e,
    // FONT
    0x0100_1022,
    // FONT_STANDARD
    0x0102_f679,
    0x0102_f67a,
    0x0103_0852,
    0x0103_0853,
    0x0102_f67b,
    0x0102_f67c,
    0x0102_f67d,
    0x0102_f67e,
    0x0102_f67f,
    0x0102_f680,
    0x0102_f681,
    // FIXED4X6
    0x0100_a001,
    // BASIC
    0x0100_a002,
    0x0100_a003,
    0x0100_a004,
    0x0100_a005,
    0x0100_a006,
    0x0100_a007,
    0x0100_a008,
    0x0100_a009,
    0x0100_a00a,
    // BITFONTFOUNDRY
    0x0100_a100,
];

/// The metrics of each class, as the reference has them.
///
/// The numbers are declared in the `.bid` for some (`FONT_STANDARD11` has ascent 13 and descent 3)
/// and measured in the reference for the rest. The two sources agree where they overlap.
pub fn class_metrics(class_id: u32) -> Metrics {
    let (ascent, descent, leading, max_char_width, height, bold) = match class_id {
        0x0100_a001 => (4, 2, 0, 4, 6, false),
        0x0100_a002 => (6, 2, 1, 5, 9, false),
        0x0100_a003 => (9, 3, 1, 6, 13, false),
        0x0100_a004 => (10, 3, 1, 7, 14, false),
        0x0100_a005 | 0x0101_2786 | 0x0101_402c | 0x0100_1022 => (13, 3, 2, 8, 18, false),
        0x0100_a006 | 0x0101_2788 | 0x0101_402d => (13, 3, 2, 8, 18, true),
        0x0100_a007 => (14, 4, 2, 9, 20, false),
        0x0100_a008 => (12, 4, 2, 8, 18, false),
        0x0100_a009 => (12, 4, 2, 8, 18, true),
        0x0100_a00a => (15, 4, 2, 10, 21, false),
        0x0101_2787 | 0x0101_402e => (21, 5, 2, 10, 28, false),
        // The `FONT_STANDARD` family, one line per class, with `ascent` and `descent` **copied
        // from `AEEFontsStandard.BID`** and the bold that the `.bid`'s own text declares
        // ("Bolded"). The table got three wrong: `STANDARD15B` answered 17/4 — the 18B numbers —,
        // and the bold of 23B and 26B lived in a patch outside the table, where the next edit
        // wouldn't see it. A game that reserves height from the `GetInfo` answer draws line over
        // line when it comes back wrong, and the 2 px difference between 15/3 and 17/4 is the
        // height of a whole line on a 320-pixel screen.
        0x0102_f679 => (13, 3, 2, 8, 18, false), // STANDARD11
        0x0102_f67a => (13, 3, 2, 8, 18, true),  // STANDARD11B
        0x0103_0852 => (15, 3, 2, 8, 20, false), // STANDARD15
        0x0103_0853 => (15, 3, 2, 8, 20, true),  // STANDARD15B
        0x0102_f67b => (17, 4, 2, 9, 23, false), // STANDARD18
        0x0102_f67c => (17, 4, 2, 9, 23, true),  // STANDARD18B
        0x0102_f67d => (21, 5, 2, 10, 28, false), // STANDARD23
        0x0102_f67e => (21, 5, 2, 10, 28, true), // STANDARD23B
        0x0102_f67f => (23, 6, 2, 10, 31, false), // STANDARD26
        0x0102_f680 => (23, 6, 2, 10, 31, true), // STANDARD26B
        // The largest in the system, used for big titles.
        0x0102_f681 => (38, 10, 2, 12, 50, false), // STANDARD36
        _ => (12, 4, 2, 8, 16, false),
    };
    let is_bold = bold;
    Metrics {
        ascent,
        descent,
        leading,
        max_char_width,
        height,
        bold: is_bold,
        italic: matches!(class_id, 0x0101_402c | 0x0101_402d | 0x0101_402e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vtable order of `INHERIT_IFont` in `inc/AEEIFont.h`. The alphabetical `#define`s of
    /// `AEEFont.h` put `GetInfo` first, and that is how 4 and 5 were swapped here once.
    #[test]
    fn the_font_slots_follow_inherit_ifont() {
        let slots: Vec<_> = (0..6).map(|slot| Interface::Font.method(slot)).collect();
        assert_eq!(
            slots,
            [
                Some("AddRef"),
                Some("Release"),
                Some("QueryInterface"),
                Some("DrawText"),
                Some("MeasureText"),
                Some("GetInfo"),
            ]
        );
    }

    /// **The `FONT_STANDARD` metrics are transcribed from the `.bid`, not made up.**
    ///
    /// `AEEFontsStandard.BID` declares ascent and descent for the eleven classes, and says which
    /// are bold. This test is a literal copy of that file: if someone "fixes" the table from
    /// memory, it disagrees — which is the point, because memory already got three lines wrong
    /// once.
    #[test]
    fn the_standard_metrics_match_the_bid() {
        let bid = [
            (0x0102_f679u32, 13u16, 3u16, false), // STANDARD11
            (0x0102_f67a, 13, 3, true),           // STANDARD11B
            (0x0103_0852, 15, 3, false),          // STANDARD15
            (0x0103_0853, 15, 3, true),           // STANDARD15B
            (0x0102_f67b, 17, 4, false),          // STANDARD18
            (0x0102_f67c, 17, 4, true),           // STANDARD18B
            (0x0102_f67d, 21, 5, false),          // STANDARD23
            (0x0102_f67e, 21, 5, true),           // STANDARD23B
            (0x0102_f67f, 23, 6, false),          // STANDARD26
            (0x0102_f680, 23, 6, true),           // STANDARD26B
            (0x0102_f681, 38, 10, false),         // STANDARD36
        ];
        for (class_id, ascent, descent, is_bold) in bid {
            let measured = class_metrics(class_id);
            assert_eq!(
                (measured.ascent, measured.descent),
                (ascent, descent),
                "ascent/descent of class {class_id:#010x}"
            );
            assert_eq!(measured.bold, is_bold, "bold of class {class_id:#010x}");
            assert!(
                measured.height >= measured.ascent + measured.descent,
                "height smaller than ascent plus descent in {class_id:#010x}"
            );
        }
    }

    /// `STANDARD15B` answers like the 15, not like the 18: that was the defect that was here.
    #[test]
    fn standard15b_does_not_inherit_the_18b_numbers() {
        let fifteen_b = class_metrics(0x0103_0853);
        let fifteen = class_metrics(0x0103_0852);
        assert_eq!((fifteen_b.ascent, fifteen_b.descent), (fifteen.ascent, fifteen.descent));
        assert_ne!((fifteen_b.ascent, fifteen_b.descent), (17, 4));
    }
}

impl<C: CpuBackend> Machine<C> {
    /// Creates the object of a system font, recording which class it is.
    pub(super) fn create_font(&mut self, class_id: u32, out: u32) -> Result<u32, CpuError> {
        let object = self.new_object(Interface::Font)?;
        if object == 0 {
            return Ok(ENOMEMORY);
        }
        self.fonts.insert(object, class_metrics(class_id));
        if out != 0 {
            self.cpu.write_u32(out, object)?;
        }
        Ok(SUCCESS)
    }

    /// Serves `IFont`.
    pub(super) fn font_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Font.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let metrics = self.fonts.get(&this).copied().unwrap_or_default();
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.fonts.remove(&this);
                }
                remaining
            }
            // int QueryInterface(IFont *, AEECLSID, void **ppv)
            "QueryInterface" => {
                let out = self.cpu.read_reg(Reg::R2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int GetInfo(IFont *, AEEFontInfo *pInfo, int nSize)
            //
            // The struct starts with four `int16` — ascent, descent, leading and the maximum
            // character width. Only what fits in the size the caller declared is written.
            "GetInfo" => {
                let info = self.cpu.read_reg(Reg::R1);
                let size = self.cpu.read_reg(Reg::R2);
                if info != 0 {
                    let fields: [u16; 5] = [
                        metrics.ascent,
                        metrics.descent,
                        metrics.leading,
                        metrics.max_char_width,
                        metrics.height,
                    ];
                    for (i, value) in fields.iter().enumerate() {
                        let offset = 4 + i as u32 * 2;
                        if size != 0 && offset + 2 > size {
                            break;
                        }
                        self.cpu.write_mem(info + offset, &value.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            // int MeasureText(IFont *, const AECHAR *pchText, int nChars, int nMaxWidth,
            //                 int *pnChars, int *pnWidth)
            //
            // The signature comes from the SDK's own example. `nChars` may be `-1`, which means
            // "up to the terminator".
            "MeasureText" => {
                let text = self.cpu.read_reg(Reg::R1);
                let requested = self.cpu.read_reg(Reg::R2) as i32;
                let out_chars = self.stack_arg(0)?;
                let out_width = self.stack_arg(1)?;
                let mut count = 0u32;
                if text != 0 {
                    let mut cursor = text;
                    let limit = match requested {
                        n if n < 0 => MAX_STRING as u32,
                        n => n as u32,
                    };
                    while count < limit && count < MAX_STRING as u32 {
                        let mut pair = [0u8; 2];
                        self.cpu.read_mem(cursor, &mut pair)?;
                        if u16::from_le_bytes(pair) == 0 {
                            break;
                        }
                        count += 1;
                        cursor += 2;
                    }
                }
                if out_chars != 0 {
                    self.cpu.write_u32(out_chars, count)?;
                }
                if out_width != 0 {
                    self.cpu
                        .write_u32(out_width, count * u32::from(metrics.max_char_width))?;
                }
                SUCCESS
            }
            // int DrawText(IFont *, const AECHAR *pchText, int nChars, int x, int y, ...)
            //
            // **The argument layout has not been measured yet**, unlike the other three: the
            // reference only prints the text and does not draw. Here we draw to the current
            // destination, which is what `IDISPLAY` does, and the assumption goes into the report
            // for whoever measures it.
            "DrawText" => {
                let text = self.cpu.read_reg(Reg::R1);
                let requested = self.cpu.read_reg(Reg::R2) as i32;
                let x = self.cpu.read_reg(Reg::R3) as i32;
                let y = self.stack_arg(0)? as i32;
                self.assumptions
                    .insert("IFont::DrawText draws with an inferred layout, not yet measured");
                if text != 0 {
                    let mut units = self.read_aechar_units(text)?;
                    if requested >= 0 {
                        units.truncate(requested as usize);
                    }
                    let text = String::from_utf16_lossy(&units);
                    self.draw_text(&text, x, y)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }
}
