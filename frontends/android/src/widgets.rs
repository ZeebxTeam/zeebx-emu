//! As peças de uma tela de toque.
//!
//! Uma linha de configuração aqui não é um `checkbox` com uma legenda ao lado: é uma faixa da
//! largura inteira, com o nome grande à esquerda e o controle à direita, alta o bastante para o
//! polegar acertar sem mirar. É a diferença entre uma janela de desktop encolhida e uma tela
//! feita para a mão.
//!
//! **As dicas ficam escondidas.** Os textos de ajuda do Zeebx são longos — alguns têm cinco
//! linhas — e todos abertos ao mesmo tempo viram uma parede cinza em que não se acha mais o
//! que se procurava. Cada linha mostra um `?`; quem quer a explicação toca nele, e ela abre ali
//! mesmo. Quem já sabe vê uma lista de nomes e valores.

use crate::tema::ALVO;

/// Qual dica está aberta. É o identificador da linha, e só uma fica aberta por vez.
pub type Aberta = Option<String>;

/// Um título de seção.
pub fn secao(ui: &mut egui::Ui, titulo: &str) {
    ui.add_space(14.0);
    ui.label(
        egui::RichText::new(titulo.to_uppercase())
            .small()
            .color(ui.visuals().weak_text_color())
            .strong(),
    );
    ui.add_space(4.0);
}

/// Uma faixa de linha: o fundo, o controle à direita, o `?` e o nome com o que sobrar.
///
/// A ordem de dentro é da direita para a esquerda de propósito. O controle é quem manda na
/// largura — uma chave tem 52 pontos, um selo de valor tem o que tiver —, e o nome fica com o
/// resto; ao contrário, um nome comprido empurraria o alvo de toque para fora da tela.
fn faixa<R>(
    ui: &mut egui::Ui,
    chave: &str,
    titulo: &str,
    dica: Option<&str>,
    aberta: &mut Aberta,
    controle: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let mostrando = aberta.as_deref() == Some(chave);
    let quadro = egui::Frame::NONE
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(14, 8));

    let resultado = quadro
        .show(ui, |ui| {
            // A altura da linha é dada, não descoberta. Num `right_to_left` dentro de uma área
            // de rolagem, o espaço disponível é a tela inteira, e o sub-`Ui` do nome esticava a
            // faixa até o fim — uma linha ocupava a tela e as de baixo saíam de vista.
            let caixa = egui::vec2(ui.available_width(), ALVO - 16.0);
            ui.allocate_ui_with_layout(
                caixa,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                let devolvido = controle(ui);
                if dica.is_some() {
                    ui.add_space(4.0);
                    let botao = egui::Button::new(
                        egui::RichText::new("?").color(ui.visuals().weak_text_color()),
                    )
                    .frame(false)
                    .min_size(egui::vec2(34.0, 34.0));
                    if ui.add(botao).clicked() {
                        *aberta = match mostrando {
                            true => None,
                            false => Some(chave.to_string()),
                        };
                    }
                }
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label(titulo);
                });
                devolvido
                },
            )
            .inner
        })
        .inner;

    if mostrando {
        if let Some(dica) = dica {
            ui.add_space(2.0);
            // A largura é imposta antes de o texto existir. Um `label` num `horizontal` não
            // quebra linha: ele cresce, empurra a área de rolagem para os lados e desalinha
            // todas as outras faixas — a tela inteira se entorta por causa de uma dica.
            let largura = (ui.available_width() - 28.0).max(120.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.vertical(|ui| {
                    ui.set_max_width(largura);
                    ui.label(
                        egui::RichText::new(dica)
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                });
            });
            ui.add_space(4.0);
        }
    }
    resultado
}

/// Uma linha que liga e desliga.
pub fn interruptor(
    ui: &mut egui::Ui,
    chave: &str,
    titulo: &str,
    dica: Option<&str>,
    aberta: &mut Aberta,
    valor: &mut bool,
) -> bool {
    faixa(ui, chave, titulo, dica, aberta, |ui| chave_de_luz(ui, valor))
}

/// Uma escolha entre poucas opções, como os botões de um rádio de carro.
///
/// É um grupo de botões grandes lado a lado, e não uma coluna de bolinhas: a bolinha do
/// `radio_button` tem 10 pontos de diâmetro, e ninguém acerta 10 pontos com o polegar.
pub fn segmentado<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    chave: &str,
    titulo: &str,
    dica: Option<&str>,
    aberta: &mut Aberta,
    valor: &mut T,
    opcoes: &[(T, String)],
) -> bool {
    let mut mudou = false;
    faixa(ui, chave, titulo, dica, aberta, |_| {});
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (opcao, nome) in opcoes {
            let escolhido = *valor == *opcao;
            let botao = egui::Button::new(nome)
                .min_size(egui::vec2(0.0, ALVO - 8.0))
                .selected(escolhido);
            if ui.add(botao).clicked() && !escolhido {
                *valor = *opcao;
                mudou = true;
            }
        }
    });
    ui.add_space(6.0);
    mudou
}

/// Um número numa faixa, com o valor num selo à direita.
pub fn deslizante(
    ui: &mut egui::Ui,
    chave: &str,
    titulo: &str,
    dica: Option<&str>,
    aberta: &mut Aberta,
    valor: &mut u8,
    faixa_de: std::ops::RangeInclusive<u8>,
    rotulo: impl Fn(u8) -> String,
) -> bool {
    let mut mudou = false;
    let atual = rotulo(*valor);
    faixa(ui, chave, titulo, dica, aberta, |ui| {
        ui.label(egui::RichText::new(atual).monospace().strong());
    });
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        // O `Slider` não olha o espaço que recebe: ele mede pelo `spacing.slider_width`, que
        // de fábrica são 100 pontos. Num alvo de arrastar isso é curto demais — cada pixel
        // vale vários passos —, então a trilha é dita aqui, e é a linha inteira.
        ui.spacing_mut().slider_width = (ui.available_width() - 28.0).max(120.0);
        // Sem o número do lado: ele já está no selo da linha, e o campo de digitar do
        // `Slider` é um alvo minúsculo que só atrapalha no toque.
        mudou = ui
            .add(egui::Slider::new(valor, faixa_de).show_value(false))
            .changed();
    });
    ui.add_space(6.0);
    mudou
}

/// Uma linha que leva a outro lugar: o nome em cima, o valor embaixo, e a seta à direita.
pub fn navega(ui: &mut egui::Ui, titulo: &str, valor: &str) -> bool {
    let quadro = egui::Frame::NONE
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(14, 10));
    let resposta = quadro
        .show(ui, |ui| {
            let caixa = egui::vec2(ui.available_width(), ALVO - 8.0);
            ui.allocate_ui_with_layout(
                caixa,
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.label(egui::RichText::new("›").size(24.0));
                    ui.add_space(6.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                        ui.label(titulo);
                        ui.label(
                            egui::RichText::new(valor)
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                },
            );
        })
        .response;
    ui.interact(
        resposta.rect,
        egui::Id::new(("navega", titulo)),
        egui::Sense::click(),
    )
    .clicked()
}

/// O botão de ligar e desligar, desenhado à mão.
///
/// O `checkbox` do egui é um quadradinho de 18 pontos com um risco dentro — do outro lado da
/// tela não dá para saber se está marcado. Uma chave de luz diz o estado pela posição e pela
/// cor, que é o que se lê de longe.
fn chave_de_luz(ui: &mut egui::Ui, ligado: &mut bool) -> bool {
    let tamanho = egui::vec2(52.0, 30.0);
    let (rect, resposta) = ui.allocate_exact_size(tamanho, egui::Sense::click());
    if resposta.clicked() {
        *ligado = !*ligado;
    }
    // A animação dá o retorno que o dedo não tem: sem ela a chave parece não ter respondido.
    let quanto = ui
        .ctx()
        .animate_bool_responsive(resposta.id, *ligado);
    let visual = ui.style().interact_selectable(&resposta, *ligado);
    let trilho = egui::Color32::from_rgb(90, 90, 96).lerp_to_gamma(
        ui.visuals().selection.bg_fill,
        quanto,
    );
    ui.painter()
        .rect_filled(rect, rect.height() / 2.0, trilho);
    let raio = rect.height() / 2.0 - 3.0;
    let centro = egui::pos2(
        egui::lerp((rect.left() + raio + 3.0)..=(rect.right() - raio - 3.0), quanto),
        rect.center().y,
    );
    ui.painter()
        .circle(centro, raio, egui::Color32::WHITE, visual.fg_stroke);
    resposta.clicked()
}
