//! The running game.
//!
//! The frame fills everything, and the way out is Android's "back". What shows up on top are the
//! on-screen controls ([`crate::touch`]), for whoever plays without a physical controller, and
//! the debug panel, turned on in settings — which is [`zeebx::ui::debug_panel::panel`], the same
//! as on desktop.

use std::time::Instant;

use zeebx::session::Step;
use zeebx::ui::debug_panel;
use zeebx::ui::gpu;
use zeebx::ui::settings::{AspectRatio, Scaling};

use zeebx::input::bindings::Source;
use zeebx::speed::rewind::Reading;

use crate::Emulator;
use zeebx::ui::save_slots;
use zeebx::session::MAX_SLICE;

impl Emulator {
    pub(crate) fn game(&mut self, ctx: &egui::Context) {
        let Some(session) = self.session.as_mut() else {
            return;
        };

        // The session shortcuts: the on-screen piece, or the physical controller button bound to
        // them in settings. Rewind comes before everything: while rewinding, the game does not
        // advance and fast-forward is suspended. See `docs/implementation/24-speed.md`.
        let on_screen = self.overlay.shortcuts;
        let physical = |sources: &[Source]| {
            sources.iter().any(|source| match source {
                Source::Button { name } => self.physical.contains(name.as_str()),
                _ => false,
            })
        };
        let shortcuts = &self.settings.shortcuts;
        let back = on_screen.back || physical(&shortcuts.back);
        let fast_forward = on_screen.fast_forward || physical(&shortcuts.fast_forward);
        let player = self.settings.controls.player(0);
        let turbo_pressed = on_screen.turbo
            || player.is_some_and(|j| physical(j.sources(zeebx::speed::turbo::TURBO_BUTTON)));
        // The shortcuts' physical sources do not reach the game: see `Shortcuts::reserved`.
        let mut reserved_buttons = 0u32;
        for source in shortcuts.reserved() {
            if let Source::Button { name } = source
                && self.physical.contains(name.as_str())
                && let Some(index) = crate::host_input::button_index(name)
            {
                reserved_buttons |= 1 << index;
            }
        }
        if !self.confirming {
            // Going back one point rebuilds the textures on the GPU, and the window has painted
            // in the same context since the previous step: the same notice as before advancing,
            // just below.
            if back && self.settings.speed.rewind.enabled {
                session.resume_context();
            }
            let reading =
                self.rewind
                    .read(session, back, self.paused, &self.settings.speed.rewind);
            if reading == Reading::Released {
                self.last = Instant::now();
            }
        }

        // With the question on screen, paused or rewinding, the game stays still — the clock
        // included, so it does not spend the whole budget at once when it resumes.
        let stopped = self.confirming || self.paused || self.rewind.rewinding();
        // Sound stops too, except during rewind, which already silences it and swaps in each
        // point's sound. See `Session::freeze_audio`.
        session.freeze_audio(self.confirming || self.paused);
        if stopped {
            self.fast_forwarding = false;
            self.last = Instant::now();
        } else {
            // The budget is the real time that passed since the previous frame, capped at the
            // ceiling: it is how much the game needs to emulate to keep up with the wall clock.
            let elapsed = self.last.elapsed().min(MAX_SLICE);
            self.last = Instant::now();
            let mut pad = self.pad;
            pad.buttons &= !reserved_buttons;
            pad.buttons |= self.overlay.buttons;
            // The on-screen axis only counts where the physical stick is at rest: the two are not
            // added together, because half the travel of each is not the full travel of either.
            for (axis, touch) in pad.axes.iter_mut().zip(self.overlay.axes) {
                if *axis == 0 {
                    *axis = (touch * zeebx::input::AXIS_RANGE as f32).round() as i32;
                }
            }
            session.set_port_pad(0, pad);
            // Player 1's turbo: which buttons pulse comes from here, and when, from the game
            // clock.
            let pulsing = match player {
                Some(j) => self.turbo.pulsing(j.turbo, &j.turbo_button, &pad, turbo_pressed),
                None => 0,
            };
            session.set_turbo_rate(self.settings.speed.turbo_per_second);
            session.set_turbo(0, pulsing);
            // `egui` painted in the same context since the previous step, and the GPU state
            // mirror does not know it: without telling it, the game would draw with the viewport,
            // scissor and blending the window left behind. See `Session::resume_context`.
            session.resume_context();
            let fast_forwarding = self.fast_forward.update(fast_forward, self.settings.speed.fast_forward.mode);
            let pace = self.settings.speed.pace_with(fast_forwarding);
            self.fast_forwarding = fast_forwarding;
            let step = session.advance(elapsed, &pace);
            self.rewind.track(session);
            if step == Step::Stopped {
                let reason = session.stopped_reason().unwrap_or_default();
                let normal = session.exited_normally();
                log::info!("the game stopped: {reason}");
                // **Exiting the game is not failing.** The applet that asks to close ends in
                // `Outcome::Returned`, and until now every outcome became `play.failed`: closing
                // the game from its own menu went back to the library with an error on screen,
                // saying that what had gone right had gone wrong. The message is kept for what
                // breaks -- an API that does not exist, a null pointer.
                if !normal {
                    self.error = Some(
                        self.catalog
                            .format("play.failed", &[("reason", &reason)]),
                    );
                }
                self.close();
                return;
            }
        }

        // A game that exits on its own — from its menu, through `ISHELL_CloseApplet` — goes back
        // to the library, which is the home screen for whoever opened it from there.
        if session.exited_on_its_own() {
            self.close();
            return;
        }

        let smooth = self.settings.graphics.smooth;
        // With 3D on the GPU, the frame that goes to the screen is **not** `screen()`: it is the
        // texture the rasterizer just filled. And that is the only one that can be wider than
        // 4:3 — the console's framebuffer has the console's size, and the wide aspect opens the
        // field of view at render time, not at composition. That is why changing the aspect
        // changed nothing: the egui texture path always shows the device's frame.
        let via_gpu = self.gl.is_some() && self.settings.graphics.gpu_rasterizer;
        let frame_gl = via_gpu.then(|| session.gpu_frame()).flatten();

        let screen = session.screen();
        let (width, height) = (screen.width() as usize, screen.height() as usize);
        // With no 3D on screen — a menu, an intro — the GPU draws the device's frame, in RGB565,
        // which is the format it is already in. The window repaints more often than the game
        // writes the screen; converting only when generation/writes change avoids a big copy per
        // repaint.
        let frame_2d = match via_gpu && frame_gl.is_none() {
            true => {
                let key_id = (screen.generation(), screen.writes());
                let changed = self
                    .frame_565
                    .as_ref()
                    .is_none_or(|(generation, writes, _)| (*generation, *writes) != key_id);
                if changed {
                    self.frame_565 =
                        Some((key_id.0, key_id.1, std::sync::Arc::from(screen.to_rgb565_bytes())));
                }
                self.frame_565
                    .as_ref()
                    .map(|(_, _, bytes)| (key_id, bytes.clone()))
            }
            false => None,
        };

        if !via_gpu {
            let key_id = (screen.generation(), screen.writes(), smooth);
            // Upload the texture only when the frame changed: the screen repaints more often
            // than the game draws, and each full upload costs a conversion and a trip to the GPU.
            if self.texture.is_none() || self.frame != Some(key_id) {
                let rgba: Vec<u8> = screen
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
                let filter = match smooth {
                    true => egui::TextureOptions::LINEAR,
                    false => egui::TextureOptions::NEAREST,
                };
                match &mut self.texture {
                    Some(texture) => texture.set(image, filter),
                    texture => *texture = Some(ctx.load_texture("tela", image, filter)),
                }
                self.frame = Some(key_id);
            }
        }

        // The numbers come from the session before it is released: drawing needs all of `self`.
        let panel = self.settings.debug.overlay.then(|| {
            (
                session.sample(),
                session.memory(),
                session.clock_ms(),
                session
                    .history()
                    .map(|sample| (sample.speed, sample.fps))
                    .collect::<Vec<_>>(),
            )
        });

        // The panel is a strip at the bottom, not an overlay: reserving space shrinks the frame
        // instead of covering it. It has to be declared before the central panel, because in
        // egui whoever asks for space first is who gets it.
        if let Some((sample, memory, clock, history)) = panel {
            // **Cloned, not moved.** `DebugView` gained a text field (`log_level`, the core's log
            // level), and so stopped being `Copy`. The `clone` is of a struct with half a dozen
            // fields per panel draw pass — and the panel only exists when turned on. This error
            // only showed up in the Android CI: the package does not even compile outside an
            // Android target, so `cargo check` on desktop does not see it.
            let debug = self.settings.debug.clone();
            let dark = egui::Frame::NONE
                .fill(egui::Color32::from_black_alpha(200))
                .inner_margin(egui::Margin::symmetric(8, 2));
            egui::TopBottomPanel::bottom("painel-debug")
                .frame(dark)
                .show(ctx, |ui| {
                    debug_panel::panel(
                        ui,
                        &self.catalog,
                        debug,
                        sample,
                        memory,
                        clock,
                        &history,
                    );
                });
        }

        let black = egui::Frame::NONE.fill(egui::Color32::BLACK);
        egui::CentralPanel::default().frame(black).show(ctx, |ui| {
            // The "window's" aspect ratio follows its size, and so it is stated every frame: any
            // other one is stated once, at startup.
            if self.settings.graphics.ratio == AspectRatio::Window {
                let area = ui.available_size();
                let aspect = area.x / area.y.max(1.0);
                if let Some(session) = self.session.as_mut() {
                    session.set_aspect_ratio(Some(aspect));
                }
            }
            // The wide frame has its own aspect; everything else is the console's 4:3.
            let aspect = frame_gl.map_or(width as f32 / height.max(1) as f32, |q| q.ratio);
            let size = place(
                ui.available_size(),
                aspect,
                height as f32,
                self.settings.graphics.scaling,
                self.settings.graphics.keep_aspect,
            );

            if via_gpu {
                // The closure goes into egui and is called in the middle of painting, with the
                // context current: nothing here can be borrowed from `self`, and so the painter
                // lives behind an `Arc<Mutex<_>>` and the frame goes in an `Arc` -- shared with
                // the cache, not copied on every paint.
                let gl_painter = self.gl_painter.clone();
                let (lg, at) = (width as i32, height as i32);
                let rect = ui
                    .centered_and_justified(|ui| {
                        ui.allocate_exact_size(size, egui::Sense::hover()).0
                    })
                    .inner;
                ui.painter().add(egui::PaintCallback {
                    rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |info, egui_painter| {
                            let Ok(mut guard) = gl_painter.lock() else {
                                return;
                            };
                            let gl = egui_painter.gl();
                            if guard.is_none() {
                                match gpu::GlPainter::create(gl) {
                                    Ok(create) => *guard = Some(create),
                                    Err(error) => {
                                        log::error!("GL painter: {error}");
                                        return;
                                    }
                                }
                            }
                            let Some(gl_painter) = guard.as_mut() else {
                                return;
                            };
                            let vp = gpu::Viewport::from(info.viewport_in_pixels());
                            match frame_gl {
                                Some(frame) => gl_painter.draw_texture(gl, frame, vp, smooth),
                                None => {
                                    if let Some((key_id, bytes)) = &frame_2d {
                                        gl_painter.draw_frame(
                                            gl,
                                            bytes,
                                            lg,
                                            at,
                                            *key_id,
                                            vp,
                                            smooth,
                                        );
                                    }
                                }
                            }
                        },
                    )),
                });
                return;
            }

            if let Some(texture) = &self.texture {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((texture.id(), size)).fit_to_exact_size(size));
                });
            }
        });

        self.draw_overlay(ctx);
        self.draw_indicators(ctx);

        if self.confirming {
            self.question(ctx);
        }
    }

    /// The "back" question: close the game, pause, or continue where it left off.
    fn question(&mut self, ctx: &egui::Context) {
        // The curtain darkens the game and, more importantly, eats the touch: without it a
        // finger outside the dialog would land on the game behind.
        egui::Area::new(egui::Id::new("cortina"))
            .order(egui::Order::Background)
            .show(ctx, |ui| {
                let screen = ui.ctx().viewport_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
            });

        let mut close = false;
        let mut resume = false;
        let mut toggle_pause = false;
        let mut restart = false;
        let mut save_slot = None;
        let mut load_slot = None;
        let mut arm_overwrite = None;
        let paused = self.paused;
        for (slot, saved) in self.save_slots.iter().enumerate() {
            let Some(thumbnail) = saved.as_ref().and_then(|saved| saved.thumbnail.as_ref()) else {
                continue;
            };
            if !self.state_thumbnails.contains_key(&slot) {
                let texture = state_texture(ctx, slot, thumbnail);
                self.state_thumbnails.insert(slot, texture);
            }
        }
        let thumbnails: [Option<egui::TextureId>; save_slots::SLOTS] =
            std::array::from_fn(|slot| self.state_thumbnails.get(&slot).map(|t| t.id()));
        let lines: Vec<(String, String, bool)> = self
            .save_slots
            .iter()
            .enumerate()
            .map(|(slot, saved)| {
                let number = (slot + 1).to_string();
                let title = self
                    .catalog
                    .format("play.state.slot", &[("slot", &number)]);
                let status = saved.as_ref().map_or_else(
                    || self.tr("play.state.empty").to_string(),
                    |saved| {
                        let size = save_slots::size(saved.bytes);
                        self.catalog
                            .format("play.state.used", &[("size", &size)])
                    },
                );
                (title, status, saved.is_some())
            })
            .collect();
        let available_states = self.state_id.is_some();
        let confirming_slot = self.state_overwrite;
        let title = self.tr("play.close.title").to_string();
        let warning = self.tr("play.close.warning").to_string();
        let states_title = self.tr("play.state.title").to_string();
        let save_label = self.tr("play.state.save").to_string();
        let overwrite_label = self.tr("play.state.overwrite").to_string();
        let confirm_label = self.tr("play.state.confirm").to_string();
        let overwrite_warning = self.tr("play.state.overwrite.hint").to_string();
        let load_label = self.tr("play.state.load").to_string();
        let unavailable = self.tr("play.state.unavailable").to_string();
        let resume_label = self.tr("play.continue").to_string();
        let pause_label = match paused {
            true => self.tr("play.resume").to_string(),
            false => self.tr("play.pause").to_string(),
        };
        let stop_label = self.tr("play.stop").to_string();
        let restart_label = self.tr("play.restart").to_string();
        let message = self.state_message.clone();

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(580.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(warning);
                ui.add_space(8.0);
                ui.separator();
                ui.heading(&states_title);
                if !available_states {
                    ui.label(&unavailable);
                }
                if let Some(message) = &message {
                    ui.label(message);
                }
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    // At 960x544, the frontend's scaling rule leaves about 340 points of height.
                    // Two visible rows plus scrolling fit alongside the title and the exit
                    // buttons; 300 points here would push half the window off the screen.
                    .max_height(170.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (slot, (title, status, busy)) in lines.iter().enumerate() {
                            ui.group(|ui| {
                                ui.set_min_width(540.0);
                                ui.horizontal(|ui| {
                                    let size = egui::vec2(96.0, 72.0);
                                    match thumbnails[slot] {
                                        Some(id) => {
                                            ui.add(
                                                egui::Image::new((id, size))
                                                    .fit_to_exact_size(size),
                                            );
                                        }
                                        None => {
                                            let (rect, _) =
                                                ui.allocate_exact_size(size, egui::Sense::hover());
                                            ui.painter().rect_filled(
                                                rect,
                                                4.0,
                                                egui::Color32::from_gray(18),
                                            );
                                        }
                                    }

                                    ui.vertical(|ui| {
                                        ui.strong(title);
                                        ui.label(status);
                                        if confirming_slot == Some(slot) {
                                            ui.small(&overwrite_warning);
                                        }
                                    });

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let load = ui.add_enabled(
                                                available_states && *busy,
                                                egui::Button::new(&load_label)
                                                    .min_size(egui::vec2(88.0, 44.0)),
                                            );
                                            if load.clicked() {
                                                load_slot = Some(slot);
                                            }

                                            let label =
                                                match (*busy, confirming_slot == Some(slot)) {
                                                    (_, true) => &confirm_label,
                                                    (true, false) => &overwrite_label,
                                                    (false, false) => &save_label,
                                                };
                                            if ui
                                                .add_enabled(
                                                    available_states,
                                                    egui::Button::new(label)
                                                        .min_size(egui::vec2(120.0, 44.0)),
                                                )
                                                .clicked()
                                            {
                                                if *busy && confirming_slot != Some(slot) {
                                                    arm_overwrite = Some(slot);
                                                } else {
                                                    save_slot = Some(slot);
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });

                ui.separator();
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let proceed =
                        ui.add_sized([150.0, 48.0], egui::Button::new(&resume_label));
                    let pause = ui.add_sized([110.0, 48.0], egui::Button::new(&pause_label));
                    let reset =
                        ui.add_sized([110.0, 48.0], egui::Button::new(&restart_label));
                    let stop = ui.add_sized([110.0, 48.0], egui::Button::new(&stop_label));
                    resume = proceed.clicked();
                    toggle_pause = pause.clicked();
                    restart = reset.clicked();
                    close = stop.clicked();
                    // **The same first focus as the other screens.** Without it this window did
                    // not respond to any controller: egui moves focus in the arrow's direction,
                    // but only from one that exists. "Continue" is the one that gets it, because
                    // it is what someone who opened the window by accident will want to press.
                    if ui.ctx().memory(|m| m.focused()).is_none() {
                        proceed.request_focus();
                    }
                });
                ui.add_space(4.0);
            });

        if let Some(slot) = arm_overwrite {
            self.state_overwrite = Some(slot);
        }
        if let Some(slot) = save_slot {
            let result = match (self.state_id.as_deref(), self.session.as_mut()) {
                (Some(id), Some(session)) => {
                    save_slots::save(&self.states_root, id, slot, session)
                }
                _ => Err("save state unavailable".to_string()),
            };
            match result {
                Ok(saved) => {
                    self.save_slots[slot] = Some(saved);
                    self.state_thumbnails.remove(&slot);
                    self.state_overwrite = None;
                    let number = (slot + 1).to_string();
                    self.state_message = Some(
                        self.catalog
                            .format("play.state.saved", &[("slot", &number)]),
                    );
                    log::info!("save state written to slot {}", slot + 1);
                }
                Err(error) => {
                    log::error!("save state: {error}");
                    self.state_message = Some(
                        self.catalog
                            .format("play.state.error", &[("reason", &error)]),
                    );
                }
            }
        }
        if let Some(slot) = load_slot {
            let result = match (self.state_id.as_deref(), self.session.as_mut()) {
                (Some(id), Some(session)) => {
                    save_slots::load(&self.states_root, id, slot, session)
                }
                _ => Err("save state unavailable".to_string()),
            };
            match result {
                Ok(_) => {
                    log::info!("save state loaded from slot {}", slot + 1);
                    // The rewind points belong to another timeline.
                    self.rewind.forget_points();
                    // The console state is back; the physical input, however, is what is pressed
                    // now, not what was pressed at the saved instant. Clearing it keeps the
                    // button used to confirm Load from getting into the first restored frame.
                    self.pad = Default::default();
                    if let Some(session) = self.session.as_mut() {
                        session.set_port_pad(0, self.pad);
                    }
                    self.texture = None;
                    self.frame = None;
                    self.frame_565 = None;
                    self.confirming = false;
                    self.paused = false;
                    self.state_overwrite = None;
                    self.last = Instant::now();
                    return;
                }
                Err(error) => {
                    log::error!("load state: {error}");
                    self.state_message = Some(
                        self.catalog
                            .format("play.state.error", &[("reason", &error)]),
                    );
                }
            }
        }

        if resume {
            self.confirming = false;
            self.paused = false;
            self.state_overwrite = None;
        }
        if toggle_pause {
            self.paused = !paused;
            self.confirming = false;
            self.state_overwrite = None;
        }
        if restart {
            self.restart();
        }
        if close {
            self.close();
        }
    }
}

fn state_texture(
    ctx: &egui::Context,
    slot: usize,
    thumbnail: &save_slots::Thumbnail,
) -> egui::TextureHandle {
    let mut rgba = Vec::with_capacity(thumbnail.width * thumbnail.height * 4);
    for pixel in thumbnail.rgb565.chunks_exact(2) {
        let color = zeebx::video::display::Rgb::from_rgb565(u16::from_le_bytes([
            pixel[0], pixel[1],
        ]));
        rgba.extend_from_slice(&[color.r, color.g, color.b, 255]);
    }
    let image =
        egui::ColorImage::from_rgba_unmultiplied([thumbnail.width, thumbnail.height], &rgba);
    ctx.load_texture(
        format!("save-state-{slot}"),
        image,
        egui::TextureOptions::LINEAR,
    )
}

/// How much of the available area the frame takes up.
///
/// It is the same rule as desktop: integer pixels never blur and leave a border; fit respects the
/// aspect ratio; stretch only distorts when the aspect was waived.
fn place(area: egui::Vec2, aspect: f32, height: f32, mode: Scaling, keep: bool) -> egui::Vec2 {
    // Integer scaling's "1x" is the height of the console's surface: it is the measure that does
    // not change when the aspect widens, because the wide aspect adds columns, not rows.
    let native = egui::vec2(height * aspect, height);
    match mode {
        Scaling::Integer => {
            let factor = (area.x / native.x).min(area.y / native.y);
            match factor >= 1.0 {
                true => native * factor.floor(),
                // Smaller than the original frame there is no integer multiple: just fit.
                false => fit(area, aspect),
            }
        }
        Scaling::Fit => fit(area, aspect),
        Scaling::Stretch => match keep {
            true => fit(area, aspect),
            false => area,
        },
    }
}

/// The largest rectangle with the given aspect ratio that fits in the area.
fn fit(area: egui::Vec2, aspect: f32) -> egui::Vec2 {
    match area.x / area.y.max(1.0) > aspect {
        true => egui::vec2(area.y * aspect, area.y),
        false => egui::vec2(area.x, area.x / aspect),
    }
}

impl Emulator {
    /// The "◀◀", the "▶▶ 3x" and the "Turbo" over the game, in the top-left corner, while they
    /// are active: a toggle left on by mistake looks like a game bug. The same texts as the
    /// desktop window.
    fn draw_indicators(&self, ctx: &egui::Context) {
        let decimal_comma = !self.catalog.current().starts_with("en");
        let rewind = self.rewind.indicator().map(|no_points| match no_points {
            true => format!("◀◀ {}", self.catalog.get("rewind.empty")),
            false => "◀◀".to_string(),
        });
        let fast_forward = (self.fast_forwarding)
            .then(|| self.session.as_ref().map(|session| session.reached_speed()))
            .flatten()
            .map(|reached| {
                zeebx::speed::fast_forward_label(
                    self.settings.speed.fast_forward.ratio(),
                    reached,
                    decimal_comma,
                )
            });
        let turbo = self.turbo.enabled().then(|| self.catalog.get("turbo.on_screen").to_string());
        let text: Vec<String> = rewind.into_iter().chain(fast_forward).chain(turbo).collect();
        if text.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("indicadores"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(16.0, 16.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_black_alpha(200))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(text.join("  ·  "))
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                    });
            });
    }
}
