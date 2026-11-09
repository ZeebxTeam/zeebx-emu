//! The guest heap allocator.
//!
//! It serves BREW's `MALLOC`/`FREE`. All the bookkeeping stays on the host — no header is written
//! into guest memory — because the game never inspects the heap's internal structure. It does,
//! however, read past the end of what it asked for: that is why `MALLOC` asks here for one extra
//! byte and fills it (`BLOCK_PADDING`, in `machine/thread.rs`).

use std::collections::{BTreeMap, HashMap};

/// Block alignment. ARM requires 4; we use 8 so as not to penalize 64-bit accesses.
const ALIGN: u32 = 8;

#[derive(Debug)]
pub struct Heap {
    base: u32,
    end: u32,
    /// First address never used.
    next: u32,
    /// Blocks returned by [`Heap::free`], from address to size.
    ///
    /// It is an ordered map, not a list, because what matters here is finding a block's neighbor
    /// to merge the two — see [`Heap::free`].
    free_list: BTreeMap<u32, u32>,
    /// Blocks in use, so `free` knows the size without the caller providing it.
    live: HashMap<u32, u32>,
}

impl Heap {
    pub fn new(base: u32, size: usize) -> Self {
        Self {
            base,
            end: base + size as u32,
            next: base,
            free_list: BTreeMap::new(),
            live: HashMap::new(),
        }
    }

    /// Reserves `size` bytes and returns the address, or `None` if the heap is full.
    ///
    /// A `size` of zero still gets its own address: BREW returns a valid pointer in that case,
    /// and returning the same address twice would confuse `free`.
    /// The first address never used.
    ///
    /// It is what bounds what is worth saving in a save state: below it is everything the game
    /// asked for, and above it is zero since the region was built.
    pub fn next(&self) -> u32 {
        self.next
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        let total = size.max(1).div_ceil(ALIGN) * ALIGN;

        // First fit among the free blocks.
        if let Some((&addr, &len)) = self.free_list.iter().find(|&(_, &len)| len >= total) {
            self.free_list.remove(&addr);
            // The rest of the block goes back to the list instead of going to waste.
            if len > total {
                self.free_list.insert(addr + total, len - total);
            }
            self.live.insert(addr, total);
            return Some(addr);
        }

        let addr = self.next;
        if addr.checked_add(total)? > self.end {
            return None;
        }
        self.next = addr + total;
        self.live.insert(addr, total);
        Some(addr)
    }

    /// Size of a live block — used by `realloc` to know how much to copy.
    pub fn size_of(&self, ptr: u32) -> Option<u32> {
        self.live.get(&ptr).copied()
    }

    /// The live block that contains `addr`, as `(start, size)`.
    ///
    /// It walks every block: it is meant for callers that keep the answer, like the search for the
    /// stack the game allocated for itself, and not for every access.
    pub fn block_containing(&self, addr: u32) -> Option<(u32, u32)> {
        self.live
            .iter()
            .find(|&(&start, &size)| (start..start + size).contains(&addr))
            .map(|(&start, &size)| (start, size))
    }

    /// Gives a block back. Returns the size freed, or `None` if the pointer was not live —
    /// including the null pointer, which `FREE` accepts without doing anything.
    ///
    /// **The block is merged with its free neighbors.** Without that the heap falls to pieces:
    /// every screen change in Treino Cerebral returns hundreds of small blocks, none of them forms
    /// a large region again, and the one-megabyte request that comes in the next stage finds
    /// nowhere to fit even with tens of megabytes free — the game does not check for the null
    /// pointer and dies calling a method at zero.
    pub fn free(&mut self, ptr: u32) -> Option<u32> {
        let size = self.live.remove(&ptr)?;
        let (mut start, mut end) = (ptr, ptr + size);
        // The neighbor below, if it ends exactly where this one starts.
        if let Some((&before, &len)) = self.free_list.range(..start).next_back()
            && before + len == start
        {
            self.free_list.remove(&before);
            start = before;
        }
        // And the one above, if it starts exactly where this one ends.
        if let Some(&len) = self.free_list.get(&end) {
            self.free_list.remove(&end);
            end += len;
        }
        // A block that touches the top needs no list: `next` moves back and it becomes fresh
        // space again, of whatever size is asked for later.
        if end == self.next {
            self.next = start;
        } else {
            self.free_list.insert(start, end - start);
        }
        Some(size)
    }

    /// How much of the heap is handed out to the game, in bytes.
    ///
    /// It is not `next - base`: what is in the free list has already come back, and counting it
    /// would make `GetRAMFree` lie to whoever asks before allocating.
    pub fn used(&self) -> u32 {
        let free_slots: u32 = self.free_list.values().sum();
        (self.next - self.base).saturating_sub(free_slots)
    }

    /// **The largest block a single allocation can get**, and not the sum of what is left.
    ///
    /// The difference is not academic: `MALLOC` asks for a **contiguous** block, and is served by
    /// two paths — a hole in the free list of the requested size, or the bump pointer. Adding the
    /// holes to what remains ahead gives a number no allocation can get.
    ///
    /// Measured on 2026-09-22 with the Z-Wheel: it asks how much is free, asks for **that** in a
    /// single allocation (its own `Free(67001392)` becomes a `malloc` of 67001488), and the lying
    /// sum made the request be refused — `check_malloc: Malloc failed in Tectoy.c at line 570` —
    /// with the heap far from full. The game is what decides whether the device can cope, and it
    /// decides by the number we report.
    pub fn largest_block(&self) -> u32 {
        let ahead = self.end.saturating_sub(self.next);
        let hole = self.free_list.values().copied().max().unwrap_or(0);
        ahead.max(hole)
    }

    /// **How the heap is split up**, and not just how much is left.
    ///
    /// [`Heap::largest_block`] answers what an allocation can get; this snapshot answers *why* it
    /// gets what it gets. The gap between the total free and the largest hole is the measure of
    /// fragmentation: with merging working it stays small even after the game spends its whole
    /// life allocating and freeing blocks of different sizes. It is the number that tells "the
    /// heap filled up" from "the heap fell to pieces" — the two symptoms the game reports as being
    /// out of memory.
    pub fn snapshot(&self) -> Snapshot {
        let in_list: u32 = self.free_list.values().sum();
        Snapshot {
            holes: self.free_list.len(),
            largest_hole: self.largest_block(),
            free: in_list + self.end.saturating_sub(self.next),
            used: self.used(),
            live: self.live.len(),
            cap: self.end - self.base,
        }
    }
}

/// The state of guest memory at one instant: what fragmentation needs in order to be a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    /// How many holes the free list has.
    pub holes: usize,
    /// The largest block a single allocation gets.
    pub largest_hole: u32,
    /// How much is free in total, adding up the holes and what remains ahead.
    pub free: u32,
    /// How much is handed out to the game.
    pub used: u32,
    /// How many live blocks.
    pub live: usize,
    /// The size of the region.
    pub cap: u32,
}

impl Snapshot {
    /// The part of the total free space that is **not** usable by a single allocation, in bytes.
    ///
    /// Zero when there is a hole the size of everything that is free. The larger it is, the more
    /// fragmented the heap.
    pub fn lost_in_holes(&self) -> u32 {
        self.free.saturating_sub(self.largest_hole)
    }
}


impl Heap {
    /// Saves under a section prefix.
    ///
    /// The prefix exists because the machine has **two** heaps: the game's memory and the
    /// surfaces region. With a fixed name, the second would overwrite the first in the file — and
    /// the defect would show up as "the surfaces came back in place of the heap".
    pub fn save_with_prefix(&self, prefix: &str, dest: &mut crate::save_state::Sections) {
        dest.put_u32(&format!("{prefix}.base"), self.base);
        dest.put_u32(&format!("{prefix}.end"), self.end);
        dest.put_u32(&format!("{prefix}.next"), self.next);
        dest.put_map(
            &format!("{prefix}.free_list"),
            self.free_list.iter().map(|(a, t)| (*a, *t)),
        );
        dest.put_map(&format!("{prefix}.live"), self.live.iter().map(|(a, t)| (*a, *t)));
    }

    /// Restores from a prefix, checking that the heap belongs to this machine and that the
    /// contents are consistent.
    pub fn restore_with_prefix(
        &mut self,
        prefix: &str,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error> {
        use crate::save_state::Error;
        let base = origin.u32(&format!("{prefix}.base"))?;
        let end = origin.u32(&format!("{prefix}.end"))?;
        if base != self.base || end != self.end {
            return Err(Error::Section { name: prefix.to_string(), reason: format!(
                "the state is from a region {base:#010x}..{end:#010x} and this machine has {:#010x}..{:#010x}",
                self.base, self.end) });
        }
        let next = origin.u32(&format!("{prefix}.next"))?;
        let free_slots = origin.pairs(&format!("{prefix}.free_list"))?;
        let live = origin.pairs(&format!("{prefix}.live"))?;
        if let Some((address, _)) = live.iter().find(|(a, _)| free_slots.iter().any(|(f, _)| f == a)) {
            return Err(Error::Section {
                name: prefix.to_string(),
                reason: format!("address {address:#010x} appears both free and in use"),
            });
        }
        self.next = next;
        self.free_list = free_slots.into_iter().collect();
        self.live = live.into_iter().collect();
        Ok(())
    }
}

impl crate::save_state::Saveable for Heap {
    /// Saves the five fields. `base` and `end` do not change after construction, and go along on
    /// purpose: on the way back they serve as a **check** that the state is from this machine.
    fn save(&self, dest: &mut crate::save_state::Sections) {
        self.save_with_prefix("heap", dest);
    }

    /// Restores, and **refuses** a state that is not from this machine or is inconsistent.
    ///
    /// The two checks are not decoration: a heap restored with a different `base` would make every
    /// guest pointer point to the wrong place, and the symptom would show up much later, as an
    /// invalid access at an address unrelated to the save state.
    fn restore(&mut self, origin: &crate::save_state::Reader<'_>) -> Result<(), crate::save_state::Error> {
        self.restore_with_prefix("heap", origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The block that contains an address is found from its middle, and disappears when it is
    /// given back.**
    ///
    /// It is what finds the stack Iron Sight allocated for itself, from `sp`: a returned block
    /// that could still be found would make the emulator dirty memory that already belongs to
    /// someone else.
    #[test]
    fn the_block_containing_an_address_is_found_from_its_middle() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(0x40).unwrap();
        let b = heap.alloc(0x100).unwrap();
        assert_eq!(heap.block_containing(b + 0x80), Some((b, 0x100)));
        assert_eq!(heap.block_containing(a), Some((a, 0x40)));
        assert_eq!(heap.block_containing(b + 0x100), None, "the block's end is exclusive");
        heap.free(b);
        assert_eq!(heap.block_containing(b + 0x80), None);
    }

    /// **"How much is free" for a single allocation is the largest contiguous block.**
    ///
    /// The test exists because of a measurement: the Z-Wheel asks how much is free and asks for
    /// that in a single allocation. Adding the holes to what remains ahead, the answer was larger
    /// than any allocation can get — the request was refused with the heap far from full, and the
    /// game concluded, correctly for the number it heard, that there was no memory.
    #[test]
    fn the_free_space_after_a_single_allocation_is_the_largest_block() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(0x100).unwrap();
        let b = heap.alloc(0x100).unwrap();
        let c = heap.alloc(0x100).unwrap();
        heap.free(b);
        // 0x1000 in total, 0x300 in use: 0xD00 remains ahead, and the middle hole is 0x100.
        assert_eq!(heap.largest_block(), 0xD00, "the hole does not count");
        // With the largest block smaller than the request, `alloc` refuses — and that is what
        // `CheckAvail` has to say beforehand, instead of adding up.
        assert_eq!(heap.alloc(0xD08), None);
        // 0xD00 still fits ahead, and that is where it comes from: the middle hole is too small
        // for this request, and the allocator does not move live blocks.
        assert_eq!(heap.alloc(0xD00), Some(0x1300), "served by the bump pointer");
        // What counts now is only the hole: a request larger than it fits nowhere.
        assert_eq!(heap.largest_block(), 0x100, "the middle hole remains");
        assert_eq!(heap.alloc(0x108), None);
        let _ = (a, c);
    }

    /// **Merging keeps the heap from falling to pieces.**
    ///
    /// The pattern is the one from the Treino Cerebral report: small blocks allocated and freed
    /// between two large ones. The test **exhausts the bump pointer before measuring** — without
    /// that the space ahead hides the fragmentation, and the snapshot looks healthy precisely when
    /// it is not.
    #[test]
    fn freeing_small_blocks_returns_a_contiguous_region() {
        let mut heap = Heap::new(0x1000_0000, 1 << 20);
        let _first = heap.alloc(64 * 1024).unwrap();
        let small: Vec<u32> = (0..64).map(|_| heap.alloc(4 * 1024).unwrap()).collect();
        // Fills what remains ahead: that way the only free space left is the middle holes.
        while heap.alloc(4 * 1024).is_some() {}
        let before = heap.snapshot();
        assert!(
            before.largest_hole < 64 * 1024,
            "the bump pointer still hid space: the largest block is {}",
            before.largest_hole
        );
        for p in &small {
            heap.free(*p);
        }
        let after = heap.snapshot();
        assert!(
            after.largest_hole >= 256 * 1024,
            "the 64 blocks of 4 KiB should form 256 KiB again; the largest is {}",
            after.largest_hole
        );
        assert_eq!(after.holes, 1, "and become a single hole");
        // What is left ahead (under 4 KiB) is of no use for anything large, and it is the only
        // part of the free space not in a hole: the fragmentation stays below that.
        assert!(
            after.lost_in_holes() < 8 * 1024,
            "free space stuck in holes: {} bytes",
            after.lost_in_holes()
        );
    }

    #[test]
    fn allocates_sequentially_and_aligned() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(4).unwrap();
        let b = heap.alloc(4).unwrap();
        assert_eq!(a, 0x1000);
        assert_eq!(b, 0x1008, "blocks must respect the alignment of {ALIGN}");
    }

    #[test]
    fn reuses_a_freed_block() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(32).unwrap();
        assert_eq!(heap.free(a), Some(32));
        assert_eq!(heap.alloc(16), Some(a), "should reuse the free block");
        // The rest of the block stays available.
        assert_eq!(heap.alloc(16), Some(a + 16));
    }

    #[test]
    fn free_of_an_invalid_pointer_is_harmless() {
        let mut heap = Heap::new(0x1000, 0x1000);
        assert_eq!(heap.free(0), None);
        assert_eq!(heap.free(0xdead), None);
    }

    #[test]
    fn does_not_hand_out_two_equal_pointers_for_size_zero() {
        let mut heap = Heap::new(0x1000, 0x1000);
        assert_ne!(heap.alloc(0).unwrap(), heap.alloc(0).unwrap());
    }

    #[test]
    fn freed_neighbor_blocks_merge_back_into_one() {
        // Without merging, three returned blocks of 32 become three holes of 32 and a request of
        // 96 goes to the top of the heap. That is how Treino Cerebral used up 64 MB.
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(32).unwrap();
        let b = heap.alloc(32).unwrap();
        let c = heap.alloc(32).unwrap();
        let after = heap.alloc(8).unwrap();
        heap.free(b);
        heap.free(a);
        heap.free(c);
        assert_eq!(heap.alloc(96), Some(a), "the three should have become one");
        assert_eq!(heap.free(after), Some(8));
    }

    #[test]
    fn the_top_block_returns_its_space_to_the_heap() {
        let mut heap = Heap::new(0x1000, 64);
        let a = heap.alloc(32).unwrap();
        heap.free(a);
        assert_eq!(heap.used(), 0, "nothing remains handed out");
        assert!(heap.alloc(64).is_some(), "the whole heap fits again");
    }

    #[test]
    fn usage_does_not_count_what_was_already_freed() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(64).unwrap();
        let _b = heap.alloc(64).unwrap();
        assert_eq!(heap.used(), 128);
        heap.free(a);
        assert_eq!(heap.used(), 64, "the free block in the middle is not handed out");
    }

    /// No live block may touch another: merging works on neighborhoods, and a mistake there hands
    /// out the same address twice — the game writes over what was its own.
    #[test]
    fn under_stress_no_live_blocks_overlap() {
        let mut heap = Heap::new(0x1000, 0x8000);
        let mut live: Vec<(u32, u32)> = Vec::new();
        let mut seed: u32 = 12345;
        let mut next_random = move || {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            seed >> 8
        };
        for step in 0..4000 {
            let release = !live.is_empty() && (step % 3 == 0 || live.len() > 40);
            if release {
                let which = next_random() as usize % live.len();
                let (addr, _) = live.swap_remove(which);
                heap.free(addr);
                continue;
            }
            let size = 1 + next_random() % 600;
            let Some(addr) = heap.alloc(size) else {
                continue;
            };
            let end = addr + size;
            for &(other, other_size) in &live {
                assert!(
                    end <= other || addr >= other + other_size,
                    "block {addr:#x}+{size} touches {other:#x}+{other_size}"
                );
            }
            live.push((addr, size));
        }
    }

    /// **The heap comes back from the save state exactly as it was**, and a state from another
    /// machine is refused instead of half applied.
    #[test]
    fn the_heap_round_trips_through_the_save_state() {
        use crate::save_state::{Saveable, Reader, Sections};

        let mut before = Heap::new(0x2000_0000, 4096);
        let a = before.alloc(64).expect("first block");
        let _b = before.alloc(128).expect("second block");
        let c = before.alloc(32).expect("third block");
        before.free(c);
        let _d = before.alloc(16).expect("reuses the smaller block");

        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("the state opened");

        let mut after = Heap::new(0x2000_0000, 4096);
        after.restore(&reader).expect("restored");

        // **The arithmetic is not mine, it is the two heaps'.** Instead of me computing addresses
        // in my head, both receive the same sequence of requests and have to answer the same — it
        // is the same idea as the test that compares the two rasterizers.
        let _ = a;
        let mut expected = Vec::new();
        let mut obtained = Vec::new();
        for size in [64u32, 16, 256, 32, 1024, 8] {
            expected.push(before.alloc(size));
            obtained.push(after.alloc(size));
        }
        assert_eq!(
            obtained, expected,
            "the restored heap allocated differently from the original"
        );
    }

    #[test]
    fn a_heap_from_another_machine_is_rejected() {
        use crate::save_state::{Saveable, Reader, Sections};

        let before = Heap::new(0x2000_0000, 4096);
        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        // Another machine: same size, another address. Applying this would send every guest
        // pointer to the wrong place.
        let mut foreign_heap = Heap::new(0x3000_0000, 4096);
        match foreign_heap.restore(&reader) {
            Err(crate::save_state::Error::Section { name, reason }) => {
                assert_eq!(name, "heap");
                assert!(reason.contains("0x20000000"), "{reason}");
                assert!(reason.contains("0x30000000"), "{reason}");
            }
            other => panic!("should refuse the heap from another machine, and returned {other:?}"),
        }
    }

    #[test]
    fn refuses_when_the_heap_fills_up() {
        let mut heap = Heap::new(0x1000, 16);
        assert!(heap.alloc(8).is_some());
        assert!(heap.alloc(8).is_some());
        assert_eq!(heap.alloc(8), None);
    }
}
