//! A interface do emulador: biblioteca de jogos, configurações e a tela do console.
//!
//! O emulador roda **na mesma linha de execução da interface**, um pedaço por quadro
//! desenhado. É o arranjo simples e é o certo aqui: o núcleo do unicorn não atravessa linhas de
//! execução, e o [`Session::step`] já devolve o controle sozinho a cada fatia de tempo real,
//! que é o que mantém a janela viva enquanto o jogo corre.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::loader::archive;
use crate::input::bindings::Source;
use crate::video::display::Framebuffer;
use crate::input::gamepads;
use crate::i18n::Catalog;
use crate::input::{self, Pad};
use crate::library::{self, Game};
use crate::input::padview::PadArt;
use crate::ponte;
use crate::session::Session;
use crate::settings::{self, Scaling, Settings};

/// Teto de tempo real que o jogo pode tomar num quadro da interface.
///
/// O orçamento normal **não** é fixo: é o tempo que passou desde o quadro anterior, que é
/// exatamente o quanto o jogo precisa emular para acompanhar o relógio do mundo. Uma fatia
/// fixa de 16 ms virava teto de velocidade, e um teto traiçoeiro: com a janela sincronizada
/// ao monitor, bastava emulação mais desenho passarem de um retraço para o período dobrar
/// para 33 ms — e o jogo ficava com 16 de cada 33, travado em 50% por mais folga que a
/// máquina tivesse. Era o que a tela de seleção do Crash mostrava.
///
/// O teto existe só para o caso de o host não dar conta: sem ele, um quadro atrasado pede um
/// orçamento maior, que atrasa mais o seguinte, e a janela para de responder.
const MAX_SLICE: Duration = Duration::from_millis(100);

/// A tela do Zeebo.
const SCREEN: [usize; 2] = [640, 480];

/// A imagem de quem não tem imagem nenhuma.
const PLACEHOLDER: &[u8] = include_bytes!("../assets/zeebx.png");

/// Largura de um cartão da biblioteca, e o lado do quadro em que a imagem cabe.
const CARD_WIDTH: f32 = 136.0;
const CARD_ART: f32 = 100.0;
/// Espaço reservado ao título, embaixo. Duas linhas.
const CARD_TEXT: f32 = 36.0;

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
    saves: Vec<(bool, crate::saves::Save)>,
    /// O save que espera confirmação para ser apagado.
    saves_confirmar: Option<usize>,
    /// O que dizer depois de apagar.
    saves_recado: Option<String>,
    /// O que dizer depois de mexer na trava diária do Zeeboids, se algo houver a dizer.
    sync_unlocked: Option<String>,
    /// O jogo em execução. Enquanto existe, ele tem uma janela só dele.
    session: Option<Session>,
    /// A textura em que o quadro do console é enviado para a placa de vídeo.
    frame: Option<egui::TextureHandle>,
    /// O que deu errado na última tentativa de abrir um jogo.
    error: Option<String>,
    paused: bool,
    /// Quando o jogo rodou pela última vez, para saber quanto tempo real ele tem a recuperar.
    last_step: std::time::Instant,
    /// O controle do quadro anterior, por porta, para saber o que acabou de ser apertado.
    ///
    /// Só o aperto vira tecla do console: manter apertado não repete, que é como um toque se
    /// comporta em menu.
    pad_anterior: [Pad; crate::input::PORTAS],
    teclado_apertado: HashSet<egui::Key>,
    teclas_entregues: HashSet<u32>,
    /// O que dizer sobre a última tentativa de exportar o log.
    log_status: Option<String>,
    /// A janela de log foi fechada nesta execução. Zera ao abrir outro jogo.
    log_dismissed: bool,
    /// Quando o relatório foi gravado em disco pela última vez.
    ///
    /// Ele é gravado sozinho, a cada poucos segundos, num lugar fixo. O botão de exportar abre
    /// um diálogo, e diálogo é coisa que se esquece de confirmar: passei três idas e vindas
    /// analisando um relatório velho porque o arquivo nunca tinha sido regravado. Um caminho
    /// previsível e sempre atual vale mais do que um que o usuário escolhe.
    log_gravado: Option<std::time::Instant>,
    /// Os controles de verdade ligados no computador.
    gamepads: gamepads::Gamepads,
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
                catalog.select_best(&crate::i18n::system_language());
            }
        }
        // O tema escuro é o que se espera de um emulador, e deixa a imagem do jogo no centro
        // sem uma moldura clara puxando o olho.
        context.egui_ctx.set_theme(egui::Theme::Dark);

        let games = settings
            .roms_dir
            .as_deref()
            .map(library::scan)
            .unwrap_or_default();
        if let Err(err) = library::sync_catalog(&games) {
            eprintln!("catálogo de jogos: {err}");
        }
        Self {
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
            session: None,
            frame: None,
            error: None,
            paused: false,
            last_step: std::time::Instant::now(),
            pad_anterior: Default::default(),
            teclado_apertado: HashSet::new(),
            teclas_entregues: HashSet::new(),
            log_status: None,
            log_dismissed: false,
            log_gravado: None,
            gamepads: gamepads::Gamepads::default(),
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
        }
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
    }

    /// O que o console vê em cada porta, a partir do que está configurado.
    ///
    /// Uma porta desligada vira `None` e some da enumeração. É o que faz o jogo enxergar um
    /// controle, dois, ou um teclado — e é o mesmo caminho que responde ao
    /// `IHID::GetConnectedDevices`.
    fn portas_configuradas(&self) -> [Option<crate::input::bindings::Aparelho>; crate::input::PORTAS] {
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

    fn play(&mut self, path: PathBuf) {
        self.error = None;
        self.pad_anterior = Default::default();
        self.teclado_apertado.clear();
        self.teclas_entregues.clear();
        self.frame = None;
        self.paused = false;
        self.log_dismissed = false;
        self.log_gravado = None;
        self.log_status = None;
        // A serial é ligada junto com o começo, e não depois: o construtor do applet roda
        // dentro do `start_with`, e o que ele faz ao nascer precisa estar na captura.
        let serial = self
            .settings
            .debug
            .log
            .then(|| Self::caminho_da_serial(&library::title_for(&path)));
        match Session::start_with(&path, self.portas_configuradas(), serial.as_deref()) {
            Ok(mut session) => {
                session.set_installed_applets(self.games.iter().filter_map(|game| game.clsid));
                // Ligar o som aqui é seguro **porque o jogo ainda não começou**: o `start` só
                // prepara, e o `EVT_APP_START` sai na primeira volta do laço. Antes disso o
                // jogo já tocava dentro do `start`, e o som saía com a tela vazia.
                let audio = &self.settings.audio;
                if let Some(err) = session.set_audio(audio.enabled, audio.volume) {
                    eprintln!("sem som: {err}");
                }
                self.session = Some(session);
            }
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

    fn library_screen(&mut self, ui: &mut egui::Ui) {
        let Some(dir) = self.settings.roms_dir.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.label(self.tr("library.no_folder"));
                if ui.button(self.tr("nav.settings")).clicked() {
                    self.settings_open = true;
                }
            });
            return;
        };

        ui.horizontal(|ui| {
            ui.label(
                self.catalog
                    .format("library.count", &[("count", &self.games.len().to_string())]),
            );
            if ui.button(self.tr("library.rescan")).clicked() {
                self.rescan();
            }
        });
        ui.separator();

        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.separator();
        }

        if self.games.is_empty() {
            ui.label(
                self.catalog
                    .format("library.empty", &[("folder", &dir.display().to_string())]),
            );
            return;
        }

        // A escolha sai do laço: mexer em `self` enquanto a lista está emprestada não passa
        // pelo compilador, e guardar o caminho é mais claro que contorná-lo.
        let mut chosen = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Os cartões se acomodam sozinhos na largura da janela em vez de ocuparem um
            // número fixo de colunas: a mesma tela serve numa janela estreita e numa larga.
            ui.horizontal_wrapped(|ui| {
                let Self {
                    games,
                    art_cache,
                    placeholder,
                    ..
                } = self;
                for game in games.iter() {
                    let texture = art_cache
                        .entry(game.path.clone())
                        .or_insert_with(|| upload_art_of(ui.ctx(), game, placeholder));
                    if game_card(ui, game, texture.as_ref()).clicked() {
                        chosen = Some(game.path.clone());
                    }
                }
            });
        });
        if let Some(path) = chosen {
            self.play(path);
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
        let changed = match self.tab {
            Tab::General => self.general_tab(ui),
            Tab::Controls => self.controls_tab(ui),
            Tab::Graphics => self.graphics_tab(ui),
            Tab::Audio => self.audio_tab(ui),
            Tab::Debug => self.debug_tab(ui),
            Tab::About => self.about_tab(ui),
        };
        if changed {
            self.save();
            // Mexer nas portas com um jogo aberto vale na hora. Guardar e só aplicar na próxima
            // partida seria a configuração parecer que não pegou.
            let portas = self.portas_configuradas();
            if let Some(session) = &mut self.session {
                session.set_portas(portas);
            }
        }
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
                    (Aparelho::Controle, "controls.kind.pad"),
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
        if self.settings.controls.player_mut(porta).aparelho == Aparelho::Teclado {
            ui.weak(self.catalog.get("controls.kind.hint"));
        }
        ui.add_space(8.0);
        changed
    }

    /// O desenho do controle. Devolve o botão clicado.
    ///
    /// Ele acende o que está apertado agora, e é por isso que vale mais que a lista: um
    /// direcional que fica aceso sem ninguém encostar no controle mostra na hora um problema
    /// que a lista de texto esconderia.
    fn controller_view(&mut self, ui: &mut egui::Ui) -> Option<String> {
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

        if let Some(button) = self.controller_view(ui) {
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
        let devices = self.gamepads.names();
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
                });
            if let Some(device) = chosen {
                // Escolher um controle traz o mapeamento típico dele junto; ficar sem controle
                // volta para o teclado puro. Nos dois casos o que estava configurado à mão se
                // perde, e é por isso que a troca é um clique deliberado numa lista.
                // Trocar o controle troca **o mapeamento**, não a porta: quem ela é e se está
                // ligada foi decidido acima, e perder isso aqui seria a configuração se desfazer
                // sozinha ao escolher um aparelho na lista.
                let atual = self.settings.controls.player_mut(self.porta_editada);
                let (ligada, aparelho) = (atual.ligada, atual.aparelho);
                *atual = match device {
                    Some(name) => crate::input::bindings::Player::with_gamepad(name),
                    None => crate::input::bindings::Player::default(),
                };
                atual.ligada = ligada;
                atual.aparelho = aparelho;
                changed = true;
            }
            if ui.button(self.catalog.get("controls.rescan")).clicked() {
                self.gamepads.poll();
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
                *self.settings.controls.player_mut(self.porta_editada) = match device {
                    Some(name) => crate::input::bindings::Player::with_gamepad(name),
                    None => crate::input::bindings::Player::default(),
                };
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
        ui.label(self.catalog.get("controls.axes"));
        ui.weak(self.catalog.get("controls.axes.hint"));

        // O valor de cada eixo, ao vivo. Um manche que não chega ao emulador aparece aqui como
        // um zero teimoso, e é o que separa "não mapeado" de "mapeado no eixo errado".
        let pad = self.pad_of(ui.ctx(), self.porta_editada);
        ui.horizontal(|ui| {
            for (name, value) in crate::input::AXIS_NAMES.iter().zip(pad.axes) {
                let share = value as f32 / crate::input::AXIS_MAX as f32;
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
        let source = match key {
            Some(key) => Some(Source::key(key.name())),
            None => {
                let device = self
                    .settings
                    .controls
                    .player(self.porta_editada)
                    .and_then(|player| player.device.clone());
                self.gamepads.first_active(device.as_deref())
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
        changed
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
            if let Some(session) = &mut self.session {
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
        // Os caches feitos antes de o manifesto existir não sabem o que veio do pacote. Antes de
        // listar, reconstrói o manifesto de cada um a partir do zip — que continua na pasta de
        // ROMs. Sem isso o jogo antigo simplesmente não apareceria na lista.
        if let Some(roms) = self.settings.roms_dir.clone() {
            for entrada in std::fs::read_dir(roms).into_iter().flatten().flatten() {
                let caminho = entrada.path();
                if caminho
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
                {
                    let _ = archive::completar_manifesto(&caminho);
                }
            }
        }
        let jogos = crate::saves::dos_jogos(&archive::cache_dir());
        let aparelho = crate::saves::do_aparelho(&archive::device_dir());
        self.saves = jogos
            .into_iter()
            .map(|s| (false, s))
            .chain(aparelho.into_iter().map(|s| (true, s)))
            .collect();
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
                        ("size", &crate::saves::tamanho(bytes)),
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
                                let resultado = crate::saves::apagar(&self.saves[indice].1);
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
        let linhas = match &self.session {
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

    /// Grava o log num arquivo escolhido pelo usuário e devolve o que dizer sobre isso.
    /// Onde o relatório desta execução é gravado sozinho.
    /// Onde a captura de serial daquele jogo é gravada, ao lado do relatório.
    fn caminho_da_serial(titulo: &str) -> PathBuf {
        let nome = match titulo.is_empty() {
            true => "zeebx.serial.log".to_string(),
            false => format!("{titulo}.serial.log"),
        };
        crate::settings::config_dir().join("relatorios").join(nome)
    }

    pub fn caminho_do_relatorio(&self) -> PathBuf {
        let nome = match self.session.as_ref().map(Session::title) {
            Some(title) if !title.is_empty() => format!("{title}.log"),
            _ => "zeebx.log".to_string(),
        };
        crate::settings::config_dir().join("relatorios").join(nome)
    }

    /// Grava o relatório em disco, no máximo uma vez a cada [`Self::INTERVALO_DO_RELATORIO`].
    ///
    /// Sem isto o único jeito de ver o relatório de um jogo que **não termina** — e a Z-Wheel
    /// não termina, ela repete a abertura — é abrir a janela de log e exportar à mão.
    fn grava_relatorio(&mut self) {
        let agora = std::time::Instant::now();
        if self
            .log_gravado
            .is_some_and(|antes| agora - antes < Self::INTERVALO_DO_RELATORIO)
        {
            return;
        }
        self.log_gravado = Some(agora);
        let Some(session) = &self.session else {
            return;
        };
        let destino = self.caminho_do_relatorio();
        if let Some(pai) = destino.parent() {
            let _ = std::fs::create_dir_all(pai);
        }
        let _ = std::fs::write(&destino, session.log().join("\n") + "\n");
    }

    fn export_log(&self, linhas: &[String]) -> String {
        let sugestao = match self.session.as_ref().map(Session::title) {
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
        let title = match self.session.as_ref().map(Session::title) {
            Some(title) if !title.is_empty() => title.to_string(),
            _ => self.catalog.get("library.unknown_title").to_string(),
        };
        let id = egui::ViewportId::from_hash_of("jogo");
        let builder = egui::ViewportBuilder::default()
            .with_title(format!("{title} — Zeebx"))
            .with_inner_size([SCREEN[0] as f32, SCREEN[1] as f32 + 32.0])
            .with_min_inner_size([320.0, 240.0]);
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ctx, _class| {
            close = self.playing_screen(ctx);
        });
        if close {
            self.session = None;
            self.frame = None;
        }
    }

    /// O estado de cada porta agora, montado a partir do mapeamento dela.
    ///
    /// Uma porta desligada não entra: o que sai daqui são os pares `(porta, controle)` das que
    /// estão ligadas, e é o que a sessão entrega ao jogo.
    fn pads_now(&mut self, ctx: &egui::Context) -> Vec<(usize, Pad)> {
        self.gamepads.poll();
        let portas: Vec<usize> = self
            .settings
            .controls
            .ligadas()
            .map(|(indice, _)| indice)
            .collect();
        portas
            .into_iter()
            .map(|porta| (porta, self.pad_of(ctx, porta)))
            .collect()
    }

    /// O estado do controle de uma porta, montado a partir do mapeamento dela.
    ///
    /// O teclado e o controle são consultados juntos: quem tem os dois pode usar os dois, e é
    /// isso que ter mais de uma origem por botão significa.
    fn pad_of(&self, ctx: &egui::Context, porta: usize) -> Pad {
        let Some(player) = self.settings.controls.player(porta) else {
            return Pad::default();
        };
        let device = player.device.clone();
        let keys: Vec<Source> = player
            .buttons
            .values()
            .flatten()
            .filter(|source| source.is_key())
            .cloned()
            .collect();
        // Quais teclas estão apertadas sai numa consulta só: entrar no estado de entrada do
        // egui uma vez por origem seria uma travada por botão, a cada quadro.
        let pressed: Vec<Source> = ctx.input(|input| {
            keys.into_iter()
                .filter(|source| match source {
                    Source::Key { name } => {
                        egui::Key::from_name(name).is_some_and(|key| input.key_down(key))
                    }
                    _ => false,
                })
                .collect()
        });
        let gamepads = &self.gamepads;
        player.pad(
            |source| pressed.contains(source) || gamepads.is_active(device.as_deref(), source),
            |axis| gamepads.value(device.as_deref(), axis),
        )
    }

    /// Combina as fontes antes de emitir transições: uma seta física pode estar
    /// mapeada também no controle, mas continua sendo um único aperto BREW.
    fn transicoes_de_teclas(
        anteriores: &mut HashSet<u32>,
        atuais: HashSet<u32>,
    ) -> Vec<(u32, bool)> {
        let mut eventos: Vec<_> = anteriores
            .difference(&atuais)
            .map(|&key| (key, false))
            .collect();
        eventos.extend(atuais.difference(anteriores).map(|&key| (key, true)));
        eventos.sort_unstable();
        *anteriores = atuais;
        eventos
    }

    fn avks_ativos(teclado: &HashSet<egui::Key>, pads: &[Pad]) -> HashSet<u32> {
        let mut keys: HashSet<_> = teclado
            .iter()
            .filter_map(|key| Self::avk_de(*key))
            .collect();
        for pad in pads {
            keys.extend(
                Self::teclas_do_controle(&Pad::default(), pad)
                    .into_iter()
                    .filter_map(|(key, down)| down.then_some(key)),
            );
        }
        keys
    }

    /// As teclas que o direcional do controle manda, comparando com o quadro anterior.
    fn teclas_do_controle(antes: &Pad, agora: &Pad) -> Vec<(u32, bool)> {
        const DE_BOTAO: [(&str, u32); 3] = [
            ("left", input::avk::RODA_ANTERIOR),
            ("right", input::avk::RODA_SEGUINTE),
            ("b1", input::avk::CONFIRMA),
        ];

        let mut teclas = Vec::new();
        for (nome, avk) in DE_BOTAO {
            let Some(indice) = Pad::button_by_name(nome) else {
                continue;
            };
            if agora.is_down(indice) != antes.is_down(indice) {
                teclas.push((avk, agora.is_down(indice)));
            }
        }
        teclas
    }

    /// O código virtual do BREW de uma tecla da janela, quando ela tem um.
    ///
    /// `Esc` e `P` ficam de fora de propósito: são as duas da janela, encerrar e pausar.
    fn avk_de(key: egui::Key) -> Option<u32> {
        use egui::Key::*;
        Some(match key {
            ArrowUp => input::avk::UP,
            ArrowDown => input::avk::DOWN,
            // Medidas na Z-Wheel: ver [`input::avk::RODA_ANTERIOR`]. Os dígitos continuam
            // valendo, então `3` e `4` seguem girando a roda como sempre — as setas passam a
            // fazer o mesmo, que é o que se espera de uma seta.
            ArrowLeft => input::avk::RODA_ANTERIOR,
            ArrowRight => input::avk::RODA_SEGUINTE,
            Enter | Space => input::avk::CONFIRMA,
            Backspace | Delete => input::avk::CLR,
            Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 => {
                input::avk::ZERO + (key as u32 - Num0 as u32)
            }
            _ => return None,
        })
    }

    /// Roda e desenha o jogo na janela dele. Devolve se é hora de fechá-la.
    /// De quanto em quanto tempo o relatório é regravado. Dois segundos é frequente o bastante
    /// para acompanhar uma execução e raro o bastante para não pesar.
    const INTERVALO_DO_RELATORIO: std::time::Duration = std::time::Duration::from_secs(2);

    fn playing_screen(&mut self, ctx: &egui::Context) -> bool {
        if self.session.is_none() {
            return true;
        }
        // A entrada é lida antes de pegar a sessão emprestada: montar o estado do controle
        // precisa do mapeamento e dos controles ligados, que também vivem no `self`.
        let pads = match self.paused {
            true => None,
            false => Some(self.pads_now(ctx)),
        };
        let mut teclas = Vec::new();
        if let Some(pads) = &pads {
            // Guardamos teclas físicas: soltar 4 enquanto a seta continua apertada
            // não deve soltar o AVK que ambas representam. Repetições do SO não
            // acrescentam apertos; a repetição de navegação pertence ao guest.
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
                            self.teclado_apertado.insert(*key);
                        } else {
                            self.teclado_apertado.remove(key);
                        }
                        let atuais = Self::avks_ativos(&self.teclado_apertado, &self.pad_anterior);
                        teclas.extend(Self::transicoes_de_teclas(
                            &mut self.teclas_entregues,
                            atuais,
                        ));
                    }
                }
                if !i.focused {
                    self.teclado_apertado.clear();
                }
            });
            self.pad_anterior = Default::default();
            for (porta, pad) in pads {
                self.pad_anterior[*porta] = *pad;
            }
            let atuais = Self::avks_ativos(&self.teclado_apertado, &self.pad_anterior);
            teclas.extend(Self::transicoes_de_teclas(
                &mut self.teclas_entregues,
                atuais,
            ));
        }
        let limit = self.settings.graphics.speed_limit;
        let Some(session) = &mut self.session else {
            return true;
        };
        if let Some(pads) = pads {
            for (porta, pad) in pads {
                session.set_port_pad(porta, pad);
            }
            for (avk, apertada) in teclas {
                session.set_key(avk, apertada);
            }
            // O orçamento é o tempo real que passou desde o quadro anterior.
            let now = std::time::Instant::now();
            let slice = (now - self.last_step).min(MAX_SLICE);
            self.last_step = now;
            let _ = session.step(slice, limit);
        }
        if let Some(cls) = session.take_launch_request() {
            let path = self
                .games
                .iter()
                .find(|game| game.clsid == Some(cls))
                .map(|game| game.path.clone());
            if let Some(path) = path {
                self.play(path);
                self.last_step = std::time::Instant::now();
                return false;
            }
        }
        // Sem barra superior, o teclado é o único caminho: `Esc` encerra e `P` pausa. Nenhuma
        // das duas colide com o controle do Zeebo, que usa setas, Z, X, C, V, Q, W, F, G, H,
        // Backspace e Enter.
        let stop =
            ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested());
        if ctx.input(|i| i.key_pressed(egui::Key::P)) {
            self.paused = !self.paused;
        }

        let smooth = self.settings.graphics.smooth;
        upload(ctx, &mut self.frame, session.screen(), smooth);
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
            let debug = self.settings.debug;
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
                let Some(texture) = &self.frame else {
                    return;
                };
                let size = placement(
                    ui.available_size(),
                    self.settings.graphics.scaling,
                    self.settings.graphics.keep_aspect,
                );
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
        if !stop && !self.paused && ended.is_none() {
            ctx.request_repaint();
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        stop
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // A janela principal é só a biblioteca. As configurações e o jogo são janelas do
        // sistema, cada uma com o seu título e o seu botão de fechar.
        egui::TopBottomPanel::top("nav").show(ctx, |ui| self.nav(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.library_screen(ui));
        if self.settings_open {
            self.settings_window(ctx);
        }
        if self.saves_open {
            self.saves_window(ctx);
        }
        if self.session.is_some() {
            self.grava_relatorio();
            self.game_window(ctx);
            // A janela de log acompanha o jogo: só existe enquanto há execução para registrar.
            if self.settings.debug.log && !self.log_dismissed {
                self.log_window(ctx);
            }
        }
    }
}

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

/// Manda a imagem de um jogo para a placa de vídeo, caindo na reserva quando ele não tem uma.
fn upload_art_of(
    ctx: &egui::Context,
    game: &Game,
    placeholder: &Option<crate::video::icon::Image>,
) -> Option<egui::TextureHandle> {
    let image = game.art.as_ref().or(placeholder.as_ref())?;
    let color = egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba);
    // Um ícone de 26 pixels aparece ampliado quatro vezes: interpolar viraria um borrão, e o
    // bloco quadrado é o que o console mostrava. Uma imagem grande já entra reduzida, e aí a
    // interpolação é que evita o serrilhado.
    let options = match image.width.max(image.height) < CARD_ART as usize {
        true => egui::TextureOptions::NEAREST,
        false => egui::TextureOptions::LINEAR,
    };
    Some(ctx.load_texture(format!("capa-{}", game.path.display()), color, options))
}

/// Um cartão da biblioteca: a imagem do jogo, o título embaixo, e o clique que o abre.
fn game_card(
    ui: &mut egui::Ui,
    game: &Game,
    texture: Option<&egui::TextureHandle>,
) -> egui::Response {
    let size = egui::vec2(CARD_WIDTH, CARD_ART + CARD_TEXT + 24.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, visuals.weak_bg_fill);
    painter.rect_stroke(rect, 8.0, visuals.bg_stroke, egui::StrokeKind::Inside);

    let frame = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 12.0 + CARD_ART / 2.0),
        egui::vec2(CARD_ART, CARD_ART),
    );
    if let Some(texture) = texture {
        // A imagem cabe no quadro sem esticar: um ícone de 65×42 deformado até virar quadrado
        // fica pior que um com sobra dos lados.
        let source = texture.size_vec2();
        let scale = (frame.width() / source.x).min(frame.height() / source.y);
        let placed = egui::Rect::from_center_size(frame.center(), source * scale);
        painter.image(
            texture.id(),
            placed,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    let galley = painter.layout(
        game.title.clone(),
        egui::FontId::proportional(12.0),
        visuals.text_color(),
        rect.width() - 16.0,
    );
    let text = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        frame.bottom() + 8.0,
    );
    painter.galley(text, galley, visuals.text_color());

    // O cartão corta o título comprido, então o nome inteiro fica à espera do ponteiro.
    match game.clsid {
        Some(clsid) => response.on_hover_text(format!("{}\n{clsid:#010x}", game.title)),
        None => response.on_hover_text(&game.title),
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
            pad.axes[axes[0]] as f32 / crate::input::AXIS_MAX as f32,
            pad.axes[axes[1]] as f32 / crate::input::AXIS_MAX as f32,
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

/// Envia o quadro do console para a textura, criando-a na primeira vez.
fn upload(
    ctx: &egui::Context,
    handle: &mut Option<egui::TextureHandle>,
    screen: &Framebuffer,
    smooth: bool,
) {
    let mut rgba = Vec::with_capacity(screen.to_argb().len() * 4);
    for pixel in screen.to_argb() {
        rgba.extend_from_slice(&[(pixel >> 16) as u8, (pixel >> 8) as u8, pixel as u8, 255]);
    }
    let size = [screen.width() as usize, screen.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
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

/// O retângulo em que a imagem do console é desenhada dentro de `area`.
///
/// Separado da interface porque é a única parte com regra de verdade, e a única que dá para
/// conferir sem abrir uma janela.
fn placement(area: egui::Vec2, scaling: Scaling, keep_aspect: bool) -> egui::Vec2 {
    let native = egui::vec2(SCREEN[0] as f32, SCREEN[1] as f32);
    if area.x <= 0.0 || area.y <= 0.0 {
        return native;
    }
    match (scaling, keep_aspect) {
        (Scaling::Stretch, false) => area,
        (Scaling::Stretch, true) | (Scaling::Fit, _) => {
            let factor = (area.x / native.x).min(area.y / native.y);
            native * factor
        }
        (Scaling::Integer, _) => {
            // Nunca some: abaixo de uma vez o tamanho original, encolhe proporcional em vez de
            // não caber, porque uma janela pequena não pode esconder o jogo.
            let factor = (area.x / native.x).min(area.y / native.y);
            match factor >= 1.0 {
                true => native * factor.floor(),
                false => native * factor,
            }
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn teclado_e_controle_compartilham_um_aperto() {
        use std::collections::HashSet;
        let mut pad = Pad::default();
        pad.press(Pad::button_by_name("right").unwrap(), true);
        let keyboard = HashSet::from([egui::Key::ArrowRight, egui::Key::Num4]);
        let mut delivered = HashSet::new();
        let active = App::avks_ativos(&keyboard, &[pad]);
        assert_eq!(
            App::transicoes_de_teclas(&mut delivered, active.clone()),
            vec![(crate::input::avk::RODA_SEGUINTE, true)]
        );
        assert!(App::transicoes_de_teclas(&mut delivered, active).is_empty());
        // Soltar o teclado não solta um comando ainda mantido pelo controle.
        let active = App::avks_ativos(&HashSet::new(), &[pad]);
        assert!(App::transicoes_de_teclas(&mut delivered, active).is_empty());
        assert_eq!(
            App::transicoes_de_teclas(&mut delivered, HashSet::new()),
            vec![(crate::input::avk::RODA_SEGUINTE, false)]
        );
    }

    /// Só a transição vira tecla: segurar o direcional não repete.
    #[test]
    fn o_direcional_manda_tecla_uma_vez() {
        let mut antes = Pad::default();
        let mut agora = Pad::default();
        let direita = Pad::button_by_name("right").unwrap();
        agora.press(direita, true);
        assert_eq!(
            App::teclas_do_controle(&antes, &agora),
            vec![(crate::input::avk::RODA_SEGUINTE, true)]
        );
        antes = agora;
        assert!(App::teclas_do_controle(&antes, &agora).is_empty());
        agora.press(direita, false);
        assert_eq!(
            App::teclas_do_controle(&antes, &agora),
            vec![(crate::input::avk::RODA_SEGUINTE, false)]
        );
    }

    /// Os dígitos saem da ordem do `egui::Key`, e do `AVK_0` em diante. As duas listas são
    /// contíguas hoje; se uma deixar de ser, é aqui que se descobre.
    #[test]
    fn digitos_viram_avk() {
        use eframe::egui::Key;
        assert_eq!(App::avk_de(Key::Num0), Some(crate::input::avk::ZERO));
        assert_eq!(App::avk_de(Key::Num7), Some(crate::input::avk::ZERO + 7));
        assert_eq!(App::avk_de(Key::Num9), Some(crate::input::avk::ZERO + 9));
        assert_eq!(App::avk_de(Key::Backspace), Some(crate::input::avk::CLR));
        assert_eq!(App::avk_de(Key::Escape), None);
        assert_eq!(App::avk_de(Key::P), None);
    }
    use super::*;

    #[test]
    fn a_ampliacao_inteira_so_usa_multiplos_exatos() {
        // Numa janela de 1500x1100 cabem duas vezes a tela de 640x480, e não duas e pouco.
        let size = placement(egui::vec2(1500.0, 1100.0), Scaling::Integer, true);
        assert_eq!(size, egui::vec2(1280.0, 960.0));
    }

    #[test]
    fn a_ampliacao_inteira_encolhe_quando_nao_cabe_uma_vez() {
        // Uma janela menor que a tela não pode esconder o jogo, então ali ela encolhe.
        let size = placement(egui::vec2(320.0, 240.0), Scaling::Integer, true);
        assert_eq!(size, egui::vec2(320.0, 240.0));
    }

    #[test]
    fn caber_na_janela_mantem_a_proporcao() {
        // Janela larga demais: sobra borda dos lados, não estica.
        let size = placement(egui::vec2(1920.0, 480.0), Scaling::Fit, true);
        assert_eq!(size, egui::vec2(640.0, 480.0));
    }

    #[test]
    fn preencher_so_deforma_quando_a_proporcao_e_dispensada() {
        let area = egui::vec2(1000.0, 500.0);
        assert_eq!(placement(area, Scaling::Stretch, false), area);
        // Com a proporção mantida, "preencher" vira "caber".
        assert_eq!(
            placement(area, Scaling::Stretch, true),
            placement(area, Scaling::Fit, true)
        );
    }
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
