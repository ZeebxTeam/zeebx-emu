//! As configurações, para um polegar.
//!
//! O que se mexe aqui é o **mesmo** [`zeebx::ui::settings::Settings`] do desktop, gravado no
//! mesmo formato, e os rótulos saem do mesmo catálogo de idiomas. O que não é o mesmo é a
//! forma: no desktop as abas ficam em cima e cada opção é uma caixinha com a legenda ao lado;
//! aqui as seções ficam numa trilha à esquerda — o aparelho é travado em paisagem, e em
//! paisagem a largura é o que sobra — e cada opção é uma faixa da largura inteira.
//!
//! O que ficou de fora ficou por não existir aqui: as opções de janela não valem numa tela só,
//! e o mapeamento dos controles, o Discord e as atualizações ainda não estão ligados neste
//! frontend. A seção "Controles" daqui é a dos controles desenhados na tela.

use zeebx::input::bindings::Source;
use zeebx::ui::settings::{self, ModoDosControlesNaTela, Proporcao, Scaling, Settings};
use zeebx::velocidade::turbo::{self, ModoDoTurbo};
use zeebx::velocidade::rewind::AjustesDoRewind;
use zeebx::velocidade::{Avanco, Frameskip, LimiteFps, ModoDoAtalho};

use crate::tema::ALVO;
use crate::{Emulador, Onde, sistema, widgets};

/// Um atalho que o controle físico pode acionar: os dois da sessão e a tecla de turbo do jogador
/// 1. O nome do botão é o do desktop — ver `crate::entrada::FISICOS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtalhoFisico {
    Avancar,
    Voltar,
    Turbo,
}

impl AtalhoFisico {
    const TODOS: [Self; 3] = [Self::Avancar, Self::Voltar, Self::Turbo];

    fn chave(self) -> &'static str {
        match self {
            Self::Avancar => "controls.shortcut.fast_forward",
            Self::Voltar => "controls.shortcut.rewind",
            Self::Turbo => "button.turbo",
        }
    }

    fn origens(self, settings: &Settings) -> Vec<Source> {
        match self {
            Self::Avancar => settings.atalhos.avancar.clone(),
            Self::Voltar => settings.atalhos.voltar.clone(),
            Self::Turbo => settings
                .controls
                .player(0)
                .map(|jogador| jogador.sources(turbo::BOTAO_DO_TURBO).to_vec())
                .unwrap_or_default(),
        }
    }

    fn origens_mut(self, settings: &mut Settings) -> &mut Vec<Source> {
        match self {
            Self::Avancar => &mut settings.atalhos.avancar,
            Self::Voltar => &mut settings.atalhos.voltar,
            Self::Turbo => settings
                .controls
                .player_mut(0)
                .buttons
                .entry(turbo::BOTAO_DO_TURBO.to_string())
                .or_default(),
        }
    }
}

impl Emulador {
    /// Um botão do controle físico desceu: com uma captura aberta, ele vira o atalho.
    ///
    /// **Só os botões ficam**, as teclas gravadas (o F10 do desktop) continuam no arquivo: o mesmo
    /// `settings.json` pode ir do celular para o computador. O botão que confirmou a captura
    /// também chega ao egui como "Enter" na mesma rodada, e reabriria a captura que acabou de
    /// fechar: o `capturado_agora` faz a aba ignorar o clique desta rodada.
    pub(crate) fn capturou_botao(&mut self, nome: &'static str) {
        let Some(qual) = self.capturando.take() else {
            return;
        };
        let origens = qual.origens_mut(&mut self.settings);
        origens.retain(|fonte| fonte.is_key());
        origens.push(Source::button(nome));
        self.capturado_agora = true;
        self.salva();
    }
}

/// As seções, na mesma ordem das abas do desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Aba {
    Geral,
    Graficos,
    Velocidade,
    Audio,
    Controles,
    Depuracao,
    Sobre,
}

impl Aba {
    const TODAS: [Self; 7] = [
        Self::Geral,
        Self::Graficos,
        Self::Velocidade,
        Self::Audio,
        Self::Controles,
        Self::Depuracao,
        Self::Sobre,
    ];

    /// A chave do nome da seção no catálogo.
    fn chave(self) -> &'static str {
        match self {
            Self::Geral => "settings.tab.general",
            Self::Graficos => "settings.tab.graphics",
            Self::Velocidade => "settings.tab.speed",
            Self::Audio => "settings.tab.audio",
            Self::Controles => "settings.tab.controls",
            Self::Depuracao => "settings.tab.debug",
            Self::Sobre => "settings.tab.about",
        }
    }

    /// Um sinal para a trilha. Ícone reconhecido de longe vale mais que nome lido de perto.
    fn sinal(self) -> &'static str {
        match self {
            Self::Geral => "⛭",
            Self::Graficos => "🖵",
            Self::Velocidade => "»",
            Self::Audio => "🔊",
            Self::Controles => "🎮",
            Self::Depuracao => "⏱",
            Self::Sobre => "ℹ",
        }
    }
}

impl Emulador {
    pub(crate) fn ajustes(&mut self, ctx: &egui::Context) {
        // A trilha à esquerda: o "voltar" em cima e as seções embaixo, cada uma um alvo de
        // toque inteiro. Em paisagem ela custa 150 pontos de largura e devolve a tela toda de
        // altura para o conteúdo — bem melhor que uma fila de abas que rola de lado.
        egui::SidePanel::left("trilha")
            .exact_width(150.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(10.0);
                let voltar =
                    ui.add_sized([ui.available_width(), ALVO], egui::Button::new("◀  Voltar"));
                if voltar.clicked() {
                    self.onde = Onde::Biblioteca;
                }
                // **Sem foco não há navegação por setas.** O egui move o foco na direção da seta,
                // mas só a partir de um foco que já exista -- do nada, quem o concede é o `Tab`,
                // que nenhum controle produz. Entrando na tela sem foco, o direcional não movia
                // coisa nenhuma aqui. Damos o primeiro, e daí o egui se vira: seta anda, Enter
                // aperta.
                if ctx.memory(|m| m.focused()).is_none() {
                    voltar.request_focus();
                }
                ui.add_space(12.0);
                for aba in Aba::TODAS {
                    let nome = format!("{}  {}", aba.sinal(), self.catalogo.get(aba.chave()));
                    let botao = egui::Button::new(nome)
                        .min_size(egui::vec2(ui.available_width(), ALVO))
                        .selected(self.aba == aba);
                    if ui.add(botao).clicked() {
                        self.aba = aba;
                        // Trocar de seção fecha a dica aberta: ela era de outra tela.
                        self.dica = None;
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let titulo = self.catalogo.get(self.aba.chave()).to_string();
            ui.add_space(10.0);
            ui.heading(titulo);
            ui.add_space(6.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mudou = match self.aba {
                    Aba::Geral => self.aba_geral(ui),
                    Aba::Graficos => self.aba_graficos(ui),
                    Aba::Velocidade => self.aba_velocidade(ui),
                    Aba::Audio => self.aba_audio(ui),
                    Aba::Controles => self.aba_controles(ui),
                    Aba::Depuracao => self.aba_depuracao(ui),
                    Aba::Sobre => self.aba_sobre(ui),
                };
                if mudou {
                    self.salva();
                }
                ui.add_space(24.0);
            });
        });
    }

    /// Idioma e pasta de ROMs.
    fn aba_geral(&mut self, ui: &mut egui::Ui) -> bool {
        let mut mudou = false;

        widgets::secao(ui, self.catalogo.get("settings.roms_folder"));
        let pasta = self.roms();
        let onde = match pasta.as_os_str().is_empty() {
            true => self.catalogo.get("library.no_folder").to_string(),
            false => pasta.display().to_string(),
        };
        if widgets::navega(ui, self.catalogo.get("settings.browse"), &onde) {
            self.onde = Onde::Seletor(match pasta.is_dir() {
                true => pasta,
                false => std::path::PathBuf::from("/sdcard"),
            });
        }
        let contagem = self
            .catalogo
            .format("library.count", &[("count", &self.jogos.len().to_string())]);
        let pode_varrer = self.varredura.is_none();
        if ui
            .add_enabled_ui(pode_varrer, |ui| {
                widgets::navega(ui, self.catalogo.get("library.rescan"), &contagem)
            })
            .inner
        {
            self.recarrega();
        }

        widgets::secao(ui, self.catalogo.get("settings.language"));
        // Um idioma por faixa, marcado o que vale. São dois ou três: uma lista de escolha é
        // mais direta que uma caixa que abre por cima da tela.
        let idiomas: Vec<(String, String)> = self
            .catalogo
            .languages()
            .iter()
            .map(|idioma| (idioma.code.clone(), idioma.name.clone()))
            .collect();
        let atual = self.catalogo.current().to_string();
        let mut escolhido = None;
        for (codigo, nome) in &idiomas {
            let botao = egui::Button::new(nome)
                .min_size(egui::vec2(ui.available_width(), ALVO))
                .selected(*codigo == atual);
            if ui.add(botao).clicked() && *codigo != atual {
                escolhido = Some(codigo.clone());
            }
        }
        if let Some(codigo) = escolhido {
            self.catalogo.select(&codigo);
            self.settings.language = Some(codigo);
            mudou = true;
        }

        ui.add_space(14.0);
        ui.label(
            egui::RichText::new(self.catalogo.format(
                "settings.saved_at",
                &[("path", &self.arquivo.display().to_string())],
            ))
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        mudou
    }

    /// O ritmo do jogo contra o relógio do mundo. Ver `docs/implementacao/24-velocidade.md`.
    fn aba_velocidade(&mut self, ui: &mut egui::Ui) -> bool {
        let catalogo = &self.catalogo;
        let velocidade = &mut self.settings.velocidade;
        let dica = &mut self.dica;
        let mut mudou = false;

        let limites: Vec<(LimiteFps, String)> = LimiteFps::TODOS
            .iter()
            .map(|limite| (*limite, catalogo.get(limite.chave()).to_string()))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "fps_limit",
            catalogo.get("speed.fps_limit"),
            Some(catalogo.get("speed.fps_limit.hint")),
            dica,
            &mut velocidade.limite_fps,
            &limites,
        );
        let pulos: Vec<(Frameskip, String)> = Frameskip::todos()
            .into_iter()
            .map(|pulo| (pulo, settings::rotulo_do_frameskip(pulo, catalogo)))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "frameskip",
            catalogo.get("speed.frameskip"),
            Some(catalogo.get("speed.frameskip.hint")),
            dica,
            &mut velocidade.frameskip,
            &pulos,
        );

        widgets::secao(ui, catalogo.get("speed.ff"));
        let proporcoes: Vec<(u8, String)> = Avanco::escolhas()
            .into_iter()
            .map(|n| match n {
                0 => (n, catalogo.get("speed.ff.unlimited").to_string()),
                n => (n, format!("{n}x")),
            })
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "ff_ratio",
            catalogo.get("speed.ff.ratio"),
            Some(catalogo.get("speed.ff.hint")),
            dica,
            &mut velocidade.avanco.proporcao,
            &proporcoes,
        );
        let modos: Vec<(ModoDoAtalho, String)> = ModoDoAtalho::TODOS
            .iter()
            .map(|modo| (*modo, catalogo.get(modo.chave()).to_string()))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "ff_mode",
            catalogo.get("speed.ff.mode"),
            None,
            dica,
            &mut velocidade.avanco.modo,
            &modos,
        );
        mudou |= widgets::interruptor(
            ui,
            "ff_mute",
            catalogo.get("speed.ff.mute"),
            Some(catalogo.get("speed.ff.mute.hint")),
            dica,
            &mut velocidade.avanco.sem_som,
        );

        widgets::secao(ui, catalogo.get("speed.rewind"));
        mudou |= widgets::interruptor(
            ui,
            "rewind_on",
            catalogo.get("speed.rewind.on"),
            Some(catalogo.get("speed.rewind.on.hint")),
            dica,
            &mut velocidade.rewind.ligado,
        );
        let ligado = velocidade.rewind.ligado;
        ui.add_enabled_ui(ligado, |ui| {
            let intervalos: Vec<(u32, String)> = AjustesDoRewind::INTERVALOS_MS
                .iter()
                .map(|&ms| {
                    let texto = match ms % 1000 {
                        0 => catalogo.format("speed.rewind.every_s", &[("n", &(ms / 1000).to_string())]),
                        _ => catalogo.format("speed.rewind.every_ms", &[("n", &ms.to_string())]),
                    };
                    (ms, texto)
                })
                .collect();
            mudou |= widgets::segmentado(
                ui,
                "rewind_interval",
                catalogo.get("speed.rewind.interval"),
                None,
                dica,
                &mut velocidade.rewind.intervalo_ms,
                &intervalos,
            );
            let memorias: Vec<(u32, String)> = AjustesDoRewind::MEMORIAS_MB
                .iter()
                .map(|&mb| (mb, format!("{mb} MB")))
                .collect();
            mudou |= widgets::segmentado(
                ui,
                "rewind_memory",
                catalogo.get("speed.rewind.memory"),
                Some(catalogo.get("speed.rewind.memory.hint")),
                dica,
                &mut velocidade.rewind.memoria_mb,
                &memorias,
            );
        });
        mudou
    }

    /// O que muda o desenho do jogo.
    fn aba_graficos(&mut self, ui: &mut egui::Ui) -> bool {
        // Os três campos vêm separados de propósito: o catálogo é lido, os gráficos são
        // escritos e a dica aberta é trocada no mesmo trecho, e só emprestando cada um por si
        // isso passa pelo verificador.
        let catalogo = &self.catalogo;
        let graficos = &mut self.settings.graphics;
        let dica = &mut self.dica;
        let mut mudou = false;

        // Sem título de seção antes de um segmentado: a própria faixa dele já traz o nome e o
        // `?`, e os dois juntos eram a mesma palavra duas vezes.
        let modos: Vec<(Scaling, String)> = Scaling::ALL
            .iter()
            .map(|modo| (*modo, catalogo.get(modo.key()).to_string()))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "scaling",
            catalogo.get("graphics.scaling"),
            Some(catalogo.get("graphics.scaling.integer.hint")),
            dica,
            &mut graficos.scaling,
            &modos,
        );
        // O 4:3 só tem efeito sobre "Preencher": nos outros dois modos a proporção já é
        // mantida, e mostrar a opção ali dava um botão que não faz nada. Pior, ele era a razão
        // de "Preencher" parecer quebrado — ligado, ele desfaz o preenchimento.
        if graficos.scaling == Scaling::Stretch {
            mudou |= widgets::interruptor(
                ui,
                "keep_aspect",
                catalogo.get("graphics.keep_aspect"),
                None,
                dica,
                &mut graficos.keep_aspect,
            );
        }
        mudou |= widgets::interruptor(
            ui,
            "smooth",
            catalogo.get("graphics.smooth"),
            None,
            dica,
            &mut graficos.smooth,
        );

        widgets::secao(ui, "3D");
        // A névoa vale nos dois rasterizadores; o resto desta seção, só na placa.
        mudou |= widgets::interruptor(
            ui,
            "fog",
            catalogo.get("graphics.fog"),
            Some(catalogo.get("graphics.fog.hint")),
            dica,
            &mut graficos.neblina,
        );
        mudou |= widgets::interruptor(
            ui,
            "gpu_rasterizer",
            catalogo.get("graphics.gpu_rasterizer"),
            Some(catalogo.get("graphics.gpu_rasterizer.hint")),
            dica,
            &mut graficos.gpu_rasterizer,
        );
        // **Tudo daqui para baixo é vazio no software.** O `define_proporcao`, o
        // `define_escala`, o `define_antialias` e o anisotrópico do `Rasterizador` são funções
        // sem corpo quando o 3D roda na CPU: só a placa sabe fazer isso. Apagá-los enquanto ela
        // está desligada é dizer a verdade — antes eles mexiam e nada acontecia.
        let com_placa = graficos.gpu_rasterizer;
        // Apagar as opções não basta: um controle apagado se lê como quebrado, não como
        // "depende daquele ali". A linha diz de que ele depende.
        if !com_placa {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(
                    egui::RichText::new(format!(
                        "↑  {} precisa estar ligado para o que vem abaixo.",
                        catalogo.get("graphics.gpu_rasterizer")
                    ))
                    .small()
                    .color(ui.visuals().warn_fg_color),
                );
            });
            ui.add_space(4.0);
        }
        ui.add_enabled_ui(com_placa, |ui| {
        mudou |= widgets::deslizante(
            ui,
            "resolucao",
            catalogo.get("graphics.internal_resolution"),
            Some(catalogo.get("graphics.internal_resolution.hint")),
            dica,
            &mut graficos.resolucao_interna,
            1..=4,
            |valor| format!("{valor}×"),
        );
        mudou |= widgets::deslizante(
            ui,
            "antialias",
            catalogo.get("graphics.antialias"),
            Some(catalogo.get("graphics.antialias.hint")),
            dica,
            &mut graficos.antialias,
            1..=4,
            |valor| format!("{valor}×"),
        );
        mudou |= widgets::deslizante(
            ui,
            "anisotropico",
            catalogo.get("graphics.anisotropic"),
            Some(catalogo.get("graphics.anisotropic.hint")),
            dica,
            &mut graficos.anisotropico,
            1..=16,
            |valor| format!("{valor}×"),
        );

        let proporcoes: Vec<(Proporcao, String)> = Proporcao::TODAS
            .iter()
            .map(|p| (*p, catalogo.get(p.chave()).to_string()))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "proporcao",
            catalogo.get("graphics.aspect"),
            Some(catalogo.get("graphics.aspect.hint")),
            dica,
            &mut graficos.proporcao,
            &proporcoes,
        );
        });

        // O que se mexe agora vale no jogo que já está aberto, e não só no próximo.
        if mudou {
            let graficos = self.settings.graphics.clone();
            if let Some(sessao) = &mut self.sessao {
                sessao.define_resolucao_interna(graficos.resolucao_interna as usize);
                sessao.define_melhorias(graficos.antialias as usize, graficos.anisotropico as usize);
                sessao.define_neblina(graficos.neblina);
                sessao.define_proporcao(graficos.proporcao.aspecto(16.0 / 9.0));
            }
        }
        mudou
    }

    fn aba_audio(&mut self, ui: &mut egui::Ui) -> bool {
        let catalogo = &self.catalogo;
        let audio = &mut self.settings.audio;
        let dica = &mut self.dica;
        let mut mudou = widgets::interruptor(
            ui,
            "audio",
            catalogo.get("audio.enabled"),
            Some(catalogo.get("audio.hint")),
            dica,
            &mut audio.enabled,
        );
        mudou |= widgets::deslizante(
            ui,
            "volume",
            catalogo.get("audio.volume"),
            None,
            dica,
            &mut audio.volume,
            0..=100,
            |valor| format!("{valor}%"),
        );

        // O banco é aberto quando o jogo abre: a dica diz que a troca vale para o próximo.
        widgets::secao(ui, self.catalogo.get("audio.soundfont"));
        ui.weak(self.catalogo.get("audio.soundfont.hint"));
        let banco = self.settings.audio.soundfont.clone();
        let mostrado = match &banco {
            Some(caminho) => caminho.display().to_string(),
            None => self.catalogo.get("audio.soundfont.auto").to_string(),
        };
        if widgets::navega(ui, self.catalogo.get("settings.browse"), &mostrado) {
            let inicio = banco
                .as_deref()
                .and_then(std::path::Path::parent)
                .filter(|pasta| pasta.is_dir())
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from("/sdcard"));
            self.onde = Onde::SeletorDeBanco(inicio);
        }
        if banco.is_some()
            && ui
                .add_sized(
                    [ui.available_width(), 52.0],
                    egui::Button::new(self.catalogo.get("audio.soundfont.clear")),
                )
                .clicked()
        {
            self.settings.audio.soundfont = None;
            mudou = true;
        }
        mudou |= widgets::interruptor(
            ui,
            "efeitos_midi",
            self.catalogo.get("audio.midi_effects"),
            Some(self.catalogo.get("audio.midi_effects.hint")),
            &mut self.dica,
            &mut self.settings.audio.midi_effects,
        );

        if mudou {
            let audio = self.settings.audio.clone();
            if let Some(sessao) = &mut self.sessao {
                if let Some(erro) = sessao.set_audio(audio.enabled, audio.volume) {
                    log::error!("sem som: {erro}");
                }
            }
        }
        mudou
    }

    /// Os controles desenhados na tela: quando aparecem, o tamanho, a transparência e onde ficam.
    fn aba_controles(&mut self, ui: &mut egui::Ui) -> bool {
        let catalogo = &self.catalogo;
        let toque = &mut self.settings.controles_na_tela;
        let dica = &mut self.dica;
        let modos: Vec<(ModoDosControlesNaTela, String)> = ModoDosControlesNaTela::TODOS
            .iter()
            .map(|modo| (*modo, catalogo.get(modo.chave()).to_string()))
            .collect();
        let mut mudou = widgets::segmentado(
            ui,
            "touch_mode",
            catalogo.get("touch.mode"),
            Some(catalogo.get("touch.mode.hint")),
            dica,
            &mut toque.modo,
            &modos,
        );
        // Tamanho e posição valem também no "nunca": quem desliga para jogar com controle pode
        // deixar tudo arrumado para a próxima vez sem controle.
        mudou |= widgets::deslizante(
            ui,
            "touch_scale",
            catalogo.get("touch.scale"),
            None,
            dica,
            &mut toque.escala,
            50..=200,
            |v| format!("{v}%"),
        );
        mudou |= widgets::deslizante(
            ui,
            "touch_opacity",
            catalogo.get("touch.opacity"),
            None,
            dica,
            &mut toque.opacidade,
            15..=100,
            |v| format!("{v}%"),
        );
        let arrumadas = match toque.posicoes.is_empty() && toque.tamanhos.is_empty() {
            true => catalogo.get("touch.layout.default"),
            false => catalogo.get("touch.layout.custom"),
        };
        if widgets::navega(ui, catalogo.get("touch.layout"), arrumadas) {
            self.onde = Onde::EditaToque;
        }
        // As peças de atalho, uma chave cada: o turbo e o rewind, além disso, só aparecem com a
        // função em uso. Ver `ControlesNaTela::mostra`.
        for (chave, rotulo, valor) in [
            ("touch_show_ff", "touch.show.ff", &mut toque.mostrar_avancar),
            ("touch_show_rewind", "touch.show.rewind", &mut toque.mostrar_voltar),
            ("touch_show_turbo", "touch.show.turbo", &mut toque.mostrar_turbo),
        ] {
            mudou |= widgets::interruptor(ui, chave, catalogo.get(rotulo), None, dica, valor);
        }

        // O turbo do jogador 1, o único que o Android tem. Ver `zeebx::velocidade::turbo`.
        widgets::secao(ui, catalogo.get("turbo"));
        let jogador = self.settings.controls.player_mut(0);
        let modos: Vec<(ModoDoTurbo, String)> = ModoDoTurbo::TODOS
            .iter()
            .map(|modo| (*modo, catalogo.get(modo.chave()).to_string()))
            .collect();
        mudou |= widgets::segmentado(
            ui,
            "turbo_mode",
            catalogo.get("turbo.mode"),
            Some(catalogo.get("turbo.hint.touch")),
            dica,
            &mut jogador.turbo,
            &modos,
        );
        let botoes: Vec<(&str, String)> = turbo::PULSAVEIS
            .iter()
            .map(|botao| (*botao, catalogo.get(&format!("button.{botao}")).to_string()))
            .collect();
        let mut botao = turbo::PULSAVEIS
            .iter()
            .copied()
            .find(|b| *b == jogador.botao_do_turbo)
            .unwrap_or(turbo::BOTAO_PADRAO);
        if widgets::segmentado(
            ui,
            "turbo_button",
            catalogo.get("turbo.button"),
            None,
            dica,
            &mut botao,
            &botoes,
        ) {
            jogador.botao_do_turbo = botao.to_string();
            mudou = true;
        }
        mudou |= widgets::deslizante(
            ui,
            "turbo_rate",
            catalogo.get("turbo.rate"),
            None,
            dica,
            &mut self.settings.velocidade.turbo_por_segundo,
            turbo::TOQUES_MENOS..=turbo::TOQUES_MAIS,
            |n| catalogo.format("turbo.rate.value", &[("n", &n.to_string())]),
        );

        // Os atalhos pelo controle físico: no celular não existe F10. Tocar numa linha espera o
        // próximo botão do controle; tocar de novo desiste. Ver [`Emulador::capturou_botao`].
        widgets::secao(ui, catalogo.get("settings.tab.shortcuts"));
        let capturado_agora = std::mem::take(&mut self.capturado_agora);
        for qual in AtalhoFisico::TODOS {
            let valor = match self.capturando == Some(qual) {
                true => catalogo.get("touch.shortcut.waiting").to_string(),
                false => {
                    let origens = qual.origens(&self.settings);
                    let botoes: Vec<String> = origens
                        .iter()
                        .filter(|fonte| !fonte.is_key())
                        .map(Source::label)
                        .collect();
                    match botoes.is_empty() {
                        true => catalogo.get("controls.unbound").to_string(),
                        false => botoes.join(", "),
                    }
                }
            };
            if widgets::navega(ui, catalogo.get(qual.chave()), &valor) && !capturado_agora {
                self.capturando = match self.capturando == Some(qual) {
                    true => None,
                    false => Some(qual),
                };
            }
        }
        if ui
            .add_sized([ui.available_width(), ALVO], egui::Button::new(catalogo.get("touch.shortcut.clear")))
            .clicked()
        {
            for qual in AtalhoFisico::TODOS {
                qual.origens_mut(&mut self.settings).retain(|fonte| fonte.is_key());
            }
            self.capturando = None;
            mudou = true;
        }
        mudou
    }

    fn aba_depuracao(&mut self, ui: &mut egui::Ui) -> bool {
        let catalogo = &self.catalogo;
        let debug = &mut self.settings.debug;
        let dica = &mut self.dica;
        let mut mudou = widgets::interruptor(
            ui,
            "overlay",
            catalogo.get("debug.overlay"),
            Some(catalogo.get("debug.hint")),
            dica,
            &mut debug.overlay,
        );
        // As linhas do painel só fazem sentido com o painel ligado.
        ui.add_enabled_ui(debug.overlay, |ui| {
            widgets::secao(ui, "O que mostrar");
            for (chave, rotulo, valor) in [
                ("speed", "debug.speed", &mut debug.speed),
                ("clock", "debug.clock", &mut debug.clock),
                ("memory", "debug.memory", &mut debug.memory),
                ("timeline", "debug.timeline", &mut debug.timeline),
            ] {
                mudou |= widgets::interruptor(ui, chave, catalogo.get(rotulo), None, dica, valor);
            }
        });
        mudou
    }

    fn aba_sobre(&mut self, ui: &mut egui::Ui) -> bool {
        ui.add_space(6.0);
        ui.label(self.tr("about.tagline"));
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                self.catalogo
                    .format("about.version", &[("version", env!("CARGO_PKG_VERSION"))]),
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(16.0);
        // Um link aqui não abre nada sozinho: é a `Intent` de VIEW que manda o endereço para o
        // navegador do aparelho.
        for (chave, endereco) in [
            ("about.repository", zeebx::ui::REPOSITORIO),
            ("about.discord", zeebx::ui::DISCORD),
        ] {
            if widgets::navega(ui, self.tr(chave), endereco) {
                sistema::abre_endereco(&self.app, endereco);
            }
        }
        false
    }
}
