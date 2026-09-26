//! A janela crua: o jogo e nada mais.
//!
//! **Não há eframe aqui, e é essa a razão de este arquivo existir** — pela mesma razão que o
//! frontend de Android tem o seu. O eframe existe para pôr uma interface na tela, e aqui não há
//! interface: há um quadro, uma janela e um contexto de OpenGL que a sessão vai usar para
//! rasterizar o 3D. Girar um laço de egui por baixo de nada seria pagar a interface inteira
//! para desenhar um retângulo.
//!
//! O contexto **é o da janela**, e não o de fora de tela do [`zeebx::video::contexto`]. Com
//! janela, o quadro que a placa acabou de preencher já está na memória dela: pintá-lo é ligar a
//! textura e desenhar um triângulo. Pelo contexto de fora de tela ele teria de voltar à CPU e
//! subir de novo, uma vez por quadro.

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowAttributes};

use zeebx::ui::gpu::Pintor;
use zeebx::ui::settings::Scaling;

use crate::config::{Headless, Video};

/// A janela, o contexto e o pincel que põe o quadro nela.
pub struct Janela {
    pub janela: Window,
    superficie: Surface<WindowSurface>,
    contexto: PossiblyCurrentContext,
    /// O mesmo `glow` que a sessão recebe quando o 3D roda na placa.
    pub gl: Arc<glow::Context>,
    pintor: Option<Pintor>,
}

impl Janela {
    /// Abre a janela e monta o contexto. Só serve dentro do `resumed` do winit: antes disso não
    /// há laço ativo para criar janela nenhuma.
    pub fn abre(laco: &ActiveEventLoop, opcoes: &Headless) -> Result<Self, String> {
        let (largura, altura) = opcoes.tamanho;
        let atributos = WindowAttributes::default()
            .with_title(opcoes.titulo.clone())
            .with_inner_size(winit::dpi::PhysicalSize::new(largura, altura))
            // A tela cheia é a "de borda", e não a exclusiva: trocar o modo de vídeo do monitor
            // para mostrar 640×480 ampliado não serve a ninguém, e sair de uma exclusiva que
            // travou é muito pior do que sair de uma janela.
            .with_fullscreen(match opcoes.video {
                Video::TelaCheia => Some(Fullscreen::Borderless(None)),
                _ => None,
            });

        // Sem alpha e sem profundidade na janela: o que se desenha aqui é um retângulo opaco
        // com uma textura. A profundidade que o 3D usa é a do framebuffer do rasterizador, que
        // é dele e não desta superfície.
        let modelo = ConfigTemplateBuilder::new().with_alpha_size(0);
        let (janela, config) = DisplayBuilder::new()
            .with_window_attributes(Some(atributos))
            .build(laco, modelo, escolhe_config)
            .map_err(|erro| format!("no OpenGL window: {erro}"))?;
        let janela = janela.ok_or("the system returned no window")?;

        let janela_crua = janela
            .window_handle()
            .map_err(|erro| format!("window without a handle: {erro}"))?
            .as_raw();
        let display = config.display();
        // Sem pedir versão nem perfil: o glutin resolve isso no melhor que o driver oferece, e
        // é o mesmo que o eframe faz — o que mantém os shaders do [`Pintor`] valendo nos dois.
        let atributos = ContextAttributesBuilder::new().build(Some(janela_crua));
        let contexto = unsafe { display.create_context(&config, &atributos) }
            .map_err(|erro| format!("no GL context: {erro}"))?;

        let atributos = janela
            .build_surface_attributes(Default::default())
            .map_err(|erro| format!("the window describes no surface: {erro}"))?;
        let superficie = unsafe { display.create_window_surface(&config, &atributos) }
            .map_err(|erro| format!("no GL surface: {erro}"))?;
        let contexto = contexto
            .make_current(&superficie)
            .map_err(|erro| format!("the context did not become current: {erro}"))?;

        // O vsync é escolha de quem configurou. Ligado, a janela troca o quadro no retraço e o
        // laço dorme nele; desligado, ela corre solta — que é o que quem mede quer.
        let intervalo = match opcoes.vsync {
            true => SwapInterval::Wait(NonZeroU32::new(1).unwrap()),
            false => SwapInterval::DontWait,
        };
        if let Err(erro) = superficie.set_swap_interval(&contexto, intervalo) {
            eprintln!("warning: the driver refused the requested vsync: {erro}");
        }

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|nome| display.get_proc_address(nome))
        });
        // O pincel é opcional: um driver que recuse os shaders deixa a janela preta em vez de
        // derrubar o emulador, e a mensagem diz por quê.
        let pintor = match Pintor::novo(&gl) {
            Ok(pintor) => Some(pintor),
            Err(erro) => {
                eprintln!("warning: without the GL painter the window stays black: {erro}");
                None
            }
        };

        Ok(Self {
            janela,
            superficie,
            contexto,
            gl,
            pintor,
        })
    }

    /// O tamanho da superfície em pixels.
    pub fn tamanho(&self) -> (u32, u32) {
        let tamanho = self.janela.inner_size();
        (tamanho.width.max(1), tamanho.height.max(1))
    }

    /// Acompanha a janela quando ela muda de tamanho.
    pub fn redimensiona(&self, largura: u32, altura: u32) {
        let (Some(largura), Some(altura)) = (NonZeroU32::new(largura), NonZeroU32::new(altura))
        else {
            return;
        };
        self.superficie.resize(&self.contexto, largura, altura);
    }

    /// Pinta o quadro que a placa preencheu, sem ele voltar à CPU.
    /// `altura_nativa` é a do quadro do console, que é a unidade da escala inteira — a
    /// textura já vem ampliada pela resolução interna, e contar múltiplos sobre ela daria
    /// uma janela cheia de borda sem motivo.
    pub fn mostra_textura(&mut self, quadro: zeebx::video::rasterizer::QuadroNaPlaca, altura_nativa: u32, escala: Scaling, proporcao_4_3: bool, suave: bool) {
        let aspecto = match proporcao_4_3 {
            true => Some(quadro.proporcao),
            false => None,
        };
        let vp = self.limpa_e_enquadra(altura_nativa, aspecto, escala);
        if let Some(pintor) = &mut self.pintor {
            pintor.desenha_textura(&self.gl, quadro, vp.into(), suave);
        }
        self.troca();
    }

    /// Pinta o quadro do aparelho, em RGB565 — que é o formato em que ele já está.
    pub fn mostra_quadro(&mut self, bytes: &[u8], largura: i32, altura: i32, escala: Scaling, proporcao_4_3: bool, suave: bool) {
        let aspecto = match proporcao_4_3 && altura > 0 {
            true => Some(largura as f32 / altura as f32),
            false => None,
        };
        let vp = self.limpa_e_enquadra(altura.max(0) as u32, aspecto, escala);
        if let Some(pintor) = &mut self.pintor {
            pintor.desenha(&self.gl, bytes, largura, altura, vp.into(), suave);
        }
        self.troca();
    }

    /// Limpa a janela de preto e devolve o retângulo em que o quadro cabe.
    ///
    /// A limpeza vem antes porque a borda que sobra é da janela, não do jogo: sem ela ficaria o
    /// lixo do quadro anterior nas faixas laterais.
    fn limpa_e_enquadra(&self, altura_nativa: u32, aspecto: Option<f32>, escala: Scaling) -> egui::epaint::ViewportInPixels {
        use glow::HasContext;
        let (largura, altura) = self.tamanho();
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.viewport(0, 0, largura as i32, altura as i32);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        enquadra(largura, altura, altura_nativa, aspecto, escala)
    }

    /// Devolve à placa o que foi criado aqui. O contexto ainda está corrente neste ponto.
    pub fn solta(self) {
        if let Some(pintor) = &self.pintor {
            pintor.solta(&self.gl);
        }
    }

    fn troca(&self) {
        if let Err(erro) = self.superficie.swap_buffers(&self.contexto) {
            eprintln!("warning: the buffer swap failed: {erro}");
        }
    }
}

/// Onde o quadro fica dentro da janela.
///
/// `aspecto` é largura sobre altura da imagem; `None` significa que ninguém pediu proporção e
/// o quadro preenche tudo. `altura_nativa` é a altura em pixels do quadro do console, e serve
/// **só** à escala inteira — que é ampliação por múltiplo do quadro original, e por isso
/// precisa saber qual é o original. O retângulo sai centralizado: a borda que sobra é igual
/// dos dois lados.
fn enquadra(
    largura: u32,
    altura: u32,
    altura_nativa: u32,
    aspecto: Option<f32>,
    escala: Scaling,
) -> egui::epaint::ViewportInPixels {
    let (janela_l, janela_a) = (largura as f32, altura as f32);
    let cheia = || retangulo(0, 0, largura as i32, altura as i32, altura);
    let Some(aspecto) = aspecto.filter(|a| *a > 0.0) else {
        return cheia();
    };
    if escala == Scaling::Stretch {
        return cheia();
    }

    // O maior tamanho que cabe mantendo a proporção. É o `cabe`, e é também de onde a escala
    // inteira parte quando nem uma cópia do quadro original caberia.
    let (mut quadro_l, mut quadro_a) = match janela_l / janela_a > aspecto {
        // A janela é mais larga que a imagem: a altura manda, e sobra dos lados.
        true => (janela_a * aspecto, janela_a),
        false => (janela_l, janela_l / aspecto),
    };

    // O múltiplo inteiro nunca borra, e o preço é a moldura maior — quem escolhe `inteira` está
    // escolhendo exatamente isso. A conta é sobre o **quadro original**, não sobre o que coube:
    // partir do tamanho já ajustado daria sempre uma vez, e a opção não faria nada.
    if escala == Scaling::Integer && altura_nativa > 0 {
        let nativa_a = altura_nativa as f32;
        let nativa_l = nativa_a * aspecto;
        let vezes = (janela_l / nativa_l).min(janela_a / nativa_a).floor();
        // Abaixo de uma cópia inteira não há como descer: numa janela menor que o quadro do
        // console, `inteira` volta a ser `cabe`, que ao menos mostra a imagem toda.
        if vezes >= 1.0 {
            quadro_l = nativa_l * vezes;
            quadro_a = nativa_a * vezes;
        }
    }

    let esquerda = ((janela_l - quadro_l) / 2.0).round() as i32;
    let topo = ((janela_a - quadro_a) / 2.0).round() as i32;
    retangulo(esquerda, topo, quadro_l.round() as i32, quadro_a.round() as i32, altura)
}

/// O retângulo do jeito que o `glViewport` quer: a origem embaixo.
fn retangulo(esquerda: i32, topo: i32, largura: i32, altura: i32, janela_a: u32) -> egui::epaint::ViewportInPixels {
    egui::epaint::ViewportInPixels {
        left_px: esquerda,
        top_px: topo,
        from_bottom_px: janela_a as i32 - topo - altura,
        width_px: largura,
        height_px: altura,
    }
}

/// A configuração de GL a usar: a com mais amostras entre as que servem.
fn escolhe_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|a, b| match b.num_samples() > a.num_samples() {
            true => b,
            false => a,
        })
        .expect("the driver offered no GL configuration")
}

#[cfg(test)]
mod testes {
    use super::*;

    /// Numa janela larga, o 4:3 fica no meio com as bordas iguais dos dois lados.
    #[test]
    fn o_quatro_por_tres_fica_centralizado_na_janela_larga() {
        let vp = enquadra(1600, 900, 480, Some(4.0 / 3.0), Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (1200, 900));
        assert_eq!(vp.left_px, 200);
        assert_eq!(vp.top_px, 0);
        assert_eq!(vp.from_bottom_px, 0);
    }

    /// Esticar ignora a proporção, que é o que ela promete.
    #[test]
    fn esticar_preenche_a_janela() {
        let vp = enquadra(1600, 900, 480, Some(4.0 / 3.0), Scaling::Stretch);
        assert_eq!((vp.width_px, vp.height_px), (1600, 900));
        assert_eq!((vp.left_px, vp.top_px), (0, 0));
    }

    /// Sem proporção pedida, o quadro toma a janela inteira.
    #[test]
    fn sem_proporcao_o_quadro_toma_tudo() {
        let vp = enquadra(1000, 500, 480, None, Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (1000, 500));
    }

    /// A escala inteira amplia por múltiplo do quadro do console, e sobra borda por isso.
    #[test]
    fn a_escala_inteira_amplia_por_multiplo_do_quadro() {
        // 640x480 numa janela de 1600x900: cabem 1,40 na altura, então uma cópia dupla não
        // cabe e vale a simples.
        let vp = enquadra(1600, 900, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (640, 480));
        assert_eq!(vp.left_px, 480);

        // Numa de 1920x1080 cabem duas.
        let vp = enquadra(1920, 1080, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (1280, 960));
    }

    /// Janela menor que o quadro não tem múltiplo inteiro: aí `inteira` vira `cabe`, que ao
    /// menos mostra a imagem toda.
    #[test]
    fn inteira_numa_janela_pequena_volta_a_caber() {
        let vp = enquadra(320, 240, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (320, 240));
    }

    /// A origem do `glViewport` é embaixo: uma faixa em cima vira faixa embaixo na conta.
    #[test]
    fn a_origem_vira_de_cabeca_para_baixo() {
        let vp = enquadra(800, 900, 480, Some(4.0 / 3.0), Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (800, 600));
        assert_eq!(vp.top_px, 150);
        assert_eq!(vp.from_bottom_px, 150);
    }
}

// ---------------------------------------------------------------------------------------------
// O laço da janela.
// ---------------------------------------------------------------------------------------------

use std::path::PathBuf;
use std::process::ExitCode;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::console::{Console, Fim};

/// Abre a janela e gira o console nela até fecharem.
pub fn roda(console: Console, primeiro: PathBuf) -> ExitCode {
    let laco = match EventLoop::new() {
        Ok(laco) => laco,
        Err(erro) => {
            eprintln!("error: no event loop: {erro}");
            eprintln!("       on a headless server, use `[video] mode = none`.");
            return ExitCode::FAILURE;
        }
    };
    // Rodar sem parar: quem segura a volta é o vsync da troca de quadro, ou o instante de sono
    // quando o jogo está adiantado. Esperar evento deixaria o emulador parado entre teclas.
    laco.set_control_flow(ControlFlow::Poll);

    let mut aplicativo = Aplicativo {
        console,
        primeiro,
        janela: None,
        codigo: ExitCode::SUCCESS,
    };
    if let Err(erro) = laco.run_app(&mut aplicativo) {
        eprintln!("error: the event loop stopped: {erro}");
        return ExitCode::FAILURE;
    }
    aplicativo.codigo
}

struct Aplicativo {
    console: Console,
    primeiro: PathBuf,
    janela: Option<Janela>,
    codigo: ExitCode,
}

impl Aplicativo {
    fn morre(&mut self, laco: &winit::event_loop::ActiveEventLoop, erro: &str) {
        eprintln!("error: {erro}");
        self.codigo = ExitCode::FAILURE;
        laco.exit();
    }

    /// Põe na janela o quadro que a sessão tem agora.
    fn pinta(&mut self) {
        // As opções saem antes da sessão: ler as duas ao mesmo tempo seria pegar o `console`
        // emprestado de dois jeitos, e o compilador tem razão em recusar.
        let g = self.console.settings.graphics.clone();
        let (escala, suave, proporcao_4_3) = (g.scaling, g.smooth, g.keep_aspect);
        let Some(janela) = self.janela.as_mut() else {
            return;
        };
        let Some(sessao) = self.console.sessao_mut() else {
            return;
        };

        // A proporção "a da janela" acompanha o tamanho dela, e por isso é dita a cada quadro;
        // qualquer outra foi dita uma vez, na abertura.
        if g.proporcao == zeebx::ui::settings::Proporcao::Janela {
            let (largura, altura) = janela.tamanho();
            sessao.define_proporcao(Some(largura as f32 / altura.max(1) as f32));
        }

        let altura_nativa = sessao.screen().height();
        // Com o 3D na placa, o quadro que vai à tela **não** é o `screen()`: é a textura que o
        // rasterizador acabou de preencher, e é a única que pode ser mais larga que 4:3 — a
        // proporção larga abre o campo de visão na renderização, não na composição.
        let na_placa = g.gpu_rasterizer.then(|| sessao.quadro_na_placa()).flatten();
        match na_placa {
            Some(quadro) => janela.mostra_textura(quadro, altura_nativa, escala, proporcao_4_3, suave),
            None => {
                let tela = sessao.screen();
                let (largura, altura) = (tela.width() as i32, tela.height() as i32);
                let bytes = tela.to_rgb565_bytes();
                janela.mostra_quadro(&bytes, largura, altura, escala, proporcao_4_3, suave);
            }
        }
    }
}

impl ApplicationHandler for Aplicativo {
    /// A janela nasce aqui, e **só na primeira vez**.
    ///
    /// O winit chama isto de novo quando o sistema devolve o aplicativo à vida. Refazer a
    /// janela nessa hora trocaria o contexto de GL por baixo da sessão, que guarda texturas e
    /// programas criados no antigo — o primeiro desenho depois disso seria uma falha dentro do
    /// driver.
    fn resumed(&mut self, laco: &winit::event_loop::ActiveEventLoop) {
        if self.janela.is_some() {
            return;
        }
        let janela = match Janela::abre(laco, &self.console.opcoes) {
            Ok(janela) => janela,
            Err(erro) => return self.morre(laco, &erro),
        };
        self.console.usa_contexto(Some(janela.gl.clone()));
        self.janela = Some(janela);
        let primeiro = self.primeiro.clone();
        if let Err(erro) = self.console.abre(&primeiro) {
            return self.morre(laco, &format!("{}: {erro}", primeiro.display()));
        }
    }

    fn window_event(
        &mut self,
        laco: &winit::event_loop::ActiveEventLoop,
        _id: winit::window::WindowId,
        evento: WindowEvent,
    ) {
        match evento {
            WindowEvent::CloseRequested => laco.exit(),
            WindowEvent::Resized(tamanho) => {
                if let Some(janela) = &self.janela {
                    janela.redimensiona(tamanho.width, tamanho.height);
                }
            }
            // Fora do foco, o que estava apertado não está mais: o soltar da tecla acontece
            // noutra janela e nunca chegaria aqui.
            WindowEvent::Focused(false) => self.console.teclado.solta_tudo(),
            WindowEvent::KeyboardInput { event, .. } => {
                // O `Esc` fecha, como em qualquer emulador, e por isso não é mapeável: quem
                // rodou precisa de uma saída que não dependa do arquivo estar certo.
                if event.state.is_pressed()
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                {
                    laco.exit();
                    return;
                }
                self.console
                    .teclado
                    .evento(event.physical_key, event.state.is_pressed());
            }
            _ => {}
        }
    }

    /// Uma volta do console por volta do laço.
    fn about_to_wait(&mut self, laco: &winit::event_loop::ActiveEventLoop) {
        if self.janela.is_none() {
            return;
        }
        match self.console.passo() {
            Fim::Segue => {}
            Fim::Acabou(motivo) => {
                eprintln!("stopped: {motivo}");
                return laco.exit();
            }
            Fim::Erro(erro) => return self.morre(laco, &erro),
        }
        // Adiantado, o jogo não tem quadro novo a mostrar: dormir um instante devolve a máquina
        // em vez de girar o laço à toa. Com o vsync ligado quem segura é a troca de quadro, mas
        // ela não é garantida — há driver que a ignora.
        if self.console.adiantado() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            return;
        }
        self.pinta();
    }

    /// O contexto e o pincel são soltos aqui, com o laço ainda de pé.
    fn exiting(&mut self, _laco: &winit::event_loop::ActiveEventLoop) {
        self.console.fecha();
        if let Some(janela) = self.janela.take() {
            janela.solta();
        }
    }
}
