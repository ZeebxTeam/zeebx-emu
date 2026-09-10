//! MP3: a música dos jogos, decodificada — e cronometrada quando não dá para decodificar.
//!
//! **A música do Zeebo é MP3.** Nove jogos a entregam assim: Quake, Quake 2, Galaxy on Fire,
//! Rally Master Pro, Powerboat Challenge, Action Hero 3D, Need For Speed, zeetris e a Z-Wheel.
//! Efeito sonoro é RIFF/WAVE e sempre tocou; música não tocava em jogo nenhum, e a razão era
//! esta — o formato inteiro faltava.
//!
//! Duas coisas vivem aqui, e é útil saber por quê:
//!
//! - [`decode`] devolve o som. O Layer III é Huffman, requantização, estéreo conjunto, IMDCT e
//!   banco de síntese; escrever isso sem uma referência para comparar dá som que toca e sai no
//!   tom errado. A decodificação é do `symphonia`, Rust puro — a mesma decisão do SQLite para o
//!   `ISQLMgr` e do `ab_glyph` para o `DrawText`.
//! - [`probe`] lê só o cabeçalho e a etiqueta do codificador, e diz **quanto dura**. Continua
//!   valendo: um MP3 que o decodificador recuse ainda pode ser cronometrado, e cronometrar é o
//!   que impede o jogo de travar.
//!
//! O segundo caminho nasceu do Tekken 2. A música dele chega por `IMEDIA_SetMediaParm` e nós só
//! sabíamos ler RIFF/WAVE. Sem som legível o `Play` respondia "esse som já acabou", o jogo
//! consultava o estado, via "pronto" e mandava tocar de novo — **766 mil vezes em quatro
//! segundos virtuais**. Não era um jogo pesado: era um jogo insistindo contra uma resposta
//! nossa. Com a duração, o som "toca" em silêncio pelo tempo certo do relógio virtual e o jogo
//! segue. Hoje o Tekken toca de verdade, e o caminho da duração ficou como rede de segurança.

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

/// Decodifica o MP3 inteiro para amostras. `None` quando não é um MP3 que se possa ler.
///
/// Devolve o mesmo [`Sound`](crate::wav::Sound) que o RIFF/WAVE produz, e por isso o misturador
/// não precisa saber de onde o som veio: reamostragem, volume e repetição já funcionam iguais.
///
/// A decodificação é feita **de uma vez**, e não em fluxo. Uma trilha de trinta e seis segundos
/// a 22 kHz mono são três milhões de amostras, seis megabytes de `f32` — cabe, e o resultado
/// fica no cache de sons do `machine.rs`, decodificado uma vez por trilha e não por `Play`.
/// Fluxo seria o certo para um jogo que troque de música o tempo todo; nenhum dos nossos faz
/// isso, e streaming acrescentaria estado e uma linha de execução a mais para nada.
pub fn decode(data: &[u8]) -> Option<crate::wav::Sound> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // O `MediaSourceStream` quer posse do que lê, e o buffer é do guest: copiar é o preço de
    // não deixar o decodificador olhando para a memória do jogo enquanto ele a reescreve.
    let fonte = std::io::Cursor::new(data.to_vec());
    let fluxo = MediaSourceStream::new(Box::new(fonte), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let sondado = symphonia::default::get_probe()
        .format(
            &hint,
            fluxo,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .ok()?;
    let mut formato = sondado.format;
    let trilha = formato.default_track()?;
    let id = trilha.id;
    let mut decodificador = symphonia::default::get_codecs()
        .make(&trilha.codec_params, &DecoderOptions::default())
        .ok()?;

    let mut samples: Vec<f32> = Vec::new();
    let mut rate = 0;
    let mut channels = 0;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let pacote = match formato.next_packet() {
            Ok(pacote) => pacote,
            // Fim do arquivo chega como erro de leitura, e é o desfecho normal.
            Err(_) => break,
        };
        if pacote.track_id() != id {
            continue;
        }
        let quadro = match decodificador.decode(&pacote) {
            Ok(quadro) => quadro,
            // Quadro estragado no meio não invalida a música: o codificador de época deixa
            // lixo no começo de alguns arquivos, e parar aí devolveria silêncio.
            Err(Error::DecodeError(_)) => continue,
            Err(_) => break,
        };
        let spec = *quadro.spec();
        rate = spec.rate;
        channels = spec.channels.count() as u16;
        let buffer = buffer.get_or_insert_with(|| {
            SampleBuffer::new(quadro.capacity() as u64, spec)
        });
        buffer.copy_interleaved_ref(quadro);
        samples.extend_from_slice(buffer.samples());
    }
    if samples.is_empty() || rate == 0 || channels == 0 {
        return None;
    }
    Some(crate::wav::Sound {
        rate,
        channels,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Um quarto de segundo de 440 Hz, mono, 22.050 Hz, gerado com o `lame`.
    ///
    /// Fixture de verdade em vez de bytes montados à mão: o que se quer provar é que um MP3 real
    /// entra e sai com a altura certa, e cabeçalho sintético nenhum prova isso. Entra só no
    /// binário de teste.
    const TOM: &[u8] = include_bytes!("../assets/teste/tom-440hz.mp3");

    /// A música sai com a taxa, os canais e **a altura** certos.
    ///
    /// A altura é o que se confere contando cruzamentos por zero: um som que toca, parece bem e
    /// está no tom errado passaria por qualquer outra verificação — é o mesmo cuidado que o
    /// misturador já tem com a reamostragem.
    #[test]
    fn um_mp3_de_verdade_decodifica_com_a_altura_certa() {
        let som = decode(TOM).expect("o tom de 440 Hz é um MP3 legível");
        assert_eq!((som.rate, som.channels), (22050, 1));
        // O codificador acrescenta silêncio no começo e no fim, então a duração não é exata.
        let segundos = som.frames() as f32 / som.rate as f32;
        assert!((0.2..0.4).contains(&segundos), "{segundos} s");
        let pico = som.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(pico > 0.3, "som quase mudo: pico {pico}");
        // Só o miolo: as bordas têm o silêncio do codificador, e zero não cruza zero.
        let miolo = &som.samples[som.samples.len() / 4..som.samples.len() * 3 / 4];
        let cruzamentos = miolo
            .windows(2)
            .filter(|par| (par[0] < 0.0) != (par[1] < 0.0))
            .count();
        let hz = cruzamentos as f32 / 2.0 / (miolo.len() as f32 / som.rate as f32);
        assert!((430.0..450.0).contains(&hz), "altura medida: {hz} Hz");
    }

    /// O que não é MP3 não vira som — e não entra em pânico.
    #[test]
    fn o_que_nao_e_mp3_nao_decodifica() {
        assert!(decode(b"RIFF\x00\x00\x00\x00WAVEfmt ").is_none());
        assert!(decode(&[]).is_none());
        assert!(decode(&[0xff; 512]).is_none());
    }

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
