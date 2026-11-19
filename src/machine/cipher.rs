//! Cryptography: BREW's ICipher and IHash.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `ILicense` (`AEECLSID_LICENSE` = `0x0100100f`), from `QINTERFACE(ILicense)` in
    /// `sdk/inc/AEELicense.h`.
    ///
    /// Answers what holds for a ROM legitimately bought and installed on the console: a
    /// license with no expiration (`LT_NONE`) and a permanent purchase (`PT_PURCHASE`). It is
    /// not bypassing any check — it is the answer the device itself would give for a game
    /// its owner bought.
    /// Network and cryptography: `IWeb`, `IHash` (`AEECLSID_MD5`), `ICipherFactory` and `ICipher1`.
    ///
    /// Boomerang Sports Dodgeball creates the three in sequence and **does not check the return
    /// value**: if the first creation fails, it skips the other two and uses the pointer that was
    /// never written. That is why the objects must exist even with the console offline — refusing
    /// them brought the game down before the first screen.
    pub(super) fn crypto_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                // The address goes back to being another object's: a new hash there would continue
                // this one's MD5.
                if remaining == 0 {
                    self.hashes.remove(&this);
                    self.ciphers.remove(&this);
                }
                remaining
            }
            "QueryInterface" => {
                let out = self.cpu.read_reg(Reg::R2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateCipher(ICipherFactory *, AEECLSID cipher, int direction,
            //                  AEECLSID mode, int padding, ICipher1 **ppCipher)
            "CreateCipher" => {
                let out = self.stack_arg(1)?;
                let cipher = self.new_object(Interface::Cipher)?;
                if cipher == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let padding = self.stack_arg(0)?;
                self.ciphers.insert(
                    cipher,
                    CipherState {
                        padding,
                        ..Default::default()
                    },
                );
                if out != 0 {
                    self.cpu.write_u32(out, cipher)?;
                }
                SUCCESS
            }
            // int Init(IHashCTX *, void *pCtx, int nCtxSize)
            "Init" if iface == Interface::HashCtx => {
                let (ctx, size) = (self.arg(1), self.arg(2));
                if ctx == 0 || size < MD5_CTX_SIZE {
                    return Ok(Some(EBADPARM));
                }
                self.write_md5_ctx(ctx, &crate::brew::crypto::Md5::new())?;
                SUCCESS
            }
            // int Update(IHashCTX *, void *pCtx, int nCtxSize, const byte *pData, int nLen)
            "Update" if iface == Interface::HashCtx => {
                let (ctx, size, data, count) =
                    (self.arg(1), self.arg(2), self.arg(3), self.arg(4));
                if ctx == 0 || size < MD5_CTX_SIZE {
                    return Ok(Some(EBADPARM));
                }
                let mut md5 = self.read_md5_ctx(ctx)?;
                if count != 0 {
                    md5.update(&self.read_bytes(data, count)?);
                }
                self.write_md5_ctx(ctx, &md5)?;
                SUCCESS
            }
            // int GetResult(IHashCTX *, void *pCtx, int nCtxSize, byte *pResult, int *pnLen)
            //
            // Writes up to `*pnLen` bytes of the digest and returns sixteen in the size, like
            // `IHash`'s `GetDigest`.
            "GetResult" if iface == Interface::HashCtx => {
                let (ctx, size, dest, pn) =
                    (self.arg(1), self.arg(2), self.arg(3), self.arg(4));
                if ctx == 0 || size < MD5_CTX_SIZE {
                    return Ok(Some(EBADPARM));
                }
                let summary = self.read_md5_ctx(ctx)?.finish();
                let fits = match pn {
                    0 => summary.len(),
                    p => (self.cpu.read_u32(p).unwrap_or(0) as usize).min(summary.len()),
                };
                if dest != 0 {
                    self.cpu.write_mem(dest, &summary[..fits])?;
                }
                if pn != 0 {
                    self.cpu.write_u32(pn, summary.len() as u32)?;
                }
                SUCCESS
            }
            // void IHASH_Reset(IHash *) — restarts the digest from scratch.
            "Reset" => {
                self.hashes.insert(this, HashState::default());
                SUCCESS
            }
            // void IHASH_Update(IHash *, const byte *pData, int nLen)
            "Update" => {
                let (src, len) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let bytes = match len {
                    0 => Vec::new(),
                    _ => self.read_bytes(src, len)?,
                };
                self.hashes.entry(this).or_default().md5.update(&bytes);
                SUCCESS
            }
            // void GetDigest(IHash *, byte *pBuf, int *pnLen)
            //
            // The caller zeroes a 33-byte buffer and puts 17 in the size — sixteen bytes of digest
            // and the terminator. We write the sixteen and return sixteen in the size, honoring the
            // cap it asked for.
            "GetDigest" => {
                let (dest, size) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let summary = self.hashes.entry(this).or_default().md5.clone().finish();
                let fits = match size {
                    0 => summary.len() as u32,
                    p => self
                        .cpu
                        .read_u32(p)
                        .unwrap_or(0)
                        .min(summary.len() as u32 + 1),
                };
                let to_write = fits.min(summary.len() as u32) as usize;
                if dest != 0 {
                    self.cpu.write_mem(dest, &summary[..to_write])?;
                }
                if size != 0 {
                    self.cpu.write_u32(size, to_write as u32)?;
                }
                SUCCESS
            }
            // int QueryCipher(ICipherFactory *, AEECLSID cipher, AEECLSID mode, int padding,
            //                 unsigned keysize)
            // void IWEB_GetResponse(IWeb *po, IWeb *po, IWebResp **ppResp, AEECallback *pcb,
            //                       const char *pszUrl, ...)
            //
            // The SDK macro repeats `po` as the first vararg, so `r1` is the object itself, `r2` is
            // the response's output pointer, `r3` is the callback and the URL comes from the stack.
            // The options follow after that, terminated by `WEBOPT_END`.
            //
            // For now this **observes and refuses**. Observing first is deliberate: the varargs
            // format is not in any header we have, and implementing HTTP against a guessed layout
            // would give a client that gets the address wrong without saying so. Once what the game
            // asks for is recorded, the next step is to serve it for real — and then giving network
            // access to a binary of external origin is a project decision, with explicit
            // authorization.
            "GetResponse" => {
                let (r1, output) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let url_ptr = self.stack_arg(0)?;
                let url = match url_ptr {
                    0 => String::new(),
                    p => self.cpu.read_cstring(p, MAX_STRING),
                };
                self.web_requests.insert(match url.is_empty() {
                    true => format!("(no URL) r1={r1:#x} r2={output:#x}"),
                    false => url,
                });
                // Zeroed output pointer: the game needs to see that there is no response, instead
                // of carrying on with garbage.
                if output != 0 {
                    self.cpu.write_u32(output, 0)?;
                }
                SUCCESS
            }
            "QueryCipher" => SUCCESS,
            // int AddOpt(IWeb *, WebOpt *apWebOpt) — headers, timeout and the like. Accepting costs
            // nothing: what decides the request's destination is `GetResponse`.
            //
            // `AddOptBuffer` is slot 6, which in the firmware builds a descriptor on the stack and
            // calls this same function. Accepting both is the same decision.
            "AddOpt" | "AddOptBuffer" => SUCCESS,
            // The `IWeb` slots that the firmware's vtable shows to exist and that have not shown up
            // in use yet. Answering success lets them show up in the call report instead of
            // bringing the game down — which is how slot 6 was found.
            "slot4" | "slot5" | "slot7" | "slot8" | "slot9" | "slot10" | "slot12"
                if iface == Interface::Web =>
            {
                SUCCESS
            }
            // int SetParam(ICipher1 *, int nId, const void *pParam, unsigned uParamLen)
            "SetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let Some(state) = self.ciphers.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                match id {
                    CIPHER_PARAM_KEY | CIPHER_PARAM_IV => {
                        if len as usize != AES_BLOCK {
                            return Ok(Some(EBADPARM));
                        }
                        let mut bytes = [0u8; AES_BLOCK];
                        self.cpu.read_mem(param, &mut bytes)?;
                        let state = self.ciphers.get_mut(&this).expect("was just found");
                        if id == CIPHER_PARAM_KEY {
                            state.key = Some(bytes);
                        } else {
                            state.iv = bytes;
                        }
                        SUCCESS
                    }
                    CIPHER_PARAM_PADDING => {
                        state.padding = self.cpu.read_u32(param)?;
                        SUCCESS
                    }
                    // The direction and the mode already came in `CreateCipher`; repeating them
                    // changes nothing, and refusing would make the game give up.
                    CIPHER_PARAM_DIRECTION | CIPHER_PARAM_MODE => SUCCESS,
                    _ => EUNSUPPORTED,
                }
            }
            // int GetParam(ICipher1 *, int nId, void *pParam, unsigned *puParamLen)
            "GetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let value = match id {
                    CIPHER_PARAM_KEY_SIZE | CIPHER_PARAM_IV_SIZE | CIPHER_PARAM_BLOCKSIZE => {
                        AES_BLOCK as u32
                    }
                    _ => return Ok(Some(EUNSUPPORTED)),
                };
                if param != 0 {
                    self.cpu.write_u32(param, value)?;
                }
                if len != 0 {
                    self.cpu.write_u32(len, 4)?;
                }
                SUCCESS
            }
            // int Process(ICipher1 *, const byte *pbIn, unsigned cbIn, byte *pbOut,
            //             unsigned *pcbOut)
            "Process" | "ProcessLast" => self.cipher_process(this, name == "ProcessLast")?,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ICipher1_Process` and `ICipher1_ProcessLast`, which differ only in how they finish.
    ///
    /// `ICipher1` is a stream: the game hands over as many bytes as it wants, and what does not
    /// complete a block is kept for the next call. Only `ProcessLast` fills the missing block, the
    /// way `padding` asks.
    pub(super) fn cipher_process(&mut self, this: u32, last: bool) -> Result<u32, CpuError> {
        // `Process` has the five arguments; `ProcessLast` only has the output and its size.
        let (input, count, out, out_len) = match last {
            false => (
                self.cpu.read_reg(Reg::R1),
                self.cpu.read_reg(Reg::R2) as usize,
                self.cpu.read_reg(Reg::R3),
                self.stack_arg(0)?,
            ),
            true => (0, 0, self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2)),
        };
        let Some(state) = self.ciphers.get(&this) else {
            return Ok(EBADPARM);
        };
        let Some(key) = state.key else {
            return Ok(EBADPARM);
        };

        let mut data = state.pending.clone();
        if count > 0 {
            let mut bytes = vec![0u8; guest_size(count)?];
            self.cpu.read_mem(input, &mut bytes)?;
            data.extend_from_slice(&bytes);
        }
        if last {
            match state.padding {
                CIPHER_PADDING_NONE if data.len() % AES_BLOCK != 0 => return Ok(EBADPARM),
                CIPHER_PADDING_NONE => {}
                // `CIPHER_PADDING_ZERO` and whatever we do not know fill with zeros: it is the
                // padding BREW uses by default in block ciphers.
                _ => data.resize(data.len().div_ceil(AES_BLOCK) * AES_BLOCK, 0),
            }
        }
        // What the game encrypts is recorded in the clear, and it is the most useful thing this
        // method does for anyone studying the protocol: the body that goes out over the network is
        // encrypted, and decrypting it on the other side requires having the key and getting the
        // mode right. Here it goes through us before that.
        if !data.is_empty() {
            if self.plaintexts.len() == PLAINTEXT_MAX {
                self.plaintexts.pop_front();
            }
            self.plaintexts
                .push_back(data[..data.len().min(PLAINTEXT_BYTES)].to_vec());
        }
        // What does not close a block waits for the next call.
        let whole = data.len() / AES_BLOCK * AES_BLOCK;
        let leftover = data.split_off(whole);

        let mut iv = state.iv;
        crypto::cbc_encrypt(&crypto::Aes128::new(&key), &mut iv, &mut data);
        if let Some(state) = self.ciphers.get_mut(&this) {
            state.iv = iv;
            state.pending = leftover;
        }

        // The offered capacity arrives in the same pointer that returns how much was written.
        let capacity = match out_len {
            0 => data.len(),
            addr => self.cpu.read_u32(addr)? as usize,
        };
        if capacity < data.len() {
            if out_len != 0 {
                self.cpu.write_u32(out_len, data.len() as u32)?;
            }
            return Ok(EBUFFERTOOSMALL);
        }
        if out != 0 && !data.is_empty() {
            self.cpu.write_mem(out, &data)?;
        }
        if out_len != 0 {
            self.cpu.write_u32(out_len, data.len() as u32)?;
        }
        Ok(SUCCESS)
    }

    /// What the game encrypted, in the clear, in the order it handed it over.
    pub fn plaintexts(&self) -> Vec<&[u8]> {
        self.plaintexts.iter().map(Vec::as_slice).collect()
    }

    /// The cipher keys the games configured, in hexadecimal.
    ///
    /// It exists for a practical reason: Zeeboids **encrypts the body before sending**, so the
    /// server receives noise. The key is the game's own and goes through us in
    /// `ICipher1::SetParam`; without it, the server's log says nothing about the protocol.
    pub fn cipher_keys(&self) -> Vec<String> {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        self.ciphers
            .values()
            .filter_map(|c| {
                c.key
                    .map(|k| format!("key {} iv {}", hex(&k), hex(&c.iv)))
            })
            .collect()
    }
}

/// The size of `MD5_CTX`: state (16), byte count (8) and the pending block (64).
const MD5_CTX_SIZE: u32 = 88;

impl<C: CpuBackend> Machine<C> {
    /// The MD5 kept in the game's context, in the `MD5_CTX` layout.
    ///
    /// **The state lives in guest memory, not here**: `IHashCTX` exists precisely so the
    /// caller holds the context, and that is how it survives a save state without hidden state.
    pub(super) fn read_md5_ctx(&self, ctx: u32) -> Result<crate::brew::crypto::Md5, CpuError> {
        let bytes = self.read_bytes(ctx, MD5_CTX_SIZE)?;
        let word = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        let state = [word(0), word(4), word(8), word(12)];
        let total = u64::from_le_bytes(bytes[16..24].try_into().expect("8 bytes"));
        let pending = (total % 64) as usize;
        let mut md5 = crate::brew::crypto::Md5::new();
        // A context the game never initialized cannot produce a format error: the rest of the block
        // is always smaller than 64 by the arithmetic above.
        let _ = md5.restore_state(state, &bytes[24..24 + pending], total);
        Ok(md5)
    }

    pub(super) fn write_md5_ctx(&mut self, ctx: u32, md5: &crate::brew::crypto::Md5) -> Result<(), CpuError> {
        let (state, pending, total) = md5.state();
        let mut bytes = Vec::with_capacity(MD5_CTX_SIZE as usize);
        for word in state {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&total.to_le_bytes());
        bytes.extend_from_slice(pending);
        bytes.resize(MD5_CTX_SIZE as usize, 0);
        self.cpu.write_mem(ctx, &bytes)
    }
}
