//! O quadro do console pintado pelo GL do host, dentro da janela do egui.
//!
//! O caminho normal sobe o quadro como textura do egui: para isso ele é convertido de RGB565
//! para ARGB e depois para RGBA, dois vetores por quadro. Aqui o mesmo quadro vai para a placa
//! **no formato em que ele já está** — `GL_UNSIGNED_SHORT_5_6_5` é um formato que o OpenGL
//! aceita direto —, e quem amplia para o tamanho da janela é a placa.
//!
//! **Isto não é o rasterizador na GPU.** O 3D continua sendo rasterizado em software; o que
//! muda é só a última etapa, a de pôr o quadro pronto na tela. Existe para provar que sabemos
//! desenhar com GL dentro do egui sem mexer no emulador — e é sobre este pedaço que um backend
//! de GPU seria construído depois.
//!
//! O contexto que o `eframe` cria é **core 3.3** (o glutin resolve "sem perfil, sem versão"
//! assim), então não há pipeline fixo: mesmo para um quadrado com textura é preciso shader.

use eframe::egui;
use eframe::glow::{self, HasContext};

/// O triângulo que cobre a tela inteira, gerado sem vetor de vértices.
///
/// Três vértices cobrem o retângulo [-1, 1] com sobra, e o recorte da placa descarta o resto.
/// Sai mais barato que um quadrado com dois triângulos, e — o que importa mais aqui — dispensa
/// buffer de vértices: o `gl_VertexID` é o único dado de entrada.
const VERTICE: &str = r#"
out vec2 uv;
void main() {
    vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
    // A origem da textura é em cima; a do OpenGL, embaixo. Inverter aqui evita inverter o
    // quadro inteiro na CPU.
    uv = vec2(p.x, 1.0 - p.y);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENTO: &str = r#"
in vec2 uv;
uniform sampler2D quadro;
out vec4 cor;
void main() {
    cor = texture(quadro, uv);
}
"#;

/// O programa e a textura que pintam o quadro.
pub struct Pintor {
    program: glow::Program,
    vao: glow::VertexArray,
    textura: glow::Texture,
    /// Tamanho da textura que já está na placa, para reaproveitá-la enquanto não muda.
    medida: (i32, i32),
    suave: bool,
}

impl Pintor {
    /// Monta o programa. Tenta GLSL 3.30 e cai para ES 3.00 — o `eframe` pede um contexto de
    /// OpenGL e só cai para GLES quando o primeiro falha, então os dois casos existem.
    pub fn novo(gl: &glow::Context) -> Result<Self, String> {
        let program = ["#version 330 core\n", "#version 300 es\nprecision mediump float;\n"]
            .into_iter()
            .find_map(|cabecalho| unsafe { compila(gl, cabecalho) }.ok())
            .ok_or("nenhuma versão de GLSL aceita")?;
        unsafe {
            let vao = gl.create_vertex_array()?;
            let textura = gl.create_texture()?;
            Ok(Self {
                program,
                vao,
                textura,
                medida: (0, 0),
                suave: false,
            })
        }
    }

    /// Sobe o quadro e o desenha no retângulo pedido.
    ///
    /// `bytes` é o quadro em RGB565, do jeito que a superfície do console o guarda.
    pub fn desenha(
        &mut self,
        gl: &glow::Context,
        bytes: &[u8],
        largura: i32,
        altura: i32,
        vp: &egui::epaint::ViewportInPixels,
        suave: bool,
    ) {
        if largura <= 0 || altura <= 0 || bytes.len() < (largura * altura * 2) as usize {
            return;
        }
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.textura));
            // Cada linha tem `largura * 2` bytes: o alinhamento padrão de quatro só valeria com
            // largura par, e um quadro de largura ímpar sairia enviesado.
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 2);
            // **A storage é alocada uma vez, e cada quadro só troca o conteúdo.** Chamar
            // `tex_image_2d` a cada quadro realoca a textura inteira na placa — é o tropeço que
            // o zeebulator documenta, e era o que estava aqui.
            if self.medida != (largura, altura) {
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGB as i32,
                    largura,
                    altura,
                    0,
                    glow::RGB,
                    glow::UNSIGNED_SHORT_5_6_5,
                    glow::PixelUnpackData::Slice(None),
                );
            }
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                largura,
                altura,
                glow::RGB,
                glow::UNSIGNED_SHORT_5_6_5,
                glow::PixelUnpackData::Slice(Some(bytes)),
            );
            if self.medida != (largura, altura) || self.suave != suave {
                self.medida = (largura, altura);
                self.suave = suave;
                // O pixel do console é grande e quadrado: ampliar sem interpolar é o padrão, e
                // suavizar é escolha de quem olha — a mesma regra do caminho do egui.
                let filtro = match suave {
                    true => glow::LINEAR,
                    false => glow::NEAREST,
                } as i32;
                for (nome, valor) in [
                    (glow::TEXTURE_MIN_FILTER, filtro),
                    (glow::TEXTURE_MAG_FILTER, filtro),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, nome, valor);
                }
            }
            if let Some(local) = gl.get_uniform_location(self.program, "quadro") {
                gl.uniform_1_i32(Some(&local), 0);
            }
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            // Devolver o estado que mexemos: o egui desenha o resto da interface depois de nós,
            // e um programa ou VAO deixado ligado aparece como interface sem textura.
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
    }

    /// Devolve à placa o que foi criado. O `eframe` chama isto no fim, pelo `on_exit`.
    pub fn solta(&self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.program);
            gl.delete_vertex_array(self.vao);
            gl.delete_texture(self.textura);
        }
    }
}

/// Compila e liga o par de shaders com o cabeçalho de versão dado.
unsafe fn compila(gl: &glow::Context, cabecalho: &str) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (tipo, fonte) in [
            (glow::VERTEX_SHADER, VERTICE),
            (glow::FRAGMENT_SHADER, FRAGMENTO),
        ] {
            let shader = gl.create_shader(tipo)?;
            gl.shader_source(shader, &format!("{cabecalho}{fonte}"));
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                let erro = gl.get_shader_info_log(shader);
                gl.delete_shader(shader);
                for shader in shaders {
                    gl.delete_shader(shader);
                }
                gl.delete_program(program);
                return Err(erro);
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(program) {
            let erro = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(erro);
        }
        Ok(program)
    }
}
