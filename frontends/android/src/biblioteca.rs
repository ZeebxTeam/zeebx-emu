//! A grade de jogos.
//!
//! Quem acha os jogos, lê os títulos e tira as capas dos `.mif` é o `library::scan` do núcleo —
//! o mesmo que a janela do desktop usa. Aqui só se decide como isso ocupa uma tela de mão.
//!
//! Duas decisões que não vêm do desktop. A primeira é o tamanho: o cartão tem 170 pontos e o
//! título 15, porque num portátil segurado com as duas mãos o que se lê de relance é a capa, e
//! o nome serve para confirmar. A segunda é o **direcional**: num aparelho com botões, chegar
//! ao jogo sem encostar na tela é o caminho normal, não a alternativa — então a grade tem um
//! cartão escolhido, o direcional o move e o botão 1 abre.

use std::collections::HashMap;
use std::path::PathBuf;

use zeebx::ui::library::Game;

use crate::tema::ALVO;
use crate::{Emulador, Onde, sistema, textura_de};

/// Largura de um cartão, em pontos.
const LARGURA_DO_CARTAO: f32 = 170.0;

/// Altura da área da capa dentro do cartão. As capas de `.mif` são 65×42, então a moldura é
/// larga de propósito: uma alta deixaria o ícone perdido no meio.
const ALTURA_DA_CAPA: f32 = 104.0;

impl Emulador {
    /// A tela inicial: a barra de cima, a busca e a grade.
    pub(crate) fn biblioteca(&mut self, ctx: &egui::Context) {
        let abrir_escolhido = self.direcional(ctx, self.filtrados.len());
        self.barra(ctx);
        self.avisos(ctx);

        if self.jogos.is_empty() {
            let varrendo = self.varredura.is_some();
            egui::CentralPanel::default().show(ctx, |ui| {
                if varrendo {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(self.tr("library.scanning"));
                    });
                } else {
                    self.biblioteca_vazia(ui);
                }
            });
            return;
        }

        if self.filtrados.is_empty() {
            let mensagem = self
                .catalogo
                .format("library.no_match", &[("query", &self.busca)]);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(mensagem);
                });
            });
            return;
        }

        let escolhido = self.selecionado.min(self.filtrados.len() - 1);
        let rolar = std::mem::take(&mut self.rolar);
        let escolhido_por_controle = self
            .filtrados
            .get(escolhido)
            .copied()
            .filter(|_| abrir_escolhido);
        let mut abrir = None;
        let mut colunas_vistas = 0usize;
        {
            let jogos = &self.jogos;
            let filtrados = &self.filtrados;
            let capas = &mut self.capas;
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    // Quantas colunas cabem, nunca menos de duas: numa tela estreita um cartão por
                    // linha vira lista, e a grade deixa de ser grade.
                    let colunas =
                        ((ui.available_width() / LARGURA_DO_CARTAO).floor() as usize).max(2);
                    colunas_vistas = colunas;
                    let largura = (ui.available_width()
                        - ui.spacing().item_spacing.x * (colunas as f32 - 1.0))
                        / colunas as f32;
                    egui::Grid::new("grade")
                        .num_columns(colunas)
                        .spacing(ui.spacing().item_spacing)
                        .show(ui, |ui| {
                            for (posicao, &indice) in filtrados.iter().enumerate() {
                                let jogo = &jogos[indice];
                                let marcado = posicao == escolhido;
                                let rect = cartao(ui, jogo, capas, largura, marcado);
                                if rect.1 {
                                    abrir = Some(jogo.path.clone());
                                }
                                if marcado && rolar {
                                    ui.scroll_to_rect(rect.0, Some(egui::Align::Center));
                                }
                                if (posicao + 1) % colunas == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(12.0);
                });
            });
        }
        self.colunas = colunas_vistas.max(1);
        if let Some(indice) = escolhido_por_controle {
            abrir = Some(self.jogos[indice].path.clone());
        }
        if let Some(caminho) = abrir {
            self.abre(&caminho);
        }
    }

    /// O direcional andando pela grade. Devolve se o botão de abrir foi apertado.
    ///
    /// As colunas são as da última pintura — a grade sabe quantas são, e este trecho roda antes
    /// dela. Quatro é um palpite razoável para a primeira volta; da segunda em diante o número
    /// é o de verdade.
    fn direcional(&mut self, ctx: &egui::Context, quantos: usize) -> bool {
        if quantos == 0 {
            return false;
        }
        let colunas = self.colunas.max(1);

        // **Quem tem o foco manda.** A barra de cima -- ajustes, recarregar e a busca -- é feita
        // de widgets do egui, e lá o foco é dele. Enquanto ele estiver com alguém, a grade não
        // toca nas setas: senão o cursor daqui andaria junto com o foco de lá, e a busca nunca
        // conseguiria editar texto. Era o que este comentário prometia e ninguém tinha escrito.
        //
        // A seta para baixo é a porta de volta: solta o foco e o cursor da grade reassume.
        if let Some(foco) = ctx.memory(|m| m.focused()) {
            if ctx.input(|entrada| entrada.key_pressed(egui::Key::ArrowDown)) {
                ctx.memory_mut(|m| m.surrender_focus(foco));
            }
            return false;
        }

        let (mut passo, mut abrir) = (0i64, false);
        ctx.input(|entrada| {
            if entrada.key_pressed(egui::Key::ArrowRight) {
                passo += 1;
            }
            if entrada.key_pressed(egui::Key::ArrowLeft) {
                passo -= 1;
            }
            if entrada.key_pressed(egui::Key::ArrowDown) {
                passo += colunas as i64;
            }
            if entrada.key_pressed(egui::Key::ArrowUp) {
                passo -= colunas as i64;
            }
            abrir = entrada.key_pressed(egui::Key::Enter);
        });
        // Subir da primeira fila sai da grade e entra na barra: é o caminho para o recarregar,
        // os ajustes e a busca, que antes só o dedo alcançava. O `clamp` de baixo prendia o
        // cursor na fila de cima, e a barra ficava inalcançável por controle.
        if passo < 0 && self.selecionado < colunas {
            if let Some(id) = self.foco_da_barra {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            return abrir;
        }
        if passo != 0 {
            let destino = self.selecionado as i64 + passo;
            self.selecionado = destino.clamp(0, quantos as i64 - 1) as usize;
            self.rolar = true;
        }
        self.selecionado = self.selecionado.min(quantos - 1);
        abrir
    }

    /// A barra de cima: o nome, a contagem, a busca e o caminho para os ajustes.
    fn barra(&mut self, ctx: &egui::Context) {
        let mut recarregar = false;
        let mut busca_mudou = false;
        let varrendo = self.varredura.is_some();
        egui::TopBottomPanel::top("barra").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Zeebx");
                ui.add_space(10.0);

                let total = self.jogos.len();
                let mostrados = self.filtrados.len();
                let contagem = match mostrados == total {
                    true => self
                        .catalogo
                        .format("library.count", &[("count", &total.to_string())]),
                    false => self.catalogo.format(
                        "library.count_filtered",
                        &[
                            ("shown", &mostrados.to_string()),
                            ("count", &total.to_string()),
                        ],
                    ),
                };
                ui.label(
                    egui::RichText::new(contagem)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                if varrendo {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new(self.tr("library.scanning"))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ajustes = ui.add_sized(
                        [ALVO, ALVO],
                        egui::Button::new(egui::RichText::new("⛭").size(22.0)),
                    );
                    // A porta de entrada da barra para quem usa controle: é este que recebe o
                    // foco quando a seta sobe da grade, e daí as setas andam entre os botões e a
                    // busca pelo caminho normal do egui.
                    self.foco_da_barra = Some(ajustes.id);
                    if ajustes.clicked() {
                        self.onde = Onde::Ajustes;
                    }
                    if ui
                        .add_enabled(
                            !varrendo,
                            egui::Button::new(egui::RichText::new("⟳").size(22.0))
                                .min_size(egui::vec2(ALVO, ALVO)),
                        )
                        .clicked()
                    {
                        recarregar = true;
                    }
                    ui.add_space(4.0);
                    // A busca fica com o que sobra da linha: é o campo que mais serve numa
                    // coleção grande e o que menos serve numa pequena.
                    let largura = ui.available_width().min(280.0);
                    let campo = egui::TextEdit::singleline(&mut self.busca)
                        .desired_width(largura)
                        .margin(egui::Margin::symmetric(12, 10))
                        .hint_text("🔎  Buscar");
                    if ui.add(campo).changed() {
                        busca_mudou = true;
                    }
                });
            });
            ui.add_space(8.0);
        });
        if busca_mudou {
            self.selecionado = 0;
            self.rolar = true;
            self.atualiza_filtro();
        }
        if recarregar {
            self.recarrega();
        }
    }

    /// O que precisa ser dito antes da grade: a permissão que falta, e o último jogo que não
    /// abriu.
    fn avisos(&mut self, ctx: &egui::Context) {
        // Sem o acesso concedido, a listagem funciona e a abertura não: o usuário veria uma
        // biblioteca cheia em que nenhum jogo abre. Melhor dizer isso antes.
        let sem_acesso = !sistema::tem_acesso_a_arquivos(&self.app);
        if !sem_acesso && self.erro.is_none() {
            return;
        }
        egui::TopBottomPanel::top("avisos").show(ctx, |ui| {
            ui.add_space(8.0);
            if let Some(erro) = self.erro.clone() {
                ui.horizontal(|ui| {
                    if ui.add_sized([ALVO, ALVO], egui::Button::new("✕")).clicked() {
                        self.erro = None;
                    }
                    ui.colored_label(ui.visuals().error_fg_color, erro);
                });
            }
            if sem_acesso {
                ui.horizontal(|ui| {
                    if ui
                        .add_sized([200.0, ALVO], egui::Button::new("Conceder acesso"))
                        .clicked()
                    {
                        sistema::pede_acesso_a_arquivos(&self.app);
                    }
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "O Zeebx só enxerga a própria pasta.",
                    );
                });
            }
            ui.add_space(8.0);
        });
    }

    /// A tela sem nenhum jogo: ou não há pasta, ou a pasta está vazia.
    fn biblioteca_vazia(&mut self, ui: &mut egui::Ui) {
        let pasta = self.roms();
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            if pasta.as_os_str().is_empty() {
                ui.label(self.tr("library.no_folder"));
            } else {
                ui.label(
                    self.catalogo
                        .format("library.empty", &[("folder", &pasta.display().to_string())]),
                );
            }
            ui.add_space(20.0);
            if ui
                .add_sized(
                    [300.0, ALVO + 8.0],
                    egui::Button::new(self.tr("settings.roms_folder")),
                )
                .clicked()
            {
                self.onde = Onde::Seletor(match pasta.is_dir() {
                    true => pasta,
                    false => PathBuf::from("/sdcard"),
                });
            }
        });
    }

}

/// Um cartão. Devolve onde ele ficou e se foi escolhido.
///
/// É função solta, e não método, porque desenhar um cartão precisa de um jogo emprestado e do
/// cache de capas ao mesmo tempo — dois campos do mesmo `Emulador`, que só se pega juntos
/// quando eles chegam separados.
///
/// O cartão inteiro é **uma** célula da grade, e por isso ele é desenhado com o pincel em vez
/// de com widgets: cada widget acrescentado direto ao `ui` de uma `Grid` vira uma célula nova,
/// e um rótulo a mais por cartão dobrava as colunas.
fn cartao(
    ui: &mut egui::Ui,
    jogo: &Game,
    capas: &mut HashMap<PathBuf, egui::TextureHandle>,
    largura: f32,
    marcado: bool,
) -> (egui::Rect, bool) {
    let altura = ALTURA_DA_CAPA + 42.0;
    let (rect, resposta) =
        ui.allocate_exact_size(egui::vec2(largura, altura), egui::Sense::click());
    let estilo = match resposta.hovered() || marcado {
        true => ui.visuals().widgets.hovered,
        false => ui.visuals().widgets.inactive,
    };
    ui.painter().rect_filled(rect, 12.0, estilo.weak_bg_fill);
    // O escolhido pelo direcional ganha uma borda: sem ponteiro na tela, é o único jeito de
    // saber em qual jogo o botão vai bater.
    if marcado {
        ui.painter().rect_stroke(
            rect,
            12.0,
            egui::Stroke::new(2.5_f32, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }

    let dentro = rect.shrink(8.0);
    let moldura = egui::Rect::from_min_size(dentro.min, egui::vec2(dentro.width(), ALTURA_DA_CAPA));
    desenha_capa(ui, jogo, capas, moldura);

    // Um título longo é cortado com reticências, não quebrado em duas linhas: cartões de
    // alturas diferentes na mesma linha estragariam a grade.
    let texto = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: dentro.width(),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        },
        ..egui::text::LayoutJob::simple_singleline(
            jogo.title.clone(),
            egui::FontId::proportional(15.0),
            estilo.text_color(),
        )
    };
    let galeria = ui.painter().layout_job(texto);
    let onde = egui::pos2(
        dentro.center().x - galeria.size().x / 2.0,
        moldura.bottom() + 8.0,
    );
    ui.painter().galley(onde, galeria, estilo.text_color());

    (rect, resposta.clicked())
}

/// A capa do cartão, ou uma moldura com a inicial quando o jogo não tem nenhuma.
fn desenha_capa(
    ui: &mut egui::Ui,
    jogo: &Game,
    capas: &mut HashMap<PathBuf, egui::TextureHandle>,
    moldura: egui::Rect,
) {
    let textura = match capas.get(&jogo.path) {
        Some(textura) => Some(textura.clone()),
        None => jogo.art.as_ref().map(|arte| {
            let textura = textura_de(ui.ctx(), &jogo.title, arte);
            capas.insert(jogo.path.clone(), textura.clone());
            textura
        }),
    };

    let Some(textura) = textura else {
        // Sem capa, a inicial num retângulo: o cartão continua do mesmo tamanho, e a grade não
        // fica com buracos.
        ui.painter()
            .rect_filled(moldura, 8.0, egui::Color32::from_black_alpha(90));
        let inicial = jogo
            .title
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            moldura.center(),
            egui::Align2::CENTER_CENTER,
            inicial,
            egui::FontId::proportional(34.0),
            ui.visuals().weak_text_color(),
        );
        return;
    };

    // A capa cabe na moldura sem esticar: o ícone de um `.mif` é largo e uma capa de verdade é
    // alta — as duas precisam caber na mesma caixa.
    let [largura_px, altura_px] = textura.size();
    let escala = (moldura.width() / largura_px as f32).min(moldura.height() / altura_px as f32);
    let desenhada = egui::vec2(largura_px as f32 * escala, altura_px as f32 * escala);
    ui.painter().image(
        textura.id(),
        egui::Rect::from_center_size(moldura.center(), desenhada),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}
