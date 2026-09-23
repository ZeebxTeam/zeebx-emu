//! A janela do Android como superfície de OpenGL ES, e o pincel do egui em cima dela.
//!
//! Sem o eframe, o contexto é nosso. O que o Android entrega é uma `ANativeWindow`; o resto é
//! EGL, e quem fala EGL aqui é o glutin — a mesma caixa que o desktop já usa para o contexto
//! fora de tela do rasterizador.
//!
//! **O contexto e a superfície têm vidas diferentes, e é isto que separa os dois tipos daqui.**
//! A janela do Android nasce e morre várias vezes: trocar de aplicativo, apagar a tela, girar o
//! aparelho. A superfície vai junto. O contexto, não — e não pode ir, porque o rasterizador da
//! placa guarda texturas, buffers e programas criados nele. Destruir o contexto ao trocar de
//! aplicativo deixaria a sessão com um monte de nomes de objetos que não existem mais, e o
//! primeiro desenho depois de voltar seria uma falha dentro do driver.
//!
//! Por isso a [`Placa`] é criada uma vez e vive até o fim, e a [`Tela`] é a superfície, feita e
//! desfeita com a janela.

use std::num::NonZeroU32;
use std::sync::Arc;

use android_activity::AndroidApp;
use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext,
    PossiblyCurrentGlContext, Version,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, WindowSurface};
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
};

/// O que sobrevive à janela: o display EGL, o contexto, o `glow` e o pincel do egui.
pub struct Placa {
    display: Display,
    config: Config,
    contexto: PossiblyCurrentContext,
    /// O mesmo `glow` que a sessão recebe quando o 3D roda na placa.
    pub gl: Arc<glow::Context>,
    pub pincel: egui_glow::Painter,
}

/// A superfície da janela de agora.
pub struct Tela {
    superficie: Surface<WindowSurface>,
    /// O tamanho da superfície em pixels. O egui trabalha em pontos; a conversão é a densidade.
    pub tamanho: [u32; 2],
}

impl Placa {
    /// Monta o contexto sobre a primeira janela que a atividade criar.
    ///
    /// Precisa de uma janela **nesta** chamada porque o EGL escolhe a configuração pelo formato
    /// da superfície e porque um contexto só fica corrente com uma superfície; daí em diante a
    /// janela pode ir e vir à vontade.
    pub fn nova(app: &AndroidApp) -> Result<(Self, Tela), String> {
        let janela = app.native_window().ok_or("a atividade não tem janela")?;
        let largura = NonZeroU32::new(janela.width() as u32).ok_or("janela sem largura")?;
        let altura = NonZeroU32::new(janela.height() as u32).ok_or("janela sem altura")?;

        let janela_crua =
            RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(janela.ptr().cast()));
        let display_cru = RawDisplayHandle::Android(AndroidDisplayHandle::new());

        // No Android só existe EGL: dizer isso explicitamente evita o glutin tentar GLX.
        let display = unsafe { Display::new(display_cru, DisplayApiPreference::Egl) }
            .map_err(|erro| format!("sem display EGL: {erro}"))?;

        let modelo = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .compatible_with_native_window(janela_crua)
            .build();
        // A melhor configuração é a com mais amostras que ainda case com a janela.
        let config = unsafe { display.find_configs(modelo) }
            .map_err(|erro| format!("sem configuração EGL: {erro}"))?
            .reduce(|a, b| if b.num_samples() > a.num_samples() { b } else { a })
            .ok_or("nenhuma configuração EGL serve para esta janela")?;

        // O manifesto pede GLES 3.0, e é o que o rasterizador do núcleo espera quando a placa
        // é ligada. Pedir explicitamente evita cair num contexto 2.0 por omissão.
        let atributos = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
            .build(Some(janela_crua));
        let contexto = unsafe { display.create_context(&config, &atributos) }
            .map_err(|erro| format!("sem contexto de GL: {erro}"))?;

        let superficie = cria_superficie(&display, &config, janela_crua, largura, altura)?;
        let contexto = contexto
            .make_current(&superficie)
            .map_err(|erro| format!("o contexto não ficou corrente: {erro}"))?;

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|nome| display.get_proc_address(nome))
        });
        let pincel = egui_glow::Painter::new(gl.clone(), "", None, false)
            .map_err(|erro| format!("o egui_glow não montou: {erro}"))?;

        log::info!("tela de {}x{}", largura.get(), altura.get());
        Ok((
            Self {
                display,
                config,
                contexto,
                gl,
                pincel,
            },
            Tela {
                superficie,
                tamanho: [largura.get(), altura.get()],
            },
        ))
    }

    /// Faz a superfície da janela que a atividade acabou de criar, com o contexto que já existe.
    pub fn refaz_a_tela(&self, app: &AndroidApp) -> Result<Tela, String> {
        let janela = app.native_window().ok_or("a atividade não tem janela")?;
        let largura = NonZeroU32::new(janela.width() as u32).ok_or("janela sem largura")?;
        let altura = NonZeroU32::new(janela.height() as u32).ok_or("janela sem altura")?;
        let janela_crua =
            RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(janela.ptr().cast()));

        let superficie =
            cria_superficie(&self.display, &self.config, janela_crua, largura, altura)?;
        self.contexto
            .make_current(&superficie)
            .map_err(|erro| format!("o contexto não ficou corrente: {erro}"))?;
        log::info!("tela refeita em {}x{}", largura.get(), altura.get());
        Ok(Tela {
            superficie,
            tamanho: [largura.get(), altura.get()],
        })
    }

    /// A janela mudou de tamanho — girou, ou a barra do sistema apareceu.
    pub fn redimensiona(&self, tela: &mut Tela, app: &AndroidApp) {
        let Some(janela) = app.native_window() else {
            return;
        };
        let (Some(largura), Some(altura)) = (
            NonZeroU32::new(janela.width() as u32),
            NonZeroU32::new(janela.height() as u32),
        ) else {
            return;
        };
        tela.superficie.resize(&self.contexto, largura, altura);
        tela.tamanho = [largura.get(), altura.get()];
    }

    /// Desenha o que o egui produziu e troca os buffers.
    pub fn pinta(
        &mut self,
        tela: &Tela,
        primitivas: &[egui::ClippedPrimitive],
        texturas: &egui::TexturesDelta,
        pontos_por_pixel: f32,
    ) {
        self.pincel.clear(tela.tamanho, [0.0, 0.0, 0.0, 1.0]);
        self.pincel
            .paint_and_update_textures(tela.tamanho, pontos_por_pixel, primitivas, texturas);
        if let Err(erro) = tela.superficie.swap_buffers(&self.contexto) {
            log::error!("a troca de buffers falhou: {erro}");
        }
    }

    /// Volta a ser o contexto corrente depois de a atividade ter sido retomada.
    pub fn retoma(&self, tela: &Tela) {
        if !self.contexto.is_current() {
            if let Err(erro) = self.contexto.make_current(&tela.superficie) {
                log::error!("o contexto não voltou a ser corrente: {erro}");
            }
        }
    }
}

fn cria_superficie(
    display: &Display,
    config: &Config,
    janela: RawWindowHandle,
    largura: NonZeroU32,
    altura: NonZeroU32,
) -> Result<Surface<WindowSurface>, String> {
    let atributos = SurfaceAttributesBuilder::<WindowSurface>::new().build(janela, largura, altura);
    unsafe { display.create_window_surface(config, &atributos) }
        .map_err(|erro| format!("sem superfície: {erro}"))
}
