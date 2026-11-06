//! Objects the emulator exposes to the guest.
//!
//! A BREW object is a pointer whose first word points to the vtable. Since our vtables are fixed
//! per interface and live in read-only memory, each object needs only one word of guest memory
//! — the rest of the state stays here, on the host.

use std::collections::HashMap;

use crate::brew::aee::Interface;

/// Space reserved for each object in the guest region.
///
/// Most need a single word, the vtable pointer. The exception is `IDIB`, which exposes 36 bytes
/// of public fields the game reads directly — hence the slack.
const OBJECT_STRIDE: u32 = 64;

#[derive(Debug)]
pub struct ObjectStore {
    end: u32,
    next: u32,
    /// Addresses that have already been released and can be reused.
    ///
    /// Without this the object region is a block that only moves forward: 64 KB at 64 bytes
    /// each, one thousand and twenty-four objects, and that's it. A game that creates and
    /// releases objects in a loop exhausts it in seconds — the Z-Wheel, repeating its intro in
    /// attract mode, ran out, and from then on **everything** failed: `Unable to create vector
    /// model`, forms with error 3, and the game spinning forever because nothing else could be
    /// created.
    ///
    /// Reusing is what an allocator does. A dangling pointer the game kept by mistake ends up
    /// pointing at another object instead of at garbage — which is harder to debug, yes, but it
    /// is exactly what happens on the console, and the alternative is a ceiling no long game
    /// respects.
    free_slots: Vec<u32>,
    /// Interface of each live object, indexed by its guest pointer.
    kinds: HashMap<u32, Interface>,
    /// Reference count, to answer `AddRef`/`Release` honestly.
    refs: HashMap<u32, u32>,
}

impl ObjectStore {
    /// The first never-used address. Same role as [`crate::brew::heap::Heap::next`].
    pub fn next(&self) -> u32 {
        self.next
    }

    pub fn new(base: u32, size: usize) -> Self {
        Self {
            end: base + size as u32,
            next: base,
            free_slots: Vec::new(),
            kinds: HashMap::new(),
            refs: HashMap::new(),
        }
    }

    /// Reserves the address of a new object. The caller writes the vtable pointer.
    pub fn create(&mut self, iface: Interface) -> Option<u32> {
        let addr = match self.free_slots.pop() {
            Some(reused) => reused,
            None => {
                let addr = self.next;
                if addr.checked_add(OBJECT_STRIDE)? > self.end {
                    return None;
                }
                self.next = addr + OBJECT_STRIDE;
                addr
            }
        };
        self.kinds.insert(addr, iface);
        self.refs.insert(addr, 1);
        Some(addr)
    }

    /// Marks an already existing address as an object of an interface — used for the objects
    /// the loader builds before the machine exists, such as `IShell`.
    pub fn adopt(&mut self, addr: u32, iface: Interface) {
        self.kinds.insert(addr, iface);
        self.refs.insert(addr, 1);
    }

    /// Interface of a live object. Returns `None` for a pointer we did not create.
    pub fn kind_of(&self, addr: u32) -> Option<Interface> {
        self.kinds.get(&addr).copied()
    }

    /// How many references an object has now. Zero for one that doesn't exist.
    pub fn count(&self, addr: u32) -> u32 {
        self.refs.get(&addr).copied().unwrap_or(0)
    }

    /// Increments and returns the new count.
    ///
    /// **An already released address does not come back to life.** It is in the free list;
    /// recreating its count without taking it out of there would make the next `Release` put it
    /// on the list **again**, and two live objects would be born in the same place. That is how
    /// the `IGraphics` the Z-Wheel creates to draw the launch notice was born on top of a bitmap:
    /// its `SetDestination` landed on the `IBitmap`, and the chosen game never opened.
    pub fn add_ref(&mut self, addr: u32) -> u32 {
        if !self.refs.contains_key(&addr) && self.free_slots.contains(&addr) {
            return 0;
        }
        let count = self.refs.entry(addr).or_insert(0);
        *count += 1;
        *count
    }

    /// Decrements and returns the remaining count. When it reaches zero, the object is forgotten.
    pub fn release(&mut self, addr: u32) -> u32 {
        let Some(count) = self.refs.get_mut(&addr) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        let remaining = *count;
        if remaining == 0 {
            self.refs.remove(&addr);
            self.kinds.remove(&addr);
            // Never the same address twice in the list: that would be two live objects on it.
            if !self.free_slots.contains(&addr) {
                self.free_slots.push(addr);
            }
        }
        remaining
    }

    pub fn live_count(&self) -> usize {
        self.kinds.len()
    }

    /// How many live objects of each interface, from the most numerous to the least.
    ///
    /// It serves the report. A game that reaches the region's ceiling is leaking references, and
    /// without this list that shows up as "some class stopped being created" — which is exactly
    /// how the Z-Wheel leak manifested.
    pub fn live_by_kind(&self) -> Vec<(Interface, usize)> {
        let mut count: HashMap<Interface, usize> = HashMap::new();
        for kind in self.kinds.values() {
            *count.entry(*kind).or_default() += 1;
        }
        let mut output: Vec<(Interface, usize)> = count.into_iter().collect();
        output.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name().cmp(b.0.name())));
        output
    }
}


impl crate::save_state::Saveable for ObjectStore {
    /// Writes the whole ledger: the end of the region, the first never-used address, the released
    /// addresses and, per pointer, the interface and reference count of each live object.
    ///
    /// **Without `kinds` the save state loads objects that don't know what they are**: the
    /// pointer comes back pointing at the right vtable in memory, and the emulator's dispatch
    /// doesn't know which interface to serve. The interface encoding is the enum discriminant,
    /// and there is a test that checks all 62 round-trip.
    fn save(&self, dest: &mut crate::save_state::Sections) {
        dest.put_u32("objects.end", self.end);
        dest.put_u32("objects.next", self.next);
        dest.put_u32s("objects.livres", self.free_slots.iter().copied());
        dest.put_map(
            "objects.kinds",
            self.kinds.iter().map(|(a, i)| (*a, crate::brew::aee::code(*i))),
        );
        dest.put_map("objects.refs", self.refs.iter().map(|(a, r)| (*a, *r)));
    }

    /// Restores, and rejects before applying: the end of the region has to be this machine's,
    /// every live address has to know its interface, and an address cannot be both free and live.
    fn restore(
        &mut self,
        origin: &crate::save_state::Reader<'_>,
    ) -> Result<(), crate::save_state::Error> {
        use crate::save_state::Error;
        let end = origin.u32("objects.end")?;
        if end != self.end {
            return Err(Error::Section {
                name: "objects".to_string(),
                reason: format!(
                    "the state is from a region {end:#010x}.. and this machine has {:#010x}..",
                    self.end
                ),
            });
        }
        let next = origin.u32("objects.next")?;
        let free_slots = origin.u32s("objects.livres")?;
        let raw_kinds = origin.pairs("objects.kinds")?;
        let refs = origin.pairs("objects.refs")?;

        let mut kinds = std::collections::HashMap::new();
        for (address, code) in raw_kinds {
            let iface = crate::brew::aee::from_code(code).ok_or_else(|| Error::Section {
                name: "objects.kinds".to_string(),
                reason: format!("object {address:#010x} claims to be interface {code}, which this engine does not know"),
            })?;
            kinds.insert(address, iface);
        }
        if let Some((address, _)) = kinds
            .keys()
            .find(|a| free_slots.contains(a))
            .map(|a| (*a, 0))
        {
            return Err(Error::Section {
                name: "objects".to_string(),
                reason: format!("object {address:#010x} appears both released and live"),
            });
        }

        self.next = next;
        self.free_slots = free_slots;
        self.kinds = kinds;
        self.refs = refs.into_iter().collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The object ledger comes back whole**: the next address, the released ones, the interface
    /// of each live object and the reference count.
    ///
    /// What this test guards is the difference between "the pointer came back" and "the pointer
    /// came back **knowing what it is**". Without the interface, the emulator's dispatch doesn't
    /// know which vtable to serve, and the game breaks on the first method it calls.
    #[test]
    fn the_object_ledger_round_trips() {
        use crate::save_state::{Saveable, Reader, Sections};

        let mut before = ObjectStore::new(0x4000_0000, 4096);
        let a = before.create(Interface::Shell).expect("first object");
        let b = before.create(Interface::Display).expect("second object");
        // A third one that **dies**: that way the state has an entry in `free_slots` to check,
        // and two live objects of different interfaces.
        let dead = before.create(Interface::Bitmap).expect("third object");
        let _ = before.add_ref(a);
        before.release(dead);

        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        let mut after = ObjectStore::new(0x4000_0000, 4096);
        after.restore(&reader).expect("restored");

        assert_eq!(
            after.kind_of(a),
            Some(Interface::Shell),
            "the interface of {a:#x} did not come back"
        );
        assert_eq!(after.kind_of(b), Some(Interface::Display));
        assert_eq!(after.kind_of(dead), None, "the released object should not come back alive");
        assert_eq!(after.count(a), before.count(a), "the count of {a:#x}");
        assert_eq!(after.count(b), before.count(b), "the count of {b:#x}");
        // And the next object lands where it would have before: that is what lets the game
        // carry on from where it stopped.
        assert_eq!(after.create(Interface::Bitmap), before.create(Interface::Bitmap));
    }

    #[test]
    fn a_ledger_from_another_machine_is_rejected() {
        use crate::save_state::{Saveable, Reader, Sections};

        let before = ObjectStore::new(0x4000_0000, 4096);
        let mut sections = Sections::create();
        before.save(&mut sections);
        let file = sections.close();
        let reader = Reader::open(&file).expect("opened");

        let mut other = ObjectStore::new(0x4000_0000, 8192);
        assert!(matches!(
            other.restore(&reader),
            Err(crate::save_state::Error::Section { .. })
        ));
    }


    #[test]
    fn addref_on_a_released_object_does_not_duplicate_the_address() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let a = store.create(Interface::Bitmap).unwrap();
        assert_eq!(store.release(a), 0);
        // Someone still had the pointer and asks for a reference on it: it does not come back.
        assert_eq!(store.add_ref(a), 0);
        assert_eq!(store.release(a), 0);
        // The address comes back **once** only: two new objects cannot be born in the same place.
        let b = store.create(Interface::Graphics).unwrap();
        let c = store.create(Interface::Bitmap).unwrap();
        assert_ne!(b, c);
    }

    #[test]
    fn objects_get_distinct_addresses() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let a = store.create(Interface::Display).unwrap();
        let b = store.create(Interface::FileMgr).unwrap();
        assert_ne!(a, b);
        assert_eq!(store.kind_of(a), Some(Interface::Display));
        assert_eq!(store.kind_of(b), Some(Interface::FileMgr));
    }

    #[test]
    fn the_reference_count_goes_up_and_down() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let obj = store.create(Interface::Display).unwrap();
        assert_eq!(store.add_ref(obj), 2);
        assert_eq!(store.release(obj), 1);
        assert_eq!(store.release(obj), 0);
        assert_eq!(
            store.kind_of(obj),
            None,
            "the object disappears when the count reaches zero"
        );
    }

    #[test]
    /// Releasing gives the address back to the allocator. Without that the object region is a
    /// ceiling of one thousand and twenty-four creations for the whole run, and a game that
    /// creates and releases in a loop stops being able to create anything.
    fn a_released_address_is_reused() {
        let mut store = ObjectStore::new(0x3000_0000, 0x10000);
        let first = store.create(Interface::Widget).expect("fits");
        let second = store.create(Interface::Widget).expect("fits");
        assert_ne!(first, second);
        assert_eq!(store.release(first), 0);
        assert_eq!(store.create(Interface::Widget), Some(first));
    }

    #[test]
    /// The ceiling exists, but it is on **live** objects, not on creations. Creating and
    /// releasing in a loop has to be able to go on forever.
    fn creating_and_releasing_in_a_loop_does_not_exhaust() {
        let mut store = ObjectStore::new(0x3000_0000, 0x400);
        for _ in 0..10_000 {
            let object = store
                .create(Interface::Widget)
                .expect("one at a time always fits");
            store.release(object);
        }
        assert_eq!(store.live_count(), 0);
    }

    #[test]
    fn release_of_an_unknown_object_does_not_panic() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        assert_eq!(store.release(0xdead), 0);
    }

    #[test]
    fn refuses_when_the_region_fills_up() {
        let mut store = ObjectStore::new(0x3000_0000, OBJECT_STRIDE as usize);
        assert!(store.create(Interface::Display).is_some());
        assert!(store.create(Interface::Display).is_none());
    }
}
