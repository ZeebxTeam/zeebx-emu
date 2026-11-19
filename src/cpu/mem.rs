//! Guest memory.
//!
//! The map is built from named regions instead of a flat 4 GiB block: that way an access outside
//! the map becomes an identifiable error (and that is exactly what the vtable trampoline will use
//! to detect calls to BREW APIs).

// Removed as soon as the core is wired into the main loop.
#![allow(dead_code)]

use std::fmt;

/// A contiguous range of guest memory.
#[derive(Debug)]
pub struct Region {
    /// Name used in logs and error messages.
    pub name: &'static str,
    /// Start address in the guest's address space.
    pub base: u32,
    pub bytes: Vec<u8>,
    pub writable: bool,
    /// Whether the processor may fetch instructions here. Only the null page may not: reading
    /// from it is something the console tolerates, jumping to it is an error we want to see.
    pub executable: bool,
}

impl Region {
    fn contains(&self, addr: u32, len: u32) -> bool {
        let end = match self.base.checked_add(self.bytes.len() as u32) {
            Some(end) => end,
            None => return false,
        };
        addr >= self.base && addr.checked_add(len).is_some_and(|a| a <= end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    /// No region covers the requested address.
    Unmapped { addr: u32, len: u32 },
    /// The region exists but is read-only.
    ReadOnly { addr: u32, region: &'static str },
    /// Two regions overlap — an error in building the map, not in the guest.
    Overlap { name: &'static str, base: u32 },
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped { addr, len } => {
                write!(f, "access to {addr:#010x} (+{len}) outside the memory map")
            }
            Self::ReadOnly { addr, region } => {
                write!(
                    f,
                    "write to {addr:#010x}, region '{region}' is read-only"
                )
            }
            Self::Overlap { name, base } => {
                write!(
                    f,
                    "region '{name}' at {base:#010x} overlaps one already mapped"
                )
            }
        }
    }
}

impl std::error::Error for MemError {}

/// The bytes of a region, seen from the guest address.
///
/// The pointer lives as long as the region exists. Mapping another region does not move this
/// one's bytes: each has its own `Vec`. The interpreter keeps this so it does not have to look up
/// the region on every instruction. `ptr` comes from `as_mut_ptr`: writing through a `*mut` that
/// came from `as_ptr` is undefined.
#[derive(Clone, Copy)]
pub struct Window {
    pub base: u32,
    pub ptr: *mut u8,
    pub len: u32,
    pub writable: bool,
    pub executable: bool,
}

#[derive(Debug, Default)]
pub struct GuestMemory {
    regions: Vec<Region>,
}

impl GuestMemory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps a new region. Fails if it collides with one that already exists.
    pub fn map(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
    ) -> Result<(), MemError> {
        self.map_executable(name, base, bytes, writable, true)
    }

    /// Like [`GuestMemory::map`], also saying whether the region can be executed.
    pub fn map_executable(
        &mut self,
        name: &'static str,
        base: u32,
        bytes: Vec<u8>,
        writable: bool,
        executable: bool,
    ) -> Result<(), MemError> {
        let end = base as u64 + bytes.len() as u64;
        let collides = self.regions.iter().any(|r| {
            let r_end = r.base as u64 + r.bytes.len() as u64;
            (base as u64) < r_end && (r.base as u64) < end
        });
        if collides {
            return Err(MemError::Overlap { name, base });
        }
        self.regions.push(Region {
            name,
            base,
            bytes,
            writable,
            executable,
        });
        Ok(())
    }

    /// Maps a zeroed, writable region — stack, heap, framebuffer.
    pub fn map_zeroed(
        &mut self,
        name: &'static str,
        base: u32,
        len: usize,
    ) -> Result<(), MemError> {
        self.map(name, base, vec![0; len], true)
    }

    /// Whether there is code that can be fetched at `addr`.
    pub fn executable(&self, addr: u32) -> bool {
        self.region_for(addr, 4).is_some_and(|r| r.executable)
    }

    fn region_for(&self, addr: u32, len: u32) -> Option<&Region> {
        self.regions.iter().find(|r| r.contains(addr, len))
    }

    fn region_for_mut(&mut self, addr: u32, len: u32) -> Option<&mut Region> {
        self.regions.iter_mut().find(|r| r.contains(addr, len))
    }

    /// The region that contains `addr`, with the pointer to its bytes already taken.
    ///
    /// The borrow is mutable even for a read: the interpreter keeps the pointer and the next
    /// write uses the same shortcut. `as_mut_ptr` gives write provenance.
    pub fn window_mut(&mut self, addr: u32) -> Option<Window> {
        let region = self.region_for_mut(addr, 1)?;
        let len = u32::try_from(region.bytes.len()).ok()?;
        let _end = region.base.checked_add(len)?;
        Some(Window {
            base: region.base,
            ptr: region.bytes.as_mut_ptr(),
            len,
            writable: region.writable,
            executable: region.executable,
        })
    }

    pub fn read(&self, addr: u32, len: u32) -> Result<&[u8], MemError> {
        let region = self
            .region_for(addr, len)
            .ok_or(MemError::Unmapped { addr, len })?;
        let start = (addr - region.base) as usize;
        Ok(&region.bytes[start..start + len as usize])
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemError> {
        let len = data.len() as u32;
        let region = self
            .region_for_mut(addr, len)
            .ok_or(MemError::Unmapped { addr, len })?;
        if !region.writable {
            return Err(MemError::ReadOnly {
                addr,
                region: region.name,
            });
        }
        let start = (addr - region.base) as usize;
        region.bytes[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }

    /// Fills the regions directly, without creating an intermediate buffer.
    ///
    /// Validates the whole range before touching the bytes. That way a failure in the second
    /// region does not leave the first one modified without the caller invalidating the matching
    /// JIT cache.
    pub fn fill(&mut self, addr: u32, value: u8, len: u32) -> Result<(), MemError> {
        let start = u64::from(addr);
        let end = start + u64::from(len);
        if end > u64::from(u32::MAX) + 1 {
            return Err(MemError::Unmapped { addr, len });
        }

        let mut cursor = start;
        while cursor < end {
            let location = cursor as u32;
            let remaining = end - cursor;
            let region = self.region_for(location, 1).ok_or(MemError::Unmapped {
                addr: location,
                len: remaining.min(u64::from(u32::MAX)) as u32,
            })?;
            if !region.writable {
                return Err(MemError::ReadOnly {
                    addr: location,
                    region: region.name,
                });
            }
            let region_end = u64::from(region.base) + region.bytes.len() as u64;
            cursor = end.min(region_end);
        }

        cursor = start;
        while cursor < end {
            let location = cursor as u32;
            let region = self.region_for_mut(location, 1).expect("range already validated");
            let offset_in_region = (location - region.base) as usize;
            let step = ((end - cursor) as usize).min(region.bytes.len() - offset_in_region);
            region.bytes[offset_in_region..offset_in_region + step].fill(value);
            cursor += step as u64;
        }
        Ok(())
    }

    pub fn read_u32(&self, addr: u32) -> Result<u32, MemError> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), MemError> {
        self.write(addr, &value.to_le_bytes())
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn regions_mut(&mut self) -> &mut [Region] {
        &mut self.regions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> GuestMemory {
        let mut m = GuestMemory::new();
        m.map("rom", 0x0000_0000, vec![0xaa; 0x100], false).unwrap();
        m.map_zeroed("ram", 0x1000_0000, 0x100).unwrap();
        m
    }

    #[test]
    fn reads_and_writes_inside_regions() {
        let mut m = mem();
        assert_eq!(m.read(0, 2).unwrap(), &[0xaa, 0xaa]);
        m.write_u32(0x1000_0010, 0xdead_beef).unwrap();
        assert_eq!(m.read_u32(0x1000_0010).unwrap(), 0xdead_beef);
    }

    #[test]
    fn an_unmapped_access_fails() {
        let m = mem();
        assert_eq!(
            m.read(0x2000_0000, 4),
            Err(MemError::Unmapped {
                addr: 0x2000_0000,
                len: 4
            })
        );
    }

    #[test]
    fn an_access_crossing_the_region_end_fails() {
        let m = mem();
        assert!(m.read(0xfe, 4).is_err());
    }

    #[test]
    fn a_write_to_a_read_only_region_fails() {
        let mut m = mem();
        assert_eq!(
            m.write_u32(0, 1),
            Err(MemError::ReadOnly {
                addr: 0,
                region: "rom"
            })
        );
    }

    #[test]
    fn fill_writes_ram_without_an_intermediate_buffer() {
        let mut m = mem();
        m.fill(0x1000_0010, 0x5a, 0x20).unwrap();
        assert_eq!(m.read(0x1000_0010, 0x20).unwrap(), &[0x5a; 0x20]);
    }

    #[test]
    fn fill_respects_read_only_regions() {
        let mut m = mem();
        assert_eq!(
            m.fill(0, 0, 4),
            Err(MemError::ReadOnly {
                addr: 0,
                region: "rom"
            })
        );
    }

    #[test]
    fn a_fill_failing_in_the_second_region_leaves_the_first_untouched() {
        let mut m = GuestMemory::new();
        m.map("w", 0x2000, vec![0xaa; 4], true).unwrap();
        m.map("ro", 0x2004, vec![0xbb; 4], false).unwrap();
        assert_eq!(
            m.fill(0x2002, 0, 4),
            Err(MemError::ReadOnly {
                addr: 0x2004,
                region: "ro"
            })
        );
        assert_eq!(m.read(0x2000, 4).unwrap(), &[0xaa; 4]);
    }

    #[test]
    fn the_second_region_on_the_same_page_stays_readable() {
        let mut m = GuestMemory::new();
        m.map("a", 0x100, vec![0x11; 16], true).unwrap();
        m.map("b", 0x110, vec![0x22; 16], true).unwrap();
        assert_eq!(m.read(0x100, 4).unwrap(), &[0x11; 4]);
        assert_eq!(m.read(0x110, 4).unwrap(), &[0x22; 4]);
        assert!(m.read(0x10c, 8).is_err(), "the access does not cross regions");
    }

    #[test]
    fn the_rest_of_a_partial_page_is_not_part_of_the_region() {
        let mut m = GuestMemory::new();
        m.map("curta", 0, vec![0xab; 100], true).unwrap();
        assert!(m.read(98, 4).is_err());
        assert_eq!(m.read(96, 4).unwrap(), &[0xab; 4]);
    }
}
