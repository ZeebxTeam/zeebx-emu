//! IWeb and ISource: the network, the request, and the response handed back to the game.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Serves `ISource` and `IPeek`. See [`Interface::Peek`].
    ///
    /// Of `IPeek`, only slot 8 has a body, and it is the reason all of this exists: the Z-Wheel
    /// reads `tectoy.cfg` line by line through it. The call is `slot8(this, &pair, 3)`, with
    /// `pair` being `{pointer, size}` — the game reads both and **copies** the text before asking
    /// for the next line, which is what lets a single buffer be reused.
    ///
    /// **The return value is a hypothesis, and it is measured.** The loop at `0x884fc` continues
    /// while `-return >= 2` and stops at `-3`; returning zero would end it on the first line,
    /// even on an empty line. So: `1` while there is a line, `-3` at the end. Whether the console
    /// returns the same `1` is unknown — what is known is the loop condition.
    pub(super) fn source_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        /// "Out of lines": the only value the loop at `0x88338` accepts as the end.
        const NO_MORE_LINES: u32 = (-3i32) as u32;
        /// "Got a line". See the note about the return value, above.
        const GOT_LINE: u32 = 1;

        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        if aee::is_placeholder(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.sources.remove(&this);
                    // The line buffer is ours, the size of the whole source.
                    if let Some(peek) = self.peeks.remove(&this) {
                        self.free(peek.buffer);
                    }
                }
                remaining
            }
            "QueryInterface" => {
                let output = self.cpu.read_reg(Reg::R2);
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int32 ISOURCE_Read(ISource *po, char *pcBuf, int32 cbBuf)`.
            "Read" => {
                let (dest, fits) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                let Some(bytes) = self.sources.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let chunk = bytes[..bytes.len().min(fits)].to_vec();
                self.cpu.write_mem(dest, &chunk)?;
                self.sources.insert(this, bytes[chunk.len()..].to_vec());
                chunk.len() as u32
            }
            "LerLinha" => {
                let pair = self.cpu.read_reg(Reg::R1);
                let Some(reader) = self.peeks.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let Some(line) = reader.next_line() else {
                    return Ok(Some(NO_MORE_LINES));
                };
                let (buffer, size) = (reader.buffer, line.len() as u32);
                self.cpu.write_mem(buffer, &line)?;
                self.cpu.write_mem(buffer + size, &[0])?;
                if pair != 0 {
                    self.cpu.write_u32(pair, buffer)?;
                    self.cpu.write_u32(pair + 4, size)?;
                }
                GOT_LINE
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Serves `ISourceUtil`. See [`Interface::SourceUtil`].
    pub(super) fn source_util_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SourceUtil.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let output = self.cpu.read_reg(Reg::R2);
                if output != 0 {
                    self.cpu.write_u32(output, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int PeekSourceFromSource(ISourceUtil *po, ISource *ps, int nMax, IPeek **ppo)`.
            //
            // `nMax` is the cap on what the reader may keep in memory; the Z-Wheel passes the
            // file size plus one, that is, the whole file. Since we already have all the bytes,
            // it changes nothing here — but it is what tells us the game expects to read it all.
            "PeekSourceFromSource" => {
                let (source, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R3));
                let Some(bytes) = self.sources.get(&source).cloned() else {
                    return Ok(Some(EBADPARM));
                };
                let reader = self.new_object(Interface::Peek)?;
                if reader == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // The line buffer lives with the reader: the game gets a pointer to it and
                // **copies** the contents before asking for the next one, so a single reused
                // buffer is enough. Reserving the size of the source guarantees the longest
                // possible line fits.
                let buffer = self.alloc_internal(bytes.len() as u32 + 1)?;
                if buffer == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.peeks.insert(
                    reader,
                    Peek {
                        bytes,
                        position: 0,
                        buffer,
                    },
                );
                if output != 0 {
                    self.cpu.write_u32(output, reader)?;
                }
                SUCCESS
            }
            // `int SourceFromFile(ISourceUtil *po, IFile *pf, ISource **ppo)`.
            //
            // We read the whole file by its path, not through the open descriptor, so as not to
            // move the position of the game's `IFile` — it still belongs to the game. It is the
            // path of what was opened, not the game's name resolved again: the two diverge on
            // the `tectoy.cfg` without end of life, and rereading by name read the factory one.
            //
            // **`IFile` is an `IAStream`**, and the method accepts any of them. Aviãozinho, a
            // fan-made Quake port, passes an `IAStream` of its own whose `Read` returns what the
            // mixer has just mixed, and plays the resulting `ISource` on a PCM `IMedia`.
            // Refused, `SNDDMA_Init` gave up and `S_Init` read the null sound buffer. The
            // `Read` of `IAStream` and that of `ISource` sit in the same slot, with the same
            // arguments and the same return, so the stream itself serves as the source: the
            // player calls the game's code, as it already does with a game `ISource`.
            "SourceFromFile" => {
                let (file, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some(path) = self
                    .open_files
                    .get(&file)
                    .map(|opened| opened.path.clone())
                else {
                    if file == 0 || self.objects.kind_of(file).is_some() {
                        return Ok(Some(EBADPARM));
                    }
                    if output != 0 {
                        self.cpu.write_u32(output, file)?;
                    }
                    return Ok(Some(SUCCESS));
                };
                let Ok(bytes) = std::fs::read(path) else {
                    return Ok(Some(EFAILED));
                };
                let source = self.new_object(Interface::Source)?;
                if source == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.sources.insert(source, bytes);
                if output != 0 {
                    self.cpu.write_u32(output, source)?;
                }
                SUCCESS
            }
            // `int SourceFromMemory(ISourceUtil *po, const void *pBuf, int nSize,
            //                       PFNNOTIFY pfn, void *pUser, ISource **ppo)`.
            //
            // This is where the Zeeboids bridge comes in, and it is worth explaining why. The
            // method sends nothing: it wraps a chunk of memory in an `ISource` so that `IWeb`
            // can read it. But the chunk of memory, in Zeeboids, **is the POST body** — and
            // this is the only point where it exists whole and still in the clear.
            //
            // In the firmware it registers the pair stored by `SetHandler` and queues the work
            // on the internal object. Here we do the work right away: `r1` and `r2` are the body
            // and the size, and the response comes back through the output pointer.
            //
            // **When it is not a send, it is what the name says.** Prey Evil wraps 715 KB of
            // data from its own package here, with no network at all: treating every call as a
            // POST returned `EFAILED`, it was left without its data source and drew a black
            // screen every frame. With no URL in the caller's object, the chunk of memory
            // becomes an `ISource`, which is the method's contract.
            "SourceFromMemory" => {
                let (body, size) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let output = self.stack_arg(1)?;
                match self.send_request(body, size, output)? {
                    Some(request_result) => request_result,
                    None => self.source_from_memory(body, size, output)?,
                }
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Wraps a chunk of the game's memory in an `ISource`, which is the contract of
    /// `SourceFromMemory`.
    pub(super) fn source_from_memory(
        &mut self,
        body: u32,
        size: u32,
        output: u32,
    ) -> Result<u32, CpuError> {
        let bytes = self.read_bytes(body, size)?;
        let source = self.new_object(Interface::Source)?;
        if source == 0 {
            return Ok(ENOMEMORY);
        }
        self.sources.insert(source, bytes);
        if output != 0 {
            self.cpu.write_u32(output, source)?;
        }
        Ok(SUCCESS)
    }

    /// Sends the body the game handed to the dispatcher and stores the response.
    ///
    /// **Finding the URL is the delicate step.** It does not come in the arguments: it lives in
    /// the game's `ConnectionManager` object, which is who called us. Instead of hardcoding an
    /// offset — Zeeboids' is `+0x244`, and it would hold only for it —, we search the object for
    /// the **triple** `{url, body, size}` whose last two words are exactly the arguments we just
    /// received. This checks itself: a triple that matches the pointer and size received is no
    /// coincidence, and one that does not match is discarded.
    ///
    /// `r4` belongs to the caller — in ARM it is preserved by the called function, so at the
    /// call boundary it still holds the caller's object.
    ///
    /// `None` when the body is not a send — that is, when there is no URL in the caller's
    /// object.
    pub(super) fn send_request(
        &mut self,
        body: u32,
        size: u32,
        output: u32,
    ) -> Result<Option<u32>, CpuError> {
        let object = self.cpu.read_reg(Reg::R4);
        let mut found = None;
        for i in 0..MAX_OBJECT_FIELDS {
            let base = object + i * 4;
            if self.cpu.read_u32(base + 4) == Ok(body)
                && self.cpu.read_u32(base + 8) == Ok(size)
                && let Ok(pointer) = self.cpu.read_u32(base)
            {
                let text = self.cpu.read_cstring(pointer, MAX_STRING);
                if text.starts_with("http") {
                    found = Some((text, base));
                    break;
                }
            }
        }
        let Some((url, base)) = found else {
            return Ok(None);
        };

        let data = self.read_bytes(body, size.min(MAX_REQUEST_BODY))?;
        if !self.network {
            self.web_requests
                .insert(format!("{url} ({size} bytes, network off)"));
            return Ok(Some(EFAILED));
        }
        // The output area has no known format yet: what the console put there is found out by
        // watching the game read it. We store the response and leave the pointer as it is,
        // instead of writing a guessed struct over the game's memory.
        let _ = output;
        self.log_body("request", &data);
        let (result, state) = match host_net::post(&url, &data, self.network_to.as_deref()) {
            Ok(response) => {
                self.web_requests.insert(format!(
                    "{url} -> {} ({} response bytes)",
                    response.status,
                    response.body.len()
                ));
                self.log_body("response", &response.body);
                self.web_response = response.body;
                (SUCCESS, STATE_RECEIVING)
            }
            Err(error) => {
                self.web_requests.insert(format!("{url} -> failed: {error}"));
                (EFAILED, STATE_FAILED)
            }
        };
        // Nothing is delivered here. We are in the middle of dispatching a call, with the game
        // inside `ConnectionManager::init`, and that is how the bridge crashed the game: calling
        // its allocator at that moment reenters a manager that is in the middle of an operation.
        //
        // The emulator already has the right boundary for this — the same one as the signals,
        // which runs outside dispatch, when the guest is not inside anything. The response
        // waits in the queue until then.
        self.pending_response = Some((object, base, state));
        Ok(Some(result))
    }

    /// Deposits the response and signals the state, outside dispatch.
    ///
    /// The order matters: the fields first, the state after. The game checks the state on every
    /// turn of the loop and, on seeing it at "receiving", reads the count — if we signaled
    /// before delivering, it would read zero and conclude that nothing came.
    pub(super) fn flush_response(&mut self) -> Result<(), CpuError> {
        // The end of the stream is announced at the boundary after the delivery, to give the
        // game a whole turn in which it consumes the fields.
        if let Some(response) = self.pending_end.take() {
            self.cpu.write_mem(response + END_OF_STREAM, &[1])?;
        }
        let Some((object, base, state)) = self.pending_response.take() else {
            return Ok(());
        };
        if state == STATE_RECEIVING {
            self.deliver_response(object)?;
        }
        self.finish_request(base, state)
    }

    /// Delivers the response to the object the game prepared to receive it.
    ///
    /// The sender stores that object at its own `+8` (`str r2, [r4, #8]` at `0x94eb8`), and the
    /// handler for the "receiving" state reads it like this (`0x85b44`): `[+8]` is the field
    /// count and zero means "nothing came", `[+4]` is the pointer array and `[+0xc]` the
    /// capacity. It is the game's own `ttdArray`.
    ///
    /// The strings **come from the game's allocator**, through [`crate::bridge`], because that
    /// is where the memory manager expects to get them back from. With no bridge declared for
    /// the module, we deliver nothing: better for the game to see "no response came" than to
    /// see memory it will reject. What goes over the wire does not change in either case.
    pub(super) fn deliver_response(&mut self, object: u32) -> Result<(), CpuError> {
        // The bridge **comes on**, and is turned off by `ZEEBX_NO_BRIDGE` (see how the `bridge`
        // field is built in `machine::mod`). The comment here used to say the opposite — "it is
        // optional and comes off" — and that sentence described an earlier version: whoever read
        // only this spot would conclude delivery is disabled by default, the opposite of what
        // happens.
        //
        // The caution that motivated the sentence still holds, which is why it was not just
        // deleted: the bridge touches the game's memory, and a wrong delivery does not fail on
        // the spot — it corrupts and breaks later, as has already happened. The array holds
        // `ttdString` objects, with the length at `[0]` and the text at `[4]`, and delivering raw
        // text made the destructor free garbage. With the bridge off the game sees "no response
        // came", which is a state it knows how to handle.
        if !self.bridge {
            return Ok(());
        }
        let Some(bridge) = bridge::to(self.applet_class) else {
            self.delivered.push(format!(
                "no bridge declared for module {:#010x}",
                self.applet_class
            ));
            return Ok(());
        };
        // An empty body **is** a response: the server may have nothing to return. What it must
        // not do is stay silent, or the game waits forever for an end of stream that never
        // comes. So the path is the same, with empty text.
        // **The `import` body comes compressed, and only that one.** On the console the game
        // itself inflates it, with an `AEECLSID_UNZIPSTREAM` it creates at `0x93454` and reads in
        // 499-byte blocks; the bridge skips the console's dispatcher, so it skips that step
        // along with it and has to inflate here.
        //
        // Without this, the compressed bytes went through `from_utf8_lossy`, every invalid byte
        // became a three-byte `U+FFFD` — 416 bytes of response became 742 of "text" — and the
        // game's parser, which splits on `;`, found **one** field. The avatar arrived, the game
        // reported success and saved nothing.
        //
        // Trying to inflate what is not compressed does no harm: `export` and `synchronize`
        // answer plain text, and the inflater rejects plain text.
        // Copies, does not consume: the same body still feeds the normal `IWeb` read.
        let body = inflate(&self.web_response).unwrap_or_else(|| self.web_response.clone());
        let text = String::from_utf8_lossy(&body)
            .trim_end_matches(['\r', '\n', '\0'])
            .to_string();

        // The game does the splitting. Its parser reads the text at `+0x20`, appends the chunk
        // it receives and splits on `;`, filling the array with memory from its own allocator
        // and setting the flags the consumer expects — including the "I read what I needed"
        // one, which was what was missing. Reproducing this by hand was the earlier mistake: we
        // delivered fields it never recognized as complete, and the game waited forever.
        //
        // So we give only the text, in the place where it looks for it, and tell it to split.
        let response = self.cpu.read_u32(object + 8)?;
        if response == 0 {
            self.delivered
                .push("rejected: the response object does not exist".to_string());
            return Ok(());
        }
        let bytes = text.as_bytes();
        let buffer = self.alloc_in_game(bridge, bytes.len() as u32 + 1)?;
        if buffer == 0 {
            return Ok(());
        }
        self.cpu.write_mem(buffer, bytes)?;
        self.cpu.write_mem(buffer + bytes.len() as u32, &[0])?;

        // Whatever was already there is returned to the allocator, or it leaks.
        let previous = self.cpu.read_u32(response + RESPONSE_TEXT)?;
        if previous != 0 {
            self.call_guest_with_stack(bridge.free_fn, [previous, 0, 0, 0], &[], BRIDGE_BUDGET)?;
        }
        self.cpu.write_u32(response + RESPONSE_TEXT, buffer)?;

        // The parser's second argument is the chunk to append. The whole text is already in
        // place, so an empty string goes.
        let empty = self.alloc_in_game(bridge, 1)?;
        if empty == 0 {
            return Ok(());
        }
        self.cpu.write_mem(empty, &[0])?;
        self.call_guest_with_stack(bridge.parser, [response, empty, 0, 0], &[], BRIDGE_BUDGET)?;

        // The "new data arrived" flag. The handler at `0x85b5c` only interprets field 0 when it
        // is set, and clears it right after — it is a one-way flag, and on the console the one
        // that set it was the dispatcher.
        self.cpu.write_mem(response + 0x18, &[1])?;

        // The "end of stream" flag is left for the **next** boundary, and that wait is the point.
        //
        // `ConnectionManager` receives in chunks: for each one it calls the parser, and only when
        // a zero-byte one arrives does it set the flag. Between one chunk and the next the game
        // checks the response and **consumes the fields**. Setting it together with the
        // delivery skipped that consumption — the handler at `0x85c9c` branches straight to the
        // end when the flag is already there, returns success and the avatar is left without
        // the numbers.
        //
        // We deliver everything at once, so we imitate the gap: the text now, the end later.
        self.pending_end = Some(response);

        let fields = self.cpu.read_u32(response + 8).unwrap_or(0);
        self.delivered.push(format!(
            "delivered to the game's parser: {} bytes, {fields} field(s) recognized",
            bytes.len()
        ));
        Ok(())
    }

    /// Asks the game's own allocator for memory, through the bridge.
    ///
    /// The observed signature is `(size, pool, line, file, 1)`, with pool zero — which it
    /// requires to be below 32 — and the two in the middle serving its origin tracking.
    pub(super) fn alloc_in_game(
        &mut self,
        bridge: bridge::Bridge,
        size: u32,
    ) -> Result<u32, CpuError> {
        let outcome = self.call_guest_with_stack(
            bridge.allocator,
            [size, 0, ORIGIN_LINE, bridge.origin],
            &[1],
            BRIDGE_BUDGET,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(code),
            _ => {
                self.assumptions
                    .insert("the response was not delivered: the game's allocator did not return");
                Ok(0)
            }
        }
    }

    /// Tells the game the request has finished.
    ///
    /// It waits for no event: the sync screen **polls a state field** in the requester's
    /// object, and while it is 0 or 2 it keeps showing "Connecting". With 9 it moves to
    /// "ReceivingData" and reads the response; with 11 it goes to the failure branch.
    ///
    /// The field sits twelve bytes before the URL, in the same object — so it is found through
    /// the same anchor that was already checked, not through a loose offset.
    ///
    /// **The safeguard is here**: we only write if the field currently holds one of the values
    /// the game's own code treats as "in progress". If it holds anything else, the anchor is not
    /// what we think and we do not touch its memory. Writing a guess over guest memory is the
    /// kind of mistake that costs dearly and late.
    pub(super) fn finish_request(&mut self, base: u32, state: u32) -> Result<(), CpuError> {
        let field = base - STATE_OFFSET_BEFORE_URL;
        match self.cpu.read_u32(field) {
            Ok(STATE_WORKING | STATE_WORKING_2) => self.cpu.write_u32(field, state),
            _ => {
                self.assumptions
                    .insert("the connection state was not signaled: the field looked unexpected");
                Ok(())
            }
        }
    }

    /// Turns network access on or off.
    pub fn set_network(&mut self, enabled: bool) {
        self.network = enabled;
    }

    /// Turns on the module bridge, which delivers the response to the game. On by default
    /// (`ZEEBX_NO_BRIDGE` turns it off).
    pub fn set_bridge(&mut self, enabled: bool) {
        self.bridge = enabled;
    }

    /// Redirects connections to another machine or port, without touching what the game asked
    /// for.
    pub fn set_network_to(&mut self, dest: Option<String>) {
        self.network_to = dest;
    }

    /// The responses the bridge delivered to the game.
    pub fn delivered(&self) -> &[String] {
        &self.delivered
    }

    /// The body of the last HTTP response received.
    pub fn web_response(&self) -> &[u8] {
        &self.web_response
    }

    pub fn web_requests(&self) -> Vec<String> {
        self.web_requests.iter().cloned().collect()
    }
}
