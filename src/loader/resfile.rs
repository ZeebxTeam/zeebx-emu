//! BREW resource files — the `.bar` files.
//!
//! It is the **same container as the `.mif`**: header `0x0011`, section table with `n + 1`
//! offsets, and the content stuck at the end. The difference is the index, which the `.mif` does
//! not use: a table of 8-byte entries that says which section each resource lives in.
//!
//! ```text
//! 0x06  u16 number of index entries
//! 0x08  u32 index offset
//! 0x0c  u32 index size (entries * 8)
//! 0x10  u32 section table offset
//! 0x14  u32 number of sections
//! ```
//!
//! Each entry is `u16 kind, u16 id, u16 a, u16 b`, and covers a **range** of resources: the ids
//! `id..=id + a` live in the sections `b..=b + a`. The rule was confirmed in the two real `.bar`
//! files we have: in `pacmania.bar` the last entry gives exactly the sections 126 to 142, and
//! the file has 143; in Peggle's `resources.bar` the start of each entry is the end of the
//! previous one plus one, from the first resource to the last.
//!
//! The types are those of `ResType` in `AEEShell.h`: `RESTYPE_STRING = 1`,
//! `RESTYPE_IMAGE = 6` (data with an `AEEResBlob` header) and `RESTYPE_BINARY = 0x5000`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAGIC: u16 = 0x0011;

/// `RESTYPE_STRING`, from `AEEShell.h`.
pub const RESTYPE_STRING: u16 = 1;

/// `RESTYPE_IMAGE`, from `AEEShell.h`.
pub const RESTYPE_IMAGE: u16 = 6;

/// The content of an image resource, without the `AEEResBlob` header that comes in front.
///
/// The header is a `uint16` with the offset to the data, followed by the zero-terminated MIME
/// type — it is what the SDK's `RESBLOB_DATA()` skips. In `tekken2.bar` entry 5034 starts with
/// `0c 00` and `"image/bmp\0"`, and the `BM` comes at byte 12.
///
/// Whoever calls `LoadResData` gets the whole block, header included, because the documentation
/// says that is what comes out of it. Whoever asks for an image **object** does not: there the
/// header is ours to interpret.
pub fn blob_data(raw: &[u8]) -> Option<&[u8]> {
    let offset = u16::from_le_bytes([*raw.first()?, *raw.get(1)?]) as usize;
    // The byte before the data is the zero that terminates the MIME type. Without this check, a
    // resource that is not a blob would become an arbitrary slice of its own content.
    match raw.get(offset.checked_sub(1)?) == Some(&0) {
        true => raw.get(offset..),
        false => None,
    }
}

/// A range of resources from the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    kind: u16,
    first_id: u16,
    /// How many resources the range covers. It is `a + 1` in the file.
    count: u32,
    first_section: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResFile {
    data: Vec<u8>,
    /// Bounds of each section, as `(offset, size)`.
    sections: Vec<(u32, u32)>,
    ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResError {
    BadMagic { found: u16 },
    Truncated,
}

impl std::fmt::Display for ResError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic { found } => write!(f, "does not look like a .bar (magic {found:#06x})"),
            Self::Truncated => write!(f, "truncated resource file"),
        }
    }
}

impl ResFile {
    pub fn parse(data: Vec<u8>) -> Result<Self, ResError> {
        let magic = u16(&data, 0)?;
        if magic != MAGIC {
            return Err(ResError::BadMagic { found: magic });
        }
        let index_count = u16(&data, 0x06)? as usize;
        let index_offset = u32(&data, 0x08)? as usize;
        let table_offset = u32(&data, 0x10)? as usize;
        let section_count = u32(&data, 0x14)? as usize;

        // The table holds n+1 offsets: the extra one closes the last section.
        let mut bounds = Vec::with_capacity(section_count + 1);
        for i in 0..=section_count {
            bounds.push(u32(&data, table_offset + i * 4)?);
        }
        if bounds
            .last()
            .is_some_and(|&last| last as usize > data.len())
        {
            return Err(ResError::Truncated);
        }
        let sections: Vec<(u32, u32)> = bounds
            .windows(2)
            .map(|w| (w[0], w[1].saturating_sub(w[0])))
            .collect();

        let mut ranges = Vec::with_capacity(index_count);
        for i in 0..index_count {
            let at = index_offset + i * 8;
            ranges.push(Range {
                kind: u16(&data, at)?,
                first_id: u16(&data, at + 2)?,
                count: u32::from(u16(&data, at + 4)?) + 1,
                first_section: u16(&data, at + 6)?,
            });
        }
        Ok(Self {
            data,
            sections,
            ranges,
        })
    }

    /// The raw content of resource `(kind, id)`.
    pub fn get(&self, kind: u16, id: u16) -> Option<&[u8]> {
        let section = self.section_of(kind, id)?;
        let &(offset, len) = self.sections.get(section)?;
        self.data
            .get(offset as usize..(offset as usize).saturating_add(len as usize))
    }

    fn section_of(&self, kind: u16, id: u16) -> Option<usize> {
        self.ranges.iter().find_map(|range| {
            if range.kind != kind {
                return None;
            }
            let offset = u32::from(id).checked_sub(u32::from(range.first_id))?;
            match offset < range.count {
                true => Some(range.first_section as usize + offset as usize),
                false => None,
            }
        })
    }

    /// The text of a string resource, in UTF-16 as BREW delivers it.
    ///
    /// The section states its own encoding at the start. Two show up in the files we have:
    ///
    /// - **UTF-16**, marked by a BOM — `FF FE` for little-endian, `FE FF` for big-endian. It is
    ///   the one in the Z-Wheel's `tectoy_pt.brf`, and the text comes in two-byte units.
    /// - **ISO-Latin-1**, with an encoding byte in front and one code point per byte. It is the
    ///   one in the games' `.bar` files.
    ///
    /// Reading everything as Latin-1 turned the BOM into text and made the high byte of the first
    /// character end the string at the following zero: `"Meus Z-Credits"` came out as `"þM"`, and
    /// that is what the Z-Wheel put in the widgets of the z-pad form.
    ///
    /// The text may carry several items separated by `^`, but that is the game's own packing:
    /// what BREW delivers is the whole section.
    pub fn string(&self, id: u16) -> Option<Vec<u16>> {
        let raw = self.get(RESTYPE_STRING, id)?;
        let wide = |pairs: &[u8], large: bool| {
            pairs
                .chunks_exact(2)
                .map(|p| match large {
                    true => u16::from_be_bytes([p[0], p[1]]),
                    false => u16::from_le_bytes([p[0], p[1]]),
                })
                .take_while(|&u| u != 0)
                .collect()
        };
        match raw {
            [0xff, 0xfe, rest @ ..] => Some(wide(rest, false)),
            [0xfe, 0xff, rest @ ..] => Some(wide(rest, true)),
            _ => {
                let (_encoding, text) = raw.split_first()?;
                Some(
                    text.iter()
                        .take_while(|&&b| b != 0)
                        .map(|&b| u16::from(b))
                        .collect(),
                )
            }
        }
    }
}

/// Keeps the `.bar` files already opened, so as not to reread a thirteen-megabyte file for every
/// resource.
#[derive(Debug, Default)]
pub struct ResCache {
    files: HashMap<PathBuf, Option<ResFile>>,
}

impl ResCache {
    /// The resource file at `path`, read the first time it is requested.
    ///
    /// Whatever does not open is marked as absent, and is not tried again: a game that asks for
    /// the same resource every frame cannot cause one disk read per frame.
    pub fn open(&mut self, path: &Path) -> Option<&ResFile> {
        self.files
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let bytes = std::fs::read(path).ok()?;
                ResFile::parse(bytes).ok()
            })
            .as_ref()
    }
}

fn u16(data: &[u8], at: usize) -> Result<u16, ResError> {
    let bytes = data.get(at..at + 2).ok_or(ResError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32(data: &[u8], at: usize) -> Result<u32, ResError> {
    let b = data.get(at..at + 4).ok_or(ResError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blob_header_is_skipped_by_the_offset() {
        // Entry 5034 of `tekken2.bar`, the way it starts in the file.
        let mut raw = vec![0x0c, 0x00];
        raw.extend(b"image/bmp\0");
        raw.extend(b"BM\x38\xb4");
        assert_eq!(blob_data(&raw), Some(&b"BM\x38\xb4"[..]));
    }

    #[test]
    fn non_blobs_do_not_become_arbitrary_slices() {
        // A loose PNG: the offset would read garbage, and the zero that ends the MIME is not there.
        assert_eq!(blob_data(&[0x89, b'P', b'N', b'G', 1, 2, 3, 4]), None);
        assert_eq!(blob_data(&[]), None);
        assert_eq!(blob_data(&[0x0c, 0x00]), None);
    }

    /// Builds a `.bar` with the given ranges and sections.
    fn build(ranges: &[(u16, u16, u16, u16)], sections: &[&[u8]]) -> Vec<u8> {
        let index_offset = 0x20usize;
        let table_offset = index_offset + ranges.len() * 8;
        let first_section = table_offset + (sections.len() + 1) * 4;

        let mut header = vec![0u8; 0x20];
        header[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        header[2..4].copy_from_slice(&1u16.to_le_bytes());
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..8].copy_from_slice(&(ranges.len() as u16).to_le_bytes());
        header[8..12].copy_from_slice(&(index_offset as u32).to_le_bytes());
        header[12..16].copy_from_slice(&((ranges.len() * 8) as u32).to_le_bytes());
        header[16..20].copy_from_slice(&(table_offset as u32).to_le_bytes());
        header[20..24].copy_from_slice(&(sections.len() as u32).to_le_bytes());
        header[24..28].copy_from_slice(&(first_section as u32).to_le_bytes());

        let mut index = Vec::new();
        for (kind, id, a, b) in ranges {
            index.extend_from_slice(&kind.to_le_bytes());
            index.extend_from_slice(&id.to_le_bytes());
            index.extend_from_slice(&a.to_le_bytes());
            index.extend_from_slice(&b.to_le_bytes());
        }

        let mut table = Vec::new();
        let mut at = first_section as u32;
        for section in sections {
            table.extend_from_slice(&at.to_le_bytes());
            at += section.len() as u32;
        }
        table.extend_from_slice(&at.to_le_bytes());

        let mut out = header;
        out.extend_from_slice(&index);
        out.extend_from_slice(&table);
        for section in sections {
            out.extend_from_slice(section);
        }
        out
    }

    /// Section 44 of the Z-Wheel's `tectoy_pt.brf`, as it is in the file: BOM and UTF-16LE.
    #[test]
    fn a_section_with_bom_is_read_as_utf16() {
        let mut section = vec![0xff, 0xfe];
        for u in "Meus Z-Credits".encode_utf16() {
            section.extend_from_slice(&u.to_le_bytes());
        }
        let bar = build(&[(RESTYPE_STRING, 1049, 0, 0)], &[&section]);
        let res = ResFile::parse(bar).unwrap();
        let loaded = String::from_utf16_lossy(&res.string(1049).unwrap());
        assert_eq!(loaded, "Meus Z-Credits");
    }

    /// The other direction of the BOM, which none of our files use and the format allows.
    #[test]
    fn a_reversed_bom_is_also_read() {
        let mut section = vec![0xfe, 0xff];
        for u in "Idioma".encode_utf16() {
            section.extend_from_slice(&u.to_be_bytes());
        }
        let bar = build(&[(RESTYPE_STRING, 1045, 0, 0)], &[&section]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&res.string(1045).unwrap()),
            "Idioma"
        );
    }

    /// Without a BOM it is still Latin-1 with an encoding byte in front, which is what the games'
    /// `.bar` files carry.
    #[test]
    fn without_bom_stays_latin1() {
        let mut section = vec![0x01];
        section.extend_from_slice(
            "Ação\0"
                .as_bytes()
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let bar = build(&[(RESTYPE_STRING, 7, 0, 0)], &[&section]);
        let res = ResFile::parse(bar).unwrap();
        let loaded = String::from_utf16_lossy(&res.string(7).unwrap());
        assert!(loaded.starts_with('A'), "{loaded:?}");
    }

    #[test]
    fn a_range_covers_consecutive_ids_in_consecutive_sections() {
        // `a = 2` means three resources: ids 100, 101 and 102 in sections 0, 1 and 2.
        let bar = build(&[(6, 100, 2, 0)], &[b"zero", b"um", b"dois"]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.get(6, 100), Some(&b"zero"[..]));
        assert_eq!(res.get(6, 101), Some(&b"um"[..]));
        assert_eq!(res.get(6, 102), Some(&b"dois"[..]));
        // Outside the range, nothing.
        assert_eq!(res.get(6, 103), None);
        // And the type is part of the key.
        assert_eq!(res.get(1, 100), None);
    }

    #[test]
    fn ranges_of_different_types_coexist() {
        let bar = build(
            &[(1, 4000, 0, 0), (0x5000, 3000, 1, 1)],
            &[b"\x03texto", b"bin0", b"bin1"],
        );
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.get(0x5000, 3001), Some(&b"bin1"[..]));
        assert_eq!(res.string(4000), Some("texto".encode_utf16().collect()));
    }

    #[test]
    fn the_string_drops_the_encoding_byte_and_stops_at_the_terminator() {
        let bar = build(&[(1, 7, 0, 0)], &[b"\x03NEW GAME\0lixo depois"]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.string(7), Some("NEW GAME".encode_utf16().collect()));
    }

    #[test]
    fn a_file_that_is_not_a_bar_is_rejected() {
        let err = ResFile::parse(b"nao sou um bar".to_vec());
        assert!(matches!(err, Err(ResError::BadMagic { .. })));
    }

    #[test]
    fn a_truncated_bar_does_not_overflow() {
        let mut bar = build(&[(6, 1, 0, 0)], &[b"dados"]);
        bar.truncate(bar.len() - 3);
        // Either it refuses on open, or it returns less than the table promises — what it must
        // not do is read outside the vector.
        if let Ok(res) = ResFile::parse(bar) {
            assert!(res.get(6, 1).is_none_or(|d| d.len() <= 5));
        }
    }
}
