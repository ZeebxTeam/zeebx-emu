//! Leitura de RIFF/WAVE.
//!
//! É o formato em que os jogos entregam som ao BREW: eles passam um `AEEMediaData` de
//! `MMD_BUFFER` apontando para um RIFF em memória. Quake, Zeebo Sports Peteca e Double Dragon
//! usam PCM sem compressão, mono, de 8 ou 16 bits, em taxas de 11025 a 44100 Hz; o Pac-Mania
//! usa IMA ADPCM, que comprime cada amostra em quatro bits.
//!
//! Escrito à mão em vez de vir de uma dependência porque é um cabeçalho de doze bytes e uma
//! lista de blocos. Um formato que ainda não sabemos ler é **recusado com nome**, e não
//! decodificado por acaso: é o nome que diz o que implementar depois.

/// `WAVE_FORMAT_PCM` e `WAVE_FORMAT_IMA_ADPCM`, do cabeçalho de formato do RIFF.
const FORMAT_PCM: u16 = 1;
const FORMAT_IMA_ADPCM: u16 = 17;

/// Quanto o índice da tabela de passos anda, por nibble. É a tabela do IMA ADPCM.
const INDEX_TABLE: [i8; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Os 89 passos de quantização do IMA ADPCM.
const STEP_TABLE: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

/// O estado de um canal do IMA ADPCM: a amostra anterior e onde estamos na tabela de passos.
#[derive(Debug, Clone, Copy)]
struct AdpcmChannel {
    predictor: i32,
    index: i32,
}

impl AdpcmChannel {
    /// Decodifica um nibble e avança o estado.
    fn decode(&mut self, nibble: u8) -> i16 {
        let step = STEP_TABLE[self.index.clamp(0, 88) as usize];
        // O nibble traz três bits de magnitude e um de sinal, e a soma é uma fração do passo:
        // `step/8` é o termo constante que existe mesmo quando os três bits são zero.
        let magnitude = i32::from(nibble & 7);
        let mut delta = step >> 3;
        if magnitude & 4 != 0 {
            delta += step;
        }
        if magnitude & 2 != 0 {
            delta += step >> 1;
        }
        if magnitude & 1 != 0 {
            delta += step >> 2;
        }
        if nibble & 8 != 0 {
            self.predictor -= delta;
        } else {
            self.predictor += delta;
        }
        self.predictor = self
            .predictor
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        self.index = (self.index + i32::from(INDEX_TABLE[(nibble & 0xf) as usize])).clamp(0, 88);
        self.predictor as i16
    }
}

/// Decodifica o IMA ADPCM de um WAVE.
///
/// Os dados vêm em blocos de `block_align` bytes. Cada bloco começa com quatro bytes por canal
/// — a primeira amostra e o índice da tabela — e segue com os nibbles. Em estéreo eles vêm
/// alternando de quatro em quatro bytes por canal; em mono, direto.
fn decode_ima_adpcm(payload: &[u8], channels: usize, block_align: usize) -> Vec<f32> {
    if channels == 0 || block_align < 4 * channels {
        return Vec::new();
    }
    let mut out = Vec::new();
    for block in payload.chunks(block_align) {
        if block.len() < 4 * channels {
            break;
        }
        let mut state: Vec<AdpcmChannel> = (0..channels)
            .map(|channel| {
                let at = channel * 4;
                AdpcmChannel {
                    predictor: i32::from(i16::from_le_bytes([block[at], block[at + 1]])),
                    index: i32::from(block[at + 2]),
                }
            })
            .collect();
        // A amostra do cabeçalho é a primeira de cada canal.
        for channel in &state {
            out.push(channel.predictor as f32 / 32768.0);
        }

        let body = &block[4 * channels..];
        // Cada grupo traz quatro bytes — oito amostras — de cada canal, em ordem.
        for group in body.chunks(4 * channels) {
            let mut decoded = vec![Vec::with_capacity(8); channels];
            for (channel, lane) in group.chunks(4).enumerate().take(channels) {
                for &byte in lane {
                    decoded[channel].push(state[channel].decode(byte & 0xf));
                    decoded[channel].push(state[channel].decode(byte >> 4));
                }
            }
            let frames = decoded.iter().map(Vec::len).min().unwrap_or(0);
            for frame in 0..frames {
                for lane in decoded.iter().take(channels) {
                    out.push(f32::from(lane[frame]) / 32768.0);
                }
            }
        }
    }
    out
}

/// Um som pronto para tocar: amostras normalizadas em `-1.0..=1.0`, intercaladas por canal.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Sound {
    /// Quantos quadros de áudio o som tem — uma amostra por canal conta como um.
    pub fn frames(&self) -> usize {
        match self.channels {
            0 => 0,
            channels => self.samples.len() / channels as usize,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    /// Não é um RIFF/WAVE.
    NotWave,
    /// Falta o bloco de formato ou o de dados.
    Incomplete,
    /// O formato existe, mas não é PCM. O número é o `wFormatTag`, que diz qual é.
    NotPcm(u16),
    /// Profundidade de bits que não sabemos ler.
    BadDepth(u16),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotWave => write!(f, "não é um RIFF/WAVE"),
            Self::Incomplete => write!(f, "faltam blocos obrigatórios no WAVE"),
            Self::NotPcm(tag) => write!(f, "o WAVE não é PCM (formato {tag})"),
            Self::BadDepth(bits) => write!(f, "o WAVE tem {bits} bits por amostra"),
        }
    }
}

/// Lê um RIFF/WAVE de PCM.
pub fn parse(data: &[u8]) -> Result<Sound, WavError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    // `(formato, canais, taxa, alinhamento de bloco, bits)`.
    let mut format: Option<(u16, u16, u32, u16, u16)> = None;
    let mut payload: Option<&[u8]> = None;

    // Os blocos vêm em sequência, cada um com identificador e tamanho. Um bloco de tamanho
    // ímpar é seguido de um byte de alinhamento que não conta no tamanho.
    let mut at = 12usize;
    while at + 8 <= data.len() {
        let id = &data[at..at + 4];
        let len = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
        let body = at + 8;
        let end = body.saturating_add(len as usize).min(data.len());
        match id {
            b"fmt " if end - body >= 16 => {
                let read16 = |i: usize| u16::from_le_bytes([data[body + i], data[body + i + 1]]);
                let rate = u32::from_le_bytes([
                    data[body + 4],
                    data[body + 5],
                    data[body + 6],
                    data[body + 7],
                ]);
                format = Some((read16(0), read16(2), rate, read16(12), read16(14)));
            }
            b"data" => payload = Some(&data[body..end]),
            _ => {}
        }
        at = body + len as usize + (len as usize & 1);
    }

    let (Some((tag, channels, rate, align, bits)), Some(payload)) = (format, payload) else {
        return Err(WavError::Incomplete);
    };
    if tag == FORMAT_IMA_ADPCM {
        return Ok(Sound {
            rate: rate.max(1),
            channels: channels.max(1),
            samples: decode_ima_adpcm(payload, channels.max(1) as usize, align as usize),
        });
    }
    if tag != FORMAT_PCM {
        return Err(WavError::NotPcm(tag));
    }
    let samples = match bits {
        // PCM de 8 bits é **sem sinal**, com o silêncio em 128; o de 16 é com sinal.
        8 => payload
            .iter()
            .map(|&b| (b as f32 - 128.0) / 128.0)
            .collect(),
        16 => payload
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
            .collect(),
        other => return Err(WavError::BadDepth(other)),
    };
    Ok(Sound {
        rate: rate.max(1),
        channels: channels.max(1),
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um WAVE com o formato e os bytes de dados dados. O alinhamento de bloco sai da
    /// profundidade, como num PCM.
    fn build(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
        build_aligned(tag, channels, rate, bits, channels * bits / 8, data)
    }

    /// A mesma coisa, com o alinhamento de bloco dito à parte — que é o que o ADPCM precisa,
    /// porque nele o bloco não tem relação com a profundidade.
    fn build_aligned(
        tag: u16,
        channels: u16,
        rate: u32,
        bits: u16,
        align: u16,
        data: &[u8],
    ) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        fmt.extend_from_slice(&(rate * u32::from(channels) * u32::from(bits) / 8).to_le_bytes());
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());

        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        let total = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&total.to_le_bytes());
        out
    }

    #[test]
    fn le_pcm_de_16_bits() {
        let mut data = Vec::new();
        for value in [0i16, 16384, -16384, i16::MIN] {
            data.extend_from_slice(&value.to_le_bytes());
        }
        let sound = parse(&build(FORMAT_PCM, 1, 22050, 16, &data)).unwrap();
        assert_eq!(sound.rate, 22050);
        assert_eq!(sound.channels, 1);
        assert_eq!(sound.frames(), 4);
        assert_eq!(sound.samples[0], 0.0);
        assert_eq!(sound.samples[1], 0.5);
        assert_eq!(sound.samples[3], -1.0);
    }

    #[test]
    fn o_pcm_de_8_bits_e_sem_sinal_com_o_silencio_em_128() {
        // Ler 8 bits como se fosse com sinal transforma silêncio em estouro; é o erro clássico
        // deste formato.
        let sound = parse(&build(FORMAT_PCM, 1, 11025, 8, &[128, 255, 0, 192])).unwrap();
        assert_eq!(sound.samples[0], 0.0);
        assert_eq!(sound.samples[1], (255.0 - 128.0) / 128.0);
        assert_eq!(sound.samples[2], -1.0);
        assert_eq!(sound.samples[3], 0.5);
    }

    #[test]
    fn blocos_desconhecidos_no_meio_sao_pulados() {
        // O Double Dragon manda um `bext` antes do `fmt `; parar no primeiro bloco estranho
        // perderia o som inteiro.
        let mut wave = build(FORMAT_PCM, 1, 8000, 8, &[128, 128]);
        let mut com_extra = wave[..12].to_vec();
        com_extra.extend_from_slice(b"bext");
        com_extra.extend_from_slice(&3u32.to_le_bytes());
        com_extra.extend_from_slice(&[1, 2, 3, 0]); // três bytes mais o alinhamento
        com_extra.extend_from_slice(&wave[12..]);
        wave = com_extra;
        let sound = parse(&wave).unwrap();
        assert_eq!(sound.frames(), 2);
    }

    #[test]
    fn um_formato_desconhecido_e_recusado_com_nome() {
        // Recusar dizendo qual é o formato é o que permite saber o que implementar depois: foi
        // assim que o ADPCM do Pac-Mania apareceu.
        let err = parse(&build(85, 1, 8000, 4, &[0, 1, 2, 3]));
        assert_eq!(err, Err(WavError::NotPcm(85)));
    }

    #[test]
    fn o_ima_adpcm_segue_a_tabela_de_passos() {
        // Valores conferidos à mão pela especificação do IMA. Bloco com a primeira amostra em
        // zero e o índice em zero, seguido dos nibbles 4 e 4:
        //   nibble 4: passo 7, delta = 7>>3 + 7 = 7  -> amostra 7,  índice vai a 2
        //   nibble 4: passo 9, delta = 9>>3 + 9 = 10 -> amostra 17, índice vai a 4
        // Os nibbles vêm com o de baixo primeiro, então os dois cabem no byte 0x44.
        let mut data = vec![0u8, 0, 0, 0]; // predictor = 0, índice = 0, reservado
        data.extend_from_slice(&[0x44, 0, 0, 0]);
        let sound = parse(&build_aligned(FORMAT_IMA_ADPCM, 1, 8000, 4, 8, &data)).unwrap();

        assert_eq!(sound.samples[0], 0.0, "a amostra do cabeçalho vem primeiro");
        assert_eq!(sound.samples[1], 7.0 / 32768.0);
        assert_eq!(sound.samples[2], 17.0 / 32768.0);
    }

    #[test]
    fn o_adpcm_com_bloco_maior_que_os_dados_nao_estoura() {
        // Um bloco anunciado com 1024 bytes e um punhado de dados de verdade tem que sair
        // curto, não derrubar o emulador.
        let sound = parse(&build_aligned(
            FORMAT_IMA_ADPCM,
            1,
            8000,
            4,
            1024,
            &[0, 0, 0, 0, 0x11],
        ))
        .unwrap();
        assert!(!sound.samples.is_empty());
    }

    #[test]
    fn o_que_nao_e_wave_e_recusado() {
        assert_eq!(parse(b"nao sou um wave"), Err(WavError::NotWave));
        assert_eq!(parse(&[]), Err(WavError::NotWave));
    }

    #[test]
    fn um_wave_sem_dados_nao_vira_som() {
        let mut wave = build(FORMAT_PCM, 1, 8000, 16, &[]);
        // Corta o bloco `data` fora.
        wave.truncate(wave.len() - 8);
        assert_eq!(parse(&wave), Err(WavError::Incomplete));
    }
}
