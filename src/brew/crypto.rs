//! AES-128 in CBC, which is what BREW provides through `AEECLSID_BlockAES128`.
//!
//! Written here instead of coming from a dependency because it is small, self-contained and
//! verifiable: the tests use the vectors from the specifications themselves — FIPS-197 for the
//! block and NIST SP 800-38A for the chaining.

/// The AES substitution box, from table 4 of FIPS-197.
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

/// Multiplication in the AES field, `GF(2^8)` with the polynomial `x^8 + x^4 + x^3 + x + 1`.
fn xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0 }
}

fn multiply(a: u8, b: u8) -> u8 {
    let (mut a, mut b, mut out) = (a, b, 0u8);
    while b != 0 {
        if b & 1 != 0 {
            out ^= a;
        }
        a = xtime(a);
        b >>= 1;
    }
    out
}

/// AES-128 block cipher, with the round keys already expanded.
#[derive(Debug, Clone)]
pub struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut words = [[0u8; 4]; 44];
        for (i, word) in words.iter_mut().take(4).enumerate() {
            word.copy_from_slice(&key[i * 4..i * 4 + 4]);
        }
        let mut rcon = 1u8;
        for i in 4..44 {
            let mut temp = words[i - 1];
            if i % 4 == 0 {
                temp.rotate_left(1);
                for byte in temp.iter_mut() {
                    *byte = SBOX[*byte as usize];
                }
                temp[0] ^= rcon;
                rcon = xtime(rcon);
            }
            words[i] = std::array::from_fn(|j| words[i - 4][j] ^ temp[j]);
        }
        let round_keys =
            std::array::from_fn(|r| std::array::from_fn(|i| words[r * 4 + i / 4][i % 4]));
        Self { round_keys }
    }

    /// Encrypts a 16-byte block in place.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for round in 1..11 {
            for byte in block.iter_mut() {
                *byte = SBOX[*byte as usize];
            }
            shift_rows(block);
            if round != 10 {
                mix_columns(block);
            }
            add_round_key(block, &self.round_keys[round]);
        }
    }
}

fn add_round_key(block: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, k) in block.iter_mut().zip(key) {
        *byte ^= k;
    }
}

/// The AES state is a matrix filled by columns, so the row `r` of byte `i` is `i % 4` — and
/// shifting row `r` by `r` positions touches bytes scattered across the block.
fn shift_rows(block: &mut [u8; 16]) {
    let original = *block;
    for column in 0..4 {
        for row in 0..4 {
            block[column * 4 + row] = original[((column + row) % 4) * 4 + row];
        }
    }
}

fn mix_columns(block: &mut [u8; 16]) {
    for column in block.chunks_exact_mut(4) {
        let c: [u8; 4] = column.try_into().expect("chunk of four");
        column[0] = multiply(c[0], 2) ^ multiply(c[1], 3) ^ c[2] ^ c[3];
        column[1] = c[0] ^ multiply(c[1], 2) ^ multiply(c[2], 3) ^ c[3];
        column[2] = c[0] ^ c[1] ^ multiply(c[2], 2) ^ multiply(c[3], 3);
        column[3] = multiply(c[0], 3) ^ c[1] ^ c[2] ^ multiply(c[3], 2);
    }
}

/// Encrypts `data` in CBC, in place, advancing `iv`. The length must be a multiple of 16 — the
/// padding is decided by `ICipher1`, not here.
pub fn cbc_encrypt(cipher: &Aes128, iv: &mut [u8; 16], data: &mut [u8]) {
    for block in data.chunks_exact_mut(16) {
        for (byte, previous) in block.iter_mut().zip(iv.iter()) {
            *byte ^= previous;
        }
        let mut current: [u8; 16] = block.try_into().expect("chunk of sixteen");
        cipher.encrypt_block(&mut current);
        block.copy_from_slice(&current);
        *iv = current;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn aes128_matches_the_fips_197_vector() {
        // Appendix B of FIPS-197.
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let mut block: [u8; 16] = [
            0x32, 0x43, 0xf6, 0xa8, 0x88, 0x5a, 0x30, 0x8d, 0x31, 0x31, 0x98, 0xa2, 0xe0, 0x37,
            0x07, 0x34,
        ];
        Aes128::new(&key).encrypt_block(&mut block);
        assert_eq!(hex(&block), "3925841d02dc09fbdc118597196a0b32");
    }

    #[test]
    fn aes128_matches_fips_197_example_c1() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut block: [u8; 16] = std::array::from_fn(|i| (i as u8) * 0x11);
        Aes128::new(&key).encrypt_block(&mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
    }

    #[test]
    fn cbc_chains_the_blocks() {
        // Vector from NIST SP 800-38A, F.2.1: the first two blocks of CBC-AES128.
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let mut iv: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut data: Vec<u8> = vec![
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac,
            0x45, 0xaf, 0x8e, 0x51,
        ];
        cbc_encrypt(&Aes128::new(&key), &mut iv, &mut data);
        assert_eq!(
            hex(&data),
            "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2"
        );
    }
}

/// MD5 digest, from RFC 1321.
///
/// BREW exposes this as `IHash` under `AEECLSID_MD5`. Games use it to check the integrity of
/// their own data — Zeeboids computes the digest of a string and compares it with the one it
/// keeps —, so what matters is matching the specification, not being fast.
///
/// The implementation is streaming: `update` can be called any number of times, with chunks of
/// any size, because that is how the BREW interface is used.
#[derive(Debug, Clone)]
pub struct Md5 {
    /// The four state registers, `A`, `B`, `C` and `D`.
    state: [u32; 4],
    /// Bytes that have not yet filled a 64-byte block.
    buffer: Vec<u8>,
    /// Total bytes seen, which goes into the final padding.
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

/// Rotation amounts, one row per round (table in section 3.4 of the RFC).
const MD5_SHIFTS: [[u32; 4]; 4] = [
    [7, 12, 17, 22],
    [5, 9, 14, 20],
    [4, 11, 16, 23],
    [6, 10, 15, 21],
];

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476],
            buffer: Vec::new(),
            length: 0,
        }
    }

    /// The internal state, for the save state: the four registers, the bytes that have not yet
    /// filled a 64-byte block, and the total seen.
    ///
    /// **An MD5 in the middle of a computation is real state.** Without this, a game that saves
    /// between two `Update` calls of a digest loses what it had already accumulated — and the
    /// digest comes out wrong later, with nothing pointing at the save state. The state fits in a
    /// few bytes: four words and, at most, sixty-three bytes of remainder.
    pub fn state(&self) -> ([u32; 4], &[u8], u64) {
        (self.state, &self.buffer, self.length)
    }

    /// Restores the state recorded by [`Md5::state`].
    ///
    /// Rejects a block remainder larger than sixty-three bytes: that is not MD5 state, it is the
    /// wrong file — and such a remainder would make the next `compress` read what it shouldn't.
    pub fn restore_state(&mut self, state: [u32; 4], buffer: &[u8], length: u64) -> Result<(), String> {
        if buffer.len() >= 64 {
            return Err(format!(
                "the block remainder has {} bytes, and MD5 keeps at most 63",
                buffer.len()
            ));
        }
        self.state = state;
        self.buffer = buffer.to_vec();
        self.length = length;
        Ok(())
    }

    /// The constant `T[i]`, defined in the RFC as `floor(2^32 * abs(sin(i + 1)))`.
    fn t(i: usize) -> u32 {
        ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length += data.len() as u64;
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= 64 {
            let block: [u8; 64] = self.buffer[..64].try_into().expect("64 bytes");
            self.compress(&block);
            self.buffer.drain(..64);
        }
    }

    /// Finalizes the digest. Consumes the state because a finalized MD5 takes nothing more.
    pub fn finish(mut self) -> [u8; 16] {
        // The padding is a 1 bit, zeros until eight bytes are left in the block, and the length
        // in bits as a 64-bit little-endian integer.
        let bits = self.length.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bits.to_le_bytes());
        let pending = std::mem::take(&mut self.buffer);
        for block in pending.chunks_exact(64) {
            self.compress(block.try_into().expect("64 bytes"));
        }
        let mut out = [0u8; 16];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let m: [u32; 16] = std::array::from_fn(|i| {
            u32::from_le_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ])
        });
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            // Each round of sixteen steps has its own function and its own way of picking the
            // message word.
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let sum = a
                .wrapping_add(f)
                .wrapping_add(Self::t(i))
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(MD5_SHIFTS[i / 16][i % 4]));
        }
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }
}

#[cfg(test)]
mod md5_tests {
    use super::*;

    fn summary(entry: &[u8]) -> String {
        let mut h = Md5::new();
        h.update(entry);
        h.finish().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The vectors from appendix A.5 of RFC 1321.
    #[test]
    fn matches_the_rfc_vectors() {
        assert_eq!(summary(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(summary(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(summary(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            summary(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            summary(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            summary(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    /// The BREW interface delivers the data in chunks, and the result has to be the same.
    #[test]
    fn feeding_in_pieces_gives_the_same_digest() {
        let integer = summary(b"abcdefghijklmnopqrstuvwxyz");
        let mut h = Md5::new();
        for chunk in b"abcdefghijklmnopqrstuvwxyz".chunks(7) {
            h.update(chunk);
        }
        let chunked: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(chunked, integer);
    }

    /// More than one 64-byte block, to exercise compression in sequence.
    #[test]
    fn a_long_message_spans_several_blocks() {
        let entry = vec![b'x'; 1000];
        let mut h = Md5::new();
        h.update(&entry);
        let a: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        let mut h = Md5::new();
        for chunk in entry.chunks(64) {
            h.update(chunk);
        }
        let b: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }
}
