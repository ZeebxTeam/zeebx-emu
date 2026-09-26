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
        anterior: Option<&mut Partida>,
    ) -> Result<Self, StartError> {
        let settings = abertura.settings;
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
        let mut sessao = Session::start_with(
            caminho,
            portas,
            abertura.serial,
            settings.graphics.gpu_rasterizer,
            abertura.gl,
            settings.z_wheel,
        )?;
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
        })
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
        limite_de_velocidade: bool,
    ) {
        if self.pausada {
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
            let _ = self.sessao.step(fatia, limite_de_velocidade);
        }
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
