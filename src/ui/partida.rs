//! Um jogo aberto numa janela: a sessão, e o que a janela precisa lembrar entre dois quadros.
//!
//! Saiu do `App` do egui para que a interface Qt abra, rode e encerre um jogo do mesmo jeito —
//! ver `docs/implementacao/21-migracao-para-qt.md`, fase 1. Aqui não há toolkit: a entrada chega
//! pronta (controles por porta, movimento, teclas já em AVK), e o que se devolve é o que a janela
//! deve fazer em seguida. Ler o teclado, achar um jogo na biblioteca e desenhar ficam com ela.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::input::{self, Pad, PORTAS};
use crate::session::{FATIA_MAXIMA, Session, StartError, Z_WHEEL};
use crate::ui::settings::{Scaling, Settings};
use crate::input::bindings::Controls;
use crate::velocidade::rewind::{AjustesDoRewind, ControleDoRewind, Leitura};
use crate::velocidade::turbo::TurboDaPorta;
use crate::velocidade::{Avanco, Interruptor, ModoDoAtalho, Ritmo};

/// De quanto em quanto tempo o relatório é regravado. Dois segundos é frequente o bastante para
/// acompanhar uma execução e raro o bastante para não pesar.
const INTERVALO_DO_RELATORIO: Duration = Duration::from_secs(2);

/// Onde a captura de serial de um jogo é gravada, ao lado do relatório. `titulo` é o da
/// biblioteca, [`crate::library::title_for`].
pub fn caminho_da_serial(titulo: &str) -> PathBuf {
    let nome = match titulo.is_empty() {
        true => "zeebx.serial.log".to_string(),
        false => format!("{titulo}.serial.log"),
    };
    crate::config::config_dir().join("relatorios").join(nome)
}

/// A pasta das sessões gravadas. Ver [`caminho_da_sessao`].
pub fn pasta_de_sessoes() -> PathBuf {
    crate::config::config_dir().join("session_logs")
}

/// Onde a sessão de `titulo` que começa em `inicio` é gravada: `aaaa-mm-dd_hh-mm-ss_nome.log`.
///
/// Um arquivo por execução, e não um por jogo como o [`Relatorio`]: o problema que este arquivo
/// existe para pegar aparece depois de meia hora, e a execução seguinte não pode apagá-lo.
pub fn caminho_da_sessao(titulo: &str, inicio: std::time::SystemTime) -> PathBuf {
    let mut nome = String::new();
    for letra in titulo.chars().flat_map(char::to_lowercase) {
        match letra.is_alphanumeric() {
            true => nome.push(letra),
            false if !nome.is_empty() && !nome.ends_with('-') => nome.push('-'),
            false => {}
        }
    }
    let nome = match nome.trim_end_matches('-') {
        "" => "zeebx",
        nome => nome,
    };
    pasta_de_sessoes().join(format!(
        "{}_{nome}.log",
        crate::registro::carimbo_de_arquivo(inicio)
    ))
}

/// De quanto em quanto tempo a sessão gravada registra a saúde do processo.
///
/// Dez segundos casam com o relatório da placa (300 quadros): numa sessão de meia hora são 180
/// linhas, o bastante para ver uma curva subir sem afogar o resto.
const INTERVALO_DA_SAUDE: Duration = Duration::from_secs(10);

/// A gravação da sessão de uma partida. Ver [`crate::registro::grava_em`].
struct Diario {
    /// O número da gravação no registro.
    numero: u64,
    inicio: Instant,
    proxima_saude: Instant,
    /// A parada do jogo já foi registrada: ela continua valendo a cada volta, e uma linha basta.
    parada_registrada: bool,
    /// As chamadas de API na linha de saúde anterior, para a taxa da janela.
    chamadas_antes: u64,
    saude_antes: Instant,
}

/// A memória do processo inteiro em bytes: residente e comprometida (privada).
///
/// É o número que separa vazamento nosso — do emulador, do driver — de vazamento do jogo, que
/// fica dentro do heap do guest e aparece no [`crate::brew::heap::Retrato`]. A residente sozinha
/// engana, porque o sistema a encolhe quando falta memória; a comprometida só cresce se alguém
/// pediu e não devolveu.
fn memoria_do_processo() -> Option<(u64, u64)> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let campo = |nome: &str| {
            status
                .lines()
                .find_map(|linha| linha.strip_prefix(nome))
                .and_then(|resto| resto.split_whitespace().next()?.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        };
        Some((campo("VmRSS:")?, campo("VmData:")?))
    }
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Default)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(
                processo: isize,
                contadores: *mut ProcessMemoryCounters,
                tamanho: u32,
            ) -> i32;
        }
        let mut contadores = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: a estrutura é a `PROCESS_MEMORY_COUNTERS` do Windows, com o `cb` preenchido, e
        // o pseudo-identificador do processo atual não precisa ser fechado.
        let ok = unsafe {
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut contadores, contadores.cb)
        };
        (ok != 0).then_some((
            contadores.working_set_size as u64,
            contadores.pagefile_usage as u64,
        ))
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", windows)))]
    {
        None
    }
}

/// O relatório de uma execução, gravado sozinho num lugar fixo.
///
/// Sem isto o único jeito de ver o relatório de um jogo que **não termina** — e a Z-Wheel não
/// termina, ela repete a abertura — é abrir a janela de log e exportar à mão. E um diálogo de
/// exportar é coisa que se esquece de confirmar: foram três idas e vindas analisando um relatório
/// velho porque o arquivo nunca tinha sido regravado. Um caminho previsível e sempre atual vale mais
/// do que um que o usuário escolhe.
#[derive(Default)]
pub struct Relatorio {
    gravado: Option<Instant>,
}

impl Relatorio {
    /// Um jogo novo abriu: o próximo pedido grava na hora.
    pub fn esquece(&mut self) {
        self.gravado = None;
    }

    /// Grava o relatório da partida, no máximo uma vez a cada dois segundos.
    pub fn grava(&mut self, partida: &Partida) {
        let agora = Instant::now();
        if self.gravado.is_some_and(|antes| agora - antes < INTERVALO_DO_RELATORIO) {
            return;
        }
        self.gravado = Some(agora);
        let destino = partida.caminho_do_relatorio();
        if let Some(pai) = destino.parent() {
            let _ = std::fs::create_dir_all(pai);
        }
        let _ = std::fs::write(&destino, partida.sessao.log().join("\n") + "\n");
    }
}

/// O que a janela faz depois de uma volta. Ver [`Partida::saida`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saida {
    /// O jogo continua.
    Segue,
    /// O jogo saiu sozinho e quem volta é a Z-Wheel, como no console.
    ReabreZWheel,
    /// O jogo saiu sozinho e não há para onde voltar: a janela fecha.
    Fecha,
}

/// O que é preciso, além do caminho, para abrir um jogo. Ver [`Partida::abre`].
pub struct Abertura<'a> {
    pub settings: &'a Settings,
    /// Onde gravar a serial do jogo, quando o log está ligado.
    pub serial: Option<&'a Path>,
    /// O contexto de GL que o rasterizador na placa recebe emprestado, quando a janela tem um.
    pub gl: Option<Arc<glow::Context>>,
    /// Os jogos da biblioteca, pelo ClassID e pelo identificador do módulo: é a lista que a
    /// Z-Wheel mostra e da qual ela pede para lançar.
    pub instalados: Vec<(u32, String)>,
}

pub struct Partida {
    sessao: Session,
    /// O controle do quadro anterior, por porta. Só o aperto vira tecla do console: manter
    /// apertado não repete, que é como um toque se comporta em menu.
    pad_anterior: [Pad; PORTAS],
    /// As teclas BREW já entregues, para só a diferença virar evento.
    teclas_entregues: HashSet<u32>,
    /// Os eventos de tecla que chegaram entre dois quadros, na ordem em que chegaram.
    teclas_pendentes: Vec<(u32, bool)>,
    /// Quando o jogo rodou pela última vez, para saber quanto tempo real ele tem a recuperar.
    ultimo_passo: Instant,
    pub pausada: bool,
    /// Se o jogo foi aberto pela Z-Wheel. Quando ele sai sozinho, a Z-Wheel volta, como no
    /// console; aberto pela biblioteca, sair encerra.
    pub aberta_pela_z_wheel: bool,
    /// A gravação da sessão em arquivo, quando a opção está ligada.
    diario: Option<Diario>,
    /// O atalho do fast-forward, que se segura ou se alterna. Ver [`Partida::le_o_avanco`].
    avanco: Interruptor,
    /// Se o fast-forward valeu na última volta.
    avancando: bool,
    /// O turbo de cada porta: o estado do modo de alternar. Ver [`crate::velocidade::turbo`].
    turbos: [TurboDaPorta; PORTAS],
    /// Os pontos de retorno e o atalho do rewind. Ver [`Partida::le_o_rewind`].
    rewind: ControleDoRewind,
}

/// O que o turbo precisa saber numa volta: a tecla de turbo de cada porta, o mapeamento de onde
/// saem o modo e o botão padrão de cada jogador, e o ritmo.
pub struct TurboDaVolta<'a> {
    pub apertado: [bool; PORTAS],
    pub controles: &'a Controls,
    pub toques_por_segundo: u8,
}

impl Partida {
    /// Monta a sessão do jogo com o que está nas configurações.
    ///
    /// `anterior` é a partida que esta substitui: um jogo aberto pela Z-Wheel começa com a tela
    /// que ela deixou (ver [`Session::herda_tela`]); a própria Z-Wheel, reaberta, abre com a dela.
    /// Ela vem como `&mut` porque o quadro que a placa deixou pendente precisa entrar na tela da
    /// CPU antes de ser lido (ver [`Session::materializa_quadro_gl`]) — e isto roda com o
    /// contexto de GL dela corrente, que é o que as duas janelas já garantem ao abrir um jogo.
    pub fn abre(
        caminho: &Path,
        abertura: Abertura<'_>,
        mut anterior: Option<&mut Partida>,
    ) -> Result<Self, StartError> {
        let settings = abertura.settings;
        // A gravação começa antes da sessão: o que dá errado no carregamento é das coisas que
        // mais se quer ver. A anterior fecha primeiro, com o relatório dela, senão o começo deste
        // jogo cairia no arquivo do outro.
        if let Some(anterior) = anterior.as_deref_mut() {
            anterior.encerra_diario("a sessão terminou: outro jogo foi aberto");
        }
        let numero = match settings.debug.gravar_sessao {
            true => comeca_diario(caminho, &abertura),
            false => None,
        };
        // O banco é aberto quando a máquina nasce: escolhido agora, vale para este jogo.
        crate::audio::soundfont::define_banco(settings.audio.soundfont.clone());
        crate::audio::soundfont::define_efeitos(settings.audio.midi_effects);
        let tela_anterior = anterior
            .map(Partida::sessao_mut)
            .filter(|sessao| sessao.classe() == Z_WHEEL)
            .map(|sessao| {
                sessao.materializa_quadro_gl();
                sessao.screen().to_rgb565_bytes()
            });
        let portas = std::array::from_fn(|porta| {
            settings
                .controls
                .player(porta)
                .filter(|jogador| jogador.ligada)
                .map(|jogador| jogador.aparelho)
        });
        let sessao = Session::start_with(
            caminho,
            portas,
            abertura.serial,
            settings.graphics.gpu_rasterizer,
            abertura.gl,
            settings.z_wheel,
        );
        let mut sessao = match sessao {
            Ok(sessao) => sessao,
            Err(erro) => {
                if let Some(numero) = numero {
                    crate::registro::escreve(
                        crate::registro::Nivel::Erro,
                        "sessao",
                        &format!("o jogo não abriu: {erro}"),
                    );
                    crate::registro::para_de_gravar(numero, "sessão encerrada: o jogo não abriu");
                }
                return Err(erro);
            }
        };
        if numero.is_some() {
            crate::registro::escreve(
                crate::registro::Nivel::Informacao,
                "sessao",
                &format!(
                    "aberto: \"{}\", applet {:#010x}",
                    crate::library::sem_impressao_digital(sessao.title()),
                    sessao.classe()
                ),
            );
        }
        sessao.define_resolucao_interna(settings.graphics.resolucao_interna as usize);
        sessao.define_proporcao(settings.graphics.proporcao.aspecto(16.0 / 9.0));
        sessao.define_melhorias(
            settings.graphics.antialias as usize,
            settings.graphics.anisotropico as usize,
        );
        sessao.define_neblina(settings.graphics.neblina);
        if let Some(tela) = tela_anterior.filter(|_| sessao.classe() != Z_WHEEL) {
            sessao.herda_tela(&tela);
        }
        sessao.set_installed_applets(abertura.instalados);
        // A janela transforma o controle em tecla BREW (ver [`Partida::avanca`]): o turbo tem de
        // pulsar a tecla também.
        sessao.turbo_nas_teclas(true);
        // Ligar o som aqui é seguro **porque o jogo ainda não começou**: o `start` só prepara, e
        // o `EVT_APP_START` sai na primeira volta do laço. Antes disso o jogo já tocava dentro do
        // `start`, e o som saía com a tela vazia. Sem a feature `audio` — o core Libretro, que
        // entrega o som ao frontend dele — a sessão não tem saída de som para ligar.
        #[cfg(feature = "audio")]
        if let Some(erro) = sessao.set_audio(settings.audio.enabled, settings.audio.volume) {
            eprintln!("sem som: {erro}");
        }
        Ok(Self {
            sessao,
            pad_anterior: Default::default(),
            teclas_entregues: HashSet::new(),
            teclas_pendentes: Vec::new(),
            ultimo_passo: Instant::now(),
            pausada: false,
            aberta_pela_z_wheel: false,
            avanco: Interruptor::default(),
            avancando: false,
            turbos: Default::default(),
            rewind: ControleDoRewind::default(),
            diario: numero.map(|numero| Diario {
                numero,
                inicio: Instant::now(),
                proxima_saude: Instant::now() + INTERVALO_DA_SAUDE,
                parada_registrada: false,
                chamadas_antes: 0,
                saude_antes: Instant::now(),
            }),
        })
    }

    /// Fecha a gravação desta partida, com o relatório do emulador antes da última linha.
    ///
    /// O relatório vai no fim, e não aos poucos, porque é cumulativo: APIs que faltaram, acessos
    /// inválidos, o log agrupado do jogo. O retrato no fechamento é o que tem tudo.
    fn encerra_diario(&mut self, motivo: &str) {
        let Some(diario) = self.diario.take() else {
            return;
        };
        let relatorio = self.sessao.log();
        if !relatorio.is_empty() {
            crate::registro::grava_bloco(
                diario.numero,
                &format!(
                    "\n——— relatório do emulador no fim da sessão ———\n{}\n———\n\n",
                    relatorio.join("\n")
                ),
            );
        }
        let minutos = diario.inicio.elapsed().as_secs_f64() / 60.0;
        crate::registro::para_de_gravar(
            diario.numero,
            &format!("{motivo}, depois de {minutos:.1} min"),
        );
    }

    /// A linha de saúde da sessão gravada, e a parada do jogo quando ela acontece.
    fn acompanha_diario(&mut self) {
        let Some(diario) = self.diario.as_mut() else {
            return;
        };
        if !diario.parada_registrada {
            if let Some(motivo) = self.sessao.stopped_reason() {
                diario.parada_registrada = true;
                crate::registro::escreve(
                    crate::registro::Nivel::Erro,
                    "sessao",
                    &format!("o jogo parou: {motivo}"),
                );
            }
        }
        let agora = Instant::now();
        if agora < diario.proxima_saude {
            return;
        }
        diario.proxima_saude = agora + INTERVALO_DA_SAUDE;
        // Por segundo real, como as outras taxas da linha. O total vai junto porque foi ele, e
        // não a taxa, que esbarrava no teto antigo de 200 milhões (issue #70).
        let chamadas = self.sessao.api_calls();
        let janela = (agora - diario.saude_antes).as_secs_f64().max(0.001);
        let chamadas_por_segundo =
            chamadas.saturating_sub(diario.chamadas_antes) as f64 / janela;
        diario.chamadas_antes = chamadas;
        diario.saude_antes = agora;
        let amostra = self.sessao.sample();
        let heap = self.sessao.heap_retrato();
        let (_, objetos) = self.sessao.memory();
        let mb = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        let processo = match memoria_do_processo() {
            Some((residente, comprometida)) => format!(
                "processo {:.1} MB residentes e {:.1} MB comprometidos",
                mb(residente),
                mb(comprometida)
            ),
            None => "memória do processo indisponível neste sistema".to_string(),
        };
        crate::registro::escreve(
            crate::registro::Nivel::Informacao,
            "saude",
            &format!(
                "{} min de sessão, {:.1} s no relógio do jogo; velocidade {}%, {} quadros/s, \
                 {:.1} M instruções/s, {:.0} mil chamadas de API/s ({:.1} M no total); \
                 {processo}; heap do jogo {:.1} de {:.1} MB em {} blocos, \
                 maior buraco {:.1} MB; {objetos} objetos BREW vivos",
                diario.inicio.elapsed().as_secs() / 60,
                f64::from(self.sessao.clock_ms()) / 1000.0,
                amostra.speed,
                amostra.fps,
                amostra.ips as f64 / 1e6,
                chamadas_por_segundo / 1e3,
                chamadas as f64 / 1e6,
                mb(u64::from(heap.usado)),
                mb(u64::from(heap.teto)),
                heap.vivos,
                mb(u64::from(heap.maior_buraco)),
            ),
        );
    }

    /// Onde o relatório desta execução é gravado sozinho. Ver [`Relatorio`].
    pub fn caminho_do_relatorio(&self) -> PathBuf {
        let nome = match self.sessao.title() {
            titulo if !titulo.is_empty() => format!("{titulo}.log"),
            _ => "zeebx.log".to_string(),
        };
        crate::config::config_dir().join("relatorios").join(nome)
    }

    pub fn sessao(&self) -> &Session {
        &self.sessao
    }

    pub fn sessao_mut(&mut self) -> &mut Session {
        &mut self.sessao
    }

    /// Esquece o que a entrada tinha deixado: o controle anterior, as teclas entregues, a pausa.
    ///
    /// É o que abrir outro jogo fazia com o estado da janela antes de a partida existir, e continua
    /// valendo para a partida que fica quando a abertura da seguinte falha.
    pub fn esquece_entrada(&mut self) {
        self.pad_anterior = Default::default();
        self.teclas_entregues.clear();
        self.teclas_pendentes.clear();
        self.pausada = false;
        self.avanco.desliga();
        self.avancando = false;
        for turbo in &mut self.turbos {
            turbo.desliga();
        }
        self.rewind.solta();
    }

    /// Lê o atalho do rewind desta volta. Vem **antes** do [`Partida::le_o_avanco`] e do
    /// [`Partida::avanca`]: voltando, o jogo não anda e o fast-forward fica suspenso. Ver
    /// [`ControleDoRewind::le`].
    pub fn le_o_rewind(&mut self, apertado: bool, ajustes: &AjustesDoRewind) {
        let leitura = self.rewind.le(&mut self.sessao, apertado, self.pausada, ajustes);
        if leitura == Leitura::Soltou {
            self.reinicia_relogio();
        }
    }

    /// O que a janela escreve por cima do jogo enquanto ele volta: `Some(true)` quando não há mais
    /// ponto nenhum.
    pub fn indicador_do_rewind(&self) -> Option<bool> {
        self.rewind.indicador()
    }

    /// Esquece os pontos de retorno: um save state carregado é outra linha do tempo.
    pub fn esquece_os_pontos(&mut self) {
        self.rewind.esquece_os_pontos();
    }

    /// Se algum jogador está com o turbo de alternar ligado: a janela diz "Turbo" na tela.
    pub fn turbo_ligado(&self) -> bool {
        self.turbos.iter().any(TurboDaPorta::ligado)
    }

    /// Lê o atalho do fast-forward desta volta e diz se ele vale. Pausado, não vale: a pausa é
    /// pausa, e o alternar ligado fica esperando a volta do jogo.
    pub fn le_o_avanco(&mut self, apertado: bool, modo: ModoDoAtalho) -> bool {
        let ligado = self.avanco.atualiza(apertado, modo);
        self.avancando = ligado && !self.pausada && !self.rewind.voltando();
        self.avancando
    }

    /// O que a janela escreve por cima do jogo enquanto ele avança, ou nada. `virgula` é o
    /// separador decimal do idioma. Ver [`crate::velocidade::rotulo_do_avanco`].
    pub fn indicador_do_avanco(&self, avanco: &Avanco, virgula: bool) -> Option<String> {
        self.avancando.then(|| {
            let alcancada = self.sessao.sample().speed as f32 / 100.0;
            crate::velocidade::rotulo_do_avanco(avanco.proporcao(), alcancada, virgula)
        })
    }

    /// Recomeça a contagem de tempo real: o que passou até aqui não é para o jogo recuperar.
    pub fn reinicia_relogio(&mut self) {
        self.ultimo_passo = Instant::now();
    }

    /// Um evento de teclado: `teclado` são as teclas físicas apertadas agora, já em AVK.
    ///
    /// **Cada evento conta, e não só o estado no fim do quadro.** Um toque que começa e termina
    /// entre dois quadros precisa chegar ao jogo como aperto e soltura; olhando só o estado no
    /// quadro seguinte, ele sumiria. Os controles entram com o estado do quadro anterior, que é o
    /// que se sabe deles até a próxima leitura.
    pub fn teclado_mudou(&mut self, teclado: impl IntoIterator<Item = u32>) {
        if self.pausada {
            return;
        }
        let ativos = input::avks_ativos(teclado, &self.pad_anterior);
        let eventos = input::transicoes(&mut self.teclas_entregues, ativos);
        self.teclas_pendentes.extend(eventos);
    }

    /// Uma volta: entrega a entrada ao jogo e o faz andar o tempo real que passou.
    ///
    /// Pausada, não entrega nem anda — mas a [`Partida::saida`] continua valendo, porque um
    /// pedido de lançamento feito antes da pausa ainda precisa ser atendido.
    pub fn avanca(
        &mut self,
        pads: &[(usize, Pad)],
        movimentos: [[f32; 3]; PORTAS],
        teclado: impl IntoIterator<Item = u32>,
        ritmo: &Ritmo,
        turbo: &TurboDaVolta<'_>,
    ) {
        if self.pausada || self.rewind.voltando() {
            return;
        }
        self.pad_anterior = Default::default();
        for &(porta, pad) in pads {
            self.pad_anterior[porta] = pad;
        }
        let ativos = input::avks_ativos(teclado, &self.pad_anterior);
        let eventos = input::transicoes(&mut self.teclas_entregues, ativos);
        self.teclas_pendentes.extend(eventos);

        for &(porta, pad) in pads {
            self.sessao.set_port_pad(porta, pad);
        }
        // Quais botões pulsam sai daqui; quando eles estão apertados, da sessão, pelo relógio do
        // jogo. Uma porta sem controle nesta volta fica sem turbo.
        self.sessao.define_toques_do_turbo(turbo.toques_por_segundo);
        for porta in 0..PORTAS {
            let pad = pads.iter().find(|(p, _)| *p == porta).map(|(_, pad)| *pad);
            let jogador = turbo.controles.player(porta);
            let pulsando = match (pad, jogador) {
                (Some(pad), Some(jogador)) => self.turbos[porta].pulsando(
                    jogador.turbo,
                    &jogador.botao_do_turbo,
                    &pad,
                    turbo.apertado[porta],
                ),
                _ => 0,
            };
            self.sessao.define_turbo(porta, pulsando);
        }
        for (porta, movimento) in movimentos.into_iter().enumerate() {
            self.sessao.set_port_motion(porta, movimento);
        }
        for (avk, apertada) in self.teclas_pendentes.drain(..) {
            self.sessao.set_key(avk, apertada);
        }
        // O orçamento é o tempo real que passou desde o quadro anterior — e **não** uma fatia
        // fixa. Uma fatia de 16 ms virava teto de velocidade: com a janela sincronizada ao
        // monitor, bastava emulação mais desenho passarem de um retraço para o período dobrar
        // para 33 ms, e o jogo ficava com 16 de cada 33, travado em 50%. Era o que a tela de
        // seleção do Crash mostrava. O teto de `FATIA_MAXIMA` é só para o host que não dá conta.
        //
        // Com telas intermediárias à espera, uma vai à tela e o jogo não anda neste quadro.
        let agora = Instant::now();
        let fatia = (agora - self.ultimo_passo).min(FATIA_MAXIMA);
        self.ultimo_passo = agora;
        if !self.sessao.mostra_quadro_intermediario() {
            // O `egui` pintou no mesmo contexto desde o passo anterior.
            self.sessao.retoma_o_contexto();
            let _ = self.sessao.anda(fatia, ritmo);
            self.rewind.acompanha(&mut self.sessao);
        }
        self.acompanha_diario();
    }

    /// O ClassID que o jogo pediu para lançar, se pediu. Consome o pedido.
    ///
    /// **Olhar isto antes da [`Partida::saida`].** A Z-Wheel reaberta pede o jogo e sai na mesma
    /// volta; olhando a saída primeiro, a janela a reabria de novo e o jogo nunca abria.
    pub fn pedido_de_lancamento(&mut self) -> Option<u32> {
        self.sessao.take_launch_request()
    }

    /// O que fazer depois desta volta.
    ///
    /// A Z-Wheel sai sozinha para abrir o jogo escolhido: reabri-la é o papel do console. O mesmo
    /// quando o jogo que ela abriu fecha — pelo `ISHELL_CloseApplet` do menu dele, por exemplo: o
    /// console volta para a tela inicial. Aberto pela biblioteca, o jogo que sai sozinho fecha a
    /// janela dele. Uma falha não é saída: o jogo continua na tela, com o motivo.
    pub fn saida(&self) -> Saida {
        if !self.sessao.saiu_sozinho() {
            return Saida::Segue;
        }
        match self.sessao.classe() == Z_WHEEL || self.aberta_pela_z_wheel {
            true => Saida::ReabreZWheel,
            false => Saida::Fecha,
        }
    }
}

impl Drop for Partida {
    fn drop(&mut self) {
        self.encerra_diario("sessão encerrada normalmente");
    }
}

/// Abre o arquivo da sessão com o cabeçalho. `None` quando não deu: a gravação é instrumento, e
/// não abrir o arquivo não é motivo para não abrir o jogo.
fn comeca_diario(caminho: &Path, abertura: &Abertura<'_>) -> Option<u64> {
    let settings = abertura.settings;
    let inicio = std::time::SystemTime::now();
    let titulo = crate::library::sem_impressao_digital(&crate::library::title_for(caminho));
    let destino = caminho_da_sessao(&titulo, inicio);
    let id = crate::library::id_do_modulo(caminho).unwrap_or_else(|| "?".to_string());
    let applet = match crate::library::applet_clsid(caminho) {
        Some(classe) => format!("{classe:#010x}"),
        None => "na primeira linha depois de abrir".to_string(),
    };
    let graficos = &settings.graphics;
    let rasterizador = match (graficos.gpu_rasterizer, abertura.gl.is_some()) {
        (true, true) => "placa",
        (true, false) => "placa pedida, mas sem contexto de GL: software",
        (false, _) => "software",
    };
    let cabecalho = format!(
        "Zeebx — registro da sessão\n\
         Jogo:          {titulo}\n\
         ID:            módulo {id}, applet {applet}\n\
         Pacote:        {}\n\
         Início:        {}\n\
         Emulador:      Zeebx {} ({}/{})\n\
         Vídeo:         rasterizador {rasterizador}, resolução interna {}x, antialias {}x, \
         anisotrópico {}x\n\
         Áudio:         {}, volume {}\n\
         Registro:      deste arquivo, de INFO para cima; da janela de log, de {} para cima\n\
         Saúde:         uma linha `saude:` a cada {} s, e `placa:` a cada 300 quadros\n\
         Encerramento:  a última linha diz como a sessão terminou. Sem ela, o processo morreu\n\
         \x20              sem aviso: crash nativo, fechado à força ou queda de energia.\n\
         {}\n",
        caminho.display(),
        crate::registro::carimbo(inicio),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        graficos.resolucao_interna,
        graficos.antialias,
        graficos.anisotropico,
        match settings.audio.enabled {
            true => "ligado",
            false => "desligado",
        },
        settings.audio.volume,
        settings.debug.nivel_de_log,
        INTERVALO_DA_SAUDE.as_secs(),
        "=".repeat(96),
    );
    match crate::registro::grava_em(&destino, &cabecalho) {
        Ok(numero) => Some(numero),
        Err(erro) => {
            eprintln!("não deu para gravar a sessão em {}: {erro}", destino.display());
            None
        }
    }
}

/// A altura da tela do console, em pixels. A largura sai da proporção: 640 no 4:3.
const ALTURA_DA_TELA: f32 = 480.0;

/// O tamanho em que o quadro é desenhado dentro de `area`, que é o espaço livre da janela.
///
/// `aspecto` é largura sobre altura da imagem: 4:3 no nativo, mais larga no 16:9 experimental.
/// Separado das janelas porque é a única parte com regra de verdade, e a única que dá para
/// conferir sem abrir uma — e é a mesma no egui e no Qt.
pub fn enquadra(area: [f32; 2], escala: Scaling, manter_proporcao: bool, aspecto: f32) -> [f32; 2] {
    let nativo = [ALTURA_DA_TELA * aspecto, ALTURA_DA_TELA];
    if area[0] <= 0.0 || area[1] <= 0.0 {
        return nativo;
    }
    let vezes = |fator: f32| [nativo[0] * fator, nativo[1] * fator];
    let cabe = (area[0] / nativo[0]).min(area[1] / nativo[1]);
    match (escala, manter_proporcao) {
        (Scaling::Stretch, false) => area,
        (Scaling::Stretch, true) | (Scaling::Fit, _) => vezes(cabe),
        // Nunca some: abaixo de uma vez o tamanho original, encolhe proporcional em vez de não
        // caber, porque uma janela pequena não pode esconder o jogo.
        (Scaling::Integer, _) => match cabe >= 1.0 {
            true => vezes(cabe.floor()),
            false => vezes(cabe),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ampliacao_inteira_so_usa_multiplos_exatos() {
        // Numa janela de 1500x1100 cabem duas vezes a tela de 640x480, e não duas e pouco.
        let tamanho = enquadra([1500.0, 1100.0], Scaling::Integer, true, 4.0 / 3.0);
        assert_eq!(tamanho, [1280.0, 960.0]);
    }

    #[test]
    fn a_ampliacao_inteira_encolhe_quando_nao_cabe_uma_vez() {
        // Uma janela menor que a tela não pode esconder o jogo, então ali ela encolhe.
        let tamanho = enquadra([320.0, 240.0], Scaling::Integer, true, 4.0 / 3.0);
        assert_eq!(tamanho, [320.0, 240.0]);
    }

    #[test]
    fn caber_na_janela_mantem_a_proporcao() {
        // Janela larga demais: sobra borda dos lados, não estica.
        let tamanho = enquadra([1920.0, 480.0], Scaling::Fit, true, 4.0 / 3.0);
        assert_eq!(tamanho, [640.0, 480.0]);
    }

    #[test]
    fn preencher_so_deforma_quando_a_proporcao_e_dispensada() {
        let area = [1000.0, 500.0];
        assert_eq!(enquadra(area, Scaling::Stretch, false, 4.0 / 3.0), area);
        // Com a proporção mantida, "preencher" vira "caber".
        assert_eq!(
            enquadra(area, Scaling::Stretch, true, 4.0 / 3.0),
            enquadra(area, Scaling::Fit, true, 4.0 / 3.0)
        );
    }
}
