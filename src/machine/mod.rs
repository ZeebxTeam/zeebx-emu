//! O laço de execução: roda o módulo, atende as chamadas de API e continua.
//!
//! Quando o guest chama um método, o núcleo para com [`StopReason::ApiCall`]. Aqui decidimos
//! o que aquela chamada significa, escrevemos o retorno em `r0` e retomamos a execução em `lr`
//! — que é onde o `bl`/`blx` original deixou o endereço de retorno.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::brew::aee::{self, Interface};
use crate::brew::aee_helpers;
use crate::brew::cformat::{self, ArgSource};
use crate::brew::crypto;
use crate::brew::fmath;
use crate::brew::heap::Heap;
use crate::brew::objects::ObjectStore;
use crate::brew::vfs::Vfs;
use crate::cpu::RETURN_MAGIC;
use crate::cpu::{CpuBackend, CpuError, Reg, StopReason};
use crate::input::{self, Pad};
use crate::loader::{self, LoadedModule};
use crate::ponte;
use crate::rede;
use crate::video::atc;
use crate::video::display::{Framebuffer, Rect, Rgb};
use crate::video::gles;
use crate::video::paltex;
use crate::video::rasterizer::{self, GlState, Rasterizador, Vertex};

mod save;
mod bitmap;
mod cifra;
mod diagnostico;
mod display;
mod diversos;
mod egl;
mod file;
mod font;
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
    // O espaço do `isspace` do C, e não o do Unicode: lido em Latin-1, o `0xA0` viraria espaço.
    let start = text.len()
        - text
            .trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r'])
            .len();
    // **O sinal vale.** A `strtoul` do C aceita `+` e `-`, e com `-` devolve o número negado. O
    // Alice no País das Maravilhas lê linhas como `-2 12 16 73 ...`: parado no `-`, o laço dele
    // contava o campo sem sair do lugar e repetia a linha até esgotar os 18 MB do pool.
    let (negativo, depois_do_sinal) = match text[start..].as_bytes().first() {
        Some(b'-') => (true, start + 1),
        Some(b'+') => (false, start + 1),
        _ => (false, start),
    };
    let rest = &text[depois_do_sinal..];
    let hex = rest.starts_with("0x") || rest.starts_with("0X");
    let (digits, base) = match base {
        0 | 16 if hex => (&rest[2..], 16),
        // O `0` do prefixo octal também é dígito: `"0 1"` consome o zero.
        0 if rest.starts_with('0') => (rest, 8),
        0 => (rest, 10),
        base => (rest, base),
    };
    let taken = digits.chars().take_while(|c| c.is_digit(base)).count();
    let value = u32::from_str_radix(&digits[..taken], base).unwrap_or(u32::MAX);
    let consumed = if taken == 0 {
        0
    } else {
        text.len() - digits.len() + taken
    };
    let value = match (taken, negativo) {
        (0, _) => 0,
        (_, true) => value.wrapping_neg(),
        (_, false) => value,
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
/// Arredonda `valor` para o próximo múltiplo de `alinhamento`.
///
/// É a conta que o `glPixelStorei` manda fazer: cada linha de uma textura começa num múltiplo do
/// alinhamento, e o que sobra entre o fim de uma linha e o começo da seguinte é enchimento. O GL
/// só admite 1, 2, 4 e 8, e o padrão é 4.
pub(super) fn arredonda_para(valor: u32, alinhamento: u32) -> u32 {
    let passo = alinhamento.max(1);
    valor.div_ceil(passo) * passo
}

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
/// O Z-Pad: a entrada "Zeebo Game Controller" do `hid_devices.cfg`.
const ZPAD_VENDOR_ID: u16 = 0x1a5c;
const ZPAD_PRODUCT_ID: u16 = 0x3033;
/// O receptor do Boomerang: a entrada "Zeebo Accelerometer Controller" do `hid_devices.cfg`.
/// Os jogos da Boomerang Sports só tratam o aparelho como Boomerang com este par.
/// De quanto em quanto o receptor do Boomerang manda um relatório: 100 por segundo, alternando os
/// dois jogadores.
const BOOMERANG_PERIODO_US: u64 = 10_000;
const BOOMERANG_VENDOR_ID: u16 = 0x15a2;
const BOOMERANG_PRODUCT_ID: u16 = 0x0003;
/// Identificador de uma porta na enumeração: `1` e `2`, na ordem das portas.
///
/// O `CreateDevice` recebe este número de volta, e é por ele que sabemos de qual porta o
/// aparelho que o jogo acabou de criar vai ler.
const fn handle_da_porta(porta: usize) -> u32 {
    porta as u32 + 1
}
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
/// `AEECLSID_BMP` = `0x01004001`, a mesma família de um em `0x01004000`.
///
/// O Action Hero 3D pede esta, e a identificação não é por palpite: os recursos dele são 67
/// BMPs num `.res` cujo primeiro nome de entrada é `66.bmp`, e ele alimenta o objeto com um
/// `IMemAStream` igual ao que o Bejeweled Twist usa com a `0x01004004`. As duas são `IImage`.
const AEECLSID_BMP: u32 = 0x0100_4001;
/// `AEECLSID_PNGDecoder` e `AEECLSID_PNGDecoderBREW`, de `inc/AEEPNGDecoder*.bid`. A interface
/// padrão das duas é `IImageDecoder`.
const AEECLSID_PNGDECODER: u32 = 0x0102_6e23;
const AEECLSID_PNGDECODER_BREW: u32 = 0x0103_0766;
/// `AEECLSID_JPEGDECODER_BREW` (`AEECLSID_JPEGDecoderBREW` na tabela de ClassIDs do toolset).
///
/// **É o que faltava para o Zuma's Revenge**: ele pede esta classe, recebia recusa, seguia com o
/// ponteiro nulo e quebrava — a varredura o pegava em "quebrou no laço de quadros" com o motivo
/// `acesso inválido a 0x0`. O decodificador que ele quer é o mesmo que já atende PNG, BMP, JPEG e
/// GIF: o nosso olha a assinatura dos bytes e escolhe o formato sozinho.
const AEECLSID_JPEGDECODER_BREW: u32 = 0x0102_fd92;
/// `IPARM_*` de `inc/AEEIImage.h`.
///
/// O `SIZE`, o `OFFSET` e o `ROP` saíram do uso: o Action Hero 3D escreve cada letra do menu
/// com `SetParm(0, cx, 12)`, `SetParm(1, x, 0|12|24)` e `Draw` sobre a folha de fontes de
/// 201x37 — três linhas de 12 pixels —, e prepara a imagem com `SetParm(3, AEE_RO_TRANSPARENT)`.
const IPARM_SIZE: u32 = 0;
const IPARM_OFFSET: u32 = 1;
const IPARM_CXFRAME: u32 = 2;
const IPARM_ROP: u32 = 3;
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
/// `AEECLSID_MEDIAMIDIOUTMSG` = `AEECLSID_MULTIMEDIA + 5`, pela lista do SDK. O Need for Speed a
/// cria ao pular a cena de abertura e, sem conferir o retorno, chama o `SetMediaParm` do ponteiro
/// — recusá-la era saltar para o endereço zero. Por muito tempo esteve com o nome de MPEG4, que é
/// o `+ 7`.
const AEECLSID_MEDIAMIDIOUTMSG: u32 = 0x0100_5505;
/// As outras classes da família, do `AEECLSID_MULTIMEDIA` do SDK: QCP (`+3`), PMD (`+4`),
/// MIDIOUTQCP (`+6`), MPEG4 (`+7`), MMF (`+8`), PHR (`+9`), AAC (`+b`), IMELODY (`+c`), AMR
/// (`+e`), XMF (`+0x12`) e DLS (`+0x13`). Todas viram o mesmo objeto de mídia: o conteúdo passa
/// pelos decodificadores que temos, e o que nenhum lê termina na hora, como qualquer som que não
/// sabemos tocar. Recusar a classe derrubava quem cria sem conferir.
const AEECLSID_MEDIA_FAMILIA: [u32; 11] = [
    0x0100_5503,
    0x0100_5504,
    0x0100_5506,
    0x0100_5507,
    0x0100_5508,
    0x0100_5509,
    0x0100_550b,
    0x0100_550c,
    0x0100_550e,
    0x0100_5512,
    0x0100_5513,
];
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

/// `MMD_FILE_NAME` e `MMD_BUFFER`, o `clsData` de um `AEEMediaData`: nome de arquivo ou memória.
/// O de memória veio da observação — é o que os três primeiros jogos que tocaram som passam; o
/// de arquivo é o que o Galaxy on Fire usa para as músicas.
const MMD_FILE_NAME: u32 = 0;
/// O maior som que lemos de uma vez. Um tamanho absurdo é ponteiro errado, não música.
const MAX_MEDIA_BUFFER: u32 = 64 * 1024 * 1024;
const MMD_BUFFER: u32 = 1;
/// `MMD_ISOURCE`, o `clsData` de um `AEEMediaDataEx` cujos dados vêm de um `ISource` que o
/// próprio jogo implementa. É o `AEECLSID_SOURCE` do BREW, e é o que os ports de arcade da Data
/// East passam, com `bRaw` ligado e um `AEEMediaWaveSpec` dizendo o formato das amostras.
const MMD_ISOURCE: u32 = 0x0100_1012;
/// Slot de `Read` na vtable de `ISource`: `AddRef`, `Release`, `QueryInterface`, `Read`,
/// `Readable`.
const ISOURCE_READ_SLOT: u32 = 3;
/// O maior pedaço de PCM pedido ao `ISource` de uma vez, em bytes.
const MAX_LEITURA_PCM: u32 = 16 * 1024;

/// Um som que o jogo gera enquanto toca: o `ISource` de onde as amostras vêm e o formato delas.
#[derive(Debug, Clone, Copy)]
struct FluxoPcm {
    fonte: u32,
    taxa: u32,
    canais: u16,
    bits: u16,
    sem_sinal: bool,
    /// Quando o `Play` começou, no relógio virtual, e quantos quadros já foram pedidos desde
    /// então. A diferença entre o que o relógio manda e o que já veio é o que falta pedir.
    inicio_us: u64,
    quadros_lidos: u64,
    tocando: bool,
}

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

/// Classes do firmware que não temos e que o jogo usa **sem conferir** se existem.
///
/// O Powerboat Challenge cria a `0x01001039`, guarda o ponteiro e chama um método dela sem olhar
/// o retorno: com a recusa honesta — que é o que o BREW responde para classe que não existe — ele
/// saltava para o endereço zero antes do menu de idioma. Um objeto que responde sucesso a tudo o
/// deixa seguir, e o que ele chamar nele aparece no relatório da sonda.
///
/// A classe em si continua sendo do firmware do console, que ainda não lemos (ver
/// [`15-o-que-falta-da-nand.md`](../../docs/implementacao/15-o-que-falta-da-nand.md)).
const CLASSES_POR_OBSERVACAO: &[u32] = &[0x0100_1039];

/// `AEECLSID_QEGL`, do `AEECLSID_QEGL.bid` do SDK: o objeto que dá acesso ao EGL e ao OpenGL
/// ES pelas interfaces novas do BREW. É por ele que o Quake tenta primeiro.
const AEECLSID_QEGL: u32 = 0x0103_d8ec;
/// `AEECLSID_EGL` e `AEECLSID_GL`, de `sdk/inc/AEEGL.h` — as interfaces antigas, que o Crash
/// Nitro Kart pede direto.
const AEECLSID_EGL: u32 = 0x0101_4bc4;
/// `AEECLSID_WEB`, do `BMPIds.csv` do SDK: o cliente HTTP do BREW.
const AEECLSID_WEB: u32 = 0x0100_5000;
/// `AEECLSID_DOWNLOAD`, do SDK 4.0.2 — e a resposta para o enigma do `0x01000000`.
///
/// O `AEE_CLSIDs.h` diz, em três linhas seguidas: `QVERSION` é `0x01000000`, `AEECLSID_PRIV` é
/// `QVERSION` e **`AEECLSID_DOWNLOAD` é `AEECLSID_PRIV`**. A leitura anterior parou na primeira
/// das três e registrou que a classe era "só o `QVERSION`", sem interface. Medido: é o
/// `IDownload`, e é ele que a Z-Wheel pede em `ShopAction_Init` — recusado, a biblioteca de jogos
/// da roda não monta.
const AEECLSID_DOWNLOAD: u32 = 0x0100_0000;
/// Quantos blocos de texto claro o registro guarda, e quanto de cada um.
const PLAINTEXT_MAX: usize = 8;
const PLAINTEXT_BYTES: usize = 512;

/// Uma palavra do jogo como número: ponto fixo 16.16 nas formas `x`, `float` nas formas `f`.
fn escalar(palavra: u32, fixo: bool) -> f32 {
    match fixo {
        true => gles::fixed(palavra),
        false => f32::from_bits(palavra),
    }
}

const ATENDIDAS_EM_SILENCIO: &[&str] = &[
    "DepthFunc",
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
/// Quantos eventos de botão esperam o `GetNextButtonEvent`, por porta.
const PAD_EVENTS_MAX: usize = 64;
/// Quantas linhas do [`Machine::media_log`] ficam guardadas.
const MEDIA_LOG_MAX: usize = 600;

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
/// `AEEIID_GLES11EXT`, de `sdk/inc/AEEGLES11Ext.h` — as extensões OES do OpenGL ES 1.1.
///
/// **O Prey Evil pede esta classe por `ISHELL_CreateInstance`** e desiste do caminho de desenho
/// quando recebe nulo: sem ela, o levantamento o pega com onze métodos de GL e nenhum desenho.
const AEECLSID_GLES11EXT: u32 = 0x0103_d8eb;

/// `AEEIID_GLES10EXT`, de `sdk/inc/AEEGLES10Ext.h`. O Prey Evil a pede no objeto do EGL.
const AEEIID_GLES10EXT: u32 = 0x0103_d8de;
/// `AEEIID_EGLGETPOWERLEVEL`, de `sdk/inc/AEEEGLGetPowerLevel.h`.
const AEEIID_EGLGETPOWERLEVEL: u32 = 0x0103_d8f0;
/// `AEEIID_EGLOESSWAPINTERVAL`, de `sdk/inc/AEEEGLOESSwapInterval.h`.
const AEEIID_EGLOESSWAPINTERVAL: u32 = 0x0104_26e3;
/// `AEEIID_EGLGETCOLORBUFFER`, de `sdk/inc/AEEEGLGetColorBuffer.h`.
const AEEIID_EGLGETCOLORBUFFER: u32 = 0x0103_d8ef;
/// `AEEIID_GLES11EXTPAK`, de `sdk/inc/AEEGLES11ExtPak.h`.
const AEEIID_GLES11EXTPAK: u32 = 0x0103_def1;
/// `AEECLSID_IJOYSTICK1` e `AEECLSID_IJOYSTICK2`, de `sdk/inc/AEEJoystick.h`.
///
/// **O Prey Evil cria esta e guarda o resultado.** Nula, o gerenciador de joystick da Qualcomm
/// segue com o ponteiro vazio e cai no primeiro `Read` — que é a falha que o levantamento pegou.
const AEECLSID_IJOYSTICK1: u32 = 0x0102_1c2b;
const AEECLSID_IJOYSTICK2: u32 = 0x0102_1dac;

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
/// `AEEIID_TRANSFORM`, de `AEETransform.h`: escala e rotação de um bitmap sobre outro.
///
/// Esteve aqui como "terceiro IID do `IDIB`", deduzido por ser vizinho do `AEEIID_DIB_20`, e
/// respondido com o próprio bitmap. O uso desmente: o Zenonia pede este IID ao bitmap **da
/// tela**, guarda o ponteiro e chama o slot 4 dele com `(x, y, pSrc, xSrc, ySrc, dx, dy,
/// pMatrix, nComposite)` — a assinatura do `TransformBltComplex`. Respondido como bitmap, o
/// slot 4 caía no `NativeToRGB`, o quadro nunca chegava à tela e o jogo ficava preto
/// desenhando o tempo todo num canvas 320x240.
const AEEIID_TRANSFORM: u32 = 0x0100_1029;
/// A interface de canvas que a Z-Wheel pede a um bitmap. Ver [`Interface::Canvas`].
const AEEIID_CANVAS: u32 = 0x0101_e443;
/// `IDIB_COLORSCHEME_565`, de `inc/AEEIDIB.h`: 5 bits de vermelho, 6 de verde, 5 de azul.
const IDIB_COLORSCHEME_565: u8 = 16;
/// `IDIB_COLORSCHEME_888`: 8 bits por canal.
const IDIB_COLORSCHEME_888: u8 = 24;
/// `AEECLSID_DIB` = `AEECLSID_CORE + 69`. É o bitmap com acesso direto aos pixels.
const AEECLSID_DIB: u32 = 0x0100_1045;
/// `AEEIID_IBitmap`, de `inc/AEEIBitmap.h`.
const AEEIID_IBITMAP: u32 = 0x0100_1021;
/// Teto de vértices por polígono, contra ponteiro corrompido.
const MAX_POLYGON_POINTS: usize = 4096;
/// `AEE_MAX_FILE_NAME`.
const MAX_FILE_NAME: usize = 64;
/// Quantas linhas de texto desenhado o relatório guarda.
const MAX_TEXTOS_DESENHADOS: usize = 64;
/// Quantas chamadas de arquivo o relatório guarda. Um jogo de 6 s faz centenas de `Test`;
/// o que interessa é o começo da execução, e é ele que a fila preserva.
const MAX_FS_LOG: usize = 96;
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
    /// Os liberadores dos dois trios, a terceira palavra de cada: `(do tratador, do desenho)`.
    /// Ver o `Release` do widget.
    liberadores: (u32, u32),
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
/// O formulário raiz do applet, o `[app+0x24]` da Z-Wheel. Nele o slot 6 é o
/// `IROOTFORM_RemoveForm`. Ver o braço do `DefinirVisivel`.
const WIDGET_RAIZ: u32 = 0x0102_8e51;

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
/// A cor de fundo de um widget, `RRGGBBAA`, pelo mesmo acessador da [`PROP_COR`].
const PROP_COR_DE_FUNDO: u32 = 0x130;

/// A classe do widget de HTML, que a tela de ajuda (`FAQ_Form.c`) cria para mostrar as páginas.
///
/// Não renderizamos HTML: o widget é atendido como os outros da família e mostra o texto de um
/// arquivo de marcação que fica **fora** da ROM — ver `Machine::texto_do_html`. Sem a classe,
/// a tela registrava `Unable to create HTML Widget, ERROR(20)` e confirmar em "Ajuda" não abria
/// nada.
const WIDGET_HTML: u32 = 0x0102_dd32;

const FAMILIA_DOS_WIDGETS: [u32; 10] = [
    WIDGET_HTML,
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
/// A fonte com que a Z-Wheel desenha o rolo de capas. **É uma fonte do sistema**: `0x0102f67c` é o
/// `AEECLSID_FONT_STANDARD18B`, atendido por [`crate::machine::font`]. O nome fica porque é assim
/// que a Z-Wheel o chama.
#[allow(dead_code)]
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
/// `EVT_APP_STOP`, de `inc/AEEEvent.h`: o aviso de que o applet vai ser fechado.
const EVT_APP_STOP: u32 = 1;

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
    /// Onde o arquivo aberto mora no host. Nem sempre é o que o `guest_path` resolve: a
    /// `tectoy.cfg` sem fim de vida é uma cópia no perfil do aparelho.
    caminho: std::path::PathBuf,
}

/// Um callback do guest: a função e o contexto que ela recebe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Callback {
    pub function: u32,
    pub context: u32,
}

/// Um `IValueModel`: o valor e quem quer saber quando ele muda.
#[derive(Debug, Default, Clone)]
pub(crate) struct ModeloDeValor {
    valor: u32,
    tamanho: u32,
    /// Cada ouvinte é o endereço do `ModelListener` do jogo, com a função e o contexto que
    /// estavam nele quando foi registrado. Conferir os dois antes de chamar é o que evita chamar
    /// um ouvinte cuja memória o jogo já liberou e reaproveitou.
    ouvintes: Vec<(u32, u32, u32)>,
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
    /// O alfa de cada pixel, **só quando a imagem tem meio-tom**; vazio quando todo pixel é
    /// opaco ou transparente, que é o caso comum. Quem desenha a imagem na tela mistura com o
    /// que está embaixo. A moldura de seleção da Z-Wheel (`214×47`, no `tectoyli.brf`) é um
    /// traço opaco em volta de um miolo azul com alfa 51: sem a mistura, sobrava só o traço.
    alfa: Vec<u8>,
    /// Largura de cada quadro, quando o jogo divide a imagem em tiras (`IPARM_CXFRAME`).
    frame_width: u16,
}

/// Põe os quadros de um GIF lado a lado, numa imagem só.
///
/// É a forma que o resto do emulador já entende: uma imagem com `frame_width` menor que a
/// largura é uma sequência, e o `IIMAGE_DrawFrame` escolhe a coluna. Dar caminho próprio à
/// animação de GIF seria repetir o que o `IPARM_CXFRAME` já faz.
fn tira_de_quadros(gif: &crate::video::gif::Gif) -> DecodedImage {
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
        alfa: Vec::new(),
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

/// Decodifica uma imagem para RGB565, qualquer que seja o formato dela.
///
/// O `IImage` do BREW não é "o objeto de PNG": é uma interface, e o console tem uma classe por
/// formato — `AEECLSID_PNG`, `AEECLSID_BMP`, `AEECLSID_JPEG`. Todas alimentadas do mesmo jeito,
/// com um `IAStream`, e é por isso que olhar a magia dos bytes é melhor que confiar no ClassID:
/// o jogo pode pedir uma classe e entregar outra coisa, e o formato está escrito no arquivo.
///
/// O Action Hero 3D é quem cobrou isto: ele pede a `0x01004001` e os recursos dele são 67 BMPs
/// dentro de um `.res`. Recusando a classe, ele desreferenciava o nulo logo depois — e com ela
/// atendida mas o decodificador só de PNG, a imagem sairia vazia.
fn decodifica_imagem(bytes: &[u8]) -> Option<DecodedImage> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => decode_png(bytes),
        // O BMP e o JPEG passam pelo decodificador que já serve os ícones dos títulos. Ele
        // devolve RGBA, e um BMP não tem canal alfa: todo pixel é opaco, e quem quiser
        // transparência usa a cor reservada da superfície, como o BREW faz.
        _ => {
            let imagem = crate::video::icon::decode(bytes).ok()?;
            let total = imagem.width * imagem.height;
            let mut pixels = Vec::with_capacity(total);
            let mut opaque = Vec::with_capacity(total);
            let mut alfa = Vec::with_capacity(total);
            for pixel in imagem.rgba.chunks_exact(4).take(total) {
                alfa.push(pixel[3]);
                pixels.push(
                    Rgb {
                        r: pixel[0],
                        g: pixel[1],
                        b: pixel[2],
                    }
                    .to_rgb565(),
                );
                opaque.push(pixel[3] >= 128);
            }
            Some(DecodedImage {
                width: imagem.width as u32,
                height: imagem.height as u32,
                pixels,
                opaque,
                alfa: so_com_meio_tom(alfa),
                frame_width: 0,
            })
        }
    }
}

/// Decodifica um PNG para RGB565, devolvendo `None` se não for um PNG que saibamos ler.
/// Decodifica uma imagem **pela assinatura**, e não por um formato só.
///
/// O caminho do PNG continua sendo o primeiro, porque ele tem tratamento próprio: paleta com
/// `tRNS`, profundidades menores que oito bits e alfa de meio-tom, que o `decode_png` abaixo
/// resolve com as transformações do `png` e o resto do motor não sabe repetir.
///
/// **O que faltava era o resto.** O Zuma's Revenge pede o decodificador de **JPEG**, alimenta um
/// JPEG de dezesseis kilobytes e recebia `EFAILED` do `GetBitmap` — o motor só tentava PNG, e a
/// hipótese registrada era "um decodificador recebeu dados que não são um PNG". O despachante por
/// assinatura já existia em [`crate::video::icon::decode`], usado pelos ícones dos módulos; aqui
/// ele passa a servir também ao decodificador do guest.
fn decode_imagem(bytes: &[u8]) -> Option<DecodedImage> {
    if let Some(imagem) = decode_png(bytes) {
        return Some(imagem);
    }
    let imagem = crate::video::icon::decode(bytes).ok()?;
    let count = imagem.pixels();
    let mut pixels = Vec::with_capacity(count);
    let mut opaque = Vec::with_capacity(count);
    let mut alfa = Vec::with_capacity(count);
    for pixel in imagem.rgba.chunks_exact(4) {
        pixels.push(
            Rgb {
                r: pixel[0],
                g: pixel[1],
                b: pixel[2],
            }
            .to_rgb565(),
        );
        let a = pixel[3];
        opaque.push(a == 255);
        alfa.push(a);
    }
    Some(DecodedImage {
        width: imagem.width as u32,
        height: imagem.height as u32,
        pixels,
        opaque,
        alfa,
        // Sem `IPARM_CXFRAME`: quem divide a imagem em tiras é o PNG do decodificador, e o
        // caminho dos outros formatos não recebe esse parâmetro.
        frame_width: 0,
    })
}

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
    let mut alfa = Vec::with_capacity(count);
    for chunk in data.chunks_exact(channels) {
        let (r, g, b) = if channels >= 3 {
            (chunk[0], chunk[1], chunk[2])
        } else {
            (chunk[0], chunk[0], chunk[0])
        };
        pixels.push(Rgb { r, g, b }.to_rgb565());
        // Meio-tom não existe numa superfície sem canal alfa: ou o pixel entra, ou não entra.
        // Numa superfície sem canal alfa, o meio-tom vira opaco ou transparente; é o `alfa`
        // que o preserva para quem desenha a imagem por cima de outra coisa.
        let a = if has_alpha {
            chunk[channels - 1]
        } else {
            u8::MAX
        };
        opaque.push(a >= 128);
        alfa.push(a);
    }

    Some(DecodedImage {
        width: info.width,
        height: info.height,
        pixels,
        opaque,
        alfa: so_com_meio_tom(alfa),
        frame_width: 0,
    })
}

/// Um PNG em bytes de 8 bits por canal: RGB, ou RGBA quando a imagem tem alfa.
///
/// É o formato em que o decodificador de PNG do BREW entrega o `IDIB`. Tons de cinza viram RGB,
/// com ou sem alfa, e a paleta é expandida.
fn decode_png_bytes(bytes: &[u8]) -> Option<(u32, u32, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::normalize_to_color8(),
    );
    let mut reader = decoder.read_info().ok()?;
    let mut raw = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut raw).ok()?;
    let data = &raw[..info.buffer_size()];
    let (canais, saida) = match info.color_type {
        png::ColorType::Rgba => (4, data.to_vec()),
        png::ColorType::Rgb => (3, data.to_vec()),
        png::ColorType::GrayscaleAlpha => (
            4,
            data.chunks_exact(2)
                .flat_map(|c| [c[0], c[0], c[0], c[1]])
                .collect(),
        ),
        _ => (3, data.iter().flat_map(|&c| [c, c, c]).collect()),
    };
    Some((info.width, info.height, canais, saida))
}

/// O alfa de uma imagem, ou nada quando ele não tem meio-tom — aí o `opaque` já diz tudo, e o
/// desenho segue pelo caminho sem mistura.
fn so_com_meio_tom(alfa: Vec<u8>) -> Vec<u8> {
    match alfa.iter().any(|&a| a != 0 && a != u8::MAX) {
        true => alfa,
        false => Vec::new(),
    }
}

/// Mistura `cor` sobre `fundo`, os dois em RGB565, com `alfa` de 0 a 255.
fn mistura_rgb565(fundo: u16, cor: u16, alfa: u8) -> u16 {
    let a = u32::from(alfa);
    let canal = |deslocamento: u32, mascara: u32| {
        let f = (u32::from(fundo) >> deslocamento) & mascara;
        let c = (u32::from(cor) >> deslocamento) & mascara;
        ((c * a + f * (255 - a) + 127) / 255) << deslocamento
    };
    (canal(11, 0x1f) | canal(5, 0x3f) | canal(0, 0x1f)) as u16
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

/// Como um `AEEMediaData` chegou.
pub(super) enum Entrega {
    /// Já lido e guardado, pela chave.
    Pronta(u64),
    /// Um buffer de memória, para ler na volta seguinte do laço.
    Buffer(u32, u32),
    Nada,
}

/// Um som entregue a um `IMedia`, já lido.
///
/// A chave é o **conteúdo**: o cache antigo, por endereço e tamanho, tocava o som anterior quando
/// um novo caía no mesmo lugar. Quando os bytes são lidos está em [`Machine::resolve_midia`].
#[derive(Debug, Clone, Default)]
struct CargaDeMidia {
    som: Option<std::sync::Arc<crate::audio::wav::Sound>>,
    /// A duração de um som que não decodificamos mas sabemos cronometrar (hoje, MP3).
    silencio_us: Option<u64>,
}

/// O que se sabe de um objeto `IMedia`.
#[derive(Debug, Clone, Copy)]
struct MediaState {
    state: u32,
    /// O som entregue pelo `AEEMediaData`, pela chave em [`Machine::cargas_de_midia`]. Zero é
    /// "nada entregue".
    carga: u64,
    /// Um buffer entregue e ainda não lido: `(endereço, tamanho)`. Ver
    /// [`Machine::resolve_midia`].
    pendente: (u32, u32),
    /// O buffer da última entrega por memória, `(endereço, tamanho)`. Todo `Play` o relê: ver
    /// [`Machine::media_play`].
    buffer: (u32, u32),
    /// O jogo mandou tocar antes de o buffer ser lido.
    tocar_ao_ler: bool,
    /// De 0 a [`MAX_VOLUME`].
    volume: u32,
    /// Quantas vezes tocar. Zero é para sempre, que é o que o `MM_PARM_PLAY_REPEAT` define.
    repeat: u32,
    muted: bool,
    /// `PFNMEDIANOTIFY` registrado por `RegisterNotify`.
    notify: Callback,
    /// Quando o som acaba, no relógio virtual. Zero é "não está tocando", e `u64::MAX` é o
    /// `repeat` infinito.
    ends_us: u64,
}

impl Default for MediaState {
    fn default() -> Self {
        Self {
            state: MM_STATE_READY,
            carga: 0,
            pendente: (0, 0),
            buffer: (0, 0),
            tocar_ao_ler: false,
            volume: MAX_VOLUME,
            repeat: 1,
            muted: false,
            notify: Callback {
                function: 0,
                context: 0,
            },
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
    /// Se o buffer veio pelo `Set` e é do stream liberá-lo. Ver [`Machine::stream_call`].
    dono: bool,
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
    md5: crate::brew::crypto::Md5,
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

/// O pedaço de uma `IImage` que o `Draw` desenha, e como.
#[derive(Debug, Clone, Copy, Default)]
struct RecorteDeImagem {
    /// `IPARM_OFFSET`: o canto do pedaço, dentro da imagem (ou do quadro).
    x: i32,
    y: i32,
    /// `IPARM_SIZE`: o tamanho do pedaço. Sem ele, até a borda da imagem.
    tamanho: Option<(i32, i32)>,
    /// `IPARM_ROP`: com `AEE_RO_TRANSPARENT`, a cor reservada não é desenhada.
    transparente: bool,
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
fn font_do_modulo(raiz: &std::path::Path) -> Option<crate::video::font::Font> {
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
    crate::video::font::Font::load(std::fs::read(caminho).ok()?, nome)
}

/// O banco de amostras do MIDI, procurado na pasta do aparelho.
///
/// Devolve `None` em silêncio quando não há banco: é o caso comum, e o motor continua com a tabela
/// de timbres. O relatório é que diz, pela hipótese em uso, qual dos dois caminhos tocou.
#[cfg(feature = "soundfont")]
fn banco_do_aparelho(aparelho: &std::path::Path) -> Option<std::sync::Arc<crate::audio::soundfont::Banco>> {
    let caminho = crate::audio::soundfont::primeiro_banco(aparelho)?;
    crate::audio::soundfont::abre(&caminho)
}

/// A fonte do aparelho, para quem não trouxe a sua.
///
/// Sem ela, todo `DrawText` de um jogo sem `.ttf` ia só para o relatório: o menu do Kingdom
/// Hearts desenhava as cinco caixas e nenhuma das palavras dentro delas.
fn fonte_do_console(aparelho: &std::path::Path) -> Option<crate::video::font::Font> {
    // **O aparelho do frontend primeiro.** A versão anterior só olhava o caminho do desktop, então
    // a fonte que o core instala no perfil dele — `<raiz do aparelho>/shared/fonts/tectoy.ttf` —
    // nunca era encontrada: o jogo seguia sem desenhar texto, e o quadro ficava em branco quando a
    // tela é fundo branco mais as palavras.
    let do_aparelho = aparelho.join("shared").join("fonts").join("tectoy.ttf");
    let caminho = match do_aparelho.is_file() {
        true => do_aparelho,
        false => crate::loader::archive::fonte_do_sistema()?,
    };
    crate::video::font::Font::load(std::fs::read(caminho).ok()?, "tectoy.ttf".into())
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
    /// Nome do objeto de buffer ligado em `GL_ARRAY_BUFFER` **quando o ponteiro foi dado**, ou
    /// zero se não havia nenhum.
    ///
    /// A especificação é explícita nisto, e a diferença é observável: o que decide de onde o
    /// vetor vem é a ligação no instante do `glVertexPointer`, não a do `glDrawElements`. Um
    /// jogo que sobe três malhas liga cada buffer, dá os ponteiros dela e só depois desenha —
    /// se lêssemos a ligação corrente no desenho, as três sairiam do último buffer.
    buffer: u32,
}

impl ArrayPointer {
    /// Se o vetor está ligado e aponta para algum lugar.
    ///
    /// **Endereço zero só é "nenhum vetor" sem buffer ligado.** Com um buffer, o ponteiro é um
    /// deslocamento dentro dele, e zero é o deslocamento mais comum: o motor QX do SDK dá
    /// `glVertexPointer(3, GL_FIXED, 0, 0)` para toda malha. Conferir só o endereço descartava
    /// todo desenho por buffer com os vértices no começo — o cenário e os personagens do Dragon
    /// Vs Chicken sumiam inteiros.
    fn em_uso(&self) -> bool {
        self.enabled && (self.address != 0 || self.buffer != 0)
    }
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
    /// O que o jogo perguntou ao `IFileMgr`, com o caminho e o retorno.
    fs_log: std::collections::VecDeque<String>,
    /// Quantas vezes o jogo pediu cada classe, conhecida ou não.
    classes_pedidas: BTreeMap<u32, u32>,
    /// ClassIDs que o jogo pediu e não sabemos criar — a lista do que falta.
    unknown_classes: BTreeSet<u32>,
    /// ClassIDs que o `--sonda` manda atender com um objeto de observação.
    /// As URLs que o jogo pediu pelo `IWeb`, para o relatório.
    web_requests: BTreeSet<String>,
    /// As coleções vivas, cada uma com os itens e onde o cursor está.
    collections: HashMap<u32, (Vec<u32>, usize)>,
    /// O texto que o jogo mandou desenhar, com o instante e a posição, para o relatório.
    ///
    /// É o que responde "o que está escrito na tela agora?": a lista de texto *pendente* só existe
    /// quando falta a fonte, e um jogo que desenha várias telas de aviso ao longo da execução
    /// aparece no relatório como uma sopa de mensagens sem dono.
    textos_desenhados: std::collections::VecDeque<(u32, i32, i32, String)>,
    /// Alocações que o heap recusou, por tamanho pedido e por quem pediu.
    ///
    /// O `malloc` do BREW devolve zero quando não cabe, e um jogo que não distingue "não cabe" de
    /// qualquer outra falha mostra uma mensagem genérica — foi assim que o Double Dragon foi parar
    /// na tela "Memory is insufficient" sem nada no relatório que apontasse para o pedido recusado.
    alocacoes_recusadas: BTreeSet<(u32, u32)>,
    /// Perguntas de `IHeap::CheckAvail` respondidas com "não cabe", pelo mesmo motivo.
    checagens_recusadas: BTreeSet<(u32, u32)>,
    /// As fontes do sistema vivas, por objeto `IFont`, com as métricas da classe que as criou.
    fontes: std::collections::HashMap<u32, crate::machine::font::Metricas>,
    /// Os bancos SQLite abertos, por objeto `ISQLDatabase`.
    databases: HashMap<u32, crate::brew::sql::Database>,
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
    /// O objeto do `IGLES11Ext`, criado na primeira vez que o pedem. Ver o slot 2 da vtable do EGL.
    gles11_ext: u32,
    /// O objeto do `IGLES10Ext`, pelo mesmo motivo.
    gles10_ext: u32,
    /// O objeto do `IEGLGetPowerLevel`.
    egl_get_power_level: u32,
    /// O objeto do `IEGLOESSwapInterval`.
    egl_oes_swap_interval: u32,
    /// O objeto do `IEGLGetColorBuffer`.
    egl_get_color_buffer: u32,
    /// O objeto do `IGLES11ExtPak`.
    gles11_ext_pak: u32,
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
    /// Sinais que o jogo registrou para eventos de entrada, por tipo de evento **e porta**.
    ///
    /// A porta faz parte da chave porque o registro é feito no objeto do aparelho, um por
    /// controle ligado: com dois, o segundo registro sobrescrevia o primeiro e todo evento
    /// acordava o callback do controle dois. O jogo então perguntava ao aparelho errado, não
    /// achava evento nenhum e o controle um não fazia nada — era o Treino Cerebral preso no
    /// "aperte botão 1" com as duas portas ligadas.
    input_signals: BTreeMap<(&'static str, usize), u32>,
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
    portas: [Option<crate::input::bindings::Aparelho>; input::PORTAS],
    /// A porta de cada `IHIDDevice` que o jogo criou, pelo endereço do objeto.
    portas_de_aparelho: HashMap<u32, usize>,
    /// A aceleração de cada porta, em g, no referencial do Boomerang. Só as portas com Boomerang
    /// a usam.
    movimento: [[f32; 3]; input::PORTAS],
    /// O contador de pacotes do receptor do Boomerang. Ver [`Machine::pacote_do_boomerang`].
    boomerang_sequencia: u8,
    ultimo_relatorio_boomerang_us: u64,
    ultimo_pacote_boomerang_us: u64,
    /// Quantas vezes o jogo começou e terminou uma calibração do movimento, pelo que dá para
    /// ver de fora. Ver [`Machine::calibracao`].
    calibracoes: (u32, u32),
    /// Teclas apertadas e ainda não entregues, como `(código AVK, apertada)`.
    teclas: std::collections::VecDeque<(u32, bool)>,
    /// Os últimos toques entregues, para o relatório.
    ///
    /// A fila acima é consumida pelo jogo e some; esta fica. Existe porque um problema de
    /// entrada é indistinguível de um problema de interpretação sem ver o que chegou: um menu
    /// que anda duas casas por toque pode ser o jogo contando dois canais, ou o emulador
    /// mandando dois eventos, e só o registro separa os dois casos.
    pad_log: std::collections::VecDeque<(u32, usize, usize, bool)>,
    /// O que o jogo fez com o som, na ordem: `(instante, objeto, chamada)`.
    ///
    /// Existe pelo mesmo motivo do [`Machine::pad_log`]: "não sai som na partida" não aparece em
    /// relatório nenhum quando toda chamada responde sucesso. Chamadas iguais seguidas viram
    /// uma linha com a contagem, senão um `GetState` por quadro empurraria todo o resto para
    /// fora.
    media_log: std::collections::VecDeque<(u32, u32, String, u32)>,
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
    /// Onde cada linha está em `debug_output`, para agrupar sem varrer a lista a cada chamada.
    debug_indice: HashMap<String, usize>,
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
    /// O id do módulo de cada applet instalado — o nome do `.mif` dele, sem extensão. É o que
    /// o `ISHELL_EnumNextApplet` entrega no `pszMIF`.
    modulos_instalados: Vec<(u32, String)>,
    /// Onde o `EnumNextApplet` está na lista, e as cadeias de `pszMIF` já postas no guest.
    enumeracao_de_applets: usize,
    mif_no_guest: HashMap<u32, u32>,
    /// Orçamento de instruções do trecho em execução.
    ///
    /// Guardado porque o despacho de uma chamada de API não o recebe, e há um caso em que ele
    /// precisa **reentrar no guest**: a classe fornecida por um módulo de extensão, que só o
    /// `IModule::CreateInstance` da extensão sabe criar. Ver [`Machine::cria_pela_extensao`].
    orcamento: u64,
    /// `IModule*` de cada extensão. `None` = ainda não carregada; `Some(0)` = tentamos e não
    /// deu, e não se tenta de novo a cada pedido.
    ext_modules: Vec<Option<u32>>,
    pending_launch: Option<u32>,
    /// As escritas da tela logo depois do último quadro de GL. Ver [`Machine::quadro_na_placa`].
    escritas_do_quadro_gl: Option<u64>,
    wheel_boot_skipped: bool,
    /// Como a Z-Wheel lê a `tectoy.cfg`. Ver [`Machine::configura_z_wheel`].
    z_wheel: crate::config::ZWheel,
    /// O applet pediu para fechar com `ISHELL_CloseApplet`. Ver [`Machine::pediu_para_fechar`].
    applet_fechado: bool,
    /// `(raiz, formulário)` à espera do aviso de ativo. Ver [`Machine::entrega_ativacao`].
    ativacao_pendente: Option<(u32, u32)>,
    /// As telas apresentadas por `IDISPLAY_Update` dentro de uma mesma volta do laço. Ver
    /// [`Machine::toma_quadro_do_update`].
    quadros_do_update: std::collections::VecDeque<Framebuffer>,
    updates_na_volta: usize,
    /// Profundidade atual de reentrada no guest.
    nesting: u32,
    /// O trecho de guest que o teto de instruções interrompeu, quando ele era o trecho mais de
    /// fora: onde continuar e os registradores de então. Ver [`Machine::retoma_trecho`].
    trecho_interrompido: Option<(u32, [u32; 15])>,
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
    /// O `AEEImageInfo` que o `PFNIMAGEINFO` de cada imagem recebe, no heap do jogo.
    image_info: HashMap<u32, u32>,
    /// O retângulo e a operação que o `IIMAGE_SetParm` deixou para os próximos `Draw`.
    recortes_de_imagem: HashMap<u32, RecorteDeImagem>,
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
    /// A página entregue a cada widget de HTML, crua como veio do `ISource`. Ver o
    /// `AdicionarFilho` do [`Machine::widget_call`].
    paginas_html: HashMap<u32, Vec<u8>>,
    /// Quantas linhas cada widget de HTML rolou, e até quantas pode rolar (medido ao pintar).
    rolagem_html: HashMap<u32, usize>,
    rolagem_maxima_html: HashMap<u32, usize>,
    /// Setas cujo aperto rolou um painel de HTML: a soltura delas também não vai ao jogo.
    teclas_da_rolagem: std::collections::HashSet<u32>,
    /// O estado de cada `IPeek` vivo.
    peeks: HashMap<u32, Peek>,
    /// Os itens de cada `IConfig` vivo, por objeto: número do item -> bytes.
    config_items: HashMap<u32, HashMap<u32, Vec<u8>>>,
    /// O estado de cada widget vivo. Ver [`Widget`].
    widgets: HashMap<u32, Widget>,
    /// Quantas vezes cada par `(classe de widget, seletor)` passou pelo acessador. Ver o braço
    /// `Acessador` de `widget_call`: é o que responde "que comportamento esta classe proprietária
    /// espera" pelo uso, sem sonda e sem desmonte. Ligado por [`Machine::liga_censo_de_widgets`].
    seletores_por_classe: std::collections::BTreeMap<(u32, u32), u32>,
    /// Se o censo do acessador por classe está ligado. Desligado, para não mexer na linha de base.
    censo_de_widgets: bool,
    /// As APIs que faltaram, com quem as chamou. Ver o `None` do despacho.
    missing_apis: BTreeSet<String>,
    /// Acessos inválidos que aconteceram dentro de retorno de chamada e não pararam o jogo.
    falhas_engolidas: BTreeSet<String>,
    /// Chamadas de GL atendidas com sucesso sem fazer nada.
    ignored_gl: BTreeSet<&'static str>,
    /// `glPixelStorei(GL_UNPACK_ALIGNMENT, n)`: com quantos bytes cada linha de textura começa
    /// alinhada na memória do guest. O padrão do OpenGL é 4.
    ///
    /// Mora no `Machine`, e não no estado do rasterizador, porque é propriedade **da memória do
    /// jogo** — quem lê os texels é `machine/gl.rs`, antes de entregá-los a qualquer rasterizador.
    unpack_alignment: u32,
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
    resources: crate::loader::resfile::ResCache,
    unzips: HashMap<u32, UnzipState>,
    /// Callbacks do guest já disparados e ainda não entregues.
    ///
    /// Fila única porque todos têm a mesma forma — um endereço de função e até quatro
    /// argumentos — e porque nenhum deles pode rodar no meio do despacho de uma chamada.
    pending_calls: Vec<GuestCall>,
    /// Os avisos do `IMedia` ainda não entregues — objeto, comando, status e o tratador de quando
    /// o aviso nasceu. Saem na volta do laço, não na saída da chamada: ver
    /// [`Machine::notify_media`].
    avisos_de_midia: Vec<(u32, u32, u32, Callback)>,
    /// Os `IMedia` que tocam PCM gerado pelo jogo, por objeto. Ver [`FluxoPcm`].
    fluxos_pcm: HashMap<u32, FluxoPcm>,
    /// O buffer no guest onde o `ISource::Read` escreve as amostras.
    buffer_de_fluxo: u32,
    /// O bloco onde cada `AEEMediaCmdNotify` é montado na hora da entrega. Um só basta: os avisos
    /// saem um de cada vez, e o tratador só o lê enquanto roda.
    bloco_de_aviso_de_midia: u32,
    /// Recursos que **algum** arquivo forneceu. Ver [`Machine::missing_files`].
    recursos_lidos: BTreeSet<u16>,
    /// Se a árvore de widgets já foi despejada na serial.
    despejou: bool,
    /// Contador de criação de widgets. Ver [`Machine::formulario_atual`].
    proximo_serial: u64,
    /// O formulário que está pintado na superfície agora. Ver [`Machine::pinta_widgets`].
    formulario_pintado: u32,
    /// Quando os widgets foram desenhados pela última vez, no relógio virtual.
    ///
    /// Quem desenha um `OwnerDrawWidget` é o jogo, e na Z-Wheel esse retorno é um compositor em
    /// software — o laço em `0x1fc70`, que sozinho responde por 63% de todas as instruções do
    /// guest. O console manda redesenhar quando a tela precisa; nós mandávamos a cada volta do
    /// laço de eventos, que são ~128 por segundo virtual para ~67 quadros apresentados.
    ultimo_desenho_us: u64,
    /// Captura de serial, quando ligada. Ver [`Machine::liga_serial`].
    serial: Option<std::io::BufWriter<std::fs::File>>,
    /// Último erro do EGL, devolvido por `eglGetError`.
    egl_error: u32,
    /// Superfícies do EGL vivas, com as dimensões de cada uma.
    egl_surfaces: HashMap<u32, (u32, u32)>,
    /// Se a viewport inicial já foi posta no tamanho do pbuffer. Ver o `eglMakeCurrent`.
    egl_viewport_inicial: bool,
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
    /// Os sons entregues aos `IMedia`, já lidos, pela chave do conteúdo. Ver
    /// [`CargaDeMidia`].
    cargas_de_midia: HashMap<u64, CargaDeMidia>,
    /// Para onde o som vai, quando há para onde.
    audio: Option<crate::audio::Mixer>,
    /// O último quadro que o jogo apresentou, já no tamanho da tela.
    ///
    /// Guardado no `eglSwapBuffers` porque é ali que o quadro está pronto: ler o buffer no fim
    /// da execução pega o desenho pela metade, quase sempre logo depois do `Clear`.
    gl_last_frame: Vec<u8>,
    /// Estado e buffers do OpenGL ES.
    ///
    /// Despacho dinâmico porque o rasterizador é trocável: a fronteira inteira está no
    /// [`Rasterizador`], e só este módulo a toca. A indireção por chamada é ruído perto do que
    /// cada uma faz — a Z-Wheel emite dezenove mil chamadas de GL em treze segundos virtuais.
    gl: Box<dyn Rasterizador>,
    /// Vetores do cliente: posição, cor e coordenada de textura.
    gl_vertices: ArrayPointer,
    gl_colors: ArrayPointer,
    gl_texcoords: ArrayPointer,
    /// O vetor de coordenadas da unidade de textura 1.
    gl_texcoords1: ArrayPointer,
    /// O vetor de normais do `glNormalPointer`. Sempre três componentes — a função nem recebe
    /// tamanho.
    gl_normals: ArrayPointer,
    /// Conteúdo de cada objeto de buffer vivo, pelo nome que o `glGenBuffers` entregou.
    ///
    /// Fica no host, e não na memória do jogo, porque é onde um driver de verdade o guarda: o
    /// jogo só enxerga o buffer pelo nome, e depois do `glBufferData` ele tem o direito de
    /// reaproveitar o ponteiro que passou. Guardar cópia nossa é o que faz esse direito valer.
    gl_buffers: HashMap<u32, Vec<u8>>,
    /// Nome ligado em `GL_ARRAY_BUFFER`, ou zero para "os ponteiros são da memória do jogo".
    gl_array_buffer: u32,
    /// Nome ligado em `GL_ELEMENT_ARRAY_BUFFER`. Aqui a ligação corrente **é** a que vale: o
    /// `glDrawElements` decide no momento do desenho se o último argumento é ponteiro ou
    /// deslocamento dentro do buffer.
    gl_element_buffer: u32,
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
    /// Os `IDIB` cujo buffer ainda guarda os pixels **do objeto anterior** naquele endereço.
    ///
    /// O buffer é reaproveitado quando um bitmap novo nasce onde outro morreu, mas os bytes que
    /// estão nele são do morto. Enquanto o endereço estiver aqui, o que vale é a superfície do
    /// host, e importar o buffer seria trazer a imagem velha por cima da nova. Sai daqui quando
    /// o host publica os próprios pixels.
    ///
    /// No Unicorn isso passava despercebido porque a vigia de escrita dizia "o jogo não mexeu" e
    /// a importação não acontecia. O Dynarmic não tem vigia e responde sempre "sujo", que só é
    /// seguro se o buffer nunca estiver atrás do host — e aqui estava: as letras do Tekken 2 e
    /// do Kingdom Hearts viravam blocos, com a folha de glifos substituída pela imagem
    /// decodificada antes dela.
    dib_herdados: HashSet<u32>,
    /// Bitmaps do decodificador de PNG cujo `IDIB` mostra os pixels no formato do próprio PNG
    /// (RGB de 24 bits ou RGBA de 32), e não em RGB565: o buffer e a capacidade dele. Ficam fora
    /// da sincronização — a nossa cópia em RGB565 serve aos blits, e o buffer é só leitura para o
    /// jogo.
    dib_do_decodificador: HashMap<u32, (u32, u32)>,
    /// A [`Framebuffer::serie`] de cada superfície na última vez que o buffer do jogo e a nossa
    /// cópia ficaram iguais — dali em diante, só a caixa suja dela precisa ir para o jogo.
    ///
    /// Sem isto, **toda** chamada que desenha reescrevia todas as superfícies expostas inteiras
    /// na memória do jogo, mudadas ou não. O Pac-Mania faz 168 mil `IIMAGE_Draw` em cinco
    /// segundos virtuais: 95% do tempo de API — 30 segundos de relógio — era essa cópia.
    dib_publicado: HashMap<u32, u64>,
    /// A região de superfícies, com o mesmo alocador do heap do jogo. Ver
    /// [`Machine::reserva_superficie`].
    superficies: Heap,
    /// Widgets cujo tratador está recebendo um aviso agora. Ver [`Machine::avisa_widget`].
    widgets_avisando: std::collections::HashSet<u32>,
    /// Cor tratada como transparente em cada superfície.
    transparency: HashMap<u32, u16>,
    /// O bitmap de destino de cada `ITransform` que o jogo pediu por `QueryInterface`.
    transformacoes: HashMap<u32, u32>,
    /// O estado de cada `IValueModel` (`0x01028e3c`).
    modelos_de_valor: HashMap<u32, ModeloDeValor>,
    /// O modo gravado no controle de sistema (`0x01006c02`) pelo slot 3.
    modo_do_sistema: u32,
    /// O bitmap de cada canvas pedido por `QueryInterface`.
    canvases: HashMap<u32, u32>,
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
    font: Option<crate::video::font::Font>,
    /// O banco de amostras com que o MIDI é tocado, quando o aparelho tem um.
    ///
    /// `None` é o caso comum: o banco não vem embutido (medido: 32.319.396 B e +64 MiB de RSS na
    /// carga), então quem não instalou um `.sf2` continua ouvindo a tabela de timbres.
    #[cfg(feature = "soundfont")]
    banco_de_som: Option<std::sync::Arc<crate::audio::soundfont::Banco>>,
    /// Quantas vezes cada método foi chamado — o retrato do que o jogo usa.
    calls: BTreeMap<(u32, u32), u64>,
    /// Total de chamadas atendidas, para aplicar o teto.
    calls_total: u64,
}

/// Se o rasterizador na placa foi pedido.
///
/// `padrao` é o que a configuração diz. A variável de ambiente tem a palavra final porque é por
/// ela que a medição escolhe o backend na linha de comando, onde não há tela de configuração.
///
/// Valor vazio conta como desligado: um `ZEEBX_GPU=` não deve ligar a placa sem que se tenha
/// pedido — foi assim que uma bateria de quatro medições saiu inteira na placa quando metade
/// devia ser em software.
fn placa_pedida(padrao: bool) -> bool {
    match std::env::var("ZEEBX_GPU") {
        Ok(valor) => !valor.is_empty() && valor != "0",
        Err(_) => padrao,
    }
}

/// O rasterizador que a construção adota. Quem tem configuração troca depois, com `usa_placa`.
///
/// Aqui não há contexto para emprestar: quem constrói a máquina direto é a linha de comando, que
/// não tem janela. O caminho com janela troca depois, já com o contexto dela.
fn rasterizador(largura: usize, altura: usize) -> Box<dyn Rasterizador> {
    match placa_pedida(false) {
        true => na_placa(largura, altura, None),
        false => Box::new(GlState::new(largura, altura)),
    }
}

/// A placa quando ela abre, o software quando não.
///
/// A queda **não é tratamento de erro**, é um caminho normal: num terminal sem EGL alcançável não
/// há placa para usar, e o emulador tem que rodar de todo jeito. O motivo é dito uma vez, porque
/// um emulador que silenciosamente roda diferente do pedido é pior que um lento.
fn na_placa(
    largura: usize,
    altura: usize,
    contexto: Option<std::sync::Arc<glow::Context>>,
) -> Box<dyn Rasterizador> {
    // Com a feature `gl`, o rasterizador de placa existe — e ele **não** cria contexto: quem
    // chama entrega o dele. Sem ela, o software é a única rota, que é o caso do core quando o
    // frontend não oferece contexto.
    #[cfg(feature = "gl")]
    {
        match crate::video::gpu::GpuState::novo(largura, altura, contexto) {
            Ok(gpu) => return Box::new(gpu),
            Err(motivo) => {
                eprintln!("sem rasterizador na placa ({motivo}); seguindo em software");
            }
        }
    }
    let _ = contexto;
    Box::new(GlState::new(largura, altura))
}

impl<C: CpuBackend> Machine<C> {
    /// `root` é o diretório do módulo — a raiz do sistema de arquivos que o jogo enxerga.
    /// Troca o rasterizador, **antes de o jogo começar**.
    ///
    /// A sessão chama isto logo depois de construir a máquina, com o que a configuração pede.
    /// Trocar depois de o jogo desenhar perderia o estado de GL acumulado — matrizes, texturas,
    /// luz —, então este é o único momento em que a troca é segura.
    /// `contexto` é o da janela, quando há uma. Ver [`crate::video::gpu::GpuState::novo`].
    pub fn usa_placa(
        &mut self,
        sim: bool,
        contexto: Option<std::sync::Arc<glow::Context>>,
    ) {
        let (largura, altura) = self.gl.frame_size();
        self.gl = match placa_pedida(sim) {
            true => na_placa(largura, altura, contexto),
            false => Box::new(GlState::new(largura, altura)),
        };
    }

    /// Começa a contar os `IDISPLAY_Update` de uma volta do laço.
    pub fn comeca_volta(&mut self) {
        self.updates_na_volta = 0;
    }

    /// Fecha a volta. Com um `Update` só, a tela final já basta e nada fica guardado.
    pub fn fecha_volta(&mut self) {
        if self.updates_na_volta <= 1 {
            self.quadros_do_update.clear();
        }
    }

    /// Se há telas intermediárias de uma volta anterior à espera de serem mostradas.
    pub fn tem_quadros_do_update(&self) -> bool {
        !self.quadros_do_update.is_empty()
    }

    /// A próxima tela intermediária, na ordem em que foi apresentada.
    ///
    /// **Uma animação que roda inteira dentro de um callback só aparece assim.** A transição
    /// da Z-Wheel (`0x74258`) desliza a tela num laço síncrono: copia um trecho, chama
    /// `IDISPLAY_Update`, anda cinco pixels, e repete umas duzentas vezes antes de devolver. No
    /// console cada `Update` vai para a tela; aqui a janela só via o fim do callback, e a
    /// transição virava um corte seco.
    pub fn toma_quadro_do_update(&mut self) -> Option<Framebuffer> {
        self.quadros_do_update.pop_front()
    }

    /// Guarda a tela no `IDISPLAY_Update`, quando o destino é a tela.
    pub(super) fn guarda_quadro_do_update(&mut self) -> Result<(), CpuError> {
        /// Teto de telas guardadas: cada uma são 600 KB. Passando dele, fica uma a cada duas.
        const TETO: usize = 96;
        let alvo = self.target()?;
        if alvo != self.device_bitmap {
            return Ok(());
        }
        self.updates_na_volta += 1;
        if self.quadros_do_update.len() >= TETO {
            let mut indice = 0;
            self.quadros_do_update.retain(|_| {
                indice += 1;
                indice % 2 == 0
            });
        }
        let tela = self.screen();
        let mut copia = Framebuffer::new(tela.width(), tela.height());
        copia.load_rgb565_bytes(&tela.to_rgb565_bytes());
        self.quadros_do_update.push_back(copia);
        Ok(())
    }

    /// Se o applet pediu para fechar com `ISHELL_CloseApplet`.
    pub fn pediu_para_fechar(&self) -> bool {
        self.applet_fechado
    }

    /// Entrega o `EVT_APP_STOP` ao applet, que é a última coisa que ele recebe antes de sair.
    ///
    /// É nele que muitos jogos gravam o progresso. O que ele responde não muda nada: fechar foi
    /// pedido pelo próprio applet.
    pub fn encerra_applet(&mut self) -> Result<(), CpuError> {
        self.send_applet_event(self.applet_class, EVT_APP_STOP, 0, 0)?;
        Ok(())
    }

    /// Escolhe como a Z-Wheel lê a `tectoy.cfg`, **antes de o applet ser criado**.
    ///
    /// Fora do padrão de fábrica, ela recebe uma cópia da cfg no perfil do aparelho com valores
    /// trocados; o pacote não é tocado.
    ///
    /// - **Fim de vida.** A de fábrica traz `EOL=1` e `zeebomenu_hide=1`. A leitura em `0x7fe80`
    ///   liga com eles os bits `0x2000` e `0x4000` de `app+0x3614`, e a montagem da roda inferior
    ///   em `0x4f620` fica com "Jogar" e "Ajuda". Com os dois em zero ela volta a ser a de antes:
    ///   "Jogar", o logo zeebo, "Comprar" e "Configurar".
    /// - **Transições.** A `0x798c4` decide se a troca de tela desliza: com o tipo da tela no
    ///   `SlideOnceToForm`, só se ele ainda não estiver no `HasSlidToForm` das preferências, que
    ///   ela marca na primeira vez; fora dele, sempre, a não ser que esteja no `NoSlideToForm`.
    ///   A cfg traz `SlideOnceToForm=31` e o dump já vem com `HasSlidToForm=14` — Jogar,
    ///   Configurar e zeebo vistos —, então nada deslizava. Com `SlideOnceToForm=0`, tudo desliza.
    pub fn configura_z_wheel(&mut self, opcoes: crate::config::ZWheel) {
        self.z_wheel = opcoes;
    }

    pub fn new(cpu: C, module: LoadedModule, root: impl Into<std::path::PathBuf>) -> Self {
        let storage = crate::storage::StoragePaths::from_root(crate::config::config_dir());
        Self::new_with_storage(cpu, module, root, &storage, None)
    }

    /// Como [`Machine::new`], mas recebe as raízes persistentes do frontend.
    ///
    /// O construtor antigo preserva a UI desktop. Frontends isolados, como Libretro, não podem
    /// depender de `ui::settings::config_dir()` e usam esta forma com sua raiz autorizada: a NAND
    /// compartilhada e, quando o perfil pede, o overlay gravável do título.
    pub fn new_with_storage(
        cpu: C,
        module: LoadedModule,
        root: impl Into<std::path::PathBuf>,
        storage: &crate::storage::StoragePaths,
        save_root: Option<std::path::PathBuf>,
    ) -> Self {
        let raiz: std::path::PathBuf = root.into();
        let aparelho: std::path::PathBuf = storage.device.clone();
        let heap = Heap::new(loader::HEAP_BASE, loader::HEAP_SIZE);
        // Os objetos ficam depois dos ponteiros que o carregador já reservou no começo da
        // região, para não sobrescrevê-los.
        let reserved = module.objects_reserved();
        let extensoes = module.extensions.len();
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
            fs_log: std::collections::VecDeque::new(),
            classes_pedidas: BTreeMap::new(),
            unknown_classes: BTreeSet::new(),
            web_requests: BTreeSet::new(),
            collections: HashMap::new(),
            textos_desenhados: std::collections::VecDeque::new(),
            alocacoes_recusadas: BTreeSet::new(),
            checagens_recusadas: BTreeSet::new(),
            fontes: std::collections::HashMap::new(),
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
                vfs.set_device_root(aparelho.clone());
                if let Some(save) = save_root {
                    vfs.set_save_root(save);
                }
                vfs
            },
            open_files: HashMap::new(),
            file_error: SUCCESS,
            decoders: HashMap::new(),
            feeds: HashMap::new(),
            surface_manip: 0,
            imageon_ext: 0,
            gles11_ext: 0,
            gles10_ext: 0,
            egl_get_power_level: 0,
            egl_oes_swap_interval: 0,
            egl_get_color_buffer: 0,
            gles11_ext_pak: 0,
            scale_source: None,
            prefs: HashMap::new(),
            enumerations: HashMap::new(),
            missing_files: BTreeSet::new(),
            input_signals: BTreeMap::new(),
            signals: HashMap::new(),
            pending_signals: Vec::new(),
            pads: [Pad::default(); input::PORTAS],
            pad_events: std::array::from_fn(|_| std::collections::VecDeque::new()),
            movimento: [[0.0, 0.0, 1.0]; input::PORTAS],
            boomerang_sequencia: 0,
            ultimo_relatorio_boomerang_us: 0,
            ultimo_pacote_boomerang_us: 0,
            calibracoes: (0, 0),
            // Uma porta com controle é o que sempre houve; a interface muda isto ao aplicar os
            // ajustes, e o modo sem janela nunca mexe.
            portas: std::array::from_fn(|n| {
                (n == 0).then_some(crate::input::bindings::Aparelho::Controle)
            }),
            portas_de_aparelho: HashMap::new(),
            media_log: std::collections::VecDeque::new(),
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
            debug_indice: HashMap::new(),
            bitmaps: HashMap::new(),
            fault_regs: [0; 12],
            fault_stack: Vec::new(),
            timers: Vec::new(),
            applet_class: 0,
            installed_applets: HashSet::new(),
            modulos_instalados: Vec::new(),
            enumeracao_de_applets: 0,
            mif_no_guest: HashMap::new(),
            orcamento: 0,
            ext_modules: vec![None; extensoes],
            pending_launch: None,
            wheel_boot_skipped: false,
            escritas_do_quadro_gl: None,
            z_wheel: crate::config::ZWheel {
                fim_de_vida: true,
                transicoes_sempre: false,
            },
            applet_fechado: false,
            ativacao_pendente: None,
            quadros_do_update: Default::default(),
            updates_na_volta: 0,
            nesting: 0,
            trecho_interrompido: None,
            pending_probes: Vec::new(),
            pending_blits: Vec::new(),
            probed: HashSet::new(),
            ciphers: HashMap::new(),
            hashes: HashMap::new(),
            resources: crate::loader::resfile::ResCache::default(),
            unzips: HashMap::new(),
            images: HashMap::new(),
            image_bitmaps: HashMap::new(),
            api_time: HashMap::new(),
            profiling_api: false,
            image_notify: HashMap::new(),
            image_info: HashMap::new(),
            recortes_de_imagem: HashMap::new(),
            parametros_de_colecao: HashMap::new(),
            vetores: HashMap::new(),
            sources: HashMap::new(),
            paginas_html: HashMap::new(),
            rolagem_html: HashMap::new(),
            rolagem_maxima_html: HashMap::new(),
            teclas_da_rolagem: Default::default(),
            peeks: HashMap::new(),
            widgets: HashMap::new(),
            seletores_por_classe: std::collections::BTreeMap::new(),
            censo_de_widgets: false,
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
            unpack_alignment: 4,
            web_response: Vec::new(),
            streams: HashMap::new(),
            sounds: HashMap::new(),
            pending_calls: Vec::new(),
            avisos_de_midia: Vec::new(),
            fluxos_pcm: HashMap::new(),
            buffer_de_fluxo: 0,
            bloco_de_aviso_de_midia: 0,
            recursos_lidos: BTreeSet::new(),
            despejou: false,
            proximo_serial: 0,
            formulario_pintado: 0,
            ultimo_desenho_us: 0,
            serial: None,
            egl_error: gles::EGL_SUCCESS,
            egl_surfaces: HashMap::new(),
            egl_viewport_inicial: false,
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
            cargas_de_midia: HashMap::new(),
            audio: None,
            gl_last_frame: Vec::new(),
            gl: rasterizador(SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize),
            gl_vertices: ArrayPointer::default(),
            gl_colors: ArrayPointer::default(),
            gl_texcoords: ArrayPointer::default(),
            gl_texcoords1: ArrayPointer::default(),
            gl_normals: ArrayPointer::default(),
            gl_buffers: HashMap::new(),
            gl_array_buffer: 0,
            gl_element_buffer: 0,
            gl_normal_atual: [0.0, 0.0, 1.0],
            interned: HashMap::new(),
            threads: HashMap::new(),
            resume_callbacks: HashMap::new(),
            pending_threads: Vec::new(),
            stalled: None,
            current_thread: None,
            dib_buffers: HashMap::new(),
            dib_capacity: HashMap::new(),
            dib_herdados: HashSet::new(),
            dib_do_decodificador: HashMap::new(),
            dib_publicado: HashMap::new(),
            superficies: Heap::new(loader::SURFACE_BASE, loader::SURFACE_SIZE),
            widgets_avisando: std::collections::HashSet::new(),
            transparency: HashMap::new(),
            transformacoes: HashMap::new(),
            modelos_de_valor: HashMap::new(),
            modo_do_sistema: 0,
            canvases: HashMap::new(),
            device_bitmap: 0,
            display_target: 0,
            clip: None,
            screen: Framebuffer::new(SCREEN_WIDTH as u32, SCREEN_HEIGHT as u32),
            colors: default_colors(),
            pending_text: Vec::new(),
            // A fonte do aparelho sai da raiz que este motor recebeu, não da configuração do
            // desktop: é isso que faz a fonte instalada pelo frontend ser encontrada.
            font: font_do_modulo(&raiz).or_else(|| fonte_do_console(&aparelho)),
            // O banco de amostras do MIDI, quando o aparelho tem um. É opcional de propósito: o
            // banco não vem embutido, e sem ele a música volta para a tabela de timbres.
            #[cfg(feature = "soundfont")]
            banco_de_som: banco_do_aparelho(&aparelho),
            calls: BTreeMap::new(),
            calls_total: 0,
        }
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

    /// Chama o guest de **dentro** do despacho de uma chamada de API.
    ///
    /// O [`Self::call_guest`] normal não serve aqui: ele escreve em `r0..r3` e no `lr`, e quem
    /// está despachando vai ler o `lr` depois para saber onde retomar o jogo. Perdê-lo faz a
    /// execução voltar para o lugar errado — em geral para o endereço zero.
    ///
    /// Por isso o desvio de regra vem com a regra: **todo o contexto é salvo e devolvido**. A
    /// pilha não precisa de cuidado, porque a chamada aninhada empilha abaixo do `sp` corrente,
    /// que é espaço que ninguém está usando — é a mesma garantia que uma interrupção tem.
    ///
    /// É o que o console faz: o `ISHELL_CreateInstance` de uma classe de extensão entra no
    /// módulo da extensão e volta com o objeto, sem que o jogo perceba.
    fn call_guest_aninhado(
        &mut self,
        func: u32,
        args: [u32; 4],
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        const CONTEXTO: [Reg; 15] = [
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
        let salvo = CONTEXTO.map(|reg| self.cpu.read_reg(reg));
        let desfecho = self.call_guest(func, args, budget);
        for (reg, valor) in CONTEXTO.into_iter().zip(salvo) {
            self.cpu.write_reg(reg, valor);
        }
        desfecho
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
        self.orcamento = budget;
        loop {
            let gasto = self.cpu.instructions().saturating_sub(comeco);
            let fatia = teto.saturating_sub(gasto).min(budget);
            if fatia == 0 {
                self.anota_trecho_interrompido(pc);
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
                StopReason::Budget => {
                    // O bit 0 diz o modo, como em toda retomada do ARM.
                    let parou = self.cpu.read_reg(Reg::Pc) & !1;
                    let modo = u32::from(self.cpu.em_thumb());
                    self.anota_trecho_interrompido(parou | modo);
                    return Ok(Outcome::Budget);
                }
            }
        }
    }

    /// Guarda onde continuar o trecho que o teto de instruções interrompeu.
    ///
    /// **Só o trecho mais de fora.** O guest guarda o estado dele nos registradores e na pilha
    /// dele, então retomar é continuar do `pc` — mas um trecho aninhado (um callback chamado de
    /// dentro do despacho de uma API) tem quem o espera do lado de cá, e esse quadro já se foi
    /// quando a volta termina. Aninhado, o teto continua sendo só um pedido de vez.
    fn anota_trecho_interrompido(&mut self, pc: u32) {
        if self.nesting != 0 {
            return;
        }
        // **Os registradores vão junto.** Entre o corte e a retomada o emulador ainda entrega
        // sinais e callbacks desta volta, e entrar no guest para isso sobrescreve `r0`-`r3`,
        // o `lr` e o que mais o tratador usar. Sem guardar o contexto, a retomada continuava
        // com os registradores de outra coisa — e o Rolima saltava para o endereço zero.
        let mut estado = [0u32; 15];
        for (slot, reg) in estado.iter_mut().zip(THREAD_REGS) {
            *slot = self.cpu.read_reg(reg);
        }
        estado[14] = self.cpu.read_reg(Reg::Lr);
        self.trecho_interrompido = Some((pc, estado));
    }

    /// Continua o trecho interrompido, se houver um.
    ///
    /// **Um jogo pode rodar o laço inteiro dele dentro do `EVT_APP_START`.** O Zeebo Extreme
    /// Rolima faz isso: ele nunca cede a vez com `IThread::Suspend`, como os irmãos dele fazem,
    /// e o teto de instruções cortava o carregamento no meio. Sem retomada, o trecho sumia — o
    /// laço de eventos não achava timer nem callback nenhum, e a sessão terminava sozinha aos
    /// 3,8 segundos, como se o jogo tivesse acabado.
    fn retoma_trecho(&mut self, budget: u64) -> Result<Option<Outcome>, CpuError> {
        let Some((pc, estado)) = self.trecho_interrompido.take() else {
            return Ok(None);
        };
        for (valor, reg) in estado.iter().zip(THREAD_REGS) {
            self.cpu.write_reg(reg, *valor);
        }
        self.cpu.write_reg(Reg::Lr, estado[14]);
        self.execute(pc, budget).map(Some)
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
            // `int ISHELL_CloseApplet(IShell *, boolean bReturnToIdle)`. É como um jogo sai pelo
            // próprio menu. O BREW não fecha dentro da chamada: agenda, e o applet recebe o
            // `EVT_APP_STOP` depois de voltar. Aqui o pedido fica anotado, e a sessão encerra na
            // volta do laço (ver [`Machine::encerra_applet`]).
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("CloseApplet") => {
                self.applet_fechado = true;
                SUCCESS
            }
            (Interface::Shell, 4) => self.shell_get_device_info()?,
            (Interface::Shell, slot)
                if matches!(
                    Interface::Shell.method(slot),
                    Some("StartApplet" | "CanStartApplet")
                ) =>
            {
                let cls = self.cpu.read_reg(Reg::R1);
                let instalado = self.installed_applets.contains(&cls);
                match Interface::Shell.method(slot) {
                    // `boolean ISHELL_CanStartApplet(IShell *, AEECLSID)`: **verdadeiro é poder**.
                    // Respondia `SUCCESS`, que é zero — "não pode" para todo jogo instalado. A
                    // Z-Wheel pergunta isto em `0x7e58c` antes de lançar e, com zero, desiste
                    // e registra "Failure attempting to launch game".
                    Some("CanStartApplet") => u32::from(instalado),
                    _ if instalado => {
                        self.pending_launch = Some(cls);
                        SUCCESS
                    }
                    _ => ECLASSNOTSUPPORT,
                }
            }
            // `int ISHELL_EnumAppletInit(IShell *)` e `boolean ISHELL_EnumNextApplet(IShell *,
            // AEEAppInfo *)`. A Z-Wheel percorre a lista antes de lançar um jogo (`0x794c8`):
            // acha a classe escolhida, lê o `pszMIF`, corta até a última barra e tira a
            // extensão — o que ela quer é o **id do módulo**, com que monta o diretório do jogo.
            //
            // O `AEEAppInfo` é `{ AEECLSID cls; char *pszMIF; uint16 wIDBase; uint16 wAppType;
            // uint32 dwFlags; }`.
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("EnumAppletInit") => {
                self.enumeracao_de_applets = 0;
                SUCCESS
            }
            (Interface::Shell, slot) if Interface::Shell.method(slot) == Some("EnumNextApplet") => {
                let saida = self.cpu.read_reg(Reg::R1);
                match self
                    .modulos_instalados
                    .get(self.enumeracao_de_applets)
                    .cloned()
                {
                    Some((classe, id)) if saida != 0 => {
                        self.enumeracao_de_applets += 1;
                        let mif = match self.mif_no_guest.get(&classe) {
                            Some(&endereco) => endereco,
                            None => {
                                let texto = format!("fs:/mif/{id}.mif");
                                let endereco = self.heap.alloc(texto.len() as u32 + 1).unwrap_or(0);
                                if endereco != 0 {
                                    self.write_cstring_limited(endereco, &texto, texto.len() + 1)?;
                                    self.mif_no_guest.insert(classe, endereco);
                                }
                                endereco
                            }
                        };
                        self.cpu.write_mem(saida, &[0u8; 16])?;
                        self.cpu.write_u32(saida, classe)?;
                        self.cpu.write_u32(saida + 4, mif)?;
                        1
                    }
                    _ => 0,
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
            (Interface::Graphics, _)
            | (Interface::Display, _)
            | (Interface::Bitmap, _)
            | (Interface::Transform, _)
            | (Interface::Canvas, _) => {
                let whole = iface.method(slot).is_none_or(touches_whole_surface);
                if whole {
                    self.sync_surfaces_in()?;
                }
                let handled = match iface {
                    Interface::Graphics => self.graphics_call(slot)?,
                    Interface::Display => self.display_call(slot)?,
                    Interface::Transform => self.transform_call(slot)?,
                    Interface::Canvas => self.canvas_call(slot)?,
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
            (Interface::EglSurfaceManip, _)
            | (Interface::GlesImageonExt, _)
            | (Interface::Gles11Ext, _)
            | (Interface::Gles10Ext, _)
            | (Interface::EglGetPowerLevel, _)
            | (Interface::EglOesSwapInterval, _)
            | (Interface::EglGetColorBuffer, _)
            | (Interface::Gles11ExtPak, _)
            | (Interface::Joystick, _) => {
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
            (Interface::Classe28e3c, _) => match self.modelo_de_valor_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
            (Interface::Font, _) => match self.font_call(slot)? {
                Some(result) => result,
                None => return Ok(None),
            },
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
        // O trecho que o teto cortou continua antes de qualquer outra coisa: ele é o jogo no
        // meio de um quadro, e timer ou callback entregues por cima dele chegariam fora de hora.
        if let Some(outcome) = self.retoma_trecho(budget)? {
            return Ok(vec![outcome]);
        }
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

    /// Cria um objeto novo e grava o ponteiro de vtable dele na memória do guest.
    fn new_object(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let Some(addr) = self.objects.create(iface) else {
            return Ok(0);
        };
        if self.dib_buffers.contains_key(&addr) {
            self.dib_herdados.insert(addr);
        }
        // A cor transparente é do bitmap que morreu aqui, não do objeto que nasce.
        self.transparency.remove(&addr);
        self.cpu.write_u32(addr, loader::vtable_addr(iface))?;
        Ok(addr)
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

    pub fn cpu_mut(&mut self) -> &mut C {
        &mut self.cpu
    }

    pub fn cpu(&self) -> &C {
        &self.cpu
    }

    pub fn module(&self) -> &LoadedModule {
        &self.module
    }
}

#[cfg(test)]
mod tests;
