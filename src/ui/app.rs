//! Interface desktop do Zeebx.

use super::{acervo, atualizacao, discord, gpu, library, settings};
mod vitrine;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use eframe::egui;

use crate::input::bindings::Source;
use crate::input::gamepads;
use crate::input::padview::PadArt;
use crate::input::{self, Pad};
use crate::loader::archive;
use crate::ponte;
use crate::session::Session;
use crate::ui::calibracao::{Calibracao, Situacao};
use crate::ui::entrada::{EntradaDoDesktop, SensorDaPorta};
use crate::ui::partida::{self, Abertura, Partida, Saida};
use crate::ui::i18n::Catalog;
use crate::ui::library::Game;
use crate::ui::settings::{Scaling, Settings};
use crate::video::display::Framebuffer;

/// A tela do Zeebo.
const SCREEN: [usize; 2] = [640, 480];

/// A imagem de quem não tem imagem nenhuma.
const PLACEHOLDER: &[u8] = include_bytes!("../../assets/zeebx.png");

/// Quantas leituras paradas a calibração do movimento junta: meio segundo a 100 por segundo.
const AMOSTRAS_DA_CALIBRACAO: usize = 50;

/// O Boomerang, para a prévia dos controles.
const BOOMERANG: &[u8] = include_bytes!("../../assets/boomerang.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    General,
    Controls,
    Graphics,
    Audio,
    Debug,
    About,
}

impl Tab {
    const ALL: [Self; 6] = [
        Self::General,
        Self::Controls,
        Self::Graphics,
        Self::Audio,
        Self::Debug,
        Self::About,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::General => "settings.tab.general",
            Self::Controls => "settings.tab.controls",
            Self::Graphics => "settings.tab.graphics",
            Self::Audio => "settings.tab.audio",
            Self::Debug => "settings.tab.debug",
            Self::About => "settings.tab.about",
        }
    }
}

/// O repositório do projeto.
const REPOSITORY: &str = "https://github.com/ZeebxTeam/zeebx-emu";
/// O convite do servidor de conversa.
const DISCORD: &str = "https://discord.gg/D96HjsKTPa";

pub struct App {
    /// Qual porta a tela de controles está editando. Ver [`crate::input::PORTAS`].
    porta_editada: usize,
    catalog: Catalog,
    settings: Settings,
    tab: Tab,
    games: Vec<Game>,
    /// Se a janela de configurações está aberta.
    settings_open: bool,
    /// Se a janela do gerenciador de saves está aberta.
    saves_open: bool,
    /// Os saves listados, relidos a cada abertura e a cada exclusão.
    saves: Vec<(bool, crate::ui::saves::Save)>,
    /// O save que espera confirmação para ser apagado.
    saves_confirmar: Option<usize>,
    /// O que dizer depois de apagar.
    saves_recado: Option<String>,
    /// O que dizer depois de mexer na trava diária do Zeeboids, se algo houver a dizer.
    sync_unlocked: Option<String>,
    /// O jogo em execução. Enquanto existe, ele tem uma janela só dele.
    partida: Option<Partida>,
    /// A textura em que o quadro do console é enviado para a placa de vídeo.
    frame: Option<egui::TextureHandle>,
    /// A tela que está na [`App::frame`]: série, escritas e o filtro. Igual, não há o que subir.
    frame_chave: Option<(u64, u64, bool)>,
    /// O que deu errado na última tentativa de abrir um jogo.
    error: Option<String>,
    /// O que abre a Z-Wheel: o configurado, ou a que estiver entre os jogos.
    z_wheel: Option<PathBuf>,
    /// Capas, nomes e descrições que a Z-Wheel traz dos jogos.
    acervo: Option<acervo::Acervo>,
    /// A escolha e a animação da biblioteca.
    vitrine: vitrine::Vitrine,
    teclado_apertado: HashSet<egui::Key>,
    /// A imagem do Boomerang na prévia dos controles.
    boomerang_textura: Option<egui::TextureHandle>,
    /// A calibração do movimento em andamento: a porta e as leituras juntadas até agora.
    calibrando: Option<(usize, Vec<[f32; 3]>)>,
    /// O pedido de permissão para os sensores de movimento, que corre numa thread enquanto a
    /// janela de senha do sistema está aberta.
    liberacao_dos_sensores: std::sync::Arc<std::sync::Mutex<LiberacaoDosSensores>>,
    /// A última calibração foi recusada por não estar de face para cima.
    calibracao_recusada: bool,
    /// O aviso de calibração aberto na janela do jogo, e as calibrações da sessão já vistas.
    calibracao: Calibracao,
    /// O que dizer sobre a última tentativa de exportar o log.
    log_status: Option<String>,
    /// A janela de log foi fechada nesta execução. Zera ao abrir outro jogo.
    log_dismissed: bool,
    /// A presença no Discord, acompanhando o que a janela mostra.
    presenca: discord::Acompanha,
    /// O resultado da última exportação de imagens para o Discord.
    discord_recado: Option<String>,
    /// A procura por versão nova em andamento, e o que ela respondeu.
    procura_de_atualizacao: Option<std::sync::mpsc::Receiver<atualizacao::Resposta>>,
    atualizacao: Option<atualizacao::Resposta>,
    /// O aviso de versão nova está na tela.
    aviso_de_atualizacao: bool,
    /// O aviso de abertura ainda está na tela.
    aviso_de_abertura: bool,
    /// A caixa "não mostrar de novo" do aviso de abertura.
    aviso_nao_mostrar: bool,
    /// O relatório da execução, gravado sozinho a cada poucos segundos. Ver
    /// [`partida::Relatorio`].
    relatorio: partida::Relatorio,
    /// Os controles do host, os Wii Remotes e os sensores de movimento.
    entrada: EntradaDoDesktop,
    /// Qual botão do Zeebo está esperando uma tecla, na tela de controles.
    capturing: Option<String>,
    /// O desenho do controle, e as texturas dele. Ficam vazios se o desenho não abrir: a tela
    /// de controles continua servindo pela lista.
    art: Option<PadArt>,
    art_textures: Option<ArtTextures>,
    /// A imagem de cada jogo já na placa de vídeo. `None` para o jogo cuja imagem não abriu —
    /// o cartão sai sem ela, e a tentativa não se repete a cada quadro.
    art_cache: HashMap<PathBuf, Option<egui::TextureHandle>>,
    /// A imagem que representa quem não tem nenhuma.
    placeholder: Option<crate::video::icon::Image>,
    /// O contexto de GL da janela, quando o `eframe` consegue um.
    ///
    /// Guardá-lo é o que permite pintar o quadro do console com GL do host em vez de mandá-lo
    /// como textura do egui. Sem ele — e o `eframe` admite não ter —, vale o caminho antigo.
    gl: Option<std::sync::Arc<glow::Context>>,
    /// O pintor de GL, montado na primeira vez que a janela do jogo desenha.
    ///
    /// Vive atrás de um `Mutex` porque o `egui_glow` exige um retorno de chamada `Sync`, e é
    /// dentro dele que a textura é atualizada.
    pintor: std::sync::Arc<std::sync::Mutex<Option<gpu::Pintor>>>,
    /// O pintor de GL não subiu, e daqui para a frente vale a textura do egui.
    ///
    /// Quem descobre isso é o retorno de pintura, já dentro do desenho do quadro — e um driver
    /// que recuse os shaders não pode custar a tela: sem esta marca, o quadro seguinte
    /// continuaria indo para um pintor que não existe, e a janela ficaria preta.
    gpu_falhou: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// O desenho do controle já na placa de vídeo.
struct ArtTextures {
    base: egui::TextureHandle,
    /// Uma silhueta por botão, na mesma ordem das peças do desenho.
    parts: Vec<egui::TextureHandle>,
}

impl App {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let mut catalog = Catalog::new(&settings::language_dirs());
        match &settings.language {
            Some(code) => {
                catalog.select(code);
            }
            None => {
                catalog.select_best(&crate::ui::i18n::system_language());
            }
        }
        // O tema escuro é o que se espera de um emulador, e deixa a imagem do jogo no centro
        // sem uma moldura clara puxando o olho.
        context.egui_ctx.set_theme(egui::Theme::Dark);
        // **O nível do registro antes de qualquer trabalho de abertura.** O `ZEEBX_LOG` vale como
        // ponto de partida e a configuração ganha dele quando existe, que é a mesma precedência
        // do core Libretro — um vocabulário, uma ordem.
        crate::registro::le_do_ambiente();
        match crate::registro::Ajuste::de_texto(&settings.debug.nivel_de_log) {
            Some(ajuste) => ajuste.aplica(),
            None => crate::registro!(
                crate::registro::Nivel::Aviso,
                "registro",
                "`{}` não é nível de log; seguindo no padrão",
                settings.debug.nivel_de_log
            ),
        }

        let games = settings
            .roms_dir
            .as_deref()
            .map(library::scan)
            .unwrap_or_default();
        if let Err(err) = library::sync_catalog(&games) {
            eprintln!("catálogo de jogos: {err}");
        }
        let aviso_de_abertura =
            settings.aviso_dispensado_na_versao.as_deref() != Some(env!("CARGO_PKG_VERSION"));
        let mut app = Self {
            catalog,
            settings,
            tab: Tab::General,
            games,
            settings_open: false,
            saves_open: false,
            saves: Vec::new(),
            saves_confirmar: None,
            saves_recado: None,
            sync_unlocked: None,
            partida: None,
            frame: None,
            frame_chave: None,
            error: None,
            z_wheel: None,
            acervo: None,
            vitrine: Default::default(),
            teclado_apertado: HashSet::new(),
            boomerang_textura: None,
            calibrando: None,
            liberacao_dos_sensores: Default::default(),
            calibracao_recusada: false,
            calibracao: Calibracao::default(),
            log_status: None,
            log_dismissed: false,
            aviso_de_abertura,
            procura_de_atualizacao: None,
            atualizacao: None,
            aviso_de_atualizacao: false,
            presenca: discord::Acompanha::default(),
            discord_recado: None,
            aviso_nao_mostrar: false,
            relatorio: partida::Relatorio::default(),
            entrada: EntradaDoDesktop::inicia(),
            porta_editada: 0,
            capturing: None,
            // Um desenho que não abre não pode impedir as configurações de abrir.
            art: PadArt::builtin()
                .inspect_err(|err| eprintln!("controle: {err}"))
                .ok(),
            art_textures: None,
            art_cache: HashMap::new(),
            placeholder: crate::video::icon::decode(PLACEHOLDER)
                .inspect_err(|err| eprintln!("imagem reserva: {err}"))
                .ok(),
            gl: context.gl.clone(),
            pintor: Default::default(),
            gpu_falhou: Default::default(),
        };
        app.atualiza_z_wheel();
        if app.settings.atualizacoes.ao_abrir {
            app.procura_de_atualizacao = Some(atualizacao::procura());
        }
        app
    }

    fn tr(&self, key: &str) -> String {
        self.catalog.get(key).to_string()
    }

    fn rescan(&mut self) {
        // As texturas antigas não servem à lista nova, e guardá-las seguraria a memória de
        // vídeo de jogos que saíram da pasta.
        self.art_cache.clear();
        self.games = self
            .settings
            .roms_dir
            .as_deref()
            .map(library::scan)
            .unwrap_or_default();
        if let Err(err) = library::sync_catalog(&self.games) {
            eprintln!("catálogo de jogos: {err}");
        }
        self.atualiza_z_wheel();
    }

    /// Resolve de onde abrir a Z-Wheel e relê o acervo dela.
    ///
    /// O caminho configurado vale primeiro; sem ele, uma Z-Wheel que esteja na pasta de ROMs
    /// serve do mesmo jeito. As texturas saem do cache porque a capa de cada jogo pode mudar.
    fn atualiza_z_wheel(&mut self) {
        self.z_wheel = library::z_wheel_de(self.settings.z_wheel_path.as_deref(), &self.games);
        self.acervo = self.z_wheel.as_deref().and_then(acervo::Acervo::carrega);
        self.art_cache.clear();
        self.vitrine.logos.clear();
        self.vitrine.classificacoes.clear();
    }

    /// O nome com que um jogo aparece: o oficial da Z-Wheel no idioma da interface, quando ela
    /// conhece o jogo, e o da pasta ou do pacote no resto.
    /// Recolhe a resposta da procura por versão nova e mostra o aviso quando há uma.
    ///
    /// O aviso espera o de abertura sair da frente: dois modais empilhados na partida escondem
    /// um atrás do outro.
    fn acompanha_atualizacao(&mut self, ctx: &egui::Context) {
        if let Some(canal) = &self.procura_de_atualizacao {
            match canal.try_recv() {
                Ok(resposta) => {
                    self.aviso_de_atualizacao = matches!(resposta, atualizacao::Resposta::Nova(_));
                    self.atualizacao = Some(resposta);
                    self.procura_de_atualizacao = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.procura_de_atualizacao = None;
                }
            }
        }
        if !self.aviso_de_atualizacao || self.aviso_de_abertura {
            return;
        }
        let Some(atualizacao::Resposta::Nova(lancamento)) = self.atualizacao.clone() else {
            self.aviso_de_atualizacao = false;
            return;
        };
        let mut fechar = false;
        let resposta = egui::Modal::new(egui::Id::new("aviso-de-atualizacao")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(self.catalog.get("update.title"));
            ui.add_space(8.0);
            ui.label(self.catalog.format(
                "update.available",
                &[("new", &lancamento.versao), ("current", atualizacao::VERSAO_ATUAL)],
            ));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button(self.catalog.get("update.download")).clicked() {
                    ctx.open_url(egui::OpenUrl::new_tab(&lancamento.pagina));
                    fechar = true;
                }
                if ui.button(self.catalog.get("update.later")).clicked() {
                    fechar = true;
                }
            });
        });
        if fechar || resposta.should_close() {
            self.aviso_de_atualizacao = false;
        }
    }

    fn secao_de_atualizacoes(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.label(self.tr("settings.updates"));
        ui.weak(self.catalog.format(
            "settings.updates.version",
            &[("version", atualizacao::VERSAO_ATUAL)],
        ));
        changed |= ui
            .checkbox(
                &mut self.settings.atualizacoes.ao_abrir,
                self.catalog.get("settings.updates.on_start"),
            )
            .changed();
        ui.horizontal(|ui| {
            let procurando = self.procura_de_atualizacao.is_some();
            if ui
                .add_enabled(!procurando, egui::Button::new(self.catalog.get("settings.updates.check")))
                .clicked()
            {
                self.atualizacao = None;
                self.procura_de_atualizacao = Some(atualizacao::procura());
            }
            let estado = match (&self.atualizacao, procurando) {
                (_, true) => self.catalog.get("settings.updates.checking").to_string(),
                (Some(atualizacao::Resposta::Nova(lancamento)), _) => self.catalog.format(
                    "settings.updates.new",
                    &[("version", &lancamento.versao)],
                ),
                (Some(atualizacao::Resposta::EmDia), _) => {
                    self.catalog.get("settings.updates.up_to_date").to_string()
                }
                (Some(atualizacao::Resposta::Falhou(motivo)), _) => self
                    .catalog
                    .format("settings.updates.failed", &[("reason", motivo)]),
                (None, false) => String::new(),
            };
            ui.label(estado);
            if let Some(atualizacao::Resposta::Nova(lancamento)) = &self.atualizacao {
                if ui.button(self.catalog.get("update.download")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&lancamento.pagina));
                }
            }
        });
        changed
    }

    /// Diz ao Discord o que está acontecendo. Ver [`discord::Acompanha`].
    fn atualiza_presenca(&mut self) {
        let classe = self.partida.as_ref().map(Partida::sessao).map(Session::classe);
        let titulo = self.titulo_do_jogo_aberto();
        let discord = &self.settings.discord;
        self.presenca
            .atualiza(discord.ativo, &self.catalog, classe, titulo, &discord.capas_url);
    }

    /// Grava o ícone e as capas no formato que o Developer Portal aceita, com o nome de arquivo
    /// igual à chave que a presença usa: é só arrastar a pasta para as Art Assets.
    ///
    /// Fora da tela por enquanto, junto com o endereço das capas: as imagens do aplicativo ainda
    /// vão ser decididas.
    #[allow(dead_code)]
    fn exporta_imagens_do_discord(&mut self) {
        let Some(pasta) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        let mut gravadas = 0usize;
        let mut erro = None;
        let mut grava = |nome: String, dados: Result<Vec<u8>, String>| {
            match dados.and_then(|d| std::fs::write(pasta.join(nome), d).map_err(|e| e.to_string())) {
                Ok(()) => gravadas += 1,
                Err(e) => erro = Some(e),
            }
        };
        grava(format!("{}.png", discord::CHAVE_DO_ICONE), Ok(PLACEHOLDER.to_vec()));
        for jogo in &self.games {
            let Some(classe) = jogo.clsid.filter(|&c| c != crate::session::Z_WHEEL) else {
                continue;
            };
            let capa = self
                .acervo
                .as_ref()
                .and_then(|acervo| acervo.ficha(classe))
                .and_then(|ficha| ficha.capa.as_ref())
                .or(jogo.art.as_ref());
            if let Some(capa) = capa {
                let chave = discord::chave_da_capa(classe);
                grava(format!("{chave}.png"), discord::png(&discord::capa_quadrada(capa)));
            }
        }
        self.discord_recado = Some(match erro {
            None => self.catalog.format(
                "settings.discord.exported",
                &[("count", &gravadas.to_string()), ("path", &pasta.display().to_string())],
            ),
            Some(motivo) => self
                .catalog
                .format("settings.discord.export_failed", &[("reason", &motivo)]),
        });
    }

    fn secao_do_discord(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.label(self.tr("settings.discord"));
        changed |= ui
            .checkbox(&mut self.settings.discord.ativo, self.catalog.get("settings.discord.on"))
            .changed();
        ui.add_enabled_ui(self.settings.discord.ativo, |ui| {
            let estado = match self.presenca.conectado() {
                true => "settings.discord.connected",
                false => "settings.discord.waiting",
            };
            ui.label(self.catalog.get(estado));
        });
        changed
    }

    /// O nome do jogo em execução, como a biblioteca o mostra.
    ///
    /// A sessão só conhece a pasta de extração, que leva a impressão digital do pacote
    /// (`Zeebo-Extreme-Boia-Cross-21503726-1788761080`). O título certo sai do jogo na
    /// biblioteca pelo ClassID — da Z-Wheel quando ela descreve o jogo, do pacote quando não.
    fn titulo_do_jogo_aberto(&self) -> Option<String> {
        let session = self.partida.as_ref().map(Partida::sessao)?;
        let classe = session.classe();
        if let Some(jogo) = self.games.iter().find(|jogo| jogo.clsid == Some(classe)) {
            return Some(self.titulo_de(jogo));
        }
        let titulo = library::sem_impressao_digital(session.title());
        (!titulo.is_empty()).then_some(titulo)
    }

    fn titulo_de(&self, jogo: &Game) -> String {
        acervo::titulo_de(self.acervo.as_ref(), jogo, self.catalog.current())
    }

    /// O que o console vê em cada porta, a partir do que está configurado.
    ///
    /// Uma porta desligada vira `None` e some da enumeração. É o que faz o jogo enxergar um
    /// controle, dois, ou um teclado — e é o mesmo caminho que responde ao
    /// `IHID::GetConnectedDevices`.
    fn portas_configuradas(
        &self,
    ) -> [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS] {
        std::array::from_fn(|porta| {
            self.settings
                .controls
                .player(porta)
                .filter(|jogador| jogador.ligada)
                .map(|jogador| jogador.aparelho)
        })
    }

    fn save(&self) {
        if let Err(err) = self.settings.save() {
            eprintln!("não deu para guardar as configurações: {err}");
        }
    }

    /// Abre um jogo escolhido na janela principal — na lista ou no botão da Z-Wheel.
    ///
    /// Não foi a Z-Wheel quem pediu: quando ele sair sozinho, a janela fecha em vez de voltar
    /// para ela.
    fn abre_pela_biblioteca(&mut self, path: PathBuf) {
        self.play(path);
        if let Some(partida) = self.partida.as_mut() {
            partida.aberta_pela_z_wheel = false;
        }
    }

    fn play(&mut self, path: PathBuf) {
        self.error = None;
        // As contagens de calibração são da sessão: a nova começa do zero.
        self.calibracao.reinicia();
        self.teclado_apertado.clear();
        if let Some(partida) = self.partida.as_mut() {
            partida.esquece_entrada();
        }
        self.frame = None;
        self.log_dismissed = false;
        self.relatorio.esquece();
        self.log_status = None;
        // A serial é ligada junto com o começo, e não depois: o construtor do applet roda
        // dentro do `start_with`, e o que ele faz ao nascer precisa estar na captura.
        let serial = self
            .settings
            .debug
            .log
            .then(|| partida::caminho_da_serial(&library::title_for(&path)));
        let abertura = Abertura {
            settings: &self.settings,
            serial: serial.as_deref(),
            gl: self.gl.clone(),
            instalados: self
                .games
                .iter()
                .filter_map(|game| Some((game.clsid?, library::id_do_modulo(&game.path)?)))
                .collect(),
        };
        // A partida anterior só sai quando a nova existe: é dela a tela que um jogo aberto pela
        // Z-Wheel herda, e uma abertura que falha deixa o jogo de antes rodando.
        match Partida::abre(&path, abertura, self.partida.as_mut()) {
            Ok(partida) => self.partida = Some(partida),
            Err(err) => {
                self.error = Some(
                    self.catalog
                        .format("play.failed", &[("reason", &err.to_string())]),
                );
            }
        }
    }
}

impl App {
    /// A barra de cima da janela principal.
    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Zeebx");
            let abrir = ui
                .add_enabled(
                    self.z_wheel.is_some(),
                    egui::Button::new(format!("▶ {}", self.catalog.get("nav.z_wheel"))),
                )
                .on_hover_text(self.catalog.get("nav.z_wheel.hint"))
                .on_disabled_hover_text(self.catalog.get("nav.z_wheel.missing"));
            if abrir.clicked() {
                if let Some(caminho) = self.z_wheel.clone() {
                    self.abre_pela_biblioteca(caminho);
                }
            }
            self.campo_da_busca(ui);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(self.catalog.get("nav.settings")).clicked() {
                    self.settings_open = true;
                }
                if ui.button(self.catalog.get("nav.saves")).clicked() {
                    self.recarrega_saves();
                    self.saves_open = true;
                }
                // O Zeeboids só deixa sincronizar uma vez por dia, e a trava é dele: guarda a
                // data no próprio banco e compara com a de hoje. Testar rede com isso custa um
                // dia por tentativa, então o botão recua a data em um dia.
                let botao = ui
                    .button(self.catalog.get("nav.unlock_sync"))
                    .on_hover_text(self.catalog.get("nav.unlock_sync.hint"));
                if botao.clicked() {
                    self.sync_unlocked = match ponte::liberar_sincronizacao(&archive::device_dir())
                    {
                        Ok(_) => Some(self.catalog.get("nav.unlock_sync.done").to_string()),
                        Err(erro) => Some(erro),
                    };
                }
            });
        });
        if let Some(recado) = self.sync_unlocked.clone() {
            ui.horizontal(|ui| {
                ui.label(recado);
                if ui.button(self.catalog.get("nav.unlock_sync.ok")).clicked() {
                    self.sync_unlocked = None;
                }
            });
        }
    }

    /// A busca da biblioteca. Ctrl+F leva o foco a ela e Esc a limpa; o Enter joga o jogo
    /// escolhido, que é o primeiro resultado enquanto ninguém mexe na escolha.
    fn campo_da_busca(&mut self, ui: &mut egui::Ui) {
        let campo = egui::TextEdit::singleline(&mut self.vitrine.busca)
            .hint_text(self.catalog.get("nav.search"))
            .desired_width(220.0);
        let resposta = ui
            .add(campo)
            .on_hover_text(self.catalog.get("nav.search.hint"));
        self.vitrine.campo_da_busca = Some(resposta.id);
        let mut mudou = resposta.changed();
        let (atalho, esc) = ui.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::F),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if atalho {
            resposta.request_focus();
        }
        // O Esc tira o foco do campo sozinho; aqui ele também apaga o que estava escrito.
        if esc && (resposta.has_focus() || resposta.lost_focus()) && !self.vitrine.busca.is_empty()
        {
            self.vitrine.busca.clear();
            mudou = true;
        }
        if !self.vitrine.busca.is_empty()
            && ui
                .small_button("✕")
                .on_hover_text(self.catalog.get("nav.search.clear"))
                .clicked()
        {
            self.vitrine.busca.clear();
            mudou = true;
        }
        if mudou {
            self.vitrine.busca_mudou();
        }
    }

    fn settings_screen(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for tab in Tab::ALL {
                let label = self.catalog.get(tab.key()).to_string();
                ui.selectable_value(&mut self.tab, tab, label);
            }
        });
        ui.separator();
        // Toda aba rola: a gráfica já não cabe na altura padrão da janela, e uma opção que some
        // embaixo da borda é uma opção que não existe.
        let changed = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.tab {
                Tab::General => self.general_tab(ui),
                Tab::Controls => self.controls_tab(ui),
                Tab::Graphics => self.graphics_tab(ui),
                Tab::Audio => self.audio_tab(ui),
                Tab::Debug => self.debug_tab(ui),
                Tab::About => self.about_tab(ui),
            })
            .inner;
        if changed {
            self.save();
            // Mexer nas portas com um jogo aberto vale na hora. Guardar e só aplicar na próxima
            // partida seria a configuração parecer que não pegou.
            let portas = self.portas_configuradas();
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.set_portas(portas);
            }
        }
    }

    /// Onde está a Z-Wheel: escolhida à mão ou detectada, e o que ela trouxe.
    fn secao_da_z_wheel(&mut self, ui: &mut egui::Ui) -> bool {
        let mut mudou = false;
        ui.label(self.tr("settings.z_wheel"));
        ui.weak(self.tr("settings.z_wheel.hint"));
        ui.horizontal_wrapped(|ui| {
            let mostrado = self
                .settings
                .z_wheel_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.monospace(mostrado);
            if ui.button(self.catalog.get("settings.browse")).clicked() {
                if let Some(arquivo) = rfd::FileDialog::new()
                    .add_filter("Z-Wheel", &["zip", "mod"])
                    .pick_file()
                {
                    self.settings.z_wheel_path = Some(arquivo);
                    mudou = true;
                }
            }
            if ui.button(self.catalog.get("settings.z_wheel.folder")).clicked() {
                if let Some(pasta) = rfd::FileDialog::new().pick_folder() {
                    self.settings.z_wheel_path = Some(pasta);
                    mudou = true;
                }
            }
            if ui.button(self.catalog.get("settings.z_wheel.detect")).clicked() {
                let achada = library::detecta_z_wheel(
                    self.settings.roms_dir.as_deref(),
                    &self.games,
                );
                if achada.is_some() {
                    self.settings.z_wheel_path = achada;
                    mudou = true;
                }
            }
        });
        if mudou {
            self.atualiza_z_wheel();
        }
        let aviso = match (&self.z_wheel, &self.acervo) {
            (Some(_), Some(acervo)) => self
                .catalog
                .format("settings.z_wheel.found", &[("count", &acervo.len().to_string())]),
            (Some(_), None) => self.tr("settings.z_wheel.found_no_art"),
            (None, _) => self.tr("settings.z_wheel.not_found"),
        };
        match self.z_wheel {
            Some(_) => ui.weak(aviso),
            None => ui.colored_label(ui.visuals().warn_fg_color, aviso),
        };
        ui.add_space(8.0);
        mudou
    }

    fn general_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        ui.label(self.tr("settings.roms_folder"));
        ui.weak(self.tr("settings.roms_folder.hint"));
        ui.horizontal(|ui| {
            let shown = self
                .settings
                .roms_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            ui.monospace(shown);
            if ui.button(self.catalog.get("settings.browse")).clicked() {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.settings.roms_dir = Some(dir);
                    changed = true;
                }
            }
        });
        if let Some(dir) = &self.settings.roms_dir {
            if !dir.is_dir() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    self.catalog.get("common.folder_missing"),
                );
            }
        }
        if changed {
            self.rescan();
        }

        ui.add_space(16.0);
        ui.label(self.tr("settings.language"));
        ui.weak(self.tr("settings.language.hint"));
        let current = self
            .catalog
            .languages()
            .iter()
            .find(|l| l.code == self.catalog.current())
            .map(|l| l.name.clone())
            .unwrap_or_default();
        let mut picked: Option<String> = None;
        egui::ComboBox::from_id_salt("language")
            .selected_text(current)
            .show_ui(ui, |ui| {
                for language in self.catalog.languages() {
                    let selected = language.code == self.catalog.current();
                    if ui.selectable_label(selected, &language.name).clicked() {
                        picked = Some(language.code.clone());
                    }
                }
            });
        if let Some(code) = picked {
            if self.catalog.select(&code) {
                self.settings.language = Some(code);
                changed = true;
            }
        }

        ui.add_space(16.0);
        ui.label(self.tr("settings.library_view"));
        ui.weak(self.tr("settings.library_view.hint"));
        ui.horizontal(|ui| {
            use crate::ui::settings::ModoDaBiblioteca;
            for (modo, chave) in [
                (ModoDaBiblioteca::Grade, "settings.library_view.grid"),
                (ModoDaBiblioteca::Slider, "settings.library_view.slider"),
            ] {
                changed |= ui
                    .selectable_value(&mut self.settings.biblioteca, modo, self.catalog.get(chave))
                    .changed();
            }
        });

        ui.add_space(16.0);
        changed |= self.secao_da_z_wheel(ui);
        changed |= ui
            .checkbox(
                &mut self.settings.z_wheel.fim_de_vida,
                self.catalog.get("settings.z_wheel_eol"),
            )
            .changed();
        ui.weak(self.tr("settings.z_wheel_eol.hint"));
        changed |= ui
            .checkbox(
                &mut self.settings.z_wheel.transicoes_sempre,
                self.catalog.get("settings.z_wheel_transitions"),
            )
            .changed();
        ui.weak(self.tr("settings.z_wheel_transitions.hint"));

        ui.add_space(16.0);
        changed |= self.secao_do_discord(ui);

        ui.add_space(16.0);
        changed |= self.secao_de_atualizacoes(ui);

        ui.add_space(16.0);
        ui.weak(self.catalog.format(
            "settings.saved_at",
            &[("path", &settings::settings_path().display().to_string())],
        ));
        changed
    }

    /// A escolha da porta, se ela está ligada e o que o console vê nela.
    ///
    /// As duas USB do console são portas de verdade aqui: cada uma tem o seu mapeamento e o seu
    /// aparelho, e o que está ligado é o que o `GetConnectedDevices` enumera.
    fn port_picker(&mut self, ui: &mut egui::Ui) -> bool {
        use crate::input::bindings::Aparelho;

        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label(self.catalog.get("controls.port"));
            for porta in 0..crate::input::PORTAS {
                let rotulo = self
                    .catalog
                    .get("controls.port.n")
                    .replace("{n}", &(porta + 1).to_string());
                if ui
                    .selectable_label(self.porta_editada == porta, rotulo)
                    .clicked()
                {
                    self.porta_editada = porta;
                    // Deixar a captura aberta ao trocar de porta mapearia a próxima tecla no
                    // botão da porta anterior.
                    self.capturing = None;
                }
            }
        });

        let porta = self.porta_editada;
        let jogador = self.settings.controls.player_mut(porta);
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut jogador.ligada, self.catalog.get("controls.port.on"))
                .changed()
            {
                changed = true;
            }
            ui.add_enabled_ui(jogador.ligada, |ui| {
                ui.separator();
                ui.label(self.catalog.get("controls.kind"));
                for (aparelho, chave) in [
                    (Aparelho::ZPad, "controls.kind.zpad"),
                    (Aparelho::Controle, "controls.kind.dragon"),
                    (Aparelho::Boomerang, "controls.kind.boomerang"),
                    (Aparelho::Teclado, "controls.kind.keyboard"),
                ] {
                    if ui
                        .selectable_label(jogador.aparelho == aparelho, self.catalog.get(chave))
                        .clicked()
                        && jogador.aparelho != aparelho
                    {
                        jogador.aparelho = aparelho;
                        changed = true;
                    }
                }
            });
        });
        match self.settings.controls.player_mut(porta).aparelho {
            Aparelho::Teclado => {
                ui.weak(self.catalog.get("controls.kind.hint"));
            }
            Aparelho::Boomerang => {
                ui.weak(self.catalog.get("controls.kind.boomerang.hint"));
            }
            Aparelho::Controle | Aparelho::ZPad => {
                ui.weak(self.catalog.get("controls.kind.pad.hint"));
            }
        }
        ui.add_space(8.0);
        changed
    }

    /// A prévia do Boomerang: a imagem inclina com o Wii Remote, e embaixo ficam o que está
    /// apertado, a aceleração e a calibração.
    ///
    /// O desenho é a face do controle, e é assim que o jogador o vê segurando como volante: a
    /// imagem gira pelo ângulo que o Crash Nitro Kart usa para virar. Sai da gravidade e só vale
    /// com o controle quase parado — é o mesmo que os jogos fazem.
    fn boomerang_view(&mut self, ui: &mut egui::Ui) {
        self.entrada.gamepads.poll();
        ui.ctx().request_repaint();
        let porta = self.porta_editada;
        let pad = self.pad_of(ui.ctx(), porta);
        let sensor = self.sensor_da_porta(porta);
        let com_leitura = sensor.aceleracao().is_some();
        let [x, y, z] = self.movimento_da_porta(porta);
        // A calibração em andamento junta leituras paradas e fecha na média.
        let bruto_agora = self.movimento_bruto_da_porta(porta);
        if let Some((de, amostras)) = &mut self.calibrando
            && *de == porta
            && let Some(bruto) = bruto_agora
        {
            amostras.push(bruto);
            if amostras.len() >= AMOSTRAS_DA_CALIBRACAO {
                let n = amostras.len() as f32;
                let media: [f32; 3] =
                    std::array::from_fn(|i| amostras.iter().map(|a| a[i]).sum::<f32>() / n);
                match crate::input::bindings::CalibracaoDeMovimento::de_repouso(media) {
                    Some(calibracao) => {
                        self.settings.controls.player_mut(porta).calibracao_movimento = calibracao;
                        self.calibracao_recusada = false;
                        self.save();
                    }
                    None => self.calibracao_recusada = true,
                }
                self.calibrando = None;
            }
        }
        let textura = self.textura_do_boomerang(ui.ctx());
        // O ângulo de volante, o mesmo que o Crash Nitro Kart calcula: a gravidade no plano da
        // face. Deitado, ela sai desse plano e o ângulo vira ruído, então a imagem só gira quando
        // a gravidade está de fato ali.
        let no_plano = (x * x + y * y).sqrt();
        let volante = x.atan2(y);
        let giro = volante * ((no_plano - 0.3) / 0.4).clamp(0.0, 1.0);
        let calibrando = self.calibrando.is_some();
        let mut calibrar = false;
        let mut restaurar = false;
        ui.vertical_centered(|ui| {
            let largura = ui.available_width().min(360.0);
            let fonte = textura.size_vec2();
            let altura = largura * fonte.y / fonte.x;
            let (area, _) =
                ui.allocate_exact_size(egui::vec2(largura, largura * 0.9), egui::Sense::hover());
            egui::Image::new(&textura)
                .rotate(giro, egui::Vec2::splat(0.5))
                .paint_at(
                    ui,
                    egui::Rect::from_center_size(area.center(), egui::vec2(largura, altura)),
                );

            let nomes: Vec<&str> = ["up", "down", "left", "right", "b1", "b2", "back"]
                .into_iter()
                .filter(|nome| Pad::button_by_name(nome).is_some_and(|i| pad.is_down(i)))
                .collect();
            ui.label(self.descreve_sensor(&sensor));
            // O caso mais comum de um controle com sensor que não mexe o Boomerang: o nó do
            // sensor sem permissão. Um clique grava a regra que resolve, pedindo a senha.
            if matches!(&sensor, SensorDaPorta::Controle(s) if s.sem_permissao) {
                self.oferece_liberar_sensores(ui);
            }
            ui.monospace(format!(
                "x {x:+.2}  y {y:+.2}  z {z:+.2} g   {}  {}",
                format!("⟲ {:+.0}°", volante.to_degrees()),
                nomes.join(" ")
            ));
            ui.horizontal(|ui| {
                ui.add_enabled_ui(com_leitura && !calibrando, |ui| {
                    calibrar = ui.button(self.catalog.get("controls.boomerang.calibrate")).clicked();
                });
                restaurar = ui.button(self.catalog.get("controls.boomerang.calibrate_reset")).clicked();
            });
            if self.calibracao_recusada && !calibrando {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    self.catalog.get("controls.boomerang.calibrate_refused"),
                );
            }
            ui.weak(match calibrando {
                true => self.tr("controls.boomerang.calibrating"),
                false => self.tr("controls.boomerang.calibrate_hint"),
            });
        });
        if calibrar {
            self.calibrando = Some((porta, Vec::new()));
        }
        if restaurar {
            self.calibrando = None;
            self.settings.controls.player_mut(porta).calibracao_movimento = Default::default();
            self.save();
        }
    }

    /// A imagem do Boomerang, subida uma vez e usada na prévia e no aviso de calibração.
    fn textura_do_boomerang(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.boomerang_textura
            .get_or_insert_with(|| {
                let imagem = crate::video::icon::decode(BOOMERANG)
                    .map(|imagem| imagem.downscaled(480))
                    .unwrap_or(crate::video::icon::Image {
                        width: 1,
                        height: 1,
                        rgba: vec![0; 4],
                    });
                ctx.load_texture(
                    "boomerang",
                    egui::ColorImage::from_rgba_unmultiplied(
                        [imagem.width, imagem.height],
                        &imagem.rgba,
                    ),
                    egui::TextureOptions::LINEAR,
                )
            })
            .clone()
    }

    /// O aviso de calibração no canto de baixo à direita da janela do jogo.
    ///
    /// Abre quando a sessão diz que o jogo começou uma calibração
    /// ([`Session::calibracao`]) e mostra o Boomerang inclinando com o Wii Remote e se ele está
    /// parado. Fecha quando o jogo diz que terminou: aí o modelo anima até ficar reto e o aviso
    /// some sozinho logo depois.
    fn aviso_de_calibracao(&mut self, ctx: &egui::Context, calibracao: (u32, u32)) {
        use crate::input::bindings::Aparelho;
        let porta = self
            .settings
            .controls
            .ligadas()
            .find(|(_, jogador)| jogador.aparelho == Aparelho::Boomerang)
            .map(|(indice, _)| indice);
        let leitura = porta.map(|porta| {
            (self.movimento_da_porta(porta), self.movimento_bruto_da_porta(porta).is_some())
        });
        let ligado = self.settings.movimento.aviso_de_calibracao;
        let agora = std::time::Instant::now();
        let (Some(porta), Some(vista)) =
            (porta, self.calibracao.quadro(calibracao, leitura, ligado, agora))
        else {
            return;
        };
        let giro = vista.giro;
        let titulo = match vista.concluido {
            true => self.tr("calibration.toast.done"),
            false => self.tr("calibration.toast.title"),
        };
        let parado = vista.situacao == Some(Situacao::Parado);
        let estado = match vista.situacao {
            None => String::new(),
            Some(Situacao::SemSensor) => self.descreve_sensor(&self.sensor_da_porta(porta)),
            Some(Situacao::Parado) => self.tr("calibration.toast.still"),
            Some(Situacao::Mexendo) => self.tr("calibration.toast.moving"),
        };
        let textura = self.textura_do_boomerang(ctx);
        egui::Area::new(egui::Id::new("aviso-de-calibracao"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let largura = 110.0;
                        let fonte = textura.size_vec2();
                        let (area, _) = ui.allocate_exact_size(
                            egui::vec2(largura, largura * 0.7),
                            egui::Sense::hover(),
                        );
                        egui::Image::new(&textura).rotate(giro, egui::Vec2::splat(0.5)).paint_at(
                            ui,
                            egui::Rect::from_center_size(
                                area.center(),
                                egui::vec2(largura, largura * fonte.y / fonte.x),
                            ),
                        );
                        ui.vertical(|ui| {
                            ui.strong(titulo);
                            if !estado.is_empty() {
                                let cor = match parado {
                                    true => ui.visuals().text_color(),
                                    false => ui.visuals().warn_fg_color,
                                };
                                ui.colored_label(cor, estado);
                            }
                        });
                    });
                });
            });
        ctx.request_repaint();
    }

    /// O desenho do controle. Devolve o botão clicado.
    ///
    /// Ele acende o que está apertado agora, e é por isso que vale mais que a lista: um
    /// direcional que fica aceso sem ninguém encostar no controle mostra na hora um problema
    /// que a lista de texto esconderia.
    fn controller_view(&mut self, ui: &mut egui::Ui) -> Option<String> {
        // **O gilrs só atualiza o estado quando a fila de eventos é drenada**, e aqui isso não
        // acontecia: o `poll` estava só no botão de procurar controles. O desenho ficava apagado
        // por mais que se apertasse, e o jogo — que consulta a cada quadro — era o único lugar
        // onde acender funcionava.
        self.entrada.gamepads.poll();
        // E o egui só repinta quando tem evento **dele**. Aperto de controle não é: sem este
        // pedido, a arte só mudaria quando o mouse se mexesse por cima dela.
        ui.ctx().request_repaint();
        let pad = self.pad_of(ui.ctx(), self.porta_editada);
        let capturing = self.capturing.clone();
        // Os campos saem separados porque as texturas são criadas a partir do desenho, e pedir
        // os dois pelo `self` de uma vez seria um empréstimo mutável em cima de um imutável.
        let Self {
            art, art_textures, ..
        } = self;
        let art = art.as_ref()?;
        let textures = art_textures.get_or_insert_with(|| upload_art(ui.ctx(), art));
        draw_controller(ui, art, textures, &pad, capturing.as_deref())
    }

    fn controls_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        changed |= self.port_picker(ui);
        ui.weak(self.catalog.get("controls.hint"));
        ui.add_space(8.0);

        // Uma porta livre não tem o que configurar, e mostrar o desenho do controle nela seria
        // convidar a mapear um aparelho que o console não vai enumerar.
        if !self
            .settings
            .controls
            .player(self.porta_editada)
            .is_some_and(|jogador| jogador.ligada)
        {
            ui.weak(self.catalog.get("controls.port.off_hint"));
            return changed;
        }

        let e_boomerang = self
            .settings
            .controls
            .player(self.porta_editada)
            .is_some_and(|jogador| jogador.aparelho == crate::input::bindings::Aparelho::Boomerang);
        if e_boomerang {
            changed |= ui
                .checkbox(
                    &mut self.settings.movimento.aviso_de_calibracao,
                    self.catalog.get("calibration.toast.setting"),
                )
                .changed();
            self.boomerang_view(ui);
        } else if let Some(button) = self.controller_view(ui) {
            // Clicar na peça é o mesmo que clicar em "Atribuir" na linha dela.
            self.capturing = match self.capturing.as_deref() == Some(button.as_str()) {
                true => None,
                false => Some(button),
            };
        }
        if self.art.is_some() {
            ui.vertical_centered(|ui| ui.weak(self.catalog.get("controls.art_hint")));
            // O botão de um controle não gera evento no egui: sem redesenhar sozinha, a tela
            // só acenderia quando o mouse passasse por cima.
            ui.ctx().request_repaint();
        }
        ui.add_space(8.0);

        // Escolha do controle. O teclado nunca sai: quem liga um controle continua podendo
        // usar as teclas, e é o que se espera de um emulador.
        let devices = self.entrada.gamepads.names();
        ui.horizontal(|ui| {
            ui.label(self.catalog.get("controls.device"));
            let current = self
                .settings
                .controls
                .player(self.porta_editada)
                .and_then(|player| player.device.clone())
                .unwrap_or_else(|| self.catalog.get("controls.device.none").to_string());
            let mut chosen: Option<Option<String>> = None;
            egui::ComboBox::from_id_salt("controle")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(false, self.catalog.get("controls.device.none"))
                        .clicked()
                    {
                        chosen = Some(None);
                    }
                    for device in &devices {
                        if ui.selectable_label(false, device).clicked() {
                            chosen = Some(Some(device.clone()));
                        }
                    }
                    for indice in 0..self.entrada.wiimotes.quantos() {
                        let nome = crate::input::wiimote::Wiimotes::nome(indice);
                        if ui.selectable_label(false, &nome).clicked() {
                            chosen = Some(Some(nome));
                        }
                    }
                });
            if let Some(device) = chosen {
                self.settings
                    .controls
                    .player_mut(self.porta_editada)
                    .troca_controle(device);
                changed = true;
            }
            if ui.button(self.catalog.get("controls.rescan")).clicked() {
                self.entrada.gamepads.poll();
            }
        });
        if devices.is_empty() {
            ui.weak(self.catalog.get("controls.no_devices"));
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(self.catalog.get("controls.reset")).clicked() {
                let device = self
                    .settings
                    .controls
                    .player(self.porta_editada)
                    .and_then(|player| player.device.clone());
                *self.settings.controls.player_mut(self.porta_editada) =
                    crate::input::bindings::Player::padrao_do_controle(device);
                changed = true;
            }
        });
        ui.separator();

        // A captura sai do laço: mexer no mapeamento enquanto ele está emprestado para desenhar
        // não passa pelo compilador.
        let mut assign: Option<String> = None;
        let mut clear: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("botoes").num_columns(3).show(ui, |ui| {
                for button in crate::input::bindings::CONFIGURABLE {
                    ui.label(self.catalog.get(&format!("button.{button}")));

                    let sources = self
                        .settings
                        .controls
                        .player(self.porta_editada)
                        .map(|player| player.sources(button))
                        .unwrap_or(&[]);
                    let text = match sources.is_empty() {
                        true => self.catalog.get("controls.unbound").to_string(),
                        false => sources
                            .iter()
                            .map(Source::label)
                            .collect::<Vec<_>>()
                            .join(", "),
                    };
                    ui.label(text);

                    ui.horizontal(|ui| {
                        let waiting = self.capturing.as_deref() == Some(button);
                        let label = match waiting {
                            true => self.catalog.get("controls.assigning"),
                            false => self.catalog.get("controls.assign"),
                        };
                        if ui.selectable_label(waiting, label).clicked() {
                            assign = Some(button.to_string());
                        }
                        if ui.button(self.catalog.get("controls.clear")).clicked() {
                            clear = Some(button.to_string());
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(button) = assign {
            // Clicar de novo no mesmo botão desiste da captura.
            self.capturing = match self.capturing.as_deref() == Some(button.as_str()) {
                true => None,
                false => Some(button),
            };
        }
        if let Some(button) = clear {
            self.settings
                .controls
                .player_mut(self.porta_editada)
                .clear(&button);
            changed = true;
        }

        ui.add_space(12.0);
        changed |= self.axes_section(ui);

        ui.add_space(12.0);
        ui.weak(self.catalog.get("controls.players_note"));
        changed
    }

    /// De onde vem cada eixo analógico do console.
    ///
    /// Fica numa seção própria, e não junto dos botões, porque um eixo não é um botão: ele não
    /// tem "apertado", tem curso, e por isso a origem é uma só e ganha um sentido.
    fn axes_section(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;

        // O direcional espelhado nos eixos — o pedido do issue #39, e o mesmo ajuste do
        // `zeebx_dpad_to_analog_p1`/`_p2` do core Libretro. **Por porta**, porque são dois
        // jogadores: quem joga de manche no controle 1 não decide pelo dono do controle 2.
        let rotulo = self.catalog.get("controls.dpad_to_analog").to_string();
        let dica = self.catalog.get("controls.dpad_to_analog.hint").to_string();
        changed |= ui
            .checkbox(
                &mut self
                    .settings
                    .controls
                    .player_mut(self.porta_editada)
                    .direcional_nos_eixos,
                rotulo,
            )
            .on_hover_text(dica)
            .changed();
        ui.add_space(6.0);

        ui.label(self.catalog.get("controls.axes"));
        ui.weak(self.catalog.get("controls.axes.hint"));

        // O valor de cada eixo, ao vivo. Um manche que não chega ao emulador aparece aqui como
        // um zero teimoso, e é o que separa "não mapeado" de "mapeado no eixo errado".
        let pad = self.pad_of(ui.ctx(), self.porta_editada);
        ui.horizontal(|ui| {
            for (name, value) in crate::input::AXIS_NAMES.iter().zip(pad.axes) {
                let share = value as f32 / crate::input::AXIS_CURSO as f32;
                ui.monospace(format!("{name}: {share:+.2}"));
            }
        });

        let none = self.catalog.get("controls.axis.none").to_string();
        let invert = self.catalog.get("controls.invert").to_string();
        let labels: Vec<String> = crate::input::AXIS_NAMES
            .iter()
            .map(|axis| self.catalog.get(&format!("axis.{axis}")).to_string())
            .collect();
        let player = self.settings.controls.player_mut(self.porta_editada);
        egui::Grid::new("eixos").num_columns(3).show(ui, |ui| {
            for (axis, label) in crate::input::AXIS_NAMES.iter().zip(&labels) {
                ui.label(label);

                let current = player
                    .axes
                    .get(*axis)
                    .map(|source| source.name.clone())
                    .unwrap_or_else(|| none.clone());
                let mut chosen: Option<Option<String>> = None;
                egui::ComboBox::from_id_salt(format!("eixo-{axis}"))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(false, &none).clicked() {
                            chosen = Some(None);
                        }
                        for name in gamepads::axis_names() {
                            if ui.selectable_label(false, name).clicked() {
                                chosen = Some(Some(name.to_string()));
                            }
                        }
                    });
                match chosen {
                    Some(Some(name)) => {
                        let inverted = player.axes.get(*axis).is_some_and(|s| s.invert);
                        player.axes.insert(
                            axis.to_string(),
                            crate::input::bindings::AxisSource {
                                name,
                                invert: inverted,
                            },
                        );
                        changed = true;
                    }
                    Some(None) => {
                        player.axes.remove(*axis);
                        changed = true;
                    }
                    None => {}
                }

                if let Some(source) = player.axes.get_mut(*axis) {
                    changed |= ui.checkbox(&mut source.invert, &invert).changed();
                }
                ui.end_row();
            }
        });
        changed
    }

    /// Enquanto a tela de controles espera, a próxima tecla ou botão apertado vira a origem.
    ///
    /// Devolve se algo foi ligado — o chamador guarda as configurações nesse caso.
    fn capture(&mut self, ctx: &egui::Context) -> bool {
        let Some(button) = self.capturing.clone() else {
            return false;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.capturing = None;
            return false;
        }
        // O teclado primeiro: é o que está debaixo da mão de quem está configurando.
        let key = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Key {
                    key, pressed: true, ..
                } => Some(*key),
                _ => None,
            })
        });
        // Mesma razão do `controller_view`: sem drenar a fila do gilrs um aperto de controle
        // nunca aparece, e a captura aceitaria só teclado.
        self.entrada.gamepads.poll();
        ctx.request_repaint();
        let source = match key {
            Some(key) => Some(Source::key(key.name())),
            None => {
                let device = self
                    .settings
                    .controls
                    .player(self.porta_editada)
                    .and_then(|player| player.device.clone());
                // O Wii Remote escolhido responde pelos botões dele; os outros, pelo gilrs.
                match self.wiimote_da_porta(self.porta_editada) {
                    Some(wiimote) if device.is_some() => wiimote.primeira_fonte(),
                    _ => self.entrada.gamepads.first_active(device.as_deref(), self.porta_editada),
                }
            }
        };
        let Some(source) = source else {
            return false;
        };
        self.settings
            .controls
            .player_mut(self.porta_editada)
            .bind(&button, source);
        self.capturing = None;
        true
    }

    fn debug_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let debug = &mut self.settings.debug;
        ui.weak(self.catalog.get("debug.hint"));
        ui.add_space(8.0);
        changed |= ui
            .checkbox(&mut debug.overlay, self.catalog.get("debug.overlay"))
            .changed();
        ui.add_space(8.0);
        // O que segue só existe dentro do painel; sem ele, marcar não faria efeito nenhum.
        ui.add_enabled_ui(debug.overlay, |ui| {
            changed |= ui
                .checkbox(&mut debug.speed, self.catalog.get("debug.speed"))
                .changed();
            changed |= ui
                .checkbox(&mut debug.clock, self.catalog.get("debug.clock"))
                .changed();
            changed |= ui
                .checkbox(&mut debug.memory, self.catalog.get("debug.memory"))
                .changed();
            changed |= ui
                .checkbox(&mut debug.timeline, self.catalog.get("debug.timeline"))
                .changed();
        });
        ui.add_space(8.0);
        changed |= ui
            .checkbox(&mut debug.log, self.catalog.get("debug.log"))
            .changed();
        ui.weak(self.catalog.get("debug.log.hint"));
        changed
    }

    /// A aba "Sobre": versão e para onde ir. Não muda configuração nenhuma, por isso devolve
    /// `false` sempre.
    fn about_tab(&mut self, ui: &mut egui::Ui) -> bool {
        ui.heading(self.catalog.get("app.name"));
        ui.label(
            self.catalog
                .format("about.version", &[("version", env!("CARGO_PKG_VERSION"))]),
        );
        ui.add_space(4.0);
        ui.weak(self.catalog.get("about.tagline"));

        ui.add_space(16.0);
        // O ícone do GitHub vem na fonte do egui; para o Discord não há um, e o balão de fala é
        // o mais próximo que a fonte de emoji oferece.
        ui.hyperlink_to(
            format!(
                "{} {}",
                egui::special_emojis::GITHUB,
                self.catalog.get("about.repository")
            ),
            REPOSITORY,
        );
        ui.hyperlink_to(format!("💬 {}", self.catalog.get("about.discord")), DISCORD);
        false
    }

    fn graphics_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        let graphics = &mut self.settings.graphics;

        let mut janela_mudou = false;
        for (rotulo, modo) in [
            ("graphics.window.main", &mut graphics.janela),
            ("graphics.window.game", &mut graphics.janela_do_jogo),
        ] {
            ui.horizontal(|ui| {
                ui.label(self.catalog.get(rotulo));
                egui::ComboBox::from_id_salt(rotulo)
                    .selected_text(self.catalog.get(modo.chave()))
                    .show_ui(ui, |ui| {
                        for opcao in crate::ui::settings::ModoDaJanela::TODOS {
                            let mudou = ui
                                .selectable_value(modo, opcao, self.catalog.get(opcao.chave()))
                                .changed();
                            janela_mudou |= mudou && rotulo == "graphics.window.main";
                            changed |= mudou;
                        }
                    });
            });
        }
        // A principal já está aberta: a escolha vale na hora. A do jogo vale no próximo jogo.
        if janela_mudou {
            for comando in graphics.janela.comandos() {
                ui.ctx()
                    .send_viewport_cmd_to(egui::ViewportId::ROOT, comando);
            }
        }
        ui.weak(self.catalog.get("graphics.window.hint"));

        ui.add_space(12.0);
        ui.label(self.catalog.get("graphics.scaling"));
        for option in Scaling::ALL {
            let label = self.catalog.get(option.key()).to_string();
            changed |= ui
                .radio_value(&mut graphics.scaling, option, label)
                .changed();
        }
        if graphics.scaling == Scaling::Integer {
            ui.weak(self.catalog.get("graphics.scaling.integer.hint"));
        }

        ui.add_space(12.0);
        changed |= ui
            .checkbox(&mut graphics.smooth, self.catalog.get("graphics.smooth"))
            .changed();
        changed |= ui
            .checkbox(
                &mut graphics.keep_aspect,
                self.catalog.get("graphics.keep_aspect"),
            )
            .changed();
        changed |= ui
            .checkbox(
                &mut graphics.speed_limit,
                self.catalog.get("graphics.speed_limit"),
            )
            .changed();
        ui.weak(self.catalog.get("graphics.speed_limit.hint"));

        ui.add_space(12.0);
        changed |= ui
            .checkbox(
                &mut graphics.gpu_present,
                self.catalog.get("graphics.gpu_present"),
            )
            .changed();
        ui.weak(self.catalog.get("graphics.gpu_present.hint"));

        ui.add_space(12.0);
        // A névoa vale nos dois rasterizadores, então fica fora da parte que depende da placa.
        let neblina_mudou = ui
            .checkbox(&mut graphics.neblina, self.catalog.get("graphics.fog"))
            .changed();
        changed |= neblina_mudou;
        ui.weak(self.catalog.get("graphics.fog.hint"));

        ui.add_space(12.0);
        changed |= ui
            .checkbox(
                &mut graphics.gpu_rasterizer,
                self.catalog.get("graphics.gpu_rasterizer"),
            )
            .changed();
        ui.weak(self.catalog.get("graphics.gpu_rasterizer.hint"));

        ui.add_space(12.0);
        let mut resolucao_mudou = false;
        let mut melhoria_mudou = false;
        let mut proporcao_mudou = false;
        let desligado = self.catalog.get("common.off").to_string();
        ui.add_enabled_ui(graphics.gpu_rasterizer, |ui| {
            ui.label(self.catalog.get("graphics.internal_resolution"));
            let atual = graphics.resolucao_interna.clamp(1, 6);
            egui::ComboBox::from_id_salt("resolucao-interna")
                .selected_text(settings::rotulo_da_resolucao(atual))
                .show_ui(ui, |ui| {
                    for fator in 1..=6u8 {
                        resolucao_mudou |= ui
                            .selectable_value(
                                &mut graphics.resolucao_interna,
                                fator,
                                settings::rotulo_da_resolucao(fator),
                            )
                            .changed();
                    }
                });
            ui.weak(self.catalog.get("graphics.internal_resolution.hint"));

            ui.add_space(8.0);
            ui.label(self.catalog.get("graphics.aspect"));
            egui::ComboBox::from_id_salt("proporcao")
                .selected_text(self.catalog.get(graphics.proporcao.chave()))
                .show_ui(ui, |ui| {
                    for opcao in crate::ui::settings::Proporcao::TODAS {
                        proporcao_mudou |= ui
                            .selectable_value(
                                &mut graphics.proporcao,
                                opcao,
                                self.catalog.get(opcao.chave()),
                            )
                            .changed();
                    }
                });
            ui.colored_label(ui.visuals().warn_fg_color, self.catalog.get("graphics.aspect.hint"));

            ui.add_space(8.0);
            ui.label(self.catalog.get("graphics.antialias"));
            egui::ComboBox::from_id_salt("antialias")
                .selected_text(settings::rotulo_de_nivel(graphics.antialias, "MSAA", &desligado))
                .show_ui(ui, |ui| {
                    for n in [1u8, 2, 4, 8] {
                        melhoria_mudou |= ui
                            .selectable_value(&mut graphics.antialias, n, settings::rotulo_de_nivel(n, "MSAA", &desligado))
                            .changed();
                    }
                });
            ui.weak(self.catalog.get("graphics.antialias.hint"));

            ui.add_space(8.0);
            ui.label(self.catalog.get("graphics.anisotropic"));
            egui::ComboBox::from_id_salt("anisotropico")
                .selected_text(settings::rotulo_de_nivel(graphics.anisotropico, "AF", &desligado))
                .show_ui(ui, |ui| {
                    for n in [1u8, 2, 4, 8, 16] {
                        melhoria_mudou |= ui
                            .selectable_value(&mut graphics.anisotropico, n, settings::rotulo_de_nivel(n, "AF", &desligado))
                            .changed();
                    }
                });
            ui.weak(self.catalog.get("graphics.anisotropic.hint"));
        });
        if melhoria_mudou {
            let (amostras, nivel) = (graphics.antialias as usize, graphics.anisotropico as usize);
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.define_melhorias(amostras, nivel);
            }
        }
        // Vale na hora: o próximo desenho já sai com ou sem névoa.
        if neblina_mudou {
            let permitida = graphics.neblina;
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.define_neblina(permitida);
            }
        }
        // Vale na hora para o jogo aberto: o destino é refeito no próximo quadro.
        if proporcao_mudou {
            let aspecto = graphics.proporcao.aspecto(16.0 / 9.0);
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.define_proporcao(aspecto);
            }
        }
        if resolucao_mudou {
            let fator = graphics.resolucao_interna as usize;
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.define_resolucao_interna(fator);
            }
        }
        changed | resolucao_mudou | melhoria_mudou | proporcao_mudou
    }

    fn audio_tab(&mut self, ui: &mut egui::Ui) -> bool {
        let mut changed = false;
        ui.weak(self.catalog.get("audio.hint"));
        ui.add_space(12.0);
        let audio = &mut self.settings.audio;
        changed |= ui
            .checkbox(&mut audio.enabled, self.catalog.get("audio.enabled"))
            .changed();
        ui.add_enabled_ui(audio.enabled, |ui| {
            changed |= ui
                .add(
                    egui::Slider::new(&mut audio.volume, 0..=100)
                        .text(self.catalog.get("audio.volume")),
                )
                .changed();
        });
        // Mexer no volume com o jogo aberto tem que valer na hora, não só na próxima abertura.
        if changed {
            let (enabled, volume) = (audio.enabled, audio.volume);
            if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                session.set_audio(enabled, volume);
            }
        }
        changed
    }

    /// A janela de configurações.
    ///
    /// É uma janela do sistema, não um painel dentro da principal: o `show_viewport_immediate`
    /// desenha o fecho na mesma linha de execução, que é o que permite mexer no `self` de
    /// dentro dele. A variante adiada exigiria um fecho compartilhável entre linhas, e nem o
    /// emulador nem as configurações atravessam essa fronteira.
    /// Relê a lista de saves. Chamada ao abrir a janela e depois de cada exclusão.
    ///
    /// A leitura toca o disco, então não vai no desenho do quadro: a janela redesenha muitas
    /// vezes por segundo e varrer o cache em cada uma seria varrer à toa.
    fn recarrega_saves(&mut self) {
        self.saves = crate::ui::saves::todos(self.settings.roms_dir.as_deref());
        self.saves_confirmar = None;
    }

    /// Uma linha da lista: o que é, quanto ocupa, e o botão de excluir.
    fn linha_de_save(&mut self, ui: &mut egui::Ui, indice: usize) {
        let (_, save) = &self.saves[indice];
        let (titulo, arquivos, bytes) = (save.titulo.clone(), save.arquivos, save.bytes);
        ui.horizontal(|ui| {
            ui.label(&titulo);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(self.catalog.get("saves.delete")).clicked() {
                    self.saves_confirmar = Some(indice);
                }
                ui.label(self.catalog.format(
                    "saves.files",
                    &[
                        ("count", &arquivos.to_string()),
                        ("size", &crate::ui::saves::tamanho(bytes)),
                    ],
                ));
            });
        });
    }

    /// A janela do gerenciador de saves.
    ///
    /// Duas listas, e a separação é a do console: o que o jogo escreveu na pasta dele, e o que
    /// está no sistema de arquivos do aparelho — que é de todos. Apagar o `zeeboiddata` tira os
    /// bonecos do Zeeboids **e** o que o Zeebo F.C. lê deles; a dica embaixo do título diz isso,
    /// porque a lista sozinha não diria.
    /// O aviso de abertura: o emulador ainda em desenvolvimento, e o controle a configurar antes
    /// de jogar. A caixa marcada guarda a versão, e a próxima versão mostra o aviso de novo.
    fn aviso_de_abertura(&mut self, ctx: &egui::Context) {
        let mut fechar = false;
        let mut configurar = false;
        let resposta = egui::Modal::new(egui::Id::new("aviso-de-abertura")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.heading(self.catalog.get("welcome.title"));
            ui.add_space(8.0);
            ui.label(self.catalog.get("welcome.development"));
            ui.add_space(6.0);
            ui.label(self.catalog.get("welcome.controls"));
            ui.add_space(12.0);
            ui.checkbox(&mut self.aviso_nao_mostrar, self.catalog.get("welcome.dont_show"));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(self.catalog.get("welcome.controls.open")).clicked() {
                    configurar = true;
                }
                if ui.button(self.catalog.get("welcome.dismiss")).clicked() {
                    fechar = true;
                }
            });
        });
        if !(fechar || configurar || resposta.should_close()) {
            return;
        }
        self.aviso_de_abertura = false;
        if self.aviso_nao_mostrar {
            self.settings.aviso_dispensado_na_versao = Some(env!("CARGO_PKG_VERSION").to_owned());
            self.save();
        }
        if configurar {
            self.tab = Tab::Controls;
            self.settings_open = true;
        }
    }

    fn saves_window(&mut self, ctx: &egui::Context) {
        let id = egui::ViewportId::from_hash_of("saves");
        let builder = egui::ViewportBuilder::default()
            .with_title(self.catalog.get("saves.title"))
            .with_inner_size([520.0, 420.0])
            .with_min_inner_size([380.0, 260.0]);
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(recado) = self.saves_recado.clone() {
                    ui.horizontal(|ui| {
                        ui.label(recado);
                        if ui.button(self.catalog.get("saves.confirm.no")).clicked() {
                            self.saves_recado = None;
                        }
                    });
                    ui.separator();
                }
                if self.saves.is_empty() {
                    ui.label(self.catalog.get("saves.none"));
                    return;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let jogos: Vec<usize> = (0..self.saves.len())
                        .filter(|&i| !self.saves[i].0)
                        .collect();
                    let aparelho: Vec<usize> =
                        (0..self.saves.len()).filter(|&i| self.saves[i].0).collect();
                    if !jogos.is_empty() {
                        ui.heading(self.catalog.get("saves.games"));
                        for i in jogos {
                            self.linha_de_save(ui, i);
                        }
                        ui.add_space(12.0);
                    }
                    if !aparelho.is_empty() {
                        ui.heading(self.catalog.get("saves.device"));
                        ui.label(self.catalog.get("saves.device.hint"));
                        for i in aparelho {
                            self.linha_de_save(ui, i);
                        }
                    }
                });
            });
            // A confirmação é modal de propósito: apagar não tem volta, e um clique errado num
            // botão de lista é fácil demais.
            if let Some(indice) = self.saves_confirmar {
                let titulo = self.saves[indice].1.titulo.clone();
                egui::Window::new(self.catalog.get("saves.title"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                    .show(ctx, |ui| {
                        ui.label(self.catalog.format("saves.confirm", &[("name", &titulo)]));
                        ui.horizontal(|ui| {
                            if ui.button(self.catalog.get("saves.confirm.yes")).clicked() {
                                let resultado = crate::ui::saves::apagar(&self.saves[indice].1);
                                self.saves_recado = Some(match resultado {
                                    Ok(()) => {
                                        self.catalog.format("saves.deleted", &[("name", &titulo)])
                                    }
                                    Err(erro) => self.catalog.format(
                                        "saves.failed",
                                        &[("name", &titulo), ("reason", &erro.to_string())],
                                    ),
                                });
                                self.recarrega_saves();
                            }
                            if ui.button(self.catalog.get("saves.confirm.no")).clicked() {
                                self.saves_confirmar = None;
                            }
                        });
                    });
            }
            close = ctx.input(|i| i.viewport().close_requested());
        });
        if close {
            self.saves_open = false;
            self.saves_confirmar = None;
            self.saves_recado = None;
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let id = egui::ViewportId::from_hash_of("configuracoes");
        let builder = egui::ViewportBuilder::default()
            .with_title(self.catalog.get("settings.title"))
            .with_inner_size([560.0, 480.0])
            .with_min_inner_size([420.0, 320.0]);
        let mut close = false;
        let mut captured = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            // A captura é lida no contexto **desta** janela: é nela que a tecla é apertada.
            captured = self.capture(ctx);
            egui::CentralPanel::default().show(ctx, |ui| self.settings_screen(ui));
            close = ctx.input(|i| i.viewport().close_requested());
            // Esperando uma tecla, a janela precisa se redesenhar sozinha para perceber o
            // botão de um controle, que não gera evento nenhum no egui.
            if self.capturing.is_some() {
                ctx.request_repaint();
            }
        });
        if captured {
            self.save();
        }
        if close {
            self.settings_open = false;
            self.capturing = None;
        }
    }

    /// A janela de log da execução.
    ///
    /// Separada da do jogo de propósito: quem está lendo log quer as duas coisas ao mesmo
    /// tempo, e um painel dentro da janela do jogo roubaria espaço do quadro.
    fn log_window(&mut self, ctx: &egui::Context) {
        let id = egui::ViewportId::from_hash_of("log");
        let builder = egui::ViewportBuilder::default()
            .with_title(self.catalog.get("debug.log.title"))
            .with_inner_size([640.0, 420.0])
            .with_min_inner_size([360.0, 200.0]);
        let linhas = match self.partida.as_ref().map(Partida::sessao) {
            Some(session) => session.log(),
            None => Vec::new(),
        };
        let mut close = false;
        let mut exportar = false;
        let caminho = self.caminho_do_relatorio().display().to_string();
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            egui::TopBottomPanel::top("log-barra").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    exportar = ui.button(self.catalog.get("debug.log.export")).clicked();
                    if let Some(aviso) = &self.log_status {
                        ui.weak(aviso);
                    } else {
                        // O caminho fixo à vista: quem quer o arquivo não precisa exportar nada.
                        ui.weak(caminho.clone());
                    }
                });
            });
            egui::CentralPanel::default().show(ctx, |ui| {
                if linhas.is_empty() {
                    ui.weak(self.catalog.get("debug.log.empty"));
                    return;
                }
                // Preso no fim: log que não acompanha o que acabou de acontecer não serve.
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for linha in &linhas {
                            ui.monospace(linha);
                        }
                    });
            });
            close = ctx.input(|i| i.viewport().close_requested());
            // O log muda enquanto o jogo roda, e o egui só acorda com mouse ou teclado.
            ctx.request_repaint();
        });
        if exportar {
            self.log_status = Some(self.export_log(&linhas));
        }
        // Fechar a janela dispensa o log **desta** execução, e não a preferência: quando o
        // jogo termina, o egui pede o fecho das janelas filhas, e gravar isso desligava a
        // opção sozinha — a janela abria uma vez e nunca mais.
        if close {
            self.log_dismissed = true;
            self.log_status = None;
        }
    }

    /// Onde o relatório desta execução é gravado sozinho. Ver [`partida::Relatorio`].
    pub fn caminho_do_relatorio(&self) -> PathBuf {
        match self.partida.as_ref() {
            Some(partida) => partida.caminho_do_relatorio(),
            None => crate::ui::settings::config_dir().join("relatorios").join("zeebx.log"),
        }
    }

    /// Grava o log num arquivo escolhido pelo usuário e devolve o que dizer sobre isso.
    fn export_log(&self, linhas: &[String]) -> String {
        let sugestao = match self.partida.as_ref().map(Partida::sessao).map(Session::title) {
            Some(title) if !title.is_empty() => format!("{title}.log"),
            _ => "zeebx.log".to_string(),
        };
        let Some(destino) = rfd::FileDialog::new()
            .set_file_name(&sugestao)
            .add_filter("log", &["log"])
            .save_file()
        else {
            return String::new();
        };
        match std::fs::write(&destino, linhas.join("\n") + "\n") {
            Ok(()) => self.catalog.format(
                "debug.log.saved",
                &[("path", &destino.display().to_string())],
            ),
            Err(err) => self
                .catalog
                .format("debug.log.failed", &[("reason", &err.to_string())]),
        }
    }

    /// A janela do jogo.
    ///
    /// Ela lê o teclado do **seu próprio** contexto, e não do da janela principal: a entrada do
    /// egui vai para a janela em foco, e ler do lugar errado faria o jogo só responder quando a
    /// biblioteca estivesse na frente.
    fn game_window(&mut self, ctx: &egui::Context) {
        let title = self
            .titulo_do_jogo_aberto()
            .unwrap_or_else(|| self.catalog.get("library.unknown_title").to_string());
        let id = egui::ViewportId::from_hash_of("jogo");
        let builder = self.settings.graphics.janela_do_jogo.no_construtor(
            egui::ViewportBuilder::default()
                .with_title(format!("{title} — Zeebx"))
                .with_inner_size([SCREEN[0] as f32, SCREEN[1] as f32 + 32.0])
                .with_min_inner_size([320.0, 240.0]),
        );
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            close = self.playing_screen(ctx);
        });
        if close {
            self.partida = None;
            self.frame = None;
        }
    }

    /// O estado de cada porta ligada agora. Ver [`EntradaDoDesktop::pads`].
    fn pads_now(&mut self, ctx: &egui::Context) -> Vec<(usize, Pad)> {
        let teclas = ctx.input(|input| input.keys_down.clone());
        self.entrada.pads(&self.settings.controls, &teclas)
    }

    /// O estado do controle de uma porta. Ver [`EntradaDoDesktop::pad`].
    fn pad_of(&self, ctx: &egui::Context, porta: usize) -> Pad {
        let teclas = ctx.input(|input| input.keys_down.clone());
        self.entrada.pad(&self.settings.controls, porta, &teclas)
    }

    fn wiimote_da_porta(&self, porta: usize) -> Option<crate::input::wiimote::EstadoWiimote> {
        self.entrada.wiimote_da_porta(&self.settings.controls, porta)
    }

    /// O botão que libera os sensores de movimento, e o que aconteceu da última vez.
    ///
    /// Se o pedido falhar — sem `pkexec`, sem agente de senha, senha recusada —, a regra aparece
    /// para ser criada à mão.
    fn oferece_liberar_sensores(&self, ui: &mut egui::Ui) {
        use crate::input::sensores;
        let estado = self
            .liberacao_dos_sensores
            .lock()
            .map(|e| e.clone())
            .unwrap_or(LiberacaoDosSensores::Parada);
        match &estado {
            LiberacaoDosSensores::Pedindo => {
                ui.weak(self.tr("controls.boomerang.unlock_waiting"));
            }
            _ => {
                if ui.button(self.tr("controls.boomerang.unlock")).clicked() {
                    if let Ok(mut e) = self.liberacao_dos_sensores.lock() {
                        *e = LiberacaoDosSensores::Pedindo;
                    }
                    let liberacao = self.liberacao_dos_sensores.clone();
                    let _ = std::thread::Builder::new()
                        .name("libera-sensores".into())
                        .spawn(move || {
                            let resultado = match sensores::libera_sensores() {
                                Ok(()) => LiberacaoDosSensores::Feita,
                                Err(erro) => LiberacaoDosSensores::Falhou(erro),
                            };
                            if let Ok(mut e) = liberacao.lock() {
                                *e = resultado;
                            }
                        });
                }
                ui.weak(self.tr("controls.boomerang.unlock_hint"));
            }
        }
        match estado {
            // A regra entrou, e a leitura volta na próxima tentativa, dentro de um segundo.
            LiberacaoDosSensores::Feita => {
                ui.weak(self.tr("controls.boomerang.unlock_done"));
            }
            LiberacaoDosSensores::Falhou(erro) => {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    self.tr("controls.boomerang.unlock_failed")
                        .replace("{erro}", &erro)
                        .replace("{arquivo}", sensores::ARQUIVO_DA_REGRA),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut sensores::REGRA_DO_UDEV.to_string())
                        .font(egui::TextStyle::Monospace)
                        .desired_width(f32::INFINITY),
                );
            }
            _ => {}
        }
    }

    /// Em uma linha, de onde vem o movimento do Boomerang e se ele está chegando.
    fn descreve_sensor(&self, sensor: &SensorDaPorta) -> String {
        sensor.descreve(&self.catalog)
    }

    fn sensor_da_porta(&self, porta: usize) -> SensorDaPorta {
        self.entrada.sensor_da_porta(&self.settings.controls, porta)
    }

    fn movimento_da_porta(&self, porta: usize) -> [f32; 3] {
        self.entrada.movimento_da_porta(&self.settings.controls, porta)
    }

    fn movimento_bruto_da_porta(&self, porta: usize) -> Option<[f32; 3]> {
        self.entrada.movimento_bruto_da_porta(&self.settings.controls, porta)
    }

    /// Roda e desenha o jogo na janela dele. Devolve se é hora de fechá-la.
    fn playing_screen(&mut self, ctx: &egui::Context) -> bool {
        let Some(calibracao) = self.partida.as_ref().map(Partida::sessao).map(Session::calibracao) else {
            return true;
        };
        // O aviso é uma área flutuante: pode ser declarado antes dos painéis sem tirar espaço
        // do quadro.
        self.aviso_de_calibracao(ctx, calibracao);
        // A entrada é lida antes de pegar a partida emprestada: montar o estado do controle
        // precisa do mapeamento e dos controles ligados, que também vivem no `self`.
        let pausada = self.partida.as_ref().is_none_or(|partida| partida.pausada);
        let pads = match pausada {
            true => None,
            false => Some(self.pads_now(ctx)),
        };
        let movimentos: [[f32; 3]; crate::input::PORTAS] =
            std::array::from_fn(|porta| self.movimento_da_porta(porta));
        let limit = self.settings.graphics.speed_limit;
        let Some(partida) = self.partida.as_mut() else {
            return true;
        };
        if let Some(pads) = &pads {
            // Guardamos teclas físicas: soltar 4 enquanto a seta continua apertada
            // não deve soltar o AVK que ambas representam. Repetições do SO não
            // acrescentam apertos; a repetição de navegação pertence ao guest.
            let teclado = &mut self.teclado_apertado;
            ctx.input(|i| {
                for event in &i.events {
                    if let egui::Event::Key {
                        key,
                        pressed,
                        repeat: false,
                        ..
                    } = event
                    {
                        if *pressed {
                            teclado.insert(*key);
                        } else {
                            teclado.remove(key);
                        }
                        partida.teclado_mudou(teclado.iter().filter_map(|k| input::avk_de(*k)));
                    }
                }
                if !i.focused {
                    teclado.clear();
                }
            });
            partida.avanca(
                pads,
                movimentos,
                self.teclado_apertado.iter().filter_map(|k| input::avk_de(*k)),
                limit,
            );
        }
        // O pedido de lançar vem antes da saída: ver [`Partida::pedido_de_lancamento`].
        if let Some(cls) = partida.pedido_de_lancamento() {
            let path = self
                .games
                .iter()
                .find(|game| game.clsid == Some(cls))
                .map(|game| game.path.clone());
            if let Some(path) = path {
                self.play(path);
                if let Some(partida) = self.partida.as_mut() {
                    partida.aberta_pela_z_wheel = true;
                    partida.reinicia_relogio();
                }
                return false;
            }
        }
        // Aberto pela biblioteca, o jogo que sai sozinho fecha a janela dele: a biblioteca é a
        // tela inicial de quem não passou pela Z-Wheel, e uma janela parada no último quadro não
        // serve para nada. Sem Z-Wheel configurada, a volta para ela também fecha.
        match partida.saida() {
            Saida::Segue => {}
            Saida::ReabreZWheel if self.z_wheel.is_some() => {
                if let Some(path) = self.z_wheel.clone() {
                    self.play(path);
                }
                if let Some(partida) = self.partida.as_mut() {
                    partida.reinicia_relogio();
                }
                return false;
            }
            Saida::ReabreZWheel | Saida::Fecha => return true,
        }
        // Sem barra superior, o teclado é o único caminho: `Esc` encerra e `P` pausa. Nenhuma
        // das duas colide com o controle do Zeebo, que usa setas, Z, X, C, V, Q, W, F, G, H,
        // Backspace e Enter.
        let stop =
            ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested());
        if ctx.input(|i| i.key_pressed(egui::Key::P)) {
            if let Some(partida) = self.partida.as_mut() {
                partida.pausada = !partida.pausada;
            }
        }
        alterna_tela_cheia(ctx);
        let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) else {
            return true;
        };

        let smooth = self.settings.graphics.smooth;
        // Com GL não há por que converter o mesmo quadro de novo para textura do egui: seriam
        // duas conversões por repaint, e só uma delas iria para a tela.
        let pela_placa = self.settings.graphics.gpu_present
            && self.gl.is_some()
            && !self.gpu_falhou.load(std::sync::atomic::Ordering::Relaxed);
        // **A tela da CPU só é lida quando ela é que vai à janela.** Com o quadro indo pela placa,
        // os pixels da CPU não são usados — e buscá-los custaria, no caminho de placa, a leitura
        // do quadro de volta a cada repaint, que é justamente o que a apresentação pela placa
        // existe para evitar. Ver [`Session::materializa_quadro_gl`]: com o readback adiado, o
        // quadro precisa ser materializado antes de qualquer conversão para bytes.
        let (quadro_largura, quadro_altura, quadro_bytes) = match pela_placa {
            true => (0, 0, Vec::new()),
            false => {
                session.materializa_quadro_gl();
                let tela = session.screen();
                (
                    tela.width() as i32,
                    tela.height() as i32,
                    tela.to_rgb565_bytes(),
                )
            }
        };
        if !pela_placa {
            // Subir a textura só quando a tela mudou: a janela repinta mais vezes que o jogo
            // desenha, e cada subida inteira custa uma conversão e uma ida à placa.
            let tela = session.screen();
            let chave = (tela.serie(), tela.escritas(), smooth);
            if self.frame.is_none() || self.frame_chave != Some(chave) {
                upload(ctx, &mut self.frame, tela, smooth);
                self.frame_chave = Some(chave);
            }
        }
        // O quadro 3D grande só vai pela placa, e só quando é ele que está na tela.
        let na_placa = pela_placa.then(|| session.quadro_na_placa()).flatten();
        // O que vai na tela sai da sessão agora, antes de desenhar: o empréstimo do jogo não
        // pode atravessar os fechos da interface, que precisam do `self` inteiro.
        //
        // Um jogo que parou continua com o último quadro à mostra, mas o motivo precisa
        // aparecer em algum lugar: sem a barra superior, ele vira uma faixa sobre o quadro.
        let ended = session.stopped_reason();

        // O painel é uma faixa embaixo, e não uma sobreposição: reservar espaço encolhe o
        // quadro do jogo em vez de tapá-lo. Precisa ser declarado antes do painel central,
        // porque no egui quem pede espaço primeiro é quem o recebe.
        if self.settings.debug.overlay {
            let debug = self.settings.debug.clone();
            let sample = session.sample();
            let (heap, objetos) = session.memory();
            let clock = session.clock_ms();
            let historia: Vec<(u32, u32)> = session
                .history()
                .map(|amostra| (amostra.speed, amostra.fps))
                .collect();
            egui::TopBottomPanel::bottom("painel-debug").show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if debug.speed {
                        ui.monospace(self.catalog.format(
                            "debug.speed.value",
                            &[
                                ("percent", &sample.speed.to_string()),
                                ("fps", &sample.fps.to_string()),
                            ],
                        ));
                        ui.separator();
                    }
                    if debug.clock {
                        ui.monospace(self.catalog.format(
                            "debug.clock.value",
                            &[
                                ("mips", &instrucoes_legiveis(sample.ips)),
                                ("clock", &format!("{:.1}s", clock as f32 / 1000.0)),
                            ],
                        ));
                        ui.separator();
                    }
                    if debug.memory {
                        ui.monospace(self.catalog.format(
                            "debug.memory.value",
                            &[
                                ("heap", &bytes_legiveis(heap)),
                                ("objects", &objetos.to_string()),
                            ],
                        ));
                    }
                    if debug.timeline && !historia.is_empty() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            desenhar_linha_do_tempo(ui, &historia);
                        });
                    }
                });
                ui.add_space(2.0);
            });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                // A proporção "da janela" acompanha o tamanho dela: o destino só é refeito quando as
                // colunas a mais mudam.
                if self.settings.graphics.proporcao == crate::ui::settings::Proporcao::Janela {
                    let area = ui.available_size();
                    let aspecto = area.x / area.y.max(1.0);
                    if let Some(session) = self.partida.as_mut().map(Partida::sessao_mut) {
                        session.define_proporcao(Some(aspecto));
                    }
                }
                // O quadro largo experimental tem a proporção dele; o resto é o 4:3 do console.
                let aspecto = na_placa.map_or(SCREEN[0] as f32 / SCREEN[1] as f32, |q| q.proporcao);
                let size = placement(
                    ui.available_size(),
                    self.settings.graphics.scaling,
                    self.settings.graphics.keep_aspect,
                    aspecto,
                );
                // Com contexto de GL, o quadro vai para a placa em RGB565 e é ela que amplia.
                // Sem ele, vale a textura do egui — que é o caminho de sempre.
                if pela_placa {
                    let pintor = self.pintor.clone();
                    let falhou = self.gpu_falhou.clone();
                    let suave = self.settings.graphics.smooth;
                    let (largura, altura) = (quadro_largura, quadro_altura);
                    let bytes = quadro_bytes.clone();
                    let rect = ui.centered_and_justified(|ui| {
                        ui.allocate_exact_size(size, egui::Sense::hover()).0
                    });
                    let rect = rect.inner;
                    ui.painter().add(egui::PaintCallback {
                        rect,
                        callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                            move |info, painter| {
                                let mut guarda = match pintor.lock() {
                                    Ok(guarda) => guarda,
                                    Err(_) => return,
                                };
                                if guarda.is_none() {
                                    match gpu::Pintor::novo(painter.gl()) {
                                        Ok(novo) => *guarda = Some(novo),
                                        Err(erro) => {
                                            eprintln!(
                                                "pintor de GL: {erro} — seguindo pela textura do egui"
                                            );
                                            falhou.store(true, std::sync::atomic::Ordering::Relaxed);
                                            return;
                                        }
                                    }
                                }
                                if let Some(pintor) = guarda.as_mut() {
                                    match na_placa {
                                        Some(quadro) => pintor.desenha_textura(
                                            painter.gl(),
                                            quadro,
                                            info.viewport_in_pixels().into(),
                                            suave,
                                        ),
                                        None => pintor.desenha(
                                            painter.gl(),
                                            &bytes,
                                            largura,
                                            altura,
                                            info.viewport_in_pixels().into(),
                                            suave,
                                        ),
                                    }
                                }
                            },
                        )),
                    });
                    return;
                }
                let Some(texture) = &self.frame else {
                    return;
                };
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(size));
                });
            });

        if let Some(reason) = &ended {
            let texto = self.catalog.format("play.failed", &[("reason", reason)]);
            egui::TopBottomPanel::bottom("parou")
                .frame(egui::Frame::NONE.fill(egui::Color32::from_black_alpha(200)))
                .show(ctx, |ui| {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(ui.visuals().warn_fg_color, texto);
                    });
                    ui.weak(self.catalog.get("play.stop.hint"));
                    ui.add_space(4.0);
                });
        }

        // Enquanto o jogo roda, a interface precisa ser redesenhada sozinha: sem isso o egui
        // só acorda quando o mouse ou o teclado se mexem, e o jogo pararia entre as teclas. O
        // pedido vai também para a janela principal porque é dentro do quadro dela que esta
        // aqui é desenhada — pedir só para si mesma deixaria o jogo parado sempre que a
        // biblioteca estivesse ociosa.
        let pausada = self.partida.as_ref().is_some_and(|partida| partida.pausada);
        if !stop && !pausada && ended.is_none() {
            ctx.request_repaint();
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        stop
    }
}

impl eframe::App for App {
    /// Devolve à placa o que o pintor criou.
    ///
    /// O contexto só existe enquanto a janela existe: soltar depois seria mexer num contexto
    /// morto, e não soltar deixa programa e textura vivos até o processo acabar. O `eframe`
    /// chama isto com o contexto ainda de pé, que é a única hora em que dá para fazer certo.
    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        let (Some(gl), Ok(mut guarda)) = (gl, self.pintor.lock()) else {
            return;
        };
        if let Some(pintor) = guarda.take() {
            pintor.solta(gl);
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A janela principal é só a biblioteca. As configurações e o jogo são janelas do
        // sistema, cada uma com o seu título e o seu botão de fechar.
        alterna_tela_cheia(ctx);
        egui::TopBottomPanel::top("nav").show(ctx, |ui| self.nav(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.library_screen(ui));
        if self.settings_open {
            self.settings_window(ctx);
        }
        if self.saves_open {
            self.saves_window(ctx);
        }
        if self.aviso_de_abertura {
            self.aviso_de_abertura(ctx);
        }
        self.acompanha_atualizacao(ctx);
        self.atualiza_presenca();
        // O log do núcleo sai por aqui, uma vez por quadro. Com o anel vazio — que é o caso
        // comum, com o nível padrão — isto é um cadeado e uma leitura.
        crate::registro::despeja_no_stderr();
        if self.partida.is_some() {
            if let Some(partida) = &self.partida {
                self.relatorio.grava(partida);
            }
            self.game_window(ctx);
            // A janela de log acompanha o jogo: só existe enquanto há execução para registrar.
            if self.settings.debug.log && !self.log_dismissed {
                self.log_window(ctx);
            }
        }
    }
}

/// Agora, em milissegundos Unix — o relógio que o Discord usa para contar o tempo de jogo.
/// A luz de um botão apertado agora. Translúcida de propósito: ela acende o botão, não o
/// substitui — o traço do desenho continua aparecendo por baixo.
const LIT: [u8; 4] = [0x2f, 0xd6, 0x8a, 0xb4];
/// A luz de um botão sob o cursor, mais fraca: ela só diz onde o clique vai cair.
const HOVER: [u8; 4] = [0x6c, 0x9c, 0xff, 0x50];
/// A cor do ponto que marca a posição de um analógico.
const STICK: [u8; 4] = [0x2f, 0x6b, 0xd6, 0xd0];
/// A cor de um botão esperando uma tecla. Ela pulsa, para não se confundir com "apertado".
const WAITING: [u8; 3] = [0xff, 0xa0, 0x28];

/// Manda o desenho do controle para a placa de vídeo.
///
/// As silhuetas viram texturas brancas com a opacidade da peça: a cor sai do tingimento na hora
/// de desenhar, e é isso que deixa o realce legível por cima de um desenho de qualquer cor.
fn upload_art(ctx: &egui::Context, art: &PadArt) -> ArtTextures {
    let base = ctx.load_texture(
        "controle",
        egui::ColorImage::from_rgba_unmultiplied([art.width, art.height], &art.base),
        egui::TextureOptions::LINEAR,
    );
    let parts = art
        .parts()
        .iter()
        .map(|part| {
            let mut rgba = Vec::with_capacity(part.alpha.len() * 4);
            for alpha in &part.alpha {
                rgba.extend_from_slice(&[255, 255, 255, *alpha]);
            }
            ctx.load_texture(
                format!("controle-{}", part.button),
                egui::ColorImage::from_rgba_unmultiplied([part.width, part.height], &rgba),
                egui::TextureOptions::LINEAR,
            )
        })
        .collect();
    ArtTextures { base, parts }
}

/// A maior altura que o desenho pode tomar. A janela de configurações também precisa caber a
/// lista de botões.
const ART_HEIGHT: f32 = 210.0;

/// `F11`, ou `Alt+Enter`, põe e tira a janela em foco da tela cheia.
///
/// O `Alt+Enter` só vale com o `Alt`: o `Enter` sozinho é botão do controle no teclado.
fn alterna_tela_cheia(ctx: &egui::Context) {
    let pediu = ctx.input(|i| {
        i.key_pressed(egui::Key::F11) || (i.modifiers.alt && i.key_pressed(egui::Key::Enter))
    });
    if pediu {
        let cheia = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!cheia));
    }
}

/// A luz, como a interface a entende.
fn light([r, g, b, a]: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn draw_controller(
    ui: &mut egui::Ui,
    art: &PadArt,
    textures: &ArtTextures,
    pad: &Pad,
    capturing: Option<&str>,
) -> Option<String> {
    let aspect = art.height as f32 / art.width as f32;
    let mut size = egui::vec2(ui.available_width().min(500.0), 0.0);
    size.y = size.x * aspect;
    if size.y > ART_HEIGHT {
        size = egui::vec2(ART_HEIGHT / aspect, ART_HEIGHT);
    }
    // A faixa toma a largura toda e o desenho fica no meio dela: assim ele não gruda na
    // esquerda quando a janela é alargada.
    let (outer, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size.y),
        egui::Sense::click(),
    );
    let rect = egui::Rect::from_center_size(outer.center(), size);
    let painter = ui.painter_at(outer);
    let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(textures.base.id(), rect, whole, egui::Color32::WHITE);

    let hovered = response.hover_pos().and_then(|pos| {
        let point = (pos - rect.min) / rect.size();
        art.hit(point.x, point.y)
    });
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // O pulso do botão em captura vem do relógio da interface, não de um contador nosso: ele
    // continua batendo no mesmo ritmo mesmo se a taxa de quadros variar.
    let time = ui.ctx().input(|input| input.time);
    let pulse = (time * 6.0).sin() as f32 * 0.5 + 0.5;
    let waiting = light([
        WAITING[0],
        WAITING[1],
        WAITING[2],
        (100.0 + 110.0 * pulse) as u8,
    ]);

    for (part, texture) in art.parts().iter().zip(&textures.parts) {
        let name = part.button.as_str();
        let down = Pad::button_by_name(name).is_some_and(|index| pad.is_down(index));
        let tint = match (capturing == Some(name), down, hovered == Some(name)) {
            (true, ..) => waiting,
            (_, true, _) => light(LIT),
            (_, _, true) => light(HOVER),
            _ => continue,
        };
        let [u0, v0, u1, v1] = part.bounds(art);
        let place = egui::Rect::from_min_max(
            rect.lerp_inside(egui::vec2(u0, v0)),
            rect.lerp_inside(egui::vec2(u1, v1)),
        );
        painter.image(texture.id(), place, whole, tint);
    }

    // O ponto de cada manche, dentro do círculo desenhado. É o que mostra que um analógico
    // está mesmo chegando ao emulador, e com quanto curso — coisa que acender o botão não diz.
    for (button, axes) in [("lthumb", [0, 1]), ("rthumb", [2, 3])] {
        let Some(part) = art.parts().iter().find(|part| part.button == button) else {
            continue;
        };
        let [u0, v0, u1, v1] = part.bounds(art);
        let circle = egui::Rect::from_min_max(
            rect.lerp_inside(egui::vec2(u0, v0)),
            rect.lerp_inside(egui::vec2(u1, v1)),
        );
        let offset = egui::vec2(
            pad.axes[axes[0]] as f32 / crate::input::AXIS_CURSO as f32,
            pad.axes[axes[1]] as f32 / crate::input::AXIS_CURSO as f32,
        );
        let radius = circle.width().min(circle.height()) / 2.0;
        let center = circle.center() + offset * radius * 0.6;
        painter.circle_filled(center, (radius * 0.22).max(2.0), light(STICK));
    }

    match response.clicked() {
        true => hovered.map(str::to_string),
        false => None,
    }
}

fn upload(
    ctx: &egui::Context,
    handle: &mut Option<egui::TextureHandle>,
    screen: &Framebuffer,
    smooth: bool,
) {
    // Uma passada só, do RGB565 direto para as cores do egui. Antes eram duas chamadas ao
    // `to_argb` — uma delas só para saber a capacidade —, uma cópia em RGBA e outra dentro do
    // `from_rgba_unmultiplied`: quatro vetores do tamanho da tela por repaint.
    let size = [screen.width() as usize, screen.height() as usize];
    let pixels = screen
        .to_argb()
        .into_iter()
        .map(|p| egui::Color32::from_rgb((p >> 16) as u8, (p >> 8) as u8, p as u8))
        .collect();
    let image = egui::ColorImage::new(size, pixels);
    // O pixel do console é grande e quadrado; suavizar é escolha de quem olha, não padrão.
    let options = match smooth {
        true => egui::TextureOptions::LINEAR,
        false => egui::TextureOptions::NEAREST,
    };
    match handle {
        Some(handle) => handle.set(image, options),
        None => *handle = Some(ctx.load_texture("zeebo", image, options)),
    }
}

/// O tamanho do quadro na área dada. Ver [`crate::ui::partida::enquadra`].
fn placement(area: egui::Vec2, scaling: Scaling, keep_aspect: bool, aspecto: f32) -> egui::Vec2 {
    egui::Vec2::from(partida::enquadra(area.into(), scaling, keep_aspect, aspecto))
}

#[cfg(test)]
mod tests {

    /// Só a transição vira tecla: segurar o direcional não repete.
    #[test]
    fn o_botao_de_confirmar_manda_tecla_uma_vez() {
        let mut antes = Pad::default();
        let mut agora = Pad::default();
        let b1 = Pad::button_by_name("b1").unwrap();
        agora.press(b1, true);
        assert_eq!(
            input::teclas_do_controle(&antes, &agora),
            vec![(crate::input::avk::CONFIRMA, true)]
        );
        antes = agora;
        assert!(input::teclas_do_controle(&antes, &agora).is_empty());
        agora.press(b1, false);
        assert_eq!(
            input::teclas_do_controle(&antes, &agora),
            vec![(crate::input::avk::CONFIRMA, false)]
        );
    }

    /// O direcional manda as quatro setas do BREW, cada uma no seu sentido.
    #[test]
    fn o_direcional_manda_as_quatro_setas() {
        use crate::input::avk;
        let antes = Pad::default();
        for (nome, esperado) in [
            ("up", avk::UP),
            ("down", avk::DOWN),
            ("left", avk::LEFT),
            ("right", avk::RIGHT),
        ] {
            let mut agora = Pad::default();
            agora.press(Pad::button_by_name(nome).unwrap(), true);
            assert_eq!(input::teclas_do_controle(&antes, &agora), vec![(esperado, true)]);
        }
        let mut voltar = Pad::default();
        voltar.press(Pad::button_by_name("b2").unwrap(), true);
        assert_eq!(
            input::teclas_do_controle(&antes, &voltar),
            vec![(crate::input::avk::CLR, true)]
        );
    }

    use super::*;

}

/// Instruções por segundo, na escala que couber.
///
/// Um jogo em espera ociosa executa pouquíssimo — arredondar tudo para milhões mostraria zero
/// justamente aí, e zero se lê como defeito e não como "está esperando".
fn instrucoes_legiveis(por_segundo: u64) -> String {
    match por_segundo {
        0..=9_999 => format!("{por_segundo}"),
        10_000..=9_999_999 => format!("{} K", por_segundo / 1_000),
        _ => format!("{} M", por_segundo / 1_000_000),
    }
}

/// Um tamanho em bytes no jeito que se lê.
fn bytes_legiveis(bytes: u32) -> String {
    match bytes {
        0..=9_999 => format!("{bytes} B"),
        10_000..=9_999_999 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f32 / (1024.0 * 1024.0)),
    }
}

/// O gráfico do painel: velocidade e quadros por segundo ao longo do último minuto.
///
/// É desenhado à mão em vez de com uma biblioteca de gráficos porque o que se quer aqui é
/// enxergar a forma — onde afundou, onde estabilizou —, e para isso duas linhas numa faixa de
/// 40 pixels bastam.
fn desenhar_linha_do_tempo(ui: &mut egui::Ui, historia: &[(u32, u32)]) {
    const ALTURA: f32 = 40.0;
    const LARGURA: f32 = 220.0;
    let (resposta, pintor) = ui.allocate_painter(egui::vec2(LARGURA, ALTURA), egui::Sense::hover());
    let area = resposta.rect;
    pintor.rect_filled(area, 2.0, egui::Color32::from_black_alpha(120));

    // A linha dos 100% é a referência que interessa: acima dela o jogo está no ritmo do
    // console, abaixo está devendo.
    let cem = area.bottom() - ALTURA * 0.5;
    pintor.line_segment(
        [egui::pos2(area.left(), cem), egui::pos2(area.right(), cem)],
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(40)),
    );

    let passo = LARGURA / historia.len().max(2) as f32;
    // A velocidade vai até 200% no gráfico; o que passar disso encosta no teto.
    let ponto = |i: usize, valor: u32, teto: f32| {
        egui::pos2(
            area.left() + i as f32 * passo,
            area.bottom() - ALTURA * (valor as f32 / teto).min(1.0),
        )
    };
    for (valores, cor, teto) in [
        (0, egui::Color32::from_rgb(120, 200, 255), 200.0),
        (1, egui::Color32::from_rgb(160, 255, 160), 60.0),
    ] {
        let linha: Vec<egui::Pos2> = historia
            .iter()
            .enumerate()
            .map(|(i, amostra)| match valores {
                0 => ponto(i, amostra.0, teto),
                _ => ponto(i, amostra.1, teto),
            })
            .collect();
        pintor.add(egui::Shape::line(linha, egui::Stroke::new(1.0_f32, cor)));
    }
}

/// O pedido de permissão para os sensores. Ver [`App::oferece_liberar_sensores`].
#[derive(Debug, Clone, Default)]
enum LiberacaoDosSensores {
    #[default]
    Parada,
    /// A janela de senha do sistema está aberta.
    Pedindo,
    Feita,
    Falhou(String),
}
