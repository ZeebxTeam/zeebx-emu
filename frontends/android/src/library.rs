//! The game grid.
//!
//! What finds the games, reads the titles and pulls the covers out of the `.mif` files is the
//! core's `library::scan` — the same one the desktop window uses. Here we only decide how that
//! fills a handheld screen.
//!
//! Two decisions that do not come from the desktop. The first is size: the card is 170 points
//! and the title 15, because on a handheld held with both hands what you read at a glance is the
//! cover, and the name is there to confirm. The second is the **D-pad**: on a device with
//! buttons, reaching the game without touching the screen is the normal path, not the
//! alternative — so the grid has a selected card, the D-pad moves it and button 1 opens it.

use std::collections::HashMap;
use std::path::PathBuf;

use zeebx::ui::library::Game;

use crate::theme::TARGET_SIZE;
use crate::{Emulator, View, system, texture_of};

/// Width of a card, in points.
const CARD_WIDTH: f32 = 170.0;

/// Height of the cover area inside the card. `.mif` covers are 65×42, so the frame is wide on
/// purpose: a tall one would leave the icon lost in the middle.
const COVER_HEIGHT: f32 = 104.0;

impl Emulator {
    /// The home screen: the top bar, the search and the grid.
    pub(crate) fn library(&mut self, ctx: &egui::Context) {
        let open_chosen = self.dpad(ctx, self.filtered.len());
        self.bar(ctx);
        self.warnings(ctx);

        if self.games.is_empty() {
            let scanning = self.scan.is_some();
            egui::CentralPanel::default().show(ctx, |ui| {
                if scanning {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.add_space(8.0);
                        ui.label(self.tr("library.scanning"));
                    });
                } else {
                    self.empty_library(ui);
                }
            });
            return;
        }

        if self.filtered.is_empty() {
            let message = self
                .catalog
                .format("library.no_match", &[("query", &self.search)]);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(32.0);
                ui.vertical_centered(|ui| {
                    ui.label(message);
                });
            });
            return;
        }

        let chosen = self.selected.min(self.filtered.len() - 1);
        let scroll = std::mem::take(&mut self.scroll);
        let chosen_by_controller = self
            .filtered
            .get(chosen)
            .copied()
            .filter(|_| open_chosen);
        let mut open = None;
        let mut visible_columns = 0usize;
        {
            let games = &self.games;
            let filtered = &self.filtered;
            let covers = &mut self.covers;
            egui::CentralPanel::default().show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    // How many columns fit, never fewer than two: on a narrow screen one card per
                    // row becomes a list, and the grid stops being a grid.
                    let columns =
                        ((ui.available_width() / CARD_WIDTH).floor() as usize).max(2);
                    visible_columns = columns;
                    let width = (ui.available_width()
                        - ui.spacing().item_spacing.x * (columns as f32 - 1.0))
                        / columns as f32;
                    egui::Grid::new("grade")
                        .num_columns(columns)
                        .spacing(ui.spacing().item_spacing)
                        .show(ui, |ui| {
                            for (position, &index) in filtered.iter().enumerate() {
                                let game = &games[index];
                                let highlighted = position == chosen;
                                let rect = card(ui, game, covers, width, highlighted);
                                if rect.1 {
                                    open = Some(game.path.clone());
                                }
                                if highlighted && scroll {
                                    ui.scroll_to_rect(rect.0, Some(egui::Align::Center));
                                }
                                if (position + 1) % columns == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                    ui.add_space(12.0);
                });
            });
        }
        self.columns = visible_columns.max(1);
        if let Some(index) = chosen_by_controller {
            open = Some(self.games[index].path.clone());
        }
        if let Some(path) = open {
            self.open(&path);
        }
    }

    /// The D-pad walking the grid. Returns whether the open button was pressed.
    ///
    /// The columns are the ones from the last paint — the grid knows how many there are, and this
    /// code runs before it. Four is a reasonable guess for the first pass; from the second on the
    /// number is the real one.
    fn dpad(&mut self, ctx: &egui::Context, count: usize) -> bool {
        if count == 0 {
            return false;
        }
        let columns = self.columns.max(1);

        // **Whoever has focus is in charge.** The top bar -- settings, reload and the search --
        // is made of egui widgets, and there focus belongs to egui. While someone holds it, the
        // grid does not touch the arrows: otherwise the cursor here would move along with the
        // focus there, and the search could never edit text. This is what this comment promised
        // and nobody had written.
        //
        // The down arrow is the way back: it releases focus and the grid cursor takes over
        // again.
        if let Some(focus) = ctx.memory(|m| m.focused()) {
            if ctx.input(|entry| entry.key_pressed(egui::Key::ArrowDown)) {
                ctx.memory_mut(|m| m.surrender_focus(focus));
            }
            return false;
        }

        let (mut step, mut open) = (0i64, false);
        ctx.input(|entry| {
            if entry.key_pressed(egui::Key::ArrowRight) {
                step += 1;
            }
            if entry.key_pressed(egui::Key::ArrowLeft) {
                step -= 1;
            }
            if entry.key_pressed(egui::Key::ArrowDown) {
                step += columns as i64;
            }
            if entry.key_pressed(egui::Key::ArrowUp) {
                step -= columns as i64;
            }
            open = entry.key_pressed(egui::Key::Enter);
        });
        // Going up from the first row leaves the grid and enters the bar: it is the path to
        // reload, settings and the search, which before only a finger could reach. The `clamp`
        // below used to trap the cursor in the top row, and the bar was unreachable by
        // controller.
        if step < 0 && self.selected < columns {
            if let Some(id) = self.bar_focus {
                ctx.memory_mut(|m| m.request_focus(id));
            }
            return open;
        }
        if step != 0 {
            let dest = self.selected as i64 + step;
            self.selected = dest.clamp(0, count as i64 - 1) as usize;
            self.scroll = true;
        }
        self.selected = self.selected.min(count - 1);
        open
    }

    /// The top bar: the name, the count, the search and the way to settings.
    fn bar(&mut self, ctx: &egui::Context) {
        let mut reload = false;
        let mut search_changed = false;
        let scanning = self.scan.is_some();
        egui::TopBottomPanel::top("barra").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.heading("Zeebx");
                ui.add_space(10.0);

                let total = self.games.len();
                let shown = self.filtered.len();
                let count = match shown == total {
                    true => self
                        .catalog
                        .format("library.count", &[("count", &total.to_string())]),
                    false => self.catalog.format(
                        "library.count_filtered",
                        &[
                            ("shown", &shown.to_string()),
                            ("count", &total.to_string()),
                        ],
                    ),
                };
                ui.label(
                    egui::RichText::new(count)
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
                if scanning {
                    ui.spinner();
                    ui.label(
                        egui::RichText::new(self.tr("library.scanning"))
                            .small()
                            .color(ui.visuals().weak_text_color()),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let settings_page = ui.add_sized(
                        [TARGET_SIZE, TARGET_SIZE],
                        egui::Button::new(egui::RichText::new("⛭").size(22.0)),
                    );
                    // The bar's entry point for controller users: this is the one that receives
                    // focus when the arrow goes up from the grid, and from there the arrows move
                    // between the buttons and the search along egui's normal path.
                    self.bar_focus = Some(settings_page.id);
                    if settings_page.clicked() {
                        self.location = View::Settings;
                    }
                    if ui
                        .add_enabled(
                            !scanning,
                            egui::Button::new(egui::RichText::new("⟳").size(22.0))
                                .min_size(egui::vec2(TARGET_SIZE, TARGET_SIZE)),
                        )
                        .clicked()
                    {
                        reload = true;
                    }
                    ui.add_space(4.0);
                    // The search gets whatever is left of the row: it is the field that helps
                    // most in a large collection and least in a small one.
                    let width = ui.available_width().min(280.0);
                    let field = egui::TextEdit::singleline(&mut self.search)
                        .desired_width(width)
                        .margin(egui::Margin::symmetric(12, 10))
                        .hint_text("🔎  Search");
                    if ui.add(field).changed() {
                        search_changed = true;
                    }
                });
            });
            ui.add_space(8.0);
        });
        if search_changed {
            self.selected = 0;
            self.scroll = true;
            self.update_filter();
        }
        if reload {
            self.reload();
        }
    }

    /// What needs saying before the grid: the missing permission, and the last game that failed
    /// to open.
    fn warnings(&mut self, ctx: &egui::Context) {
        // Without the access granted, listing works and opening does not: the user would see a
        // full library in which no game opens. Better to say so up front.
        let no_access = !system::has_file_access(&self.app);
        if !no_access && self.error.is_none() {
            return;
        }
        egui::TopBottomPanel::top("avisos").show(ctx, |ui| {
            ui.add_space(8.0);
            if let Some(error) = self.error.clone() {
                ui.horizontal(|ui| {
                    if ui.add_sized([TARGET_SIZE, TARGET_SIZE], egui::Button::new("✕")).clicked() {
                        self.error = None;
                    }
                    ui.colored_label(ui.visuals().error_fg_color, error);
                });
            }
            if no_access {
                ui.horizontal(|ui| {
                    if ui
                        .add_sized([200.0, TARGET_SIZE], egui::Button::new("Grant access"))
                        .clicked()
                    {
                        system::request_file_access(&self.app);
                    }
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "Zeebx can only see its own folder.",
                    );
                });
            }
            ui.add_space(8.0);
        });
    }

    /// The screen with no games: either there is no folder, or the folder is empty.
    fn empty_library(&mut self, ui: &mut egui::Ui) {
        let dir = self.roms();
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            if dir.as_os_str().is_empty() {
                ui.label(self.tr("library.no_folder"));
            } else {
                ui.label(
                    self.catalog
                        .format("library.empty", &[("folder", &dir.display().to_string())]),
                );
            }
            ui.add_space(20.0);
            if ui
                .add_sized(
                    [300.0, TARGET_SIZE + 8.0],
                    egui::Button::new(self.tr("settings.roms_folder")),
                )
                .clicked()
            {
                self.location = View::FolderSelector(match dir.is_dir() {
                    true => dir,
                    false => PathBuf::from("/sdcard"),
                });
            }
        });
    }

}

/// A card. Returns where it ended up and whether it was chosen.
///
/// It is a free function, not a method, because drawing a card needs a borrowed game and the
/// cover cache at the same time — two fields of the same `Emulator`, which can only be borrowed
/// together when they come in separately.
///
/// The whole card is **one** grid cell, and that is why it is drawn with the painter instead of
/// with widgets: every widget added straight to a `Grid`'s `ui` becomes a new cell, and one extra
/// label per card doubled the columns.
fn card(
    ui: &mut egui::Ui,
    game: &Game,
    covers: &mut HashMap<PathBuf, egui::TextureHandle>,
    width: f32,
    highlighted: bool,
) -> (egui::Rect, bool) {
    let height = COVER_HEIGHT + 42.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());
    let style = match response.hovered() || highlighted {
        true => ui.visuals().widgets.hovered,
        false => ui.visuals().widgets.inactive,
    };
    ui.painter().rect_filled(rect, 12.0, style.weak_bg_fill);
    // The one selected by the D-pad gets a border: with no pointer on screen, it is the only way
    // to know which game the button will hit.
    if highlighted {
        ui.painter().rect_stroke(
            rect,
            12.0,
            egui::Stroke::new(2.5_f32, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }

    let inside = rect.shrink(8.0);
    let bounds = egui::Rect::from_min_size(inside.min, egui::vec2(inside.width(), COVER_HEIGHT));
    draw_cover(ui, game, covers, bounds);

    // A long title is cut with an ellipsis, not wrapped onto two lines: cards of different
    // heights on the same row would ruin the grid.
    let title_job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: inside.width(),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        },
        ..egui::text::LayoutJob::simple_singleline(
            game.title.clone(),
            egui::FontId::proportional(15.0),
            style.text_color(),
        )
    };
    let galley = ui.painter().layout_job(title_job);
    let location = egui::pos2(
        inside.center().x - galley.size().x / 2.0,
        bounds.bottom() + 8.0,
    );
    ui.painter().galley(location, galley, style.text_color());

    (rect, response.clicked())
}

/// The card's cover, or a frame with the initial when the game has none.
fn draw_cover(
    ui: &mut egui::Ui,
    game: &Game,
    covers: &mut HashMap<PathBuf, egui::TextureHandle>,
    bounds: egui::Rect,
) {
    let texture = match covers.get(&game.path) {
        Some(texture) => Some(texture.clone()),
        None => game.art.as_ref().map(|art| {
            let texture = texture_of(ui.ctx(), &game.title, art);
            covers.insert(game.path.clone(), texture.clone());
            texture
        }),
    };

    let Some(texture) = texture else {
        // No cover, the initial in a rectangle: the card stays the same size, and the grid gets
        // no holes.
        ui.painter()
            .rect_filled(bounds, 8.0, egui::Color32::from_black_alpha(90));
        let initial = game
            .title
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            bounds.center(),
            egui::Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(34.0),
            ui.visuals().weak_text_color(),
        );
        return;
    };

    // The cover fits the frame without stretching: a `.mif` icon is wide and a real cover is
    // tall — both have to fit in the same box.
    let [width_px, height_px] = texture.size();
    let scale = (bounds.width() / width_px as f32).min(bounds.height() / height_px as f32);
    let drawn = egui::vec2(width_px as f32 * scale, height_px as f32 * scale);
    ui.painter().image(
        texture.id(),
        egui::Rect::from_center_size(bounds.center(), drawn),
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}
