//! The machine state in sections: registers, memory and the allocation counters.
//!
//! ## What goes in, and why
//!
//! **Every writable region** of the map. What stays out is what the loader builds once and
//! nobody changes afterwards: the vtables and the null page. Both are derived from the image, so
//! saving them would only waste bytes — and a state that loads old vtables over the new ones
//! would be worse than loading nothing.
//!
//! ## The cut in the regions that only grow
//!
//! The heap (64 MB), the object region (4 MB) and the surfaces (8 MB) start out **zeroed** and
//! only move forward: `next` is the first address never used, and below it is everything the
//! game asked for. Saving beyond that would mean writing tens of megabytes of zeros into every
//! save state. The cut is the `next` of each region's allocator, and it is what makes the state
//! size proportional to what the game used — a few megabytes, not eighty.
//!
//! ## What does not go in yet
//!
//! The object region goes in, but its **ledger** does not: [`crate::brew::objects::ObjectStore`]
//! keeps each object's interface in an `enum` that does not have a stable encoding yet. Until
//! that and the per-object state tables go in, the core's `retro_serialize_size` keeps
//! answering zero — see the plan. Restoring a partial state is exactly what the criterion
//! forbids, and this is the gate.

use super::{
    Callback, CipherState, DecodedImage, PcmStream, Machine, MediaState, MemStream, ValueModel,
    OpenFile, Outcome, Peek, PendingBlit, PendingSurfaceBlit, ImageClip, SoundState,
    ThreadState, Timer, DecoderState, GuestCall, HashState, UnzipState, Widget,
};
use crate::input::Pad;
use crate::machine::{default_colors, ArrayPointer, AES_BLOCK, CLR_COUNT, GraphicsState};
use crate::video::display::{Rect, Rgb};
use crate::video::display::Framebuffer;
use crate::cpu::{CpuBackend, Reg};
use crate::save_state::{Error, Saveable, Reader, Sections};

/// The registers, in the order the section saves them.
///
/// The order is fixed and explicit: saving "all registers" in `enum` order would leave the format
/// hostage to someone reordering the enumeration.
const REGISTERS: [Reg; 16] = [
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
    Reg::Pc,
];

/// How much of a region was used, from the first address never used.
///
/// A `next` outside the region would be an allocator defect; saving everything is the safe answer.
fn used(next: u32, base: u32, size: usize) -> usize {
    let used = next.saturating_sub(base) as usize;
    if used > size { size } else { used }
}

/// The section name of a region.
fn region_section(name: &str) -> String {
    format!("mem.{name}")
}

impl<C: CpuBackend> Machine<C> {
    /// How many bytes of a region should be saved.
    ///
    /// The `next` of the region's allocator, when it has one; the whole size, when it does not.
    /// Returns `None` for a region that must not be saved at all.
    fn len_to_save(&self, name: &str, base: u32, size: usize, writable_path: bool) -> Option<usize> {
        if !writable_path {
            return None;
        }
        // **The three regions that only grow are cut at the first address never used**, and the
        // cut is possible because the ledgers of all three go into the state: that is where the
        // expected size comes from on the way back. Saving all three whole would be 76 MB of zeros
        // per save state.
        let used = match name {
            "heap" => used(self.heap.next(), base, size),
            "objects" => used(self.objects.next(), base, size),
            "surfaces" => used(self.surfaces.next(), base, size),
            "internal" => used(self.internal_heap.next(), base, size),
            _ => size,
        };
        Some(used)
    }

    /// How much of a region the **state** says was used.
    ///
    /// It comes from the file, not from the current machine: whoever loads a save state loads the
    /// heap that was there. That is what makes it possible to check the section size before
    /// writing any byte.
    fn used_len_in_state(
        reader: &Reader<'_>,
        name: &str,
        base: u32,
        size: usize,
    ) -> Result<usize, Error> {
        let (prefix, field) = match name {
            "heap" => ("heap", "heap.next"),
            "objects" => ("objects", "objects.next"),
            "surfaces" => ("surfaces", "surfaces.next"),
            "internal" => ("internal", "internal.next"),
            _ => return Ok(size),
        };
        let next = reader.u32(field)?;
        let start = match prefix {
            "objects" => base,
            _ => reader.u32(&format!("{prefix}.base"))?,
        };
        Ok(used(next, start, size))
    }

    /// Saves the state as one section per region, plus the registers and the ledgers.
    pub fn can_save(&mut self) -> Result<(), String> {
        // **Drain first, refuse afterwards.** A batch waiting its turn is not a half-finished
        // draw: it is work that was going to be done on the next frame. Refusing the save because
        // of it would block the player over something the engine resolves on its own. What is left
        // after draining is a truly interrupted draw — a `glBegin` without `glEnd` —, and then
        // the refusal is honest.
        self.gl.flush_draw();
        if self.gl.draw_in_progress() {
            return Err(
                "a draw was started and not finished; save at the end of the frame".to_string(),
            );
        }
        Ok(())
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut sections = Sections::create();
        sections.put_u32s(
            "cpu.regs",
            REGISTERS.iter().map(|reg| self.cpu.read_reg(*reg)),
        );
        sections.put_u32("cpu.thumb", u32::from(self.cpu.in_thumb()));
        // The flags and the virtual clock. Without them the game comes back with the right memory
        // and the wrong decisions: the comparison it made before saving still holds, and the
        // branch comes afterwards.
        sections.put_u32("cpu.cpsr", self.cpu.cpsr());
        sections.put(
            "cpu.instructions",
            self.cpu.instructions().to_le_bytes().to_vec(),
        );
        // The other term of the clock: the idle time the emulator skipped. See `relogio.us` in
        // the header of [`Machine::save_scalars_and_maps`].
        sections.put("relogio.us", self.clock_us.to_le_bytes().to_vec());
        for region in self.module.mem.regions() {
            let Some(amount) =
                self.len_to_save(region.name, region.base, region.bytes.len(), region.writable)
            else {
                continue;
            };
            let mut bytes = vec![0u8; amount];
            // The **live** memory is in the core, not in the loader's map: this is where you read
            // what the game has written since the `reset`.
            if self.cpu.read_mem(region.base, &mut bytes).is_ok() {
                sections.put(&region_section(region.name), bytes);
            }
        }
        // The three ledgers. The surfaces one goes under the region's name, so the section and
        // the ledger have the same name — whoever reads the file needs no translation table.
        self.save_input_and_time(&mut sections);
        self.save_numeric_tables(&mut sections);
        self.save_fonts_and_files(&mut sections);
        self.save_contents(&mut sections);
        self.save_surfaces_and_images(&mut sections);
        self.save_scalars_and_maps(&mut sections);
        self.save_lists_and_stall(&mut sections);
        self.save_remaining_tables(&mut sections);
        self.save_the_rest(&mut sections);
        self.save_widgets(&mut sections);
        self.save_libraries(&mut sections);
        self.save_trailing(&mut sections);
        // The rasterizer is guest state. Without it, CPU and memory come back but textures,
        // matrices, lights and 3D buffers stay at the wrong instant.
        self.gl.save_state(&mut sections);
        self.heap.save_with_prefix("heap", &mut sections);
        self.objects.save(&mut sections);
        self.surfaces.save_with_prefix("surfaces", &mut sections);
        self.internal_heap.save_with_prefix("internal", &mut sections);
        sections.close()
    }

    pub fn restore_state(&mut self, file: &[u8]) -> Result<(), Error> {
        let reader = Reader::open(file)?;

        let registers = reader.u32s("cpu.regs")?;
        if registers.len() != REGISTERS.len() {
            return Err(Error::Section {
                name: "cpu.regs".to_string(),
                reason: format!(
                    "the state has {} registers and the machine has {}",
                    registers.len(),
                    REGISTERS.len()
                ),
            });
        }
        let _thumb = reader.u32("cpu.thumb")?;
        let cpsr = reader.u32("cpu.cpsr")?;
        let clock_bytes = reader.section("cpu.instructions").ok_or_else(|| Error::Section {
            name: "cpu.instructions".to_string(),
            reason: "the section is not in the file".to_string(),
        })?;
        if clock_bytes.len() != 8 {
            return Err(Error::Section {
                name: "cpu.instructions".to_string(),
                reason: format!("expected 8 bytes and got {}", clock_bytes.len()),
            });
        }
        let clock = u64::from_le_bytes(clock_bytes.try_into().unwrap());
        // Optional: a state saved before 2026-10-10 does not have the section, and loads as it
        // used to — with the current skipped time. See the header of
        // [`Machine::save_scalars_and_maps`].
        let skipped_time = match reader.section("relogio.us") {
            None => None,
            Some(bytes) => Some(u64::from_le_bytes(bytes.try_into().map_err(|_| Error::Section {
                name: "relogio.us".to_string(),
                reason: format!("expected 8 bytes and got {}", bytes.len()),
            })?)),
        };

        // Memory is read and checked before any write.
        let mut regions: Vec<(u32, Vec<u8>)> = Vec::new();
        for region in self.module.mem.regions() {
            let Some(_) = self.len_to_save(region.name, region.base, region.bytes.len(), region.writable) else {
                continue;
            };
            let section = region_section(region.name);
            // A state from before the internal region: our blocks are inside the heap, and
            // `free` returns them there by address. The region stays as it is, and the internal
            // allocator starts over empty further down.
            if region.name == "internal" && reader.section(&section).is_none() {
                continue;
            }
            let bytes = reader.section(&section).ok_or_else(|| Error::Section {
                name: section.clone(),
                reason: "the section is not in the file".to_string(),
            })?;
            // **The expected size comes from the state itself**, not from the current machine.
            let amount = Self::used_len_in_state(&reader, region.name, region.base, region.bytes.len())?;
            if bytes.len() != amount {
                return Err(Error::Section {
                    name: section,
                    reason: format!(
                        "the state has {} bytes of this region and this machine expects {amount} \
                         — it is a state from another game or another engine version",
                        bytes.len()
                    ),
                });
            }
            regions.push((region.base, bytes.to_vec()));
        }

        // From here down it is applying: all or nothing.
        self.restore_input_and_time(&reader)?;
        self.restore_numeric_tables(&reader)?;
        self.restore_fonts_and_files(&reader)?;
        self.restore_contents(&reader)?;
        self.restore_surfaces_and_images(&reader)?;
        self.restore_scalars_and_maps(&reader)?;
        self.restore_lists_and_stall(&reader)?;
        self.restore_remaining_tables(&reader)?;
        self.restore_the_rest(&reader)?;
        self.restore_widgets(&reader)?;
        self.restore_libraries(&reader)?;
        self.restore_trailing(&reader)?;
        // The original 0.3.0 saved states without the GL sections due to an integration slip.
        // Continuing to accept those files keeps the old slots of 2D games working; new states
        // always load the full rasterizer.
        if reader.section("gl.matrizes").is_some() {
            self.gl.restore_state(&reader)?;
        }
        self.heap.restore_with_prefix("heap", &reader)?;
        self.objects.restore(&reader)?;
        self.surfaces
            .restore_with_prefix("surfaces", &reader)?;
        if reader.section("internal.base").is_some() {
            self.internal_heap.restore_with_prefix("internal", &reader)?;
        } else {
            self.internal_heap = crate::brew::heap::Heap::new(crate::loader::INTERNAL_BASE, crate::loader::INTERNAL_SIZE);
        }
        for (base, bytes) in regions {
            self.cpu.write_mem(base, &bytes).map_err(|error| Error::Section {
                name: format!("mem.{base:#010x}"),
                reason: format!("could not write: {error}"),
            })?;
        }
        for (reg, value) in REGISTERS.iter().zip(registers) {
            self.cpu.write_reg(*reg, value);
        }
        // `CPSR` goes last: writing to the `PC` can touch the mode bits in some cores, and the
        // value that came from the state is the one that rules.
        self.cpu.set_cpsr(cpsr);
        self.cpu.set_instructions(clock);
        if let Some(skipped_time) = skipped_time {
            self.clock_us = skipped_time;
        }
        // With the clock already in place: each effect's sound resumes from the point it marks.
        self.rebuild_sound();
        // The GPU texture does not come back with the state: until the next buffer swap, the
        // image is the one from the restored screen. See [`Machine::gpu_frame`].
        self.gpu_without_frame = true;
        self.gl_frame_pending = false;
        Ok(())
    }
}


/// The code for an input signal name.
///
/// The two names are the **closed** set `IHIDDevice` uses to notify whoever registered:
/// `RegisterForButtonEvent` and `RegisterForPositionChange`. Storing the name as text would leave
/// the format hostage to a string; storing the code makes the read impossible to get wrong
/// silently — an unknown code is a refusal, not a lost registration.
fn input_signal_code(name: &str) -> u32 {
    match DEVICE_SIGNALS.iter().position(|n| *n == name) {
        Some(index) => index as u32,
        None => {
            // **The writer shouts instead of writing the sentinel.** I wrote `u32::MAX` as "I do
            // not know this name" and the reader rejects it — which looked safe, and was not: the
            // result was a save state **saved successfully and impossible to load**. The bug only
            // showed up because the round-trip test exists, and the missing name was the third
            // one (`RegisterForConnectEvents`), which I had assumed did not exist.
            debug_assert!(false, "the device signal \"{name}\" is not in DEVICE_SIGNALS");
            u32::MAX
        }
    }
}

/// The device signal names `IHIDDevice` uses to notify whoever registered.
///
/// **Measured, not assumed**: there are three. The list exists so the code is stable across
/// versions, and both directions (`input_signal_code` and `input_signal_name`) come from it — so
/// adding a name means touching a single place, and the test demands the round trip of **all**.
const DEVICE_SIGNALS: [&str; 3] = [
    "RegisterForButtonEvent",
    "RegisterForConnectEvents",
    "RegisterForPositionChange",
];

/// The name of an input signal, by code.
fn input_signal_name(code: u32) -> Option<&'static str> {
    DEVICE_SIGNALS.get(code as usize).copied()
}

impl<C: CpuBackend> Machine<C> {
    /// Input and scheduling: key and button queues, who registered a device signal, the timers
    /// coming due and the pending callbacks.
    ///
    /// These are the tables a game feels right away: without the queues, the key it has not read
    /// yet disappears; without the timers, the clock it armed for two hundred milliseconds from
    /// now ceases to exist; and without the signals, the notice that the stick moved never
    /// reaches whoever asked for it.
    fn save_input_and_time(&self, sections: &mut Sections) {
        sections.put_u32s(
            "entrada.teclas",
            self.key_events.iter().map(|(avk, down)| [*avk, u32::from(*down)]).flatten(),
        );
        for (port, queue) in self.pad_events.iter().enumerate() {
            sections.put_u32s(
                &format!("entrada.pad_events.{port}"),
                queue.iter()
                    .map(|(index, down)| [*index as u32, u32::from(*down)])
                    .flatten(),
            );
        }
        sections.put_map("entrada.portas", self.device_ports.iter().map(|(a, p)| (*a, *p as u32)));
        sections.put_triples(
            "entrada.sinais",
            self.input_signals.iter().map(|((name, port), sign)| {
                (
                    input_signal_code(name),
                    *port as u32,
                    *sign,
                )
            }),
        );
        sections.put_triples("agenda.timers", self.timers.iter().map(|t| {
            (t.deadline_ms, t.callback.function, t.callback.context)
        }));
        sections.put_triples("agenda.sinais", self.signals.iter().map(|(id, cb)| {
            (*id, cb.function, cb.context)
        }));
        sections.put_u32s(
            "agenda.pendentes",
            self.pending_signals
                .iter()
                .flat_map(|cb| [cb.function, cb.context]),
        );
    }

    /// Reads and checks input and scheduling. Nothing is applied if any section does not match.
    fn restore_input_and_time(
        &mut self,
        reader: &Reader<'_>,
    ) -> Result<(), Error> {
        // **In order**: the key queue is a sequence, not a map.
        let key_events = reader.ordered_pairs("entrada.teclas")?;
        let ports = reader.pairs("entrada.portas")?;
        let raw_signals = reader.triples("entrada.sinais")?;
        let timers = reader.triples("agenda.timers")?;
        let signals = reader.triples("agenda.sinais")?;
        let pending = reader.ordered_pairs("agenda.pendentes")?;

        let mut queues = Vec::new();
        for port in 0..self.pad_events.len() {
            queues.push(reader.ordered_pairs(&format!("entrada.pad_events.{port}"))?);
        }

        // The device signal registration is the only one with a name: an unknown code is a
        // refusal.
        let mut saved_input_signals = std::collections::BTreeMap::new();
        for (code, port, sign) in raw_signals {
            let name = input_signal_name(code).ok_or_else(|| Error::Section {
                name: "entrada.sinais".to_string(),
                reason: format!(
                    "the state records device signal {code}, which this engine does not know"
                ),
            })?;
            if port as usize >= self.pad_events.len() {
                return Err(Error::Section {
                    name: "entrada.sinais".to_string(),
                    reason: format!("port {port} does not exist"),
                });
            }
            saved_input_signals.insert((name, port as usize), sign);
        }
        let mut device_ports = std::collections::HashMap::new();
        for (device, port) in ports {
            if port as usize >= self.pad_events.len() {
                return Err(Error::Section {
                    name: "entrada.portas".to_string(),
                    reason: format!("port {port} does not exist"),
                });
            }
            device_ports.insert(device, port as usize);
        }

        // From here down it is applying.
        self.key_events = key_events
            .into_iter()
            .map(|(avk, down)| (avk, down != 0))
            .collect();
        for (port, queue) in queues.into_iter().enumerate() {
            self.pad_events[port] = queue
                .into_iter()
                .map(|(index, down)| (index as usize, down != 0))
                .collect();
        }
        self.device_ports = device_ports;
        self.input_signals = saved_input_signals;
        self.timers = timers
            .into_iter()
            .map(|(deadline_ms, function, context)| Timer {
                deadline_ms,
                callback: Callback { function, context },
            })
            .collect();
        self.signals = signals
            .into_iter()
            .map(|(id, function, context)| (id, Callback { function, context }))
            .collect();
        self.pending_signals = pending
            .into_iter()
            .map(|(function, context)| Callback { function, context })
            .collect();
        Ok(())
    }
}


/// The per-object state tables whose contents are **numbers**.
///
/// The list is declared once, here, and it is what both saves and reads — with the same section
/// name on both sides. That is how most of the state got in: every table has the same shape
/// (`HashMap<u32, number>` or `HashMap<u32, handful of numbers>`), and only the name changes.
///
/// **What does not go in:** the tables that hold pixels or bytes in bulk — `bitmaps`,
/// `images`, `gl_last_frame_words`. They are most of what is left and need a format of their own
/// (compress, or point to guest memory when the content is already there). Until they go in, the
/// core keeps saying it does not save.
impl<C: CpuBackend> Machine<C> {
    /// The numeric tables: `(section name, values)`.
    /// Saves the numeric tables, one section per table.
    fn save_numeric_tables(&self, sections: &mut Sections) {
        for (name, values) in self.numeric_tables() {
            sections.put_u32s(name, values);
        }
    }

    /// Reads and applies the numeric tables.
    ///
    /// As with the rest: **read everything before writing anything**. And check the range of every
    /// field that is smaller than `u32` in the engine — a volume that does not fit in `u16` or an
    /// `owner` that is not boolean is a corrupt file, not a value to accommodate.
    fn restore_numeric_tables(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let map = |name: &str| -> Result<std::collections::HashMap<u32, u32>, Error> {
            Ok(reader.pairs(name)?.into_iter().collect())
        };
        let dib_buffers = map("tab.dib_buffers")?;
        let dib_capacity = map("tab.dib_capacity")?;
        let transforms = map("tab.transformacoes")?;
        let canvases = map("tab.canvases")?;
        let feeds = map("tab.feeds")?;
        let image_bitmaps = map("tab.image_bitmaps")?;
        let image_info = map("tab.image_info")?;

        let mut raw_transparency = map("tab.transparency")?;
        for (index, value) in raw_transparency.iter() {
            if *value > u32::from(u16::MAX) {
                return Err(Error::Section {
                    name: "tab.transparency".to_string(),
                    reason: format!("object {index:#010x} has transparency {value}, which does not fit in a u16"),
                });
            }
        }
        let transparency: std::collections::HashMap<u32, u16> = raw_transparency
            .drain()
            .map(|(a, v)| (a, v as u16))
            .collect();

        let mut streams = std::collections::HashMap::new();
        for logging in reader.records("tab.streams", 5)? {
            let owner = match logging[4] {
                0 => false,
                1 => true,
                other => {
                    return Err(Error::Section {
                        name: "tab.streams".to_string(),
                        reason: format!("the `owner` field is {other}, and it is boolean"),
                    })
                }
            };
            streams.insert(
                logging[0],
                MemStream {
                    buffer: logging[1],
                    size: logging[2],
                    position: logging[3],
                    owner,
                },
            );
        }

        let mut sounds = std::collections::HashMap::new();
        for logging in reader.records("tab.sounds", 9)? {
            if logging[3] > u32::from(u16::MAX) {
                return Err(Error::Section {
                    name: "tab.sounds".to_string(),
                    reason: format!("sound {:#010x} has volume {}", logging[0], logging[3]),
                });
            }
            let mut info = [0u8; 5];
            for (dest, value) in info.iter_mut().zip(&logging[4..9]) {
                if *value > 255 {
                    return Err(Error::Section {
                        name: "tab.sounds".to_string(),
                        reason: format!("the `AEESoundInfo` of sound {:#010x} has byte {value}", logging[0]),
                    });
                }
                *dest = *value as u8;
            }
            sounds.insert(
                logging[0],
                SoundState {
                    notify: Callback {
                        function: logging[1],
                        context: logging[2],
                    },
                    info,
                    volume: logging[3] as u16,
                },
            );
        }

        // Applying.
        self.dib_buffers = dib_buffers;
        self.dib_capacity = dib_capacity;
        self.transforms = transforms;
        self.canvases = canvases;
        self.feeds = feeds;
        self.image_bitmaps = image_bitmaps;
        self.image_info = image_info;
        self.transparency = transparency;
        self.streams = streams;
        self.sounds = sounds;
        Ok(())
    }

    fn numeric_tables(&self) -> Vec<(&'static str, Vec<u32>)> {
        let map = |m: &std::collections::HashMap<u32, u32>| -> Vec<u32> {
            let mut pairs: Vec<(u32, u32)> = m.iter().map(|(a, b)| (*a, *b)).collect();
            pairs.sort_unstable();
            pairs.into_iter().flat_map(|(a, b)| [a, b]).collect()
        };
        vec![
            ("tab.dib_buffers", map(&self.dib_buffers)),
            ("tab.dib_capacity", map(&self.dib_capacity)),
            ("tab.transformacoes", map(&self.transforms)),
            ("tab.canvases", map(&self.canvases)),
            ("tab.feeds", map(&self.feeds)),
            ("tab.image_bitmaps", map(&self.image_bitmaps)),
            ("tab.image_info", map(&self.image_info)),
            (
                "tab.transparency",
                map(
                    &self
                        .transparency
                        .iter()
                        .map(|(a, v)| (*a, u32::from(*v)))
                        .collect(),
                ),
            ),
            // Where each memory stream's read position was: four numbers per object.
            (
                "tab.streams",
                self.streams
                    .iter()
                    .map(|(id, s)| {
                        vec![
                            *id,
                            s.buffer,
                            s.size,
                            s.position,
                            u32::from(s.owner),
                        ]
                    })
                    .flatten()
                    .collect(),
            ),
            // The state of each `ISound`: who gets notified, the five bytes of `AEESoundInfo` and
            // the volume.
            (
                "tab.sounds",
                self.sounds
                    .iter()
                    .map(|(id, s)| {
                        let mut logging = vec![
                            *id,
                            s.notify.function,
                            s.notify.context,
                            u32::from(s.volume),
                        ];
                        logging.extend(s.info.iter().map(|b| u32::from(*b)));
                        logging
                    })
                    .flatten()
                    .collect(),
            ),
        ]
    }
}


/// The font metrics and the open files.
///
/// Two tables that do **not** hold content, which is why they are cheap:
///
/// - the font metrics are seven numbers per object — they come from the `.bid` and are queried on
///   every `GetFontMetrics`;
/// - an open file has a **path and an offset**, not bytes: the content is on disk. On the way back
///   the file is **reopened by path** and the offset is restored with a `seek`. It is honest and
///   it is what a console save state does: if the file changed on disk between saving and
///   loading, the state loads the current file — and that is said here, so it does not become a
///   surprise.
impl<C: CpuBackend> Machine<C> {
    fn save_fonts_and_files(&self, sections: &mut Sections) {
        sections.put_records(
            "tab.fontes",
            self.fonts
                .iter()
                .map(|(id, m)| {
                    vec![
                        *id,
                        u32::from(m.ascent),
                        u32::from(m.descent),
                        u32::from(m.leading),
                        u32::from(m.max_char_width),
                        u32::from(m.height),
                        u32::from(m.bold),
                        u32::from(m.italic),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        // One object per file, with its own section names: the identifier goes into the name, not
        // into a parallel list of identifiers someone would have to keep in order.
        sections.put_u32s("arq.ids", self.open_files.keys().copied());
        for (id, file) in &self.open_files {
            sections.put_text(&format!("arq.{id}.guest"), &file.guest_path);
            sections.put_text(&format!("arq.{id}.caminho"), &file.path.to_string_lossy());
            // The offset read **now**: it is what says where in the read the game was.
            let position = offset(&file.file).unwrap_or(0);
            sections.put(&format!("arq.{id}.pos"), position.to_le_bytes().to_vec());
        }
    }

    fn restore_fonts_and_files(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let mut fonts = std::collections::HashMap::new();
        for logging in reader.records("tab.fontes", 8)? {
            let smallest = |index: usize| -> Result<u16, Error> {
                u16::try_from(logging[index]).map_err(|_| Error::Section {
                    name: "tab.fontes".to_string(),
                    reason: format!(
                        "font {:#010x} has field {index} set to {}, which does not fit in a u16",
                        logging[0], logging[index]
                    ),
                })
            };
            fonts.insert(
                logging[0],
                crate::machine::font::Metrics {
                    ascent: smallest(1)?,
                    descent: smallest(2)?,
                    leading: smallest(3)?,
                    max_char_width: smallest(4)?,
                    height: smallest(5)?,
                    bold: logging[6] != 0,
                    italic: logging[7] != 0,
                },
            );
        }

        // The files are **reopened** before anything is applied: a path that no longer exists is a
        // refusal, not a file table with an empty entry.
        let ids = reader.u32s("arq.ids")?;
        let mut opened = std::collections::HashMap::new();
        for id in ids {
            let guest_path = reader.text(&format!("arq.{id}.guest"))?;
            let recorded = std::path::PathBuf::from(reader.text(&format!("arq.{id}.caminho"))?);
            // The host path comes from the state file, and a state is just a file: outside what
            // the game can reach, what counts is what the guest path resolves to today. It is
            // also what takes a state from before the shared save store to the migrated copy.
            let path = match self.vfs.contains_host(&recorded) {
                true => recorded,
                false => self.vfs.resolve(&guest_path).ok_or_else(|| Error::Section {
                    name: format!("arq.{id}"),
                    reason: format!(
                        "the state holds \"{}\", outside what the game can reach",
                        recorded.display()
                    ),
                })?,
            };
            let position_bytes = reader.section(&format!("arq.{id}.pos")).ok_or_else(|| Error::Section {
                name: format!("arq.{id}.pos"),
                reason: "the section is not in the file".to_string(),
            })?;
            if position_bytes.len() != 8 {
                return Err(Error::Section {
                    name: format!("arq.{id}.pos"),
                    reason: format!("expected 8 bytes and got {}", position_bytes.len()),
                });
            }
            let position = u64::from_le_bytes(position_bytes.try_into().unwrap());
            let mut file = std::fs::File::open(&path).map_err(|error| Error::Section {
                name: format!("arq.{id}"),
                reason: format!(
                    "the state holds \"{}\" open and it could not be reopened: {error}",
                    path.display()
                ),
            })?;
            use std::io::Seek as _;
            file.seek(std::io::SeekFrom::Start(position)).map_err(|error| Error::Section {
                name: format!("arq.{id}"),
                reason: format!("could not seek the file back to byte {position}: {error}"),
            })?;
            // Reopened read-only, as it always was: the only thing that changes here is the record.
            opened.insert(id, OpenFile { file, guest_path, path, for_writing: false });
        }

        self.fonts = fonts;
        self.open_files = opened;
        Ok(())
    }
}

/// At which byte of the file the read is.
fn offset(file: &std::fs::File) -> std::io::Result<u64> {
    use std::io::Seek as _;
    let mut copy = file;
    copy.stream_position()
}


/// The tables that hold **content**: preferences, collection parameters, `IConfig` data, the
/// decrypted text, the network response and what `IWeb` has already downloaded.
///
/// They all use the same block helper, and the only difference between them is how many numbers
/// make up the key — one for `HashMap<u32, Vec<u8>>`, two for `HashMap<(u32, u32), Vec<u8>>`.
/// They keep the content because it **is nowhere else** outside the engine: unlike the open
/// files, which are reopened from disk, a `SetPrefs` the game did no longer exists on any disk.
impl<C: CpuBackend> Machine<C> {
    fn save_contents(&self, sections: &mut Sections) {
        sections.put_blocks(
            "cont.prefs",
            2,
            self.prefs
                .iter()
                .map(|((class_id, version), data)| {
                    (vec![*class_id, u32::from(*version)], data.clone())
                })
                .collect::<Vec<_>>(),
        );
        sections.put_blocks(
            "cont.parametros",
            2,
            self.collection_params
                .iter()
                .map(|((a, b), data)| (vec![*a, *b], data.clone()))
                .collect::<Vec<_>>(),
        );
        sections.put_blocks(
            "cont.sources",
            1,
            self.sources
                .iter()
                .map(|(id, data)| (vec![*id], data.clone()))
                .collect::<Vec<_>>(),
        );
        sections.put_blocks(
            "cont.paginas",
            1,
            self.html_pages
                .iter()
                .map(|(id, data)| (vec![*id], data.clone()))
                .collect::<Vec<_>>(),
        );
        // `IConfig` has one map per object: a two-number key per entry, the object and the
        // item.
        sections.put_blocks(
            "cont.config",
            2,
            self.config_items
                .iter()
                .flat_map(|(object, items)| {
                    items
                        .iter()
                        .map(move |(item, data)| (vec![*object, *item], data.clone()))
                })
                .collect::<Vec<_>>(),
        );
        sections.put_blocks(
            "cont.plaintexts",
            1,
            self.plaintexts
                .iter()
                .enumerate()
                .map(|(order, data)| (vec![order as u32], data.clone()))
                .collect::<Vec<_>>(),
        );
        sections.put_blocks("cont.resposta", 1, vec![(vec![0u32], self.web_response.clone())]);
    }

    fn restore_contents(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        // The key width is checked **before** any indexing: a file that says "one-number key" on a
        // two-number table would make the index `key_id[1]` panic, and a panic while loading a
        // save state brings the frontend down.
        let check = |name: &str, key_id: &[u32], expected: usize| -> Result<(), Error> {
            if key_id.len() != expected {
                return Err(Error::Section {
                    name: name.to_string(),
                    reason: format!(
                        "the key has {} number(s) and this table uses {expected}",
                        key_id.len()
                    ),
                });
            }
            Ok(())
        };
        let mut prefs = std::collections::HashMap::new();
        for (key_id, data) in reader.blocks("cont.prefs")? {
            check("cont.prefs", &key_id, 2)?;
            prefs.insert((key_id[0], key_id[1] as u16), data);
        }
        let mut parameters = std::collections::HashMap::new();
        for (key_id, data) in reader.blocks("cont.parametros")? {
            check("cont.parametros", &key_id, 2)?;
            parameters.insert((key_id[0], key_id[1]), data);
        }
        let mut sources = std::collections::HashMap::new();
        for (key_id, data) in reader.blocks("cont.sources")? {
            check("cont.sources", &key_id, 1)?;
            sources.insert(key_id[0], data);
        }
        let mut html_pages = std::collections::HashMap::new();
        for (key_id, data) in reader.blocks("cont.paginas")? {
            check("cont.paginas", &key_id, 1)?;
            html_pages.insert(key_id[0], data);
        }
        let mut config_items: std::collections::HashMap<u32, std::collections::HashMap<u32, Vec<u8>>> =
            std::collections::HashMap::new();
        for (key_id, data) in reader.blocks("cont.config")? {
            check("cont.config", &key_id, 2)?;
            config_items.entry(key_id[0]).or_default().insert(key_id[1], data);
        }
        let mut plaintexts = std::collections::VecDeque::new();
        for (key_id, data) in reader.blocks("cont.plaintexts")? {
            check("cont.plaintexts", &key_id, 1)?;
            let order = key_id[0] as usize;
            // The queue is **ordered**, and the order is the content: the saved index is its place.
            while plaintexts.len() <= order {
                plaintexts.push_back(Vec::new());
            }
            plaintexts[order] = data;
        }
        let response = reader
            .blocks("cont.resposta")?
            .into_iter()
            .map(|(_, data)| data)
            .next()
            .ok_or_else(|| Error::Section {
                name: "cont.resposta".to_string(),
                reason: "the section does not have the response".to_string(),
            })?;

        self.prefs = prefs;
        self.collection_params = parameters;
        self.sources = sources;
        self.html_pages = html_pages;
        self.config_items = config_items;
        self.plaintexts = plaintexts;
        self.web_response = response;
        Ok(())
    }
}



/// Packs a `Vec<bool>` into bits, one per pixel.
///
/// `DecodedImage` keeps one boolean per pixel saying whether it is drawn. One byte per pixel
/// would spend eight times what is needed on a table that already keeps two bytes of color per
/// pixel.
fn pack_bits(bits: &[bool]) -> Vec<u8> {
    let mut output = vec![0u8; bits.len().div_ceil(8)];
    for (index, bit) in bits.iter().enumerate() {
        if *bit {
            output[index / 8] |= 1 << (index % 8);
        }
    }
    output
}

/// Unpacks what [`pack_bits`] wrote.
fn unpack_bits(bytes: &[u8], count: usize) -> Vec<bool> {
    (0..count)
        .map(|index| bytes[index / 8] & (1 << (index % 8)) != 0)
        .collect()
}

/// The surfaces and the decoded images: the **two families that hold pixels**.
///
/// They are what is left of content in the engine after the numeric tables, and they cannot be
/// re-derived from anything: unlike a guest surface, which lives in the memory that goes into the
/// state, a `Framebuffer` is a host-side copy, created by `Framebuffer::new` and filled with what
/// the game drew. The same goes for an already decoded image — the source file may have changed on
/// disk, and decoding it again would give something else.
///
/// Each surface takes two sections, with the identifier in the name: one of numbers (size, what
/// has already been written and the dirty box) and one of pixels. It is on purpose — that way the
/// pixels stay in one contiguous block, not mixed with numbers in the middle.
impl<C: CpuBackend> Machine<C> {
    fn save_surfaces_and_images(&self, sections: &mut Sections) {
        sections.put_u32s("sup.ids", self.bitmaps.keys().copied());
        for (id, surface) in &self.bitmaps {
            save_surface(sections, &format!("sup.{id}"), surface);
        }
        // The screen is a surface too, and the only one that always exists: whoever looks at the
        // frame sees this one.
        save_surface(sections, "sup.tela", &self.screen);

        sections.put_u32s("img.ids", self.images.keys().copied());
        for (id, image) in &self.images {
            sections.put_u32s(
                &format!("img.{id}.meta"),
                [
                    image.width,
                    image.height,
                    u32::from(image.frame_width),
                    image.pixels.len() as u32,
                    image.alpha.len() as u32,
                ],
            );
            sections.put(&format!("img.{id}.pixels"), pixels_to_bytes(&image.pixels));
            sections.put(
                &format!("img.{id}.opaque"),
                pack_bits(&image.opaque),
            );
            sections.put(&format!("img.{id}.alfa"), image.alpha.clone());
        }
    }

    fn restore_surfaces_and_images(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let ids = reader.u32s("sup.ids")?;
        let mut surfaces = std::collections::HashMap::new();
        for id in ids {
            surfaces.insert(id, read_surface(reader, &format!("sup.{id}"))?);
        }
        let screen = read_surface(reader, "sup.tela")?;

        let ids = reader.u32s("img.ids")?;
        let mut images = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("img.{id}.meta"))?;
            if meta.len() != 5 {
                return Err(Error::Section {
                    name: format!("img.{id}.meta"),
                    reason: format!("expected 5 numbers and got {}", meta.len()),
                });
            }
            let (img_width, img_height, frame) = (meta[0], meta[1], meta[2]);
            let count = (img_width as usize) * (img_height as usize);
            if meta[3] as usize != count {
                return Err(Error::Section {
                    name: format!("img.{id}.meta"),
                    reason: format!(
                        "the image is {img_width}x{img_height} and claims to have {} pixels",
                        meta[3]
                    ),
                });
            }
            let pixels = bytes_to_pixels(&required_section(reader, &format!("img.{id}.pixels"))?);
            if pixels.len() != count {
                return Err(Error::Section {
                    name: format!("img.{id}.pixels"),
                    reason: format!("expected {count} pixels and got {}", pixels.len()),
                });
            }
            let opaque_bytes = required_section(reader, &format!("img.{id}.opaque"))?;
            if opaque_bytes.len() < count.div_ceil(8) {
                return Err(Error::Section {
                    name: format!("img.{id}.opaque"),
                    reason: format!(
                        "expected {} bytes of bits and got {}",
                        count.div_ceil(8),
                        opaque_bytes.len()
                    ),
                });
            }
            let alpha = required_section(reader, &format!("img.{id}.alfa"))?;
            if alpha.len() != meta[4] as usize {
                return Err(Error::Section {
                    name: format!("img.{id}.alfa"),
                    reason: format!("expected {} bytes of alpha and got {}", meta[4], alpha.len()),
                });
            }
            images.insert(
                id,
                std::rc::Rc::new(DecodedImage {
                    width: img_width,
                    height: img_height,
                    pixels,
                    opaque: unpack_bits(&opaque_bytes, count),
                    alpha,
                    frame_width: frame as u16,
                }),
            );
        }

        self.bitmaps = surfaces;
        self.screen = screen;
        self.images = images;
        Ok(())
    }
}

/// Saves a surface in two sections: the numbers and the pixels.
fn save_surface(sections: &mut Sections, prefix: &str, surface: &Framebuffer) {
    let (writes, generation) = (surface.writes(), surface.generation());
    let dirty = surface.dirty_rect().unwrap_or([0; 4]);
    sections.put_u32s(
        &format!("{prefix}.meta"),
        [
            surface.width(),
            surface.height(),
            writes as u32,
            (writes >> 32) as u32,
            generation as u32,
            (generation >> 32) as u32,
            u32::from(surface.dirty_rect().is_some()),
            dirty[0],
            dirty[1],
            dirty[2],
            dirty[3],
        ],
    );
    sections.put(&format!("{prefix}.pixels"), pixels_to_bytes(surface.pixels()));
}

/// Reads a surface saved by [`save_surface`].
fn read_surface(reader: &Reader<'_>, prefix: &str) -> Result<Framebuffer, Error> {
    let meta = reader.u32s(&format!("{prefix}.meta"))?;
    if meta.len() != 11 {
        return Err(Error::Section {
            name: format!("{prefix}.meta"),
            reason: format!("expected 11 numbers and got {}", meta.len()),
        });
    }
    let (width, height) = (meta[0], meta[1]);
    let count = (width as usize) * (height as usize);
    let pixels = bytes_to_pixels(&required_section(reader, &format!("{prefix}.pixels"))?);
    if pixels.len() != count {
        return Err(Error::Section {
            name: format!("{prefix}.pixels"),
            reason: format!("the surface is {width}x{height} and {} pixels came", pixels.len()),
        });
    }
    let mut surface = Framebuffer::new(width, height);
    let writes = u64::from(meta[2]) | (u64::from(meta[3]) << 32);
    let generation = u64::from(meta[4]) | (u64::from(meta[5]) << 32);
    let dirty = match meta[6] {
        0 => None,
        1 => Some([meta[7], meta[8], meta[9], meta[10]]),
        other => {
            return Err(Error::Section {
                name: format!("{prefix}.meta"),
                reason: format!("the dirty box field is {other}, and it is boolean"),
            })
        }
    };
    surface.restore_state(writes, generation, dirty, pixels);
    Ok(surface)
}

/// A section that must exist.
fn required_section<'a>(reader: &'a Reader<'a>, name: &str) -> Result<Vec<u8>, Error> {
    reader
        .section(name)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| Error::Section {
            name: name.to_string(),
            reason: "the section is not in the file".to_string(),
        })
}

/// RGB565 pixels as bytes, in reading order.
fn pixels_to_bytes(pixels: &[u16]) -> Vec<u8> {
    let mut output = Vec::with_capacity(pixels.len() * 2);
    for pixel in pixels {
        output.extend_from_slice(&pixel.to_le_bytes());
    }
    output
}

/// The way back from [`pixels_to_bytes`]. An odd size is a refusal.
fn bytes_to_pixels(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}


/// The scalars, the sets and the simple maps.
///
/// It is the part of the state that **has no shape of its own**: a seconds counter, the random
/// generator's state, which applet is running, which modules the shell has installed. On their
/// own, each would be ten lines of saving and ten of reading; together, they are one section of
/// numbers and a handful of maps.
///
/// ## What stays out, and it is a decision
///
/// - `stalled`: the engine is **stopped in the middle of a probe**. Saving from here and loading
///   later would promise a state that never existed — nobody knows what the probe had seen. Until
///   it goes in, the state is not complete, and the `retro_serialize_size` gate stays closed. It
///   is the kind of hole that must be stated, not covered up;
/// - `serial`: the `BufWriter` of an open file. Like the open files, it is rebuilt from disk —
///   but the log writer is not part of the game, and stays out;
/// - `installed_modules` and `enumerations`: lists of **text**. They need the same text handling
///   as the paths, and are left for the next pass;
/// - the clock in microseconds (`clock_us`) goes in separately, in the `relogio.us` section, not
///   here. **It is not the instruction counter divided by the rate**, as this paragraph used to
///   say: the clock is `clock_us + instructions / 528` (see `time.rs`), and `clock_us` is the idle
///   time the emulator skipped — the vsync, `skip_idle_time`, `MSLEEP` —, which in the measured
///   games is most of it. Without the section, restoring brought back the instructions and left
///   the skipped time at "now": in Double Dragon, a state saved at 20 265 ms came back at
///   25 408 ms, and the timers and the next vsync — saved in absolute time — all expired at once.
///   Measured on 2026-10-10, when rewind exposed the defect;
/// - `z_wheel` is a content path, and the core is the one that knows it.
impl<C: CpuBackend> Machine<C> {
    fn save_scalars_and_maps(&self, sections: &mut Sections) {
        let mut numbers: Vec<u32> = vec![
            self.epoch_seconds,
            self.random_state,
            self.nesting,
            self.system_mode,
            self.device_bitmap,
            self.display_target,
            self.current_applet,
            self.applet_class,
            self.egl_next_handle,
            // This used to be the OpenGL ES name counter. The largest name in use goes here: an
            // old engine that opens this state keeps counting from there, without repeating a live
            // name.
            self.texture_names
                .largest()
                .max(self.buffer_names.largest()),
            self.egl_swaps,
            self.gl_clears,
            self.unpack_alignment,
            self.updates_in_pass as u32,
            u32::from(self.applet_closed),
            u32::from(self.wheel_boot_skipped),
            // Presence booleans: 1 and 0, so the field can be checked on the way back.
            u32::from(self.pending_launch.is_some()),
            u32::from(self.gl_frame_writes.is_some()),
            u32::from(self.scale_source.is_some()),
            u32::from(self.current_thread.is_some()),
            u32::from(self.pending_activation.is_some()),
        ];
        numbers.push(self.next_vsync_us as u32);
        numbers.push((self.next_vsync_us >> 32) as u32);
        numbers.push(self.budget as u32);
        numbers.push((self.budget >> 32) as u32);
        numbers.push(self.next_serial as u32);
        numbers.push((self.next_serial >> 32) as u32);
        // And the values of the ones that exist.
        numbers.push(self.pending_launch.unwrap_or(0));
        numbers.push(self.gl_frame_writes.unwrap_or(0) as u32);
        numbers.push((self.gl_frame_writes.unwrap_or(0) >> 32) as u32);
        numbers.push(self.scale_source.unwrap_or((0, 0)).0 as u32);
        numbers.push(self.scale_source.unwrap_or((0, 0)).1 as u32);
        numbers.push(self.current_thread.unwrap_or(0));
        numbers.push(self.pending_activation.unwrap_or((0, 0)).0);
        numbers.push(self.pending_activation.unwrap_or((0, 0)).1);
        sections.put_u32s("esc.numeros", numbers);
        sections.put_u32s("esc.nomes_de_textura", self.texture_names.in_use());
        sections.put_u32s("esc.nomes_de_buffer", self.buffer_names.in_use());

        let mut sets: Vec<u32> = self.installed_applets.iter().copied().collect();
        sets.sort_unstable();
        sections.put_u32s("esc.applets", sets);
        let mut inherited: Vec<u32> = self.inherited_dibs.iter().copied().collect();
        inherited.sort_unstable();
        sections.put_u32s("esc.dib_herdados", inherited);
        let mut notifying: Vec<u32> = self.notifying_widgets.iter().copied().collect();
        notifying.sort_unstable();
        sections.put_u32s("esc.widgets_avisando", notifying);

        sections.put_map("esc.mif", self.mif_in_guest.iter().map(|(a, b)| (*a, *b)));
        sections.put_u32s(
            "esc.ext_modules",
            self.ext_modules.iter().map(|m| m.unwrap_or(u32::MAX)),
        );
        sections.put_map(
            "esc.rolagem",
            self.html_scroll.iter().map(|(a, b)| (*a, *b as u32)),
        );
        sections.put_map(
            "esc.rolagem_maxima",
            self.html_max_scroll.iter().map(|(a, b)| (*a, *b as u32)),
        );
        sections.put_triples(
            "esc.image_notify",
            self.image_notify
                .iter()
                .map(|(id, cb)| (*id, cb.function, cb.context)),
        );
        sections.put_records(
            "esc.dib_decodificador",
            self.decoder_dib
                .iter()
                .map(|(id, (a, b))| vec![*id, *a, *b])
                .collect::<Vec<_>>(),
        );
        sections.put_map(
            "esc.guest_dibs",
            self.guest_dibs.iter().map(|(dib, buffer)| (*dib, *buffer)),
        );
        sections.put_records(
            "esc.dib_publicado",
            self.published_dib
                .iter()
                .map(|(id, when)| vec![*id, *when as u32, (*when >> 32) as u32])
                .collect::<Vec<_>>(),
        );
        // The per-object number lists: `vectors` and `collections`. Each has a key, a block of
        // numbers and a loose field — the block goes as u32 bytes, and the field in the meta.
        sections.put_blocks(
            "esc.vetores",
            1,
            self.vectors
                .iter()
                .map(|(id, (values, extra))| {
                    let mut bytes = Vec::with_capacity(4 + values.len() * 4);
                    bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
                    for value in values {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                    bytes.extend_from_slice(&extra.to_le_bytes());
                    (vec![*id], bytes)
                })
                .collect::<Vec<_>>(),
        );
        sections.put_blocks(
            "esc.collections",
            1,
            self.collections
                .iter()
                .map(|(id, (values, extra))| {
                    let mut bytes = Vec::with_capacity(4 + values.len() * 4);
                    bytes.extend_from_slice(&(values.len() as u32).to_le_bytes());
                    for value in values {
                        bytes.extend_from_slice(&value.to_le_bytes());
                    }
                    bytes.extend_from_slice(&(*extra as u32).to_le_bytes());
                    (vec![*id], bytes)
                })
                .collect::<Vec<_>>(),
        );
    }


}

/// How many numbers the `esc.numeros` section has.
///
/// It is written here **and** checked on reading: a state saved with a different number of fields
/// is a refusal, not a shifted read. It is what forces whoever adds a field to touch the count —
/// and the test below demands that it be right.
const SCALARS: usize = 35;

impl<C: CpuBackend> Machine<C> {
    fn restore_scalars_and_maps(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let numbers = reader.u32s("esc.numeros")?;
        if numbers.len() != SCALARS {
            return Err(Error::Section {
                name: "esc.numeros".to_string(),
                reason: format!(
                    "the state has {} numbers and this engine version uses {SCALARS}",
                    numbers.len()
                ),
            });
        }
        let boolean = |index: usize, field: &str| -> Result<bool, Error> {
            match numbers[index] {
                0 => Ok(false),
                1 => Ok(true),
                other => Err(Error::Section {
                    name: "esc.numeros".to_string(),
                    reason: format!("the `{field}` field is {other}, and it is boolean"),
                }),
            }
        };
        let u64_at = |high: usize| -> u64 {
            u64::from(numbers[high]) | (u64::from(numbers[high + 1]) << 32)
        };

        let epoch_seconds = numbers[0];
        let random_state = numbers[1];
        let nesting = numbers[2];
        let system_mode = numbers[3];
        let device_bitmap = numbers[4];
        let display_target = numbers[5];
        let current_applet = numbers[6];
        let applet_class = numbers[7];
        let egl_next_handle = numbers[8];
        let old_max_name = numbers[9];
        // A state from before [`super::gl::GlNames`] only has the counter: everything it already
        // handed out stays in use, which is exactly what the counter guaranteed.
        let names = |section: &str| -> Result<super::gl::GlNames, Error> {
            Ok(match reader.section(section) {
                Some(_) => super::gl::GlNames::from(reader.u32s(section)?),
                None => super::gl::GlNames::from(1..=old_max_name),
            })
        };
        let texture_names = names("esc.nomes_de_textura")?;
        let buffer_names = names("esc.nomes_de_buffer")?;
        let egl_swaps = numbers[10];
        let gl_clears = numbers[11];
        let unpack_alignment = numbers[12];
        let updates_in_pass = numbers[13] as usize;
        let applet_closed = boolean(14, "applet_closed")?;
        let wheel_boot_skipped = boolean(15, "wheel_boot_skipped")?;
        let has_pending_launch = boolean(16, "pending_launch")?;
        let has_writes = boolean(17, "gl_frame_writes")?;
        let has_scale = boolean(18, "scale_source")?;
        let has_thread = boolean(19, "current_thread")?;
        let has_activation = boolean(20, "pending_activation")?;
        let next_vsync_us = u64_at(21);
        let budget = u64_at(23);
        let next_serial = u64_at(25);
        // **The indices at the end are counted from the start of the list**, not from the
        // previous field: this is where I got it wrong the first time, reading `pending_launch`
        // from the high half of `next_serial`. The test caught it because the value that came back
        // was another field's.
        let pending_launch = has_pending_launch.then_some(numbers[27]);
        let gl_frame_writes = has_writes.then(|| u64_at(28));
        let scale_source = has_scale.then(|| (numbers[30] as i32, numbers[31] as i32));
        let current_thread = has_thread.then_some(numbers[32]);
        let pending_activation = has_activation.then(|| (numbers[33], numbers[34]));

        // The maps and sets, read before anything is applied.
        let installed_applets: std::collections::HashSet<u32> =
            reader.u32s("esc.applets")?.into_iter().collect();
        let inherited_dibs: std::collections::HashSet<u32> =
            reader.u32s("esc.dib_herdados")?.into_iter().collect();
        let notifying_widgets: std::collections::HashSet<u32> =
            reader.u32s("esc.widgets_avisando")?.into_iter().collect();
        let mif_in_guest: std::collections::HashMap<u32, u32> =
            reader.pairs("esc.mif")?.into_iter().collect();
        let ext_modules: Vec<Option<u32>> = reader
            .u32s("esc.ext_modules")?
            .into_iter()
            .map(|m| (m != u32::MAX).then_some(m))
            .collect();
        let html_scroll: std::collections::HashMap<u32, usize> = reader
            .pairs("esc.rolagem")?
            .into_iter()
            .map(|(a, v)| (a, v as usize))
            .collect();
        let html_max_scroll: std::collections::HashMap<u32, usize> = reader
            .pairs("esc.rolagem_maxima")?
            .into_iter()
            .map(|(a, v)| (a, v as usize))
            .collect();
        let image_notify: std::collections::HashMap<u32, Callback> = reader
            .triples("esc.image_notify")?
            .into_iter()
            .map(|(id, function, context)| (id, Callback { function, context }))
            .collect();
        let mut decoder_dib = std::collections::HashMap::new();
        for logging in reader.records("esc.dib_decodificador", 3)? {
            decoder_dib.insert(logging[0], (logging[1], logging[2]));
        }
        // A state from before `IDISPLAY_CreateDIBitmap` existed has none of them.
        let guest_dibs: std::collections::HashMap<u32, u32> = match reader.section("esc.guest_dibs") {
            Some(_) => reader.pairs("esc.guest_dibs")?.into_iter().collect(),
            None => std::collections::HashMap::new(),
        };
        let mut published_dib = std::collections::HashMap::new();
        for logging in reader.records("esc.dib_publicado", 3)? {
            published_dib.insert(
                logging[0],
                u64::from(logging[1]) | (u64::from(logging[2]) << 32),
            );
        }
        let mut vectors = std::collections::HashMap::new();
        for (key_id, bytes) in reader.blocks("esc.vetores")? {
            vectors.insert(key_id[0], read_list_with_extra(&bytes, "esc.vetores")?);
        }
        let mut collections = std::collections::HashMap::new();
        for (key_id, bytes) in reader.blocks("esc.collections")? {
            let (values, extra) = read_list_with_extra(&bytes, "esc.collections")?;
            collections.insert(key_id[0], (values, extra as usize));
        }

        // Applying.
        self.epoch_seconds = epoch_seconds;
        self.random_state = random_state;
        self.nesting = nesting;
        self.system_mode = system_mode;
        self.device_bitmap = device_bitmap;
        self.display_target = display_target;
        self.current_applet = current_applet;
        self.applet_class = applet_class;
        self.egl_next_handle = egl_next_handle;
        self.texture_names = texture_names;
        self.buffer_names = buffer_names;
        self.egl_swaps = egl_swaps;
        self.gl_clears = gl_clears;
        self.unpack_alignment = unpack_alignment;
        self.updates_in_pass = updates_in_pass;
        self.applet_closed = applet_closed;
        self.wheel_boot_skipped = wheel_boot_skipped;
        self.pending_launch = pending_launch;
        self.gl_frame_writes = gl_frame_writes;
        self.scale_source = scale_source;
        self.current_thread = current_thread;
        self.pending_activation = pending_activation;
        self.next_vsync_us = next_vsync_us;
        self.budget = budget;
        self.next_serial = next_serial;
        self.installed_applets = installed_applets;
        self.inherited_dibs = inherited_dibs;
        self.notifying_widgets = notifying_widgets;
        self.mif_in_guest = mif_in_guest;
        self.ext_modules = ext_modules;
        self.html_scroll = html_scroll;
        self.html_max_scroll = html_max_scroll;
        self.image_notify = image_notify;
        self.decoder_dib = decoder_dib;
        self.guest_dibs = guest_dibs;
        self.published_dib = published_dib;
        self.vectors = vectors;
        self.collections = collections;
        Ok(())
    }
}

/// Reads a list of numbers saved as bytes, with the loose field at the end.
fn read_list_with_extra(bytes: &[u8], location: &str) -> Result<(Vec<u32>, u32), Error> {
    let malformed = |reason: String| Error::Section {
        name: location.to_string(),
        reason,
    };
    if bytes.len() < 8 {
        return Err(malformed(format!(
            "expected the count and the loose field, and has {} bytes",
            bytes.len()
        )));
    }
    let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != 4 + count * 4 + 4 {
        return Err(malformed(format!(
            "claims {count} values ({} bytes with the loose field) and has {}",
            4 + count * 4 + 4,
            bytes.len()
        )));
    }
    let values = bytes[4..4 + count * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let extra = u32::from_le_bytes([
        bytes[4 + count * 4],
        bytes[4 + count * 4 + 1],
        bytes[4 + count * 4 + 2],
        bytes[4 + count * 4 + 3],
    ]);
    Ok((values, extra))
}


/// The installed modules, the enumerations in progress and the **stop point**.
///
/// ## `stalled`: the declared hole, now covered
///
/// The engine can stop in the middle of a probe — an API call with no answer, a loop that went
/// past the ceiling. Earlier I had written that saving from here "would promise a state that never
/// existed". That was wrong: the state exists and is well defined (the guest stopped at a call
/// boundary), and what was missing was the encoding. It goes in here, with the same fields as
/// `Outcome` — what changes is that now the way back knows how to reject an outcome this version
/// does not know.
///
/// ## `interned`: the cache that does **not** go in
///
/// `interned` maps a static text (what `glGetString` returns) to the address where it was copied
/// in the guest. The keys are `&'static str`, and a save state does not bring static text back:
/// the pointers the game kept stay valid because the **memory** comes back, and the cache rebuilds
/// itself on the next query. The effect of not saving it is one extra copy per load (a few dozen
/// bytes), and that is stated instead of hidden.
impl<C: CpuBackend> Machine<C> {
    fn save_lists_and_stall(&self, sections: &mut Sections) {
        sections.put_u32s(
            "tex.modulos_ids",
            self.installed_modules.iter().map(|(id, _)| *id),
        );
        sections.put_texts(
            "tex.modulos_nomes",
            self.installed_modules
                .iter()
                .map(|(_, name)| name.clone())
                .collect::<Vec<_>>(),
        );
        sections.put_u32s("tex.enumeracoes_ids", self.enumerations.keys().copied());
        for (id, items) in &self.enumerations {
            sections.put_texts(
                &format!("tex.enumeracao.{id}"),
                items.iter().cloned().collect::<Vec<_>>(),
            );
        }

        // The stop point, with the variant's label and its fields.
        let (label, fields) = describe(&self.stalled);
        let mut numbers = vec![label];
        numbers.extend(fields);
        sections.put_u32s("parada", numbers);
    }

    fn restore_lists_and_stall(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let ids = reader.u32s("tex.modulos_ids")?;
        let names = reader.texts("tex.modulos_nomes")?;
        if ids.len() != names.len() {
            return Err(Error::Section {
                name: "tex.modulos_nomes".to_string(),
                reason: format!(
                    "the state has {} installed modules and {names_len} name(s)",
                    ids.len(),
                    names_len = names.len()
                ),
            });
        }
        let installed_modules: Vec<(u32, String)> =
            ids.into_iter().zip(names).collect();

        let ids = reader.u32s("tex.enumeracoes_ids")?;
        let mut enumerations = std::collections::HashMap::new();
        for id in ids {
            enumerations.insert(
                id,
                reader
                    .texts(&format!("tex.enumeracao.{id}"))?
                    .into_iter()
                    .collect(),
            );
        }

        let stop = reader.u32s("parada")?;
        let stalled = build(&stop)?;

        self.installed_modules = installed_modules;
        self.enumerations = enumerations;
        self.stalled = stalled;
        // The static text cache rebuilds itself; see the note at the top of this block.
        self.interned.clear();
        Ok(())
    }
}

/// The label of each [`Outcome`] variant in the file.
///
/// An explicit number, not the position in the enumeration: reordering the variants in the code
/// must not change the meaning of an already saved save state.
fn outcome_label(outcome: &Outcome) -> u32 {
    match outcome {
        Outcome::Returned { .. } => 1,
        Outcome::Unimplemented { .. } => 2,
        Outcome::Fault { .. } => 3,
        Outcome::Exception { .. } => 4,
        Outcome::Budget => 5,
        Outcome::CallLimit { .. } => 6,
    }
}

/// The outcome as a label and fields.
fn describe(outcome: &Option<Outcome>) -> (u32, Vec<u32>) {
    match outcome {
        None => (0, Vec::new()),
        Some(recorded_outcome) => {
            let label = outcome_label(recorded_outcome);
            let fields = match recorded_outcome {
                Outcome::Returned { code } => vec![*code],
                Outcome::Unimplemented { addr, args, caller } => {
                    vec![*addr, args[0], args[1], args[2], args[3], *caller]
                }
                Outcome::Fault { addr, pc, lr } => vec![*addr, *pc, *lr],
                Outcome::Exception { pc } => vec![*pc],
                Outcome::Budget => Vec::new(),
                Outcome::CallLimit { calls } => {
                    vec![*calls as u32, (*calls >> 32) as u32]
                }
            };
            (label, fields)
        }
    }
}

/// The way back from [`describe`]. An unknown label is a refusal, and so is a missing field.
fn build(numbers: &[u32]) -> Result<Option<Outcome>, Error> {
    let error = |reason: String| Error::Section {
        name: "parada".to_string(),
        reason,
    };
    let Some((&label, fields)) = numbers.split_first() else {
        return Err(error("the section is empty".to_string()));
    };
    let needed = |count: usize| -> Result<(), Error> {
        if fields.len() < count {
            return Err(error(format!(
                "outcome {label} needs {count} field(s) and got {}",
                fields.len()
            )));
        }
        Ok(())
    };
    Ok(match label {
        0 => None,
        1 => {
            needed(1)?;
            Some(Outcome::Returned { code: fields[0] })
        }
        2 => {
            needed(6)?;
            Some(Outcome::Unimplemented {
                addr: fields[0],
                args: [fields[1], fields[2], fields[3], fields[4]],
                caller: fields[5],
            })
        }
        3 => {
            needed(3)?;
            Some(Outcome::Fault {
                addr: fields[0],
                pc: fields[1],
                lr: fields[2],
            })
        }
        4 => {
            needed(1)?;
            Some(Outcome::Exception { pc: fields[0] })
        }
        5 => Some(Outcome::Budget),
        6 => {
            needed(2)?;
            Some(Outcome::CallLimit {
                calls: u64::from(fields[0]) | (u64::from(fields[1]) << 32),
            })
        }
        other => {
            return Err(error(format!(
                "the state holds outcome {other}, which this engine does not know"
            )))
        }
    })
}


/// The remaining object tables: cipher, decompression, peek, clip, value model, PCM stream,
/// pending deliveries and the media state.
///
/// They all use the same three format helpers — numbers, fixed-size records and byte blocks —,
/// and the only difference between them is the shape of what they hold. That is the advantage of
/// having started with the format: this is the part of the work where each new table costs a few
/// lines.
///
/// ## What does **not** go in, and it is a decision
///
/// - `MediaPayload`: holds an `Arc<Sound>` — an already decoded WAV, the size of the file. It is a
///   cache of what is on disk, and dropping it makes the sound reload the next time the game asks
///   for it. What is lost is the sound that **was playing** at the moment of the save: it comes
///   back from the start, not from the middle. That is stated;
/// - `HashState`: an `Md5` in the middle of a computation. The implementation does not expose its
///   internal state, and keeping the partial result on the side would mean inventing a format for
///   a third-party algorithm. A game that saves a state exactly between two `Update` calls of an
///   MD5 loses that computation — and it is the only case;
/// - `Widget`: the largest of the missing ones, with maps inside maps and text. It is left for the
///   next pass, and it is what remains before the gate can open.
impl<C: CpuBackend> Machine<C> {
    fn save_remaining_tables(&self, sections: &mut Sections) {
        sections.put_u32s("cif.ids", self.ciphers.keys().copied());
        for (id, cipher) in &self.ciphers {
            sections.put_u32s(
                &format!("cif.{id}.meta"),
                [u32::from(cipher.key.is_some()), cipher.padding],
            );
            let mut key_ids = Vec::with_capacity(2 * AES_BLOCK);
            if let Some(key_id) = cipher.key {
                key_ids.extend_from_slice(&key_id);
            } else {
                key_ids.extend(std::iter::repeat_n(0u8, AES_BLOCK));
            }
            key_ids.extend_from_slice(&cipher.iv);
            sections.put(&format!("cif.{id}.chaves"), key_ids);
            sections.put(&format!("cif.{id}.pendente"), cipher.pending.clone());
        }

        sections.put_u32s("zip.ids", self.unzips.keys().copied());
        for (id, decompressor) in &self.unzips {
            sections.put_u32s(
                &format!("zip.{id}.meta"),
                [
                    decompressor.source,
                    decompressor.position as u32,
                    u32::from(decompressor.expanded),
                ],
            );
            sections.put(&format!("zip.{id}.saida"), decompressor.output.clone());
        }

        sections.put_u32s("peek.ids", self.peeks.keys().copied());
        for (id, peek) in &self.peeks {
            sections.put_u32s(
                &format!("peek.{id}.meta"),
                [peek.position as u32, peek.buffer],
            );
            sections.put(&format!("peek.{id}.bytes"), peek.bytes.clone());
        }

        sections.put_u32s("rec.ids", self.image_clips.keys().copied());
        for (id, clip) in &self.image_clips {
            let (has, width, height) = match clip.size {
                Some((l, a)) => (1u32, l as u32, a as u32),
                None => (0, 0, 0),
            };
            sections.put_u32s(
                &format!("rec.{id}.meta"),
                [
                    clip.x as u32,
                    clip.y as u32,
                    has,
                    width,
                    height,
                    u32::from(clip.transparent),
                ],
            );
        }

        sections.put_u32s("mod.ids", self.value_models.keys().copied());
        for (id, model) in &self.value_models {
            sections.put_u32s(
                &format!("mod.{id}.meta"),
                [model.value, model.size],
            );
            sections.put_records(
                &format!("mod.{id}.ouvintes"),
                model
                    .listeners
                    .iter()
                    .map(|(a, b, c)| vec![*a, *b, *c])
                    .collect::<Vec<_>>(),
            );
        }

        sections.put_u32s("pcm.ids", self.pcm_streams.keys().copied());
        for (id, stream) in &self.pcm_streams {
            sections.put_u32s(
                &format!("pcm.{id}.meta"),
                [
                    stream.source,
                    stream.rate,
                    u32::from(stream.channels),
                    u32::from(stream.bits),
                    u32::from(stream.unsigned),
                    u32::from(stream.playing),
                    stream.start_us as u32,
                    (stream.start_us >> 32) as u32,
                    stream.frames_read as u32,
                    (stream.frames_read >> 32) as u32,
                ],
            );
        }

        sections.put_records(
            "blit.registros",
            self.pending_blits
                .iter()
                .map(|blit| {
                    vec![
                        blit.image,
                        blit.target,
                        blit.x as u32,
                        blit.y as u32,
                        u32::from(blit.frame.is_some()),
                        blit.frame.unwrap_or(0),
                        blit.src_x as u32,
                        blit.src_y as u32,
                        blit.width,
                        blit.height,
                        blit.rop,
                    ]
                })
                .collect::<Vec<_>>(),
        );
        sections.put_records(
            "blit.superficies",
            self.pending_surface_blits
                .iter()
                .map(|blit| {
                    vec![
                        blit.source,
                        blit.target,
                        blit.x as u32,
                        blit.y as u32,
                        blit.width,
                        blit.height,
                        blit.rop,
                    ]
                })
                .collect::<Vec<_>>(),
        );

        sections.put_u32s("mid.ids", self.media.keys().copied());
        for (id, media) in &self.media {
            sections.put_u32s(
                &format!("mid.{id}.meta"),
                [
                    media.state,
                    media.payload as u32,
                    (media.payload >> 32) as u32,
                    media.pending.0,
                    media.pending.1,
                    media.buffer.0,
                    media.buffer.1,
                    u32::from(media.play_on_load),
                    media.volume,
                    media.repeat,
                    u32::from(media.muted),
                    media.notify.function,
                    media.notify.context,
                    media.ends_us as u32,
                    (media.ends_us >> 32) as u32,
                ],
            );
        }
    }

    fn restore_remaining_tables(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let ids = reader.u32s("cif.ids")?;
        let mut ciphers = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("cif.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Error::Section {
                    name: format!("cif.{id}.meta"),
                    reason: format!("expected 2 numbers and got {}", meta.len()),
                });
            }
            let key_ids = required_section(reader, &format!("cif.{id}.chaves"))?;
            if key_ids.len() != 2 * AES_BLOCK {
                return Err(Error::Section {
                    name: format!("cif.{id}.chaves"),
                    reason: format!(
                        "expected {} bytes and got {}",
                        2 * AES_BLOCK,
                        key_ids.len()
                    ),
                });
            }
            let mut iv = [0u8; AES_BLOCK];
            iv.copy_from_slice(&key_ids[AES_BLOCK..]);
            let key = match meta[0] {
                0 => None,
                1 => {
                    let mut key_id = [0u8; AES_BLOCK];
                    key_id.copy_from_slice(&key_ids[..AES_BLOCK]);
                    Some(key_id)
                }
                other => {
                    return Err(Error::Section {
                        name: format!("cif.{id}.meta"),
                        reason: format!("the `key` field is {other}, and it is boolean"),
                    })
                }
            };
            ciphers.insert(
                id,
                CipherState {
                    key,
                    iv,
                    padding: meta[1],
                    pending: required_section(reader, &format!("cif.{id}.pendente"))?,
                },
            );
        }

        let ids = reader.u32s("zip.ids")?;
        let mut unzips = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("zip.{id}.meta"))?;
            if meta.len() != 3 {
                return Err(Error::Section {
                    name: format!("zip.{id}.meta"),
                    reason: format!("expected 3 numbers and got {}", meta.len()),
                });
            }
            unzips.insert(
                id,
                UnzipState {
                    source: meta[0],
                    output: required_section(reader, &format!("zip.{id}.saida"))?,
                    position: meta[1] as usize,
                    expanded: meta[2] != 0,
                },
            );
        }

        let ids = reader.u32s("peek.ids")?;
        let mut peeks = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("peek.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Error::Section {
                    name: format!("peek.{id}.meta"),
                    reason: format!("expected 2 numbers and got {}", meta.len()),
                });
            }
            peeks.insert(
                id,
                Peek {
                    bytes: required_section(reader, &format!("peek.{id}.bytes"))?,
                    position: meta[0] as usize,
                    buffer: meta[1],
                },
            );
        }

        let ids = reader.u32s("rec.ids")?;
        let mut image_clips = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("rec.{id}.meta"))?;
            if meta.len() != 6 {
                return Err(Error::Section {
                    name: format!("rec.{id}.meta"),
                    reason: format!("expected 6 numbers and got {}", meta.len()),
                });
            }
            let size = match meta[2] {
                0 => None,
                1 => Some((meta[3] as i32, meta[4] as i32)),
                other => {
                    return Err(Error::Section {
                        name: format!("rec.{id}.meta"),
                        reason: format!("the `size` field is {other}, and it is boolean"),
                    })
                }
            };
            image_clips.insert(
                id,
                ImageClip {
                    x: meta[0] as i32,
                    y: meta[1] as i32,
                    size,
                    transparent: meta[5] != 0,
                },
            );
        }

        let ids = reader.u32s("mod.ids")?;
        let mut value_models = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("mod.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Error::Section {
                    name: format!("mod.{id}.meta"),
                    reason: format!("expected 2 numbers and got {}", meta.len()),
                });
            }
            let listeners = reader
                .records(&format!("mod.{id}.ouvintes"), 3)?
                .into_iter()
                .map(|r| (r[0], r[1], r[2]))
                .collect();
            value_models.insert(
                id,
                ValueModel {
                    value: meta[0],
                    size: meta[1],
                    listeners,
                },
            );
        }

        let ids = reader.u32s("pcm.ids")?;
        let mut pcm_streams = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("pcm.{id}.meta"))?;
            if meta.len() != 10 {
                return Err(Error::Section {
                    name: format!("pcm.{id}.meta"),
                    reason: format!("expected 10 numbers and got {}", meta.len()),
                });
            }
            let smallest = |index: usize, field: &str| -> Result<u16, Error> {
                u16::try_from(meta[index]).map_err(|_| Error::Section {
                    name: format!("pcm.{id}.meta"),
                    reason: format!("the `{field}` field is {}, which does not fit in a u16", meta[index]),
                })
            };
            pcm_streams.insert(
                id,
                PcmStream {
                    source: meta[0],
                    rate: meta[1],
                    channels: smallest(2, "channels")?,
                    bits: smallest(3, "bits")?,
                    unsigned: meta[4] != 0,
                    // Not saved: a restored stream goes back to requesting by the clock, with no
                    // block.
                    block: 0,
                    start_us: u64::from(meta[6]) | (u64::from(meta[7]) << 32),
                    frames_read: u64::from(meta[8]) | (u64::from(meta[9]) << 32),
                    playing: meta[5] != 0,
                    // The end notice belongs to the log, not to the game state: a restored save
                    // may notify again, and that is better than hiding the end of a stream.
                    logged_end: false,
                },
            );
        }

        let mut pending_blits = Vec::new();
        for logging in reader.records("blit.registros", 11)? {
            let frame = match logging[4] {
                0 => None,
                1 => Some(logging[5]),
                other => {
                    return Err(Error::Section {
                        name: "blit.registros".to_string(),
                        reason: format!("the `frame` field is {other}, and it is boolean"),
                    })
                }
            };
            pending_blits.push(PendingBlit {
                image: logging[0],
                target: logging[1],
                x: logging[2] as i32,
                y: logging[3] as i32,
                src_x: logging[6] as i32,
                src_y: logging[7] as i32,
                width: logging[8],
                height: logging[9],
                rop: logging[10],
                frame,
            });
        }
        let mut pending_surface_blits = Vec::new();
        for logging in reader.records("blit.superficies", 7)? {
            pending_surface_blits.push(PendingSurfaceBlit {
                source: logging[0],
                target: logging[1],
                x: logging[2] as i32,
                y: logging[3] as i32,
                width: logging[4],
                height: logging[5],
                rop: logging[6],
            });
        }

        let ids = reader.u32s("mid.ids")?;
        let mut media = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("mid.{id}.meta"))?;
            if meta.len() != 15 {
                return Err(Error::Section {
                    name: format!("mid.{id}.meta"),
                    reason: format!("expected 15 numbers and got {}", meta.len()),
                });
            }
            media.insert(
                id,
                MediaState {
                    state: meta[0],
                    payload: u64::from(meta[1]) | (u64::from(meta[2]) << 32),
                    pending: (meta[3], meta[4]),
                    buffer: (meta[5], meta[6]),
                    play_on_load: meta[7] != 0,
                    volume: meta[8],
                    repeat: meta[9],
                    muted: meta[10] != 0,
                    notify: Callback {
                        function: meta[11],
                        context: meta[12],
                    },
                    ends_us: u64::from(meta[13]) | (u64::from(meta[14]) << 32),
                },
            );
        }

        self.ciphers = ciphers;
        self.unzips = unzips;
        self.peeks = peeks;
        self.image_clips = image_clips;
        self.value_models = value_models;
        self.pcm_streams = pcm_streams;
        self.pending_blits = pending_blits;
        self.pending_surface_blits = pending_surface_blits;
        self.media = media;
        Ok(())
    }
}


/// What is left without a shape of its own: scalars, buffers, the `IGraphics` and `IGL` state, the
/// keyboards and the threads.
///
/// ## What still stays out, and why
///
/// - **`widgets`**: the largest. Each widget has maps of children, properties and models, plus
///   text. It needs a composite key scheme that does not exist in the format yet;
/// - **`gl`**: the rasterizer object, with the guest's GL state machine — matrices, current color,
///   stacks. It is real state (**the game may be in the middle of a `glBegin`**), and so it does
///   not go in just any way: it needs its own encoding, with the same care as the surfaces;
/// - **`decoders`** and **`databases`**: an image decoder in the middle of a stream and an open
///   SQL database. Both are library state, and each calls for the same decision as the `Md5` —
///   expose the internals, or accept losing what was in progress;
/// - **`vfs`** and **`resources`**: they are a **cache of what is on disk** (the device's file
///   system and the package's resources). Dropping them makes them reload, and that is right:
///   saving them would duplicate the package inside the save state;
/// - **`audio`**: the host `Mixer`. What the game asked for is in `media`, and the mixer is
///   rebuilt from it on restore: see [`Machine::rebuild_sound`];
/// - **`ignored_gl`**, **`api_time`**, **`profiling_api`**, **`tracing`**, **`fault_*`**:
///   instrumentation and diagnostics, for the usual reason — saving them would make two save
///   states legitimately different.
impl<C: CpuBackend> Machine<C> {
    /// The scalars and the small lists.
    fn rest_numbers(&self) -> Vec<u32> {
        let mut n: Vec<u32> = vec![
            self.file_error,
            self.surface_manip,
            self.imageon_ext,
            self.gles11_ext,
            self.gles10_ext,
            self.egl_get_power_level,
            self.egl_oes_swap_interval,
            self.egl_get_color_buffer,
            self.gles11_ext_pak,
            u32::from(self.boomerang_sequence),
            self.spin_polls,
            self.applet_enumeration as u32,
            u32::from(self.bridge),
            self.stream_buffer,
            self.media_notify_block,
            u32::from(self.widgets_dumped),
            self.painted_form,
            self.egl_error,
            u32::from(self.egl_initial_viewport),
            self.gles_object,
            self.egl_surface,
            self.egl_context,
            self.gl_array_buffer,
            self.gl_element_buffer,
            self.last_draw_us as u32,
            (self.last_draw_us >> 32) as u32,
            self.last_boomerang_report_us as u32,
            (self.last_boomerang_report_us >> 32) as u32,
            self.last_boomerang_packet_us as u32,
            (self.last_boomerang_packet_us >> 32) as u32,
            self.calibrations.0,
            self.calibrations.1,
            u32::from(self.pending_end.is_some()),
            self.pending_end.unwrap_or(0),
            u32::from(self.pending_response.is_some()),
            self.pending_response.unwrap_or((0, 0, 0)).0,
            self.pending_response.unwrap_or((0, 0, 0)).1,
            self.pending_response.unwrap_or((0, 0, 0)).2,
            u32::from(self.egl_color_dimensions.is_some()),
            self.egl_color_dimensions.unwrap_or((0, 0)).0 as u32,
            self.egl_color_dimensions.unwrap_or((0, 0)).1 as u32,
            self.egl_color_buffer.0,
            self.egl_color_buffer.1 as u32,
            u32::from(self.clip.is_some()),
            u32::from(self.network),
            u32::from(self.network_to.is_some()),
        ];
        // The signed rectangles and the `Rgb`s of IGraphics.
        if let Some(clip) = self.clip {
            n.extend([
                clip.x as u32,
                clip.y as u32,
                clip.width as u32,
                clip.height as u32,
            ]);
        } else {
            n.extend([0, 0, 0, 0]);
        }
        n.extend([
            u32::from(self.graphics.stroke.r),
            u32::from(self.graphics.stroke.g),
            u32::from(self.graphics.stroke.b),
            u32::from(self.graphics.fill.r),
            u32::from(self.graphics.fill.g),
            u32::from(self.graphics.fill.b),
            u32::from(self.graphics.background.r),
            u32::from(self.graphics.background.g),
            u32::from(self.graphics.background.b),
            u32::from(self.graphics.fill_mode),
            u32::from(self.graphics.point_size),
            self.graphics.origin.0 as u32,
            self.graphics.origin.1 as u32,
        ]);
        // The `IGraphics` palette: seventeen `Rgb`s, three bytes each.
        for color in &self.colors {
            n.extend([u32::from(color.r), u32::from(color.g), u32::from(color.b)]);
        }
        // The two controllers, each with its buttons and its four axes.
        for pad in &self.pads {
            n.push(pad.buttons);
            n.extend(pad.axes.iter().map(|axis| *axis as u32));
        }
        // The Boomerang's motion, as bits: they are `f32`.
        for acceleration in &self.motion {
            for component in acceleration {
                n.push(component.to_bits());
            }
        }
        // The five GL client arrays: six numbers each.
        for pointer in [
            &self.gl_vertices,
            &self.gl_colors,
            &self.gl_texcoords,
            &self.gl_texcoords1,
            &self.gl_normals,
        ] {
            n.extend([
                pointer.size,
                pointer.kind,
                pointer.stride,
                pointer.address,
                u32::from(pointer.enabled),
                pointer.buffer,
            ]);
        }
        for component in self.gl_current_normal {
            n.push(component.to_bits());
        }
        n
    }

    fn save_the_rest(&self, sections: &mut Sections) {
        sections.put_u32s("resto.numeros", self.rest_numbers());
        sections.put_text("resto.network_to", self.network_to.as_deref().unwrap_or(""));
        let mut key_events: Vec<u32> = self.scroll_keys.iter().copied().collect();
        key_events.sort_unstable();
        sections.put_u32s("resto.teclas_rolagem", key_events);
        let mut got: Vec<u32> = self.resources_read.iter().map(|r| u32::from(*r)).collect();
        got.sort_unstable();
        sections.put_u32s("resto.recursos_lidos", got);
        sections.put_records(
            "resto.avisos_de_midia",
            self.media_notify_queue
                .iter()
                .map(|(a, b, c, cb)| vec![*a, *b, *c, cb.function, cb.context])
                .collect::<Vec<_>>(),
        );
        sections.put_u32s("resto.avisos_de_imagem", self.image_notify_queue.iter().copied());
        sections.put_blocks(
            "resto.gl_buffers",
            1,
            self.gl_buffers
                .iter()
                .map(|(name, bytes)| (vec![*name], bytes.clone()))
                .collect::<Vec<_>>(),
        );
        sections.put("resto.egl_bytes", self.egl_color_bytes.clone());
        sections.put("resto.egl_readback", self.egl_color_readback.clone());

        // The threads: the fourteen context registers, the flags and whoever is waiting for each.
        sections.put_u32s("th.ids", self.threads.keys().copied());
        for (id, thread) in &self.threads {
            let mut numbers = vec![
                thread.stack,
                thread.resume_cb,
                thread.resume_pc,
                u32::from(thread.started),
                u32::from(thread.suspended),
                u32::from(thread.finished),
                thread.exit_code,
            ];
            numbers.extend(thread.context);
            sections.put_u32s(&format!("th.{id}.meta"), numbers);
            sections.put_records(
                &format!("th.{id}.joiners"),
                thread
                    .joiners
                    .iter()
                    .map(|(cb, value)| vec![cb.function, cb.context, *value])
                    .collect::<Vec<_>>(),
            );
        }
        sections.put_map("th.resume", self.resume_callbacks.iter().map(|(a, b)| (*a, *b)));
        sections.put_u32s("th.pendentes", self.pending_threads.iter().copied());

        // The `Update` frames: they are surfaces, and go through the same routine as surfaces.
        sections.put_u32s("upd.quantos", [self.update_frames.len() as u32]);
        for (index, frame) in self.update_frames.iter().enumerate() {
            save_surface(sections, &format!("upd.{index}"), frame);
        }
    }

    /// Reads and applies the rest: scalars, buffers, `IGraphics`, `IGL`, keyboards and threads.
    fn restore_the_rest(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let n = reader.u32s("resto.numeros")?;
        if n.len() != REST_LEN {
            return Err(Error::Section {
                name: "resto.numeros".to_string(),
                reason: format!(
                    "the state has {} numbers and this engine version uses {REST_LEN}",
                    n.len()
                ),
            });
        }
        let boolean = |index: usize, field: &str| -> Result<bool, Error> {
            match n[index] {
                0 => Ok(false),
                1 => Ok(true),
                other => Err(Error::Section {
                    name: "resto.numeros".to_string(),
                    reason: format!("the `{field}` field is {other}, and it is boolean"),
                }),
            }
        };
        let u64_at = |index: usize| u64::from(n[index]) | (u64::from(n[index + 1]) << 32);

        // The first 49; the others come afterwards, in fixed-size blocks.
        let file_error = n[0];
        let surface_manip = n[1];
        let imageon_ext = n[2];
        let gles11_ext = n[3];
        let gles10_ext = n[4];
        let egl_get_power_level = n[5];
        let egl_oes_swap_interval = n[6];
        let egl_get_color_buffer = n[7];
        let gles11_ext_pak = n[8];
        let boomerang_sequence = n[9] as u8;
        let spin_polls = n[10];
        let applet_enumeration = n[11] as usize;
        let bridge = boolean(12, "bridge")?;
        let stream_buffer = n[13];
        let media_notify_block = n[14];
        let widgets_dumped = boolean(15, "widgets_dumped")?;
        let painted_form = n[16];
        let egl_error = n[17];
        let egl_initial_viewport = boolean(18, "egl_initial_viewport")?;
        let gles_object = n[19];
        let egl_surface = n[20];
        let egl_context = n[21];
        let gl_array_buffer = n[22];
        let gl_element_buffer = n[23];
        let last_draw_us = u64_at(24);
        let last_boomerang_report_us = u64_at(26);
        let last_boomerang_packet_us = u64_at(28);
        let calibrations = (n[30], n[31]);
        let pending_end = boolean(32, "pending_end")?.then_some(n[33]);
        let pending_response = boolean(34, "pending_response")?.then(|| (n[35], n[36], n[37]));
        let egl_color_dimensions =
            boolean(38, "egl_color_dimensions")?.then(|| (n[39] as usize, n[40] as usize));
        let egl_color_buffer = (n[41], n[42] as usize);
        // The order here **is** the writer's, not the one that looks more natural: the two network
        // fields come before the rectangle. When I wrote the reader in the "pretty" order, the
        // values came out swapped — `network` came back with the rectangle's width. The test
        // caught it.
        let clip = boolean(43, "clip")?.then(|| Rect {
            x: n[46] as i16,
            y: n[47] as i16,
            width: n[48] as i16,
            height: n[49] as i16,
        });
        let network = boolean(44, "network")?;
        let has_network_to = boolean(45, "network_to")?;
        let color = |base: usize| Rgb {
            r: n[base] as u8,
            g: n[base + 1] as u8,
            b: n[base + 2] as u8,
        };
        let graphics = GraphicsState {
            stroke: color(50),
            fill: color(53),
            background: color(56),
            fill_mode: n[59] != 0,
            point_size: n[60] as u8,
            origin: (n[61] as i32, n[62] as i32),
        };
        let mut colors = default_colors();
        for (index, dest) in colors.iter_mut().enumerate() {
            let base = 63 + index * 3;
            *dest = Rgb {
                r: n[base] as u8,
                g: n[base + 1] as u8,
                b: n[base + 2] as u8,
            };
        }
        let pads_base = 63 + CLR_COUNT * 3;
        let mut pads = [Pad::default(); crate::input::PORTS];
        for (index, dest) in pads.iter_mut().enumerate() {
            let base = pads_base + index * 5;
            dest.buttons = n[base];
            for (axis, value) in dest.axes.iter_mut().enumerate() {
                *value = n[base + 1 + axis] as i32;
            }
        }
        let motion_base = pads_base + crate::input::PORTS * 5;
        let mut motion = [[0.0f32; 3]; crate::input::PORTS];
        for (index, acceleration) in motion.iter_mut().enumerate() {
            for (component, value) in acceleration.iter_mut().enumerate() {
                *value = f32::from_bits(n[motion_base + index * 3 + component]);
            }
        }
        let pointers_base = motion_base + crate::input::PORTS * 3;
        let mut pointers = [ArrayPointer::default(); 5];
        for (index, dest) in pointers.iter_mut().enumerate() {
            let base = pointers_base + index * 6;
            *dest = ArrayPointer {
                size: n[base],
                kind: n[base + 1],
                stride: n[base + 2],
                address: n[base + 3],
                enabled: n[base + 4] != 0,
                buffer: n[base + 5],
            };
        }
        let normal_base = pointers_base + 30;
        let gl_current_normal = [
            f32::from_bits(n[normal_base]),
            f32::from_bits(n[normal_base + 1]),
            f32::from_bits(n[normal_base + 2]),
        ];

        // Lists and blocks, read before applying.
        let network_to = {
            let text = reader.text("resto.network_to")?;
            has_network_to.then_some(text)
        };
        let scroll_keys: std::collections::HashSet<u32> =
            reader.u32s("resto.teclas_rolagem")?.into_iter().collect();
        let resources_read: std::collections::BTreeSet<u16> = reader
            .u32s("resto.recursos_lidos")?
            .into_iter()
            .map(|r| r as u16)
            .collect();
        let image_notify_queue = reader.u32s("resto.avisos_de_imagem")?;
        let media_notify_queue: Vec<(u32, u32, u32, Callback)> = reader
            .records("resto.avisos_de_midia", 5)?
            .into_iter()
            .map(|r| {
                (
                    r[0],
                    r[1],
                    r[2],
                    Callback {
                        function: r[3],
                        context: r[4],
                    },
                )
            })
            .collect();
        let gl_buffers: std::collections::HashMap<u32, Vec<u8>> =
            reader.blocks("resto.gl_buffers")?.into_iter().map(|(c, b)| (c[0], b)).collect();
        let egl_color_bytes = required_section(reader, "resto.egl_bytes")?;
        let egl_color_readback = required_section(reader, "resto.egl_readback")?;

        let ids = reader.u32s("th.ids")?;
        let mut threads = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("th.{id}.meta"))?;
            if meta.len() != 21 {
                return Err(Error::Section {
                    name: format!("th.{id}.meta"),
                    reason: format!("expected 21 numbers and got {}", meta.len()),
                });
            }
            let mut context = [0u32; 14];
            context.copy_from_slice(&meta[7..21]);
            let joiners = reader
                .records(&format!("th.{id}.joiners"), 3)?
                .into_iter()
                .map(|r| {
                    (
                        Callback {
                            function: r[0],
                            context: r[1],
                        },
                        r[2],
                    )
                })
                .collect();
            threads.insert(
                id,
                ThreadState {
                    stack: meta[0],
                    resume_cb: meta[1],
                    resume_pc: meta[2],
                    started: meta[3] != 0,
                    suspended: meta[4] != 0,
                    finished: meta[5] != 0,
                    exit_code: meta[6],
                    context,
                    joiners,
                },
            );
        }
        let resume_callbacks: std::collections::HashMap<u32, u32> =
            reader.pairs("th.resume")?.into_iter().collect();
        let pending_threads = reader.u32s("th.pendentes")?;

        let frame_count = reader.u32s("upd.quantos")?;
        if frame_count.len() != 1 {
            return Err(Error::Section {
                name: "upd.quantos".to_string(),
                reason: format!("expected 1 number and got {}", frame_count.len()),
            });
        }
        let mut update_frames = std::collections::VecDeque::new();
        for index in 0..frame_count[0] as usize {
            update_frames.push_back(read_surface(reader, &format!("upd.{index}"))?);
        }

        // Applying.
        self.file_error = file_error;
        self.surface_manip = surface_manip;
        self.imageon_ext = imageon_ext;
        self.gles11_ext = gles11_ext;
        self.gles10_ext = gles10_ext;
        self.egl_get_power_level = egl_get_power_level;
        self.egl_oes_swap_interval = egl_oes_swap_interval;
        self.egl_get_color_buffer = egl_get_color_buffer;
        self.gles11_ext_pak = gles11_ext_pak;
        self.boomerang_sequence = boomerang_sequence;
        self.spin_polls = spin_polls;
        self.applet_enumeration = applet_enumeration;
        self.bridge = bridge;
        self.stream_buffer = stream_buffer;
        self.media_notify_block = media_notify_block;
        self.widgets_dumped = widgets_dumped;
        self.painted_form = painted_form;
        self.egl_error = egl_error;
        self.egl_initial_viewport = egl_initial_viewport;
        self.gles_object = gles_object;
        self.egl_surface = egl_surface;
        self.egl_context = egl_context;
        self.gl_array_buffer = gl_array_buffer;
        self.gl_element_buffer = gl_element_buffer;
        self.last_draw_us = last_draw_us;
        self.last_boomerang_report_us = last_boomerang_report_us;
        self.last_boomerang_packet_us = last_boomerang_packet_us;
        self.calibrations = calibrations;
        self.pending_end = pending_end;
        self.pending_response = pending_response;
        self.egl_color_dimensions = egl_color_dimensions;
        self.egl_color_buffer = egl_color_buffer;
        self.clip = clip;
        self.network = network;
        self.network_to = network_to;
        self.graphics = graphics;
        self.colors = colors;
        self.pads = pads;
        self.motion = motion;
        self.gl_vertices = pointers[0];
        self.gl_colors = pointers[1];
        self.gl_texcoords = pointers[2];
        self.gl_texcoords1 = pointers[3];
        self.gl_normals = pointers[4];
        self.gl_current_normal = gl_current_normal;
        self.scroll_keys = scroll_keys;
        self.resources_read = resources_read;
        self.media_notify_queue = media_notify_queue;
        self.image_notify_queue = image_notify_queue;
        self.gl_buffers = gl_buffers;
        self.egl_color_bytes = egl_color_bytes;
        self.egl_color_readback = egl_color_readback;
        self.threads = threads;
        self.resume_callbacks = resume_callbacks;
        self.pending_threads = pending_threads;
        self.update_frames = update_frames;
        Ok(())
    }
}

/// How many numbers the `resto.numeros` section has.
///
/// Checked on reading, like `SCALARS`: a state with a different number of fields is a refusal,
/// not a shifted read. The test demands that the count be right.
const REST_LEN: usize = 63 + CLR_COUNT * 3 + crate::input::PORTS * 5 + crate::input::PORTS * 3 + 30 + 3;


/// **The `IWidget` widgets** — the last big table.
///
/// Each widget has three maps of numbers inside (children, properties and models), two pairs of
/// coordinates, text, and three function-and-context pairs (handler, drawing and releasers).
///
/// The identifier goes into the section names, as with the surfaces and the files: that way the
/// inner map goes in a block of its own, not mixed with the widget's numbers.
///
/// The `serial` goes along because it is what orders the widgets in [`Machine::current_form`]:
/// without it, the widget the game drew last would become a different one after loading.
impl<C: CpuBackend> Machine<C> {
    fn save_widgets(&self, sections: &mut Sections) {
        sections.put_u32s("wid.ids", self.widgets.keys().copied());
        for (id, widget) in &self.widgets {
            sections.put_u32s(
                &format!("wid.{id}.meta"),
                [
                    widget.size.0,
                    widget.size.1,
                    widget.position.0 as u32,
                    widget.position.1 as u32,
                    widget.class_id,
                    widget.serial as u32,
                    (widget.serial >> 32) as u32,
                    u32::from(widget.visible),
                    widget.parent,
                    widget.handler.0,
                    widget.handler.1,
                    widget.drawing.0,
                    widget.drawing.1,
                    widget.releasers.0,
                    widget.releasers.1,
                    u32::from(widget.start_notified),
                ],
            );
            sections.put_map(
                &format!("wid.{id}.filhos"),
                widget.children.iter().map(|(a, b)| (*a, *b)),
            );
            sections.put_map(
                &format!("wid.{id}.propriedades"),
                widget.properties.iter().map(|(a, b)| (*a, *b)),
            );
            sections.put_map(
                &format!("wid.{id}.modelos"),
                widget.models.iter().map(|(a, b)| (*a, *b)),
            );
            sections.put_u32s(&format!("wid.{id}.anexados"), widget.attached.iter().copied());
            sections.put_text(&format!("wid.{id}.texto"), &widget.text);
        }
    }

    fn restore_widgets(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let ids = reader.u32s("wid.ids")?;
        let mut widgets = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("wid.{id}.meta"))?;
            if meta.len() != 16 {
                return Err(Error::Section {
                    name: format!("wid.{id}.meta"),
                    reason: format!("expected 16 numbers and got {}", meta.len()),
                });
            }
            let children = reader.pairs(&format!("wid.{id}.filhos"))?.into_iter().collect();
            let properties = reader
                .pairs(&format!("wid.{id}.propriedades"))?
                .into_iter()
                .collect();
            let models = reader.pairs(&format!("wid.{id}.modelos"))?.into_iter().collect();
            widgets.insert(
                id,
                Widget {
                    children,
                    properties,
                    models,
                    size: (meta[0], meta[1]),
                    position: (meta[2] as i32, meta[3] as i32),
                    class_id: meta[4],
                    text: reader.text(&format!("wid.{id}.texto"))?,
                    serial: u64::from(meta[5]) | (u64::from(meta[6]) << 32),
                    attached: reader.u32s(&format!("wid.{id}.anexados"))?,
                    visible: meta[7] != 0,
                    parent: meta[8],
                    handler: (meta[9], meta[10]),
                    drawing: (meta[11], meta[12]),
                    releasers: (meta[13], meta[14]),
                    start_notified: meta[15] != 0,
                },
            );
        }
        self.widgets = widgets;
        Ok(())
    }
}


/// The last two library tables: the image decoder and the open database.
///
/// ## The decoder: real state, and it fits
///
/// `DecoderState` is the **file assembled piece by piece** by `IForceFeed::Write`, the already
/// created bitmap and whether the image has transparency. All three fields are encodable, and
/// they go in: a game that saves in the middle of an image transfer continues from where it was.
///
/// ## The database: the content **is not in the engine**
///
/// `Database` holds a live SQLite connection, and a connection does not serialize. But the
/// content **does not need to either**: it is in the file, on disk, and what the save state saves
/// is the path — exactly as it does with the open files. The way back reopens it.
///
/// What is lost, and it is stated: an **open transaction** at the moment of the save. SQLite only
/// writes what was committed, so what was in progress comes back as not done. It is the same kind
/// of thing as a file that changed on disk between saving and loading — and that is why the path
/// is kept inside the `Database`: without it, the way back would be impossible.
impl<C: CpuBackend> Machine<C> {
    fn save_libraries(&self, sections: &mut Sections) {
        sections.put_u32s("dec.ids", self.decoders.keys().copied());
        for (id, decoder) in &self.decoders {
            sections.put_u32s(
                &format!("dec.{id}.meta"),
                [
                    decoder.bitmap.unwrap_or(0),
                    u32::from(decoder.bitmap.is_some()),
                    u32::from(decoder.transparent),
                ],
            );
            sections.put(&format!("dec.{id}.fed"), decoder.fed.clone());
        }

        sections.put_u32s("db.ids", self.databases.keys().copied());
        for (id, soundfont) in &self.databases {
            sections.put_text(&format!("db.{id}.caminho"), &soundfont.path().to_string_lossy());
        }
    }

    fn restore_libraries(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let ids = reader.u32s("dec.ids")?;
        let mut decoders = std::collections::HashMap::new();
        for id in ids {
            let meta = reader.u32s(&format!("dec.{id}.meta"))?;
            if meta.len() != 3 {
                return Err(Error::Section {
                    name: format!("dec.{id}.meta"),
                    reason: format!("expected 3 numbers and got {}", meta.len()),
                });
            }
            let has_bitmap = match meta[1] {
                0 => false,
                1 => true,
                other => {
                    return Err(Error::Section {
                        name: format!("dec.{id}.meta"),
                        reason: format!("the `bitmap` field is {other}, and it is boolean"),
                    })
                }
            };
            decoders.insert(
                id,
                DecoderState {
                    fed: required_section(reader, &format!("dec.{id}.fed"))?,
                    bitmap: has_bitmap.then_some(meta[0]),
                    transparent: meta[2] != 0,
                },
            );
        }

        // The databases are **reopened before applying**: a path that no longer exists is a
        // refusal, not a database table with a half-built entry.
        let ids = reader.u32s("db.ids")?;
        let mut databases = std::collections::HashMap::new();
        for id in ids {
            let path =
                std::path::PathBuf::from(reader.text(&format!("db.{id}.caminho"))?);
            // As with the files: the state does not open a database outside what the game can
            // reach. The `tt_game_info` profile copy lives in the desktop's device directory,
            // which may not be the VFS one.
            let reachable = self.vfs.contains_host(&path)
                || (path.starts_with(crate::loader::archive::device_dir().join("z-wheel"))
                    && !path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir)));
            if !reachable {
                return Err(Error::Section {
                    name: format!("db.{id}"),
                    reason: format!(
                        "the state holds \"{}\", outside what the game can reach",
                        path.display()
                    ),
                });
            }
            let soundfont =
                crate::brew::sql::Database::open(&path).map_err(|error| Error::Section {
                    name: format!("db.{id}"),
                    reason: format!(
                        "the state holds \"{}\" open and it could not be reopened: {error}",
                        path.display()
                    ),
                })?;
            databases.insert(id, soundfont);
        }

        self.decoders = decoders;
        self.databases = databases;
        Ok(())
    }
}


/// The last missing state fields: the interrupted segment, the pending calls, the EGL surfaces
/// and the digests in progress.
///
/// ## What does **not** go in, and now with the reason measured
///
/// `media_payloads` holds the already decoded sounds — an `Arc<Sound>` with all the samples, which
/// for a long sound goes past a hundred megabytes. It **looks** like a gap, and it is not: it is a
/// cache **with a reload path**. `resolve_media` looks up the key and, when it does not find it,
/// reads the `AEEMediaData` and loads it again. Dropping it costs one reread of the file, and
/// saving it would add the whole file to the save state.
///
/// The same argument holds for `vfs` and `resources`, which were already out. And the other 25
/// remaining fields are instrumentation and diagnostics: call counters, traces, assumptions, lists
/// of files that were missing. **Saving them is not just unnecessary, it is wrong**: two equally
/// valid sessions of the same game would produce different save states, and comparing two states
/// would end up measuring how much each one was observed.
impl<C: CpuBackend> Machine<C> {
    fn save_trailing(&self, sections: &mut Sections) {
        // The interrupted segment: the address and the fifteen registers of whoever was
        // interrupted.
        let (has, address, registers) = match self.interrupted_segment {
            Some((address, registers)) => (1u32, address, registers),
            None => (0, 0, [0u32; 15]),
        };
        let mut numbers = vec![has, address];
        numbers.extend(registers);
        sections.put_u32s("ult.trecho", numbers);

        sections.put_records(
            "ult.chamadas",
            self.pending_calls
                .iter()
                .map(|call| {
                    vec![
                        call.function,
                        call.args[0],
                        call.args[1],
                        call.args[2],
                        call.args[3],
                    ]
                })
                .collect::<Vec<_>>(),
        );

        sections.put_records(
            "ult.egl_surfaces",
            self.egl_surfaces
                .iter()
                .map(|(id, (a, b))| vec![*id, *a, *b])
                .collect::<Vec<_>>(),
        );

        // The digests in progress. The MD5 state fits in four words, the rest of the block and
        // the length.
        sections.put_u32s("hash.ids", self.hashes.keys().copied());
        for (id, hash) in &self.hashes {
            let (state, buffer, length) = hash.md5.state();
            let mut numbers = Vec::with_capacity(4 + 2 + buffer.len());
            numbers.extend(state);
            numbers.push(length as u32);
            numbers.push((length >> 32) as u32);
            numbers.push(buffer.len() as u32);
            numbers.extend(buffer.iter().map(|b| u32::from(*b)));
            sections.put_u32s(&format!("hash.{id}.md5"), numbers);
        }
    }

    fn restore_trailing(&mut self, reader: &Reader<'_>) -> Result<(), Error> {
        let numbers = reader.u32s("ult.trecho")?;
        if numbers.len() != 17 {
            return Err(Error::Section {
                name: "ult.trecho".to_string(),
                reason: format!("expected 17 numbers and got {}", numbers.len()),
            });
        }
        let interrupted_segment = match numbers[0] {
            0 => None,
            1 => {
                let mut registers = [0u32; 15];
                registers.copy_from_slice(&numbers[2..17]);
                Some((numbers[1], registers))
            }
            other => {
                return Err(Error::Section {
                    name: "ult.trecho".to_string(),
                    reason: format!("the presence field is {other}, and it is boolean"),
                })
            }
        };

        let pending_calls: Vec<GuestCall> = reader
            .records("ult.chamadas", 5)?
            .into_iter()
            .map(|r| GuestCall {
                function: r[0],
                args: [r[1], r[2], r[3], r[4]],
            })
            .collect();

        let egl_surfaces: std::collections::HashMap<u32, (u32, u32)> = reader
            .records("ult.egl_surfaces", 3)?
            .into_iter()
            .map(|r| (r[0], (r[1], r[2])))
            .collect();

        let ids = reader.u32s("hash.ids")?;
        let mut hashes = std::collections::HashMap::new();
        for id in ids {
            let numbers = reader.u32s(&format!("hash.{id}.md5"))?;
            if numbers.len() < 7 {
                return Err(Error::Section {
                    name: format!("hash.{id}.md5"),
                    reason: format!("expected at least 7 numbers and got {}", numbers.len()),
                });
            }
            let count = numbers[6] as usize;
            if numbers.len() != 7 + count {
                return Err(Error::Section {
                    name: format!("hash.{id}.md5"),
                    reason: format!(
                        "claims {count} byte(s) in the rest of the block and {} came",
                        numbers.len() - 7
                    ),
                });
            }
            let mut md5 = crate::brew::crypto::Md5::new();
            let buffer: Vec<u8> = numbers[7..].iter().map(|b| *b as u8).collect();
            md5.restore_state(
                [numbers[0], numbers[1], numbers[2], numbers[3]],
                &buffer,
                u64::from(numbers[4]) | (u64::from(numbers[5]) << 32),
            )
            .map_err(|reason| Error::Section {
                name: format!("hash.{id}.md5"),
                reason,
            })?;
            hashes.insert(id, HashState { md5 });
        }

        self.interrupted_segment = interrupted_segment;
        self.pending_calls = pending_calls;
        self.egl_surfaces = egl_surfaces;
        self.hashes = hashes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::DefaultBackend;
    use crate::loader::self as loader;

    /// The smallest module the loader accepts. It does not need to do anything: the target here is
    /// the machine state around it — memory, registers and the allocation counters.
    fn modulo() -> crate::loader::modfile::ModImage {
        let code = [
            0xe3a0_0010u32.to_le_bytes(), // mov r0, #16
            0xe12f_ff1eu32.to_le_bytes(), // bx lr
        ]
        .concat();
        crate::loader::modfile::ModImage::parse(code).unwrap()
    }

    fn machine() -> Machine<DefaultBackend> {
        let module = loader::load(&modulo()).unwrap();
        let mut machine = Machine::new(DefaultBackend::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine
    }

    /// **The flags and the clock come back with the state.**
    ///
    /// They are the pair that makes a save state "almost" work when missing: the memory comes
    /// back, the program comes back, the flags stay those of another run — the comparison the game
    /// made before saving still holds and the next decision may take the other path. And the clock
    /// is the time the game sees: without it, the `SetTimer`s and the per-frame audio come out of a
    /// different instant.
    #[test]
    fn the_flags_and_the_clock_come_back() {
        let mut before = machine();
        // The negative bit, so the value differs from what the new machine has.
        let cpsr = before.cpu.cpsr() | (1 << 31);
        before.cpu.set_cpsr(cpsr);
        before.cpu.set_instructions(12_345_678);

        let mut after = machine();
        assert_eq!(after.cpu.instructions(), 0, "the new machine starts with the clock at zero");

        let file = before.save_state();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.cpu.cpsr(), cpsr, "the flags did not come back");
        assert_eq!(after.cpu.instructions(), 12_345_678, "the clock did not come back");
    }

    /// The skipped time comes back too, and the whole clock is the one from when the state was
    /// saved. An old state, without the section, loads with the current skipped time, as before.
    #[test]
    fn skipped_time_comes_back_with_the_state() {
        let mut before = machine();
        before.clock_us = 20_000_000;
        before.cpu.set_instructions(528 * 1_000);
        let file = before.save_state();
        let clock = before.clock_ms();

        let mut after = machine();
        after.clock_us = 90_000_000;
        after.restore_state(&file).expect("restored");
        assert_eq!(after.clock_us, 20_000_000, "the skipped time did not come back");
        assert_eq!(after.clock_ms(), clock);

        let reader = crate::save_state::Reader::open(&file).unwrap();
        let without_section: Vec<(String, Vec<u8>)> = reader
            .names()
            .into_iter()
            .filter(|name| *name != "relogio.us")
            .map(|name| (name.to_string(), reader.section(name).unwrap().to_vec()))
            .collect();
        let old = crate::save_state::write(&without_section);
        let mut after = machine();
        after.clock_us = 90_000_000;
        after.restore_state(&old).expect("the old state loads");
        assert_eq!(after.clock_us, 90_000_000);
    }

    /// The OpenGL ES names in use come back with the state, and the next `glGen*` does not repeat
    /// a live name. An old state, with only the counter, leaves in use everything the counter
    /// handed out.
    #[test]
    fn the_gl_names_come_back_with_the_state() {
        let mut before = machine();
        before.texture_names = super::super::gl::GlNames::from([1, 2, 5]);
        before.buffer_names = super::super::gl::GlNames::from([3]);
        let file = before.save_state();

        let mut after = machine();
        after.restore_state(&file).expect("restored");
        assert_eq!(after.texture_names, before.texture_names);
        assert_eq!(after.buffer_names, before.buffer_names);
        assert_eq!(
            after.texture_names.generate(),
            3,
            "the hole at 3 is the smallest free one"
        );

        let reader = crate::save_state::Reader::open(&file).unwrap();
        let without_sections: Vec<(String, Vec<u8>)> = reader
            .names()
            .into_iter()
            .filter(|name| !name.starts_with("esc.nomes_de_"))
            .map(|name| (name.to_string(), reader.section(name).unwrap().to_vec()))
            .collect();
        let old = crate::save_state::write(&without_sections);
        let mut after = machine();
        after
            .restore_state(&old)
            .expect("the old state loads");
        // The counter field holds the largest name in use, 5: everything from 1 to 5 stays
        // reserved.
        assert_eq!(after.texture_names.generate(), 6);
        assert_eq!(after.buffer_names.generate(), 6);
    }

    /// The Machine file includes the rasterizer state, not just the isolated proof of
    /// GlState. This wiring is what was missing in 0.3.0: the rasterizer tests passed, but a
    /// game's final ZBXS contained no gl.* section.
    #[test]
    fn the_machine_saves_and_restores_the_rasterizer() {
        let mut before = machine();
        before.gl.set_clear_color([0.125, 0.25, 0.5, 1.0]);
        before.gl.bind_texture(0x1234);
        before.gl.upload_level(
            0x1234,
            0,
            2,
            1,
            vec![[255, 0, 0, 255], [0, 0, 255, 255]],
        );

        let file = before.save_state();
        let reader = Reader::open(&file).expect("valid state");
        assert!(reader.section("gl.matrizes").is_some(), "the GL state is missing");
        assert_eq!(reader.u32s("tex.ids").unwrap(), vec![0x1234]);

        let mut after = machine();
        after.restore_state(&file).expect("restored");
        let mut sections = Sections::create();
        after.gl.save_state(&mut sections);
        let gl_file = sections.close();
        let gl = Reader::open(&gl_file).expect("restored GL state");
        assert_eq!(gl.u32s("tex.ids").unwrap(), vec![0x1234]);
        assert_eq!(
            reader.section("gl.onde").unwrap(),
            gl.section("gl.onde").unwrap(),
            "GL colors and target changed on the way back"
        );
    }

    /// **The state comes back identical**: registers, heap, stack and the heap ledger.
    ///
    /// The heap is the interesting case because its memory does *not* go in whole: it goes up to
    /// the first address never used. If the cut is wrong, what the game had written disappears —
    /// and a save state that loses the game's data is worse than a save state that does not exist.
    #[test]
    fn registers_and_memory_come_back() {
        let mut before = machine();
        let block = before.heap.alloc(0x4000).expect("block on the heap");
        let on_heap = block + 0x100;
        before.cpu.write_mem(on_heap, b"estado do jogo").unwrap();
        let on_stack = loader::STACK_BASE + 0x300;
        before.cpu.write_mem(on_stack, &[7u8; 16]).unwrap();
        before.cpu.write_reg(Reg::R5, 0xdead_beef);
        before.cpu.write_reg(Reg::R12, 0x1020_3040);

        let file = before.save_state();

        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.cpu.read_reg(Reg::R5), 0xdead_beef);
        assert_eq!(after.cpu.read_reg(Reg::R12), 0x1020_3040);
        let mut loaded = [0u8; 14];
        after.cpu.read_mem(on_heap, &mut loaded).unwrap();
        assert_eq!(&loaded, b"estado do jogo");
        let mut stack = [0u8; 16];
        after.cpu.read_mem(on_stack, &mut stack).unwrap();
        assert_eq!(stack, [7u8; 16]);

        // And the restored machine allocates where the original would, as in the heap-only test.
        let next_before = before.heap.alloc(64);
        let next_after = after.heap.alloc(64);
        assert_eq!(next_after, next_before);
    }

    /// **The heap comes back with the `next` the state had**, not with the current machine's.
    ///
    /// It is the opposite of what I wrote first: I treated "the state's heap is larger" as an
    /// error, and it is not — it is the normal case. Whoever loads a save state loads the heap that
    /// was there.
    #[test]
    fn the_heap_comes_back_with_the_state_size() {
        let mut before = machine();
        before.heap.alloc(0x2000).expect("block on the heap");
        let file = before.save_state();

        let mut after = machine();
        assert_eq!(after.heap.next(), loader::HEAP_BASE, "the new machine starts empty");
        after.restore_state(&file).expect("restored");
        assert_eq!(after.heap.next(), before.heap.next());
    }

    /// A state from **another module** is rejected, without writing anything.
    ///
    /// Here the machine has another module, hence another `module` region: the size does not
    /// match, and the error states both numbers. Applying it would write one game's data over
    /// another's code.
    #[test]
    fn state_from_another_module_is_rejected() {
        let mut before = machine();
        before.heap.alloc(0x1000).expect("block on the heap");
        let file = before.save_state();

        let largest = [
            0xe3a0_0010u32.to_le_bytes(),
            0xe3a0_1010u32.to_le_bytes(),
            0xe3a0_2010u32.to_le_bytes(),
            0xe3a0_3010u32.to_le_bytes(),
            0xe12f_ff1eu32.to_le_bytes(),
        ]
        .concat();
        let module = loader::load(
            &crate::loader::modfile::ModImage::parse(largest).unwrap(),
        )
        .unwrap();
        let mut other_machine = Machine::new(DefaultBackend::new().unwrap(), module, ".");
        other_machine.cpu.reset(&other_machine.module.mem).unwrap();

        let next_before = other_machine.heap.next();
        match other_machine.restore_state(&file) {
            Err(Error::Section { name, reason }) => {
                assert_eq!(name, "mem.module");
                assert!(reason.contains("bytes of this region"), "{reason}");
            }
            other => panic!("should have rejected the state from another module, and returned {other:?}"),
        }
        assert_eq!(
            other_machine.heap.next(),
            next_before,
            "the refusal must not have applied half"
        );
    }

    /// **The state size, measured.** It is what decides whether this is usable: a save state that
    /// saves the whole memory would come out at 84 MB, and RetroArch saves one on every player
    /// hotkey.
    ///
    /// The cut in the heap is what makes the difference, and the regions that still go in whole
    /// (objects and surfaces) are most of what is left. The test prints the number because it
    /// changes when the next sections go in — and it is good that it changes in plain sight.
    #[test]
    fn the_state_does_not_carry_the_whole_memory() {
        let mut machine = machine();
        machine.heap.alloc(64 * 1024).expect("block on the heap");
        let file = machine.save_state();
        let mb = file.len() as f64 / (1024.0 * 1024.0);
        eprintln!(
            "minimal machine state: {} bytes ({mb:.2} MB); the mapped memory is 84 MB",
            file.len()
        );
        assert!(
            file.len() < 24 * 1024 * 1024,
            "the state went past 24 MB: {} bytes",
            file.len()
        );
    }

    #[test]
    fn a_file_that_is_not_a_save_state_is_rejected() {
        let mut machine = machine();
        assert!(matches!(
            machine.restore_state(b"nao sou eu"),
            Err(Error::Truncated { .. })
        ));
    }

    /// **Input and scheduling come back whole.**
    ///
    /// These are the tables the game feels right away: the key queue it has not read yet, the
    /// button event queue, who registered for device notices, the timers coming due and the
    /// pending callbacks. Without them the save state "comes back" and the game loses what it
    /// already had in hand.
    #[test]
    fn input_and_scheduling_come_back() {
        use crate::input::avk;

        let mut before = machine();
        before.set_key(avk::SELECT, true);
        before.set_key(avk::ZERO, false);
        let mut pad = crate::input::Pad::default();
        pad.press(1, true);
        before.set_port_pad(0, pad);
        before
            .input_signals
            .insert(("RegisterForPositionChange", 0), 0x5555_0000);
        before.timers.push(Timer {
            deadline_ms: 424_242,
            callback: Callback {
                function: 0x1000_2000,
                context: 0xdead_0000,
            },
        });
        before.signals.insert(
            0x77,
            Callback {
                function: 0x1000_3000,
                context: 0xbeef_0000,
            },
        );
        before.pending_signals.push(Callback {
            function: 0x1000_4000,
            context: 0x1234_0000,
        });

        let file = before.save_state();
        let mut after = machine();
        assert!(after.key_events.is_empty(), "the new machine starts with no queue");
        after.restore_state(&file).expect("restored");

        assert_eq!(after.key_events, before.key_events, "the key queue did not come back");
        assert_eq!(
            after.pad_events[0], before.pad_events[0],
            "the button event queue did not come back"
        );
        assert_eq!(
            after.input_signals.get(&("RegisterForPositionChange", 0)),
            Some(&0x5555_0000),
            "the device notice registration did not come back"
        );
        assert_eq!(after.timers.len(), 1, "the timer did not come back");
        assert_eq!(after.timers[0].deadline_ms, 424_242);
        assert_eq!(after.timers[0].callback.function, 0x1000_2000);
        assert_eq!(after.signals.get(&0x77).map(|c| c.context), Some(0xbeef_0000));
        assert_eq!(after.pending_signals.len(), 1);
        assert_eq!(after.pending_signals[0].function, 0x1000_4000);
    }

    /// The device signal name code round-trips for **all** known names.
    ///
    /// It is the same idea as the 62-interfaces test: a new name with no entry in the table turns
    /// this red, instead of a registration silently lost in a save state.
    #[test]
    fn the_signal_name_round_trips() {
        // **The whole list**, and it is the one the writer uses: a new name that does not go in
        // here leaves the writer shouting instead of saving a state that does not load.
        for name in DEVICE_SIGNALS {
            let code = input_signal_code(name);
            assert_ne!(code, u32::MAX, "{name} has no code");
            assert_eq!(input_signal_name(code), Some(name));
        }
        assert_eq!(input_signal_name(9999), None);
        // **A made-up name is not tested here on purpose**: the writer has a `debug_assert!` for
        // it, and that is what we want — a new name nobody put in the list makes the test shout,
        // instead of saving a state that does not load. That is how the third name turned up.
    }

    /// **The numeric tables come back**, and a value outside the engine's range is rejected.
    ///
    /// The second half is the one that matters most: a volume that does not fit in `u16` or an
    /// `owner` that is not boolean is a corrupt file, and silently accommodating it would mean
    /// loading a state the game did not write.
    #[test]
    fn the_numeric_tables_come_back() {
        let mut before = machine();
        before.dib_buffers.insert(0x11, 0x1000);
        before.dib_capacity.insert(0x11, 640 * 480 * 2);
        before.transforms.insert(0x22, 7);
        before.canvases.insert(0x33, 0x2000);
        before.feeds.insert(0x44, 12);
        before.transparency.insert(0x55, 0x8000);
        before.streams.insert(
            0x66,
            MemStream {
                buffer: 0x3000,
                size: 512,
                position: 128,
                owner: true,
            },
        );
        let mut info = [0u8; 5];
        info[1] = 9;
        before.sounds.insert(
            0x77,
            SoundState {
                notify: Callback {
                    function: 0x1000_5000,
                    context: 0x7777,
                },
                info,
                volume: 42,
            },
        );

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.dib_capacity.get(&0x11), Some(&(640 * 480 * 2)));
        assert_eq!(after.transforms.get(&0x22), Some(&7));
        assert_eq!(after.canvases.get(&0x33), Some(&0x2000));
        assert_eq!(after.feeds.get(&0x44), Some(&12));
        assert_eq!(after.transparency.get(&0x55), Some(&0x8000));
        let stream = after.streams.get(&0x66).expect("the stream came back");
        assert_eq!((stream.buffer, stream.size, stream.position), (0x3000, 512, 128));
        assert!(stream.owner, "the buffer's `owner` did not come back");
        let sound = after.sounds.get(&0x77).expect("the sound came back");
        assert_eq!(sound.volume, 42);
        assert_eq!(sound.info[1], 9);
        assert_eq!(sound.notify.function, 0x1000_5000);
    }

    /// A volume that does not fit in a `u16` is **rejected**, not accommodated.
    #[test]
    fn a_sound_with_impossible_volume_is_rejected() {
        use crate::save_state::Sections;

        let mut sections = Sections::create();
        // The sections the restorer reads first, empty, and the sounds one with the impossible
        // value.
        for name in [
            "tab.dib_buffers",
            "tab.dib_capacity",
            "tab.transformacoes",
            "tab.canvases",
            "tab.feeds",
            "tab.image_bitmaps",
            "tab.image_info",
            "tab.transparency",
            "tab.streams",
        ] {
            sections.put_u32s(name, []);
        }
        // id, notify.function, notify.context, volume, five bytes of info
        sections.put_u32s("tab.sounds", [0x77u32, 0, 0, 999_999, 0, 0, 0, 0, 0]);
        let file = sections.close();
        let reader = crate::save_state::Reader::open(&file).expect("opened");

        let mut machine = machine();
        match machine.restore_numeric_tables(&reader) {
            Err(Error::Section { name, reason }) => {
                assert_eq!(name, "tab.sounds");
                assert!(reason.contains("999999"), "{reason}");
            }
            other => panic!("should have rejected the impossible volume, and returned {other:?}"),
        }
    }

    /// **The font metrics and the open files come back.**
    ///
    /// The file is the interesting case: it does not keep bytes, it keeps **path and offset**, and
    /// the way back reopens the file by path and restores the offset. It is what makes a game that
    /// was reading in the middle of a file keep reading from the same point.
    #[test]
    fn fonts_and_open_files_come_back() {
        use std::io::{Read as _, Write as _};

        // A real file, on disk, with known content. It lives in the machine's device directory:
        // the state only reopens what the game can reach.
        let device = std::env::temp_dir().join(format!("zeebx-teste-do-estado-{}", std::process::id()));
        std::fs::create_dir_all(&device).expect("create the test device");
        let path = device.join("cena.bin");
        let mut created = std::fs::File::create(&path).expect("create the test file");
        let all_256_bytes: Vec<u8> = (0u8..=255).collect();
        created
            .write_all(&all_256_bytes)
            .expect("write");
        drop(created);

        let mut before = machine();
        before.fonts.insert(
            0x1010,
            crate::machine::font::Metrics {
                ascent: 13,
                descent: 3,
                leading: 1,
                max_char_width: 9,
                height: 17,
                bold: true,
                italic: false,
            },
        );
        {
            let mut file = std::fs::File::open(&path).expect("open for reading");
            // The game has already read seven bytes of this file.
            let mut got = [0u8; 7];
            file.read_exact(&mut got).expect("read seven bytes");
            assert_eq!(got, [0u8, 1, 2, 3, 4, 5, 6]);
            before.open_files.insert(
                0x2020,
                OpenFile {
                    file,
                    guest_path: "./dados/cena.bin".to_string(),
                    path: path.clone(),
                    for_writing: false,
                },
            );
        }

        let state_bytes = before.save_state();
        let mut after = machine();
        after.vfs.set_device_root(&device);
        after.restore_state(&state_bytes).expect("restored");

        let metrics = after.fonts.get(&0x1010).expect("the metrics came back");
        assert_eq!(
            (
                metrics.ascent,
                metrics.descent,
                metrics.leading,
                metrics.max_char_width,
                metrics.height
            ),
            (13, 3, 1, 9, 17)
        );
        assert!(metrics.bold);
        assert!(!metrics.italic);

        let opened = after.open_files.get_mut(&0x2020).expect("the file came back");
        assert_eq!(opened.guest_path, "./dados/cena.bin");
        let mut next = [0u8; 2];
        opened.file.read_exact(&mut next).expect("read the next ones");
        assert_eq!(
            next,
            [7, 8],
            "the file should continue from byte 7, not from the start"
        );
        let _ = std::fs::remove_dir_all(&device);
    }

    /// **The content tables come back** — preferences, parameters, `IConfig`, decrypted text,
    /// network response.
    ///
    /// They keep the content because it **does not exist anywhere** outside the engine: unlike an
    /// open file, which is reopened from disk, a `SetPrefs` the game did is no longer on any
    /// disk.
    #[test]
    fn the_content_tables_come_back() {
        let mut before = machine();
        before.prefs.insert((0x0100_0001, 3), vec![1, 2, 3]);
        before.prefs.insert((0x0100_0002, 0), Vec::new());
        before
            .collection_params
            .insert((0x10, 0x20), b"parametro".to_vec());
        before.sources.insert(0x30, b"fonte".to_vec());
        before.html_pages.insert(0x40, b"<html>".to_vec());
        before
            .config_items
            .entry(0x50)
            .or_default()
            .insert(0x60, b"valor".to_vec());
        before.plaintexts.push_back(b"primeiro".to_vec());
        before.plaintexts.push_back(b"segundo".to_vec());
        before.web_response = b"resposta".to_vec();

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.prefs.get(&(0x0100_0001, 3)), Some(&vec![1, 2, 3]));
        assert_eq!(after.prefs.get(&(0x0100_0002, 0)), Some(&Vec::new()));
        assert_eq!(
            after.collection_params.get(&(0x10, 0x20)),
            Some(&b"parametro".to_vec())
        );
        assert_eq!(after.sources.get(&0x30), Some(&b"fonte".to_vec()));
        assert_eq!(after.html_pages.get(&0x40), Some(&b"<html>".to_vec()));
        assert_eq!(
            after.config_items.get(&0x50).and_then(|i| i.get(&0x60)),
            Some(&b"valor".to_vec())
        );
        // The decrypted text queue comes back **in order**, which is its content.
        let queue: Vec<&Vec<u8>> = after.plaintexts.iter().collect();
        assert_eq!(
            queue,
            vec![&b"primeiro".to_vec(), &b"segundo".to_vec()],
            "the order of the decrypted text did not come back"
        );
        assert_eq!(after.web_response, b"resposta".to_vec());
    }

    /// A block with a size larger than what the file has is **rejected**.
    #[test]
    fn a_block_with_a_lying_size_is_rejected() {
        use crate::save_state::Sections;

        let mut sections = Sections::create();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes()); // one item
        bytes.extend_from_slice(&2u32.to_le_bytes()); // two-number key
        bytes.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&999u32.to_le_bytes()); // claims 999 bytes
        bytes.extend_from_slice(b"curto");
        sections.put("blocos", bytes);
        let file = sections.close();
        let reader = crate::save_state::Reader::open(&file).expect("opened");
        match reader.blocks("blocos") {
            Err(Error::Section { name, reason }) => {
                assert_eq!(name, "blocos");
                assert!(reason.contains("999"), "{reason}");
            }
            other => panic!("should have rejected the lying block, and returned {other:?}"),
        }
    }

    /// **The surfaces and the decoded images come back** — the pixels, and what the surface has
    /// already lived through (`writes` and `generation`), which is how the frontend knows what
    /// changed.
    #[test]
    fn surfaces_and_images_round_trip() {
        let mut before = machine();
        // A 4x2 surface with distinct pixels, so the test can tell pixel from pixel.
        let mut surface = Framebuffer::new(4, 2);
        for (index, value) in [0x1111u16, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777, 0x8888]
            .iter()
            .enumerate()
        {
            surface.set_pixel_native((index % 4) as i32, (index / 4) as i32, *value);
        }
        // Writing touches `touched` and the dirty box, which is what the frontend reads.
        assert!(surface.is_dirty());
        before.bitmaps.insert(0x9000, surface);

        let mut image = DecodedImage {
            width: 2,
            height: 2,
            pixels: vec![0xaaaa, 0xbbbb, 0xcccc, 0xdddd],
            opaque: vec![true, false, true, true],
            alpha: vec![0, 51, 255, 7],
            frame_width: 1,
        };
        image.frame_width = 1;
        before.images.insert(0xa000, std::rc::Rc::new(image));

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        let returned = after.bitmaps.get(&0x9000).expect("the surface came back");
        assert_eq!((returned.width(), returned.height()), (4, 2));
        assert_eq!(
            returned.pixels(),
            &[0x1111u16, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777, 0x8888],
            "the pixels did not come back"
        );
        let before_save = before.bitmaps.get(&0x9000).expect("the original");
        assert_eq!(
            returned.writes(),
            before_save.writes(),
            "the write counter did not come back"
        );
        assert_eq!(
            returned.generation(),
            before_save.generation(),
            "the surface's generation did not come back"
        );
        assert_eq!(
            returned.dirty_rect(),
            before_save.dirty_rect(),
            "the dirty box did not come back"
        );

        let image = after.images.get(&0xa000).expect("the image came back");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixels, vec![0xaaaa, 0xbbbb, 0xcccc, 0xdddd]);
        assert_eq!(image.opaque, vec![true, false, true, true], "`opaque` did not come back");
        assert_eq!(image.alpha, vec![0, 51, 255, 7], "the alpha did not come back");
        assert_eq!(image.frame_width, 1);
    }

    /// The surface comes back **without** losing the dirty box: saving a state must not consume
    /// the notice that the screen changed, or the next frame comes out stale because of the save
    /// state.
    #[test]
    fn the_dirty_rect_is_not_consumed_on_save() {
        let mut machine = machine();
        let mut surface = Framebuffer::new(2, 2);
        surface.set_pixel_native(1, 1, 0x1234);
        let expected = surface.dirty_rect();
        assert!(expected.is_some(), "drawing should dirty the surface");
        machine.bitmaps.insert(0x1000, surface);

        let _ = machine.save_state();
        assert_eq!(
            machine.bitmaps.get(&0x1000).and_then(|s| s.dirty_rect()),
            expected,
            "saving the state consumed the dirty box"
        );
    }

    /// The `opaque` bits go packed and come back in the right order.
    #[test]
    fn the_opaque_bits_round_trip() {
        for count in [0usize, 1, 7, 8, 9, 16, 17, 100] {
            let bits: Vec<bool> = (0..count).map(|i| i % 3 == 0).collect();
            let bytes = pack_bits(&bits);
            assert_eq!(bytes.len(), count.div_ceil(8), "size with {count} bits");
            assert_eq!(unpack_bits(&bytes, count), bits, "with {count} bits");
        }
    }

    /// **The scalars and the simple maps come back** — and the field count is right.
    ///
    /// The first half of this test is the one that protects the format: if someone adds a field
    /// without touching the list, the count stops matching and the test goes red **before** a save
    /// state comes out shifted.
    #[test]
    fn the_scalars_come_back_and_the_count_is_right() {
        let before = machine();
        let mut sections = Sections::create();
        before.save_scalars_and_maps(&mut sections);
        let file = sections.close();
        let reader = crate::save_state::Reader::open(&file).expect("opened");
        assert_eq!(
            reader.u32s("esc.numeros").expect("the section").len(),
            SCALARS,
            "the field count is out of date: whoever added a field did not update SCALARS"
        );
    }

    #[test]
    fn scalars_and_maps_come_back() {
        let mut before = machine();
        before.epoch_seconds = 1_700_000_000;
        before.random_state = 0x1234_5678;
        before.nesting = 3;
        before.current_applet = 0x0102_0304;
        before.applet_class = 0x0102_0305;
        before.next_vsync_us = 0x1_2345_6789;
        before.budget = 0x9_8765_4321;
        before.next_serial = 0x5_5555_5555;
        before.applet_closed = true;
        before.wheel_boot_skipped = true;
        before.pending_launch = Some(0x0102_8e35);
        before.gl_frame_writes = Some(0x4_4444_4444);
        before.scale_source = Some((-2, 7));
        before.current_thread = Some(0x5151);
        before.pending_activation = Some((0x61, 0x62));
        before.installed_applets.insert(0x1);
        before.installed_applets.insert(0x2);
        before.mif_in_guest.insert(0x70, 0x80);
        before.ext_modules = vec![Some(0x90), None, Some(0xa0)];
        before.html_scroll.insert(0xb0, 12);
        before.image_notify.insert(
            0xc0,
            Callback {
                function: 0x1000_6000,
                context: 0xd0,
            },
        );
        before.decoder_dib.insert(0xe0, (0xf0, 0x11));
        before.guest_dibs.insert(0xe4, 0x4000_0100);
        before.published_dib.insert(0x12, 0x13_0000_0014);
        before.vectors.insert(0x14, (vec![1, 2, 3], 9));
        before.collections.insert(0x15, (vec![4, 5], 6));

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.epoch_seconds, 1_700_000_000);
        assert_eq!(after.random_state, 0x1234_5678);
        assert_eq!(after.nesting, 3);
        assert_eq!(after.current_applet, 0x0102_0304);
        assert_eq!(after.applet_class, 0x0102_0305);
        assert_eq!(after.next_vsync_us, 0x1_2345_6789, "the high u64 did not come back");
        assert_eq!(after.budget, 0x9_8765_4321);
        assert_eq!(after.next_serial, 0x5_5555_5555);
        assert!(after.applet_closed);
        assert!(after.wheel_boot_skipped);
        assert_eq!(after.pending_launch, Some(0x0102_8e35));
        assert_eq!(after.gl_frame_writes, Some(0x4_4444_4444));
        assert_eq!(after.scale_source, Some((-2, 7)));
        assert_eq!(after.current_thread, Some(0x5151));
        assert_eq!(after.pending_activation, Some((0x61, 0x62)));
        assert_eq!(after.installed_applets.len(), 2);
        assert_eq!(after.mif_in_guest.get(&0x70), Some(&0x80));
        assert_eq!(after.ext_modules, vec![Some(0x90), None, Some(0xa0)]);
        assert_eq!(after.html_scroll.get(&0xb0), Some(&12));
        assert_eq!(
            after.image_notify.get(&0xc0).map(|c| c.function),
            Some(0x1000_6000)
        );
        assert_eq!(after.decoder_dib.get(&0xe0), Some(&(0xf0, 0x11)));
        assert_eq!(after.guest_dibs.get(&0xe4), Some(&0x4000_0100));
        assert_eq!(after.published_dib.get(&0x12), Some(&0x13_0000_0014));
        assert_eq!(after.vectors.get(&0x14), Some(&(vec![1, 2, 3], 9)));
        assert_eq!(after.collections.get(&0x15), Some(&(vec![4, 5], 6)));
    }

    /// A missing `pending_launch` must come back missing — and not turn into zero.
    #[test]
    fn a_missing_pending_launch_does_not_become_zero() {
        let mut before = machine();
        before.pending_launch = None;
        let file = before.save_state();
        let mut after = machine();
        after.pending_launch = Some(0x9999);
        after.restore_state(&file).expect("restored");
        assert_eq!(
            after.pending_launch, None,
            "the state said there was no launch request, and one came back"
        );
    }

    /// **The text lists and the stop point come back.**
    ///
    /// The stop point is the hole I had declared as "cannot be saved". It can: the state is well
    /// defined, and what was missing was the encoding. The test covers each `Outcome` variant,
    /// because it is precisely on the least used path that a field gets lost.
    #[test]
    fn text_lists_and_stall_point_come_back() {
        let mut before = machine();
        before.installed_modules = vec![
            (0x0102_8e35, "Z-Wheel".to_string()),
            (0x0102_8e36, "Face".to_string()),
        ];
        before
            .enumerations
            .insert(0x1000, ["primeiro".to_string(), "segundo".to_string()].into());
        before.stalled = Some(Outcome::Fault {
            addr: 0x10,
            pc: 0x20,
            lr: 0x30,
        });

        let file = before.save_state();
        let mut after = machine();
        after.interned.insert("OpenGL ES-CM 1.1", 0x999);
        after.restore_state(&file).expect("restored");

        assert_eq!(after.installed_modules, before.installed_modules);
        let list: Vec<String> = after
            .enumerations
            .get(&0x1000)
            .expect("the enumeration came back")
            .iter()
            .cloned()
            .collect();
        assert_eq!(list, vec!["primeiro".to_string(), "segundo".to_string()]);
        assert_eq!(
            after.stalled,
            Some(Outcome::Fault {
                addr: 0x10,
                pc: 0x20,
                lr: 0x30
            })
        );
        assert!(
            after.interned.is_empty(),
            "the static text cache should have been cleared, not loaded"
        );
    }

    /// Every `Outcome` variant round-trips. It is where a field gets lost without anyone noticing.
    #[test]
    fn every_outcome_round_trips() {
        let outcomes = vec![
            Outcome::Returned { code: 7 },
            Outcome::Unimplemented {
                addr: 0x1,
                args: [2, 3, 4, 5],
                caller: 6,
            },
            Outcome::Fault {
                addr: 8,
                pc: 9,
                lr: 10,
            },
            Outcome::Exception { pc: 11 },
            Outcome::Budget,
            Outcome::CallLimit { calls: 0x1_0000_0002 },
        ];
        for outcome in outcomes {
            let (label, fields) = describe(&Some(outcome.clone()));
            let mut numbers = vec![label];
            numbers.extend(fields);
            assert_eq!(
                build(&numbers).expect("came back"),
                Some(outcome.clone()),
                "outcome {outcome:?} did not come back the same"
            );
        }
        // Missing goes and comes back as missing.
        assert_eq!(build(&[0]).expect("missing"), None);
        // And an unknown outcome is a refusal, not an invention.
        assert!(build(&[99]).is_err());
        assert!(build(&[2]).is_err(), "missing fields should be a refusal");
    }

    /// **The seven tables of the last batch round-trip** — cipher, decompression, peek, clip,
    /// value model, PCM stream, pending deliveries and media state.
    #[test]
    fn the_remaining_tables_come_back() {
        let mut before = machine();
        before.ciphers.insert(
            0x100,
            CipherState {
                key: Some([7u8; 16]),
                iv: [9u8; 16],
                padding: 2,
                pending: vec![1, 2, 3],
            },
        );
        before.ciphers.insert(
            0x101,
            CipherState {
                key: None,
                iv: [0u8; 16],
                padding: 0,
                pending: Vec::new(),
            },
        );
        before.unzips.insert(
            0x200,
            UnzipState {
                source: 0x300,
                output: vec![4, 5, 6],
                position: 2,
                expanded: true,
            },
        );
        before.peeks.insert(
            0x400,
            Peek {
                bytes: vec![7, 8],
                position: 1,
                buffer: 0x500,
            },
        );
        before.image_clips.insert(
            0x600,
            ImageClip {
                x: -3,
                y: 4,
                size: Some((10, 20)),
                transparent: true,
            },
        );
        before.image_clips.insert(
            0x601,
            ImageClip {
                x: 0,
                y: 0,
                size: None,
                transparent: false,
            },
        );
        before.value_models.insert(
            0x700,
            ValueModel {
                value: 42,
                size: 4,
                listeners: vec![(0x800, 0x900, 0xa00)],
            },
        );
        before.pcm_streams.insert(
            0xb00,
            PcmStream {
                source: 0xc00,
                rate: 44_100,
                channels: 2,
                bits: 16,
                unsigned: false,
                block: 0,
                start_us: 0x1_0000_0005,
                frames_read: 0x2_0000_0007,
                playing: true,
                logged_end: false,
            },
        );
        before.pending_blits.push(PendingBlit {
            image: 0xd00,
            target: 0xe00,
            x: -5,
            y: 6,
            src_x: 7,
            src_y: 8,
            width: 9,
            height: 10,
            rop: 2,
            frame: Some(3),
        });
        before.pending_surface_blits.push(PendingSurfaceBlit {
            source: 0xd10,
            target: 0xe10,
            x: -7,
            y: 8,
            width: 11,
            height: 12,
            rop: 7,
        });
        before.media.insert(
            0xf00,
            MediaState {
                state: 5,
                payload: 0x1_0000_0009,
                pending: (0x10, 0x11),
                buffer: (0x12, 0x13),
                play_on_load: true,
                volume: 80,
                repeat: 2,
                muted: false,
                notify: Callback {
                    function: 0x1000_7000,
                    context: 0x14,
                },
                ends_us: 0x2_0000_000b,
            },
        );

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.ciphers.get(&0x100).map(|c| c.key), Some(Some([7u8; 16])));
        assert_eq!(after.ciphers.get(&0x100).map(|c| c.padding), Some(2));
        assert_eq!(after.ciphers.get(&0x100).map(|c| c.pending.clone()), Some(vec![1, 2, 3]));
        assert_eq!(after.ciphers.get(&0x101).map(|c| c.key), Some(None));
        let decompressor = after.unzips.get(&0x200).expect("the decompression came back");
        assert_eq!(decompressor.output, vec![4, 5, 6]);
        assert_eq!(decompressor.position, 2);
        assert!(decompressor.expanded);
        assert_eq!(after.peeks.get(&0x400).map(|p| p.bytes.clone()), Some(vec![7, 8]));
        assert_eq!(
            after.image_clips.get(&0x600).map(|r| (r.x, r.y, r.size)),
            Some((-3, 4, Some((10, 20))))
        );
        assert_eq!(
            after.image_clips.get(&0x601).and_then(|r| r.size),
            None,
            "the clip without a size came back with one"
        );
        assert_eq!(
            after.value_models.get(&0x700).map(|m| (m.value, m.listeners.clone())),
            Some((42, vec![(0x800, 0x900, 0xa00)]))
        );
        let stream = after.pcm_streams.get(&0xb00).expect("the stream came back");
        assert_eq!((stream.channels, stream.bits), (2, 16));
        assert_eq!(stream.start_us, 0x1_0000_0005, "the start u64 did not come back");
        assert_eq!(stream.frames_read, 0x2_0000_0007);
        assert!(stream.playing);
        assert_eq!(after.pending_blits.len(), 1);
        assert_eq!(after.pending_blits[0].frame, Some(3));
        assert_eq!(after.pending_surface_blits.len(), 1);
        assert_eq!(after.pending_surface_blits[0].width, 11);
        let media = after.media.get(&0xf00).expect("the media came back");
        assert_eq!(media.payload, 0x1_0000_0009);
        assert_eq!(media.pending, (0x10, 0x11));
        assert_eq!(media.buffer, (0x12, 0x13));
        assert_eq!(media.ends_us, 0x2_0000_000b);
        assert_eq!(media.notify.function, 0x1000_7000);
    }

    /// **After restoring, the image is the screen's, not the GPU's.** The GPU texture does not go
    /// into the state and comes back empty; until the next buffer swap, the window shows the
    /// restored screen. A frame pending from before the `Load` must not land on top of it either.
    #[test]
    fn restoring_keeps_the_image_on_screen_until_the_next_swap() {
        let mut machine = machine();
        let file = machine.save_state();
        machine.gl_frame_pending = true;
        machine.restore_state(&file).expect("restored");
        assert!(machine.gpu_without_frame, "the window would show the GPU's empty texture");
        assert!(!machine.gl_frame_pending, "the frame from before the Load would land on top of the screen");
        assert!(machine.gpu_frame().is_none());
    }

    /// **The mixer goes back to what the restored state plays.** What it was playing after the
    /// saved point goes away — a loop would stay forever, because the restored game does not know
    /// about it —, and what the state says is playing comes back: the loop from the start, the
    /// effect from the point where the clock left it.
    #[test]
    fn restoring_rebuilds_the_sound_from_the_state() {
        let sound = std::sync::Arc::new(crate::audio::wav::Sound::create(1_000, 1, vec![0.5; 1_000]));
        let mut machine = machine();
        let mixer = crate::audio::Mixer::silent(1_000);
        machine.set_audio(Some(mixer.clone()));
        machine.media_payloads.insert(
            7,
            crate::machine::MediaPayload {
                sound: Some(sound.clone()),
                silence_us: None,
            },
        );
        let now = machine.now_us();
        let playing = |repeat, ends_us| MediaState {
            state: crate::machine::MM_STATE_PLAY,
            payload: 7,
            repeat,
            ends_us,
            ..MediaState::default()
        };
        // The looping music and a one-second effect with 0.4 s left to play.
        machine.media.insert(0xa00, playing(0, u64::MAX));
        machine.media.insert(0xb00, playing(1, now + 400_000));
        // An effect that has already ended as far as the clock is concerned does not come back.
        machine.media.insert(0xc00, playing(1, now));
        let file = machine.save_state();

        // After the saved point, another piece of music started on another voice.
        mixer.play(0xd00, sound, 1.0, 0);
        machine.restore_state(&file).expect("restored");

        assert!(!mixer.is_playing(0xd00), "the music from after the point kept playing");
        assert!(mixer.is_playing(0xa00), "the state's loop did not come back");
        assert!(!mixer.is_playing(0xc00), "an already finished effect played again");
        let mut frames = 0usize;
        while mixer.is_playing(0xb00) && frames < 5_000 {
            let _ = mixer.render(10);
            frames += 10;
        }
        assert!(
            frames.abs_diff(400 + 64) <= 20,
            "the effect should play only the remaining 0.4 s, and played {frames} frames"
        );
    }

    /// **The rest: scalars, GL, IGraphics, keyboards and threads** — and the right field count.
    #[test]
    fn the_rest_comes_back_and_the_count_is_right() {
        let before = machine();
        let mut sections = Sections::create();
        before.save_the_rest(&mut sections);
        let file = sections.close();
        let reader = crate::save_state::Reader::open(&file).expect("opened");
        assert_eq!(
            reader.u32s("resto.numeros").expect("the section").len(),
            REST_LEN,
            "the field count is out of date: whoever added a field did not update REST_LEN"
        );
    }

    #[test]
    fn gl_scalars_and_threads_come_back() {
        let mut before = machine();
        before.file_error = 12;
        before.surface_manip = 0x2222;
        before.gles11_ext = 0x3333;
        before.boomerang_sequence = 7;
        before.calibrations = (0x44, 0x55);
        before.pending_response = Some((1, 2, 3));
        before.pending_end = Some(9);
        before.egl_color_dimensions = Some((640, 480));
        before.egl_color_buffer = (0x66, 480 * 640 * 2);
        before.clip = Some(Rect {
            x: -1,
            y: -2,
            width: 300,
            height: 200,
        });
        before.network = true;
        before.network_to = Some("127.0.0.1:80".to_string());
        before.graphics.fill_mode = true;
        before.graphics.point_size = 3;
        before.graphics.origin = (-4, 5);
        before.graphics.stroke = Rgb { r: 1, g: 2, b: 3 };
        before.colors[7] = Rgb { r: 200, g: 100, b: 50 };
        let mut pad = Pad::default();
        pad.press(2, true);
        pad.set_axis(1, -100);
        before.pads[0] = pad;
        before.motion[1] = [0.5, -0.25, 1.0];
        before.gl_vertices = ArrayPointer {
            size: 3,
            kind: 0x1406,
            stride: 12,
            address: 0x7000,
            enabled: true,
            buffer: 0x88,
        };
        before.gl_current_normal = [0.0, 1.0, 0.0];
        before.scroll_keys.insert(0x99);
        before.resources_read.insert(0x1234);
        before.media_notify_queue.push((
            0xaa,
            0xbb,
            0xcc,
            Callback {
                function: 0x1000_8000,
                context: 0xdd,
            },
        ));
        before.image_notify_queue.push(0x3000_0450);
        before.gl_buffers.insert(0xee, vec![1, 2, 3, 4]);
        before.egl_color_bytes = vec![5, 6];
        before.egl_color_readback = vec![7, 8, 9];
        let mut saved_context = [0u32; 14];
        saved_context[5] = 0xff00;
        before.threads.insert(
            0x111,
            ThreadState {
                stack: 0x222,
                resume_cb: 0x333,
                resume_pc: 0x444,
                started: true,
                suspended: false,
                finished: false,
                exit_code: 0,
                context: saved_context,
                joiners: vec![(
                    Callback {
                        function: 0x555,
                        context: 0x666,
                    },
                    0x777,
                )],
            },
        );
        before.resume_callbacks.insert(0x888, 0x999);
        before.pending_threads.push(0xaaa);
        let mut frame = Framebuffer::new(2, 1);
        frame.set_pixel_native(0, 0, 0xbeef);
        before.update_frames.push_back(frame);

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        assert_eq!(after.file_error, 12);
        assert_eq!(after.surface_manip, 0x2222);
        assert_eq!(after.gles11_ext, 0x3333);
        assert_eq!(after.boomerang_sequence, 7);
        assert_eq!(after.calibrations, (0x44, 0x55));
        assert_eq!(after.pending_response, Some((1, 2, 3)));
        assert_eq!(after.pending_end, Some(9));
        assert_eq!(after.egl_color_dimensions, Some((640, 480)));
        assert_eq!(after.egl_color_buffer, (0x66, 480 * 640 * 2));
        let clip = after.clip.expect("the clip came back");
        assert_eq!(
            (clip.x, clip.y, clip.width, clip.height),
            (-1, -2, 300, 200)
        );
        assert!(after.network);
        assert_eq!(after.network_to.as_deref(), Some("127.0.0.1:80"));
        assert!(after.graphics.fill_mode);
        assert_eq!(after.graphics.point_size, 3);
        assert_eq!(after.graphics.origin, (-4, 5));
        assert_eq!((after.graphics.stroke.r, after.graphics.stroke.g), (1, 2));
        assert_eq!(after.colors[7].r, 200);
        assert_eq!(after.pads[0].buttons, before.pads[0].buttons, "the pad buttons");
        assert_eq!(after.pads[0].axes[1], -100, "the pad axis");
        assert_eq!(after.motion[1], [0.5, -0.25, 1.0], "the motion is f32");
        assert_eq!(after.gl_vertices.size, 3);
        assert_eq!(after.gl_vertices.address, 0x7000);
        assert!(after.gl_vertices.enabled);
        assert_eq!(after.gl_vertices.buffer, 0x88);
        assert_eq!(after.gl_current_normal, [0.0, 1.0, 0.0]);
        assert!(after.scroll_keys.contains(&0x99));
        assert!(after.resources_read.contains(&0x1234));
        assert_eq!(after.media_notify_queue.len(), 1);
        assert_eq!(after.media_notify_queue[0].3.function, 0x1000_8000);
        assert_eq!(after.image_notify_queue.len(), 1);
        assert_eq!(after.image_notify_queue, vec![0x3000_0450]);
        assert_eq!(after.gl_buffers.get(&0xee), Some(&vec![1, 2, 3, 4]));
        assert_eq!(after.egl_color_bytes, vec![5, 6]);
        assert_eq!(after.egl_color_readback, vec![7, 8, 9]);
        let thread = after.threads.get(&0x111).expect("the thread came back");
        assert_eq!(thread.stack, 0x222);
        assert_eq!(thread.context[5], 0xff00, "the thread context did not come back");
        assert!(thread.started);
        assert_eq!(thread.joiners.len(), 1);
        assert_eq!(thread.joiners[0].1, 0x777);
        assert_eq!(after.resume_callbacks.get(&0x888), Some(&0x999));
        assert_eq!(after.pending_threads, vec![0xaaa]);
        let frame = after.update_frames.front().expect("the frame came back");
        assert_eq!(frame.pixels(), &[0xbeef, 0x0000]);
    }

    /// **The widgets come back whole** — the three maps, the text, the coordinates and the
    /// function-and-context pairs.
    ///
    /// The `serial` has its own check on purpose: it is what orders the widgets in
    /// `current_form`, and a widget that loses its order becomes a different form after loading.
    #[test]
    fn the_widgets_come_back() {
        let mut before = machine();
        let mut children = std::collections::HashMap::new();
        children.insert(0x11u32, 0x22u32);
        children.insert(0x33, 0x44);
        let mut properties = std::collections::HashMap::new();
        properties.insert(6u32, 0x55u32);
        let mut models = std::collections::HashMap::new();
        models.insert(0x8000u32, 0x8001u32);
        before.widgets.insert(
            0x1000,
            Widget {
                children,
                properties,
                models,
                size: (640, 480),
                position: (-7, 9),
                class_id: 0x0102_8e3f,
                text: "Abrir".to_string(),
                serial: 0x1_0000_0002,
                attached: vec![0x66, 0x67],
                visible: true,
                parent: 0x68,
                handler: (0x69, 0x6a),
                drawing: (0x6b, 0x6c),
                releasers: (0x6d, 0x6e),
                start_notified: true,
            },
        );

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        let widget = after.widgets.get(&0x1000).expect("the widget came back");
        assert_eq!(widget.size, (640, 480));
        assert_eq!(widget.position, (-7, 9));
        assert_eq!(widget.class_id, 0x0102_8e3f);
        assert_eq!(widget.text, "Abrir");
        assert_eq!(widget.serial, 0x1_0000_0002, "the `serial` did not come back whole");
        assert_eq!(widget.attached, vec![0x66, 0x67]);
        assert!(widget.visible);
        assert_eq!(widget.parent, 0x68);
        assert_eq!(widget.handler, (0x69, 0x6a));
        assert_eq!(widget.drawing, (0x6b, 0x6c));
        assert_eq!(widget.releasers, (0x6d, 0x6e));
        assert!(widget.start_notified);
        assert_eq!(
            widget.children.get(&0x11),
            Some(&0x22),
            "the children map did not come back"
        );
        assert_eq!(widget.properties.get(&6), Some(&0x55));
        assert_eq!(widget.models.get(&0x8000), Some(&0x8001));
    }

    /// **The decoder comes back whole, and the database comes back reopened.**
    ///
    /// The database test is the most interesting of this whole front: it **writes a row, saves,
    /// loads and reads the row back**. It is not a field test — it is the content surviving the
    /// round trip through the path, which is how the save state treats everything that lives on
    /// disk.
    #[test]
    fn decoder_and_database_come_back() {
        let mut before = machine();
        before.decoders.insert(
            0x100,
            DecoderState {
                fed: vec![1, 2, 3, 4],
                bitmap: Some(0x200),
                transparent: true,
            },
        );
        before.decoders.insert(
            0x101,
            DecoderState {
                fed: Vec::new(),
                bitmap: None,
                transparent: false,
            },
        );

        // A real database, on disk, with one row written, in the machine's device directory: the
        // state only reopens what the game can reach.
        let device = std::env::temp_dir().join(format!("zeebx-teste-do-banco-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&device);
        std::fs::create_dir_all(&device).expect("create the test device");
        let path = device.join("banco.db");
        let soundfont = crate::brew::sql::Database::open(&path).expect("open the database");
        soundfont
            .exec("CREATE TABLE t(a INTEGER, b TEXT)")
            .expect("create the table");
        soundfont
            .exec("INSERT INTO t VALUES (7, 'sete')")
            .expect("write the row");
        before.databases.insert(0x300, soundfont);

        let file = before.save_state();
        let mut after = machine();
        after.vfs.set_device_root(&device);
        after.restore_state(&file).expect("restored");

        let decoder = after.decoders.get(&0x100).expect("the decoder came back");
        assert_eq!(decoder.fed, vec![1, 2, 3, 4]);
        assert_eq!(decoder.bitmap, Some(0x200));
        assert!(decoder.transparent);
        let empty = after.decoders.get(&0x101).expect("the second one came back");
        assert_eq!(empty.bitmap, None, "the absence of a bitmap did not turn into zero");
        assert!(!empty.transparent);

        let reopened = after.databases.get(&0x300).expect("the database came back");
        let lines = reopened
            .exec("SELECT a, b FROM t")
            .expect("read the row back");
        assert_eq!(lines.len(), 1, "the row did not survive the round trip");
        assert_eq!(lines[0].values[0].as_deref(), Some("7"));
        assert_eq!(lines[0].values[1].as_deref(), Some("sete"));

        let _ = std::fs::remove_dir_all(&device);
    }

    /// **An MD5 in the middle of a computation comes back, and the digest comes out right.**
    ///
    /// It is the test that gives meaning to the decision: instead of checking fields, it
    /// **computes the digest halfway**. It starts an MD5, saves in the middle, loads into a new
    /// engine, finishes the computation and compares it with the digest of the same input computed
    /// in one go. If the state did not come back whole, the digest would come out different — and
    /// nothing would point to the save state.
    #[test]
    fn an_md5_mid_computation_comes_back_and_finishes_right() {
        let message: Vec<u8> = (0..200u8).collect();
        // The digest of the whole message, computed in one go.
        let mut integer = crate::brew::crypto::Md5::new();
        integer.update(&message);

        let mut before = machine();
        let mut md5 = crate::brew::crypto::Md5::new();
        md5.update(&message[..137]);
        before.hashes.insert(0x500, HashState { md5 });

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        // `finish` consumes the state, so the MD5 leaves the map: that is how the engine uses it.
        let mut resumed = after.hashes.remove(&0x500).expect("the hash came back").md5;
        resumed.update(&message[137..]);
        assert_eq!(
            resumed.finish(),
            integer.finish(),
            "the resumed digest does not match the one computed in one go"
        );
    }

    /// The interrupted segment, the pending calls and the EGL surfaces.
    #[test]
    fn slice_calls_and_surfaces_round_trip() {
        let mut before = machine();
        let mut registers = [0u32; 15];
        registers[3] = 0xabcd;
        before.interrupted_segment = Some((0x1000, registers));
        before.pending_calls.push(GuestCall {
            function: 0x2000,
            args: [1, 2, 3, 4],
        });
        before.egl_surfaces.insert(0x3000, (640, 480));

        let file = before.save_state();
        let mut after = machine();
        after.restore_state(&file).expect("restored");

        let (address, restored) = after.interrupted_segment.expect("the segment came back");
        assert_eq!(address, 0x1000);
        assert_eq!(restored[3], 0xabcd, "the segment's register did not come back");
        assert_eq!(after.pending_calls.len(), 1);
        assert_eq!(after.pending_calls[0].function, 0x2000);
        assert_eq!(after.pending_calls[0].args, [1, 2, 3, 4]);
        assert_eq!(after.egl_surfaces.get(&0x3000), Some(&(640, 480)));
    }

    /// A missing segment comes back missing, not as address zero.
    #[test]
    fn a_missing_segment_does_not_become_address_zero() {
        let mut before = machine();
        before.interrupted_segment = None;
        let file = before.save_state();
        let mut after = machine();
        after.interrupted_segment = Some((0x7777, [0; 15]));
        after.restore_state(&file).expect("restored");
        assert_eq!(after.interrupted_segment, None);
    }
}
