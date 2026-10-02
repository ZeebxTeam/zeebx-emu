//! Os controles desenhados na tela: a sobreposição do jogo e o editor que muda as peças de lugar.
//!
//! A conta de onde fica cada peça e do que o dedo aperta é do núcleo, em
//! [`zeebx::input::toque`]; daqui é o desenho e a leitura do toque.
//!
//! **O toque do jogo não passa pelo egui.** O egui recebe o dedo como um ponteiro só, e jogar
//! pede dois ao mesmo tempo: o polegar segurando o direcional e o outro apertando o 1. Por isso a
//! [`Sobreposicao`] lê o `MotionEvent` inteiro, com todos os dedos.

use android_activity::input::{Axis, MotionAction, MotionEvent, ToolType};

use zeebx::input::toque::{self, Elemento, Peca};
use zeebx::ui::settings::{ControlesNaTela, ModoDosControlesNaTela};

use crate::tema::ALVO;
use crate::{Emulador, Onde};

/// O que a sobreposição sabe entre um quadro e outro.
#[derive(Default)]
pub struct Sobreposicao {
    /// Os botões que os dedos apertam agora, um bit por índice do [`zeebx::input::Pad`]. É
    /// somado ao controle físico, e não escrito por cima dele: tirar o dedo da tela não pode
    /// soltar um botão que está apertado no controle.
    pub botoes: u32,
    /// O último aperto veio de um controle físico. No modo automático, isso esconde as peças.
    pub controle_fisico: bool,
    /// O dedo que segura cada manche, pelo id que o Android dá a ele, na ordem de [`MANCHES`].
    ///
    /// Os botões não guardam estado — o que está apertado é só função de onde os dedos estão.
    /// O manche guarda: o dedo que encostou nele é dele até levantar, ver
    /// [`toque::pega_manche`]. E o id, e não a posição na lista, porque a posição de um dedo muda
    /// quando outro levanta.
    manches: [Option<i32>; 2],
    /// Onde os manches estão, de `-1` a `1`, na ordem de [`zeebx::input::Pad::axes`].
    pub eixos: [f32; 4],
    /// As peças como ficaram no último desenho.
    ///
    /// O toque usa estas, e não uma conta nova: o evento chega antes de o quadro ser desenhado, e
    /// é só no desenho que se sabe o tamanho da tela.
    pub elementos: Vec<Elemento>,
}

impl Sobreposicao {
    /// Se as peças estão na tela.
    pub fn visivel(&self, ajustes: &ControlesNaTela) -> bool {
        match ajustes.modo {
            ModoDosControlesNaTela::Sempre => true,
            ModoDosControlesNaTela::Automatico => !self.controle_fisico,
            ModoDosControlesNaTela::Nunca => false,
        }
    }

    /// Chegou um aperto de controle físico: no modo automático, as peças saem da frente.
    pub fn viu_controle(&mut self) {
        self.controle_fisico = true;
        self.solta();
    }

    /// Solta tudo. Para quando o toque deixa de ir para o jogo — a pergunta do "voltar" abriu, o
    /// aplicativo perdeu o foco — com um dedo ainda em cima de uma peça.
    pub fn solta(&mut self) {
        self.botoes = 0;
        self.manches = [None; 2];
        self.eixos = [0.0; 4];
    }

    /// Um evento de toque, com todos os dedos que estão na tela.
    pub fn toque(&mut self, movimento: &MotionEvent, pixels_por_ponto: f32) {
        // Qualquer toque devolve as peças no modo automático: é como quem largou o controle
        // avisa que quer jogar pela tela.
        self.controle_fisico = false;

        let acao = movimento.action();
        if matches!(acao, MotionAction::Cancel) {
            self.solta();
            return;
        }
        let da_acao = movimento.pointer_index();

        // Um dedo que acabou de encostar pode pegar um manche, se ninguém o segura ainda.
        if matches!(acao, MotionAction::Down | MotionAction::PointerDown) {
            let dedo = movimento.pointer_at_index(da_acao);
            let ponto = [
                dedo.axis_value(Axis::X) / pixels_por_ponto,
                dedo.axis_value(Axis::Y) / pixels_por_ponto,
            ];
            if let Some(peca) = toque::pega_manche(&self.elementos, ponto)
                && let Some(vaga) = MANCHES.iter().position(|&m| m == peca)
                && self.manches[vaga].is_none()
            {
                self.manches[vaga] = Some(dedo.pointer_id());
            }
        }

        // O dedo que está subindo ainda vem na lista deste evento. Ele sai da conta, senão o
        // botão só soltaria no próximo movimento de outro dedo, e solta o manche que segurava.
        let subindo = matches!(acao, MotionAction::Up | MotionAction::PointerUp).then_some(da_acao);
        self.eixos = [0.0; 4];
        let mut livres = Vec::new();
        for dedo in movimento.pointers() {
            let id = dedo.pointer_id();
            if Some(dedo.pointer_index()) == subindo {
                for manche in &mut self.manches {
                    if *manche == Some(id) {
                        *manche = None;
                    }
                }
                continue;
            }
            if dedo.tool_type() == ToolType::Palm {
                continue;
            }
            let ponto = [
                dedo.axis_value(Axis::X) / pixels_por_ponto,
                dedo.axis_value(Axis::Y) / pixels_por_ponto,
            ];
            let Some(vaga) = self.manches.iter().position(|&m| m == Some(id)) else {
                livres.push(ponto);
                continue;
            };
            let peca = MANCHES[vaga];
            if let (Some(elemento), Some([horizontal, vertical])) = (
                self.elementos.iter().find(|e| e.peca == peca),
                peca.eixos(),
            ) {
                let [x, y] = toque::manche(elemento, ponto);
                self.eixos[horizontal] = x;
                self.eixos[vertical] = y;
            }
        }
        // O dedo de um manche não aperta botão, nem quando passa por cima de um.
        self.botoes = toque::botoes(&self.elementos, livres);
    }
}

/// Os manches, na ordem das vagas de [`Sobreposicao::manches`].
const MANCHES: [Peca; 2] = [Peca::MancheEsquerdo, Peca::MancheDireito];

/// O tamanho da tela em pontos, no formato do núcleo.
fn tamanho(ctx: &egui::Context) -> [f32; 2] {
    let tela = ctx.content_rect();
    [tela.width(), tela.height()]
}

/// Desenha as peças. `apertados` acende as que estão apertadas, `eixos` põe a bolinha de cada
/// manche no lugar; `destaque` é a que o editor está arrastando.
fn desenha(
    pincel: &egui::Painter,
    elementos: &[Elemento],
    apertados: u32,
    eixos: [f32; 4],
    opacidade: u8,
    destaque: Option<Peca>,
) {
    let alfa = (f32::from(opacidade.min(100)) / 100.0 * 255.0) as u8;
    let fundo = egui::Color32::from_rgba_unmultiplied(20, 20, 24, alfa / 2 + alfa / 4);
    let aceso = egui::Color32::from_rgba_unmultiplied(0, 110, 190, alfa);
    let tinta = egui::Color32::from_rgba_unmultiplied(255, 255, 255, alfa);
    let borda = egui::Stroke::new(2.0_f32, tinta);

    for elemento in elementos {
        let centro = egui::pos2(elemento.centro[0], elemento.centro[1]);
        let [mx, my] = elemento.meio;
        let caixa = egui::Rect::from_center_size(centro, egui::vec2(2.0 * mx, 2.0 * my));
        let realce = match destaque == Some(elemento.peca) {
            true => egui::Stroke::new(3.0_f32, egui::Color32::from_rgb(255, 200, 0)),
            false => borda,
        };

        match elemento.peca {
            Peca::MancheEsquerdo | Peca::MancheDireito => {
                // A base e a bolinha. Encostada na borda, a bolinha ainda fica inteira dentro da
                // base: o deslocamento máximo é o raio da base menos o dela.
                let [horizontal, vertical] = elemento.peca.eixos().unwrap_or([0, 1]);
                let deslocamento = egui::vec2(eixos[horizontal], eixos[vertical]);
                let bolinha = mx * 0.45;
                let em_uso = deslocamento != egui::Vec2::ZERO;
                pincel.circle_filled(centro, mx, fundo);
                pincel.circle_stroke(centro, mx, realce);
                let cor = match em_uso {
                    true => aceso,
                    false => egui::Color32::from_rgba_unmultiplied(200, 200, 210, alfa / 2),
                };
                let onde = centro + deslocamento * (mx - bolinha);
                pincel.circle_filled(onde, bolinha, cor);
                pincel.circle_stroke(onde, bolinha, borda);
            }
            Peca::Direcional => {
                // Uma cruz só, como a do controle, com cada braço aceso por si: as diagonais
                // acendem dois.
                //
                // **O miolo e os braços não se sobrepõem.** Eram quatro retângulos cruzando no
                // centro, e com a cor semitransparente o cruzamento saía mais escuro, com os
                // cantos arredondados de cada braço aparecendo por dentro. E o preenchimento vai
                // numa malha sem suavização de borda: peças vizinhas desenhadas como formas do
                // egui ganham uma linha clara na emenda, que é a borda suavizada de cada uma.
                let braco = mx * 0.34;
                let ponta = mx * 0.95;
                let [cima, baixo, esquerda, direita] = zeebx::input::DPAD;
                let ret = |x0: f32, y0: f32, x1: f32, y1: f32| {
                    egui::Rect::from_min_max(
                        centro + egui::vec2(x0, y0),
                        centro + egui::vec2(x1, y1),
                    )
                };
                let cor_do = |indice: usize| match apertados & (1 << indice) != 0 {
                    true => aceso,
                    false => fundo,
                };
                let mut malha = egui::Mesh::default();
                malha.add_colored_rect(ret(-braco, -braco, braco, braco), fundo);
                malha.add_colored_rect(ret(-braco, -ponta, braco, -braco), cor_do(cima));
                malha.add_colored_rect(ret(-braco, braco, braco, ponta), cor_do(baixo));
                malha.add_colored_rect(ret(-ponta, -braco, -braco, braco), cor_do(esquerda));
                malha.add_colored_rect(ret(braco, -braco, ponta, braco), cor_do(direita));
                pincel.add(malha);
                // O contorno é o da cruz inteira, numa linha só.
                let contorno = [
                    (-braco, -ponta),
                    (braco, -ponta),
                    (braco, -braco),
                    (ponta, -braco),
                    (ponta, braco),
                    (braco, braco),
                    (braco, ponta),
                    (-braco, ponta),
                    (-braco, braco),
                    (-ponta, braco),
                    (-ponta, -braco),
                    (-braco, -braco),
                ]
                .map(|(x, y)| centro + egui::vec2(x, y));
                pincel.add(egui::Shape::closed_line(contorno.to_vec(), realce));
                // As setas nas pontas, para a cruz se ler como direcional e não como um "+".
                for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
                    let eixo = egui::vec2(dx, dy);
                    let lado = egui::vec2(-dy, dx);
                    let bico = centro + eixo * ponta * 0.82;
                    let base = centro + eixo * ponta * 0.55;
                    pincel.add(egui::Shape::convex_polygon(
                        vec![bico, base + lado * braco * 0.55, base - lado * braco * 0.55],
                        tinta,
                        egui::Stroke::NONE,
                    ));
                }
            }
            peca => {
                let apertado = peca
                    .indice()
                    .is_some_and(|indice| apertados & (1 << indice) != 0);
                let cor = match apertado {
                    true => aceso,
                    false => fundo,
                };
                match peca.redonda() && mx == my {
                    true => {
                        pincel.circle_filled(centro, mx, cor);
                        pincel.circle_stroke(centro, mx, realce);
                    }
                    false => {
                        pincel.rect_filled(caixa, my, cor);
                        pincel.rect_stroke(caixa, my, realce, egui::StrokeKind::Inside);
                    }
                }
                let letra = match peca {
                    Peca::Home => my * 0.75,
                    _ => my * 0.85,
                };
                pincel.text(
                    centro,
                    egui::Align2::CENTER_CENTER,
                    peca.rotulo(),
                    egui::FontId::proportional(letra),
                    tinta,
                );
            }
        }
    }
}

impl Emulador {
    /// As peças por cima do jogo, quando visíveis. Chamado pela tela do jogo depois do quadro,
    /// para ficar por cima dele.
    pub(crate) fn desenha_sobreposicao(&mut self, ctx: &egui::Context) {
        let ajustes = &self.settings.controles_na_tela;
        if !self.sobreposicao.visivel(ajustes) {
            self.sobreposicao.elementos.clear();
            return;
        }
        self.sobreposicao.elementos = toque::monta(tamanho(ctx), ajustes.escala, &ajustes.posicoes);
        let pincel = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("controles-na-tela"),
        ));
        desenha(
            &pincel,
            &self.sobreposicao.elementos,
            self.sobreposicao.botoes,
            self.sobreposicao.eixos,
            ajustes.opacidade,
            None,
        );
    }

    /// A tela de mudar as peças de lugar: arrasta-se com o dedo, e grava ao soltar.
    pub(crate) fn edita_toque(&mut self, ctx: &egui::Context) {
        let tela = tamanho(ctx);
        let fundo = egui::Frame::NONE.fill(egui::Color32::from_rgb(16, 16, 20));
        let mut gravar = false;
        egui::CentralPanel::default().frame(fundo).show(ctx, |ui| {
            let area = ctx.content_rect();
            // O contorno do quadro do jogo, em 4:3 e centrado, como ele aparece no "Ajustar": é
            // a referência de quanto cada peça vai tapar.
            let altura = area.height();
            let quadro =
                egui::Rect::from_center_size(area.center(), egui::vec2(altura * 4.0 / 3.0, altura));
            ui.painter().rect_filled(quadro, 0.0, egui::Color32::from_rgb(34, 38, 48));
            ui.painter().text(
                quadro.center(),
                egui::Align2::CENTER_CENTER,
                self.catalogo.get("touch.editor.hint"),
                egui::FontId::proportional(18.0),
                egui::Color32::from_gray(150),
            );

            let ajustes = &mut self.settings.controles_na_tela;
            let resposta = ui.interact(area, egui::Id::new("editor-de-toque"), egui::Sense::drag());
            let elementos = toque::monta(tela, ajustes.escala, &ajustes.posicoes);

            if resposta.drag_started() {
                // A peça é a que estava debaixo do dedo **quando ele encostou**: o arrasto só
                // começa depois de alguns pontos de movimento, e aí o dedo já pode ter saído dela.
                let origem = ctx.input(|i| i.pointer.press_origin());
                self.arrastando = origem.and_then(|origem| {
                    let peca = toque::debaixo(&elementos, [origem.x, origem.y])?;
                    let elemento = elementos.iter().find(|e| e.peca == peca)?;
                    // Guarda onde, dentro da peça, o dedo pegou: sem isto ela pularia para
                    // centrar no dedo.
                    Some((peca, egui::pos2(elemento.centro[0], elemento.centro[1]) - origem))
                });
            }
            if resposta.dragged()
                && let (Some((peca, desvio)), Some(dedo)) =
                    (self.arrastando, resposta.interact_pointer_pos())
            {
                let centro = dedo + desvio;
                ajustes
                    .posicoes
                    .insert(peca.nome().to_string(), toque::fracao([centro.x, centro.y], tela));
            }
            if resposta.drag_stopped() && self.arrastando.take().is_some() {
                gravar = true;
            }

            // No editor as peças aparecem mesmo com a opacidade lá embaixo: ninguém arrasta o que
            // não enxerga.
            let elementos = toque::monta(tela, ajustes.escala, &ajustes.posicoes);
            desenha(
                ui.painter(),
                &elementos,
                0,
                [0.0; 4],
                ajustes.opacidade.max(70),
                self.arrastando.map(|(peca, _)| peca),
            );
        });

        // A barra fica numa área por cima do painel: assim os botões dela ganham do arrasto.
        egui::Area::new(egui::Id::new("barra-do-editor"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 12.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let restaurar = egui::Button::new(self.catalogo.get("touch.editor.reset"))
                        .min_size(egui::vec2(0.0, ALVO));
                    if ui.add(restaurar).clicked() {
                        self.settings.controles_na_tela.posicoes.clear();
                        gravar = true;
                    }
                    let pronto = egui::Button::new(self.catalogo.get("touch.editor.done"))
                        .min_size(egui::vec2(0.0, ALVO));
                    if ui.add(pronto).clicked() {
                        self.onde = Onde::Ajustes;
                    }
                });
            });

        if gravar {
            self.salva();
        }
    }
}
