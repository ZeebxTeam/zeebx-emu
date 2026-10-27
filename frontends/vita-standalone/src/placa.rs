//! A placa da Vita. O vitaGL abre o contexto; o preenchimento do jogo é o shader do núcleo.
//!
//! O vitaGL traduz GLSL ES 1.00 e se apresenta como OpenGL ES 2.0. Se a inicialização falha,
//! o chamador segue no framebuffer do kernel. Se ela funciona, o vitaGL fica dono da tela até
//! o processo acabar — não há `vglEnd` — e o menu sobe como textura em vez de
//! `sceDisplaySetFrameBuf`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use glow::HasContext;

use super::anota;

const LARGURA: f32 = 960.0;
const ALTURA: f32 = 544.0;

struct Estado {
    gl: Arc<glow::Context>,
    /// O quadrado da placa, quando o quadro volta da memória. A lista e o jogo
    /// no processador não o usam: neles o blit é que aparece.
    programa: glow::Program,
    vbo: glow::Buffer,
    /// De onde o blit lê a imagem que veio da CPU.
    leitura: glow::Framebuffer,
    textura_cpu: glow::Texture,
}

static ESTADO: Mutex<Option<Estado>> = Mutex::new(None);

unsafe extern "C" {
    fn vglGetProcAddress(name: *const i8) -> *const c_void;
    fn vglInitWithCustomSizes(
        legacy_pool_size: i32,
        width: i32,
        height: i32,
        ram_size: i32,
        cdram_size: i32,
        phycont_size: i32,
        cdlg_size: i32,
        msaa: i32,
    ) -> i32;
    fn vglSwapBuffers(has_common_dialog: u8);
    fn vglWaitVblankStart(enable: u8);
    fn vglUseTripleBuffering(usage: u8);
    fn vglSetShaderCachePath(path: *const i8);
    fn vglMemalign(alignment: u32, size: u32) -> *mut u8;
    fn vglBufferData(target: u32, data: *const u8);
    /// Só para achar, no binário já relocado, o pool circular de vértices.
    fn vglAllocFromScratch(size: usize) -> *mut u8;
}

const VERTICE: &str = r#"
attribute vec2 pos;
attribute vec2 uv;
varying vec2 vuv;
void main() {
    vuv = uv;
    gl_Position = vec4(pos, 0.0, 1.0);
}
"#;

const FRAGMENTO: &str = r#"
precision mediump float;
varying vec2 vuv;
uniform sampler2D imagem;
void main() {
    gl_FragColor = texture2D(imagem, vuv);
}
"#;

/// O que deu para fazer com a placa.
pub enum Abertura {
    /// Contexto e shader da tela prontos. O vitaGL é o dono da tela.
    Pronta,
    /// O vitaGL recusou o init. O framebuffer do kernel ainda serve.
    SemPlaca,
    /// O init passou e o vitaGL ficou com a tela, mas o shader de apresentação não ligou.
    /// Voltar ao framebuffer do kernel brigaria com ele.
    TomouATela(String),
}

/// Sobe o vitaGL.
pub fn abre() -> Abertura {
    let _ = std::fs::create_dir_all("ux0:data/zeebx/shader_cache");
    // O cache tem de existir antes do init: a primeira compilação passa pelo SceShaccCg.
    unsafe {
        vglSetShaderCachePath(c"ux0:data/zeebx/shader_cache".as_ptr());
        // O byte nasce em 3. O terceiro buffer é um quadro a mais entre o
        // comando e a imagem: o personagem já andou, e a tela ainda mostra o
        // quadro anterior. O mesmo byte diz em quantos pools o init reparte o
        // anel de vértices, então tem de ser antes.
        vglUseTripleBuffering(0);
        // O `vglInitExtended` trata o quarto argumento como o que sobra, não como o
        // tamanho do pool. Com 16 MiB de limiar ele ficou com o resto da partição do
        // usuário: no coredump `0x000136399b` o `ram_mempool` tinha 137 MiB, a CDRAM
        // 96 MiB e a PHYCONT 26 MiB, por cima do heap de 192 MiB. O `glClear` seguinte
        // morreu em `sceGxmReserveVertexDefaultUniformBuffer` sem página física
        // (`ScePhyMemPartGame`). Aqui o tamanho é o bloco. 16 MiB de RAM deixam o
        // resto da partição para o GXM e para os blocos grandes do jogo; 64 MiB de
        // CDRAM cabem o anel de vértices de 32 MiB e as texturas; 8 MiB de PHYCONT
        // cabem os alvos de render. O diálogo fica com o mesmo `0x8C6000` que o
        // `vglInitExtended` gravava.
        let codigo = vglInitWithCustomSizes(
            0x10_0000, 960, 544, 0x100_0000, 0x400_0000, 0x80_0000, 0x8C_6000, 0,
        );
        // O cabeçalho diz `GLboolean`, mas o binário devolve 0 quando 960×544 cabe na
        // tela e 1 quando precisa ajustar. Os dois já subiram o GXM. Ler o 0 como
        // falha pedia de novo a PHYCONT e a CDRAM, que o init tinha acabado de tomar:
        // `0x80024302` e `0x80024309`, sem página física livre.
        anota(&format!("vglInit {codigo}"));
        if codigo < 0 {
            anota("vitaGL não subiu; o quadro fica no processador");
            return Abertura::SemPlaca;
        }
        super::anota_livre();
        vglWaitVblankStart(1);
        // O init deixa uma thread desenhando o logo do vitaGL. Ela só larga a tela na
        // primeira troca. Compilar o shader antes disso deixa o logo na frente o tempo
        // todo, e as duas threads disputam o mesmo contexto.
        anota("vitaGL no ar; tirando o logo");
        vglSwapBuffers(0);
    }
    let gl = Arc::new(unsafe {
        glow::Context::from_loader_function(|nome| {
            let Ok(c) = std::ffi::CString::new(nome) else {
                return std::ptr::null();
            };
            vglGetProcAddress(c.as_ptr())
        })
    });
    unsafe {
        anota(&format!(
            "GL {} / {} / {} / {}",
            gl.get_parameter_string(glow::VENDOR),
            gl.get_parameter_string(glow::RENDERER),
            gl.get_parameter_string(glow::VERSION),
            gl.get_parameter_string(glow::SHADING_LANGUAGE_VERSION),
        ));
    }
    anota("compilando o shader da tela");
    let programa = match unsafe { programa_de_apresentacao(&gl) } {
        Ok(programa) => programa,
        Err(erro) => {
            // O vitaGL já tomou a tela. Sem este shader não há como mostrar o menu nem o jogo,
            // e o framebuffer do kernel não recupera a tela.
            return Abertura::TomouATela(format!("o shader da tela não ligou ({erro})"));
        }
    };
    let (vbo, leitura, textura_cpu) = unsafe {
        let vbo = match gl.create_buffer() {
            Ok(vbo) => vbo,
            Err(erro) => {
                return Abertura::TomouATela(format!("sem buffer da tela ({erro})"));
            }
        };
        let leitura = match gl.create_framebuffer() {
            Ok(leitura) => leitura,
            Err(erro) => {
                return Abertura::TomouATela(format!("sem framebuffer da tela ({erro})"));
            }
        };
        let textura = match gl.create_texture() {
            Ok(textura) => textura,
            Err(erro) => {
                return Abertura::TomouATela(format!("sem textura da tela ({erro})"));
            }
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(textura));
        // 960×544 não é potência de dois. No ES 2 essa textura só amostra com clamp e sem mip.
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
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        (vbo, leitura, textura)
    };
    // Um `glClear` num framebuffer próprio tem de voltar na leitura. Neste Vita3K
    // não volta: a troca seguinte só republica a lista. O jogo, então, sai pelo
    // processador, que é o quadro que o blit da lista consegue mostrar.
    let guarda_quadro = o_quadro_volta(&gl);
    if guarda_quadro {
        anota("o framebuffer da placa devolveu o quadro; o jogo segue na placa");
    } else {
        anota("o framebuffer da placa não devolve o quadro; o jogo segue no processador");
    }
    GUARDA_O_QUADRO.store(guarda_quadro, Ordering::Relaxed);
    let mut guarda = ESTADO.lock().unwrap_or_else(|e| e.into_inner());
    *guarda = Some(Estado {
        gl,
        programa,
        vbo,
        leitura,
        textura_cpu,
    });
    Abertura::Pronta
}

static GUARDA_O_QUADRO: AtomicBool = AtomicBool::new(false);

/// A placa devolveu o verde que foi pedido num framebuffer de 2×2.
fn o_quadro_volta(gl: &glow::Context) -> bool {
    unsafe {
        let Ok(textura) = gl.create_texture() else {
            return false;
        };
        gl.bind_texture(glow::TEXTURE_2D, Some(textura));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            2,
            2,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        let Ok(fbo) = gl.create_framebuffer() else {
            gl.delete_texture(textura);
            return false;
        };
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(textura),
            0,
        );
        gl.disable(glow::SCISSOR_TEST);
        gl.color_mask(true, true, true, true);
        gl.clear_color(0.0, 1.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.finish();
        let mut lido = [0u8; 4];
        gl.read_pixels(
            0,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut lido)),
        );
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.delete_framebuffer(fbo);
        gl.delete_texture(textura);
        lido[1] > 200 && lido[3] > 200
    }
}

/// O jogo pode desenhar na placa. Falso quando o quadro não volta da memória.
pub fn guarda_o_quadro() -> bool {
    GUARDA_O_QUADRO.load(Ordering::Relaxed)
}

pub fn ativa() -> bool {
    ESTADO.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub fn contexto() -> Option<Arc<glow::Context>> {
    ESTADO
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|estado| Arc::clone(&estado.gl))
}

/// O menu e o quadro de software. A palavra é `0xAABBGGRR`, que na memória sai R, G, B, A.
pub fn mostra_rgba(pixels: &[u32], w: i32, h: i32) {
    let guarda = ESTADO.lock().unwrap_or_else(|e| e.into_inner());
    let Some(estado) = guarda.as_ref() else {
        return;
    };
    unsafe {
        let gl = &estado.gl;
        gl.bind_texture(glow::TEXTURE_2D, Some(estado.textura_cpu));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
        let bytes = std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(bytes)),
        );
        // A limpeza vem primeiro: é ela que espera o logo sair. O anexo marca
        // a cena suja e o blit abre outra, só com a imagem. A lista ocupa a
        // tela inteira, então o preto da cena anterior não aparece.
        prepara_a_tela(gl);
        gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(estado.leitura));
        gl.framebuffer_texture_2d(
            glow::READ_FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(estado.textura_cpu),
            0,
        );
        copia(gl, w, h, 0, 0, w, h);
        vglSwapBuffers(0);
    }
}

/// Devolve o pool de vértices do vitaGL ao começo quando a folga acaba.
///
/// Cada `glDrawArrays` copia os vértices para esse pool, e o GXM ainda lê a
/// cópia depois da troca de quadro. O cursor só anda para a frente: passado o
/// fim, o desenho pede um bloco e o enfileira para o coletor, que larga de
/// devolver o que enfileirou. Voltar o cursor ao começo em todo quadro pisa na
/// cópia que o quadro da tela ainda usa — o sprite sai com o uv de outro, um
/// quadrado de outra folha. A folga cabe vários quadros. Só rebobinamos quando
/// ela acaba, e o `glFinish` espera o GXM soltar o pool antes.
pub fn prepara_o_quadro() {
    let Some(estrutura) = estrutura_do_pool() else {
        return;
    };
    // Um quadro de sprites fica abaixo disto. Com menos folga, o próximo
    // desenho estoura o pool.
    const MARGEM: u32 = 6 << 20;
    let (base, cursor, limite) = unsafe {
        let campos = estrutura as *const u32;
        (
            campos.add(0x58 / 4).read(),
            campos.add(0x70 / 4).read(),
            campos.add(0x88 / 4).read(),
        )
    };
    if base != 0 && cursor >= base && limite.saturating_sub(cursor) > MARGEM {
        return;
    }
    if let Some(gl) = contexto() {
        unsafe { gl.finish() };
    }
    if base != 0 {
        unsafe {
            let campos = estrutura as *mut u32;
            campos.add(0x70 / 4).write(base);
        }
    }
}

static AVISO_DO_POOL: AtomicBool = AtomicBool::new(false);
static APRESENTACAO: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static APRESENTACAO_LIGADA: AtomicBool = AtomicBool::new(false);

/// O `vglAllocFromScratch` carrega o endereço do pool com `movw`/`movt`. No processo
/// esses imediatos já estão relocados; o valor no ELF não serve.
fn estrutura_do_pool() -> Option<*mut u8> {
    static CACHE: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    let ja = CACHE.load(Ordering::Relaxed);
    if !ja.is_null() {
        return Some(ja);
    }
    let entrada = (vglAllocFromScratch as *const () as usize) & !1;
    let codigo = unsafe { std::slice::from_raw_parts(entrada as *const u16, 16) };
    let mut baixo = None;
    let mut alto = None;
    let mut i = 0;
    while i + 1 < codigo.len() {
        let hw1 = codigo[i];
        let hw2 = codigo[i + 1];
        let opcode = hw1 & 0xfbf0;
        let movt = match opcode {
            0xf240 => Some(false),
            0xf2c0 => Some(true),
            _ => None,
        };
        if let Some(movt) = movt {
            let rd = (hw2 >> 8) & 0xf;
            if rd == 3 {
                let imm = ((u32::from(hw1 & 0xf)) << 12)
                    | ((u32::from((hw1 >> 10) & 1)) << 11)
                    | ((u32::from((hw2 >> 12) & 7)) << 8)
                    | u32::from(hw2 & 0xff);
                if movt {
                    alto.get_or_insert(imm);
                } else {
                    baixo.get_or_insert(imm);
                }
            }
            i += 2;
            continue;
        }
        // Thumb de 32 bits começa com 11101, 1111 ou 11110. O resto é 16 bits.
        let trinta_e_dois = hw1 >= 0xe800;
        i += if trinta_e_dois { 2 } else { 1 };
    }
    let (Some(baixo), Some(alto)) = (baixo, alto) else {
        if !AVISO_DO_POOL.swap(true, Ordering::Relaxed) {
            anota("não achei o pool de vértices do vitaGL");
        }
        return None;
    };
    let endereco = ((alto << 16) | baixo) as *mut u8;
    // Base e limite do pool 0. O init grava os dois; a folga é 32 MiB
    // repartidos pelos buffers da tela. Com dois, cada pool fica em 16 MiB.
    let (base, limite) = unsafe {
        let campos = endereco as *const u32;
        (campos.add(0x58 / 4).read(), campos.add(0x88 / 4).read())
    };
    let folga = limite.wrapping_sub(base);
    if base == 0 || !(1 << 20..17 << 20).contains(&folga) {
        if !AVISO_DO_POOL.swap(true, Ordering::Relaxed) {
            anota("o pool de vértices do vitaGL não está onde o scratch aponta");
        }
        return None;
    }
    CACHE.store(endereco, Ordering::Relaxed);
    if !AVISO_DO_POOL.swap(true, Ordering::Relaxed) {
        anota("o pool de vértices do vitaGL rebobina quando a folga acaba");
    }
    Some(endereco)
}

/// O quadro do jogo, já na placa. `x` e `y` são o canto superior esquerdo, em pixels da tela.
pub fn mostra_textura(id: u32, x: i32, y: i32, w: i32, h: i32) {
    let guarda = ESTADO.lock().unwrap_or_else(|e| e.into_inner());
    let Some(estado) = guarda.as_ref() else {
        return;
    };
    unsafe {
        pinta(estado, id, x, y, w, h);
        vglSwapBuffers(0);
    }
}

unsafe fn programa_de_apresentacao(gl: &glow::Context) -> Result<glow::Program, String> {
    let mut ultimo = String::from("shader da tela");
    for cabecalho in ["#version 100\n", "\n"] {
        match unsafe { liga(gl, cabecalho) } {
            Ok(programa) => return Ok(programa),
            Err(erro) => ultimo = erro,
        }
    }
    Err(ultimo)
}

unsafe fn liga(gl: &glow::Context, cabecalho: &str) -> Result<glow::Program, String> {
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
        gl.bind_attrib_location(programa, 0, "pos");
        gl.bind_attrib_location(programa, 1, "uv");
        gl.link_program(programa);
        // O `vglGetProcAddress` do vitaGL não tem `glDetachShader`. O glow aborta
        // quando o ponteiro vem nulo ("called glDetachShader but it was not loaded",
        // no coredump). Apagar o shader ainda ligado é o que a especificação prevê:
        // ele é liberado quando o programa morre.
        for shader in shaders {
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

/// Desenha a textura. O `v` zero é o topo: o preenchimento do jogo inverte o Y, e a primeira
/// linha do menu também é o topo visual.
unsafe fn pinta(estado: &Estado, textura: u32, x: i32, y: i32, w: i32, h: i32) {
    let (x0, y1) = ndc(x as f32, y as f32);
    let (x1, y0) = ndc((x + w) as f32, (y + h) as f32);
    // Dois triângulos. O topo da tela (y de NDC maior) amostra v = 0.
    let vertices: [f32; 24] = [
        x0, y1, 0.0, 0.0, x1, y1, 1.0, 0.0, x0, y0, 0.0, 1.0, x1, y1, 1.0, 0.0, x1, y0, 1.0, 1.0,
        x0, y0, 0.0, 1.0,
    ];
    let gl = &estado.gl;
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.viewport(0, 0, LARGURA as i32, ALTURA as i32);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::STENCIL_TEST);
        gl.color_mask(true, true, true, true);
        // O vitaGL tem dois buffers. O quadrado do jogo só cobre 640×480 no centro;
        // o resto ficava com a lista de jogos, e ela aparecia atrás da cena. A limpeza
        // é deste buffer, a cada troca — limpar uma vez só apaga o outro na alternância.
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.bind_vertex_array(None);
        gl.use_program(Some(estado.programa));
        let imagem = gl.get_uniform_location(estado.programa, "imagem");
        gl.uniform_1_i32(imagem.as_ref(), 0);
        gl.active_texture(glow::TEXTURE0);
        let nativa = glow::NativeTexture(std::num::NonZeroU32::new(textura).unwrap());
        gl.bind_texture(glow::TEXTURE_2D, Some(nativa));
        // A cor do jogo é 640×480, também fora de potência de dois.
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
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(estado.vbo));
        // `glBufferData` pede um bloco novo a cada troca. O quadrado da tela cabe num
        // bloco só, escrito por cima: o desenho copia os seis vértices antes de voltar.
        let bytes = bytemuck_f32(&vertices);
        let mut bloco = APRESENTACAO.load(Ordering::Relaxed);
        if bloco.is_null() {
            bloco = vglMemalign(16, bytes.len() as u32);
            if !bloco.is_null() {
                APRESENTACAO.store(bloco, Ordering::Relaxed);
            }
        }
        if bloco.is_null() {
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
        } else {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), bloco, bytes.len());
            if !APRESENTACAO_LIGADA.swap(true, Ordering::Relaxed) {
                vglBufferData(glow::ARRAY_BUFFER, bloco);
            }
        }
        let passo = 4 * 4;
        gl.enable_vertex_attrib_array(0);
        gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, passo, 0);
        gl.enable_vertex_attrib_array(1);
        gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, passo, 8);
        for indice in 2..5 {
            gl.disable_vertex_attrib_array(indice);
        }
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.bind_buffer(glow::ARRAY_BUFFER, None);
        gl.use_program(None);
    }
}

fn ndc(px: f32, py: f32) -> (f32, f32) {
    (px / LARGURA * 2.0 - 1.0, 1.0 - py / ALTURA * 2.0)
}

fn bytemuck_f32(vertices: &[f32; 24]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(vertices.as_ptr().cast::<u8>(), vertices.len() * 4) }
}

/// Abre a cena da tela e pinta de preto. A leitura ligada continua valendo.
unsafe fn prepara_a_tela(gl: &glow::Context) {
    unsafe {
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::BLEND);
        gl.disable(glow::CULL_FACE);
        gl.disable(glow::STENCIL_TEST);
        gl.color_mask(true, true, true, true);
        // Dois buffers. O jogo cobre 640×480; limpar só uma vez deixa a lista
        // no outro, e ela volta na alternância.
        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.viewport(0, 0, LARGURA as i32, ALTURA as i32);
    }
}

/// Copia o que está ligado para leitura. O `v` zero é o topo da imagem.
///
/// O blit põe o `v` 1 no topo da tela. A origem do Y vem trocada para o topo
/// visual, que está no `v` 0, continuar no topo. A escrita volta para o que
/// estava antes do blit; quem chama e vai desenhar de novo na tela precisa
/// ligá-la outra vez.
unsafe fn blita(gl: &glow::Context, fw: i32, fh: i32, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        gl.blit_framebuffer(
            0,
            fh,
            fw,
            0,
            x,
            y,
            x + w,
            y + h,
            glow::COLOR_BUFFER_BIT,
            glow::NEAREST,
        );
    }
}

unsafe fn copia(gl: &glow::Context, fw: i32, fh: i32, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        blita(gl, fw, fh, x, y, w, h);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}
