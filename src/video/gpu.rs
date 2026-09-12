//! O rasterizador na placa: o mesmo pipeline, com o preenchimento feito pelo OpenGL do host.
//!
//! **Isto não é uma reimplementação do pipeline fixo do GLES1.** Ele não seria necessário: o
//! [`GlState`] já faz toda a etapa de vértice na CPU — modelo-visão, projeção, matriz de textura
//! e iluminação por vértice —, e quando os triângulos chegam ao preenchimento os vértices estão
//! em espaço de recorte com cor e `uv` finais. É exatamente o que uma placa espera receber.
//!
//! Então este backend **contém** um `GlState`, usa-o para toda a contabilidade de estado e para a
//! etapa de vértice, e substitui só o que a medição apontou como caro: o preenchimento. Ter uma
//! fonte única para matrizes e luz é o que torna a comparação entre os dois honesta — se a luz
//! estiver errada, estará errada igual nos dois, e o que sobrar de diferença é do preenchimento.
//!
//! Um efeito colateral bem-vindo: as ~100 mil chamadas de `glEnable`/`glDisable` que a Z-Wheel e
//! o Crash fazem deixam de ser chamadas de GL uma a uma. O estado é aplicado **uma vez por draw**,
//! a partir do que foi anotado aqui.

use super::contexto::Contexto;
use super::gles;
use super::rasterizer::{GlState, Matrix, Rasterizador, Vertex};
use eframe::glow::{self, HasContext};
use std::collections::HashMap;

/// Quantos `f32` cada vértice ocupa no buffer: posição, cor e coordenada de textura.
const FLOATS_POR_VERTICE: usize = 4 + 4 + 2;

/// O que a placa precisa saber de uma textura do jogo, além dos pixels que já estão nela.
struct Textura {
    objeto: glow::Texture,
    largura: usize,
    altura: usize,
    /// O maior nível de mipmap já enviado. Um filtro que peça mipmap sem a cadeia completa
    /// desenha preto no OpenGL — então, quando só existe o nível zero, o filtro é rebaixado.
    maior_nivel: u32,
    crop: [i32; 4],
    filtro: u32,
    filtro_min: u32,
    wrap: [u32; 2],
}

/// O estado do preenchimento, anotado das chamadas e aplicado uma vez por draw.
#[derive(Clone)]
struct Estado {
    teste_profundidade: bool,
    mascara_profundidade: bool,
    func_profundidade: u32,
    mistura: bool,
    mistura_src: u32,
    mistura_dst: u32,
    teste_alfa: bool,
    func_alfa: u32,
    ref_alfa: f32,
    mascara_cor: [bool; 4],
    descarte: bool,
    modo_descarte: u32,
    face_frontal: u32,
    teste_stencil: bool,
    func_stencil: u32,
    ref_stencil: i32,
    mascara_valor_stencil: u32,
    mascara_escrita_stencil: u32,
    op_stencil: [u32; 3],
    env_textura: u32,
    textura_ligada: u32,
    texturando: bool,
    viewport: (i32, i32, i32, i32),
    limpa_cor: [f32; 4],
    limpa_profundidade: f32,
    limpa_stencil: i32,
}

impl Default for Estado {
    fn default() -> Self {
        Self {
            teste_profundidade: false,
            mascara_profundidade: true,
            func_profundidade: gles::GL_LESS,
            mistura: false,
            mistura_src: gles::GL_ONE,
            mistura_dst: gles::GL_ZERO,
            teste_alfa: false,
            func_alfa: gles::GL_ALWAYS,
            ref_alfa: 0.0,
            mascara_cor: [true; 4],
            descarte: false,
            modo_descarte: gles::GL_BACK,
            face_frontal: gles::GL_CCW,
            teste_stencil: false,
            func_stencil: gles::GL_ALWAYS,
            ref_stencil: 0,
            mascara_valor_stencil: u32::MAX,
            mascara_escrita_stencil: u32::MAX,
            op_stencil: [gles::GL_KEEP; 3],
            env_textura: gles::GL_MODULATE,
            textura_ligada: 0,
            texturando: false,
            viewport: (0, 0, 0, 0),
            limpa_cor: [0.0, 0.0, 0.0, 1.0],
            limpa_profundidade: 1.0,
            limpa_stencil: 0,
        }
    }
}

pub struct GpuState {
    /// A contabilidade de estado e a etapa de vértice, compartilhadas com o software.
    estado: GlState,
    /// O contexto que **nós** abrimos, quando não havia nenhum.
    ///
    /// Nunca é lido: existe para não ser solto enquanto o backend vive. Soltá-lo destruiria o
    /// contexto de onde vêm as funções de GL que o `gl` acabou de guardar.
    _proprio: Option<Contexto>,
    /// As funções de GL: emprestadas da janela, ou do contexto próprio.
    gl: std::sync::Arc<glow::Context>,
    /// Se o contexto é de outro. Nesse caso o estado tem que ser devolvido depois de cada uso —
    /// ver [`GpuState::devolve_o_contexto`].
    emprestado: bool,
    fill: Estado,
    /// O destino: uma textura de cor mais profundidade e stencil juntos.
    quadro: Option<Destino>,
    programa: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    /// A textura de apoio do [`GpuState::import_rgb565_changes`].
    ponte: glow::Texture,
    texturas: HashMap<u32, Textura>,
    /// Buffers reaproveitados entre chamadas, para não pedir memória por quadro.
    vertices: Vec<f32>,
    pixels: Vec<u8>,
    /// Se alguma coisa foi desenhada desde a última conversão do quadro.
    sujo: bool,
}

struct Destino {
    fbo: glow::Framebuffer,
    cor: glow::Texture,
    profundidade: glow::Renderbuffer,
    medida: (usize, usize),
}

impl GpuState {
    /// Monta o programa sobre um contexto, ou diz por que não deu.
    ///
    /// `emprestado` é o contexto da janela, quando há uma. **Receber em vez de criar não é
    /// economia, é correção:** o núcleo roda na mesma thread da interface, e um contexto nosso
    /// tornado corrente ali desliga o do eframe — o egui para de pintar e a janela congela,
    /// enquanto o áudio, que é outra thread, segue tocando.
    ///
    /// Sem janela — o `run` da linha de comando, onde a medição é feita — não há o que emprestar
    /// e abrimos o pbuffer.
    pub fn novo(
        largura: usize,
        altura: usize,
        emprestado: Option<std::sync::Arc<glow::Context>>,
    ) -> Result<Self, String> {
        let (proprio, gl, emprestado) = match emprestado {
            Some(gl) => (None, gl, true),
            None => {
                let proprio = Contexto::novo()?;
                let gl = proprio.gl.clone();
                (Some(proprio), gl, false)
            }
        };
        let (programa, vao, vbo, ponte) = unsafe {
            let programa = compila(&gl)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ponte = gl.create_texture()?;
            (programa, vao, vbo, ponte)
        };
        Ok(Self {
            estado: GlState::new(largura, altura),
            _proprio: proprio,
            gl,
            emprestado,
            // **A viewport nasce com a tela inteira**, que é o que o OpenGL especifica como
            // padrão e o que o `GlState::new` faz. Nascer em zero era o que apagava toda a
            // geometria da Z-Wheel: ela nunca chama `glViewport` — zero vezes em treze segundos
            // — e ficava com um `glViewport(0, 0, 0, 0)`, de onde nada sai. Só o `import`
            // aparecia, porque ele põe a sua própria.
            fill: Estado {
                viewport: (0, 0, largura as i32, altura as i32),
                ..Estado::default()
            },
            quadro: None,
            programa,
            vao,
            vbo,
            ponte,
            texturas: HashMap::new(),
            vertices: Vec::new(),
            pixels: Vec::new(),
            sujo: true,
        })
    }

    /// Garante que o destino existe no tamanho do quadro e o deixa ligado.
    fn destino(&mut self) {
        let medida = self.estado.frame_size();
        if self.quadro.as_ref().is_some_and(|d| d.medida == medida) {
            let fbo = self.quadro.as_ref().map(|d| d.fbo);
            unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, fbo) };
            return;
        }
        let gl = &self.gl;
        unsafe {
            if let Some(antigo) = self.quadro.take() {
                gl.delete_framebuffer(antigo.fbo);
                gl.delete_texture(antigo.cor);
                gl.delete_renderbuffer(antigo.profundidade);
            }
            let (largura, altura) = (medida.0 as i32, medida.1 as i32);
            let cor = gl.create_texture().expect("textura de cor");
            gl.bind_texture(glow::TEXTURE_2D, Some(cor));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                largura,
                altura,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            // Profundidade e stencil no mesmo anexo: é a combinação que o OpenGL garante.
            let profundidade = gl.create_renderbuffer().expect("buffer de profundidade");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(profundidade));
            gl.renderbuffer_storage(
                glow::RENDERBUFFER,
                glow::DEPTH24_STENCIL8,
                largura,
                altura,
            );
            let fbo = gl.create_framebuffer().expect("framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(cor),
                0,
            );
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(profundidade),
            );
            // Um framebuffer novo tem conteúdo indefinido, enquanto os vetores do rasterizador
            // de software nascem em preto opaco, profundidade 1 e stencil 0. Igualar aqui evita
            // que o primeiro quadro dependa do que a placa deixou na memória.
            gl.viewport(0, 0, largura, altura);
            gl.disable(glow::SCISSOR_TEST);
            gl.color_mask(true, true, true, true);
            gl.depth_mask(true);
            gl.stencil_mask(u32::MAX);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear_stencil(0);
            gl.clear(
                glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT,
            );
            self.quadro = Some(Destino {
                fbo,
                cor,
                profundidade,
                medida,
            });
        }
    }

    /// Põe na placa o estado anotado. Chamado uma vez por draw.
    fn aplica(&mut self) {
        let gl = &self.gl;
        let e = &self.fill;
        unsafe {
            let (x, y, w, h) = e.viewport;
            gl.viewport(x, y, w.max(0), h.max(0));
            // **O rasterizador de software só recorta no plano próximo.** O OpenGL recorta nos
            // seis planos do frustum, e o plano distante fazia superfícies inteiras desaparecerem
            // — na Z-Wheel era uma faixa do fundo, entre a linha do horizonte e o chão. Preso em
            // vez de recortado, o comportamento volta a ser o do software.
            //
            // É core desde o OpenGL 3.2, que é o perfil pedido. Na queda para GLES 3.0 ele não
            // existe e a chamada não tem efeito: ali o plano distante volta a recortar.
            gl.enable(glow::DEPTH_CLAMP);
            liga(gl, glow::DEPTH_TEST, e.teste_profundidade);
            gl.depth_func(e.func_profundidade);
            gl.depth_mask(e.mascara_profundidade);
            liga(gl, glow::BLEND, e.mistura);
            gl.blend_func(e.mistura_src, e.mistura_dst);
            let [r, g, b, a] = e.mascara_cor;
            gl.color_mask(r, g, b, a);
            liga(gl, glow::CULL_FACE, e.descarte);
            gl.cull_face(e.modo_descarte);
            // A linha 0 do framebuffer é tratada como o topo da imagem, e o Y é virado no shader
            // de vértice. Isso inverte a orientação vista pelo descarte de face, então a face
            // frontal é trocada aqui para compensar — sem isto o descarte come o lado errado.
            gl.front_face(match e.face_frontal {
                gles::GL_CCW => glow::CW,
                _ => glow::CCW,
            });
            liga(gl, glow::STENCIL_TEST, e.teste_stencil);
            gl.stencil_func(e.func_stencil, e.ref_stencil, e.mascara_valor_stencil);
            gl.stencil_mask(e.mascara_escrita_stencil);
            gl.stencil_op(e.op_stencil[0], e.op_stencil[1], e.op_stencil[2]);
        }
    }

    /// Manda o lote de vértices já transformados para a placa.
    ///
    /// `textura` existe porque a ponte do [`GpuState::import_rgb565_changes`] não pertence ao
    /// jogo e portanto não está no mapa de texturas dele.
    fn submete_com(&mut self, modo: u32, virar: f32, textura: Option<glow::Texture>) {
        let quantos = self.vertices.len() / FLOATS_POR_VERTICE;
        if quantos == 0 {
            return;
        }
        self.destino();
        self.aplica();
        let textura = textura.or_else(|| {
            self.fill
                .texturando
                .then(|| self.texturas.get(&self.fill.textura_ligada))
                .flatten()
                .map(|t| t.objeto)
        });
        let gl = &self.gl;
        unsafe {
            gl.use_program(Some(self.programa));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytes_de_f32(&self.vertices),
                glow::DYNAMIC_DRAW,
            );
            let passo = (FLOATS_POR_VERTICE * 4) as i32;
            for (indice, tamanho, deslocamento) in [(0u32, 4i32, 0i32), (1, 4, 16), (2, 2, 32)] {
                gl.enable_vertex_attrib_array(indice);
                gl.vertex_attrib_pointer_f32(
                    indice,
                    tamanho,
                    glow::FLOAT,
                    false,
                    passo,
                    deslocamento,
                );
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, textura);
            uniforme_i32(gl, self.programa, "amostra", 0);
            uniforme_i32(gl, self.programa, "texturando", i32::from(textura.is_some()));
            uniforme_i32(gl, self.programa, "env", codigo_env(self.fill.env_textura));
            uniforme_i32(
                gl,
                self.programa,
                "func_alfa",
                match self.fill.teste_alfa {
                    true => codigo_alfa(self.fill.func_alfa),
                    false => 7,
                },
            );
            uniforme_f32(gl, self.programa, "ref_alfa", self.fill.ref_alfa);
            uniforme_f32(gl, self.programa, "virar", virar);
            gl.draw_arrays(modo, 0, quantos as i32);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }
        self.devolve_o_contexto();
        self.sujo = true;
    }

    /// Empilha um vértice no buffer de envio.
    fn poe(&mut self, v: &Vertex) {
        self.vertices.extend_from_slice(&v.position);
        self.vertices.extend_from_slice(&v.color);
        self.vertices.extend_from_slice(&v.uv);
    }

    /// Lê o quadro da placa para `self.pixels`, em RGBA, com a linha 0 no topo.
    fn le_quadro(&mut self, largura: usize, altura: usize) {
        self.destino();
        self.pixels.clear();
        self.pixels.resize(largura * altura * 4, 0);
        unsafe {
            self.gl.read_pixels(
                0,
                0,
                largura as i32,
                altura as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut self.pixels)),
            );
        }
        self.devolve_o_contexto();
    }

    /// Devolve o estado que o egui pressupõe, quando o contexto é de outro.
    ///
    /// No caminho com janela o egui pinta na **mesma thread e no mesmo contexto**, logo depois de
    /// nós, e ele não reconfigura tudo o que usa: deixar o nosso framebuffer ligado, ou o teste
    /// de profundidade aceso com uma profundidade que não é a dele, faz a interface desaparecer.
    ///
    /// Com contexto próprio isto não custa nada porque não roda: ninguém mais o usa.
    fn devolve_o_contexto(&self) {
        if !self.emprestado {
            return;
        }
        let gl = &self.gl;
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::DEPTH_CLAMP);
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
            gl.stencil_mask(u32::MAX);
            gl.color_mask(true, true, true, true);
        }
    }

    /// Reaplica os parâmetros de uma textura, rebaixando o filtro quando falta a cadeia.
    fn parametros(&self, t: &Textura) {
        let gl = &self.gl;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(t.objeto));
            let tem_cadeia = t.maior_nivel > 0;
            let min = match (t.filtro_min, tem_cadeia) {
                (f, true) => f,
                // Sem níveis auxiliares, um filtro de mipmap desenha preto no OpenGL. O
                // rasterizador de software cai para o nível zero nesse caso; aqui a queda é
                // escolher o filtro equivalente sem mipmap.
                (gles::GL_NEAREST_MIPMAP_NEAREST | gles::GL_NEAREST_MIPMAP_LINEAR, false) => {
                    gles::GL_NEAREST
                }
                (gles::GL_LINEAR_MIPMAP_NEAREST | gles::GL_LINEAR_MIPMAP_LINEAR, false) => {
                    gles::GL_LINEAR
                }
                (f, false) => f,
            };
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, min as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, t.filtro as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, t.maior_nivel as i32);
            for (eixo, modo) in [
                (glow::TEXTURE_WRAP_S, t.wrap[0]),
                (glow::TEXTURE_WRAP_T, t.wrap[1]),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, eixo, modo as i32);
            }
        }
    }
}

impl Drop for GpuState {
    fn drop(&mut self) {
        let gl = &self.gl;
        unsafe {
            gl.delete_program(self.programa);
            gl.delete_vertex_array(self.vao);
            gl.delete_buffer(self.vbo);
            gl.delete_texture(self.ponte);
            for t in self.texturas.values() {
                gl.delete_texture(t.objeto);
            }
            if let Some(d) = self.quadro.take() {
                gl.delete_framebuffer(d.fbo);
                gl.delete_texture(d.cor);
                gl.delete_renderbuffer(d.profundidade);
            }
        }
    }
}

fn liga(gl: &glow::Context, capacidade: u32, ligado: bool) {
    unsafe {
        match ligado {
            true => gl.enable(capacidade),
            false => gl.disable(capacidade),
        }
    }
}

fn uniforme_i32(gl: &glow::Context, programa: glow::Program, nome: &str, valor: i32) {
    unsafe {
        if let Some(onde) = gl.get_uniform_location(programa, nome) {
            gl.uniform_1_i32(Some(&onde), valor);
        }
    }
}

fn uniforme_f32(gl: &glow::Context, programa: glow::Program, nome: &str, valor: f32) {
    unsafe {
        if let Some(onde) = gl.get_uniform_location(programa, nome) {
            gl.uniform_1_f32(Some(&onde), valor);
        }
    }
}

/// Os bytes de um vetor de `f32`, para o `buffer_data`.
fn bytes_de_f32(dados: &[f32]) -> &[u8] {
    // Um `f32` não tem invariante de bits, e o alinhamento de quatro serve para um de um.
    unsafe { std::slice::from_raw_parts(dados.as_ptr().cast(), std::mem::size_of_val(dados)) }
}

/// O código que o shader usa para cada modo de `glTexEnv`. Ver `combine` no rasterizador.
fn codigo_env(modo: u32) -> i32 {
    match modo {
        gles::GL_REPLACE => 0,
        gles::GL_DECAL => 1,
        gles::GL_ADD => 2,
        _ => 3,
    }
}

/// O código que o shader usa para cada função de teste de alfa.
fn codigo_alfa(func: u32) -> i32 {
    match func {
        gles::GL_NEVER => 0,
        gles::GL_LESS => 1,
        gles::GL_EQUAL => 2,
        gles::GL_LEQUAL => 3,
        gles::GL_GREATER => 4,
        gles::GL_NOTEQUAL => 5,
        gles::GL_GEQUAL => 6,
        _ => 7,
    }
}

/// O shader de vértice é **passagem direta**: as posições já chegam em espaço de recorte, porque
/// a etapa de vértice acontece na CPU, compartilhada com o rasterizador de software.
///
/// O `virar` inverte o Y. Ele existe porque a linha 0 do framebuffer é tratada como o topo da
/// imagem — assim o `glReadPixels` devolve as linhas na mesma ordem que a superfície de software
/// usa, e nenhum quadro precisa ser espelhado na CPU. A ponte do `import` passa `+1` porque já
/// entrega coordenadas prontas.
///
/// Não há caminho de sombreamento plano porque o rasterizador de software não tem: ele guarda o
/// `glShadeModel` e nunca o lê. Ver [`GpuState::set_shade_model`].
const VERTICE: &str = r#"
layout(location = 0) in vec4 pos;
layout(location = 1) in vec4 cor;
layout(location = 2) in vec2 uv;
uniform float virar;
out vec4 vcor;
out vec2 vuv;
void main() {
    vcor = cor;
    vuv = uv;
    gl_Position = vec4(pos.x, pos.y * virar, pos.z, pos.w);
}
"#;

/// O fragmento faz o que sobrou do pipeline fixo: combinar a textura e testar o alfa.
///
/// A combinação segue `combine` do rasterizador de software, caso a caso, para que os dois
/// backends respondam a `glTexEnv` do mesmo jeito.
const FRAGMENTO: &str = r#"
in vec4 vcor;
in vec2 vuv;
uniform sampler2D amostra;
uniform int texturando;
uniform int env;
uniform int func_alfa;
uniform float ref_alfa;
out vec4 saida;

vec4 combina(vec4 fonte, vec4 texel) {
    if (env == 0) { return texel; }
    if (env == 1) {
        return vec4(fonte.rgb * (1.0 - texel.a) + texel.rgb * texel.a, fonte.a);
    }
    if (env == 2) {
        return vec4(min(fonte.rgb + texel.rgb, vec3(1.0)), fonte.a * texel.a);
    }
    return fonte * texel;
}

void main() {
    vec4 cor = vcor;
    if (texturando == 1) {
        cor = combina(cor, texture(amostra, vuv));
    }
    bool passa;
    if      (func_alfa == 0) { passa = false; }
    else if (func_alfa == 1) { passa = cor.a <  ref_alfa; }
    else if (func_alfa == 2) { passa = cor.a == ref_alfa; }
    else if (func_alfa == 3) { passa = cor.a <= ref_alfa; }
    else if (func_alfa == 4) { passa = cor.a >  ref_alfa; }
    else if (func_alfa == 5) { passa = cor.a != ref_alfa; }
    else if (func_alfa == 6) { passa = cor.a >= ref_alfa; }
    else                     { passa = true; }
    if (!passa) { discard; }
    saida = cor;
}
"#;

/// Compila o par de shaders, tentando GLSL 3.30 e caindo para ES 3.00.
unsafe fn compila(gl: &glow::Context) -> Result<glow::Program, String> {
    ["#version 330 core\n", "#version 300 es\nprecision highp float;\n"]
        .into_iter()
        .find_map(|cabecalho| unsafe { liga_programa(gl, cabecalho) }.ok())
        .ok_or_else(|| "nenhuma versão de GLSL aceita".to_string())
}

unsafe fn liga_programa(gl: &glow::Context, cabecalho: &str) -> Result<glow::Program, String> {
    unsafe {
        let programa = gl.create_program()?;
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
                gl.delete_program(programa);
                return Err(erro);
            }
            gl.attach_shader(programa, shader);
            shaders.push(shader);
        }
        gl.link_program(programa);
        for shader in shaders {
            gl.detach_shader(programa, shader);
            gl.delete_shader(shader);
        }
        if !gl.get_program_link_status(programa) {
            let erro = gl.get_program_info_log(programa);
            gl.delete_program(programa);
            return Err(erro);
        }
        Ok(programa)
    }
}

/// Um pixel RGB565 expandido para RGB888. Mesma expansão do rasterizador de software.
fn expande565(bytes: &[u8], offset: usize) -> [u8; 3] {
    let pixel = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let r = ((pixel >> 11) & 31) as u8;
    let g = ((pixel >> 5) & 63) as u8;
    let b = (pixel & 31) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

impl Rasterizador for GpuState {
    fn set_matrix_mode(&mut self, mode: u32) {
        self.estado.set_matrix_mode(mode);
    }
    fn load_identity(&mut self) {
        self.estado.load_identity();
    }
    fn load_matrix(&mut self, m: Matrix) {
        self.estado.load_matrix(m);
    }
    fn mult_matrix(&mut self, m: Matrix) {
        self.estado.mult_matrix(m);
    }
    fn push_matrix(&mut self) {
        self.estado.push_matrix();
    }
    fn pop_matrix(&mut self) {
        self.estado.pop_matrix();
    }

    fn set_viewport(&mut self, x: i32, y: i32, width: i32, height: i32) {
        self.estado.set_viewport(x, y, width, height);
        self.fill.viewport = (x, y, width, height);
    }
    fn set_surface(&mut self, width: usize, height: usize) {
        self.estado.set_surface(width, height);
    }
    fn surface(&self) -> (usize, usize) {
        self.estado.surface()
    }
    fn frame_size(&self) -> (usize, usize) {
        self.estado.frame_size()
    }

    fn set_clear_color(&mut self, color: [f32; 4]) {
        self.estado.set_clear_color(color);
        self.fill.limpa_cor = color;
    }
    fn set_clear_depth(&mut self, depth: f32) {
        self.estado.set_clear_depth(depth);
        self.fill.limpa_profundidade = depth;
    }
    fn set_clear_stencil(&mut self, valor: i32) {
        self.estado.set_clear_stencil(valor);
        self.fill.limpa_stencil = valor;
    }
    fn set_color(&mut self, color: [f32; 4]) {
        self.estado.set_color(color);
    }
    fn current_color(&self) -> [f32; 4] {
        self.estado.current_color()
    }

    fn clear(&mut self, mask: u32) {
        self.destino();
        // **O `clear` do rasterizador de software ignora as máscaras**: ele preenche os vetores
        // direto. O `glClear` respeita `glDepthMask`, `glStencilMask` e `glColorMask`, então elas
        // são abertas aqui — senão uma limpeza pedida com máscara fechada simplesmente não
        // aconteceria, e o quadro seguinte desenharia sobre o anterior.
        let gl = &self.gl;
        let mut bits = 0;
        unsafe {
            if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
                let [r, g, b, a] = self.fill.limpa_cor;
                gl.color_mask(true, true, true, true);
                gl.clear_color(r, g, b, a);
                bits |= glow::COLOR_BUFFER_BIT;
            }
            if mask & gles::GL_DEPTH_BUFFER_BIT != 0 {
                gl.depth_mask(true);
                gl.clear_depth_f32(self.fill.limpa_profundidade);
                bits |= glow::DEPTH_BUFFER_BIT;
            }
            if mask & gles::GL_STENCIL_BUFFER_BIT != 0 {
                gl.stencil_mask(u32::MAX);
                gl.clear_stencil(self.fill.limpa_stencil);
                bits |= glow::STENCIL_BUFFER_BIT;
            }
            if bits != 0 {
                // O recorte do `glScissor` fica de fora: o rasterizador de software não o tem, e
                // divergir aqui tornaria a comparação entre os dois inútil.
                gl.disable(glow::SCISSOR_TEST);
                gl.clear(bits);
            }
        }
        self.devolve_o_contexto();
        if mask & gles::GL_COLOR_BUFFER_BIT != 0 {
            self.sujo = true;
        }
    }

    fn set_capability(&mut self, capability: u32, on: bool) {
        self.estado.set_capability(capability, on);
        match capability {
            gles::GL_DEPTH_TEST => self.fill.teste_profundidade = on,
            gles::GL_BLEND => self.fill.mistura = on,
            gles::GL_ALPHA_TEST => self.fill.teste_alfa = on,
            gles::GL_CULL_FACE => self.fill.descarte = on,
            gles::GL_STENCIL_TEST => self.fill.teste_stencil = on,
            gles::GL_TEXTURE_2D => self.fill.texturando = on,
            _ => {}
        }
    }
    fn set_shade_model(&mut self, mode: u32) {
        // **Os dois backends ignoram o `glShadeModel`**, e de propósito: o rasterizador de
        // software guarda o modo e nunca o lê — sempre interpola. É uma divergência do GLES1 que
        // eles compartilham, e fazer a placa honrar o modo plano aqui só tornaria a comparação
        // entre os dois ruidosa sem corrigir nada que se veja.
        self.estado.set_shade_model(mode);
    }
    fn set_light(&mut self, index: usize, pname: u32, valores: [f32; 4]) {
        self.estado.set_light(index, pname, valores);
    }
    fn set_material(&mut self, pname: u32, valores: [f32; 4]) {
        self.estado.set_material(pname, valores);
    }
    fn set_light_model(&mut self, pname: u32, valores: [f32; 4]) {
        self.estado.set_light_model(pname, valores);
    }

    fn set_blend_func(&mut self, src: u32, dst: u32) {
        self.estado.set_blend_func(src, dst);
        self.fill.mistura_src = src;
        self.fill.mistura_dst = dst;
    }
    fn set_alpha_func(&mut self, func: u32, reference: f32) {
        self.estado.set_alpha_func(func, reference);
        self.fill.func_alfa = func;
        self.fill.ref_alfa = reference;
    }
    fn set_depth_func(&mut self, func: u32) {
        self.estado.set_depth_func(func);
        self.fill.func_profundidade = func;
    }
    fn set_depth_mask(&mut self, on: bool) {
        self.estado.set_depth_mask(on);
        self.fill.mascara_profundidade = on;
    }
    fn set_color_mask(&mut self, mask: [bool; 4]) {
        self.estado.set_color_mask(mask);
        self.fill.mascara_cor = mask;
    }
    fn set_cull_face(&mut self, mode: u32) {
        self.estado.set_cull_face(mode);
        self.fill.modo_descarte = mode;
    }
    fn set_front_face(&mut self, face: u32) {
        self.estado.set_front_face(face);
        self.fill.face_frontal = face;
    }
    fn set_stencil_func(&mut self, func: u32, referencia: i32, mask: u32) {
        self.estado.set_stencil_func(func, referencia, mask);
        self.fill.func_stencil = func;
        self.fill.ref_stencil = referencia;
        self.fill.mascara_valor_stencil = mask;
    }
    fn set_stencil_op(&mut self, falha: u32, falha_z: u32, passa: u32) {
        self.estado.set_stencil_op(falha, falha_z, passa);
        self.fill.op_stencil = [falha, falha_z, passa];
    }
    fn set_stencil_mask(&mut self, mask: u32) {
        self.estado.set_stencil_mask(mask);
        self.fill.mascara_escrita_stencil = mask;
    }

    fn set_active_texture(&mut self, unit: u32) {
        self.estado.set_active_texture(unit);
    }
    fn set_client_active_texture(&mut self, unit: u32) {
        self.estado.set_client_active_texture(unit);
    }
    fn base_client_unit(&self) -> bool {
        self.estado.base_client_unit()
    }
    fn bind_texture(&mut self, name: u32) {
        self.estado.bind_texture(name);
        self.fill.textura_ligada = name;
    }
    fn bound_texture(&self) -> u32 {
        self.estado.bound_texture()
    }
    fn set_texture_env(&mut self, mode: u32) {
        self.estado.set_texture_env(mode);
        self.fill.env_textura = mode;
    }
    fn set_texture_parameter(&mut self, name: u32, value: u32) {
        self.estado.set_texture_parameter(name, value);
        let ligada = self.fill.textura_ligada;
        if let Some(t) = self.texturas.get_mut(&ligada) {
            match name {
                gles::GL_TEXTURE_MIN_FILTER => t.filtro_min = value,
                gles::GL_TEXTURE_MAG_FILTER => t.filtro = value,
                gles::GL_TEXTURE_WRAP_S => t.wrap[0] = value,
                gles::GL_TEXTURE_WRAP_T => t.wrap[1] = value,
                _ => {}
            }
        }
        if let Some(t) = self.texturas.get(&ligada) {
            self.parametros(t);
        }
    }
    fn set_texture_crop(&mut self, crop: [i32; 4]) {
        self.estado.set_texture_crop(crop);
        let ligada = self.fill.textura_ligada;
        if let Some(t) = self.texturas.get_mut(&ligada) {
            t.crop = crop;
        }
    }
    fn delete_texture(&mut self, name: u32) {
        self.estado.delete_texture(name);
        if let Some(t) = self.texturas.remove(&name) {
            unsafe { self.gl.delete_texture(t.objeto) };
        }
    }

    fn upload_level(
        &mut self,
        name: u32,
        level: u32,
        width: usize,
        height: usize,
        pixels: Vec<[u8; 4]>,
    ) {
        let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
        let gl = &self.gl;
        let objeto = match self.texturas.get(&name) {
            Some(t) => t.objeto,
            None => {
                let objeto = match unsafe { gl.create_texture() } {
                    Ok(objeto) => objeto,
                    Err(_) => return,
                };
                self.texturas.insert(
                    name,
                    Textura {
                        objeto,
                        largura: width,
                        altura: height,
                        maior_nivel: 0,
                        crop: [0; 4],
                        filtro: gles::GL_LINEAR,
                        filtro_min: gles::GL_LINEAR,
                        wrap: [gles::GL_REPEAT; 2],
                    },
                );
                objeto
            }
        };
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(objeto));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                level as i32,
                glow::RGBA8 as i32,
                width as i32,
                height as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&bytes)),
            );
        }
        if let Some(t) = self.texturas.get_mut(&name) {
            if level == 0 {
                t.largura = width;
                t.altura = height;
            }
            t.maior_nivel = t.maior_nivel.max(level);
        }
        if let Some(t) = self.texturas.get(&name) {
            self.parametros(t);
        }
        self.estado.upload_level(name, level, width, height, pixels);
    }

    fn sub_image(
        &mut self,
        name: u32,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        pixels: &[[u8; 4]],
    ) -> Result<(), Option<(u32, u32)>> {
        let resultado = self.estado.sub_image(name, x, y, width, height, pixels);
        if resultado.is_ok() {
            if let Some(t) = self.texturas.get(&name) {
                let bytes: Vec<u8> = pixels.iter().flatten().copied().collect();
                unsafe {
                    let gl = &self.gl;
                    gl.bind_texture(glow::TEXTURE_2D, Some(t.objeto));
                    gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        x as i32,
                        y as i32,
                        width as i32,
                        height as i32,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&bytes)),
                    );
                }
            }
        }
        resultado
    }

    fn draw(&mut self, mode: u32, vertices: &[Vertex]) {
        // Pontos e linhas não aparecem nos jogos do console, e o rasterizador de software também
        // os deixa sem tratamento. Desenhá-los aqui divergiria dele sem ganho nenhum.
        let modo = match mode {
            gles::GL_TRIANGLES => glow::TRIANGLES,
            gles::GL_TRIANGLE_STRIP => glow::TRIANGLE_STRIP,
            gles::GL_TRIANGLE_FAN => glow::TRIANGLE_FAN,
            _ => return,
        };
        self.estado.etapa_de_vertice(vertices);
        // O buffer sai do `self` antes do laço: assim `transformados()` empresta o estado só de
        // leitura e não briga com a escrita no buffer.
        let mut destino = std::mem::take(&mut self.vertices);
        destino.clear();
        for v in self.estado.transformados() {
            destino.extend_from_slice(&v.position);
            destino.extend_from_slice(&v.color);
            destino.extend_from_slice(&v.uv);
        }
        self.vertices = destino;
        self.submete_com(modo, -1.0, None);
    }

    fn draw_texture(&mut self, x: f32, y: f32, z: f32, width: f32, height: f32) {
        let ligada = self.fill.textura_ligada;
        let Some(t) = self.texturas.get(&ligada) else {
            return;
        };
        let (tw, th) = (t.largura as f32, t.altura as f32);
        if tw == 0.0 || th == 0.0 || width == 0.0 || height == 0.0 {
            return;
        }
        let [ucr, vcr, wcr, hcr] = t.crop.map(|v| v as f32);
        // Recorte zerado é a textura inteira, como no rasterizador de software.
        let (wcr, hcr) = match (wcr, hcr) {
            (0.0, 0.0) => (tw, th),
            _ => (wcr, hcr),
        };
        let (s0, s1) = (ucr / tw, (ucr + wcr) / tw);
        let (t0, t1) = (vcr / th, (vcr + hcr) / th);
        let altura_superficie = self.surface().1 as f32;
        let (esquerda, direita) = (x, x + width);
        let (topo, base) = (altura_superficie - y - height, altura_superficie - y);
        let cor = self.current_color();
        let (vx, vy, vw, vh) = self.fill.viewport;
        if vw <= 0 || vh <= 0 {
            return;
        }
        let cantos = [
            ([esquerda, topo], [s0, t1]),
            ([esquerda, base], [s0, t0]),
            ([direita, base], [s1, t0]),
            ([direita, topo], [s1, t1]),
        ];
        self.vertices.clear();
        for ([sx, sy], uv) in cantos {
            let v = Vertex {
                normal: [0.0, 0.0, 1.0],
                position: [
                    ((sx - vx as f32) / vw as f32) * 2.0 - 1.0,
                    1.0 - ((sy - vy as f32) / vh as f32) * 2.0,
                    z * 2.0 - 1.0,
                    1.0,
                ],
                color: cor,
                uv,
            };
            self.poe(&v);
        }
        // A extensão não define orientação para o retângulo, então descartá-lo por face seria
        // descartar um desenho que o jogo espera ver.
        let guarda = self.fill.clone();
        self.fill.descarte = false;
        self.fill.texturando = true;
        self.submete_com(glow::TRIANGLE_FAN, -1.0, None);
        self.fill = guarda;
    }

    fn read_rect(&mut self, x: i32, y: i32, width: usize, height: usize) -> Vec<[u8; 4]> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        self.destino();
        let mut bytes = vec![0u8; width * height * 4];
        unsafe {
            self.gl.read_pixels(
                x,
                y,
                width as i32,
                height as i32,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
        }
        self.devolve_o_contexto();
        bytes
            .chunks_exact(4)
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect()
    }

    fn frame_rgb565(&mut self, width: usize, height: usize, out: &mut Vec<u8>) {
        // Mesmo atalho do rasterizador de software: quadro igual ao que já está em `out` não tem
        // o que reconverter. Na placa isso vale ainda mais, porque a leitura é uma ida e volta.
        if !self.sujo && out.len() == width * height * 2 {
            return;
        }
        let (sw, sh) = self.surface();
        if sw == 0 || sh == 0 {
            return;
        }
        self.le_quadro(sw, sh);
        out.clear();
        out.resize(width * height * 2, 0);
        let converte = |p: &[u8]| -> u16 {
            ((p[0] as u16 >> 3) << 11) | ((p[1] as u16 >> 2) << 5) | (p[2] as u16 >> 3)
        };
        for (y, saida) in out.chunks_exact_mut(width * 2).enumerate() {
            let linha = (y * sh / height).min(sh - 1);
            for (x, par) in saida.chunks_exact_mut(2).enumerate() {
                let coluna = (x * sw / width).min(sw - 1);
                let offset = (linha * sw + coluna) * 4;
                par.copy_from_slice(&converte(&self.pixels[offset..offset + 4]).to_le_bytes());
            }
        }
        self.sujo = false;
    }

    fn import_rgb565_changes(&mut self, width: usize, height: usize, old: &[u8], new: &[u8]) {
        let (sw, sh) = self.surface();
        if width == 0 || height == 0 || old.len() != width * height * 2 || new.len() != old.len() {
            return;
        }
        let (fw, fh) = self.frame_size();
        // **A fonte e o destino podem ter tamanhos diferentes, e quase sempre têm.** Na Z-Wheel o
        // pedido é de 640x330 — o pbuffer — e a superfície é 640x480. O rasterizador de software
        // cai num laço com escala e estica a fonte sobre a superfície; desenhar 1:1 nas 330
        // primeiras linhas comprimia tudo em 0..226 depois da reamostragem do quadro.
        //
        // Aqui quem estica é o amostrador da placa, de graça: a textura é a fonte inteira e o
        // retângulo cobre a superfície.
        let (colunas, linhas) = (width, height);
        let destino_x = sw.min(fw);
        let destino_y = sh.min(fh);
        if colunas == 0 || linhas == 0 || destino_x == 0 || destino_y == 0 {
            return;
        }
        // **O alfa carrega a máscara.** O rasterizador de software escreve só os pixels que
        // mudaram e preserva o alfa do destino; aqui o mesmo efeito sai numa transferência só:
        // alfa 255 onde mudou, 0 onde não, o teste de alfa descarta o resto e o `glColorMask`
        // fecha o canal de alfa para não sobrescrever o do destino.
        self.pixels.clear();
        self.pixels.resize(colunas * linhas * 4, 0);
        let mut mudou = false;
        for y in 0..linhas {
            let inicio = y * width * 2;
            let fim = inicio + colunas * 2;
            if old[inicio..fim] == new[inicio..fim] {
                continue;
            }
            for x in 0..colunas {
                let offset = inicio + x * 2;
                if old[offset..offset + 2] == new[offset..offset + 2] {
                    continue;
                }
                let [r, g, b] = expande565(new, offset);
                let destino = (y * colunas + x) * 4;
                self.pixels[destino..destino + 4].copy_from_slice(&[r, g, b, 255]);
                mudou = true;
            }
        }
        if !mudou {
            return;
        }
        self.destino();
        unsafe {
            let gl = &self.gl;
            gl.bind_texture(glow::TEXTURE_2D, Some(self.ponte));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                colunas as i32,
                linhas as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&self.pixels)),
            );
            for (nome, valor) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32),
                (glow::TEXTURE_MAX_LEVEL, 0),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, nome, valor);
            }
        }
        // O retângulo cobre a superfície dentro do quadro. As coordenadas já saem prontas, então
        // o shader não vira o Y: `virar` vale +1.
        let x1 = (destino_x as f32 / fw as f32) * 2.0 - 1.0;
        let y1 = (destino_y as f32 / fh as f32) * 2.0 - 1.0;
        self.vertices.clear();
        for ([px, py], uv) in [
            ([-1.0f32, -1.0f32], [0.0f32, 0.0f32]),
            ([x1, -1.0], [1.0, 0.0]),
            ([x1, y1], [1.0, 1.0]),
            ([-1.0, y1], [0.0, 1.0]),
        ] {
            self.poe(&Vertex {
                position: [px, py, 0.0, 1.0],
                color: [1.0; 4],
                uv,
                normal: [0.0, 0.0, 1.0],
            });
        }
        let guarda = self.fill.clone();
        self.fill.viewport = (0, 0, fw as i32, fh as i32);
        self.fill.teste_profundidade = false;
        self.fill.teste_stencil = false;
        self.fill.mistura = false;
        self.fill.descarte = false;
        self.fill.mascara_profundidade = false;
        self.fill.mascara_cor = [true, true, true, false];
        self.fill.env_textura = gles::GL_REPLACE;
        self.fill.teste_alfa = true;
        self.fill.func_alfa = gles::GL_GREATER;
        self.fill.ref_alfa = 0.5;
        let ponte = self.ponte;
        self.submete_com(glow::TRIANGLE_FAN, 1.0, Some(ponte));
        self.fill = guarda;
        self.devolve_o_contexto();
    }

    fn present(&mut self, width: usize, height: usize) -> Vec<u16> {
        let mut bytes = Vec::new();
        self.frame_rgb565(width, height, &mut bytes);
        bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre os dois rasterizadores no mesmo tamanho, ou desiste quando não há placa.
    ///
    /// Sem contexto não há o que comparar, e exigir uma placa de quem roda a suíte seria pedir
    /// que ela falhasse em máquina sem EGL. Os testes daqui relatam e passam nesse caso.
    fn par(largura: usize, altura: usize) -> Option<(GpuState, GlState)> {
        match GpuState::novo(largura, altura, None) {
            Ok(gpu) => Some((gpu, GlState::new(largura, altura))),
            Err(motivo) => {
                println!("sem placa nesta máquina: {motivo}");
                None
            }
        }
    }

    /// Desenha a mesma coisa nos dois e devolve os quadros em RGB565, para comparar.
    fn ambos(
        gpu: &mut GpuState,
        sw: &mut GlState,
        largura: usize,
        altura: usize,
        cena: impl Fn(&mut dyn Rasterizador),
    ) -> (Vec<u8>, Vec<u8>) {
        let mut a = Vec::new();
        let mut b = Vec::new();
        cena(gpu);
        cena(sw);
        gpu.frame_rgb565(largura, altura, &mut a);
        sw.frame_rgb565(largura, altura, &mut b);
        (a, b)
    }

    /// O pixel `(x, y)` de um quadro RGB565, como `(r, g, b)` de cinco/seis bits.
    fn pixel(quadro: &[u8], largura: usize, x: usize, y: usize) -> (u16, u16, u16) {
        let i = (y * largura + x) * 2;
        let v = u16::from_le_bytes([quadro[i], quadro[i + 1]]);
        ((v >> 11) & 31, (v >> 5) & 63, v & 31)
    }

    /// Um triângulo que cobre o canto superior esquerdo, para pegar orientação e preenchimento.
    ///
    /// É o teste que separa "a placa não desenhou" de "a placa desenhou no lugar errado" — e o
    /// lugar errado é o erro mais fácil de cometer aqui, porque o OpenGL conta linhas de baixo
    /// para cima e a superfície do console, de cima para baixo.
    #[test]
    fn o_triangulo_cai_no_mesmo_canto_nos_dois_rasterizadores() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Em espaço de recorte: canto superior esquerdo é (-1, +1).
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);

        let canto_placa = pixel(&a, largura, 1, 1);
        let canto_software = pixel(&b, largura, 1, 1);
        assert_eq!(
            canto_software.0, 31,
            "o software devia pintar o canto de cima de vermelho"
        );
        assert_eq!(
            canto_placa, canto_software,
            "canto de cima: placa {canto_placa:?} contra software {canto_software:?}"
        );
        let baixo_placa = pixel(&a, largura, 14, 14);
        let baixo_software = pixel(&b, largura, 14, 14);
        assert_eq!(
            baixo_placa, baixo_software,
            "canto de baixo: placa {baixo_placa:?} contra software {baixo_software:?}"
        );
    }

    /// Quem nunca chama `glViewport` tem que desenhar de todo jeito.
    ///
    /// A Z-Wheel é assim — zero chamadas em treze segundos —, e foi este o defeito que apagou a
    /// roda, o chão e o diálogo dela na primeira execução na placa: a viewport nascia em zero.
    #[test]
    fn sem_glviewport_a_placa_desenha_na_tela_inteira() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 0.0, 1.0],
                color: [0.0, 1.0, 0.0, 1.0],
                uv: [0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
        assert_eq!(
            pixel(&b, largura, 1, 1).1,
            63,
            "o software devia pintar o canto de verde sem viewport nenhuma"
        );
        assert_eq!(
            pixel(&a, largura, 1, 1),
            pixel(&b, largura, 1, 1),
            "sem viewport: placa {:?} contra software {:?}",
            pixel(&a, largura, 1, 1),
            pixel(&b, largura, 1, 1)
        );
    }

    /// Geometria além do plano distante: o software desenha, e o OpenGL recortaria.
    ///
    /// O rasterizador de software só recorta no plano **próximo**; o OpenGL recorta nos seis
    /// planos do frustum. Um jogo que passe do plano distante — e a Z-Wheel passa — veria a
    /// superfície simplesmente desaparecer na placa. É o que o `GL_DEPTH_CLAMP` resolve: em vez
    /// de recortar em profundidade, ele prende o valor na faixa.
    #[test]
    fn o_que_passa_do_plano_distante_aparece_nos_dois() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
            // Z além de 1 em espaço de recorte: fora do frustum pelo plano distante.
            let canto = |x: f32, y: f32| Vertex {
                position: [x, y, 1.5, 1.0],
                color: [0.0, 0.0, 1.0, 1.0],
                uv: [0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            };
            r.draw(
                gles::GL_TRIANGLES,
                &[canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
            );
        };
        let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
        assert_eq!(
            pixel(&b, largura, 2, 2).2,
            31,
            "o software devia desenhar, porque não recorta no plano distante"
        );
        assert_eq!(
            pixel(&a, largura, 2, 2),
            pixel(&b, largura, 2, 2),
            "além do plano distante: placa {:?} contra software {:?}",
            pixel(&a, largura, 2, 2),
            pixel(&b, largura, 2, 2)
        );
    }

    /// O descarte por face tem que concordar nos dois, nas quatro combinações.
    ///
    /// É o ponto onde as convenções se cruzam: o rasterizador de software decide em coordenadas
    /// de tela e chama CCW o caso de área **negativa** (linha do `counter_clockwise`), enquanto o
    /// OpenGL chama CCW o de área positiva. Como o Y é virado no shader, as coordenadas dos dois
    /// são numericamente as mesmas — então a face frontal precisa ser invertida ao entrar na
    /// placa. Este teste é o que prova que a inversão está no sentido certo, e não o contrário.
    #[test]
    fn o_descarte_por_face_concorda_nos_dois() {
        let (largura, altura) = (16, 16);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        for frente in [gles::GL_CCW, gles::GL_CW] {
            for invertido in [false, true] {
                let cena = |r: &mut dyn Rasterizador| {
                    r.set_viewport(0, 0, largura as i32, altura as i32);
                    r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
                    r.clear(gles::GL_COLOR_BUFFER_BIT);
                    r.set_capability(gles::GL_CULL_FACE, true);
                    r.set_cull_face(gles::GL_BACK);
                    r.set_front_face(frente);
                    let canto = |x: f32, y: f32| Vertex {
                        position: [x, y, 0.0, 1.0],
                        color: [1.0, 1.0, 1.0, 1.0],
                        uv: [0.0, 0.0],
                        normal: [0.0, 0.0, 1.0],
                    };
                    // Um triângulo que cobre o centro, nas duas ordens de vértice.
                    let tri = match invertido {
                        false => [canto(-1.0, 1.0), canto(-1.0, -1.0), canto(1.0, 1.0)],
                        true => [canto(1.0, 1.0), canto(-1.0, -1.0), canto(-1.0, 1.0)],
                    };
                    r.draw(gles::GL_TRIANGLES, &tri);
                };
                let (a, b) = ambos(&mut gpu, &mut sw, largura, altura, cena);
                let na_placa = pixel(&a, largura, 2, 2);
                let no_software = pixel(&b, largura, 2, 2);
                assert_eq!(
                    na_placa, no_software,
                    "frente={frente:#x} invertido={invertido}: placa {na_placa:?} contra \
                     software {no_software:?}"
                );
            }
        }
    }

    /// O `import_rgb565_changes` é por onde o 2D do jogo entra no palco: é ele que leva o diálogo
    /// e as capas da Z-Wheel para dentro do quadro do OpenGL.
    #[test]
    fn o_import_poe_os_pixels_do_jogo_no_mesmo_lugar() {
        let (largura, altura) = (8, 8);
        let Some((mut gpu, mut sw)) = par(largura, altura) else {
            return;
        };
        // Um quadro todo preto, e um segundo com a primeira linha branca: só ela deve mudar.
        let velho = vec![0u8; largura * altura * 2];
        let mut novo = velho.clone();
        for x in 0..largura {
            novo[x * 2] = 0xff;
            novo[x * 2 + 1] = 0xff;
        }
        let cena = |r: &mut dyn Rasterizador| {
            r.set_viewport(0, 0, largura as i32, altura as i32);
            r.set_clear_color([0.0, 0.0, 0.0, 1.0]);
            r.clear(gles::GL_COLOR_BUFFER_BIT);
        };
        cena(&mut gpu);
        cena(&mut sw);
        gpu.import_rgb565_changes(largura, altura, &velho, &novo);
        sw.import_rgb565_changes(largura, altura, &velho, &novo);
        let mut a = Vec::new();
        let mut b = Vec::new();
        gpu.frame_rgb565(largura, altura, &mut a);
        sw.frame_rgb565(largura, altura, &mut b);

        assert_eq!(
            pixel(&b, largura, 3, 0),
            (31, 63, 31),
            "o software devia ter a primeira linha branca"
        );
        assert_eq!(
            pixel(&a, largura, 3, 0),
            pixel(&b, largura, 3, 0),
            "primeira linha: placa {:?} contra software {:?}",
            pixel(&a, largura, 3, 0),
            pixel(&b, largura, 3, 0)
        );
        assert_eq!(
            pixel(&a, largura, 3, 5),
            pixel(&b, largura, 3, 5),
            "linha do meio devia ter ficado preta nos dois"
        );
    }
}
