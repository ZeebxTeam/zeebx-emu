//! IUnzipAStream and the streams: the inflate and the chunked reads.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IUnzipAStream` (`AEECLSID_UNZIPSTREAM` = `0x01001014`), from `sdk/inc/AEEUnzipStream.h`.
    ///
    /// Extends `IAStream` to read a compressed stream as if it were plain data: the game builds a
    /// stream over the compressed bytes, hands it over in `SetStream`, and reads from here. It is
    /// what Double Dragon uses to open `data.ggz` and `sound.ggz`.
    ///
    /// Decompression is done **all at once**, on the first read, and the result is kept. BREW
    /// decompresses as it reads, but the visible effect is the same, and doing it at once avoids
    /// keeping partial inflate state between calls.
    pub(super) fn unzip_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::UnzipStream.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.unzips.remove(&this);
                }
                remaining
            }
            // void SetStream(IUnzipAStream *, IAStream *pInIAStream)
            "SetStream" => {
                self.unzips.insert(
                    this,
                    UnzipState {
                        source: a1,
                        ..UnzipState::default()
                    },
                );
                SUCCESS
            }
            // The whole content is already in memory, so there is always something to read.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                self.expand_unzip(this)?;
                let Some(state) = self.unzips.get(&this) else {
                    return Ok(Some(EFAILED));
                };
                let want = (a2 as usize).min(state.output.len().saturating_sub(state.position));
                if want > 0 {
                    let at = state.position;
                    let bytes = state.output[at..at + want].to_vec();
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(state) = self.unzips.get_mut(&this) {
                        state.position += want;
                    }
                }
                want as u32
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Reads **everything** left in a guest `IAStream`.
    ///
    /// `None` when the object is not a stream we know how to read from the inside — which is
    /// different from a read error, which comes as `Some(Err(..))`.
    pub(super) fn drain_stream(&mut self, source: u32) -> Option<Result<Vec<u8>, CpuError>> {
        if let Some(stream) = self.streams.get(&source).copied() {
            let available = stream.size.saturating_sub(stream.position);
            return Some(self.read_bytes(stream.buffer + stream.position, available));
        }
        // An open `IFile` is also an `IAStream`, and that is how Double Dragon hands over
        // `data.ggz`: we read from the current position to the end, as a sequence of `Read`s would.
        let open = self.open_files.get_mut(&source)?;
        let mut bytes = Vec::new();
        Some(
            match std::io::Read::read_to_end(&mut open.file, &mut bytes) {
                Ok(_) => Ok(bytes),
                Err(_) => Ok(Vec::new()),
            },
        )
    }

    /// Decompresses the input of an `IUnzipAStream`, only once.
    pub(super) fn expand_unzip(&mut self, this: u32) -> Result<(), CpuError> {
        let Some(state) = self.unzips.get(&this) else {
            return Ok(());
        };
        if state.expanded {
            return Ok(());
        }
        let source = state.source;
        if let Some(state) = self.unzips.get_mut(&this) {
            state.expanded = true;
        }

        // The input is an `IAStream`, and in BREW both a memory block and an open file serve as
        // one. Double Dragon hands over the `IFile` of `data.ggz` directly.
        let Some(compressed) = self.drain_stream(source) else {
            // Saying *which* interface arrived is what turns this from "it didn't work" into a
            // lead: the next source to support is the one that shows up here.
            let kind = self
                .objects
                .kind_of(source)
                .map(Interface::name)
                .unwrap_or("unknown");
            self.bad_pointers.insert(format!(
                "an IUnzipAStream received as input an object of {kind}, which we cannot read yet"
            ));
            return Ok(());
        };
        let compressed = compressed?;
        match inflate(&compressed) {
            Some(output) => {
                if let Some(state) = self.unzips.get_mut(&this) {
                    state.output = output;
                }
                // The input stream was consumed in full.
                if let Some(stream) = self.streams.get_mut(&source) {
                    stream.position = stream.size;
                }
            }
            None => {
                self.assumptions
                    .insert("an IUnzipAStream received data that does not decompress");
            }
        }
        Ok(())
    }

    /// `IMemAStream` (`AEECLSID_MEMASTREAM` = `0x0100100c`), from `sdk/inc/AEE.h`.
    ///
    /// Presents a memory block as a stream: that is how BREW hands data to an image decoder. Here
    /// the "asynchronous" part is trivial — the bytes are all already in memory, so `Readable` can
    /// notify the listener right away.
    pub(super) fn stream_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::MemAStream.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.release_stream(this);
                }
                remaining
            }
            // void Set(IMemAStream *, byte *pBuff, uint32 dwSize, uint32 dwOffset,
            //          boolean bSysMem)
            //
            // **The buffer becomes the stream's**, and the stream gives it back when destroyed or
            // when it receives another one — `bSysMem` only says which heap it came from, and here
            // both are the same. Action Hero 3D builds 7616-byte bitmaps, several per frame, hands
            // them to the stream and never touches them again: without this it was 1.9 MB per
            // second of gameplay, and within three minutes `MALLOC` returned null.
            //
            // `SetEx` replaces `bSysMem` with a `pfnFree` from the game, and that one is left out:
            // calling guest code in the middle of a dispatch is the path that has crashed games
            // before.
            "Set" | "SetEx" => {
                // Setting the same buffer again is not replacing it.
                if self.streams.get(&this).is_some_and(|s| s.buffer == a1) {
                    self.streams.remove(&this);
                }
                self.release_stream(this);
                self.streams.insert(
                    this,
                    MemStream {
                        buffer: a1,
                        size: a2,
                        position: a3.min(a2),
                        owner: name == "Set",
                    },
                );
                SUCCESS
            }
            // void Readable(IAStream *, void (*pfnNotify)(void *), void *pUser)
            //
            // The stream is always ready, so the callback goes straight to the queue — delivering
            // it here would re-enter the guest in the middle of the dispatch.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                let Some(stream) = self.streams.get(&this).copied() else {
                    return Ok(Some(EFAILED));
                };
                let want = a2.min(stream.size.saturating_sub(stream.position));
                if want > 0 {
                    let bytes = self.read_bytes(stream.buffer + stream.position, want)?;
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(stream) = self.streams.get_mut(&this) {
                        stream.position += want;
                    }
                }
                want
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Forgets the buffer of an `IMemAStream`, returning it to the heap when it belongs to the
    /// stream.
    fn release_stream(&mut self, this: u32) {
        if let Some(stream) = self.streams.remove(&this)
            && stream.owner
        {
            self.free(stream.buffer);
        }
    }
}
