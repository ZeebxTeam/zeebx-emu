//! Parser for the `.mif` — the Module Information File, which describes what a BREW module offers.
//!
//! The format is not public. What is here was worked out by comparing 24 real files: the `.mif`
//! files from BREW SDK 4.0.2 (whose ClassIDs we know from the `.bid` files), the simulator's system
//! modules, the OpenZeebo homebrew and the console titles.
//!
//! Confirmed structure:
//!
//! ```text
//! 0x00  u16 magic = 0x0011
//! 0x02  u16 version = 1
//! 0x04  u16 (always 1)
//! 0x06  u16 number of index entries
//! 0x08  u32 index offset              (0x20 in every file seen)
//! 0x0c  u32 index size                (entries * 8)
//! 0x10  u32 section table offset
//! 0x14  u32 number of sections
//! 0x18  u32 offset of the first section
//! 0x1c  u32 total size of the sections
//! ```
//!
//! The section table has `n + 1` offsets: each section runs from `offs[i]` to `offs[i+1]`, and the
//! last value is the file size.
//!
//! The applet record is **20 bytes** long and starts with the `AEECLSID`. Every applet `.mif`
//! examined has exactly one section of that size. The rule was validated against values known
//! from another source: `mediaplayer.mif` returns `0x01010EF6`, the same ClassID that the reverse
//! engineering of the Zeebo firmware recorded for the Media Player, and `274755.mif` (the Z-Wheel)
//! returns `0x01070798`, the App ID the firmware calls "TECTOY".

use std::fmt;

const MAGIC: u16 = 0x0011;
/// Size of the record that describes an applet.
const APPLET_RECORD_LEN: u32 = 20;
/// Size of the record that names a class: `<u32 ClassID> <u32 zero>`.
///
/// It appears on both sides of the same relationship, with **identical bytes**: in the game's
/// `.mif`, which declares the class it depends on, and in the `.mif` of the module that provides
/// it. What tells them apart is not the record, it is the file around it — see
/// [`MifFile::extension`].
const CLASS_RECORD_LEN: u32 = 8;
/// Words of the applet record that are always zero.
///
/// The record is `ClassID`, a zero, a number, another zero and a field that varies. Those two
/// zeros are what separate an applet record from some other section that happens to also be 20
/// bytes long.
const APPLET_ZEROS: [usize; 2] = [4, 12];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MifFile {
    /// ClassIDs of the applets the module exposes.
    pub applets: Vec<u32>,
    /// ClassIDs named in the class records: what the module provides, when it is an extension,
    /// or what it requires, when it is an applet.
    pub classes: Vec<u32>,
    /// Bounds of each section, as `(offset, size)`.
    pub sections: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MifError {
    BadMagic { found: u16 },
    Truncated { needed: usize, len: usize },
}

impl fmt::Display for MifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { found } => write!(f, "does not look like a .mif (magic {found:#06x})"),
            Self::Truncated { needed, len } => {
                write!(f, "truncated .mif: needs {needed} bytes, has {len}")
            }
        }
    }
}

impl std::error::Error for MifError {}

impl MifFile {
    pub fn parse(data: &[u8]) -> Result<Self, MifError> {
        let magic = read_u16(data, 0)?;
        if magic != MAGIC {
            return Err(MifError::BadMagic { found: magic });
        }

        let table_offset = read_u32(data, 0x10)? as usize;
        let section_count = read_u32(data, 0x14)? as usize;

        // The table holds n+1 offsets: the extra one closes the last section.
        let mut bounds = Vec::with_capacity(section_count + 1);
        for i in 0..=section_count {
            bounds.push(read_u32(data, table_offset + i * 4)?);
        }

        // In every real file examined the last bound is exactly the file size. Requiring it
        // keeps a truncated `.mif` from slipping through and simply reporting no applet at all.
        if let Some(&last) = bounds.last() {
            if last as usize > data.len() {
                return Err(MifError::Truncated {
                    needed: last as usize,
                    len: data.len(),
                });
            }
        }

        let sections: Vec<(u32, u32)> = bounds
            .windows(2)
            .map(|w| (w[0], w[1].saturating_sub(w[0])))
            .collect();

        let applets = sections
            .iter()
            .filter(|&&(_, len)| len == APPLET_RECORD_LEN)
            .filter(|&&(offset, _)| is_applet_record(data, offset as usize))
            .filter_map(|&(offset, _)| read_u32(data, offset as usize).ok())
            .filter(|&id| id != 0)
            .collect();

        let classes = sections
            .iter()
            .filter(|&&(_, len)| len == CLASS_RECORD_LEN)
            .filter_map(|&(offset, _)| {
                let clsid = read_u32(data, offset as usize).ok()?;
                // The record ends in zero, and the real files have an 8-byte one that is
                // **entirely** zeroed — rejecting both is what keeps a `0x0` out of the list.
                let rest = read_u32(data, offset as usize + 4).ok()?;
                (clsid != 0 && rest == 0).then_some(clsid)
            })
            .collect();

        Ok(Self {
            applets,
            classes,
            sections,
        })
    }

    /// The module's images — the title's icons —, in the order they appear.
    ///
    /// An image section starts with a `u16` holding the length **of the header itself**, followed
    /// by the zero-terminated MIME type; the file comes right after. Real `.mif` files always
    /// write 12 and `image/png`, `image/bmp` or `image/jpg`.
    pub fn images<'d>(&self, data: &'d [u8]) -> Vec<&'d [u8]> {
        self.sections
            .iter()
            .filter_map(|&(offset, len)| {
                let section = data.get(offset as usize..(offset + len) as usize)?;
                let header = u16::from_le_bytes([*section.first()?, *section.get(1)?]) as usize;
                let mime = section.get(2..header)?.split(|&b| b == 0).next()?;
                mime.starts_with(b"image/")
                    .then(|| section.get(header..))
                    .flatten()
            })
            .collect()
    }

    /// ClassID of the main applet — the one the emulator instantiates.
    pub fn main_applet(&self) -> Option<u32> {
        self.applets.first().copied()
    }

    /// The classes this module **provides**, when it is an extension module.
    ///
    /// An extension module has no applet: it exists only to export classes, and the console loads
    /// it when someone asks for one of them. Two titles we have depend on this, and in both the
    /// extension module ships inside the package itself:
    ///
    /// | title | module | class |
    /// |---|---|---|
    /// | Action Hero 3D | `mod/12875/imicro3d.mod` | `0x010292c3` |
    /// | Kingdom Hearts | `Kingdon Hearts/kh.mod` | `0x0102bbfc` |
    ///
    /// The class record is the same in both `.mif` files of the relationship, so "having an
    /// applet" is what separates the one asking from the one providing. Without that distinction,
    /// the game's `.mif` would offer to serve the very class it is asking for.
    pub fn extension(&self) -> &[u32] {
        if self.applets.is_empty() {
            &self.classes
        } else {
            &[]
        }
    }
}

/// Whether the 20-byte section at `offset` has the shape of an applet record.
///
/// The rule used to be different: require the ClassID to fall in Qualcomm's `0x01xxxxxx` range.
/// It worked for 62 of the 67 records in the games we have, and left out Zenonia (`0xbf2e2021`) —
/// which would not open because of it. The shape gets all 67 right: it takes the 62 from before,
/// plus Zenonia and a homebrew with ClassID `0x12345678`, and still rejects the three 20-byte
/// sections that are not applets at all (Prey Evil, Reckless Racing and Zeebo App, all starting
/// with `0x00xxfeff`).
fn is_applet_record(data: &[u8], offset: usize) -> bool {
    APPLET_ZEROS
        .iter()
        .all(|&field| read_u32(data, offset + field) == Ok(0))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, MifError> {
    let bytes = slice(data, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, MifError> {
    let b = slice(data, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8], MifError> {
    let end = offset.checked_add(len).ok_or(MifError::Truncated {
        needed: len,
        len: data.len(),
    })?;
    data.get(offset..end).ok_or(MifError::Truncated {
        needed: end,
        len: data.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_come_from_sections_that_declare_a_mime_type() {
        // The header counts its own size: reading the length as if it covered only the text
        // would leave two bytes of the file behind and no decoder would accept the image.
        let mut section = Vec::new();
        section.extend_from_slice(&12u16.to_le_bytes());
        section.extend_from_slice(b"image/png\0");
        section.extend_from_slice(b"CONTEUDO");

        let mut other = Vec::new();
        other.extend_from_slice(&12u16.to_le_bytes());
        other.extend_from_slice(b"text/plain");
        other.extend_from_slice(b"nao e imagem");

        let mut data = vec![0u8; 4];
        let mut sections = Vec::new();
        for part in [&section, &other] {
            sections.push((data.len() as u32, part.len() as u32));
            data.extend_from_slice(part);
        }
        let mif = MifFile {
            applets: Vec::new(),
            classes: Vec::new(),
            sections,
        };
        assert_eq!(mif.images(&data), vec![b"CONTEUDO".as_slice()]);
    }

    #[test]
    fn a_truncated_section_does_not_become_an_image() {
        let mif = MifFile {
            applets: Vec::new(),
            classes: Vec::new(),
            sections: vec![(0, 40)],
        };
        assert!(mif.images(&[0u8; 4]).is_empty());
    }

    /// Builds a minimal `.mif` with two sections, the second being the applet record.
    fn mif_with_applet(clsid: u32) -> Vec<u8> {
        let table_offset = 0x20u32;
        let sections = [0x40u32, 0x50, 0x64];
        let mut data = vec![0u8; 0x64];
        data[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        data[2..4].copy_from_slice(&1u16.to_le_bytes());
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        data[6..8].copy_from_slice(&2u16.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&table_offset.to_le_bytes());
        data[0x14..0x18].copy_from_slice(&2u32.to_le_bytes());
        for (i, offset) in sections.iter().enumerate() {
            let at = table_offset as usize + i * 4;
            data[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        }
        // Section 1: 0x40..0x50 (16 bytes, not an applet). Section 2: 0x50..0x64 (20 bytes).
        data[0x50..0x54].copy_from_slice(&clsid.to_le_bytes());
        data
    }

    /// Builds a minimal `.mif` with three sections of 8 bytes: a class record, a zeroed one and
    /// one more. This is the shape of the two real extension `.mif` files, which carry the record
    /// followed by an 8-byte section that is entirely zeroed.
    fn extension_mif(clsid: u32, applet: Option<u32>) -> Vec<u8> {
        let table_offset = 0x20u32;
        let sections = [0x40u32, 0x48, 0x50, 0x64];
        let mut data = vec![0u8; 0x64];
        data[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&table_offset.to_le_bytes());
        data[0x14..0x18].copy_from_slice(&3u32.to_le_bytes());
        for (i, offset) in sections.iter().enumerate() {
            let at = table_offset as usize + i * 4;
            data[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        }
        // 0x40..0x48: the class record. 0x48..0x50: the eight zeroed bytes.
        data[0x40..0x44].copy_from_slice(&clsid.to_le_bytes());
        // 0x50..0x64: twenty bytes, which only become an applet if they have the shape of one.
        if let Some(applet) = applet {
            data[0x50..0x54].copy_from_slice(&applet.to_le_bytes());
        } else {
            // Breaks the applet record shape by putting a value in one of the fields that are zero.
            data[0x54..0x58].copy_from_slice(&1u32.to_le_bytes());
        }
        data
    }

    /// The 8-byte record is what names an extension's class. The **zeroed** eight-byte one that
    /// follows it in the real files must not make it into the list: it would yield a `0x0` that
    /// would offer to serve any request for ClassID zero.
    #[test]
    fn the_class_record_comes_out_and_the_zeroed_one_does_not() {
        let mif = MifFile::parse(&extension_mif(0x0102_92c3, None)).unwrap();
        assert_eq!(mif.classes, vec![0x0102_92c3]);
    }

    /// **The rule that separates the one providing from the one asking.** Both `.mif` files of
    /// the relationship carry the same record, with the same bytes: the game's declaring the
    /// dependency and the extension's declaring what it provides. Without "having an applet" as
    /// the criterion, the Action Hero 3D manifest would offer to serve the `0x010292c3` it is
    /// itself asking for — and the emulator would call the game's `AEEMod_Load` again, from
    /// inside the dispatch, to create it.
    #[test]
    fn only_the_mif_without_an_applet_offers_to_provide_the_class() {
        let extension = MifFile::parse(&extension_mif(0x0102_92c3, None)).unwrap();
        assert_eq!(extension.extension(), &[0x0102_92c3]);

        let game = MifFile::parse(&extension_mif(0x0102_92c3, Some(0x0108_1970))).unwrap();
        assert_eq!(game.applets, vec![0x0108_1970], "this is the game's");
        assert_eq!(game.classes, vec![0x0102_92c3], "and it names the same class");
        assert!(
            game.extension().is_empty(),
            "but does not offer to provide it"
        );
    }

    #[test]
    fn extracts_the_applet_classid() {
        let mif = MifFile::parse(&mif_with_applet(0x0100_9ff0)).unwrap();
        assert_eq!(mif.applets, vec![0x0100_9ff0]);
        assert_eq!(mif.main_applet(), Some(0x0100_9ff0));
    }

    /// Zenonia declares its applet with ClassID `0xbf2e2021`, and a homebrew in the set uses
    /// `0x12345678`. The old rule required Qualcomm's `0x01xxxxxx` range and rejected both:
    /// Zenonia would not open because of it.
    #[test]
    fn accepts_classid_outside_the_qualcomm_range() {
        let mif = MifFile::parse(&mif_with_applet(0xbf2e_2021)).unwrap();
        assert_eq!(mif.main_applet(), Some(0xbf2e_2021));
    }

    /// Not every 20-byte section is an applet: Prey Evil, Reckless Racing and Zeebo App have one
    /// that starts with `0x00xxfeff` and is not. What separates them is the shape — the applet
    /// record has zeros in the second and fourth fields.
    #[test]
    fn ignores_a_20_byte_section_not_shaped_like_an_applet() {
        let mut data = mif_with_applet(0x0050_feff);
        data[0x54..0x58].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let mif = MifFile::parse(&data).unwrap();
        assert!(mif.applets.is_empty());
        assert_eq!(mif.main_applet(), None);
    }

    #[test]
    fn reads_the_section_bounds() {
        let mif = MifFile::parse(&mif_with_applet(0x0100_9ff0)).unwrap();
        assert_eq!(mif.sections, vec![(0x40, 0x10), (0x50, 20)]);
    }

    #[test]
    fn rejects_file_with_wrong_magic() {
        let mut data = mif_with_applet(0x0100_9ff0);
        data[0] = 0xff;
        assert!(matches!(
            MifFile::parse(&data),
            Err(MifError::BadMagic { .. })
        ));
    }

    #[test]
    fn rejects_truncated_file() {
        let data = mif_with_applet(0x0100_9ff0);
        assert!(matches!(
            MifFile::parse(&data[..0x30]),
            Err(MifError::Truncated { .. })
        ));
    }
}
