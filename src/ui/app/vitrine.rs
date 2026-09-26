//! A biblioteca na tela principal: em grade, ou num slider no jeito da Z-Wheel.
//!
//! Os dois modos andam pelo controle e pelo teclado, e não só pelo mouse. O direcional, o manche
//! esquerdo e as setas escolhem; o botão 1, o Start, o Enter e o espaço abrem; o HOME abre a
//! Z-Wheel. O controle não gera evento no egui, então enquanto a biblioteca está à vista a
//! janela se redesenha sozinha para ler o controle.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::input::Pad;
use crate::ui::acervo;
use crate::ui::library::{self, Game};
use crate::ui::navegacao::{self, Comando, Navegacao};
use crate::ui::settings::ModoDaBiblioteca;
use crate::video::icon::Image;

use super::App;

/// Largura de um cartão da grade, e o quadro em que a imagem cabe: em pé, na proporção da caixa
/// dos jogos que a Z-Wheel traz (170×220). Um ícone de `.mif` fica centrado nele.
const CARD_WIDTH: f32 = 136.0;
const CARD_ART: f32 = 112.0;
const CARD_ART_ALTURA: f32 = 145.0;
/// Espaço reservado ao título, embaixo. Duas linhas.
const CARD_TEXT: f32 = 36.0;

/// De quanto em quanto a biblioteca lê o controle quando nada mais pede redesenho.
const LEITURA_DO_CONTROLE: Duration = Duration::from_millis(33);

/// O estado da biblioteca que não é configuração: a escolha, a animação e as texturas dos logos.
#[derive(Default)]
pub struct Vitrine {
    /// O jogo escolhido, sem limite: o slider dá a volta, e a posição do jogo na lista é o resto
    /// da divisão. Assim a animação da última para a primeira anda um passo, e não a lista toda.
    cursor: i64,
    /// Onde o slider está desenhado agora, correndo atrás do `cursor`.
    posicao: f32,
    /// O controle e o teclado virando comandos. Ver [`crate::ui::navegacao`].
    navegacao: Navegacao,
    /// A escolha mudou pela entrada neste quadro, e a grade precisa rolar até ela.
    rolar: bool,
    /// Acumula a roda do mouse até valer um passo no slider.
    roda: f32,
    pub logos: HashMap<u32, Option<egui::TextureHandle>>,
    pub classificacoes: HashMap<u32, Option<egui::TextureHandle>>,
    /// O que está escrito na busca da barra de cima. Vazia, a lista mostra todos os jogos.
    pub busca: String,
    /// O campo da busca, para saber se ele tem o foco: com ele, o teclado escreve e não navega.
    pub campo_da_busca: Option<egui::Id>,
}

impl Vitrine {
    fn indice(&self, total: usize) -> usize {
        self.cursor.rem_euclid(total.max(1) as i64) as usize
    }

    /// A busca mudou: a escolha volta ao primeiro resultado, sem a animação atravessar a lista.
    pub fn busca_mudou(&mut self) {
        self.cursor = 0;
        self.posicao = 0.0;
        self.rolar = true;
    }
}

impl App {
    /// Os jogos que a lista mostra, em ordem de título: todos menos a Z-Wheel, que abre pela
    /// barra de cima, e os que a busca deixa. Cada item é `(título, índice em self.games)`.
    fn lista_visivel(&self) -> Vec<(String, usize)> {
        let mut lista: Vec<(String, usize)> = (0..self.games.len())
            .filter(|&i| self.games[i].clsid != Some(crate::session::Z_WHEEL))
            .map(|i| (self.titulo_de(&self.games[i]), i))
            .filter(|(titulo, _)| library::casa_com_a_busca(titulo, &self.vitrine.busca))
            .collect();
        lista.sort_by_key(|(titulo, _)| titulo.to_lowercase());
        lista
    }

    pub(super) fn library_screen(&mut self, ui: &mut egui::Ui) {
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
        let lista = self.lista_visivel();

        let buscando = !self.vitrine.busca.trim().is_empty();
        ui.horizontal(|ui| {
            let contagem = if buscando {
                let total = self
                    .games
                    .iter()
                    .filter(|jogo| jogo.clsid != Some(crate::session::Z_WHEEL))
                    .count();
                self.catalog.format(
                    "library.count_filtered",
                    &[
                        ("shown", &lista.len().to_string()),
                        ("count", &total.to_string()),
                    ],
                )
            } else {
                self.catalog
                    .format("library.count", &[("count", &lista.len().to_string())])
            };
            ui.label(contagem);
            if ui.button(self.tr("library.rescan")).clicked() {
                self.rescan();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.weak(self.tr("library.controls_hint"));
            });
        });
        ui.separator();

        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
            ui.separator();
        }

        if lista.is_empty() && buscando {
            ui.label(
                self.catalog
                    .format("library.no_match", &[("query", self.vitrine.busca.trim())]),
            );
            return;
        }
        if lista.is_empty() {
            ui.label(
                self.catalog
                    .format("library.empty", &[("folder", &dir.display().to_string())]),
            );
            return;
        }

        let colunas = colunas_da_grade(ui);
        let mut escolhido = None;
        for comando in self.comandos_da_biblioteca(ui.ctx()) {
            let passo = match (comando, self.settings.biblioteca) {
                (Comando::Esquerda, _) => -1,
                (Comando::Direita, _) => 1,
                (Comando::Cima, ModoDaBiblioteca::Grade) => -(colunas as i64),
                (Comando::Baixo, ModoDaBiblioteca::Grade) => colunas as i64,
                (Comando::Cima | Comando::Baixo, ModoDaBiblioteca::Slider) => 0,
                (Comando::Abrir, _) => {
                    escolhido = Some(lista[self.vitrine.indice(lista.len())].1);
                    0
                }
                (Comando::ZWheel, _) => {
                    if let Some(caminho) = self.z_wheel.clone() {
                        self.abre_pela_biblioteca(caminho);
                        return;
                    }
                    0
                }
            };
            if passo != 0 {
                self.vitrine.cursor = match self.settings.biblioteca {
                    ModoDaBiblioteca::Slider => self.vitrine.cursor + passo,
                    // A grade não dá a volta: descer da última linha fica na última.
                    ModoDaBiblioteca::Grade => {
                        let atual = self.vitrine.indice(lista.len()) as i64;
                        (atual + passo).clamp(0, lista.len() as i64 - 1)
                    }
                };
                self.vitrine.rolar = true;
            }
        }

        let clicado = match self.settings.biblioteca {
            ModoDaBiblioteca::Grade => self.grade(ui, &lista),
            ModoDaBiblioteca::Slider => self.slider(ui, &lista),
        };
        self.vitrine.rolar = false;
        if let Some(i) = escolhido.or(clicado) {
            self.abre_pela_biblioteca(self.games[i].path.clone());
        }
    }

    /// Lê teclado e controle e devolve os comandos deste quadro.
    ///
    /// A biblioteca só escuta com a janela principal livre: com um jogo aberto o controle é
    /// dele, e com as configurações abertas ele pode estar sendo mapeado.
    fn comandos_da_biblioteca(&mut self, ctx: &egui::Context) -> Vec<Comando> {
        if self.partida.is_some() || self.settings_open || self.capturing.is_some() {
            self.vitrine.navegacao.silencia();
            return Vec::new();
        }
        ctx.request_repaint_after(LEITURA_DO_CONTROLE);
        // Escrevendo na busca, setas e espaço são do texto. O Enter tira o foco do campo antes
        // de a biblioteca ler o quadro, e por isso continua abrindo o jogo escolhido.
        let escrevendo = self
            .vitrine
            .campo_da_busca
            .is_some_and(|campo| ctx.memory(|m| m.has_focus(campo)));
        let pads: Vec<Pad> = self.pads_now(ctx).into_iter().map(|(_, pad)| pad).collect();
        let teclas = ctx.input(|i| {
            let k = |tecla| !escrevendo && i.key_down(tecla);
            [
                k(egui::Key::ArrowUp),
                k(egui::Key::ArrowDown),
                k(egui::Key::ArrowLeft),
                k(egui::Key::ArrowRight),
                k(egui::Key::Enter) || k(egui::Key::Space),
            ]
        });
        let agora = navegacao::apertado(teclas, &pads);
        self.vitrine.navegacao.comandos(agora, Instant::now())
    }

    /// A textura da caixa de um jogo, subida uma vez.
    fn textura_da_capa(&mut self, ctx: &egui::Context, jogo: usize) -> Option<egui::TextureHandle> {
        let game = &self.games[jogo];
        if let Some(textura) = self.art_cache.get(&game.path) {
            return textura.clone();
        }
        let ficha = game.clsid.and_then(|cls| self.acervo.as_ref()?.ficha(cls));
        // Uma vez por jogo: a textura fica no `art_cache`, e esta função não volta a ser chamada.
        let ao_lado = acervo::capa_ao_lado(game);
        let imagem = acervo::imagem_do_jogo(game, ficha, self.placeholder.as_ref(), ao_lado);
        let textura = imagem.map(|imagem| upload_art_of(ctx, game, imagem));
        self.art_cache.insert(game.path.clone(), textura.clone());
        textura
    }

    /// A textura do logo do rolo de cima, quando o jogo tem cena no palco da Z-Wheel.
    fn textura_do_logo(&mut self, ctx: &egui::Context, jogo: usize) -> Option<egui::TextureHandle> {
        let cls = self.games[jogo].clsid?;
        if let Some(textura) = self.vitrine.logos.get(&cls) {
            return textura.clone();
        }
        let textura = self
            .acervo
            .as_ref()
            .and_then(|acervo| acervo.ficha(cls)?.logo.as_ref())
            .map(|logo| {
                sobe(
                    ctx,
                    &format!("logo-{cls:#x}"),
                    logo,
                    egui::TextureOptions::LINEAR,
                )
            });
        self.vitrine.logos.insert(cls, textura.clone());
        textura
    }

    fn grade(&mut self, ui: &mut egui::Ui, lista: &[(String, usize)]) -> Option<usize> {
        let idioma = self.catalog.current().to_string();
        let selecionado = self.vitrine.indice(lista.len());
        let rolar = self.vitrine.rolar;
        let mut clicado = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            // Os cartões se acomodam sozinhos na largura da janela em vez de ocuparem um
            // número fixo de colunas: a mesma tela serve numa janela estreita e numa larga.
            ui.horizontal_wrapped(|ui| {
                for (posicao, (titulo, i)) in lista.iter().enumerate() {
                    let textura = self.textura_da_capa(ui.ctx(), *i);
                    let game = &self.games[*i];
                    let descricao = game
                        .clsid
                        .and_then(|cls| self.acervo.as_ref()?.ficha(cls))
                        .and_then(|ficha| ficha.descricao(&idioma));
                    let escolhido = posicao == selecionado;
                    let resposta = game_card(
                        ui,
                        titulo,
                        descricao,
                        game.clsid,
                        textura.as_ref(),
                        escolhido,
                    );
                    if escolhido && rolar {
                        resposta.scroll_to_me(Some(egui::Align::Center));
                    }
                    if resposta.clicked() {
                        self.vitrine.cursor = posicao as i64;
                        clicado = Some(*i);
                    }
                }
            });
        });
        clicado
    }

    /// O slider: o rolo de logos em cima, o nome, as caixas passando de uma para a outra e, embaixo,
    /// a descrição e a classificação.
    fn slider(&mut self, ui: &mut egui::Ui, lista: &[(String, usize)]) -> Option<usize> {
        let total = lista.len() as i64;
        let ctx = ui.ctx().clone();
        let area = ui.available_rect_before_wrap();
        let resposta = ui.allocate_rect(area, egui::Sense::click());

        // A roda do mouse anda um jogo por entalhe, de qualquer eixo.
        if resposta.hovered() {
            let delta = ctx.input(|i| i.smooth_scroll_delta);
            self.vitrine.roda += delta.x - delta.y;
            while self.vitrine.roda.abs() >= 60.0 {
                self.vitrine.cursor += self.vitrine.roda.signum() as i64;
                self.vitrine.roda -= 60.0 * self.vitrine.roda.signum();
            }
        }

        // A posição corre atrás do cursor com uma mola amortecida: rápida no começo, macia no fim.
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let alvo = self.vitrine.cursor as f32;
        self.vitrine.posicao += (alvo - self.vitrine.posicao) * (1.0 - (-dt * 12.0).exp());
        if (alvo - self.vitrine.posicao).abs() < 0.002 {
            self.vitrine.posicao = alvo;
        } else {
            ctx.request_repaint();
        }
        let posicao = self.vitrine.posicao;
        let visuals = ui.visuals().clone();
        let destaque = visuals.selection.stroke.color;
        let painter = ui.painter_at(area);

        let altura_do_rolo = (area.height() * 0.17).clamp(56.0, 104.0);
        let rolo = egui::Rect::from_min_size(
            area.min + egui::vec2(0.0, 8.0),
            egui::vec2(area.width(), altura_do_rolo),
        );
        let faixa_do_nome = egui::Rect::from_min_size(
            egui::pos2(area.left(), rolo.bottom() + 6.0),
            egui::vec2(area.width(), 40.0),
        );
        let altura_da_ficha = (area.height() * 0.2).clamp(72.0, 130.0);
        let palco = egui::Rect::from_min_max(
            egui::pos2(area.left(), faixa_do_nome.bottom() + 4.0),
            egui::pos2(area.right(), area.bottom() - altura_da_ficha),
        );
        let ficha_rect = egui::Rect::from_min_max(
            egui::pos2(area.left() + 16.0, palco.bottom() + 8.0),
            area.max - egui::vec2(16.0, 4.0),
        );

        // O rolo: um cilindro de painéis, cada um num ângulo. O da frente é o do jogo escolhido.
        painter.rect_filled(
            rolo.shrink2(egui::vec2(12.0, 0.0)),
            altura_do_rolo / 2.0,
            visuals.extreme_bg_color,
        );
        let raio = rolo.width() * 0.42;
        let centro = posicao.round() as i64;
        let mut paineis: Vec<(f32, i64)> = (centro - 5..=centro + 5)
            .map(|k| ((k as f32 - posicao) * 0.42, k))
            .filter(|(angulo, _)| angulo.abs() < std::f32::consts::FRAC_PI_2 * 0.95)
            .collect();
        paineis.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
        let mut clicado = None;
        let clique = resposta
            .clicked()
            .then(|| resposta.interact_pointer_pos())
            .flatten();
        for (angulo, k) in paineis {
            let (titulo, jogo) = &lista[k.rem_euclid(total) as usize];
            let frente = angulo.cos();
            let largura = altura_do_rolo * 1.45 * frente;
            let painel = egui::Rect::from_center_size(
                egui::pos2(rolo.center().x + raio * angulo.sin(), rolo.center().y),
                egui::vec2(largura, altura_do_rolo * 0.78),
            );
            let opacidade = 0.25 + 0.75 * frente * frente;
            let fundo = egui::Color32::from_gray(236).gamma_multiply(opacidade);
            painter.rect_filled(painel, 6.0, fundo);
            match self.textura_do_logo(&ctx, *jogo) {
                // A textura do logo é quadrada (64×64), mas no palco da Z-Wheel ela é esticada
                // sobre um painel retangular: o logo foi desenhado já comprimido na largura para
                // isso. Desenhá-la quadrada deixava o logo estreito no meio do painel.
                Some(logo) => {
                    let dentro = painel.shrink(3.0);
                    painter.image(
                        logo.id(),
                        dentro,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE.gamma_multiply(opacidade),
                    );
                }
                // Sem cena no palco, o painel leva o nome: o rolo não fica com buracos.
                None => {
                    let texto = painter.layout(
                        titulo.clone(),
                        egui::FontId::proportional(11.0 + 2.0 * frente),
                        egui::Color32::from_gray(40).gamma_multiply(opacidade),
                        (painel.width() - 8.0).max(8.0),
                    );
                    let canto = painel.center() - texto.size() / 2.0;
                    painter.with_clip_rect(painel.shrink(2.0)).galley(
                        canto,
                        texto,
                        egui::Color32::BLACK,
                    );
                }
            }
            if k == self.vitrine.cursor && (posicao - alvo).abs() < 0.5 {
                painter.rect_filled(painel, 6.0, destaque.gamma_multiply(0.18));
                painter.rect_stroke(
                    painel,
                    6.0,
                    egui::Stroke::new(2.0_f32, destaque),
                    egui::StrokeKind::Outside,
                );
            }
            if let Some(ponto) = clique
                && painel.contains(ponto)
                && clicado.is_none()
            {
                clicado = Some(k);
            }
        }

        // O nome do jogo escolhido, em cima das caixas.
        let (titulo_atual, jogo_atual) = &lista[self.vitrine.indice(lista.len())];
        painter.text(
            faixa_do_nome.center(),
            egui::Align2::CENTER_CENTER,
            titulo_atual,
            egui::FontId::proportional(24.0),
            visuals.strong_text_color(),
        );

        // As caixas: a escolhida grande no meio, as vizinhas menores e apagadas dos lados.
        let altura_da_caixa = palco.height() * 0.92;
        let mut caixas: Vec<(f32, i64)> = (centro - 4..=centro + 4)
            .map(|k| (k as f32 - posicao, k))
            .filter(|(d, _)| d.abs() < 3.6)
            .collect();
        caixas.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()));
        for (d, k) in caixas {
            let jogo = lista[k.rem_euclid(total) as usize].1;
            let Some(textura) = self.textura_da_capa(&ctx, jogo) else {
                continue;
            };
            let escala = 1.0 / (1.0 + 0.38 * d.abs());
            let fonte = textura.size_vec2();
            let altura = altura_da_caixa * escala;
            let largura = (altura * fonte.x / fonte.y).min(palco.width() * 0.5);
            let passo = altura_da_caixa * 0.62;
            let x = palco.center().x + passo * d.signum() * (d.abs() * 1.35).powf(0.8);
            let caixa = egui::Rect::from_center_size(
                egui::pos2(x, palco.center().y),
                egui::vec2(largura, altura),
            );
            let opacidade = (1.0 - 0.28 * d.abs()).clamp(0.0, 1.0);
            // A sombra dá o chão: sem ela as caixas parecem coladas no fundo.
            painter.rect_filled(
                caixa.translate(egui::vec2(0.0, 6.0 * escala)).expand(2.0),
                6.0,
                egui::Color32::from_black_alpha((90.0 * opacidade) as u8),
            );
            painter.image(
                textura.id(),
                caixa,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE.gamma_multiply(opacidade),
            );
            if d.abs() < 0.5 {
                painter.rect_stroke(
                    caixa.expand(3.0),
                    6.0,
                    egui::Stroke::new(2.0_f32, destaque),
                    egui::StrokeKind::Outside,
                );
            }
            if let Some(ponto) = clique
                && caixa.contains(ponto)
                && clicado.is_none()
            {
                clicado = Some(k);
            }
        }

        // A ficha: classificação, descrição e o que apertar.
        let idioma = self.catalog.current().to_string();
        let ficha = self.games[*jogo_atual]
            .clsid
            .and_then(|cls| self.acervo.as_ref()?.ficha(cls));
        let mut texto_x = ficha_rect.left();
        let classificacao = self.games[*jogo_atual].clsid.and_then(|cls| {
            self.vitrine
                .classificacoes
                .entry(cls)
                .or_insert_with(|| {
                    let imagem = ficha?.classificacao.as_ref()?;
                    let nome = format!("classificacao-{cls:#x}");
                    Some(sobe(&ctx, &nome, imagem, egui::TextureOptions::LINEAR))
                })
                .clone()
        });
        if let Some(textura) = classificacao {
            let fonte = textura.size_vec2();
            let altura = (ficha_rect.height() - 8.0).min(fonte.y).max(1.0);
            let caixa = egui::Rect::from_min_size(
                ficha_rect.min,
                egui::vec2(altura * fonte.x / fonte.y, altura),
            );
            painter.image(
                textura.id(),
                caixa,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            texto_x = caixa.right() + 12.0;
        }
        if let Some(descricao) = ficha.and_then(|ficha| ficha.descricao(&idioma)) {
            let largura = ficha_rect.right() - texto_x;
            let texto = painter.layout(
                descricao.to_string(),
                egui::FontId::proportional(13.0),
                visuals.text_color(),
                largura.max(40.0),
            );
            painter
                .with_clip_rect(egui::Rect::from_min_max(
                    egui::pos2(texto_x, ficha_rect.top()),
                    ficha_rect.max,
                ))
                .galley(
                    egui::pos2(texto_x, ficha_rect.top()),
                    texto,
                    visuals.text_color(),
                );
        }

        // Um clique no escolhido abre; num vizinho, traz ele para o meio.
        match clicado {
            Some(k) if k == self.vitrine.cursor => Some(*jogo_atual),
            Some(k) => {
                self.vitrine.cursor = k;
                None
            }
            None => None,
        }
    }
}

/// Quantas colunas a grade tem nesta largura, para cima e baixo andarem uma linha.
fn colunas_da_grade(ui: &egui::Ui) -> usize {
    let espaco = ui.spacing().item_spacing.x;
    // A barra de rolagem come um pouco da largura.
    let largura = ui.available_width() - ui.spacing().scroll.bar_width;
    (((largura + espaco) / (CARD_WIDTH + espaco)).floor() as usize).max(1)
}

fn sobe(
    ctx: &egui::Context,
    nome: &str,
    imagem: &Image,
    opcoes: egui::TextureOptions,
) -> egui::TextureHandle {
    let cor = egui::ColorImage::from_rgba_unmultiplied([imagem.width, imagem.height], &imagem.rgba);
    ctx.load_texture(nome, cor, opcoes)
}

/// Manda a imagem de um jogo para a placa de vídeo.
fn upload_art_of(ctx: &egui::Context, game: &Game, image: &Image) -> egui::TextureHandle {
    let options = match acervo::amplia_sem_interpolar(image, [CARD_ART, CARD_ART_ALTURA]) {
        true => egui::TextureOptions::NEAREST,
        false => egui::TextureOptions::LINEAR,
    };
    sobe(ctx, &format!("capa-{}", game.path.display()), image, options)
}

/// Um cartão da grade: a imagem do jogo, o título embaixo, e o clique que o abre. O escolhido
/// pelo controle ganha o preenchimento translúcido e a borda, como a seleção da Z-Wheel.
fn game_card(
    ui: &mut egui::Ui,
    titulo: &str,
    descricao: Option<&str>,
    clsid: Option<u32>,
    texture: Option<&egui::TextureHandle>,
    escolhido: bool,
) -> egui::Response {
    let size = egui::vec2(CARD_WIDTH, CARD_ART_ALTURA + CARD_TEXT + 24.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, visuals.weak_bg_fill);
    painter.rect_stroke(rect, 8.0, visuals.bg_stroke, egui::StrokeKind::Inside);

    let frame = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 12.0 + CARD_ART_ALTURA / 2.0),
        egui::vec2(CARD_ART, CARD_ART_ALTURA),
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
        titulo.to_string(),
        egui::FontId::proportional(12.0),
        visuals.text_color(),
        rect.width() - 16.0,
    );
    let text = egui::pos2(
        rect.center().x - galley.size().x / 2.0,
        frame.bottom() + 8.0,
    );
    painter.galley(text, galley, visuals.text_color());

    if escolhido {
        let cor = ui.visuals().selection.stroke.color;
        painter.rect_filled(rect, 8.0, cor.gamma_multiply(0.16));
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(2.0_f32, cor),
            egui::StrokeKind::Inside,
        );
    }

    // O cartão corta o título comprido, então o nome inteiro fica à espera do ponteiro, com a
    // descrição da Z-Wheel quando ela conhece o jogo.
    response.on_hover_ui(|ui| {
        ui.set_max_width(320.0);
        ui.strong(titulo);
        if let Some(descricao) = descricao {
            ui.label(descricao);
        }
        if let Some(clsid) = clsid {
            ui.weak(format!("{clsid:#010x}"));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_slider_da_a_volta_na_lista() {
        let mut vitrine = Vitrine::default();
        vitrine.cursor = -1;
        assert_eq!(vitrine.indice(5), 4);
        vitrine.cursor = 7;
        assert_eq!(vitrine.indice(5), 2);
    }
}
