//! O painel de depuração: velocidade, relógio, memória e o gráfico do último minuto.
//!
//! Ele mora aqui, e não no `ui::app`, porque não é da janela do desktop: é a mesma faixa, com
//! os mesmos números e as mesmas traduções, no desktop e no Android. Quem decide **onde** ela
//! fica — uma faixa que encolhe o jogo, uma sobreposição — é cada frontend; o que ela diz é
//! isto aqui.

use crate::session::Sample;
use crate::ui::i18n::Catalog;
use crate::ui::settings::DebugView;

/// Os textos do painel, já traduzidos, cada um só se a opção dele estiver ligada.
///
/// É a parte do painel que não depende de toolkit: o [`painel`] do egui e a janela Qt escrevem
/// os mesmos números com as mesmas palavras.
pub struct Textos {
    pub velocidade: Option<String>,
    pub relogio: Option<String>,
    pub memoria: Option<String>,
}

impl Textos {
    pub fn novos(
        catalogo: &Catalog,
        debug: &DebugView,
        amostra: Sample,
        memoria: (u32, usize),
        relogio_ms: u32,
    ) -> Self {
        let (heap, objetos) = memoria;
        Self {
            velocidade: debug.speed.then(|| {
                catalogo.format(
                    "debug.speed.value",
                    &[
                        ("percent", &amostra.speed.to_string()),
                        ("fps", &amostra.fps.to_string()),
                    ],
                )
            }),
            relogio: debug.clock.then(|| {
                catalogo.format(
                    "debug.clock.value",
                    &[
                        ("mips", &instrucoes_legiveis(amostra.ips)),
                        ("clock", &format!("{:.1}s", relogio_ms as f32 / 1000.0)),
                    ],
                )
            }),
            memoria: debug.memory.then(|| {
                catalogo.format(
                    "debug.memory.value",
                    &[
                        ("heap", &bytes_legiveis(heap)),
                        ("objects", &objetos.to_string()),
                    ],
                )
            }),
        }
    }
}

/// Desenha a linha do painel no `ui` dado.
///
/// `historia` são as amostras recentes como `(velocidade, quadros)`, da mais antiga para a mais
/// nova — é o que a [`crate::session::Session::history`] entrega.
pub fn painel(
    ui: &mut egui::Ui,
    catalogo: &Catalog,
    debug: DebugView,
    amostra: Sample,
    memoria: (u32, usize),
    relogio_ms: u32,
    historia: &[(u32, u32)],
) {
    let textos = Textos::novos(catalogo, &debug, amostra, memoria, relogio_ms);
    ui.horizontal(|ui| {
        if let Some(texto) = textos.velocidade {
            ui.monospace(texto);
            ui.separator();
        }
        if let Some(texto) = textos.relogio {
            ui.monospace(texto);
            ui.separator();
        }
        if let Some(texto) = textos.memoria {
            ui.monospace(texto);
        }
        // O gráfico só entra se couber inteiro. Numa tela de mão os três números já tomam a
        // faixa, e um gráfico espremido não vira gráfico menor: ele é desenhado do mesmo
        // tamanho, por cima do texto.
        let cabe = ui.available_width() >= LARGURA_DO_GRAFICO + 12.0;
        if debug.timeline && cabe && !historia.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                desenhar_linha_do_tempo(ui, historia);
            });
        }
    });
}

/// Quanto o gráfico ocupa de largura. É fixo: o que ele mostra é a forma do último minuto, e
/// uma largura que muda com a janela faria a mesma queda parecer outra a cada redimensionamento.
pub const LARGURA_DO_GRAFICO: f32 = 220.0;

/// Um número de instruções por segundo no jeito que se lê.
pub fn instrucoes_legiveis(por_segundo: u64) -> String {
    match por_segundo {
        0..=9_999 => format!("{por_segundo}"),
        10_000..=9_999_999 => format!("{} K", por_segundo / 1_000),
        _ => format!("{} M", por_segundo / 1_000_000),
    }
}

/// Um tamanho em bytes no jeito que se lê.
pub fn bytes_legiveis(bytes: u32) -> String {
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
pub fn desenhar_linha_do_tempo(ui: &mut egui::Ui, historia: &[(u32, u32)]) {
    const ALTURA: f32 = 40.0;
    const LARGURA: f32 = LARGURA_DO_GRAFICO;
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
