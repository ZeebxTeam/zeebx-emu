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
//!
//! Censo de formatos (outubro de 2026, 62 ROMs do acervo: soltos mais `RIFF` embutidos nos
//! `.mod`): só PCM (44 arquivos) e IMA-ADPCM (29) — nenhum IEEE-float, A-law, μ-law ou
//! MS-ADPCM. É por isso que só esses dois existem aqui; se um `NotPcm(tag)` aparecer na
//! natureza, a lista do `dr_mp3.h`/`dr_wav.h` do Infuse é o checklist do que fazer.

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
///
/// Quanto o bloco `data` pode passar do fim que o `RIFF` declara antes de valer o `RIFF`.
const FOLGA_DO_RIFF: usize = 4096;

/// Um WAVE de PCM em que o bloco `data` passa muito do fim que o `RIFF` declara, com as posições
/// em bytes contadas do começo do buffer.
///
/// É o caso em que o cabeçalho não decide sozinho onde o som acaba. No Zeebo F.C. Super League o
/// `RIFF` está certo e o que vem depois é lixo; na Turma da Mônica o `RIFF` é de um molde e a fala
/// continua depois dele, **escrita enquanto toca**. Quem separa os dois é o que o jogo faz com o
/// buffer depois do `Play` — ver `Machine::bombeia_buffers_vivos`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmAberto {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    /// Onde começa o PCM.
    pub inicio: usize,
    /// Onde o `RIFF` diz que o som acaba.
    pub fim_riff: usize,
    /// Onde o bloco `data` acaba, limitado ao buffer.
    pub fim_data: usize,
}

/// O [`PcmAberto`] de um buffer, ou `None` quando o cabeçalho basta — outro formato, ou um `data`
/// que não passa do `RIFF` mais que a folga.
pub fn pcm_aberto(data: &[u8]) -> Option<PcmAberto> {
    let estrutura = estrutura(data).ok()?;
    let (tag, channels, rate, _, bits) = estrutura.format?;
    let (inicio, fim_data) = estrutura.data?;
    let passa_do_riff =
        estrutura.fim_do_riff >= 44 && fim_data > estrutura.fim_do_riff + FOLGA_DO_RIFF;
    (tag == FORMAT_PCM && matches!(bits, 8 | 16) && channels > 0 && rate > 0 && passa_do_riff)
        .then_some(PcmAberto {
            channels,
            rate,
            bits,
            inicio,
            fim_riff: estrutura.fim_do_riff.max(inicio),
            fim_data,
        })
}

/// Os blocos de um RIFF/WAVE, sem interpretar o PCM.
struct Estrutura {
    /// `(formato, canais, taxa, alinhamento de bloco, bits)`.
    format: Option<(u16, u16, u32, u16, u16)>,
    /// Onde o bloco `data` começa e acaba, limitado ao buffer.
    data: Option<(usize, usize)>,
    fim_do_riff: usize,
}

fn estrutura(data: &[u8]) -> Result<Estrutura, WavError> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let fim_do_riff = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize + 8;
    let mut estrutura = Estrutura {
        format: None,
        data: None,
        fim_do_riff,
    };
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
                estrutura.format = Some((read16(0), read16(2), rate, read16(12), read16(14)));
            }
            b"data" => estrutura.data = Some((body, end)),
            _ => {}
        }
        at = body + len as usize + (len as usize & 1);
    }
    Ok(estrutura)
}

pub fn parse(data: &[u8]) -> Result<Sound, WavError> {
    // **O arquivo acaba onde o RIFF diz, e não onde o buffer acaba.** O Zeebo F.C. Super League
    // monta cada som num buffer de rascunho de 500 KB e escreve no bloco `data` o tamanho do
    // buffer inteiro, mas o `RIFF` com o tamanho de verdade (14 KB). Seguir o `data` tocava onze
    // segundos: o efeito e depois lixo de memória, alto, por cima da música. Um `RIFF` que não cabe
    // num cabeçalho — zero, ou maior que o buffer, como o de quem grava em fluxo — não limita nada.
    //
    // Só vale quando o `data` passa **muito** do fim declarado: um arquivo editado com um bloco a
    // mais e o `RIFF` desatualizado erra por poucos bytes, e cortá-lo perderia o fim do som.
    //
    // Esta é a leitura de um instante. Quando o jogo continua escrevendo depois do `RIFF` enquanto
    // o som toca — a fala da Turma da Mônica —, quem estende o som é a reprodução, e não o
    // parser: ver [`pcm_aberto`].
    let estrutura = estrutura(data)?;
    let (Some((tag, channels, rate, align, bits)), Some((body, end))) =
        (estrutura.format, estrutura.data)
    else {
        return Err(WavError::Incomplete);
    };
    let fim_do_riff = estrutura.fim_do_riff;
    let end = match fim_do_riff >= 44 && end > fim_do_riff + FOLGA_DO_RIFF {
        true => fim_do_riff.max(body),
        false => end,
    };
    let payload = &data[body..end];
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

    /// O som ocupa o começo de um buffer maior, o `data` diz o tamanho do buffer e o `RIFF` diz o
    /// do som: vale o `RIFF`, e o lixo depois dele não toca. É o buffer de rascunho do Zeebo F.C.
    /// Super League.
    ///
    /// **O `sfx_bal_all.wav` da Turma da Mônica não confirma esta regra**, embora já tenha sido
    /// citado como prova. O buffer de 882.000 bytes dela é um molde com o cabeçalho desse efeito
    /// (`RIFF` de 56.352), e por ele passam as falas, de até cinco segundos, decodificadas enquanto
    /// tocam. O `RIFF` do molde nunca muda — medido em trinta segundos de jogo. Quem estende o som
    /// além dele é a reprodução: ver [`pcm_aberto`].
    #[test]
    fn o_riff_limita_um_data_maior_que_o_som() {
        assert_eq!(parse(&buffer_de_rascunho()).unwrap().frames(), 2);
    }

    /// Dois quadros de som, o `RIFF` dizendo isso e o `data` dizendo 10.000 bytes de buffer.
    fn buffer_de_rascunho() -> Vec<u8> {
        let som = build(FORMAT_PCM, 1, 22050, 16, &[0x10, 0x00, 0x20, 0x00]);
        let mut buffer = som.clone();
        buffer.extend(std::iter::repeat(0x7f).take(9996));
        let data_em = som.len() - 4 - 4;
        buffer[data_em..data_em + 4].copy_from_slice(&10_000u32.to_le_bytes());
        buffer
    }

    /// O buffer em que o cabeçalho não decide sozinho é reconhecido, com as três posições.
    #[test]
    fn o_pcm_aberto_diz_onde_cada_cabecalho_acaba() {
        let buffer = buffer_de_rascunho();
        let aberto = pcm_aberto(&buffer).expect("o data passa do RIFF");
        assert_eq!((aberto.rate, aberto.channels, aberto.bits), (22050, 1, 16));
        assert_eq!(aberto.inicio, 44);
        assert_eq!(aberto.fim_riff, 48);
        assert_eq!(aberto.fim_data, buffer.len());
    }

    /// Um WAVE comum, e um com o `RIFF` só um pouco atrás do `data`, não são abertos: o
    /// cabeçalho basta.
    #[test]
    fn um_wave_comum_nao_e_aberto() {
        let som = build(FORMAT_PCM, 1, 22050, 16, &[0; 4000]);
        assert_eq!(pcm_aberto(&som), None);
        let mut editado = som.clone();
        editado[4..8].copy_from_slice(&((som.len() - 8 - 100) as u32).to_le_bytes());
        assert_eq!(pcm_aberto(&editado), None);
        assert_eq!(pcm_aberto(b"RIFF\0\0\0\0WAVE"), None);
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
