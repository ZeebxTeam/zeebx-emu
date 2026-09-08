//! Quanto tempo dura um MP3, sem decodificá-lo.
//!
//! Este módulo **não** toca MP3: ele lê o cabeçalho do primeiro quadro e, quando existe, a
//! etiqueta `Xing`/`Info` que o codificador deixou. Isso basta para saber a duração.
//!
//! A razão é o Tekken 2. A música dele é um MP3 entregue por `IMEDIA_SetMediaParm` e nós só
//! sabíamos ler RIFF/WAVE. Sem som legível o `Play` respondia "esse som já acabou", o jogo
//! consultava o estado, via "pronto" e mandava tocar de novo — **766 mil vezes em quatro
//! segundos virtuais**. Não era um jogo pesado: era um jogo insistindo contra uma resposta
//! nossa.
//!
//! Com a duração, o som "toca" em silêncio pelo tempo certo do relógio virtual e o jogo segue.
//! É uma troca declarada: o Tekken fica mudo, mas anda.

/// Um MP3 do qual sabemos o suficiente para cronometrá-lo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp3 {
    /// Amostras por segundo.
    pub rate: u32,
    /// Quantos quadros de áudio o arquivo tem.
    pub frames: u64,
    /// Amostras por quadro: 1152 no MPEG-1 e 576 no MPEG-2 e no MPEG-2.5.
    pub samples_per_frame: u32,
}

impl Mp3 {
    /// A duração em microssegundos.
    pub fn duration_us(&self) -> u64 {
        match self.rate {
            0 => 0,
            rate => self.frames * u64::from(self.samples_per_frame) * 1_000_000 / u64::from(rate),
        }
    }
}

/// Tabela de taxa de bits do Layer III, em kbps, indexada pelo campo do cabeçalho.
///
/// A linha do MPEG-1 e a do MPEG-2 são diferentes. O índice 0 (livre) e o 15 (inválido) não têm
/// taxa, e entram como zero: quem lê trata o zero como "não dá para estimar pelo tamanho".
const BITRATE_V1: [u32; 16] = [
    0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
];
const BITRATE_V2: [u32; 16] = [
    0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
];

/// Taxas de amostragem por versão, na ordem do campo do cabeçalho.
const RATES_V1: [u32; 3] = [44100, 48000, 32000];
const RATES_V2: [u32; 3] = [22050, 24000, 16000];
const RATES_V25: [u32; 3] = [11025, 12000, 8000];

/// Quantos bytes de dados o `ID3v2` na frente do arquivo ocupa, se houver.
///
/// O tamanho vem em quatro bytes de sete bits cada — o formato guarda assim para nunca formar
/// um `0xFF` que pareça a sincronia de um quadro.
fn id3_len(data: &[u8]) -> usize {
    if data.len() < 10 || &data[..3] != b"ID3" {
        return 0;
    }
    let size = data[6..10]
        .iter()
        .fold(0usize, |acc, byte| (acc << 7) | (*byte as usize & 0x7f));
    10 + size
}

/// Lê o que dá para saber de um MP3 sem decodificá-lo. `None` quando não é um.
pub fn probe(data: &[u8]) -> Option<Mp3> {
    let start = id3_len(data);
    let head = data.get(start..start + 4)?;
    // Sincronia: onze bits ligados.
    if head[0] != 0xff || head[1] & 0xe0 != 0xe0 {
        return None;
    }
    // Só o Layer III interessa: é o que "MP3" quer dizer, e é o que os jogos entregam.
    if (head[1] >> 1) & 0b11 != 0b01 {
        return None;
    }
    let (rates, bitrates, samples_per_frame) = match (head[1] >> 3) & 0b11 {
        0b11 => (RATES_V1, BITRATE_V1, 1152),
        0b10 => (RATES_V2, BITRATE_V2, 576),
        0b00 => (RATES_V25, BITRATE_V2, 576),
        // 0b01 é reservado.
        _ => return None,
    };
    let rate = *rates.get(((head[2] >> 2) & 0b11) as usize)?;
    let bitrate = bitrates[((head[2] >> 4) & 0b1111) as usize] * 1000;
    let mono = (head[3] >> 6) & 0b11 == 0b11;

    // A etiqueta do codificador vive no primeiro quadro, num deslocamento que depende da versão
    // e do número de canais — é onde ficariam os dados laterais, que o quadro de etiqueta não
    // usa. Quando ela traz a contagem de quadros, a duração é exata mesmo com taxa variável.
    let tag_at = start
        + 4
        + match (samples_per_frame, mono) {
            (1152, true) => 17,
            (1152, false) => 32,
            (_, true) => 9,
            (_, false) => 17,
        };
    let tag = data
        .get(tag_at..tag_at + 12)
        .filter(|tag| (&tag[..4] == b"Xing" || &tag[..4] == b"Info") && tag[7] & 1 == 1);
    if let Some(tag) = tag {
        let frames = u32::from_be_bytes([tag[8], tag[9], tag[10], tag[11]]);
        return Some(Mp3 {
            rate,
            frames: u64::from(frames),
            samples_per_frame,
        });
    }

    // Sem etiqueta, a conta é a da taxa constante: o tamanho dividido pela taxa de bits. Erra
    // em arquivo de taxa variável, e é a única estimativa possível sem varrer todos os quadros.
    if bitrate == 0 {
        return None;
    }
    let bytes = (data.len() - start) as u64;
    let seconds_by_bits = bytes * 8 * u64::from(rate) / u64::from(bitrate);
    Some(Mp3 {
        rate,
        frames: seconds_by_bits / u64::from(samples_per_frame),
        samples_per_frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O cabeçalho real do primeiro quadro da música do Tekken 2, com a etiqueta `Info` que o
    /// codificador dele deixou: MPEG-2, Layer III, 64 kbps, 22.050 Hz, mono.
    fn tekken_header(frames: u32) -> Vec<u8> {
        let mut out = vec![0xff, 0xf3, 0x80, 0xc4];
        out.extend(std::iter::repeat_n(0u8, 9));
        out.extend(b"Info");
        out.extend([0, 0, 0, 1]); // sinalizadores: só a contagem de quadros
        out.extend(frames.to_be_bytes());
        out.resize(140642, 0);
        out
    }

    #[test]
    fn a_etiqueta_do_codificador_da_a_duracao_exata() {
        let mp3 = probe(&tekken_header(1350)).expect("é um MP3");
        assert_eq!((mp3.rate, mp3.samples_per_frame), (22050, 576));
        assert_eq!(mp3.frames, 1350);
        // 1350 quadros de 576 amostras a 22.050 Hz são pouco menos de 36 segundos.
        assert_eq!(mp3.duration_us() / 1000, 35265);
    }

    #[test]
    fn sem_etiqueta_a_duracao_sai_da_taxa_de_bits() {
        let mut data = tekken_header(1350);
        // Estraga a etiqueta: sobra a conta do tamanho pela taxa de bits.
        data[13..17].copy_from_slice(b"xxxx");
        let mp3 = probe(&data).expect("é um MP3");
        // 140.642 bytes a 64 kbps são 17,5 segundos.
        assert_eq!(mp3.duration_us() / 1_000_000, 17);
    }

    #[test]
    fn um_id3_na_frente_nao_atrapalha() {
        let mut data = b"ID3\x04\x00\x00\x00\x00\x02\x01".to_vec();
        data.resize(10 + 257, 0);
        data.extend(tekken_header(100));
        let mp3 = probe(&data).expect("é um MP3");
        assert_eq!(mp3.frames, 100);
    }

    #[test]
    fn um_riff_nao_e_mp3() {
        assert_eq!(probe(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
        assert_eq!(probe(&[]), None);
        // Sincronia certa, camada errada: Layer II não é MP3.
        assert_eq!(probe(&[0xff, 0xf5, 0x80, 0xc4, 0, 0, 0, 0]), None);
    }
}
