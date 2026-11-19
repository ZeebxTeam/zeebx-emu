//! The debug panel: speed, clock, memory and the graph of the last minute.
//!
//! It lives in the core because it does not belong to a single window: it is the same strip, with
//! the same numbers and the same translations, in Qt and on Android. Deciding **where** it
//! goes — a strip that shrinks the game, an overlay — is up to each frontend; what it says is
//! this, here.

use crate::session::Sample;
use crate::ui::i18n::Catalog;
use crate::ui::settings::DebugView;

/// The panel's texts, already translated, each one only if its option is on.
///
/// It is the part of the panel that does not depend on a toolkit: the egui [`panel`] and the Qt
/// window write the same numbers with the same words.
pub struct Texts {
    pub speed: Option<String>,
    pub clock: Option<String>,
    pub memory: Option<String>,
}

impl Texts {
    pub fn fresh(
        catalog: &Catalog,
        debug: &DebugView,
        sample: Sample,
        memory: (u32, usize),
        clock_ms: u32,
    ) -> Self {
        let (heap, objects) = memory;
        Self {
            speed: debug.speed.then(|| {
                catalog.format(
                    "debug.speed.value",
                    &[
                        ("percent", &sample.speed.to_string()),
                        ("fps", &sample.fps.to_string()),
                    ],
                )
            }),
            clock: debug.clock.then(|| {
                catalog.format(
                    "debug.clock.value",
                    &[
                        ("mips", &human_instructions(sample.ips)),
                        ("clock", &format!("{:.1}s", clock_ms as f32 / 1000.0)),
                    ],
                )
            }),
            memory: debug.memory.then(|| {
                catalog.format(
                    "debug.memory.value",
                    &[
                        ("heap", &human_bytes(heap)),
                        ("objects", &objects.to_string()),
                    ],
                )
            }),
        }
    }
}

/// Draws the panel's line in the given `ui`.
///
/// `history` is the recent samples as `(speed, frames)`, from oldest to
/// newest — it is what [`crate::session::Session::history`] hands over.
pub fn panel(
    ui: &mut egui::Ui,
    catalog: &Catalog,
    debug: DebugView,
    sample: Sample,
    memory: (u32, usize),
    clock_ms: u32,
    history: &[(u32, u32)],
) {
    let texts = Texts::fresh(catalog, &debug, sample, memory, clock_ms);
    ui.horizontal(|ui| {
        if let Some(text) = texts.speed {
            ui.monospace(text);
            ui.separator();
        }
        if let Some(text) = texts.clock {
            ui.monospace(text);
            ui.separator();
        }
        if let Some(text) = texts.memory {
            ui.monospace(text);
        }
        // The graph only goes in if it fits whole. On a handheld screen the three numbers already
        // take up the strip, and a squeezed graph does not become a smaller graph: it is drawn at
        // the same size, on top of the text.
        let fits = ui.available_width() >= GRAPH_WIDTH + 12.0;
        if debug.timeline && fits && !history.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                draw_timeline(ui, history);
            });
        }
    });
}

/// How wide the graph is. It is fixed: what it shows is the shape of the last minute, and a width
/// that changes with the window would make the same drop look different on every resize.
pub const GRAPH_WIDTH: f32 = 220.0;

/// A number of instructions per second, in the way it is read.
pub fn human_instructions(per_second: u64) -> String {
    match per_second {
        0..=9_999 => format!("{per_second}"),
        10_000..=9_999_999 => format!("{} K", per_second / 1_000),
        _ => format!("{} M", per_second / 1_000_000),
    }
}

/// A size in bytes, in the way it is read.
pub fn human_bytes(bytes: u32) -> String {
    match bytes {
        0..=9_999 => format!("{bytes} B"),
        10_000..=9_999_999 => format!("{} KB", bytes / 1024),
        _ => format!("{:.1} MB", bytes as f32 / (1024.0 * 1024.0)),
    }
}

/// The panel's graph: speed and frames per second over the last minute.
///
/// It is drawn by hand instead of with a charting library because what is wanted here is to see
/// the shape — where it sank, where it stabilized —, and for that two lines in a 40-pixel strip
/// are enough.
pub fn draw_timeline(ui: &mut egui::Ui, history: &[(u32, u32)]) {
    const HEIGHT: f32 = 40.0;
    const WIDTH: f32 = GRAPH_WIDTH;
    let (response, gl_painter) = ui.allocate_painter(egui::vec2(WIDTH, HEIGHT), egui::Sense::hover());
    let area = response.rect;
    gl_painter.rect_filled(area, 2.0, egui::Color32::from_black_alpha(120));

    // The 100% line is the reference that matters: above it the game keeps the console's pace,
    // below it the game is falling behind.
    let hundred = area.bottom() - HEIGHT * 0.5;
    gl_painter.line_segment(
        [egui::pos2(area.left(), hundred), egui::pos2(area.right(), hundred)],
        egui::Stroke::new(1.0_f32, egui::Color32::from_white_alpha(40)),
    );

    let step = WIDTH / history.len().max(2) as f32;
    // Speed goes up to 200% on the graph; anything above that touches the ceiling.
    let point = |i: usize, value: u32, cap: f32| {
        egui::pos2(
            area.left() + i as f32 * step,
            area.bottom() - HEIGHT * (value as f32 / cap).min(1.0),
        )
    };
    for (values, color, cap) in [
        (0, egui::Color32::from_rgb(120, 200, 255), 200.0),
        (1, egui::Color32::from_rgb(160, 255, 160), 60.0),
    ] {
        let line: Vec<egui::Pos2> = history
            .iter()
            .enumerate()
            .map(|(i, sample)| match values {
                0 => point(i, sample.0, cap),
                _ => point(i, sample.1, cap),
            })
            .collect();
        gl_painter.add(egui::Shape::line(line, egui::Stroke::new(1.0_f32, color)));
    }
}
