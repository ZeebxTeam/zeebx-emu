//! Saída de som do host, e a mistura das vozes que tocam ao mesmo tempo.
//!
//! O console mistura vários sons de uma vez, e cada um vem na taxa em que foi gravado — o
//! Peteca tem sons a 11025, 22050 e 44100 Hz na mesma sessão. Quem toca não pode se importar
//! com isso, então o mixer reamostra cada voz para a taxa da placa e soma tudo.
//!
//! O estado fica atrás de um [`Mutex`] compartilhado com a linha de execução de áudio, que é
//! quem o consome. Ela roda em tempo real: nada aqui aloca nem bloqueia dentro do laço de
//! mistura.

pub mod midi;
pub mod mp3;
pub mod wav;

/// Síntese por banco de amostras. Ver a feature `soundfont` e o cabeçalho do módulo.
#[cfg(feature = "soundfont")]
pub mod soundfont;

/// Onde o banco de amostras deve ficar, quando a build não tem o sintetizador de banco.
///
/// A mensagem existe nos dois casos de propósito: uma compilação sem a feature precisa dizer que
/// **não tem** o recurso, e não calar — silêncio aqui vira "o banco não funciona".
#[cfg(not(feature = "soundfont"))]
pub mod soundfont {
    use std::path::Path;

    pub fn relato(aparelho: &Path) -> String {
        let pasta = aparelho.join("soundfonts");
        format!(
            "Zeebx: esta compilação não tem o sintetizador de banco de amostras; o MIDI toca com a \
             tabela de timbres (o .sf2 iria em {})",
            pasta.display()
        )
    }
}

use std::sync::{Arc, Mutex};

#[cfg(feature = "audio")]
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::wav::Sound;

/// Uma voz tocando: o som, onde ela está e como sai.
#[derive(Debug)]
struct Voice {
    sound: Arc<Sound>,
    /// Posição em quadros do som, fracionária porque a taxa dele raramente é a da placa.
    position: f64,
    /// Quanto avançar por quadro da placa: a razão entre as duas taxas.
    step: f64,
    volume: f32,
    /// Quantas vezes ainda tocar. `None` é para sempre, que é o `MM_PARM_PLAY_REPEAT` zero.
    remaining: Option<u32>,
    paused: bool,
    done: bool,
    /// Quadros que faltam para a voz sumir, quando o som chegou ao fim.
    ///
    /// **Uma voz que some de uma vez estala.** O som acaba, o nível pode estar em 0,947 — foi o
    /// medido na Peteca — e a amostra seguinte é zero: a queda de quase o curso inteiro em uma
    /// amostra é um estalo audível, e não uma propriedade do som do jogo. A descida leva
    /// [`DESCIDA_FRAMES`] quadros, que a 44,1 kHz são menos de dois milissegundos: some sem degrau
    /// e sem atrasar o efeito seguinte.
    descida: u32,
}

/// Quantos quadros a voz leva para sumir quando o som acaba.
///
/// 64 quadros a 44,1 kHz são 1,5 ms — abaixo do que o ouvido distingue como silêncio à parte, e
/// suficiente para o degrau deixar de existir.
const DESCIDA_FRAMES: u32 = 64;

impl Voice {
    /// A amostra do canal `channel` na posição corrente, interpolada entre os dois quadros
    /// vizinhos. Sem a interpolação, reamostrar 11025 para 48000 chia.
    fn sample(&self, channel: usize) -> f32 {
        let channels = self.sound.channels as usize;
        let frames = self.sound.frames();
        if frames == 0 {
            return 0.0;
        }
        let index = self.position.floor() as usize;
        let fraction = (self.position - self.position.floor()) as f32;
        // Som mono alimenta os dois canais; som estéreo usa o canal pedido.
        let lane = channel.min(channels - 1);
        let at = |frame: usize| -> f32 {
            self.sound
                .samples
                .get(frame.min(frames - 1) * channels + lane)
                .copied()
                .unwrap_or(0.0)
        };
        at(index) + (at(index + 1) - at(index)) * fraction
    }

    /// Avança um quadro da placa, tratando o fim do som e a repetição.
    fn advance(&mut self) {
        self.position += self.step;
        if (self.position as usize) < self.sound.frames() {
            return;
        }
        match &mut self.remaining {
            None => self.position = 0.0,
            // O fim do som **não** corta: segura o último valor e desce em `DESCIDA_FRAMES`
            // quadros. Repetir não é fim — a volta começa de novo no começo e não precisa disto.
            Some(0) | Some(1) => {
                if self.descida == 0 {
                    self.descida = DESCIDA_FRAMES;
                }
            }
            Some(left) => {
                *left -= 1;
                self.position = 0.0;
            }
        }
    }

    /// Anda um quadro da placa: ou o som avança, ou a descida consome o que resta dele.
    ///
    /// **Os dois caminhos da mixagem usam este passo** — o audível e o mudo. O mudo também precisa
    /// consumir a descida: sem isso a voz nunca chegaria ao fim ali, e o teste que cobra "o som
    /// andou até o fim" pegou exatamente isso.
    fn passo(&mut self) {
        match self.descida {
            0 => self.advance(),
            1 => self.done = true,
            restante => self.descida = restante - 1,
        }
    }

    /// O quanto esta voz ainda vale, de 1,0 durante o som a 0,0 no fim da descida.
    fn decaimento(&self) -> f32 {
        match self.descida {
            0 => 1.0,
            restante => restante as f32 / DESCIDA_FRAMES as f32,
        }
    }
}

/// Uma voz que o jogo alimenta aos poucos: amostras chegam enquanto ela toca.
///
/// É o som dos ports de arcade da Data East, que geram o áudio do emulador deles quadro a quadro e
/// o entregam por um `ISource`. Não há fim conhecido nem duração: o que falta vira silêncio até a
/// próxima remessa.
#[derive(Debug)]
struct Stream {
    /// Amostras intercaladas por canal, ainda não tocadas.
    samples: std::collections::VecDeque<f32>,
    channels: usize,
    /// Quanto avançar no fluxo por quadro da placa.
    step: f64,
    /// Onde a placa está entre `previous` e `current`, de 0 a 1.
    fraction: f64,
    previous: [f32; 2],
    current: [f32; 2],
    volume: f32,
    paused: bool,
    /// O máximo de amostras guardadas: se o jogo entrega mais rápido do que a placa toca, as mais
    /// antigas saem, em vez de o atraso crescer sem fim.
    capacity: usize,
    /// Quanto uma amostra restante cai por quadro da saída quando o fluxo seca.
    ///
    /// Segurar a última amostra durante uma falta curta transforma um underrun em tensão DC e
    /// soa como um zumbido/estalo. Um fade curtíssimo mantém continuidade sem inventar áudio.
    underrun_decay: f32,
}

impl Stream {
    /// Avança um quadro da placa e devolve o quadro estéreo daquele instante.
    fn next_frame(&mut self) -> [f32; 2] {
        self.fraction += self.step;
        while self.fraction >= 1.0 {
            self.fraction -= 1.0;
            self.previous = self.current;
            if self.samples.len() < self.channels {
                // Faltou amostra: não segure o último valor indefinidamente. Isso vira uma
                // componente DC audível (principalmente nos ports que alimentam PCM em blocos).
                // Cai suavemente para zero em poucos milissegundos e não acumula atraso.
                self.fraction = 0.0;
                let out = self.current;
                for sample in &mut self.current {
                    *sample *= self.underrun_decay;
                    if sample.abs() < 0.0001 {
                        *sample = 0.0;
                    }
                }
                self.previous = self.current;
                return out;
            }
            let left = self.samples.pop_front().unwrap_or(0.0);
            let right = match self.channels {
                1 => left,
                _ => {
                    let right = self.samples.pop_front().unwrap_or(0.0);
                    for _ in 2..self.channels {
                        self.samples.pop_front();
                    }
                    right
                }
            };
            self.current = [left, right];
        }
        let f = self.fraction as f32;
        [
            self.previous[0] + (self.current[0] - self.previous[0]) * f,
            self.previous[1] + (self.current[1] - self.previous[1]) * f,
        ]
    }
}

#[derive(Debug, Default)]
struct State {
    /// As vozes, indexadas pelo objeto `IMedia` do guest que as criou.
    voices: std::collections::HashMap<u32, Voice>,
    /// As vozes alimentadas aos poucos, pelo mesmo índice.
    streams: std::collections::HashMap<u32, Stream>,
    /// Volume geral, de 0 a 1.
    master: f32,
    muted: bool,
    rate: u32,
}

/// O mixer, compartilhado entre o emulador e a linha de execução de áudio.
#[derive(Debug, Clone)]
pub struct Mixer {
    state: Arc<Mutex<State>>,
}

impl Mixer {
    fn new(rate: u32, master: f32, muted: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                rate,
                master,
                muted,
                ..State::default()
            })),
        }
    }

    /// Começa a tocar `sound` na voz `id`, substituindo o que houvesse nela.
    ///
    /// `repeat` segue o `MM_PARM_PLAY_REPEAT` do BREW: 1 toca uma vez, 0 toca para sempre.
    pub fn play(&self, id: u32, sound: Arc<Sound>, volume: f32, repeat: u32) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let step = f64::from(sound.rate) / f64::from(state.rate.max(1));
        state.voices.insert(
            id,
            Voice {
                descida: 0,
                sound,
                position: 0.0,
                step,
                volume,
                remaining: match repeat {
                    0 => None,
                    times => Some(times),
                },
                paused: false,
                done: false,
            },
        );
    }

    /// Abre na voz `id` um fluxo de `rate` Hz e `channels` canais, vazio.
    pub fn open_stream(&self, id: u32, rate: u32, channels: u16, volume: f32) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let channels = usize::from(channels.max(1));
        let step = f64::from(rate) / f64::from(state.rate.max(1));
        // O decaimento é aplicado quando avançamos um quadro da fonte, não um quadro da saída.
        // Usar a taxa da placa aqui alonga o fade quando um jogo entrega 11/22 kHz.
        // Aproximadamente 5 ms até -60 dB, independentemente das duas taxas.
        let fade_frames = (rate.max(1) as f32 * 0.005).max(1.0);
        let underrun_decay = 0.001_f32.powf(1.0 / fade_frames);
        state.voices.remove(&id);
        state.streams.insert(
            id,
            Stream {
                samples: std::collections::VecDeque::with_capacity(rate as usize * channels),
                channels,
                step,
                fraction: 0.0,
                previous: [0.0; 2],
                current: [0.0; 2],
                volume: volume.clamp(0.0, 1.0),
                paused: false,
                // Meio segundo de folga.
                capacity: rate as usize * channels / 2,
                underrun_decay,
            },
        );
    }

    /// Entrega amostras intercaladas ao fluxo da voz `id`.
    pub fn feed_stream(&self, id: u32, samples: &[f32]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(stream) = state.streams.get_mut(&id) else {
            return;
        };
        stream.samples.extend(samples.iter().copied());
        let excess = stream.samples.len().saturating_sub(stream.capacity);
        let excess = excess - excess % stream.channels;
        stream.samples.drain(..excess);
    }

    pub fn stop(&self, id: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.voices.remove(&id);
            state.streams.remove(&id);
        }
    }

    pub fn set_paused(&self, id: u32, paused: bool) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.paused = paused;
            }
            if let Some(stream) = state.streams.get_mut(&id) {
                stream.paused = paused;
            }
        }
    }

    pub fn set_volume(&self, id: u32, volume: f32) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.volume = volume.clamp(0.0, 1.0);
            }
            if let Some(stream) = state.streams.get_mut(&id) {
                stream.volume = volume.clamp(0.0, 1.0);
            }
        }
    }

    /// Se a voz `id` ainda está tocando.
    ///
    /// Quem decide o fim de um som para o jogo é o relógio virtual, não o mixer — o emulador
    /// roda mudo sem deixar de contar o tempo. Isto aqui é a janela dos testes para a vida das
    /// vozes, e é só para isso que serve.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_playing(&self, id: u32) -> bool {
        self.state
            .lock()
            .map(|state| state.voices.contains_key(&id) || state.streams.contains_key(&id))
            .unwrap_or(false)
    }

    pub fn set_master(&self, volume: f32, muted: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.master = volume.clamp(0.0, 1.0);
            state.muted = muted;
        }
    }

    /// Um mixer sem placa nenhuma, para gravar em arquivo o que sairia pelo alto-falante.
    ///
    /// Existe para poder **conferir** o som: sem isto, a única forma de saber se o áudio está
    /// certo é ouvi-lo, e isso não cabe num teste nem numa execução automática.
    pub fn silent(rate: u32) -> Self {
        Self::new(rate, 1.0, false)
    }

    /// Mistura `frames` quadros estéreo e devolve as amostras intercaladas.
    pub fn render(&self, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        self.fill(&mut out, 2);
        out
    }

    /// Preenche `out` com a mistura das vozes. `channels` é quantos canais a placa quer.
    fn fill(&self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        let channels = channels.max(1);
        let frames = out.len() / channels;
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let master = match state.muted {
            true => 0.0,
            false => state.master,
        };
        if master == 0.0 {
            // Mudo ainda consome as vozes: o som continua correndo, só não sai.
            for voice in state.voices.values_mut() {
                if !voice.paused && !voice.done {
                    for _ in 0..frames {
                        voice.passo();
                    }
                }
            }
            state.voices.retain(|_, voice| !voice.done);
            for stream in state.streams.values_mut() {
                if !stream.paused {
                    for _ in 0..frames {
                        stream.next_frame();
                    }
                }
            }
            return;
        }
        for voice in state.voices.values_mut() {
            if voice.paused || voice.done {
                continue;
            }
            for frame in out.chunks_mut(channels) {
                if voice.done {
                    break;
                }
                // **O ganho fica dentro do laço.** O `decaimento()` muda a cada quadro enquanto a
                // voz desce; içá-lo para fora congelaria o primeiro valor e apagaria a descida.
                let gain = voice.volume * master * voice.decaimento();
                for (channel, slot) in frame.iter_mut().enumerate() {
                    *slot += voice.sample(channel) * gain;
                }
                // Depois de começar a descida, o som não avança mais: o que se ouve é o último
                // valor, cada vez menor. Quando a descida acaba, a voz sai.
                voice.passo();
            }
        }
        for stream in state.streams.values_mut() {
            if stream.paused {
                continue;
            }
            let gain = stream.volume * master;
            for frame in out.chunks_mut(channels) {
                let stereo = stream.next_frame();
                for (channel, slot) in frame.iter_mut().enumerate() {
                    *slot += stereo[channel.min(1)] * gain;
                }
            }
        }
        // Somar vozes estoura a faixa; cortar é o que uma placa faria de qualquer forma, e é
        // melhor que deixar o valor dar a volta e virar estalo.
        for sample in out.iter_mut() {
            *sample = sample.clamp(-1.0, 1.0);
        }
        // A voz que acabou sai aqui: é o que faz `is_playing` dizer a verdade, e é a única
        // limpeza necessária — quem começa um som novo no mesmo objeto substitui a voz.
        state.voices.retain(|_, voice| !voice.done);
    }
}

/// A saída de áudio. Enquanto ela existe, o som toca; largá-la fecha o fluxo.
///
/// Só existe com a feature `desktop`: um frontend Libretro recebe o que o mixer gera por callback
/// e não tem placa própria para abrir.
#[cfg(feature = "audio")]
pub struct Output {
    _stream: cpal::Stream,
    mixer: Mixer,
}

#[cfg(feature = "audio")]
impl Output {
    /// Abre a placa padrão do sistema.
    ///
    /// Devolve o motivo em texto quando não dá: um host sem áudio não pode impedir o jogo de
    /// rodar, então quem chama trata isso como "sem som", não como erro fatal.
    pub fn open(volume: f32, muted: bool) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("nenhuma saída de áudio disponível")?;
        // O backend Android do CPAL 0.15 escolhe 44,1 kHz pela heurística genérica mesmo em
        // aparelhos cuja saída nativa é 48 kHz. Isso faz o nosso mixer reamostrar para 44,1 e o
        // Android reamostrar de novo para 48. Preferir 48 kHz/F32 evita essa segunda conversão.
        let config = if cfg!(target_os = "android") {
            let rate = cpal::SampleRate(48_000);
            let ranges: Vec<_> = device
                .supported_output_configs()
                .map(|configs| configs.collect())
                .unwrap_or_default();
            let preferred = ranges
                .iter()
                .copied()
                .filter(|range| {
                    range.channels() == 2 && range.sample_format() == cpal::SampleFormat::F32
                })
                .find_map(|range| range.try_with_sample_rate(rate))
                .or_else(|| {
                    ranges
                        .iter()
                        .copied()
                        .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
                        .find_map(|range| range.try_with_sample_rate(rate))
                });
            match preferred {
                Some(config) => config,
                None => device
                    .default_output_config()
                    .map_err(|err| err.to_string())?,
            }
        } else {
            device
                .default_output_config()
                .map_err(|err| err.to_string())?
        };
        let channels = config.channels() as usize;
        let mixer = Mixer::new(config.sample_rate().0, volume, muted);
        let mut stream_config = config.config();
        // O emulador pode ter picos pesados de CPU/GPU. Dar ~21 ms de capacidade ao Oboe evita
        // que uma fatia ruim vire crackle, sem empurrar a latência para valores perceptivelmente
        // altos.
        //
        // **Não há recuo para o padrão do driver, e não falta:** o backend Oboe do cpal 0.15.3
        // traduz `BufferSize::Fixed(n)` em `set_buffer_capacity_in_frames(n)` e não tem caminho
        // de recusa. Um `match` de retry aqui seria um ramo que nunca roda.
        if cfg!(target_os = "android") {
            stream_config.buffer_size = cpal::BufferSize::Fixed(1024);
        }

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mixer = mixer.clone();
                device.build_output_stream(
                    &stream_config,
                    move |out: &mut [f32], _| mixer.fill(out, channels),
                    |err| eprintln!("erro na saída de áudio: {err}"),
                    None,
                )
            }
            other => return Err(format!("formato de áudio não suportado: {other}")),
        }
        .map_err(|err| err.to_string())?;
        stream.play().map_err(|err| err.to_string())?;
        Ok(Self {
            _stream: stream,
            mixer,
        })
    }

    pub fn mixer(&self) -> Mixer {
        self.mixer.clone()
    }
}

#[cfg(feature = "audio")]
impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output").finish_non_exhaustive()
    }
}

/// Escreve amostras estéreo como um WAVE de 16 bits.
///
/// É a contrapartida do leitor: o emulador lê RIFF do jogo e, quando se quer conferir o que
/// saiu, escreve RIFF de volta.
pub fn to_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let payload: Vec<u8> = samples
        .iter()
        .flat_map(|&s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&1u16.to_le_bytes()); // PCM
    fmt.extend_from_slice(&2u16.to_le_bytes()); // estéreo
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * 4).to_le_bytes()); // bytes por segundo
    fmt.extend_from_slice(&4u16.to_le_bytes()); // alinhamento do quadro
    fmt.extend_from_slice(&16u16.to_le_bytes());

    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&((36 + payload.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
    out.extend_from_slice(&fmt);
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize) -> Arc<Sound> {
        Arc::new(Sound {
            rate,
            channels: 1,
            samples: (0..frames).map(|_| 1.0).collect(),
        })
    }

    /// Um mixer sem placa, para testar a mistura sem depender de áudio no host.
    fn mixer(rate: u32) -> Mixer {
        Mixer::new(rate, 1.0, false)
    }

    #[test]
    fn o_que_sai_do_mixer_volta_a_ser_um_wave() {
        // O despejo em arquivo é como se confere o som sem ouvi-lo, então a ida e a volta
        // precisam bater.
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 8), 1.0, 1);
        let samples = mixer.render(4);
        let wave = to_wav(&samples, 8000);
        let lido = crate::audio::wav::parse(&wave).unwrap();
        assert_eq!(lido.rate, 8000);
        assert_eq!(lido.channels, 2);
        assert_eq!(lido.frames(), 4);
        assert!(lido.samples.iter().all(|&s| s > 0.9));
    }

    #[test]
    fn uma_voz_sai_no_volume_pedido() {
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 100), 0.5, 1);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn duas_vozes_somam_e_o_resultado_e_cortado() {
        // Três vozes cheias somariam 3.0; a placa recebe 1.0, não um valor que dá a volta.
        let mixer = mixer(8000);
        for id in 1..=3 {
            mixer.play(id, tone(8000, 100), 1.0, 1);
        }
        let mut out = [0.0f32; 4];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 1.0).abs() < 1e-6));
    }

    #[test]
    fn a_voz_que_termina_sai_da_lista() {
        // Dois quadros de som numa placa da mesma taxa: o som acaba, e a voz **desce** antes de
        // sair — some sozinha, senão a lista de vozes cresceria a cada efeito tocado.
        let mixer = mixer(8000);
        mixer.play(7, tone(8000, 2), 1.0, 1);
        let mut out = [0.0f32; (DESCIDA_FRAMES as usize + 16) * 2];
        mixer.fill(&mut out, 2);
        assert!(!mixer.is_playing(7), "a voz sai depois da descida");
    }

    /// **A descida existe, e é monótona.**
    ///
    /// O som acaba no meio da onda, o nível pode estar em quase o curso inteiro, e cortar ali é um
    /// estalo. A voz segura o último valor e desce em [`DESCIDA_FRAMES`] quadros; como a posição
    /// não anda nesse trecho, a saída é exatamente uma rampa — e é isso que o teste cobra.
    #[test]
    fn a_voz_desce_em_vez_de_sumir_de_uma_vez() {
        let mixer = mixer(8000);
        mixer.play(7, tone(8000, 2), 1.0, 1);
        let mut out = vec![0.0f32; (DESCIDA_FRAMES as usize + 4) * 2];
        mixer.fill(&mut out, 2);
        let esquerdo: Vec<f32> = out.chunks_exact(2).map(|par| par[0]).collect();
        let ultimo = esquerdo[..DESCIDA_FRAMES as usize].to_vec();
        assert!(
            ultimo.windows(2).all(|par| par[1] <= par[0] + 1e-6),
            "a descida tem de ser monótona: {ultimo:?}"
        );
        assert!(
            ultimo.first().copied().unwrap_or(0.0) > 0.0,
            "a descida começa onde o som parou"
        );
    }

    #[test]
    fn repeticao_infinita_nao_termina() {
        // `MM_PARM_PLAY_REPEAT` zero é "toca para sempre".
        let mixer = mixer(8000);
        mixer.play(3, tone(8000, 2), 1.0, 0);
        let mut out = [0.0f32; 64];
        mixer.fill(&mut out, 2);
        assert!(mixer.is_playing(3));
    }

    #[test]
    fn uma_voz_pausada_nao_sai_e_nao_anda() {
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 4), 1.0, 1);
        mixer.set_paused(1, true);
        let mut out = [0.0f32; 16];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0));
        // E continua de onde parou quando voltar.
        mixer.set_paused(1, false);
        mixer.fill(&mut out, 2);
        assert!(out[0] > 0.0);
    }

    #[test]
    fn fluxo_que_seca_cai_para_silencio_em_vez_de_zumbir() {
        // Um bloco curto não pode deixar a última amostra presa na saída. Antes, um fluxo que
        // atrasasse a próxima remessa sustentava 1.0 indefinidamente — tensão DC audível como
        // estalo/zumbido. A última amostra sai inteira uma vez e depois desaparece em ~5 ms.
        let mixer = mixer(1000);
        mixer.open_stream(9, 1000, 1, 1.0);
        mixer.feed_stream(9, &[1.0]);
        let samples = mixer.render(12);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        assert!(left.iter().any(|sample| *sample > 0.9), "{left:?}");
        assert!(left.last().copied().unwrap_or(1.0).abs() < 0.001, "{left:?}");
    }

    #[test]
    fn fade_de_underrun_nao_depende_da_taxa_da_placa() {
        // A fonte é 1 kHz e a placa 4 kHz. O decaimento acontece ao consumir quadros da fonte;
        // depois de ~10 ms ele já deve estar efetivamente em silêncio mesmo com a placa 4x maior.
        let mixer = mixer(4000);
        mixer.open_stream(10, 1000, 1, 1.0);
        mixer.feed_stream(10, &[1.0]);
        let samples = mixer.render(48);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        assert!(left.iter().any(|sample| *sample > 0.9), "{left:?}");
        assert!(left.last().copied().unwrap_or(1.0).abs() < 0.001, "{left:?}");
    }

    #[test]
    fn a_taxa_do_som_e_convertida_para_a_da_placa() {
        // Som de 4000 Hz numa placa de 8000: cada quadro do som rende dois da placa, então
        // quatro quadros de som viram oito antes de acabar.
        let mixer = mixer(8000);
        mixer.play(1, tone(4000, 4), 1.0, 1);
        let mut out = [0.0f32; 12]; // seis quadros estéreo
        mixer.fill(&mut out, 2);
        assert!(mixer.is_playing(1), "ainda não podia ter acabado");
        let mut resto = [0.0f32; (DESCIDA_FRAMES as usize + 16) * 2];
        mixer.fill(&mut resto, 2);
        assert!(!mixer.is_playing(1), "sai depois da descida");
    }

    #[test]
    fn reamostrar_preserva_a_altura_do_som() {
        // Um seno de 1000 Hz gravado a 8000 e tocado numa placa de 32000 tem que continuar
        // sendo 1000 Hz. Inverter a razão entre as taxas soa igualmente "certo" num teste de
        // volume, e só a frequência denuncia — daí contar as passagens por zero.
        let rate = 8000;
        let hertz = 1000.0;
        let sound = Arc::new(Sound {
            rate,
            channels: 1,
            samples: (0..rate)
                .map(|i| (i as f32 / rate as f32 * hertz * std::f32::consts::TAU).sin())
                .collect(),
        });

        let device = 32000;
        let mixer = mixer(device);
        mixer.play(1, sound, 1.0, 1);
        // Meio segundo da placa, em quadros estéreo.
        let samples = mixer.render(device as usize / 2);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let crossings = left
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        // Meio segundo de 1000 Hz tem mil passagens por zero.
        assert!(
            (900..=1100).contains(&crossings),
            "esperava perto de 1000 passagens por zero, deu {crossings}"
        );
    }

    #[test]
    fn o_fluxo_toca_o_que_o_jogo_entrega_e_segura_o_ultimo_valor_quando_falta() {
        // Um fluxo de 8000 Hz numa placa de 8000: quadro entregue é quadro tocado.
        let mixer = mixer(8000);
        mixer.open_stream(7, 8000, 1, 1.0);
        mixer.feed_stream(7, &[0.5, -0.5, 0.25]);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        // Os dois canais recebem o mesmo. O fluxo parte do silêncio: a primeira amostra entregue
        // sai no quadro seguinte, e o que falta segura o último valor em vez de estalar.
        assert_eq!(out[0], out[1]);
        assert_eq!(&out[..8], &[0.0, 0.0, 0.5, 0.5, -0.5, -0.5, 0.25, 0.25]);
        assert!(mixer.is_playing(7));
        mixer.stop(7);
        assert!(!mixer.is_playing(7));
    }

    #[test]
    fn no_mudo_o_som_corre_mas_nao_sai() {
        // Silenciar não é pausar: quem volta o volume no meio de uma música espera achá-la
        // adiantada, não parada.
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 4), 1.0, 1);
        mixer.set_master(1.0, true);
        let mut out = [0.0f32; (DESCIDA_FRAMES as usize + 8) * 2];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0), "no mudo nada sai");
        assert!(!mixer.is_playing(1), "o som andou até o fim, inclusive a descida");
    }
}

#[cfg(test)]
mod testes_do_relato {
    /// **A build sem o sintetizador também responde**, e diz que não tem — em vez de calar.
    /// Silêncio aqui viraria a mesma conclusão errada do outro lado: "o banco não funciona".
    #[cfg(not(feature = "soundfont"))]
    #[test]
    fn o_relato_avisa_que_a_build_nao_tem_banco() {
        let aparelho = std::env::temp_dir().join("zeebx-aparelho-sem-feature");
        let texto = super::soundfont::relato(&aparelho);
        assert!(
            texto.contains("não tem o sintetizador de banco"),
            "o relato tem de dizer que a build não tem o recurso: {texto}"
        );
        assert!(texto.contains("soundfonts"), "e onde o arquivo iria: {texto}");
    }
}
