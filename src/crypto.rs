//! O AES-128 em CBC, que é o que o BREW entrega pelo `AEECLSID_BlockAES128`.
//!
//! Escrito aqui em vez de vir de uma dependência porque é pequeno, fechado e verificável: os
//! testes usam os vetores das próprias especificações — o FIPS-197 para o bloco e o NIST
//! SP 800-38A para o encadeamento.

/// Caixa de substituição do AES, da tabela 4 do FIPS-197.
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

/// Multiplicação no corpo do AES, `GF(2^8)` com o polinômio `x^8 + x^4 + x^3 + x + 1`.
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

/// Cifra de bloco AES-128, com as chaves de rodada já expandidas.
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

    /// Cifra um bloco de 16 bytes no lugar.
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

/// O estado do AES é uma matriz preenchida por colunas, então a linha `r` do byte `i` é
/// `i % 4` — e deslocar a linha `r` por `r` posições mexe em bytes espalhados pelo bloco.
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
        let c: [u8; 4] = column.try_into().expect("chunk de quatro");
        column[0] = multiply(c[0], 2) ^ multiply(c[1], 3) ^ c[2] ^ c[3];
        column[1] = c[0] ^ multiply(c[1], 2) ^ multiply(c[2], 3) ^ c[3];
        column[2] = c[0] ^ c[1] ^ multiply(c[2], 2) ^ multiply(c[3], 3);
        column[3] = multiply(c[0], 3) ^ c[1] ^ c[2] ^ multiply(c[3], 2);
    }
}

/// Cifra `data` em CBC, no lugar, avançando `iv`. O comprimento precisa ser múltiplo de 16 —
/// o preenchimento é decidido pelo `ICipher1`, não aqui.
pub fn cbc_encrypt(cipher: &Aes128, iv: &mut [u8; 16], data: &mut [u8]) {
    for block in data.chunks_exact_mut(16) {
        for (byte, previous) in block.iter_mut().zip(iv.iter()) {
            *byte ^= previous;
        }
        let mut current: [u8; 16] = block.try_into().expect("chunk de dezesseis");
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
    fn aes128_bate_com_o_vetor_do_fips_197() {
        // Apêndice B do FIPS-197.
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
    fn aes128_bate_com_o_exemplo_c1_do_fips_197() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let mut block: [u8; 16] = std::array::from_fn(|i| (i as u8) * 0x11);
        Aes128::new(&key).encrypt_block(&mut block);
        assert_eq!(hex(&block), "69c4e0d86a7b0430d8cdb78070b4c55a");
    }

    #[test]
    fn cbc_encadeia_os_blocos() {
        // Vetor do NIST SP 800-38A, F.2.1: os dois primeiros blocos do CBC-AES128.
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

/// Resumo MD5, da RFC 1321.
///
/// O BREW expõe isto como `IHash` sob o `AEECLSID_MD5`. Os jogos usam para conferir integridade
/// dos próprios dados — o Zeeboids calcula o resumo de uma string e compara com o que ele
/// guarda —, então o que importa é bater com a especificação, não ser rápido.
///
/// A implementação é de fluxo: `update` pode ser chamado quantas vezes for, com pedaços de
/// qualquer tamanho, porque é assim que a interface do BREW é usada.
#[derive(Debug, Clone)]
pub struct Md5 {
    /// Os quatro registradores de estado, `A`, `B`, `C` e `D`.
    state: [u32; 4],
    /// Bytes que ainda não completaram um bloco de 64.
    buffer: Vec<u8>,
    /// Total de bytes vistos, que entra no preenchimento final.
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

/// Deslocamentos de rotação, uma linha por rodada (tabela da seção 3.4 da RFC).
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

    /// A constante `T[i]`, definida na RFC como `floor(2^32 * abs(sin(i + 1)))`.
    fn t(i: usize) -> u32 {
        ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32
    }

    pub fn update(&mut self, data: &[u8]) {
        self.length += data.len() as u64;
        self.buffer.extend_from_slice(data);
        while self.buffer.len() >= 64 {
            let bloco: [u8; 64] = self.buffer[..64].try_into().expect("64 bytes");
            self.compress(&bloco);
            self.buffer.drain(..64);
        }
    }

    /// Fecha o resumo. Consome o estado porque um MD5 fechado não recebe mais nada.
    pub fn finish(mut self) -> [u8; 16] {
        // O preenchimento é um bit 1, zeros até faltarem oito bytes no bloco, e o comprimento
        // em bits como inteiro de 64 bits little-endian.
        let bits = self.length.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bits.to_le_bytes());
        let pendente = std::mem::take(&mut self.buffer);
        for bloco in pendente.chunks_exact(64) {
            self.compress(bloco.try_into().expect("64 bytes"));
        }
        let mut out = [0u8; 16];
        for (i, palavra) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&palavra.to_le_bytes());
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
            // Cada rodada de dezesseis passos tem a sua função e a sua forma de escolher a
            // palavra da mensagem.
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let soma = a
                .wrapping_add(f)
                .wrapping_add(Self::t(i))
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(soma.rotate_left(MD5_SHIFTS[i / 16][i % 4]));
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

    fn resumo(entrada: &[u8]) -> String {
        let mut h = Md5::new();
        h.update(entrada);
        h.finish().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Os vetores do apêndice A.5 da RFC 1321.
    #[test]
    fn bate_com_os_vetores_da_rfc() {
        assert_eq!(resumo(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(resumo(b"a"), "0cc175b9c0f1b6a831c399e269772661");
        assert_eq!(resumo(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            resumo(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );
        assert_eq!(
            resumo(b"abcdefghijklmnopqrstuvwxyz"),
            "c3fcd3d76192e4007dfb496cca67e13b"
        );
        assert_eq!(
            resumo(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            ),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    /// A interface do BREW entrega os dados aos pedaços, e o resultado tem de ser o mesmo.
    #[test]
    fn entregar_aos_pedacos_da_o_mesmo_resumo() {
        let inteiro = resumo(b"abcdefghijklmnopqrstuvwxyz");
        let mut h = Md5::new();
        for pedaco in b"abcdefghijklmnopqrstuvwxyz".chunks(7) {
            h.update(pedaco);
        }
        let partido: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(partido, inteiro);
    }

    /// Mais de um bloco de 64 bytes, para exercitar a compressão em sequência.
    #[test]
    fn mensagem_longa_atravessa_varios_blocos() {
        let entrada = vec![b'x'; 1000];
        let mut h = Md5::new();
        h.update(&entrada);
        let a: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        let mut h = Md5::new();
        for pedaco in entrada.chunks(64) {
            h.update(pedaco);
        }
        let b: String = h.finish().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }
}
