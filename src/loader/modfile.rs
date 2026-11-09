//! Parser for the `.mod` format — BREW's executable module.
//!
//! A `.mod` is produced by Qualcomm's `elf2mod` from an ARM ELF linked with `elf2mod.x`, which
//! sets `ro-base = 0x0` and forces `.text.AEEMod_Load` to the start of `.text`. Since the image
//! is linked at base zero, loading it at guest address 0 needs no relocation at all — which is
//! what we do today.
//!
//! Two variants show up in the real files:
//!
//! - `Raw`: the file starts directly with ARM code (`AEEMod_Load`). This is what the BREW SDK
//!   4.0.2 samples use.
//! - `BrewHeader`: the Zeebo titles carry two ARM branches, the `BREW` signature and a 0x40-byte
//!   header before the code.
//!
//! The meaning of most fields of the `BREW` header has not been confirmed yet — they are exposed
//! in [`BrewHeader::unknown`] instead of being given made-up names.

use std::fmt;

/// Signature that identifies the variant with a header.
const MAGIC: &[u8; 4] = b"BREW";
/// Offset of the signature within the file.
const MAGIC_OFFSET: usize = 0x08;
/// Size of the `BREW` header, in bytes. The code starts right after it.
const HEADER_LEN: usize = 0x40;

/// Encoding of an ARM `B <label>` instruction (condition `AL`, no link).
const ARM_BRANCH_MASK: u32 = 0xff00_0000;
const ARM_BRANCH_AL: u32 = 0xea00_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// File starts directly with ARM code.
    Raw,
    /// File carries the 0x40-byte `BREW` header.
    BrewHeader,
}

/// Header of the `BREW` variant, read raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrewHeader {
    /// Field at 0x0C. It is 1 in every module observed.
    pub version: u32,
    /// Field at 0x10. It is 0x40 in every module observed — the start of the code.
    pub code_offset: u32,
    /// Fields from 0x14 to 0x3C, not yet deciphered.
    ///
    /// Observed values (bjt.mod / tectoy.mod):
    /// `0x101 0x200 {0x7a120,0x8fdf0} {0x70,0x20} {0x234,0xa0} {0x690,0x40} 0x90 0x0
    ///  {0x7a120,0x8fdf0} {0x8000,0x0}`.
    pub unknown: [u32; 10],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// File too small to hold even a header.
    TooSmall { len: usize },
    /// The file has the `BREW` signature but the whole header does not fit.
    TruncatedHeader { len: usize },
    /// The first instruction is neither an ARM branch nor a plausible prologue.
    NotArmCode { first_word: u32 },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall { len } => {
                write!(f, "file too small for a .mod ({len} bytes)")
            }
            Self::TruncatedHeader { len } => {
                write!(f, "truncated BREW header (file has {len} bytes)")
            }
            Self::NotArmCode { first_word } => {
                write!(
                    f,
                    "first word {first_word:#010x} does not look like ARM code"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// A `.mod` loaded in host memory, ready to be mapped into the guest.
#[derive(Debug, Clone)]
pub struct ModImage {
    data: Vec<u8>,
    variant: Variant,
    header: Option<BrewHeader>,
    entry: u32,
}

impl ModImage {
    /// Interprets the bytes of a `.mod` file.
    pub fn parse(data: Vec<u8>) -> Result<Self, ParseError> {
        if data.len() < 8 {
            return Err(ParseError::TooSmall { len: data.len() });
        }

        let has_magic = data.len() >= MAGIC_OFFSET + 4
            && &data[MAGIC_OFFSET..MAGIC_OFFSET + 4] == MAGIC.as_slice();

        if has_magic {
            if data.len() < HEADER_LEN {
                return Err(ParseError::TruncatedHeader { len: data.len() });
            }
            let version = read_u32(&data, 0x0c);
            let code_offset = read_u32(&data, 0x10);
            let mut unknown = [0u32; 10];
            for (i, slot) in unknown.iter_mut().enumerate() {
                *slot = read_u32(&data, 0x14 + i * 4);
            }
            // The branch at 0x00 points to the real entry point; `code_offset` is the second
            // opinion on the same address.
            let entry = branch_target(read_u32(&data, 0), 0).unwrap_or(code_offset);
            return Ok(Self {
                data,
                variant: Variant::BrewHeader,
                header: Some(BrewHeader {
                    version,
                    code_offset,
                    unknown,
                }),
                entry,
            });
        }

        // No signature: the image starts directly at AEEMod_Load. We require the first word to
        // at least have a valid condition (high nibble != 0xF) so that we do not accept any file
        // as a module.
        let first_word = read_u32(&data, 0);
        if first_word >> 28 == 0xf {
            return Err(ParseError::NotArmCode { first_word });
        }
        Ok(Self {
            data,
            variant: Variant::Raw,
            header: None,
            entry: 0,
        })
    }

    /// Bytes of the image, in the order they must be mapped into the guest.
    // Comes into use when the loader maps the module into guest memory.
    #[allow(dead_code)]
    pub fn image(&self) -> &[u8] {
        &self.data
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    pub fn header(&self) -> Option<&BrewHeader> {
        self.header.as_ref()
    }

    /// Offset of `AEEMod_Load` within the image.
    ///
    /// Since the image is linked at ro-base 0, this value is also the absolute address of the
    /// entry point when the module is loaded at guest address 0.
    pub fn entry(&self) -> u32 {
        self.entry
    }
}

/// Reads a little-endian `u32`. The caller guarantees that `offset + 4 <= data.len()`.
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Resolves the target of an ARM `B <label>` located at `pc`.
///
/// The offset is a signed 24-bit value, in words, relative to `pc + 8` because of the pipeline.
fn branch_target(word: u32, pc: u32) -> Option<u32> {
    if word & ARM_BRANCH_MASK != ARM_BRANCH_AL {
        return None;
    }
    let imm24 = word & 0x00ff_ffff;
    // Sign-extends from 24 to 32 bits and converts from words to bytes.
    let offset = ((imm24 << 8) as i32 >> 8) * 4;
    Some(pc.wrapping_add(8).wrapping_add(offset as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic `.mod` with a BREW header and code starting at 0x40.
    fn brew_mod() -> Vec<u8> {
        let mut data = vec![0u8; 0x50];
        // b 0x40  ->  offset in words = (0x40 - 0 - 8) / 4 = 0x0e
        data[0x00..0x04].copy_from_slice(&0xea00_000eu32.to_le_bytes());
        // b 0x48
        data[0x04..0x08].copy_from_slice(&0xea00_000fu32.to_le_bytes());
        data[0x08..0x0c].copy_from_slice(MAGIC);
        data[0x0c..0x10].copy_from_slice(&1u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&0x40u32.to_le_bytes());
        data[0x14..0x18].copy_from_slice(&0x101u32.to_le_bytes());
        data
    }

    #[test]
    fn reads_brew_header_and_resolves_entry_from_branch() {
        let image = ModImage::parse(brew_mod()).unwrap();
        assert_eq!(image.variant(), Variant::BrewHeader);
        assert_eq!(image.entry(), 0x40);
        let header = image.header().unwrap();
        assert_eq!(header.version, 1);
        assert_eq!(header.code_offset, 0x40);
        assert_eq!(header.unknown[0], 0x101);
    }

    #[test]
    fn unsigned_module_enters_at_zero() {
        // push {lr} — first instruction of the BREW SDK samples.
        let data = 0xe52d_e004u32.to_le_bytes().repeat(4);
        let image = ModImage::parse(data).unwrap();
        assert_eq!(image.variant(), Variant::Raw);
        assert_eq!(image.entry(), 0);
    }

    #[test]
    fn rejects_file_not_starting_with_a_valid_instruction() {
        let data = 0xf000_0000u32.to_le_bytes().repeat(4);
        assert!(matches!(
            ModImage::parse(data),
            Err(ParseError::NotArmCode { .. })
        ));
    }

    #[test]
    fn rejects_file_too_short() {
        assert!(matches!(
            ModImage::parse(vec![0, 1, 2]),
            Err(ParseError::TooSmall { len: 3 })
        ));
    }

    #[test]
    fn negative_branch_jumps_backwards() {
        // b -8 : offset in words = -4  ->  0xea_fffffc, starting from pc = 0x20
        assert_eq!(branch_target(0xeaff_fffc, 0x20), Some(0x18));
    }
}
