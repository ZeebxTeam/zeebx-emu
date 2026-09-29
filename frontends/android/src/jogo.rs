//! O jogo rodando.
//!
//! Num aparelho de mão o controle é físico, e botão na tela só rouba espaço: o quadro ocupa
//! tudo, e quem sai é o "voltar" do Android. O que aparece por cima é o painel de depuração,
//! ligado nas configurações — e ele é o [`zeebx::ui::depuracao::painel`], o mesmo do desktop.

use std::time::Instant;

use zeebx::session::Step;
use zeebx::ui::depuracao;
use zeebx::ui::gpu;
use zeebx::ui::settings::{Proporcao, Scaling};

use crate::Emulador;
use crate::estado;
use zeebx::session::FATIA_MAXIMA;

impl Emulador {
    pub(crate) fn jogo(&mut self, ctx: &egui::Context) {
        let Some(sessao) = self.sessao.as_mut() else {
            return;
        };

        // Com a pergunta na tela, ou em pausa, o jogo fica parado — inclusive o relógio, para
        // ele não gastar o orçamento todo de uma vez quando voltar.
        let parado = self.confirmando || self.pausado;
        if parado {
            self.ultimo = Instant::now();
        } else {
            // O orçamento é o tempo real que passou desde o quadro anterior, preso ao teto: é
            // quanto o jogo precisa emular para acompanhar o relógio do mundo.
            let passou = self.ultimo.elapsed().min(FATIA_MAXIMA);
            self.ultimo = Instant::now();
            sessao.set_port_pad(0, self.pad);
            if sessao.step(passou, self.settings.graphics.speed_limit) == Step::Stopped {
                let motivo = sessao.stopped_reason().unwrap_or_default();
                let normal = sessao.saiu_normalmente();
                log::info!("o jogo parou: {motivo}");
                // **Sair do jogo não é falhar.** O applet que pede para fechar termina em
                // `Outcome::Returned`, e até aqui todo desfecho virava `play.failed`: fechar o
                // jogo pelo menu dele devolvia à biblioteca com um erro na tela, dizendo que
                // tinha dado errado o que tinha dado certo. A mensagem fica para o que quebra --
                // API que não existe, ponteiro vazio.
                if !normal {
                    self.erro = Some(
                        self.catalogo
                            .format("play.failed", &[("reason", &motivo)]),
                    );
                }
                self.fecha();
                return;
            }
        }

        // Um jogo que sai sozinho — pelo menu dele, pelo `ISHELL_CloseApplet` — volta para a
        // biblioteca, que é a tela inicial de quem abriu por ela.
        if sessao.saiu_sozinho() {
            self.fecha();
            return;
        }

        let suave = self.settings.graphics.smooth;
        // Com o 3D na placa, o quadro que vai à tela **não** é o `screen()`: é a textura que o
        // rasterizador acabou de preencher. E essa é a única que pode ser mais larga que 4:3 —
        // o framebuffer do console tem o tamanho do console, e a proporção larga abre o campo
        // de visão na renderização, não na composição. Era por isso que mexer na proporção não
        // mudava nada: o caminho da textura do egui sempre mostra o quadro do aparelho.
        let pela_placa = self.gl.is_some() && self.settings.graphics.gpu_rasterizer;
        let quadro_gl = pela_placa.then(|| sessao.quadro_na_placa()).flatten();

        let tela = sessao.screen();
        let (largura, altura) = (tela.width() as usize, tela.height() as usize);
        // Sem 3D na tela — um menu, uma abertura — a placa desenha o quadro do aparelho, em
        // RGB565, que é o formato em que ele já está. A janela repinta mais vezes que o jogo
        // escreve a tela; converter só quando série/escritas mudarem evita uma cópia grande por
        // repaint.
        let quadro_2d = match pela_placa && quadro_gl.is_none() {
            true => {
                let chave = (tela.serie(), tela.escritas());
                let mudou = self
                    .quadro_565
                    .as_ref()
                    .is_none_or(|(serie, escritas, _)| (*serie, *escritas) != chave);
                if mudou {
                    self.quadro_565 =
                        Some((chave.0, chave.1, std::sync::Arc::from(tela.to_rgb565_bytes())));
                }
                self.quadro_565
                    .as_ref()
                    .map(|(_, _, bytes)| (chave, bytes.clone()))
            }
            false => None,
        };

        if !pela_placa {
            let chave = (tela.serie(), tela.escritas(), suave);
            // Subir a textura só quando o quadro mudou: a tela repinta mais vezes que o jogo
            // desenha, e cada subida inteira custa uma conversão e uma ida à placa.
            if self.textura.is_none() || self.quadro != Some(chave) {
                let rgba: Vec<u8> = tela
                    .to_argb()
                    .into_iter()
                    .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                    .collect();
                let imagem = egui::ColorImage::from_rgba_unmultiplied([largura, altura], &rgba);
                let filtro = match suave {
                    true => egui::TextureOptions::LINEAR,
                    false => egui::TextureOptions::NEAREST,
                };
                match &mut self.textura {
                    Some(textura) => textura.set(imagem, filtro),
                    textura => *textura = Some(ctx.load_texture("tela", imagem, filtro)),
                }
                self.quadro = Some(chave);
            }
        }

        // Os números vêm da sessão antes de ela ser solta: desenhar precisa do `self` inteiro.
        let painel = self.settings.debug.overlay.then(|| {
            (
                sessao.sample(),
                sessao.memory(),
                sessao.clock_ms(),
                sessao
                    .history()
                    .map(|amostra| (amostra.speed, amostra.fps))
                    .collect::<Vec<_>>(),
            )
        });

        // O painel é uma faixa embaixo, e não uma sobreposição: reservar espaço encolhe o
        // quadro em vez de tapá-lo. Precisa ser declarado antes do painel central, porque no
        // egui quem pede espaço primeiro é quem o recebe.
        if let Some((amostra, memoria, relogio, historia)) = painel {
            // **Clonado, e não movido.** O `DebugView` ganhou um campo de texto (`nivel_de_log`,
            // o nível do registro do núcleo), e por isso deixou de ser `Copy`. O `clone` é de uma
            // struct de meia dúzia de campos por volta de desenho do painel — e o painel só existe
            // quando ligado. Este erro só apareceu na CI do Android: o pacote nem compila fora de
            // um alvo Android, então `cargo check` no desktop não o vê.
            let debug = self.settings.debug.clone();
            let escuro = egui::Frame::NONE
                .fill(egui::Color32::from_black_alpha(200))
                .inner_margin(egui::Margin::symmetric(8, 2));
            egui::TopBottomPanel::bottom("painel-debug")
                .frame(escuro)
                .show(ctx, |ui| {
                    depuracao::painel(
                        ui,
                        &self.catalogo,
                        debug,
                        amostra,
                        memoria,
                        relogio,
                        &historia,
                    );
                });
        }

        let preto = egui::Frame::NONE.fill(egui::Color32::BLACK);
        egui::CentralPanel::default().frame(preto).show(ctx, |ui| {
            // A proporção "a da janela" acompanha o tamanho dela, e por isso é dita a cada
            // quadro: qualquer outra é dita uma vez, na abertura.
            if self.settings.graphics.proporcao == Proporcao::Janela {
                let area = ui.available_size();
                let aspecto = area.x / area.y.max(1.0);
                if let Some(sessao) = self.sessao.as_mut() {
                    sessao.define_proporcao(Some(aspecto));
                }
            }
            // O quadro largo tem a proporção dele; o resto é o 4:3 do console.
            let aspecto = quadro_gl.map_or(largura as f32 / altura.max(1) as f32, |q| q.proporcao);
            let tamanho = coloca(
                ui.available_size(),
                aspecto,
                altura as f32,
                self.settings.graphics.scaling,
                self.settings.graphics.keep_aspect,
            );

            if pela_placa {
                // O fecho vai para dentro do egui e é chamado no meio da pintura, com o
                // contexto corrente: nada daqui pode ser emprestado do `self`, e por isso o
                // pintor mora atrás de um `Arc<Mutex<_>>` e o quadro vai num `Arc` --
                // partilhado com o cache, e não copiado a cada pintura.
                let pintor = self.pintor.clone();
                let (lg, at) = (largura as i32, altura as i32);
                let rect = ui
                    .centered_and_justified(|ui| {
                        ui.allocate_exact_size(tamanho, egui::Sense::hover()).0
                    })
                    .inner;
                ui.painter().add(egui::PaintCallback {
                    rect,
                    callback: std::sync::Arc::new(egui_glow::CallbackFn::new(
                        move |info, pintor_do_egui| {
                            let Ok(mut guarda) = pintor.lock() else {
                                return;
                            };
                            let gl = pintor_do_egui.gl();
                            if guarda.is_none() {
                                match gpu::Pintor::novo(gl) {
                                    Ok(novo) => *guarda = Some(novo),
                                    Err(erro) => {
                                        log::error!("pintor de GL: {erro}");
                                        return;
                                    }
                                }
                            }
                            let Some(pintor) = guarda.as_mut() else {
                                return;
                            };
                            let vp = gpu::Viewport::from(info.viewport_in_pixels());
                            match quadro_gl {
                                Some(quadro) => pintor.desenha_textura(gl, quadro, vp, suave),
                                None => {
                                    if let Some((chave, bytes)) = &quadro_2d {
                                        pintor.desenha_quadro(
                                            gl,
                                            bytes,
                                            lg,
                                            at,
                                            *chave,
                                            vp,
                                            suave,
                                        );
                                    }
                                }
                            }
                        },
                    )),
                });
                return;
            }

            if let Some(textura) = &self.textura {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((textura.id(), tamanho)).fit_to_exact_size(tamanho));
                });
            }
        });

        if self.confirmando {
            self.pergunta(ctx);
        }
    }

    /// A pergunta do "voltar": fechar o jogo, pausar, ou continuar de onde parou.
    fn pergunta(&mut self, ctx: &egui::Context) {
        // A cortina escurece o jogo e, mais importante, come o toque: sem ela um dedo fora da
        // caixa iria parar no jogo atrás.
        egui::Area::new(egui::Id::new("cortina"))
            .order(egui::Order::Background)
            .show(ctx, |ui| {
                let tela = ui.ctx().viewport_rect();
                ui.painter()
                    .rect_filled(tela, 0.0, egui::Color32::from_black_alpha(180));
            });

        let mut fechar = false;
        let mut continuar = false;
        let mut alterna_pausa = false;
        let mut salvar_slot = None;
        let mut carregar_slot = None;
        let mut armar_sobrescrita = None;
        let pausado = self.pausado;
        for (slot, salvo) in self.estados.iter().enumerate() {
            let Some(miniatura) = salvo.as_ref().and_then(|salvo| salvo.miniatura.as_ref()) else {
                continue;
            };
            if !self.miniaturas_estado.contains_key(&slot) {
                let textura = textura_de_estado(ctx, slot, miniatura);
                self.miniaturas_estado.insert(slot, textura);
            }
        }
        let miniaturas: [Option<egui::TextureId>; estado::SLOTS] =
            std::array::from_fn(|slot| self.miniaturas_estado.get(&slot).map(|t| t.id()));
        let linhas: Vec<(String, String, bool)> = self
            .estados
            .iter()
            .enumerate()
            .map(|(slot, salvo)| {
                let numero = (slot + 1).to_string();
                let titulo = self
                    .catalogo
                    .format("play.state.slot", &[("slot", &numero)]);
                let status = salvo.as_ref().map_or_else(
                    || self.tr("play.state.empty").to_string(),
                    |salvo| {
                        let tamanho = estado::tamanho(salvo.bytes);
                        self.catalogo
                            .format("play.state.used", &[("size", &tamanho)])
                    },
                );
                (titulo, status, salvo.is_some())
            })
            .collect();
        let estados_disponiveis = self.estado_id.is_some();
        let confirmando_slot = self.estado_sobrescrever;
        let titulo = self.tr("play.close.title").to_string();
        let aviso = self.tr("play.close.warning").to_string();
        let estados_titulo = self.tr("play.state.title").to_string();
        let salvar_rotulo = self.tr("play.state.save").to_string();
        let substituir_rotulo = self.tr("play.state.overwrite").to_string();
        let confirmar_rotulo = self.tr("play.state.confirm").to_string();
        let substituir_aviso = self.tr("play.state.overwrite.hint").to_string();
        let carregar_rotulo = self.tr("play.state.load").to_string();
        let indisponivel = self.tr("play.state.unavailable").to_string();
        let continuar_rotulo = self.tr("play.continue").to_string();
        let pausa_rotulo = match pausado {
            true => self.tr("play.resume").to_string(),
            false => self.tr("play.pause").to_string(),
        };
        let parar_rotulo = self.tr("play.stop").to_string();
        let mensagem = self.estado_mensagem.clone();

        egui::Window::new(titulo)
            .collapsible(false)
            .resizable(false)
            .default_width(580.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(aviso);
                ui.add_space(8.0);
                ui.separator();
                ui.heading(&estados_titulo);
                if !estados_disponiveis {
                    ui.label(&indisponivel);
                }
                if let Some(mensagem) = &mensagem {
                    ui.label(mensagem);
                }
                ui.add_space(4.0);

                egui::ScrollArea::vertical()
                    // Em 960x544, a regra de escala do frontend deixa cerca de 340 pontos de
                    // altura. Duas linhas visíveis e rolagem cabem junto do título e dos botões
                    // de sair; 300 pontos aqui empurrariam metade da janela para fora da tela.
                    .max_height(170.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (slot, (titulo, status, ocupado)) in linhas.iter().enumerate() {
                            ui.group(|ui| {
                                ui.set_min_width(540.0);
                                ui.horizontal(|ui| {
                                    let tamanho = egui::vec2(96.0, 72.0);
                                    match miniaturas[slot] {
                                        Some(id) => {
                                            ui.add(
                                                egui::Image::new((id, tamanho))
                                                    .fit_to_exact_size(tamanho),
                                            );
                                        }
                                        None => {
                                            let (rect, _) =
                                                ui.allocate_exact_size(tamanho, egui::Sense::hover());
                                            ui.painter().rect_filled(
                                                rect,
                                                4.0,
                                                egui::Color32::from_gray(18),
                                            );
                                        }
                                    }

                                    ui.vertical(|ui| {
                                        ui.strong(titulo);
                                        ui.label(status);
                                        if confirmando_slot == Some(slot) {
                                            ui.small(&substituir_aviso);
                                        }
                                    });

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let load = ui.add_enabled(
                                                estados_disponiveis && *ocupado,
                                                egui::Button::new(&carregar_rotulo)
                                                    .min_size(egui::vec2(88.0, 44.0)),
                                            );
                                            if load.clicked() {
                                                carregar_slot = Some(slot);
                                            }

                                            let rotulo =
                                                match (*ocupado, confirmando_slot == Some(slot)) {
                                                    (_, true) => &confirmar_rotulo,
                                                    (true, false) => &substituir_rotulo,
                                                    (false, false) => &salvar_rotulo,
                                                };
                                            if ui
                                                .add_enabled(
                                                    estados_disponiveis,
                                                    egui::Button::new(rotulo)
                                                        .min_size(egui::vec2(120.0, 44.0)),
                                                )
                                                .clicked()
                                            {
                                                if *ocupado && confirmando_slot != Some(slot) {
                                                    armar_sobrescrita = Some(slot);
                                                } else {
                                                    salvar_slot = Some(slot);
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
                    let seguir =
                        ui.add_sized([150.0, 48.0], egui::Button::new(&continuar_rotulo));
                    let pausa = ui.add_sized([110.0, 48.0], egui::Button::new(&pausa_rotulo));
                    let parar = ui.add_sized([110.0, 48.0], egui::Button::new(&parar_rotulo));
                    continuar = seguir.clicked();
                    alterna_pausa = pausa.clicked();
                    fechar = parar.clicked();
                    // **O mesmo primeiro foco das outras telas.** Sem ele esta janela não
                    // respondia a controle nenhum: o egui move o foco na direção da seta, mas só
                    // a partir de um que exista. O "continuar" é o que recebe, porque é o que
                    // quem abriu a janela sem querer vai querer apertar.
                    if ui.ctx().memory(|m| m.focused()).is_none() {
                        seguir.request_focus();
                    }
                });
                ui.add_space(4.0);
            });

        if let Some(slot) = armar_sobrescrita {
            self.estado_sobrescrever = Some(slot);
        }
        if let Some(slot) = salvar_slot {
            let resultado = match (self.estado_id.as_deref(), self.sessao.as_mut()) {
                (Some(id), Some(sessao)) => {
                    estado::salva(&self.estados_raiz, id, slot, sessao)
                }
                _ => Err("save state indisponível".to_string()),
            };
            match resultado {
                Ok(salvo) => {
                    self.estados[slot] = Some(salvo);
                    self.miniaturas_estado.remove(&slot);
                    self.estado_sobrescrever = None;
                    let numero = (slot + 1).to_string();
                    self.estado_mensagem = Some(
                        self.catalogo
                            .format("play.state.saved", &[("slot", &numero)]),
                    );
                    log::info!("save state gravado no slot {}", slot + 1);
                }
                Err(erro) => {
                    log::error!("save state: {erro}");
                    self.estado_mensagem = Some(
                        self.catalogo
                            .format("play.state.error", &[("reason", &erro)]),
                    );
                }
            }
        }
        if let Some(slot) = carregar_slot {
            let resultado = match (self.estado_id.as_deref(), self.sessao.as_mut()) {
                (Some(id), Some(sessao)) => {
                    estado::carrega(&self.estados_raiz, id, slot, sessao)
                }
                _ => Err("save state indisponível".to_string()),
            };
            match resultado {
                Ok(_) => {
                    log::info!("save state carregado do slot {}", slot + 1);
                    // O estado do console voltou; a entrada física, porém, é o que está apertado
                    // agora, não o que estava apertado no instante salvo. Zerar evita que o botão
                    // usado para confirmar o Load entre no primeiro quadro restaurado.
                    self.pad = Default::default();
                    if let Some(sessao) = self.sessao.as_mut() {
                        sessao.set_port_pad(0, self.pad);
                    }
                    self.textura = None;
                    self.quadro = None;
                    self.quadro_565 = None;
                    self.confirmando = false;
                    self.pausado = false;
                    self.estado_sobrescrever = None;
                    self.ultimo = Instant::now();
                    return;
                }
                Err(erro) => {
                    log::error!("load state: {erro}");
                    self.estado_mensagem = Some(
                        self.catalogo
                            .format("play.state.error", &[("reason", &erro)]),
                    );
                }
            }
        }

        if continuar {
            self.confirmando = false;
            self.pausado = false;
            self.estado_sobrescrever = None;
        }
        if alterna_pausa {
            self.pausado = !pausado;
            self.confirmando = false;
            self.estado_sobrescrever = None;
        }
        if fechar {
            self.fecha();
        }
    }
}

fn textura_de_estado(
    ctx: &egui::Context,
    slot: usize,
    miniatura: &estado::Miniatura,
) -> egui::TextureHandle {
    let mut rgba = Vec::with_capacity(miniatura.largura * miniatura.altura * 4);
    for pixel in miniatura.rgb565.chunks_exact(2) {
        let cor = zeebx::video::display::Rgb::from_rgb565(u16::from_le_bytes([
            pixel[0], pixel[1],
        ]));
        rgba.extend_from_slice(&[cor.r, cor.g, cor.b, 255]);
    }
    let imagem =
        egui::ColorImage::from_rgba_unmultiplied([miniatura.largura, miniatura.altura], &rgba);
    ctx.load_texture(
        format!("save-state-{slot}"),
        imagem,
        egui::TextureOptions::LINEAR,
    )
}

/// Que tamanho o quadro ocupa na área disponível.
///
/// É a mesma regra do desktop: pixel inteiro nunca borra e sobra borda; caber respeita a
/// proporção; preencher só deforma quando o formato foi dispensado.
fn coloca(area: egui::Vec2, aspecto: f32, altura: f32, modo: Scaling, manter: bool) -> egui::Vec2 {
    // O "uma vez" do pixel inteiro é a altura da superfície do console: é a medida que não
    // muda quando a proporção abre, porque a proporção larga acrescenta colunas, não linhas.
    let nativo = egui::vec2(altura * aspecto, altura);
    match modo {
        Scaling::Integer => {
            let fator = (area.x / nativo.x).min(area.y / nativo.y);
            match fator >= 1.0 {
                true => nativo * fator.floor(),
                // Menor que o quadro original não há múltiplo inteiro: cabe, e pronto.
                false => caber(area, aspecto),
            }
        }
        Scaling::Fit => caber(area, aspecto),
        Scaling::Stretch => match manter {
            true => caber(area, aspecto),
            false => area,
        },
    }
}

/// O maior retângulo com a proporção dada que cabe na área.
fn caber(area: egui::Vec2, aspecto: f32) -> egui::Vec2 {
    match area.x / area.y.max(1.0) > aspecto {
        true => egui::vec2(area.y * aspecto, area.y),
        false => egui::vec2(area.x, area.x / aspecto),
    }
}
