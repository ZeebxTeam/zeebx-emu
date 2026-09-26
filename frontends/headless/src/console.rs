//! O console rodando, sem nada em volta.
//!
//! É o que os dois modos — com janela e sem — têm em comum: a sessão, a entrada, o relógio e a
//! cadeia da Z-Wheel. O que muda entre eles é só para onde o quadro vai, e isso fica de fora
//! daqui de propósito: quem tem janela chama [`Console::passo`] e pinta; quem não tem chama o
//! mesmo [`Console::passo`] e escreve.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use zeebx::input::gamepads::Gamepads;
use zeebx::input::{self, Pad};
use zeebx::session::{FATIA_MAXIMA, Session, Step};
use zeebx::ui::library::{self, Game};
use zeebx::ui::settings::Settings;

use crate::config::Headless;
use crate::entrada::{self, Teclado};

/// Por que o laço acabou.
pub enum Fim {
    /// Continua.
    Segue,
    /// O jogo parou — terminou, saiu sozinho ou quebrou —, e não há para onde voltar.
    Acabou(String),
    /// Quebrou de um jeito que precisa ser dito.
    Erro(String),
}

pub struct Console {
    pub settings: Settings,
    pub opcoes: Headless,
    sessao: Option<Session>,
    /// O que a pasta de ROMs tem. É o que permite a Z-Wheel abrir um título pelo ClassID.
    jogos: Vec<Game>,
    /// Para onde o console volta quando um jogo sai. `None` faz o emulador sair junto.
    z_wheel: Option<PathBuf>,
    /// Se o que está rodando foi aberto pela Z-Wheel. Ver [`Console::passo`].
    pela_z_wheel: bool,
    /// O contexto de GL que a sessão usa para rasterizar o 3D. Com janela é o dela; sem janela,
    /// o de fora de tela do núcleo.
    gl: Option<Arc<glow::Context>>,
    pub teclado: Teclado,
    pads: Gamepads,
    /// O estado de cada porta no quadro anterior, para saber o que mudou.
    anteriores: [Pad; input::PORTAS],
    /// As teclas do BREW já entregues, para mandar só as transições.
    entregues: HashSet<u32>,
    ultimo: Instant,
    /// Se a última volta encontrou o jogo adiantado. Quem gira o laço usa isto para dormir um
    /// instante em vez de voltar já: sem janela, ou com o vsync desligado, nada mais segura a
    /// volta e o laço queimaria um núcleo inteiro esperando o relógio.
    adiantado: bool,
    /// Quando parar, em milissegundos do relógio do jogo. `None` roda até fecharem.
    limite_de_relogio: Option<u32>,
}

impl Console {
    /// Monta o console. A sessão ainda não começa: com janela, ela precisa do contexto de GL
    /// que só existe depois que a janela abre.
    pub fn novo(settings: Settings, opcoes: Headless) -> Self {
        let jogos = match &opcoes.roms {
            Some(pasta) => library::scan(pasta),
            None => Vec::new(),
        };
        // **Só a Z-Wheel que o arquivo apontar.** O núcleo sabe procurar uma sozinho — pela
        // pasta de ROMs e, na falta dela, por nome dentro da pasta pessoal —, e isso faz sentido
        // na interface, onde é um chute simpático para quem ainda não configurou nada. Num
        // binário sem interface não faz: ele seria chamado por outro programa e abriria algo que
        // ninguém pediu, achado vasculhando a casa de quem rodou. O `z_wheel_em` que fica aqui
        // não é procura, é resolução — ele transforma o `.zip`, o `.mod` ou a pasta que a pessoa
        // apontou no caminho que se abre.
        let z_wheel = opcoes.z_wheel.as_deref().and_then(library::z_wheel_em);
        Self {
            limite_de_relogio: opcoes.segundos.map(|s| s.saturating_mul(1000)),
            settings,
            opcoes,
            sessao: None,
            jogos,
            z_wheel,
            pela_z_wheel: false,
            gl: None,
            teclado: Teclado::default(),
            pads: Gamepads::new(),
            anteriores: Default::default(),
            entregues: HashSet::new(),
            ultimo: Instant::now(),
            adiantado: false,
        }
    }

    /// Se o jogo está adiantado em relação ao relógio do mundo.
    pub fn adiantado(&self) -> bool {
        self.adiantado
    }

    /// O contexto em que o 3D vai ser rasterizado. Dito antes de abrir o jogo.
    pub fn usa_contexto(&mut self, gl: Option<Arc<glow::Context>>) {
        self.gl = gl;
    }

    pub fn sessao_mut(&mut self) -> Option<&mut Session> {
        self.sessao.as_mut()
    }

    /// Abre um jogo. Um que já estava rodando é largado — é o que o console faz.
    pub fn abre(&mut self, caminho: &Path) -> Result<(), String> {
        // A tela que a Z-Wheel deixou continua valendo enquanto o jogo não desenha a primeira
        // dele: no console a troca não passa por um quadro preto.
        // O quadro pendente da sessão anterior entra antes de ela ser largada: sem isto, um jogo
        // aberto logo depois de a Z-Wheel desenhar herdaria a tela de dois quadros atrás.
        if let Some(anterior) = self.sessao.as_mut() {
            anterior.materializa_quadro_gl();
        }
        let tela_anterior = self
            .sessao
            .as_ref()
            .filter(|anterior| anterior.classe() == zeebx::session::Z_WHEEL)
            .map(|anterior| anterior.screen().to_rgb565_bytes());

        let portas = std::array::from_fn(|porta| {
            self.settings
                .controls
                .player(porta)
                .filter(|jogador| jogador.ligada)
                .map(|jogador| jogador.aparelho)
        });
        let g = &self.settings.graphics;
        let mut sessao = Session::start_with(
            caminho,
            portas,
            None,
            g.gpu_rasterizer,
            self.gl.clone(),
            self.settings.z_wheel,
        )
        .map_err(|erro| erro.to_string())?;

        sessao.define_resolucao_interna(g.resolucao_interna as usize);
        sessao.define_proporcao(g.proporcao.aspecto(16.0 / 9.0));
        sessao.define_melhorias(g.antialias as usize, g.anisotropico as usize);
        sessao.define_neblina(g.neblina);
        if let Some(tela) = tela_anterior.filter(|_| sessao.classe() != zeebx::session::Z_WHEEL) {
            sessao.herda_tela(&tela);
        }
        // O que a Z-Wheel enxerga instalado. Sem isto a lista dela vem vazia e não há o que abrir.
        sessao.set_installed_applets(
            self.jogos
                .iter()
                .filter_map(|jogo| Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?))),
        );
        // Ligar o som aqui é seguro **porque o jogo ainda não começou**: o `start` só prepara, e
        // o `EVT_APP_START` sai na primeira volta do laço.
        let audio = &self.settings.audio;
        if let Some(erro) = sessao.set_audio(audio.enabled, audio.volume) {
            eprintln!("warning: no sound: {erro}");
        }

        eprintln!("running: {}", sessao.title());
        self.sessao = Some(sessao);
        self.anteriores = Default::default();
        self.entregues.clear();
        self.teclado.solta_tudo();
        self.ultimo = Instant::now();
        Ok(())
    }

    /// Larga a sessão.
    ///
    /// Precisa acontecer **antes** de o contexto de GL morrer: o rasterizador guarda texturas e
    /// buffers criados nele, e soltá-los depois seria falar de objetos que já não existem.
    pub fn fecha(&mut self) {
        self.sessao = None;
    }

    /// Uma volta: lê a entrada, anda com o jogo, e trata o que ele pediu ao sair.
    pub fn passo(&mut self) -> Fim {
        if self.sessao.is_none() {
            return Fim::Acabou("no game".to_string());
        }

        self.pads.poll();
        // O estado de cada porta ligada, do mapeamento dela. Montado antes de pegar a sessão
        // emprestada: ele precisa das configurações, que moram no mesmo `self`.
        let portas: Vec<(usize, Pad)> = self
            .settings
            .controls
            .ligadas()
            .map(|(indice, jogador)| {
                (
                    indice,
                    entrada::pad_da_porta(jogador, &self.teclado, &self.pads, indice),
                )
            })
            .collect();

        // O console manda teclas do BREW além do estado do controle: é com elas que a Z-Wheel
        // navega. Só as transições vão, senão cada volta do laço repetiria o aperto.
        let mut atuais: HashSet<u32> = self
            .teclado
            .apertadas()
            .iter()
            .filter_map(|key| input::avk_de(*key))
            .collect();
        for (_, pad) in &portas {
            atuais.extend(
                input::teclas_do_controle(&Pad::default(), pad)
                    .into_iter()
                    .filter_map(|(avk, apertada)| apertada.then_some(avk)),
            );
        }
        let teclas = transicoes(&mut self.entregues, atuais);
        for (indice, pad) in &portas {
            self.anteriores[*indice] = *pad;
        }

        let limite = self.settings.graphics.speed_limit;
        let sessao = self.sessao.as_mut().expect("conferido acima");
        for (indice, pad) in portas {
            sessao.set_port_pad(indice, pad);
        }
        for (avk, apertada) in teclas {
            sessao.set_key(avk, apertada);
        }

        // O orçamento é o tempo real que passou desde a volta anterior, preso ao teto: é quanto
        // o jogo precisa emular para acompanhar o relógio do mundo.
        let agora = Instant::now();
        let fatia = (agora - self.ultimo).min(FATIA_MAXIMA);
        self.ultimo = agora;
        // Uma tela intermediária à espera vai à tela primeiro, e o jogo não anda nesta volta.
        self.adiantado = false;
        if !sessao.mostra_quadro_intermediario() {
            match sessao.step(fatia, limite) {
                Step::Stopped => return Fim::Acabou(sessao.stopped_reason().unwrap_or_default()),
                Step::Ahead => self.adiantado = true,
                Step::Presented | Step::Running => {}
            }
        }

        if let Some(teto) = self.limite_de_relogio {
            if sessao.clock_ms() >= teto {
                return Fim::Acabou("the requested time is up".to_string());
            }
        }

        // A Z-Wheel abre um jogo saindo e pedindo a classe dele. É o console que faz a troca.
        if let Some(classe) = sessao.take_launch_request() {
            let caminho = self
                .jogos
                .iter()
                .find(|jogo| jogo.clsid == Some(classe))
                .map(|jogo| jogo.path.clone());
            return match caminho {
                Some(caminho) => match self.abre(&caminho) {
                    Ok(()) => {
                        self.pela_z_wheel = true;
                        Fim::Segue
                    }
                    Err(erro) => Fim::Erro(format!("{}: {erro}", caminho.display())),
                },
                None => Fim::Erro(format!(
                    "the Z-Wheel asked for class {classe:#010x}, which is not in the ROMs folder"
                )),
            };
        }

        if sessao.saiu_sozinho() {
            // Um jogo que sai — pelo menu dele, pelo `ISHELL_CloseApplet` — devolve o console à
            // tela inicial, e a tela inicial do Zeebo é a Z-Wheel. Sem ela, ou com
            // `sair_com_o_jogo`, o emulador sai junto: é o que um frontend de fora espera, já
            // que a tela dele é que volta a aparecer.
            let volta = sessao.classe() == zeebx::session::Z_WHEEL || self.pela_z_wheel;
            let z_wheel = self.z_wheel.clone();
            if let (false, true, Some(caminho)) = (self.opcoes.sair_com_o_jogo, volta, z_wheel) {
                self.pela_z_wheel = false;
                return match self.abre(&caminho) {
                    Ok(()) => Fim::Segue,
                    Err(erro) => Fim::Erro(format!("{}: {erro}", caminho.display())),
                };
            }
            return Fim::Acabou("the game exited".to_string());
        }

        Fim::Segue
    }
}

/// O que mudou entre dois conjuntos de teclas: as que soltaram e as que apertaram.
fn transicoes(anteriores: &mut HashSet<u32>, atuais: HashSet<u32>) -> Vec<(u32, bool)> {
    let mut eventos: Vec<_> = anteriores
        .difference(&atuais)
        .map(|&avk| (avk, false))
        .collect();
    eventos.extend(atuais.difference(anteriores).map(|&avk| (avk, true)));
    eventos.sort_unstable();
    *anteriores = atuais;
    eventos
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Só as bordas vão: uma tecla que continua apertada não é repetida, e uma que soltou sai
    /// com `false`. Sem isso a Z-Wheel andaria uma casa por volta do laço.
    #[test]
    fn so_as_transicoes_sao_entregues() {
        let mut entregues = HashSet::new();
        assert_eq!(
            transicoes(&mut entregues, HashSet::from([input::avk::UP])),
            vec![(input::avk::UP, true)]
        );
        assert!(transicoes(&mut entregues, HashSet::from([input::avk::UP])).is_empty());
        assert_eq!(
            transicoes(&mut entregues, HashSet::new()),
            vec![(input::avk::UP, false)]
        );
    }
}
