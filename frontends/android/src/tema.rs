//! O tamanho das coisas numa tela que se segura com as mãos.
//!
//! O egui vem afinado para um monitor com um mouse: texto de 12 pontos, alvos de 18, espaços de
//! 4. Isso é o que dá para acertar com a ponta de uma seta; com o polegar, não. E a leitura
//! muda junto: um portátil fica a quarenta ou cinquenta centímetros do rosto, não a trinta,
//! então o mesmo tamanho aparente exige mais ponto.
//!
//! Os números daqui saem do que o Android pede para si mesmo: 48 de lado mínimo num alvo de
//! toque, corpo de texto em 16, e espaço entre linhas que deixe o dedo errar por alguns pontos
//! sem acertar o vizinho.

/// Quanto mede o lado menor de qualquer coisa em que se toca.
pub const ALVO: f32 = 48.0;

/// Põe a interface no tamanho de uma tela de mão.
pub fn aplica(ctx: &egui::Context) {
    let mut estilo = (*ctx.style()).clone();

    use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
    estilo.text_styles = [
        (TextStyle::Heading, FontId::new(26.0, Proportional)),
        (TextStyle::Body, FontId::new(17.0, Proportional)),
        (TextStyle::Button, FontId::new(17.0, Proportional)),
        // O "pequeno" é a dica e o rótulo secundário. Em 14 ele ainda se lê de braço
        // esticado; no tamanho de fábrica, 9, ele é decoração.
        (TextStyle::Small, FontId::new(14.0, Proportional)),
        (TextStyle::Monospace, FontId::new(15.0, Monospace)),
    ]
    .into();

    let espaco = &mut estilo.spacing;
    espaco.item_spacing = egui::vec2(10.0, 10.0);
    espaco.button_padding = egui::vec2(14.0, 10.0);
    espaco.interact_size = egui::vec2(ALVO, ALVO);
    espaco.icon_width = 22.0;
    espaco.icon_width_inner = 12.0;
    espaco.slider_rail_height = 10.0;
    espaco.scroll.bar_width = 12.0;
    espaco.window_margin = egui::Margin::same(16);
    espaco.menu_margin = egui::Margin::same(12);

    // Cantos mais redondos e uma diferença maior entre parado e tocado: sem ponteiro pairando,
    // o único retorno que o dedo tem é o da hora do toque.
    for widget in [
        &mut estilo.visuals.widgets.inactive,
        &mut estilo.visuals.widgets.hovered,
        &mut estilo.visuals.widgets.active,
        &mut estilo.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::same(10);
    }
    estilo.visuals.window_corner_radius = egui::CornerRadius::same(14);
    estilo.visuals.selection.bg_fill = egui::Color32::from_rgb(0, 110, 190);

    ctx.set_style(estilo);
}
