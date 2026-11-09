//! The save state **format**: versioned, with named sections and an integrity check.
//!
//! It is kept apart from the state on purpose. The core's `retro_serialize_size` answers **zero**
//! until the whole state is inside it — zero is how the frontend understands "this core does not
//! save", and it is the only honest answer while some field is left out. What this module solves
//! first is the part the goal names: **a versioned format**.
//!
//! ## Why named sections
//!
//! A machine's state has a hundred and ninety fields, and many of them are tables of guest
//! objects. An opaque block of bytes would force saving and loading in the same order forever:
//! moving a field would turn into an incompatible save state **silently**. With names, each piece
//! finds itself, and a section the reader does not know is **skipped** — that is what allows
//! reading a state written by a newer version without guessing.
//!
//! ## Integrity
//!
//! The `crc32` of the content goes in the header. Without it, a file truncated by lack of disk
//! space would load as a valid state, and the game would crash somewhere unrelated to the
//! problem. With it, the rejection happens at the door.
//!
//! ## Layout
//!
//! ```text
//! 0..4    signature "ZBXS"
//! 4..6    format version, u16 (little-endian)
//! 6..10   content size, u32
//! 10..14  content crc32, u32
//! 14..    content: sections, one after the other
//! ```
//!
//! Each section is a `u8` with the name length, the name, a `u32` with the data length, and the
//! data.

use std::collections::BTreeMap;
use std::ops::Range;

/// File signature. Four bytes that say "this is a Zeebx save state".
pub const SIGNATURE: [u8; 4] = *b"ZBXS";

/// Format version. It goes up when the **meaning** of the sections changes, not when a new section
/// appears: an unknown section is skipped by the reader, and that is how an old state stays
/// readable after the engine gains a field.
pub const VERSION: u16 = 1;

/// The header size, in bytes.
const HEADER: usize = 14;

/// What went wrong when opening a state.
///
/// Each variant says **what** was wrong and **what** it ran into. "Invalid save state" on its own
/// forces the reader to investigate the file from scratch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file does not start with [`SIGNATURE`].
    Signature([u8; 4]),
    /// The file has a version this engine cannot read.
    Version { found: u16, supported: u16 },
    /// The file ends before what the header promises.
    Truncated { expected: usize, found: usize },
    /// The content does not match the header's `crc32`.
    Integrity { expected: u32, found: u32 },
    /// A section is malformed inside.
    Section { name: String, reason: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Signature(bytes) => write!(
                f,
                "not a Zeebx save state: the first four bytes are {bytes:02x?}, \
                 and should be {SIGNATURE:02x?}"
            ),
            Error::Version {
                found,
                supported,
            } => write!(
                f,
                "save state of version {found}, and this engine reads up to {supported}"
            ),
            Error::Truncated {
                expected,
                found,
            } => write!(
                f,
                "truncated save state: the header promises {expected} bytes and the file has {found}"
            ),
            Error::Integrity {
                expected,
                found,
            } => write!(
                f,
                "corrupted save state: crc32 {found:08x} where the header says {expected:08x}"
            ),
            Error::Section { name, reason } => {
                write!(f, "malformed section \"{name}\": {reason}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Writes the sections in the format. The order they go in is the order they come out in the file.
pub fn write(sections: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut content = Vec::new();
    for (name, data) in sections {
        let name = name.as_bytes();
        // The name fits in a byte: these are section names, not paths.
        let name_len = u8::try_from(name.len().min(u8::MAX as usize)).unwrap_or(u8::MAX);
        content.push(name_len);
        content.extend_from_slice(&name[..name_len as usize]);
        content.extend_from_slice(&(data.len() as u32).to_le_bytes());
        content.extend_from_slice(data);
    }

    let mut output = Vec::with_capacity(HEADER + content.len());
    output.extend_from_slice(&SIGNATURE);
    output.extend_from_slice(&VERSION.to_le_bytes());
    output.extend_from_slice(&(content.len() as u32).to_le_bytes());
    output.extend_from_slice(&crc32(&content).to_le_bytes());
    output.extend_from_slice(&content);
    output
}

/// The `crc32` the header stores.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(data);
    crc.sum()
}

/// An opened state, ready to have its sections read.
#[derive(Debug)]
pub struct Reader<'a> {
    data: &'a [u8],
    sections: BTreeMap<String, Range<usize>>,
}

impl<'a> Reader<'a> {
    /// Checks the header and indexes the sections.
    ///
    /// It does **not** interpret any section: whoever owns the state knows what each one means.
    pub fn open(file: &'a [u8]) -> Result<Self, Error> {
        if file.len() < HEADER {
            return Err(Error::Truncated {
                expected: HEADER,
                found: file.len(),
            });
        }
        let signature = [file[0], file[1], file[2], file[3]];
        if signature != SIGNATURE {
            return Err(Error::Signature(signature));
        }
        let version = u16::from_le_bytes([file[4], file[5]]);
        if version > VERSION {
            return Err(Error::Version {
                found: version,
                supported: VERSION,
            });
        }
        let promised = u32::from_le_bytes([file[6], file[7], file[8], file[9]]) as usize;
        let expected = u32::from_le_bytes([file[10], file[11], file[12], file[13]]);
        let available = file.len() - HEADER;
        if available < promised {
            return Err(Error::Truncated {
                expected: HEADER + promised,
                found: file.len(),
            });
        }
        let content = &file[HEADER..HEADER + promised];
        let found = crc32(content);
        if found != expected {
            return Err(Error::Integrity {
                expected,
                found,
            });
        }

        let mut sections = BTreeMap::new();
        let mut position = 0usize;
        while position < content.len() {
            let name_len = content[position] as usize;
            position += 1;
            if position + name_len > content.len() {
                return Err(Error::Section {
                    name: "?".to_string(),
                    reason: "the section header runs past the end of the file".to_string(),
                });
            }
            let name = String::from_utf8_lossy(&content[position..position + name_len])
                .into_owned();
            position += name_len;
            if position + 4 > content.len() {
                return Err(Error::Section {
                    name,
                    reason: "the data length is missing".to_string(),
                });
            }
            let size = u32::from_le_bytes([
                content[position],
                content[position + 1],
                content[position + 2],
                content[position + 3],
            ]) as usize;
            position += 4;
            if position + size > content.len() {
                return Err(Error::Section {
                    name,
                    reason: format!(
                        "claims {size} bytes and {} remain",
                        content.len() - position
                    ),
                });
            }
            sections.insert(name, position..position + size);
            position += size;
        }
        Ok(Self {
            data: content,
            sections,
        })
    }

    /// The data of a section, or `None` if the file does not have it.
    pub fn section(&self, name: &str) -> Option<&'a [u8]> {
        self.sections.get(name).map(|strip| &self.data[strip.clone()])
    }

    /// The section names, in order.
    pub fn names(&self) -> Vec<&str> {
        self.sections.keys().map(String::as_str).collect()
    }
}

/// A piece of state that knows how to save and restore itself.
///
/// Each engine subsystem implements this in its own file, where the fields are known. What stays
/// here is only the contract: saving returns bytes by name, restoring consumes them and **rejects**
/// what does not match — it never applies half.
pub trait Saveable {
    /// Writes the state into the sections.
    fn save(&self, dest: &mut Sections);

    /// Reads the state from the sections.
    fn restore(&mut self, origin: &Reader<'_>) -> Result<(), Error>;
}

/// Builds the content of a state, section by section.
#[derive(Debug, Default)]
pub struct Sections {
    pairs: Vec<(String, Vec<u8>)>,
}

impl Sections {
    pub fn create() -> Self {
        Self::default()
    }

    /// Stores raw bytes in a section.
    pub fn put(&mut self, name: &str, bytes: Vec<u8>) {
        self.pairs.push((name.to_string(), bytes));
    }

    /// Stores a number.
    pub fn put_u32(&mut self, name: &str, value: u32) {
        self.put(name, value.to_le_bytes().to_vec());
    }

    /// Stores a list of numbers, with the count in front.
    /// Writes a map, **in address order**, so the file does not depend on the `HashMap` order.
    pub fn put_map(&mut self, name: &str, map: impl IntoIterator<Item = (u32, u32)>) {
        let mut pairs: Vec<(u32, u32)> = map.into_iter().collect();
        pairs.sort_unstable();
        self.put_u32s(name, pairs.into_iter().flat_map(|(a, b)| [a, b]));
    }

    pub fn put_u32s(&mut self, name: &str, values: impl IntoIterator<Item = u32>) {
        let values: Vec<u32> = values.into_iter().collect();
        let mut bytes = Vec::with_capacity(4 + values.len() * 4);
        bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        self.put(name, bytes);
    }

    /// Writes a list of texts: the count and then each text with its length in front.
    pub fn put_texts(&mut self, name: &str, texts: impl IntoIterator<Item = String>) {
        let texts: Vec<String> = texts.into_iter().collect();
        let mut output = Vec::new();
        output.extend_from_slice(&(texts.len() as u32).to_le_bytes());
        for text in texts {
            let bytes = text.as_bytes();
            output.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            output.extend_from_slice(bytes);
        }
        self.put(name, output);
    }

    /// Writes byte blocks indexed by a key of `key_ids` numbers each.
    ///
    /// It is the shape of every table that stores **content** instead of a number: the preferences,
    /// the collection parameters, the data `IConfig` received. `key_ids` says how many numbers make
    /// up the key — one for `HashMap<u32, Vec<u8>>`, two for `HashMap<(u32, u32), Vec<u8>>`.
    pub fn put_blocks(
        &mut self,
        name: &str,
        key_ids: usize,
        items: impl IntoIterator<Item = (Vec<u32>, Vec<u8>)>,
    ) {
        let items: Vec<(Vec<u32>, Vec<u8>)> = items.into_iter().collect();
        let mut output = Vec::new();
        output.extend_from_slice(&(items.len() as u32).to_le_bytes());
        output.extend_from_slice(&(key_ids as u32).to_le_bytes());
        for (key_id, data) in items {
            debug_assert_eq!(key_id.len(), key_ids);
            for n in key_id {
                output.extend_from_slice(&n.to_le_bytes());
            }
            output.extend_from_slice(&(data.len() as u32).to_le_bytes());
            output.extend_from_slice(&data);
        }
        self.put(name, output);
    }

    /// Writes a UTF-8 text, with its length in front.
    ///
    /// File path, font name, title: whatever is not a number goes this way. The length comes first
    /// because a separator would have to be a byte the text cannot contain — and in a file path no
    /// such byte exists.
    pub fn put_text(&mut self, name: &str, text: &str) {
        let bytes = text.as_bytes();
        let mut output = Vec::with_capacity(4 + bytes.len());
        output.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(bytes);
        self.put(name, output);
    }

    /// Writes **fixed-size records**: each one is the object's index followed by its fields.
    ///
    /// It is the shape of most engine tables — `HashMap<u32, State>` where the state is a handful
    /// of numbers. With a single helper, each new table becomes five lines of writing and five of
    /// reading, and not a format of its own that someone has to understand all over again.
    pub fn put_records(
        &mut self,
        name: &str,
        records: impl IntoIterator<Item = Vec<u32>>,
    ) {
        let records: Vec<u32> = records.into_iter().flatten().collect();
        self.put_u32s(name, records);
    }

    /// Writes triples of numbers, with the count in front.
    pub fn put_triples(&mut self, name: &str, values: impl IntoIterator<Item = (u32, u32, u32)>) {
        let values: Vec<u32> = values
            .into_iter()
            .flat_map(|(a, b, c)| [a, b, c])
            .collect();
        self.put_u32s(name, values);
    }

    /// Replaces the content of a section, or appends it if it does not exist.
    ///
    /// It exists for the rejection tests: the honest way to prove that a corrupted state is
    /// rejected is to build a **valid** state and break one of its fields. Rebuilding the sections
    /// by hand would make the test depend on all the others being right, and it would end up
    /// measuring something else.
    pub fn swap(&mut self, name: &str, bytes: Vec<u8>) {
        match self.pairs.iter_mut().find(|(n, _)| n == name) {
            Some((_, data)) => *data = bytes,
            None => self.put(name, bytes),
        }
    }

    /// How many sections have been added so far.
    pub fn count(&self) -> usize {
        self.pairs.len()
    }

    /// Closes the state, in the [`write`] format.
    pub fn close(self) -> Vec<u8> {
        write(&self.pairs)
    }
}

impl Reader<'_> {
    /// A number from a section, with an error that says **which** section and what was missing.
    ///
    /// A missing section is an error, not zero: a field that silently goes back to zero is the
    /// difference between "the game resumes oddly" and "the load was rejected".
    pub fn u32(&self, name: &str) -> Result<u32, Error> {
        let bytes = self.section(name).ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected 4 bytes and has {}", bytes.len()),
            });
        }
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A list of texts written by [`Sections::put_texts`].
    pub fn texts(&self, name: &str) -> Result<Vec<String>, Error> {
        let bytes = self.section(name).ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        let malformed = |reason: String| Error::Section {
            name: name.to_string(),
            reason,
        };
        if bytes.len() < 4 {
            return Err(malformed(format!(
                "expected the count and has {} bytes",
                bytes.len()
            )));
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        // **Check before reserving.** The number comes from the file, and a forged state with a
        // huge `count` made `with_capacity` ask for tens of gigabytes — the process died over a
        // number, before any check on the content. Each text takes at least the four bytes of its
        // own length, so the ceiling is what fits in the section.
        if count > bytes.len() / 4 {
            return Err(malformed(format!(
                "claims {count} texts, and the section has {} bytes",
                bytes.len()
            )));
        }
        let mut position = 4usize;
        let mut texts = Vec::with_capacity(count);
        for _ in 0..count {
            if bytes.len() < position + 4 {
                return Err(malformed(format!("the text length at {position} is missing")));
            }
            let size = u32::from_le_bytes([
                bytes[position],
                bytes[position + 1],
                bytes[position + 2],
                bytes[position + 3],
            ]) as usize;
            position += 4;
            if bytes.len() < position + size {
                return Err(malformed(format!(
                    "the text at {position} claims {size} bytes and {} remain",
                    bytes.len() - position
                )));
            }
            texts.push(
                String::from_utf8(bytes[position..position + size].to_vec()).map_err(|error| {
                    malformed(format!("one of the texts is not UTF-8: {error}"))
                })?,
            );
            position += size;
        }
        Ok(texts)
    }

    /// Blocks written by [`Sections::put_blocks`], with the key count the file states.
    ///
    /// The number of keys lives **in the file**, not only in the code: that way a two-key table
    /// read as if it were a single-key one is a rejection, and not a shifted read.
    pub fn blocks(&self, name: &str) -> Result<Vec<(Vec<u32>, Vec<u8>)>, Error> {
        let bytes = self.section(name).ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        let malformed = |reason: String| Error::Section {
            name: name.to_string(),
            reason,
        };
        if bytes.len() < 8 {
            return Err(malformed(format!(
                "expected the count and the key width, and has {} bytes",
                bytes.len()
            )));
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let key_ids = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        if key_ids == 0 || key_ids > 8 {
            return Err(malformed(format!("key width {key_ids}")));
        }
        // Same check as in `texts`: each item needs `key_ids` values plus the length.
        if count > bytes.len() / (key_ids * 4 + 4) {
            return Err(malformed(format!(
                "claims {count} items of {key_ids} key(s), and the section has {} bytes",
                bytes.len()
            )));
        }
        let mut position = 8usize;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            let needed = position + key_ids * 4 + 4;
            if bytes.len() < needed {
                return Err(malformed(format!(
                    "the item at {position} runs past the end ({} bytes)",
                    bytes.len()
                )));
            }
            let mut key_id = Vec::with_capacity(key_ids);
            for _ in 0..key_ids {
                key_id.push(u32::from_le_bytes([
                    bytes[position],
                    bytes[position + 1],
                    bytes[position + 2],
                    bytes[position + 3],
                ]));
                position += 4;
            }
            let size = u32::from_le_bytes([
                bytes[position],
                bytes[position + 1],
                bytes[position + 2],
                bytes[position + 3],
            ]) as usize;
            position += 4;
            if bytes.len() < position + size {
                return Err(malformed(format!(
                    "the item {key_id:?} claims {size} bytes and {} remain",
                    bytes.len() - position
                )));
            }
            items.push((key_id, bytes[position..position + size].to_vec()));
            position += size;
        }
        Ok(items)
    }

    /// A text written by [`Sections::put_text`].
    ///
    /// A length beyond what the file has is a rejection: silently cutting it would return half a
    /// path, and the file would be opened in the wrong place.
    pub fn text(&self, name: &str) -> Result<String, Error> {
        let bytes = self.section(name).ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected the length and has {} bytes", bytes.len()),
            });
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if bytes.len() < 4 + count {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!(
                    "claims {count} bytes of text and {} remain",
                    bytes.len() - 4
                ),
            });
        }
        String::from_utf8(bytes[4..4 + count].to_vec()).map_err(|error| Error::Section {
            name: name.to_string(),
            reason: format!("the text is not UTF-8: {error}"),
        })
    }

    /// Records written by [`Sections::put_records`], with the number of fields in each.
    ///
    /// The number of fields is **declared by the reader**, and a nonzero remainder is a rejection: a
    /// four-field record read as if it had three would shift everything after it, and the defect
    /// would show up as another game's state inside the wrong object.
    pub fn records(&self, name: &str, fields: usize) -> Result<Vec<Vec<u32>>, Error> {
        assert!(fields > 0, "a record with no fields is not a record");
        let values = self.u32s(name)?;
        if values.len() % fields != 0 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!(
                    "expected a multiple of {fields} values and got {}",
                    values.len()
                ),
            });
        }
        Ok(values.chunks_exact(fields).map(|r| r.to_vec()).collect())
    }

    /// Triples written by [`Sections::put_triples`].
    pub fn triples(&self, name: &str) -> Result<Vec<(u32, u32, u32)>, Error> {
        let values = self.u32s(name)?;
        if values.len() % 3 != 0 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected triples and got {} value(s)", values.len()),
            });
        }
        Ok(values
            .chunks_exact(3)
            .map(|t| (t[0], t[1], t[2]))
            .collect())
    }

    /// Pairs **in the order they were written**.
    ///
    /// It is what a queue needs: the order of the events is the content. It exists separately from
    /// [`Reader::pairs`], which sorts — sorting is right for a map, where the `HashMap` order is not
    /// stable, and **destroys** a queue. A save state that shuffles the key queue delivers the keys
    /// in the wrong order, and the game responds to a sequence nobody pressed.
    pub fn ordered_pairs(&self, name: &str) -> Result<Vec<(u32, u32)>, Error> {
        let values = self.u32s(name)?;
        if values.len() % 2 != 0 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected pairs and got an odd count ({})", values.len()),
            });
        }
        Ok(values.chunks_exact(2).map(|p| (p[0], p[1])).collect())
    }

    /// Pairs from a list written by [`Sections::put_u32s`], for the maps.
    ///
    /// **Sorts** the pairs: the order of a `HashMap` is not stable between runs, and a state that
    /// comes out different on every save is useless for comparing two runs.
    pub fn pairs(&self, name: &str) -> Result<Vec<(u32, u32)>, Error> {
        let values = self.u32s(name)?;
        if values.len() % 2 != 0 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected pairs and got an odd count ({})", values.len()),
            });
        }
        let mut pairs: Vec<(u32, u32)> = values.chunks_exact(2).map(|p| (p[0], p[1])).collect();
        pairs.sort_unstable();
        Ok(pairs)
    }

    /// A list of numbers written by [`Sections::put_u32s`].
    pub fn u32s(&self, name: &str) -> Result<Vec<u32>, Error> {
        let bytes = self.section(name).ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        if bytes.len() < 4 {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!("expected the count and has {} bytes", bytes.len()),
            });
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        let expected = 4 + count * 4;
        if bytes.len() < expected {
            return Err(Error::Section {
                name: name.to_string(),
                reason: format!(
                    "claims {count} values ({expected} bytes) and has {}",
                    bytes.len()
                ),
            });
        }
        Ok((0..count)
            .map(|i| {
                let p = 4 + i * 4;
                u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]])
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections() -> Vec<(String, Vec<u8>)> {
        vec![
            ("cpu".to_string(), vec![1, 2, 3, 4]),
            ("heap".to_string(), vec![9; 8]),
        ]
    }

    /// A fake piece of state, to exercise the `Saveable` contract without dragging in the engine.
    #[derive(Debug, PartialEq, Eq)]
    struct Counter {
        next: u32,
        free_slots: Vec<u32>,
    }

    impl Saveable for Counter {
        fn save(&self, dest: &mut Sections) {
            dest.put_u32("contador.proximo", self.next);
            dest.put_u32s("contador.livres", self.free_slots.iter().copied());
        }

        fn restore(&mut self, origin: &Reader<'_>) -> Result<(), Error> {
            self.next = origin.u32("contador.proximo")?;
            self.free_slots = origin.u32s("contador.livres")?;
            Ok(())
        }
    }

    #[test]
    fn a_saveable_round_trips() {
        let before = Counter {
            next: 0x1020_3040,
            free_slots: vec![7, 0xabc, 0],
        };
        let mut sections = Sections::create();
        before.save(&mut sections);
        assert_eq!(sections.count(), 2);
        let file = sections.close();

        let reader = Reader::open(&file).expect("the state opened");
        let mut after = Counter {
            next: 0,
            free_slots: Vec::new(),
        };
        after.restore(&reader).expect("restored");
        assert_eq!(after, before);
    }

    #[test]
    fn a_missing_section_is_an_error_not_zero() {
        let mut sections = Sections::create();
        sections.put_u32("so-isto", 1);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");
        match reader.u32("nao-existe") {
            Err(Error::Section { name, reason }) => {
                assert_eq!(name, "nao-existe");
                assert!(reason.contains("is not"), "{reason}");
            }
            other => panic!("should say the section is missing, and returned {other:?}"),
        }
    }

    /// **Sorting is right for a map; for a queue, it destroys it.** That is why there are two
    /// readers, and this test is the difference between them written down: a save state that
    /// shuffles the key queue delivers the keys in the wrong order, and the game responds to a
    /// sequence nobody pressed.
    #[test]
    fn the_queue_is_not_sorted_but_the_map_is() {
        let mut sections = Sections::create();
        // A key queue out of ascending order, on purpose.
        sections.put_u32s("fila", [30u32, 1, 20, 0]);
        sections.put_map("mapa", [(30u32, 1u32), (7, 2), (20, 3)]);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        assert_eq!(
            reader.ordered_pairs("fila"),
            Ok(vec![(30, 1), (20, 0)]),
            "the queue must come out in the order it was written"
        );
        assert_eq!(
            reader.pairs("mapa"),
            Ok(vec![(7, 2), (20, 3), (30, 1)]),
            "the map comes out sorted, so it does not depend on the HashMap order"
        );
    }

    #[test]
    fn a_number_list_with_a_lying_count_is_rejected() {
        let mut sections = Sections::create();
        // The count says 5 values and only two come: the reader must reject, not read garbage.
        let mut bytes = 5u32.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
        sections.put("mentirosa", bytes);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");
        assert!(matches!(reader.u32s("mentirosa"), Err(Error::Section { .. })));
    }

    #[test]
    fn a_round_trip_returns_the_sections() {
        let file = write(&sections());
        let reader = Reader::open(&file).expect("the state opened");
        assert_eq!(reader.names(), vec!["cpu", "heap"]);
        assert_eq!(reader.section("cpu"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(reader.section("heap"), Some(&[9u8; 8][..]));
        assert_eq!(reader.section("inexistente"), None);
    }

    #[test]
    fn an_unknown_section_is_skipped_without_breaking() {
        let mut all = sections();
        all.push(("secao-do-futuro".to_string(), vec![7; 5]));
        let file = write(&all);
        let reader = Reader::open(&file).expect("the state opened");
        // The known ones stay readable: this is what lets an old engine read a new state.
        assert_eq!(reader.section("cpu"), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(reader.section("secao-do-futuro"), Some(&[7u8; 5][..]));
    }

    #[test]
    fn a_flipped_byte_in_the_content_is_rejected() {
        let mut file = write(&sections());
        let last = file.len() - 1;
        file[last] ^= 0xff;
        match Reader::open(&file) {
            Err(Error::Integrity { .. }) => {}
            other => panic!("should reject on integrity, and returned {other:?}"),
        }
    }

    #[test]
    fn a_future_version_is_rejected_saying_which() {
        let mut file = write(&sections());
        file[4..6].copy_from_slice(&(VERSION + 3).to_le_bytes());
        match Reader::open(&file) {
            Err(Error::Version {
                found,
                supported,
            }) => {
                assert_eq!(found, VERSION + 3);
                assert_eq!(supported, VERSION);
            }
            other => panic!("should reject on the version, and returned {other:?}"),
        }
    }

    #[test]
    fn a_file_of_something_else_is_rejected() {
        match Reader::open(b"NOPE nao sou um save state") {
            Err(Error::Signature(bytes)) => assert_eq!(&bytes, b"NOPE"),
            other => panic!("should reject on the signature, and returned {other:?}"),
        }
    }

    #[test]
    fn a_truncated_state_is_rejected_with_both_numbers() {
        let file = write(&sections());
        let truncated = &file[..file.len() - 3];
        match Reader::open(truncated) {
            Err(Error::Truncated {
                expected,
                found,
            }) => {
                assert_eq!(expected, file.len());
                assert_eq!(found, truncated.len());
            }
            other => panic!("should reject as truncated, and returned {other:?}"),
        }
    }

    #[test]
    fn an_empty_file_is_rejected_without_panicking() {
        assert!(matches!(
            Reader::open(&[]),
            Err(Error::Truncated {
                expected: HEADER,
                found: 0
            })
        ));
    }

    /// The error message says what happened and what it ran into. Without this, whoever gets
    /// "invalid save state" starts the investigation from scratch.
    #[test]
    fn the_messages_say_what_happened() {
        let text = Error::Version {
            found: 7,
            supported: VERSION,
        }
        .to_string();
        assert!(text.contains('7'), "{text}");
        assert!(text.contains(&VERSION.to_string()), "{text}");
        let text = Error::Integrity {
            expected: 0xdead_beef,
            found: 0x1234_5678,
        }
        .to_string();
        assert!(text.contains("deadbeef"), "{text}");
        assert!(text.contains("12345678"), "{text}");
    }
}
