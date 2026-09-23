//! O estado da máquina em seções: registradores, memória e os contadores de alocação.
//!
//! ## O que entra, e por quê
//!
//! **Toda região gravável** do mapa. O que fica de fora é o que o carregador monta uma vez e
//! ninguém altera depois: as vtables e a página nula. As duas são derivadas da imagem, então
//! gravá-las só gastaria bytes — e um estado que carrega vtables velhas por cima das novas seria
//! pior que não carregar nada.
//!
//! ## O corte nas regiões que só crescem
//!
//! O heap (64 MB), a região de objetos (4 MB) e as superfícies (8 MB) nascem **zeradas** e só
//! avançam: `next` é o primeiro endereço nunca usado, e abaixo dele está tudo o que o jogo pediu.
//! Gravar além disso seria escrever dezenas de megabytes de zero em cada save state. O corte é o
//! `next` do alocador de cada região, e é o que faz o tamanho do estado ser proporcional ao que o
//! jogo usou — alguns megabytes, e não oitenta.
//!
//! ## O que ainda não entra
//!
//! A região de objetos entra, mas o **livro** dela não: [`crate::brew::objects::ObjectStore`]
//! guarda a interface de cada objeto num `enum` que ainda não tem codificação estável. Enquanto
//! isso e as tabelas de estado por objeto não entrarem, o `retro_serialize_size` do core continua
//! respondendo zero — ver o plano. Restaurar um estado parcial é exatamente o que o critério
//! proíbe, e a porta é essa.

use super::{
    Callback, CipherState, DecodedImage, FluxoPcm, Machine, MediaState, MemStream, ModeloDeValor,
    OpenFile, Outcome, Peek, PendingBlit, RecorteDeImagem, SoundState, ThreadState, Timer,
    DecoderState, GuestCall, HashState, UnzipState, Widget,
};
use crate::input::Pad;
use crate::machine::{default_colors, ArrayPointer, AES_BLOCK, CLR_COUNT, GraphicsState};
use crate::video::display::{Rect, Rgb};
use crate::video::display::Framebuffer;
use crate::cpu::{CpuBackend, Reg};
use crate::save_state::{Erro, Guardavel, Leitor, Secoes};

/// Os registradores, na ordem em que a seção os grava.
///
/// A ordem é fixa e explícita: gravar "todos os registradores" na ordem do `enum` deixaria o
/// formato refém de alguém reordenar a enumeração.
const REGISTRADORES: [Reg; 16] = [
    Reg::R0,
    Reg::R1,
    Reg::R2,
    Reg::R3,
    Reg::R4,
    Reg::R5,
    Reg::R6,
    Reg::R7,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::Sp,
    Reg::Lr,
    Reg::Pc,
];

/// Quanto de uma região foi usado, a partir do primeiro endereço nunca usado.
///
/// Um `next` fora da região seria defeito do alocador; gravar tudo é a resposta segura.
fn usado(proximo: u32, base: u32, tamanho: usize) -> usize {
    let usado = proximo.saturating_sub(base) as usize;
    if usado > tamanho { tamanho } else { usado }
}

/// O nome da seção de uma região.
fn secao_da_regiao(nome: &str) -> String {
    format!("mem.{nome}")
}

impl<C: CpuBackend> Machine<C> {
    /// Quantos bytes de uma região devem ser gravados.
    ///
    /// O `next` do alocador da região, quando ela tem um; o tamanho inteiro, quando não tem.
    /// Devolve `None` para a região que não deve ser gravada de jeito nenhum.
    fn quanto_gravar(&self, nome: &str, base: u32, tamanho: usize, gravavel: bool) -> Option<usize> {
        if !gravavel {
            return None;
        }
        // **As três regiões que só crescem vão cortadas no primeiro endereço nunca usado**, e o
        // corte é possível porque os livros das três entram no estado: é de lá que sai o tamanho
        // esperado na volta. Gravar as três inteiras seriam 76 MB de zero por save state.
        let usado = match nome {
            "heap" => usado(self.heap.proximo(), base, tamanho),
            "objects" => usado(self.objects.proximo(), base, tamanho),
            "surfaces" => usado(self.superficies.proximo(), base, tamanho),
            _ => tamanho,
        };
        Some(usado)
    }

    /// O quanto de uma região o **estado** diz que era usado.
    ///
    /// Vem do arquivo, e não da máquina de agora: quem carrega um save state carrega o heap que
    /// estava lá. É o que permite conferir o tamanho da seção antes de escrever qualquer byte.
    fn quanto_do_estado(
        leitor: &Leitor<'_>,
        nome: &str,
        base: u32,
        tamanho: usize,
    ) -> Result<usize, Erro> {
        let (prefixo, campo) = match nome {
            "heap" => ("heap", "heap.next"),
            "objects" => ("objects", "objects.next"),
            "surfaces" => ("surfaces", "surfaces.next"),
            _ => return Ok(tamanho),
        };
        let proximo = leitor.u32(campo)?;
        let inicio = match prefixo {
            "objects" => base,
            _ => leitor.u32(&format!("{prefixo}.base"))?,
        };
        Ok(usado(proximo, inicio, tamanho))
    }

    /// Grava o estado em uma seção por região, mais os registradores e os livros.
    pub fn pode_salvar(&mut self) -> Result<(), String> {
        // **Drena primeiro, recusa depois.** Um lote esperando a vez não é desenho pela metade: é
        // trabalho que ia ser feito no quadro seguinte. Recusar o save por causa dele bloquearia o
        // jogador por algo que o motor resolve sozinho. O que sobra depois de drenar é o desenho
        // interrompido de verdade — um `glBegin` sem `glEnd` —, e aí a recusa é honesta.
        self.gl.descarrega_o_desenho();
        if self.gl.desenho_em_curso() {
            return Err(
                "há um desenho começado e não terminado; salve no fim do quadro".to_string(),
            );
        }
        Ok(())
    }

    pub fn grava_estado(&self) -> Vec<u8> {
        let mut secoes = Secoes::nova();
        secoes.poe_u32s(
            "cpu.regs",
            REGISTRADORES.iter().map(|reg| self.cpu.read_reg(*reg)),
        );
        secoes.poe_u32("cpu.thumb", u32::from(self.cpu.em_thumb()));
        // As flags e o relógio virtual. Sem eles o jogo volta com a memória certa e as decisões
        // erradas: a comparação que ele fez antes de salvar vale, e o desvio vem depois.
        secoes.poe_u32("cpu.cpsr", self.cpu.cpsr());
        secoes.poe(
            "cpu.instructions",
            self.cpu.instructions().to_le_bytes().to_vec(),
        );
        for regiao in self.module.mem.regions() {
            let Some(quanto) =
                self.quanto_gravar(regiao.name, regiao.base, regiao.bytes.len(), regiao.writable)
            else {
                continue;
            };
            let mut bytes = vec![0u8; quanto];
            // A memória **viva** está no núcleo, e não no mapa do carregador: é por aqui que se
            // lê o que o jogo escreveu desde o `reset`.
            if self.cpu.read_mem(regiao.base, &mut bytes).is_ok() {
                secoes.poe(&secao_da_regiao(regiao.name), bytes);
            }
        }
        // Os três livros. O das superfícies vai com o nome da região, para a seção e o livro
        // terem o mesmo nome — quem lê o arquivo não precisa de tabela de tradução.
        self.grava_entrada_e_tempo(&mut secoes);
        self.grava_tabelas_numericas(&mut secoes);
        self.grava_fontes_e_arquivos(&mut secoes);
        self.grava_conteudo(&mut secoes);
        self.grava_superficies_e_imagens(&mut secoes);
        self.grava_escalares_e_mapas(&mut secoes);
        self.grava_listas_e_parada(&mut secoes);
        self.grava_resto_das_tabelas(&mut secoes);
        self.grava_o_resto(&mut secoes);
        self.grava_widgets(&mut secoes);
        self.grava_bibliotecas(&mut secoes);
        self.grava_ultimos(&mut secoes);
        self.heap.grava_com_prefixo("heap", &mut secoes);
        self.objects.grava(&mut secoes);
        self.superficies.grava_com_prefixo("surfaces", &mut secoes);
        secoes.fecha()
    }

    pub fn restaura_estado(&mut self, arquivo: &[u8]) -> Result<(), Erro> {
        let leitor = Leitor::abre(arquivo)?;

        let registradores = leitor.u32s("cpu.regs")?;
        if registradores.len() != REGISTRADORES.len() {
            return Err(Erro::Secao {
                nome: "cpu.regs".to_string(),
                motivo: format!(
                    "o estado tem {} registradores e a máquina tem {}",
                    registradores.len(),
                    REGISTRADORES.len()
                ),
            });
        }
        let _thumb = leitor.u32("cpu.thumb")?;
        let cpsr = leitor.u32("cpu.cpsr")?;
        let bytes_do_relogio = leitor.secao("cpu.instructions").ok_or_else(|| Erro::Secao {
            nome: "cpu.instructions".to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })?;
        if bytes_do_relogio.len() != 8 {
            return Err(Erro::Secao {
                nome: "cpu.instructions".to_string(),
                motivo: format!("esperava 8 bytes e tem {}", bytes_do_relogio.len()),
            });
        }
        let relogio = u64::from_le_bytes(bytes_do_relogio.try_into().unwrap());

        // A memória é lida e conferida antes de qualquer escrita.
        let mut regioes: Vec<(u32, Vec<u8>)> = Vec::new();
        for regiao in self.module.mem.regions() {
            let Some(_) = self.quanto_gravar(regiao.name, regiao.base, regiao.bytes.len(), regiao.writable) else {
                continue;
            };
            let secao = secao_da_regiao(regiao.name);
            let bytes = leitor.secao(&secao).ok_or_else(|| Erro::Secao {
                nome: secao.clone(),
                motivo: "a seção não está no arquivo".to_string(),
            })?;
            // **O tamanho esperado sai do próprio estado**, e não da máquina de agora.
            let quanto = Self::quanto_do_estado(&leitor, regiao.name, regiao.base, regiao.bytes.len())?;
            if bytes.len() != quanto {
                return Err(Erro::Secao {
                    nome: secao,
                    motivo: format!(
                        "o estado tem {} bytes desta região e esta máquina espera {quanto} \
                         — é um estado de outro jogo ou de outra versão do motor",
                        bytes.len()
                    ),
                });
            }
            regioes.push((regiao.base, bytes.to_vec()));
        }

        // Daqui para baixo é aplicação: ou tudo, ou nada.
        self.restaura_entrada_e_tempo(&leitor)?;
        self.restaura_tabelas_numericas(&leitor)?;
        self.restaura_fontes_e_arquivos(&leitor)?;
        self.restaura_conteudo(&leitor)?;
        self.restaura_superficies_e_imagens(&leitor)?;
        self.restaura_escalares_e_mapas(&leitor)?;
        self.restaura_listas_e_parada(&leitor)?;
        self.restaura_resto_das_tabelas(&leitor)?;
        self.restaura_o_resto(&leitor)?;
        self.restaura_widgets(&leitor)?;
        self.restaura_bibliotecas(&leitor)?;
        self.restaura_ultimos(&leitor)?;
        self.heap.restaura_com_prefixo("heap", &leitor)?;
        self.objects.restaura(&leitor)?;
        self.superficies
            .restaura_com_prefixo("surfaces", &leitor)?;
        for (base, bytes) in regioes {
            self.cpu.write_mem(base, &bytes).map_err(|erro| Erro::Secao {
                nome: format!("mem.{base:#010x}"),
                motivo: format!("não deu para escrever: {erro}"),
            })?;
        }
        for (reg, valor) in REGISTRADORES.iter().zip(registradores) {
            self.cpu.write_reg(*reg, valor);
        }
        // O `CPSR` fica por último: escrever no `PC` pode mexer nos bits de modo em alguns
        // núcleos, e o valor que veio do estado é o que manda.
        self.cpu.set_cpsr(cpsr);
        self.cpu.set_instructions(relogio);
        Ok(())
    }
}


/// O código do nome de um sinal de entrada.
///
/// Os dois nomes são o conjunto **fechado** que o `IHIDDevice` usa para avisar quem registrou:
/// `RegisterForButtonEvent` e `RegisterForPositionChange`. Guardar o nome como texto deixaria o
/// formato refém de uma string; guardar o código deixa a leitura impossível de errar em silêncio —
/// código desconhecido é recusa, e não um registro perdido.
fn codigo_do_sinal_de_entrada(nome: &str) -> u32 {
    match SINAIS_DE_APARELHO.iter().position(|n| *n == nome) {
        Some(indice) => indice as u32,
        None => {
            // **O gravador grita em vez de escrever o sentinela.** Eu escrevi `u32::MAX` como
            // "não conheço este nome" e o leitor o recusa — o que parecia seguro, e não era: o
            // resultado foi um save state **gravado com sucesso e impossível de carregar**. O
            // erro só apareceu porque o teste de ida e volta existe, e o nome que faltava era o
            // terceiro (`RegisterForConnectEvents`), que eu tinha suposto não existir.
            debug_assert!(false, "o sinal de aparelho \"{nome}\" não está em SINAIS_DE_APARELHO");
            u32::MAX
        }
    }
}

/// Os nomes de sinal de aparelho que o `IHIDDevice` usa para avisar quem registrou.
///
/// **Medidos, e não supostos**: são três. A lista existe para o código ser estável entre versões,
/// e os dois sentidos (`codigo_do_sinal_de_entrada` e `nome_do_sinal_de_entrada`) saem dela — assim
/// acrescentar um nome é mexer num lugar só, e o teste cobra a volta de **todos**.
const SINAIS_DE_APARELHO: [&str; 3] = [
    "RegisterForButtonEvent",
    "RegisterForConnectEvents",
    "RegisterForPositionChange",
];

/// O nome de um sinal de entrada pelo código.
fn nome_do_sinal_de_entrada(codigo: u32) -> Option<&'static str> {
    SINAIS_DE_APARELHO.get(codigo as usize).copied()
}

impl<C: CpuBackend> Machine<C> {
    /// A entrada e o agendamento: filas de tecla e de botão, quem registrou sinal de aparelho, os
    /// temporizadores vencendo e os retornos pendentes.
    ///
    /// São as tabelas que um jogo sente na hora: sem as filas, a tecla que ele ainda não leu
    /// desaparece; sem os timers, o relógio que ele armou para daqui a duzentos milissegundos deixa
    /// de existir; e sem os sinais, o aviso de que o manche mudou não chega a quem o pediu.
    fn grava_entrada_e_tempo(&self, secoes: &mut Secoes) {
        secoes.poe_u32s(
            "entrada.teclas",
            self.teclas.iter().map(|(avk, baixo)| [*avk, u32::from(*baixo)]).flatten(),
        );
        for (porta, fila) in self.pad_events.iter().enumerate() {
            secoes.poe_u32s(
                &format!("entrada.pad_events.{porta}"),
                fila.iter()
                    .map(|(indice, baixo)| [*indice as u32, u32::from(*baixo)])
                    .flatten(),
            );
        }
        secoes.poe_mapa("entrada.portas", self.portas_de_aparelho.iter().map(|(a, p)| (*a, *p as u32)));
        secoes.poe_trios(
            "entrada.sinais",
            self.input_signals.iter().map(|((nome, porta), sinal)| {
                (
                    codigo_do_sinal_de_entrada(nome),
                    *porta as u32,
                    *sinal,
                )
            }),
        );
        secoes.poe_trios("agenda.timers", self.timers.iter().map(|t| {
            (t.deadline_ms, t.callback.function, t.callback.context)
        }));
        secoes.poe_trios("agenda.sinais", self.signals.iter().map(|(id, cb)| {
            (*id, cb.function, cb.context)
        }));
        secoes.poe_u32s(
            "agenda.pendentes",
            self.pending_signals
                .iter()
                .flat_map(|cb| [cb.function, cb.context]),
        );
    }

    /// Lê e confere entrada e agendamento. Nada é aplicado se alguma seção não bater.
    fn restaura_entrada_e_tempo(
        &mut self,
        leitor: &Leitor<'_>,
    ) -> Result<(), Erro> {
        // **Em ordem**: a fila de teclas é uma sequência, e não um mapa.
        let teclas = leitor.pares_em_ordem("entrada.teclas")?;
        let portas = leitor.pares("entrada.portas")?;
        let sinais_crus = leitor.trios("entrada.sinais")?;
        let timers = leitor.trios("agenda.timers")?;
        let signals = leitor.trios("agenda.sinais")?;
        let pendentes = leitor.pares_em_ordem("agenda.pendentes")?;

        let mut filas = Vec::new();
        for porta in 0..self.pad_events.len() {
            filas.push(leitor.pares_em_ordem(&format!("entrada.pad_events.{porta}"))?);
        }

        // O registro do sinal de aparelho é o único com nome: código desconhecido é recusa.
        let mut sinais = std::collections::BTreeMap::new();
        for (codigo, porta, sinal) in sinais_crus {
            let nome = nome_do_sinal_de_entrada(codigo).ok_or_else(|| Erro::Secao {
                nome: "entrada.sinais".to_string(),
                motivo: format!(
                    "o estado registra o sinal de aparelho {codigo}, que este motor não conhece"
                ),
            })?;
            if porta as usize >= self.pad_events.len() {
                return Err(Erro::Secao {
                    nome: "entrada.sinais".to_string(),
                    motivo: format!("a porta {porta} não existe"),
                });
            }
            sinais.insert((nome, porta as usize), sinal);
        }
        let mut portas_de_aparelho = std::collections::HashMap::new();
        for (aparelho, porta) in portas {
            if porta as usize >= self.pad_events.len() {
                return Err(Erro::Secao {
                    nome: "entrada.portas".to_string(),
                    motivo: format!("a porta {porta} não existe"),
                });
            }
            portas_de_aparelho.insert(aparelho, porta as usize);
        }

        // Daqui para baixo é aplicação.
        self.teclas = teclas
            .into_iter()
            .map(|(avk, baixo)| (avk, baixo != 0))
            .collect();
        for (porta, fila) in filas.into_iter().enumerate() {
            self.pad_events[porta] = fila
                .into_iter()
                .map(|(indice, baixo)| (indice as usize, baixo != 0))
                .collect();
        }
        self.portas_de_aparelho = portas_de_aparelho;
        self.input_signals = sinais;
        self.timers = timers
            .into_iter()
            .map(|(deadline_ms, function, context)| Timer {
                deadline_ms,
                callback: Callback { function, context },
            })
            .collect();
        self.signals = signals
            .into_iter()
            .map(|(id, function, context)| (id, Callback { function, context }))
            .collect();
        self.pending_signals = pendentes
            .into_iter()
            .map(|(function, context)| Callback { function, context })
            .collect();
        Ok(())
    }
}


/// As tabelas de estado por objeto cujo conteúdo são **números**.
///
/// A lista é declarada uma vez, aqui, e é ela que grava e que lê — com o mesmo nome de seção dos
/// dois lados. Foi assim que a maior parte do estado entrou: cada tabela é a mesma forma
/// (`HashMap<u32, número>` ou `HashMap<u32, punhado de números>`), e o que muda é só o nome.
///
/// **O que não entra:** as tabelas que guardam pixels ou bytes em quantidade — `bitmaps`,
/// `images`, `gl_last_frame`. Elas são a maior parte do que sobra e precisam de um formato próprio
/// (comprimir, ou apontar para a memória do guest quando o conteúdo já está lá). Enquanto não
/// entrarem, o core continua dizendo que não salva.
impl<C: CpuBackend> Machine<C> {
    /// As tabelas numéricas: `(nome da seção, valores)`.
    /// Grava as tabelas numéricas, uma seção por tabela.
    fn grava_tabelas_numericas(&self, secoes: &mut Secoes) {
        for (nome, valores) in self.tabelas_numericas() {
            secoes.poe_u32s(nome, valores);
        }
    }

    /// Lê e aplica as tabelas numéricas.
    ///
    /// Como no resto: **lê tudo antes de escrever qualquer coisa**. E confere a faixa de cada campo
    /// que é menor que `u32` no motor — um volume que não cabe em `u16` ou um `dono` que não é
    /// booleano é arquivo corrompido, e não valor a acomodar.
    fn restaura_tabelas_numericas(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let mapa = |nome: &str| -> Result<std::collections::HashMap<u32, u32>, Erro> {
            Ok(leitor.pares(nome)?.into_iter().collect())
        };
        let dib_buffers = mapa("tab.dib_buffers")?;
        let dib_capacity = mapa("tab.dib_capacity")?;
        let transformacoes = mapa("tab.transformacoes")?;
        let canvases = mapa("tab.canvases")?;
        let feeds = mapa("tab.feeds")?;
        let image_bitmaps = mapa("tab.image_bitmaps")?;
        let image_info = mapa("tab.image_info")?;

        let mut transparency_crua = mapa("tab.transparency")?;
        for (indice, valor) in transparency_crua.iter() {
            if *valor > u32::from(u16::MAX) {
                return Err(Erro::Secao {
                    nome: "tab.transparency".to_string(),
                    motivo: format!("o objeto {indice:#010x} tem transparência {valor}, que não cabe num u16"),
                });
            }
        }
        let transparency: std::collections::HashMap<u32, u16> = transparency_crua
            .drain()
            .map(|(a, v)| (a, v as u16))
            .collect();

        let mut streams = std::collections::HashMap::new();
        for registro in leitor.registros("tab.streams", 5)? {
            let dono = match registro[4] {
                0 => false,
                1 => true,
                outro => {
                    return Err(Erro::Secao {
                        nome: "tab.streams".to_string(),
                        motivo: format!("o campo `dono` vale {outro}, e é booleano"),
                    })
                }
            };
            streams.insert(
                registro[0],
                MemStream {
                    buffer: registro[1],
                    size: registro[2],
                    position: registro[3],
                    dono,
                },
            );
        }

        let mut sounds = std::collections::HashMap::new();
        for registro in leitor.registros("tab.sounds", 9)? {
            if registro[3] > u32::from(u16::MAX) {
                return Err(Erro::Secao {
                    nome: "tab.sounds".to_string(),
                    motivo: format!("o som {:#010x} tem volume {}", registro[0], registro[3]),
                });
            }
            let mut info = [0u8; 5];
            for (destino, valor) in info.iter_mut().zip(&registro[4..9]) {
                if *valor > 255 {
                    return Err(Erro::Secao {
                        nome: "tab.sounds".to_string(),
                        motivo: format!("o `AEESoundInfo` do som {:#010x} tem byte {valor}", registro[0]),
                    });
                }
                *destino = *valor as u8;
            }
            sounds.insert(
                registro[0],
                SoundState {
                    notify: Callback {
                        function: registro[1],
                        context: registro[2],
                    },
                    info,
                    volume: registro[3] as u16,
                },
            );
        }

        // Aplicação.
        self.dib_buffers = dib_buffers;
        self.dib_capacity = dib_capacity;
        self.transformacoes = transformacoes;
        self.canvases = canvases;
        self.feeds = feeds;
        self.image_bitmaps = image_bitmaps;
        self.image_info = image_info;
        self.transparency = transparency;
        self.streams = streams;
        self.sounds = sounds;
        Ok(())
    }

    fn tabelas_numericas(&self) -> Vec<(&'static str, Vec<u32>)> {
        let mapa = |m: &std::collections::HashMap<u32, u32>| -> Vec<u32> {
            let mut pares: Vec<(u32, u32)> = m.iter().map(|(a, b)| (*a, *b)).collect();
            pares.sort_unstable();
            pares.into_iter().flat_map(|(a, b)| [a, b]).collect()
        };
        vec![
            ("tab.dib_buffers", mapa(&self.dib_buffers)),
            ("tab.dib_capacity", mapa(&self.dib_capacity)),
            ("tab.transformacoes", mapa(&self.transformacoes)),
            ("tab.canvases", mapa(&self.canvases)),
            ("tab.feeds", mapa(&self.feeds)),
            ("tab.image_bitmaps", mapa(&self.image_bitmaps)),
            ("tab.image_info", mapa(&self.image_info)),
            (
                "tab.transparency",
                mapa(
                    &self
                        .transparency
                        .iter()
                        .map(|(a, v)| (*a, u32::from(*v)))
                        .collect(),
                ),
            ),
            // Onde ficava a leitura de cada stream de memória: são quatro números por objeto.
            (
                "tab.streams",
                self.streams
                    .iter()
                    .map(|(id, s)| {
                        vec![
                            *id,
                            s.buffer,
                            s.size,
                            s.position,
                            u32::from(s.dono),
                        ]
                    })
                    .flatten()
                    .collect(),
            ),
            // O estado de cada `ISound`: quem avisa, os cinco bytes do `AEESoundInfo` e o volume.
            (
                "tab.sounds",
                self.sounds
                    .iter()
                    .map(|(id, s)| {
                        let mut registro = vec![
                            *id,
                            s.notify.function,
                            s.notify.context,
                            u32::from(s.volume),
                        ];
                        registro.extend(s.info.iter().map(|b| u32::from(*b)));
                        registro
                    })
                    .flatten()
                    .collect(),
            ),
        ]
    }
}


/// As métricas de fonte e os arquivos abertos.
///
/// Duas tabelas que **não** guardam conteúdo, e é por isso que são baratas:
///
/// - as métricas de fonte são sete números por objeto — vêm do `.bid` e são consultadas a cada
///   `GetFontMetrics`;
/// - um arquivo aberto tem **caminho e deslocamento**, e não bytes: o conteúdo está no disco. Na
///   volta o arquivo é **reaberto pelo caminho** e o deslocamento é devolvido com um `seek`. É
///   honesto e é o que um save state de console faz: se o arquivo mudou no disco entre salvar e
///   carregar, o estado carrega o arquivo de agora — e isso vai dito aqui, para não virar surpresa.
impl<C: CpuBackend> Machine<C> {
    fn grava_fontes_e_arquivos(&self, secoes: &mut Secoes) {
        secoes.poe_registros(
            "tab.fontes",
            self.fontes
                .iter()
                .map(|(id, m)| {
                    vec![
                        *id,
                        u32::from(m.ascent),
                        u32::from(m.descent),
                        u32::from(m.leading),
                        u32::from(m.max_char_width),
                        u32::from(m.height),
                        u32::from(m.bold),
                        u32::from(m.italic),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        // Um objeto por arquivo, com os próprios nomes de seção: o identificador entra no nome, e
        // não numa lista paralela de identificadores que alguém teria de manter em ordem.
        secoes.poe_u32s("arq.ids", self.open_files.keys().copied());
        for (id, arquivo) in &self.open_files {
            secoes.poe_texto(&format!("arq.{id}.guest"), &arquivo.guest_path);
            secoes.poe_texto(&format!("arq.{id}.caminho"), &arquivo.caminho.to_string_lossy());
            // O deslocamento lido **agora**: é ele que diz em que ponto da leitura o jogo estava.
            let posicao = deslocamento(&arquivo.file).unwrap_or(0);
            secoes.poe(&format!("arq.{id}.pos"), posicao.to_le_bytes().to_vec());
        }
    }

    fn restaura_fontes_e_arquivos(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let mut fontes = std::collections::HashMap::new();
        for registro in leitor.registros("tab.fontes", 8)? {
            let menor = |indice: usize| -> Result<u16, Erro> {
                u16::try_from(registro[indice]).map_err(|_| Erro::Secao {
                    nome: "tab.fontes".to_string(),
                    motivo: format!(
                        "a fonte {:#010x} tem o campo {indice} valendo {}, que não cabe num u16",
                        registro[0], registro[indice]
                    ),
                })
            };
            fontes.insert(
                registro[0],
                crate::machine::font::Metricas {
                    ascent: menor(1)?,
                    descent: menor(2)?,
                    leading: menor(3)?,
                    max_char_width: menor(4)?,
                    height: menor(5)?,
                    bold: registro[6] != 0,
                    italic: registro[7] != 0,
                },
            );
        }

        // Os arquivos são **reabertos** antes de qualquer coisa ser aplicada: um caminho que não
        // existe mais é recusa, e não uma tabela de arquivos com uma entrada vazia.
        let ids = leitor.u32s("arq.ids")?;
        let mut abertos = std::collections::HashMap::new();
        for id in ids {
            let guest_path = leitor.texto(&format!("arq.{id}.guest"))?;
            let caminho = std::path::PathBuf::from(leitor.texto(&format!("arq.{id}.caminho"))?);
            let bytes_da_posicao = leitor.secao(&format!("arq.{id}.pos")).ok_or_else(|| Erro::Secao {
                nome: format!("arq.{id}.pos"),
                motivo: "a seção não está no arquivo".to_string(),
            })?;
            if bytes_da_posicao.len() != 8 {
                return Err(Erro::Secao {
                    nome: format!("arq.{id}.pos"),
                    motivo: format!("esperava 8 bytes e tem {}", bytes_da_posicao.len()),
                });
            }
            let posicao = u64::from_le_bytes(bytes_da_posicao.try_into().unwrap());
            let mut file = std::fs::File::open(&caminho).map_err(|erro| Erro::Secao {
                nome: format!("arq.{id}"),
                motivo: format!(
                    "o estado guarda \"{}\" aberto e ele não pôde ser reaberto: {erro}",
                    caminho.display()
                ),
            })?;
            use std::io::Seek as _;
            file.seek(std::io::SeekFrom::Start(posicao)).map_err(|erro| Erro::Secao {
                nome: format!("arq.{id}"),
                motivo: format!("não deu para voltar o arquivo ao byte {posicao}: {erro}"),
            })?;
            abertos.insert(id, OpenFile { file, guest_path, caminho });
        }

        self.fontes = fontes;
        self.open_files = abertos;
        Ok(())
    }
}

/// Em que byte do arquivo a leitura está.
fn deslocamento(file: &std::fs::File) -> std::io::Result<u64> {
    use std::io::Seek as _;
    let mut copia = file;
    copia.stream_position()
}


/// As tabelas que guardam **conteúdo**: preferências, parâmetros de coleção, dados de `IConfig`,
/// o texto decifrado, a resposta da rede e o que o `IWeb` já baixou.
///
/// Todas usam o mesmo ajudante de blocos, e a diferença entre elas é só quantos números formam a
/// chave — um para `HashMap<u32, Vec<u8>>`, dois para `HashMap<(u32, u32), Vec<u8>>`. Guardam o
/// conteúdo porque ele **não está em lugar nenhum** fora do motor: ao contrário dos arquivos
/// abertos, que se reabrem do disco, um `SetPrefs` que o jogo fez já não existe em disco nenhum.
impl<C: CpuBackend> Machine<C> {
    fn grava_conteudo(&self, secoes: &mut Secoes) {
        secoes.poe_blocos(
            "cont.prefs",
            2,
            self.prefs
                .iter()
                .map(|((classe, versao), dados)| {
                    (vec![*classe, u32::from(*versao)], dados.clone())
                })
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "cont.parametros",
            2,
            self.parametros_de_colecao
                .iter()
                .map(|((a, b), dados)| (vec![*a, *b], dados.clone()))
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "cont.sources",
            1,
            self.sources
                .iter()
                .map(|(id, dados)| (vec![*id], dados.clone()))
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "cont.paginas",
            1,
            self.paginas_html
                .iter()
                .map(|(id, dados)| (vec![*id], dados.clone()))
                .collect::<Vec<_>>(),
        );
        // O `IConfig` tem um mapa por objeto: uma chave de dois números por entrada, o objeto e o
        // item.
        secoes.poe_blocos(
            "cont.config",
            2,
            self.config_items
                .iter()
                .flat_map(|(objeto, itens)| {
                    itens
                        .iter()
                        .map(move |(item, dados)| (vec![*objeto, *item], dados.clone()))
                })
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "cont.plaintexts",
            1,
            self.plaintexts
                .iter()
                .enumerate()
                .map(|(ordem, dados)| (vec![ordem as u32], dados.clone()))
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos("cont.resposta", 1, vec![(vec![0u32], self.web_response.clone())]);
    }

    fn restaura_conteudo(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        // A largura da chave é conferida **antes** de qualquer índice: um arquivo que diga "chave
        // de um número" numa tabela de dois faria o índice `chave[1]` entrar em pânico, e pânico
        // no carregamento de um save state derruba o frontend.
        let conferir = |nome: &str, chave: &[u32], esperada: usize| -> Result<(), Erro> {
            if chave.len() != esperada {
                return Err(Erro::Secao {
                    nome: nome.to_string(),
                    motivo: format!(
                        "a chave tem {} número(s) e esta tabela usa {esperada}",
                        chave.len()
                    ),
                });
            }
            Ok(())
        };
        let mut prefs = std::collections::HashMap::new();
        for (chave, dados) in leitor.blocos("cont.prefs")? {
            conferir("cont.prefs", &chave, 2)?;
            prefs.insert((chave[0], chave[1] as u16), dados);
        }
        let mut parametros = std::collections::HashMap::new();
        for (chave, dados) in leitor.blocos("cont.parametros")? {
            conferir("cont.parametros", &chave, 2)?;
            parametros.insert((chave[0], chave[1]), dados);
        }
        let mut sources = std::collections::HashMap::new();
        for (chave, dados) in leitor.blocos("cont.sources")? {
            conferir("cont.sources", &chave, 1)?;
            sources.insert(chave[0], dados);
        }
        let mut paginas_html = std::collections::HashMap::new();
        for (chave, dados) in leitor.blocos("cont.paginas")? {
            conferir("cont.paginas", &chave, 1)?;
            paginas_html.insert(chave[0], dados);
        }
        let mut config_items: std::collections::HashMap<u32, std::collections::HashMap<u32, Vec<u8>>> =
            std::collections::HashMap::new();
        for (chave, dados) in leitor.blocos("cont.config")? {
            conferir("cont.config", &chave, 2)?;
            config_items.entry(chave[0]).or_default().insert(chave[1], dados);
        }
        let mut plaintexts = std::collections::VecDeque::new();
        for (chave, dados) in leitor.blocos("cont.plaintexts")? {
            conferir("cont.plaintexts", &chave, 1)?;
            let ordem = chave[0] as usize;
            // A fila é **ordenada**, e a ordem é o conteúdo: o índice gravado é o lugar dela.
            while plaintexts.len() <= ordem {
                plaintexts.push_back(Vec::new());
            }
            plaintexts[ordem] = dados;
        }
        let resposta = leitor
            .blocos("cont.resposta")?
            .into_iter()
            .map(|(_, dados)| dados)
            .next()
            .ok_or_else(|| Erro::Secao {
                nome: "cont.resposta".to_string(),
                motivo: "a seção não tem a resposta".to_string(),
            })?;

        self.prefs = prefs;
        self.parametros_de_colecao = parametros;
        self.sources = sources;
        self.paginas_html = paginas_html;
        self.config_items = config_items;
        self.plaintexts = plaintexts;
        self.web_response = resposta;
        Ok(())
    }
}



/// Empacota um `Vec<bool>` em bits, um por pixel.
///
/// O `DecodedImage` guarda um booleano por pixel dizendo se ele é desenhado. Um byte por pixel
/// gastaria oito vezes o necessário numa tabela que já guarda dois bytes de cor por pixel.
fn empacota_bits(bits: &[bool]) -> Vec<u8> {
    let mut saida = vec![0u8; bits.len().div_ceil(8)];
    for (indice, bit) in bits.iter().enumerate() {
        if *bit {
            saida[indice / 8] |= 1 << (indice % 8);
        }
    }
    saida
}

/// Desempacota o que [`empacota_bits`] escreveu.
fn desempacota_bits(bytes: &[u8], quantos: usize) -> Vec<bool> {
    (0..quantos)
        .map(|indice| bytes[indice / 8] & (1 << (indice % 8)) != 0)
        .collect()
}

/// As superfícies e as imagens decodificadas: as **duas famílias que guardam pixels**.
///
/// Elas são o que resta de conteúdo no motor depois das tabelas numéricas, e não dá para
/// re-derivá-las de nada: ao contrário de uma superfície do guest, que está na memória que entra no
/// estado, um `Framebuffer` é uma cópia do lado do host, criada por `Framebuffer::new` e preenchida
/// com o que o jogo desenhou. O mesmo vale para uma imagem já decodificada — o arquivo de origem
/// pode ter mudado no disco, e decodificar de novo daria outra coisa.
///
/// Cada superfície ocupa duas seções, com o identificador no nome: uma de números (tamanho, o que
/// já foi escrito e a caixa suja) e uma de pixels. É de propósito — assim os pixels ficam num bloco
/// corrido, e não misturados com números no meio.
impl<C: CpuBackend> Machine<C> {
    fn grava_superficies_e_imagens(&self, secoes: &mut Secoes) {
        secoes.poe_u32s("sup.ids", self.bitmaps.keys().copied());
        for (id, superficie) in &self.bitmaps {
            grava_superficie(secoes, &format!("sup.{id}"), superficie);
        }
        // A tela é uma superfície também, e a única que sempre existe: quem olha o quadro vê esta.
        grava_superficie(secoes, "sup.tela", &self.screen);

        secoes.poe_u32s("img.ids", self.images.keys().copied());
        for (id, imagem) in &self.images {
            secoes.poe_u32s(
                &format!("img.{id}.meta"),
                [
                    imagem.width,
                    imagem.height,
                    u32::from(imagem.frame_width),
                    imagem.pixels.len() as u32,
                    imagem.alfa.len() as u32,
                ],
            );
            secoes.poe(&format!("img.{id}.pixels"), pixels_em_bytes(&imagem.pixels));
            secoes.poe(
                &format!("img.{id}.opaque"),
                empacota_bits(&imagem.opaque),
            );
            secoes.poe(&format!("img.{id}.alfa"), imagem.alfa.clone());
        }
    }

    fn restaura_superficies_e_imagens(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let ids = leitor.u32s("sup.ids")?;
        let mut superficies = std::collections::HashMap::new();
        for id in ids {
            superficies.insert(id, le_superficie(leitor, &format!("sup.{id}"))?);
        }
        let tela = le_superficie(leitor, "sup.tela")?;

        let ids = leitor.u32s("img.ids")?;
        let mut imagens = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("img.{id}.meta"))?;
            if meta.len() != 5 {
                return Err(Erro::Secao {
                    nome: format!("img.{id}.meta"),
                    motivo: format!("esperava 5 números e veio {}", meta.len()),
                });
            }
            let (largura, altura, quadro) = (meta[0], meta[1], meta[2]);
            let quantos = (largura as usize) * (altura as usize);
            if meta[3] as usize != quantos {
                return Err(Erro::Secao {
                    nome: format!("img.{id}.meta"),
                    motivo: format!(
                        "a imagem é {largura}x{altura} e diz ter {} pixels",
                        meta[3]
                    ),
                });
            }
            let pixels = bytes_em_pixels(&secao_exigida(leitor, &format!("img.{id}.pixels"))?);
            if pixels.len() != quantos {
                return Err(Erro::Secao {
                    nome: format!("img.{id}.pixels"),
                    motivo: format!("esperava {quantos} pixels e veio {}", pixels.len()),
                });
            }
            let opacos = secao_exigida(leitor, &format!("img.{id}.opaque"))?;
            if opacos.len() < quantos.div_ceil(8) {
                return Err(Erro::Secao {
                    nome: format!("img.{id}.opaque"),
                    motivo: format!(
                        "esperava {} bytes de bits e veio {}",
                        quantos.div_ceil(8),
                        opacos.len()
                    ),
                });
            }
            let alfa = secao_exigida(leitor, &format!("img.{id}.alfa"))?;
            if alfa.len() != meta[4] as usize {
                return Err(Erro::Secao {
                    nome: format!("img.{id}.alfa"),
                    motivo: format!("esperava {} bytes de alfa e veio {}", meta[4], alfa.len()),
                });
            }
            imagens.insert(
                id,
                std::rc::Rc::new(DecodedImage {
                    width: largura,
                    height: altura,
                    pixels,
                    opaque: desempacota_bits(&opacos, quantos),
                    alfa,
                    frame_width: quadro as u16,
                }),
            );
        }

        self.bitmaps = superficies;
        self.screen = tela;
        self.images = imagens;
        Ok(())
    }
}

/// Grava uma superfície em duas seções: os números e os pixels.
fn grava_superficie(secoes: &mut Secoes, prefixo: &str, superficie: &Framebuffer) {
    let (escritas, serie) = (superficie.escritas(), superficie.serie());
    let sujo = superficie.sujeira().unwrap_or([0; 4]);
    secoes.poe_u32s(
        &format!("{prefixo}.meta"),
        [
            superficie.width(),
            superficie.height(),
            escritas as u32,
            (escritas >> 32) as u32,
            serie as u32,
            (serie >> 32) as u32,
            u32::from(superficie.sujeira().is_some()),
            sujo[0],
            sujo[1],
            sujo[2],
            sujo[3],
        ],
    );
    secoes.poe(&format!("{prefixo}.pixels"), pixels_em_bytes(superficie.pixels()));
}

/// Lê uma superfície gravada por [`grava_superficie`].
fn le_superficie(leitor: &Leitor<'_>, prefixo: &str) -> Result<Framebuffer, Erro> {
    let meta = leitor.u32s(&format!("{prefixo}.meta"))?;
    if meta.len() != 11 {
        return Err(Erro::Secao {
            nome: format!("{prefixo}.meta"),
            motivo: format!("esperava 11 números e veio {}", meta.len()),
        });
    }
    let (largura, altura) = (meta[0], meta[1]);
    let quantos = (largura as usize) * (altura as usize);
    let pixels = bytes_em_pixels(&secao_exigida(leitor, &format!("{prefixo}.pixels"))?);
    if pixels.len() != quantos {
        return Err(Erro::Secao {
            nome: format!("{prefixo}.pixels"),
            motivo: format!("a superfície é {largura}x{altura} e vieram {} pixels", pixels.len()),
        });
    }
    let mut superficie = Framebuffer::new(largura, altura);
    let escritas = u64::from(meta[2]) | (u64::from(meta[3]) << 32);
    let serie = u64::from(meta[4]) | (u64::from(meta[5]) << 32);
    let sujo = match meta[6] {
        0 => None,
        1 => Some([meta[7], meta[8], meta[9], meta[10]]),
        outro => {
            return Err(Erro::Secao {
                nome: format!("{prefixo}.meta"),
                motivo: format!("o campo da caixa suja vale {outro}, e é booleano"),
            })
        }
    };
    superficie.restaura_estado(escritas, serie, sujo, pixels);
    Ok(superficie)
}

/// Uma seção que tem de existir.
fn secao_exigida<'a>(leitor: &'a Leitor<'a>, nome: &str) -> Result<Vec<u8>, Erro> {
    leitor
        .secao(nome)
        .map(|bytes| bytes.to_vec())
        .ok_or_else(|| Erro::Secao {
            nome: nome.to_string(),
            motivo: "a seção não está no arquivo".to_string(),
        })
}

/// Pixels RGB565 como bytes, na ordem de leitura.
fn pixels_em_bytes(pixels: &[u16]) -> Vec<u8> {
    let mut saida = Vec::with_capacity(pixels.len() * 2);
    for pixel in pixels {
        saida.extend_from_slice(&pixel.to_le_bytes());
    }
    saida
}

/// O caminho de volta de [`pixels_em_bytes`]. Tamanho ímpar é recusa.
fn bytes_em_pixels(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|par| u16::from_le_bytes([par[0], par[1]]))
        .collect()
}


/// Os escalares, os conjuntos e os mapas simples.
///
/// É a parte do estado que **não tem forma própria**: um contador de segundos, o estado do gerador
/// aleatório, qual applet está rodando, quais módulos o shell tem instalados. Sozinhos, cada um
/// seria dez linhas de gravação e dez de leitura; juntos, são uma seção de números e um punhado de
/// mapas.
///
/// ## O que fica de fora, e é decisão
///
/// - `stalled`: o motor está **parado no meio de uma sondagem**. Gravar daqui e carregar depois
///   prometeria um estado que nunca existiu — não se sabe o que a sondagem tinha visto. Enquanto
///   ele não entrar, o estado não está completo, e a porta do `retro_serialize_size` continua
///   fechada. É o tipo de buraco que precisa ser dito, e não coberto;
/// - `serial`: o `BufWriter` de um arquivo aberto. Como os arquivos abertos, ele se refaz do disco
///   — mas o gravador de log não faz parte do jogo, e fica fora;
/// - `modulos_instalados` e `enumerations`: listas de **texto**. Precisam do mesmo tratamento de
///   texto que os caminhos, e ficam para a passada seguinte;
/// - o relógio em microssegundos (`clock_us`) **não** entra: ele é o contador de instruções
///   dividido pela taxa, e o contador já entra. Gravar os dois seria guardar a mesma coisa duas
///   vezes, com a chance de voltarem discordando;
/// - `z_wheel` é caminho de conteúdo, e quem o conhece é o core.
impl<C: CpuBackend> Machine<C> {
    fn grava_escalares_e_mapas(&self, secoes: &mut Secoes) {
        let mut numeros: Vec<u32> = vec![
            self.epoch_seconds,
            self.random_state,
            self.nesting,
            self.modo_do_sistema,
            self.device_bitmap,
            self.display_target,
            self.current_applet,
            self.applet_class,
            self.egl_next_handle,
            self.gles_next_name,
            self.egl_swaps,
            self.gl_clears,
            self.unpack_alignment,
            self.updates_na_volta as u32,
            u32::from(self.applet_fechado),
            u32::from(self.wheel_boot_skipped),
            // Booleanos de presença: 1 e 0, para o campo poder ser conferido na volta.
            u32::from(self.pending_launch.is_some()),
            u32::from(self.escritas_do_quadro_gl.is_some()),
            u32::from(self.scale_source.is_some()),
            u32::from(self.current_thread.is_some()),
            u32::from(self.ativacao_pendente.is_some()),
        ];
        numeros.push(self.next_vsync_us as u32);
        numeros.push((self.next_vsync_us >> 32) as u32);
        numeros.push(self.orcamento as u32);
        numeros.push((self.orcamento >> 32) as u32);
        numeros.push(self.proximo_serial as u32);
        numeros.push((self.proximo_serial >> 32) as u32);
        // E os valores dos que existem.
        numeros.push(self.pending_launch.unwrap_or(0));
        numeros.push(self.escritas_do_quadro_gl.unwrap_or(0) as u32);
        numeros.push((self.escritas_do_quadro_gl.unwrap_or(0) >> 32) as u32);
        numeros.push(self.scale_source.unwrap_or((0, 0)).0 as u32);
        numeros.push(self.scale_source.unwrap_or((0, 0)).1 as u32);
        numeros.push(self.current_thread.unwrap_or(0));
        numeros.push(self.ativacao_pendente.unwrap_or((0, 0)).0);
        numeros.push(self.ativacao_pendente.unwrap_or((0, 0)).1);
        secoes.poe_u32s("esc.numeros", numeros);

        let mut conjuntos: Vec<u32> = self.installed_applets.iter().copied().collect();
        conjuntos.sort_unstable();
        secoes.poe_u32s("esc.applets", conjuntos);
        let mut herdados: Vec<u32> = self.dib_herdados.iter().copied().collect();
        herdados.sort_unstable();
        secoes.poe_u32s("esc.dib_herdados", herdados);
        let mut avisando: Vec<u32> = self.widgets_avisando.iter().copied().collect();
        avisando.sort_unstable();
        secoes.poe_u32s("esc.widgets_avisando", avisando);

        secoes.poe_mapa("esc.mif", self.mif_no_guest.iter().map(|(a, b)| (*a, *b)));
        secoes.poe_u32s(
            "esc.ext_modules",
            self.ext_modules.iter().map(|m| m.unwrap_or(u32::MAX)),
        );
        secoes.poe_mapa(
            "esc.rolagem",
            self.rolagem_html.iter().map(|(a, b)| (*a, *b as u32)),
        );
        secoes.poe_mapa(
            "esc.rolagem_maxima",
            self.rolagem_maxima_html.iter().map(|(a, b)| (*a, *b as u32)),
        );
        secoes.poe_trios(
            "esc.image_notify",
            self.image_notify
                .iter()
                .map(|(id, cb)| (*id, cb.function, cb.context)),
        );
        secoes.poe_registros(
            "esc.dib_decodificador",
            self.dib_do_decodificador
                .iter()
                .map(|(id, (a, b))| vec![*id, *a, *b])
                .collect::<Vec<_>>(),
        );
        secoes.poe_registros(
            "esc.dib_publicado",
            self.dib_publicado
                .iter()
                .map(|(id, quando)| vec![*id, *quando as u32, (*quando >> 32) as u32])
                .collect::<Vec<_>>(),
        );
        // As listas de números por objeto: `vetores` e `collections`. Cada uma tem uma chave, um
        // bloco de números e um campo solto — o bloco vai como bytes de u32, e o campo na meta.
        secoes.poe_blocos(
            "esc.vetores",
            1,
            self.vetores
                .iter()
                .map(|(id, (valores, extra))| {
                    let mut bytes = Vec::with_capacity(4 + valores.len() * 4);
                    bytes.extend_from_slice(&(valores.len() as u32).to_le_bytes());
                    for valor in valores {
                        bytes.extend_from_slice(&valor.to_le_bytes());
                    }
                    bytes.extend_from_slice(&extra.to_le_bytes());
                    (vec![*id], bytes)
                })
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "esc.collections",
            1,
            self.collections
                .iter()
                .map(|(id, (valores, extra))| {
                    let mut bytes = Vec::with_capacity(4 + valores.len() * 4);
                    bytes.extend_from_slice(&(valores.len() as u32).to_le_bytes());
                    for valor in valores {
                        bytes.extend_from_slice(&valor.to_le_bytes());
                    }
                    bytes.extend_from_slice(&(*extra as u32).to_le_bytes());
                    (vec![*id], bytes)
                })
                .collect::<Vec<_>>(),
        );
    }


}

/// Quantos números a seção `esc.numeros` tem.
///
/// Está escrito aqui **e** conferido na leitura: um estado gravado com outro número de campos é
/// recusa, e não leitura deslocada. É o que obriga quem acrescentar um campo a mexer na conta — e
/// o teste abaixo cobra que ela esteja certa.
const ESCALARES: usize = 35;

impl<C: CpuBackend> Machine<C> {
    fn restaura_escalares_e_mapas(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let numeros = leitor.u32s("esc.numeros")?;
        if numeros.len() != ESCALARES {
            return Err(Erro::Secao {
                nome: "esc.numeros".to_string(),
                motivo: format!(
                    "o estado tem {} números e esta versão do motor usa {ESCALARES}",
                    numeros.len()
                ),
            });
        }
        let booleano = |indice: usize, campo: &str| -> Result<bool, Erro> {
            match numeros[indice] {
                0 => Ok(false),
                1 => Ok(true),
                outro => Err(Erro::Secao {
                    nome: "esc.numeros".to_string(),
                    motivo: format!("o campo `{campo}` vale {outro}, e é booleano"),
                }),
            }
        };
        let u64_de = |alto: usize| -> u64 {
            u64::from(numeros[alto]) | (u64::from(numeros[alto + 1]) << 32)
        };

        let epoch_seconds = numeros[0];
        let random_state = numeros[1];
        let nesting = numeros[2];
        let modo_do_sistema = numeros[3];
        let device_bitmap = numeros[4];
        let display_target = numeros[5];
        let current_applet = numeros[6];
        let applet_class = numeros[7];
        let egl_next_handle = numeros[8];
        let gles_next_name = numeros[9];
        let egl_swaps = numeros[10];
        let gl_clears = numeros[11];
        let unpack_alignment = numeros[12];
        let updates_na_volta = numeros[13] as usize;
        let applet_fechado = booleano(14, "applet_fechado")?;
        let wheel_boot_skipped = booleano(15, "wheel_boot_skipped")?;
        let tem_pending_launch = booleano(16, "pending_launch")?;
        let tem_escritas = booleano(17, "escritas_do_quadro_gl")?;
        let tem_scale = booleano(18, "scale_source")?;
        let tem_thread = booleano(19, "current_thread")?;
        let tem_ativacao = booleano(20, "ativacao_pendente")?;
        let next_vsync_us = u64_de(21);
        let orcamento = u64_de(23);
        let proximo_serial = u64_de(25);
        // **Os índices do fim são contados a partir do começo da lista**, e não do campo anterior:
        // foi aqui que eu errei a primeira vez, lendo o `pending_launch` na metade alta do
        // `proximo_serial`. O teste pegou porque o valor que voltou era o de outro campo.
        let pending_launch = tem_pending_launch.then_some(numeros[27]);
        let escritas_do_quadro_gl = tem_escritas.then(|| u64_de(28));
        let scale_source = tem_scale.then(|| (numeros[30] as i32, numeros[31] as i32));
        let current_thread = tem_thread.then_some(numeros[32]);
        let ativacao_pendente = tem_ativacao.then(|| (numeros[33], numeros[34]));

        // Os mapas e conjuntos, lidos antes de qualquer aplicação.
        let installed_applets: std::collections::HashSet<u32> =
            leitor.u32s("esc.applets")?.into_iter().collect();
        let dib_herdados: std::collections::HashSet<u32> =
            leitor.u32s("esc.dib_herdados")?.into_iter().collect();
        let widgets_avisando: std::collections::HashSet<u32> =
            leitor.u32s("esc.widgets_avisando")?.into_iter().collect();
        let mif_no_guest: std::collections::HashMap<u32, u32> =
            leitor.pares("esc.mif")?.into_iter().collect();
        let ext_modules: Vec<Option<u32>> = leitor
            .u32s("esc.ext_modules")?
            .into_iter()
            .map(|m| (m != u32::MAX).then_some(m))
            .collect();
        let rolagem_html: std::collections::HashMap<u32, usize> = leitor
            .pares("esc.rolagem")?
            .into_iter()
            .map(|(a, v)| (a, v as usize))
            .collect();
        let rolagem_maxima_html: std::collections::HashMap<u32, usize> = leitor
            .pares("esc.rolagem_maxima")?
            .into_iter()
            .map(|(a, v)| (a, v as usize))
            .collect();
        let image_notify: std::collections::HashMap<u32, Callback> = leitor
            .trios("esc.image_notify")?
            .into_iter()
            .map(|(id, function, context)| (id, Callback { function, context }))
            .collect();
        let mut dib_do_decodificador = std::collections::HashMap::new();
        for registro in leitor.registros("esc.dib_decodificador", 3)? {
            dib_do_decodificador.insert(registro[0], (registro[1], registro[2]));
        }
        let mut dib_publicado = std::collections::HashMap::new();
        for registro in leitor.registros("esc.dib_publicado", 3)? {
            dib_publicado.insert(
                registro[0],
                u64::from(registro[1]) | (u64::from(registro[2]) << 32),
            );
        }
        let mut vetores = std::collections::HashMap::new();
        for (chave, bytes) in leitor.blocos("esc.vetores")? {
            vetores.insert(chave[0], le_lista_com_extra(&bytes, "esc.vetores")?);
        }
        let mut collections = std::collections::HashMap::new();
        for (chave, bytes) in leitor.blocos("esc.collections")? {
            let (valores, extra) = le_lista_com_extra(&bytes, "esc.collections")?;
            collections.insert(chave[0], (valores, extra as usize));
        }

        // Aplicação.
        self.epoch_seconds = epoch_seconds;
        self.random_state = random_state;
        self.nesting = nesting;
        self.modo_do_sistema = modo_do_sistema;
        self.device_bitmap = device_bitmap;
        self.display_target = display_target;
        self.current_applet = current_applet;
        self.applet_class = applet_class;
        self.egl_next_handle = egl_next_handle;
        self.gles_next_name = gles_next_name;
        self.egl_swaps = egl_swaps;
        self.gl_clears = gl_clears;
        self.unpack_alignment = unpack_alignment;
        self.updates_na_volta = updates_na_volta;
        self.applet_fechado = applet_fechado;
        self.wheel_boot_skipped = wheel_boot_skipped;
        self.pending_launch = pending_launch;
        self.escritas_do_quadro_gl = escritas_do_quadro_gl;
        self.scale_source = scale_source;
        self.current_thread = current_thread;
        self.ativacao_pendente = ativacao_pendente;
        self.next_vsync_us = next_vsync_us;
        self.orcamento = orcamento;
        self.proximo_serial = proximo_serial;
        self.installed_applets = installed_applets;
        self.dib_herdados = dib_herdados;
        self.widgets_avisando = widgets_avisando;
        self.mif_no_guest = mif_no_guest;
        self.ext_modules = ext_modules;
        self.rolagem_html = rolagem_html;
        self.rolagem_maxima_html = rolagem_maxima_html;
        self.image_notify = image_notify;
        self.dib_do_decodificador = dib_do_decodificador;
        self.dib_publicado = dib_publicado;
        self.vetores = vetores;
        self.collections = collections;
        Ok(())
    }
}

/// Lê uma lista de números gravada como bytes, com o campo solto no fim.
fn le_lista_com_extra(bytes: &[u8], onde: &str) -> Result<(Vec<u32>, u32), Erro> {
    let malformada = |motivo: String| Erro::Secao {
        nome: onde.to_string(),
        motivo,
    };
    if bytes.len() < 8 {
        return Err(malformada(format!(
            "esperava a contagem e o campo solto, e tem {} bytes",
            bytes.len()
        )));
    }
    let quantos = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() != 4 + quantos * 4 + 4 {
        return Err(malformada(format!(
            "diz ter {quantos} valores ({} bytes com o campo solto) e tem {}",
            4 + quantos * 4 + 4,
            bytes.len()
        )));
    }
    let valores = bytes[4..4 + quantos * 4]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let extra = u32::from_le_bytes([
        bytes[4 + quantos * 4],
        bytes[4 + quantos * 4 + 1],
        bytes[4 + quantos * 4 + 2],
        bytes[4 + quantos * 4 + 3],
    ]);
    Ok((valores, extra))
}


/// Os módulos instalados, as enumerações em curso e o **ponto de parada**.
///
/// ## `stalled`: o buraco declarado, agora coberto
///
/// O motor pode parar no meio de uma sondagem — uma chamada de API sem resposta, um laço que passou
/// do teto. Antes eu tinha escrito que gravar daqui "prometeria um estado que nunca existiu". Estava
/// errado: o estado existe e é bem definido (o guest parado num limite de chamada), e o que faltava
/// era a codificação. Ela entra aqui, com os mesmos campos do `Outcome` — o que muda é que agora a
/// volta sabe recusar um desfecho que esta versão não conhece.
///
/// ## `interned`: o cache que **não** entra
///
/// `interned` mapeia um texto estático (o que `glGetString` devolve) para o endereço onde ele foi
/// copiado no guest. As chaves são `&'static str`, e um save state não traz texto estático de volta:
/// os ponteiros que o jogo guardou continuam válidos porque a **memória** volta, e o cache se refaz
/// sozinho na próxima consulta. O efeito de não gravá-lo é uma cópia a mais por carregamento
/// (algumas dezenas de bytes), e isso vai dito em vez de escondido.
impl<C: CpuBackend> Machine<C> {
    fn grava_listas_e_parada(&self, secoes: &mut Secoes) {
        secoes.poe_u32s(
            "tex.modulos_ids",
            self.modulos_instalados.iter().map(|(id, _)| *id),
        );
        secoes.poe_textos(
            "tex.modulos_nomes",
            self.modulos_instalados
                .iter()
                .map(|(_, nome)| nome.clone())
                .collect::<Vec<_>>(),
        );
        secoes.poe_u32s("tex.enumeracoes_ids", self.enumerations.keys().copied());
        for (id, itens) in &self.enumerations {
            secoes.poe_textos(
                &format!("tex.enumeracao.{id}"),
                itens.iter().cloned().collect::<Vec<_>>(),
            );
        }

        // O ponto de parada, com o rótulo da variante e os campos dela.
        let (rotulo, campos) = descreve(&self.stalled);
        let mut numeros = vec![rotulo];
        numeros.extend(campos);
        secoes.poe_u32s("parada", numeros);
    }

    fn restaura_listas_e_parada(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let ids = leitor.u32s("tex.modulos_ids")?;
        let nomes = leitor.textos("tex.modulos_nomes")?;
        if ids.len() != nomes.len() {
            return Err(Erro::Secao {
                nome: "tex.modulos_nomes".to_string(),
                motivo: format!(
                    "o estado tem {} módulos instalados e {nomes_len} nome(s)",
                    ids.len(),
                    nomes_len = nomes.len()
                ),
            });
        }
        let modulos_instalados: Vec<(u32, String)> =
            ids.into_iter().zip(nomes).collect();

        let ids = leitor.u32s("tex.enumeracoes_ids")?;
        let mut enumerations = std::collections::HashMap::new();
        for id in ids {
            enumerations.insert(
                id,
                leitor
                    .textos(&format!("tex.enumeracao.{id}"))?
                    .into_iter()
                    .collect(),
            );
        }

        let parada = leitor.u32s("parada")?;
        let stalled = monta(&parada)?;

        self.modulos_instalados = modulos_instalados;
        self.enumerations = enumerations;
        self.stalled = stalled;
        // O cache de textos estáticos se refaz sozinho; ver a nota no topo deste bloco.
        self.interned.clear();
        Ok(())
    }
}

/// O rótulo de cada variante de [`Outcome`] no arquivo.
///
/// Número explícito, e não a posição na enumeração: reordenar as variantes no código não pode
/// mudar o significado de um save state já gravado.
fn rotulo_do_desfecho(outcome: &Outcome) -> u32 {
    match outcome {
        Outcome::Returned { .. } => 1,
        Outcome::Unimplemented { .. } => 2,
        Outcome::Fault { .. } => 3,
        Outcome::Exception { .. } => 4,
        Outcome::Budget => 5,
        Outcome::CallLimit { .. } => 6,
    }
}

/// O desfecho como rótulo e campos.
fn descreve(outcome: &Option<Outcome>) -> (u32, Vec<u32>) {
    match outcome {
        None => (0, Vec::new()),
        Some(desfecho) => {
            let rotulo = rotulo_do_desfecho(desfecho);
            let campos = match desfecho {
                Outcome::Returned { code } => vec![*code],
                Outcome::Unimplemented { addr, args, caller } => {
                    vec![*addr, args[0], args[1], args[2], args[3], *caller]
                }
                Outcome::Fault { addr, pc, lr } => vec![*addr, *pc, *lr],
                Outcome::Exception { pc } => vec![*pc],
                Outcome::Budget => Vec::new(),
                Outcome::CallLimit { calls } => {
                    vec![*calls as u32, (*calls >> 32) as u32]
                }
            };
            (rotulo, campos)
        }
    }
}

/// O caminho de volta de [`descreve`]. Rótulo desconhecido é recusa, e campo faltando também.
fn monta(numeros: &[u32]) -> Result<Option<Outcome>, Erro> {
    let erro = |motivo: String| Erro::Secao {
        nome: "parada".to_string(),
        motivo,
    };
    let Some((&rotulo, campos)) = numeros.split_first() else {
        return Err(erro("a seção está vazia".to_string()));
    };
    let precisa = |quantos: usize| -> Result<(), Erro> {
        if campos.len() < quantos {
            return Err(erro(format!(
                "o desfecho {rotulo} precisa de {quantos} campo(s) e veio {}",
                campos.len()
            )));
        }
        Ok(())
    };
    Ok(match rotulo {
        0 => None,
        1 => {
            precisa(1)?;
            Some(Outcome::Returned { code: campos[0] })
        }
        2 => {
            precisa(6)?;
            Some(Outcome::Unimplemented {
                addr: campos[0],
                args: [campos[1], campos[2], campos[3], campos[4]],
                caller: campos[5],
            })
        }
        3 => {
            precisa(3)?;
            Some(Outcome::Fault {
                addr: campos[0],
                pc: campos[1],
                lr: campos[2],
            })
        }
        4 => {
            precisa(1)?;
            Some(Outcome::Exception { pc: campos[0] })
        }
        5 => Some(Outcome::Budget),
        6 => {
            precisa(2)?;
            Some(Outcome::CallLimit {
                calls: u64::from(campos[0]) | (u64::from(campos[1]) << 32),
            })
        }
        outro => {
            return Err(erro(format!(
                "o estado guarda o desfecho {outro}, que este motor não conhece"
            )))
        }
    })
}


/// As tabelas de objeto que restam: cifra, descompressão, espiada, recorte, modelo de valor,
/// fluxo de PCM, entregas pendentes e o estado de mídia.
///
/// Todas usam os mesmos três ajudantes do formato — números, registros de tamanho fixo e blocos de
/// bytes —, e a diferença entre elas é só a forma do que guardam. É a vantagem de ter começado pelo
/// formato: esta é a parte do trabalho em que cada tabela nova custa poucas linhas.
///
/// ## O que **não** entra, e é decisão
///
/// - `CargaDeMidia`: guarda um `Arc<Sound>` — um WAV já decodificado, do tamanho do arquivo. É
///   cache do que está no disco, e derrubá-lo faz o som ser recarregado na próxima vez que o jogo
///   o pedir. O que se perde é o som que **estava tocando** no instante do save: ele volta do
///   começo, não do meio. Vai dito;
/// - `HashState`: um `Md5` no meio de um cálculo. A implementação não expõe o estado interno, e
///   guardar o resultado parcial por fora seria inventar um formato para um algoritmo de terceiro.
///   Um jogo que salve um state exatamente entre dois `Update` de um MD5 perde aquele cálculo — e
///   é o único caso;
/// - `Widget`: o maior dos que faltam, com mapas dentro de mapas e texto. Fica para a passada
///   seguinte, e é o que resta antes de a porta poder abrir.
impl<C: CpuBackend> Machine<C> {
    fn grava_resto_das_tabelas(&self, secoes: &mut Secoes) {
        secoes.poe_u32s("cif.ids", self.ciphers.keys().copied());
        for (id, cifra) in &self.ciphers {
            secoes.poe_u32s(
                &format!("cif.{id}.meta"),
                [u32::from(cifra.key.is_some()), cifra.padding],
            );
            let mut chaves = Vec::with_capacity(2 * AES_BLOCK);
            if let Some(chave) = cifra.key {
                chaves.extend_from_slice(&chave);
            } else {
                chaves.extend(std::iter::repeat_n(0u8, AES_BLOCK));
            }
            chaves.extend_from_slice(&cifra.iv);
            secoes.poe(&format!("cif.{id}.chaves"), chaves);
            secoes.poe(&format!("cif.{id}.pendente"), cifra.pending.clone());
        }

        secoes.poe_u32s("zip.ids", self.unzips.keys().copied());
        for (id, descompressor) in &self.unzips {
            secoes.poe_u32s(
                &format!("zip.{id}.meta"),
                [
                    descompressor.source,
                    descompressor.position as u32,
                    u32::from(descompressor.expanded),
                ],
            );
            secoes.poe(&format!("zip.{id}.saida"), descompressor.output.clone());
        }

        secoes.poe_u32s("peek.ids", self.peeks.keys().copied());
        for (id, espiada) in &self.peeks {
            secoes.poe_u32s(
                &format!("peek.{id}.meta"),
                [espiada.posicao as u32, espiada.buffer],
            );
            secoes.poe(&format!("peek.{id}.bytes"), espiada.bytes.clone());
        }

        secoes.poe_u32s("rec.ids", self.recortes_de_imagem.keys().copied());
        for (id, recorte) in &self.recortes_de_imagem {
            let (tem, largura, altura) = match recorte.tamanho {
                Some((l, a)) => (1u32, l as u32, a as u32),
                None => (0, 0, 0),
            };
            secoes.poe_u32s(
                &format!("rec.{id}.meta"),
                [
                    recorte.x as u32,
                    recorte.y as u32,
                    tem,
                    largura,
                    altura,
                    u32::from(recorte.transparente),
                ],
            );
        }

        secoes.poe_u32s("mod.ids", self.modelos_de_valor.keys().copied());
        for (id, modelo) in &self.modelos_de_valor {
            secoes.poe_u32s(
                &format!("mod.{id}.meta"),
                [modelo.valor, modelo.tamanho],
            );
            secoes.poe_registros(
                &format!("mod.{id}.ouvintes"),
                modelo
                    .ouvintes
                    .iter()
                    .map(|(a, b, c)| vec![*a, *b, *c])
                    .collect::<Vec<_>>(),
            );
        }

        secoes.poe_u32s("pcm.ids", self.fluxos_pcm.keys().copied());
        for (id, fluxo) in &self.fluxos_pcm {
            secoes.poe_u32s(
                &format!("pcm.{id}.meta"),
                [
                    fluxo.fonte,
                    fluxo.taxa,
                    u32::from(fluxo.canais),
                    u32::from(fluxo.bits),
                    u32::from(fluxo.sem_sinal),
                    u32::from(fluxo.tocando),
                    fluxo.inicio_us as u32,
                    (fluxo.inicio_us >> 32) as u32,
                    fluxo.quadros_lidos as u32,
                    (fluxo.quadros_lidos >> 32) as u32,
                ],
            );
        }

        secoes.poe_registros(
            "blit.registros",
            self.pending_blits
                .iter()
                .map(|blit| {
                    vec![
                        blit.image,
                        blit.target,
                        blit.x as u32,
                        blit.y as u32,
                        u32::from(blit.frame.is_some()),
                        blit.frame.unwrap_or(0),
                    ]
                })
                .collect::<Vec<_>>(),
        );

        secoes.poe_u32s("mid.ids", self.media.keys().copied());
        for (id, midia) in &self.media {
            secoes.poe_u32s(
                &format!("mid.{id}.meta"),
                [
                    midia.state,
                    midia.carga as u32,
                    (midia.carga >> 32) as u32,
                    midia.pendente.0,
                    midia.pendente.1,
                    midia.buffer.0,
                    midia.buffer.1,
                    u32::from(midia.tocar_ao_ler),
                    midia.volume,
                    midia.repeat,
                    u32::from(midia.muted),
                    midia.notify.function,
                    midia.notify.context,
                    midia.ends_us as u32,
                    (midia.ends_us >> 32) as u32,
                ],
            );
        }
    }

    fn restaura_resto_das_tabelas(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let ids = leitor.u32s("cif.ids")?;
        let mut ciphers = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("cif.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Erro::Secao {
                    nome: format!("cif.{id}.meta"),
                    motivo: format!("esperava 2 números e veio {}", meta.len()),
                });
            }
            let chaves = secao_exigida(leitor, &format!("cif.{id}.chaves"))?;
            if chaves.len() != 2 * AES_BLOCK {
                return Err(Erro::Secao {
                    nome: format!("cif.{id}.chaves"),
                    motivo: format!(
                        "esperava {} bytes e veio {}",
                        2 * AES_BLOCK,
                        chaves.len()
                    ),
                });
            }
            let mut iv = [0u8; AES_BLOCK];
            iv.copy_from_slice(&chaves[AES_BLOCK..]);
            let key = match meta[0] {
                0 => None,
                1 => {
                    let mut chave = [0u8; AES_BLOCK];
                    chave.copy_from_slice(&chaves[..AES_BLOCK]);
                    Some(chave)
                }
                outro => {
                    return Err(Erro::Secao {
                        nome: format!("cif.{id}.meta"),
                        motivo: format!("o campo `key` vale {outro}, e é booleano"),
                    })
                }
            };
            ciphers.insert(
                id,
                CipherState {
                    key,
                    iv,
                    padding: meta[1],
                    pending: secao_exigida(leitor, &format!("cif.{id}.pendente"))?,
                },
            );
        }

        let ids = leitor.u32s("zip.ids")?;
        let mut unzips = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("zip.{id}.meta"))?;
            if meta.len() != 3 {
                return Err(Erro::Secao {
                    nome: format!("zip.{id}.meta"),
                    motivo: format!("esperava 3 números e veio {}", meta.len()),
                });
            }
            unzips.insert(
                id,
                UnzipState {
                    source: meta[0],
                    output: secao_exigida(leitor, &format!("zip.{id}.saida"))?,
                    position: meta[1] as usize,
                    expanded: meta[2] != 0,
                },
            );
        }

        let ids = leitor.u32s("peek.ids")?;
        let mut peeks = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("peek.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Erro::Secao {
                    nome: format!("peek.{id}.meta"),
                    motivo: format!("esperava 2 números e veio {}", meta.len()),
                });
            }
            peeks.insert(
                id,
                Peek {
                    bytes: secao_exigida(leitor, &format!("peek.{id}.bytes"))?,
                    posicao: meta[0] as usize,
                    buffer: meta[1],
                },
            );
        }

        let ids = leitor.u32s("rec.ids")?;
        let mut recortes_de_imagem = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("rec.{id}.meta"))?;
            if meta.len() != 6 {
                return Err(Erro::Secao {
                    nome: format!("rec.{id}.meta"),
                    motivo: format!("esperava 6 números e veio {}", meta.len()),
                });
            }
            let tamanho = match meta[2] {
                0 => None,
                1 => Some((meta[3] as i32, meta[4] as i32)),
                outro => {
                    return Err(Erro::Secao {
                        nome: format!("rec.{id}.meta"),
                        motivo: format!("o campo `tamanho` vale {outro}, e é booleano"),
                    })
                }
            };
            recortes_de_imagem.insert(
                id,
                RecorteDeImagem {
                    x: meta[0] as i32,
                    y: meta[1] as i32,
                    tamanho,
                    transparente: meta[5] != 0,
                },
            );
        }

        let ids = leitor.u32s("mod.ids")?;
        let mut modelos_de_valor = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("mod.{id}.meta"))?;
            if meta.len() != 2 {
                return Err(Erro::Secao {
                    nome: format!("mod.{id}.meta"),
                    motivo: format!("esperava 2 números e veio {}", meta.len()),
                });
            }
            let ouvintes = leitor
                .registros(&format!("mod.{id}.ouvintes"), 3)?
                .into_iter()
                .map(|r| (r[0], r[1], r[2]))
                .collect();
            modelos_de_valor.insert(
                id,
                ModeloDeValor {
                    valor: meta[0],
                    tamanho: meta[1],
                    ouvintes,
                },
            );
        }

        let ids = leitor.u32s("pcm.ids")?;
        let mut fluxos_pcm = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("pcm.{id}.meta"))?;
            if meta.len() != 10 {
                return Err(Erro::Secao {
                    nome: format!("pcm.{id}.meta"),
                    motivo: format!("esperava 10 números e veio {}", meta.len()),
                });
            }
            let menor = |indice: usize, campo: &str| -> Result<u16, Erro> {
                u16::try_from(meta[indice]).map_err(|_| Erro::Secao {
                    nome: format!("pcm.{id}.meta"),
                    motivo: format!("o campo `{campo}` vale {}, que não cabe num u16", meta[indice]),
                })
            };
            fluxos_pcm.insert(
                id,
                FluxoPcm {
                    fonte: meta[0],
                    taxa: meta[1],
                    canais: menor(2, "canais")?,
                    bits: menor(3, "bits")?,
                    sem_sinal: meta[4] != 0,
                    inicio_us: u64::from(meta[6]) | (u64::from(meta[7]) << 32),
                    quadros_lidos: u64::from(meta[8]) | (u64::from(meta[9]) << 32),
                    tocando: meta[5] != 0,
                },
            );
        }

        let mut pending_blits = Vec::new();
        for registro in leitor.registros("blit.registros", 6)? {
            let frame = match registro[4] {
                0 => None,
                1 => Some(registro[5]),
                outro => {
                    return Err(Erro::Secao {
                        nome: "blit.registros".to_string(),
                        motivo: format!("o campo `frame` vale {outro}, e é booleano"),
                    })
                }
            };
            pending_blits.push(PendingBlit {
                image: registro[0],
                target: registro[1],
                x: registro[2] as i32,
                y: registro[3] as i32,
                frame,
            });
        }

        let ids = leitor.u32s("mid.ids")?;
        let mut media = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("mid.{id}.meta"))?;
            if meta.len() != 15 {
                return Err(Erro::Secao {
                    nome: format!("mid.{id}.meta"),
                    motivo: format!("esperava 15 números e veio {}", meta.len()),
                });
            }
            media.insert(
                id,
                MediaState {
                    state: meta[0],
                    carga: u64::from(meta[1]) | (u64::from(meta[2]) << 32),
                    pendente: (meta[3], meta[4]),
                    buffer: (meta[5], meta[6]),
                    tocar_ao_ler: meta[7] != 0,
                    volume: meta[8],
                    repeat: meta[9],
                    muted: meta[10] != 0,
                    notify: Callback {
                        function: meta[11],
                        context: meta[12],
                    },
                    ends_us: u64::from(meta[13]) | (u64::from(meta[14]) << 32),
                },
            );
        }

        self.ciphers = ciphers;
        self.unzips = unzips;
        self.peeks = peeks;
        self.recortes_de_imagem = recortes_de_imagem;
        self.modelos_de_valor = modelos_de_valor;
        self.fluxos_pcm = fluxos_pcm;
        self.pending_blits = pending_blits;
        self.media = media;
        Ok(())
    }
}


/// O que resta sem forma própria: escalares, buffers, o estado de `IGraphics` e `IGL`, os teclados
/// e as threads.
///
/// ## O que continua de fora, e por que
///
/// - **`widgets`**: o maior. Cada widget tem mapas de filhos, propriedades e modelos, mais texto.
///   Precisa de um esquema de chave composta que ainda não existe no formato;
/// - **`gl`**: o objeto do rasterizador, com a máquina de estados de GL do guest — matrizes, cor
///   corrente, pilhas. É estado de verdade (**o jogo pode estar no meio de um `glBegin`**), e por
///   isso não entra de qualquer jeito: precisa da própria codificação, com o mesmo cuidado das
///   superfícies;
/// - **`decoders`** e **`databases`**: decodificador de imagem no meio de um fluxo e banco SQL
///   aberto. Os dois são estados de biblioteca, e cada um pede a mesma decisão que o `Md5` — expor
///   o de dentro, ou aceitar perder o que estava em curso;
/// - **`vfs`** e **`resources`**: são **cache do que está em disco** (o sistema de arquivos do
///   aparelho e os recursos do pacote). Derrubá-los faz recarregar, e é o certo: gravar seria
///   duplicar o pacote dentro do save state;
/// - **`audio`**: o `Mixer` do host. O que o jogo pediu está nos `sounds`, e o mixer se refaz;
/// - **`ignored_gl`**, **`api_time`**, **`profiling_api`**, **`tracing`**, **`fault_*`**: instrumento
///   e diagnóstico, pela mesma razão de sempre — salvar faria dois save states legitimamente
///   diferentes.
impl<C: CpuBackend> Machine<C> {
    /// Os escalares e as listas pequenas.
    fn numeros_do_resto(&self) -> Vec<u32> {
        let mut n: Vec<u32> = vec![
            self.file_error,
            self.surface_manip,
            self.imageon_ext,
            self.gles11_ext,
            self.gles10_ext,
            self.egl_get_power_level,
            self.egl_oes_swap_interval,
            self.egl_get_color_buffer,
            self.gles11_ext_pak,
            u32::from(self.boomerang_sequencia),
            self.spin_polls,
            self.enumeracao_de_applets as u32,
            u32::from(self.bridge),
            self.buffer_de_fluxo,
            self.bloco_de_aviso_de_midia,
            u32::from(self.despejou),
            self.formulario_pintado,
            self.egl_error,
            u32::from(self.egl_viewport_inicial),
            self.gles_object,
            self.egl_surface,
            self.egl_context,
            self.gl_array_buffer,
            self.gl_element_buffer,
            self.ultimo_desenho_us as u32,
            (self.ultimo_desenho_us >> 32) as u32,
            self.ultimo_relatorio_boomerang_us as u32,
            (self.ultimo_relatorio_boomerang_us >> 32) as u32,
            self.ultimo_pacote_boomerang_us as u32,
            (self.ultimo_pacote_boomerang_us >> 32) as u32,
            self.calibracoes.0,
            self.calibracoes.1,
            u32::from(self.pending_end.is_some()),
            self.pending_end.unwrap_or(0),
            u32::from(self.pending_response.is_some()),
            self.pending_response.unwrap_or((0, 0, 0)).0,
            self.pending_response.unwrap_or((0, 0, 0)).1,
            self.pending_response.unwrap_or((0, 0, 0)).2,
            u32::from(self.egl_color_dimensions.is_some()),
            self.egl_color_dimensions.unwrap_or((0, 0)).0 as u32,
            self.egl_color_dimensions.unwrap_or((0, 0)).1 as u32,
            self.egl_color_buffer.0,
            self.egl_color_buffer.1 as u32,
            u32::from(self.clip.is_some()),
            u32::from(self.network),
            u32::from(self.network_to.is_some()),
        ];
        // Os retângulos assinados e os `Rgb` de IGraphics.
        if let Some(recorte) = self.clip {
            n.extend([
                recorte.x as u32,
                recorte.y as u32,
                recorte.width as u32,
                recorte.height as u32,
            ]);
        } else {
            n.extend([0, 0, 0, 0]);
        }
        n.extend([
            u32::from(self.graphics.stroke.r),
            u32::from(self.graphics.stroke.g),
            u32::from(self.graphics.stroke.b),
            u32::from(self.graphics.fill.r),
            u32::from(self.graphics.fill.g),
            u32::from(self.graphics.fill.b),
            u32::from(self.graphics.background.r),
            u32::from(self.graphics.background.g),
            u32::from(self.graphics.background.b),
            u32::from(self.graphics.fill_mode),
            u32::from(self.graphics.point_size),
            self.graphics.origin.0 as u32,
            self.graphics.origin.1 as u32,
        ]);
        // A paleta de `IGraphics`: dezessete `Rgb`, três bytes cada.
        for cor in &self.colors {
            n.extend([u32::from(cor.r), u32::from(cor.g), u32::from(cor.b)]);
        }
        // Os dois controles, cada um com os botões e os quatro eixos.
        for pad in &self.pads {
            n.push(pad.buttons);
            n.extend(pad.axes.iter().map(|eixo| *eixo as u32));
        }
        // O movimento do Boomerang, em bits: são `f32`.
        for aceleracao in &self.movimento {
            for componente in aceleracao {
                n.push(componente.to_bits());
            }
        }
        // Os cinco vetores de cliente de GL: seis números cada.
        for ponteiro in [
            &self.gl_vertices,
            &self.gl_colors,
            &self.gl_texcoords,
            &self.gl_texcoords1,
            &self.gl_normals,
        ] {
            n.extend([
                ponteiro.size,
                ponteiro.kind,
                ponteiro.stride,
                ponteiro.address,
                u32::from(ponteiro.enabled),
                ponteiro.buffer,
            ]);
        }
        for componente in self.gl_normal_atual {
            n.push(componente.to_bits());
        }
        n
    }

    fn grava_o_resto(&self, secoes: &mut Secoes) {
        secoes.poe_u32s("resto.numeros", self.numeros_do_resto());
        secoes.poe_texto("resto.network_to", self.network_to.as_deref().unwrap_or(""));
        let mut teclas: Vec<u32> = self.teclas_da_rolagem.iter().copied().collect();
        teclas.sort_unstable();
        secoes.poe_u32s("resto.teclas_rolagem", teclas);
        let mut lidos: Vec<u32> = self.recursos_lidos.iter().map(|r| u32::from(*r)).collect();
        lidos.sort_unstable();
        secoes.poe_u32s("resto.recursos_lidos", lidos);
        secoes.poe_registros(
            "resto.avisos_de_midia",
            self.avisos_de_midia
                .iter()
                .map(|(a, b, c, cb)| vec![*a, *b, *c, cb.function, cb.context])
                .collect::<Vec<_>>(),
        );
        secoes.poe_blocos(
            "resto.gl_buffers",
            1,
            self.gl_buffers
                .iter()
                .map(|(nome, bytes)| (vec![*nome], bytes.clone()))
                .collect::<Vec<_>>(),
        );
        secoes.poe("resto.egl_bytes", self.egl_color_bytes.clone());
        secoes.poe("resto.egl_readback", self.egl_color_readback.clone());

        // As threads: os catorze registradores de contexto, os sinalizadores e quem espera por ela.
        secoes.poe_u32s("th.ids", self.threads.keys().copied());
        for (id, thread) in &self.threads {
            let mut numeros = vec![
                thread.stack,
                thread.resume_cb,
                thread.resume_pc,
                u32::from(thread.started),
                u32::from(thread.suspended),
                u32::from(thread.finished),
                thread.exit_code,
            ];
            numeros.extend(thread.context);
            secoes.poe_u32s(&format!("th.{id}.meta"), numeros);
            secoes.poe_registros(
                &format!("th.{id}.joiners"),
                thread
                    .joiners
                    .iter()
                    .map(|(cb, valor)| vec![cb.function, cb.context, *valor])
                    .collect::<Vec<_>>(),
            );
        }
        secoes.poe_mapa("th.resume", self.resume_callbacks.iter().map(|(a, b)| (*a, *b)));
        secoes.poe_u32s("th.pendentes", self.pending_threads.iter().copied());

        // Os quadros do `Update`: são superfícies, e vão pela mesma rotina delas.
        secoes.poe_u32s("upd.quantos", [self.quadros_do_update.len() as u32]);
        for (indice, quadro) in self.quadros_do_update.iter().enumerate() {
            grava_superficie(secoes, &format!("upd.{indice}"), quadro);
        }
    }

    /// Lê e aplica o resto: escalares, buffers, `IGraphics`, `IGL`, teclados e threads.
    fn restaura_o_resto(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let n = leitor.u32s("resto.numeros")?;
        if n.len() != REST0 {
            return Err(Erro::Secao {
                nome: "resto.numeros".to_string(),
                motivo: format!(
                    "o estado tem {} números e esta versão do motor usa {REST0}",
                    n.len()
                ),
            });
        }
        let booleano = |indice: usize, campo: &str| -> Result<bool, Erro> {
            match n[indice] {
                0 => Ok(false),
                1 => Ok(true),
                outro => Err(Erro::Secao {
                    nome: "resto.numeros".to_string(),
                    motivo: format!("o campo `{campo}` vale {outro}, e é booleano"),
                }),
            }
        };
        let u64_de = |indice: usize| u64::from(n[indice]) | (u64::from(n[indice + 1]) << 32);

        // Os 49 primeiros; os demais vêm depois, em blocos de tamanho fixo.
        let file_error = n[0];
        let surface_manip = n[1];
        let imageon_ext = n[2];
        let gles11_ext = n[3];
        let gles10_ext = n[4];
        let egl_get_power_level = n[5];
        let egl_oes_swap_interval = n[6];
        let egl_get_color_buffer = n[7];
        let gles11_ext_pak = n[8];
        let boomerang_sequencia = n[9] as u8;
        let spin_polls = n[10];
        let enumeracao_de_applets = n[11] as usize;
        let bridge = booleano(12, "bridge")?;
        let buffer_de_fluxo = n[13];
        let bloco_de_aviso_de_midia = n[14];
        let despejou = booleano(15, "despejou")?;
        let formulario_pintado = n[16];
        let egl_error = n[17];
        let egl_viewport_inicial = booleano(18, "egl_viewport_inicial")?;
        let gles_object = n[19];
        let egl_surface = n[20];
        let egl_context = n[21];
        let gl_array_buffer = n[22];
        let gl_element_buffer = n[23];
        let ultimo_desenho_us = u64_de(24);
        let ultimo_relatorio_boomerang_us = u64_de(26);
        let ultimo_pacote_boomerang_us = u64_de(28);
        let calibracoes = (n[30], n[31]);
        let pending_end = booleano(32, "pending_end")?.then_some(n[33]);
        let pending_response = booleano(34, "pending_response")?.then(|| (n[35], n[36], n[37]));
        let egl_color_dimensions =
            booleano(38, "egl_color_dimensions")?.then(|| (n[39] as usize, n[40] as usize));
        let egl_color_buffer = (n[41], n[42] as usize);
        // A ordem aqui **é** a do escritor, e não a que parece mais natural: os dois campos de
        // rede vêm antes do retângulo. Quando eu escrevi o leitor na ordem "bonita", os valores
        // saíram trocados — o `network` voltou com a largura do retângulo. O teste pegou.
        let clip = booleano(43, "clip")?.then(|| Rect {
            x: n[46] as i16,
            y: n[47] as i16,
            width: n[48] as i16,
            height: n[49] as i16,
        });
        let network = booleano(44, "network")?;
        let tem_network_to = booleano(45, "network_to")?;
        let cor = |base: usize| Rgb {
            r: n[base] as u8,
            g: n[base + 1] as u8,
            b: n[base + 2] as u8,
        };
        let graphics = GraphicsState {
            stroke: cor(50),
            fill: cor(53),
            background: cor(56),
            fill_mode: n[59] != 0,
            point_size: n[60] as u8,
            origin: (n[61] as i32, n[62] as i32),
        };
        let mut colors = default_colors();
        for (indice, destino) in colors.iter_mut().enumerate() {
            let base = 63 + indice * 3;
            *destino = Rgb {
                r: n[base] as u8,
                g: n[base + 1] as u8,
                b: n[base + 2] as u8,
            };
        }
        let base_dos_pads = 63 + CLR_COUNT * 3;
        let mut pads = [Pad::default(); crate::input::PORTAS];
        for (indice, destino) in pads.iter_mut().enumerate() {
            let base = base_dos_pads + indice * 5;
            destino.buttons = n[base];
            for (eixo, valor) in destino.axes.iter_mut().enumerate() {
                *valor = n[base + 1 + eixo] as i32;
            }
        }
        let base_do_movimento = base_dos_pads + crate::input::PORTAS * 5;
        let mut movimento = [[0.0f32; 3]; crate::input::PORTAS];
        for (indice, aceleracao) in movimento.iter_mut().enumerate() {
            for (componente, valor) in aceleracao.iter_mut().enumerate() {
                *valor = f32::from_bits(n[base_do_movimento + indice * 3 + componente]);
            }
        }
        let base_dos_ponteiros = base_do_movimento + crate::input::PORTAS * 3;
        let mut ponteiros = [ArrayPointer::default(); 5];
        for (indice, destino) in ponteiros.iter_mut().enumerate() {
            let base = base_dos_ponteiros + indice * 6;
            *destino = ArrayPointer {
                size: n[base],
                kind: n[base + 1],
                stride: n[base + 2],
                address: n[base + 3],
                enabled: n[base + 4] != 0,
                buffer: n[base + 5],
            };
        }
        let base_do_normal = base_dos_ponteiros + 30;
        let gl_normal_atual = [
            f32::from_bits(n[base_do_normal]),
            f32::from_bits(n[base_do_normal + 1]),
            f32::from_bits(n[base_do_normal + 2]),
        ];

        // Listas e blocos, lidos antes de aplicar.
        let network_to = {
            let texto = leitor.texto("resto.network_to")?;
            tem_network_to.then_some(texto)
        };
        let teclas_da_rolagem: std::collections::HashSet<u32> =
            leitor.u32s("resto.teclas_rolagem")?.into_iter().collect();
        let recursos_lidos: std::collections::BTreeSet<u16> = leitor
            .u32s("resto.recursos_lidos")?
            .into_iter()
            .map(|r| r as u16)
            .collect();
        let avisos_de_midia: Vec<(u32, u32, u32, Callback)> = leitor
            .registros("resto.avisos_de_midia", 5)?
            .into_iter()
            .map(|r| {
                (
                    r[0],
                    r[1],
                    r[2],
                    Callback {
                        function: r[3],
                        context: r[4],
                    },
                )
            })
            .collect();
        let gl_buffers: std::collections::HashMap<u32, Vec<u8>> =
            leitor.blocos("resto.gl_buffers")?.into_iter().map(|(c, b)| (c[0], b)).collect();
        let egl_color_bytes = secao_exigida(leitor, "resto.egl_bytes")?;
        let egl_color_readback = secao_exigida(leitor, "resto.egl_readback")?;

        let ids = leitor.u32s("th.ids")?;
        let mut threads = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("th.{id}.meta"))?;
            if meta.len() != 21 {
                return Err(Erro::Secao {
                    nome: format!("th.{id}.meta"),
                    motivo: format!("esperava 21 números e veio {}", meta.len()),
                });
            }
            let mut context = [0u32; 14];
            context.copy_from_slice(&meta[7..21]);
            let joiners = leitor
                .registros(&format!("th.{id}.joiners"), 3)?
                .into_iter()
                .map(|r| {
                    (
                        Callback {
                            function: r[0],
                            context: r[1],
                        },
                        r[2],
                    )
                })
                .collect();
            threads.insert(
                id,
                ThreadState {
                    stack: meta[0],
                    resume_cb: meta[1],
                    resume_pc: meta[2],
                    started: meta[3] != 0,
                    suspended: meta[4] != 0,
                    finished: meta[5] != 0,
                    exit_code: meta[6],
                    context,
                    joiners,
                },
            );
        }
        let resume_callbacks: std::collections::HashMap<u32, u32> =
            leitor.pares("th.resume")?.into_iter().collect();
        let pending_threads = leitor.u32s("th.pendentes")?;

        let quantos_quadros = leitor.u32s("upd.quantos")?;
        if quantos_quadros.len() != 1 {
            return Err(Erro::Secao {
                nome: "upd.quantos".to_string(),
                motivo: format!("esperava 1 número e veio {}", quantos_quadros.len()),
            });
        }
        let mut quadros_do_update = std::collections::VecDeque::new();
        for indice in 0..quantos_quadros[0] as usize {
            quadros_do_update.push_back(le_superficie(leitor, &format!("upd.{indice}"))?);
        }

        // Aplicação.
        self.file_error = file_error;
        self.surface_manip = surface_manip;
        self.imageon_ext = imageon_ext;
        self.gles11_ext = gles11_ext;
        self.gles10_ext = gles10_ext;
        self.egl_get_power_level = egl_get_power_level;
        self.egl_oes_swap_interval = egl_oes_swap_interval;
        self.egl_get_color_buffer = egl_get_color_buffer;
        self.gles11_ext_pak = gles11_ext_pak;
        self.boomerang_sequencia = boomerang_sequencia;
        self.spin_polls = spin_polls;
        self.enumeracao_de_applets = enumeracao_de_applets;
        self.bridge = bridge;
        self.buffer_de_fluxo = buffer_de_fluxo;
        self.bloco_de_aviso_de_midia = bloco_de_aviso_de_midia;
        self.despejou = despejou;
        self.formulario_pintado = formulario_pintado;
        self.egl_error = egl_error;
        self.egl_viewport_inicial = egl_viewport_inicial;
        self.gles_object = gles_object;
        self.egl_surface = egl_surface;
        self.egl_context = egl_context;
        self.gl_array_buffer = gl_array_buffer;
        self.gl_element_buffer = gl_element_buffer;
        self.ultimo_desenho_us = ultimo_desenho_us;
        self.ultimo_relatorio_boomerang_us = ultimo_relatorio_boomerang_us;
        self.ultimo_pacote_boomerang_us = ultimo_pacote_boomerang_us;
        self.calibracoes = calibracoes;
        self.pending_end = pending_end;
        self.pending_response = pending_response;
        self.egl_color_dimensions = egl_color_dimensions;
        self.egl_color_buffer = egl_color_buffer;
        self.clip = clip;
        self.network = network;
        self.network_to = network_to;
        self.graphics = graphics;
        self.colors = colors;
        self.pads = pads;
        self.movimento = movimento;
        self.gl_vertices = ponteiros[0];
        self.gl_colors = ponteiros[1];
        self.gl_texcoords = ponteiros[2];
        self.gl_texcoords1 = ponteiros[3];
        self.gl_normals = ponteiros[4];
        self.gl_normal_atual = gl_normal_atual;
        self.teclas_da_rolagem = teclas_da_rolagem;
        self.recursos_lidos = recursos_lidos;
        self.avisos_de_midia = avisos_de_midia;
        self.gl_buffers = gl_buffers;
        self.egl_color_bytes = egl_color_bytes;
        self.egl_color_readback = egl_color_readback;
        self.threads = threads;
        self.resume_callbacks = resume_callbacks;
        self.pending_threads = pending_threads;
        self.quadros_do_update = quadros_do_update;
        Ok(())
    }
}

/// Quantos números a seção `resto.numeros` tem.
///
/// Conferido na leitura, como o `ESCALARES`: um estado com outro número de campos é recusa, e não
/// leitura deslocada. O teste cobra que a conta esteja certa.
const REST0: usize = 63 + CLR_COUNT * 3 + crate::input::PORTAS * 5 + crate::input::PORTAS * 3 + 30 + 3;


/// **Os widgets do `IWidget`** — a última tabela grande.
///
/// Cada widget tem três mapas de números dentro (filhos, propriedades e modelos), dois pares de
/// coordenadas, texto, e três duplas de função e contexto (tratador, desenho e liberadores).
///
/// O identificador entra no nome das seções, como nas superfícies e nos arquivos: assim o mapa de
/// dentro vai num bloco próprio, e não misturado com os números do widget.
///
/// A `serial` vai junto porque é ela que ordena os widgets em [`Machine::formulario_atual`]: sem
/// ela, o widget que o jogo desenhou por último passaria a ser outro depois de carregar.
impl<C: CpuBackend> Machine<C> {
    fn grava_widgets(&self, secoes: &mut Secoes) {
        secoes.poe_u32s("wid.ids", self.widgets.keys().copied());
        for (id, widget) in &self.widgets {
            secoes.poe_u32s(
                &format!("wid.{id}.meta"),
                [
                    widget.tamanho.0,
                    widget.tamanho.1,
                    widget.posicao.0 as u32,
                    widget.posicao.1 as u32,
                    widget.classe,
                    widget.serial as u32,
                    (widget.serial >> 32) as u32,
                    u32::from(widget.visivel),
                    widget.pai,
                    widget.tratador.0,
                    widget.tratador.1,
                    widget.desenho.0,
                    widget.desenho.1,
                    widget.liberadores.0,
                    widget.liberadores.1,
                    u32::from(widget.partiu),
                ],
            );
            secoes.poe_mapa(
                &format!("wid.{id}.filhos"),
                widget.filhos.iter().map(|(a, b)| (*a, *b)),
            );
            secoes.poe_mapa(
                &format!("wid.{id}.propriedades"),
                widget.propriedades.iter().map(|(a, b)| (*a, *b)),
            );
            secoes.poe_mapa(
                &format!("wid.{id}.modelos"),
                widget.modelos.iter().map(|(a, b)| (*a, *b)),
            );
            secoes.poe_u32s(&format!("wid.{id}.anexados"), widget.anexados.iter().copied());
            secoes.poe_texto(&format!("wid.{id}.texto"), &widget.texto);
        }
    }

    fn restaura_widgets(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let ids = leitor.u32s("wid.ids")?;
        let mut widgets = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("wid.{id}.meta"))?;
            if meta.len() != 16 {
                return Err(Erro::Secao {
                    nome: format!("wid.{id}.meta"),
                    motivo: format!("esperava 16 números e veio {}", meta.len()),
                });
            }
            let filhos = leitor.pares(&format!("wid.{id}.filhos"))?.into_iter().collect();
            let propriedades = leitor
                .pares(&format!("wid.{id}.propriedades"))?
                .into_iter()
                .collect();
            let modelos = leitor.pares(&format!("wid.{id}.modelos"))?.into_iter().collect();
            widgets.insert(
                id,
                Widget {
                    filhos,
                    propriedades,
                    modelos,
                    tamanho: (meta[0], meta[1]),
                    posicao: (meta[2] as i32, meta[3] as i32),
                    classe: meta[4],
                    texto: leitor.texto(&format!("wid.{id}.texto"))?,
                    serial: u64::from(meta[5]) | (u64::from(meta[6]) << 32),
                    anexados: leitor.u32s(&format!("wid.{id}.anexados"))?,
                    visivel: meta[7] != 0,
                    pai: meta[8],
                    tratador: (meta[9], meta[10]),
                    desenho: (meta[11], meta[12]),
                    liberadores: (meta[13], meta[14]),
                    partiu: meta[15] != 0,
                },
            );
        }
        self.widgets = widgets;
        Ok(())
    }
}


/// As duas últimas tabelas de biblioteca: o decodificador de imagem e o banco aberto.
///
/// ## O decodificador: estado de verdade, e cabe
///
/// `DecoderState` é o **arquivo montado pedaço a pedaço** pelo `IForceFeed::Write`, o bitmap já
/// criado e se a imagem tem transparência. Os três campos são codificáveis, e entram: um jogo que
/// salve no meio de uma transferência de imagem continua de onde estava.
///
/// ## O banco: o conteúdo **não está no motor**
///
/// `Database` guarda uma conexão viva do SQLite, e conexão não se serializa. Mas o conteúdo
/// **também não precisa**: ele está no arquivo, no disco, e o que o save state grava é o caminho —
/// exatamente como faz com os arquivos abertos. A volta reabre.
///
/// O que se perde, e vai dito: uma **transação aberta** no instante do save. O SQLite só grava o
/// que foi confirmado, então o que estava em curso volta como não feito. É a mesma natureza do
/// arquivo que mudou no disco entre salvar e carregar — e é por isso que o caminho fica guardado
/// dentro do `Database`: sem ele, a volta seria impossível.
impl<C: CpuBackend> Machine<C> {
    fn grava_bibliotecas(&self, secoes: &mut Secoes) {
        secoes.poe_u32s("dec.ids", self.decoders.keys().copied());
        for (id, decodificador) in &self.decoders {
            secoes.poe_u32s(
                &format!("dec.{id}.meta"),
                [
                    decodificador.bitmap.unwrap_or(0),
                    u32::from(decodificador.bitmap.is_some()),
                    u32::from(decodificador.transparent),
                ],
            );
            secoes.poe(&format!("dec.{id}.fed"), decodificador.fed.clone());
        }

        secoes.poe_u32s("db.ids", self.databases.keys().copied());
        for (id, banco) in &self.databases {
            secoes.poe_texto(&format!("db.{id}.caminho"), &banco.caminho().to_string_lossy());
        }
    }

    fn restaura_bibliotecas(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let ids = leitor.u32s("dec.ids")?;
        let mut decoders = std::collections::HashMap::new();
        for id in ids {
            let meta = leitor.u32s(&format!("dec.{id}.meta"))?;
            if meta.len() != 3 {
                return Err(Erro::Secao {
                    nome: format!("dec.{id}.meta"),
                    motivo: format!("esperava 3 números e veio {}", meta.len()),
                });
            }
            let tem_bitmap = match meta[1] {
                0 => false,
                1 => true,
                outro => {
                    return Err(Erro::Secao {
                        nome: format!("dec.{id}.meta"),
                        motivo: format!("o campo `bitmap` vale {outro}, e é booleano"),
                    })
                }
            };
            decoders.insert(
                id,
                DecoderState {
                    fed: secao_exigida(leitor, &format!("dec.{id}.fed"))?,
                    bitmap: tem_bitmap.then_some(meta[0]),
                    transparent: meta[2] != 0,
                },
            );
        }

        // Os bancos são **reabertos antes de aplicar**: um caminho que não existe mais é recusa, e
        // não uma tabela de bancos com uma entrada pela metade.
        let ids = leitor.u32s("db.ids")?;
        let mut databases = std::collections::HashMap::new();
        for id in ids {
            let caminho =
                std::path::PathBuf::from(leitor.texto(&format!("db.{id}.caminho"))?);
            let banco =
                crate::brew::sql::Database::open(&caminho).map_err(|erro| Erro::Secao {
                    nome: format!("db.{id}"),
                    motivo: format!(
                        "o estado guarda \"{}\" aberto e ele não pôde ser reaberto: {erro}",
                        caminho.display()
                    ),
                })?;
            databases.insert(id, banco);
        }

        self.decoders = decoders;
        self.databases = databases;
        Ok(())
    }
}


/// Os últimos campos de estado que faltavam: o trecho interrompido, as chamadas pendentes, as
/// superfícies do EGL e os resumos em curso.
///
/// ## O que **não** entra, e agora com a razão medida
///
/// `cargas_de_midia` guarda os sons já decodificados — uma `Arc<Sound>` com todas as amostras, que
/// num som longo passa de cem megabytes. Ele **parece** uma lacuna, e não é: é cache **com caminho
/// de recarga**. O `resolve_midia` procura a chave e, quando não acha, lê o `AEEMediaData` e
/// carrega de novo. Derrubá-lo custa uma releitura do arquivo, e gravá-lo somaria o arquivo inteiro
/// ao save state.
///
/// O mesmo argumento vale para `vfs` e `resources`, que já estavam de fora. E os outros 25 campos
/// que sobram são instrumento e diagnóstico: contadores de chamada, rastreios, hipóteses, listas de
/// arquivos que faltaram. **Salvá-los não é só desnecessário, é errado**: duas sessões igualmente
/// válidas do mesmo jogo produziriam save states diferentes, e comparar dois estados passaria a
/// medir quanto cada um foi observado.
impl<C: CpuBackend> Machine<C> {
    fn grava_ultimos(&self, secoes: &mut Secoes) {
        // O trecho interrompido: o endereço e os quinze registradores de quem foi interrompido.
        let (tem, endereco, registradores) = match self.trecho_interrompido {
            Some((endereco, registradores)) => (1u32, endereco, registradores),
            None => (0, 0, [0u32; 15]),
        };
        let mut numeros = vec![tem, endereco];
        numeros.extend(registradores);
        secoes.poe_u32s("ult.trecho", numeros);

        secoes.poe_registros(
            "ult.chamadas",
            self.pending_calls
                .iter()
                .map(|chamada| {
                    vec![
                        chamada.function,
                        chamada.args[0],
                        chamada.args[1],
                        chamada.args[2],
                        chamada.args[3],
                    ]
                })
                .collect::<Vec<_>>(),
        );

        secoes.poe_registros(
            "ult.egl_surfaces",
            self.egl_surfaces
                .iter()
                .map(|(id, (a, b))| vec![*id, *a, *b])
                .collect::<Vec<_>>(),
        );

        // Os resumos em curso. O estado do MD5 cabe em quatro palavras, o resto do bloco e o
        // comprimento.
        secoes.poe_u32s("hash.ids", self.hashes.keys().copied());
        for (id, hash) in &self.hashes {
            let (state, buffer, length) = hash.md5.estado();
            let mut numeros = Vec::with_capacity(4 + 2 + buffer.len());
            numeros.extend(state);
            numeros.push(length as u32);
            numeros.push((length >> 32) as u32);
            numeros.push(buffer.len() as u32);
            numeros.extend(buffer.iter().map(|b| u32::from(*b)));
            secoes.poe_u32s(&format!("hash.{id}.md5"), numeros);
        }
    }

    fn restaura_ultimos(&mut self, leitor: &Leitor<'_>) -> Result<(), Erro> {
        let numeros = leitor.u32s("ult.trecho")?;
        if numeros.len() != 17 {
            return Err(Erro::Secao {
                nome: "ult.trecho".to_string(),
                motivo: format!("esperava 17 números e veio {}", numeros.len()),
            });
        }
        let trecho_interrompido = match numeros[0] {
            0 => None,
            1 => {
                let mut registradores = [0u32; 15];
                registradores.copy_from_slice(&numeros[2..17]);
                Some((numeros[1], registradores))
            }
            outro => {
                return Err(Erro::Secao {
                    nome: "ult.trecho".to_string(),
                    motivo: format!("o campo de presença vale {outro}, e é booleano"),
                })
            }
        };

        let pending_calls: Vec<GuestCall> = leitor
            .registros("ult.chamadas", 5)?
            .into_iter()
            .map(|r| GuestCall {
                function: r[0],
                args: [r[1], r[2], r[3], r[4]],
            })
            .collect();

        let egl_surfaces: std::collections::HashMap<u32, (u32, u32)> = leitor
            .registros("ult.egl_surfaces", 3)?
            .into_iter()
            .map(|r| (r[0], (r[1], r[2])))
            .collect();

        let ids = leitor.u32s("hash.ids")?;
        let mut hashes = std::collections::HashMap::new();
        for id in ids {
            let numeros = leitor.u32s(&format!("hash.{id}.md5"))?;
            if numeros.len() < 7 {
                return Err(Erro::Secao {
                    nome: format!("hash.{id}.md5"),
                    motivo: format!("esperava ao menos 7 números e veio {}", numeros.len()),
                });
            }
            let quantos = numeros[6] as usize;
            if numeros.len() != 7 + quantos {
                return Err(Erro::Secao {
                    nome: format!("hash.{id}.md5"),
                    motivo: format!(
                        "diz ter {quantos} byte(s) no resto do bloco e vieram {}",
                        numeros.len() - 7
                    ),
                });
            }
            let mut md5 = crate::brew::crypto::Md5::new();
            let buffer: Vec<u8> = numeros[7..].iter().map(|b| *b as u8).collect();
            md5.restaura_estado(
                [numeros[0], numeros[1], numeros[2], numeros[3]],
                &buffer,
                u64::from(numeros[4]) | (u64::from(numeros[5]) << 32),
            )
            .map_err(|motivo| Erro::Secao {
                nome: format!("hash.{id}.md5"),
                motivo,
            })?;
            hashes.insert(id, HashState { md5 });
        }

        self.trecho_interrompido = trecho_interrompido;
        self.pending_calls = pending_calls;
        self.egl_surfaces = egl_surfaces;
        self.hashes = hashes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // **O backend padrão, e não o unicorn por nome.** No Windows ARM64 o `unicorn` não existe (o
    // QEMU de lá precisa de um montador MASM que só há para x86), e o alias resolve para o
    // `dynarmic`. Usar o nome do unicorn aqui deixava o alvo vermelho no CI — que é justamente
    // quem enxerga o que o teste local não pode ver.
    use crate::cpu::BackendPadrao as UnicornCpu;
    use crate::loader::self as loader;

    /// O menor módulo que o carregador aceita. Não precisa fazer nada: o alvo aqui é o estado da
    /// máquina em volta dele — memória, registradores e os contadores de alocação.
    fn modulo() -> crate::loader::modfile::ModImage {
        let code = [
            0xe3a0_0010u32.to_le_bytes(), // mov r0, #16
            0xe12f_ff1eu32.to_le_bytes(), // bx lr
        ]
        .concat();
        crate::loader::modfile::ModImage::parse(code).unwrap()
    }

    fn maquina() -> Machine<UnicornCpu> {
        let module = loader::load(&modulo()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine
    }

    /// **As flags e o relógio voltam com o estado.**
    ///
    /// É o par que faz um save state "quase" funcionar quando falta: a memória volta, o programa
    /// volta, as flags ficam as de outra execução — a comparação que o jogo fez antes de salvar
    /// continua valendo e a decisão seguinte pode tomar o outro caminho. E o relógio é o tempo que
    /// o jogo enxerga: sem ele, os `SetTimer` e o áudio por quadro saem de outro instante.
    #[test]
    fn as_flags_e_o_relogio_vem_de_volta() {
        let mut antes = maquina();
        // O bit de negativo, para o valor ser diferente do que a máquina nova tem.
        let cpsr = antes.cpu.cpsr() | (1 << 31);
        antes.cpu.set_cpsr(cpsr);
        antes.cpu.set_instructions(12_345_678);

        let mut depois = maquina();
        assert_eq!(depois.cpu.instructions(), 0, "a máquina nova começa com o relógio zerado");

        let arquivo = antes.grava_estado();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.cpu.cpsr(), cpsr, "as flags não voltaram");
        assert_eq!(depois.cpu.instructions(), 12_345_678, "o relógio não voltou");
    }

    /// **O estado volta igual**: registradores, heap, pilha e o livro do heap.
    ///
    /// O heap é o caso interessante porque a memória dele *não* vai inteira: vai até o primeiro
    /// endereço nunca usado. Se o corte estiver errado, o que o jogo tinha escrito some — e um
    /// save state que perde os dados do jogo é pior que um save state que não existe.
    #[test]
    fn registradores_e_memoria_vem_de_volta() {
        let mut antes = maquina();
        let bloco = antes.heap.alloc(0x4000).expect("bloco no heap");
        let no_heap = bloco + 0x100;
        antes.cpu.write_mem(no_heap, b"estado do jogo").unwrap();
        let na_pilha = loader::STACK_BASE + 0x300;
        antes.cpu.write_mem(na_pilha, &[7u8; 16]).unwrap();
        antes.cpu.write_reg(Reg::R5, 0xdead_beef);
        antes.cpu.write_reg(Reg::R12, 0x1020_3040);

        let arquivo = antes.grava_estado();

        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.cpu.read_reg(Reg::R5), 0xdead_beef);
        assert_eq!(depois.cpu.read_reg(Reg::R12), 0x1020_3040);
        let mut lido = [0u8; 14];
        depois.cpu.read_mem(no_heap, &mut lido).unwrap();
        assert_eq!(&lido, b"estado do jogo");
        let mut pilha = [0u8; 16];
        depois.cpu.read_mem(na_pilha, &mut pilha).unwrap();
        assert_eq!(pilha, [7u8; 16]);

        // E a máquina restaurada aloca onde a original alocaria, como no teste do heap sozinho.
        let proximo_antes = antes.heap.alloc(64);
        let proximo_depois = depois.heap.alloc(64);
        assert_eq!(proximo_depois, proximo_antes);
    }

    /// **O heap volta com o `next` que o estado tinha**, e não com o da máquina de agora.
    ///
    /// É o contrário do que eu escrevi primeiro: eu tratei "o heap do estado é maior" como erro,
    /// e não é — é o caso normal. Quem carrega um save state carrega o heap que estava lá.
    #[test]
    fn o_heap_volta_com_o_tamanho_do_estado() {
        let mut antes = maquina();
        antes.heap.alloc(0x2000).expect("bloco no heap");
        let arquivo = antes.grava_estado();

        let mut depois = maquina();
        assert_eq!(depois.heap.proximo(), loader::HEAP_BASE, "a máquina nova começa vazia");
        depois.restaura_estado(&arquivo).expect("restaurou");
        assert_eq!(depois.heap.proximo(), antes.heap.proximo());
    }

    /// Um estado de **outro módulo** é recusado, e sem escrever nada.
    ///
    /// Aqui a máquina tem outro módulo, logo outra região `module`: o tamanho não bate, e o erro
    /// diz os dois números. Aplicar isso escreveria dados de um jogo por cima do código de outro.
    #[test]
    fn estado_de_outro_modulo_e_recusado() {
        let mut antes = maquina();
        antes.heap.alloc(0x1000).expect("bloco no heap");
        let arquivo = antes.grava_estado();

        let maior = [
            0xe3a0_0010u32.to_le_bytes(),
            0xe3a0_1010u32.to_le_bytes(),
            0xe3a0_2010u32.to_le_bytes(),
            0xe3a0_3010u32.to_le_bytes(),
            0xe12f_ff1eu32.to_le_bytes(),
        ]
        .concat();
        let module = loader::load(
            &crate::loader::modfile::ModImage::parse(maior).unwrap(),
        )
        .unwrap();
        let mut outra = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        outra.cpu.reset(&outra.module.mem).unwrap();

        let proximo_antes = outra.heap.proximo();
        match outra.restaura_estado(&arquivo) {
            Err(Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "mem.module");
                assert!(motivo.contains("bytes desta região"), "{motivo}");
            }
            outro => panic!("devia recusar o estado de outro módulo, e devolveu {outro:?}"),
        }
        assert_eq!(
            outra.heap.proximo(),
            proximo_antes,
            "a recusa não pode ter aplicado metade"
        );
    }

    /// **O tamanho do estado, medido.** É o que decide se isso é usável: um save state que grava
    /// a memória inteira sairia com 84 MB, e o RetroArch grava um a cada atalho do jogador.
    ///
    /// O corte no heap é o que faz a diferença, e as regiões que ainda vão inteiras (objetos e
    /// superfícies) são a maior parte do que sobra. O teste imprime o número porque ele muda
    /// quando as próximas seções entrarem — e é bom que mude à vista.
    #[test]
    fn o_estado_nao_carrega_a_memoria_inteira() {
        let mut maquina = maquina();
        maquina.heap.alloc(64 * 1024).expect("bloco no heap");
        let arquivo = maquina.grava_estado();
        let mb = arquivo.len() as f64 / (1024.0 * 1024.0);
        eprintln!(
            "estado da máquina mínima: {} bytes ({mb:.2} MB); a memória mapeada é 84 MB",
            arquivo.len()
        );
        assert!(
            arquivo.len() < 24 * 1024 * 1024,
            "o estado passou de 24 MB: {} bytes",
            arquivo.len()
        );
    }

    #[test]
    fn um_arquivo_que_nao_e_save_state_e_recusado() {
        let mut maquina = maquina();
        assert!(matches!(
            maquina.restaura_estado(b"nao sou eu"),
            Err(Erro::Truncado { .. })
        ));
    }

    /// **Entrada e agendamento voltam inteiros.**
    ///
    /// São as tabelas que o jogo sente na hora: a fila de teclas que ele ainda não leu, a fila de
    /// eventos de botão, quem registrou aviso de aparelho, os temporizadores vencendo e os retornos
    /// pendentes. Sem elas o save state "volta" e o jogo perde o que já tinha na mão.
    #[test]
    fn entrada_e_agendamento_vem_de_volta() {
        use crate::input::avk;

        let mut antes = maquina();
        antes.set_key(avk::SELECT, true);
        antes.set_key(avk::ZERO, false);
        let mut pad = crate::input::Pad::default();
        pad.press(1, true);
        antes.set_port_pad(0, pad);
        antes
            .input_signals
            .insert(("RegisterForPositionChange", 0), 0x5555_0000);
        antes.timers.push(Timer {
            deadline_ms: 424_242,
            callback: Callback {
                function: 0x1000_2000,
                context: 0xdead_0000,
            },
        });
        antes.signals.insert(
            0x77,
            Callback {
                function: 0x1000_3000,
                context: 0xbeef_0000,
            },
        );
        antes.pending_signals.push(Callback {
            function: 0x1000_4000,
            context: 0x1234_0000,
        });

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        assert!(depois.teclas.is_empty(), "a máquina nova começa sem fila");
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.teclas, antes.teclas, "a fila de teclas não voltou");
        assert_eq!(
            depois.pad_events[0], antes.pad_events[0],
            "a fila de eventos de botão não voltou"
        );
        assert_eq!(
            depois.input_signals.get(&("RegisterForPositionChange", 0)),
            Some(&0x5555_0000),
            "o registro do aviso de aparelho não voltou"
        );
        assert_eq!(depois.timers.len(), 1, "o temporizador não voltou");
        assert_eq!(depois.timers[0].deadline_ms, 424_242);
        assert_eq!(depois.timers[0].callback.function, 0x1000_2000);
        assert_eq!(depois.signals.get(&0x77).map(|c| c.context), Some(0xbeef_0000));
        assert_eq!(depois.pending_signals.len(), 1);
        assert_eq!(depois.pending_signals[0].function, 0x1000_4000);
    }

    /// O código do nome do sinal de aparelho vai e volta para **todos** os nomes conhecidos.
    ///
    /// É a mesma ideia do teste das 62 interfaces: nome novo sem entrada na tabela deixa isto
    /// vermelho, e não um registro que se perde em silêncio num save state.
    #[test]
    fn o_nome_do_sinal_vai_e_volta() {
        // **A lista inteira**, e é ela que o gravador usa: nome novo sem entrar aqui deixa o
        // gravador gritando em vez de gravar um estado que não carrega.
        for nome in SINAIS_DE_APARELHO {
            let codigo = codigo_do_sinal_de_entrada(nome);
            assert_ne!(codigo, u32::MAX, "{nome} não tem código");
            assert_eq!(nome_do_sinal_de_entrada(codigo), Some(nome));
        }
        assert_eq!(nome_do_sinal_de_entrada(9999), None);
        // **Um nome inventado não é testado aqui de propósito**: o gravador tem `debug_assert!`
        // para ele, e é isso que se quer — nome novo que ninguém pôs na lista faz o teste gritar,
        // em vez de gravar um estado que não carrega. Foi assim que o terceiro nome apareceu.
    }

    /// **As tabelas numéricas voltam**, e um valor fora da faixa do motor é recusado.
    ///
    /// A segunda metade é a que importa mais: um volume que não cabe em `u16` ou um `dono` que não
    /// é booleano é arquivo corrompido, e acomodar em silêncio seria carregar um estado que o jogo
    /// não escreveu.
    #[test]
    fn as_tabelas_numericas_vem_de_volta() {
        let mut antes = maquina();
        antes.dib_buffers.insert(0x11, 0x1000);
        antes.dib_capacity.insert(0x11, 640 * 480 * 2);
        antes.transformacoes.insert(0x22, 7);
        antes.canvases.insert(0x33, 0x2000);
        antes.feeds.insert(0x44, 12);
        antes.transparency.insert(0x55, 0x8000);
        antes.streams.insert(
            0x66,
            MemStream {
                buffer: 0x3000,
                size: 512,
                position: 128,
                dono: true,
            },
        );
        let mut info = [0u8; 5];
        info[1] = 9;
        antes.sounds.insert(
            0x77,
            SoundState {
                notify: Callback {
                    function: 0x1000_5000,
                    context: 0x7777,
                },
                info,
                volume: 42,
            },
        );

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.dib_capacity.get(&0x11), Some(&(640 * 480 * 2)));
        assert_eq!(depois.transformacoes.get(&0x22), Some(&7));
        assert_eq!(depois.canvases.get(&0x33), Some(&0x2000));
        assert_eq!(depois.feeds.get(&0x44), Some(&12));
        assert_eq!(depois.transparency.get(&0x55), Some(&0x8000));
        let stream = depois.streams.get(&0x66).expect("o stream voltou");
        assert_eq!((stream.buffer, stream.size, stream.position), (0x3000, 512, 128));
        assert!(stream.dono, "o `dono` do buffer não voltou");
        let som = depois.sounds.get(&0x77).expect("o som voltou");
        assert_eq!(som.volume, 42);
        assert_eq!(som.info[1], 9);
        assert_eq!(som.notify.function, 0x1000_5000);
    }

    /// Um volume que não cabe num `u16` é **recusado**, e não acomodado.
    #[test]
    fn som_com_volume_impossivel_e_recusado() {
        use crate::save_state::Secoes;

        let mut secoes = Secoes::nova();
        // As seções que o restaurador lê primeiro, vazias, e a dos sons com o valor impossível.
        for nome in [
            "tab.dib_buffers",
            "tab.dib_capacity",
            "tab.transformacoes",
            "tab.canvases",
            "tab.feeds",
            "tab.image_bitmaps",
            "tab.image_info",
            "tab.transparency",
            "tab.streams",
        ] {
            secoes.poe_u32s(nome, []);
        }
        // id, notify.function, notify.context, volume, cinco bytes de info
        secoes.poe_u32s("tab.sounds", [0x77u32, 0, 0, 999_999, 0, 0, 0, 0, 0]);
        let arquivo = secoes.fecha();
        let leitor = crate::save_state::Leitor::abre(&arquivo).expect("abriu");

        let mut maquina = maquina();
        match maquina.restaura_tabelas_numericas(&leitor) {
            Err(Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "tab.sounds");
                assert!(motivo.contains("999999"), "{motivo}");
            }
            outro => panic!("devia recusar o volume impossível, e devolveu {outro:?}"),
        }
    }

    /// **As métricas de fonte e os arquivos abertos voltam.**
    ///
    /// O caso do arquivo é o interessante: ele não guarda bytes, guarda **caminho e deslocamento**,
    /// e a volta reabre o arquivo pelo caminho e devolve o deslocamento. É o que faz um jogo que
    /// estava lendo no meio de um arquivo continuar lendo do mesmo ponto.
    #[test]
    fn fontes_e_arquivos_abertos_vem_de_volta() {
        use std::io::{Read as _, Write as _};

        // Um arquivo de verdade, em disco, com conteúdo conhecido.
        let caminho = std::env::temp_dir().join("zeebx-teste-do-estado.bin");
        let mut criado = std::fs::File::create(&caminho).expect("criar o arquivo de teste");
        let duzentos_e_cinquenta_e_seis: Vec<u8> = (0u8..=255).collect();
        criado
            .write_all(&duzentos_e_cinquenta_e_seis)
            .expect("escrever");
        drop(criado);

        let mut antes = maquina();
        antes.fontes.insert(
            0x1010,
            crate::machine::font::Metricas {
                ascent: 13,
                descent: 3,
                leading: 1,
                max_char_width: 9,
                height: 17,
                bold: true,
                italic: false,
            },
        );
        {
            let mut file = std::fs::File::open(&caminho).expect("abrir para ler");
            // O jogo já leu sete bytes deste arquivo.
            let mut lidos = [0u8; 7];
            file.read_exact(&mut lidos).expect("ler sete bytes");
            assert_eq!(lidos, [0u8, 1, 2, 3, 4, 5, 6]);
            antes.open_files.insert(
                0x2020,
                OpenFile {
                    file,
                    guest_path: "./dados/cena.bin".to_string(),
                    caminho: caminho.clone(),
                },
            );
        }

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        let metricas = depois.fontes.get(&0x1010).expect("as métricas voltaram");
        assert_eq!(
            (
                metricas.ascent,
                metricas.descent,
                metricas.leading,
                metricas.max_char_width,
                metricas.height
            ),
            (13, 3, 1, 9, 17)
        );
        assert!(metricas.bold);
        assert!(!metricas.italic);

        let aberto = depois.open_files.get_mut(&0x2020).expect("o arquivo voltou");
        assert_eq!(aberto.guest_path, "./dados/cena.bin");
        let mut seguinte = [0u8; 2];
        aberto.file.read_exact(&mut seguinte).expect("ler o seguinte");
        assert_eq!(
            seguinte,
            [7, 8],
            "o arquivo devia continuar do byte 7, e não do começo"
        );
        let _ = std::fs::remove_file(&caminho);
    }

    /// **As tabelas de conteúdo voltam** — preferências, parâmetros, `IConfig`, texto decifrado,
    /// resposta da rede.
    ///
    /// Elas guardam o conteúdo porque ele **não existe em lugar nenhum** fora do motor: ao contrário
    /// de um arquivo aberto, que se reabre do disco, um `SetPrefs` que o jogo fez já não está em
    /// disco nenhum.
    #[test]
    fn as_tabelas_de_conteudo_vem_de_volta() {
        let mut antes = maquina();
        antes.prefs.insert((0x0100_0001, 3), vec![1, 2, 3]);
        antes.prefs.insert((0x0100_0002, 0), Vec::new());
        antes
            .parametros_de_colecao
            .insert((0x10, 0x20), b"parametro".to_vec());
        antes.sources.insert(0x30, b"fonte".to_vec());
        antes.paginas_html.insert(0x40, b"<html>".to_vec());
        antes
            .config_items
            .entry(0x50)
            .or_default()
            .insert(0x60, b"valor".to_vec());
        antes.plaintexts.push_back(b"primeiro".to_vec());
        antes.plaintexts.push_back(b"segundo".to_vec());
        antes.web_response = b"resposta".to_vec();

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.prefs.get(&(0x0100_0001, 3)), Some(&vec![1, 2, 3]));
        assert_eq!(depois.prefs.get(&(0x0100_0002, 0)), Some(&Vec::new()));
        assert_eq!(
            depois.parametros_de_colecao.get(&(0x10, 0x20)),
            Some(&b"parametro".to_vec())
        );
        assert_eq!(depois.sources.get(&0x30), Some(&b"fonte".to_vec()));
        assert_eq!(depois.paginas_html.get(&0x40), Some(&b"<html>".to_vec()));
        assert_eq!(
            depois.config_items.get(&0x50).and_then(|i| i.get(&0x60)),
            Some(&b"valor".to_vec())
        );
        // A fila de texto decifrado volta **na ordem**, que é o conteúdo dela.
        let fila: Vec<&Vec<u8>> = depois.plaintexts.iter().collect();
        assert_eq!(
            fila,
            vec![&b"primeiro".to_vec(), &b"segundo".to_vec()],
            "a ordem do texto decifrado não voltou"
        );
        assert_eq!(depois.web_response, b"resposta".to_vec());
    }

    /// Um bloco com tamanho maior do que o arquivo tem é **recusado**.
    #[test]
    fn bloco_com_tamanho_mentiroso_e_recusado() {
        use crate::save_state::Secoes;

        let mut secoes = Secoes::nova();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u32.to_le_bytes()); // um item
        bytes.extend_from_slice(&2u32.to_le_bytes()); // chave de dois números
        bytes.extend_from_slice(&[1, 0, 0, 0, 2, 0, 0, 0]);
        bytes.extend_from_slice(&999u32.to_le_bytes()); // diz ter 999 bytes
        bytes.extend_from_slice(b"curto");
        secoes.poe("blocos", bytes);
        let arquivo = secoes.fecha();
        let leitor = crate::save_state::Leitor::abre(&arquivo).expect("abriu");
        match leitor.blocos("blocos") {
            Err(Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "blocos");
                assert!(motivo.contains("999"), "{motivo}");
            }
            outro => panic!("devia recusar o bloco mentiroso, e devolveu {outro:?}"),
        }
    }

    /// **As superfícies e as imagens decodificadas voltam** — os pixels, e o que a superfície já
    /// viveu (`escritas` e `serie`), que é por onde o frontend sabe o que mudou.
    #[test]
    fn superficies_e_imagens_vem_de_volta() {
        let mut antes = maquina();
        // Uma superfície de 4x2 com pixels distintos, para o teste distinguir pixel de pixel.
        let mut superficie = Framebuffer::new(4, 2);
        for (indice, valor) in [0x1111u16, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777, 0x8888]
            .iter()
            .enumerate()
        {
            superficie.set_pixel_native((indice % 4) as i32, (indice / 4) as i32, *valor);
        }
        // Escrever mexe no `touched` e na caixa suja, que é o que o frontend lê.
        assert!(superficie.is_dirty());
        antes.bitmaps.insert(0x9000, superficie);

        let mut imagem = DecodedImage {
            width: 2,
            height: 2,
            pixels: vec![0xaaaa, 0xbbbb, 0xcccc, 0xdddd],
            opaque: vec![true, false, true, true],
            alfa: vec![0, 51, 255, 7],
            frame_width: 1,
        };
        imagem.frame_width = 1;
        antes.images.insert(0xa000, std::rc::Rc::new(imagem));

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        let voltou = depois.bitmaps.get(&0x9000).expect("a superfície voltou");
        assert_eq!((voltou.width(), voltou.height()), (4, 2));
        assert_eq!(
            voltou.pixels(),
            &[0x1111u16, 0x2222, 0x3333, 0x4444, 0x5555, 0x6666, 0x7777, 0x8888],
            "os pixels não voltaram"
        );
        let antes_da_gravacao = antes.bitmaps.get(&0x9000).expect("a original");
        assert_eq!(
            voltou.escritas(),
            antes_da_gravacao.escritas(),
            "o contador de escritas não voltou"
        );
        assert_eq!(
            voltou.serie(),
            antes_da_gravacao.serie(),
            "a série da superfície não voltou"
        );
        assert_eq!(
            voltou.sujeira(),
            antes_da_gravacao.sujeira(),
            "a caixa suja não voltou"
        );

        let imagem = depois.images.get(&0xa000).expect("a imagem voltou");
        assert_eq!((imagem.width, imagem.height), (2, 2));
        assert_eq!(imagem.pixels, vec![0xaaaa, 0xbbbb, 0xcccc, 0xdddd]);
        assert_eq!(imagem.opaque, vec![true, false, true, true], "o `opaque` não voltou");
        assert_eq!(imagem.alfa, vec![0, 51, 255, 7], "o alfa não voltou");
        assert_eq!(imagem.frame_width, 1);
    }

    /// A superfície volta **sem** perder a caixa suja: gravar um estado não pode consumir o aviso
    /// de que a tela mudou, senão o quadro seguinte sai velho por causa do save state.
    #[test]
    fn a_caixa_suja_nao_e_consumida_ao_gravar() {
        let mut maquina = maquina();
        let mut superficie = Framebuffer::new(2, 2);
        superficie.set_pixel_native(1, 1, 0x1234);
        let esperada = superficie.sujeira();
        assert!(esperada.is_some(), "desenhar devia sujar a superfície");
        maquina.bitmaps.insert(0x1000, superficie);

        let _ = maquina.grava_estado();
        assert_eq!(
            maquina.bitmaps.get(&0x1000).and_then(|s| s.sujeira()),
            esperada,
            "gravar o estado consumiu a caixa suja"
        );
    }

    /// Os bits do `opaque` vão empacotados e voltam na ordem certa.
    #[test]
    fn os_bits_do_opaque_vao_e_voltam() {
        for quantos in [0usize, 1, 7, 8, 9, 16, 17, 100] {
            let bits: Vec<bool> = (0..quantos).map(|i| i % 3 == 0).collect();
            let bytes = empacota_bits(&bits);
            assert_eq!(bytes.len(), quantos.div_ceil(8), "tamanho com {quantos} bits");
            assert_eq!(desempacota_bits(&bytes, quantos), bits, "com {quantos} bits");
        }
    }

    /// **Os escalares e os mapas simples voltam** — e a conta de campos está certa.
    ///
    /// A primeira metade deste teste é a que protege o formato: se alguém acrescentar um campo sem
    /// mexer na lista, a contagem deixa de bater e o teste fica vermelho **antes** de um save state
    /// sair deslocado.
    #[test]
    fn os_escalares_vem_de_volta_e_a_conta_esta_certa() {
        let antes = maquina();
        let mut secoes = Secoes::nova();
        antes.grava_escalares_e_mapas(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = crate::save_state::Leitor::abre(&arquivo).expect("abriu");
        assert_eq!(
            leitor.u32s("esc.numeros").expect("a seção").len(),
            ESCALARES,
            "a conta de campos está desatualizada: quem acrescentou um campo não mexeu em ESCALARES"
        );
    }

    #[test]
    fn escalares_e_mapas_vem_de_volta() {
        let mut antes = maquina();
        antes.epoch_seconds = 1_700_000_000;
        antes.random_state = 0x1234_5678;
        antes.nesting = 3;
        antes.current_applet = 0x0102_0304;
        antes.applet_class = 0x0102_0305;
        antes.next_vsync_us = 0x1_2345_6789;
        antes.orcamento = 0x9_8765_4321;
        antes.proximo_serial = 0x5_5555_5555;
        antes.applet_fechado = true;
        antes.wheel_boot_skipped = true;
        antes.pending_launch = Some(0x0102_8e35);
        antes.escritas_do_quadro_gl = Some(0x4_4444_4444);
        antes.scale_source = Some((-2, 7));
        antes.current_thread = Some(0x5151);
        antes.ativacao_pendente = Some((0x61, 0x62));
        antes.installed_applets.insert(0x1);
        antes.installed_applets.insert(0x2);
        antes.mif_no_guest.insert(0x70, 0x80);
        antes.ext_modules = vec![Some(0x90), None, Some(0xa0)];
        antes.rolagem_html.insert(0xb0, 12);
        antes.image_notify.insert(
            0xc0,
            Callback {
                function: 0x1000_6000,
                context: 0xd0,
            },
        );
        antes.dib_do_decodificador.insert(0xe0, (0xf0, 0x11));
        antes.dib_publicado.insert(0x12, 0x13_0000_0014);
        antes.vetores.insert(0x14, (vec![1, 2, 3], 9));
        antes.collections.insert(0x15, (vec![4, 5], 6));

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.epoch_seconds, 1_700_000_000);
        assert_eq!(depois.random_state, 0x1234_5678);
        assert_eq!(depois.nesting, 3);
        assert_eq!(depois.current_applet, 0x0102_0304);
        assert_eq!(depois.applet_class, 0x0102_0305);
        assert_eq!(depois.next_vsync_us, 0x1_2345_6789, "o u64 alto não voltou");
        assert_eq!(depois.orcamento, 0x9_8765_4321);
        assert_eq!(depois.proximo_serial, 0x5_5555_5555);
        assert!(depois.applet_fechado);
        assert!(depois.wheel_boot_skipped);
        assert_eq!(depois.pending_launch, Some(0x0102_8e35));
        assert_eq!(depois.escritas_do_quadro_gl, Some(0x4_4444_4444));
        assert_eq!(depois.scale_source, Some((-2, 7)));
        assert_eq!(depois.current_thread, Some(0x5151));
        assert_eq!(depois.ativacao_pendente, Some((0x61, 0x62)));
        assert_eq!(depois.installed_applets.len(), 2);
        assert_eq!(depois.mif_no_guest.get(&0x70), Some(&0x80));
        assert_eq!(depois.ext_modules, vec![Some(0x90), None, Some(0xa0)]);
        assert_eq!(depois.rolagem_html.get(&0xb0), Some(&12));
        assert_eq!(
            depois.image_notify.get(&0xc0).map(|c| c.function),
            Some(0x1000_6000)
        );
        assert_eq!(depois.dib_do_decodificador.get(&0xe0), Some(&(0xf0, 0x11)));
        assert_eq!(depois.dib_publicado.get(&0x12), Some(&0x13_0000_0014));
        assert_eq!(depois.vetores.get(&0x14), Some(&(vec![1, 2, 3], 9)));
        assert_eq!(depois.collections.get(&0x15), Some(&(vec![4, 5], 6)));
    }

    /// Um `pending_launch` ausente tem de voltar ausente — e não virar zero.
    #[test]
    fn a_ausencia_de_pending_launch_nao_vira_zero() {
        let mut antes = maquina();
        antes.pending_launch = None;
        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.pending_launch = Some(0x9999);
        depois.restaura_estado(&arquivo).expect("restaurou");
        assert_eq!(
            depois.pending_launch, None,
            "o estado disse que não havia pedido de abertura, e voltou um"
        );
    }

    /// **As listas de texto e o ponto de parada voltam.**
    ///
    /// O ponto de parada é o buraco que eu tinha declarado como "não dá para salvar". Dá: o estado
    /// é bem definido, e o que faltava era a codificação. O teste cobre cada variante do `Outcome`,
    /// porque é justamente no caminho menos usado que um campo se perde.
    #[test]
    fn listas_de_texto_e_ponto_de_parada_vem_de_volta() {
        let mut antes = maquina();
        antes.modulos_instalados = vec![
            (0x0102_8e35, "Z-Wheel".to_string()),
            (0x0102_8e36, "Face".to_string()),
        ];
        antes
            .enumerations
            .insert(0x1000, ["primeiro".to_string(), "segundo".to_string()].into());
        antes.stalled = Some(Outcome::Fault {
            addr: 0x10,
            pc: 0x20,
            lr: 0x30,
        });

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.interned.insert("OpenGL ES-CM 1.1", 0x999);
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.modulos_instalados, antes.modulos_instalados);
        let lista: Vec<String> = depois
            .enumerations
            .get(&0x1000)
            .expect("a enumeração voltou")
            .iter()
            .cloned()
            .collect();
        assert_eq!(lista, vec!["primeiro".to_string(), "segundo".to_string()]);
        assert_eq!(
            depois.stalled,
            Some(Outcome::Fault {
                addr: 0x10,
                pc: 0x20,
                lr: 0x30
            })
        );
        assert!(
            depois.interned.is_empty(),
            "o cache de textos estáticos devia ter sido limpo, e não carregado"
        );
    }

    /// Todas as variantes do `Outcome` vão e voltam. É onde um campo se perde sem ninguém notar.
    #[test]
    fn todos_os_desfechos_vao_e_voltam() {
        let desfechos = vec![
            Outcome::Returned { code: 7 },
            Outcome::Unimplemented {
                addr: 0x1,
                args: [2, 3, 4, 5],
                caller: 6,
            },
            Outcome::Fault {
                addr: 8,
                pc: 9,
                lr: 10,
            },
            Outcome::Exception { pc: 11 },
            Outcome::Budget,
            Outcome::CallLimit { calls: 0x1_0000_0002 },
        ];
        for desfecho in desfechos {
            let (rotulo, campos) = descreve(&Some(desfecho.clone()));
            let mut numeros = vec![rotulo];
            numeros.extend(campos);
            assert_eq!(
                monta(&numeros).expect("voltou"),
                Some(desfecho.clone()),
                "o desfecho {desfecho:?} não voltou igual"
            );
        }
        // Ausente vai e volta como ausente.
        assert_eq!(monta(&[0]).expect("ausente"), None);
        // E desfecho desconhecido é recusa, não invenção.
        assert!(monta(&[99]).is_err());
        assert!(monta(&[2]).is_err(), "campos faltando devia ser recusa");
    }

    /// **As sete tabelas do último lote vão e voltam** — cifra, descompressão, espiada, recorte,
    /// modelo de valor, fluxo de PCM, entregas pendentes e estado de mídia.
    #[test]
    fn o_resto_das_tabelas_vem_de_volta() {
        let mut antes = maquina();
        antes.ciphers.insert(
            0x100,
            CipherState {
                key: Some([7u8; 16]),
                iv: [9u8; 16],
                padding: 2,
                pending: vec![1, 2, 3],
            },
        );
        antes.ciphers.insert(
            0x101,
            CipherState {
                key: None,
                iv: [0u8; 16],
                padding: 0,
                pending: Vec::new(),
            },
        );
        antes.unzips.insert(
            0x200,
            UnzipState {
                source: 0x300,
                output: vec![4, 5, 6],
                position: 2,
                expanded: true,
            },
        );
        antes.peeks.insert(
            0x400,
            Peek {
                bytes: vec![7, 8],
                posicao: 1,
                buffer: 0x500,
            },
        );
        antes.recortes_de_imagem.insert(
            0x600,
            RecorteDeImagem {
                x: -3,
                y: 4,
                tamanho: Some((10, 20)),
                transparente: true,
            },
        );
        antes.recortes_de_imagem.insert(
            0x601,
            RecorteDeImagem {
                x: 0,
                y: 0,
                tamanho: None,
                transparente: false,
            },
        );
        antes.modelos_de_valor.insert(
            0x700,
            ModeloDeValor {
                valor: 42,
                tamanho: 4,
                ouvintes: vec![(0x800, 0x900, 0xa00)],
            },
        );
        antes.fluxos_pcm.insert(
            0xb00,
            FluxoPcm {
                fonte: 0xc00,
                taxa: 44_100,
                canais: 2,
                bits: 16,
                sem_sinal: false,
                inicio_us: 0x1_0000_0005,
                quadros_lidos: 0x2_0000_0007,
                tocando: true,
            },
        );
        antes.pending_blits.push(PendingBlit {
            image: 0xd00,
            target: 0xe00,
            x: -5,
            y: 6,
            frame: Some(3),
        });
        antes.media.insert(
            0xf00,
            MediaState {
                state: 5,
                carga: 0x1_0000_0009,
                pendente: (0x10, 0x11),
                buffer: (0x12, 0x13),
                tocar_ao_ler: true,
                volume: 80,
                repeat: 2,
                muted: false,
                notify: Callback {
                    function: 0x1000_7000,
                    context: 0x14,
                },
                ends_us: 0x2_0000_000b,
            },
        );

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.ciphers.get(&0x100).map(|c| c.key), Some(Some([7u8; 16])));
        assert_eq!(depois.ciphers.get(&0x100).map(|c| c.padding), Some(2));
        assert_eq!(depois.ciphers.get(&0x100).map(|c| c.pending.clone()), Some(vec![1, 2, 3]));
        assert_eq!(depois.ciphers.get(&0x101).map(|c| c.key), Some(None));
        let descompressor = depois.unzips.get(&0x200).expect("a descompressão voltou");
        assert_eq!(descompressor.output, vec![4, 5, 6]);
        assert_eq!(descompressor.position, 2);
        assert!(descompressor.expanded);
        assert_eq!(depois.peeks.get(&0x400).map(|p| p.bytes.clone()), Some(vec![7, 8]));
        assert_eq!(
            depois.recortes_de_imagem.get(&0x600).map(|r| (r.x, r.y, r.tamanho)),
            Some((-3, 4, Some((10, 20))))
        );
        assert_eq!(
            depois.recortes_de_imagem.get(&0x601).and_then(|r| r.tamanho),
            None,
            "o recorte sem tamanho voltou com um"
        );
        assert_eq!(
            depois.modelos_de_valor.get(&0x700).map(|m| (m.valor, m.ouvintes.clone())),
            Some((42, vec![(0x800, 0x900, 0xa00)]))
        );
        let fluxo = depois.fluxos_pcm.get(&0xb00).expect("o fluxo voltou");
        assert_eq!((fluxo.canais, fluxo.bits), (2, 16));
        assert_eq!(fluxo.inicio_us, 0x1_0000_0005, "o u64 de início não voltou");
        assert_eq!(fluxo.quadros_lidos, 0x2_0000_0007);
        assert!(fluxo.tocando);
        assert_eq!(depois.pending_blits.len(), 1);
        assert_eq!(depois.pending_blits[0].frame, Some(3));
        let midia = depois.media.get(&0xf00).expect("a mídia voltou");
        assert_eq!(midia.carga, 0x1_0000_0009);
        assert_eq!(midia.pendente, (0x10, 0x11));
        assert_eq!(midia.buffer, (0x12, 0x13));
        assert_eq!(midia.ends_us, 0x2_0000_000b);
        assert_eq!(midia.notify.function, 0x1000_7000);
    }

    /// **O resto: escalares, GL, IGraphics, teclados e threads** — e a conta de campos certa.
    #[test]
    fn o_resto_vem_de_volta_e_a_conta_esta_certa() {
        let antes = maquina();
        let mut secoes = Secoes::nova();
        antes.grava_o_resto(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = crate::save_state::Leitor::abre(&arquivo).expect("abriu");
        assert_eq!(
            leitor.u32s("resto.numeros").expect("a seção").len(),
            REST0,
            "a conta de campos está desatualizada: quem acrescentou um campo não mexeu em REST0"
        );
    }

    #[test]
    fn escalares_gl_e_threads_vem_de_volta() {
        let mut antes = maquina();
        antes.file_error = 12;
        antes.surface_manip = 0x2222;
        antes.gles11_ext = 0x3333;
        antes.boomerang_sequencia = 7;
        antes.calibracoes = (0x44, 0x55);
        antes.pending_response = Some((1, 2, 3));
        antes.pending_end = Some(9);
        antes.egl_color_dimensions = Some((640, 480));
        antes.egl_color_buffer = (0x66, 480 * 640 * 2);
        antes.clip = Some(Rect {
            x: -1,
            y: -2,
            width: 300,
            height: 200,
        });
        antes.network = true;
        antes.network_to = Some("127.0.0.1:80".to_string());
        antes.graphics.fill_mode = true;
        antes.graphics.point_size = 3;
        antes.graphics.origin = (-4, 5);
        antes.graphics.stroke = Rgb { r: 1, g: 2, b: 3 };
        antes.colors[7] = Rgb { r: 200, g: 100, b: 50 };
        let mut pad = Pad::default();
        pad.press(2, true);
        pad.set_axis(1, -100);
        antes.pads[0] = pad;
        antes.movimento[1] = [0.5, -0.25, 1.0];
        antes.gl_vertices = ArrayPointer {
            size: 3,
            kind: 0x1406,
            stride: 12,
            address: 0x7000,
            enabled: true,
            buffer: 0x88,
        };
        antes.gl_normal_atual = [0.0, 1.0, 0.0];
        antes.teclas_da_rolagem.insert(0x99);
        antes.recursos_lidos.insert(0x1234);
        antes.avisos_de_midia.push((
            0xaa,
            0xbb,
            0xcc,
            Callback {
                function: 0x1000_8000,
                context: 0xdd,
            },
        ));
        antes.gl_buffers.insert(0xee, vec![1, 2, 3, 4]);
        antes.egl_color_bytes = vec![5, 6];
        antes.egl_color_readback = vec![7, 8, 9];
        let mut contexto = [0u32; 14];
        contexto[5] = 0xff00;
        antes.threads.insert(
            0x111,
            ThreadState {
                stack: 0x222,
                resume_cb: 0x333,
                resume_pc: 0x444,
                started: true,
                suspended: false,
                finished: false,
                exit_code: 0,
                context: contexto,
                joiners: vec![(
                    Callback {
                        function: 0x555,
                        context: 0x666,
                    },
                    0x777,
                )],
            },
        );
        antes.resume_callbacks.insert(0x888, 0x999);
        antes.pending_threads.push(0xaaa);
        let mut quadro = Framebuffer::new(2, 1);
        quadro.set_pixel_native(0, 0, 0xbeef);
        antes.quadros_do_update.push_back(quadro);

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        assert_eq!(depois.file_error, 12);
        assert_eq!(depois.surface_manip, 0x2222);
        assert_eq!(depois.gles11_ext, 0x3333);
        assert_eq!(depois.boomerang_sequencia, 7);
        assert_eq!(depois.calibracoes, (0x44, 0x55));
        assert_eq!(depois.pending_response, Some((1, 2, 3)));
        assert_eq!(depois.pending_end, Some(9));
        assert_eq!(depois.egl_color_dimensions, Some((640, 480)));
        assert_eq!(depois.egl_color_buffer, (0x66, 480 * 640 * 2));
        let recorte = depois.clip.expect("o clip voltou");
        assert_eq!(
            (recorte.x, recorte.y, recorte.width, recorte.height),
            (-1, -2, 300, 200)
        );
        assert!(depois.network);
        assert_eq!(depois.network_to.as_deref(), Some("127.0.0.1:80"));
        assert!(depois.graphics.fill_mode);
        assert_eq!(depois.graphics.point_size, 3);
        assert_eq!(depois.graphics.origin, (-4, 5));
        assert_eq!((depois.graphics.stroke.r, depois.graphics.stroke.g), (1, 2));
        assert_eq!(depois.colors[7].r, 200);
        assert_eq!(depois.pads[0].buttons, antes.pads[0].buttons, "os botões do pad");
        assert_eq!(depois.pads[0].axes[1], -100, "o eixo do pad");
        assert_eq!(depois.movimento[1], [0.5, -0.25, 1.0], "o movimento é f32");
        assert_eq!(depois.gl_vertices.size, 3);
        assert_eq!(depois.gl_vertices.address, 0x7000);
        assert!(depois.gl_vertices.enabled);
        assert_eq!(depois.gl_vertices.buffer, 0x88);
        assert_eq!(depois.gl_normal_atual, [0.0, 1.0, 0.0]);
        assert!(depois.teclas_da_rolagem.contains(&0x99));
        assert!(depois.recursos_lidos.contains(&0x1234));
        assert_eq!(depois.avisos_de_midia.len(), 1);
        assert_eq!(depois.avisos_de_midia[0].3.function, 0x1000_8000);
        assert_eq!(depois.gl_buffers.get(&0xee), Some(&vec![1, 2, 3, 4]));
        assert_eq!(depois.egl_color_bytes, vec![5, 6]);
        assert_eq!(depois.egl_color_readback, vec![7, 8, 9]);
        let thread = depois.threads.get(&0x111).expect("a thread voltou");
        assert_eq!(thread.stack, 0x222);
        assert_eq!(thread.context[5], 0xff00, "o contexto da thread não voltou");
        assert!(thread.started);
        assert_eq!(thread.joiners.len(), 1);
        assert_eq!(thread.joiners[0].1, 0x777);
        assert_eq!(depois.resume_callbacks.get(&0x888), Some(&0x999));
        assert_eq!(depois.pending_threads, vec![0xaaa]);
        let quadro = depois.quadros_do_update.front().expect("o quadro voltou");
        assert_eq!(quadro.pixels(), &[0xbeef, 0x0000]);
    }

    /// **Os widgets voltam inteiros** — os três mapas, o texto, as coordenadas e as duplas de
    /// função e contexto.
    ///
    /// A `serial` tem prova própria de propósito: é ela que ordena os widgets no
    /// `formulario_atual`, e um widget que perde a ordem vira outro formulário depois de carregar.
    #[test]
    fn os_widgets_vem_de_volta() {
        let mut antes = maquina();
        let mut filhos = std::collections::HashMap::new();
        filhos.insert(0x11u32, 0x22u32);
        filhos.insert(0x33, 0x44);
        let mut propriedades = std::collections::HashMap::new();
        propriedades.insert(6u32, 0x55u32);
        let mut modelos = std::collections::HashMap::new();
        modelos.insert(0x8000u32, 0x8001u32);
        antes.widgets.insert(
            0x1000,
            Widget {
                filhos,
                propriedades,
                modelos,
                tamanho: (640, 480),
                posicao: (-7, 9),
                classe: 0x0102_8e3f,
                texto: "Abrir".to_string(),
                serial: 0x1_0000_0002,
                anexados: vec![0x66, 0x67],
                visivel: true,
                pai: 0x68,
                tratador: (0x69, 0x6a),
                desenho: (0x6b, 0x6c),
                liberadores: (0x6d, 0x6e),
                partiu: true,
            },
        );

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        let widget = depois.widgets.get(&0x1000).expect("o widget voltou");
        assert_eq!(widget.tamanho, (640, 480));
        assert_eq!(widget.posicao, (-7, 9));
        assert_eq!(widget.classe, 0x0102_8e3f);
        assert_eq!(widget.texto, "Abrir");
        assert_eq!(widget.serial, 0x1_0000_0002, "a `serial` não voltou inteira");
        assert_eq!(widget.anexados, vec![0x66, 0x67]);
        assert!(widget.visivel);
        assert_eq!(widget.pai, 0x68);
        assert_eq!(widget.tratador, (0x69, 0x6a));
        assert_eq!(widget.desenho, (0x6b, 0x6c));
        assert_eq!(widget.liberadores, (0x6d, 0x6e));
        assert!(widget.partiu);
        assert_eq!(
            widget.filhos.get(&0x11),
            Some(&0x22),
            "o mapa de filhos não voltou"
        );
        assert_eq!(widget.propriedades.get(&6), Some(&0x55));
        assert_eq!(widget.modelos.get(&0x8000), Some(&0x8001));
    }

    /// **O decodificador volta inteiro, e o banco volta reaberto.**
    ///
    /// O teste do banco é o mais interessante da frente inteira: ele **escreve uma linha, salva,
    /// carrega e lê a linha de volta**. Não é um teste de campos — é o conteúdo sobrevivendo à
    /// ida e volta pelo caminho, que é como o save state trata tudo o que mora em disco.
    #[test]
    fn decodificador_e_banco_vem_de_volta() {
        let mut antes = maquina();
        antes.decoders.insert(
            0x100,
            DecoderState {
                fed: vec![1, 2, 3, 4],
                bitmap: Some(0x200),
                transparent: true,
            },
        );
        antes.decoders.insert(
            0x101,
            DecoderState {
                fed: Vec::new(),
                bitmap: None,
                transparent: false,
            },
        );

        // Um banco de verdade, em disco, com uma linha escrita.
        let caminho = std::env::temp_dir().join("zeebx-teste-do-banco.db");
        let _ = std::fs::remove_file(&caminho);
        let banco = crate::brew::sql::Database::open(&caminho).expect("abrir o banco");
        banco
            .exec("CREATE TABLE t(a INTEGER, b TEXT)")
            .expect("criar a tabela");
        banco
            .exec("INSERT INTO t VALUES (7, 'sete')")
            .expect("escrever a linha");
        antes.databases.insert(0x300, banco);

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        let decodificador = depois.decoders.get(&0x100).expect("o decodificador voltou");
        assert_eq!(decodificador.fed, vec![1, 2, 3, 4]);
        assert_eq!(decodificador.bitmap, Some(0x200));
        assert!(decodificador.transparent);
        let vazio = depois.decoders.get(&0x101).expect("o segundo voltou");
        assert_eq!(vazio.bitmap, None, "a ausência de bitmap não virou zero");
        assert!(!vazio.transparent);

        let reaberto = depois.databases.get(&0x300).expect("o banco voltou");
        let linhas = reaberto
            .exec("SELECT a, b FROM t")
            .expect("ler a linha de volta");
        assert_eq!(linhas.len(), 1, "a linha não sobreviveu à ida e volta");
        assert_eq!(linhas[0].values[0].as_deref(), Some("7"));
        assert_eq!(linhas[0].values[1].as_deref(), Some("sete"));

        let _ = std::fs::remove_file(&caminho);
    }

    /// **O MD5 no meio de um cálculo volta, e o resumo fecha certo.**
    ///
    /// É o teste que dá sentido à decisão: em vez de conferir campos, ele **calcula o resumo pela
    /// metade**. Começa um MD5, salva no meio, carrega num motor novo, termina o cálculo e compara
    /// com o resumo da mesma entrada calculado de uma vez. Se o estado não voltasse inteiro, o
    /// resumo sairia diferente — e nada apontaria para o save state.
    #[test]
    fn o_md5_no_meio_do_calculo_volta_e_fecha_certo() {
        let mensagem: Vec<u8> = (0..200u8).collect();
        // O resumo da mensagem inteira, calculado de uma vez.
        let mut inteiro = crate::brew::crypto::Md5::new();
        inteiro.update(&mensagem);

        let mut antes = maquina();
        let mut md5 = crate::brew::crypto::Md5::new();
        md5.update(&mensagem[..137]);
        antes.hashes.insert(0x500, HashState { md5 });

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        // O `finish` consome o estado, então o MD5 sai do mapa: é assim que o motor o usa.
        let mut retomado = depois.hashes.remove(&0x500).expect("o hash voltou").md5;
        retomado.update(&mensagem[137..]);
        assert_eq!(
            retomado.finish(),
            inteiro.finish(),
            "o resumo retomado não bate com o calculado de uma vez"
        );
    }

    /// O trecho interrompido, as chamadas pendentes e as superfícies do EGL.
    #[test]
    fn trecho_chamadas_e_superficies_vem_de_volta() {
        let mut antes = maquina();
        let mut registradores = [0u32; 15];
        registradores[3] = 0xabcd;
        antes.trecho_interrompido = Some((0x1000, registradores));
        antes.pending_calls.push(GuestCall {
            function: 0x2000,
            args: [1, 2, 3, 4],
        });
        antes.egl_surfaces.insert(0x3000, (640, 480));

        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.restaura_estado(&arquivo).expect("restaurou");

        let (endereco, voltados) = depois.trecho_interrompido.expect("o trecho voltou");
        assert_eq!(endereco, 0x1000);
        assert_eq!(voltados[3], 0xabcd, "o registrador do trecho não voltou");
        assert_eq!(depois.pending_calls.len(), 1);
        assert_eq!(depois.pending_calls[0].function, 0x2000);
        assert_eq!(depois.pending_calls[0].args, [1, 2, 3, 4]);
        assert_eq!(depois.egl_surfaces.get(&0x3000), Some(&(640, 480)));
    }

    /// Um trecho ausente volta ausente, e não como endereço zero.
    #[test]
    fn a_ausencia_de_trecho_nao_vira_endereco_zero() {
        let mut antes = maquina();
        antes.trecho_interrompido = None;
        let arquivo = antes.grava_estado();
        let mut depois = maquina();
        depois.trecho_interrompido = Some((0x7777, [0; 15]));
        depois.restaura_estado(&arquivo).expect("restaurou");
        assert_eq!(depois.trecho_interrompido, None);
    }
}
