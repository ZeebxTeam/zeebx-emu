//! MIDI: a música dos ports de arcade, sintetizada aqui.
//!
//! Onze jogos entregam a trilha como MIDI — Double Dragon, Bad Dudes, Caveman Ninja, Dark Seal,
//! Heavy Barrel, Karnov's Revenge, Magical Drop 3, Spin Master, Street Hoop, Super BurgerTime e
//! Wizard Fire. Cada um traz **um** `.wav`, que é o efeito, e a música chega pelo `IMedia` como
//! `audio/mid`.
//!
//! **MIDI não é som, é partitura.** Não há amostra dentro do arquivo: há "toque a nota 69 no
//! instrumento 25 com força 100". Quem transformava isso em som era o sintetizador do firmware do
//! console, com o banco de instrumentos dele — e esse banco não está na partição que temos. Então
//! o que este módulo faz é uma **aproximação declarada**: lê a partitura certa e a toca com
//! timbres nossos.
//!
//! Dizer isso na cara importa mais que o de costume, porque aqui o erro é silencioso: uma música
//! sintetizada errado *toca*, e soa como se fosse assim mesmo. Por isso o que este módulo cobra
//! de si é o que dá para verificar sem ouvir — **a nota certa, na hora certa, pelo tempo certo**:
//!
//! - a partitura é lida do jeito que a especificação manda, inclusive o status corrente;
//! - a conversão de pulso para segundo honra a divisão do cabeçalho e as mudanças de tempo;
//! - a nota 69 sai em 440 Hz, medido por correlação — e não por parecer certo.
//!
//! O timbre é o que **não** dá para verificar assim, e é justamente a parte que é palpite. Fica
//! registrado como hipótese em uso no relatório.

/// Taxa em que a música é sintetizada.
///
/// 22.050 Hz é a taxa das próprias trilhas dos jogos e metade da placa: o misturador reamostra
/// de qualquer jeito, e sintetizar em 44.100 dobraria o custo e a memória para agudo que não
/// existe na partitura.
pub const RATE: u32 = 22_050;

/// Teto de duração sintetizada, em segundos.
///
/// Uma trilha de jogo tem um ou dois minutos e repete — quem repete é o misturador, pelo
/// `MM_PARM_PLAY_REPEAT`. O teto existe para um arquivo estragado, cujo último evento pode cair
/// a horas de distância, não virar um pedido de gigabytes de memória.
const MAX_SEGUNDOS: f64 = 300.0;

/// Quantas notas podem soar ao mesmo tempo.
///
/// Passar disso não é música, é arquivo estragado ou nota que nunca foi solta. O corte evita que
/// um `Note On` sem `Note Off` empilhe voz até o fim da síntese.
const MAX_VOZES: usize = 48;

/// O canal da percussão no General MIDI. É o décimo, contado de zero.
const CANAL_PERCUSSAO: u8 = 9;

/// Microssegundos por batida antes de qualquer `Set Tempo`: 120 batidas por minuto, o padrão da
/// especificação.
const TEMPO_PADRAO: u32 = 500_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evento {
    Toca { canal: u8, nota: u8, forca: u8 },
    Solta { canal: u8, nota: u8 },
    Programa { canal: u8, programa: u8 },
    /// Volume (7) e expressão (11) do canal. O resto do `Control Change` é ignorado.
    Volume { canal: u8, valor: u8 },
    /// Todas as notas do canal soltas de uma vez — `All Notes Off` e `All Sound Off`.
    SoltaTudo { canal: u8 },
    Tempo { us_por_batida: u32 },
}

/// A partitura lida, em pulsos.
#[derive(Debug, Clone)]
struct Partitura {
    /// O campo `division` do cabeçalho, como está: positivo é pulso por batida, negativo é SMPTE.
    divisao: i16,
    /// Os eventos de todas as trilhas, juntos e em ordem de pulso.
    eventos: Vec<(u64, Evento)>,
}

/// Lê um número de tamanho variável: sete bits por byte, o bit alto dizendo que continua.
fn varlen(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let mut valor = 0u32;
    // Quatro bytes é o máximo que a especificação permite, e o limite impede que lixo com o bit
    // alto sempre ligado leve a leitura até o fim do arquivo.
    for _ in 0..4 {
        let byte = *bytes.get(*pos)?;
        *pos += 1;
        valor = (valor << 7) | u32::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some(valor);
        }
    }
    None
}

/// Lê uma trilha `MTrk` inteira, devolvendo os eventos com o pulso absoluto de cada um.
///
/// **O status corrente é obrigatório, não otimização.** Um arquivo real omite o byte de status
/// quando ele repete o anterior, e uma trilha de notas seguidas é quase toda assim: ignorar isso
/// não perde um evento, perde a trilha inteira a partir do primeiro.
fn le_trilha(bytes: &[u8]) -> Vec<(u64, Evento)> {
    let mut eventos = Vec::new();
    let mut pos = 0usize;
    let mut pulso = 0u64;
    let mut status = 0u8;
    while pos < bytes.len() {
        let Some(delta) = varlen(bytes, &mut pos) else {
            break;
        };
        pulso += u64::from(delta);
        let Some(&primeiro) = bytes.get(pos) else { break };
        // Byte de dados no lugar do status: vale o status anterior, e o byte é o primeiro dado.
        if primeiro & 0x80 != 0 {
            status = primeiro;
            pos += 1;
        }
        let canal = status & 0x0f;
        let dado = |pos: &mut usize| -> Option<u8> {
            let byte = *bytes.get(*pos)?;
            *pos += 1;
            Some(byte & 0x7f)
        };
        match status & 0xf0 {
            // `Note On` com força zero **é** um `Note Off`, e é assim que a maioria dos
            // sequenciadores solta nota. Tratar como toque deixaria a nota presa para sempre.
            0x90 => {
                let (Some(nota), Some(forca)) = (dado(&mut pos), dado(&mut pos)) else {
                    break;
                };
                eventos.push((
                    pulso,
                    match forca {
                        0 => Evento::Solta { canal, nota },
                        forca => Evento::Toca { canal, nota, forca },
                    },
                ));
            }
            0x80 => {
                let (Some(nota), Some(_)) = (dado(&mut pos), dado(&mut pos)) else {
                    break;
                };
                eventos.push((pulso, Evento::Solta { canal, nota }));
            }
            0xb0 => {
                let (Some(controle), Some(valor)) = (dado(&mut pos), dado(&mut pos)) else {
                    break;
                };
                match controle {
                    7 | 11 => eventos.push((pulso, Evento::Volume { canal, valor })),
                    120 | 123 => eventos.push((pulso, Evento::SoltaTudo { canal })),
                    _ => {}
                }
            }
            0xc0 => match dado(&mut pos) {
                Some(programa) => eventos.push((pulso, Evento::Programa { canal, programa })),
                None => break,
            },
            // Pressão de canal: um byte. Pressão de nota, afinação e controle: dois.
            0xd0 => {
                if dado(&mut pos).is_none() {
                    break;
                }
            }
            0xa0 | 0xe0 => {
                if dado(&mut pos).is_none() || dado(&mut pos).is_none() {
                    break;
                }
            }
            0xf0 => match status {
                // Meta evento: tipo, tamanho, dados.
                0xff => {
                    let Some(&tipo) = bytes.get(pos) else { break };
                    pos += 1;
                    let Some(tamanho) = varlen(bytes, &mut pos) else {
                        break;
                    };
                    let fim = pos + tamanho as usize;
                    if tipo == 0x51
                        && tamanho == 3
                        && let Some(t) = bytes.get(pos..pos + 3)
                    {
                        let us = u32::from_be_bytes([0, t[0], t[1], t[2]]);
                        if us > 0 {
                            eventos.push((pulso, Evento::Tempo { us_por_batida: us }));
                        }
                    }
                    // Fim de trilha (0x2f) e o resto dos metas: pular pelo tamanho declarado.
                    if tipo == 0x2f {
                        break;
                    }
                    pos = fim;
                }
                // SysEx, com tamanho declarado. O status corrente **não** vale depois dele.
                0xf0 | 0xf7 => {
                    let Some(tamanho) = varlen(bytes, &mut pos) else {
                        break;
                    };
                    pos += tamanho as usize;
                    status = 0;
                }
                _ => break,
            },
            // Status que não existe: sem saber o tamanho, não há como pular com segurança.
            _ => break,
        }
    }
    eventos
}

/// Lê um Standard MIDI File. `None` quando não é um.
fn le(data: &[u8]) -> Option<Partitura> {
    if data.get(..4)? != b"MThd" {
        return None;
    }
    let cabecalho = u32::from_be_bytes(data.get(4..8)?.try_into().ok()?) as usize;
    let corpo = data.get(8..8 + cabecalho.max(6))?;
    let formato = u16::from_be_bytes(corpo.get(..2)?.try_into().ok()?);
    let divisao = i16::from_be_bytes(corpo.get(4..6)?.try_into().ok()?);
    // Formato 2 é uma coleção de padrões independentes, não uma música; tocar as trilhas dele
    // juntas daria uma sobreposição que ninguém compôs.
    if formato > 1 || divisao == 0 {
        return None;
    }

    let mut eventos = Vec::new();
    let mut pos = 8 + cabecalho;
    while pos + 8 <= data.len() {
        let tamanho = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let corpo = data.get(pos + 8..(pos + 8 + tamanho).min(data.len()))?;
        // Um bloco que não é `MTrk` é pulado pelo tamanho: a especificação manda ignorar bloco
        // desconhecido, e há arquivo com bloco de autoria antes das trilhas.
        if &data[pos..pos + 4] == b"MTrk" {
            eventos.extend(le_trilha(corpo));
        }
        pos += 8 + tamanho;
    }
    if eventos.is_empty() {
        return None;
    }
    // Ordem estável por pulso: eventos no mesmo pulso mantêm a ordem em que apareceram, que é o
    // que faz um `Programa` antes do `Toca` no mesmo instante continuar valendo para ele.
    eventos.sort_by_key(|(pulso, _)| *pulso);
    Some(Partitura { divisao, eventos })
}

impl Partitura {
    /// Os eventos com o instante em segundos, aplicando a divisão e as mudanças de tempo.
    ///
    /// A conta não pode ser feita uma vez para todo o arquivo: o `Set Tempo` muda o tamanho do
    /// pulso no meio da música, e um `accelerando` recalculado do zero colocaria tudo o que vem
    /// depois no lugar errado. Então o instante é acumulado trecho a trecho.
    fn no_tempo(&self) -> Vec<(f64, Evento)> {
        let smpte = self.divisao < 0;
        // SMPTE: o byte alto é quadros por segundo (com 29 significando 29,97) e o baixo é
        // pulsos por quadro. Aqui o tempo não depende de batida, e o `Set Tempo` não vale.
        let por_pulso_smpte = match smpte {
            false => 0.0,
            true => {
                // O byte alto é o número de quadros **em complemento de dois** — 0xe7 é -25 —, e
                // não o campo inteiro negado: negar os dezesseis bits juntos dá 24 onde deveria
                // dar 25, e a música toca 4% fora do tempo.
                let bits = self.divisao as u16;
                let quadros = match -((bits >> 8) as u8 as i8) as i32 {
                    // 29 quadros quer dizer 29,97: é o vídeo NTSC, e a especificação diz isso.
                    29 => 29.97,
                    outros => f64::from(outros),
                };
                let subquadros = f64::from(bits & 0xff);
                1.0 / (quadros.max(1.0) * subquadros.max(1.0))
            }
        };
        let por_batida = f64::from(self.divisao.max(1) as u32);
        let mut us_por_batida = f64::from(TEMPO_PADRAO);
        let mut segundos = 0.0;
        let mut ultimo = 0u64;
        let mut saida = Vec::with_capacity(self.eventos.len());
        for &(pulso, evento) in &self.eventos {
            let passo = (pulso - ultimo) as f64;
            segundos += match smpte {
                true => passo * por_pulso_smpte,
                false => passo * us_por_batida / 1_000_000.0 / por_batida,
            };
            ultimo = pulso;
            if let Evento::Tempo { us_por_batida: us } = evento {
                us_por_batida = f64::from(us);
                continue;
            }
            saida.push((segundos, evento));
        }
        saida
    }
}

/// A forma de onda de uma voz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Forma {
    Senoide,
    Triangulo,
    Quadrada,
    Dente,
    /// Percussão: ruído com decaimento, sem altura definida.
    Ruido,
}

impl Forma {
    /// A amostra da forma na fase `fase` (uma volta é 1,0).
    ///
    /// Quadrada e dente são somadas por harmônicos até abaixo de Nyquist, e não geradas pela
    /// forma crua. Uma quadrada crua a 22 kHz **rebate**: os harmônicos acima da metade da taxa
    /// voltam como frequências que não existem na partitura, e o resultado é uma nota
    /// acompanhada de um assobio que sobe quando ela desce.
    fn amostra(self, fase: f32, harmonicos: u32, ruido: &mut u32) -> f32 {
        use std::f32::consts::TAU;
        match self {
            Self::Senoide => (fase * TAU).sin(),
            Self::Triangulo => {
                let mut soma = 0.0;
                let mut k = 1;
                while k <= harmonicos.min(9) {
                    let sinal = match (k / 2) % 2 {
                        0 => 1.0,
                        _ => -1.0,
                    };
                    soma += sinal * (fase * TAU * k as f32).sin() / (k * k) as f32;
                    k += 2;
                }
                soma * 8.0 / (std::f32::consts::PI * std::f32::consts::PI)
            }
            Self::Quadrada => {
                let mut soma = 0.0;
                let mut k = 1;
                while k <= harmonicos.min(9) {
                    soma += (fase * TAU * k as f32).sin() / k as f32;
                    k += 2;
                }
                soma * 4.0 / std::f32::consts::PI
            }
            Self::Dente => {
                let mut soma = 0.0;
                for k in 1..=harmonicos.min(12) {
                    soma += (fase * TAU * k as f32).sin() / k as f32;
                }
                soma * 2.0 / std::f32::consts::PI
            }
            Self::Ruido => {
                // Congruência linear: barato, determinístico e suficiente para percussão. Ruído
                // que muda de execução para execução faria o teste do misturador tremer.
                *ruido = ruido.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (*ruido >> 8) as f32 / 8_388_608.0 - 1.0
            }
        }
    }
}

/// Como um instrumento soa: forma, envoltória e ganho.
#[derive(Debug, Clone, Copy)]
struct Timbre {
    forma: Forma,
    ataque: f32,
    decaimento: f32,
    sustentacao: f32,
    liberacao: f32,
    ganho: f32,
}

/// O timbre de um programa do General MIDI, pela família dele.
///
/// **Isto é a parte que é palpite**, e é palpite por falta de dado, não por preguiça: o banco de
/// instrumentos do console está no firmware que ainda não lemos. O que a tabela tenta acertar é o
/// **comportamento** de cada família — piano decai, órgão sustenta, metal ataca devagar —, porque
/// é isso que faz a música ser reconhecível mesmo com o timbre errado.
fn timbre(programa: u8) -> Timbre {
    let (forma, ataque, decaimento, sustentacao, liberacao, ganho) = match programa {
        // Piano e percussão cromática: ataque imediato e decaimento, sem sustentação.
        0..=15 => (Forma::Triangulo, 0.002, 0.50, 0.25, 0.15, 0.9),
        // Órgão e acordeão: soam enquanto a tecla está apertada.
        16..=23 => (Forma::Quadrada, 0.01, 0.05, 0.90, 0.08, 0.6),
        // Violão e guitarra.
        24..=31 => (Forma::Dente, 0.004, 0.40, 0.30, 0.12, 0.7),
        // Baixo: fundamental forte, pouco harmônico.
        32..=39 => (Forma::Triangulo, 0.005, 0.30, 0.55, 0.10, 1.0),
        // Cordas e conjunto: ataque lento.
        40..=55 => (Forma::Dente, 0.06, 0.20, 0.80, 0.25, 0.6),
        // Metais.
        56..=63 => (Forma::Quadrada, 0.03, 0.15, 0.85, 0.12, 0.6),
        // Palheta e sopro.
        64..=79 => (Forma::Senoide, 0.02, 0.10, 0.85, 0.12, 0.8),
        // Lead sintetizado.
        80..=87 => (Forma::Quadrada, 0.005, 0.10, 0.80, 0.10, 0.6),
        // Pad e efeito: entrada e saída longas.
        88..=103 => (Forma::Senoide, 0.10, 0.30, 0.75, 0.40, 0.7),
        // Étnico, percussivo e efeito sonoro.
        _ => (Forma::Triangulo, 0.005, 0.35, 0.30, 0.15, 0.7),
    };
    Timbre {
        forma,
        ataque,
        decaimento,
        sustentacao,
        liberacao,
        ganho,
    }
}

/// O timbre de uma nota da percussão, pelo número dela.
///
/// Não há altura: o que muda entre um bombo e um prato é quanto tempo o ruído dura e quanto de
/// grave ele tem. Duas famílias bastam para a música ficar reconhecível.
fn percussao(nota: u8) -> (Timbre, f32) {
    let (decaimento, corte, ganho) = match nota {
        // Bombo e surdo.
        35 | 36 | 41 | 43 | 45 | 47 | 48 | 50 => (0.18, 90.0, 1.0),
        // Caixa e palmas.
        37..=40 => (0.14, 900.0, 0.7),
        // Pratos de condução e ataque: os mais longos.
        49 | 51 | 52 | 53 | 55 | 57 | 59 => (0.45, 5_000.0, 0.4),
        // Chimbau e o resto: curtos e agudos.
        _ => (0.07, 4_000.0, 0.5),
    };
    (
        Timbre {
            forma: Forma::Ruido,
            ataque: 0.001,
            decaimento,
            sustentacao: 0.0,
            liberacao: 0.02,
            ganho,
        },
        corte,
    )
}

/// Uma nota soando, do `Note On` até a envoltória zerar.
#[derive(Debug, Clone, Copy)]
struct Voz {
    canal: u8,
    nota: u8,
    /// Amostra em que a nota começou e em que ela foi solta.
    inicio: usize,
    solta: Option<usize>,
    frequencia: f32,
    timbre: Timbre,
    /// Corte do filtro da percussão, em Hz. Zero para nota com altura.
    corte: f32,
    amplitude: f32,
}

impl Voz {
    /// A envoltória em `t` segundos desde o começo, com a nota solta em `solta` segundos.
    fn envoltoria(&self, t: f32, solta: Option<f32>) -> f32 {
        let Timbre {
            ataque,
            decaimento,
            sustentacao,
            liberacao,
            ..
        } = self.timbre;
        if t < 0.0 {
            return 0.0;
        }
        let mantida = match t < ataque {
            true => t / ataque.max(1e-6),
            false => {
                let desde = t - ataque;
                match desde < decaimento {
                    true => 1.0 - (1.0 - sustentacao) * (desde / decaimento.max(1e-6)),
                    false => sustentacao,
                }
            }
        };
        match solta {
            Some(solta) if t > solta => {
                let desde = t - solta;
                match desde < liberacao {
                    true => mantida * (1.0 - desde / liberacao.max(1e-6)),
                    false => 0.0,
                }
            }
            _ => mantida,
        }
    }

    /// Quantas amostras esta voz ainda produz som depois de solta.
    fn cauda(&self) -> usize {
        (self.timbre.liberacao * RATE as f32) as usize + 1
    }
}

/// Sintetiza um MIDI. `None` quando não é um MIDI legível.
///
/// Devolve o mesmo [`Sound`](crate::wav::Sound) que o WAVE e o MP3 produzem — o misturador não
/// precisa saber que aqui não havia som nenhum, só partitura.
pub fn decode(data: &[u8]) -> Option<crate::wav::Sound> {
    let partitura = le(data)?;
    let eventos = partitura.no_tempo();
    let fim = eventos.last()?.0.min(MAX_SEGUNDOS);
    // Meio segundo de sobra para a última nota terminar de soltar.
    let total = ((fim + 0.5) * f64::from(RATE)) as usize + 1;
    let mut samples = vec![0.0f32; total];

    let mut programa = [0u8; 16];
    let mut volume = [1.0f32; 16];
    let mut soando: Vec<Voz> = Vec::new();
    let mut mortas: Vec<Voz> = Vec::new();

    let amostra_de = |segundos: f64| (segundos * f64::from(RATE)) as usize;
    for &(segundos, evento) in &eventos {
        if segundos > MAX_SEGUNDOS {
            break;
        }
        let agora = amostra_de(segundos);
        match evento {
            Evento::Programa { canal, programa: p } => programa[canal as usize & 15] = p,
            Evento::Volume { canal, valor } => {
                volume[canal as usize & 15] = f32::from(valor) / 127.0
            }
            Evento::Tempo { .. } => {}
            Evento::SoltaTudo { canal } => {
                for voz in soando.iter_mut().filter(|v| v.canal == canal) {
                    voz.solta.get_or_insert(agora);
                }
                mortas.extend(soando.iter().copied().filter(|v| v.canal == canal));
                soando.retain(|v| v.canal != canal);
            }
            Evento::Solta { canal, nota } => {
                // Só a nota mais antiga é solta, e não todas as iguais: a mesma nota tocada duas
                // vezes antes de ser solta são duas vozes, e soltar as duas de uma vez corta a
                // segunda antes da hora.
                if let Some(i) = soando
                    .iter()
                    .position(|v| v.canal == canal && v.nota == nota)
                {
                    let mut voz = soando.remove(i);
                    voz.solta = Some(agora);
                    mortas.push(voz);
                }
            }
            Evento::Toca { canal, nota, forca } => {
                if soando.len() >= MAX_VOZES {
                    continue;
                }
                let canal_idx = canal as usize & 15;
                let (timbre, corte, frequencia) = match canal == CANAL_PERCUSSAO {
                    true => {
                        let (timbre, corte) = percussao(nota);
                        (timbre, corte, 0.0)
                    }
                    false => (
                        timbre(programa[canal_idx]),
                        0.0,
                        // A afinação do MIDI: a nota 69 é o lá de 440 Hz, e cada semitom é a raiz
                        // duodécima de dois.
                        440.0 * 2.0f32.powf((f32::from(nota) - 69.0) / 12.0),
                    ),
                };
                soando.push(Voz {
                    canal,
                    nota,
                    inicio: agora,
                    solta: None,
                    frequencia,
                    timbre,
                    corte,
                    amplitude: f32::from(forca) / 127.0 * volume[canal_idx] * timbre.ganho,
                });
            }
        }
    }
    // Nota que a música nunca soltou: solta no fim, para ela não ficar em sustentação eterna.
    let fim_amostra = amostra_de(fim);
    for mut voz in soando {
        voz.solta = Some(fim_amostra);
        mortas.push(voz);
    }

    for voz in &mortas {
        toca_voz(voz, &mut samples);
    }
    if samples.iter().all(|s| *s == 0.0) {
        return None;
    }
    normaliza(&mut samples);
    Some(crate::wav::Sound {
        rate: RATE,
        channels: 1,
        samples,
    })
}

/// Soma uma voz no buffer, do começo dela até a envoltória zerar.
fn toca_voz(voz: &Voz, samples: &mut [f32]) {
    let taxa = RATE as f32;
    let solta = voz.solta.map(|s| (s.saturating_sub(voz.inicio)) as f32 / taxa);
    let fim = match voz.solta {
        Some(s) => (s + voz.cauda()).min(samples.len()),
        None => samples.len(),
    };
    // Quantos harmônicos cabem abaixo de Nyquist. Um a mais e a nota ganha um assobio que não
    // está na partitura.
    let harmonicos = match voz.frequencia > 0.0 {
        true => (taxa / 2.0 / voz.frequencia) as u32,
        false => 1,
    };
    let mut ruido = 0x1234_5678u32 ^ ((voz.nota as u32) << 16) ^ voz.inicio as u32;
    // O filtro é o que separa bombo de chimbau na percussão.
    let mut anterior = 0.0f32;
    // Filtro RC de um polo: `alpha = fc / (fc + fs/2π)`. Sem filtro, `alpha` é 1 e o ruído passa
    // inteiro — que é o que se quer para nota com altura, onde ele nem é usado.
    let alpha = match voz.corte > 0.0 {
        true => (voz.corte / (voz.corte + taxa / std::f32::consts::TAU)).clamp(0.02, 1.0),
        false => 1.0,
    };
    let mut fase = 0.0f32;
    let passo = voz.frequencia / taxa;
    let Some(trecho) = samples.get_mut(voz.inicio..fim) else {
        return;
    };
    for (i, amostra) in trecho.iter_mut().enumerate() {
        let t = i as f32 / taxa;
        let envoltoria = voz.envoltoria(t, solta);
        if envoltoria <= 0.0 && solta.is_some_and(|s| t > s) {
            break;
        }
        let crua = voz.timbre.forma.amostra(fase, harmonicos.max(1), &mut ruido);
        anterior += alpha * (crua - anterior);
        *amostra += anterior * envoltoria * voz.amplitude;
        fase = (fase + passo).fract();
    }
}

/// Deixa o pico em 0,8, sem cortar.
///
/// Somar dezenas de vozes passa de 1,0 com facilidade, e o que passa de 1,0 **corta** — vira
/// distorção no misturador, que é o defeito mais fácil de confundir com "o sintetizador é ruim".
/// Escalar a música inteira preserva a proporção entre as vozes.
fn normaliza(samples: &mut [f32]) {
    let pico = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if pico <= 0.0 {
        return;
    }
    let fator = 0.8 / pico;
    for amostra in samples.iter_mut() {
        *amostra *= fator;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um SMF de formato 0 com uma trilha só, para os testes não dependerem de arquivo.
    fn smf(divisao: i16, trilha: &[u8]) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend(6u32.to_be_bytes());
        out.extend(0u16.to_be_bytes()); // formato 0
        out.extend(1u16.to_be_bytes()); // uma trilha
        out.extend(divisao.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((trilha.len() as u32).to_be_bytes());
        out.extend(trilha);
        out
    }

    /// Um delta de tempo em tamanho variável, como o arquivo o escreve.
    ///
    /// Escrever 250 num byte só é o erro que dá para cometer aqui: o bit alto ligado quer dizer
    /// "continua", e o leitor engoliria o byte de status seguinte como parte do número.
    fn delta(mut valor: u32) -> Vec<u8> {
        let mut bytes = vec![(valor & 0x7f) as u8];
        valor >>= 7;
        while valor > 0 {
            bytes.push((valor & 0x7f) as u8 | 0x80);
            valor >>= 7;
        }
        bytes.reverse();
        bytes
    }

    /// Uma nota que começa no pulso zero e é solta em `duracao` pulsos.
    fn uma_nota(nota: u8, duracao: u32) -> Vec<u8> {
        let mut out = vec![0x00, 0x90, nota, 100];
        out.extend(delta(duracao));
        out.extend([0x80, nota, 0x40]);
        out.extend([0x00, 0xff, 0x2f, 0x00]);
        out
    }

    #[test]
    fn o_tamanho_variavel_vai_e_volta() {
        for valor in [0, 1, 127, 128, 250, 8192, 0x0fff_ffff] {
            let bytes = delta(valor);
            let mut pos = 0;
            assert_eq!(varlen(&bytes, &mut pos), Some(valor), "{valor}");
            assert_eq!(pos, bytes.len());
        }
    }

    #[test]
    fn o_cabecalho_e_a_trilha_saem_com_pulso_e_evento() {
        let partitura = le(&smf(96, &uma_nota(69, 96))).expect("é um SMF");
        assert_eq!(partitura.divisao, 96);
        assert_eq!(
            partitura.eventos,
            [
                (
                    0,
                    Evento::Toca {
                        canal: 0,
                        nota: 69,
                        forca: 100
                    }
                ),
                (96, Evento::Solta { canal: 0, nota: 69 }),
            ]
        );
    }

    /// 96 pulsos por batida a 120 por minuto é meio segundo por batida.
    #[test]
    fn o_pulso_vira_segundo_pela_divisao_e_pelo_tempo() {
        let partitura = le(&smf(96, &uma_nota(69, 96))).unwrap();
        let eventos = partitura.no_tempo();
        assert_eq!(eventos[0].0, 0.0);
        assert!((eventos[1].0 - 0.5).abs() < 1e-9, "{:?}", eventos[1].0);
    }

    /// O `Set Tempo` no meio vale só do ponto dele para a frente.
    ///
    /// Recalcular a música inteira com o tempo novo é o erro fácil aqui, e ele coloca tudo o que
    /// vem depois de um `accelerando` no lugar errado sem que nada pareça quebrado.
    #[test]
    fn a_mudanca_de_tempo_vale_a_partir_dela() {
        // Uma batida a 120, depois o dobro de velocidade, depois outra batida.
        let trilha = [
            &[0x00u8, 0x90, 60, 100][..],
            &[96, 0x80, 60, 0x40][..],
            // Set Tempo = 250.000 us por batida, ou seja 240 por minuto.
            &[0x00, 0xff, 0x51, 0x03, 0x03, 0xd0, 0x90][..],
            &[0x00, 0x90, 62, 100][..],
            &[96, 0x80, 62, 0x40][..],
            &[0x00, 0xff, 0x2f, 0x00][..],
        ]
        .concat();
        let eventos = le(&smf(96, &trilha)).unwrap().no_tempo();
        let instantes: Vec<f64> = eventos.iter().map(|(t, _)| (t * 1000.0).round()).collect();
        // 0 ms, 500 ms (uma batida a 120), e a segunda nota dura 250 ms.
        assert_eq!(instantes, [0.0, 500.0, 500.0, 750.0]);
    }

    /// O status corrente: o segundo evento vem sem o byte de status.
    #[test]
    fn o_status_corrente_vale_para_o_evento_seguinte() {
        let trilha = [
            0x00, 0x90, 60, 100, // toca 60
            0x10, 62, 100, // e aqui só os dados: é outro Note On
            0x10, 60, 0, // força zero é soltar
            0x00, 0xff, 0x2f, 0x00,
        ];
        let partitura = le(&smf(96, &trilha)).unwrap();
        assert_eq!(
            partitura.eventos,
            [
                (
                    0,
                    Evento::Toca {
                        canal: 0,
                        nota: 60,
                        forca: 100
                    }
                ),
                (
                    16,
                    Evento::Toca {
                        canal: 0,
                        nota: 62,
                        forca: 100
                    }
                ),
                (32, Evento::Solta { canal: 0, nota: 60 }),
            ]
        );
    }

    /// Meta evento e SysEx são pulados pelo tamanho declarado, sem perder o que vem depois.
    #[test]
    fn meta_e_sysex_nao_engolem_a_trilha() {
        let trilha = [
            0x00, 0xff, 0x03, 0x04, b'n', b'o', b'm', b'e', // nome da trilha
            0x00, 0xf0, 0x03, 0x41, 0x42, 0xf7, // SysEx
            0x00, 0x90, 69, 100, //
            0x30, 0x80, 69, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let partitura = le(&smf(96, &trilha)).unwrap();
        assert_eq!(partitura.eventos.len(), 2);
        assert_eq!(
            partitura.eventos[0].1,
            Evento::Toca {
                canal: 0,
                nota: 69,
                forca: 100
            }
        );
    }

    /// **A nota 69 sai em 440 Hz.**
    ///
    /// Medido por correlação com dois senos, o certo e o semitom vizinho: contar cruzamento de
    /// zero depende da forma da onda, e correlação não. Errar a afinação é o defeito que mais
    /// facilmente passa por "o sintetizador é assim" — a música toca inteira, no tom errado.
    #[test]
    fn a_nota_69_soa_em_440_hz() {
        // Programa 64 é palheta, que na nossa tabela é senoide: forma simples para medir.
        let trilha = [
            0x00, 0xc0, 64, // programa
            0x00, 0x90, 69, 127, //
            96, 0x80, 69, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let som = decode(&smf(96, &trilha)).expect("uma nota é música o bastante");
        assert_eq!((som.rate, som.channels), (RATE, 1));

        let correlacao = |hz: f32| -> f32 {
            // Do meio do sustentado, longe do ataque e da soltura.
            let inicio = (0.1 * RATE as f32) as usize;
            let n = (0.2 * RATE as f32) as usize;
            let mut soma = 0.0;
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                soma += som.samples[inicio + i] * (t * hz * std::f32::consts::TAU).sin();
            }
            (soma / n as f32).abs()
        };
        let certo = correlacao(440.0);
        assert!(certo > 0.05, "nota quase muda: {certo}");
        assert!(
            certo > correlacao(466.16) * 3.0,
            "440 Hz ({certo}) não se destacou do semitom vizinho ({})",
            correlacao(466.16)
        );
        assert!(
            certo > correlacao(415.30) * 3.0,
            "440 Hz não se destacou do semitom de baixo"
        );
    }

    /// Uma oitava acima dobra a frequência — é a conta que mais dói errar, porque afina a
    /// música inteira meio tom torta sem nada parecer quebrado.
    #[test]
    fn cada_oitava_dobra_a_frequencia() {
        let de = |nota: u8| 440.0 * 2.0f32.powf((f32::from(nota) - 69.0) / 12.0);
        assert!((de(69) - 440.0).abs() < 0.01);
        assert!((de(81) - 880.0).abs() < 0.01);
        assert!((de(57) - 220.0).abs() < 0.01);
        // Dó central, o valor tabelado.
        assert!((de(60) - 261.63).abs() < 0.01, "{}", de(60));
    }

    /// A percussão sai como ruído: som de verdade, e sem altura definida.
    #[test]
    fn a_percussao_soa_sem_altura() {
        // Canal 9, nota 36: bombo.
        let trilha = [
            0x00, 0x99, 36, 127, //
            96, 0x89, 36, 0x40, //
            0x00, 0xff, 0x2f, 0x00,
        ];
        let som = decode(&smf(96, &trilha)).expect("percussão é música");
        let pico = som.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(pico > 0.5, "percussão quase muda: {pico}");
    }

    /// Nota sem `Note Off` não fica soando para sempre.
    #[test]
    fn nota_que_ninguem_solta_termina_no_fim_da_musica() {
        let trilha = [
            0x00, 0x90, 69, 100, // toca e nunca solta
            96, 0x90, 71, 100, // outra nota, para haver fim
            0x00, 0xff, 0x2f, 0x00,
        ];
        let som = decode(&smf(96, &trilha)).expect("é música");
        // O fim é o último evento mais a sobra da soltura: um segundo e meio cobre tudo.
        assert!(som.frames() < (1.5 * RATE as f32) as usize, "{}", som.frames());
        // E o rabo do arquivo é silêncio, não uma nota presa.
        let ultimas = &som.samples[som.samples.len() - 100..];
        assert!(ultimas.iter().all(|s| s.abs() < 1e-6));
    }

    /// O pico fica em 0,8: acima de 1,0 o misturador cortaria, e corte soa como sintetizador
    /// ruim sem ser.
    #[test]
    fn muitas_vozes_juntas_nao_estouram() {
        let mut trilha = Vec::new();
        for nota in 40..70 {
            trilha.extend([0x00, 0x90, nota, 127]);
        }
        trilha.extend([96, 0xff, 0x2f, 0x00]);
        let som = decode(&smf(96, &trilha)).expect("é música");
        let pico = som.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!((0.79..=0.81).contains(&pico), "pico {pico}");
    }

    #[test]
    fn o_que_nao_e_midi_nao_decodifica() {
        assert!(decode(b"RIFF\x00\x00\x00\x00WAVEfmt ").is_none());
        assert!(decode(&[]).is_none());
        assert!(decode(b"MThd").is_none());
        // Formato 2 é coleção de padrões independentes, não uma música.
        let mut dois = smf(96, &uma_nota(69, 96));
        dois[9] = 2;
        assert!(decode(&dois).is_none());
        // Divisão zero não diz quanto dura um pulso.
        assert!(decode(&smf(0, &uma_nota(69, 96))).is_none());
    }

    /// Trilha truncada no meio de um evento não entra em pânico nem inventa nota.
    #[test]
    fn arquivo_truncado_para_onde_da() {
        let completo = smf(96, &uma_nota(69, 96));
        for corte in 1..completo.len() {
            let _ = decode(&completo[..corte]);
        }
    }

    /// Divisão SMPTE: o tempo vem de quadros por segundo, e o `Set Tempo` não vale.
    #[test]
    fn a_divisao_smpte_conta_por_quadro() {
        // 0xe7 é -25 em complemento de dois: 25 quadros por segundo, 40 pulsos por quadro,
        // ou seja mil pulsos por segundo. É assim que o campo é escrito no arquivo.
        let divisao = i16::from_be_bytes([0xe7, 40]);
        let eventos = le(&smf(divisao, &uma_nota(69, 250))).unwrap().no_tempo();
        assert!((eventos[1].0 - 0.25).abs() < 1e-6, "{:?}", eventos[1].0);
    }
}
