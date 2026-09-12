//! Um contexto de OpenGL **fora de tela**, para o rasterizador na placa.
//!
//! O núcleo do emulador não tem janela. No caminho `run` da linha de comando não existe nenhuma,
//! e é justamente ali que a medição é feita — o `--profile` e a comparação das superfícies. Um
//! backend de GPU que só funcionasse com janela seria um backend que não se pode medir.
//!
//! Por isso o contexto é próprio e independente do que o `eframe` cria para a interface. Além de
//! dar um caminho só para os dois modos, ele evita acoplar o núcleo à thread de render e ao
//! estado de GL do egui — que desenha a interface no mesmo contexto e não espera encontrá-lo
//! mexido.
//!
//! A superfície é um pbuffer de 1x1 que nunca é desenhado: o destino real é um framebuffer
//! próprio, criado pelo backend no tamanho da superfície do console. O pbuffer existe só porque
//! o EGL exige uma superfície para tornar um contexto corrente.

use eframe::glow;
use glutin::config::{ConfigSurfaceTypes, ConfigTemplateBuilder};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, Version,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{PbufferSurface, Surface, SurfaceAttributesBuilder};
use raw_window_handle::{RawDisplayHandle, XlibDisplayHandle};
use std::num::NonZeroU32;

/// O contexto e o carregador de funções, vivos enquanto o backend existir.
pub struct Contexto {
    /// As funções de GL já resolvidas. É o que o backend usa.
    ///
    /// Compartilhável porque o caminho com janela **não** cria contexto: ele recebe o do eframe.
    /// Ver [`crate::video::gpu::GpuState::novo`].
    pub gl: std::sync::Arc<glow::Context>,
    /// O contexto corrente. Solto no fim, depois do `gl`.
    _contexto: PossiblyCurrentContext,
    /// A superfície mínima que o EGL exige para tornar o contexto corrente.
    _superficie: Surface<PbufferSurface>,
    /// O display fica por último de propósito: ele é quem criou os dois de cima.
    _display: Display,
}

impl Contexto {
    /// Abre um contexto fora de tela, ou diz por que não deu.
    ///
    /// Falhar aqui é um caso normal, não um erro do programa: num terminal sem EGL alcançável não
    /// há placa para usar. Quem chama trata o `Err` caindo para o rasterizador de software.
    pub fn novo() -> Result<Self, String> {
        // `display: None` é o `EGL_DEFAULT_DISPLAY`: pede ao EGL o display que ele considera
        // padrão, sem precisar de uma conexão de janela aberta por nós.
        let handle = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        let display = unsafe { Display::new(handle, DisplayApiPreference::Egl) }
            .map_err(|erro| format!("não abriu o display EGL: {erro}"))?;

        // Profundidade e stencil são exigências do console, não enfeite: o palco da Z-Wheel marca
        // o chão no stencil para desenhar o reflexo, e sem oito bits ali o reflexo se perde.
        let template = ConfigTemplateBuilder::new()
            .with_depth_size(24)
            .with_stencil_size(8)
            .with_surface_type(ConfigSurfaceTypes::PBUFFER)
            .build();
        let config = unsafe { display.find_configs(template) }
            .map_err(|erro| format!("não listou configurações: {erro}"))?
            .next()
            .ok_or("nenhuma configuração com profundidade de 24 e stencil de 8")?;

        // O shader é escrito em GLSL 3.30, então o pedido é por OpenGL 3.3 core. O GLES 3.0 é a
        // queda para as placas que só oferecem o perfil embarcado — o mesmo par de tentativas que
        // o pintor da interface já faz.
        let contexto = [
            ContextApi::OpenGl(Some(Version::new(3, 3))),
            ContextApi::Gles(Some(Version::new(3, 0))),
        ]
        .into_iter()
        .find_map(|api| {
            let attrs = ContextAttributesBuilder::new().with_context_api(api).build(None);
            unsafe { display.create_context(&config, &attrs) }.ok()
        })
        .ok_or("nem OpenGL 3.3 nem GLES 3.0 foram aceitos")?;

        let um = NonZeroU32::new(1).expect("1 não é zero");
        let attrs = SurfaceAttributesBuilder::<PbufferSurface>::new().build(um, um);
        let superficie = unsafe { display.create_pbuffer_surface(&config, &attrs) }
            .map_err(|erro| format!("não criou o pbuffer: {erro}"))?;
        let contexto = contexto
            .make_current(&superficie)
            .map_err(|erro| format!("não tornou o contexto corrente: {erro}"))?;

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|nome| display.get_proc_address(nome).cast())
        };
        Ok(Self {
            gl: std::sync::Arc::new(gl),
            _contexto: contexto,
            _superficie: superficie,
            _display: display,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre o contexto e pergunta quem respondeu.
    ///
    /// Numa máquina sem EGL alcançável — um terminal puro, um contêiner sem placa — não haver
    /// contexto **não é falha do programa**: o backend cai para o rasterizador de software. Então
    /// o teste relata o motivo e passa, em vez de exigir uma placa de quem roda a suíte.
    #[test]
    fn o_contexto_fora_de_tela_abre_ou_diz_por_que_nao() {
        match Contexto::novo() {
            Ok(contexto) => {
                use eframe::glow::HasContext;
                let versao = unsafe { contexto.gl.get_parameter_string(glow::VERSION) };
                let placa = unsafe { contexto.gl.get_parameter_string(glow::RENDERER) };
                println!("contexto aberto: {versao} — {placa}");
                assert!(!versao.is_empty(), "contexto sem versão de GL");
            }
            Err(motivo) => println!("sem contexto nesta máquina: {motivo}"),
        }
    }
}
