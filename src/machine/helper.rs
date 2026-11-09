//! BREW's helper functions: printf, qsort and string conversion.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `qsort` from BREW's stdlib, with the comparison done by the game.
    ///
    /// Sorts **in guest memory**, really swapping the elements around, so that the pointers the
    /// comparison function receives are the real addresses inside the array — which is what C's
    /// `qsort` does and what a comparator can observe.
    ///
    /// The algorithm is heapsort: it sorts in place, with no extra memory, and makes `n log n`
    /// comparisons. Since each comparison costs an entry into the guest, their number is what
    /// matters here — an insertion sort would be simple but quadratic, and a large array would
    /// cost dearly.
    pub(super) fn qsort(
        &mut self,
        base: u32,
        count: u32,
        size: u32,
        compare: u32,
    ) -> Result<(), CpuError> {
        if base == 0 || compare == 0 || size == 0 || count < 2 {
            return Ok(());
        }
        // Re-entering the guest requires nesting room, as with callbacks.
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("a qsort was skipped because of too-deep nesting");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let result = self.heapsort(base, count as usize, size, compare);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        result
    }

    pub(super) fn heapsort(
        &mut self,
        base: u32,
        count: usize,
        size: u32,
        compare: u32,
    ) -> Result<(), CpuError> {
        for start in (0..count / 2).rev() {
            self.sift_down(base, size, compare, start, count)?;
        }
        for end in (1..count).rev() {
            self.swap_elements(base, size, 0, end)?;
            self.sift_down(base, size, compare, 0, end)?;
        }
        Ok(())
    }

    /// Pushes the element at `root` down until the heap holds again.
    pub(super) fn sift_down(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        mut root: usize,
        end: usize,
    ) -> Result<(), CpuError> {
        loop {
            let child = root * 2 + 1;
            if child >= end {
                return Ok(());
            }
            let mut largest = child;
            if child + 1 < end && self.compare_elements(base, size, compare, child, child + 1)? < 0
            {
                largest = child + 1;
            }
            if self.compare_elements(base, size, compare, root, largest)? >= 0 {
                return Ok(());
            }
            self.swap_elements(base, size, root, largest)?;
            root = largest;
        }
    }

    /// Calls the game's comparison function with the addresses of the two elements.
    pub(super) fn compare_elements(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        a: usize,
        b: usize,
    ) -> Result<i32, CpuError> {
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let outcome = self.call_guest(compare, [pa, pb, 0, 0], QSORT_BUDGET)?;
        match outcome {
            // A comparison that does not return leaves the order as it is, instead of bringing
            // everything down.
            Outcome::Returned { code } => Ok(code as i32),
            _ => Ok(0),
        }
    }

    pub(super) fn swap_elements(
        &mut self,
        base: u32,
        size: u32,
        a: usize,
        b: usize,
    ) -> Result<(), CpuError> {
        if a == b {
            return Ok(());
        }
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let first = self.read_bytes(pa, size)?;
        let second = self.read_bytes(pb, size)?;
        self.cpu.write_mem(pa, &second)?;
        self.cpu.write_mem(pb, &first)?;
        Ok(())
    }

    /// An argument beyond the four registers, counted from the top of the stack.
    pub(super) fn stack_arg(&self, index: u32) -> Result<u32, CpuError> {
        self.cpu.read_u32(self.cpu.read_reg(Reg::Sp) + index * 4)
    }

    /// Reads an `AECHAR` string — zero-terminated little-endian UTF-16, which is how BREW
    /// represents text.
    /// Reads a wide string from the guest as the raw UTF-16 units.
    ///
    /// It exists apart from the `String` version because `wstrchr` and the comparisons work on
    /// `AECHAR`, and converting to `String` would lose the position of each character.
    pub(super) fn read_aechar_units(&self, addr: u32) -> Result<Vec<u16>, CpuError> {
        if addr == 0 {
            return Ok(Vec::new());
        }
        /// How many units per read. Same reason as `read_cbytes`: each read
        /// goes through the CPU backend, which looks up the region before copying — 57 ns to
        /// bring in two bytes. Sixty-four units cover the typical string in one go.
        const BLOCK: usize = 64;

        let mut units = Vec::new();
        while units.len() < MAX_STRING {
            let wanted = BLOCK.min(MAX_STRING - units.len());
            let base = addr + units.len() as u32 * 2;
            let mut bytes = vec![0u8; wanted * 2];
            if self.cpu.read_mem(base, &mut bytes).is_err() {
                // The block may cross the end of the mapped region, and then the whole read fails
                // even when there are valid units before it. Two by two only in this case.
                for i in 0..wanted {
                    let mut pair = [0u8; 2];
                    if self.cpu.read_mem(base + i as u32 * 2, &mut pair).is_err() {
                        return Ok(units);
                    }
                    match u16::from_le_bytes(pair) {
                        0 => return Ok(units),
                        unit => units.push(unit),
                    }
                }
                continue;
            }
            for pair in bytes.chunks_exact(2) {
                match u16::from_le_bytes([pair[0], pair[1]]) {
                    0 => return Ok(units),
                    unit => units.push(unit),
                }
            }
        }
        Ok(units)
    }

    pub(super) fn read_aechar(&self, addr: u32) -> Result<String, CpuError> {
        // It was the same loop as `read_aechar_units` written again. The only difference is
        // what is done with the units at the end.
        Ok(String::from_utf16_lossy(&self.read_aechar_units(addr)?))
    }

    /// BREW stdlib helpers, dispatched by slot name — the table comes from
    /// `struct AEEHelperFuncs`, in the SDK's `AEEStdLib.h`.
    pub(super) fn helper_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Helpers.method(slot) else {
            return Ok(None);
        };
        let (a0, a1, a2) = (
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
        );
        let result = match name {
            // void *SetupNativeImage(AEECLSID cls, void *pBuffer, AEEImageInfo *pii,
            //                         boolean *pbRealloc)
            //
            // Converts an encoded image to the device's native format. It is what sits
            // behind the SDK's `CONVERTBMP`, and the two games that call it pass
            // `AEECLSID_WINBMP` with a Windows bitmap.
            "SetupNativeImage" => self.setup_native_image(a1, a2, self.cpu.read_reg(Reg::R3))?,
            "malloc" => {
                let r = self.malloc(a0)?;
                r
            }
            "free" => {
                self.free(a0);
                SUCCESS
            }
            "realloc" => self.realloc(a0, a1 & !ALLOC_NO_ZMEM)?,
            "memmove" => {
                self.copy_guest(a0, a1, a2)?;
                a0
            }
            "memset" => {
                if a2 > 0 {
                    // In blocks: see [`CpuBackend::fill_mem`] for the reason, which is measured.
                    self.cpu.fill_mem(a0, a1 as u8, a2)?;
                    self.cpu.mark_dirty(a0, a2);
                }
                a0
            }
            "memcmp" => {
                let (left, right) = (self.read_bytes(a0, a2)?, self.read_bytes(a1, a2)?);
                cmp_to_int(left.cmp(&right))
            }
            // C string functions count **bytes**, not characters: any byte above
            // 0x7f would make the `String` version lie.
            "strlen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "strcpy" => {
                let mut src = self.cpu.read_cbytes(a1, MAX_STRING);
                src.push(0);
                self.cpu.write_mem(a0, &src)?;
                a0
            }
            "strcat" => {
                let base = self.cpu.read_cbytes(a0, MAX_STRING).len() as u32;
                let mut extra = self.cpu.read_cbytes(a1, MAX_STRING);
                extra.push(0);
                self.cpu.write_mem(a0 + base, &extra)?;
                a0
            }
            "strcmp" => {
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                cmp_to_int(left.cmp(&right))
            }
            "strncmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                // `strncmp` compares at most `n` bytes; a shorter string counts in full.
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "wstrlen" => self.read_aechar(a0)?.encode_utf16().count() as u32,
            "wstrsize" => (self.read_aechar(a0)?.encode_utf16().count() as u32 + 1) * 2,
            // AECHAR *strtowstr(const char *pszIn, AECHAR *pDest, int nSize) — `nSize` is in
            // bytes, not characters.
            "strtowstr" => {
                let text = self.cpu.read_cstring(a0, MAX_STRING);
                self.write_aechar(a1, &text, a2 as usize / 2)?;
                a1
            }
            // char *wstrtostr(const AECHAR *pIn, char *pszDest, int nSize)
            "wstrtostr" => {
                let text = self.read_aechar(a0)?;
                self.write_cstring_limited(a1, &text, a2 as usize)?;
                a1
            }
            "wstrcpy" => {
                let text = self.read_aechar(a1)?;
                self.write_aechar(a0, &text, usize::MAX)?;
                a0
            }
            "wstrcat" => {
                let base = self.read_aechar(a0)?;
                let extra = self.read_aechar(a1)?;
                let offset = base.encode_utf16().count() as u32 * 2;
                self.write_aechar(a0 + offset, &extra, usize::MAX)?;
                a0
            }
            "wstrcmp" => {
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.cmp(&right))
            }
            "wstrncmp" => {
                let take = a2 as usize;
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.get(..take).cmp(&right.get(..take)))
            }
            // char *strncpy(char *dst, const char *src, size_t n) — no terminator if `src`
            // does not fit, as the original function says.
            "strncpy" => {
                let n = a2 as usize;
                let mut bytes = self.cpu.read_cbytes(a1, MAX_STRING);
                bytes.truncate(n);
                bytes.resize(n, 0);
                if n > 0 {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            "strchr" | "strrchr" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let found = if name == "strchr" {
                    text.iter().position(|&b| b == needle)
                } else {
                    text.iter().rposition(|&b| b == needle)
                };
                found.map(|i| a0 + i as u32).unwrap_or(0)
            }
            // char *strstr(const char *s, const char *needle)
            //
            // **An empty needle matches at the start.** That is what C says — every string contains
            // the empty string — and it is not a spec curiosity: Need For Speed registers the
            // game's sounds by looking up each one's name in a table, and one of its five names is
            // the empty string. Returning "not found", it scanned the seventy entries, concluded
            // that the sound does not exist, printed `ZeeboSnd.cpp:327 BREAKPOINT!` and went into a
            // jump to itself — on purpose. The game was not slow: it was stuck, and the instruction
            // budget ran out on that.
            "strstr" => {
                let haystack = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                let match_addr = match needle.is_empty() {
                    true => a0,
                    false => haystack
                        .windows(needle.len())
                        .position(|w| w == needle)
                        .map(|i| a0 + i as u32)
                        .unwrap_or(0),
                };
                if self.serial.is_some() {
                    let n = String::from_utf8_lossy(&needle).into_owned();
                    self.log_serial(format!("<strstr {n:?} -> {match_addr:#x}>"));
                }
                match_addr
            }
            "stricmp" => {
                let fold = |addr| {
                    self.cpu
                        .read_cbytes(addr, MAX_STRING)
                        .iter()
                        .map(u8::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                };
                cmp_to_int(fold(a0).cmp(&fold(a1)))
            }
            "memchr" => {
                let bytes = self.read_bytes(a0, a2)?;
                bytes
                    .iter()
                    .position(|&b| b == a1 as u8)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // --- Wide strings (`AECHAR`, UTF-16) -------------------------------------
            "wstrchr" | "wstrrchr" => {
                let units = self.read_aechar_units(a0)?;
                let needle = a1 as u16;
                let found = if name == "wstrchr" {
                    units.iter().position(|&u| u == needle)
                } else {
                    units.iter().rposition(|&u| u == needle)
                };
                found.map(|i| a0 + i as u32 * 2).unwrap_or(0)
            }
            "wstrdup" => {
                let units = self.read_aechar_units(a0)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            "wstrlower" | "wstrupper" => {
                let units = self.read_aechar_units(a0)?;
                let mapped: Vec<u8> = units
                    .iter()
                    .map(|&u| match u8::try_from(u) {
                        Ok(b) if name == "wstrlower" => b.to_ascii_lowercase() as u16,
                        Ok(b) => b.to_ascii_uppercase() as u16,
                        Err(_) => u,
                    })
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                if !mapped.is_empty() {
                    self.cpu.write_mem(a0, &mapped)?;
                }
                a0
            }
            "wstricmp" | "wstrnicmp" => {
                let take = if name == "wstricmp" {
                    usize::MAX
                } else {
                    a2 as usize
                };
                let left = self.read_aechar_units(a0)?;
                let right = self.read_aechar_units(a1)?;
                cmp_to_int(fold_case(&left, take).cmp(&fold_case(&right, take)))
            }
            // `size_t wstrlcpy/wstrlcat(AECHAR *dst, const AECHAR *src, size_t nSize)`: the
            // BSD versions, which return the size the source *would have* taken. `nSize` counts
            // characters, not bytes.
            "wstrlcpy" | "wstrlcat" => {
                let source = self.read_aechar(a1)?;
                let existing = if name == "wstrlcat" {
                    self.read_aechar_units(a0)?.len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_aechar(a0 + existing as u32 * 2, &source, room)?;
                (existing + source.encode_utf16().count()) as u32
            }
            // `AECHAR *wwritelong(AECHAR *pszBuf, long n)` — writes the number and returns the
            // pointer to the terminator, so the caller can continue from there.
            "wwritelong" => {
                let text = (a1 as i32).to_string();
                self.write_aechar(a0, &text, usize::MAX)?;
                a0 + text.len() as u32 * 2
            }
            // `int wstrncopyn(AECHAR *dst, int cbDest, const AECHAR *src, int lenSource)`: `cbDest`
            // counts characters and `lenSource` limits the source (-1 = up to the terminator).
            "wstrncopyn" => {
                let mut units = self.read_aechar_units(a2)?;
                let limit = self.cpu.read_reg(Reg::R3) as i32;
                if limit >= 0 {
                    units.truncate(limit as usize);
                }
                let text = String::from_utf16_lossy(&units);
                self.write_aechar(a0, &text, a1 as usize)?;
                text.encode_utf16()
                    .count()
                    .min((a1 as usize).saturating_sub(1)) as u32
            }
            // `void wsprintf(AECHAR *dst, int nSize, const AECHAR *fmt, ...)` — same
            // grammar as `snprintf`, with source and destination in UTF-16.
            "wsprintf" => {
                let fmt = self.read_aechar(a2)?;
                let text = self.format_wide_from(3, &fmt);
                self.write_aechar(a0, &text, a1 as usize / 2)?;
                SUCCESS
            }
            // `void strexpand(const byte *pSrc, int nCount, AECHAR *pDest, int nSize)`
            "strexpand" => {
                let bytes = self.read_bytes(a0, a1)?;
                let text: String = bytes.iter().map(|&b| b as char).collect();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &text, limit)?;
                SUCCESS
            }
            "wstrtofloat" => {
                let text = self.read_aechar(a0)?;
                let (value, _) = parse_leading_double(&text);
                self.return_double(value)
            }
            // `boolean floattowstr(double val, AECHAR *psz, int nSize)`: the `double` takes
            // `r0:r1`, so the destination lands in `r2` and the size in `r3`.
            "floattowstr" => {
                let value = fmath::from_words(a0, a1);
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &format!("{value}"), limit)?;
                TRUE
            }
            // `boolean utf8towstr(const byte *pszIn, int nLen, AECHAR *pDest, int nSizeBytes)`
            //
            // The terminator only goes in if it fits. The Z-Wheel (`0x8afc8`) allocates `(n+1)*2`
            // bytes and passes `n*2`: it counts on the `n` whole characters and on the zero of the
            // freshly allocated memory. Reserving the terminator inside `nSizeBytes` ate the last
            // letter of every game name in the list — "Alic", "Alien Breake".
            "utf8towstr" => {
                let bytes = if (a1 as i32) < 0 {
                    self.cpu.read_cbytes(a0, MAX_STRING)
                } else {
                    self.read_bytes(a0, a1)?
                };
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                let mut units: Vec<u16> = text.encode_utf16().collect();
                units.truncate(limit);
                if units.len() < limit {
                    units.push(0);
                }
                if a2 != 0 {
                    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
                    self.cpu.write_mem(a2, &bytes)?;
                }
                TRUE
            }
            // `boolean wstrtoutf8(const AECHAR *pszIn, int nLen, byte *pDest, int nSizeBytes)`
            //
            // **Real UTF-8, and the zero only when it fits.** FIFA 09 takes the player's initial by
            // converting one character into a one-byte buffer: reserving room for the zero, the
            // initial came out empty, and the HUD did not show "K. Chafni" — only those with a
            // single name, like "Chris". And this wrote through `write_cstring_limited`, which is
            // Latin-1: the "ô" in "Chantôme" became a stray byte. A character that does not fit
            // whole is left out, so as not to leave half a sequence at the end.
            "wstrtoutf8" => {
                let mut units = self.read_aechar_units(a0)?;
                if (a1 as i32) >= 0 {
                    units.truncate(a1 as usize);
                }
                let fits = self.cpu.read_reg(Reg::R3) as usize;
                let mut bytes = Vec::with_capacity(fits);
                for c in String::from_utf16_lossy(&units).chars() {
                    let mut buffer = [0u8; 4];
                    let utf8 = c.encode_utf8(&mut buffer).as_bytes();
                    if bytes.len() + utf8.len() > fits {
                        break;
                    }
                    bytes.extend_from_slice(utf8);
                }
                if bytes.len() < fits {
                    bytes.push(0);
                }
                if a2 != 0 && !bytes.is_empty() {
                    self.cpu.write_mem(a2, &bytes)?;
                }
                TRUE
            }

            // --- Byte strings -----------------------------------------------------
            "strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // void qsort(void *base, size_t nmemb, size_t size, int (*compar)(const void *,
            //            const void *))
            //
            // The comparison is a **game** function, so sorting means re-entering the guest for
            // each pair. It is the same detour the pending callbacks take, and at the same point:
            // the API call has finished and the guest has not resumed yet.
            "qsort" => {
                let compare = self.cpu.read_reg(Reg::R3);
                self.qsort(a0, a1, a2, compare)?;
                SUCCESS
            }
            // `uint32 strtoul(const char *nptr, char **endptr, int base)`
            "strtoul" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_unsigned(&text, a2);
                if a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                value
            }
            "strnicmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let right = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "stristr" => {
                let hay = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let needle = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `char *memstr(const char *cpHaystack, const char *cpszNeedle, size_t nLen)` — the
            // haystack has a fixed size; the needle is still NUL-terminated.
            "memstr" => {
                let hay = self.read_bytes(a0, a2)?;
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `boolean strbegins(const char *cpszPrefix, const char *psz)` and its pair `strends`:
            // the piece being looked for comes **first**.
            "strbegins" | "strends" | "aee_stribegins" => {
                let part = self.cpu.read_cbytes(a0, MAX_STRING);
                let whole = self.cpu.read_cbytes(a1, MAX_STRING);
                let matched = match name {
                    "strbegins" => whole.starts_with(&part),
                    "strends" => whole.ends_with(&part),
                    _ => whole
                        .to_ascii_lowercase()
                        .starts_with(&part.to_ascii_lowercase()),
                };
                u32::from(matched)
            }
            // `char *strchrend(const char *pszSrc, char c)` — like `strchr`, but when it does not
            // find it returns the terminator instead of zero.
            "strchrend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let at = text.iter().position(|&b| b == needle).unwrap_or(text.len());
                a0 + at as u32
            }
            // `char *strchrsend(const char *pszSrc, const char *pszChars)` — the first byte
            // that appears in the set, or the terminator.
            "strchrsend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let set = self.cpu.read_cbytes(a1, MAX_STRING);
                let at = text
                    .iter()
                    .position(|b| set.contains(b))
                    .unwrap_or(text.len());
                a0 + at as u32
            }
            // BREW's `mem*` family works on a fixed-size block: `memrchr` finds
            // the last occurrence, `memchrend`/`memrchrbegin` return the block's bounds
            // when they do not find it.
            "memrchr" | "memchrend" | "memrchrbegin" => {
                let block = self.read_bytes(a0, a2)?;
                let needle = a1 as u8;
                let at = match name {
                    "memchrend" => block
                        .iter()
                        .position(|&b| b == needle)
                        .unwrap_or(block.len()),
                    "memrchrbegin" => block.iter().rposition(|&b| b == needle).unwrap_or_default(),
                    _ => match block.iter().rposition(|&b| b == needle) {
                        Some(i) => i,
                        None => return Ok(Some(0)),
                    },
                };
                a0 + at as u32
            }
            "strlower" | "strupper" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                if name == "strlower" {
                    bytes.make_ascii_lowercase();
                } else {
                    bytes.make_ascii_uppercase();
                }
                if !bytes.is_empty() {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            // `size_t strlcpy/strlcat(char *dst, const char *src, size_t nSize)`, BSD style.
            "strlcpy" | "strlcat" => {
                let source = self.cpu.read_cstring(a1, MAX_STRING);
                let existing = if name == "strlcat" {
                    self.cpu.read_cbytes(a0, MAX_STRING).len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_cstring_limited(a0 + existing as u32, &source, room)?;
                (existing + source.len()) as u32
            }
            // boolean MAKEPATH(const char *cpszDir, const char *cpszFile, char *pszOut,
            //                  int *pnOutLen)
            //
            // Joins directory and file with a slash. With a null `pszOut` it only reports the size,
            // terminator included — and that is how the Z-Wheel uses it, in two passes: measure,
            // allocate, build. It was missing, and confirming on "Jogar" stopped here, on the path
            // that opens the game list.
            "aee_makepath" => {
                let out_size = self.cpu.read_reg(Reg::R3);
                let joined_path = join_path(
                    &self.cpu.read_cstring(a0, MAX_STRING),
                    &self.cpu.read_cstring(a1, MAX_STRING),
                );
                let needed = joined_path.len() as u32 + 1;
                let fits = match (a2, out_size) {
                    (0, _) => true,
                    (_, 0) => {
                        self.write_cstring_limited(a2, &joined_path, needed as usize)?;
                        true
                    }
                    (_, location) => {
                        let spacing = self.cpu.read_u32(location)?;
                        if spacing >= needed {
                            self.write_cstring_limited(a2, &joined_path, needed as usize)?;
                        }
                        spacing >= needed
                    }
                };
                if out_size != 0 {
                    self.cpu.write_u32(out_size, needed)?;
                }
                u32::from(fits)
            }
            "OEMStrLen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "OEMStrSize" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32 + 1,
            "swapl" => a0.swap_bytes(),
            "swaps" => (a0 as u16).swap_bytes() as u32,

            // --- Memory, version and debugging ------------------------------------------
            "sysfree" => {
                self.free(a0);
                SUCCESS
            }
            "err_strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr == 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_mem(ptr, &bytes)?;
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `int err_realloc(uint32 uSize, void **pp)` — the `realloc` that returns an error
            // code and only swaps the pointer if it succeeds.
            "err_realloc" => {
                let current = self.cpu.read_u32(a1)?;
                let ptr = self.realloc(current, a0)?;
                if ptr == 0 && a0 != 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `uint32 GetAEEVersion(byte *pszFormatted, int nSize, uint16 wFlags)`: the high byte
            // of the high word is the major version, and so on — hence `4.0.2.0`.
            "GetAEEVersion" => {
                if a0 != 0 {
                    if a2 & GAV_LATIN1 != 0 {
                        self.write_cstring_limited(a0, AEE_VERSION_TEXT, a1 as usize)?;
                    } else {
                        self.write_aechar(a0, AEE_VERSION_TEXT, a1 as usize / 2)?;
                    }
                }
                AEE_VERSION
            }
            // `uint32 GetFSFree(uint32 *pdwTotal)` — we have no file system quota, and
            // answering a large number is more faithful than answering zero.
            "GetFSFree" => {
                if a0 != 0 {
                    self.cpu.write_u32(a0, FS_TOTAL)?;
                }
                FS_TOTAL
            }
            "aee_GetUTCSeconds" => self.elapsed_ms() / 1000,
            // `int32 aee_LocalTimeOffset(boolean *pbDaylightSavings)` — the emulator runs in UTC.
            "aee_LocalTimeOffset" => {
                if a0 != 0 {
                    self.cpu.write_mem(a0, &[0])?;
                }
                0
            }
            // BREW's debugging hooks: they exist for the platform's log, and the
            // correct behavior without it is to do nothing.
            "dumpheap" | "dbgevent" => SUCCESS,
            "dbgheapmark" => a0,
            // `int lockmem/unlockmem(void **ppHandle)` — the device's BREW may move blocks
            // and therefore locks them; our heap is fixed, so locking always succeeds.
            "lockmem" | "unlockmem" => TRUE,
            // `char *aee_basename(const char *cpszPath)`
            "aee_basename" => {
                let path = self.cpu.read_cbytes(a0, MAX_STRING);
                let at = path
                    .iter()
                    .rposition(|&b| b == b'/' || b == b'\\')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                a0 + at as u32
            }
            "atoi" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                c_atoi(&text) as u32
            }
            // The stdlib's floating-point family. In AAPCS a `double` takes a pair of
            // registers with the low word first, so `v1` comes in `r0:r1`, `v2` in
            // `r2:r3` and the rest goes to the stack.
            "f_op" | "f_cmp" => {
                let (v1, v2) = (
                    fmath::from_words(a0, a1),
                    fmath::from_words(a2, self.cpu.read_reg(Reg::R3)),
                );
                let kind = self.cpu.read_u32(self.cpu.read_reg(Reg::Sp))?;
                if name == "f_cmp" {
                    let Some(answer) = fmath::cmp(v1, v2, kind) else {
                        return Ok(None);
                    };
                    u32::from(answer)
                } else {
                    let Some(value) = fmath::op(v1, v2, kind) else {
                        return Ok(None);
                    };
                    self.return_double(value)
                }
            }
            "f_calc" => {
                let Some(value) = fmath::calc(fmath::from_words(a0, a1), a2) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_get" => {
                let Some(value) = fmath::get(a0) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_assignint" => self.return_double(a0 as i32 as f64),
            "f_assignstr" | "strtod" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_leading_double(&text);
                // `strtod` also returns, through the `char **`, where it stopped reading.
                if name == "strtod" && a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                self.return_double(value)
            }
            // `f_toint` rounds toward zero, like the C cast; `trunc`/`utrunc` are the same
            // computation with the result's sign changing.
            "f_toint" | "trunc" => fmath::from_words(a0, a1).trunc() as i32 as u32,
            "utrunc" => fmath::from_words(a0, a1).trunc() as u32,
            // int vsnprintf(char *dst, int nSize, const char *fmt, va_list args) and
            // int vsprintf(char *dst, const char *fmt, va_list args).
            //
            // In AAPCS `va_list` is a pointer to the argument area, so it is enough to read
            // words in sequence starting from it.
            "vsnprintf" | "vsprintf" => {
                let (fmt_addr, args_addr, limit) = if name == "vsnprintf" {
                    (a2, self.cpu.read_reg(Reg::R3), a1 as usize)
                } else {
                    (a1, a2, usize::MAX)
                };
                let fmt = self.cpu.read_cstring(fmt_addr, MAX_STRING);
                // `AEEOldVaList` is `int **` on ARM (`inc/AEEOldVaList.h`): what arrives is the
                // address of the `va_list` variable, not the argument area. Without that
                // indirection the first `%d` prints the stack pointer itself — that is what
                // turned `inva1_shotgun` into `inva537919380_shotgun` in Quake.
                let mut source = GuestArgs {
                    words: Vec::new(),
                    index: 0,
                    stack: self.cpu.read_u32(args_addr)?,
                    cpu: &self.cpu,
                };
                let text = cformat::format(&fmt, &mut source);
                self.write_cstring_limited(a0, &text, limit)?;
                latin1_len(&text)
            }
            // int snprintf(char *dst, int nSize, const char *fmt, ...)
            //
            // **BREW's counts the terminator**, unlike C99: it returns the bytes written to the
            // buffer including the zero, and with a null `dst` the size the result would need, also
            // including the zero. Need For Speed builds the answer of the career screens with `p +=
            // snprintf(p, n, "%s=", name) - 1` and each item with `"%d%c"` and the separator
            // `0x7f`. Returning C's length, each array lost the `=` and the separators:
            // `aCarNames=A\x7fB` arrived as `aCarNamesAB`, the car selection screen opened with the
            // name in garbage and the attributes empty, and the career did not progress (issue
            // #35).
            "snprintf" => {
                let fmt = self.cpu.read_cstring(a2, MAX_STRING);
                let text = self.format_from(3, &fmt);
                self.write_cstring_limited(a0, &text, a1 as usize)?;
                let with_nul = latin1_len(&text) + 1;
                match a0 {
                    0 => with_nul,
                    _ => with_nul.min(a1),
                }
            }
            // `sprintf(char *dst, const char *fmt, ...)`: the variadic arguments start at `r2`.
            "sprintf" => {
                let fmt = self.cpu.read_cstring(a1, MAX_STRING);
                let text = self.format_from(2, &fmt);
                self.write_cstring(a0, &text)?;
                latin1_len(&text)
            }
            "dbgprintf" => {
                let fmt = self.cpu.read_cstring(a0, MAX_STRING);
                let text = self.format_from(1, &fmt);
                self.record_debug(text);
                SUCCESS
            }
            // `IApplet *GetAppInstance(void)` — no arguments, which explains why the call site
            // could use `r0` as scratch for the function pointer.
            //
            // The game calls this *during* its own creation, after `AEEApplet_New` has already
            // written the pointer to the `void **ppObj` we passed. So, until we finish
            // creating the applet, the right answer is precisely the contents of that pointer.
            "GetAppInstance" => match self.current_applet {
                0 => self.cpu.read_u32(self.module.out_module + 4)?,
                applet => applet,
            },
            // `GETUPTIMEMS` is since the device was turned on; `GETTIMESECONDS` is the
            // **calendar**, seconds since January 6, 1980. Answering uptime for both was saying
            // that today is the console's launch day: the Z-Wheel computes the "next day" alarm
            // from there and kept spinning — fifteen million iterations in two methods.
            "aee_GetTimeMS" | "aee_GetUpTimeMS" => self.elapsed_ms(),
            "aee_GetSeconds" => self.brew_seconds(),
            // void GETJULIANDATE(uint32 dwSecs, JulianType *pDate)
            //
            // `dwSecs` is BREW's clock: seconds since January 6, 1980, GMT. Zero means
            // "now", and our now is the virtual clock — the same one that answers
            // `GetSeconds`, so that the two computations never contradict each other.
            "aee_GetJulianDate" => {
                // A helper has no `this`: the first argument is `r0`.
                let seconds = match a0 {
                    0 => self.brew_seconds(),
                    data_byte => data_byte,
                };
                if a1 != 0 {
                    let data = julian_date(seconds);
                    let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
                    self.cpu.write_mem(a1, &bytes)?;
                }
                SUCCESS
            }
            // A simple, deterministic generator: repeating the same session must give the same
            // result, or else debugging a game with randomness becomes a lottery.
            "aee_GetRand" => {
                let count = a1;
                for i in 0..count {
                    self.random_state = self
                        .random_state
                        .wrapping_mul(1_103_515_245)
                        .wrapping_add(12_345);
                    self.cpu
                        .write_mem(a0 + i, &[(self.random_state >> 16) as u8])?;
                }
                SUCCESS
            }
            "GetRAMFree" => (loader::HEAP_SIZE as u32).saturating_sub(self.heap.used()),
            // Really sleeping would only delay the emulator: guest time moves with the host's
            // clock anyway.
            // void sleep(uint32 msecs) — the SDK's `MSLEEP`.
            //
            // On the console the ARM really stops and time passes on its own. Here the clock moves
            // with the instructions, so whoever sleeps without the clock advancing sleeps forever:
            // Magical Drop 3 called this a hundred million times in a loop that never expired.
            // Advancing the clock is what the device does.
            "sleep" => {
                self.clock_us += u64::from(a0.min(MAX_SLEEP_MS)) * 1000;
                SUCCESS
            }
            "getlasterror" => self.file_error,
            "aee_IsBadPtr" => u32::from(self.cpu.read_u32(a0).is_err()),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Prepares the return of a `double`: the high word goes to `r1`, and the low one is the value
    /// the dispatch writes to `r0`.
    pub(super) fn return_double(&mut self, value: f64) -> u32 {
        let (low, high) = fmath::to_words(value);
        self.cpu.write_reg(Reg::R1, high);
        low
    }

    /// Like [`Machine::format_from`], with `%s` reading `AECHAR`: the one for `WSPRINTF`.
    pub(super) fn format_wide_from(&self, first: usize, fmt: &str) -> String {
        let words = [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ];
        let mut source = GuestArgs {
            words: words[first..].to_vec(),
            index: 0,
            stack: self.cpu.read_reg(Reg::Sp),
            cpu: &self.cpu,
        };
        cformat::format_wide(fmt, &mut source)
    }

    /// Formats a guest string taking the variadic arguments starting at register `first`.
    pub(super) fn format_from(&self, first: usize, fmt: &str) -> String {
        let words = [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ];
        let mut source = GuestArgs {
            words: words[first..].to_vec(),
            index: 0,
            stack: self.cpu.read_reg(Reg::Sp),
            cpu: &self.cpu,
        };
        cformat::format(fmt, &mut source)
    }

    pub(super) fn read_bytes(&self, addr: u32, len: u32) -> Result<Vec<u8>, CpuError> {
        let mut buf = vec![0u8; guest_size(len as usize)?];
        if len > 0 {
            self.cpu.read_mem(addr, &mut buf)?;
        }
        Ok(buf)
    }

    /// Copies within guest memory. It goes through the host, so overlapping regions come out
    /// right — which is exactly what `memmove` promises.
    pub(super) fn copy_guest(&mut self, dst: u32, src: u32, len: u32) -> Result<(), CpuError> {
        let bytes = self.read_bytes(src, len)?;
        if len > 0 {
            self.cpu.write_mem(dst, &bytes)?;
            self.cpu.mark_dirty(dst, len);
        }
        Ok(())
    }

    /// Writes an `AECHAR` string (little-endian UTF-16, zero-terminated).
    ///
    /// `max_units` limits how many `AECHAR`s fit in the destination, terminator included.
    pub(super) fn write_aechar(
        &mut self,
        addr: u32,
        text: &str,
        max_units: usize,
    ) -> Result<(), CpuError> {
        if addr == 0 || max_units == 0 {
            return Ok(());
        }
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.truncate(max_units.saturating_sub(1));
        units.push(0);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Writes a C string honoring the destination's size.
    pub(super) fn write_cstring_limited(
        &mut self,
        addr: u32,
        text: &str,
        max_bytes: usize,
    ) -> Result<(), CpuError> {
        if addr == 0 || max_bytes == 0 {
            return Ok(());
        }
        // ISO-8859-1, as when reading: one character, one byte. Besides keeping the accent, it is
        // what makes the size cut always land on a character boundary.
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.truncate(max_bytes.saturating_sub(1));
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes)
    }

    pub(super) fn write_cstring(&mut self, addr: u32, text: &str) -> Result<(), CpuError> {
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} while writing {} bytes at {addr:#010x}",
                bytes.len()
            ))
        })
    }
}

/// How many bytes the text takes in the game's memory, which is where `printf` counts.
///
/// The formatter's `String` keeps each character above `0x7f` in two UTF-8 bytes, and in
/// memory it goes back to being a single byte (see [`crate::cpu::latin1_encode`]). `len()` counted
/// the UTF-8 ones: Need For Speed's texts are UTF-8 inside Latin-1, and "ARMAZÉM" came back two
/// bytes longer than what had been written.
fn latin1_len(text: &str) -> u32 {
    text.chars().count() as u32
}

/// `MAKEPATH`: the directory, a single slash, and the file.
fn join_path(dir: &str, file: &str) -> String {
    match (dir.is_empty(), dir.ends_with('/')) {
        (true, _) => file.to_string(),
        (false, true) => format!("{dir}{file}"),
        (false, false) => format!("{dir}/{file}"),
    }
}

/// C's `atoi`: leading whitespace, an optional sign and whatever digits follow, stopping at the
/// first one that is not a digit. With no digit at all, zero.
///
/// **What comes after the number does not invalidate the number.** Need for Speed looks up the
/// parts of a mesh by name, with `atoi` on a four-byte field without a terminator (`"1   "`
/// followed by the rest of the record). Requiring the whole text to be numeric gave zero for all
/// of them, part `1` was never found, and the game read a null pointer at `0x20738`.
fn c_atoi(text: &str) -> i32 {
    let rest = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let value = rest
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i32, |acc, d| acc.wrapping_mul(10).wrapping_add(i32::from(d - b'0')));
    match negative {
        true => value.wrapping_neg(),
        false => value,
    }
}

#[cfg(test)]
mod atoi_tests {
    use super::c_atoi;

    #[test]
    fn stops_at_the_first_non_digit() {
        assert_eq!(c_atoi("1   \u{10}xyz"), 1);
        assert_eq!(c_atoi("  -42abc"), -42);
        assert_eq!(c_atoi("+7"), 7);
        assert_eq!(c_atoi("abc"), 0);
        assert_eq!(c_atoi(""), 0);
    }
}

#[cfg(test)]
mod makepath_tests {
    use super::join_path;

    #[test]
    fn joins_with_a_single_slash() {
        assert_eq!(join_path("fs:/mod/274755", "tectoy.cfg"), "fs:/mod/274755/tectoy.cfg");
        assert_eq!(join_path("fs:/mod/", "a.db"), "fs:/mod/a.db");
        assert_eq!(join_path("", "a.db"), "a.db");
    }
}
