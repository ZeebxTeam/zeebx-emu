//! Saída de som do host, e a mistura das vozes que tocam ao mesmo tempo.
//!
//! O console mistura vários sons de uma vez, e cada um vem na taxa em que foi gravado — o
//! Peteca tem sons a 11025, 22050 e 44100 Hz na mesma sessão. Quem toca não pode se importar
//! com isso, então o mixer reamostra cada voz para a taxa da placa e soma tudo.
//!
//! O estado fica atrás de um [`Mutex`] compartilhado com a linha de execução de áudio, que é
//! quem o consome. Ela roda em tempo real: nada aqui aloca nem bloqueia dentro do laço de
//! mistura.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::wav::Sound;

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
}

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
            Some(0) | Some(1) => self.done = true,
            Some(left) => {
                *left -= 1;
                self.position = 0.0;
            }
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// As vozes, indexadas pelo objeto `IMedia` do guest que as criou.
    voices: std::collections::HashMap<u32, Voice>,
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

    pub fn stop(&self, id: u32) {
        if let Ok(mut state) = self.state.lock() {
            state.voices.remove(&id);
        }
    }

    pub fn set_paused(&self, id: u32, paused: bool) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.paused = paused;
            }
        }
    }

    pub fn set_volume(&self, id: u32, volume: f32) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(voice) = state.voices.get_mut(&id) {
                voice.volume = volume.clamp(0.0, 1.0);
            }
        }
    }

    /// Se a voz `id` ainda está tocando.
    pub fn is_playing(&self, id: u32) -> bool {
        self.state
            .lock()
            .map(|state| state.voices.contains_key(&id))
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
                    for _ in 0..out.len() / channels.max(1) {
                        voice.advance();
                    }
                }
            }
            state.voices.retain(|_, voice| !voice.done);
            return;
        }
        for voice in state.voices.values_mut() {
            if voice.paused || voice.done {
                continue;
            }
            for frame in out.chunks_mut(channels.max(1)) {
                if voice.done {
                    break;
                }
                let gain = voice.volume * master;
                for (channel, slot) in frame.iter_mut().enumerate() {
                    *slot += voice.sample(channel) * gain;
                }
                voice.advance();
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
pub struct Output {
    _stream: cpal::Stream,
    mixer: Mixer,
}

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
        let config = device
            .default_output_config()
            .map_err(|err| err.to_string())?;
        let channels = config.channels() as usize;
        let mixer = Mixer::new(config.sample_rate().0, volume, muted);

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let mixer = mixer.clone();
                device.build_output_stream(
                    &config.into(),
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
        let lido = crate::wav::parse(&wave).unwrap();
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
        // Dois quadros de som numa placa da mesma taxa: depois de dois quadros, acabou — e
        // some sozinha, senão a lista de vozes cresceria a cada efeito tocado.
        let mixer = mixer(8000);
        mixer.play(7, tone(8000, 2), 1.0, 1);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        assert!(!mixer.is_playing(7));
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
    fn a_taxa_do_som_e_convertida_para_a_da_placa() {
        // Som de 4000 Hz numa placa de 8000: cada quadro do som rende dois da placa, então
        // quatro quadros de som viram oito antes de acabar.
        let mixer = mixer(8000);
        mixer.play(1, tone(4000, 4), 1.0, 1);
        let mut out = [0.0f32; 12]; // seis quadros estéreo
        mixer.fill(&mut out, 2);
        assert!(mixer.is_playing(1), "ainda não podia ter acabado");
        mixer.fill(&mut out, 2);
        assert!(!mixer.is_playing(1));
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
    fn no_mudo_o_som_corre_mas_nao_sai() {
        // Silenciar não é pausar: quem volta o volume no meio de uma música espera achá-la
        // adiantada, não parada.
        let mixer = mixer(8000);
        mixer.play(1, tone(8000, 4), 1.0, 1);
        mixer.set_master(1.0, true);
        let mut out = [0.0f32; 8];
        mixer.fill(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(!mixer.is_playing(1), "o som andou até o fim");
    }
}
