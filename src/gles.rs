//! Constantes e estado do EGL 1.0 e do OpenGL ES 1.0 do BREW.
//!
//! Os valores saem de `sdk/inc/gles/egl.h` e `sdk/inc/gles/gl.h` do BREW SDK 4.0.2. O console
//! tem uma Adreno 130 e os jogos 3D falam com ela por estas duas interfaces; aqui a GPU é de
//! software, mas a conversa com o jogo é a mesma.

/// Códigos de retorno booleanos do EGL.
pub const EGL_FALSE: u32 = 0;
pub const EGL_TRUE: u32 = 1;
/// `EGL_SUCCESS` — o `eglGetError` sem erro pendente.
pub const EGL_SUCCESS: u32 = 0x3000;
pub const EGL_BAD_ATTRIBUTE: u32 = 0x3004;

/// `EGL_NONE`, que também encerra as listas de atributos.
pub const EGL_NONE: u32 = 0x3038;

/// Atributos de configuração, de `egl.h`.
pub const EGL_BUFFER_SIZE: u32 = 0x3020;
pub const EGL_ALPHA_SIZE: u32 = 0x3021;
pub const EGL_BLUE_SIZE: u32 = 0x3022;
pub const EGL_GREEN_SIZE: u32 = 0x3023;
pub const EGL_RED_SIZE: u32 = 0x3024;
pub const EGL_DEPTH_SIZE: u32 = 0x3025;
pub const EGL_STENCIL_SIZE: u32 = 0x3026;
pub const EGL_CONFIG_CAVEAT: u32 = 0x3027;
pub const EGL_CONFIG_ID: u32 = 0x3028;
pub const EGL_LEVEL: u32 = 0x3029;
pub const EGL_MAX_PBUFFER_HEIGHT: u32 = 0x302a;
pub const EGL_MAX_PBUFFER_PIXELS: u32 = 0x302b;
pub const EGL_MAX_PBUFFER_WIDTH: u32 = 0x302c;
pub const EGL_NATIVE_RENDERABLE: u32 = 0x302d;
pub const EGL_NATIVE_VISUAL_ID: u32 = 0x302e;
pub const EGL_NATIVE_VISUAL_TYPE: u32 = 0x302f;
pub const EGL_SAMPLES: u32 = 0x3031;
pub const EGL_SAMPLE_BUFFERS: u32 = 0x3032;
pub const EGL_SURFACE_TYPE: u32 = 0x3033;
pub const EGL_TRANSPARENT_TYPE: u32 = 0x3034;
pub const EGL_TRANSPARENT_BLUE_VALUE: u32 = 0x3035;
pub const EGL_TRANSPARENT_GREEN_VALUE: u32 = 0x3036;
pub const EGL_TRANSPARENT_RED_VALUE: u32 = 0x3037;
pub const EGL_BIND_TO_TEXTURE_RGB: u32 = 0x3039;
pub const EGL_BIND_TO_TEXTURE_RGBA: u32 = 0x303a;
pub const EGL_MIN_SWAP_INTERVAL: u32 = 0x303b;
pub const EGL_MAX_SWAP_INTERVAL: u32 = 0x303c;
pub const EGL_COLOR_BUFFER_TYPE: u32 = 0x303f;
pub const EGL_RENDERABLE_TYPE: u32 = 0x3040;
/// `EGL_RGB_BUFFER`, o único tipo de buffer de cor que oferecemos.
pub const EGL_RGB_BUFFER: u32 = 0x308e;
/// Máscara de `EGL_SURFACE_TYPE`: pbuffer, pixmap e janela.
pub const EGL_ALL_SURFACE_BITS: u32 = 0x07;
/// Máscara de `EGL_RENDERABLE_TYPE`: só OpenGL ES.
pub const EGL_OPENGL_ES_BIT: u32 = 0x01;

/// Nomes que `eglQueryString` aceita.
pub const EGL_VENDOR: u32 = 0x3053;
pub const EGL_VERSION: u32 = 0x3054;
pub const EGL_EXTENSIONS: u32 = 0x3055;

/// Atributos que `eglQuerySurface` responde.
pub const EGL_HEIGHT: u32 = 0x3056;
pub const EGL_WIDTH: u32 = 0x3057;

/// Nomes que `glGetString` aceita, de `gl.h`.
pub const GL_VENDOR: u32 = 0x1f00;
pub const GL_RENDERER: u32 = 0x1f01;
pub const GL_VERSION: u32 = 0x1f02;
/// `GL_EXTENSIONS` — respondemos com a lista vazia, que é uma resposta válida.
pub const GL_EXTENSIONS: u32 = 0x1f03;

/// `GL_NO_ERROR`.
pub const GL_NO_ERROR: u32 = 0;

/// A única configuração que oferecemos: RGB565 com profundidade de 16 bits, que é o formato
/// nativo da tela do console.
///
/// Responder uma configuração só é fiel ao que importa: o jogo pede uma que sirva, e esta
/// serve. `None` para um atributo desconhecido vira `EGL_BAD_ATTRIBUTE`, como manda a spec.
pub fn config_attrib(attribute: u32, width: u32, height: u32) -> Option<u32> {
    Some(match attribute {
        EGL_BUFFER_SIZE => 16,
        EGL_RED_SIZE | EGL_BLUE_SIZE => 5,
        EGL_GREEN_SIZE => 6,
        EGL_ALPHA_SIZE => 0,
        EGL_DEPTH_SIZE => 16,
        EGL_STENCIL_SIZE => 8,
        EGL_CONFIG_CAVEAT | EGL_NATIVE_VISUAL_TYPE | EGL_TRANSPARENT_TYPE => EGL_NONE,
        EGL_CONFIG_ID => 1,
        EGL_MAX_PBUFFER_WIDTH => width,
        EGL_MAX_PBUFFER_HEIGHT => height,
        EGL_MAX_PBUFFER_PIXELS => width * height,
        EGL_NATIVE_RENDERABLE => EGL_TRUE,
        EGL_SURFACE_TYPE => EGL_ALL_SURFACE_BITS,
        EGL_COLOR_BUFFER_TYPE => EGL_RGB_BUFFER,
        EGL_RENDERABLE_TYPE => EGL_OPENGL_ES_BIT,
        EGL_MIN_SWAP_INTERVAL | EGL_MAX_SWAP_INTERVAL => 1,
        EGL_LEVEL
        | EGL_NATIVE_VISUAL_ID
        | EGL_SAMPLES
        | EGL_SAMPLE_BUFFERS
        | EGL_TRANSPARENT_RED_VALUE
        | EGL_TRANSPARENT_GREEN_VALUE
        | EGL_TRANSPARENT_BLUE_VALUE
        | EGL_BIND_TO_TEXTURE_RGB
        | EGL_BIND_TO_TEXTURE_RGBA => 0,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_limites_do_opengl_nao_sao_zero() {
        // Responder zero para o tamanho máximo de textura diz ao jogo que nenhuma cabe.
        assert_eq!(integer(GL_MAX_TEXTURE_SIZE, 640, 480), Some(&[1024][..]));
        assert_eq!(integer(GL_MAX_TEXTURE_UNITS, 640, 480), Some(&[1][..]));
        assert_eq!(integer(GL_DEPTH_BITS, 640, 480), Some(&[16][..]));
        assert_eq!(integer(GL_GREEN_BITS, 640, 480), Some(&[6][..]));
        assert_eq!(
            integer(GL_MAX_VIEWPORT_DIMS, 640, 480),
            Some(&[640, 480][..])
        );
        assert_eq!(integer(0x9999, 640, 480), None);
    }

    #[test]
    fn a_configuracao_e_rgb565_com_profundidade() {
        assert_eq!(config_attrib(EGL_RED_SIZE, 640, 480), Some(5));
        assert_eq!(config_attrib(EGL_GREEN_SIZE, 640, 480), Some(6));
        assert_eq!(config_attrib(EGL_BLUE_SIZE, 640, 480), Some(5));
        assert_eq!(config_attrib(EGL_ALPHA_SIZE, 640, 480), Some(0));
        assert_eq!(config_attrib(EGL_BUFFER_SIZE, 640, 480), Some(16));
        assert_eq!(config_attrib(EGL_DEPTH_SIZE, 640, 480), Some(16));
        assert_eq!(
            config_attrib(EGL_MAX_PBUFFER_PIXELS, 640, 480),
            Some(307_200)
        );
        // Atributo que não existe tem de virar erro, não zero.
        assert_eq!(config_attrib(0x9999, 640, 480), None);
    }
}

// --- Enums do OpenGL ES 1.1, de `sdk/inc/gles/gl.h` -----------------------------------

/// Modos de `DrawArrays`/`DrawElements`.
pub const GL_POINTS: u32 = 0x0000;
pub const GL_LINES: u32 = 0x0001;
pub const GL_LINE_LOOP: u32 = 0x0002;
pub const GL_LINE_STRIP: u32 = 0x0003;
pub const GL_TRIANGLES: u32 = 0x0004;
pub const GL_TRIANGLE_STRIP: u32 = 0x0005;
pub const GL_TRIANGLE_FAN: u32 = 0x0006;

/// Tipos de dado dos vetores de vértice e dos índices.
pub const GL_BYTE: u32 = 0x1400;
pub const GL_UNSIGNED_BYTE: u32 = 0x1401;
pub const GL_SHORT: u32 = 0x1402;
pub const GL_UNSIGNED_SHORT: u32 = 0x1403;
pub const GL_FLOAT: u32 = 0x1406;
/// `GL_FIXED` — 16.16 com sinal, o formato nativo do perfil Common-Lite.
pub const GL_FIXED: u32 = 0x140c;

/// Pilhas de matriz.
pub const GL_MODELVIEW: u32 = 0x1700;
pub const GL_PROJECTION: u32 = 0x1701;
pub const GL_TEXTURE: u32 = 0x1702;

/// Vetores do cliente.
/// A iluminação de função fixa, e o que ela lê.
///
/// Os nomes e valores são os do `GLES/gl.h` do OpenGL ES 1.1. Só entram aqui os que o
/// pipeline usa; `GL_LIGHT0` é a base de uma faixa de oito.
pub const GL_STENCIL_TEST: u32 = 0x0b90;
pub const GL_LIGHTING: u32 = 0x0b50;
pub const GL_LIGHT_MODEL_AMBIENT: u32 = 0x0b53;
pub const GL_COLOR_MATERIAL: u32 = 0x0b57;
pub const GL_NORMALIZE: u32 = 0x0ba1;
pub const GL_RESCALE_NORMAL: u32 = 0x803a;
pub const GL_LIGHT0: u32 = 0x4000;
pub const LUZES: usize = 8;

pub const GL_AMBIENT: u32 = 0x1200;
pub const GL_DIFFUSE: u32 = 0x1201;
pub const GL_SPECULAR: u32 = 0x1202;
pub const GL_POSITION: u32 = 0x1203;
pub const GL_SPOT_DIRECTION: u32 = 0x1204;
pub const GL_SPOT_EXPONENT: u32 = 0x1205;
pub const GL_SPOT_CUTOFF: u32 = 0x1206;
pub const GL_CONSTANT_ATTENUATION: u32 = 0x1207;
pub const GL_LINEAR_ATTENUATION: u32 = 0x1208;
pub const GL_QUADRATIC_ATTENUATION: u32 = 0x1209;
pub const GL_EMISSION: u32 = 0x1600;
pub const GL_SHININESS: u32 = 0x1601;
pub const GL_AMBIENT_AND_DIFFUSE: u32 = 0x1602;

pub const GL_FLAT: u32 = 0x1d00;
pub const GL_SMOOTH: u32 = 0x1d01;

/// Quantos componentes um parâmetro de luz ou de material tem.
///
/// Serve para os dois: o `glLightxv` e o `glMaterialxv` leem do ponteiro exatamente isto, e ler
/// quatro palavras de um parâmetro de uma só passa por cima do que houver depois.
pub fn componentes(pname: u32) -> usize {
    match pname {
        GL_AMBIENT | GL_DIFFUSE | GL_SPECULAR | GL_POSITION | GL_EMISSION
        | GL_AMBIENT_AND_DIFFUSE | GL_LIGHT_MODEL_AMBIENT => 4,
        GL_SPOT_DIRECTION => 3,
        _ => 1,
    }
}

pub const GL_VERTEX_ARRAY: u32 = 0x8074;
pub const GL_NORMAL_ARRAY: u32 = 0x8075;
pub const GL_COLOR_ARRAY: u32 = 0x8076;
pub const GL_TEXTURE_COORD_ARRAY: u32 = 0x8078;

/// Estados de `Enable`/`Disable` que o rasterizador respeita.
pub const GL_TEXTURE_2D: u32 = 0x0de1;

/// Primeira unidade de textura. As demais são `GL_TEXTURE0 + n`.
pub const GL_TEXTURE0: u32 = 0x84c0;
pub const GL_DEPTH_TEST: u32 = 0x0b71;
pub const GL_BLEND: u32 = 0x0be2;
pub const GL_ALPHA_TEST: u32 = 0x0bc0;
pub const GL_CULL_FACE: u32 = 0x0b44;
pub const GL_SCISSOR_TEST: u32 = 0x0c11;

/// Máscaras de `Clear`.
pub const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
pub const GL_DEPTH_BUFFER_BIT: u32 = 0x0100;

/// Fatores de mistura.
pub const GL_ZERO: u32 = 0;
pub const GL_ONE: u32 = 1;
pub const GL_SRC_COLOR: u32 = 0x0300;
pub const GL_ONE_MINUS_SRC_COLOR: u32 = 0x0301;
pub const GL_SRC_ALPHA: u32 = 0x0302;
pub const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
pub const GL_DST_ALPHA: u32 = 0x0304;
pub const GL_ONE_MINUS_DST_ALPHA: u32 = 0x0305;
pub const GL_DST_COLOR: u32 = 0x0306;
pub const GL_ONE_MINUS_DST_COLOR: u32 = 0x0307;

/// Funções de comparação, usadas pelo teste de profundidade e pelo de alfa.
pub const GL_NEVER: u32 = 0x0200;
pub const GL_LESS: u32 = 0x0201;
pub const GL_EQUAL: u32 = 0x0202;
pub const GL_LEQUAL: u32 = 0x0203;
pub const GL_GREATER: u32 = 0x0204;
pub const GL_NOTEQUAL: u32 = 0x0205;
pub const GL_GEQUAL: u32 = 0x0206;
pub const GL_ALWAYS: u32 = 0x0207;

/// Formatos e tipos de `TexImage2D`.
pub const GL_ALPHA: u32 = 0x1906;
pub const GL_RGB: u32 = 0x1907;
pub const GL_RGBA: u32 = 0x1908;
pub const GL_LUMINANCE: u32 = 0x1909;
pub const GL_LUMINANCE_ALPHA: u32 = 0x190a;
pub const GL_UNSIGNED_SHORT_4_4_4_4: u32 = 0x8033;
pub const GL_UNSIGNED_SHORT_5_5_5_1: u32 = 0x8034;
pub const GL_UNSIGNED_SHORT_5_6_5: u32 = 0x8363;

/// Faces e orientação.
pub const GL_FRONT: u32 = 0x0404;
pub const GL_BACK: u32 = 0x0405;
pub const GL_FRONT_AND_BACK: u32 = 0x0408;
pub const GL_CW: u32 = 0x0900;
pub const GL_CCW: u32 = 0x0901;

/// Parâmetros de textura e seus valores.
pub const GL_TEXTURE_MAG_FILTER: u32 = 0x2800;
pub const GL_TEXTURE_MIN_FILTER: u32 = 0x2801;
/// `GL_TEXTURE_CROP_RECT_OES`, do `GL_OES_draw_texture`: o retângulo da textura que o
/// `glDrawTex*OES` desenha.
pub const GL_TEXTURE_CROP_RECT_OES: u32 = 0x8b9d;
pub const GL_TEXTURE_WRAP_S: u32 = 0x2802;
pub const GL_TEXTURE_WRAP_T: u32 = 0x2803;
pub const GL_NEAREST: u32 = 0x2600;
pub const GL_LINEAR: u32 = 0x2601;
pub const GL_REPEAT: u32 = 0x2901;
pub const GL_CLAMP_TO_EDGE: u32 = 0x812f;

/// Ambiente de textura.
pub const GL_TEXTURE_ENV_MODE: u32 = 0x2200;
pub const GL_MODULATE: u32 = 0x2100;
pub const GL_DECAL: u32 = 0x2101;
pub const GL_REPLACE: u32 = 0x1e01;

/// As operações do `glStencilOp`. O `GL_REPLACE` acima é uma delas e já existia, vindo do
/// `glTexEnv`: o mesmo valor serve aos dois no OpenGL.
pub const GL_ZERO_OP: u32 = 0;
pub const GL_KEEP: u32 = 0x1e00;
pub const GL_INCR: u32 = 0x1e02;
pub const GL_DECR: u32 = 0x1e03;
pub const GL_INVERT: u32 = 0x150a;
pub const GL_STENCIL_BUFFER_BIT: u32 = 0x0400;
pub const GL_ADD: u32 = 0x0104;

/// Limites e capacidades que `glGetIntegerv` responde, de `gl.h`.
pub const GL_MAX_LIGHTS: u32 = 0x0d31;
pub const GL_MAX_TEXTURE_SIZE: u32 = 0x0d33;
pub const GL_MAX_MODELVIEW_STACK_DEPTH: u32 = 0x0d36;
pub const GL_MAX_PROJECTION_STACK_DEPTH: u32 = 0x0d38;
pub const GL_MAX_TEXTURE_STACK_DEPTH: u32 = 0x0d39;
pub const GL_MAX_VIEWPORT_DIMS: u32 = 0x0d3a;
pub const GL_SUBPIXEL_BITS: u32 = 0x0d50;
pub const GL_RED_BITS: u32 = 0x0d52;
pub const GL_GREEN_BITS: u32 = 0x0d53;
pub const GL_BLUE_BITS: u32 = 0x0d54;
pub const GL_ALPHA_BITS: u32 = 0x0d55;
pub const GL_DEPTH_BITS: u32 = 0x0d56;
pub const GL_STENCIL_BITS: u32 = 0x0d57;
pub const GL_MAX_ELEMENTS_VERTICES: u32 = 0x80e8;
pub const GL_MAX_ELEMENTS_INDICES: u32 = 0x80e9;
pub const GL_MAX_TEXTURE_UNITS: u32 = 0x84e2;
pub const GL_NUM_COMPRESSED_TEXTURE_FORMATS: u32 = 0x86a2;

/// Maior textura que aceitamos. Não há limite real num rasterizador de software, mas os jogos
/// dimensionam os atlas por este número, e um valor absurdo só levaria a alocações absurdas.
pub const MAX_TEXTURE_SIZE: i32 = 1024;

/// O que `glGetIntegerv` responde. `None` para o que não conhecemos.
///
/// Alguns nomes devolvem mais de um inteiro, daí a fatia. Responder zero para tudo — que era o
/// que fazíamos — diz ao jogo que a maior textura possível tem lado zero.
pub fn integer(name: u32, width: i32, height: i32) -> Option<&'static [i32]> {
    // As listas precisam de tempo de vida estático, e todas são constantes.
    const ONE: [i32; 1] = [1];
    const EIGHT: [i32; 1] = [8];
    const SIXTEEN: [i32; 1] = [16];
    const ZERO: [i32; 1] = [0];
    const FOUR: [i32; 1] = [4];
    const FIVE: [i32; 1] = [5];
    const SIX: [i32; 1] = [6];
    const MAX_TEXTURE: [i32; 1] = [MAX_TEXTURE_SIZE];
    const MAX_ELEMENTS: [i32; 1] = [65_535];
    Some(match name {
        GL_MAX_TEXTURE_SIZE => &MAX_TEXTURE,
        GL_MAX_TEXTURE_UNITS => &ONE,
        GL_MAX_LIGHTS => &EIGHT,
        GL_MAX_MODELVIEW_STACK_DEPTH
        | GL_MAX_PROJECTION_STACK_DEPTH
        | GL_MAX_TEXTURE_STACK_DEPTH => &SIXTEEN,
        // O único nome com dois valores, e o único que depende da tela.
        GL_MAX_VIEWPORT_DIMS => VIEWPORT_DIMS.get_or_init(|| [width, height]).as_slice(),
        GL_SUBPIXEL_BITS => &FOUR,
        GL_RED_BITS | GL_BLUE_BITS => &FIVE,
        GL_GREEN_BITS => &SIX,
        GL_ALPHA_BITS | GL_NUM_COMPRESSED_TEXTURE_FORMATS => &ZERO,
        GL_DEPTH_BITS => &SIXTEEN,
        GL_STENCIL_BITS => &EIGHT,
        GL_MAX_ELEMENTS_VERTICES | GL_MAX_ELEMENTS_INDICES => &MAX_ELEMENTS,
        _ => return None,
    })
}

/// O maior tamanho de viewport, preenchido na primeira consulta porque depende da tela.
static VIEWPORT_DIMS: std::sync::OnceLock<[i32; 2]> = std::sync::OnceLock::new();

/// Converte um `GLfixed` (16.16 com sinal) para `f32`.
pub fn fixed(value: u32) -> f32 {
    value as i32 as f32 / 65536.0
}

#[cfg(test)]
mod fixed_tests {
    use super::fixed;

    #[test]
    fn ponto_fixo_e_16_16_com_sinal() {
        assert_eq!(fixed(0x0001_0000), 1.0);
        assert_eq!(fixed(0x0000_8000), 0.5);
        assert_eq!(fixed(0xffff_0000), -1.0);
        assert_eq!(fixed(0), 0.0);
    }
}

/// Formatos comprimidos do `GL_AMD_compressed_ATC_texture` — a compressão do Adreno.
pub const GL_ATC_RGB_AMD: u32 = 0x8c92;
pub const GL_ATC_RGBA_EXPLICIT_ALPHA_AMD: u32 = 0x8c93;

/// Formatos do `OES_compressed_paletted_texture`, parte do núcleo do OpenGL ES 1.1.
pub const GL_PALETTE4_RGB8_OES: u32 = 0x8b90;
pub const GL_PALETTE4_RGBA8_OES: u32 = 0x8b91;
pub const GL_PALETTE4_R5_G6_B5_OES: u32 = 0x8b92;
pub const GL_PALETTE4_RGBA4_OES: u32 = 0x8b93;
pub const GL_PALETTE4_RGB5_A1_OES: u32 = 0x8b94;
pub const GL_PALETTE8_RGB8_OES: u32 = 0x8b95;
pub const GL_PALETTE8_RGBA8_OES: u32 = 0x8b96;
pub const GL_PALETTE8_R5_G6_B5_OES: u32 = 0x8b97;
pub const GL_PALETTE8_RGBA4_OES: u32 = 0x8b98;
pub const GL_PALETTE8_RGB5_A1_OES: u32 = 0x8b99;
