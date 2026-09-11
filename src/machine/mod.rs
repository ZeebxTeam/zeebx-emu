//! O laço de execução: roda o módulo, atende as chamadas de API e continua.
//!
//! Quando o guest chama um método, o núcleo para com [`StopReason::ApiCall`]. Aqui decidimos
//! o que aquela chamada significa, escrevemos o retorno em `r0` e retomamos a execução em `lr`
//! — que é onde o `bl`/`blx` original deixou o endereço de retorno.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::aee::{self, Interface};
use crate::aee_helpers;
use crate::atc;
use crate::cformat::{self, ArgSource};
use crate::cpu::unicorn::RETURN_MAGIC;
use crate::cpu::{CpuBackend, CpuError, Reg, StopReason};
use crate::crypto;
use crate::display::{Framebuffer, Rect, Rgb};
use crate::fmath;
use crate::gles;
use crate::heap::Heap;
use crate::input::{self, Pad};
use crate::loader::{self, LoadedModule};
use crate::objects::ObjectStore;
use crate::paltex;
use crate::ponte;
use crate::rasterizer::{self, GlState, Vertex};
use crate::rede;
use crate::vfs::Vfs;

mod bitmap;
mod cifra;
mod display;
mod egl;
mod file;
mod gl;
mod helper;
mod hid;
mod image;
mod media;
mod net;
mod probe;
mod shell;
mod signal;
mod sound;
mod sql;
mod thread;
mod time;
mod widget;
mod zip;

/// Teto para leitura de string do guest, contra ponteiro corrompido.
/// Teto de uma instrução SQL vinda do guest. As maiores do Z-Wheel têm 110 bytes.
const MAX_SQL: usize = 4096;
const MAX_STRING: usize = 4096;

/// Teto para as conversões de número (`strtoul`, `atoi`, `strtod`).
///
/// Elas leem de um ponteiro que costuma apontar para o meio de um texto grande, e todo byte
/// além do número é lido à toa. Nenhuma representação decimal, hexadecimal ou de ponto
/// flutuante que caiba em 32 ou 64 bits chega perto disto, contando o espaço em branco à
/// frente e o sinal.
const MAX_NUMBER: usize = 64;
/// `AEE_ENC_ISOLATIN1`, de `AEEShell.h`. É a codificação que o BREW usa para `char *` na
/// maioria dos aparelhos; o Zeebo é brasileiro e usa texto acentuado, então é a aposta certa
/// até que algum jogo mostre o contrário.
const AEE_ENC_ISOLATIN1: u16 = 3;

/// Quantos itens de cor o `AEEClrItem` define, mais o índice zero que não é usado.
const CLR_COUNT: usize = 17;
/// `RGB_NONE`: "use a cor corrente", e não "não pinte".
///
/// A diferença decide se a tela é limpa: o `IDISPLAY_ClearScreen` do SDK é escrito como
/// `DrawRect(NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)`, ou seja, preencha a superfície inteira
/// com a cor de fundo corrente. Enquanto `RGB_NONE` valia "não pinte", essa chamada não fazia
/// nada — e o Tekken 2, que limpa a tela uma vez por quadro, ficava com o texto da tela
/// anterior por baixo do menu.
const RGB_NONE: u32 = 0xffff_ffff;

/// `CLR_USER_BACKGROUND`, o item de cor que o preenchimento usa quando vem `RGB_NONE`.
const CLR_USER_BACKGROUND: usize = 2;

/// `CLR_USER_LINE`, o item de cor da moldura.
const CLR_USER_LINE: usize = 3;

/// `IDF_RECT_FRAME` e `IDF_RECT_FILL`, de `AEEDisp.h`: o que a chamada quer desenhar.
const IDF_RECT_FRAME: u32 = 1;
const IDF_RECT_FILL: u32 = 2;
/// Valores do enum `AEERasterOp`, de `inc/AEERasterOp.h`: `OR`, `XOR`, `COPY`, `NOT`,
/// `OLDMASK`, `MERGENOT`, `ANDNOT`, `TRANSPARENT`, `AND`, `BLEND`.
const AEE_RO_XOR: u32 = 1;
#[cfg_attr(not(test), allow(dead_code))]
const AEE_RO_COPY: u32 = 2;
const AEE_RO_TRANSPARENT: u32 = 7;

/// Cores iniciais: texto preto sobre fundo branco, como um aparelho BREW padrão.
fn default_colors() -> [Rgb; CLR_COUNT] {
    let mut colors = [Rgb::WHITE; CLR_COUNT];
    // CLR_USER_TEXT = 1, CLR_USER_LINE = 3.
    colors[1] = Rgb::BLACK;
    colors[3] = Rgb::BLACK;
    colors
}

/// Traduz uma comparação para o inteiro que as funções `str*cmp` devolvem.
fn cmp_to_int(ordering: std::cmp::Ordering) -> u32 {
    match ordering {
        std::cmp::Ordering::Less => (-1i32) as u32,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

/// Lê o maior prefixo de `text` que forma um número, como o `strtod` do C.
///
/// Devolve o valor e quantos bytes foram consumidos — o `strtod` precisa do segundo para
/// preencher o `char **ppszEnd`.
fn parse_leading_double(text: &str) -> (f64, usize) {
    let bytes = text.as_bytes();
    let mut end = 0;
    let mut best = None;
    // O espaço em branco inicial é ignorado, e daí em diante vale o maior prefixo que o
    // parser do Rust aceitar: é o mesmo resultado do `strtod`, sem reimplementar a gramática.
    let start = bytes.iter().take_while(|b| b.is_ascii_whitespace()).count();
    for cut in start + 1..=bytes.len() {
        if let Ok(value) = text[start..cut].parse::<f64>() {
            best = Some(value);
            end = cut;
        }
    }
    match best {
        Some(value) => (value, end),
        None => (0.0, 0),
    }
}

/// Procura `needle` dentro de `hay` e devolve o índice do começo.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Reduz uma string larga a minúsculas ASCII, truncada em `take` caracteres.
///
/// É o que as comparações `wstricmp`/`wstrnicmp` precisam: o BREW só dobra o caso do ASCII.
fn fold_case(units: &[u16], take: usize) -> Vec<u16> {
    units
        .iter()
        .take(take)
        .map(|&u| match u8::try_from(u) {
            Ok(b) => b.to_ascii_lowercase() as u16,
            Err(_) => u,
        })
        .collect()
}

/// Lê o maior prefixo de `text` que forma um inteiro sem sinal na base pedida.
///
/// Base 0 significa deduzir do prefixo, como no `strtoul` do C: `0x` é hexadecimal, `0` é
/// octal e o resto é decimal. Devolve o valor e quantos bytes foram consumidos.
fn parse_unsigned(text: &str, base: u32) -> (u32, usize) {
    let start = text.len() - text.trim_start().len();
    let rest = &text[start..];
    let (digits, base) = match base {
        0 if rest.starts_with("0x") || rest.starts_with("0X") => (&rest[2..], 16),
        16 if rest.starts_with("0x") || rest.starts_with("0X") => (&rest[2..], 16),
        0 if rest.starts_with('0') && rest.len() > 1 => (&rest[1..], 8),
        0 => (rest, 10),
        base => (rest, base),
    };
    let taken = digits.chars().take_while(|c| c.is_digit(base)).count();
    let value = u32::from_str_radix(&digits[..taken], base).unwrap_or(0);
    let consumed = if taken == 0 {
        0
    } else {
        text.len() - digits.len() + taken
    };
    (value, consumed)
}

/// Tamanho em bytes de um componente de vetor.
fn component_size(kind: u32) -> u32 {
    match kind {
        gles::GL_BYTE | gles::GL_UNSIGNED_BYTE => 1,
        gles::GL_SHORT | gles::GL_UNSIGNED_SHORT => 2,
        _ => 4,
    }
}

/// Tamanho em bytes de um texel, conforme o par formato/tipo do `TexImage2D`.
fn bytes_per_texel(format: u32, kind: u32) -> u32 {
    match kind {
        gles::GL_UNSIGNED_BYTE => match format {
            gles::GL_RGB => 3,
            gles::GL_RGBA => 4,
            gles::GL_LUMINANCE_ALPHA => 2,
            _ => 1,
        },
        // Os três tipos compactos do OpenGL ES são todos de 16 bits.
        _ => 2,
    }
}

/// Converte os texels para RGBA de 8 bits, o formato único do rasterizador.
fn decode_texels(bytes: &[u8], format: u32, kind: u32, count: usize) -> Vec<[u8; 4]> {
    // Repetir os cinco bits mais altos nos três de baixo espalha o valor por toda a faixa: é o
    // que faz 0b11111 virar 255 e não 248.
    let expand = |value: u16, bits: u32| -> u8 {
        let max = (1u16 << bits) - 1;
        ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
    };
    let size = bytes_per_texel(format, kind) as usize;
    (0..count)
        .map(|i| {
            let at = i * size;
            if at + size > bytes.len() {
                return [255; 4];
            }
            match kind {
                gles::GL_UNSIGNED_SHORT_5_6_5 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 11, 5),
                        expand((v >> 5) & 0x3f, 6),
                        expand(v & 0x1f, 5),
                        255,
                    ]
                }
                gles::GL_UNSIGNED_SHORT_4_4_4_4 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 12, 4),
                        expand((v >> 8) & 0xf, 4),
                        expand((v >> 4) & 0xf, 4),
                        expand(v & 0xf, 4),
                    ]
                }
                gles::GL_UNSIGNED_SHORT_5_5_5_1 => {
                    let v = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
                    [
                        expand(v >> 11, 5),
                        expand((v >> 6) & 0x1f, 5),
                        expand((v >> 1) & 0x1f, 5),
                        if v & 1 != 0 { 255 } else { 0 },
                    ]
                }
                _ => match format {
                    gles::GL_RGB => [bytes[at], bytes[at + 1], bytes[at + 2], 255],
                    gles::GL_RGBA => [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]],
                    gles::GL_LUMINANCE_ALPHA => {
                        let l = bytes[at];
                        [l, l, l, bytes[at + 1]]
                    }
                    gles::GL_ALPHA => [255, 255, 255, bytes[at]],
                    gles::GL_LUMINANCE => [bytes[at], bytes[at], bytes[at], 255],
                    // Formato que não conhecemos: um canal só, opaco. Vira cinza, e cinza é
                    // visível — errar assim aparece na tela em vez de virar um buraco preto.
                    _ => [bytes[at], bytes[at], bytes[at], 255],
                },
            }
        })
        .collect()
}

/// A classe que sabe abrir um tipo MIME, para o `ISHELL_GetHandler`.
///
/// No console isso sai do registro do BREW, montado a partir dos `.mif` instalados. Aqui a
/// tabela é fixa e cobre o que os jogos pedem; zero significa "ninguém sabe abrir isso", que é
/// a resposta prevista na documentação.
fn handler_for(mime: &str) -> u32 {
    match mime {
        "image/png" => AEECLSID_PNG,
        "image/jpeg" => AEECLSID_JPEG,
        "image/gif" => AEECLSID_GIF,
        "image/bmp" | "image/x-ms-bmp" => AEECLSID_WINBMP,
        "audio/mid" | "audio/midi" => AEECLSID_MEDIAMIDI,
        "audio/mpeg" | "audio/mp3" => AEECLSID_MEDIAMP3,
        "audio/wav" | "audio/x-wav" => AEECLSID_MEDIAPCM,
        "audio/vnd.qcelp" => AEECLSID_MEDIAADPCM,
        _ => 0,
    }
}

/// Descobre o tipo MIME de um conteúdo, pelo começo dos bytes ou pela extensão do nome.
///
/// É o que o `ISHELL_DetectType` responde. O reconhecimento por assinatura vem primeiro porque
/// é o que a documentação manda: "pBuf tem precedência sobre pszName".
fn detect_mime(bytes: &[u8], name: &str) -> Option<&'static str> {
    let starts = |magic: &[u8]| bytes.starts_with(magic);
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if starts(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some("image/gif");
    }
    if starts(b"BM") {
        return Some("image/bmp");
    }
    if starts(b"MThd") {
        return Some("audio/mid");
    }
    if starts(b"ID3") || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0) {
        return Some("audio/mpeg");
    }
    if starts(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WAVE" {
        return Some("audio/wav");
    }
    if starts(b"#!AMR") {
        return Some("audio/amr");
    }

    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "mid" | "midi" => "audio/mid",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "amr" => "audio/amr",
        "qcp" => "audio/vnd.qcelp",
        "txt" => "text/plain",
        _ => return None,
    })
}

/// Empacota uma cor no `RGBVAL` que o guest espera de volta.
fn to_rgbval(color: Rgb) -> u32 {
    (color.r as u32) << 8 | (color.g as u32) << 16 | (color.b as u32) << 24
}

/// Identificadores do gamepad, do descritor USB capturado do console
/// (`docs/vendor/tripleoxygen/hardware/peripheral/joystick_descriptor.txt`): "My Power / Usb
/// Game Pad".
const GAMEPAD_VENDOR_ID: u16 = 0x1eaa;
const GAMEPAD_PRODUCT_ID: u16 = 0x0135;
/// Identificador de uma porta na enumeração: `1` e `2`, na ordem das portas.
///
/// O `CreateDevice` recebe este número de volta, e é por ele que sabemos de qual porta o
/// aparelho que o jogo acabou de criar vai ler.
const fn handle_da_porta(porta: usize) -> u32 {
    porta as u32 + 1
}
const HID_TYPE_GAMEPAD: u32 = 1;
const HID_STATUS_CONNECTED: u32 = 1;
/// `AEEUID_HID_Joystick_Device`, de `AEEHIDDevice_Joystick.h`: o tipo de dispositivo que os
/// jogos pedem em `GetConnectedDevices`.
const UID_JOYSTICK_DEVICE: u32 = 0x0106_c3fd;
/// O tipo de teclado, vizinho do joystick por um.
///
/// Não veio de header: veio da Z-Wheel. Ela chama o `GetConnectedDevices` **duas** vezes, uma
/// pedindo `0x0106c3fd` e outra pedindo este; quando a segunda volta vazia, a `Joystick.c:183`
/// imprime `No keyboard reported`. Ou seja, o console enumera teclado USB, e a mensagem que a
/// gente via no log era a resposta certa para "não tem nenhum ligado".
const UID_KEYBOARD_DEVICE: u32 = 0x0106_c3fc;
/// O tipo que o `GetDeviceInfo` reporta para um teclado.
const HID_TYPE_KEYBOARD: u32 = 2;
/// `EBADPARM` do BREW.
const EBADPARM: u32 = 2;
/// `AEE_EUNSUPPORTED`, de `AEEStdErr.h`: a API existe, mas não para este item.
/// Leituras seguidas do relógio, sem nenhum outro trabalho pelo meio, a partir das quais o
/// jogo é considerado em espera ocupada. O limite deixa passar de graça as leituras que fazem
/// parte do trabalho normal de um quadro.
const SPIN_THRESHOLD: u32 = 64;

/// Quanto o relógio adianta a cada leitura, uma vez reconhecida a espera. É também o quanto ele
/// pode ultrapassar o prazo pelo qual o jogo espera.
const SPIN_STEP_US: u64 = 250;

const EUNSUPPORTED: u32 = 20;
/// `AEE_EBUFFERTOOSMALL`, de `AEEStdErr.h`.
const EBUFFERTOOSMALL: u32 = 38;
/// `AEE_DEVICEITEM_IMEI`, de `AEEDeviceItems.h`: o identificador do aparelho em ASCII.
const DEVICEITEM_IMEI: u32 = 28;
/// O IMEI que respondemos. É sintético — não conhecemos o de nenhum Zeebo —, mas tem os 15
/// dígitos e o dígito verificador de Luhn certos, porque quem pede um IMEI costuma conferir.
const IMEI: &[u8] = b"350000000000006\0";
/// Modos de `IFILEMGR_OpenFile`, de `AEEFile.h`.
const OFM_READWRITE: u32 = 0x0002;
const OFM_CREATE: u32 = 0x0004;
const OFM_APPEND: u32 = 0x0008;
/// Tipos de `IFILE_Seek`, de `AEEFile.h`.
const SEEK_END: u32 = 1;
const SEEK_CURRENT: u32 = 2;
/// Atributos de arquivo.
const FA_NORMAL: u32 = 0x00;
const FA_DIR: u32 = 0x02;
/// `FALSE` do BREW — o `boolean` dele é um `uint8`.
const FALSE: u32 = 0;
/// `AEECLSID_PNG`, de `sdk/inc/AEEPNG.bid` — o decodificador de PNG, exposto como `IImage`.
const AEECLSID_PNG: u32 = 0x0100_4004;
/// `AEECLSID_PNGDecoder` e `AEECLSID_PNGDecoderBREW`, de `inc/AEEPNGDecoder*.bid`. A interface
/// padrão das duas é `IImageDecoder`.
const AEECLSID_PNGDECODER: u32 = 0x0102_6e23;
const AEECLSID_PNGDECODER_BREW: u32 = 0x0103_0766;
/// `IPARM_*` de `inc/AEEIImage.h`.
const IPARM_CXFRAME: u32 = 2;
const IPARM_NFRAMES: u32 = 4;
const IPARM_GETBITMAP: u32 = 10;
/// `AEECLSID_MEMASTREAM` = `AEECLSID_CORE + 12`, de `sdk/inc/AEEClassIDs.h`.
const AEECLSID_MEMASTREAM: u32 = 0x0100_100c;
/// `AEECLSID_LICENSE` = `AEECLSID_CORE + 15`, de `sdk/inc/AEEClassIDs.h`.
const AEECLSID_LICENSE: u32 = 0x0100_100f;
/// `LT_NONE` e `PT_PURCHASE`, de `inc/AEELicenseInfo.h`: módulo sem expiração, comprado.
const LT_NONE: u32 = 0;
const PT_PURCHASE: u32 = 2;
/// `BV_UNLIMITED`, de `inc/AEELicenseInfo.h`: o valor que significa "nunca expira".
const BV_UNLIMITED: u32 = 0xffff_ffff;
/// Classes de mídia, de `AEECLSID_MULTIMEDIA` (`QVERSION + 0x5500`) em `sdk/inc/AEEClassIDs.h`.
const AEECLSID_MEDIA: u32 = 0x0100_5500;
const AEECLSID_MEDIAMIDI: u32 = 0x0100_5501;
const AEECLSID_MEDIAMP3: u32 = 0x0100_5502;
const AEECLSID_MEDIAADPCM: u32 = 0x0100_550a;
/// `AEECLSID_MEDIAUTIL` = `AEECLSID_MULTIMEDIA + 13` — a fábrica dos objetos de mídia.
const AEECLSID_MEDIAUTIL: u32 = 0x0100_550d;
const AEECLSID_MEDIAPCM: u32 = 0x0100_5511;
/// Visualizadores de imagem, de `AEECLSID_VIEW` (`QVERSION + 0x4000`).
const AEECLSID_WINBMP: u32 = 0x0100_4001;
const AEECLSID_GIF: u32 = 0x0100_4003;
const AEECLSID_JPEG: u32 = 0x0100_4005;
/// Parâmetros de `IMEDIA_SetMediaParm`, na ordem da tabela `MM_PARM_XXX` da documentação do
/// BREW — os headers do SDK 4.0.2 não trazem os valores, só a tabela traz a ordem. Dois deles
/// se confirmam no que os jogos fazem: o 1 recebe ponteiro para um `AEEMediaData`, e o 4 recebe
/// números entre 0 e 100.
const MM_PARM_MEDIA_DATA: u32 = 1;
const MM_PARM_VOLUME: u32 = 4;
const MM_PARM_MUTE: u32 = 5;
const MM_PARM_PLAY_REPEAT: u32 = 11;

/// `AEE_MAX_VOLUME`. O valor não está em nenhum header que temos, e saiu do que os jogos usam:
/// o Double Dragon manda 0 e 100, o Quake manda 70 e 80, e nada em nenhuma ROM passa de 100.
const MAX_VOLUME: u32 = 100;

/// `MMD_BUFFER`, o `clsData` de um `AEEMediaData` que aponta para memória. Também veio da
/// observação: é o que os três jogos que tocam som passam.
const MMD_BUFFER: u32 = 1;

/// Comandos e status de `IMedia`, de `inc/AEEIMedia.h` do SDK do BREW 4.0.2.
const MM_CMD_PLAY: u32 = 4;
const MM_STATUS_START: u32 = 1;
const MM_STATUS_DONE: u32 = 2;

/// Tamanho do `AEEMediaCmdNotify`: `clsMedia`, `pIMedia`, `nCmd`, `nSubCmd`, `nStatus`,
/// `pCmdData` e `dwSize`, sete palavras.
const MEDIA_NOTIFY_LEN: u32 = 28;

/// Estados de `IMEDIA_GetState`, de `inc/AEEIMedia.h`.
const MM_STATE_READY: u32 = 2;
const MM_STATE_PLAY: u32 = 3;
const MM_STATE_PLAY_PAUSE: u32 = 5;

/// `AEECLSID_QEGL`, do `AEECLSID_QEGL.bid` do SDK: o objeto que dá acesso ao EGL e ao OpenGL
/// ES pelas interfaces novas do BREW. É por ele que o Quake tenta primeiro.
const AEECLSID_QEGL: u32 = 0x0103_d8ec;
/// `AEECLSID_EGL` e `AEECLSID_GL`, de `sdk/inc/AEEGL.h` — as interfaces antigas, que o Crash
/// Nitro Kart pede direto.
const AEECLSID_EGL: u32 = 0x0101_4bc4;
/// `AEECLSID_WEB`, do `BMPIds.csv` do SDK: o cliente HTTP do BREW.
const AEECLSID_WEB: u32 = 0x0100_5000;
/// Quantos blocos de texto claro o registro guarda, e quanto de cada um.
const PLAINTEXT_MAX: usize = 8;
const PLAINTEXT_BYTES: usize = 512;

/// As chamadas de GL que não fazemos **de propósito**, e que por isso não entram no relatório.
///
/// São estado que o nosso rasterizador não usa — profundidade, névoa, luz, stencil. Listá-las
/// junto das que faltam esconderia as que importam no meio do ruído.
/// Guarda um nível de uma textura, criando a cadeia de redução conforme ela chega.
///
/// **O nível zero limpa os menores.** Uma imagem nova no nível base torna a cadeia antiga
/// mentira, e servir um mipmap de outra textura é pior do que não ter nenhum.
fn guarda_nivel(
    texture: &mut crate::rasterizer::Texture,
    level: u32,
    width: usize,
    height: usize,
    pixels: Vec<[u8; 4]>,
) {
    if level == 0 {
        texture.width = width;
        texture.height = height;
        texture.pixels = pixels;
        texture.mipmaps.clear();
        return;
    }
    let indice = level as usize - 1;
    if texture.mipmaps.len() <= indice {
        texture
            .mipmaps
            .resize_with(indice + 1, || crate::rasterizer::Nivel {
                width: 0,
                height: 0,
                pixels: Vec::new(),
            });
    }
    texture.mipmaps[indice] = crate::rasterizer::Nivel {
        width,
        height,
        pixels,
    };
}

/// Uma palavra do jogo como número: ponto fixo 16.16 nas formas `x`, `float` nas formas `f`.
fn escalar(palavra: u32, fixo: bool) -> f32 {
    match fixo {
        true => gles::fixed(palavra),
        false => f32::from_bits(palavra),
    }
}

const ATENDIDAS_EM_SILENCIO: &[&str] = &[
    "DepthFunc",
    "DepthRangef",
    "DepthRangex",
    "Fogf",
    "Fogfv",
    "Fogx",
    "Fogxv",
    "Hint",
    "LineWidth",
    "LineWidthx",
    "PolygonOffset",
    "PolygonOffsetx",
    "SampleCoverage",
    "SampleCoveragex",
    "Flush",
    "Finish",
];

/// A marca de "o fluxo acabou", que o `ConnectionManager` liga ao receber um pedaço vazio.
const FIM_DO_FLUXO: u32 = 0x1a;

/// Onde o objeto de resposta guarda o texto que vai fatiar, visto no parser em `0x85d18`.
const TEXTO_DA_RESPOSTA: u32 = 0x20;

/// Teto de instruções para uma chamada pela ponte. O alocador é uma função curta.
const PONTE_BUDGET: u64 = 5_000_000;
/// A linha que acompanha o nome de arquivo no rastreio do alocador. Ele só guarda para os
/// relatórios dele, então qualquer valor serve; um distinto ajuda a reconhecer o que veio daqui.
const LINHA_DE_ORIGEM: u32 = 0;

/// Os estados da conexão, lidos do `switch` da tela de sync do Zeeboids em `0x95bb8`.
///
/// Ela lê o campo toda volta: `0` e `2` mantêm o "Connecting", `9` leva ao "ReceivingData" e
/// `11` ao ramo de falha, ao lado da string `Connection_Failed`.
const ESTADO_TRABALHANDO: u32 = 0;
const ESTADO_TRABALHANDO_2: u32 = 2;
const ESTADO_RECEBENDO: u32 = 9;
const ESTADO_FALHOU: u32 = 11;

/// Onde fica o estado, em relação à URL: doze bytes antes dela, no mesmo objeto.
const OFFSET_ESTADO_ANTES_DA_URL: u32 = 0xc;

/// Até onde procurar o trio `{url, corpo, tamanho}` no objeto de quem pediu o envio.
///
/// O do Zeeboids está em `+0x244`; mil bytes cobrem folgadamente objetos desse tamanho sem sair
/// varrendo a memória do jogo.
const MAX_CAMPOS_DO_OBJETO: u32 = 256;

/// Teto do corpo de uma requisição. O remetente do Zeeboids aloca 2 KB, que é o tamanho que ele
/// mesmo se dá; este teto é generoso o bastante para não cortar nada real.
const MAX_CORPO_ENVIADO: u32 = 1 << 16;

/// Quantos toques o registro guarda.
///
/// Quarenta cobriam uma reprodução curta e não cobrem uma longa: navegar até a importação,
/// digitar um ZID e uma senha passa disso com folga, e o que sobrava era a cauda — inútil para
/// reproduzir, porque o começo é justamente o que leva o jogo ao estado certo. Com este limite,
/// um registro vira um roteiro de `--keys` completo, e uma sessão de teste do usuário rende
/// quantas repetições eu precisar aqui.
const PAD_LOG_MAX: usize = 400;

/// O estado de um `IPeek`: os bytes da fonte, onde a leitura está e onde a linha é montada.
struct Peek {
    bytes: Vec<u8>,
    posicao: usize,
    /// Endereço, na memória do guest, do buffer de uma linha. Ver [`Machine::source_call`].
    buffer: u32,
}

impl Peek {
    /// A próxima linha, sem o `\n` e sem o `\r` que o acompanha nos arquivos do console.
    ///
    /// Devolve `None` quando acabou. Uma linha vazia é uma linha: quem separa "linha vazia" de
    /// "fim do arquivo" é o `Option`, não o tamanho — o `tectoy.cfg` tem linhas em branco entre
    /// as seções, e confundir as duas coisas pararia a leitura na primeira delas.
    fn proxima_linha(&mut self) -> Option<Vec<u8>> {
        if self.posicao >= self.bytes.len() {
            return None;
        }
        let resto = &self.bytes[self.posicao..];
        let fim = resto.iter().position(|&b| b == b'\n');
        let linha = match fim {
            Some(n) => {
                self.posicao += n + 1;
                &resto[..n]
            }
            None => {
                self.posicao = self.bytes.len();
                resto
            }
        };
        Some(match linha.last() {
            Some(b'\r') => linha[..linha.len() - 1].to_vec(),
            _ => linha.to_vec(),
        })
    }
}

/// `AEECLSID_SOURCEUTIL`, a fábrica de `ISource`. Ver [`Interface::SourceUtil`] para como o
/// número foi identificado — durante muito tempo ele esteve aqui com o nome errado.
const AEECLSID_SOURCEUTIL: u32 = 0x0100_1011;

/// `AEECLSID_MD5`: o resumo MD5, exposto como `IHash`.
const AEECLSID_MD5: u32 = 0x0100_1015;
/// `AEECLSID_CipherFactory`, de `inc/AEECipherFactory.bid`.
const AEECLSID_CIPHER_FACTORY: u32 = 0x0102_cce1;
const AEECLSID_GL: u32 = 0x0101_4bc3;
/// IIDs que o `QueryInterface` do objeto do EGL atende, dos headers de cada interface.
const AEEIID_GLES10: u32 = 0x0103_d8dd;
const AEEIID_GLES11: u32 = 0x0103_d8ea;
/// `AEEIID_EGLSURFACEMANIP_V1` e `AEEIID_EGLSURFACEMANIP`, de `sdk/inc/AEEEGLSurfaceManip.h`.
const AEEIID_EGL_SURFACE_MANIP_V1: u32 = 0x0104_34cc;
const AEEIID_EGL_SURFACE_MANIP: u32 = 0x0105_1834;
/// `AEEIID_GLESIMAGEONEXT_V1` e `AEEIID_GLESIMAGEONEXT`, de `sdk/inc/AEEGLESImageonEXT.h`.
const AEEIID_GLES_IMAGEON_EXT_V1: u32 = 0x0104_59b1;
const AEEIID_GLES_IMAGEON_EXT: u32 = 0x0105_8546;
const AEEIID_EGL10: u32 = 0x0103_d8ed;
const AEEIID_EGL11: u32 = 0x0103_d8ee;
/// Identificador do display do EGL. Só existe um, e o valor é arbitrário — o que não pode é
/// ser zero, que é `EGL_NO_DISPLAY`.
const EGL_DISPLAY: u32 = 1;
/// A única `EGLConfig` que oferecemos, casando com o `EGL_CONFIG_ID` de [`gles::config_attrib`].
const EGL_CONFIG: u32 = 1;
/// Primeiro identificador de superfície e de contexto. Fica longe do display e da configuração
/// para que confundir um com o outro apareça na hora.
const EGL_HANDLE_BASE: u32 = 0x100;

/// `AEECLSID_THREAD` = `AEECLSID_CORE + 23`, de `sdk/inc/AEEClassIDs.h`.
const AEECLSID_THREAD: u32 = 0x0100_1017;
/// `EALREADY`: a thread já foi iniciada uma vez, e `IThread` não é reutilizável.
const EALREADY: u32 = 8;
/// Registradores que formam o contexto de uma thread cooperativa.
///
/// O `lr` fica de fora de propósito: para retomar uma thread o que importa é onde ela parou,
/// e isso guardamos separado, no `resume_pc`.
const THREAD_REGS: [Reg; 14] = [
    Reg::R0,
    Reg::R1,
    Reg::R2,
    Reg::R3,
    Reg::R4,
    Reg::R5,
    Reg::R6,
    Reg::R7,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::Sp,
];
/// Tamanho do `AEECallback` do BREW (`inc/AEECallback.h`), em bytes.
const CALLBACK_SIZE: u32 = 28;
/// Piso para a pilha de uma thread, caso o jogo peça um valor pequeno demais.
const THREAD_MIN_STACK: u32 = 64 * 1024;

/// `AEECLSID_SOUND` = `AEECLSID_CORE + 86`, confirmado pelo `COMPILE_ASSERT` de `AEEClassIDs.h`.
const AEECLSID_SOUND: u32 = 0x0100_1056;
const AEECLSID_HEAP: u32 = 0x0100_1002;
/// `AEECLSID_UNZIPSTREAM`, de `sdk/inc/AEEClassIDs.h`.
const AEECLSID_UNZIPSTREAM: u32 = 0x0100_1014;

/// Métricas da fonte que o `GetFontMetrics` reporta. Não desenhamos texto ainda; o que os
/// jogos precisam daqui é de números coerentes para medir e posicionar.
/// O que um decodificador de imagem juntou até agora.
#[derive(Default)]
struct DecoderState {
    /// O arquivo, montado pedaço a pedaço pelo `IForceFeed::Write`.
    fed: Vec<u8>,
    /// O bitmap pronto, criado no primeiro `GetBitmap`.
    bitmap: Option<u32>,
    /// Se a imagem tem transparência — decide o `GetRop`.
    transparent: bool,
}

/// Teto de uma espera. Um valor absurdo — um jogo que peça horas por engano — não pode levar o
/// relógio junto.
const MAX_SLEEP_MS: u32 = 10_000;

/// Em qual argumento cada método da manipulação de superfície guarda o `AEEEGLBoolean *ret`.
/// A contagem inclui o `this`, então o primeiro parâmetro do método é o índice 1.
const EXTENSION_RESULT_SLOT: [(&str, usize); 11] = [
    ("SurfaceScaleEnable", 4),
    ("SurfaceRotateEnable", 4),
    ("SetSurfaceRotate", 6),
    ("SurfaceTransparencyEnable", 4),
    ("SetSurfaceTransparency", 4),
    ("SetSurfaceTransparencyMap", 4),
    ("SurfaceColorKeyEnable", 4),
    ("SetSurfaceColorKey", 6),
    ("SurfaceOverlayEnable", 4),
    ("SurfaceOverlayLayerEnable", 5),
    ("SurfaceOverlayBind", 5),
];

/// Teto do que um decodificador aceita, para um jogo que escreve sem parar não consumir a
/// memória do host.
const MAX_DECODED_INPUT: usize = 16 * 1024 * 1024;

/// `AEEIID_FORCEFEED`. O valor não está nos headers que temos; ele foi identificado pelo uso —
/// é a interface que o `.bid` do `AEECLSID_PNGDecoderBREW` declara suportar, e é a que o Heavy
/// Weapon pede ao decodificador antes de buscar o bitmap.
const AEEIID_FORCEFEED: u32 = 0x0101_eb0b;

/// Teto de uma imagem convertida para o formato nativo, para um cabeçalho estragado não pedir
/// memória demais.
const MAX_NATIVE_IMAGE: usize = 8 * 1024 * 1024;

/// Teto de um registro de preferências, para um tamanho absurdo não pedir memória demais.
const MAX_PREFS: usize = 64 * 1024;

/// Quanto cada caractere avança na horizontal, em pixels.
///
/// Não desenhamos texto, então este número só serve para o jogo posicionar o que ele mesmo
/// desenha. Ele acompanha a altura declarada em [`FONT_ASCENT`]: uma fonte de tela pequena,
/// coerente consigo mesma.
const FONT_ADVANCE: i32 = 7;
/// Corpo em que o texto é desenhado quando há fonte. O console tinha tamanhos nomeados
/// (`AEE_FONT_NORMAL` e companhia) e um `fontsize.map` que dizia quantos pixels cada um vale;
/// esse arquivo não veio no pacote, então por ora há um tamanho só.
const FONT_SIZE: f32 = 16.0;
/// `CLR_USER_TEXT`, o item de cor que o `IDISPLAY_SetColor` usa para texto.
const CLR_USER_TEXT: usize = 1;

const FONT_ASCENT: u32 = 12;
const FONT_DESCENT: u32 = 4;
/// `AEE_MAX_VOLUME`, de `inc/AEEISound.h`.
const AEE_MAX_VOLUME: u16 = 100;
/// `AEE_SOUND_SUCCESS` e `AEE_SOUND_PLAY_DONE`, do enum `AEESoundStatus`.
const AEE_SOUND_SUCCESS: u32 = 1;
const AEE_SOUND_PLAY_DONE: u32 = 2;
/// `AEE_SOUND_STATUS_CB` e `AEE_SOUND_VOLUME_CB`, do enum `AEESoundCmd`.
const AEE_SOUND_STATUS_CB: u32 = 0;
const AEE_SOUND_VOLUME_CB: u32 = 1;
/// `AEEIID_DIB_20`, de `inc/AEEIDIB.h`: o IID que o `IDIB` tinha no BREW 2.0.
const AEEIID_DIB_20: u32 = 0x0100_102c;
/// O terceiro IID do `IDIB`, vizinho do anterior na mesma faixa do `AEEIDIB.h`.
///
/// É o que o Zenonia pede — e só ele, em todo o acervo. O jogo cria a superfície com
/// `CreateCompatibleBitmap`, pede este IID nela, guarda o ponteiro e passa a escrever os pixels
/// direto. Recusando, ele guardava nulo, seguia assim mesmo e apresentava 258 quadros de uma
/// superfície vazia: tela preta com o jogo desenhando o tempo todo.
const AEEIID_DIB_ANTIGO: u32 = 0x0100_1029;
/// `IDIB_COLORSCHEME_565`, de `inc/AEEIDIB.h`: 5 bits de vermelho, 6 de verde, 5 de azul.
const IDIB_COLORSCHEME_565: u8 = 16;
/// `AEECLSID_DIB` = `AEECLSID_CORE + 69`. É o bitmap com acesso direto aos pixels.
const AEECLSID_DIB: u32 = 0x0100_1045;
/// `AEEIID_IBitmap`, de `inc/AEEIBitmap.h`.
const AEEIID_IBITMAP: u32 = 0x0100_1021;
/// Teto de vértices por polígono, contra ponteiro corrompido.
const MAX_POLYGON_POINTS: usize = 4096;
/// `AEE_MAX_FILE_NAME`.
const MAX_FILE_NAME: usize = 64;
/// Espaço que reportamos no cartão. O Zeebo tem 1 GB de NAND; anunciamos algo dessa ordem
/// para que nenhum jogo se recuse a salvar por falta de espaço.
const FS_TOTAL_BYTES: u32 = 512 * 1024 * 1024;
const FS_FREE_BYTES: u32 = 256 * 1024 * 1024;

/// `EFAILED` do BREW: falha genérica. Usado para "não há evento pendente".
const EFAILED: u32 = 1;

/// `ALLOC_NO_ZMEM`, de `AEEStdLib.h`: pede memória sem zerar, embutido no próprio tamanho.
const ALLOC_NO_ZMEM: u32 = 0x8000_0000;

/// Teto de linhas do rastreamento, para um laço não encher a memória.
const MAX_TRACE: usize = 2000;

/// Quantos textos distintos guardar do `IDisplay::DrawText`.
///
/// A lista existe só para o relatório dizer o que o jogo escreveu numa tela que ainda não
/// sabemos desenhar, e um jogo redesenha o mesmo texto a cada quadro: sem teto ela crescia
/// para sempre, uma `String` por chamada, e o emulador ia ficando mais lento quanto mais tempo
/// passasse numa tela com texto.
const MAX_TEXT: usize = 64;

/// Teto de chamadas de API por execução. O orçamento de instruções não segura um laço que
/// chama API a cada volta, porque ele é reiniciado a cada chamada atendida.
/// O teto existe para um laço de repetição não travar o emulador, e é generoso porque um jogo
/// 3D chama a API às centenas de milhares por segundo: só o Quake faz mais de trezentas
/// chamadas de OpenGL por quadro.
const MAX_CALLS: u64 = 200_000_000;

/// Instruções por milissegundo do relógio virtual: o ARM11 do MSM7201A roda a 528 MHz, e o
/// núcleo do unicorn não é superescalar, então uma instrução por ciclo é a conta certa.
const INSTRUCTIONS_PER_US: u64 = 528;

/// Período do retraço vertical da tela do console, em microssegundos — 60 Hz.
///
/// A configuração que oferecemos declara `EGL_MIN_SWAP_INTERVAL` e `EGL_MAX_SWAP_INTERVAL`
/// iguais a 1, que é o que o console faz: o `eglSwapBuffers` espera o retraço. Sem essa espera
/// o Quake desenhava quatrocentos quadros por segundo de tempo virtual, coisa que nenhum
/// aparelho faria, e a lógica dele corria na mesma proporção.
const VSYNC_PERIOD_US: u64 = 1_000_000 / 60;

/// `SUCCESS` do BREW (`AEEError.h`).
const SUCCESS: u32 = 0;
/// `ECLASSNOTSUPPORT`: o ClassID pedido não existe nesta plataforma.
const ECLASSNOTSUPPORT: u32 = 20;
/// `ENOMEMORY`: acabou memória.
const ENOMEMORY: u32 = 3;
/// `ENOTYPE`: não há tipo associado a este conteúdo (`AEEError.h`).
const ENOTYPE: u32 = 34;
/// `ENEEDMORE`: faltam dados para decidir (`AEEError.h`).
const ENEEDMORE: u32 = 35;
/// Quantos bytes bastam para reconhecer os formatos que conhecemos — o maior é o `RIFF`, que
/// precisa dos doze primeiros para confirmar o `WAVE`.
const DETECT_TYPE_BYTES: u32 = 16;
/// `TRUE` do BREW — os helpers que devolvem `boolean` usam 1/0.
const TRUE: u32 = 1;

/// `GAV_LATIN1` de `AEEStdLib.h`: pede a versão como string de um byte, não como `AECHAR`.
const GAV_LATIN1: u32 = 0x0001;
/// Versão do BREW que respondemos, no formato do `GETAEEVERSION`: byte alto da palavra alta é
/// a versão maior, depois menor, sub-versão e build. É o 4.0.2 do SDK que os jogos usam.
const AEE_VERSION: u32 = 0x0400_0200;
const AEE_VERSION_TEXT: &str = "4.0.2.0";
/// Espaço livre que `GetFSFree` informa. O console tem cartão de memória opcional e uma
/// partição de dados; o que importa para os jogos é o número não ser apertado.
const FS_TOTAL: u32 = 64 * 1024 * 1024;

/// ClassIDs que sabemos instanciar.
///
/// Os valores saem de `AEEClassIDs.h`: `AEECLSID_CORE = QVERSION + 0x1000` com
/// `QVERSION = 0x01000000`, então `AEECLSID_DISPLAY = AEECLSID_CORE + 1`.
const AEECLSID_DISPLAY: u32 = 0x0100_1001;

/// `AEECLSID_DISPLAY1`, de `sdk/inc/AEEDisp.h`. As classes `DISPLAY1` a `DISPLAY4` são as telas
/// numeradas de um aparelho com mais de uma, e expõem a **mesma `IDisplay`** da tela padrão —
/// o header é explícito nisso. O Zeebo tem uma tela só, então a primeira é a que existe.
///
/// Três jogos pedem esta classe: Magical Drop 3, Peggle e Pac-Mania. O Magical Drop guardava a
/// recusa num campo e escrevia nele logo depois, num ponteiro nulo.
const AEECLSID_DISPLAY1: u32 = 0x0101_27d4;
/// `AEECLSID_FILEMGR`, do `AEECLSID_FILEMGR.bid` do SDK. No `AEEClassIDs.h` ele aparece só
/// comentado, o que já me fez errar esse valor uma vez.
const AEECLSID_FILEMGR: u32 = 0x0100_1003;
/// O estado de um widget: os filhos, as propriedades e o tamanho.
///
/// Guardar os filhos é o que faz o acessador ser coerente consigo mesmo — pedir duas vezes o
/// filho `0x5000` tem de devolver o mesmo objeto, ou o jogo fica com dois.
#[derive(Default)]
struct Widget {
    filhos: HashMap<u32, u32>,
    propriedades: HashMap<u32, u32>,
    /// Modelos associados pelo slot 17. O roller da Z-Wheel prende aqui a fonte sob o id
    /// `0x8000`; o chamador solta a referência temporária logo depois, portanto o widget é
    /// quem precisa mantê-la viva enquanto o roller existir.
    modelos: HashMap<u32, u32>,
    /// Largura e altura, do slot 7. A Z-Wheel manda `640 × 480` — a tela inteira.
    tamanho: (u32, u32),
    /// Onde o pai pendurou este widget, em coordenadas dele. Ver o `AdicionarFilho`.
    posicao: (i32, i32),
    /// A classe com que foi criado. Cinco classes da família dividem a mesma tabela de slots, e
    /// o mesmo número quer dizer coisas diferentes em cada uma — ver o slot 6.
    classe: u32,
    /// O texto que o slot 6 pôs nele, quando a classe é das que põem texto.
    texto: String,
    /// Ordem de criação. Ver [`Machine::formulario_atual`].
    serial: u64,
    /// Os filhos que entraram pelo slot 5, que não os identifica por número.
    anexados: Vec<u32>,
    /// Se o widget deve aparecer. O slot 6 é quem diz.
    visivel: bool,
    /// Quem o pendurou, do slot 5. Ver o `PegarPai`.
    pai: u32,
    /// O tratador que o slot 4 registrou, já lido: `(função, contexto)`.
    ///
    /// Guardamos os **valores**, não o endereço da estrutura, porque o `DefinirTratador`
    /// devolve o tratador anterior escrevendo-o de volta nessa mesma estrutura — depois da
    /// chamada ela não descreve mais quem acabou de se registrar.
    tratador: (u32, u32),

    /// O retorno de desenho que o slot 16 registrou: `(função, contexto)`.
    ///
    /// Vem de um trio `{função, contexto, liberador}` que o jogo monta na própria estrutura e
    /// passa por ponteiro; o slot devolve o anterior escrevendo-o de volta nas duas primeiras
    /// palavras, do mesmo jeito que o slot 4 faz com o tratador. É por isso que a função
    /// registrada pode chamar "o de baixo" sem guardar nada: ela lê do lugar onde escreveu.
    desenho: (u32, u32),
    /// Se o aviso de partida já foi entregue. Ver [`Machine::parte_animacao`].
    partiu: bool,
}

/// O número que uma linha de arquivo faltando traz entre parênteses, se ela traz um.
///
/// Serve ao [`Machine::missing_files`]: as linhas de recurso têm a forma
/// `caminho (recurso 5035)`, e as de arquivo mesmo não têm parte nenhuma entre parênteses.
fn id_do_recurso(falta: &str) -> Option<u16> {
    falta
        .rsplit_once("(recurso ")
        .and_then(|(_, resto)| resto.strip_suffix(')'))
        .and_then(|numero| numero.parse().ok())
}

/// As classes da extensão de interface que respondem ao mesmo acessador do
/// [`Interface::Widget`].
///
/// A primeira, `0x01028e51`, foi lida no código da Z-Wheel. As outras entraram por medição: o
/// jogo as pede em sequência — a `0x01028e19` e a `0x01028e2a` são o "frame widget" da
/// `AnimationVideo_Form.c:93`, a `0x01028e47` é o formulário de vídeo em si —, e atendê-las com
/// este acessador faz cada mensagem de erro sair e a seguinte aparecer.
///
/// Elas são vizinhas de numeração e aparecem juntas numa mesma tabela do firmware, em
/// `0x1035cf24`. Nenhuma está na tabela de classes, então não há vtable para conferir: o que
/// sustenta a lista é o jogo andar, e é por isso que ela mora aqui, com o porquê escrito, em
/// vez de virar um `|` no meio do despacho.
/// A classe de um **formulário**: tem tratador de evento e pendura o conteúdo no item `0x5000`.
///
/// Lida na árvore: a abertura e o formulário do z-pad são dois objetos desta classe, filhos da
/// raiz do applet (`0x01028e51`), cada um com o seu container pendurado no item `0x5000`.
const WIDGET_FORMULARIO: u32 = 0x0102_8e47;

/// A classe da família em que o slot 6 **põe texto**, em vez de esconder ou mostrar.
///
/// Medido dos dois lados. Do lado da Z-Wheel, seguindo o que o `ISHELL_LoadResString` carrega
/// até onde ele para: o texto vira o primeiro argumento do slot 6, com o comprimento no
/// segundo, e o objeto que o recebe foi criado com esta classe. Do lado do firmware, o
/// `0x01028e2a` é a classe mais usada da família — quarenta ocorrências.
///
/// Nas outras classes o mesmo slot continua sendo visibilidade, que é como ele foi lido
/// primeiro. Não é contradição: a tabela de slots é a mesma e a implementação por trás não.
const WIDGET_DE_TEXTO: u32 = 0x0102_8e2a;

/// A propriedade que guarda a cor do widget, com alfa no byte de baixo.
///
/// A Z-Wheel grava `0x444444ff` nela — o cinza do texto da tela de boas-vindas. O firmware
/// grava valores da mesma cara pelo ajustador em `0x1035f222`.
const PROP_COR: u32 = 0x140;

const FAMILIA_DOS_WIDGETS: [u32; 9] = [
    AEECLSID_WIDGET,
    // A `0x01028e05` é a última que o palco pede. Depois de montar o pbuffer — `ChooseConfig`,
    // `CreatePbufferSurface`, `CreateContext`, `MakeCurrent` — a Z-Wheel cria a `0x01028e14` e
    // logo esta; recusada, o `CreateStageWidget` desiste e leva a roda de jogos junto. No
    // firmware ela aparece nos mesmos depósitos de literais que a `0x01028e19` e a
    // `0x01028e4b`, ao lado do seletor `0x801` e de uma cor, que é como as outras da família
    // são usadas.
    0x0102_8e05,
    // A `0x01028e14` é o `OwnerDrawWidget`: o `CreateTectoyRollerWidget` a cria e, recusada,
    // registra `Failure in call to CreateOwnerDrawWidget` e desiste da roda de jogos inteira.
    0x0102_8e14,
    0x0102_8e19,
    // A `0x01028e26` também entrou pela sonda: `slot3(0x801, 0x186, 0xff0000ff)` — o acessador,
    // com uma cor — e o mesmo slot 14 das outras.
    0x0102_8e26,
    0x0102_8e2a,
    // A `0x01028e36` entrou pela sonda, e não por vizinhança de numeração: atendida por
    // observação, o jogo chamou nela `slot3(0x801, 0x156, …)` — o acessador de widget, com o
    // seletor de gravar e um id de propriedade da mesma faixa dos outros. Sem ela, o
    // `ZPad_Keyboard_Instructions_Form.c` falhava com `ECLASSNOTSUPPORT`, que é o 20 do
    // `Couldn't create z-pad instruction form (20)`.
    0x0102_8e36,
    0x0102_8e3f,
    0x0102_8e47,
];

/// `0x01035156`, a fonte TrueType do console. Ver [`Interface::Typeface`].
const AEECLSID_TYPEFACE: u32 = 0x0103_5156;
/// Classe concreta de fonte usada pelo roller da Z-Wheel.
const AEECLSID_ROLLER_FONT: u32 = 0x0102_f67c;

/// `0x01006c01`, o controle do cartão SIM. Ver [`Interface::SimCardCtl`].
const AEECLSID_SIMCARDCTL: u32 = 0x0100_6c01;

/// `0x01006c02`, o controle de sistema. Ver [`Interface::SystemCtl`].
const AEECLSID_SYSTEMCTL: u32 = 0x0100_6c02;

/// `0x01011810`, o `ICM`. Ver [`Interface::Cm`].
const AEECLSID_CM: u32 = 0x0101_1810;

/// `0x01028e3c`, a terceira extensão que a Z-Wheel pede. Ver [`Interface::Classe28e3c`].
const AEECLSID_28E3C: u32 = 0x0102_8e3c;

/// `0x01028e35`, a lista genérica da Z-Wheel. Ver [`Interface::Vetor`].
const AEECLSID_VETOR: u32 = 0x0102_8e35;

/// `0x01001027`, a `IConfig`. Ver [`Interface::Config`].
const AEECLSID_CONFIG: u32 = 0x0100_1027;

/// `0x01006c05`, o ZEEBOMCP. Ver [`Interface::ZeeboMcp`].
const AEECLSID_ZEEBOMCP: u32 = 0x0100_6c05;

/// `0x01028e51`, o widget da interface da Z-Wheel. Ver [`Interface::Widget`].
///
/// Também não está em header nenhum nem na tabela de classes do firmware que temos. O valor
/// veio do próprio módulo: é o literal em `0x7c69c`, carregado pela `tectoymain.c:1001` e
/// entregue ao `ISHELL_CreateInstance` cujo fracasso imprime `Could not create root form`.
const AEECLSID_WIDGET: u32 = 0x0102_8e51;
/// `0x01003109`, controle de texto usado pelo Zenonia (`CWBLText`).
const AEECLSID_CONTROL: u32 = 0x0100_3109;

/// A coleção genérica que a interface da Z-Wheel usa.
///
/// Não há header. O que identifica a classe é o pool de literais do módulo: a constante aparece
/// vinte bytes antes de `Could not create root form`, e também no `Tectoy_Start` que instancia o
/// "app history" e no `Tectoy_LaunchMainMenu` — três lugares sem nada em comum além de guardar
/// itens.
const AEECLSID_COLLECTION: u32 = 0x0100_104f;

/// `AEECLSID_SQLMGR` — o gerenciador de bancos do console.
///
/// O valor não veio de header nenhum: veio do log do próprio Z-Wheel, que imprime
/// `No SQLMGR: 20` de `tectoy_prefsDB.c` toda vez que o `ISHELL_CreateInstance` desta classe é
/// recusado — 21.845 vezes seguidas, até estourar a pilha.
const AEECLSID_SQLMGR: u32 = 0x0102_c4e8;
/// `AEECLSID_HID` — o gamepad do Zeebo.
///
/// Não está em header nenhum que tenhamos. Foi identificado assim: o valor aparece três vezes
/// dentro da `IHID.dll` do SDK do Zeebo (cujo instalador só continha a extensão de HID, com as
/// strings `"Failed to open HID device"` e `fs:/sys/hid_devices.cfg`), o `conftest` do SDK
/// chama `ISHELL_CreateInstance(shell, AEECLSID_HID, &pIHID)` e o `conftest.elf` contém a mesma
/// constante — e `AEEIID_IHID` é `0x0106c38d`, vizinho de faixa.
const AEECLSID_HID: u32 = 0x0106_c411;
/// `AEECLSID_SignalCBFactory`, do `AEESignalCBFactory.bid` do BREW SDK 4.0.2. É por aqui que o
/// app cria os sinais que o sistema dispara para avisá-lo de eventos — no caso dos jogos,
/// eventos de botão do gamepad.
const AEECLSID_SIGNAL_CB_FACTORY: u32 = 0x0104_1207;
/// `AEECLSID_GRAPHICS` = `AEECLSID_CORE + 0x1001`, resolvido do `AEEClassIDs.h`.
const AEECLSID_GRAPHICS: u32 = 0x0100_2001;

/// `EVT_APP_START`, de `inc/AEEEvent.h`. Vale zero — o primeiro evento que um applet recebe.
const EVT_APP_START: u32 = 0;

/// Tela do Zeebo: VGA 640×480, saída composta.
const SCREEN_WIDTH: u16 = 640;
const SCREEN_HEIGHT: u16 = 480;
/// Profundidade de cor em bits. O framebuffer do console é RGB565.
const COLOR_DEPTH: u16 = 16;

/// Por que a execução do módulo terminou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `AEEMod_Load` retornou. `code` é o valor de `r0`.
    Returned { code: u32 },
    /// O guest chamou uma API que ainda não implementamos. `caller` é o endereço de retorno
    /// guardado em `lr` — ou seja, logo depois da instrução que fez a chamada, o que permite
    /// achar o trecho responsável ao desmontar o módulo.
    Unimplemented {
        addr: u32,
        args: [u32; 4],
        caller: u32,
    },
    /// Acesso a memória fora do mapa.
    Fault {
        addr: u32,
        pc: u32,
        /// `lr` no momento da falha: diz de onde a função com problema foi chamada.
        lr: u32,
    },
    /// Exceção do núcleo — instrução inválida, SWI, etc.
    Exception { pc: u32 },
    /// O orçamento de instruções acabou sem chegar a lugar nenhum.
    Budget,
    /// O guest passou do teto de chamadas de API — quase sempre um laço de repetição por
    /// causa de alguma API que devolve erro e o jogo tenta de novo indefinidamente.
    CallLimit { calls: u64 },
}

/// Desfecho de [`Machine::create_applet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppletResult {
    /// `AEEMod_Load` não deixou um `IModule*` — não há o que chamar.
    NoModule,
    /// `CreateInstance` retornou. `code` é o erro do BREW, `applet` o ponteiro criado.
    Called { code: u32, applet: u32 },
    /// A execução parou antes de retornar.
    Stopped(Outcome),
}

/// Estado de desenho do `IGraphics`.
#[derive(Debug, Clone, Copy)]
struct GraphicsState {
    stroke: Rgb,
    fill: Rgb,
    background: Rgb,
    fill_mode: bool,
    point_size: u8,
    /// Deslocamento aplicado a todas as coordenadas, definido por `Translate`.
    origin: (i32, i32),
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            stroke: Rgb::BLACK,
            fill: Rgb::WHITE,
            background: Rgb::WHITE,
            fill_mode: false,
            point_size: 1,
            origin: (0, 0),
        }
    }
}

/// Um arquivo aberto pelo jogo.
#[derive(Debug)]
struct OpenFile {
    file: std::fs::File,
    /// Caminho como o jogo pediu, para devolver em `GetInfo`.
    guest_path: String,
}

/// Um callback do guest: a função e o contexto que ela recebe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Callback {
    pub function: u32,
    pub context: u32,
}

/// Um timer armado por `ISHELL_SetTimer`.
///
/// Os timers do BREW são de um disparo só: quem quer periodicidade rearma dentro do próprio
/// callback. É exatamente assim que um jogo monta o laço de quadros dele.
#[derive(Debug, Clone, Copy)]
struct Timer {
    /// Valor do relógio virtual em que ele vence.
    deadline_ms: u32,
    callback: Callback,
}

/// Uma imagem já decodificada, pronta para desenhar.
#[derive(Debug, Clone)]
struct DecodedImage {
    width: u32,
    height: u32,
    /// Pixels em RGB565, na ordem de leitura.
    pixels: Vec<u16>,
    /// Se o pixel deve ser desenhado. O PNG traz canal alfa e os jogos contam com ele.
    opaque: Vec<bool>,
    /// Largura de cada quadro, quando o jogo divide a imagem em tiras (`IPARM_CXFRAME`).
    frame_width: u16,
}

/// Põe os quadros de um GIF lado a lado, numa imagem só.
///
/// É a forma que o resto do emulador já entende: uma imagem com `frame_width` menor que a
/// largura é uma sequência, e o `IIMAGE_DrawFrame` escolhe a coluna. Dar caminho próprio à
/// animação de GIF seria repetir o que o `IPARM_CXFRAME` já faz.
fn tira_de_quadros(gif: &crate::gif::Gif) -> DecodedImage {
    let (largura, altura) = (gif.largura as usize, gif.altura as usize);
    let quadros = gif.quadros.len();
    let total = largura * quadros * altura;
    let mut pixels = vec![0u16; total];
    let mut opaque = vec![false; total];
    for (n, quadro) in gif.quadros.iter().enumerate() {
        for y in 0..altura {
            for x in 0..largura {
                let cor = quadro[y * largura + x];
                let onde = y * largura * quadros + n * largura + x;
                pixels[onde] = Rgb {
                    r: cor[0],
                    g: cor[1],
                    b: cor[2],
                }
                .to_rgb565();
                // O alfa do GIF é binário: ou a cor é a transparente da paleta, ou não é.
                opaque[onde] = cor[3] != 0;
            }
        }
    }
    DecodedImage {
        width: (largura * quadros) as u32,
        height: altura as u32,
        pixels,
        opaque,
        // Um GIF de um quadro só não é sequência: dizer que é faria o `GetInfo` anunciar uma
        // largura de quadro que o jogo não pediu.
        frame_width: match quadros > 1 {
            true => largura as u16,
            false => 0,
        },
    }
}

/// Descomprime um bloco de deflate.
///
/// A documentação do `IUnzipAStream` fala do "algoritmo deflate, o usado pelo gzip", e as duas
/// formas aparecem na prática: o fluxo cru e o mesmo fluxo dentro de um envelope de gzip ou de
/// zlib. Tentamos os três, do mais provável ao menos, porque distinguir pelo cabeçalho falha
/// justamente no caso cru, que não tem cabeçalho nenhum.
fn inflate(compressed: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let raw = || {
        let mut out = Vec::new();
        flate2::read::DeflateDecoder::new(compressed)
            .read_to_end(&mut out)
            .ok()
            .map(|_| out)
    };
    let gzip = || {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(compressed)
            .read_to_end(&mut out)
            .ok()
            .map(|_| out)
    };
    let zlib = || {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(compressed)
            .read_to_end(&mut out)
            .ok()
            .map(|_| out)
    };
    [gzip(), zlib(), raw()]
        .into_iter()
        .flatten()
        .find(|out| !out.is_empty())
}

/// Decodifica um PNG para RGB565, devolvendo `None` se não for um PNG que saibamos ler.
fn decode_png(bytes: &[u8]) -> Option<DecodedImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    // Os PNGs do Bejeweled Twist usam paleta com `tRNS`. Pedir a expansão aqui evita ter de
    // reimplementar paleta, transparência indexada e profundidades menores que 8 bits.
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::normalize_to_color8(),
    );
    let mut reader = decoder.read_info().ok()?;
    let mut raw = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut raw).ok()?;
    let data = &raw[..info.buffer_size()];

    let channels = info.color_type.samples();
    let has_alpha = matches!(
        info.color_type,
        png::ColorType::Rgba | png::ColorType::GrayscaleAlpha
    );
    let count = (info.width * info.height) as usize;
    let mut pixels = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    for chunk in data.chunks_exact(channels) {
        let (r, g, b) = if channels >= 3 {
            (chunk[0], chunk[1], chunk[2])
        } else {
            (chunk[0], chunk[0], chunk[0])
        };
        pixels.push(Rgb { r, g, b }.to_rgb565());
        // Meio-tom não existe numa superfície sem canal alfa: ou o pixel entra, ou não entra.
        opaque.push(!has_alpha || chunk[channels - 1] >= 128);
    }

    Some(DecodedImage {
        width: info.width,
        height: info.height,
        pixels,
        opaque,
        frame_width: 0,
    })
}

/// Estado de um `IUnzipAStream`: de onde vêm os bytes comprimidos e o que já saiu deles.
#[derive(Debug, Default, Clone)]
struct UnzipState {
    /// O `IAStream` de entrada, como o jogo o entregou em `SetStream`.
    source: u32,
    /// O resultado da descompressão, produzido de uma vez na primeira leitura.
    output: Vec<u8>,
    position: usize,
    /// Se já tentamos descomprimir. Uma entrada que não descomprime não é tentada de novo.
    expanded: bool,
}

/// O que se sabe de um objeto `IMedia`.
#[derive(Debug, Clone, Copy)]
struct MediaState {
    state: u32,
    /// O `AEEMediaData` já lido: onde estão os bytes e quantos são.
    buffer: u32,
    size: u32,
    /// De 0 a [`MAX_VOLUME`].
    volume: u32,
    /// Quantas vezes tocar. Zero é para sempre, que é o que o `MM_PARM_PLAY_REPEAT` define.
    repeat: u32,
    muted: bool,
    /// `PFNMEDIANOTIFY` registrado por `RegisterNotify`.
    notify: Callback,
    /// O bloco na memória do guest onde o `AEEMediaCmdNotify` é montado. Um por objeto, criado
    /// na primeira notificação e reaproveitado: o callback só o lê enquanto roda.
    notify_block: u32,
    /// Quando o som acaba, no relógio virtual. Zero é "não está tocando", e `u64::MAX` é o
    /// `repeat` infinito.
    ends_us: u64,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            state: MM_STATE_READY,
            buffer: 0,
            size: 0,
            volume: MAX_VOLUME,
            repeat: 1,
            muted: false,
            notify: Callback {
                function: 0,
                context: 0,
            },
            notify_block: 0,
            ends_us: 0,
        }
    }
}

impl MediaState {
    /// O volume como o mixer quer, já com o mudo aplicado.
    fn gain(&self) -> f32 {
        match self.muted {
            true => 0.0,
            false => self.volume.min(MAX_VOLUME) as f32 / MAX_VOLUME as f32,
        }
    }
}

/// Um bloco de memória do guest servido como stream por um `IMemAStream`.
#[derive(Debug, Clone, Copy)]
struct MemStream {
    buffer: u32,
    size: u32,
    position: u32,
}

/// Estado de um objeto `ISound`.
#[derive(Debug, Clone, Copy)]
struct SoundState {
    /// `PFNSOUNDSTATUS` registrado por `RegisterNotify`.
    notify: Callback,
    /// Os cinco `int8` do `AEESoundInfo`, guardados como o jogo os entregou.
    info: [u8; 5],
    volume: u16,
}

impl Default for SoundState {
    fn default() -> Self {
        Self {
            notify: Callback {
                function: 0,
                context: 0,
            },
            info: [0; 5],
            volume: AEE_MAX_VOLUME,
        }
    }
}

/// Contexto que uma chamada ao guest feita por nós precisa devolver intacto: tudo o que a
/// convenção de chamada do ARM manda preservar, mais os registradores de argumento, porque a
/// API que acabou de ser atendida já deixou o resultado em `r0`.
const SAVED_REGS: [Reg; 15] = [
    Reg::R0,
    Reg::R1,
    Reg::R2,
    Reg::R3,
    Reg::R4,
    Reg::R5,
    Reg::R6,
    Reg::R7,
    Reg::R8,
    Reg::R9,
    Reg::R10,
    Reg::R11,
    Reg::R12,
    Reg::Sp,
    Reg::Lr,
];
/// Slot de `QueryInterface` na vtable de `IBitmap`.
const BITMAP_QUERY_INTERFACE_SLOT: u32 = 2;
/// Slot de `BltIn` na vtable de `IBitmap`.
const BITMAP_BLT_IN_SLOT: u32 = 10;
/// Cor usada como transparente ao entregar uma imagem com alfa a uma superfície sem alfa.
/// Magenta puro é a escolha tradicional, justamente por não aparecer em arte real.
const TRANSPARENT_KEY: u16 = 0xf81f;
/// Tamanho do `AEEDeviceInfo` com os campos estendidos: até `dwPlatformID`, em `sdk/inc/AEEShell.h`.
const DEVICE_INFO_SIZE: u32 = 64;
/// `AEE_MAX_FILE_NAME`, de `inc/AEEFile.h`: o teto de um caminho no sistema de arquivos do BREW.
const AEE_MAX_FILE_NAME: u32 = 64;
/// Quantos endereços de retorno colher da pilha numa falha.
const STACK_DEPTH: usize = 24;
/// Profundidade máxima de reentrada no guest.
const MAX_NESTING: u32 = 4;

/// Teto de instruções por comparação do `qsort`. Um comparador é uma função curta; este limite
/// existe para um comparador quebrado não travar a ordenação inteira.
/// Uma chamada observada pelo `--sonda`: classe, objeto, slot, argumentos e o texto de cada
/// argumento que apontava para texto.
pub type ProbeCall = (u32, u32, u32, [u32; 4], [Option<String>; 4], u64);

const QSORT_BUDGET: u64 = 10_000_000;

/// Teto de instruções por linha entregue ao callback de uma consulta SQL. Mesma ideia do
/// [`QSORT_BUDGET`]: o callback é uma função curta que copia campos.
const SQL_CALLBACK_BUDGET: u64 = 10_000_000;

/// Quantas rodadas de callbacks entregar antes de desistir — um callback pode enfileirar
/// outro, e sem teto um ciclo prenderia o emulador.
const CALLBACK_ROUNDS: usize = 64;

/// Quantos orçamentos de fatia um trecho de execução pode gastar antes de devolver a vez ao
/// laço de quadros. Ver [`Machine::execute`].
const TETO_DE_TRECHO: u64 = 4;

/// Ids de parâmetro do `ICipher1`, de `inc/AEEICipher1.h`.
const CIPHER_PARAM_DIRECTION: u32 = 0;
const CIPHER_PARAM_KEY: u32 = 1;
const CIPHER_PARAM_KEY_SIZE: u32 = 2;
const CIPHER_PARAM_IV: u32 = 3;
const CIPHER_PARAM_IV_SIZE: u32 = 4;
const CIPHER_PARAM_PADDING: u32 = 5;
const CIPHER_PARAM_BLOCKSIZE: u32 = 6;
const CIPHER_PARAM_MODE: u32 = 8;
/// `CIPHER_PADDING_NONE`, de `inc/AEEICipher1.h`: o único que muda o que fazemos — os outros
/// completam o bloco, e completamos com zeros.
const CIPHER_PADDING_NONE: u32 = 0;
/// Tamanho do bloco e da chave do AES-128, em bytes.
const AES_BLOCK: usize = 16;

/// Estado de um objeto `IHash`.
///
/// Só o resumo em andamento. O `GetDigest` escreve no buffer que o jogo fornece, então não há
/// nada para guardar na memória dele — o campo que existia para isso vinha da assinatura errada.
#[derive(Debug, Default)]
struct HashState {
    md5: crate::crypto::Md5,
}

/// Estado de um `ICipher1`: a configuração que chegou pelo `SetParam` e o que sobrou de um
/// `Process` para o próximo.
#[derive(Debug, Default)]
struct CipherState {
    key: Option<[u8; AES_BLOCK]>,
    iv: [u8; AES_BLOCK],
    padding: u32,
    /// Bytes que ainda não completaram um bloco. O `ICipher1` é de fluxo: o jogo pode entregar
    /// qualquer quantidade e só o `ProcessLast` fecha o que faltar.
    pending: Vec<u8>,
}

/// Um desenho numa superfície do próprio jogo, à espera da fronteira da chamada.
#[derive(Debug, Clone, Copy)]
struct PendingBlit {
    image: u32,
    target: u32,
    x: i32,
    y: i32,
    frame: Option<u32>,
}

/// Se o método pode mexer em mais de um pixel da superfície.
///
/// Quando o jogo pede o `IDIB`, ele passa a enxergar os pixels direto na memória dele, e a
/// superfície precisa ser copiada nos dois sentidos em volta de cada chamada que os lê ou
/// escreve — 1,2 MB de ida e volta. Só que a maior parte de `IDisplay`, `IGraphics` e `IBitmap`
/// não toca em pixel nenhum: são ajustes de estado (cor, fonte, recorte) e consultas.
///
/// O Pac-Mania mostrou os dois extremos. Ele desenha **pixel a pixel** pela API — 1,16 milhão de
/// `DrawPixel` em dez quadros — e consulta o recorte 112 mil vezes a cada quinze segundos.
/// Cobrar a superfície inteira de cada uma dessas chamadas custava dezenas de gigabytes de
/// cópia por segundo.
///
/// A lista abaixo é de exclusão, e não de inclusão, de propósito: esquecer um método que
/// desenha aqui daria pixel errado, que é difícil de perceber; deixar de fora um que não
/// desenha só custa a cópia, que é visível na medição. Na dúvida, copia.
fn touches_whole_surface(name: &str) -> bool {
    !matches!(
        name,
        // Comuns a todas elas.
        "AddRef" | "Release" | "QueryInterface"
        // `IBitmap`: estes dois tratam do seu pixel direto no buffer do jogo; o resto é consulta.
        | "DrawPixel" | "GetPixel"
        | "RGBToNative" | "NativeToRGB"
        | "GetInfo" | "CreateCompatibleBitmap"
        | "SetTransparencyColor" | "GetTransparencyColor"
        // `IDisplay`: estado e medição.
        | "GetFontMetrics" | "MeasureTextEx" | "SetFont"
        | "SetClipRect" | "GetClipRect"
        | "SetColor" | "GetSymbol"
        | "SetDestination" | "GetDestination" | "GetDeviceBitmap"
        | "SetAnnunciators" | "Backlight" | "MakeDefault" | "IsEnabled" | "NotifyEnable"
        | "SetPrefs"
        // `IGraphics`: só os pares de ajuste, nunca os `Draw*` nem os `Clear*`.
        | "SetBackground" | "GetBackground" | "GetColor"
        | "SetFillMode" | "GetFillMode" | "SetFillColor" | "GetFillColor"
        | "SetPointSize" | "GetPointSize"
        | "SetClip" | "GetClip" | "SetViewport" | "GetViewport"
        | "SetPaintMode" | "GetPaintMode" | "GetColorDepth"
        | "EnableDoubleBuffer" | "Translate"
        | "SetAlgorithmHint" | "GetAlgorithmHint"
        | "SetStrokeStyle" | "GetStrokeStyle"
        // `IImage`: só `Draw`, `DrawFrame` e `Start` põem pixel na superfície.
        | "SetParm" | "Notify" | "Stop" | "HandleEvent" | "SetStream"
    )
}

/// Quebra os segundos do relógio do BREW num `JulianType`.
///
/// A struct é `{ wYear, wMonth, wDay, wHour, wMinute, wSecond, wWeekDay }`, sete `uint16`, de
/// `AEEStdLib.h`. O mês e o dia começam em 1; o dia da semana começa em **domingo valendo 0**,
/// que é a convenção do BREW.
///
/// A época é 6 de janeiro de 1980, GMT — a do GPS, não a do Unix. Errar isso desloca tudo em
/// dez anos e o jogo mostra uma data que não existe.
fn julian_date(segundos: u32) -> [u16; 7] {
    const EPOCA_BREW_EM_DIAS_UNIX: i64 = 3657; // 1980-01-06 menos 1970-01-01
    let dias = segundos as i64 / 86_400;
    let resto = segundos as i64 % 86_400;
    // Dia da semana: 6 de janeiro de 1980 foi um domingo, que é o zero do BREW.
    let semana = (dias % 7) as u16;

    // Contagem civil a partir dos dias desde a época Unix, pelo algoritmo de Howard Hinnant:
    // desloca o ano para começar em março, o que faz fevereiro e o bissexto caírem no fim.
    let z = dias + EPOCA_BREW_EM_DIAS_UNIX + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let ano = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let dia = (doy - (153 * mp + 2) / 5 + 1) as u16;
    let mes = if mp < 10 { mp + 3 } else { mp - 9 } as u16;
    let ano = (ano + i64::from(mes <= 2)) as u16;

    [
        ano,
        mes,
        dia,
        (resto / 3600) as u16,
        (resto % 3600 / 60) as u16,
        (resto % 60) as u16,
        semana,
    ]
}

/// A fonte que o próprio jogo empacotou, se houver uma.
///
/// A do console vinha da firmware, que não temos. Vários módulos trazem a sua — a Z-Wheel
/// empacota a `tectoy.ttf`, que é a fonte com que a loja foi desenhada. Usar a do jogo é mais
/// fiel do que escolher uma por nós, e quando não há nenhuma o texto continua sem sair, o que
/// o relatório informa.
fn font_do_modulo(raiz: &std::path::Path) -> Option<crate::font::Font> {
    let mut fontes: Vec<_> = std::fs::read_dir(raiz)
        .ok()?
        .filter_map(Result::ok)
        .map(|entrada| entrada.path())
        .filter(|caminho| {
            caminho
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf"))
        })
        .collect();
    // Ordem estável: dois `.ttf` no mesmo diretório não podem dar resultados diferentes entre
    // execuções por causa da ordem em que o sistema de arquivos os lista.
    fontes.sort();
    let caminho = fontes.first()?;
    let nome = caminho
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::font::Font::load(std::fs::read(caminho).ok()?, nome)
}

/// Corta `rect` pelo recorte. `None` quando não sobra nada para desenhar.
fn clip_rect(clip: Option<Rect>, rect: Rect) -> Option<Rect> {
    // **Sem recorte definido, o recorte é a tela inteira** — e não "nada passa". Era o que
    // estava escrito aqui, e o efeito é silencioso: todo `IDISPLAY_DrawRect` de um jogo que não
    // define recorte era descartado. O Tekken 2 limpa a tela uma vez por quadro com um
    // `ClearScreen`, que é exatamente um `DrawRect`; a limpeza nunca acontecia, e o menu dele
    // aparecia por cima do texto da tela anterior. O `clip_blit`, logo abaixo, sempre tratou o
    // mesmo caso do jeito certo.
    let Some(clip) = clip else {
        return Some(rect);
    };
    let (x0, y0) = (rect.x.max(clip.x) as i32, rect.y.max(clip.y) as i32);
    let x1 = (rect.x as i32 + rect.width as i32).min(clip.x as i32 + clip.width as i32);
    let y1 = (rect.y as i32 + rect.height as i32).min(clip.y as i32 + clip.height as i32);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: x0 as i16,
        y: y0 as i16,
        width: (x1 - x0) as i16,
        height: (y1 - y0) as i16,
    })
}

/// Um blit já resolvido: destino, tamanho e origem na fonte.
type Blit = ((i32, i32), (i32, i32), (i32, i32));

/// Corta um blit pelo recorte, andando com a origem na fonte junto.
///
/// Corrigir a origem é o que faz o recorte mostrar **o pedaço certo** da fonte: um corte que só
/// encolhesse o destino mostraria o canto errado da imagem, e é assim que um atlas de fontes
/// vira letra trocada.
fn clip_blit(
    clip: Option<Rect>,
    dst: (i32, i32),
    size: (i32, i32),
    src: (i32, i32),
) -> Option<Blit> {
    let Some(clip) = clip else {
        return Some((dst, size, src));
    };
    let (cx0, cy0) = (clip.x as i32, clip.y as i32);
    let (cx1, cy1) = (cx0 + clip.width as i32, cy0 + clip.height as i32);
    let (x0, y0) = (dst.0.max(cx0), dst.1.max(cy0));
    let (x1, y1) = ((dst.0 + size.0).min(cx1), (dst.1 + size.1).min(cy1));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some((
        (x0, y0),
        (x1 - x0, y1 - y0),
        (src.0 + (x0 - dst.0), src.1 + (y0 - dst.1)),
    ))
}

/// Uma chamada ao guest esperando na fila.
#[derive(Debug, Clone, Copy)]
struct GuestCall {
    function: u32,
    args: [u32; 4],
}

/// Uma thread cooperativa do BREW.
///
/// "Cooperativa" é o que torna isto viável num emulador de um núcleo só: o guest só perde o
/// controle quando chama `Suspend`, e é exatamente aí que salvamos os registradores. Retomar
/// é restaurá-los e continuar de onde o `Suspend` voltaria.
#[derive(Default)]
struct ThreadState {
    /// Bloco da heap do guest que serve de pilha para esta thread.
    stack: u32,
    /// O `AEECallback` que `GetResumeCBK` devolve — é por ele que o jogo pede a retomada.
    resume_cb: u32,
    /// Registradores salvos, na ordem de [`THREAD_REGS`].
    context: [u32; 14],
    /// Endereço onde a execução continua na próxima retomada.
    resume_pc: u32,
    started: bool,
    /// Verdadeiro entre um `Suspend` e a retomada seguinte. É o que distingue "a thread cedeu
    /// o controle" de "a função de entrada retornou".
    suspended: bool,
    finished: bool,
    exit_code: u32,
    /// Callbacks registrados por `Join`, disparados quando a thread termina.
    joiners: Vec<(Callback, u32)>,
}

/// Um vetor de atributos do cliente, como `glVertexPointer` o descreve.
///
/// Os dados ficam na memória do guest e só são lidos na hora de desenhar: é assim que o
/// OpenGL ES funciona, e é o que permite ao jogo alterar o conteúdo entre duas chamadas sem
/// avisar ninguém.
#[derive(Debug, Default, Clone, Copy)]
struct ArrayPointer {
    /// Quantos componentes por elemento (2, 3 ou 4).
    size: u32,
    /// Tipo do componente (`GL_FIXED`, `GL_FLOAT`, …).
    kind: u32,
    /// Distância em bytes entre dois elementos; zero significa "colados".
    stride: u32,
    address: u32,
    enabled: bool,
}

/// Adapta os registradores e a pilha do guest ao formatador de `printf`.
struct GuestArgs<'a, C: CpuBackend> {
    words: Vec<u32>,
    index: usize,
    stack: u32,
    cpu: &'a C,
}

impl<C: CpuBackend> ArgSource for GuestArgs<'_, C> {
    fn next_word(&mut self) -> u32 {
        let value = match self.words.get(self.index) {
            Some(&word) => word,
            // Esgotados r1..r3, o resto vem da pilha, em ordem.
            None => {
                let offset = (self.index - self.words.len()) as u32 * 4;
                self.cpu.read_u32(self.stack + offset).unwrap_or(0)
            }
        };
        self.index += 1;
        value
    }

    fn read_cstring(&mut self, addr: u32) -> String {
        self.cpu.read_cstring(addr, MAX_STRING)
    }
}

pub struct Machine<C: CpuBackend> {
    cpu: C,
    module: LoadedModule,
    heap: Heap,
    objects: ObjectStore,
    /// ClassIDs que o jogo pediu e não sabemos criar — a lista do que falta.
    unknown_classes: BTreeSet<u32>,
    /// ClassIDs que o `--sonda` manda atender com um objeto de observação.
    /// As URLs que o jogo pediu pelo `IWeb`, para o relatório.
    web_requests: BTreeSet<String>,
    /// As coleções vivas, cada uma com os itens e onde o cursor está.
    collections: HashMap<u32, (Vec<u32>, usize)>,
    /// Os bancos SQLite abertos, por objeto `ISQLDatabase`.
    databases: HashMap<u32, crate::sql::Database>,
    probe_classes: BTreeSet<u32>,
    /// Respostas combinadas para slots de sonda: `(classe, slot) -> valor`.
    probe_answers: HashMap<(u32, u32), u32>,
    /// Que horas eram quando a máquina foi criada, em segundos da época do BREW.
    epoch_seconds: u32,
    /// De qual classe é cada objeto-sonda vivo.
    probe_objects: HashMap<u32, u32>,
    /// O que foi chamado em cada sonda: `(classe, objeto, slot)` e os argumentos da primeira
    /// vez. O objeto entra na chave porque é ele que revela a **família**: o gerenciador
    /// devolve um banco, o banco devolve uma consulta, e sem distinguir os três a sequência
    /// vira uma lista de slots sem dono.
    probe_log: Vec<ProbeCall>,
    /// Chamadas em que o ponteiro `this` não era o objeto esperado.
    suspicious_objects: BTreeSet<(u32, u32)>,
    /// Chamadas em que um ponteiro do guest não apontava para memória mapeada.
    bad_pointers: BTreeSet<String>,
    /// APIs que atendemos com base em hipótese, não em documentação. Sai no relatório para
    /// ninguém confundir palpite com comportamento conhecido.
    assumptions: BTreeSet<&'static str>,
    /// Estado do `IGraphics`.
    graphics: GraphicsState,
    /// Sistema de arquivos do jogo.
    vfs: Vfs,
    /// Arquivos abertos, indexados pelo ponteiro do `IFile` no guest.
    open_files: HashMap<u32, OpenFile>,
    /// Último erro de arquivo, devolvido por `IFILEMGR_GetLastError`.
    file_error: u32,
    /// Os bytes já entregues a cada decodificador de imagem, e o resultado quando ele fecha.
    decoders: HashMap<u32, DecoderState>,
    /// De qual decodificador é cada `IForceFeed` — o `QueryInterface` devolve um objeto
    /// separado, porque as duas interfaces têm métodos diferentes no mesmo slot.
    feeds: HashMap<u32, u32>,
    /// Os objetos das extensões gráficas do console, criados na primeira vez que são pedidos.
    surface_manip: u32,
    imageon_ext: u32,
    /// O retângulo em que o jogo desenha, quando ele o declara pelo `SetSurfaceScale`. Vale
    /// mais que a dedução por viewport: aqui o jogo **diz** o tamanho.
    scale_source: Option<(i32, i32)>,
    /// As preferências gravadas por `ISHELL_SetPrefs`, por classe e versão.
    prefs: HashMap<(u32, u16), Vec<u8>>,
    /// A listagem em curso de cada `IFileMgr`. O BREW guarda esse estado dentro do próprio
    /// gerenciador, e dois gerenciadores enumeram diretórios diferentes ao mesmo tempo.
    enumerations: HashMap<u32, std::collections::VecDeque<String>>,
    /// Arquivos que o jogo tentou abrir e não existem — bom indício de asset faltando.
    missing_files: BTreeSet<String>,
    /// Sinais que o jogo registrou para eventos de entrada, por tipo de evento.
    input_signals: BTreeMap<&'static str, u32>,
    /// Callback de cada sinal vivo, indexado pelo ponteiro do objeto no guest.
    signals: HashMap<u32, Callback>,
    /// Sinais disparados e ainda não entregues ao guest.
    pending_signals: Vec<Callback>,
    /// Estado corrente de cada porta. Ver [`crate::input::PORTAS`].
    pads: [Pad; input::PORTAS],
    /// Apertos e solturas ainda não lidos pelo jogo, por porta, na ordem em que aconteceram.
    pad_events: [std::collections::VecDeque<(usize, bool)>; input::PORTAS],
    /// Que aparelho o console vê em cada porta, e se ela está ligada.
    ///
    /// É o que o `GetConnectedDevices` responde. Uma porta desligada não é enumerada — e é
    /// assim que se testa um jogo que se comporta diferente com dois controles.
    portas: [Option<crate::bindings::Aparelho>; input::PORTAS],
    /// A porta de cada `IHIDDevice` que o jogo criou, pelo endereço do objeto.
    portas_de_aparelho: HashMap<u32, usize>,
    /// Teclas apertadas e ainda não entregues, como `(código AVK, apertada)`.
    teclas: std::collections::VecDeque<(u32, bool)>,
    /// Os últimos toques entregues, para o relatório.
    ///
    /// A fila acima é consumida pelo jogo e some; esta fica. Existe porque um problema de
    /// entrada é indistinguível de um problema de interpretação sem ver o que chegou: um menu
    /// que anda duas casas por toque pode ser o jogo contando dois canais, ou o emulador
    /// mandando dois eventos, e só o registro separa os dois casos.
    pad_log: std::collections::VecDeque<(u32, usize, usize, bool)>,
    /// O applet corrente, devolvido por `GetAppInstance`.
    current_applet: u32,
    /// Semente do gerador pseudoaleatório — fixa, para que a mesma sessão se repita igual.
    random_state: u32,
    /// Quando a execução começou, para os helpers de tempo.
    /// Relógio virtual do guest, em milissegundos.
    ///
    /// Deliberadamente não é o relógio do host: o jogo pede um timer de 33 ms esperando um
    /// quadro, e o laço de quadros é quem decide quanto tempo passou. Assim o ritmo do jogo
    /// não depende de quão rápido o emulador consegue interpretar as instruções, e uma mesma
    /// execução dá sempre o mesmo resultado.
    clock_us: u64,
    spin_polls: u32,
    /// Instante do próximo retraço vertical, em microssegundos.
    next_vsync_us: u64,

    /// Registro de cada chamada na ordem em que aconteceu, quando o rastreamento está ligado.
    trace: Vec<String>,
    tracing: bool,
    /// Quando presente, só entram no rastreamento as chamadas cujo nome contém este trecho.
    trace_filter: Option<String>,
    /// O que o jogo escreveu via `DBGPRINTF`, na ordem em que apareceu e com quantas vezes
    /// cada mensagem se repetiu — um laço pode gerar milhares de linhas idênticas.
    debug_output: Vec<(String, u64)>,
    /// Superfícies de desenho, indexadas pelo ponteiro do `IBitmap` no guest.
    bitmaps: HashMap<u32, Framebuffer>,
    /// `r0..r11` no momento da última falha de memória, para o relatório.
    fault_regs: [u32; 12],
    /// Endereços de retorno vistos na pilha da última falha.
    fault_stack: Vec<u32>,
    /// Timers armados pelo jogo com `ISHELL_SetTimer`.
    timers: Vec<Timer>,
    /// ClassID do applet que o módulo instanciou, para responder ao `ISHELL_GetClassItemID`.
    applet_class: u32,
    installed_applets: HashSet<u32>,
    pending_launch: Option<u32>,
    wheel_boot_skipped: bool,
    /// Profundidade atual de reentrada no guest.
    nesting: u32,
    /// Superfícies do jogo à espera de serem consultadas sobre onde ficam seus pixels.
    pending_probes: Vec<u32>,
    /// Desenhos que precisam passar pelo `BltIn` de uma superfície do jogo.
    pending_blits: Vec<PendingBlit>,
    /// Superfícies já consultadas — a resposta não muda, e perguntar de novo custaria uma
    /// entrada no guest a cada `SetDestination`.
    probed: HashSet<u32>,
    /// Imagens decodificadas, por objeto `IImage`.
    /// As imagens carregadas, sob `Rc` porque desenhar é o caminho quente: o Pac-Mania chama
    /// `IIMAGE_Draw` dezenas de milhares de vezes por quadro.
    images: HashMap<u32, std::rc::Rc<DecodedImage>>,
    /// A superfície já materializada de cada imagem, para o `IPARM_GETBITMAP`.
    image_bitmaps: HashMap<u32, u32>,
    /// Quanto tempo **real** cada método de API custou, ligado pelo `--profile`.
    ///
    /// O perfil do guest diz onde o jogo gasta o tempo dele; este diz onde o emulador gasta o
    /// nosso. Sem ele, um método que custa meio milissegundo por chamada se esconde atrás de
    /// uma média: o que aparece é "8 µs por chamada de API", e não "o `IIMAGE_Draw` sozinho é
    /// dois terços do despacho".
    api_time: HashMap<(u32, u32), u64>,
    profiling_api: bool,
    /// Callback de `IIMAGE_Notify`, por objeto.
    image_notify: HashMap<u32, Callback>,
    /// Blocos de memória apresentados como stream.
    streams: HashMap<u32, MemStream>,
    /// Estado de cada `ISound` vivo.
    sounds: HashMap<u32, SoundState>,
    /// Estado de cada `ICipher1` vivo.
    ciphers: HashMap<u32, CipherState>,
    /// O que o `Definir` da coleção genérica recebeu: `(objeto, id) -> bytes`.
    parametros_de_colecao: HashMap<(u32, u32), Vec<u8>>,
    /// Os itens e o liberador de cada lista viva. Ver [`Interface::Vetor`].
    vetores: HashMap<u32, (Vec<u32>, u32)>,
    /// Os bytes de cada `ISource` vivo.
    sources: HashMap<u32, Vec<u8>>,
    /// O estado de cada `IPeek` vivo.
    peeks: HashMap<u32, Peek>,
    /// Os itens de cada `IConfig` vivo, por objeto: número do item -> bytes.
    config_items: HashMap<u32, HashMap<u32, Vec<u8>>>,
    /// O estado de cada widget vivo. Ver [`Widget`].
    widgets: HashMap<u32, Widget>,
    /// As APIs que faltaram, com quem as chamou. Ver o `None` do despacho.
    missing_apis: BTreeSet<String>,
    /// Acessos inválidos que aconteceram dentro de retorno de chamada e não pararam o jogo.
    falhas_engolidas: BTreeSet<String>,
    /// Chamadas de GL atendidas com sucesso sem fazer nada.
    ignored_gl: BTreeSet<&'static str>,
    /// O que o jogo entregou ao `ICipher1`, em claro, antes de ser cifrado.
    plaintexts: std::collections::VecDeque<Vec<u8>>,
    /// Se a ponte do módulo pode entregar a resposta ao jogo. Ver [`crate::ponte`].
    bridge: bool,
    /// Uma resposta esperando a fronteira de chamada: `(objeto, âncora, estado)`.
    pending_response: Option<(u32, u32, u32)>,
    /// O objeto que ainda precisa saber que o fluxo acabou, na fronteira seguinte.
    pending_end: Option<u32>,
    /// As respostas que a ponte chegou a depositar na memória do jogo.
    delivered: Vec<String>,
    /// Para onde desviar as conexões, quando se quer um servidor que não é o do endereço.
    network_to: Option<String>,
    /// Se o emulador pode falar com a rede.
    ///
    /// Dar rede a um binário de origem externa é decisão de projeto, então ela é explícita e
    /// aparece no relatório. Fica ligada porque é para isso que a pilha existe, e o `--sem-rede`
    /// desliga.
    network: bool,
    /// O corpo da última resposta recebida.
    web_response: Vec<u8>,
    /// Estado de cada `IHash` vivo.
    hashes: HashMap<u32, HashState>,
    resources: crate::resfile::ResCache,
    unzips: HashMap<u32, UnzipState>,
    /// Callbacks do guest já disparados e ainda não entregues.
    ///
    /// Fila única porque todos têm a mesma forma — um endereço de função e até quatro
    /// argumentos — e porque nenhum deles pode rodar no meio do despacho de uma chamada.
    pending_calls: Vec<GuestCall>,
    /// Recursos que **algum** arquivo forneceu. Ver [`Machine::missing_files`].
    recursos_lidos: BTreeSet<u16>,
    /// Se a árvore de widgets já foi despejada na serial.
    despejou: bool,
    /// Contador de criação de widgets. Ver [`Machine::formulario_atual`].
    proximo_serial: u64,
    /// O formulário que está pintado na superfície agora. Ver [`Machine::pinta_widgets`].
    formulario_pintado: u32,
    /// Captura de serial, quando ligada. Ver [`Machine::liga_serial`].
    serial: Option<std::io::BufWriter<std::fs::File>>,
    /// Último erro do EGL, devolvido por `eglGetError`.
    egl_error: u32,
    /// Superfícies do EGL vivas, com as dimensões de cada uma.
    egl_surfaces: HashMap<u32, (u32, u32)>,
    /// Onde os pixels do buffer de cor ficam visíveis para o jogo, e de que tamanho.
    ///
    /// Reservado na primeira vez que a `eglGetColorBufferQUALCOMM` é chamada, e reaproveitado
    /// depois: a região de superfícies não tem como devolver o que já deu, e a Z-Wheel pede o
    /// buffer uma vez por quadro.
    egl_color_buffer: (u32, usize),
    /// Os bytes já convertidos, reaproveitados de uma chamada para a outra.
    egl_color_bytes: Vec<u8>,
    /// Dimensões e memória de trabalho do buffer exposto ao guest. Ele é gravável:
    /// a Z-Wheel copia o fundo para este endereço antes de desenhar o palco.
    egl_color_dimensions: Option<(usize, usize)>,
    egl_color_readback: Vec<u8>,
    /// Próximo identificador livre de superfície ou contexto.
    egl_next_handle: u32,
    /// Quantas vezes o jogo apresentou um quadro com `eglSwapBuffers`.
    egl_swaps: u32,
    /// Quantos `glClear` limparam a cor. Ver [`Machine::gl_swaps`].
    gl_clears: u32,
    /// Último nome de textura ou buffer entregue pelo OpenGL ES.
    gles_next_name: u32,
    /// O objeto `IGLES11`, criado sob demanda pelo `QueryInterface` do EGL.
    gles_object: u32,
    /// Superfície e contexto correntes do EGL.
    egl_surface: u32,
    egl_context: u32,
    /// Estado de reprodução de cada `IMedia` vivo.
    media: HashMap<u32, MediaState>,
    /// Sons já lidos, para não reinterpretar um RIFF de megabytes a cada `Play`.
    waves: HashMap<(u32, u32), std::sync::Arc<crate::wav::Sound>>,
    /// Para onde o som vai, quando há para onde.
    audio: Option<crate::audio::Mixer>,
    /// O último quadro que o jogo apresentou, já no tamanho da tela.
    ///
    /// Guardado no `eglSwapBuffers` porque é ali que o quadro está pronto: ler o buffer no fim
    /// da execução pega o desenho pela metade, quase sempre logo depois do `Clear`.
    gl_last_frame: Vec<u8>,
    /// Estado e buffers do OpenGL ES.
    gl: GlState,
    /// Vetores do cliente: posição, cor e coordenada de textura.
    gl_vertices: ArrayPointer,
    gl_colors: ArrayPointer,
    gl_texcoords: ArrayPointer,
    /// O vetor de normais do `glNormalPointer`. Sempre três componentes — a função nem recebe
    /// tamanho.
    gl_normals: ArrayPointer,
    /// A normal do `glNormal3x`, usada quando não há vetor. O padrão do OpenGL é `(0, 0, 1)`.
    gl_normal_atual: [f32; 3],
    /// Strings constantes já copiadas para a memória do guest, indexadas pelo texto.
    interned: HashMap<&'static str, u32>,
    /// Estado de cada `IThread` vivo.
    threads: HashMap<u32, ThreadState>,
    /// Callback de retomada de cada thread, indexado pelo endereço do `AEECallback`.
    resume_callbacks: HashMap<u32, u32>,
    /// Threads prontas para rodar na próxima fronteira entre chamadas de API.
    pending_threads: Vec<u32>,
    /// Desfecho que interrompeu um callback ou uma thread, para o laço de quadros contar.
    stalled: Option<Outcome>,
    /// A thread em execução, se houver — retomar uma thread de dentro dela mesma seria
    /// reentrância, não concorrência.
    current_thread: Option<u32>,
    /// Buffer de pixels no guest de cada superfície exposta como `IDIB`.
    dib_buffers: HashMap<u32, u32>,
    /// Quantos bytes o buffer publicado de cada `IDIB` tem.
    ///
    /// Existe porque o endereço de um objeto **volta a ser usado**: liberado o anterior, o
    /// próximo bitmap nasce no mesmo lugar, e com outro tamanho. Sem a capacidade não há como
    /// decidir entre reaproveitar o buffer e reservar outro.
    dib_capacity: HashMap<u32, u32>,
    /// Próximo endereço livre na região de superfícies.
    surface_next: u32,
    /// Cor tratada como transparente em cada superfície.
    transparency: HashMap<u32, u16>,
    /// A superfície da tela — o "device bitmap" do BREW. Zero enquanto ninguém pediu.
    device_bitmap: u32,
    /// Onde o `IDisplay` desenha. Normalmente é a tela.
    display_target: u32,
    /// O retângulo de recorte de `IDisplay`. `None` é a superfície inteira, que é o padrão do
    /// BREW e o que vale antes do primeiro `SetClipRect`.
    clip: Option<Rect>,
    /// A tela, para quando ainda não existe device bitmap.
    screen: Framebuffer,
    /// Cores ativas do `IDisplay`, indexadas pelo `AEEClrItem` (`CLR_USER_TEXT` = 1 em diante).
    colors: [Rgb; CLR_COUNT],
    /// Textos que o jogo mandou desenhar. Ainda não temos fonte para rasterizá-los, então
    /// ficam registrados aqui em vez de desaparecerem.
    pending_text: Vec<String>,
    /// A fonte do próprio jogo, quando ele empacota uma.
    font: Option<crate::font::Font>,
    /// Quantas vezes cada método foi chamado — o retrato do que o jogo usa.
    calls: BTreeMap<(u32, u32), u64>,
    /// Total de chamadas atendidas, para aplicar o teto.
    calls_total: u64,
}

impl<C: CpuBackend> Machine<C> {
    /// `root` é o diretório do módulo — a raiz do sistema de arquivos que o jogo enxerga.
    pub fn new(cpu: C, module: LoadedModule, root: impl Into<std::path::PathBuf>) -> Self {
        let raiz: std::path::PathBuf = root.into();
        let heap = Heap::new(loader::HEAP_BASE, loader::HEAP_SIZE);
        // Os objetos ficam depois dos ponteiros que o carregador já reservou no começo da
        // região, para não sobrescrevê-los.
        let reserved = module.out_module + 8 - loader::OBJECT_BASE;
        let mut objects = ObjectStore::new(
            loader::OBJECT_BASE + reserved,
            loader::OBJECT_SIZE - reserved as usize,
        );
        objects.adopt(module.shell, Interface::Shell);
        Self {
            cpu,
            module,
            heap,
            objects,
            unknown_classes: BTreeSet::new(),
            web_requests: BTreeSet::new(),
            collections: HashMap::new(),
            databases: HashMap::new(),
            probe_classes: BTreeSet::new(),
            probe_answers: HashMap::new(),
            // 6 de janeiro de 1980 é a época do BREW; a do Unix é dez anos e seis dias antes.
            epoch_seconds: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().saturating_sub(315_964_800) as u32)
                .unwrap_or(0),
            probe_objects: HashMap::new(),
            probe_log: Vec::new(),
            suspicious_objects: BTreeSet::new(),
            assumptions: BTreeSet::new(),
            bad_pointers: BTreeSet::new(),
            graphics: GraphicsState::default(),
            vfs: {
                let mut vfs = Vfs::new(raiz.clone());
                // Todos os jogos compartilham o mesmo `fs:/`, como no console.
                vfs.set_device_root(crate::archive::device_dir());
                vfs
            },
            open_files: HashMap::new(),
            file_error: SUCCESS,
            decoders: HashMap::new(),
            feeds: HashMap::new(),
            surface_manip: 0,
            imageon_ext: 0,
            scale_source: None,
            prefs: HashMap::new(),
            enumerations: HashMap::new(),
            missing_files: BTreeSet::new(),
            input_signals: BTreeMap::new(),
            signals: HashMap::new(),
            pending_signals: Vec::new(),
            pads: [Pad::default(); input::PORTAS],
            pad_events: std::array::from_fn(|_| std::collections::VecDeque::new()),
            // Uma porta com controle é o que sempre houve; a interface muda isto ao aplicar os
            // ajustes, e o modo sem janela nunca mexe.
            portas: std::array::from_fn(|n| {
                (n == 0).then_some(crate::bindings::Aparelho::Controle)
            }),
            portas_de_aparelho: HashMap::new(),
            teclas: std::collections::VecDeque::new(),
            pad_log: std::collections::VecDeque::new(),
            current_applet: 0,
            random_state: 0x1234_5678,
            clock_us: 0,
            spin_polls: 0,
            next_vsync_us: 0,
            trace: Vec::new(),
            tracing: false,
            trace_filter: None,
            debug_output: Vec::new(),
            bitmaps: HashMap::new(),
            fault_regs: [0; 12],
            fault_stack: Vec::new(),
            timers: Vec::new(),
            applet_class: 0,
            installed_applets: HashSet::new(),
            pending_launch: None,
            wheel_boot_skipped: false,
            nesting: 0,
            pending_probes: Vec::new(),
            pending_blits: Vec::new(),
            probed: HashSet::new(),
            ciphers: HashMap::new(),
            hashes: HashMap::new(),
            resources: crate::resfile::ResCache::default(),
            unzips: HashMap::new(),
            images: HashMap::new(),
            image_bitmaps: HashMap::new(),
            api_time: HashMap::new(),
            profiling_api: false,
            image_notify: HashMap::new(),
            parametros_de_colecao: HashMap::new(),
            vetores: HashMap::new(),
            sources: HashMap::new(),
            peeks: HashMap::new(),
            widgets: HashMap::new(),
            config_items: HashMap::new(),
            network: true,
            // Pelo mesmo motivo, o desvio de servidor também vem do ambiente:
            // `ZEEBX_SERVIDOR=127.0.0.1:8080`. Sem isso, apontar um jogo para um servidor de
            // testes exigiria a porta 80, que pede privilégio.
            network_to: std::env::var("ZEEBX_SERVIDOR").ok(),
            // A ponte vem ligada. Ela derrubou o jogo enquanto entregava de dentro do despacho,
            // e por isso ficou desligada por um tempo; com a entrega na fronteira de chamada
            // isso não acontece mais, e deixá-la desligada só criava um caso em que o jogo
            // parece quebrado por falta de uma variável de ambiente. O `ZEEBX_SEM_PONTE`
            // desliga.
            bridge: std::env::var_os("ZEEBX_SEM_PONTE").is_none(),
            pending_response: None,
            pending_end: None,
            delivered: Vec::new(),
            plaintexts: std::collections::VecDeque::new(),
            missing_apis: BTreeSet::new(),
            falhas_engolidas: BTreeSet::new(),
            ignored_gl: BTreeSet::new(),
            web_response: Vec::new(),
            streams: HashMap::new(),
            sounds: HashMap::new(),
            pending_calls: Vec::new(),
            recursos_lidos: BTreeSet::new(),
            despejou: false,
            proximo_serial: 0,
            formulario_pintado: 0,
            serial: None,
            egl_error: gles::EGL_SUCCESS,
            egl_surfaces: HashMap::new(),
            egl_color_buffer: (0, 0),
            egl_color_bytes: Vec::new(),
            egl_color_dimensions: None,
            egl_color_readback: Vec::new(),
            egl_next_handle: EGL_HANDLE_BASE,
            egl_swaps: 0,
            gl_clears: 0,
            gles_next_name: 0,
            gles_object: 0,
            egl_surface: 0,
            egl_context: 0,
            media: HashMap::new(),
            waves: HashMap::new(),
            audio: None,
            gl_last_frame: Vec::new(),
            gl: GlState::new(SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize),
            gl_vertices: ArrayPointer::default(),
            gl_colors: ArrayPointer::default(),
            gl_texcoords: ArrayPointer::default(),
            gl_normals: ArrayPointer::default(),
            gl_normal_atual: [0.0, 0.0, 1.0],
            interned: HashMap::new(),
            threads: HashMap::new(),
            resume_callbacks: HashMap::new(),
            pending_threads: Vec::new(),
            stalled: None,
            current_thread: None,
            dib_buffers: HashMap::new(),
            dib_capacity: HashMap::new(),
            surface_next: loader::SURFACE_BASE,
            transparency: HashMap::new(),
            device_bitmap: 0,
            display_target: 0,
            clip: None,
            screen: Framebuffer::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32),
            colors: default_colors(),
            pending_text: Vec::new(),
            font: font_do_modulo(&raiz),
            calls: BTreeMap::new(),
            calls_total: 0,
        }
    }

    /// Liga o registro de todas as chamadas, na ordem.
    pub fn set_tracing(&mut self, on: bool) {
        self.tracing = on;
    }

    /// Restringe o rastreamento às chamadas cujo nome contém `part`.
    ///
    /// Sem isso, um jogo que faz milhares de `strlen` por quadro empurra para fora da janela
    /// justamente as chamadas que se quer ver.
    pub fn set_trace_filter(&mut self, part: Option<String>) {
        self.trace_filter = part;
    }

    /// As chamadas registradas, na ordem.
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Chama uma função do guest e espera o retorno.
    ///
    /// É o caminho inverso do despacho de API: aqui somos nós que chamamos o módulo. Montamos
    /// os argumentos pela AAPCS, apontamos `lr` para o sentinela e rodamos o mesmo laço — se o
    /// guest chamar alguma API no meio do caminho, ela é atendida normalmente.
    ///
    /// Só os quatro primeiros argumentos são suportados; nenhuma API do BREW que precisamos
    /// hoje passa mais que isso pela pilha.
    pub fn call_guest(
        &mut self,
        func: u32,
        args: [u32; 4],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        self.cpu.write_reg(Reg::R0, args[0]);
        self.cpu.write_reg(Reg::R1, args[1]);
        self.cpu.write_reg(Reg::R2, args[2]);
        self.cpu.write_reg(Reg::R3, args[3]);
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        self.execute(func, budget)
    }

    /// Chama uma função do guest com mais de quatro argumentos.
    ///
    /// A AAPCS põe os quatro primeiros em `r0..r3` e o resto na pilha, em ordem crescente a
    /// partir de `sp`, que precisa estar alinhado em 8 bytes na entrada da função. É assim que
    /// se chama o `IBITMAP_BltIn`, que tem nove.
    fn call_guest_with_stack(
        &mut self,
        func: u32,
        regs: [u32; 4],
        extra: &[u32],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        let sp = self.cpu.read_reg(Reg::Sp);
        let bytes: Vec<u8> = extra.iter().flat_map(|word| word.to_le_bytes()).collect();
        // Se a pilha não tem espaço, chamar seria escrever fora dela.
        if (sp as usize) < bytes.len() + 8 {
            return Ok(Outcome::Returned { code: EFAILED });
        }
        let new_sp = (sp - bytes.len() as u32) & !7;
        self.cpu.write_mem(new_sp, &bytes)?;
        self.cpu.write_reg(Reg::Sp, new_sp);
        let outcome = self.call_guest(func, regs, budget);
        self.cpu.write_reg(Reg::Sp, sp);
        outcome
    }

    /// Carrega o módulo no núcleo e executa `AEEMod_Load` até um desfecho.
    ///
    /// `budget` é o teto de instruções **por fatia**, entre duas chamadas de API; existe para
    /// que um laço infinito no guest não trave o emulador.
    pub fn run(&mut self, budget: u64) -> Result<Outcome, CpuError> {
        self.cpu.reset(&self.module.mem)?;
        self.cpu.write_reg(Reg::R0, self.module.shell);
        self.cpu.write_reg(Reg::R1, self.module.helpers);
        self.cpu.write_reg(Reg::R2, self.module.out_module);
        self.cpu
            .write_reg(Reg::Sp, loader::STACK_BASE + loader::STACK_SIZE as u32 - 16);
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);

        self.execute(self.module.entry, budget)
    }

    /// O laço propriamente dito: roda, atende chamadas de API e continua até um desfecho.
    fn execute(&mut self, entry: u32, budget: u64) -> Result<Outcome, CpuError> {
        let mut pc = entry;
        // **Há um teto para o trecho inteiro, além do de cada fatia.** O orçamento era passado
        // a cada `cpu.run` e recomeçava do zero depois de toda chamada de API, então um jogo
        // que chamasse uma API por volta rodava para sempre dentro de **uma** volta do laço de
        // quadros — e enquanto isso nem a entrada do jogador chegava, nem o teto de tempo real
        // era conferido, porque as duas coisas moram no laço de fora.
        //
        // Foi o que prendeu a Z-Wheel em modo de atração: sem ninguém tocar, ela repete a
        // abertura, e cada repetição é uma chamada de API. O jogo estava certo; quem não
        // devolvia a vez éramos nós.
        //
        // O teto tem folga medida. Igual ao orçamento de uma fatia ele quebra jogo que trabalha
        // muito num quadro só — o Zeeboids parava no meio, com dois segundos e meio em vez de
        // dez. Com quatro vezes, o Zeeboids roda idêntico (mil quatrocentos e cinquenta milhões
        // de instruções em 12.978 voltas, os mesmos números de antes) e a Z-Wheel devolve a vez
        // sete vezes mais cedo, que é a diferença entre a interface responder e congelar.
        //
        // E estourar o teto **não é fim de jogo**: é pedido de vez. Quem chama trata o
        // `Outcome::Budget` como volta normal — tratá-lo como desfecho ruim parava a Z-Wheel na
        // primeira volta, porque ela repete a abertura enquanto ninguém toca e cada repetição
        // gasta orçamento.
        let comeco = self.cpu.instructions();
        let teto = budget.saturating_mul(TETO_DE_TRECHO);
        loop {
            let gasto = self.cpu.instructions().saturating_sub(comeco);
            let fatia = teto.saturating_sub(gasto).min(budget);
            if fatia == 0 {
                return Ok(Outcome::Budget);
            }
            match self.cpu.run(pc, fatia)? {
                StopReason::ApiCall { addr } if self.calls_total >= MAX_CALLS => {
                    let _ = addr;
                    return Ok(Outcome::CallLimit {
                        calls: self.calls_total,
                    });
                }
                StopReason::ApiCall { addr } => match self.dispatch(addr)? {
                    Some(result) => {
                        self.cpu.write_reg(Reg::R0, result);
                        // O `lr` guarda o endereço de retorno, com o bit 0 indicando Thumb —
                        // o núcleo entende essa convenção, então repassamos como está.
                        pc = self.cpu.read_reg(Reg::Lr);
                        // Aqui é a fronteira entre duas chamadas de API — o único ponto em que
                        // dá para entrar no guest sem interromper nada pela metade.
                        self.run_pending_callbacks(budget)?;
                    }
                    None => {
                        // Registrar aqui, e não só onde o desfecho é lido, porque nem todo
                        // desfecho é lido: uma API que falta **dentro de um retorno de
                        // chamada** aborta aquela chamada e a execução segue, sem deixar
                        // rastro. Foi assim que o slot 12 do widget passou despercebido — a
                        // Z-Wheel montava a tela inteira, morria calada no retorno da imagem, e
                        // o relatório saía limpo.
                        let caller = self.cpu.read_reg(Reg::Lr);
                        self.missing_apis
                            .insert(format!("{} (de {caller:#010x})", aee::describe(addr)));
                        return Ok(Outcome::Unimplemented {
                            addr,
                            args: self.args(),
                            caller,
                        });
                    }
                },
                StopReason::Returned => {
                    return Ok(Outcome::Returned {
                        code: self.cpu.read_reg(Reg::R0),
                    });
                }
                StopReason::MemoryFault { addr, pc } => {
                    let lr = self.cpu.read_reg(Reg::Lr);
                    self.fault_stack = self.scan_stack();
                    self.fault_regs = [
                        Reg::R0,
                        Reg::R1,
                        Reg::R2,
                        Reg::R3,
                        Reg::R4,
                        Reg::R5,
                        Reg::R6,
                        Reg::R7,
                        Reg::R8,
                        Reg::R9,
                        Reg::R10,
                        Reg::R11,
                    ]
                    .map(|reg| self.cpu.read_reg(reg));
                    // Registrar aqui, e não só onde o desfecho é lido, pelo mesmo motivo das
                    // APIs que faltam: um acesso inválido **dentro de um retorno de chamada**
                    // aborta aquela chamada e a execução segue, e quem mandou o evento lê
                    // apenas "ninguém tratou". Foi assim que o `SendEvent` do
                    // `0x885d8` da Z-Wheel sumia do relatório enquanto derrubava o formulário
                    // do z-pad com erro 6.
                    self.falhas_engolidas
                        .insert(format!("acesso inválido a {addr:#010x} em pc {pc:#010x}"));
                    return Ok(Outcome::Fault { addr, pc, lr });
                }
                StopReason::Exception { pc } => return Ok(Outcome::Exception { pc }),
                StopReason::Budget => return Ok(Outcome::Budget),
            }
        }
    }

    /// Atende uma chamada. `None` significa "ainda não implementada".
    fn dispatch(&mut self, addr: u32) -> Result<Option<u32>, CpuError> {
        let Some((iface, slot)) = aee::decode(addr) else {
            return Ok(None);
        };
        *self.calls.entry((iface as u32, slot)).or_insert(0) += 1;
        self.calls_total += 1;
        self.note_spin(iface, slot);
        let traced = self.tracing
            && match &self.trace_filter {
                Some(part) => aee::describe(addr).contains(part.as_str()),
                None => true,
            };
        if traced {
            let args = self.args();
            // Guarda as últimas chamadas: num jogo que faz dezenas de milhares delas, o que
            // interessa é o fim, não o começo.
            if self.trace.len() >= MAX_TRACE {
                self.trace.remove(0);
            }
            self.trace.push(format!(
                "{} (r0={:#x} r1={:#x} r2={:#x} r3={:#x}) volta em {:#010x}",
                aee::describe(addr),
                args[0],
                args[1],
                args[2],
                args[3],
                self.cpu.read_reg(Reg::Lr)
            ));
        }

        let entry = if traced {
            self.trace.len().checked_sub(1)
        } else {
            None
        };
        // Um ponteiro ruim vindo do guest não pode derrubar o emulador: viramos `EBADPARM`,
        // que é o que o BREW responde nesse caso, e registramos para aparecer no relatório.
        let started = self.profiling_api.then(std::time::Instant::now);
        let result = match self.dispatch_inner(iface, slot) {
            Ok(Some(value)) => value,
            Ok(None) => return Ok(None),
            Err(err) => {
                self.bad_pointers
                    .insert(format!("{} ({err})", aee::describe(addr)));
                EBADPARM
            }
        };
        if let Some(started) = started {
            *self.api_time.entry((iface as u32, slot)).or_insert(0) +=
                started.elapsed().as_nanos() as u64;
        }
        // Anexa o retorno à linha do rastreamento: sem ele não dá para ver qual chamada
        // devolveu o erro que fez o jogo desistir.
        if let Some(line) = entry.and_then(|i| self.trace.get_mut(i)) {
            line.push_str(&format!(" -> {result:#x}"));
        }
        Ok(Some(result))
    }

    /// O despacho propriamente dito, separado para que falhas de acesso à memória do guest
    /// possam ser tratadas em [`Machine::dispatch`].
    fn dispatch_inner(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let result = match (iface, slot) {
            (Interface::Helpers, _) => match self.helper_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::MediaUtil, _) | (Interface::Media, _) => {
                match self.media_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Egl, _) | (Interface::EglLegacy, _) => match self.egl_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Gles, _) | (Interface::GlLegacy, _) => {
                match self.gles_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Thread, _) => match self.thread_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Shell, 2) => self.shell_create_instance()?,
            (Interface::Shell, 4) => self.shell_get_device_info()?,
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("StartApplet" | "CanStartApplet")
                ) =>
            {
                let cls = self.cpu.read_reg(Reg::R1);
                if self.installed_applets.contains(&cls) {
                    if Interface::Shell.method(slot) == Some("StartApplet") {
                        self.pending_launch = Some(cls);
                    }
                    SUCCESS
                } else {
                    ECLASSNOTSUPPORT
                }
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("ActiveApplet") => {
                self.applet_class
            }

            (Interface::Shell, slot)
                if Interface::Shell.method(slot) == Some("GetDeviceInfoEx") =>
            {
                self.shell_get_device_info_ex()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("LoadResObject") => {
                self.shell_load_res_object()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("LoadResString") => {
                self.shell_load_res_string()?
            }
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("LoadResData" | "LoadResDataEx")
                ) =>
            {
                let with_type = Interface::Shell.method(slot) == Some("LoadResDataEx");
                self.shell_load_res_data(with_type)?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("FreeResData") => {
                self.heap.free(self.cpu.read_reg(Reg::R1));
                SUCCESS
            }
            // Entrega um evento a um applet. O Z-Wheel manda um para **ele mesmo** — a string
            // dele diz o motivo: "SendEvent to get PrefsDB failed", ou seja, é assim que uma
            // parte do app pede à outra o ponteiro do banco de preferências.
            //
            // A chamada observada tem a forma do `PostEventEx`: `r1` são sinalizadores (zero),
            // `r2` é o ClassID e `r3` o evento. Como a forma de cinco argumentos do `SendEvent`
            // põe o ClassID em `r1`, lemos as duas: quem manda é qual dos dois registradores
            // traz o ClassID do applet que está rodando.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("SendEvent") => {
                let (a1, a2, a3) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let (cls, evt, w) = match a2 == self.applet_class {
                    true => (a2, a3, self.stack_arg(0)? as u16),
                    false => (a1, a2, a3 as u16),
                };
                let dw = self.stack_arg(1)?;
                self.send_applet_event(cls, evt, w, dw)?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("GetHandler") => {
                let mime = self
                    .cpu
                    .read_cstring(self.cpu.read_reg(Reg::R2), MAX_STRING);
                handler_for(&mime)
            }
            // int ISHELL_GetPrefs(IShell *, AEECLSID cls, uint16 wVer, void *pCfg, uint16 nSize)
            // int ISHELL_SetPrefs(IShell *, AEECLSID cls, uint16 wVer, void *pCfg, uint16 nSize)
            //
            // As preferências vivem só enquanto o emulador roda. Guardá-las em disco seria
            // inventar um formato: o console tinha um, e não sabemos qual. O que importa é que
            // um jogo que grava e relê no mesmo instante encontre o que gravou.
            (Interface::Shell, slot)
                if matches!(Interface::Shell.method(slot), Some("GetPrefs" | "SetPrefs")) =>
            {
                let (cls, version) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let (buffer, size) = (self.cpu.read_reg(Reg::R3), self.stack_arg(0)? as usize);
                let key = (cls, version as u16);
                match Interface::Shell.method(slot) == Some("SetPrefs") {
                    true => {
                        let mut bytes = vec![0u8; size.min(MAX_PREFS)];
                        if buffer != 0 {
                            self.cpu.read_mem(buffer, &mut bytes)?;
                        }
                        self.prefs.insert(key, bytes);
                        SUCCESS
                    }
                    false => match self.prefs.get(&key) {
                        // Sem espaço, ou sem destino, o BREW devolve o tamanho do registro.
                        Some(bytes) if buffer == 0 || size < bytes.len() => bytes.len() as u32,
                        Some(bytes) => {
                            let bytes = bytes.clone();
                            self.cpu.write_mem(buffer, &bytes)?;
                            SUCCESS
                        }
                        None => EFAILED,
                    },
                }
            }
            // boolean ISHELL_Prompt(IShell *, AEEPromptInfo *pi)
            //
            // Falso é "não criei o diálogo", que é a verdade: não temos interface de diálogo.
            // O BREW prevê essa resposta, e o jogo segue pelo caminho de quem não pôde
            // perguntar em vez de parar aqui.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("Prompt") => FALSE,
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("DetectType") => {
                self.shell_detect_type()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("Resume") => {
                self.shell_resume()?
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("GetClassItemID") => {
                self.shell_get_class_item_id()
            }
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("SetTimer" | "CancelTimer" | "GetTimerExpiration")
                ) =>
            {
                self.shell_timer_call(Interface::Shell.method(slot).unwrap_or(""))?
            }
            (Interface::Signal, _)
            | (Interface::SignalCtl, _)
            | (Interface::SignalCbFactory, _) => match self.signal_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Graphics, _) | (Interface::Display, _) | (Interface::Bitmap, _) => {
                let whole = iface.method(slot).is_none_or(touches_whole_surface);
                if whole {
                    self.sync_surfaces_in()?;
                }
                let handled = match iface {
                    Interface::Graphics => self.graphics_call(slot)?,
                    Interface::Display => self.display_call(slot)?,
                    _ => self.bitmap_call(slot)?,
                };
                let Some(result) = handled else {
                    return Ok(None);
                };
                if whole {
                    self.sync_surfaces_out()?;
                }
                result
            }
            (Interface::FileMgr, _) | (Interface::File, _) => match self.file_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Sound, _) => match self.sound_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Heap, _) => match self.heap_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::EglSurfaceManip, _) | (Interface::GlesImageonExt, _) => {
                match self.extension_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::ImageDecoder, _) | (Interface::ForceFeed, _) => {
                match self.decoder_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::UnzipStream, _) => match self.unzip_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::License, _) => match self.license_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Web, _)
            | (Interface::Hash, _)
            | (Interface::CipherFactory, _)
            | (Interface::Cipher, _) => match self.crypto_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::MemAStream, _) => match self.stream_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Image, _) => {
                // Só os três que desenham pagam a cópia. O Pac-Mania faz 168 mil `SetParm` e
                // 168 mil `Draw` em cinco segundos virtuais, um par por sprite; cobrar a
                // superfície inteira de cada `SetParm` — que não põe um pixel na tela — eram 52
                // segundos de relógio, metade de tudo que o emulador gastava atendendo o jogo.
                let whole = iface.method(slot).is_none_or(touches_whole_surface);
                if whole {
                    self.sync_surfaces_in()?;
                }
                let Some(result) = self.image_call(slot)? else {
                    return Ok(None);
                };
                if whole {
                    self.sync_surfaces_out()?;
                }
                result
            }
            (Interface::Config, _) => match self.config_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::ZeeboMcp, _) => match self.zeebo_mcp_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Widget, _) => match self.widget_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Control, _) => match self.control_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SimCardCtl, _) => match self.sim_card_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SystemCtl, _) => match self.system_ctl_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Cm, _) => match self.cm_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            // A `0x01028e3c` não tem estado nem método próprio: só a contagem.
            (Interface::Classe28e3c, 0) => self.objects.add_ref(self.cpu.read_reg(Reg::R0)),
            (Interface::Classe28e3c, 1) => self.objects.release(self.cpu.read_reg(Reg::R0)),
            // `slot3(this, &saída)`, na `0x8588c` e na `0x858a8`: dois objetos desta classe são
            // consultados em sequência, cada um enchendo um pedaço da mesma estrutura, e o
            // chamador sai fora se qualquer um devolver diferente de zero.
            //
            // **Este era o fim do ciclo de atração.** Recusar o método abortava o retorno de
            // chamada inteiro, calado — 1722 vezes, todas no primeiro segundo, e depois o jogo
            // emudecia. A linha `I28e3c::slot[3]` estava no relatório desde sempre; foi preciso
            // registrar o desfecho de cada callback para ver que era ela que matava a cadeia.
            //
            // O que a estrutura guarda ainda não sabemos. Zerar os 0x18 bytes entre os dois
            // destinos (`r4+8` e `r4+0x20`) e responder sucesso é a resposta mínima que deixa o
            // jogo seguir, e fica anotada como hipótese.
            // `slot5(this, &saída, 0, 0)`, na `0x86238`: o chamador lê uma **meia palavra** de
            // volta e a guarda em `[r5+0x10]`. Tem cara de medida — uma altura, uma contagem.
            //
            // Respondemos zero e anotamos. Um zero já derrubou o jogo uma vez, no passo de lista
            // do slot 5 do widget, então este fica sob suspeita: se aparecer divisão por zero ou
            // laço, é aqui que se olha primeiro.
            (Interface::Classe28e3c, 5) => {
                let saida = self.cpu.read_reg(Reg::R1);
                if saida != 0 {
                    self.cpu.write_mem(saida, &0u16.to_le_bytes())?;
                }
                self.assumptions.insert(
                    "a 0x01028e3c respondeu zero a uma medida, e não sabemos o que ela mede",
                );
                SUCCESS
            }
            (Interface::Classe28e3c, 3) => {
                /// A distância entre os dois destinos na `0x85880`.
                const QUANTO: usize = 0x18;

                let saida = self.cpu.read_reg(Reg::R1);
                if saida != 0 {
                    self.cpu.write_mem(saida, &[0u8; QUANTO])?;
                }
                self.assumptions.insert(
                    "a 0x01028e3c respondeu uma consulta zerada, e não sabemos o que ela guarda",
                );
                SUCCESS
            }
            (Interface::Typeface, _) => match self.typeface_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Vetor, _) => match self.vetor_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Source, _) | (Interface::Peek, _) => {
                match self.source_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::SourceUtil, _) => match self.source_util_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Collection, _) => match self.collection_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::SqlMgr, _) | (Interface::SqlDatabase, _) => {
                match self.sql_call(iface, slot)? {
                    Some(result) => result,
                    None => return Ok(None),
                }
            }
            (Interface::Probe, _) => self.probe_call(slot)?,
            (Interface::Hid, _) | (Interface::HidDevice, _) => match self.hid_call(iface, slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            // Slots 0 e 1 de toda interface são AddRef e Release, herdados de IBase.
            (_, 0) => {
                let obj = self.cpu.read_reg(Reg::R0);
                self.check_object(obj, iface);
                self.objects.add_ref(obj)
            }
            (_, 1) => {
                let obj = self.cpu.read_reg(Reg::R0);
                self.check_object(obj, iface);
                self.objects.release(obj)
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Dá uma volta no laço de eventos: pula o tempo ocioso, dispara o que venceu e deixa o
    /// guest correr.
    ///
    /// Não existe passo fixo de tempo. O relógio anda com o trabalho que o guest faz — a
    /// contagem de instruções — e o ocioso é pulado até o próximo evento, como faz o
    /// escalonador de qualquer emulador. Com um passo fixo por volta do laço, o tempo virtual
    /// corria muito mais rápido que o jogo: o Crash gastava quinze voltas para desenhar um
    /// quadro, via meio segundo ter passado entre um e outro, e integrava a física com isso.
    ///
    /// Precisa rodar fora do despacho de uma chamada, como a fila de sinais: os callbacks
    /// executam no guest.
    pub fn advance(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        self.skip_idle_time();

        // Os vencidos saem da lista *antes* de rodar, porque o callback tipicamente rearma o
        // timer — e o rearmado não pode disparar já neste mesmo quadro.
        let now = self.now_ms();
        let mut due = Vec::new();
        self.timers.retain(|timer| {
            if timer.deadline_ms <= now {
                due.push(*timer);
                false
            } else {
                true
            }
        });

        let mut outcomes = Vec::new();
        for timer in due {
            let call = self.resolve_notify(timer.callback)?;
            if call.function == 0 {
                continue;
            }
            outcomes.push(self.call_guest(call.function, [call.context, 0, 0, 0], budget)?);
        }
        self.run_pending_callbacks(budget)?;
        self.run_pending_threads(budget)?;

        // O que interrompeu um callback ou uma thread também é desfecho do quadro.
        outcomes.extend(self.stalled.take());
        Ok(outcomes)
    }

    /// Copia um trecho da memória do guest, para inspeção externa.
    ///
    /// Existe para depuração: quando o jogo quebra num ponteiro nulo, o que responde "por quê"
    /// é a struct que o levou até lá, e ela só existe na memória do guest.
    pub fn dump(&self, addr: u32, len: usize) -> Result<Vec<u8>, CpuError> {
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(bytes)
    }

    /// `r0..r11` no momento da última falha de memória.
    pub fn fault_regs(&self) -> [u32; 12] {
        self.fault_regs
    }

    /// Endereços de retorno encontrados na pilha da última falha.
    pub fn fault_stack(&self) -> &[u32] {
        &self.fault_stack
    }

    /// Varre a pilha à procura de endereços de retorno.
    ///
    /// Não é um backtrace exato — sem tabela de desenrolamento, o que dá para fazer é ler as
    /// palavras da pilha e ficar com as que apontam para logo depois de uma instrução de
    /// chamada. Falsos positivos aparecem, mas a cadeia real aparece junto, e é ela que
    /// responde "como cheguei aqui".
    fn scan_stack(&self) -> Vec<u32> {
        let sp = self.cpu.read_reg(Reg::Sp);
        let top = loader::STACK_BASE + loader::STACK_SIZE as u32;
        let module = &self.module.mem.regions()[0];
        let (low, high) = (module.base, module.base + module.bytes.len() as u32);

        let mut found = Vec::new();
        let mut address = sp;
        while address < top && found.len() < STACK_DEPTH {
            let Ok(value) = self.cpu.read_u32(address) else {
                break;
            };
            address += 4;
            if value < low || value >= high || value % 4 != 0 {
                continue;
            }
            // Um endereço de retorno tem um `bl`/`blx` logo antes dele.
            let Ok(previous) = self.cpu.read_u32(value - 4) else {
                continue;
            };
            let is_call = (previous >> 24) & 0x0f == 0x0b || (previous >> 20) & 0xff == 0x12;
            if is_call {
                found.push(value);
            }
        }
        found
    }

    /// Cria um objeto novo e grava o ponteiro de vtable dele na memória do guest.
    fn new_object(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let Some(addr) = self.objects.create(iface) else {
            return Ok(0);
        };
        self.cpu.write_u32(addr, loader::vtable_addr(iface))?;
        Ok(addr)
    }

    /// Chamadas que receberam ponteiro inválido do guest.
    pub fn bad_pointers(&self) -> Vec<String> {
        self.bad_pointers.iter().cloned().collect()
    }

    /// As extensões gráficas do console: `IEGLSurfaceManip` e `IGLESImageonExt`.
    ///
    /// Elas existem porque dez portes de arcade — todos sobre o mesmo emulador de Neo Geo —
    /// pedem as duas por `QueryInterface` no objeto EGL e desistem da inicialização gráfica
    /// sem elas, escrevendo "InitGLExtensions failed" na tela.
    ///
    /// Quase tudo aqui responde "consegui" sem fazer nada, e isso é deliberado: rotação,
    /// transparência e sobreposição de camadas não mudam o que o jogo desenha, só como o
    /// console compõe o resultado. Recusar faria o jogo desistir por causa de um recurso que
    /// ele nem chega a usar.
    ///
    /// A exceção é a **escala**: ali o jogo diz o tamanho da superfície em que desenha, e essa
    /// informação vale mais que a dedução por viewport que fazemos na falta dela.
    fn extension_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, out) = (self.arg(1), self.arg(2));
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                self.unknown_classes.insert(iid);
                ECLASSNOTSUPPORT
            }
            // int SetSurfaceScale(pMe, dpy, surf, AEEEGLSurfaceScaleRect *src, *dst,
            //                     AEEEGLBoolean *ret)
            "SetSurfaceScale" => {
                let source = self.arg(3);
                if source != 0 {
                    let width = self.cpu.read_u32(source + 8)? as i32;
                    let height = self.cpu.read_u32(source + 12)? as i32;
                    if width > 0 && height > 0 {
                        self.scale_source = Some((width, height));
                        self.gl.set_surface(width as usize, height as usize);
                    }
                }
                self.write_egl_true(4)?
            }
            // int GetSurfaceScale(pMe, dpy, surf, EGLBoolean *enabled, *src, *dst, *ret)
            "GetSurfaceScale" => {
                let (enabled, source, dest) = (self.arg(3), self.arg(4), self.arg(5));
                self.write_at(enabled, u32::from(self.scale_source.is_some()))?;
                let (width, height) = match self.scale_source {
                    Some(size) => size,
                    None => {
                        let (w, h) = self.gl.surface();
                        (w as i32, h as i32)
                    }
                };
                for (rect, size) in [
                    (source, (width, height)),
                    (dest, (SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32)),
                ] {
                    if rect != 0 {
                        self.cpu.write_u32(rect, 0)?;
                        self.cpu.write_u32(rect + 4, 0)?;
                        self.cpu.write_u32(rect + 8, size.0 as u32)?;
                        self.cpu.write_u32(rect + 12, size.1 as u32)?;
                    }
                }
                self.write_egl_true(6)?
            }
            // int GetSurfaceScaleCaps(pMe, dpy, surf, AEEEGLSurfaceScaleCaps *param, *ret)
            //
            // O console amplia da superfície do jogo para a tela; anunciamos exatamente essa
            // faixa. Os fatores são ponto fixo 16.16, como manda o `AEEEGLfixed`.
            "GetSurfaceScaleCaps" => {
                let caps = self.arg(3);
                if caps != 0 {
                    let fields: [u32; 12] = [
                        1 << 16, // MinXScaleFactor: nunca reduz
                        8 << 16, // MaxXScaleFactor
                        1 << 16, // MinYScaleFactor
                        8 << 16, // MaxYScaleFactor
                        1,       // MinSrcWidth
                        SCREEN_WIDTH as u32,
                        1, // MinSrcHeight
                        SCREEN_HEIGHT as u32,
                        1, // MinDstWidth
                        SCREEN_WIDTH as u32,
                        1, // MinDstHeight
                        SCREEN_HEIGHT as u32,
                    ];
                    for (index, value) in fields.iter().enumerate() {
                        self.cpu.write_u32(caps + index as u32 * 4, *value)?;
                    }
                }
                self.write_egl_true(4)?
            }
            // O resto da manipulação de superfície: aceitar sem fazer é honesto porque nada
            // disso muda o que o jogo desenha. O último argumento é sempre o `EGLBoolean *ret`.
            "SurfaceScaleEnable"
            | "SurfaceRotateEnable"
            | "SetSurfaceRotate"
            | "SurfaceTransparencyEnable"
            | "SetSurfaceTransparency"
            | "SetSurfaceTransparencyMap"
            | "SurfaceColorKeyEnable"
            | "SetSurfaceColorKey"
            | "SurfaceOverlayEnable"
            | "SurfaceOverlayLayerEnable"
            | "SurfaceOverlayBind" => {
                let last = EXTENSION_RESULT_SLOT
                    .iter()
                    .find(|(method, _)| *method == name)
                    .map(|(_, slot)| *slot)
                    .unwrap_or(4);
                self.write_egl_true(last)?
            }
            // As consultas que não temos como responder de verdade: zeram a saída e dizem que
            // o recurso não está ligado, que é a verdade.
            "GetSurfaceRotate"
            | "GetSurfaceRotateCaps"
            | "GetSurfaceTransparency"
            | "GetSurfaceTransparencyMap"
            | "GetSurfaceTransparencyCaps"
            | "GetSurfaceColorKey"
            | "GetSurfaceOverlayBinding"
            | "GetSurfaceOverlay"
            | "GetSurfaceOverlayCaps"
            | "CreateCompositeSurface" => {
                for index in 3..8 {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            // `IGLESImageonExt` repete métodos do OpenGL ES com outra assinatura: aqui o `this`
            // é a extensão, então os argumentos vêm um lugar à frente.
            "TexEnvi" | "TexEnviv" | "TexParameteri" | "TexParameteriv" | "TexParameterfv"
            | "TexParameterxv" => {
                let (pname, value) = (self.arg(2), self.arg(3));
                match name.ends_with('v') {
                    true if pname == gles::GL_TEXTURE_CROP_RECT_OES => {
                        let mut crop = [0i32; 4];
                        for (index, item) in crop.iter_mut().enumerate() {
                            *item = self.cpu.read_u32(value + index as u32 * 4)? as i32;
                        }
                        self.gl.set_texture_crop(crop);
                    }
                    true => {
                        let value = self.cpu.read_u32(value)?;
                        self.apply_texture_setting(name, pname, value);
                    }
                    false => self.apply_texture_setting(name, pname, value),
                }
                SUCCESS
            }
            "BlendEquationEXT"
            | "BlendEquationSeparateEXT"
            | "BlendFuncSeparateEXT"
            | "PointSizePointerOES" => SUCCESS,
            // Os buffers de vértice da ATI e da Qualcomm. Nenhum jogo do console chegou a
            // usá-los, e responder sucesso sem guardar nada faria o desenho seguinte sair de
            // lixo — recusar é mais honesto.
            "BindBufferQUALCOMM"
            | "DeleteBuffersQUALCOMM"
            | "GenBuffersQUALCOMM"
            | "BufferDataQUALCOMM"
            | "BufferSubDataQUALCOMM"
            | "IsBufferQUALCOMM"
            | "BufferDataATI"
            | "MeshListATI"
            | "DrawVertexBufferObjectATI"
            | "GetPointerv"
            | "GetMaterialfv"
            | "GetTexParameteriv"
            | "GetTexParameterfv"
            | "GetTexParameterxv" => {
                self.assumptions
                    .insert("o jogo usou um buffer de vértices da extensão, que não temos");
                EUNSUPPORTED
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    fn license_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::License.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "IsExpired" => FALSE,
            // AEELicenseType GetInfo(ILicense *, uint32 *pdwExpire)
            //
            // Com `LT_NONE` a documentação diz que não há valor associado, mas o jogo passa
            // um ponteiro e vai ler o que estiver lá — então escrevemos `BV_UNLIMITED`.
            "GetInfo" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, BV_UNLIMITED)?;
                }
                LT_NONE
            }
            // Só faz sentido em licença por uso; a própria documentação manda devolver
            // `EFAILED` quando o tipo não é `LT_USES`.
            "SetUsesRemaining" => EFAILED,
            // AEEPriceType GetPurchaseInfo(ILicense *, AEELicenseType *plt, uint32 *pdwExpire,
            //                              uint32 *pdSeq)
            "GetPurchaseInfo" => {
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[LT_NONE as u8])?;
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, BV_UNLIMITED)?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                PT_PURCHASE
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// A coleção genérica da interface da Z-Wheel.
    ///
    /// O app a percorre como um cursor: `Reset` uma vez, e depois `GetCurrent`/`AtEnd` até o
    /// fim. Enquanto o `AtEnd` respondia "ainda não" — que é o que a sonda fazia ao devolver
    /// sucesso —, ele girava quinze milhões de vezes.
    ///
    /// Os slots sem nome ainda não apareceram; se aparecerem, o relatório avisa em vez de
    /// fingir que foram atendidos. É por isso que eles não têm nome na tabela.
    /// Atende a `IConfig`. Ver [`Interface::Config`].
    ///
    /// `int ICONFIG_GetItem(IConfig *pMe, ConfigItem nItem, void *pBuff, int nSize)` e o
    /// `SetItem` de mesma forma. Os itens vivem enquanto o emulador roda, como as preferências
    /// do `ISHELL_GetPrefs`: gravá-los em disco seria inventar um formato que o console tinha e
    /// nós não conhecemos. O que precisa valer é que quem grava releia o que gravou.
    fn config_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Config.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.config_items.remove(&this);
                }
                restantes
            }
            "GetItem" => {
                let (item, buffer, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                match self
                    .config_items
                    .get(&this)
                    .and_then(|itens| itens.get(&item))
                {
                    // Devolver menos do que foi pedido seria deixar o resto do buffer com o
                    // que já estava lá, e o jogo leria lixo achando que leu configuração.
                    Some(dados) if dados.len() >= tamanho => {
                        let recorte = dados[..tamanho].to_vec();
                        self.cpu.write_mem(buffer, &recorte)?;
                        SUCCESS
                    }
                    _ => EFAILED,
                }
            }
            "SetItem" => {
                let (item, buffer, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if tamanho == 0 || tamanho > MAX_STRING {
                    return Ok(Some(EFAILED));
                }
                let mut dados = vec![0u8; tamanho];
                self.cpu.read_mem(buffer, &mut dados)?;
                self.config_items
                    .entry(this)
                    .or_default()
                    .insert(item, dados);
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o ZEEBOMCP. Ver [`Interface::ZeeboMcp`].
    ///
    /// Só os três slots lidos no firmware são atendidos; os cinco de baixo caem fora e viram
    /// relatório, que é o que queremos quando a Z-Wheel finalmente usar um deles.
    fn zeebo_mcp_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::ZeeboMcp.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // O do firmware aceita dois IIDs: o da própria classe e o `0x01000001`. Aceitar
            // qualquer um seria dizer que este objeto é toda interface do sistema.
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_ZEEBOMCP && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o controle do cartão SIM. Ver [`Interface::SimCardCtl`].
    fn sim_card_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SimCardCtl.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SIMCARDCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // Guarda o par e não avisa ninguém: não há cartão para verificar, e chamar o
            // retorno seria afirmar que há.
            // Guarda o par e não avisa ninguém: não há cartão para verificar, e chamar o
            // retorno seria afirmar que há. Hoje não chega aqui — a classe não é oferecida.
            "PedirVerificacao" => {
                self.assumptions
                    .insert("uma verificação de cartão SIM foi aceita e nunca respondida");
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o controle de sistema. Ver [`Interface::SystemCtl`].
    ///
    /// O `QueryInterface` do firmware aceita dois IIDs, o `0x01000001` e o da própria classe, e
    /// é isso que fazemos aqui — aceitar qualquer um seria dizer que este objeto é toda
    /// interface do sistema.
    fn system_ctl_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SystemCtl.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let (iid, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                if iid != AEECLSID_SYSTEMCTL && iid != 0x0100_0001 {
                    return Ok(Some(ECLASSNOTSUPPORT));
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            "Consultar" => {
                self.assumptions
                    .insert("o controle de sistema respondeu zero: não há aparelho para consultar");
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o `ICM`. Ver [`Interface::Cm`].
    fn cm_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Deslocamento do estado do serviço dentro do `AEECMSSInfo`.
        const ESTADO_DO_SERVICO: u32 = 0x0;
        /// Deslocamento do modo de operação, lido em `0x87cb0`.
        const MODO_DE_OPERACAO: u32 = 0xc;
        /// Deslocamento da intensidade do sinal, lido em `0x696f8` como meia palavra.
        const INTENSIDADE: u32 = 0x28;
        /// `AEECM_SRV_STATUS_SRV`. A `0x696e8` aceita 1, 2 ou 3 e recusa o resto com
        /// `Service status is NOT available!`; 2 é "serviço pleno".
        const COM_SERVICO: u32 = 2;
        /// `SYS_OPRT_MODE_ONLINE`. É com este número que a `0x77564` compara.
        const NO_AR: u32 = 5;
        /// A `0x69830` transforma a intensidade em barras por faixas de nove: `0x45..=0x4d`
        /// são quatro barras, e é onde este número cai.
        const SINAL: u16 = 0x48;
        /// O menor buffer que responde às três leituras que conhecemos.
        const MINIMO: usize = INTENSIDADE as usize + 2;

        let Some(name) = Interface::Cm.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // int ICM_GetSSInfo(ICM *, AEECMSSInfo *pInfo, uint32 nSize)
            //
            // Uma chamada, dois leitores: a `0x87c90` quer o modo de operação em `+0xc`, e a
            // `0x696a0` quer o estado do serviço em `+0` e, quando ele é 2, a intensidade do
            // sinal em `+0x28`. Quem só respondia ao primeiro deixava o segundo repetindo
            // `Service status is NOT available!` para sempre.
            "GetSSInfo" => {
                let (info, tamanho) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                if info == 0 || tamanho < MINIMO {
                    return Ok(Some(EBADPARM));
                }
                // Zerar o resto é parte da resposta: o jogo passa um buffer que ele mesmo
                // zerou, mas quem chama esta função não pode contar com isso.
                self.cpu.write_mem(info, &vec![0u8; tamanho])?;
                self.cpu.write_u32(info + ESTADO_DO_SERVICO, COM_SERVICO)?;
                self.cpu.write_u32(info + MODO_DE_OPERACAO, NO_AR)?;
                self.cpu
                    .write_mem(info + INTENSIDADE, &SINAL.to_le_bytes())?;
                self.assumptions.insert(
                    "o ICM respondeu rádio no ar e serviço pleno, com o resto da AEECMSSInfo zerado",
                );
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende a lista genérica da Z-Wheel. Ver [`Interface::Vetor`].
    fn vetor_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// O índice que o jogo passa para dizer "no fim".
        const NO_FIM: u32 = u32::MAX;

        let Some(name) = Interface::Vetor.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.vetores.remove(&this);
                }
                restantes
            }
            "Tamanho" => self
                .vetores
                .get(&this)
                .map_or(0, |(itens, _)| itens.len() as u32),
            "PegarEm" => {
                let (indice, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let item = self
                    .vetores
                    .get(&this)
                    .and_then(|(itens, _)| itens.get(indice as usize).copied());
                match item {
                    Some(item) => {
                        if saida != 0 {
                            self.cpu.write_u32(saida, item)?;
                        }
                        SUCCESS
                    }
                    // Fora da faixa não escreve nada: deixar a saída como estava é o que
                    // permite ao chamador distinguir "não tem" de "tem e é nulo".
                    None => EBADPARM,
                }
            }
            "InserirEm" => {
                let (indice, item) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some((itens, _)) = self.vetores.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let onde = match indice {
                    NO_FIM => itens.len(),
                    n => (n as usize).min(itens.len()),
                };
                itens.insert(onde, item);
                SUCCESS
            }
            // O par `RemoverEm(0)` + `PegarEm(0)` em `0x7d788` é um laço que drena a lista: o
            // jogo tira o primeiro, pega o novo primeiro e repete até não haver mais. Sem o
            // `RemoverEm` de verdade ele nunca acaba — foram sete milhões de voltas até o
            // orçamento de instruções estourar.
            "RemoverEm" => {
                let indice = self.cpu.read_reg(Reg::R1) as usize;
                let Some((itens, _)) = self.vetores.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                if indice >= itens.len() {
                    return Ok(Some(EBADPARM));
                }
                itens.remove(indice);
                SUCCESS
            }
            // O liberador é ponteiro de função do módulo, e é para ele que o `Esvaziar` do
            // console entrega cada item. Aqui ele só é guardado — ver a nota no `Esvaziar`.
            "DefinirLiberador" => {
                if let Some((_, liberador)) = self.vetores.get_mut(&this) {
                    *liberador = self.cpu.read_reg(Reg::R1);
                }
                SUCCESS
            }
            // Esvaziar **sem** chamar o liberador de cada item é uma dívida consciente: quem
            // alocou os itens foi o jogo, e chamar código dele no meio de um despacho é o
            // caminho que já derrubou o Zeeboids uma vez. O custo é memória que não volta ao
            // heap do jogo enquanto ele roda, e é por isso que a hipótese fica registrada.
            "Esvaziar" => {
                if let Some((itens, liberador)) = self.vetores.get_mut(&this)
                    && !itens.is_empty()
                    && *liberador != 0
                {
                    itens.clear();
                    self.assumptions.insert(
                        "uma lista foi esvaziada sem chamar o liberador que o jogo registrou",
                    );
                } else if let Some((itens, _)) = self.vetores.get_mut(&this) {
                    itens.clear();
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    fn collection_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Collection.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let a1 = self.cpu.read_reg(Reg::R1);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.collections.remove(&this);
                    self.parametros_de_colecao
                        .retain(|(obj, _), _| *obj != this);
                }
                restantes
            }
            "Reset" => {
                if let Some((_, cursor)) = self.collections.get_mut(&this) {
                    *cursor = 0;
                }
                SUCCESS
            }
            // O fim é verdade quando o cursor passou do último item — e uma coleção que
            // ninguém preencheu está no fim desde o começo.
            "AtEnd" => {
                let (itens, cursor) = self
                    .collections
                    .get(&this)
                    .map(|(itens, cursor)| (itens.len(), *cursor))
                    .unwrap_or((0, 0));
                u32::from(cursor >= itens)
            }
            // `slot10(this, id, ponteiro, tamanho)`, visto em `0x7d0c4` com
            // `(0, &{0x01070798}, 4)` — o número passado é o ClassID do próprio applet.
            //
            // O que ele **significa** não dá para dizer: a função que o chama cria a coleção,
            // faz esta chamada e solta o objeto em seguida, sem ler nada de volta. Pode ser
            // "guarde este parâmetro" ou "acrescente este item"; as duas leituras têm o mesmo
            // efeito observável, que é nenhum. Guardar os bytes cobre as duas e não inventa
            // comportamento.
            "Definir" => {
                let (id, ponteiro, tamanho) = (
                    a1,
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3) as usize,
                );
                if ponteiro == 0 || tamanho == 0 || tamanho > MAX_STRING {
                    return Ok(Some(EBADPARM));
                }
                let mut dados = vec![0u8; tamanho];
                self.cpu.read_mem(ponteiro, &mut dados)?;
                self.parametros_de_colecao.insert((this, id), dados);
                SUCCESS
            }
            // O item corrente sai pelo ponteiro de saída, e o cursor anda. Sem item, `EFAILED`.
            "GetCurrent" => {
                let item = self.collections.get_mut(&this).and_then(|(itens, cursor)| {
                    let item = itens.get(*cursor).copied();
                    if item.is_some() {
                        *cursor += 1;
                    }
                    item
                });
                match item {
                    Some(item) => {
                        if a1 != 0 {
                            self.cpu.write_u32(a1, item)?;
                        }
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Liga a medição de tempo real por método de API. Ver [`Machine::api_profile`].
    pub fn enable_api_profile(&mut self) {
        self.profiling_api = true;
    }

    /// Quanto tempo real cada método de API custou, do mais caro para o mais barato.
    pub fn api_profile(&self) -> Vec<(String, u64)> {
        let mut linhas: Vec<_> = self
            .api_time
            .iter()
            .map(|(&(iface, slot), &ns)| {
                let nome = aee::Interface::from_index_public(iface)
                    .and_then(|i| i.method(slot).map(|m| format!("{}::{m}", i.name())))
                    .unwrap_or_else(|| format!("interface {iface} slot {slot}"));
                (nome, ns)
            })
            .collect();
        linhas.sort_unstable_by_key(|linha| std::cmp::Reverse(linha.1));
        linhas
    }

    /// Troca o `GetAppInstance` de trampolim por três palavras de código ARM.
    ///
    /// A função devolve sempre o mesmo ponteiro depois que o applet existe, e é chamada aos
    /// milhões: nos jogos do BREW os globais moram dentro do applet, então todo acesso a um
    /// global passa por ela. Atendê-la pelo trampolim custa parar e religar o núcleo a cada
    /// chamada; como código, ela nem sai da CPU.
    ///
    /// ```asm
    /// ldr r0, [pc]   ; o ponteiro está logo depois do `bx`
    /// bx  lr
    /// .word <applet>
    /// ```
    fn install_app_instance_stub(&mut self, applet: u32) -> Result<(), CpuError> {
        const LDR_R0_PC: u32 = 0xe59f_0000;
        const BX_LR: u32 = 0xe12f_ff1e;
        let Some(slot) = aee_helpers::HELPERS
            .iter()
            .position(|&name| name == "GetAppInstance")
        else {
            return Ok(());
        };
        self.cpu.write_u32(loader::STUB_BASE, LDR_R0_PC)?;
        self.cpu.write_u32(loader::STUB_BASE + 4, BX_LR)?;
        self.cpu.write_u32(loader::STUB_BASE + 8, applet)?;
        self.cpu
            .write_u32(loader::HELPERS_BASE + slot as u32 * 4, loader::STUB_BASE)
    }

    /// Copia uma string constante para a memória do guest, uma vez só.
    ///
    /// `eglQueryString` e `glGetString` devolvem ponteiros que o jogo pode guardar e reler; o
    /// contrato é que eles continuem válidos, então cada texto vira um bloco permanente.
    fn intern(&mut self, text: &'static str) -> Result<u32, CpuError> {
        if let Some(&addr) = self.interned.get(text) {
            return Ok(addr);
        }
        let addr = self.malloc(text.len() as u32 + 1)?;
        if addr != 0 {
            self.write_cstring(addr, text)?;
            self.interned.insert(text, addr);
        }
        Ok(addr)
    }

    /// Põe um corpo de rede na captura de serial, em texto quando dá e em hexadecimal quando
    /// não dá — e, se for `deflate`, também o conteúdo inflado.
    ///
    /// Só a URL, o status e o tamanho iam para o relatório, e isso não basta para depurar um
    /// protocolo: o que estraga um registro é **o conteúdo**, campo a campo. Um Zeeboid chegou
    /// do servidor com os campos deslocados de uma casa e a senha truncada em dezesseis
    /// caracteres emendada no IMEI, e sem os bytes não há como dizer se quem errou foi o
    /// servidor, o transporte ou a leitura do jogo.
    ///
    /// Vai só para a serial, que é opcional: o corpo pode trazer IMEI e senha, e isso não entra
    /// num relatório que se manda por aí sem querer.
    fn registra_corpo(&mut self, que: &str, bytes: &[u8]) {
        /// Quanto de um corpo cabe no registro.
        const TETO: usize = 4096;

        if self.serial.is_none() {
            return;
        }
        let mostrar = |dados: &[u8]| -> String {
            let corte = &dados[..dados.len().min(TETO)];
            match corte
                .iter()
                .all(|&b| b == b'\n' || (0x20..0x7f).contains(&b))
            {
                true => String::from_utf8_lossy(corte).into_owned(),
                false => corte.iter().map(|b| format!("{b:02x}")).collect(),
            }
        };
        let mut linha = format!("<{que} {} bytes: {}>", bytes.len(), mostrar(bytes));
        if let Some(inflado) = inflate(bytes) {
            linha.push_str(&format!("\n<{que} inflado: {}>", mostrar(&inflado)));
        }
        self.registra_serial(linha);
    }

    /// Liga a captura de serial: cada linha de log vai para este arquivo, na ordem e com o
    /// instante do relógio virtual.
    ///
    /// O Zeebo tem uma UART de depuração, e o que sai por ela é o `DBGPRINTF` — o módulo não
    /// fala com o hardware, quem roteia é o firmware. Então o fluxo que um cabo de serial
    /// veria é exatamente este.
    ///
    /// Existe separado do relatório porque o relatório **agrupa repetições**, e agrupar perde
    /// as duas coisas que uma análise precisa: a ordem em que as linhas saíram e o intervalo
    /// entre elas. `Couldn't create z-pad instruction form (6)   (2153x)` diz que aconteceu
    /// duas mil vezes; não diz que aconteceu a cada 70 ms, nem o que veio antes da primeira.
    pub fn liga_serial(&mut self, caminho: &std::path::Path) -> std::io::Result<()> {
        let arquivo = std::fs::File::create(caminho)?;
        self.serial = Some(std::io::BufWriter::new(arquivo));
        Ok(())
    }

    /// Escreve **só** na captura de serial, sem passar pelo relatório.
    ///
    /// A instrumentação — classes criadas, bancos abertos, SQL — é ferramenta, não coisa que o
    /// jogo disse. Mandá-la pelo `record_debug` a punha no "log do jogo" do relatório, onde ela
    /// se mistura com o que o jogo de fato imprimiu e atrapalha justamente quem está lendo para
    /// entender o jogo.
    fn registra_serial(&mut self, message: String) {
        let agora = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            let _ = writeln!(serial, "[{agora:>9} ms] {message}");
            let _ = serial.flush();
        }
    }

    /// Guarda uma linha de log, agrupando repetições em vez de encher o relatório.
    ///
    /// Com a serial ligada, a linha também vai crua para o arquivo. Ver [`Machine::liga_serial`].
    fn record_debug(&mut self, message: String) {
        // O mesmo relógio que o resto do emulador reporta: o `clock_us` sozinho ignora o
        // tempo que as instruções gastaram, e a serial ficaria atrasada em relação ao rastro.
        let agora = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            // Falha de escrita não pode derrubar o jogo: a serial é instrumento, não emulação.
            let _ = writeln!(serial, "[{:>9} ms] {message}", agora);
            // **Descarrega a cada linha.** Sem isto o arquivo fica vazio enquanto a sessão corre
            // — o `BufWriter` só escreve quando enche ou quando é destruído —, e uma captura que
            // só aparece depois de fechar o jogo não serve para acompanhar o que está
            // acontecendo. É o uso inteiro da ferramenta.
            let _ = serial.flush();
        }
        match self
            .debug_output
            .iter_mut()
            .find(|(text, _)| *text == message)
        {
            Some((_, count)) => *count += 1,
            None => self.debug_output.push((message, 1)),
        }
    }

    /// Confere se o ponteiro recebido é mesmo um objeto nosso da interface esperada.
    ///
    /// Divergência aqui costuma significar vtable desalinhada — um slot fora de ordem faz o
    /// guest chamar o método errado, e o sintoma aparece longe da causa. Registrar cedo poupa
    /// horas.
    fn check_object(&mut self, addr: u32, expected: Interface) {
        if self.objects.kind_of(addr) != Some(expected) {
            self.suspicious_objects.insert((addr, expected as u32));
        }
    }

    fn args(&self) -> [u32; 4] {
        [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ]
    }

    /// Quantas vezes cada método foi chamado, em ordem — o backlog de APIs, medido.
    pub fn call_log(&self) -> Vec<(String, u64)> {
        self.calls
            .iter()
            .map(|(&(iface, slot), &count)| (aee::describe(aee::encode_raw(iface, slot)), count))
            .collect()
    }

    pub fn cpu_mut(&mut self) -> &mut C {
        &mut self.cpu
    }

    pub fn cpu(&self) -> &C {
        &self.cpu
    }

    pub fn module(&self) -> &LoadedModule {
        &self.module
    }

    /// Acessos inválidos que a execução seguiu por cima. Ver [`Machine::execute`].
    pub fn swallowed_faults(&self) -> Vec<String> {
        self.falhas_engolidas.iter().cloned().collect()
    }

    /// As APIs que faltaram durante a execução, inclusive dentro de retornos de chamada.
    pub fn missing_apis(&self) -> Vec<String> {
        self.missing_apis.iter().cloned().collect()
    }

    /// Quantos objetos vivos de cada interface. Ver [`crate::objects::ObjectStore::live_by_kind`].
    pub fn live_objects_by_kind(&self) -> Vec<(&'static str, usize)> {
        self.objects
            .live_by_kind()
            .into_iter()
            .map(|(iface, quantos)| (iface.name(), quantos))
            .collect()
    }

    /// ClassIDs pedidos que ainda não sabemos instanciar.
    pub fn unknown_classes(&self) -> Vec<u32> {
        self.unknown_classes.iter().copied().collect()
    }

    /// Quantas instruções o guest executou.
    pub fn instructions(&self) -> u64 {
        self.cpu.instructions()
    }

    /// Quantos quadros o jogo apresentou pelo OpenGL.
    ///
    /// Só as trocas de buffer, porque é isso que o laço de quadros usa para saber que há coisa
    /// nova para mostrar. Para o indicador da janela, ver [`Machine::quadros`].
    pub fn gl_swaps(&self) -> u32 {
        self.egl_swaps
    }

    /// Quantos quadros o jogo desenhou, para quem quer mostrar uma taxa.
    ///
    /// **Nem todo jogo apresenta trocando buffer.** A Z-Wheel desenha o palco num pbuffer e
    /// pega o resultado pelo `eglGetColorBufferQUALCOMM` para compor com o 2D: ela nunca chama
    /// `eglSwapBuffers`, e enquanto a taxa contava só trocas o indicador da janela marcava zero
    /// com o palco girando na tela.
    ///
    /// Sem troca, o que delimita um quadro é o `glClear` da cor — o começo do desenho seguinte.
    /// Contar começos e contar fins dá a mesma taxa, que é o que o indicador mostra.
    pub fn quadros(&self) -> u32 {
        match self.egl_swaps {
            0 => self.gl_clears,
            trocas => trocas,
        }
    }

    /// O último quadro que o jogo apresentou pelo OpenGL, se houve algum.
    ///
    /// Sai separado da tela porque os dois desenhos convivem: o `IDisplay` continua pintando
    /// por cima entre um `eglSwapBuffers` e o seguinte, e no fim quem escreveu por último é
    /// quem aparece. Ver o quadro do OpenGL sozinho é o que diz se a renderização 3D está
    /// certa.
    pub fn gl_frame(&self) -> Option<Framebuffer> {
        if self.gl_last_frame.is_empty() {
            return None;
        }
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let mut surface = Framebuffer::new(width, height);
        surface.load_rgb565_bytes(&self.gl_last_frame);
        Some(surface)
    }

    /// Quantas superfícies de desenho existem.
    pub fn bitmap_count(&self) -> usize {
        self.bitmaps.len()
    }

    /// Mensagens que o jogo mandou para o log, com a contagem de repetições.
    pub fn debug_output(&self) -> &[(String, u64)] {
        &self.debug_output
    }

    /// Palpites em uso nesta execução.
    pub fn assumptions(&self) -> Vec<&'static str> {
        self.assumptions.iter().copied().collect()
    }

    /// Ponteiros `this` que não batem com a interface chamada.
    pub fn suspicious_objects(&self) -> Vec<(u32, u32)> {
        self.suspicious_objects.iter().copied().collect()
    }

    pub fn live_objects(&self) -> usize {
        self.objects.live_count()
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// O retângulo de recorte que o Pac-Mania usa para mostrar uma letra só da folha.
    const CLIP: Rect = Rect {
        x: 100,
        y: 50,
        width: 20,
        height: 10,
    };

    #[test]
    fn sem_recorte_o_blit_passa_inteiro() {
        // Antes do primeiro `SetClipRect` o recorte é a superfície inteira, e nada muda.
        let same = clip_blit(None, (5, 7), (30, 40), (1, 2));
        assert_eq!(same, Some(((5, 7), (30, 40), (1, 2))));
    }

    #[test]
    fn o_recorte_anda_com_a_origem_da_fonte() {
        // É a parte que importa: cortar só o destino mostraria o canto errado da imagem, e é
        // assim que uma folha de fontes vira letra trocada. O deslocamento do destino tem de
        // aparecer igual na origem.
        let (dst, size, src) = clip_blit(Some(CLIP), (90, 40), (100, 100), (0, 0)).unwrap();
        assert_eq!(dst, (100, 50), "o destino começa no recorte");
        assert_eq!(src, (10, 10), "a origem andou o mesmo tanto");
        assert_eq!(size, (20, 10), "e o tamanho é o do recorte");
    }

    #[test]
    fn o_blit_fora_do_recorte_nao_desenha() {
        assert_eq!(clip_blit(Some(CLIP), (0, 0), (10, 10), (0, 0)), None);
        assert_eq!(clip_blit(Some(CLIP), (200, 200), (10, 10), (0, 0)), None);
        // Encostar na borda também não desenha: o retângulo do BREW não inclui o limite.
        assert_eq!(clip_blit(Some(CLIP), (80, 50), (20, 10), (0, 0)), None);
    }

    #[test]
    fn o_blit_menor_que_o_recorte_fica_como_esta() {
        let inside = clip_blit(Some(CLIP), (105, 52), (5, 5), (3, 4));
        assert_eq!(inside, Some(((105, 52), (5, 5), (3, 4))));
    }

    /// `ClearScreen` é `DrawRect(NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)`, e limpa a tela
    /// inteira com a cor de fundo corrente.
    ///
    /// Três leituras tinham de estar certas ao mesmo tempo, e as três estavam erradas: ponteiro
    /// nulo é a superfície inteira e não "sem retângulo"; `RGB_NONE` é "a cor corrente" e não
    /// "não pinte"; e quem decide entre moldura e preenchimento é o `flags`. O Tekken 2 limpa a
    /// tela assim uma vez por quadro — sem isso, o menu dele aparecia por cima do texto da tela
    /// anterior.
    /// `strstr` com agulha vazia devolve o próprio texto, que é o que o C manda.
    ///
    /// Parece detalhe de especificação e não é: o Need For Speed registra os sons de jogo
    /// procurando o nome de cada um numa tabela de setenta entradas, e **um dos cinco nomes dele
    /// é a string vazia**. Respondendo "não achou", ele varria a tabela inteira, concluía que o
    /// som não existe, imprimia `ZeeboSnd.cpp:327 BREAKPOINT!` e entrava num salto para si mesmo.
    /// O jogo aparecia no relatório como "orçamento de instruções esgotado" — ou seja, como se
    /// fosse pesado, quando estava parado de propósito.
    #[test]
    fn a_agulha_vazia_casa_no_comeco_do_texto() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let texto = loader::HEAP_BASE;
        let agulha = loader::HEAP_BASE + 0x100;
        machine.cpu.write_mem(texto, b"snd/skid/skid.wav ").unwrap();

        let busca = |machine: &mut Machine<UnicornCpu>, palavra: &[u8]| -> u32 {
            machine.cpu.write_mem(agulha, palavra).unwrap();
            call(
                machine,
                Interface::Helpers,
                slot_of(Interface::Helpers, "strstr"),
                [texto, agulha, 0, 0],
            )
        };
        assert_eq!(
            busca(&mut machine, b" "),
            texto,
            "agulha vazia casa no começo"
        );
        assert_eq!(busca(&mut machine, b"skid.wav "), texto + 9);
        assert_eq!(busca(&mut machine, b"snd/skid/skid.wav "), texto);
        assert_eq!(
            busca(&mut machine, b"carbon "),
            0,
            "o que não está não casa"
        );
        // Agulha maior que o texto não casa, e não pode estourar.
        assert_eq!(busca(&mut machine, b"snd/skid/skid.wav.extra "), 0);
    }

    #[test]
    fn limpar_a_tela_pinta_tudo_com_a_cor_de_fundo() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let display = machine.objects.create(Interface::Display).unwrap();
        let alvo = machine.device_bitmap().unwrap();
        machine.bitmaps.get_mut(&alvo).unwrap().fill_rect(
            Rect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            Rgb::WHITE,
        );

        // Fundo azul, e a limpeza que o jogo faz: sem retângulo, sem cores, só o sinalizador.
        call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "SetColor"),
            [
                display,
                CLR_USER_BACKGROUND as u32,
                to_rgbval(Rgb { r: 0, g: 0, b: 255 }),
                0,
            ],
        );
        machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
        machine
            .cpu
            .write_u32(loader::STACK_BASE + 0x1000, IDF_RECT_FILL)
            .unwrap();
        call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "DrawRect"),
            [display, 0, RGB_NONE, RGB_NONE],
        );
        let fb = machine.bitmaps.get(&alvo).unwrap();
        let azul = Rgb { r: 0, g: 0, b: 255 }.to_rgb565();
        assert_eq!(fb.get_pixel(0, 0), azul, "o canto não foi limpo");
        assert_eq!(fb.get_pixel(639, 479), azul, "o outro canto não foi limpo");
    }

    /// Só moldura quer dizer **só moldura**: a cor de preenchimento vem junto e não deve pintar.
    ///
    /// O Quake chama exatamente assim — `IDF_RECT_FRAME` com preto no preenchimento. Ignorar o
    /// sinalizador punha um retângulo preto que ele não pediu.
    #[test]
    fn so_a_moldura_quando_o_sinalizador_pede_so_a_moldura() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let display = machine.objects.create(Interface::Display).unwrap();
        let alvo = machine.device_bitmap().unwrap();
        machine.bitmaps.get_mut(&alvo).unwrap().fill_rect(
            Rect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            Rgb::WHITE,
        );

        let rect = loader::HEAP_BASE;
        for (i, valor) in [10i16, 10, 40, 30].iter().enumerate() {
            machine
                .cpu
                .write_mem(rect + i as u32 * 2, &valor.to_le_bytes())
                .unwrap();
        }
        machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
        machine
            .cpu
            .write_u32(loader::STACK_BASE + 0x1000, IDF_RECT_FRAME)
            .unwrap();
        call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "DrawRect"),
            [display, rect, to_rgbval(Rgb { r: 255, g: 0, b: 0 }), 0],
        );
        let fb = machine.bitmaps.get(&alvo).unwrap();
        let vermelho = Rgb { r: 255, g: 0, b: 0 }.to_rgb565();
        assert_eq!(
            fb.get_pixel(10, 10),
            vermelho,
            "a moldura não foi desenhada"
        );
        assert_eq!(
            fb.get_pixel(25, 25),
            Rgb::WHITE.to_rgb565(),
            "o miolo foi preenchido sem o jogo ter pedido"
        );
    }

    #[test]
    fn o_retangulo_e_cortado_pelo_recorte() {
        let cut = clip_rect(
            Some(CLIP),
            Rect {
                x: 90,
                y: 40,
                width: 100,
                height: 100,
            },
        )
        .unwrap();
        assert_eq!((cut.x, cut.y, cut.width, cut.height), (100, 50, 20, 10));
        // Sem recorte, o retângulo passa inteiro. O contrário estava escrito aqui, e apagava
        // todo `DrawRect` de quem não define recorte — inclusive a limpeza de tela.
        let solto = Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        };
        assert_eq!(clip_rect(None, solto), Some(solto));
        assert_eq!(
            clip_rect(
                Some(CLIP),
                Rect {
                    x: 0,
                    y: 0,
                    width: 5,
                    height: 5
                }
            ),
            None
        );
    }
    use crate::cpu::unicorn::UnicornCpu;
    use crate::modfile::ModImage;

    /// Módulo sintético que chama `MALLOC(16)` pela tabela de helpers e devolve o ponteiro.
    ///
    /// ```asm
    /// mov  r0, #16
    /// ldr  r1, [r2, #0x68]   ; r2 = tabela de helpers
    /// mov  lr, pc            ; retorno = próxima instrução (bx lr)
    /// bx   r1
    /// bx   lr                ; volta para o sentinela? não: lr foi sobrescrito
    /// ```
    ///
    /// Depois da chamada, `lr` aponta para a instrução seguinte, então terminamos saltando
    /// para o sentinela guardado em r3.
    fn module_calling_malloc() -> ModImage {
        let code = [
            0xe3a0_0010u32.to_le_bytes(), // mov r0, #16
            0xe592_1068u32.to_le_bytes(), // ldr r1, [r2, #0x68]
            0xe1a0_e00fu32.to_le_bytes(), // mov lr, pc  (aponta para bx r3)
            0xe12f_ff11u32.to_le_bytes(), // bx r1
            0xe12f_ff13u32.to_le_bytes(), // bx r3
        ]
        .concat();
        ModImage::parse(code).unwrap()
    }

    /// Chama um método diretamente pelo despacho, como se o guest tivesse chamado.
    fn call(machine: &mut Machine<UnicornCpu>, iface: Interface, slot: u32, args: [u32; 4]) -> u32 {
        machine.cpu.write_reg(Reg::R0, args[0]);
        machine.cpu.write_reg(Reg::R1, args[1]);
        machine.cpu.write_reg(Reg::R2, args[2]);
        machine.cpu.write_reg(Reg::R3, args[3]);
        machine
            .dispatch(aee::encode(iface, slot))
            .unwrap()
            .expect("método deveria estar implementado")
    }

    /// Um RIFF/WAVE mínimo, PCM de 8 bits e mono, com `frames` amostras mudas.
    fn riff_pcm8(rate: u32, frames: u32) -> Vec<u8> {
        let mut wave = Vec::new();
        wave.extend_from_slice(b"RIFF");
        wave.extend_from_slice(&(36 + frames).to_le_bytes());
        wave.extend_from_slice(b"WAVEfmt ");
        wave.extend_from_slice(&16u32.to_le_bytes());
        wave.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wave.extend_from_slice(&1u16.to_le_bytes()); // mono
        wave.extend_from_slice(&rate.to_le_bytes());
        wave.extend_from_slice(&rate.to_le_bytes()); // bytes por segundo
        wave.extend_from_slice(&1u16.to_le_bytes()); // alinhamento do bloco
        wave.extend_from_slice(&8u16.to_le_bytes()); // bits por amostra
        wave.extend_from_slice(b"data");
        wave.extend_from_slice(&frames.to_le_bytes());
        wave.extend(std::iter::repeat_n(128u8, frames as usize));
        wave
    }

    /// O slot de um método pelo nome. A busca cobre a tabela inteira: as tabelas grandes — o
    /// OpenGL e os helpers — passam bem dos primeiros slots.
    fn slot_of(iface: Interface, name: &str) -> u32 {
        (0..256)
            .find(|&s| iface.method(s) == Some(name))
            .unwrap_or_else(|| panic!("{name} não existe em {}", iface.name()))
    }

    #[test]
    fn o_tipo_sai_da_assinatura_e_so_depois_da_extensao() {
        assert_eq!(detect_mime(b"\x89PNG\r\n\x1a\n...", ""), Some("image/png"));
        assert_eq!(detect_mime(b"RIFF\0\0\0\0WAVEfmt ", ""), Some("audio/wav"));
        assert_eq!(detect_mime(b"ID3\x03", ""), Some("audio/mpeg"));
        // Sem assinatura reconhecível, vale a extensão do nome.
        assert_eq!(detect_mime(b"", "musica.MP3"), Some("audio/mpeg"));
        assert_eq!(detect_mime(b"", "sem_extensao"), None);
        // E a assinatura tem precedência sobre o nome, como manda a documentação.
        assert_eq!(
            detect_mime(b"\x89PNG\r\n\x1a\n", "x.mp3"),
            Some("image/png")
        );
    }

    #[test]
    fn o_handler_de_cada_tipo_sai_dos_class_ids_do_sdk() {
        assert_eq!(handler_for("image/png"), AEECLSID_PNG);
        assert_eq!(handler_for("audio/mpeg"), AEECLSID_MEDIAMP3);
        // Tipo que ninguém sabe abrir devolve zero, que é a resposta prevista.
        assert_eq!(handler_for("application/x-nada"), 0);
    }

    #[test]
    fn strtoul_respeita_a_base_e_diz_onde_parou() {
        assert_eq!(parse_unsigned("  42abc", 10), (42, 4));
        assert_eq!(parse_unsigned("0x1f", 0), (31, 4));
        assert_eq!(parse_unsigned("0x1f", 16), (31, 4));
        assert_eq!(parse_unsigned("017", 0), (15, 3));
        assert_eq!(parse_unsigned("ff", 16), (255, 2));
        // Sem dígito nenhum, o `strtoul` devolve zero e não consome nada.
        assert_eq!(parse_unsigned("xyz", 10), (0, 0));
    }

    #[test]
    fn strtod_le_o_maior_prefixo_numerico() {
        assert_eq!(parse_leading_double("3.5rest"), (3.5, 3));
        assert_eq!(parse_leading_double("3.5 x"), (3.5, 3));
        assert_eq!(parse_leading_double("  -2e3;"), (-2000.0, 6));
        assert_eq!(parse_leading_double("nada"), (0.0, 0));
    }

    #[test]
    fn busca_de_subsequencia_acha_o_comeco() {
        assert_eq!(find_subslice(b"abcdef", b"cde"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"xyz"), None);
        assert_eq!(find_subslice(b"abc", b""), Some(0));
    }

    #[test]
    fn comparacao_larga_dobra_o_caso_so_do_ascii() {
        let upper: Vec<u16> = "ABC".encode_utf16().collect();
        let lower: Vec<u16> = "abc".encode_utf16().collect();
        assert_eq!(fold_case(&upper, usize::MAX), fold_case(&lower, usize::MAX));
        assert_eq!(fold_case(&upper, 1), fold_case(&lower, 1));
        assert_ne!(fold_case(&upper, 2), fold_case(&lower, 1));
    }

    /// Monta uma máquina com um diretório de trabalho próprio, para os testes de arquivo.
    fn machine_at(root: &std::path::Path) -> Machine<UnicornCpu> {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, root);
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine
    }

    /// Um PNG 2x1 com alfa: um pixel opaco vermelho e um totalmente transparente.
    fn png_com_alfa() -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 2, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[255, 0, 0, 255, 0, 255, 0, 0])
            .unwrap();
        out
    }

    #[test]
    fn a_escala_de_superficie_diz_o_tamanho_em_que_o_jogo_desenha() {
        // O `EGL_QUALCOMM_surface_scale` é o console ampliando o que o jogo desenhou pequeno.
        // Quando o jogo o usa, ele **diz** o tamanho — e isso vale mais que a dedução por
        // viewport, que existe só para quem não diz.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let egl = machine.new_object(Interface::Egl).unwrap();
        let out = loader::HEAP_BASE;
        assert_eq!(
            call(
                &mut machine,
                Interface::Egl,
                slot_of(Interface::Egl, "QueryInterface"),
                [egl, AEEIID_EGL_SURFACE_MANIP, out, 0],
            ),
            SUCCESS
        );
        let manip = machine.cpu.read_u32(out).unwrap();
        assert_ne!(manip, 0);

        // AEEEGLSurfaceScaleRect { x, y, width, height } = 320x224, a tela de um arcade.
        let rect = loader::HEAP_BASE + 0x100;
        for (index, value) in [0u32, 0, 320, 224].iter().enumerate() {
            machine
                .cpu
                .write_u32(rect + index as u32 * 4, *value)
                .unwrap();
        }
        // O `ret` é o quinto argumento, e o quinto vai na pilha.
        let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 / 2;
        machine.cpu.write_reg(Reg::Sp, sp);
        machine.cpu.write_u32(sp, 0).unwrap();
        assert_eq!(
            call(
                &mut machine,
                Interface::EglSurfaceManip,
                slot_of(Interface::EglSurfaceManip, "SetSurfaceScale"),
                [manip, 1, 1, rect],
            ),
            SUCCESS
        );
        assert_eq!(machine.gl.surface(), (320, 224));
    }

    #[test]
    fn dormir_faz_o_relogio_andar() {
        // No console o ARM realmente para e o tempo passa sozinho. Aqui o relógio anda com as
        // instruções, então um `sleep` que não o adiante dorme para sempre — era o laço em que
        // o Magical Drop 3 ficava, com cem milhões de chamadas.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let antes = machine.now_ms();
        call(
            &mut machine,
            Interface::Helpers,
            slot_of(Interface::Helpers, "sleep"),
            [50, 0, 0, 0],
        );
        assert!(machine.now_ms() >= antes + 50, "o relógio andou os 50 ms");
    }

    #[test]
    fn o_decodificador_recebe_o_png_pelo_force_feed_e_devolve_o_bitmap() {
        // O caminho inteiro que os jogos usam: cria o decodificador, pede a interface de
        // entrada, escreve o arquivo em pedaços, fecha com uma escrita vazia e busca o bitmap.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let decoder = machine.new_object(Interface::ImageDecoder).unwrap();
        let out = loader::HEAP_BASE;
        assert_eq!(
            call(
                &mut machine,
                Interface::ImageDecoder,
                slot_of(Interface::ImageDecoder, "QueryInterface"),
                [decoder, AEEIID_FORCEFEED, out, 0],
            ),
            SUCCESS
        );
        let feed = machine.cpu.read_u32(out).unwrap();
        // O `IForceFeed` é um objeto **separado**: as duas interfaces têm métodos diferentes no
        // mesmo slot, e devolver o mesmo ponteiro faria o jogo chamar o método errado.
        assert_ne!(feed, decoder);

        // Entregue em dois pedaços, como um jogo que lê o arquivo aos poucos faria.
        let png = png_com_alfa();
        let buffer = loader::HEAP_BASE + 0x100;
        machine.cpu.write_mem(buffer, &png).unwrap();
        let write = slot_of(Interface::ForceFeed, "Write");
        let half = png.len() as u32 / 2;
        call(
            &mut machine,
            Interface::ForceFeed,
            write,
            [feed, buffer, half, 0],
        );
        call(
            &mut machine,
            Interface::ForceFeed,
            write,
            [feed, buffer + half, png.len() as u32 - half, 0],
        );
        call(&mut machine, Interface::ForceFeed, write, [feed, 0, 0, 0]);

        let bitmap_out = loader::HEAP_BASE + 0x800;
        assert_eq!(
            call(
                &mut machine,
                Interface::ImageDecoder,
                slot_of(Interface::ImageDecoder, "GetBitmap"),
                [decoder, bitmap_out, 0, 0],
            ),
            SUCCESS
        );
        let bitmap = machine.cpu.read_u32(bitmap_out).unwrap();
        let fb = machine.bitmaps.get(&bitmap).expect("o bitmap existe");
        assert_eq!((fb.width(), fb.height()), (2, 1));
        assert_eq!(fb.get_pixel(0, 0), Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
        // O pixel transparente vira a cor reservada, e o `GetRop` avisa o jogo disso.
        assert_eq!(fb.get_pixel(1, 0), TRANSPARENT_KEY);
        assert_eq!(
            call(
                &mut machine,
                Interface::ImageDecoder,
                slot_of(Interface::ImageDecoder, "GetRop"),
                [decoder, 0, 0, 0],
            ),
            AEE_RO_TRANSPARENT
        );
        // O bitmap decodificado já chega com os campos públicos do `IDIB` preenchidos, sem
        // esperar um `QueryInterface`: no console um `IBitmap` de software **é** um `IDIB`, e o
        // jogo lê o tamanho direto dos campos. O Peggle é quem cobrou — sem isso ele lia 0×0,
        // montava cada sprite como um quadrado de lado zero e 99% dos triângulos do quadro
        // saíam degenerados, com a tela preta e o jogo desenhando o tempo todo.
        let mut cx = [0u8; 2];
        machine.cpu.read_mem(bitmap + 20, &mut cx).unwrap();
        assert_eq!(u16::from_le_bytes(cx), 2);
        let mut cy = [0u8; 2];
        machine.cpu.read_mem(bitmap + 22, &mut cy).unwrap();
        assert_eq!(u16::from_le_bytes(cy), 1);
        assert!(machine.cpu.read_u32(bitmap + 8).unwrap() >= loader::SURFACE_BASE);

        // Pedir de novo devolve o mesmo bitmap, e não uma cópia nova a cada chamada.
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "GetBitmap"),
            [decoder, bitmap_out, 0, 0],
        );
        assert_eq!(machine.cpu.read_u32(bitmap_out).unwrap(), bitmap);
    }

    #[test]
    fn a_data_juliana_conta_da_epoca_do_brew() {
        // Zero é o próprio instante da época: 6 de janeiro de 1980, um domingo.
        assert_eq!(julian_date(0), [1980, 1, 6, 0, 0, 0, 0]);
        // Um dia depois, segunda-feira.
        assert_eq!(julian_date(86_400), [1980, 1, 7, 0, 0, 0, 1]);
        // A hora do dia sai do resto, e o dia da semana dá a volta em sete.
        assert_eq!(
            julian_date(86_400 * 7 + 3600 * 13 + 60 * 24 + 35),
            [1980, 1, 13, 13, 24, 35, 0]
        );
        // Fim de fevereiro num ano bissexto, que é onde uma contagem ingênua erra.
        assert_eq!(julian_date(86_400 * 54), [1980, 2, 29, 0, 0, 0, 5]);
        assert_eq!(julian_date(86_400 * 55), [1980, 3, 1, 0, 0, 0, 6]);
        // E a virada de século: 2000 foi bissexto, o que 1900 não seria.
        assert_eq!(julian_date(86_400 * 7300), [2000, 1, 1, 0, 0, 0, 6]);
        assert_eq!(julian_date(86_400 * 7361), [2000, 3, 2, 0, 0, 0, 4]);
    }

    #[test]
    fn o_decodificador_recusa_o_que_nao_e_png() {
        // Recusar dizendo o motivo evita a caçada a uma imagem que some sem explicação.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let decoder = machine.new_object(Interface::ImageDecoder).unwrap();
        let out = loader::HEAP_BASE;
        call(
            &mut machine,
            Interface::ImageDecoder,
            slot_of(Interface::ImageDecoder, "QueryInterface"),
            [decoder, AEEIID_FORCEFEED, out, 0],
        );
        let feed = machine.cpu.read_u32(out).unwrap();
        let buffer = loader::HEAP_BASE + 0x100;
        machine.cpu.write_mem(buffer, b"nao e um png").unwrap();
        call(
            &mut machine,
            Interface::ForceFeed,
            slot_of(Interface::ForceFeed, "Write"),
            [feed, buffer, 12, 0],
        );
        let bitmap_out = loader::HEAP_BASE + 0x800;
        assert_eq!(
            call(
                &mut machine,
                Interface::ImageDecoder,
                slot_of(Interface::ImageDecoder, "GetBitmap"),
                [decoder, bitmap_out, 0, 0],
            ),
            EFAILED
        );
        assert_eq!(machine.cpu.read_u32(bitmap_out).unwrap(), 0);
    }

    #[test]
    fn a_imagem_nativa_vira_um_bitmap_que_o_blit_entende() {
        // O ponteiro devolvido vai direto para o `IDISPLAY_BitBlt`, que recebe um `IBitmap *`.
        // Devolver um bloco solto de pixels obrigaria o desenho a ter um segundo caminho.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Um BMP de 24 bits, 2x1: vermelho e azul (o BMP guarda em BGR, de baixo para cima).
        let mut bmp = vec![0u8; 54];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&54u32.to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&1u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]);
        let size = bmp.len() as u32;
        bmp[2..6].copy_from_slice(&size.to_le_bytes());

        let buffer = loader::HEAP_BASE;
        machine.cpu.write_mem(buffer, &bmp).unwrap();
        let info = buffer + 0x200;
        let realloc = buffer + 0x300;

        let bitmap = call(
            &mut machine,
            Interface::Helpers,
            slot_of(Interface::Helpers, "SetupNativeImage"),
            [AEECLSID_WINBMP, buffer, info, realloc],
        );
        assert_ne!(bitmap, 0, "devolve o bitmap convertido");

        let fb = machine.bitmaps.get(&bitmap).expect("o bitmap existe");
        assert_eq!((fb.width(), fb.height()), (2, 1));
        assert_eq!(fb.get_pixel(0, 0), Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
        assert_eq!(fb.get_pixel(1, 0), Rgb { r: 0, g: 0, b: 255 }.to_rgb565());

        // O `AEEImageInfo` e o aviso de que a memória é nossa.
        let mut cx = [0u8; 2];
        machine.cpu.read_mem(info, &mut cx).unwrap();
        assert_eq!(u16::from_le_bytes(cx), 2);
        let mut flag = [0u8; 1];
        machine.cpu.read_mem(realloc, &mut flag).unwrap();
        assert_eq!(flag[0], 1, "a imagem saiu numa alocação nossa");
    }

    /// Duas imagens de tamanhos diferentes no **mesmo endereço** de objeto, e o `IDIB` da
    /// segunda tem de falar da segunda.
    ///
    /// É o defeito que apagava o texto do Tekken 2. Ele decodifica nove imagens em sequência,
    /// liberando cada uma antes da próxima, então todas nascem no mesmo endereço. O `IDIB`
    /// anunciava o tamanho da primeira para todas: o jogo criava uma página de 200x112 para uma
    /// folha de letras de 360x280, guardava só o canto dela e depois pedia cada glifo por
    /// coordenada da folha inteira. O que caía fora da página virava bloco, e o menu saía com
    /// as palavras como retângulos laranja.
    #[test]
    fn o_dib_acompanha_a_troca_de_imagem_no_mesmo_endereco() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        /// Um BMP de 24 bits, cinza, do tamanho pedido.
        fn bmp(largura: u32, altura: u32) -> Vec<u8> {
            let mut out = vec![0u8; 54];
            out[0..2].copy_from_slice(b"BM");
            out[10..14].copy_from_slice(&54u32.to_le_bytes());
            out[14..18].copy_from_slice(&40u32.to_le_bytes());
            out[18..22].copy_from_slice(&largura.to_le_bytes());
            out[22..26].copy_from_slice(&altura.to_le_bytes());
            out[26..28].copy_from_slice(&1u16.to_le_bytes());
            out[28..30].copy_from_slice(&24u16.to_le_bytes());
            let passo = (largura * 3).div_ceil(4) * 4;
            out.extend(std::iter::repeat_n(0x80u8, (passo * altura) as usize));
            let tamanho = out.len() as u32;
            out[2..6].copy_from_slice(&tamanho.to_le_bytes());
            out
        }

        let buffer = loader::HEAP_BASE;
        let info = buffer + 0x4000;
        let realloc = info + 0x100;
        let saida = realloc + 0x100;

        let converte = |machine: &mut Machine<UnicornCpu>, imagem: &[u8]| -> u32 {
            machine.cpu.write_mem(buffer, imagem).unwrap();
            let bitmap = call(
                machine,
                Interface::Helpers,
                slot_of(Interface::Helpers, "SetupNativeImage"),
                [AEECLSID_WINBMP, buffer, info, realloc],
            );
            assert_ne!(bitmap, 0);
            // O jogo pede o `IDIB` para ler os campos públicos, que é como ele descobre o
            // tamanho da imagem.
            call(
                machine,
                Interface::Bitmap,
                slot_of(Interface::Bitmap, "QueryInterface"),
                [bitmap, AEECLSID_DIB, saida, 0],
            );
            bitmap
        };
        let tamanho_no_dib = |machine: &Machine<UnicornCpu>, bitmap: u32| -> (u16, u16, i16) {
            let mut campos = [0u8; 6];
            machine.cpu.read_mem(bitmap + 20, &mut campos).unwrap();
            (
                u16::from_le_bytes([campos[0], campos[1]]),
                u16::from_le_bytes([campos[2], campos[3]]),
                i16::from_le_bytes([campos[4], campos[5]]),
            )
        };

        let primeiro = converte(&mut machine, &bmp(8, 4));
        assert_eq!(tamanho_no_dib(&machine, primeiro), (8, 4, 16));

        // O jogo libera a imagem antes de converter a próxima, e o endereço volta a ser usado.
        while machine.objects.release(primeiro) > 0 {}
        let segundo = converte(&mut machine, &bmp(20, 10));
        assert_eq!(segundo, primeiro, "o endereço tinha de ser reciclado");
        assert_eq!(
            tamanho_no_dib(&machine, segundo),
            (20, 10, 40),
            "o IDIB ficou falando da imagem anterior"
        );

        // E o buffer publicado tem de caber a imagem nova inteira: com o buffer antigo, os
        // pixels do fim ficariam de fora e a folha sairia cortada.
        let pixels = machine.dib_buffers.get(&segundo).copied().unwrap();
        let mut ultimo = [0u8; 2];
        machine
            .cpu
            .read_mem(pixels + 20 * 10 * 2 - 2, &mut ultimo)
            .unwrap();
        assert_ne!(
            u16::from_le_bytes(ultimo),
            0,
            "o último pixel não foi publicado"
        );
    }

    #[test]
    fn imagem_nativa_que_nao_sabemos_ler_devolve_nulo() {
        // Devolver nulo é o que o BREW faz quando não converte, e o jogo tem caminho para isso.
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let buffer = loader::HEAP_BASE;
        machine.cpu.write_mem(buffer, b"nao e um bmp").unwrap();
        let out = call(
            &mut machine,
            Interface::Helpers,
            slot_of(Interface::Helpers, "SetupNativeImage"),
            [AEECLSID_WINBMP, buffer, 0, 0],
        );
        assert_eq!(out, 0);
    }

    #[test]
    fn a_enumeracao_lista_arquivos_e_diretorios_separados() {
        // O jogo pede uma coisa ou outra, nunca as duas: o `bDirs` do `EnumInit` decide.
        let root = std::env::temp_dir().join("zeebx-testes-enum");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pasta")).unwrap();
        std::fs::write(root.join("b.dat"), b"12345").unwrap();
        std::fs::write(root.join("a.dat"), b"1").unwrap();

        let mut machine = machine_at(&root);
        let mgr = machine.new_object(Interface::FileMgr).unwrap();
        let dir = loader::HEAP_BASE;
        machine.cpu.write_mem(dir, b"fs:/~/\0").unwrap();
        let info = loader::HEAP_BASE + 0x100;

        let names = |machine: &mut Machine<UnicornCpu>, dirs: u32| {
            call(
                machine,
                Interface::FileMgr,
                slot_of(Interface::FileMgr, "EnumInit"),
                [mgr, dir, dirs, 0],
            );
            let mut out = Vec::new();
            while call(
                machine,
                Interface::FileMgr,
                slot_of(Interface::FileMgr, "EnumNext"),
                [mgr, info, 0, 0],
            ) == TRUE
            {
                out.push(machine.cpu.read_cstring(info + 12, MAX_FILE_NAME));
            }
            out
        };

        // O caminho volta com o prefixo que o jogo passou: é ele que vai para o `OpenFile`.
        assert_eq!(names(&mut machine, FALSE), ["fs:/~/a.dat", "fs:/~/b.dat"]);
        assert_eq!(names(&mut machine, TRUE), ["fs:/~/pasta"]);

        // O tamanho do último arquivo lido chega junto.
        call(
            &mut machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumInit"),
            [mgr, dir, FALSE, 0],
        );
        call(
            &mut machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumNext"),
            [mgr, info, 0, 0],
        );
        assert_eq!(
            machine.cpu.read_u32(info + 8).unwrap(),
            1,
            "a.dat tem 1 byte"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn o_fim_da_enumeracao_deixa_o_erro_que_o_brew_manda() {
        // Depois de um `EnumNext` falso o `GetLastError` devolve `EFAILED` mesmo tendo dado
        // tudo certo. A documentação chama isso de compatibilidade com o cliente 1.0, e há
        // jogo que confere.
        let root = std::env::temp_dir().join("zeebx-testes-enum-fim");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut machine = machine_at(&root);
        let mgr = machine.new_object(Interface::FileMgr).unwrap();
        let dir = loader::HEAP_BASE;
        machine.cpu.write_mem(dir, b"fs:/~/\0").unwrap();

        call(
            &mut machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumInit"),
            [mgr, dir, FALSE, 0],
        );
        let more = call(
            &mut machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "EnumNext"),
            [mgr, loader::HEAP_BASE + 0x100, 0, 0],
        );
        assert_eq!(more, FALSE, "diretório vazio não tem próximo");
        let error = call(
            &mut machine,
            Interface::FileMgr,
            slot_of(Interface::FileMgr, "GetLastError"),
            [mgr, 0, 0, 0],
        );
        assert_eq!(error, EFAILED);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn as_preferencias_voltam_como_foram_gravadas() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let shell = machine.new_object(Interface::Shell).unwrap();
        let buffer = loader::HEAP_BASE;
        machine.cpu.write_mem(buffer, &[1, 2, 3, 4]).unwrap();

        // O `nSize` é o quinto argumento, e o quinto vai na pilha.
        let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 / 2;
        machine.cpu.write_reg(Reg::Sp, sp);
        machine.cpu.write_u32(sp, 4).unwrap();
        let set = slot_of(Interface::Shell, "SetPrefs");
        assert_eq!(
            call(&mut machine, Interface::Shell, set, [shell, 7, 1, buffer]),
            SUCCESS
        );

        let out = buffer + 0x100;
        let get = slot_of(Interface::Shell, "GetPrefs");
        assert_eq!(
            call(&mut machine, Interface::Shell, get, [shell, 7, 1, out]),
            SUCCESS
        );
        let mut bytes = [0u8; 4];
        machine.cpu.read_mem(out, &mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);

        // Classe que nunca foi gravada falha, que é o que o BREW responde.
        assert_eq!(
            call(&mut machine, Interface::Shell, get, [shell, 99, 1, out]),
            EFAILED
        );
    }

    /// A thread cooperativa completa: começa, cede o controle no `Suspend` e volta do ponto
    /// exato em que parou quando o `ISHELL_Resume` dispara o callback de retomada.
    #[test]
    fn thread_cooperativa_cede_e_retoma_de_onde_parou() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let thread = machine.new_object(Interface::Thread).unwrap();
        let start = slot_of(Interface::Thread, "Start");
        assert_eq!(
            call(
                &mut machine,
                Interface::Thread,
                start,
                [thread, 4096, 0x9000, 7]
            ),
            SUCCESS
        );
        // Começar duas vezes é `EALREADY`: o `IThread` do BREW não é reutilizável.
        assert_eq!(
            call(
                &mut machine,
                Interface::Thread,
                start,
                [thread, 4096, 0x9000, 7]
            ),
            EALREADY
        );
        assert_eq!(machine.pending_threads, vec![thread]);
        machine.pending_threads.clear();

        // O `Suspend` congela os registradores e devolve o controle trocando o `lr` pelo
        // sentinela de retorno.
        let suspend = slot_of(Interface::Thread, "Suspend");
        machine.cpu.write_reg(Reg::R4, 0xabc);
        machine.cpu.write_reg(Reg::Lr, 0x1234);
        call(&mut machine, Interface::Thread, suspend, [thread, 0, 0, 0]);
        assert_eq!(machine.cpu.read_reg(Reg::Lr), RETURN_MAGIC);
        let state = &machine.threads[&thread];
        assert!(state.suspended);
        assert_eq!(state.resume_pc, 0x1234);
        assert_eq!(state.context[4], 0xabc);

        // `GetResumeCBK` devolve sempre o mesmo callback, e é por ele que o `ISHELL_Resume`
        // reconhece que quem quer voltar é a thread.
        let get_cbk = slot_of(Interface::Thread, "GetResumeCBK");
        let cbk = call(&mut machine, Interface::Thread, get_cbk, [thread, 0, 0, 0]);
        assert_ne!(cbk, 0);
        assert_eq!(
            call(&mut machine, Interface::Thread, get_cbk, [thread, 0, 0, 0]),
            cbk
        );

        let shell = machine.module.shell;
        let resume = slot_of(Interface::Shell, "Resume");
        call(&mut machine, Interface::Shell, resume, [shell, cbk, 0, 0]);
        assert_eq!(machine.pending_threads, vec![thread]);
    }

    /// As interfaces novas do BREW entregam o resultado por ponteiro de saída, e o
    /// `QueryInterface` do EGL precisa devolver um objeto de GL — não ele mesmo.
    #[test]
    fn shell_so_inicia_applet_instalado_e_enfileira_uma_vez() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine.set_installed_applets([0x123456]);
        let start = slot_of(Interface::Shell, "StartApplet");
        let can = slot_of(Interface::Shell, "CanStartApplet");
        assert_eq!(
            call(&mut machine, Interface::Shell, can, [0, 0x123456, 0, 0]),
            SUCCESS
        );
        assert_eq!(machine.take_launch_request(), None);
        assert_eq!(
            call(&mut machine, Interface::Shell, start, [0, 0x654321, 0, 0]),
            ECLASSNOTSUPPORT
        );
        assert_eq!(machine.take_launch_request(), None);
        assert_eq!(
            call(&mut machine, Interface::Shell, start, [0, 0x123456, 0, 0]),
            SUCCESS
        );
        assert_eq!(machine.take_launch_request(), Some(0x123456));
        assert_eq!(machine.take_launch_request(), None);
    }

    #[test]
    fn buffer_qualcomm_importa_o_fundo_antes_do_desenho() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine.gl = GlState::new(2, 1);
        machine.egl_surface = 1;
        machine.egl_surfaces.insert(1, (2, 1));
        let get = slot_of(Interface::EglLegacy, "eglGetColorBufferQUALCOMM");
        let buffer = call(&mut machine, Interface::EglLegacy, get, [0; 4]);
        machine
            .cpu
            .write_mem(buffer, &0xf800u16.to_le_bytes())
            .unwrap();
        // Até uma chamada sem vértices deve sincronizar o buffer antes de desenhar.
        let draw = slot_of(Interface::GlLegacy, "glDrawArrays");
        call(
            &mut machine,
            Interface::GlLegacy,
            draw,
            [gles::GL_TRIANGLES, 0, 0, 0],
        );
        assert_eq!(machine.gl.present(2, 1)[0], 0xf800);
        // Uma limpeza posterior não pode ser desfeita por uma cópia antiga do guest.
        let clear = slot_of(Interface::GlLegacy, "glClear");
        call(
            &mut machine,
            Interface::GlLegacy,
            clear,
            [gles::GL_COLOR_BUFFER_BIT, 0, 0, 0],
        );
        let again = call(&mut machine, Interface::EglLegacy, get, [0; 4]);
        assert_eq!(machine.cpu.read_u32(again).unwrap(), 0);
    }

    #[test]
    fn egl_responde_por_ponteiro_de_saida_e_separa_o_gl() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let egl = machine.new_object(Interface::Egl).unwrap();
        let out = loader::HEAP_BASE + 0x800;

        let get_display = slot_of(Interface::Egl, "GetDisplay");
        assert_eq!(
            call(&mut machine, Interface::Egl, get_display, [egl, 0, out, 0]),
            SUCCESS
        );
        assert_eq!(machine.cpu.read_u32(out).unwrap(), EGL_DISPLAY);

        // `GetConfigAttrib(this, dpy, config, attribute, *value, *ret)`: os dois últimos
        // argumentos vão na pilha.
        let attrib = slot_of(Interface::Egl, "GetConfigAttrib");
        let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 - 64;
        machine.cpu.write_reg(Reg::Sp, sp);
        machine.cpu.write_u32(sp, out).unwrap();
        machine.cpu.write_u32(sp + 4, out + 4).unwrap();
        call(
            &mut machine,
            Interface::Egl,
            attrib,
            [egl, EGL_DISPLAY, EGL_CONFIG, gles::EGL_GREEN_SIZE],
        );
        assert_eq!(machine.cpu.read_u32(out).unwrap(), 6);
        assert_eq!(machine.cpu.read_u32(out + 4).unwrap(), gles::EGL_TRUE);

        // O objeto de GL é outro: devolver `this` entregaria a vtable do EGL a quem pediu a
        // do OpenGL, e a primeira chamada cairia num slot que não existe.
        let query = slot_of(Interface::Egl, "QueryInterface");
        assert_eq!(
            call(
                &mut machine,
                Interface::Egl,
                query,
                [egl, AEEIID_GLES11, out, 0]
            ),
            SUCCESS
        );
        let gl = machine.cpu.read_u32(out).unwrap();
        assert_ne!(gl, egl);
        assert_eq!(aee::decode(machine.cpu.read_u32(gl).unwrap()), None);

        // Uma interface que não conhecemos tem de virar recusa, não um objeto errado.
        assert_eq!(
            call(
                &mut machine,
                Interface::Egl,
                query,
                [egl, 0x1234_5678, out, 0]
            ),
            ECLASSNOTSUPPORT
        );
        assert_eq!(machine.cpu.read_u32(out).unwrap(), 0);
    }

    #[test]
    fn cria_sinal_e_enfileira_o_callback() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let factory = machine.new_object(Interface::SignalCbFactory).unwrap();
        // CreateSignal(pfn, pCx, ppiSig, ppiSigCtl): o quarto argumento vai pela pilha.
        let out_signal = loader::HEAP_BASE;
        let out_control = loader::HEAP_BASE + 4;
        machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
        machine
            .cpu
            .write_u32(loader::STACK_BASE + 0x1000, out_control)
            .unwrap();
        let code = call(
            &mut machine,
            Interface::SignalCbFactory,
            slot_of(Interface::SignalCbFactory, "CreateSignal"),
            [factory, 0xcafe, 0xbeef, out_signal],
        );
        assert_eq!(code, SUCCESS);

        let signal = machine.cpu.read_u32(out_signal).unwrap();
        let control = machine.cpu.read_u32(out_control).unwrap();
        assert_ne!(signal, 0);
        assert_ne!(control, 0);
        assert_ne!(signal, control, "sinal e controle são objetos distintos");

        // Disparar o sinal enfileira o callback com o contexto que o app registrou.
        call(
            &mut machine,
            Interface::Signal,
            slot_of(Interface::Signal, "Set"),
            [signal, 0, 0, 0],
        );
        assert_eq!(
            machine.pending_signals,
            vec![Callback {
                function: 0xcafe,
                context: 0xbeef
            }]
        );
    }

    #[test]
    fn desenha_na_tela_pelo_ibitmap() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // O jogo pega a superfície da tela e pinta um retângulo vermelho nela.
        let display = machine.new_object(Interface::Display).unwrap();
        let out_bitmap = loader::HEAP_BASE + 64;
        let code = call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "GetDeviceBitmap"),
            [display, out_bitmap, 0, 0],
        );
        assert_eq!(
            code, SUCCESS,
            "GetDeviceBitmap devolve código de erro, não o ponteiro"
        );
        let bitmap = machine.cpu.read_u32(out_bitmap).unwrap();
        assert_ne!(bitmap, 0);

        let red = call(
            &mut machine,
            Interface::Bitmap,
            slot_of(Interface::Bitmap, "RGBToNative"),
            [bitmap, 0x0000_ff00, 0, 0],
        );
        assert_eq!(red, Rgb { r: 255, g: 0, b: 0 }.to_rgb565() as u32);

        // AEERect { x: 10, y: 20, largura: 30, altura: 40 } escrito na memória do guest.
        let rect_addr = loader::HEAP_BASE;
        machine
            .cpu
            .write_mem(rect_addr, &[10, 0, 20, 0, 30, 0, 40, 0])
            .unwrap();
        call(
            &mut machine,
            Interface::Bitmap,
            slot_of(Interface::Bitmap, "FillRect"),
            [bitmap, rect_addr, red, 0],
        );

        assert!(machine.screen().is_dirty());
        assert_eq!(machine.screen().get_pixel(15, 25), red as u16);
        assert_eq!(
            machine.screen().get_pixel(5, 5),
            0,
            "fora do retângulo continua intacto"
        );
    }

    #[test]
    fn bitmap_compativel_nasce_do_tamanho_pedido() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let display = machine.new_object(Interface::Display).unwrap();
        let out_screen = loader::HEAP_BASE + 64;
        call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "GetDeviceBitmap"),
            [display, out_screen, 0, 0],
        );
        let screen = machine.cpu.read_u32(out_screen).unwrap();
        let out = loader::HEAP_BASE;
        call(
            &mut machine,
            Interface::Bitmap,
            slot_of(Interface::Bitmap, "CreateCompatibleBitmap"),
            [screen, out, 64, 32],
        );

        let created = machine.cpu.read_u32(out).unwrap();
        assert_ne!(created, 0);
        assert_ne!(created, screen);
        let info = machine.bitmaps.get(&created).unwrap();
        assert_eq!((info.width(), info.height()), (64, 32));
    }

    #[test]
    fn widget_slot13_devolve_superficie_utilizavel() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let widget = machine.new_object(Interface::Widget).unwrap();
        let out = loader::HEAP_BASE;
        call(&mut machine, Interface::Widget, 13, [widget, out, 640, 100]);
        let bitmap = machine.cpu.read_u32(out).unwrap();
        assert_ne!(bitmap, 0);
        assert_eq!(machine.objects.kind_of(bitmap), Some(Interface::Bitmap));
        let surface = &machine.bitmaps[&bitmap];
        assert_eq!((surface.width(), surface.height()), (640, 100));
    }

    #[test]
    fn widget_slot17_possui_o_modelo_do_roller() {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        let roller = machine.new_object(Interface::Widget).unwrap();
        let fonte = machine.new_object(Interface::Widget).unwrap();
        machine.widgets.insert(
            roller,
            Widget {
                visivel: true,
                ..Widget::default()
            },
        );

        call(
            &mut machine,
            Interface::Widget,
            17,
            [roller, 0x8000, fonte, 0],
        );
        assert_eq!(machine.widgets[&roller].modelos.get(&0x8000), Some(&fonte));

        // O módulo solta a fonte depois de associá-la; o roller ainda a possui.
        call(&mut machine, Interface::Widget, 1, [fonte, 0, 0, 0]);
        assert_eq!(machine.objects.kind_of(fonte), Some(Interface::Widget));

        let outra_fonte = machine.new_object(Interface::Widget).unwrap();
        call(
            &mut machine,
            Interface::Widget,
            17,
            [roller, 0x8000, outra_fonte, 0],
        );
        assert_eq!(
            machine.objects.kind_of(fonte),
            None,
            "trocar solta o modelo anterior"
        );

        call(&mut machine, Interface::Widget, 1, [outra_fonte, 0, 0, 0]);
        call(&mut machine, Interface::Widget, 1, [roller, 0, 0, 0]);
        assert_eq!(machine.objects.kind_of(outra_fonte), None);
    }

    /// O corpo do `import` do Zeeboids vem em `deflate` cru, e o das outras respostas vem em
    /// texto. A ponte precisa inflar o primeiro sem estragar o segundo.
    #[test]
    fn o_inflador_reconhece_o_import_e_deixa_o_texto_em_paz() {
        use std::io::Write;

        let resposta = b"0;10;32439;202CB962;355800020137588;Ira;1;01/01/1980";
        let mut zip = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
        zip.write_all(resposta).unwrap();
        let comprimido = zip.finish().unwrap();

        assert_eq!(inflate(&comprimido).as_deref(), Some(&resposta[..]));
        // Texto puro não é deflate, e tentar inflá-lo tem de falhar em vez de devolver lixo.
        assert_eq!(inflate(b"0;2;100002;"), None);
    }

    /// A Z-Wheel pede a imagem 5035 ao arquivo do idioma, não a encontra, recua para o
    /// `tectoyli.brf` e a encontra lá. A primeira tentativa não é um arquivo faltando.
    #[test]
    fn recurso_achado_em_outro_arquivo_sai_da_lista_de_faltas() {
        assert_eq!(
            id_do_recurso("fs:/~0x01070798/tectoy_pt.brf (recurso 5035)"),
            Some(5035)
        );
        assert_eq!(id_do_recurso("fontsize.map"), None);
        assert_eq!(id_do_recurso("um (recurso não)"), None);
    }

    #[test]
    fn timer_dispara_uma_vez_e_o_callback_pode_rearmar() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let shell = machine.module.shell;
        let set = slot_of(Interface::Shell, "SetTimer");
        assert_eq!(
            call(
                &mut machine,
                Interface::Shell,
                set,
                [shell, 100, 0x1234, 0x5678]
            ),
            SUCCESS
        );
        assert_eq!(machine.armed_timers(), 1);

        // Faltam os 100 ms inteiros — é o que `GetTimerExpiration` tem de responder.
        let expiration = slot_of(Interface::Shell, "GetTimerExpiration");
        assert_eq!(
            call(
                &mut machine,
                Interface::Shell,
                expiration,
                [shell, 0x1234, 0x5678, 0]
            ),
            100
        );

        // Sem mais nada a fazer, o laço pula o tempo ocioso — um quadro por volta, que é a
        // granularidade em que uma tecla poderia chegar. O timer sai da lista antes de
        // disparar: um timer do BREW é de um disparo só.
        for _ in 0..VOLTAS_ATE_VENCER {
            machine.advance(1_000).unwrap();
            if machine.armed_timers() == 0 {
                break;
            }
        }
        assert!(machine.clock_ms() >= 100);
        assert_eq!(machine.armed_timers(), 0);

        // Rearmar o mesmo par (pfn, pUser) substitui, em vez de acumular.
        call(
            &mut machine,
            Interface::Shell,
            set,
            [shell, 10, 0x1234, 0x5678],
        );
        call(
            &mut machine,
            Interface::Shell,
            set,
            [shell, 20, 0x1234, 0x5678],
        );
        assert_eq!(machine.armed_timers(), 1);

        let cancel = slot_of(Interface::Shell, "CancelTimer");
        call(
            &mut machine,
            Interface::Shell,
            cancel,
            [shell, 0x1234, 0x5678, 0],
        );
        assert_eq!(machine.armed_timers(), 0);
    }

    /// Quantas voltas do laço dar antes de desistir de esperar um timer vencer.
    ///
    /// O salto do ocioso é de um quadro por volta, então um timer de meio segundo leva umas
    /// trinta; o número é folgado de propósito, para o teste falhar por não vencer e não por
    /// ter contado apertado.
    const VOLTAS_ATE_VENCER: usize = 200;

    /// O relógio anda com o trabalho do guest, e o ocioso é pulado um quadro por vez até o
    /// próximo timer — nunca por um passo fixo, que era o que fazia o tempo virtual correr na
    /// frente do jogo.
    #[test]
    fn o_relogio_pula_o_ocioso_ate_o_proximo_timer() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Sem timer armado não há para onde pular: o relógio fica onde está.
        let before = machine.clock_ms();
        machine.advance(1_000).unwrap();
        assert_eq!(machine.clock_ms(), before);

        let shell = machine.module.shell;
        let set = slot_of(Interface::Shell, "SetTimer");
        call(&mut machine, Interface::Shell, set, [shell, 500, 0x1234, 0]);
        for _ in 0..VOLTAS_ATE_VENCER {
            machine.advance(1_000).unwrap();
            if machine.armed_timers() == 0 {
                break;
            }
        }
        assert!(machine.clock_ms() >= before + 500);
        assert_eq!(machine.armed_timers(), 0);
    }

    /// Um PNG 2×1 com paleta e `tRNS`: o primeiro pixel vermelho opaco, o segundo transparente.
    /// É o formato que o Bejeweled Twist usa, e o que quebrava o decodificador antes da
    /// expansão de paleta.
    fn palette_png() -> Vec<u8> {
        fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
            let mut out = (data.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            let crc = crc32(&out[4..]);
            out.extend_from_slice(&crc.to_be_bytes());
            out
        }
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &byte in data {
                crc ^= byte as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 {
                        (crc >> 1) ^ 0xedb8_8320
                    } else {
                        crc >> 1
                    };
                }
            }
            !crc
        }
        // zlib sem compressão: cabeçalho, um bloco final e o adler32 da linha.
        let row = [0u8, 0, 1];
        let mut zlib = vec![0x78, 0x01, 0x01, 3, 0, 0xfc, 0xff];
        zlib.extend_from_slice(&row);
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &row {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());

        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        // 2×1, 8 bits por amostra, tipo 3 (paleta).
        let mut ihdr = 2u32.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 3, 0, 0, 0]);
        png.extend(chunk(b"IHDR", &ihdr));
        png.extend(chunk(b"PLTE", &[255, 0, 0, 0, 255, 0]));
        png.extend(chunk(b"tRNS", &[255, 0]));
        png.extend(chunk(b"IDAT", &zlib));
        png.extend(chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn decodifica_png_com_paleta_e_transparencia() {
        let image = decode_png(&palette_png()).expect("o PNG de teste deve ser aceito");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.pixels[0], Rgb { r: 255, g: 0, b: 0 }.to_rgb565());
        assert_eq!(image.opaque, vec![true, false]);
    }

    #[test]
    fn imagem_chega_pelo_stream_e_desenha_respeitando_o_alfa() {
        let module_image = module_calling_malloc();
        let module = loader::load(&module_image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // O PNG vive na memória do guest, como se tivesse vindo do `resources.dat`.
        let png = palette_png();
        let buffer = loader::HEAP_BASE + 0x1000;
        machine.cpu.write_mem(buffer, &png).unwrap();

        let stream = machine.new_object(Interface::MemAStream).unwrap();
        call(
            &mut machine,
            Interface::MemAStream,
            slot_of(Interface::MemAStream, "Set"),
            [stream, buffer, png.len() as u32, 0],
        );

        let image = machine.new_object(Interface::Image).unwrap();
        call(
            &mut machine,
            Interface::Image,
            slot_of(Interface::Image, "SetStream"),
            [image, stream, 0, 0],
        );

        // GetInfo devolve as dimensões: cx e cy como uint16.
        let info = loader::HEAP_BASE;
        call(
            &mut machine,
            Interface::Image,
            slot_of(Interface::Image, "GetInfo"),
            [image, info, 0, 0],
        );
        let mut size = [0u8; 4];
        machine.cpu.read_mem(info, &mut size).unwrap();
        assert_eq!(u16::from_le_bytes([size[0], size[1]]), 2);
        assert_eq!(u16::from_le_bytes([size[2], size[3]]), 1);

        // Desenhar deixa o pixel opaco e preserva o que havia sob o transparente.
        let screen = machine.device_bitmap().unwrap();
        machine
            .bitmaps
            .get_mut(&screen)
            .unwrap()
            .set_pixel_native(11, 5, 0x1234);
        call(
            &mut machine,
            Interface::Image,
            slot_of(Interface::Image, "Draw"),
            [image, 10, 5, 0],
        );
        let screen_fb = machine.bitmaps.get(&screen).unwrap();
        assert_eq!(
            screen_fb.get_pixel(10, 5),
            Rgb { r: 255, g: 0, b: 0 }.to_rgb565()
        );
        assert_eq!(
            screen_fb.get_pixel(11, 5),
            0x1234,
            "o alfa zero não desenha"
        );
    }

    #[test]
    fn o_qsort_ordena_chamando_a_comparacao_do_jogo() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Um comparador de inteiros, como o que um jogo escreveria:
        //   ldr r2, [r0]
        //   ldr r3, [r1]
        //   sub r0, r2, r3
        //   bx  lr
        let compare = loader::MODULE_BASE + 0x900;
        for (i, word) in [0xe590_2000u32, 0xe591_3000, 0xe042_0003, 0xe12f_ff1e]
            .iter()
            .enumerate()
        {
            machine
                .cpu
                .write_mem(compare + i as u32 * 4, &word.to_le_bytes())
                .unwrap();
        }

        let base = loader::HEAP_BASE;
        let values: [u32; 6] = [40, 10, 60, 20, 50, 30];
        for (i, value) in values.iter().enumerate() {
            machine.cpu.write_u32(base + i as u32 * 4, *value).unwrap();
        }

        machine
            .qsort(base, values.len() as u32, 4, compare)
            .unwrap();

        let sorted: Vec<u32> = (0..values.len())
            .map(|i| machine.cpu.read_u32(base + i as u32 * 4).unwrap())
            .collect();
        assert_eq!(sorted, [10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn o_qsort_devolve_os_registradores_como_os_encontrou() {
        // Ordenar reentra no guest, e o chamador não pode perceber o desvio: quando o `qsort`
        // volta, o despacho ainda vai escrever `r0` e retomar em `lr`.
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let compare = loader::MODULE_BASE + 0x900;
        for (i, word) in [0xe590_2000u32, 0xe591_3000, 0xe042_0003, 0xe12f_ff1e]
            .iter()
            .enumerate()
        {
            machine
                .cpu
                .write_mem(compare + i as u32 * 4, &word.to_le_bytes())
                .unwrap();
        }
        let base = loader::HEAP_BASE;
        for (i, value) in [3u32, 1, 2].iter().enumerate() {
            machine.cpu.write_u32(base + i as u32 * 4, *value).unwrap();
        }

        machine.cpu.write_reg(Reg::R4, 0xdead_beef);
        machine.cpu.write_reg(Reg::Lr, 0x0001_2345);
        machine.qsort(base, 3, 4, compare).unwrap();
        assert_eq!(machine.cpu.read_reg(Reg::R4), 0xdead_beef);
        assert_eq!(machine.cpu.read_reg(Reg::Lr), 0x0001_2345);
    }

    #[test]
    fn argumentos_alem_do_quarto_vao_para_a_pilha() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Uma função mínima que devolve o quinto argumento — o primeiro da pilha:
        //   ldr r0, [sp]
        //   bx  lr
        let code = loader::MODULE_BASE + 0x800;
        machine
            .cpu
            .write_mem(code, &0xe59d_0000u32.to_le_bytes())
            .unwrap();
        machine
            .cpu
            .write_mem(code + 4, &0xe12f_ff1eu32.to_le_bytes())
            .unwrap();

        machine
            .cpu
            .write_reg(Reg::Sp, loader::STACK_BASE + loader::STACK_SIZE as u32 - 16);
        let sp_before = machine.cpu.read_reg(Reg::Sp);
        let outcome = machine
            .call_guest_with_stack(code, [1, 2, 3, 4], &[0xabcd, 0x1234], 1_000)
            .unwrap();
        assert_eq!(outcome, Outcome::Returned { code: 0xabcd });
        assert_eq!(
            machine.cpu.read_reg(Reg::Sp),
            sp_before,
            "a pilha volta ao lugar"
        );
    }

    #[test]
    fn fill_rect_respeita_a_operacao_de_raster() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let screen = machine.device_bitmap().unwrap();
        let rect = loader::HEAP_BASE;
        for (i, value) in [0i16, 0, 8, 8].iter().enumerate() {
            machine
                .cpu
                .write_mem(rect + i as u32 * 2, &value.to_le_bytes())
                .unwrap();
        }
        machine
            .bitmaps
            .get_mut(&screen)
            .unwrap()
            .set_pixel_native(2, 2, 0x1234);

        // Preencher com a cor transparente e `AEE_RO_TRANSPARENT` não escreve nada.
        let fill = slot_of(Interface::Bitmap, "FillRect");
        machine.transparency.insert(screen, 0);
        call(
            &mut machine,
            Interface::Bitmap,
            fill,
            [screen, rect, 0, AEE_RO_TRANSPARENT],
        );
        assert_eq!(
            machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
            0x1234,
            "o preenchimento transparente não pode apagar a tela"
        );

        // Com `AEE_RO_COPY`, preenche.
        call(
            &mut machine,
            Interface::Bitmap,
            fill,
            [screen, rect, 0x00ff, AEE_RO_COPY],
        );
        assert_eq!(
            machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
            0x00ff
        );

        // `AEE_RO_XOR` inverte.
        call(
            &mut machine,
            Interface::Bitmap,
            fill,
            [screen, rect, 0xff00, AEE_RO_XOR],
        );
        assert_eq!(
            machine.bitmaps.get(&screen).unwrap().get_pixel(2, 2),
            0xffff
        );
    }

    #[test]
    fn varredura_de_pilha_acha_os_enderecos_de_retorno() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Um `bl` no módulo, para que o endereço seguinte pareça um retorno legítimo.
        let call_site = loader::MODULE_BASE + 0x900;
        machine
            .cpu
            .write_mem(call_site, &0xeb00_0000u32.to_le_bytes())
            .unwrap();
        let ret = call_site + 4;

        let sp = loader::STACK_BASE + loader::STACK_SIZE as u32 - 64;
        machine.cpu.write_reg(Reg::Sp, sp);
        machine.cpu.write_u32(sp, 0xdead_beef).unwrap();
        machine.cpu.write_u32(sp + 4, ret).unwrap();
        // Um endereço do módulo sem `bl` antes não conta como retorno.
        machine
            .cpu
            .write_u32(sp + 8, loader::MODULE_BASE + 0x40)
            .unwrap();

        assert_eq!(machine.scan_stack(), vec![ret]);
    }

    #[test]
    fn device_info_ex_devolve_o_imei_e_o_tamanho_dele() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let buffer = loader::HEAP_BASE;
        let size = loader::HEAP_BASE + 0x100;

        // Item que não conhecemos: `EUNSUPPORTED`, sem tocar em nada.
        machine.cpu.write_u32(size, 16).unwrap();
        machine.cpu.write_reg(Reg::R1, 1);
        machine.cpu.write_reg(Reg::R2, buffer);
        machine.cpu.write_reg(Reg::R3, size);
        assert_eq!(machine.shell_get_device_info_ex().unwrap(), EUNSUPPORTED);

        // O IMEI cabe nos 16 bytes que o Zeebo Sports Peteca oferece, com o terminador.
        machine.cpu.write_reg(Reg::R1, DEVICEITEM_IMEI);
        assert_eq!(machine.shell_get_device_info_ex().unwrap(), SUCCESS);
        assert_eq!(machine.cpu.read_u32(size).unwrap(), IMEI.len() as u32);
        let mut got = [0u8; 16];
        machine.cpu.read_mem(buffer, &mut got).unwrap();
        assert_eq!(&got, IMEI);

        // Sem `pnSize` não há como devolver o tamanho, e a chamada é inválida.
        machine.cpu.write_reg(Reg::R3, 0);
        assert_eq!(machine.shell_get_device_info_ex().unwrap(), EBADPARM);
    }

    #[test]
    fn device_info_preenche_os_campos_estendidos_quando_pedidos() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let info = loader::HEAP_BASE;
        machine.cpu.write_mem(info, &[0xffu8; 64]).unwrap();
        // Sem `wStructSize`, a parte estendida não é tocada.
        machine
            .cpu
            .write_mem(info + 44, &0u16.to_le_bytes())
            .unwrap();
        machine.cpu.write_reg(Reg::R1, info);
        machine.shell_get_device_info().unwrap();
        let mut path = [0u8; 2];
        machine.cpu.read_mem(info + 56, &mut path).unwrap();
        assert_eq!(
            u16::from_le_bytes(path),
            0xffff,
            "não deve escrever além de dwLang"
        );

        // Com `wStructSize = 64` — que é o que o Bejeweled Twist manda —, os campos
        // estendidos vêm preenchidos.
        machine
            .cpu
            .write_mem(info + 44, &64u16.to_le_bytes())
            .unwrap();
        machine.cpu.write_reg(Reg::R1, info);
        machine.shell_get_device_info().unwrap();

        let mut size = [0u8; 2];
        machine.cpu.read_mem(info + 44, &mut size).unwrap();
        assert_eq!(u16::from_le_bytes(size) as u32, DEVICE_INFO_SIZE);
        machine.cpu.read_mem(info + 56, &mut path).unwrap();
        assert_eq!(u16::from_le_bytes(path) as u32, AEE_MAX_FILE_NAME);
        // E os campos básicos continuam certos.
        let mut screen = [0u8; 4];
        machine.cpu.read_mem(info, &mut screen).unwrap();
        assert_eq!(u16::from_le_bytes([screen[0], screen[1]]), SCREEN_WIDTH);
        assert_eq!(u16::from_le_bytes([screen[2], screen[3]]), SCREEN_HEIGHT);
    }

    #[test]
    fn licenca_do_modulo_nao_expira() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let license = machine.new_object(Interface::License).unwrap();
        assert_eq!(
            call(
                &mut machine,
                Interface::License,
                slot_of(Interface::License, "IsExpired"),
                [license, 0, 0, 0],
            ),
            FALSE
        );

        let expire = loader::HEAP_BASE;
        assert_eq!(
            call(
                &mut machine,
                Interface::License,
                slot_of(Interface::License, "GetInfo"),
                [license, expire, 0, 0],
            ),
            LT_NONE
        );
        assert_eq!(machine.cpu.read_u32(expire).unwrap(), BV_UNLIMITED);

        // A própria documentação manda recusar quando o tipo não é `LT_USES`.
        assert_eq!(
            call(
                &mut machine,
                Interface::License,
                slot_of(Interface::License, "SetUsesRemaining"),
                [license, 1, 0, 0],
            ),
            EFAILED
        );

        let kind = loader::HEAP_BASE + 16;
        let seq = loader::HEAP_BASE + 24;
        assert_eq!(
            call(
                &mut machine,
                Interface::License,
                slot_of(Interface::License, "GetPurchaseInfo"),
                [license, kind, expire, seq],
            ),
            PT_PURCHASE
        );
        let mut license_type = [0u8; 1];
        machine.cpu.read_mem(kind, &mut license_type).unwrap();
        assert_eq!(license_type[0] as u32, LT_NONE);
        assert_eq!(machine.cpu.read_u32(seq).unwrap(), 0);
    }

    #[test]
    fn item_id_da_classe_vem_do_diretorio_do_modulo() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        // O `.mod` do console mora num diretório com o número do item da loja do BREW.
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, "roms/jogo/mod/277083");
        machine.cpu.reset(&machine.module.mem).unwrap();
        machine.applet_class = 0x0108_c1e1;

        machine.cpu.write_reg(Reg::R1, 0x0108_c1e1);
        assert_eq!(machine.shell_get_class_item_id(), 277083);

        // Classe de outro módulo: a documentação manda devolver zero.
        machine.cpu.write_reg(Reg::R1, 0x0100_1001);
        assert_eq!(machine.shell_get_class_item_id(), 0);
    }

    #[test]
    fn som_guarda_a_configuracao_e_avisa_o_fim_da_reproducao() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let sound = machine.new_object(Interface::Sound).unwrap();
        let info = loader::HEAP_BASE;
        // AEE_SOUND_DEVICE_SPEAKER, AEE_SOUND_METHOD_MIDI, local, sem mute.
        machine.cpu.write_mem(info, &[9, 6, 0, 0, 0]).unwrap();
        let set = call(
            &mut machine,
            Interface::Sound,
            slot_of(Interface::Sound, "Set"),
            [sound, info, 0, 0],
        );
        assert_eq!(set, SUCCESS);

        let out = loader::HEAP_BASE + 16;
        call(
            &mut machine,
            Interface::Sound,
            slot_of(Interface::Sound, "Get"),
            [sound, out, 0, 0],
        );
        let mut read_back = [0u8; 5];
        machine.cpu.read_mem(out, &mut read_back).unwrap();
        assert_eq!(read_back, [9, 6, 0, 0, 0]);

        // Sem `RegisterNotify` não há para quem avisar.
        call(
            &mut machine,
            Interface::Sound,
            slot_of(Interface::Sound, "PlayTone"),
            [sound, 0, 0, 0],
        );
        assert!(machine.pending_calls.is_empty());

        call(
            &mut machine,
            Interface::Sound,
            slot_of(Interface::Sound, "RegisterNotify"),
            [sound, 0x1234, 0x5678, 0],
        );
        call(
            &mut machine,
            Interface::Sound,
            slot_of(Interface::Sound, "PlayTone"),
            [sound, 0, 0, 0],
        );
        let notify = machine.pending_calls.first().copied().unwrap();
        assert_eq!(notify.function, 0x1234);
        // PFNSOUNDSTATUS(pUser, eCBType, eSPStatus, dwParam)
        assert_eq!(
            notify.args,
            [0x5678, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE, 0]
        );
    }

    #[test]
    fn o_fim_do_som_avisa_o_jogo() {
        // Um jogo que toca uma coisa de cada vez espera o aviso de fim para tocar a próxima.
        // Sem ele o som para depois do primeiro efeito, e foi o que acontecia.
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        // Um RIFF de meio segundo: PCM de 8 bits, mono, 8000 Hz.
        let wave = riff_pcm8(8000, 4000);
        let buffer = machine.heap.alloc(wave.len() as u32).unwrap();
        machine.cpu.write_mem(buffer, &wave).unwrap();
        let data = machine.heap.alloc(12).unwrap();
        for (index, value) in [MMD_BUFFER, buffer, wave.len() as u32]
            .into_iter()
            .enumerate()
        {
            machine
                .cpu
                .write_u32(data + index as u32 * 4, value)
                .unwrap();
        }

        let out = machine.heap.alloc(4).unwrap();
        let util = machine.new_object(Interface::MediaUtil).unwrap();
        call(
            &mut machine,
            Interface::MediaUtil,
            slot_of(Interface::MediaUtil, "CreateMedia"),
            [util, data, out, 0],
        );
        let media = machine.cpu.read_u32(out).unwrap();

        call(
            &mut machine,
            Interface::Media,
            slot_of(Interface::Media, "RegisterNotify"),
            [media, 0x4321, 0x8765, 0],
        );
        call(
            &mut machine,
            Interface::Media,
            slot_of(Interface::Media, "Play"),
            [media, 0, 0, 0],
        );

        // Tocar avisa na hora que começou.
        let start = machine
            .pending_calls
            .pop()
            .expect("faltou o aviso de início");
        assert_eq!(start.function, 0x4321);
        assert_eq!(start.args[0], 0x8765);
        let block = start.args[1];
        assert_eq!(machine.cpu.read_u32(block + 4).unwrap(), media, "pIMedia");
        assert_eq!(machine.cpu.read_u32(block + 8).unwrap(), MM_CMD_PLAY);
        assert_eq!(machine.cpu.read_u32(block + 16).unwrap(), MM_STATUS_START);

        // Antes da hora, nada: o som ainda está tocando.
        machine.poll_media().unwrap();
        assert!(machine.pending_calls.is_empty());

        // Passada a duração, o aviso de fim sai — e sai do relógio virtual, sem depender de o
        // som estar realmente saindo pela placa.
        machine.clock_us += 600_000;
        machine.poll_media().unwrap();
        let done = machine.pending_calls.pop().expect("faltou o aviso de fim");
        assert_eq!(done.function, 0x4321);
        assert_eq!(
            machine.cpu.read_u32(done.args[1] + 16).unwrap(),
            MM_STATUS_DONE
        );
        // E não se repete: o som já acabou uma vez.
        machine.poll_media().unwrap();
        assert!(machine.pending_calls.is_empty());
    }

    #[test]
    fn ler_o_relogio_em_laco_faz_o_tempo_passar() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let clock = slot_of(Interface::Helpers, "aee_GetUpTimeMS");
        let yield_ = slot_of(Interface::Thread, "Suspend");

        // Até o limite, ler o relógio é trabalho normal de quadro e não custa tempo.
        for _ in 0..SPIN_THRESHOLD {
            call(&mut machine, Interface::Helpers, clock, [0; 4]);
        }
        assert_eq!(machine.clock_us, 0);

        // Passado o limite, cada leitura adianta o relógio — e ceder a vez, que é como o laço
        // dá a volta, não interrompe a contagem.
        for _ in 0..10 {
            call(&mut machine, Interface::Thread, yield_, [0; 4]);
            call(&mut machine, Interface::Helpers, clock, [0; 4]);
        }
        assert_eq!(machine.clock_us, 10 * SPIN_STEP_US);
    }

    #[test]
    fn trabalho_no_meio_do_laco_desfaz_a_espera_ocupada() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let clock = slot_of(Interface::Helpers, "aee_GetUpTimeMS");
        let work = slot_of(Interface::Helpers, "aee_GetRand");

        // Um quadro que lê o relógio no meio do que faz continua custando o que custa: a
        // contagem zera a cada chamada que não é leitura nem cedida de vez.
        for _ in 0..(SPIN_THRESHOLD * 4) {
            call(&mut machine, Interface::Helpers, clock, [0; 4]);
            call(&mut machine, Interface::Helpers, work, [0, 0, 0, 0]);
        }
        assert_eq!(machine.clock_us, 0);
    }

    #[test]
    fn query_interface_do_dib_expoe_os_pixels_ao_jogo() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();

        let display = machine.new_object(Interface::Display).unwrap();
        let out_screen = loader::HEAP_BASE + 64;
        call(
            &mut machine,
            Interface::Display,
            slot_of(Interface::Display, "GetDeviceBitmap"),
            [display, out_screen, 0, 0],
        );
        let screen = machine.cpu.read_u32(out_screen).unwrap();

        let out = loader::HEAP_BASE;
        call(
            &mut machine,
            Interface::Bitmap,
            slot_of(Interface::Bitmap, "QueryInterface"),
            [screen, AEECLSID_DIB, out, 0],
        );

        // Um `IDIB` é o próprio `IBitmap` com os campos públicos preenchidos.
        assert_eq!(machine.cpu.read_u32(out).unwrap(), screen);
        let buffer = machine.cpu.read_u32(screen + 8).unwrap();
        assert!(buffer >= loader::SURFACE_BASE);
        let mut cx = [0u8; 2];
        machine.cpu.read_mem(screen + 20, &mut cx).unwrap();
        assert_eq!(u16::from_le_bytes(cx), SCREEN_WIDTH);
        let mut depth = [0u8; 2];
        machine.cpu.read_mem(screen + 28, &mut depth).unwrap();
        assert_eq!(depth, [COLOR_DEPTH as u8, IDIB_COLORSCHEME_565]);

        // O que o jogo escrever no buffer precisa chegar ao framebuffer do host.
        machine
            .cpu
            .write_mem(buffer, &0xf800u16.to_le_bytes())
            .unwrap();
        machine.sync_from_guest(screen).unwrap();
        assert_eq!(
            machine.bitmaps.get(&screen).unwrap().get_pixel(0, 0),
            0xf800
        );
    }

    /// Uma máquina vazia, para exercitar uma API sem carregar jogo nenhum.
    fn maquina_nua() -> Machine<UnicornCpu> {
        let module = loader::load(&module_calling_malloc()).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");
        machine.cpu.reset(&machine.module.mem).unwrap();
        // Sem pilha não há quinto argumento: o `GetConnectedDevices` lê o `nLenReq` de lá.
        machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
        machine
    }

    /// Com as duas portas ligadas em controle, a enumeração devolve as duas — e o vetor de
    /// saída recebe os dois identificadores, na ordem das portas.
    #[test]
    fn as_duas_portas_aparecem_na_enumeracao() {
        use crate::bindings::Aparelho;
        let mut machine = maquina_nua();
        machine.set_portas([Some(Aparelho::Controle), Some(Aparelho::Controle)]);
        let hid = machine.new_object(Interface::Hid).unwrap();
        let saida = machine.heap.alloc(16).unwrap();
        let quantos = machine.heap.alloc(4).unwrap();
        machine
            .cpu
            .write_u32(machine.cpu.read_reg(Reg::Sp), quantos)
            .unwrap();

        let r = call(
            &mut machine,
            Interface::Hid,
            slot_of(Interface::Hid, "GetConnectedDevices"),
            [hid, UID_JOYSTICK_DEVICE, saida, 2],
        );
        assert_eq!(r, SUCCESS);
        assert_eq!(machine.cpu.read_u32(quantos).unwrap(), 2);
        assert_eq!(machine.cpu.read_u32(saida).unwrap(), 1);
        assert_eq!(machine.cpu.read_u32(saida + 4).unwrap(), 2);
    }

    /// Uma porta em teclado sai da lista de joysticks e entra na de teclados. É o que faz a
    /// Z-Wheel trocar `No keyboard reported` por um teclado encontrado.
    #[test]
    fn a_porta_de_teclado_nao_e_joystick() {
        use crate::bindings::Aparelho;
        let mut machine = maquina_nua();
        machine.set_portas([Some(Aparelho::Controle), Some(Aparelho::Teclado)]);
        let hid = machine.new_object(Interface::Hid).unwrap();
        let saida = machine.heap.alloc(16).unwrap();
        let quantos = machine.heap.alloc(4).unwrap();
        machine
            .cpu
            .write_u32(machine.cpu.read_reg(Reg::Sp), quantos)
            .unwrap();

        for (tipo, esperado, primeiro) in [
            (UID_JOYSTICK_DEVICE, 1u32, 1u32),
            (UID_KEYBOARD_DEVICE, 1, 2),
        ] {
            call(
                &mut machine,
                Interface::Hid,
                slot_of(Interface::Hid, "GetConnectedDevices"),
                [hid, tipo, saida, 2],
            );
            assert_eq!(machine.cpu.read_u32(quantos).unwrap(), esperado);
            assert_eq!(machine.cpu.read_u32(saida).unwrap(), primeiro);
        }
    }

    /// Cada `IHIDDevice` lê a porta de onde veio. Sem isso os dois aparelhos leriam o mesmo
    /// controle, que é o defeito que o jogador de dois enxergaria primeiro.
    #[test]
    fn cada_aparelho_le_a_propria_porta() {
        use crate::bindings::Aparelho;
        let mut machine = maquina_nua();
        machine.set_portas([Some(Aparelho::Controle), Some(Aparelho::Controle)]);
        let hid = machine.new_object(Interface::Hid).unwrap();
        let saida = machine.heap.alloc(4).unwrap();

        let mut aparelho = |handle: u32| {
            call(
                &mut machine,
                Interface::Hid,
                slot_of(Interface::Hid, "CreateDevice"),
                [hid, handle, saida, 0],
            );
            machine.cpu.read_u32(saida).unwrap()
        };
        let (um, dois) = (aparelho(1), aparelho(2));
        assert_ne!(um, dois);

        let mut apertado = Pad::default();
        apertado.press(input::DPAD[0], true);
        machine.set_port_pad(1, apertado);

        let info = machine.heap.alloc(20).unwrap();
        let mut estado = |device: u32| {
            call(
                &mut machine,
                Interface::HidDevice,
                slot_of(Interface::HidDevice, "GetButtonInfo"),
                [device, input::DPAD[0] as u32, info, 0],
            );
            machine.cpu.read_u32(info + 4).unwrap()
        };
        assert_eq!(estado(um), 0, "a porta 1 está parada");
        assert_eq!(estado(dois), 1, "a porta 2 está com o direcional para cima");
    }

    #[test]
    fn atende_malloc_e_retoma_a_execucao() {
        let image = module_calling_malloc();
        let module = loader::load(&image).unwrap();
        let mut machine = Machine::new(UnicornCpu::new().unwrap(), module, ".");

        // r2 precisa apontar para a tabela de helpers e r3 para o sentinela de retorno; a
        // convenção de `AEEMod_Load` já coloca os helpers em r1, então ajustamos após o reset.
        machine.cpu.reset(&machine.module.mem).unwrap();
        let outcome = {
            machine.cpu.write_reg(Reg::R2, loader::HELPERS_BASE);
            machine.cpu.write_reg(Reg::R3, RETURN_MAGIC);
            machine.cpu.write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
            let mut pc = machine.module.entry;
            loop {
                match machine.cpu.run(pc, 100).unwrap() {
                    StopReason::ApiCall { addr } => {
                        let value = machine
                            .dispatch(addr)
                            .unwrap()
                            .expect("MALLOC implementado");
                        machine.cpu.write_reg(Reg::R0, value);
                        pc = machine.cpu.read_reg(Reg::Lr);
                    }
                    other => break other,
                }
            }
        };

        assert_eq!(outcome, StopReason::Returned);
        let ptr = machine.cpu.read_reg(Reg::R0);
        assert_eq!(
            ptr,
            loader::HEAP_BASE,
            "primeiro bloco fica no início do heap"
        );
        assert_eq!(
            machine.cpu.read_u32(ptr).unwrap(),
            0,
            "MALLOC entrega memória zerada"
        );
    }
}
