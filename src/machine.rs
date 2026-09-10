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
/// `RGB_NONE`: pedido de "não pinte esta parte".
const RGB_NONE: u32 = 0xffff_ffff;
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
    "StencilFunc",
    "StencilMask",
    "StencilOp",
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
    /// Largura e altura, do slot 7. A Z-Wheel manda `640 × 480` — a tela inteira.
    tamanho: (u32, u32),
    /// Os filhos que entraram pelo slot 5, que não os identifica por número.
    anexados: Vec<u32>,
    /// Se o widget deve aparecer. O slot 6 é quem diz.
    visivel: bool,
    /// Quem o pendurou, do slot 5. Ver o `PegarPai`.
    pai: u32,
    /// Endereço da estrutura de tratador que o slot 4 registrou: `{função, contexto}`.
    tratador: u32,
    /// Se o aviso de partida já foi entregue. Ver [`Machine::parte_animacao`].
    partiu: bool,
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
const FAMILIA_DOS_WIDGETS: [u32; 5] = [
    AEECLSID_WIDGET,
    0x0102_8e19,
    0x0102_8e2a,
    0x0102_8e3f,
    0x0102_8e47,
];

/// `0x01035156`, a fonte TrueType do console. Ver [`Interface::Typeface`].
const AEECLSID_TYPEFACE: u32 = 0x0103_5156;

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
    let clip = clip?;
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
    /// Último erro do EGL, devolvido por `eglGetError`.
    egl_error: u32,
    /// Superfícies do EGL vivas, com as dimensões de cada uma.
    egl_surfaces: HashMap<u32, (u32, u32)>,
    /// Próximo identificador livre de superfície ou contexto.
    egl_next_handle: u32,
    /// Quantas vezes o jogo apresentou um quadro com `eglSwapBuffers`.
    egl_swaps: u32,
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
            egl_error: gles::EGL_SUCCESS,
            egl_surfaces: HashMap::new(),
            egl_next_handle: EGL_HANDLE_BASE,
            egl_swaps: 0,
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
            interned: HashMap::new(),
            threads: HashMap::new(),
            resume_callbacks: HashMap::new(),
            pending_threads: Vec::new(),
            stalled: None,
            current_thread: None,
            dib_buffers: HashMap::new(),
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

    /// `qsort` da stdlib do BREW, com a comparação feita pelo jogo.
    ///
    /// Ordena **na memória do guest**, trocando os elementos de lugar de verdade, para que os
    /// ponteiros que a função de comparação recebe sejam os endereços reais dentro do vetor —
    /// que é o que o `qsort` do C faz e o que um comparador pode observar.
    ///
    /// O algoritmo é o heapsort: ordena no lugar, sem memória extra, e faz `n log n`
    /// comparações. Como cada comparação custa uma entrada no guest, o número delas é o que
    /// importa aqui — uma ordenação por inserção seria simples mas quadrática, e um vetor
    /// grande custaria caro.
    fn qsort(&mut self, base: u32, count: u32, size: u32, compare: u32) -> Result<(), CpuError> {
        if base == 0 || compare == 0 || size == 0 || count < 2 {
            return Ok(());
        }
        // Reentrar no guest exige espaço de aninhamento, como nos callbacks.
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("um qsort foi ignorado por aninhamento profundo demais");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let result = self.heapsort(base, count as usize, size, compare);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        result
    }

    fn heapsort(
        &mut self,
        base: u32,
        count: usize,
        size: u32,
        compare: u32,
    ) -> Result<(), CpuError> {
        for start in (0..count / 2).rev() {
            self.sift_down(base, size, compare, start, count)?;
        }
        for end in (1..count).rev() {
            self.swap_elements(base, size, 0, end)?;
            self.sift_down(base, size, compare, 0, end)?;
        }
        Ok(())
    }

    /// Empurra o elemento em `root` para baixo até o monte voltar a valer.
    fn sift_down(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        mut root: usize,
        end: usize,
    ) -> Result<(), CpuError> {
        loop {
            let child = root * 2 + 1;
            if child >= end {
                return Ok(());
            }
            let mut largest = child;
            if child + 1 < end && self.compare_elements(base, size, compare, child, child + 1)? < 0
            {
                largest = child + 1;
            }
            if self.compare_elements(base, size, compare, root, largest)? >= 0 {
                return Ok(());
            }
            self.swap_elements(base, size, root, largest)?;
            root = largest;
        }
    }

    /// Chama a função de comparação do jogo com os endereços dos dois elementos.
    fn compare_elements(
        &mut self,
        base: u32,
        size: u32,
        compare: u32,
        a: usize,
        b: usize,
    ) -> Result<i32, CpuError> {
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let outcome = self.call_guest(compare, [pa, pb, 0, 0], QSORT_BUDGET)?;
        match outcome {
            // Uma comparação que não retorna deixa a ordem como está, em vez de derrubar tudo.
            Outcome::Returned { code } => Ok(code as i32),
            _ => Ok(0),
        }
    }

    fn swap_elements(&mut self, base: u32, size: u32, a: usize, b: usize) -> Result<(), CpuError> {
        if a == b {
            return Ok(());
        }
        let (pa, pb) = (base + a as u32 * size, base + b as u32 * size);
        let first = self.read_bytes(pa, size)?;
        let second = self.read_bytes(pb, size)?;
        self.cpu.write_mem(pa, &second)?;
        self.cpu.write_mem(pb, &first)?;
        Ok(())
    }

    /// Roda os callbacks pendentes na fronteira entre duas chamadas de API."""
    ///
    /// O BREW entrega notificações pelo laço de eventos dele, quando o applet já devolveu o
    /// controle. Nosso equivalente mais próximo é este ponto: a chamada de API terminou, o
    /// resultado já está em `r0` e o guest ainda não retomou. Todo o contexto é salvo e
    /// devolvido, de forma que o chamador não perceba o desvio.
    fn run_pending_callbacks(&mut self, budget: u64) -> Result<(), CpuError> {
        self.poll_media()?;
        let idle = self.pending_calls.is_empty()
            && self.pending_probes.is_empty()
            && self.pending_blits.is_empty();
        if idle || self.nesting >= MAX_NESTING {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        for _ in 0..CALLBACK_ROUNDS {
            for target in std::mem::take(&mut self.pending_probes) {
                self.probe_foreign_surface(target, budget)?;
            }
            for blit in std::mem::take(&mut self.pending_blits) {
                self.blit_into_foreign(blit, budget)?;
            }
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function != 0 {
                    self.call_guest(call.function, call.args, budget)?;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Desenha uma imagem numa superfície do jogo chamando o `BltIn` dela.
    ///
    /// É a mesma coreografia do BREW: montamos um `IBitmap` nosso com a imagem decodificada e
    /// passamos como origem. O `BltIn` do jogo então pede `QueryInterface(AEECLSID_DIB)` no
    /// nosso bitmap e lê os pixels pelos campos públicos, que é justamente o que o `IDIB`
    /// existe para oferecer. Conferido na desmontagem: o `BltIn` do Bejeweled Twist faz esse
    /// `QueryInterface` com `0x01001045` e depois lê `cx`, `cy` e `nColorScheme`.
    fn blit_into_foreign(&mut self, blit: PendingBlit, budget: u64) -> Result<(), CpuError> {
        let Some(info) = self.images.get(&blit.image).cloned() else {
            return Ok(());
        };
        let (frame_width, offset) = match (blit.frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };

        // A origem é uma superfície nossa, criada só para esta chamada.
        let source = self.new_object(Interface::Bitmap)?;
        if source == 0 {
            return Ok(());
        }
        let mut surface = Framebuffer::new(frame_width, info.height);
        for row in 0..info.height {
            for column in 0..frame_width {
                let index = (row * info.width + column + offset) as usize;
                let opaque = info.opaque.get(index).copied().unwrap_or(true);
                let pixel = match (opaque, info.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    // Sem canal alfa no destino, o transparente vira uma cor reservada — é
                    // como o BREW resolve, e o `BltIn` respeita a `ncTransparent` do `IDIB`.
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(source, surface);
        self.transparency.insert(source, TRANSPARENT_KEY);
        self.expose_dib(source)?;

        let vtable = self.cpu.read_u32(blit.target)?;
        let blt_in = self.cpu.read_u32(vtable + BITMAP_BLT_IN_SLOT * 4)?;
        // BltIn(po, xDst, yDst, dx, dy, pSrc, xSrc, ySrc, rop)
        let outcome = self.call_guest_with_stack(
            blt_in,
            [blit.target, blit.x as u32, blit.y as u32, frame_width],
            &[info.height, source, 0, 0, AEE_RO_TRANSPARENT],
            budget,
        )?;
        if !matches!(outcome, Outcome::Returned { code: 0 }) {
            self.assumptions
                .insert("o BltIn de uma superfície do jogo recusou o desenho");
        }

        self.objects.release(source);
        self.bitmaps.remove(&source);
        self.dib_buffers.remove(&source);
        self.transparency.remove(&source);
        Ok(())
    }

    /// Descobre onde ficam os pixels de um `IBitmap` implementado pelo próprio jogo.
    ///
    /// É o mesmo caminho que o BREW usa: `QueryInterface(AEECLSID_DIB)` no objeto, e o `IDIB`
    /// que volta traz `pBmp`, `cx`, `cy` e `nPitch` como campos públicos. Com isso a superfície
    /// do jogo entra no mesmo mecanismo de sincronização das nossas — desenhamos no host e o
    /// resultado é copiado para a memória dele.
    fn probe_foreign_surface(&mut self, target: u32, budget: u64) -> Result<(), CpuError> {
        let Ok(vtable) = self.cpu.read_u32(target) else {
            return Ok(());
        };
        let Ok(query) = self.cpu.read_u32(vtable + BITMAP_QUERY_INTERFACE_SLOT * 4) else {
            return Ok(());
        };
        // O ponteiro de saída precisa viver na memória do guest.
        let Some(out) = self.heap.alloc(4) else {
            return Ok(());
        };
        // O `IDIB` tem dois IIDs: o atual e o que o BREW 2.0 usava. Um bitmap escrito para a
        // plataforma antiga só reconhece o segundo.
        let mut dib = 0;
        let mut outcome = Outcome::Returned { code: EFAILED };
        for iid in [AEECLSID_DIB, AEEIID_DIB_20] {
            self.cpu.write_u32(out, 0)?;
            outcome = self.call_guest(query, [target, iid, out, 0], budget)?;
            dib = self.cpu.read_u32(out).unwrap_or(0);
            if matches!(outcome, Outcome::Returned { code: 0 }) && dib != 0 {
                break;
            }
        }
        self.heap.free(out);

        let failed = !matches!(outcome, Outcome::Returned { code: 0 });
        if failed || dib == 0 {
            // O Bejeweled Twist é assim: o `QueryInterface` da superfície dele é literalmente
            // `mov r0, #0x14; bx lr` — devolve `ECLASSNOTSUPPORT` sempre. Nessa superfície o
            // único método de desenho implementado de verdade é o `BltIn`.
            self.assumptions
                .insert("uma superfície do jogo não expõe IDIB; o desenho nela ainda se perde");
            return Ok(());
        }

        let buffer = self.cpu.read_u32(dib + 8)?;
        let mut fields = [0u8; 10];
        self.cpu.read_mem(dib + 20, &mut fields)?;
        let cx = u16::from_le_bytes([fields[0], fields[1]]) as u32;
        let cy = u16::from_le_bytes([fields[2], fields[3]]) as u32;
        let pitch = i16::from_le_bytes([fields[4], fields[5]]) as i32;
        let depth = fields[8];

        // Só sabemos tratar o formato da tela do console: RGB565, linhas contíguas e para
        // baixo. Qualquer outra coisa é melhor recusar do que desenhar torto.
        if cx == 0
            || cy == 0
            || buffer == 0
            || depth != COLOR_DEPTH as u8
            || pitch != (cx * 2) as i32
        {
            self.assumptions
                .insert("uma superfície do jogo usa um formato que ainda não sabemos desenhar");
            return Ok(());
        }

        self.bitmaps.insert(target, Framebuffer::new(cx, cy));
        self.dib_buffers.insert(target, buffer);
        self.sync_from_guest(target)?;
        Ok(())
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

    /// Os três métodos de timer do `IShell`, todos com a assinatura
    /// `(IShell *, [int32 dwMsecs,] PFNNOTIFY pfn, void *pUser)`.
    ///
    /// `PFNNOTIFY` é `void (*)(void *pUser)`; o par `(pfn, pUser)` identifica o timer, e é por
    /// ele que `CancelTimer` e `GetTimerExpiration` o encontram.
    fn shell_timer_call(&mut self, name: &str) -> Result<u32, CpuError> {
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        Ok(match name {
            "SetTimer" => {
                let callback = Callback {
                    function: a2,
                    context: a3,
                };
                if callback.function == 0 {
                    return Ok(EBADPARM);
                }
                // Rearmar o mesmo par `(pfn, pUser)` substitui o timer anterior, em vez de
                // acumular dois — é o que o BREW faz, e o que o jogo espera ao rearmar dentro
                // do próprio callback.
                self.timers.retain(|timer| timer.callback != callback);
                self.timers.push(Timer {
                    deadline_ms: self.now_ms().saturating_add(a1),
                    callback,
                });
                SUCCESS
            }
            "CancelTimer" => {
                let (function, context) = (a1, a2);
                // `pfn` nulo cancela todos os timers daquele contexto.
                self.timers.retain(|timer| {
                    timer.callback.context != context
                        || (function != 0 && timer.callback.function != function)
                });
                SUCCESS
            }
            // Devolve quanto falta, em milissegundos; zero se não há timer armado.
            _ => {
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.timers
                    .iter()
                    .find(|timer| timer.callback == callback)
                    .map(|timer| timer.deadline_ms.saturating_sub(self.now_ms()))
                    .unwrap_or(0)
            }
        })
    }

    /// `int ISHELL_DetectType(IShell *, const void *cpBuf, uint32 *pdwSize,
    /// const char *cpszName, const char **pcpszMIME)`.
    ///
    /// Descobre o tipo MIME de um conteúdo. O jogo usa a resposta para achar, no registro do
    /// BREW, qual classe sabe abrir aquele arquivo.
    fn shell_detect_type(&mut self) -> Result<u32, CpuError> {
        let (buffer, size_ptr, name_ptr, mime_ptr) =
            (self.arg(1), self.arg(2), self.arg(3), self.arg(4));

        // Sem dados e sem nome, a pergunta é "de quantos bytes você precisa?".
        if buffer == 0 && name_ptr == 0 {
            self.write_at(size_ptr, DETECT_TYPE_BYTES)?;
            return Ok(ENEEDMORE);
        }

        let available = if size_ptr == 0 {
            0
        } else {
            self.cpu.read_u32(size_ptr)?
        };
        let bytes = if buffer == 0 {
            Vec::new()
        } else {
            self.read_bytes(buffer, available.min(DETECT_TYPE_BYTES))?
        };
        let name = self.cpu.read_cstring(name_ptr, MAX_STRING);

        match detect_mime(&bytes, &name) {
            Some(mime) => {
                let text = self.intern(mime)?;
                self.write_at(mime_ptr, text)?;
                Ok(SUCCESS)
            }
            None => Ok(ENOTYPE),
        }
    }

    /// `int ISHELL_Resume(IShell *, AEECallback *pcb)` — agenda o callback para a próxima
    /// volta do laço de eventos.
    ///
    /// É o mecanismo em que as threads cooperativas se apoiam: o jogo pede a retomada por
    /// aqui e só então chama `Suspend`, para que a thread tenha como voltar.
    fn shell_resume(&mut self) -> Result<u32, CpuError> {
        let pcb = self.cpu.read_reg(Reg::R1);
        if let Some(&thread) = self.resume_callbacks.get(&pcb) {
            if !self.pending_threads.contains(&thread) {
                self.pending_threads.push(thread);
            }
            return Ok(SUCCESS);
        }
        let call = self.resolve_notify(Callback {
            function: pcb,
            context: pcb,
        })?;
        self.queue_call(call);
        Ok(SUCCESS)
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

    /// Reconhece a espera ocupada do jogo e faz o tempo passar por ela.
    ///
    /// O Zeebo Sports Peteca não arma timer para o próximo quadro: ele fica num laço que lê o
    /// relógio e cede a vez até o prazo chegar — 69 mil leituras por quadro, 14 milhões de
    /// instruções gastas só em esperar. No console isso não custa nada, porque o tempo passa
    /// sozinho enquanto o ARM gira; aqui cada volta é emulada de verdade, e o jogo rodava seis
    /// vezes mais devagar que o aparelho.
    ///
    /// Quando o jogo só lê o relógio e cede a vez, ele não está progredindo — está esperando, e
    /// adiantar o relógio é exatamente o que o console faz com a passagem do tempo real.
    /// Qualquer outra chamada é sinal de trabalho e zera a contagem, então um quadro que lê o
    /// relógio no meio do que faz continua custando o que custa.
    fn note_spin(&mut self, iface: Interface, slot: u32) {
        let name = iface.method(slot).unwrap_or("");
        let reads_clock = iface == Interface::Helpers
            && matches!(name, "aee_GetTimeMS" | "aee_GetUpTimeMS" | "aee_GetSeconds");
        // Ceder a vez não é trabalho nem espera: é como o laço dá a volta. Não conta para o
        // limite, mas também não desfaz a contagem.
        let yields = matches!(
            (iface, name),
            (Interface::Thread, "Suspend" | "GetResumeCBK") | (Interface::Shell, "Resume")
        );
        if !reads_clock {
            if !yields {
                self.spin_polls = 0;
            }
            return;
        }
        self.spin_polls += 1;
        if self.spin_polls > SPIN_THRESHOLD {
            self.clock_us += SPIN_STEP_US;
        }
    }

    /// Adianta o relógio até o próximo timer, quando não há mais nada a fazer agora.
    ///
    /// É o que o console faz: sem trabalho pendente ele dorme, e acorda no vencimento. Pular
    /// esse trecho é o que mantém o tempo virtual honesto — o jogo vê exatamente o intervalo
    /// que pediu entre dois quadros, e não o que o emulador levou para chegar lá.
    fn skip_idle_time(&mut self) {
        let busy = !self.pending_calls.is_empty()
            || !self.pending_threads.is_empty()
            || !self.pending_probes.is_empty()
            || !self.pending_blits.is_empty()
            || !self.pending_signals.is_empty();
        if busy {
            return;
        }
        let now = self.now_ms();
        let Some(next) = self.timers.iter().map(|timer| timer.deadline_ms).min() else {
            return;
        };
        // O salto é de **um quadro**, não do vão inteiro. Pular até o timer mais próximo é
        // certo quando o jogo espera o quadro seguinte, e foi para isso que este atalho
        // nasceu; mas quando o único timer armado é longo, o vão não é ociosidade — é o jogo
        // esperando alguém apertar um botão. A Z-Wheel arma 120 000 ms de inatividade na tela
        // de boas-vindas: saltar o vão inteiro fazia o relógio ir a dois minutos de uma vez e
        // disparar o tempo de ocioso antes que qualquer tecla tivesse chance de chegar.
        let vao = next.saturating_sub(now) as u64 * 1000;
        self.clock_us += vao.min(VSYNC_PERIOD_US);
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

    /// Relógio virtual do guest, em milissegundos.
    pub fn clock_ms(&self) -> u32 {
        self.now_ms()
    }

    /// O relógio que o guest enxerga: o tempo adiantado pelo laço de quadros mais o tempo que
    /// o próprio guest gastou executando instruções.
    ///
    /// As duas parcelas medem coisas diferentes e por isso somam. A primeira é o tempo ocioso
    /// que o emulador pula entre quadros — no console ele passaria de verdade. A segunda faz o
    /// relógio andar *durante* uma fatia de execução, e sem ela um laço de espera do jogo
    /// ("fique aqui até passarem 1,5 s") nunca terminaria: ele lê o relógio, não avança nada e
    /// lê de novo.
    ///
    /// O tempo de execução vem da contagem de instruções, não do relógio do host: assim duas
    /// execuções da mesma ROM dão exatamente o mesmo resultado, o que é o que torna possível
    /// comparar dois quadros e saber que a diferença foi a mudança que fizemos.
    fn now_ms(&self) -> u32 {
        (self.now_us() / 1000) as u32
    }

    /// O mesmo relógio, em microssegundos — a resolução em que ele é mantido.
    ///
    /// Milissegundos não bastam: um quadro a 60 Hz dura 16,67 ms, e arredondar isso a cada
    /// quadro acumularia um erro de vários por cento.
    fn now_us(&self) -> u64 {
        self.clock_us + self.cpu.instructions() / INSTRUCTIONS_PER_US
    }

    /// Espera o retraço vertical, como o `eglSwapBuffers` do console faz.
    ///
    /// Se o quadro demorou mais que um período, o próximo retraço é o primeiro que ainda não
    /// passou — perder um retraço custa o quadro inteiro, e é assim no aparelho também.
    fn wait_for_vsync(&mut self) {
        let now = self.now_us();
        if self.next_vsync_us <= now {
            let missed = (now - self.next_vsync_us) / VSYNC_PERIOD_US + 1;
            self.next_vsync_us += missed * VSYNC_PERIOD_US;
        }
        self.clock_us += self.next_vsync_us - now;
        self.next_vsync_us += VSYNC_PERIOD_US;
    }

    /// Descobre a função a chamar por trás de um par `(pfn, pUser)`.
    ///
    /// O `ISHELL_SetTimerEx` do SDK é uma macro sobre o `SetTimer`, e ela passa **o mesmo
    /// ponteiro** nos dois argumentos:
    ///
    /// ```c
    /// #define ISHELL_SetTimerEx(p,s,pcb) \
    ///     GET_PVTBL(p,IShell)->SetTimer(p, s, (PFNNOTIFY)(void *)pcb, (void *)pcb)
    /// ```
    ///
    /// Quando os dois são iguais, o que chegou é um `AEECallback *`, e quem deve ser chamado
    /// está dentro dele: `pfnNotify` no offset 16 e `pNotifyData` no 20 (`inc/AEECallback.h`).
    /// Um `pfn` de verdade nunca coincide com o seu `pUser` — um é código, o outro é dado.
    fn resolve_notify(&mut self, callback: Callback) -> Result<Callback, CpuError> {
        if callback.function == 0 || callback.function != callback.context {
            return Ok(callback);
        }
        let base = callback.function;
        Ok(Callback {
            function: self.cpu.read_u32(base + 16).unwrap_or(0),
            context: self.cpu.read_u32(base + 20).unwrap_or(0),
        })
    }

    /// Se não há mais nada para o guest fazer: nenhum timer armado, nenhum callback na fila e
    /// nenhuma thread esperando a vez.
    ///
    /// O laço de quadros para aqui. Olhar só para os timers não bastava: quando o jogo passa a
    /// viver dentro de uma thread cooperativa — como o Quake faz depois de carregar o mapa —,
    /// ele cancela o timer e o laço encerrava com o jogo no meio.
    pub fn is_idle(&self) -> bool {
        self.timers.is_empty()
            && self.pending_calls.is_empty()
            && self.pending_threads.is_empty()
            && self.pending_probes.is_empty()
            && self.pending_blits.is_empty()
    }

    /// Quantos timers estão armados.
    pub fn armed_timers(&self) -> usize {
        self.timers.len()
    }

    /// `IBase *ISHELL_LoadResObject(IShell *po, const char *pszResFile, uint16 nResID,
    /// AEECLSID cls)`.
    ///
    /// Com `nResID` zero o arquivo inteiro é o recurso — é assim que o Quake carrega
    /// `fs:/~/../id1/splash_title.png`. Com `nResID` diferente de zero o que ele nomeia é um
    /// `.bar`, e a imagem é a entrada daquele número lá dentro.
    ///
    /// Ignorar o `nResID` custou caro: o Tekken 2 pede a entrada 5034 do `tekken2.bar` e nós
    /// tentávamos decodificar os 734 KB do `.bar` inteiro como PNG. Falhava, devolvia nulo, e o
    /// jogo seguia com uma imagem sem tamanho — que é divisão por zero na hora de montar a
    /// tela. O relatório dizia o que estava acontecendo o tempo todo, na linha "um recurso
    /// pedido por LoadResObject não é um PNG que saibamos ler".
    ///
    /// Com `cls` zero, o BREW deduz a classe pelo conteúdo; aqui a única que sabemos produzir é
    /// a imagem, e é só o que os jogos pedem.
    fn shell_load_res_object(&mut self) -> Result<u32, CpuError> {
        let guest_path = self
            .cpu
            .read_cstring(self.cpu.read_reg(Reg::R1), MAX_STRING);
        let id = self.cpu.read_reg(Reg::R2) as u16;
        let cls = self.cpu.read_reg(Reg::R3);
        let Some(path) = self.vfs.resolve(&guest_path) else {
            self.missing_files.insert(guest_path);
            return Ok(0);
        };
        let bytes = match id {
            0 => match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) => {
                    self.missing_files.insert(guest_path);
                    return Ok(0);
                }
            },
            _ => {
                let raw = self
                    .resources
                    .open(&path)
                    .and_then(|res| res.get(crate::resfile::RESTYPE_IMAGE, id))
                    .map(<[u8]>::to_vec);
                let Some(raw) = raw else {
                    self.missing_files
                        .insert(format!("{guest_path} (recurso {id})"));
                    return Ok(0);
                };
                // O cabeçalho `AEEResBlob` é nosso para pular: quem pediu foi um **objeto** de
                // imagem, não o bloco bruto que o `LoadResData` entrega.
                crate::resfile::blob_data(&raw).unwrap_or(&raw).to_vec()
            }
        };
        let Some(decoded) = self.decode_resource_image(&bytes) else {
            self.assumptions
                .insert("um recurso pedido por LoadResObject veio num formato que não sabemos ler");
            return Ok(0);
        };

        // A classe pedida decide o que sai daqui. O Tekken 2 pede `AEEIID_IBITMAP` — ele quer
        // desenhar com `IDISPLAY_BitBlt`, não com `IIMAGE_Draw` —, e devolver um `IImage` fazia
        // ele chamar um método de `IBitmap` numa vtable de `IImage`: o slot 12, que em `IImage`
        // não existe.
        if cls == AEEIID_IBITMAP {
            return self.bitmap_from_decoded(&decoded);
        }
        let image = self.new_object(Interface::Image)?;
        if image != 0 {
            self.images.insert(image, std::rc::Rc::new(decoded));
        }
        Ok(image)
    }

    /// Decodifica uma imagem de recurso pelo que ela é.
    ///
    /// O PNG passa pelo caminho próprio, que traz o canal alfa que os jogos usam para recortar
    /// o sprite. O resto — BMP e JPEG — vem pelo decodificador dos ícones, que já sabe lê-los e
    /// entrega tudo opaco, que é o que esses dois formatos são.
    fn decode_resource_image(&mut self, bytes: &[u8]) -> Option<DecodedImage> {
        if let Some(decoded) = decode_png(bytes) {
            return Some(decoded);
        }
        if let Some(gif) = crate::gif::decodifica(bytes) {
            return Some(tira_de_quadros(&gif));
        }
        let image = crate::icon::decode(bytes).ok()?;
        let count = image.width * image.height;
        let mut pixels = Vec::with_capacity(count);
        for at in (0..count * 4).step_by(4) {
            pixels.push(
                Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                }
                .to_rgb565(),
            );
        }
        Some(DecodedImage {
            width: image.width as u32,
            height: image.height as u32,
            pixels,
            opaque: vec![true; count],
            frame_width: 0,
        })
    }

    /// Abre o arquivo de recursos que o jogo nomeou.
    ///
    /// O nome pode vir nulo, e o Peggle manda nulo: o BREW entende isso como "o arquivo de
    /// recursos deste applet". Como não há convenção de nome que sirva — o Peggle chama o dele
    /// de `resources.bar` e o Pac-Mania de `pacmania.bar` —, o que resta é o único `.bar` que
    /// existe ao lado do módulo. Havendo mais de um, não há como escolher, e ninguém abre.
    fn open_res_file(&mut self, pointer: u32) -> Option<&crate::resfile::ResFile> {
        let path = match pointer {
            0 => self.default_res_file()?,
            _ => {
                let guest_path = self.cpu.read_cstring(pointer, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) => path,
                    None => {
                        self.missing_files.insert(guest_path);
                        return None;
                    }
                }
            }
        };
        self.resources.open(&path)
    }

    /// O único `.bar` ao lado do módulo, se houver exatamente um.
    fn default_res_file(&self) -> Option<std::path::PathBuf> {
        let mut found = std::fs::read_dir(self.vfs.root())
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("bar"));
        let first = found.next()?;
        match found.next() {
            None => Some(first),
            Some(_) => None,
        }
    }

    /// Escolhe um idioma quando o banco de preferências ainda não tem um.
    ///
    /// A Z-Wheel guarda em `PREFSINFO.Lang` a **etiqueta** do idioma empacotada em quatro
    /// bytes — `"pt  "` vira `0x20207470` —, e não um índice. Quem monta a tela do z-pad
    /// percorre a lista de idiomas do módulo comparando a etiqueta de cada um com esse valor;
    /// sem achar, devolve zero e o jogo repete `Couldn't create z-pad instruction form (6)`
    /// para sempre. O pacote nasce com `Lang = 0`, que é "ninguém escolheu ainda": no console
    /// quem preenche isso é a tela de primeira configuração, que ainda não alcançamos.
    ///
    /// Escolher por conta é uma hipótese, e fica anotada como tal. É o português porque é o
    /// idioma do aparelho que a TecToy vendeu, e porque `tectoy_pt.brf` está no pacote.
    fn escolhe_idioma(&mut self, db: &crate::sql::Database) {
        /// `"pt  "` lido como uma palavra de 32 bits, que é a forma como o módulo compara.
        const PORTUGUES: u32 = u32::from_le_bytes(*b"pt  ");
        let sem_escolha = db
            .exec("SELECT dwValue FROM PREFSINFO WHERE PREFSINFO.name = 'Lang'")
            .ok()
            .and_then(|linhas| linhas.into_iter().next())
            .and_then(|linha| linha.values.into_iter().next().flatten())
            .is_some_and(|valor| valor == "0");
        if !sem_escolha {
            return;
        }
        let gravou = db.exec(&format!(
            "UPDATE PREFSINFO SET dwValue = {PORTUGUES} WHERE PREFSINFO.name = 'Lang'"
        ));
        if gravou.is_ok() {
            self.assumptions.insert(
                "o idioma não estava escolhido no banco e assumimos português",
            );
        }
    }

    /// `int ISHELL_LoadResString(IShell *po, const char *pszResFile, int16 nResID,
    /// AECHAR *pBuff, int nSize)`.
    ///
    /// Devolve **quantos caracteres** foram escritos, e zero quando o recurso não existe — que
    /// é resposta legítima, não erro.
    fn shell_load_res_string(&mut self) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let (buffer, capacity) = (self.cpu.read_reg(Reg::R3), self.stack_arg(0)?);
        let Some(text) = self.open_res_file(file).and_then(|res| res.string(id)) else {
            return Ok(0);
        };
        if buffer == 0 || capacity < 2 {
            return Ok(0);
        }
        // `nSize` é em **bytes**, e cada `AECHAR` ocupa dois; o terminador entra na conta.
        let room = (capacity as usize / 2).saturating_sub(1);
        let written = text.len().min(room);
        let bytes: Vec<u8> = text[..written]
            .iter()
            .chain(std::iter::once(&0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        self.cpu.write_mem(buffer, &bytes)?;
        Ok(written as u32)
    }

    /// `void *ISHELL_LoadResData(IShell *po, const char *pszResFile, uint16 nResID,
    /// ResType nType)` e a variante `Ex`, que ainda recebe `void *pBuf, uint32 *pnBufSize`.
    ///
    /// A documentação é explícita: o que sai daqui é o conteúdo **bruto** do recurso, cabeçalho
    /// `AEEResBlob` incluído. Quem interpreta é o jogo, com o `RESBLOB_DATA()`.
    fn shell_load_res_data(&mut self, with_buffer: bool) -> Result<u32, CpuError> {
        let (file, id) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2) as u16,
        );
        let kind = self.cpu.read_reg(Reg::R3) as u16;
        let Some(data) = self
            .open_res_file(file)
            .and_then(|res| res.get(kind, id))
            .map(<[u8]>::to_vec)
        else {
            return Ok(0);
        };
        let len = data.len() as u32;

        if !with_buffer {
            // Sem buffer do chamador, o BREW aloca — e quem libera é o `FreeResData`.
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            return Ok(ptr);
        }

        let (buffer, size_out) = (self.stack_arg(0)?, self.stack_arg(1)?);
        // `pBuf` valendo -1 pede só o tamanho, sem copiar nada.
        if buffer == u32::MAX {
            self.write_at(size_out, len)?;
            return Ok(u32::MAX);
        }
        if buffer == 0 {
            let ptr = self.malloc(len)?;
            if ptr != 0 {
                self.cpu.write_mem(ptr, &data)?;
            }
            self.write_at(size_out, len)?;
            return Ok(ptr);
        }
        // Com buffer do chamador, `*pnBufSize` chega dizendo o tamanho dele. Não cabendo, a
        // documentação manda devolver nulo.
        let declared = match size_out {
            0 => 0,
            _ => self.cpu.read_u32(size_out)?,
        };
        // Nem todo jogo preenche o `*pnBufSize` de entrada. O Peggle consulta o tamanho do
        // recurso, aloca os 64.629 bytes que a consulta devolveu e chama de novo com o
        // `*pnBufSize` valendo 6 — o número do tipo, que ficou na variável. Recusar por causa
        // disso deixava o buffer zerado, e o decodificador recebia sessenta mil bytes de zeros:
        // o jogo quebrava logo depois, num ponteiro nulo que ele nem confere.
        //
        // O tamanho do bloco no heap é a medida honesta: não é o número que o jogo disse, é o
        // que ele de fato reservou. Assim a cópia continua limitada ao que existe.
        let capacity = declared.max(self.heap.size_of(buffer).unwrap_or(0));
        if capacity < len {
            self.write_at(size_out, len)?;
            return Ok(0);
        }
        self.cpu.write_mem(buffer, &data)?;
        self.write_at(size_out, len)?;
        Ok(buffer)
    }

    /// `uint32 ISHELL_GetClassItemID(IShell *po, AEECLSID cls)`.
    ///
    /// O item ID é o número que a loja do BREW dá ao pacote que instalou o módulo — e é
    /// literalmente o nome do diretório em que o `.mod` vive (`mod/277083/bjt.mod`). Não é
    /// palpite: é a mesma numeração que aparece no `.mif` e no caminho da ROM.
    ///
    /// A documentação manda devolver 0 quando a classe não é de um módulo baixado, que é o
    /// que fazemos para qualquer classe que não seja a do applet carregado.
    fn shell_get_class_item_id(&mut self) -> u32 {
        if self.cpu.read_reg(Reg::R1) != self.applet_class {
            return 0;
        }
        self.vfs
            .root()
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse().ok())
            .unwrap_or(0)
    }

    /// `ISHELL_GetDeviceInfo(IShell *po, AEEDeviceInfo *pi)`.
    ///
    /// Preenchemos só os campos do começo da struct, que têm posição inequívoca: os oito
    /// `uint16` iniciais. O que vem depois depende de como o compilador empacota bitfields, e
    /// chutar layout aqui seria pior do que deixar zerado. Os campos a partir de `wStructSize`
    /// são preenchidos pelo próprio chamador e não são tocados.
    fn shell_get_device_info(&mut self) -> Result<u32, CpuError> {
        let info = self.cpu.read_reg(Reg::R1);
        if info == 0 {
            return Ok(SUCCESS);
        }

        // O chamador escreve `wStructSize` **antes** da chamada para pedir os campos
        // estendidos, e o Bejeweled Twist pede: manda 64. Enquanto só preenchíamos os 44
        // primeiros bytes, ele recebia `wMaxPath = 0` — nenhum caminho de arquivo caberia.
        let mut requested = [0u8; 2];
        self.cpu.read_mem(info + 44, &mut requested)?;
        let requested = u16::from_le_bytes(requested) as u32;

        // Zera até `dwLang`, o último campo antes da parte que o chamador preenche.
        self.cpu.write_mem(info, &[0u8; 44])?;

        let fields: [u16; 8] = [
            SCREEN_WIDTH,
            SCREEN_HEIGHT,
            0, // cxAltScreen: o Zeebo não tem segunda tela
            0, // cyAltScreen
            8, // cxScrollBar
            AEE_ENC_ISOLATIN1,
            0, // wMenuTextScroll
            COLOR_DEPTH,
        ];
        for (i, value) in fields.iter().enumerate() {
            self.cpu
                .write_mem(info + i as u32 * 2, &value.to_le_bytes())?;
        }
        // `dwRAM` — a documentação chama de "tamanho inicial do heap do BREW".
        self.cpu.write_u32(info + 24, loader::HEAP_SIZE as u32)?;

        if requested >= DEVICE_INFO_SIZE {
            self.cpu
                .write_mem(info + 44, &(DEVICE_INFO_SIZE as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 48, 0)?; // dwNetLinger: sem rede, nada a manter aberto
            self.cpu.write_u32(info + 52, 0)?; // dwSleepDefer: o console não dorme
            self.cpu
                .write_mem(info + 56, &(AEE_MAX_FILE_NAME as u16).to_le_bytes())?;
            self.cpu.write_u32(info + 60, 0)?; // dwPlatformID: não sabemos o do Zeebo
        }
        Ok(SUCCESS)
    }

    /// `ISHELL_GetDeviceInfoEx(IShell *po, AEEDeviceItem nItem, void *pBuff, int *pnSize)`.
    ///
    /// `pnSize` é de entrada e saída: entra com o tamanho do buffer e sai com o que o item
    /// precisa. Com `pBuff` nulo a chamada serve só para perguntar esse tamanho, e é assim que
    /// o Zeebo Sports Peteca pede o IMEI — devolver sucesso sem escrever ali deixava o jogo
    /// alocar com lixo e ler fora da memória.
    fn shell_get_device_info_ex(&mut self) -> Result<u32, CpuError> {
        let (item, buffer, size) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        if size == 0 {
            return Ok(EBADPARM);
        }
        let value = match item {
            DEVICEITEM_IMEI => IMEI,
            _ => return Ok(EUNSUPPORTED),
        };
        let capacity = self.cpu.read_u32(size)? as usize;
        self.cpu.write_u32(size, value.len() as u32)?;
        if buffer != 0 {
            // Cabendo ou não, o que couber é escrito: a documentação prevê o preenchimento
            // parcial, com `pnSize` dizendo quanto faltou.
            self.cpu
                .write_mem(buffer, &value[..capacity.min(value.len())])?;
        }
        Ok(SUCCESS)
    }

    /// Manda `EVT_APP_START` para o applet, que é o que dá partida no jogo.
    ///
    /// `boolean IAPPLET_HandleEvent(IApplet *po, AEEEvent evt, uint16 wParam, uint32 dwParam)`
    /// — slot 2 da vtable de `IApplet`. Em `EVT_APP_START`, `dwParam` aponta para um
    /// `AEEAppStart`, que montamos no heap.
    pub fn start_applet(
        &mut self,
        applet: u32,
        clsid: u32,
        budget: u64,
    ) -> Result<Outcome, CpuError> {
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        // A partir daqui `GetAppInstance` tem o que devolver — e passa a responder sem sair
        // da CPU.
        self.current_applet = applet;
        self.install_app_instance_stub(applet)?;

        let display = self.objects.create(Interface::Display).unwrap_or(0);
        if display != 0 {
            self.cpu
                .write_u32(display, loader::vtable_addr(Interface::Display))?;
        }

        // AEEAppStart: error, clsApp, pDisplay, rc (4 × int16), pszArgs = 24 bytes.
        let start = self.heap.alloc(24).unwrap_or(0);
        if start != 0 {
            self.cpu.write_mem(start, &[0u8; 24])?;
            self.cpu.write_u32(start + 4, clsid)?;
            self.cpu.write_u32(start + 8, display)?;
            for (i, value) in [0i16, 0, SCREEN_WIDTH as i16, SCREEN_HEIGHT as i16]
                .iter()
                .enumerate()
            {
                self.cpu
                    .write_mem(start + 12 + i as u32 * 2, &value.to_le_bytes())?;
            }
        }

        self.call_guest(handle_event, [applet, EVT_APP_START, 0, start], budget)
    }

    /// Métodos de `IDisplay`. Despachamos pelo **nome** do slot, não pelo número: a tabela de
    /// nomes vem dos headers do SDK, então um método fora de ordem viraria erro de compilação
    /// aqui em vez de desenho errado lá.
    fn display_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Display.method(slot) else {
            return Ok(None);
        };
        let result = match name {
            // O `IDisplay` não tem contagem própria: o objeto é a tela, e ela vive enquanto o
            // emulador viver. Contamos as referências só para o jogo ver o número que espera.
            "AddRef" => self.objects.add_ref(self.cpu.read_reg(Reg::R0)),
            "Release" => self.objects.release(self.cpu.read_reg(Reg::R0)),
            // RGBVAL IDISPLAY_SetColor(IDisplay *p, AEEClrItem clr, RGBVAL rgb)
            "SetColor" => {
                let item = self.cpu.read_reg(Reg::R1) as usize;
                let previous = self.colors.get(item).copied().unwrap_or(Rgb::BLACK);
                if let Some(slot) = self.colors.get_mut(item) {
                    *slot = Rgb::from_rgbval(self.cpu.read_reg(Reg::R2));
                }
                to_rgbval(previous)
            }
            // void IDISPLAY_DrawRect(IDisplay *p, const AEERect *pr, RGBVAL cf, RGBVAL cfill,
            //                        uint32 flags)
            "DrawRect" => {
                let rect = self
                    .read_rect(self.cpu.read_reg(Reg::R1))?
                    .and_then(|rect| self.clip_rect(rect));
                let border = Rgb::from_rgbval(self.cpu.read_reg(Reg::R2));
                let fill = self.cpu.read_reg(Reg::R3);
                let target = self.target()?;
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&target)) {
                    // `RGB_NONE` marca "não pinte"; qualquer outro valor é cor de fato.
                    if fill != RGB_NONE {
                        fb.fill_rect(rect, Rgb::from_rgbval(fill));
                    }
                    fb.draw_frame(rect, border);
                }
                SUCCESS
            }
            // int IDISPLAY_GetDeviceBitmap(IDisplay *p, IBitmap **ppIBitmap).
            //
            // Devolve **código de erro** e entrega a superfície pelo ponteiro de saída, ao
            // contrário do `GetDestination` logo abaixo, que devolve o `IBitmap *` direto. Eu
            // tinha implementado os dois iguais, e o Bejeweled Twist — que testa
            // `if (retorno != 0) falhou` — desistia da inicialização por causa disso.
            "GetDeviceBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let bitmap = self.device_bitmap()?;
                if out != 0 {
                    self.cpu.write_u32(out, bitmap)?;
                }
                if bitmap == 0 {
                    ENOMEMORY
                } else {
                    // Devolve uma referência: o jogo dá `Release` quando termina, e faz isso
                    // uma vez por quadro. Sem o `AddRef` a contagem zerava e a superfície da
                    // tela era destruída no meio da execução.
                    self.objects.add_ref(bitmap);
                    SUCCESS
                }
            }
            // int SetDestination(IDisplay *, IBitmap *pbmDest)
            //
            // O destino nem sempre é uma superfície nossa: o Bejeweled Twist implementa o
            // próprio `IBitmap`, com a vtable embutida no objeto (o `DECLARE_VTBL` do BREW).
            // Para desenhar nela é preciso perguntar a ela onde ficam os pixels — o que exige
            // entrar no guest, e por isso fica para a fronteira da chamada.
            "SetDestination" => {
                let target = self.cpu.read_reg(Reg::R1);
                if target != 0 && !self.bitmaps.contains_key(&target) && self.probed.insert(target)
                {
                    self.pending_probes.push(target);
                }
                self.display_target = target;
                SUCCESS
            }
            "GetDestination" => self.target()?,
            // void IDISPLAY_BitBlt(IDisplay *p, int xd, int yd, int w, int h,
            //                      const void *pbmSource, int xs, int ys, AEERasterOp rop)
            "BitBlt" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                let target = self.target()?;
                if let Some((dst_pos, size, origin)) = self.clip_blit(dst_pos, size, origin) {
                    self.blit(target, dst_pos, size, src, origin, rop);
                }
                SUCCESS
            }
            // void IDISPLAY_DrawText(IDisplay *, AEEFont, const AECHAR *pcText, int nChars,
            //                        int x, int y, const AEERect *prcBackground, uint32 dwFlags)
            "DrawText" => {
                let text = self.read_aechar(self.cpu.read_reg(Reg::R2))?;
                let (x, y) = (self.stack_arg(0)? as i32, self.stack_arg(1)? as i32);
                match self.draw_text(&text, x, y)? {
                    true => {}
                    // Sem fonte, o texto continua indo só para o relatório: é o que permite
                    // saber que o jogo *quer* escrever mesmo quando não há com o quê.
                    false => {
                        // A comparação só roda enquanto há espaço; cheia, a lista custa um
                        // teste de tamanho por chamada.
                        if self.pending_text.len() < MAX_TEXT && !self.pending_text.contains(&text)
                        {
                            self.pending_text.push(text);
                        }
                    }
                }
                SUCCESS
            }
            // Sem efeito visível para nós: o framebuffer já está sempre atualizado.
            // void IDISPLAY_GetClipRect(IDisplay *p, AEERect *prc)
            //
            // O recorte é ignorado no desenho, então o retângulo corrente é a tela inteira —
            // que é o que o BREW devolve quando ninguém apertou o recorte. O Pac-Mania lê isto
            // para guardar e restaurar depois.
            "GetClipRect" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let rect = self.clip.unwrap_or_else(|| {
                        let screen = self.screen();
                        Rect {
                            x: 0,
                            y: 0,
                            width: screen.width() as i16,
                            height: screen.height() as i16,
                        }
                    });
                    let mut bytes = [0u8; 8];
                    bytes[0..2].copy_from_slice(&rect.x.to_le_bytes());
                    bytes[2..4].copy_from_slice(&rect.y.to_le_bytes());
                    bytes[4..6].copy_from_slice(&rect.width.to_le_bytes());
                    bytes[6..8].copy_from_slice(&rect.height.to_le_bytes());
                    self.cpu.write_mem(out, &bytes)?;
                }
                SUCCESS
            }
            // int IDISPLAY_GetFontMetrics(IDisplay *p, AEEFont font, int *pnAscent,
            //                              int *pnDescent)
            //
            // Devolve a altura da fonte. Ainda não desenhamos texto, mas os jogos medem antes
            // de posicionar: sem números aqui, o Pac-Mania nem chega a montar a tela. Os
            // valores são os de uma fonte de tela pequena, coerentes entre si.
            // int IDISPLAY_MeasureTextEx(IDisplay *, AEEFont, const AECHAR *pcText,
            //                             int nChars, int nMaxWidth, int *pnFits)
            //
            // Devolve a largura em pixels e, em `pnFits`, quantos caracteres cabem em
            // `nMaxWidth`. Como ainda não desenhamos texto, a largura sai de um avanço fixo por
            // caractere, coerente com a altura que o `GetFontMetrics` informa. É medida
            // aproximada de propósito: serve para o jogo centralizar e quebrar linha, e um
            // número plausível o deixa seguir — sem nenhum, o Pac-Mania para na escolha do
            // idioma.
            "MeasureTextEx" => {
                let text = self.read_aechar_units(self.cpu.read_reg(Reg::R2))?;
                let chars = self.cpu.read_reg(Reg::R3) as i32;
                let count = match chars < 0 {
                    true => text.len(),
                    false => (chars as usize).min(text.len()),
                };
                let max_width = self.stack_arg(0)? as i32;
                // Com fonte, a largura é medida caractere a caractere até estourar o limite;
                // sem ela sobra a largura fixa, que é chute honesto mas chute.
                let (fits, largura) = match &self.font {
                    Some(fonte) => {
                        let mut cabem = 0;
                        let mut largura = 0;
                        for fim in 1..=count {
                            let trecho: String = text[..fim]
                                .iter()
                                .filter_map(|u| char::from_u32(u32::from(*u)))
                                .collect();
                            let candidata = fonte.width(&trecho, FONT_SIZE) as i32;
                            if max_width >= 0 && candidata > max_width {
                                break;
                            }
                            (cabem, largura) = (fim, candidata);
                        }
                        (cabem, largura as u32)
                    }
                    None => {
                        let cabem = match max_width < 0 {
                            true => count,
                            false => count.min((max_width / FONT_ADVANCE).max(0) as usize),
                        };
                        (cabem, (cabem as i32 * FONT_ADVANCE) as u32)
                    }
                };
                let out = self.stack_arg(1)?;
                if out != 0 {
                    self.cpu.write_u32(out, fits as u32)?;
                }
                largura
            }
            "GetFontMetrics" => {
                // Com fonte de verdade, a medida é dela: um menu que centraliza pela altura da
                // linha fica torto se a altura for chute.
                let (ascent, descent) = match &self.font {
                    Some(fonte) => (fonte.ascent(FONT_SIZE), fonte.descent(FONT_SIZE)),
                    None => (FONT_ASCENT, FONT_DESCENT),
                };
                self.write_at(self.cpu.read_reg(Reg::R2), ascent)?;
                self.write_at(self.cpu.read_reg(Reg::R3), descent)?;
                ascent + descent
            }
            // int IDISPLAY_SetClipRect(IDisplay *p, const AEERect *prc)
            //
            // Ponteiro nulo volta ao recorte cheio, que é o que o BREW define. Ignorar esta
            // chamada custava caro: o Pac-Mania desenha a folha de fontes inteira e conta com
            // o recorte para que só a letra apareça — sem ele, a folha toda ia para a tela.
            "SetClipRect" => {
                self.clip = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                SUCCESS
            }
            // int IDISPLAY_Clone(IDisplay *po, IDisplay **ppNew)
            //
            // Uma cópia do objeto de tela, para o app desenhar noutro estado sem mexer no
            // corrente. A tela é uma só, então o que sai daqui é o mesmo objeto com uma
            // referência a mais — é o que o BREW faz quando o dispositivo tem uma tela.
            "Clone" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out == 0 {
                    return Ok(Some(EBADPARM));
                }
                // Um objeto novo, não o mesmo com uma referência a mais: o app solta a cópia
                // quando termina com ela, e devolver o original faria essa soltura derrubar a
                // tela que ele ainda usa. O estado de desenho é da `Machine`, não do objeto,
                // então dois objetos apontam para a mesma tela sem se atrapalharem.
                let clone = self.new_object(Interface::Display)?;
                if clone == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.cpu.write_u32(out, clone)?;
                SUCCESS
            }
            "Update" | "SetFont" | "SetAnnunciators" | "Backlight" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Cria um objeto novo e grava o ponteiro de vtable dele na memória do guest.
    fn new_object(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let Some(addr) = self.objects.create(iface) else {
            return Ok(0);
        };
        self.cpu.write_u32(addr, loader::vtable_addr(iface))?;
        Ok(addr)
    }

    /// O bitmap da tela, criado na primeira vez que alguém pede.
    fn device_bitmap(&mut self) -> Result<u32, CpuError> {
        if self.device_bitmap != 0 {
            return Ok(self.device_bitmap);
        }
        let addr = self.new_object(Interface::Bitmap)?;
        if addr != 0 {
            let screen = std::mem::replace(&mut self.screen, Framebuffer::new(1, 1));
            self.bitmaps.insert(addr, screen);
            self.device_bitmap = addr;
            self.display_target = addr;
        }
        Ok(addr)
    }

    /// Superfície onde o `IDisplay` desenha.
    fn target(&mut self) -> Result<u32, CpuError> {
        if self.display_target == 0 {
            return self.device_bitmap();
        }
        Ok(self.display_target)
    }

    /// Entrega os sinais disparados, chamando os callbacks do guest.
    ///
    /// Precisa rodar fora do despacho de uma chamada, quando o guest não está no meio de
    /// outra — daí a fila.
    pub fn deliver_signals(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        // A resposta de rede compartilha esta fronteira pelo mesmo motivo dos sinais: entregá-la
        // pede chamar o alocador do jogo, e isso só é seguro fora do despacho.
        self.flush_response()?;
        self.pinta_widgets()?;
        self.parte_animacao()?;
        self.flush_keys()?;
        let pending = std::mem::take(&mut self.pending_signals);
        let mut outcomes = Vec::new();
        for callback in pending {
            if callback.function == 0 {
                continue;
            }
            outcomes.push(self.call_guest(
                callback.function,
                [callback.context, 0, 0, 0],
                budget,
            )?);
        }
        Ok(outcomes)
    }

    /// Dá a partida na abertura, **uma vez** por tratador registrado.
    ///
    /// Daqui para a frente a `AnimationVideo_Form` anda sozinha, e a corrente inteira está
    /// lida: a `0x11528` arma um `ISHELL_SetTimer` de mil milissegundos com o retorno de chamada
    /// `0x114ac`, que é o próprio tique; cada tique olha `[formulário+0x2c]` e avança de estado;
    /// no estado três a `0x11610` registra um `ISHELL_Resume` para a `0x11750`, que fecha o
    /// formulário e chama a `0x82464` — e é ela que leva ao menu principal.
    ///
    /// O console dá **um** aviso e o resto é do jogo. O aviso é o par `(0x801, 0x5064)` no
    /// tratador que o slot 4 registrou, que é a forma que o `0x11828` desvia para o `0x114ac`.
    ///
    /// Entregar mais de um seria inventar cadência, e isso já custou uma travada.
    fn parte_animacao(&mut self) -> Result<(), CpuError> {
        /// O par que a `0x11828` entende como "começou".
        const PARTIDA: (u32, u32) = (0x801, 0x5064);

        let novos: Vec<(u32, u32)> = self
            .widgets
            .iter()
            .filter(|(_, widget)| widget.tratador != 0 && !widget.partiu)
            .map(|(&objeto, widget)| (objeto, widget.tratador))
            .collect();
        for (objeto, onde) in novos {
            if let Some(widget) = self.widgets.get_mut(&objeto) {
                widget.partiu = true;
            }
            let (funcao, contexto) = (self.cpu.read_u32(onde)?, self.cpu.read_u32(onde + 4)?);
            if funcao != 0 {
                self.call_guest(funcao, [contexto, PARTIDA.0, PARTIDA.1, 1], QSORT_BUDGET)?;
            }
        }
        Ok(())
    }

    /// Pinta as imagens penduradas nos widgets.
    ///
    /// **Isto é um substituto declarado, não uma emulação.** No console quem desenha a
    /// interface é a extensão de widgets, que não temos: os nossos guardam a árvore — filhos,
    /// tamanho, tratador — e não rasterizam nada. Sem alguém pintando, a Z-Wheel monta a tela
    /// inteira e o quadro fica preto, que foi exatamente o que se via.
    ///
    /// O que dá para fazer com o que está guardado é isto: toda imagem pendurada num widget vai
    /// para a tela. Para a abertura da Z-Wheel basta, porque é uma imagem de 640×480 na origem
    /// — o mesmo tamanho que o jogo manda para o widget no slot 7.
    ///
    /// A ordem é a dos endereços dos objetos, que no nosso alocador é a de criação. Não é
    /// profundidade de verdade; é a única ordem estável que temos, e uma ordem estável ao menos
    /// faz o resultado ser o mesmo a cada execução.
    fn pinta_widgets(&mut self) -> Result<(), CpuError> {
        let mut imagens: Vec<u32> = self
            .widgets
            .values()
            .filter(|widget| widget.visivel)
            .flat_map(|widget| widget.anexados.iter().copied())
            .filter(|objeto| self.images.contains_key(objeto))
            .collect();
        imagens.sort_unstable();
        imagens.dedup();
        for imagem in imagens {
            self.draw_image(imagem, 0, 0, None)?;
        }
        Ok(())
    }

    /// Entrega os callbacks enfileirados (som, imagem, o que vier).
    ///
    /// Mesma razão da fila de sinais: eles rodam no guest, e não dá para reentrar no guest no
    /// meio do despacho de uma chamada dele.
    pub fn deliver_callbacks(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        let mut outcomes = Vec::new();
        // Um callback pode enfileirar outro, então drena até esvaziar — com teto, para que um
        // ciclo entre callbacks não prenda o emulador.
        for _ in 0..CALLBACK_ROUNDS {
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function == 0 {
                    continue;
                }
                outcomes.push(self.call_guest(call.function, call.args, budget)?);
            }
        }
        Ok(outcomes)
    }

    /// Raiz do sistema de arquivos que o jogo enxerga.
    pub fn file_root(&self) -> &std::path::Path {
        self.vfs.root()
    }

    /// Arquivos que o jogo pediu e não foram encontrados.
    pub fn missing_files(&self) -> Vec<String> {
        self.missing_files.iter().cloned().collect()
    }

    /// Chamadas que receberam ponteiro inválido do guest.
    pub fn bad_pointers(&self) -> Vec<String> {
        self.bad_pointers.iter().cloned().collect()
    }

    /// `IGraphics` — a API 2D do BREW, desenhada sobre a mesma superfície do `IDisplay`.
    ///
    /// O estado (cor de traço, cor de preenchimento, se preenche ou não, translação) fica no
    /// host; as primitivas viram operações no framebuffer.
    fn graphics_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Graphics.method(slot) else {
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
            // As cores chegam como três ou quatro `uint8` soltos, não como RGBVAL.
            "SetBackground" => {
                let previous = self.graphics.background;
                self.graphics.background = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetColor" => {
                let previous = self.graphics.stroke;
                self.graphics.stroke = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillColor" => {
                let previous = self.graphics.fill;
                self.graphics.fill = Rgb {
                    r: a1 as u8,
                    g: a2 as u8,
                    b: a3 as u8,
                };
                to_rgbval(previous)
            }
            "SetFillMode" => {
                let previous = self.graphics.fill_mode;
                self.graphics.fill_mode = a1 != 0;
                u32::from(previous)
            }
            "GetFillMode" => u32::from(self.graphics.fill_mode),
            "SetPointSize" => {
                let previous = self.graphics.point_size;
                self.graphics.point_size = a1 as u8;
                previous as u32
            }
            "GetPointSize" => self.graphics.point_size as u32,
            "GetColorDepth" => COLOR_DEPTH as u32,
            "Translate" => {
                self.graphics.origin = (a1 as i16 as i32, a2 as i16 as i32);
                SUCCESS
            }
            // int DrawPoint(IGraphics *, AEEPoint *) — AEEPoint é { int16 x, y }.
            "DrawPoint" => {
                let (x, y) = self.read_point(a1)?;
                let (x, y) = self.translated(x, y);
                let color = self.graphics.stroke;
                self.with_target(|fb| fb.set_pixel(x, y, color))?;
                SUCCESS
            }
            // int DrawLine(IGraphics *, AEELine *) — AEELine é { int16 sx, sy, ex, ey }.
            "DrawLine" => {
                let mut bytes = [0u8; 8];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (sx, sy) = self.translated(read(0), read(2));
                let (ex, ey) = self.translated(read(4), read(6));
                let color = self.graphics.stroke;
                self.with_target(|fb| fb.draw_line(sx, sy, ex, ey, color))?;
                SUCCESS
            }
            "DrawRect" | "ClearRect" => {
                let Some(rect) = self.read_rect(a1)? else {
                    return Ok(Some(EBADPARM));
                };
                let rect = self.translated_rect(rect);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                // `ClearRect` pinta com a cor de fundo; `DrawRect` respeita o modo de
                // preenchimento e sempre desenha a borda.
                if name == "ClearRect" {
                    let background = self.graphics.background;
                    self.with_target(|fb| fb.fill_rect(rect, background))?;
                } else {
                    self.with_target(|fb| {
                        if filled {
                            fb.fill_rect(rect, fill);
                        }
                        fb.draw_frame(rect, stroke);
                    })?;
                }
                SUCCESS
            }
            // AEECircle é { int16 cx, cy, r }.
            "DrawCircle" => {
                let mut bytes = [0u8; 6];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let (cx, cy) = self.translated(read(0), read(2));
                let radius = read(4);
                let (stroke, fill, filled) = (
                    self.graphics.stroke,
                    self.graphics.fill,
                    self.graphics.fill_mode,
                );
                self.with_target(|fb| {
                    if filled {
                        fb.fill_circle(cx, cy, radius, fill);
                    }
                    fb.draw_circle(cx, cy, radius, stroke);
                })?;
                SUCCESS
            }
            // AEETriangle é { int16 x0, y0, x1, y1, x2, y2 }.
            "DrawTriangle" => {
                let mut bytes = [0u8; 12];
                self.cpu.read_mem(a1, &mut bytes)?;
                let read = |i: usize| i16::from_le_bytes([bytes[i], bytes[i + 1]]) as i32;
                let points: Vec<(i32, i32)> = (0..3)
                    .map(|i| self.translated(read(i * 4), read(i * 4 + 2)))
                    .collect();
                self.draw_shape(&points, true)?;
                SUCCESS
            }
            // AEEPolygon e AEEPolyline são { int16 len; AEEPoint *points }.
            "DrawPolygon" | "DrawPolyline" => {
                let count = self.cpu.read_u32(a1)? as u16 as usize;
                let array = self.cpu.read_u32(a1 + 4)?;
                let mut points = Vec::with_capacity(count);
                for i in 0..count.min(MAX_POLYGON_POINTS) {
                    let (x, y) = self.read_point(array + i as u32 * 4)?;
                    points.push(self.translated(x, y));
                }
                self.draw_shape(&points, name == "DrawPolygon")?;
                SUCCESS
            }
            "ClearViewport" => {
                let background = self.graphics.background;
                let rect = Rect {
                    x: 0,
                    y: 0,
                    width: SCREEN_WIDTH as i16,
                    height: SCREEN_HEIGHT as i16,
                };
                self.with_target(|fb| fb.fill_rect(rect, background))?;
                SUCCESS
            }
            "SetDestination" => {
                self.display_target = a1;
                SUCCESS
            }
            "GetDestination" => self.target()?,
            // Sem efeito para nós: já desenhamos direto na superfície.
            "Update" | "EnableDoubleBuffer" | "SetPaintMode" | "SetClip" | "SetViewport"
            | "SetAlgorithmHint" | "SetStrokeStyle" | "Pan" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Desenha um polígono fechado (ou uma polilinha aberta) com o estado atual.
    fn draw_shape(&mut self, points: &[(i32, i32)], closed: bool) -> Result<(), CpuError> {
        if points.is_empty() {
            return Ok(());
        }
        let (stroke, fill, filled) = (
            self.graphics.stroke,
            self.graphics.fill,
            self.graphics.fill_mode && closed,
        );
        let points = points.to_vec();
        self.with_target(move |fb| {
            if filled {
                fb.fill_polygon(&points, fill);
            }
            let last = if closed {
                points.len()
            } else {
                points.len() - 1
            };
            for i in 0..last {
                let (x0, y0) = points[i];
                let (x1, y1) = points[(i + 1) % points.len()];
                fb.draw_line(x0, y0, x1, y1, stroke);
            }
        })
    }

    /// Executa uma operação de desenho na superfície corrente.
    fn with_target(&mut self, draw: impl FnOnce(&mut Framebuffer)) -> Result<(), CpuError> {
        let target = self.target()?;
        if let Some(fb) = self.bitmaps.get_mut(&target) {
            draw(fb);
        }
        Ok(())
    }

    /// Lê um `AEEPoint` — dois `int16`.
    fn read_point(&self, addr: u32) -> Result<(i32, i32), CpuError> {
        let mut bytes = [0u8; 4];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok((
            i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
            i16::from_le_bytes([bytes[2], bytes[3]]) as i32,
        ))
    }

    /// Aplica a translação corrente do `IGraphics`.
    fn translated(&self, x: i32, y: i32) -> (i32, i32) {
        (x + self.graphics.origin.0, y + self.graphics.origin.1)
    }

    fn translated_rect(&self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + self.graphics.origin.0 as i16,
            y: rect.y + self.graphics.origin.1 as i16,
            ..rect
        }
    }

    /// `IFileMgr` e `IFile`, sobre o diretório do módulo.
    fn file_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.open_files.remove(&this);
                    self.enumerations.remove(&this);
                }
                remaining
            }
            // IFile *OpenFile(IFileMgr *, const char *pszName, OpenFileMode mode).
            // Devolve o ponteiro do arquivo, não um código de erro.
            "OpenFile" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                self.open_file(&guest_path, a2)?
            }
            // int GetInfo(IFileMgr *, const char *pszName, FileInfo *pInfo)
            "GetInfo" if iface == Interface::FileMgr => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self
                    .vfs
                    .resolve(&guest_path)
                    .and_then(|p| std::fs::metadata(&p).ok())
                {
                    Some(meta) => {
                        self.write_file_info(a2, &guest_path, &meta)?;
                        SUCCESS
                    }
                    None => EFAILED,
                }
            }
            // int GetInfoEx(IFile *, AEEFileInfoEx *pInfo)
            //
            // **O tamanho fica em `+0xc`**, e isto foi lido, não suposto: em `0x89068` a Z-Wheel
            // chama este método e, na instrução seguinte, lê `sp[0xc]` como o tamanho e o passa
            // ao `malloc` — **sem conferir o retorno**. Recusar não é resposta neutra aqui: o
            // que ela lia era pilha por inicializar, e o que sobrava lá era um ponteiro. Daí o
            // `check_malloc: Malloc failed` que aparecia no log.
            //
            // Escrevemos dezesseis bytes e nada mais, e o limite tem motivo. O quadro daquela
            // função é de `0x2c` e ela guarda um local em `sp[0x28]`; a tentativa anterior
            // preenchia com o formato do `FileInfo`, **nome de arquivo incluído**, e o nome
            // passava por cima do vizinho. O app lia `0x6f6369d5` — texto — como tamanho e
            // pedia 1,8 GB.
            //
            // Os três primeiros campos vão zerados porque não sabemos o que são. Zero é uma
            // resposta que o jogo sabe tratar; lixo não.
            "GetInfoEx" if iface == Interface::File => {
                /// Onde o tamanho mora na `AEEFileInfoEx`, lido em `0x89078`.
                const TAMANHO: u32 = 0xc;
                /// Quanto da struct preenchemos. Ver acima o porquê de não ser mais.
                const QUANTO: usize = 0x10;

                let tamanho = self
                    .open_files
                    .get(&this)
                    .and_then(|aberto| aberto.file.metadata().ok())
                    .map(|meta| meta.len() as u32);
                let Some(tamanho) = tamanho else {
                    return Ok(Some(EBADPARM));
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &[0u8; QUANTO])?;
                    self.cpu.write_u32(a1 + TAMANHO, tamanho)?;
                }
                SUCCESS
            }
            // int GetInfo(IFile *, FileInfo *pInfo)
            "GetInfo" if iface == Interface::File => match self.open_files.get(&this) {
                Some(open) => {
                    let (path, meta) = (open.guest_path.clone(), open.file.metadata().ok());
                    match meta {
                        Some(meta) => {
                            self.write_file_info(a1, &path, &meta)?;
                            SUCCESS
                        }
                        None => EFAILED,
                    }
                }
                None => EBADPARM,
            },
            "Test" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if path.exists() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "Remove" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::remove_file(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            // int IFILEMGR_Rename(IFileMgr *, const char *pszSrc, const char *pszDest)
            //
            // O destino é resolvido pelo caminho exato, sem a busca sem caixa: renomear é criar
            // um nome novo, e casar com um arquivo existente de caixa diferente sobrescreveria
            // o arquivo errado.
            "Rename" => {
                let origem = self.cpu.read_cstring(a1, MAX_STRING);
                let destino = self.cpu.read_cstring(a2, MAX_STRING);
                match (self.vfs.resolve(&origem), self.vfs.resolve_new(&destino)) {
                    (Some(de), Some(para)) if std::fs::rename(&de, &para).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "MkDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::create_dir_all(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            "RmDir" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                match self.vfs.resolve(&guest_path) {
                    Some(path) if std::fs::remove_dir(&path).is_ok() => SUCCESS,
                    _ => EFAILED,
                }
            }
            // uint32 GetFreeSpace(IFileMgr *, uint32 *pdwTotal)
            "GetFreeSpace" | "GetFreeSpaceEx" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, FS_TOTAL_BYTES)?;
                }
                FS_FREE_BYTES
            }
            // int EnumInit(IFileMgr *, const char *pszDir, boolean bDirs)
            //
            // A listagem inteira sai daqui, de uma vez. O BREW deixa o estado dentro do
            // `IFileMgr`, e é o que fazemos: a fila é do gerenciador, não global.
            "EnumInit" => {
                let guest_dir = self.cpu.read_cstring(a1, MAX_STRING);
                let entries = self.list_dir(&guest_dir, a2 != 0);
                self.enumerations.insert(this, entries);
                SUCCESS
            }
            // boolean EnumNext(IFileMgr *, FileInfo *pInfo)
            //
            // Falso encerra a enumeração, e o `GetLastError` de depois devolve `EFAILED` mesmo
            // quando tudo correu bem — a documentação chama isso de compatibilidade com o
            // cliente 1.0, e há jogo que confere.
            "EnumNext" => {
                let next = self
                    .enumerations
                    .get_mut(&this)
                    .and_then(std::collections::VecDeque::pop_front);
                match next.and_then(|guest| {
                    let meta = self
                        .vfs
                        .resolve_dir(&guest)
                        .and_then(|p| p.metadata().ok())?;
                    Some((guest, meta))
                }) {
                    Some((guest, meta)) => {
                        self.write_file_info(a1, &guest, &meta)?;
                        TRUE
                    }
                    None => {
                        self.file_error = EFAILED;
                        FALSE
                    }
                }
            }
            "GetLastError" => self.file_error,
            // int ResolvePath(IFileMgr *, const char *cpszIn, char *pszOut, int *pnOutLen)
            "ResolvePath" => {
                let guest_path = self.cpu.read_cstring(a1, MAX_STRING);
                let limit = if a3 != 0 {
                    self.cpu.read_u32(a3)? as usize
                } else {
                    0
                };
                self.write_cstring_limited(a2, &guest_path, limit)?;
                if a3 != 0 {
                    self.cpu.write_u32(a3, guest_path.len() as u32 + 1)?;
                }
                SUCCESS
            }
            // int32 Read(IFile *, void *pBuffer, uint32 dwCount)
            "Read" => {
                let count = a2 as usize;
                let mut buffer = vec![0u8; count];
                let read = match self.open_files.get_mut(&this) {
                    Some(open) => std::io::Read::read(&mut open.file, &mut buffer).unwrap_or(0),
                    None => 0,
                };
                if read > 0 {
                    self.cpu.write_mem(a1, &buffer[..read])?;
                }
                read as u32
            }
            // uint32 Write(IFile *, const void *pBuffer, uint32 dwCount)
            "Write" => {
                let bytes = self.read_bytes(a1, a2)?;
                match self.open_files.get_mut(&this) {
                    Some(open) => std::io::Write::write(&mut open.file, &bytes).unwrap_or(0) as u32,
                    None => 0,
                }
            }
            // int32 Seek(IFile *, FileSeekType seek, int32 position)
            "Seek" => {
                let position = a2 as i32 as i64;
                let from = match a1 {
                    SEEK_END => std::io::SeekFrom::End(position),
                    SEEK_CURRENT => std::io::SeekFrom::Current(position),
                    _ => std::io::SeekFrom::Start(position.max(0) as u64),
                };
                match self.open_files.get_mut(&this) {
                    Some(open) => match std::io::Seek::seek(&mut open.file, from) {
                        // Documentado em `IFILE_Seek.htm`: o retorno é `SUCCESS`, exceto no caso
                        // especial de `_SEEK_CURRENT` com deslocamento zero, que devolve a
                        // posição atual.
                        Ok(offset) if a1 == SEEK_CURRENT && a2 == 0 => offset as u32,
                        Ok(_) => SUCCESS,
                        Err(_) => EFAILED,
                    },
                    None => EFAILED,
                }
            }
            "Truncate" => match self.open_files.get_mut(&this) {
                Some(open) if open.file.set_len(a1 as u64).is_ok() => SUCCESS,
                _ => EFAILED,
            },
            // Sem cache próprio e sem mapeamento de arquivo em memória.
            "SetCacheSize" => 0,
            "Map" => 0,
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Abre um arquivo do jogo, devolvendo o `IFile*` ou zero se não deu.
    fn open_file(&mut self, guest_path: &str, mode: u32) -> Result<u32, CpuError> {
        let Some(path) = self.vfs.resolve(guest_path) else {
            self.file_error = EFAILED;
            return Ok(0);
        };
        let mut options = std::fs::OpenOptions::new();
        if mode & OFM_CREATE != 0 {
            // No console o diretório do módulo já vem pronto do instalador; aqui ele só existe
            // se o jogo o criar, e vários jogos abrem `udata\algo` sem chamar `MkDir` antes.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            options.create(true).read(true).write(true);
        } else if mode & (OFM_READWRITE | OFM_APPEND) != 0 {
            options.read(true).write(true);
        } else {
            options.read(true);
        }
        if mode & OFM_APPEND != 0 {
            options.append(true);
        }

        let Ok(file) = options.open(&path) else {
            self.file_error = EFAILED;
            self.missing_files.insert(guest_path.to_string());
            return Ok(0);
        };
        let handle = self.new_object(Interface::File)?;
        if handle == 0 {
            return Ok(0);
        }
        self.open_files.insert(
            handle,
            OpenFile {
                file,
                guest_path: guest_path.to_string(),
            },
        );
        self.file_error = SUCCESS;
        Ok(handle)
    }

    /// Preenche um `FileInfo`: `char attrib` (com três bytes de alinhamento), `uint32
    /// dwCreationDate`, `uint32 dwSize` e `char szName[64]`.
    /// Os arquivos (ou os diretórios) de um diretório do guest, já com o caminho que o jogo
    /// entende.
    ///
    /// O nome devolvido é o caminho completo, com o mesmo prefixo que o jogo passou: é ele que
    /// volta para o `OpenFile` logo em seguida, e um nome solto não abriria nada.
    fn list_dir(&self, guest_dir: &str, want_dirs: bool) -> std::collections::VecDeque<String> {
        let Some(dir) = self.vfs.resolve_dir(guest_dir) else {
            return Default::default();
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Default::default();
        };
        // Ordenar deixa a listagem repetível: `read_dir` não promete ordem, e um jogo que
        // monta um menu com ela mudaria de ordem entre duas aberturas.
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir() == want_dirs)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        let prefix = guest_dir.trim_end_matches('/');
        names
            .into_iter()
            .map(|name| match prefix.is_empty() {
                true => name,
                false => format!("{prefix}/{name}"),
            })
            .collect()
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

    /// Escreve `EGL_TRUE` no `AEEEGLBoolean *ret` do argumento `slot` e devolve `SUCCESS`.
    fn write_egl_true(&mut self, slot: usize) -> Result<u32, CpuError> {
        let out = self.arg(slot);
        if out != 0 {
            self.cpu.write_u32(out, gles::EGL_TRUE)?;
        }
        Ok(SUCCESS)
    }

    /// Um ajuste de textura vindo da extensão, com os nomes do OpenGL ES.
    fn apply_texture_setting(&mut self, name: &str, pname: u32, value: u32) {
        match name.starts_with("TexEnv") {
            true if pname == gles::GL_TEXTURE_ENV_MODE => self.gl.set_texture_env(value),
            true => {}
            false => self.gl.set_texture_parameter(pname, value),
        }
    }

    /// `IImageDecoder` e o `IForceFeed` que o alimenta.
    ///
    /// O jogo cria o decodificador, pede a ele a interface de entrada, escreve o arquivo em
    /// pedaços, fecha com uma escrita vazia e busca o bitmap. É o caminho que o Heavy Weapon, o
    /// Tork and Kral e o Peggle usam para as imagens deles.
    fn decoder_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        // Um `IForceFeed` trabalha sempre sobre o decodificador que o criou.
        let decoder = match iface {
            Interface::ForceFeed => self.feeds.get(&this).copied().unwrap_or(0),
            _ => this,
        };
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.feeds.remove(&this);
                    self.decoders.remove(&this);
                }
                remaining
            }
            (_, "QueryInterface") => {
                if a2 == 0 {
                    return Ok(Some(EBADPARM));
                }
                match a1 {
                    AEEIID_FORCEFEED => {
                        let feed = self.new_object(Interface::ForceFeed)?;
                        if feed == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.feeds.insert(feed, decoder);
                        self.cpu.write_u32(a2, feed)?;
                        SUCCESS
                    }
                    _ => {
                        self.unknown_classes.insert(a1);
                        self.cpu.write_u32(a2, 0)?;
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // int Write(IForceFeed *, void *pBuf, int cb)
            //
            // Escrita vazia é o fim do arquivo — é assim que o exemplo do SDK fecha a entrega.
            // Nada a fazer aqui: a decodificação acontece no `GetBitmap`, e adiantá-la só
            // gastaria trabalho se o jogo desistisse no meio.
            (Interface::ForceFeed, "Write") => {
                let count = a2 as usize;
                if a1 != 0 && count > 0 {
                    let mut bytes = vec![0u8; count];
                    self.cpu.read_mem(a1, &mut bytes)?;
                    let state = self.decoders.entry(decoder).or_default();
                    if state.fed.len() + count <= MAX_DECODED_INPUT {
                        state.fed.extend_from_slice(&bytes);
                    }
                }
                SUCCESS
            }
            (Interface::ForceFeed, "Reset") => {
                self.decoders.remove(&decoder);
                SUCCESS
            }
            // int GetBitmap(IImageDecoder *, IBitmap **ppiBitmap)
            (Interface::ImageDecoder, "GetBitmap") => {
                let bitmap = self.decoded_bitmap(decoder)?;
                if a1 != 0 {
                    self.cpu.write_u32(a1, bitmap)?;
                }
                match bitmap {
                    0 => EFAILED,
                    _ => SUCCESS,
                }
            }
            // int GetRop(IImageDecoder *) — com o que desenhar o bitmap devolvido.
            (Interface::ImageDecoder, "GetRop") => {
                self.decoded_bitmap(decoder)?;
                match self.decoders.get(&decoder).is_some_and(|d| d.transparent) {
                    true => AEE_RO_TRANSPARENT,
                    false => AEE_RO_COPY,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// O bitmap de um decodificador, decodificando na primeira vez que é pedido.
    fn decoded_bitmap(&mut self, decoder: u32) -> Result<u32, CpuError> {
        if let Some(state) = self.decoders.get(&decoder) {
            if let Some(bitmap) = state.bitmap {
                return Ok(bitmap);
            }
        }
        let Some(fed) = self.decoders.get(&decoder).map(|d| d.fed.clone()) else {
            return Ok(0);
        };
        let Some(image) = decode_png(&fed) else {
            self.assumptions
                .insert("um decodificador recebeu dados que não são um PNG");
            return Ok(0);
        };
        let addr = self.bitmap_from_decoded(&image)?;
        if addr == 0 {
            return Ok(0);
        }
        let transparent = self.transparency.contains_key(&addr);
        if let Some(state) = self.decoders.get_mut(&decoder) {
            state.bitmap = Some(addr);
            state.transparent = transparent;
        }
        Ok(addr)
    }

    /// Um `IBitmap` com a imagem já decodificada dentro.
    ///
    /// Sai com os campos públicos do `IDIB` preenchidos, porque um `IBitmap` de software do
    /// BREW é um `IDIB` e o jogo lê esses campos sem pedir a interface.
    fn bitmap_from_decoded(&mut self, image: &DecodedImage) -> Result<u32, CpuError> {
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(image.width, image.height);
        let mut transparent = false;
        for row in 0..image.height {
            for column in 0..image.width {
                let index = (row * image.width + column) as usize;
                let opaque = image.opaque.get(index).copied().unwrap_or(true);
                transparent |= !opaque;
                // Sem canal alfa no destino, o transparente vira a cor reservada — é como o
                // BREW resolve, e é o que o `GetRop` anuncia ao jogo em seguida.
                let pixel = match (opaque, image.pixels.get(index)) {
                    (true, Some(&pixel)) => pixel,
                    _ => TRANSPARENT_KEY,
                };
                surface.set_pixel_native(column as i32, row as i32, pixel);
            }
        }
        self.bitmaps.insert(addr, surface);
        if transparent {
            self.transparency.insert(addr, TRANSPARENT_KEY);
        }
        self.expose_dib(addr)?;
        Ok(addr)
    }

    /// Decodifica a imagem em `buffer` e devolve um `IBitmap` com ela.
    ///
    /// O ponteiro devolvido vai direto para o `IDISPLAY_BitBlt`, que recebe um `IBitmap *` — e
    /// é por isso que o "formato nativo" aqui é um bitmap nosso, e não um bloco solto de
    /// pixels: assim o desenho segue pelo mesmo caminho de todo o resto.
    ///
    /// O tamanho do bloco não vem por parâmetro: quem diz quanto ler é o cabeçalho da própria
    /// imagem, e por ora só o BMP — que é o que os jogos passam — declara o seu.
    fn setup_native_image(
        &mut self,
        buffer: u32,
        info: u32,
        realloc: u32,
    ) -> Result<u32, CpuError> {
        if realloc != 0 {
            // A imagem sai numa alocação nossa, e é isso que este sinalizador informa.
            self.cpu.write_mem(realloc, &[1])?;
        }
        let Some(len) = self.encoded_image_len(buffer)? else {
            return Ok(0);
        };
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(buffer, &mut bytes)?;
        let Ok(image) = crate::icon::decode(&bytes) else {
            self.assumptions
                .insert("uma imagem nativa veio num formato que não sabemos ler");
            return Ok(0);
        };

        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let (width, height) = (image.width as u32, image.height as u32);
        let mut fb = Framebuffer::new(width, height);
        for y in 0..image.height {
            for x in 0..image.width {
                let at = (y * image.width + x) * 4;
                let color = Rgb {
                    r: image.rgba[at],
                    g: image.rgba[at + 1],
                    b: image.rgba[at + 2],
                };
                fb.set_pixel_native(x as i32, y as i32, color.to_rgb565());
            }
        }
        self.bitmaps.insert(addr, fb);

        if info != 0 {
            let (cx, cy) = (width as u16, height as u16);
            self.cpu.write_mem(info, &cx.to_le_bytes())?;
            self.cpu.write_mem(info + 2, &cy.to_le_bytes())?;
            // `nColors` é zero para quem tem mais de 65535 cores, e `bAnimated` é falso.
            self.cpu.write_mem(info + 4, &[0u8; 4])?;
            self.cpu.write_mem(info + 8, &cx.to_le_bytes())?;
        }
        Ok(addr)
    }

    /// Quanto ler de um bloco de imagem, pelo cabeçalho dela.
    fn encoded_image_len(&self, buffer: u32) -> Result<Option<usize>, CpuError> {
        if buffer == 0 {
            return Ok(None);
        }
        let mut header = [0u8; 6];
        self.cpu.read_mem(buffer, &mut header)?;
        // O BMP declara o tamanho do arquivo na palavra seguinte à assinatura.
        if &header[0..2] != b"BM" {
            return Ok(None);
        }
        let size = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        Ok((size > 0 && size <= MAX_NATIVE_IMAGE).then_some(size))
    }

    fn write_file_info(
        &mut self,
        addr: u32,
        guest_path: &str,
        meta: &std::fs::Metadata,
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let attrib = if meta.is_dir() { FA_DIR } else { FA_NORMAL };
        self.cpu.write_u32(addr, attrib)?;
        self.cpu.write_u32(addr + 4, 0)?;
        self.cpu.write_u32(addr + 8, meta.len() as u32)?;
        let mut name = guest_path.as_bytes().to_vec();
        name.truncate(MAX_FILE_NAME - 1);
        name.resize(MAX_FILE_NAME, 0);
        self.cpu.write_mem(addr + 12, &name)
    }

    /// `ISignal`, `ISignalCtl` e `ISignalCBFactory`.
    ///
    /// Um sinal é o aviso "aconteceu alguma coisa". O app cria um pela fábrica, passando uma
    /// função de callback e um contexto, e entrega o `ISignal` a quem for notificá-lo — o
    /// gamepad, por exemplo, via `RegisterForButtonEvent`. Quando o sistema chama
    /// `ISIGNAL_Set`, o callback do app roda.
    ///
    /// Aqui o disparo é adiado em vez de imediato: chamar o guest de dentro do despacho de uma
    /// chamada do guest seria reentrância, e o BREW também não dispara na hora — ele agenda
    /// para o laço de eventos do app. Os sinais pendentes ficam registrados até termos esse
    /// laço.
    fn signal_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.signals.remove(&this);
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // int CreateSignal(ISignalCBFactory *, void (*pfn)(void *pCx), void *pCx,
            //                  ISignal **ppiSig, ISignalCtl **ppiSigCtl)
            "CreateSignal" => {
                let signal = self.new_object(Interface::Signal)?;
                let control = self.new_object(Interface::SignalCtl)?;
                if signal == 0 || control == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // Os dois objetos falam do mesmo sinal, então compartilham o callback.
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.signals.insert(signal, callback);
                self.signals.insert(control, callback);
                if a3 != 0 {
                    self.cpu.write_u32(a3, signal)?;
                }
                let out_control = self.stack_arg(0)?;
                if out_control != 0 {
                    self.cpu.write_u32(out_control, control)?;
                }
                SUCCESS
            }
            "Set" => {
                if let Some(&callback) = self.signals.get(&this) {
                    self.pending_signals.push(callback);
                }
                SUCCESS
            }
            // `Enable` rearma o sinal e `Detach` o desliga da fonte. Sem laço de eventos ainda,
            // os dois são registro de estado.
            "Enable" => SUCCESS,
            "Detach" => {
                self.signals.remove(&this);
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `IHID` e `IHIDDevice` — o gamepad do Zeebo.
    ///
    /// Apresentamos um controle sempre conectado, com os doze botões e os quatro eixos que o
    /// `hid_devices.cfg` do console descreve. O que o jogador aperta chega por
    /// [`Machine::set_pad`].
    fn hid_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IHID` herda de `IQI`, então AddRef e Release ficam nos mesmos slots 0 e 1.
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // QueryInterface: devolvemos o próprio objeto, já que cada objeto nosso tem uma
            // interface só.
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // GetConnectedDevices(int nDeviceType, int *pnHandles, int nLen, int *pnLenReq).
            // O quarto argumento não cabe nos registradores e vem da pilha.
            "GetConnectedDevices" => {
                let wanted = a1;
                let handles = a2;
                let capacity = a3;
                let out_needed = self.stack_arg(0)?;
                // O `nLenReq` é o que o aparelho **tem**, e o vetor recebe o que couber. A
                // Z-Wheel passa capacidade dois nas duas chamadas, que é o número de USB do
                // console.
                let quais = match wanted {
                    UID_JOYSTICK_DEVICE => self.portas_com(crate::bindings::Aparelho::Controle),
                    UID_KEYBOARD_DEVICE => self.portas_com(crate::bindings::Aparelho::Teclado),
                    _ => Vec::new(),
                };
                if out_needed != 0 {
                    self.cpu.write_u32(out_needed, quais.len() as u32)?;
                }
                if handles != 0 {
                    for (i, &porta) in quais.iter().take(capacity as usize).enumerate() {
                        self.cpu
                            .write_u32(handles + i as u32 * 4, handle_da_porta(porta))?;
                    }
                }
                SUCCESS
            }
            // CreateDevice(int nDevHandle, IHIDDevice **ppDevice)
            //
            // O identificador é o que saiu do `GetConnectedDevices`, e é aqui que ele vira
            // porta: sem guardar essa ligação, os dois aparelhos leriam o mesmo controle.
            "CreateDevice" => {
                let device = self.new_object(Interface::HidDevice)?;
                if device == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let porta = (a1.saturating_sub(1) as usize).min(input::PORTAS - 1);
                self.portas_de_aparelho.insert(device, porta);
                if a2 != 0 {
                    self.cpu.write_u32(a2, device)?;
                }
                SUCCESS
            }
            // GetDeviceInfo(AEEHIDDeviceInfo *pInfo): { int type; uint16 pid; uint16 vid;
            // boolean bluetooth }. Em IHID a struct vem no segundo argumento.
            "GetDeviceInfo" => {
                let out = if iface == Interface::Hid { a2 } else { a1 };
                // No `IHID` o identificador da porta vem em `r1`; no `IHIDDevice` é o próprio
                // objeto que diz de qual porta ele é.
                let porta = match iface == Interface::Hid {
                    true => (a1.saturating_sub(1) as usize).min(input::PORTAS - 1),
                    false => self.porta_do(this),
                };
                let tipo = match self.portas[porta] {
                    Some(crate::bindings::Aparelho::Teclado) => HID_TYPE_KEYBOARD,
                    _ => HID_TYPE_GAMEPAD,
                };
                if out != 0 {
                    self.cpu.write_u32(out, tipo)?;
                    self.cpu
                        .write_mem(out + 4, &GAMEPAD_PRODUCT_ID.to_le_bytes())?;
                    self.cpu
                        .write_mem(out + 6, &GAMEPAD_VENDOR_ID.to_le_bytes())?;
                    self.cpu.write_u32(out + 8, 0)?;
                }
                SUCCESS
            }
            "GetNumberOfButtons" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, input::BUTTON_UIDS.len() as u32)?;
                }
                SUCCESS
            }
            // GetButtonInfo(int nButtonID, AEEHIDButtonInfo *pInfo):
            // { id, estado, UID, mínimo, máximo }
            "GetButtonInfo" => {
                // O jogo passa um UID, não um índice — aceitamos as duas formas.
                let (index, uid) = match input::BUTTON_UIDS.iter().position(|&u| u == a1) {
                    Some(i) => (i as u32, a1),
                    None => match input::BUTTON_UIDS.get(a1 as usize) {
                        Some(&uid) => (a1, uid),
                        None => return Ok(Some(EBADPARM)),
                    },
                };
                // O estado tem de ser o de agora: um jogo que consulta em vez de esperar o
                // evento só enxerga a tecla por aqui.
                let state = u32::from(self.pads[self.porta_do(this)].is_down(index as usize));
                if a2 != 0 {
                    for (i, value) in [index, state, uid, 0, 1].iter().enumerate() {
                        self.cpu.write_u32(a2 + i as u32 * 4, *value)?;
                    }
                }
                SUCCESS
            }
            "GetDeviceStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, HID_STATUS_CONNECTED)?;
                }
                SUCCESS
            }
            // A posição corrente de cada eixo.
            "GetPositionState" => {
                let axes = self.pads[self.porta_do(this)].axes;
                self.write_position_info(a1, &axes)?;
                SUCCESS
            }
            // `GetAxesInfo` não devolve valores: devolve, em cada campo, o UID do eixo que
            // ocupa aquele campo. É assim que o `AEEHIDThumbsticks.c` do SDK descobre onde
            // está cada direção — e enquanto respondíamos zeros, ele não achava nenhuma.
            "GetAxesInfo" => {
                self.write_position_info(a1, &input::AXIS_UIDS.map(|uid| uid as i32))?;
                SUCCESS
            }
            // Os limites valem para **todos** os eixos da struct, não só os quatro que o
            // console usa. Preenchendo apenas quatro, os outros vinte ficavam com mínimo e
            // máximo iguais a zero, e um jogo que normalize um eixo desses — `(valor - min) /
            // (max - min)` — divide por zero e trava a direção num extremo. Era o que prendia
            // o carro do Crash virando para a esquerda.
            "GetMinPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MIN)?;
                SUCCESS
            }
            "GetMaxPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MAX)?;
                SUCCESS
            }
            // GetNextButtonEvent(AEEHIDButtonInfo *, uint32 *pdwTimestamp, boolean *pbDropped).
            //
            // A fila é de eventos, não de estado: cada aperto e cada soltura vira uma entrada,
            // e o jogo lê até a fila esvaziar. `EFAILED` é o "não há mais nada".
            "GetNextButtonEvent" => {
                let Some((index, down)) = self.pad_events[self.porta_do(this)].pop_front() else {
                    if a1 != 0 {
                        self.cpu.write_mem(a1, &[0u8; 20])?;
                    }
                    if a2 != 0 {
                        self.cpu.write_u32(a2, self.elapsed_ms())?;
                    }
                    if a3 != 0 {
                        self.cpu.write_u32(a3, 0)?;
                    }
                    return Ok(Some(EFAILED));
                };
                let uid = input::BUTTON_UIDS.get(index).copied().unwrap_or(0);
                if a1 != 0 {
                    // `AEEHIDButtonInfo`: id, estado, UID, mínimo, máximo.
                    let info = [index as u32, u32::from(down), uid, 0, 1];
                    for (i, value) in info.iter().enumerate() {
                        self.cpu.write_u32(a1 + i as u32 * 4, *value)?;
                    }
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, self.elapsed_ms())?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                SUCCESS
            }
            // GetNextConnectEvent(int *pnDevHandle, int *pnStatus, boolean *pbDropped).
            "GetNextConnectEvent" => {
                for out in [a1, a2, a3] {
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            // Os `RegisterFor*` recebem um `ISignal` que devemos disparar quando houver evento.
            // Guardamos qual é; disparar de fato depende de ligar a entrada do host.
            "RegisterForConnectEvents"
            | "RegisterForStatusChange"
            | "RegisterForButtonEvent"
            | "RegisterForPositionChange" => {
                if a1 != 0 {
                    self.input_signals.insert(name, a1);
                }
                SUCCESS
            }
            "SetExclusiveLevel" | "Rumble" => SUCCESS,
            "GetExclusiveLevel" | "GetRumbleStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, 0)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Preenche um `AEEHIDPositionInfo` com um valor por eixo.
    ///
    /// A struct é `boolean bRelativeAxes` seguido de vinte e quatro inteiros, um por eixo
    /// possível. O controle do Zeebo usa quatro deles — `X`, `Y`, `Z` e `RZ` —, e os outros
    /// ficam zerados: um eixo que não existe tem faixa zero, e é assim que o jogo sabe
    /// ignorá-lo. Os eixos são absolutos, então `bRelativeAxes` também fica zero.
    fn write_position_info(&mut self, addr: u32, values: &[i32; 4]) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [0u32; input::POSITION_INFO_WORDS];
        for (slot, value) in input::AXIS_SLOTS.iter().zip(values) {
            words[*slot] = *value as u32;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} ao escrever {} bytes em {addr:#010x}",
                bytes.len()
            ))
        })
    }

    /// Escreve `value` em todos os campos de eixo do `AEEHIDPositionInfo`.
    ///
    /// O primeiro campo da struct não é eixo: é o `boolean bRelativeAxes`, e ele fica em zero —
    /// os eixos do controle são absolutos, e dizer o contrário faria o jogo tratar cada leitura
    /// como um deslocamento e acumular.
    fn write_axis_range(&mut self, addr: u32, value: i32) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [value as u32; input::POSITION_INFO_WORDS];
        words[0] = 0;
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Entrega um novo estado do controle.
    ///
    /// Cada botão que mudou vira um evento na fila, e a mudança acorda o jogo pelos `ISignal`
    /// que ele registrou — sem isso o jogo só veria a tecla na próxima vez que resolvesse
    /// perguntar, e alguns nunca perguntam.
    pub fn set_pad(&mut self, pad: Pad) {
        self.set_port_pad(0, pad);
    }

    /// O mesmo, para uma porta escolhida.
    pub fn set_port_pad(&mut self, porta: usize, pad: Pad) {
        if porta >= input::PORTAS || pad == self.pads[porta] {
            return;
        }
        let changes = self.pads[porta].changes(&pad);
        let moved = pad.axes != self.pads[porta].axes;
        self.pads[porta] = pad;
        self.pad_events[porta].extend(changes.iter().copied());
        let agora = self.elapsed_ms();
        for &(index, down) in &changes {
            if self.pad_log.len() == PAD_LOG_MAX {
                self.pad_log.pop_front();
            }
            self.pad_log.push_back((agora, porta, index, down));
        }

        if !changes.is_empty() {
            self.raise_input_signal("RegisterForButtonEvent");
        }
        if moved {
            self.raise_input_signal("RegisterForPositionChange");
        }
    }

    /// A porta de um `IHIDDevice`, ou a primeira quando o objeto não foi registrado.
    ///
    /// O caminho sem janela e os testes criam aparelho sem passar pelo `CreateDevice` com
    /// identificador; para eles a porta um é a resposta certa, porque é a única que existe.
    fn porta_do(&self, aparelho: u32) -> usize {
        self.portas_de_aparelho
            .get(&aparelho)
            .copied()
            .unwrap_or(0)
            .min(input::PORTAS - 1)
    }

    /// Quais portas estão ligadas com o aparelho pedido, na ordem.
    fn portas_com(&self, aparelho: crate::bindings::Aparelho) -> Vec<usize> {
        (0..input::PORTAS)
            .filter(|&n| self.portas[n] == Some(aparelho))
            .collect()
    }

    /// Enfileira uma tecla do teclado, pelo código virtual do BREW.
    ///
    /// Não vai direto: teclado chega ao jogo como **evento**, e evento só pode ser entregue na
    /// fronteira entre duas chamadas de API — chamar o tratador do jogo no meio de um despacho é
    /// o caminho que já derrubou o Zeeboids. A fila é esvaziada em [`Machine::deliver_signals`].
    pub fn set_key(&mut self, avk: u32, down: bool) {
        self.teclas.push_back((avk, down));
    }

    /// Entrega as teclas enfileiradas.
    ///
    /// O evento é o `EVT_KEY` do BREW, com o código virtual no `wParam`. A soltura vai como
    /// `EVT_KEY + 1`, que é o `EVT_KEY_RELEASE`: um jogo que só olhe o aperto ignora a segunda
    /// sem prejuízo, e um que conte as duas precisa das duas.
    ///
    /// **Quem recebe tecla primeiro é o widget, não o aplicativo.** No BREW é a extensão de
    /// interface que roteia a entrada para quem está em foco, e o tratador do formulário de
    /// abertura da Z-Wheel prova: ele testa `evt == 0x100` e compara o `wParam` com códigos
    /// `AVK_`. Mandar direto ao aplicativo devolve zero — medido.
    ///
    /// A ordem é a do BREW: o widget tem a primeira chance e, se ninguém tratou, o evento sobe
    /// para o aplicativo. Um evento que ninguém trata não faz nada, e é por isso que entregar
    /// tecla é seguro de um jeito que inventar evento de propriedade não era.
    fn flush_keys(&mut self) -> Result<(), CpuError> {
        while let Some((avk, down)) = self.teclas.pop_front() {
            let evento = match down {
                true => input::EVT_KEY,
                false => input::EVT_KEY + 1,
            };
            let mut tratado = false;
            let tratadores: Vec<u32> = self
                .widgets
                .values()
                .map(|widget| widget.tratador)
                .filter(|&onde| onde != 0)
                .collect();
            for onde in tratadores {
                let (funcao, contexto) = (self.cpu.read_u32(onde)?, self.cpu.read_u32(onde + 4)?);
                if funcao == 0 {
                    continue;
                }
                let saida = self.call_guest(
                    funcao,
                    [contexto, evento, avk, 0],
                    QSORT_BUDGET,
                )?;
                if matches!(saida, Outcome::Returned { code } if code != 0) {
                    tratado = true;
                    break;
                }
            }
            if !tratado {
                let _ = self.send_applet_event(self.applet_class, evento, avk as u16, 0)?;
            }
        }
        Ok(())
    }

    /// Diz que aparelho o console vê em cada porta. `None` desliga a porta.
    pub fn set_portas(&mut self, portas: [Option<crate::bindings::Aparelho>; input::PORTAS]) {
        self.portas = portas;
    }

    /// Dispara o sinal registrado num dos `RegisterFor*` do `IHIDDevice`.
    fn raise_input_signal(&mut self, register: &'static str) {
        let Some(&signal) = self.input_signals.get(register) else {
            return;
        };
        if let Some(&callback) = self.signals.get(&signal) {
            self.pending_signals.push(callback);
        }
    }

    /// Métodos de `IBitmap`, despachados pelo nome do slot.
    fn bitmap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Bitmap.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IBitmap` tem `QueryInterface`, então AddRef e Release precisam ser tratados aqui:
            // o braço genérico do despacho só alcança interfaces que não têm tratamento próprio.
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            // int QueryInterface(IBitmap *, AEECLSID, void **) — o jogo usa isto para pedir um
            // `IDIB`, que dá acesso direto aos pixels. Ainda não oferecemos essa interface, e
            // `ECLASSNOTSUPPORT` é a resposta correta para isso: o BREW espera que o app tenha
            // caminho alternativo quando a plataforma não expõe o DIB.
            "QueryInterface" => {
                let requested = self.cpu.read_reg(Reg::R1);
                let out = self.cpu.read_reg(Reg::R2);
                let answer = match requested {
                    AEEIID_IBITMAP => Some(this),
                    // Um `IDIB` *é* um `IBitmap` — a struct começa com a vtable de `IBitmap` e
                    // só acrescenta campos públicos. Então o próprio objeto serve, desde que
                    // os campos estejam preenchidos.
                    AEECLSID_DIB => {
                        self.expose_dib(this)?;
                        Some(this)
                    }
                    _ => None,
                };
                match answer {
                    Some(pointer) => {
                        if out != 0 {
                            self.cpu.write_u32(out, pointer)?;
                        }
                        self.objects.add_ref(this);
                        SUCCESS
                    }
                    None => {
                        if out != 0 {
                            self.cpu.write_u32(out, 0)?;
                        }
                        self.unknown_classes.insert(requested);
                        ECLASSNOTSUPPORT
                    }
                }
            }
            // NativeColor no nosso caso é o próprio RGB565 do framebuffer.
            "RGBToNative" => Rgb::from_rgbval(self.cpu.read_reg(Reg::R1)).to_rgb565() as u32,
            "NativeToRGB" => {
                let native = self.cpu.read_reg(Reg::R1) as u16;
                to_rgbval(Rgb::from_rgb565(native))
            }
            // int DrawPixel(IBitmap *po, unsigned x, unsigned y, NativeColor c, AEERasterOp rop)
            "DrawPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let color = self.cpu.read_reg(Reg::R3) as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    fb.set_pixel_native(x, y, color);
                }
                // O buffer do jogo é a fonte da verdade quando ele existe, então o pixel vai
                // para os dois lugares — e só ele, não a superfície inteira.
                if let Some(at) = self.dib_pixel(this, x, y) {
                    self.cpu.write_mem(at, &color.to_le_bytes())?;
                }
                SUCCESS
            }
            "GetPixel" => {
                let (x, y) = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                // Do buffer do jogo, quando há um: ele pode ter escrito ali direto.
                let value = match self.dib_pixel(this, x, y) {
                    Some(at) => {
                        let mut bytes = [0u8; 2];
                        self.cpu.read_mem(at, &mut bytes)?;
                        u16::from_le_bytes(bytes)
                    }
                    None => self
                        .bitmaps
                        .get(&this)
                        .map(|fb| fb.get_pixel(x, y))
                        .unwrap_or(0),
                };
                let out = self.cpu.read_reg(Reg::R3);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            // int DrawHScanline(IBitmap *po, unsigned y, unsigned xMin, unsigned xMax, ...)
            "DrawHScanline" => {
                let y = self.cpu.read_reg(Reg::R1) as i32;
                let x_min = self.cpu.read_reg(Reg::R2) as i32;
                let x_max = self.cpu.read_reg(Reg::R3) as i32;
                let color = self.stack_arg(0)? as u16;
                if let Some(fb) = self.bitmaps.get_mut(&this) {
                    for x in x_min..=x_max {
                        fb.set_pixel_native(x, y, color);
                    }
                }
                SUCCESS
            }
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            // int FillRect(IBitmap *po, const AEERect *prc, NativeColor color, AEERasterOp rop)
            //
            // `IBITMAP_FillRect.htm`: só `AEE_RO_COPY` e `AEE_RO_XOR` valem; qualquer outra
            // operação é `EUNSUPPORTED` e **não desenha**. Enquanto ignorávamos o `rop`, o
            // Bejeweled Twist pintava a tela inteira de preto uma vez por quadro com um
            // `AEE_RO_TRANSPARENT` que o console teria recusado.
            "FillRect" => {
                let rop = self.cpu.read_reg(Reg::R3);
                let rect = self.read_rect(self.cpu.read_reg(Reg::R1))?;
                let color = self.cpu.read_reg(Reg::R2) as u16;
                // Com `AEE_RO_TRANSPARENT`, preencher com a própria cor transparente não
                // escreve nada. Ignorar o `rop` custava caro: o Bejeweled Twist chama exatamente
                // assim, com cor zero, e a tela inteira era apagada uma vez por quadro.
                let transparent = self.transparency.get(&this).copied().unwrap_or(0);
                if rop == AEE_RO_TRANSPARENT && color == transparent {
                    return Ok(Some(SUCCESS));
                }
                if let (Some(rect), Some(fb)) = (rect, self.bitmaps.get_mut(&this)) {
                    // `AEE_RO_COPY` é o normal; qualquer outra operação que chegue aqui ainda
                    // pinta, porque recusar o desenho é pior do que pintar demais.
                    if rop == AEE_RO_XOR {
                        fb.xor_rect_native(rect, color);
                    } else {
                        fb.fill_rect_native(rect, color);
                    }
                }
                SUCCESS
            }
            // int BltIn(IBitmap *po, int xDst, int yDst, int dx, int dy,
            //           IBitmap *pSrc, int xSrc, int ySrc, AEERasterOp rop)
            "BltIn" => {
                let dst = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let src = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(this, dst, size, src, origin, rop);
                SUCCESS
            }
            // BltOut inverte os papéis: a fonte é `po` e o destino vem no argumento.
            "BltOut" => {
                let dst_pos = (
                    self.cpu.read_reg(Reg::R1) as i32,
                    self.cpu.read_reg(Reg::R2) as i32,
                );
                let size = (self.cpu.read_reg(Reg::R3) as i32, self.stack_arg(0)? as i32);
                let dst = self.stack_arg(1)?;
                let origin = (self.stack_arg(2)? as i32, self.stack_arg(3)? as i32);
                let rop = self.stack_arg(4)?;
                self.blit(dst, dst_pos, size, this, origin, rop);
                SUCCESS
            }
            // int GetInfo(IBitmap *po, AEEBitmapInfo *pinfo, int nSize)
            "GetInfo" => {
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    let (cx, cy) = self
                        .bitmaps
                        .get(&this)
                        .map(|fb| (fb.width(), fb.height()))
                        .unwrap_or((0, 0));
                    self.cpu.write_u32(out, cx)?;
                    self.cpu.write_u32(out + 4, cy)?;
                    self.cpu.write_u32(out + 8, COLOR_DEPTH as u32)?;
                }
                SUCCESS
            }
            // int CreateCompatibleBitmap(IBitmap *po, IBitmap **ppIBitmap, uint16 w, uint16 h)
            "CreateCompatibleBitmap" => {
                let out = self.cpu.read_reg(Reg::R1);
                let width = self.cpu.read_reg(Reg::R2) & 0xffff;
                let height = self.cpu.read_reg(Reg::R3) & 0xffff;
                let addr = self.new_object(Interface::Bitmap)?;
                if addr == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.bitmaps.insert(addr, Framebuffer::new(width, height));
                if out != 0 {
                    self.cpu.write_u32(out, addr)?;
                }
                SUCCESS
            }
            "SetTransparencyColor" => {
                self.transparency
                    .insert(this, self.cpu.read_reg(Reg::R1) as u16);
                SUCCESS
            }
            "GetTransparencyColor" => {
                let value = self.transparency.get(&this).copied().unwrap_or(0);
                let out = self.cpu.read_reg(Reg::R1);
                if out != 0 {
                    self.cpu.write_u32(out, value as u32)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `IImage` sobre o decodificador de PNG (`AEECLSID_PNG` = `0x01004004`), de
    /// `inc/AEEIImage.h`.
    ///
    /// O jogo alimenta a imagem com um `IMemAStream` (`SetStream`), lê as dimensões com
    /// `GetInfo` e desenha com `Draw`. A decodificação em si é PNG padrão — não há nada de
    /// proprietário aqui, ao contrário do que os bytes de alta entropia do `resources.dat`
    /// sugeriam à primeira vista.
    fn image_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Image.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.images.remove(&this);
                    self.image_bitmaps.remove(&this);
                }
                remaining
            }
            // void SetStream(IImage *, IAStream *ps) — consome o stream inteiro de uma vez.
            "SetStream" => {
                self.decode_image(this, a1)?;
                SUCCESS
            }
            // void GetInfo(IImage *, AEEImageInfo *pi): cx, cy, nColors (uint16), bAnimated
            // (boolean) e cxFrame (uint16).
            "GetInfo" => {
                let (cx, cy, frame) = match self.images.get(&this) {
                    Some(image) => (image.width as u16, image.height as u16, image.frame_width),
                    None => (0, 0, 0),
                };
                if a1 != 0 {
                    self.cpu.write_mem(a1, &cx.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 2, &cy.to_le_bytes())?;
                    // `nColors` é zero para imagens com mais de 65535 cores, que é o caso.
                    self.cpu.write_mem(a1 + 4, &0u16.to_le_bytes())?;
                    self.cpu.write_mem(a1 + 6, &[0u8, 0])?;
                    self.cpu.write_mem(a1 + 8, &frame.to_le_bytes())?;
                }
                SUCCESS
            }
            // void SetParm(IImage *, int nParm, int p1, int p2)
            "SetParm" => {
                self.image_set_parm(this, a1, a2, a3)?;
                SUCCESS
            }
            "Draw" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            // void DrawFrame(IImage *, int nFrame, int x, int y)
            "DrawFrame" => {
                self.draw_image(this, a2 as i32, a3 as i32, Some(a1))?;
                SUCCESS
            }
            // Sem animação, `Start` é um `Draw` e `Stop` não tem o que parar.
            "Start" => {
                self.draw_image(this, a1 as i32, a2 as i32, None)?;
                SUCCESS
            }
            "Stop" | "HandleEvent" => SUCCESS,
            // void Notify(IImage *, PFNIMAGEINFO pfn, void *pUser)
            //
            "Notify" => {
                if a1 != 0 {
                    self.image_notify.insert(
                        this,
                        Callback {
                            function: a1,
                            context: a2,
                        },
                    );
                    // Uma imagem vinda do `LoadResObject` já está pronta quando o jogo
                    // registra o callback: a notificação é imediata, não fica esperando
                    // stream nenhum.
                    if self.images.contains_key(&this) {
                        self.notify_image(this)?;
                    }
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê o stream inteiro e decodifica o PNG.
    fn decode_image(&mut self, image: u32, stream: u32) -> Result<(), CpuError> {
        let Some(source) = self.streams.get(&stream).copied() else {
            return Ok(());
        };
        let bytes = self.read_bytes(source.buffer, source.size)?;
        match decode_png(&bytes) {
            Some(decoded) => {
                self.images.insert(image, std::rc::Rc::new(decoded));
            }
            None => {
                self.assumptions
                    .insert("um PNG do jogo foi recusado pelo decodificador");
            }
        }
        self.notify_image(image)
    }

    /// Enfileira o `PFNIMAGEINFO(pUser, IImage *, AEEImageInfo *, int nErr)` da imagem.
    ///
    /// A imagem já está pronta quando o stream chega — não há decodificação em segundo plano
    /// aqui —, mas o jogo espera a notificação para seguir carregando.
    fn notify_image(&mut self, image: u32) -> Result<(), CpuError> {
        let Some(&callback) = self.image_notify.get(&image) else {
            return Ok(());
        };
        let decoded = self.images.get(&image).cloned();
        // `AEEImageInfo` tem 10 bytes; alocamos 12 para manter o alinhamento.
        let info = self.heap.alloc(12).unwrap_or(0);
        if info != 0 {
            self.cpu.write_mem(info, &[0u8; 12])?;
            if let Some(image) = &decoded {
                self.cpu
                    .write_mem(info, &(image.width as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 2, &(image.height as u16).to_le_bytes())?;
                self.cpu
                    .write_mem(info + 8, &image.frame_width.to_le_bytes())?;
            }
        }
        let error = if decoded.is_some() { SUCCESS } else { EFAILED };
        self.pending_calls.push(GuestCall {
            function: callback.function,
            args: [callback.context, image, info, error],
        });
        Ok(())
    }

    /// `IPARM_*` de `inc/AEEIImage.h`. Só respondemos aos que mudam o desenho.
    fn image_set_parm(&mut self, image: u32, parm: u32, p1: u32, p2: u32) -> Result<(), CpuError> {
        match parm {
            IPARM_CXFRAME => {
                if let Some(info) = self.images.get_mut(&image) {
                    std::rc::Rc::make_mut(info).frame_width = p1 as u16;
                }
            }
            IPARM_NFRAMES => {
                if let Some(info) = self.images.get_mut(&image) {
                    let frames = (p1 as u16).max(1);
                    let width = info.width as u16;
                    std::rc::Rc::make_mut(info).frame_width = width / frames;
                }
            }
            // p1 = ponteiro para receber o `IBitmap *`, p2 = ponteiro para o código de retorno.
            IPARM_GETBITMAP => {
                let bitmap = self.bitmap_from_image(image)?;
                if p1 != 0 {
                    self.cpu.write_u32(p1, bitmap)?;
                }
                if p2 != 0 {
                    self.cpu
                        .write_u32(p2, if bitmap == 0 { EFAILED } else { SUCCESS })?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Materializa a imagem decodificada como uma superfície `IBitmap`.
    /// Diferente do [`Machine::bitmap_from_decoded`], este caminho **não** publica o `IDIB`.
    ///
    /// Publicar custa caro onde não é preciso: toda superfície publicada entra no laço que
    /// sincroniza os pixels a cada chamada que os toca, e o Pac-Mania — que desenha pixel a
    /// pixel pela API — saiu de "roda" para "lento demais" quando os dois caminhos foram
    /// unificados. Quem pede o DIB pede pelo `QueryInterface`, e aí ele é publicado.
    fn bitmap_from_image(&mut self, image: u32) -> Result<u32, CpuError> {
        // A mesma imagem é pedida de novo a cada quadro — o Pac-Mania faz 34 mil
        // `IPARM_GETBITMAP` em quatro segundos virtuais. Materializar uma superfície nova a
        // cada pedido gastava sete segundos e deixava trinta e quatro mil objetos vivos.
        if let Some(&bitmap) = self.image_bitmaps.get(&image) {
            return Ok(bitmap);
        }
        let Some(info) = self.images.get(&image).cloned() else {
            return Ok(0);
        };
        let addr = self.new_object(Interface::Bitmap)?;
        if addr == 0 {
            return Ok(0);
        }
        let mut surface = Framebuffer::new(info.width, info.height);
        for (index, pixel) in info.pixels.iter().enumerate() {
            let (x, y) = (index as u32 % info.width, index as u32 / info.width);
            surface.set_pixel_native(x as i32, y as i32, *pixel);
        }
        self.bitmaps.insert(addr, surface);
        self.image_bitmaps.insert(image, addr);
        Ok(addr)
    }

    /// Desenha a imagem na superfície corrente.
    ///
    /// Quando o destino é uma superfície do próprio jogo, o desenho não pode ser feito aqui:
    /// ele vira uma chamada ao `BltIn` dela, e entrar no guest no meio do despacho não é
    /// seguro. Vai para a fila da fronteira, como os callbacks.
    fn draw_image(
        &mut self,
        image: u32,
        x: i32,
        y: i32,
        frame: Option<u32>,
    ) -> Result<(), CpuError> {
        let Some(info) = self.images.get(&image).cloned() else {
            return Ok(());
        };
        let target = self.target()?;
        if !self.bitmaps.contains_key(&target) {
            self.pending_blits.push(PendingBlit {
                image,
                target,
                x,
                y,
                frame,
            });
            return Ok(());
        }
        let clip = self.clip;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(());
        };
        let (frame_width, offset) = match (frame, info.frame_width) {
            (Some(n), width) if width > 0 => (width as u32, n * width as u32),
            _ => (info.width, 0),
        };
        // O recorte não é acabamento aqui: é o que decide o tamanho do trabalho. O Pac-Mania
        // desenha a **folha de fontes inteira** e conta com o recorte para que só a letra
        // apareça. Percorrer a imagem toda e conferir pixel a pixel eram 3,9 bilhões de pixels
        // lidos em quatro segundos virtuais para pôr na tela algumas centenas de milhares — e
        // ainda punha na tela o que o jogo mandou esconder.
        let (mut first_column, mut last_column) = (0, frame_width as i32);
        let (mut first_row, mut last_row) = (0, info.height as i32);
        if let Some(clip) = clip {
            first_column = first_column.max(clip.x as i32 - x);
            last_column = last_column.min(clip.x as i32 + clip.width as i32 - x);
            first_row = first_row.max(clip.y as i32 - y);
            last_row = last_row.min(clip.y as i32 + clip.height as i32 - y);
        }
        // O mesmo vale para as bordas da superfície: o que cai fora nunca precisou ser lido.
        first_column = first_column.max(-x).max(0);
        last_column = last_column.min(surface.width() as i32 - x);
        first_row = first_row.max(-y).max(0);
        last_row = last_row.min(surface.height() as i32 - y);

        for row in first_row..last_row {
            for column in first_column..last_column {
                let source = (row as u32 * info.width + column as u32 + offset) as usize;
                if let Some(&pixel) = info.pixels.get(source) {
                    if info.opaque.get(source).copied().unwrap_or(true) {
                        surface.set_pixel_native(x + column, y + row, pixel);
                    }
                }
            }
        }
        Ok(())
    }

    /// `IUnzipAStream` (`AEECLSID_UNZIPSTREAM` = `0x01001014`), de `sdk/inc/AEEUnzipStream.h`.
    ///
    /// Estende o `IAStream` para ler um stream comprimido como se fosse texto claro: o jogo
    /// monta um stream sobre os bytes comprimidos, entrega em `SetStream`, e lê daqui. É o que
    /// o Double Dragon usa para abrir o `data.ggz` e o `sound.ggz`.
    ///
    /// A descompressão é feita **de uma vez**, na primeira leitura, e o resultado fica guardado.
    /// O BREW descomprime conforme se lê, mas o efeito visível é o mesmo, e fazer de uma vez
    /// dispensa manter estado de inflate parcial entre chamadas.
    fn unzip_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::UnzipStream.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.unzips.remove(&this);
                }
                remaining
            }
            // void SetStream(IUnzipAStream *, IAStream *pInIAStream)
            "SetStream" => {
                self.unzips.insert(
                    this,
                    UnzipState {
                        source: a1,
                        ..UnzipState::default()
                    },
                );
                SUCCESS
            }
            // O conteúdo já está inteiro na memória, então há sempre o que ler.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                self.expand_unzip(this)?;
                let Some(state) = self.unzips.get(&this) else {
                    return Ok(Some(EFAILED));
                };
                let want = (a2 as usize).min(state.output.len().saturating_sub(state.position));
                if want > 0 {
                    let at = state.position;
                    let bytes = state.output[at..at + want].to_vec();
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(state) = self.unzips.get_mut(&this) {
                        state.position += want;
                    }
                }
                want as u32
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê **tudo** o que resta de um `IAStream` do guest.
    ///
    /// `None` quando o objeto não é um stream que saibamos ler por dentro — o que é diferente
    /// de um erro de leitura, que vem como `Some(Err(..))`.
    fn drain_stream(&mut self, source: u32) -> Option<Result<Vec<u8>, CpuError>> {
        if let Some(stream) = self.streams.get(&source).copied() {
            let available = stream.size.saturating_sub(stream.position);
            return Some(self.read_bytes(stream.buffer + stream.position, available));
        }
        // Um `IFile` aberto também é um `IAStream`, e é assim que o Double Dragon entrega o
        // `data.ggz`: lemos da posição corrente até o fim, como faria uma sequência de `Read`.
        let open = self.open_files.get_mut(&source)?;
        let mut bytes = Vec::new();
        Some(
            match std::io::Read::read_to_end(&mut open.file, &mut bytes) {
                Ok(_) => Ok(bytes),
                Err(_) => Ok(Vec::new()),
            },
        )
    }

    /// Descomprime a entrada de um `IUnzipAStream`, uma vez só.
    fn expand_unzip(&mut self, this: u32) -> Result<(), CpuError> {
        let Some(state) = self.unzips.get(&this) else {
            return Ok(());
        };
        if state.expanded {
            return Ok(());
        }
        let source = state.source;
        if let Some(state) = self.unzips.get_mut(&this) {
            state.expanded = true;
        }

        // A entrada é um `IAStream`, e no BREW tanto um bloco de memória quanto um arquivo
        // aberto servem como um. O Double Dragon entrega o `IFile` do `data.ggz` direto.
        let Some(compressed) = self.drain_stream(source) else {
            // Dizer *qual* interface chegou é o que transforma isto de "não funcionou" em uma
            // pista: a próxima origem a suportar é a que aparecer aqui.
            let kind = self
                .objects
                .kind_of(source)
                .map(Interface::name)
                .unwrap_or("desconhecida");
            self.bad_pointers.insert(format!(
                "um IUnzipAStream recebeu como entrada um objeto de {kind}, que ainda não sabemos ler"
            ));
            return Ok(());
        };
        let compressed = compressed?;
        match inflate(&compressed) {
            Some(output) => {
                if let Some(state) = self.unzips.get_mut(&this) {
                    state.output = output;
                }
                // O stream de entrada foi consumido inteiro.
                if let Some(stream) = self.streams.get_mut(&source) {
                    stream.position = stream.size;
                }
            }
            None => {
                self.assumptions
                    .insert("um IUnzipAStream recebeu dados que não descomprimem");
            }
        }
        Ok(())
    }

    /// `IMemAStream` (`AEECLSID_MEMASTREAM` = `0x0100100c`), de `sdk/inc/AEE.h`.
    ///
    /// Apresenta um bloco de memória como stream: é assim que o BREW entrega dados a um
    /// decodificador de imagem. Aqui o "assíncrono" é trivial — os bytes já estão todos na
    /// memória, então `Readable` pode avisar o interessado na mesma hora.
    fn stream_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::MemAStream.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.streams.remove(&this);
                }
                remaining
            }
            // void Set(IMemAStream *, byte *pBuff, uint32 dwSize, uint32 dwOffset,
            //          boolean bSysMem)
            "Set" | "SetEx" => {
                self.streams.insert(
                    this,
                    MemStream {
                        buffer: a1,
                        size: a2,
                        position: a3.min(a2),
                    },
                );
                SUCCESS
            }
            // void Readable(IAStream *, void (*pfnNotify)(void *), void *pUser)
            //
            // O stream está sempre pronto, então o callback vai direto para a fila — entregá-lo
            // aqui seria reentrar no guest no meio do despacho.
            "Readable" => {
                if a1 != 0 {
                    self.pending_signals.push(Callback {
                        function: a1,
                        context: a2,
                    });
                }
                SUCCESS
            }
            // int32 Read(IAStream *, void *pDest, uint32 nWant)
            "Read" => {
                let Some(stream) = self.streams.get(&this).copied() else {
                    return Ok(Some(EFAILED));
                };
                let want = a2.min(stream.size.saturating_sub(stream.position));
                if want > 0 {
                    let bytes = self.read_bytes(stream.buffer + stream.position, want)?;
                    self.cpu.write_mem(a1, &bytes)?;
                    if let Some(stream) = self.streams.get_mut(&this) {
                        stream.position += want;
                    }
                }
                want
            }
            "Cancel" => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ILicense` (`AEECLSID_LICENSE` = `0x0100100f`), do `QINTERFACE(ILicense)` em
    /// `sdk/inc/AEELicense.h`.
    ///
    /// Responde o que vale para uma ROM legitimamente comprada e instalada no console:
    /// licença sem expiração (`LT_NONE`) e compra definitiva (`PT_PURCHASE`). Não é
    /// contornar verificação nenhuma — é a resposta que o próprio aparelho daria para um
    /// jogo que o dono comprou.
    /// Rede e criptografia: `IWeb`, `IHash` (`AEECLSID_MD5`), `ICipherFactory` e `ICipher1`.
    ///
    /// O Boomerang Sports Dodgeball cria os três em sequência e **não confere o retorno**: se a
    /// primeira criação falha, ele pula as outras duas e usa o ponteiro que nunca foi escrito.
    /// Por isso os objetos precisam existir mesmo com o console offline — recusá-los derrubava
    /// o jogo antes da primeira tela.
    fn crypto_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "QueryInterface" => {
                let out = self.cpu.read_reg(Reg::R2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateCipher(ICipherFactory *, AEECLSID cipher, int direction,
            //                  AEECLSID mode, int padding, ICipher1 **ppCipher)
            "CreateCipher" => {
                let out = self.stack_arg(1)?;
                let cipher = self.new_object(Interface::Cipher)?;
                if cipher == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let padding = self.stack_arg(0)?;
                self.ciphers.insert(
                    cipher,
                    CipherState {
                        padding,
                        ..Default::default()
                    },
                );
                if out != 0 {
                    self.cpu.write_u32(out, cipher)?;
                }
                SUCCESS
            }
            // void IHASH_Reset(IHash *) — recomeça o resumo do zero.
            "Reset" => {
                self.hashes.insert(this, HashState::default());
                SUCCESS
            }
            // void IHASH_Update(IHash *, const byte *pData, int nLen)
            "Update" => {
                let (src, len) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let bytes = match len {
                    0 => Vec::new(),
                    _ => self.read_bytes(src, len)?,
                };
                self.hashes.entry(this).or_default().md5.update(&bytes);
                SUCCESS
            }
            // void GetDigest(IHash *, byte *pBuf, int *pnLen)
            //
            // O chamador zera um buffer de 33 bytes e põe 17 no tamanho — dezesseis bytes de
            // resumo e o terminador. Escrevemos os dezesseis e devolvemos dezesseis no tamanho,
            // respeitando o teto que ele pediu.
            "GetDigest" => {
                let (destino, tamanho) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let resumo = self.hashes.entry(this).or_default().md5.clone().finish();
                let cabe = match tamanho {
                    0 => resumo.len() as u32,
                    p => self
                        .cpu
                        .read_u32(p)
                        .unwrap_or(0)
                        .min(resumo.len() as u32 + 1),
                };
                let escrever = cabe.min(resumo.len() as u32) as usize;
                if destino != 0 {
                    self.cpu.write_mem(destino, &resumo[..escrever])?;
                }
                if tamanho != 0 {
                    self.cpu.write_u32(tamanho, escrever as u32)?;
                }
                SUCCESS
            }
            // int QueryCipher(ICipherFactory *, AEECLSID cipher, AEECLSID mode, int padding,
            //                 unsigned keysize)
            // void IWEB_GetResponse(IWeb *po, IWeb *po, IWebResp **ppResp, AEECallback *pcb,
            //                       const char *pszUrl, ...)
            //
            // A macro do SDK repete o `po` como primeiro vararg, então o `r1` é o próprio
            // objeto, o `r2` é o ponteiro de saída da resposta, o `r3` é o callback e a URL vem
            // da pilha. As opções seguem depois, terminadas por `WEBOPT_END`.
            //
            // Por ora isto **observa e recusa**. Observar primeiro é deliberado: o formato dos
            // varargs não está em header nenhum que tenhamos, e implementar HTTP contra um
            // palpite de layout daria um cliente que erra o endereço sem dizer. Registrado o
            // que o jogo pede, o passo seguinte é atender de verdade — e aí dar acesso à rede a
            // um binário de origem externa é decisão de projeto, com autorização explícita.
            "GetResponse" => {
                let (r1, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let url_ptr = self.stack_arg(0)?;
                let url = match url_ptr {
                    0 => String::new(),
                    p => self.cpu.read_cstring(p, MAX_STRING),
                };
                self.web_requests.insert(match url.is_empty() {
                    true => format!("(sem URL) r1={r1:#x} r2={saida:#x}"),
                    false => url,
                });
                // Ponteiro de saída zerado: o jogo precisa ver que não há resposta, em vez de
                // seguir com lixo.
                if saida != 0 {
                    self.cpu.write_u32(saida, 0)?;
                }
                SUCCESS
            }
            "QueryCipher" => SUCCESS,
            // int AddOpt(IWeb *, WebOpt *apWebOpt) — cabeçalhos, tempo limite e afins. Aceitar
            // não custa nada: quem decide o destino da requisição é o `GetResponse`.
            //
            // O `AddOptBuffer` é o slot 6, que no firmware monta um descritor na pilha e chama
            // esta mesma função. Aceitar os dois é a mesma decisão.
            "AddOpt" | "AddOptBuffer" => SUCCESS,
            // Os slots do `IWeb` que a vtable do firmware mostra existir e que ainda não
            // apareceram em uso. Responder sucesso os deixa aparecer no relatório de chamadas em
            // vez de derrubar o jogo — que foi como o slot 6 foi encontrado.
            "slot4" | "slot5" | "slot7" | "slot8" | "slot9" | "slot10" | "slot12"
                if iface == Interface::Web =>
            {
                SUCCESS
            }
            // int SetParam(ICipher1 *, int nId, const void *pParam, unsigned uParamLen)
            "SetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let Some(state) = self.ciphers.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                match id {
                    CIPHER_PARAM_KEY | CIPHER_PARAM_IV => {
                        if len as usize != AES_BLOCK {
                            return Ok(Some(EBADPARM));
                        }
                        let mut bytes = [0u8; AES_BLOCK];
                        self.cpu.read_mem(param, &mut bytes)?;
                        let state = self.ciphers.get_mut(&this).expect("acabou de ser achado");
                        if id == CIPHER_PARAM_KEY {
                            state.key = Some(bytes);
                        } else {
                            state.iv = bytes;
                        }
                        SUCCESS
                    }
                    CIPHER_PARAM_PADDING => {
                        state.padding = self.cpu.read_u32(param)?;
                        SUCCESS
                    }
                    // A direção e o modo já vieram no `CreateCipher`; repeti-los não muda nada,
                    // e recusar faria o jogo desistir.
                    CIPHER_PARAM_DIRECTION | CIPHER_PARAM_MODE => SUCCESS,
                    _ => EUNSUPPORTED,
                }
            }
            // int GetParam(ICipher1 *, int nId, void *pParam, unsigned *puParamLen)
            "GetParam" => {
                let (id, param, len) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                let value = match id {
                    CIPHER_PARAM_KEY_SIZE | CIPHER_PARAM_IV_SIZE | CIPHER_PARAM_BLOCKSIZE => {
                        AES_BLOCK as u32
                    }
                    _ => return Ok(Some(EUNSUPPORTED)),
                };
                if param != 0 {
                    self.cpu.write_u32(param, value)?;
                }
                if len != 0 {
                    self.cpu.write_u32(len, 4)?;
                }
                SUCCESS
            }
            // int Process(ICipher1 *, const byte *pbIn, unsigned cbIn, byte *pbOut,
            //             unsigned *pcbOut)
            "Process" | "ProcessLast" => self.cipher_process(this, name == "ProcessLast")?,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// `ICipher1_Process` e `ICipher1_ProcessLast`, que só diferem no fecho.
    ///
    /// O `ICipher1` é de fluxo: o jogo entrega quantos bytes quiser, e o que não completa um
    /// bloco fica guardado para a chamada seguinte. Só o `ProcessLast` preenche o bloco que
    /// faltou, do jeito que o `padding` pedir.
    fn cipher_process(&mut self, this: u32, last: bool) -> Result<u32, CpuError> {
        // `Process` tem os cinco argumentos; `ProcessLast` só tem a saída e o tamanho dela.
        let (input, count, out, out_len) = match last {
            false => (
                self.cpu.read_reg(Reg::R1),
                self.cpu.read_reg(Reg::R2) as usize,
                self.cpu.read_reg(Reg::R3),
                self.stack_arg(0)?,
            ),
            true => (0, 0, self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2)),
        };
        let Some(state) = self.ciphers.get(&this) else {
            return Ok(EBADPARM);
        };
        let Some(key) = state.key else {
            return Ok(EBADPARM);
        };

        let mut data = state.pending.clone();
        if count > 0 {
            let mut bytes = vec![0u8; count];
            self.cpu.read_mem(input, &mut bytes)?;
            data.extend_from_slice(&bytes);
        }
        if last {
            match state.padding {
                CIPHER_PADDING_NONE if data.len() % AES_BLOCK != 0 => return Ok(EBADPARM),
                CIPHER_PADDING_NONE => {}
                // `CIPHER_PADDING_ZERO` e o que não conhecemos completam com zeros: é o
                // preenchimento que o BREW usa por omissão nos cifradores de bloco.
                _ => data.resize(data.len().div_ceil(AES_BLOCK) * AES_BLOCK, 0),
            }
        }
        // O que o jogo cifra é registrado em claro, e é a coisa mais útil que este método faz
        // para quem estuda protocolo: o corpo que sai pela rede vai cifrado, e decifrá-lo do
        // outro lado exige ter a chave e acertar o modo. Aqui ele passa por nós antes disso.
        if !data.is_empty() {
            if self.plaintexts.len() == PLAINTEXT_MAX {
                self.plaintexts.pop_front();
            }
            self.plaintexts
                .push_back(data[..data.len().min(PLAINTEXT_BYTES)].to_vec());
        }
        // O que não fecha um bloco espera a próxima chamada.
        let whole = data.len() / AES_BLOCK * AES_BLOCK;
        let leftover = data.split_off(whole);

        let mut iv = state.iv;
        crypto::cbc_encrypt(&crypto::Aes128::new(&key), &mut iv, &mut data);
        if let Some(state) = self.ciphers.get_mut(&this) {
            state.iv = iv;
            state.pending = leftover;
        }

        // A capacidade oferecida chega no mesmo ponteiro que devolve quanto foi escrito.
        let capacity = match out_len {
            0 => data.len(),
            addr => self.cpu.read_u32(addr)? as usize,
        };
        if capacity < data.len() {
            if out_len != 0 {
                self.cpu.write_u32(out_len, data.len() as u32)?;
            }
            return Ok(EBUFFERTOOSMALL);
        }
        if out != 0 && !data.is_empty() {
            self.cpu.write_mem(out, &data)?;
        }
        if out_len != 0 {
            self.cpu.write_u32(out_len, data.len() as u32)?;
        }
        Ok(SUCCESS)
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

    /// `ISound` (`AEECLSID_SOUND` = `0x01001056`), de `inc/AEEISound.h`.
    ///
    /// É a API de som *básica* do BREW: tons do gerador do aparelho, vibração e volume — não
    /// toca arquivos, isso é o `ISoundPlayer`. Aqui ela é silenciosa: aceita tudo, guarda o
    /// estado que o jogo pode ler de volta e avisa o callback de que a reprodução terminou.
    /// Sem isso os jogos que checam o retorno de `Set` desistem da inicialização.
    /// `IMediaUtil` e `IMedia`, de `sdk/inc/AEEMediaUtil.h` e `inc/AEEIMedia.h`.
    ///
    /// Mudos, como o `ISound`: guardam o estado de reprodução e respondem o que o jogo espera,
    /// mas não sai som. O que importa aqui é existir — o Quake cria o tocador de trilha na
    /// inicialização do áudio e, se ela falha, ele segue em frente e depois chama `Play` num
    /// ponteiro nulo, sem conferir.
    fn media_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.arg(0);
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.media.remove(&this);
                }
                remaining
            }
            (_, "QueryInterface") => {
                let out = self.arg(2);
                if out != 0 {
                    self.cpu.write_u32(out, this)?;
                }
                SUCCESS
            }
            // int CreateMedia(IMediaUtil *, AEEMediaData *, IMedia **ppm)
            (Interface::MediaUtil, "CreateMedia" | "CreateMediaEx") => {
                let out = self.arg(2);
                let media = self.new_object(Interface::Media)?;
                if media == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let mut state = MediaState::default();
                self.read_media_data(self.arg(1), &mut state)?;
                self.media.insert(media, state);
                if out != 0 {
                    self.cpu.write_u32(out, media)?;
                }
                SUCCESS
            }
            // int SetMediaParm(IMedia *, int16 nParmID, int32 p1, int32 p2)
            (Interface::Media, "SetMediaParm") => {
                let (parm, p1) = (self.arg(1), self.arg(2));
                let mut state = self
                    .media
                    .get(&this)
                    .copied()
                    .unwrap_or(MediaState::default());
                match parm {
                    MM_PARM_MEDIA_DATA => self.read_media_data(p1, &mut state)?,
                    MM_PARM_VOLUME => state.volume = p1.min(MAX_VOLUME),
                    MM_PARM_MUTE => state.muted = p1 != 0,
                    MM_PARM_PLAY_REPEAT => state.repeat = p1,
                    // O resto do `MM_PARM_XXX` é aceito e ignorado: recusar faria jogos
                    // desistirem de tocar por causa de um ajuste que não muda o som.
                    _ => {}
                }
                self.media.insert(this, state);
                if let (Some(mixer), MM_PARM_VOLUME | MM_PARM_MUTE) = (&self.audio, parm) {
                    mixer.set_volume(this, state.gain());
                }
                SUCCESS
            }
            // void RegisterNotify(IMedia *, PFNMEDIANOTIFY pfn, void *pUser)
            (Interface::Media, "RegisterNotify") => {
                let notify = Callback {
                    function: self.arg(1),
                    context: self.arg(2),
                };
                self.media.entry(this).or_default().notify = notify;
                SUCCESS
            }
            (Interface::Media, "Play") => self.media_play(this)?,
            (Interface::Media, "Resume") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, false);
                }
                SUCCESS
            }
            (Interface::Media, "Pause") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_PLAY_PAUSE;
                }
                if let Some(mixer) = &self.audio {
                    mixer.set_paused(this, true);
                }
                SUCCESS
            }
            (Interface::Media, "Stop") => {
                if let Some(state) = self.media.get_mut(&this) {
                    state.state = MM_STATE_READY;
                }
                if let Some(mixer) = &self.audio {
                    mixer.stop(this);
                }
                SUCCESS
            }
            // int GetState(IMedia *, boolean *pbStateChanging) — o estado é o retorno, e o
            // ponteiro diz se ele está em transição. Aqui nunca está: a mudança é imediata.
            (Interface::Media, "GetState") => {
                let out = self.arg(1);
                if out != 0 {
                    self.cpu.write_mem(out, &[0])?;
                }
                // Um som que acabou volta o objeto para "pronto": é o que o jogo consulta
                // para saber que o som terminou. Quem diz que acabou é o **relógio virtual**,
                // não o mixer — pelo mesmo motivo que em `media_play`: o emulador roda mudo
                // sem deixar de contar o tempo, e há som que toca em silêncio porque não
                // sabemos decodificá-lo, só cronometrá-lo.
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                if state.state == MM_STATE_PLAY && now >= state.ends_us {
                    state.state = MM_STATE_READY;
                }
                state.state
            }
            // int32 GetTotalTime(IMedia *) — a duração do som, em milissegundos.
            //
            // O som que toca em silêncio responde a duração dele como qualquer outro. Zero aqui
            // é um divisor esperando acontecer: quem monta uma barra de progresso divide pelo
            // total, e um total zero derruba o jogo por uma resposta nossa.
            (Interface::Media, "GetTotalTime") => match self.media_sound(this)? {
                Some(sound) => (sound.frames() as u64 * 1000 / u64::from(sound.rate.max(1))) as u32,
                None => (self.media_silent_length(this)?.unwrap_or(0) / 1000) as u32,
            },
            (Interface::Media, "GetMediaParm") => {
                for index in [2, 3] {
                    let out = self.arg(index);
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                SUCCESS
            }
            (Interface::Media, _) => SUCCESS,
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Lê um `AEEMediaData` do guest para o estado do objeto.
    ///
    /// A struct é `{ AEECLSID clsData; void *pData; uint32 dwSize; }`. Só a variante de memória
    /// interessa: os três jogos que tocam som passam `MMD_BUFFER` com um RIFF já carregado.
    fn read_media_data(&mut self, pointer: u32, state: &mut MediaState) -> Result<(), CpuError> {
        if pointer == 0 {
            return Ok(());
        }
        let class = self.cpu.read_u32(pointer)?;
        let data = self.cpu.read_u32(pointer + 4)?;
        let size = self.cpu.read_u32(pointer + 8)?;
        if class != MMD_BUFFER {
            self.bad_pointers.insert(format!(
                "uma mídia foi entregue como {class:#010x}, e só sabemos ler buffer de memória"
            ));
            return Ok(());
        }
        (state.buffer, state.size) = (data, size);
        Ok(())
    }

    /// O som de um objeto `IMedia`, lido do buffer do guest e guardado.
    ///
    /// Um RIFF pode ter megabytes — o do Quake tem 1,4 —, e reinterpretá-lo a cada `Play`
    /// seria trabalho repetido a cada tiro disparado.
    fn media_sound(
        &mut self,
        this: u32,
    ) -> Result<Option<std::sync::Arc<crate::wav::Sound>>, CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(None);
        };
        if state.buffer == 0 || state.size == 0 {
            return Ok(None);
        }
        let key = (state.buffer, state.size);
        if let Some(sound) = self.waves.get(&key) {
            return Ok(Some(sound.clone()));
        }
        let bytes = self.read_bytes(state.buffer, state.size)?;
        match crate::wav::parse(&bytes) {
            Ok(sound) => {
                let sound = std::sync::Arc::new(sound);
                self.waves.insert(key, sound.clone());
                Ok(Some(sound))
            }
            Err(err) => {
                // Dizer *qual* formato chegou é o que permite saber o que implementar depois —
                // e "não é um RIFF/WAVE" não diz. O que diz é a assinatura do próprio bloco:
                // é assim que se sabe que a trilha do Tekken 2 é MP3 sem abrir o jogo.
                let formato = detect_mime(&bytes, "").unwrap_or("formato desconhecido");
                self.bad_pointers
                    .insert(format!("som recusado ({formato}): {err}"));
                Ok(None)
            }
        }
    }

    /// Quanto dura um som que não sabemos decodificar, quando dá para descobrir sem decodificar.
    ///
    /// Hoje só o MP3 cai aqui, pelo cabeçalho do primeiro quadro e pela etiqueta do codificador
    /// — ver [`crate::mp3`].
    fn media_silent_length(&mut self, this: u32) -> Result<Option<u64>, CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(None);
        };
        if state.buffer == 0 || state.size == 0 {
            return Ok(None);
        }
        let bytes = self.read_bytes(state.buffer, state.size)?;
        Ok(crate::mp3::probe(&bytes).map(|mp3| mp3.duration_us()))
    }

    /// `int Play(IMedia *)`.
    fn media_play(&mut self, this: u32) -> Result<u32, CpuError> {
        let Some(sound) = self.media_sound(this)? else {
            // Um som que não sabemos ler mas sabemos **cronometrar** toca em silêncio pelo
            // tempo certo. Sem isso o Tekken 2 ficava preso: a música dele é MP3, o `Play`
            // respondia "esse som já acabou", o jogo consultava o estado, via "pronto" e
            // mandava tocar de novo — 766 mil vezes em quatro segundos virtuais, o que o
            // deixava na lista de "lento demais" sem ter trabalho nenhum para fazer.
            if let Some(length_us) = self.media_silent_length(this)? {
                self.assumptions
                    .insert("um som em formato que não decodificamos toca em silêncio, só com a duração certa");
                let now = self.now_us();
                let state = self.media.entry(this).or_default();
                state.state = MM_STATE_PLAY;
                state.ends_us = match state.repeat {
                    0 => u64::MAX,
                    times => now + length_us * u64::from(times),
                };
                self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
                return Ok(SUCCESS);
            }
            // Sem som legível não há o que tocar, mas recusar faria o jogo tratar como erro
            // grave; para ele, o som simplesmente acabou na hora.
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
            return Ok(SUCCESS);
        };
        // Quando o som acaba sai do **relógio virtual**, e não do mixer: um jogo que espera o
        // aviso de fim para tocar o próximo precisa recebê-lo mesmo com o som desligado, ou
        // emudece de vez depois do primeiro efeito.
        let length_us = sound.frames() as u64 * 1_000_000 / u64::from(sound.rate.max(1));
        let now = self.now_us();
        let state = self.media.entry(this).or_default();
        state.state = MM_STATE_PLAY;
        state.ends_us = match state.repeat {
            0 => u64::MAX,
            times => now + length_us * u64::from(times),
        };
        let (gain, repeat) = (state.gain(), state.repeat);
        if let Some(mixer) = &self.audio {
            mixer.play(this, sound, gain, repeat);
        }
        self.notify_media(this, MM_CMD_PLAY, MM_STATUS_START)?;
        Ok(SUCCESS)
    }

    /// Avisa o jogo de que um som chegou ao fim.
    ///
    /// O aviso é o que fecha o ciclo de quem toca uma coisa de cada vez: sem ele o jogo fica
    /// esperando para sempre o efeito anterior terminar, e o som para depois do primeiro.
    fn notify_media(&mut self, this: u32, cmd: u32, status: u32) -> Result<(), CpuError> {
        let Some(state) = self.media.get(&this).copied() else {
            return Ok(());
        };
        if state.notify.function == 0 {
            return Ok(());
        }
        let block = match state.notify_block {
            0 => {
                let block = self.heap.alloc(MEDIA_NOTIFY_LEN).unwrap_or(0);
                if let Some(state) = self.media.get_mut(&this) {
                    state.notify_block = block;
                }
                block
            }
            block => block,
        };
        if block == 0 {
            return Ok(());
        }
        // `AEEMediaCmdNotify`: clsMedia, pIMedia, nCmd, nSubCmd, nStatus, pCmdData, dwSize.
        // A classe vai zerada — o jogo identifica o som pelo ponteiro, não por ela.
        for (index, value) in [0, this, cmd, 0, status, 0, 0].into_iter().enumerate() {
            self.cpu.write_u32(block + index as u32 * 4, value)?;
        }
        self.pending_calls.push(GuestCall {
            function: state.notify.function,
            args: [state.notify.context, block, 0, 0],
        });
        Ok(())
    }

    /// Enfileira o aviso de fim dos sons que já terminaram.
    fn poll_media(&mut self) -> Result<(), CpuError> {
        let now = self.now_us();
        let finished: Vec<u32> = self
            .media
            .iter()
            .filter(|(_, state)| state.state == MM_STATE_PLAY && now >= state.ends_us)
            .map(|(this, _)| *this)
            .collect();
        for this in finished {
            if let Some(state) = self.media.get_mut(&this) {
                state.state = MM_STATE_READY;
                state.ends_us = 0;
            }
            self.notify_media(this, MM_CMD_PLAY, MM_STATUS_DONE)?;
        }
        Ok(())
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
                match self.config_items.get(&this).and_then(|itens| itens.get(&item)) {
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

    /// O filho de um widget guardado sob `id`, criado na primeira vez que alguém o pede.
    ///
    /// Guardar é o que faz o acessador ser coerente consigo mesmo: a `0x78acc` pede o `0x5000`,
    /// configura, pede o `0x5002` e solta os dois no fim. Se cada pedido criasse um objeto
    /// novo, o jogo soltaria objetos que não são os que usou.
    fn filho_do_widget(&mut self, this: u32, id: u32) -> Result<u32, CpuError> {
        if let Some(filho) = self
            .widgets
            .get(&this)
            .and_then(|widget| widget.filhos.get(&id).copied())
        {
            // Entregar é emprestar: quem recebe vai soltar.
            self.objects.add_ref(filho);
            return Ok(filho);
        }
        let filho = self.new_object(Interface::Widget)?;
        if filho == 0 {
            return Ok(0);
        }
        self.widgets.insert(
            filho,
            Widget {
                visivel: true,
                ..Widget::default()
            },
        );
        if let Some(widget) = self.widgets.get_mut(&this) {
            widget.filhos.insert(id, filho);
        }
        // Duas referências: **uma do mapa do pai** e uma de quem pediu. Sem a do pai o objeto
        // morria no primeiro `Release` do chamador e a entrada no mapa ficava apontando para um
        // endereço livre — que, com o alocador reusando endereço, é outro objeto.
        self.objects.add_ref(filho);
        Ok(filho)
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

    /// Atende o widget da Z-Wheel (`0x01028e51`). Ver [`Interface::Widget`].
    ///
    /// **O retorno é invertido**: diferente de zero é sucesso. Os invólucros do jogo
    /// (`0x3f72c` e `0x403c8`) fazem `cmp r0,#0; moveq r0,#3`, transformando zero em erro. Isso
    /// vale só para o acessador; o `AddRef` e o `Release` continuam devolvendo a contagem, como
    /// em todo o BREW.
    ///
    /// O acessador tem dois seletores, e ambos foram lidos no código do jogo:
    ///
    /// - `0x800` **pega o filho** de número `id` e escreve o ponteiro no terceiro argumento. O
    ///   filho é criado na primeira vez e guardado: a `0x78acc` pede o `0x5000`, configura, e
    ///   depois pede o `0x5002`, e ela solta os dois no fim — se cada pedido criasse um objeto
    ///   novo, o jogo soltaria objetos que não são os que usou.
    /// - `0x801` **grava** a propriedade `id`. O que os números querem dizer ainda não sabemos;
    ///   guardá-los custa nada e é o que permitirá reconhecê-los quando a tela aparecer.
    ///
    /// Um seletor que não seja esses dois é recusado com zero em vez de aceito em silêncio: um
    /// terceiro seletor é coisa que precisamos ver, não esconder.
    fn widget_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        /// Lê um item do widget. O que sai depende do número do item — ver abaixo.
        const LE: u32 = 0x800;
        /// A partir daqui, o item guarda um objeto; abaixo, um número.
        const PRIMEIRO_OBJETO: u32 = 0x5000;
        const GRAVA: u32 = 0x801;
        /// Sucesso para esta classe. Não é o `SUCCESS` do BREW — ver acima.
        const OK: u32 = 1;

        let Some(name) = Interface::Widget.method(slot) else {
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
                    // **Quem guarda, solta.** Os filhos que o acessador cria por conta própria
                    // e os que o `AdicionarFilho` pendura levaram uma contagem nossa; sumir com
                    // o widget sem devolvê-la vaza os dois. E vaza rápido: a Z-Wheel em modo de
                    // atração monta e desmonta a abertura sem parar, e o mil e vinte e quatro
                    // objetos da região acabavam numa volta só do laço.
                    if let Some(widget) = self.widgets.remove(&this) {
                        for filho in widget.filhos.into_values().chain(widget.anexados) {
                            self.objects.release(filho);
                        }
                    }
                }
                restantes
            }
            // Zero é sucesso aqui, ao contrário do acessador logo abaixo. Aceitamos qualquer
            // interface pedida porque, no nosso modelo, a família inteira de widgets **é** uma
            // interface só — a hipótese fica registrada, que é onde ela deve estar.
            // O slot 12 tem a mesma forma e a mesma convenção do slot 2. É por ele que a
            // Z-Wheel pega, de dentro do retorno de chamada da imagem em `0x4d4a4`, o objeto
            // em que vai pendurar o GIF de abertura: `slot12(IID, &saída)` e, em seguida,
            // `slot5(saída, imagem)`. Recusá-lo abortava o retorno de chamada inteiro, e a
            // animação nunca começava — sem erro nenhum no log, porque quem abortou fomos nós.
            "QueryInterface" | "PegarInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                self.assumptions
                    .insert("um widget aceitou toda interface que lhe pediram");
                SUCCESS
            }
            // `slot6(this, visível)`, a última coisa que a `0x11750` faz antes de sair da
            // abertura: ela esconde o formulário da animação e vai direto para a transição.
            // O retorno é ignorado.
            "DefinirVisivel" => {
                let visivel = self.cpu.read_reg(Reg::R1) != 0;
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.visivel = visivel;
                }
                SUCCESS
            }
            // `slot8(this, &saída)`, chamado a cada tique da abertura pela `0x11578`. O que
            // sai dali recebe em seguida um `slot3(widget, 0, 0)` e é solto — e slot 3 num
            // widget é o acessador. Um ponteiro de widget no lugar do seletor não é seletor
            // nenhum, então a leitura que resta é: **o pai é quem recebe o aviso de que o
            // filho mudou**, e o acessador recusa o que não conhece, que é o que ele já faz.
            //
            // **Leitura sem confirmação.** Enquanto ninguém dá a partida na animação, este
            // slot não é chamado, então não há execução que sustente ou derrube a hipótese.
            // Fica escrita para o próximo que passar aqui.
            //
            // Devolver com contagem, porque quem recebe solta logo depois.
            "PegarPai" => {
                let saida = self.cpu.read_reg(Reg::R1);
                let pai = self.widgets.get(&this).map_or(0, |widget| widget.pai);
                if pai != 0 {
                    self.objects.add_ref(pai);
                }
                if saida != 0 {
                    self.cpu.write_u32(saida, pai)?;
                }
                SUCCESS
            }
            // `slot4(this, &tratador)`, visto em `0x11a6c`. O que `r1` aponta é montado logo
            // acima, em `0x11a60`: o objeto do jogo se põe como contexto em `+0x1c` e o
            // endereço da função em `+0x20`. É um registro de tratador de eventos.
            //
            // Guardamos o endereço e não chamamos ninguém: quem dispararia estes eventos é a
            // interface que ainda não desenhamos. Quando ela existir, o tratador está aqui.
            "DefinirTratador" => {
                let tratador = self.cpu.read_reg(Reg::R1);
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.tratador = tratador;
                }
                SUCCESS
            }
            // `slot5(this, filho, 0, &posição, …)`, visto em `0x11cc0`: o jogo pendura um
            // widget no outro e solta a referência dele em seguida. O retorno é ignorado — a
            // instrução seguinte já sobrescreve `r0`.
            //
            // Aqui o filho é só registrado. Não há árvore de interface para montar enquanto
            // ninguém desenha por ela, e guardar a ligação é o que permite reconhecer, quando
            // isso mudar, que ela já existia.
            "AdicionarFilho" => {
                let filho = self.cpu.read_reg(Reg::R1);
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.anexados.push(filho);
                }
                if let Some(filho) = self.widgets.get_mut(&filho) {
                    filho.pai = this;
                }
                // Quem guarda, segura. É a convenção do BREW inteiro, e aqui ela não é
                // teoria: logo depois de pendurar a imagem no widget, a Z-Wheel solta a
                // referência dela. Sem esta contagem, o objeto morria com a imagem
                // decodificada dentro — e o que sobrava para pintar era nada.
                self.objects.add_ref(filho);
                SUCCESS
            }
            // `slot7(this, &{largura, altura})`, visto em `0x11c90` com `640 × 480` — a tela
            // inteira. O jogo ignora o retorno: a instrução seguinte já sobrescreve `r0`.
            "DefinirTamanho" => {
                // O ponteiro nem sempre é ponteiro. Quando a árvore de widgets fica grande, o
                // jogo chama este slot com `r1` apontando para fora do mapa, e ler dali derruba
                // o núcleo ARM — apareceu como `READ_UNMAPPED` no relatório da interface.
                // Ignorar o que não dá para ler é o certo: um tamanho que não veio é um tamanho
                // que não muda.
                let par = self.cpu.read_reg(Reg::R1);
                let tamanho = match (self.cpu.read_u32(par), self.cpu.read_u32(par + 4)) {
                    (Ok(largura), Ok(altura)) => (largura, altura),
                    _ => return Ok(Some(EBADPARM)),
                };
                if let Some(widget) = self.widgets.get_mut(&this) {
                    widget.tamanho = tamanho;
                }
                SUCCESS
            }
            "Acessador" => {
                let (seletor, id, terceiro) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2),
                    self.cpu.read_reg(Reg::R3),
                );
                match seletor {
                    LE => {
                        // **Nem todo `0x800` pede um filho.** O jogo lê e grava pelo mesmo
                        // seletor coisas de tipos diferentes, e o número do item é que diz
                        // qual: os do intervalo `0x5000` guardam **objetos** — a `0x88338` lê
                        // o `0x5000` e o `0x5002` e usa os dois como widgets —, e os de baixo
                        // são **números**.
                        //
                        // Isto não é dedução: o retorno de chamada da imagem lê o item `0x347`,
                        // soma dois e grava de volta. Enquanto a leitura criava um filho para
                        // qualquer item, o que ele somava dois era um **ponteiro nosso**, e o
                        // que ele gravava em `[formulário+0x24]` pelo item `0x414` era outro.
                        // Ponteiro onde o jogo espera número é o começo de uma sequência de
                        // sintomas que não se parecem com a causa.
                        // Os itens do widget são **tipados**, e o corte medido é `0x5000`: de
                        // lá para cima o item guarda objeto, abaixo guarda número.
                        //
                        // Cheguei a pôr o `0x414` como exceção, achando que ele guardava um
                        // widget: o retorno de chamada da imagem o lê para `[formulário+0x24]`,
                        // e a `0x11750` faz `[r0+0x24]->slot6(1)`. Eram campos **diferentes** —
                        // a `0x11750` recebe o **aplicativo**, não o formulário, e o objeto que
                        // ela solta é outro. O que o `0x414` guarda é número mesmo, e a
                        // `0x11668` prova: ela subtrai um e compara.
                        //
                        // A exceção custou caro enquanto durou. Com um ponteiro ali, a
                        // comparação da `0x11674` nunca era verdadeira e a abertura girava para
                        // sempre: doze milhões de idas ao acessador e seis milhões de
                        // temporizadores numa execução só.
                        let valor = match id >= PRIMEIRO_OBJETO {
                            true => self.filho_do_widget(this, id)?,
                            false => self
                                .widgets
                                .get(&this)
                                .and_then(|widget| widget.propriedades.get(&id).copied())
                                .unwrap_or(0),
                        };
                        if terceiro != 0 {
                            self.cpu.write_u32(terceiro, valor)?;
                        }
                        OK
                    }
                    GRAVA => {
                        if let Some(widget) = self.widgets.get_mut(&this) {
                            widget.propriedades.insert(id, terceiro);
                        }
                        OK
                    }
                    _ => {
                        self.assumptions
                            .insert("um seletor de widget que não conhecemos foi recusado");
                        0
                    }
                }
            }
            _ => OK,
        };
        Ok(Some(result))
    }

    /// Atende a fonte TrueType. Ver [`Interface::Typeface`].
    ///
    /// O único método com corpo é o slot 4, que a `0x7bfc8` chama assim:
    /// `slot4(this, a, b, c, &saída)`, com a saída no **primeiro argumento de pilha** — os
    /// quatro registradores já estão ocupados. Zero é sucesso, e o que sai é o objeto de fonte,
    /// que o jogo entrega ao slot 9 de um contêiner e depois usa.
    ///
    /// Damos um widget. Não é palpite de conveniência: na extensão de interface do console tudo
    /// que entra numa árvore de tela é widget, e o que o jogo faz com o objeto em seguida — um
    /// `AddRef` e um slot 6 — é vocabulário de widget. Se ele pedir algo que um widget não tem,
    /// o slot aparece no relatório, que é como o resto disto foi descoberto.
    fn typeface_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Typeface.method(slot) else {
            return Ok(None);
        };
        if aee::e_marcador(name) {
            return Ok(None);
        }
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "CriarFonte" => {
                let saida = self.stack_arg(0)?;
                let fonte = self.new_object(Interface::Widget)?;
                if fonte == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.widgets.insert(
                    fonte,
                    Widget {
                        visivel: true,
                        ..Widget::default()
                    },
                );
                if saida != 0 {
                    self.cpu.write_u32(saida, fonte)?;
                }
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
                self.cpu.write_mem(info + INTENSIDADE, &SINAL.to_le_bytes())?;
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
                    self.assumptions
                        .insert("uma lista foi esvaziada sem chamar o liberador que o jogo registrou");
                } else if let Some((itens, _)) = self.vetores.get_mut(&this) {
                    itens.clear();
                }
                SUCCESS
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende o `ISource` e o `IPeek`. Ver [`Interface::Peek`].
    ///
    /// Do `IPeek` só o slot 8 tem corpo, e ele é a razão de tudo isto existir: a Z-Wheel lê o
    /// `tectoy.cfg` linha a linha por ele. A chamada é `slot8(this, &par, 3)`, com `par` sendo
    /// `{ponteiro, tamanho}` — o jogo lê os dois e **copia** o texto antes de pedir a próxima
    /// linha, o que é o que permite reaproveitar um buffer só.
    ///
    /// **O valor de retorno é uma hipótese, e ela está medida.** O laço em `0x884fc` continua
    /// enquanto `-retorno >= 2` e para em `-3`; devolver zero o encerraria na primeira linha,
    /// inclusive numa linha vazia. Então: `1` enquanto houver linha, `-3` no fim. Que o console
    /// devolva o mesmo `1` não se sabe — o que se sabe é a condição do laço.
    fn source_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        /// "Acabaram as linhas": o único valor que o laço da `0x88338` aceita como fim.
        const FIM: u32 = (-3i32) as u32;
        /// "Veio linha". Ver a nota sobre o retorno, acima.
        const VEIO: u32 = 1;

        let Some(name) = iface.method(slot) else {
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
                    self.sources.remove(&this);
                    self.peeks.remove(&this);
                }
                restantes
            }
            "QueryInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int32 ISOURCE_Read(ISource *po, char *pcBuf, int32 cbBuf)`.
            "Read" => {
                let (destino, cabe) = (
                    self.cpu.read_reg(Reg::R1),
                    self.cpu.read_reg(Reg::R2) as usize,
                );
                let Some(bytes) = self.sources.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let pedaco = bytes[..bytes.len().min(cabe)].to_vec();
                self.cpu.write_mem(destino, &pedaco)?;
                self.sources.insert(this, bytes[pedaco.len()..].to_vec());
                pedaco.len() as u32
            }
            "LerLinha" => {
                let par = self.cpu.read_reg(Reg::R1);
                let Some(leitor) = self.peeks.get_mut(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let Some(linha) = leitor.proxima_linha() else {
                    return Ok(Some(FIM));
                };
                let (buffer, tamanho) = (leitor.buffer, linha.len() as u32);
                self.cpu.write_mem(buffer, &linha)?;
                self.cpu.write_mem(buffer + tamanho, &[0])?;
                if par != 0 {
                    self.cpu.write_u32(par, buffer)?;
                    self.cpu.write_u32(par + 4, tamanho)?;
                }
                VEIO
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Atende a `ISourceUtil`. Ver [`Interface::SourceUtil`].
    fn source_util_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::SourceUtil.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                self.objects.release(this)
            }
            "QueryInterface" => {
                let saida = self.cpu.read_reg(Reg::R2);
                if saida != 0 {
                    self.cpu.write_u32(saida, this)?;
                }
                self.objects.add_ref(this);
                SUCCESS
            }
            // `int PeekSourceFromSource(ISourceUtil *po, ISource *ps, int nMax, IPeek **ppo)`.
            //
            // O `nMax` é o teto do que o leitor pode manter em memória; a Z-Wheel passa o
            // tamanho do arquivo mais um, ou seja, o arquivo inteiro. Como já temos os bytes
            // todos, ele não muda nada aqui — mas é o que diz que o jogo espera ler tudo.
            "PeekSourceFromSource" => {
                let (fonte, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R3));
                let Some(bytes) = self.sources.get(&fonte).cloned() else {
                    return Ok(Some(EBADPARM));
                };
                let leitor = self.new_object(Interface::Peek)?;
                if leitor == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // O buffer de uma linha vive junto do leitor: o jogo recebe um ponteiro para
                // ele e **copia** o conteúdo antes de pedir a próxima, então um buffer só,
                // reaproveitado, basta. Reservar do tamanho da fonte garante que a maior linha
                // possível caiba.
                let buffer = self.heap.alloc(bytes.len() as u32 + 1).unwrap_or(0);
                if buffer == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.peeks.insert(leitor, Peek { bytes, posicao: 0, buffer });
                if saida != 0 {
                    self.cpu.write_u32(saida, leitor)?;
                }
                SUCCESS
            }
            // `int SourceFromFile(ISourceUtil *po, IFile *pf, ISource **ppo)`.
            //
            // Lemos o arquivo inteiro pelo caminho, e não pelo descritor aberto, para não mexer
            // na posição do `IFile` do jogo — ele continua sendo dele.
            "SourceFromFile" => {
                let (arquivo, saida) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let Some(caminho) = self
                    .open_files
                    .get(&arquivo)
                    .map(|aberto| aberto.guest_path.clone())
                else {
                    return Ok(Some(EBADPARM));
                };
                let Some(bytes) = self
                    .vfs
                    .resolve(&caminho)
                    .and_then(|real| std::fs::read(real).ok())
                else {
                    return Ok(Some(EFAILED));
                };
                let fonte = self.new_object(Interface::Source)?;
                if fonte == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                self.sources.insert(fonte, bytes);
                if saida != 0 {
                    self.cpu.write_u32(saida, fonte)?;
                }
                SUCCESS
            }
            // `int SourceFromMemory(ISourceUtil *po, const void *pBuf, int nSize,
            //                       PFNNOTIFY pfn, void *pUser, ISource **ppo)`.
            //
            // É aqui que a ponte do Zeeboids entra, e vale explicar por quê. O método não
            // envia nada: ele embrulha um pedaço de memória num `ISource` para que a `IWeb`
            // possa lê-lo. Só que o pedaço de memória, no Zeeboids, **é o corpo do POST** — e
            // este é o único ponto em que ele existe inteiro e ainda em claro.
            //
            // No firmware ele registra o par guardado pelo `SetHandler` e enfileira o trabalho
            // no objeto interno. Aqui fazemos o trabalho na hora: `r1` e `r2` são o corpo e o
            // tamanho, e a resposta volta no ponteiro de saída.
            "SourceFromMemory" => {
                let (corpo, tamanho) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
                let saida = self.stack_arg(1)?;
                self.send_request(corpo, tamanho, saida)?
            }
            _ => SUCCESS,
        };
        Ok(Some(result))
    }

    /// Manda o corpo que o jogo entregou ao despachante e guarda a resposta.
    ///
    /// **Achar a URL é o passo delicado.** Ela não vem nos argumentos: fica no objeto do
    /// `ConnectionManager` do jogo, que é quem nos chamou. Em vez de fixar um deslocamento — o
    /// do Zeeboids é `+0x244`, e valeria só para ele —, procuramos no objeto o **trio**
    /// `{url, corpo, tamanho}` cujas duas últimas palavras são exatamente os argumentos que
    /// acabamos de receber. Isso se confere sozinho: um trio que case com o ponteiro e o
    /// tamanho recebidos não é coincidência, e um que não case é descartado.
    ///
    /// O `r4` é do chamador — em ARM ele é preservado pela função chamada, então na fronteira da
    /// chamada ainda guarda o objeto de quem chamou.
    fn send_request(&mut self, corpo: u32, tamanho: u32, saida: u32) -> Result<u32, CpuError> {
        let objeto = self.cpu.read_reg(Reg::R4);
        let mut achado = None;
        for i in 0..MAX_CAMPOS_DO_OBJETO {
            let base = objeto + i * 4;
            if self.cpu.read_u32(base + 4) == Ok(corpo)
                && self.cpu.read_u32(base + 8) == Ok(tamanho)
                && let Ok(ponteiro) = self.cpu.read_u32(base)
            {
                let texto = self.cpu.read_cstring(ponteiro, MAX_STRING);
                if texto.starts_with("http") {
                    achado = Some((texto, base));
                    break;
                }
            }
        }
        let Some((url, base)) = achado else {
            self.assumptions
                .insert("um envio foi recusado: não achei a URL no objeto de quem chamou");
            return Ok(EFAILED);
        };

        let dados = self.read_bytes(corpo, tamanho.min(MAX_CORPO_ENVIADO))?;
        if !self.network {
            self.web_requests
                .insert(format!("{url} ({tamanho} bytes, rede desligada)"));
            return Ok(EFAILED);
        }
        // A área de saída ainda não tem formato conhecido: o que o console punha ali se descobre
        // vendo o jogo ler. Guardamos a resposta e deixamos o ponteiro como está, em vez de
        // escrever um palpite de struct sobre a memória do jogo.
        let _ = saida;
        let (resultado, estado) = match rede::post(&url, &dados, self.network_to.as_deref()) {
            Ok(resposta) => {
                self.web_requests.insert(format!(
                    "{url} -> {} ({} bytes de resposta)",
                    resposta.status,
                    resposta.corpo.len()
                ));
                self.web_response = resposta.corpo;
                (SUCCESS, ESTADO_RECEBENDO)
            }
            Err(erro) => {
                self.web_requests.insert(format!("{url} -> falhou: {erro}"));
                (EFAILED, ESTADO_FALHOU)
            }
        };
        // Nada é entregue aqui. Estamos no meio do despacho de uma chamada, com o jogo dentro
        // da `ConnectionManager::init`, e foi assim que a ponte derrubou o jogo: chamar o
        // alocador dele nesse instante reentra num gerenciador que está no meio de uma operação.
        //
        // O emulador já tem a fronteira certa para isso — a mesma dos sinais, que roda fora do
        // despacho, quando o guest não está dentro de nada. A resposta espera na fila até lá.
        self.pending_response = Some((objeto, base, estado));
        Ok(resultado)
    }

    /// Deposita a resposta e avisa o estado, fora do despacho.
    ///
    /// A ordem importa: os campos primeiro, o estado depois. O jogo consulta o estado a cada
    /// volta do laço e, ao vê-lo em "recebendo", lê a contagem — se avisássemos antes de
    /// entregar, ele leria zero e concluiria que não veio nada.
    fn flush_response(&mut self) -> Result<(), CpuError> {
        // O fim do fluxo anunciado na fronteira seguinte à entrega, para dar ao jogo uma volta
        // inteira em que ele consome os campos.
        if let Some(resposta) = self.pending_end.take() {
            self.cpu.write_mem(resposta + FIM_DO_FLUXO, &[1])?;
        }
        let Some((objeto, base, estado)) = self.pending_response.take() else {
            return Ok(());
        };
        if estado == ESTADO_RECEBENDO {
            self.deliver_response(objeto)?;
        }
        self.finish_request(base, estado)
    }

    /// Entrega a resposta ao objeto que o jogo preparou para recebê-la.
    ///
    /// O remetente guarda esse objeto em `+8` do próprio (`str r2, [r4, #8]` em `0x94eb8`), e o
    /// tratador do estado "recebendo" o lê assim (`0x85b44`): `[+8]` é a contagem de campos e
    /// zero quer dizer "não veio nada", `[+4]` é o vetor de ponteiros e `[+0xc]` a capacidade.
    /// É o `ttdArray` do próprio jogo.
    ///
    /// As strings **saem do alocador do jogo**, pela [`crate::ponte`], porque é dele que o
    /// gerenciador de memória espera recebê-las de volta. Sem ponte declarada para o módulo,
    /// não entregamos nada: melhor o jogo ver "não veio resposta" do que ver memória que ele vai
    /// recusar. O que trafega não muda em nenhum dos dois casos.
    fn deliver_response(&mut self, objeto: u32) -> Result<(), CpuError> {
        // A ponte é opcional e vem desligada. Ela mexe na memória do jogo, e uma entrega errada
        // não falha na hora: ela corrompe e quebra adiante, como aconteceu — o vetor guarda
        // objetos `ttdString`, com o comprimento em `[0]` e o texto em `[4]`, e entregar texto
        // cru fez o destrutor liberar lixo. Enquanto isso não estiver certo, o padrão é não
        // entregar: o jogo vê "não veio resposta", que é um estado que ele sabe tratar.
        if !self.bridge {
            return Ok(());
        }
        let Some(ponte) = ponte::para(self.applet_class) else {
            self.delivered.push(format!(
                "sem ponte declarada para o módulo {:#010x}",
                self.applet_class
            ));
            return Ok(());
        };
        // Corpo vazio **é** resposta: o servidor pode não ter nada a devolver. O que não pode é
        // ficar em silêncio, senão o jogo espera para sempre pelo fim do fluxo que nunca vem.
        // Então o caminho é o mesmo, com texto vazio.
        let texto = String::from_utf8_lossy(&self.web_response)
            .trim_end_matches(['\r', '\n', '\0'])
            .to_string();

        // Quem fatia é o jogo. O parser dele lê o texto de `+0x20`, anexa o pedaço que recebe e
        // divide nos `;`, preenchendo o vetor com memória do próprio alocador e ligando as
        // marcas que o consumidor espera — inclusive a de "li o que precisava", que era o que
        // faltava. Reproduzir isso à mão foi o erro anterior: entregávamos campos que ele nunca
        // reconhecia como completos, e o jogo ficava esperando para sempre.
        //
        // Então damos só o texto, no lugar onde ele o procura, e mandamos fatiar.
        let resposta = self.cpu.read_u32(objeto + 8)?;
        if resposta == 0 {
            self.delivered
                .push("recusado: o objeto de resposta não existe".to_string());
            return Ok(());
        }
        let bytes = texto.as_bytes();
        let buffer = self.alocar_no_jogo(ponte, bytes.len() as u32 + 1)?;
        if buffer == 0 {
            return Ok(());
        }
        self.cpu.write_mem(buffer, bytes)?;
        self.cpu.write_mem(buffer + bytes.len() as u32, &[0])?;

        // O que já estivesse ali é devolvido ao alocador, senão vaza.
        let anterior = self.cpu.read_u32(resposta + TEXTO_DA_RESPOSTA)?;
        if anterior != 0 {
            self.call_guest_with_stack(ponte.liberador, [anterior, 0, 0, 0], &[], PONTE_BUDGET)?;
        }
        self.cpu.write_u32(resposta + TEXTO_DA_RESPOSTA, buffer)?;

        // O segundo argumento do parser é o pedaço a anexar. O texto inteiro já está no lugar,
        // então vai uma string vazia.
        let vazio = self.alocar_no_jogo(ponte, 1)?;
        if vazio == 0 {
            return Ok(());
        }
        self.cpu.write_mem(vazio, &[0])?;
        self.call_guest_with_stack(ponte.parser, [resposta, vazio, 0, 0], &[], PONTE_BUDGET)?;

        // A marca de "chegou dado novo". O tratador em `0x85b5c` só interpreta o campo 0 quando
        // ela está ligada, e a apaga logo depois — é bandeira de uma via, e no console quem a
        // ligava era o despachante.
        self.cpu.write_mem(resposta + 0x18, &[1])?;

        // A marca de "acabou o fluxo" fica para a **próxima** fronteira, e essa espera é o ponto.
        //
        // O `ConnectionManager` recebe em pedaços: a cada um chama o parser, e só quando chega
        // um de zero bytes ele liga a marca. Entre um pedaço e outro o jogo consulta a resposta
        // e **consome os campos**. Ligá-la junto com a entrega pulava esse consumo — o tratador
        // em `0x85c9c` desvia direto para o fim quando a marca já está lá, devolve sucesso e o
        // boneco fica sem os números.
        //
        // Entregamos tudo de uma vez, então imitamos o intervalo: o texto agora, o fim depois.
        self.pending_end = Some(resposta);

        let campos = self.cpu.read_u32(resposta + 8).unwrap_or(0);
        self.delivered.push(format!(
            "entregue ao parser do jogo: {} bytes, {campos} campo(s) reconhecido(s)",
            bytes.len()
        ));
        Ok(())
    }

    /// Pede memória ao alocador do próprio jogo, pela ponte.
    ///
    /// A assinatura observada é `(tamanho, pool, linha, arquivo, 1)`, com o pool zero — que ele
    /// exige menor que 32 — e os dois do meio servindo ao rastreio de origem dele.
    fn alocar_no_jogo(&mut self, ponte: ponte::Ponte, tamanho: u32) -> Result<u32, CpuError> {
        let outcome = self.call_guest_with_stack(
            ponte.alocador,
            [tamanho, 0, LINHA_DE_ORIGEM, ponte.origem],
            &[1],
            PONTE_BUDGET,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(code),
            _ => {
                self.assumptions
                    .insert("a resposta não foi entregue: o alocador do jogo não retornou");
                Ok(0)
            }
        }
    }

    /// Avisa o jogo que a requisição terminou.
    ///
    /// Ele não espera evento nenhum: a tela de sync **consulta um campo de estado** do objeto de
    /// quem pediu, e enquanto ele valer 0 ou 2 continua mostrando "Connecting". Com 9 ela vai
    /// para "ReceivingData" e lê a resposta; com 11 vai para o ramo de falha.
    ///
    /// O campo fica doze bytes antes da URL, no mesmo objeto — então ele é achado pela mesma
    /// âncora que já se conferiu, e não por um deslocamento solto.
    ///
    /// **A trava está aqui**: só escrevemos se o campo tiver agora um dos valores que o próprio
    /// código do jogo trata como "em andamento". Se tiver qualquer outra coisa, a âncora não é o
    /// que pensamos e não mexemos na memória dele. Escrever um palpite sobre a memória do guest
    /// é o tipo de erro que se paga caro e tarde.
    fn finish_request(&mut self, base: u32, estado: u32) -> Result<(), CpuError> {
        let campo = base - OFFSET_ESTADO_ANTES_DA_URL;
        match self.cpu.read_u32(campo) {
            Ok(ESTADO_TRABALHANDO | ESTADO_TRABALHANDO_2) => self.cpu.write_u32(campo, estado),
            _ => {
                self.assumptions
                    .insert("o estado da conexão não foi avisado: o campo não parecia o esperado");
                Ok(())
            }
        }
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
                    self.parametros_de_colecao.retain(|(obj, _), _| *obj != this);
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

    /// `ISQLMgr` e `ISQLDatabase` — os bancos SQLite do console. Ver [`crate::sql`].
    ///
    /// A ordem dos slots não veio de header: veio da observação com o `--sonda`. O Z-Wheel cria
    /// o gerenciador, chama o slot 3 com `"tt_prefs.db"` e um ponteiro de saída, e no banco que
    /// recebe chama o slot 3 de novo, agora com `"PRAGMA integrity_check"`. Por isso os dois
    /// nomes que estão em [`crate::aee_slots::SQL_MGR`] são os únicos com nome.
    fn sql_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.databases.remove(&this);
                }
                restantes
            }
            // int OpenDatabase(ISQLMgr *, const char *pszName, ISQLDatabase **ppDB)
            (Interface::SqlMgr, "OpenDatabase") => {
                let nome = self.cpu.read_cstring(a1, MAX_STRING);
                // O banco fica ao lado do módulo, como qualquer arquivo do jogo, e passa pelo
                // VFS pelo mesmo motivo dos outros: nada escreve fora do diretório dele.
                let Some(caminho) = self.vfs.resolve_new(&nome) else {
                    self.missing_files.insert(nome);
                    return Ok(Some(EFAILED));
                };
                match crate::sql::Database::open(&caminho) {
                    Ok(db) => {
                        self.escolhe_idioma(&db);
                        let object = self.new_object(Interface::SqlDatabase)?;
                        if object == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.databases.insert(object, db);
                        if a2 != 0 {
                            self.cpu.write_u32(a2, object)?;
                        }
                        SUCCESS
                    }
                    Err(erro) => {
                        self.bad_pointers.insert(format!("SQL: {nome}: {erro}"));
                        SUCCESS
                    }
                }
            }
            // int Exec(ISQLDatabase *, const char *pszSQL, callback, void *pContexto)
            (Interface::SqlDatabase, "Exec") => {
                let sql = self.cpu.read_cstring(a1, MAX_SQL);
                let Some(db) = self.databases.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let resultado = db.exec(&sql);
                match resultado {
                    Ok(linhas) => {
                        // `Exec(this, sql, callback, contexto)`: a sonda mostrou o ponteiro de
                        // função em `r2` — dentro da faixa de código do módulo — e o contexto
                        // em `r3`, no heap.
                        let (callback, contexto) = (a2, self.cpu.read_reg(Reg::R3));
                        self.sql_deliver(&linhas, callback, contexto)?;
                        SUCCESS
                    }
                    Err(erro) => {
                        self.bad_pointers.insert(format!("SQL recusado: {erro}"));
                        SUCCESS
                    }
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Desenha `text` na superfície corrente. `false` quando não há fonte para desenhá-lo.
    ///
    /// A cor é a do `CLR_USER_TEXT`, que é o que o `IDISPLAY_SetColor` ajusta, e o recorte vale
    /// aqui como em qualquer outro desenho.
    fn draw_text(&mut self, text: &str, x: i32, y: i32) -> Result<bool, CpuError> {
        let Some(fonte) = self.font.as_ref() else {
            return Ok(false);
        };
        let glifos = fonte.layout(text, FONT_SIZE);
        if glifos.is_empty() {
            return Ok(true);
        }
        let cor = self
            .colors
            .get(CLR_USER_TEXT)
            .copied()
            .unwrap_or(Rgb::BLACK);
        let nativo = cor.to_rgb565();
        let recorte = self.clip;
        let target = self.target()?;
        let Some(surface) = self.bitmaps.get_mut(&target) else {
            return Ok(true);
        };
        for glifo in glifos {
            for linha in 0..glifo.height {
                for coluna in 0..glifo.width {
                    // Meio-tom não existe numa superfície sem canal alfa: ou a letra cobre o
                    // pixel, ou não cobre. Metade é o corte que deixa a borda parecida com a
                    // do console, que também não mistura.
                    if glifo.coverage[(linha * glifo.width + coluna) as usize] < 128 {
                        continue;
                    }
                    let (px, py) = (x + glifo.x + coluna as i32, y + glifo.y + linha as i32);
                    if let Some(clip) = recorte {
                        let dentro = px >= clip.x as i32
                            && py >= clip.y as i32
                            && px < clip.x as i32 + clip.width as i32
                            && py < clip.y as i32 + clip.height as i32;
                        if !dentro {
                            continue;
                        }
                    }
                    surface.set_pixel_native(px, py, nativo);
                }
            }
        }
        Ok(true)
    }

    /// As URLs que o jogo tentou buscar pelo `IWeb`.
    /// Os últimos toques entregues ao jogo, como `(instante, nome do botão, apertado)`.
    pub fn pad_log(&self) -> Vec<(u32, usize, &'static str, bool)> {
        self.pad_log
            .iter()
            .map(|&(ms, porta, index, down)| {
                (
                    ms,
                    porta,
                    input::BUTTON_NAMES.get(index).copied().unwrap_or("?"),
                    down,
                )
            })
            .collect()
    }

    /// Liga ou desliga o acesso à rede.
    pub fn set_network(&mut self, ligada: bool) {
        self.network = ligada;
    }

    /// Liga a ponte do módulo, que entrega a resposta ao jogo. Desligada por padrão.
    pub fn set_bridge(&mut self, ligada: bool) {
        self.bridge = ligada;
    }

    /// Desvia as conexões para outra máquina ou porta, sem mexer no que o jogo pediu.
    pub fn set_network_to(&mut self, destino: Option<String>) {
        self.network_to = destino;
    }

    /// As chamadas de GL que atendemos sem fazer nada.
    pub fn ignored_gl(&self) -> Vec<&'static str> {
        self.ignored_gl.iter().copied().collect()
    }

    /// As respostas que a ponte entregou ao jogo.
    pub fn delivered(&self) -> &[String] {
        &self.delivered
    }

    /// O corpo da última resposta HTTP recebida.
    pub fn web_response(&self) -> &[u8] {
        &self.web_response
    }

    /// O que o jogo cifrou, em claro, na ordem em que entregou.
    pub fn plaintexts(&self) -> Vec<&[u8]> {
        self.plaintexts.iter().map(Vec::as_slice).collect()
    }

    /// As chaves de cifra que os jogos configuraram, em hexadecimal.
    ///
    /// Existe por um motivo prático: o Zeeboids **cifra o corpo antes de enviar**, então o
    /// servidor recebe ruído. A chave é do próprio jogo e passa por nós no `ICipher1::SetParam`;
    /// sem ela, o registro do servidor não diz nada sobre o protocolo.
    pub fn cipher_keys(&self) -> Vec<String> {
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        self.ciphers
            .values()
            .filter_map(|c| {
                c.key
                    .map(|k| format!("chave {} iv {}", hex(&k), hex(&c.iv)))
            })
            .collect()
    }

    pub fn web_requests(&self) -> Vec<String> {
        self.web_requests.iter().cloned().collect()
    }

    /// De onde saiu a fonte que está desenhando o texto, se houver uma.
    pub fn font_source(&self) -> Option<&str> {
        self.font.as_ref().map(|f| f.source.as_str())
    }

    /// O calendário do guest, em segundos desde 6 de janeiro de 1980 GMT.
    ///
    /// A data vem do relógio do host, capturada **uma vez** na criação da máquina, e daí em
    /// diante anda com o relógio virtual. É o meio-termo entre as duas coisas que o projeto
    /// quer: um jogo que pergunta a data recebe uma que existe, e o tempo que ele mede
    /// continua sendo o virtual, sem depender de quanto o emulador demorou.
    fn brew_seconds(&self) -> u32 {
        self.epoch_seconds + self.elapsed_ms() / 1000
    }

    /// Entrega um evento ao applet em execução, e devolve o que ele respondeu.
    ///
    /// Só há um applet aqui, então um `cls` que não seja o dele é evento para alguém que não
    /// existe — e a resposta certa nesse caso é "ninguém tratou", que é o que o BREW responde.
    ///
    /// Chamar o guest daqui é reentrância, com o mesmo cuidado do `qsort` e da entrega de
    /// linhas de SQL: salva os registradores, respeita o teto de aninhamento, devolve tudo.
    fn send_applet_event(&mut self, cls: u32, evt: u32, w: u16, dw: u32) -> Result<u32, CpuError> {
        // O `current_applet` só é preenchido quando o `EVT_APP_START` é despachado, e há
        // evento antes disso: a Z-Wheel monta o banco de preferências **durante a
        // construção** do applet, e para isso manda um evento para a própria classe. Nesse
        // instante o objeto já existe — o `AEEApplet_New` escreveu o ponteiro de saída antes
        // de o código do jogo rodar —, então lê-lo de lá é o que o console faz: para o shell,
        // o applet passa a existir quando é registrado, não quando é iniciado.
        //
        // Sem isto o evento voltava "ninguém tratou", e a Z-Wheel imprimia
        // `SendEvent to get PrefsDB failed` onze vezes seguidas antes de desistir da
        // configuração inteira.
        let applet = match self.current_applet {
            0 => self.cpu.read_u32(self.module.out_module + 4).unwrap_or(0),
            vivo => vivo,
        };
        if applet == 0 || (cls != 0 && cls != self.applet_class) {
            return Ok(FALSE);
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("um evento de applet não foi entregue por aninhamento profundo");
            return Ok(FALSE);
        }
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let outcome = self.call_guest(handle_event, [applet, evt, u32::from(w), dw], QSORT_BUDGET);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(match outcome? {
            Outcome::Returned { code } => code,
            // Um tratador que se perde não derruba quem mandou o evento: para ele, ninguém
            // tratou.
            _ => FALSE,
        })
    }

    /// Entrega as linhas ao callback do jogo, uma chamada por linha.
    ///
    /// É a forma do `sqlite3_exec`: `callback(contexto, nColunas, azValores, azNomes)`, com os
    /// dois vetores de `char *` na memória do guest. Sem isso a consulta "funciona" e o jogo
    /// não recebe nada — foi o que fez o Z-Wheel passar no `PRAGMA integrity_check` e ainda
    /// assim dizer "Invalid database version".
    ///
    /// Chamar o guest daqui é reentrância, e por isso segue o mesmo cuidado do `qsort`: salva
    /// os registradores, respeita o teto de aninhamento e devolve tudo no lugar.
    fn sql_deliver(
        &mut self,
        linhas: &[crate::sql::Row],
        callback: u32,
        contexto: u32,
    ) -> Result<(), CpuError> {
        if callback == 0 || linhas.is_empty() {
            return Ok(());
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("uma consulta SQL foi entregue sem callback por aninhamento profundo");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let mut resultado = Ok(());
        for linha in linhas {
            match self.sql_deliver_row(linha, callback, contexto) {
                Ok(true) => {}
                // Callback que devolve diferente de zero manda parar, como no SQLite.
                Ok(false) => break,
                Err(err) => {
                    resultado = Err(err);
                    break;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        resultado
    }

    /// Monta os dois vetores de `char *` de uma linha e chama o callback. `false` pede parada.
    fn sql_deliver_row(
        &mut self,
        linha: &crate::sql::Row,
        callback: u32,
        contexto: u32,
    ) -> Result<bool, CpuError> {
        let colunas = linha.names.len() as u32;
        let valores = self.sql_write_strings(linha.values.iter().map(|v| v.as_deref()))?;
        let nomes = self.sql_write_strings(linha.names.iter().map(|n| Some(n.as_str())))?;
        let saida = match (valores, nomes) {
            (Some(valores), Some(nomes)) => {
                let outcome = self.call_guest(
                    callback,
                    [contexto, colunas, valores, nomes],
                    SQL_CALLBACK_BUDGET,
                )?;
                // Só o retorno normal conta; um callback que se perde não interrompe o resto.
                let seguir = !matches!(outcome, Outcome::Returned { code } if code != 0);
                self.heap.free(valores);
                self.heap.free(nomes);
                seguir
            }
            _ => {
                self.assumptions
                    .insert("uma linha de consulta SQL não coube na memória do jogo");
                false
            }
        };
        Ok(saida)
    }

    /// Grava as strings no heap do guest e devolve o vetor de ponteiros para elas.
    ///
    /// O vetor e o texto saem do mesmo bloco: um `free` só devolve tudo, e o callback do
    /// `sqlite3_exec` não guarda os ponteiros depois de retornar.
    fn sql_write_strings<'a>(
        &mut self,
        textos: impl Iterator<Item = Option<&'a str>> + Clone,
    ) -> Result<Option<u32>, CpuError> {
        let contagem = textos.clone().count() as u32;
        let bytes: usize = textos.clone().map(|t| t.map_or(0, |t| t.len() + 1)).sum();
        let bloco = self.malloc(contagem * 4 + bytes as u32)?;
        if bloco == 0 {
            return Ok(None);
        }
        let mut texto_em = bloco + contagem * 4;
        for (i, texto) in textos.enumerate() {
            let ponteiro = match texto {
                // Coluna nula é ponteiro nulo, como o SQLite entrega.
                None => 0,
                Some(texto) => {
                    self.cpu.write_mem(texto_em, texto.as_bytes())?;
                    self.cpu.write_mem(texto_em + texto.len() as u32, &[0])?;
                    let onde = texto_em;
                    texto_em += texto.len() as u32 + 1;
                    onde
                }
            };
            self.cpu.write_u32(bloco + i as u32 * 4, ponteiro)?;
        }
        Ok(Some(bloco))
    }

    /// Atende uma chamada num objeto-sonda: registra e responde `SUCCESS`.
    ///
    /// Responder sucesso a tudo é deliberado. A sonda não tenta acertar o comportamento — ela
    /// tenta **fazer o jogo andar** para ver o que ele pede em seguida. Um `EFAILED` honesto
    /// pararia a investigação na primeira chamada.
    fn probe_call(&mut self, slot: u32) -> Result<u32, CpuError> {
        let this = self.cpu.read_reg(Reg::R0);
        let clsid = self.probe_objects.get(&this).copied().unwrap_or(0);
        // Registra **todos** os slots, o `AddRef` e o `Release` inclusive: saber que o jogo só
        // criou e soltou o objeto é resposta tão útil quanto saber que ele chamou o slot 7.
        let args = self.args();
        // A contagem é o que separa "o app chamou isto" de "o app está preso nisto": um método
        // com milhões de chamadas é um laço contra uma resposta nossa, não uso normal.
        if let Some(entrada) = self
            .probe_log
            .iter_mut()
            .find(|(c, o, s, _, _, _)| *c == clsid && *o == this && *s == slot)
        {
            entrada.5 += 1;
        } else {
            // O argumento que aponta para texto legível é quase sempre o que interessa — o
            // nome do banco, a instrução SQL. Lê-lo aqui evita ter que descobrir onde o
            // módulo foi mapeado para ir buscar no arquivo.
            let textos = args.map(|arg| self.probe_text(arg));
            self.probe_log.push((clsid, this, slot, args, textos, 1));
        }
        match slot {
            // `AddRef` e `Release` são os dois primeiros em toda interface do BREW, e a
            // contagem precisa valer: sem ela o objeto morre ou vaza no meio da observação.
            0 => Ok(self.objects.add_ref(this)),
            1 => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.probe_objects.remove(&this);
                }
                Ok(restantes)
            }
            _ => {
                // Um método que devolve objeto escreve o ponteiro num argumento de saída, e
                // devolver `SUCCESS` sem escrever nada faz o jogo seguir com lixo e morrer no
                // primeiro uso — foi o que o Z-Wheel fez. Então a sonda **entrega outra sonda**
                // no que parecer um ponteiro de saída: assim o jogo continua e a observação
                // alcança a família inteira de objetos, não só o primeiro.
                for arg in args {
                    if self.looks_like_out_pointer(arg) {
                        let filho = self.new_object(Interface::Probe)?;
                        if filho != 0 {
                            self.probe_objects.insert(filho, clsid);
                            self.cpu.write_u32(arg, filho)?;
                        }
                        break;
                    }
                }
                // A resposta combinada troca só o **valor de retorno**; a entrega do objeto no
                // ponteiro de saída continua valendo. Juntar as duas coisas já me custou uma
                // investigação: combinei "responda 1" e o jogo recebeu 1 com o ponteiro de
                // saída vazio, que é um estado que não existe em lugar nenhum.
                Ok(self
                    .probe_answers
                    .get(&(clsid, slot))
                    .copied()
                    .unwrap_or(SUCCESS))
            }
        }
    }

    /// O texto em `addr`, quando o que está lá é mesmo texto.
    ///
    /// Exige começar com caractere imprimível e ter pelo menos dois deles antes do zero: com
    /// menos que isso, qualquer inteiro pequeno viraria "string" e o registro só teria ruído.
    fn probe_text(&self, addr: u32) -> Option<String> {
        if addr == 0 {
            return None;
        }
        let texto = self.cpu.read_cstring(addr, 120);
        let legivel = texto.len() >= 2
            && texto
                .chars()
                .all(|c| c == '\n' || c == '\t' || (' '..='~').contains(&c));
        legivel.then_some(texto)
    }

    /// Se `addr` tem cara de ponteiro de saída: alinhado, na memória do jogo e valendo zero.
    ///
    /// A exigência do zero é o que torna isto seguro de usar: um argumento que já aponta para
    /// algo não é destino de saída, e escrever nele estragaria dado do jogo.
    fn looks_like_out_pointer(&self, addr: u32) -> bool {
        let na_memoria = (loader::HEAP_BASE..loader::HEAP_BASE + loader::HEAP_SIZE as u32)
            .contains(&addr)
            || (loader::STACK_BASE..loader::STACK_BASE + loader::STACK_SIZE as u32).contains(&addr);
        na_memoria && addr.is_multiple_of(4) && self.cpu.read_u32(addr).unwrap_or(1) == 0
    }

    /// Manda atender estas classes com um objeto-sonda em vez de recusá-las.
    pub fn probe_classes(&mut self, classes: &[u32]) {
        self.probe_classes.extend(classes);
    }

    /// Combina a resposta de um slot de sonda, para explorar o outro lado de um desvio.
    pub fn probe_answer(&mut self, clsid: u32, slot: u32, value: u32) {
        self.probe_classes.insert(clsid);
        self.probe_answers.insert((clsid, slot), value);
    }

    /// O que os jogos chamaram nas sondas, na ordem em que apareceu.
    pub fn probe_log(&self) -> &[ProbeCall] {
        &self.probe_log
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

    /// Liga a saída de som. Sem ela o emulador roda igual, mudo.
    pub fn set_audio(&mut self, mixer: Option<crate::audio::Mixer>) {
        self.audio = mixer;
    }

    /// O EGL, nas duas formas em que o BREW o expõe.
    ///
    /// A nova (`IEGL11`, de `sdk/inc/AEEEGL10.h` e `AEEEGL11.h`) recebe o `this` no primeiro
    /// argumento e entrega o resultado num ponteiro de saída — sempre o último argumento —,
    /// reservando o retorno ao código de erro. A antiga (`IEGL`, de `sdk/inc/AEEGL.h`) não
    /// recebe o `this` e devolve o resultado direto.
    ///
    /// Fora isso as duas são a mesma API, na mesma ordem: tirando o prefixo `egl` dos nomes
    /// antigos, os métodos coincidem. Por isso um único tradutor atende as duas, com um
    /// deslocamento nos argumentos e uma decisão no fim sobre onde pôr a resposta.
    fn egl_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(full) = iface.method(slot) else {
            return Ok(None);
        };
        let legacy = iface == Interface::EglLegacy;
        let name = full.strip_prefix("egl").unwrap_or(full);
        // Os argumentos, já sem o `this` quando ele existe. Ler alguns a mais do que o método
        // usa é inofensivo: `arg` responde zero para o que não conseguir ler.
        let base = usize::from(!legacy);
        let a: [u32; 8] = std::array::from_fn(|i| self.arg(base + i));
        let this = self.arg(0);
        // A tela é a superfície do "device bitmap" quando o jogo pediu uma; o `self.screen`
        // vira um marcador de 1×1 nesse caso, e responder as dimensões dele fazia o Crash
        // montar uma viewport de um pixel e desenhar o jogo inteiro dentro dela.
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let (out, value) = match name {
            "AddRef" => return Ok(Some(self.objects.add_ref(this))),
            "Release" => return Ok(Some(self.objects.release(this))),
            "QueryInterface" => return Ok(Some(self.egl_query_interface(iface)?)),
            // Ler o erro também o limpa, como manda a spec do EGL.
            "GetError" => (0, std::mem::replace(&mut self.egl_error, gles::EGL_SUCCESS)),
            // Há um display só, e ele é a tela do console.
            "GetDisplay" => (1, EGL_DISPLAY),
            "Initialize" => {
                self.write_at(a[1], 1)?;
                self.write_at(a[2], 0)?;
                (3, gles::EGL_TRUE)
            }
            "Terminate" => (1, gles::EGL_TRUE),
            "QueryString" => {
                let text = match a[1] {
                    gles::EGL_VENDOR => "Zeebx",
                    gles::EGL_VERSION => "1.1",
                    // A escala de superfície do console. Os jogos procuram o nome por
                    // substring, com o espaço no fim como delimitador — é assim que a string
                    // aparece no binário deles.
                    gles::EGL_EXTENSIONS => "EGL_QUALCOMM_surface_scale ",
                    _ => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        return Ok(Some(if legacy { 0 } else { SUCCESS }));
                    }
                };
                (2, self.intern(text)?)
            }
            // void (*eglGetProcAddress(const char *procname))()
            //
            // A faixa de trampolim **é** um ponteiro de função: o endereço codifica interface e
            // slot, e é assim que toda chamada de API chega aqui. Então basta achar o método
            // pelo nome, tirando o `gl` da frente que o OpenGL usa e a vtable não.
            "GetProcAddress" => {
                let name = self.cpu.read_cstring(a[0], MAX_STRING);
                // A busca cobre a tabela inteira, e não só os slots da vtable real: as funções
                // de extensão ficam no fim dela e é só por aqui que o jogo chega a elas.
                let slot = name.strip_prefix("gl").and_then(|method| {
                    (0..crate::aee_slots::GLES.len() as u32)
                        .find(|&s| Interface::Gles.method(s) == Some(method))
                });
                match slot {
                    Some(slot) => (1, aee::encode(Interface::Gles, slot)),
                    None => {
                        self.bad_pointers
                            .insert(format!("o jogo pediu o endereço de {name}, que não temos"));
                        (1, 0)
                    }
                }
            }
            // `GetConfigs` lista as configurações e `ChooseConfig` filtra por atributos. Como
            // só existe uma, e ela é a nativa da tela, as duas respondem o mesmo.
            "GetConfigs" | "ChooseConfig" => {
                let first = usize::from(name == "ChooseConfig");
                let (configs, size, num) = (a[1 + first], a[2 + first], a[3 + first]);
                let fits = size >= 1;
                if configs != 0 && fits {
                    self.cpu.write_u32(configs, EGL_CONFIG)?;
                }
                self.write_at(num, u32::from(configs == 0 || fits))?;
                (4 + first, gles::EGL_TRUE)
            }
            "GetConfigAttrib" => match gles::config_attrib(a[2], width, height) {
                Some(answer) => {
                    self.write_at(a[3], answer)?;
                    (4, gles::EGL_TRUE)
                }
                None => {
                    self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                    (4, gles::EGL_FALSE)
                }
            },
            "CreateWindowSurface" | "CreatePixmapSurface" | "CreatePbufferSurface" => {
                let handle = self.new_egl_handle();
                self.egl_surfaces.insert(handle, (width, height));
                (if name == "CreatePbufferSurface" { 3 } else { 4 }, handle)
            }
            "DestroySurface" => {
                self.egl_surfaces.remove(&a[1]);
                (2, gles::EGL_TRUE)
            }
            "QuerySurface" => {
                let (w, h) = self
                    .egl_surfaces
                    .get(&a[1])
                    .copied()
                    .unwrap_or((width, height));
                let answer = match a[2] {
                    gles::EGL_WIDTH => Some(w),
                    gles::EGL_HEIGHT => Some(h),
                    attribute => gles::config_attrib(attribute, width, height),
                };
                match answer {
                    Some(answer) => {
                        self.write_at(a[3], answer)?;
                        (4, gles::EGL_TRUE)
                    }
                    None => {
                        self.egl_error = gles::EGL_BAD_ATTRIBUTE;
                        (4, gles::EGL_FALSE)
                    }
                }
            }
            "CreateContext" => {
                let handle = self.new_egl_handle();
                self.egl_context = handle;
                (4, handle)
            }
            "DestroyContext" => (2, gles::EGL_TRUE),
            "MakeCurrent" => {
                self.egl_surface = a[1];
                self.egl_context = a[3];
                (4, gles::EGL_TRUE)
            }
            "GetCurrentContext" => (0, self.egl_context),
            "GetCurrentSurface" => (1, self.egl_surface),
            "GetCurrentDisplay" => (0, EGL_DISPLAY),
            "QueryContext" => {
                self.write_at(a[3], 0)?;
                (4, gles::EGL_TRUE)
            }
            "WaitGL" => (0, gles::EGL_TRUE),
            "WaitNative" => (1, gles::EGL_TRUE),
            // Apresentar o quadro: o buffer de trás vira o da frente.
            "SwapBuffers" => {
                self.egl_swaps += 1;
                self.present_gl();
                self.wait_for_vsync();
                (2, gles::EGL_TRUE)
            }
            "CopyBuffers" => (3, gles::EGL_TRUE),
            "SurfaceAttrib" => (4, gles::EGL_TRUE),
            "BindTexImage" | "ReleaseTexImage" => (3, gles::EGL_TRUE),
            "SwapInterval" => (2, gles::EGL_TRUE),
            _ => return Ok(None),
        };
        if legacy {
            return Ok(Some(value));
        }
        self.write_at(a[out], value)?;
        Ok(Some(SUCCESS))
    }

    /// Escreve uma palavra num ponteiro de saída, ignorando o nulo.
    fn write_at(&mut self, pointer: u32, value: u32) -> Result<(), CpuError> {
        if pointer != 0 {
            self.cpu.write_u32(pointer, value)?;
        }
        Ok(())
    }

    /// `QueryInterface` do objeto do EGL.
    ///
    /// O `AEECLSID_QEGL` é um objeto só que responde por várias interfaces: o EGL propriamente
    /// dito e o OpenGL ES. Devolver `this` para tudo, como fazíamos, entregava a vtable do EGL
    /// para quem pediu a do GL — e a primeira chamada caía num slot que não existe.
    fn egl_query_interface(&mut self, iface: Interface) -> Result<u32, CpuError> {
        let (iid, out) = (self.arg(1), self.arg(2));
        // Um `QueryInterface` na forma antiga devolve a forma antiga do OpenGL: as duas
        // convenções não se misturam dentro de um mesmo objeto.
        let gl = match iface {
            Interface::EglLegacy => Interface::GlLegacy,
            _ => Interface::Gles,
        };
        let object = match iid {
            AEEIID_GLES10 | AEEIID_GLES11 => {
                if self.gles_object == 0 {
                    self.gles_object = self.new_object(gl)?;
                }
                self.gles_object
            }
            AEEIID_EGL10 | AEEIID_EGL11 => self.arg(0),
            // As extensões do console. A V2 é superconjunto da V1 com o mesmo prefixo de
            // vtable, então o mesmo objeto atende as duas IIDs.
            AEEIID_EGL_SURFACE_MANIP | AEEIID_EGL_SURFACE_MANIP_V1 => {
                if self.surface_manip == 0 {
                    self.surface_manip = self.new_object(Interface::EglSurfaceManip)?;
                }
                self.surface_manip
            }
            AEEIID_GLES_IMAGEON_EXT | AEEIID_GLES_IMAGEON_EXT_V1 => {
                if self.imageon_ext == 0 {
                    self.imageon_ext = self.new_object(Interface::GlesImageonExt)?;
                }
                self.imageon_ext
            }
            _ => {
                self.unknown_classes.insert(iid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };
        self.objects.add_ref(object);
        if out != 0 {
            self.cpu.write_u32(out, object)?;
        }
        Ok(SUCCESS)
    }

    /// O OpenGL ES, nas duas formas em que o BREW o expõe.
    ///
    /// A nova é o `IGLES11` de `sdk/inc/AEEGLES10.h` e `AEEGLES11.h`; a antiga é o `IGL` de
    /// `sdk/inc/AEEGL.h`, que não recebe o `this` e devolve o resultado direto. Como no EGL,
    /// tirando o prefixo `gl` os nomes coincidem, e um tradutor só atende as duas.
    ///
    /// Aqui não se desenha nada: os argumentos viram estado ou vértices, e quem rasteriza é o
    /// [`rasterizer`].
    fn gles_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(full) = iface.method(slot) else {
            return Ok(None);
        };
        let legacy = iface == Interface::GlLegacy;
        let name = full.strip_prefix("gl").unwrap_or(full);
        let base = usize::from(!legacy);
        let a: [u32; 10] = std::array::from_fn(|i| self.arg(base + i));
        let this = self.arg(0);
        // As variantes `x` levam ponto fixo 16.16 e as `f`, `float` de 32 bits — mesma função,
        // só muda como o número chega.
        let fixed = |v: u32| gles::fixed(v);
        let float = |v: u32| f32::from_bits(v);
        let number = |v: u32| {
            if name.ends_with('x') {
                fixed(v)
            } else {
                float(v)
            }
        };
        // Os poucos métodos que produzem um valor: na forma antiga ele é o retorno, na nova
        // vai para um ponteiro de saída.
        let mut answer = None;
        match name {
            "AddRef" => return Ok(Some(self.objects.add_ref(this))),
            "Release" => return Ok(Some(self.objects.release(this))),
            "QueryInterface" => {
                self.write_at(a[1], this)?;
                return Ok(Some(SUCCESS));
            }
            "GetError" => answer = Some((0, gles::GL_NO_ERROR)),
            "GetString" => {
                let text = match a[0] {
                    gles::GL_VENDOR => "Zeebx",
                    gles::GL_RENDERER => "Zeebx Software Rasterizer",
                    gles::GL_VERSION => "OpenGL ES-CM 1.1",
                    // Só o que existe de verdade. Anunciar extensão que não temos faria o jogo
                    // chamar função que não existe — e omitir uma que temos é pior ainda: os
                    // dez portes de arcade do console conferem o `GL_OES_draw_texture` aqui e
                    // desistem da inicialização gráfica sem ele.
                    gles::GL_EXTENSIONS => "GL_OES_draw_texture GL_ATI_imageon_misc ",
                    _ => "",
                };
                let addr = self.intern(text)?;
                answer = Some((1, addr));
            }
            // A spec só exige nomes distintos e fora de uso, então uma sequência serve.
            "GenTextures" | "GenBuffers" => {
                let (count, out) = (a[0], a[1]);
                for i in 0..count {
                    self.gles_next_name += 1;
                    if out != 0 {
                        self.cpu.write_u32(out + i * 4, self.gles_next_name)?;
                    }
                }
            }
            "DeleteTextures" => {
                for i in 0..a[0] {
                    let name = self.cpu.read_u32(a[1] + i * 4)?;
                    // A fila menciona texturas pelo nome; apagar uma antes de ser lida
                    // mudaria o que já foi desenhado.
                    self.gl.flush();
                    self.gl.textures.remove(&name);
                }
            }

            // --- Matrizes ---------------------------------------------------------------
            "MatrixMode" => self.gl.set_matrix_mode(a[0]),
            "LoadIdentity" => self.gl.load_identity(),
            "PushMatrix" => self.gl.push_matrix(),
            "PopMatrix" => self.gl.pop_matrix(),
            "LoadMatrixx" | "LoadMatrixf" | "MultMatrixx" | "MultMatrixf" => {
                let m = self.read_matrix(a[0], name.ends_with('x'))?;
                if name.starts_with("Load") {
                    self.gl.load_matrix(m);
                } else {
                    self.gl.mult_matrix(m);
                }
            }
            "Translatex" | "Translatef" => {
                let m = rasterizer::translation(number(a[0]), number(a[1]), number(a[2]));
                self.gl.mult_matrix(m);
            }
            "Scalex" | "Scalef" => {
                let m = rasterizer::scaling(number(a[0]), number(a[1]), number(a[2]));
                self.gl.mult_matrix(m);
            }
            "Rotatex" | "Rotatef" => {
                let m =
                    rasterizer::rotation(number(a[0]), number(a[1]), number(a[2]), number(a[3]));
                self.gl.mult_matrix(m);
            }
            "Frustumx" | "Frustumf" | "Orthox" | "Orthof" => {
                let v: Vec<f32> = a[..6].iter().map(|&word| number(word)).collect();
                let m = if name.starts_with("Frustum") {
                    rasterizer::frustum(v[0], v[1], v[2], v[3], v[4], v[5])
                } else {
                    rasterizer::ortho(v[0], v[1], v[2], v[3], v[4], v[5])
                };
                self.gl.mult_matrix(m);
            }

            // --- Estado -----------------------------------------------------------------
            "Viewport" => self
                .gl
                .set_viewport(a[0] as i32, a[1] as i32, a[2] as i32, a[3] as i32),
            "Clear" => self.gl.clear(a[0]),
            "ClearColorx" | "ClearColor" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_clear_color(c);
            }
            // `ClearDepth` já recebe a profundidade em `[0, 1]`, que é a faixa do buffer.
            "ClearDepthx" | "ClearDepthf" => self.gl.set_clear_depth(number(a[0]).clamp(0.0, 1.0)),
            "Color4x" | "Color4f" => {
                let c = std::array::from_fn(|i| number(a[i]));
                self.gl.set_color(c);
            }
            "Color4ub" => {
                let c = std::array::from_fn(|i| (a[i] & 0xff) as f32 / 255.0);
                self.gl.set_color(c);
            }
            "Enable" => self.gl.set_capability(a[0], true),
            "Disable" => self.gl.set_capability(a[0], false),
            "BlendFunc" => self.gl.set_blend_func(a[0], a[1]),
            "DepthFunc" => self.gl.set_depth_func(a[0]),
            "DepthMask" => self.gl.set_depth_mask(a[0] != 0),
            "AlphaFuncx" | "AlphaFunc" => self.gl.set_alpha_func(a[0], number(a[1])),
            "CullFace" => self.gl.set_cull_face(a[0]),
            "FrontFace" => self.gl.set_front_face(a[0]),
            "TexParameterx" | "TexParameteri" | "TexParameterf" => {
                self.gl.set_texture_parameter(a[1], a[2])
            }
            // As formas vetoriais trazem o valor por ponteiro. O Crash pede o `GL_REPLACE`
            // por aqui, e enquanto só a forma escalar era atendida o modo ficava preso no
            // `GL_MODULATE`: cada textura saía multiplicada pela cor do vértice.
            "TexParameterxv" | "TexParameteriv" | "TexParameterfv" => {
                match a[1] == gles::GL_TEXTURE_CROP_RECT_OES {
                    // O recorte são quatro inteiros com sinal, e o sinal importa: largura ou
                    // altura negativa espelha o eixo.
                    true => {
                        let mut crop = [0i32; 4];
                        for (index, slot) in crop.iter_mut().enumerate() {
                            *slot = self.cpu.read_u32(a[2] + index as u32 * 4)? as i32;
                        }
                        self.gl.set_texture_crop(crop);
                    }
                    false => {
                        let value = self.cpu.read_u32(a[2])?;
                        self.gl.set_texture_parameter(a[1], value);
                    }
                }
            }
            // void glDrawTex{sixf}OES(T x, T y, T z, T width, T height) e as formas vetoriais,
            // que trazem os cinco valores por ponteiro.
            //
            // `s` é inteiro de 16 bits, `i` de 32, `x` é ponto fixo 16.16 e `f` é float. Todas
            // desenham a mesma coisa; só muda como o número chega.
            name if name.starts_with("DrawTex") => {
                let vector = name.ends_with("vOES");
                let scale = match name.as_bytes().get(7) {
                    Some(b'x') => 1.0 / 65536.0,
                    _ => 1.0,
                };
                let float = name.as_bytes().get(7) == Some(&b'f');
                let mut values = [0f32; 5];
                for (index, slot) in values.iter_mut().enumerate() {
                    let raw = match vector {
                        true => self.cpu.read_u32(a[0] + index as u32 * 4)?,
                        false => self.arg(index + 1),
                    };
                    *slot = match float {
                        true => f32::from_bits(raw),
                        false => raw as i32 as f32 * scale,
                    };
                }
                let [x, y, z, width, height] = values;
                self.gl.draw_texture(x, y, z, width, height);
            }
            "TexEnvx" | "TexEnvi" | "TexEnvf" => {
                if a[1] == gles::GL_TEXTURE_ENV_MODE {
                    // O modo é um enum, mesmo quando chega pela variante de ponto fixo.
                    self.gl.set_texture_env(a[2]);
                }
            }
            "TexEnvxv" | "TexEnviv" | "TexEnvfv" => {
                if a[1] == gles::GL_TEXTURE_ENV_MODE {
                    let value = self.cpu.read_u32(a[2])?;
                    self.gl.set_texture_env(value);
                }
            }
            "ActiveTexture" => self.gl.set_active_texture(a[0]),
            "ClientActiveTexture" => self.gl.set_client_active_texture(a[0]),
            "BindTexture" => self.gl.bind_texture(a[1]),
            "TexImage2D" => self.gles_tex_image(&a)?,
            "TexSubImage2D" => self.gles_tex_sub_image(&a)?,
            "CompressedTexImage2D" => self.gles_compressed_tex_image(&a)?,

            // --- Vetores e desenho ------------------------------------------------------
            "VertexPointer" | "ColorPointer" | "TexCoordPointer" => {
                let pointer = ArrayPointer {
                    size: a[0],
                    kind: a[1],
                    stride: a[2],
                    address: a[3],
                    enabled: true,
                };
                // O vetor de coordenadas pertence à unidade escolhida pelo
                // `glClientActiveTexture`; as outras unidades não têm onde cair aqui.
                if name == "TexCoordPointer" && !self.gl.base_client_unit() {
                    return Ok(Some(SUCCESS));
                }
                let slot = match name {
                    "VertexPointer" => &mut self.gl_vertices,
                    "ColorPointer" => &mut self.gl_colors,
                    _ => &mut self.gl_texcoords,
                };
                // `enabled` é do `EnableClientState`, não do ponteiro: trocar o ponteiro não
                // liga nem desliga o vetor.
                let enabled = slot.enabled;
                *slot = ArrayPointer { enabled, ..pointer };
            }
            "EnableClientState" | "DisableClientState" => {
                let on = name.starts_with("Enable");
                match a[0] {
                    gles::GL_VERTEX_ARRAY => self.gl_vertices.enabled = on,
                    gles::GL_COLOR_ARRAY => self.gl_colors.enabled = on,
                    gles::GL_TEXTURE_COORD_ARRAY if self.gl.base_client_unit() => {
                        self.gl_texcoords.enabled = on
                    }
                    gles::GL_TEXTURE_COORD_ARRAY => {}
                    // As normais só serviriam para iluminação, que o pipeline não faz.
                    gles::GL_NORMAL_ARRAY => {}
                    _ => {}
                }
            }
            "DrawArrays" => {
                let indices: Vec<u32> = (0..a[2]).map(|i| a[1] + i).collect();
                self.gles_draw(a[0], &indices)?;
            }
            "DrawElements" => {
                let (mode, count, kind, list) = (a[0], a[1], a[2], a[3]);
                let mut indices = Vec::with_capacity(count as usize);
                for i in 0..count {
                    indices.push(match kind {
                        gles::GL_UNSIGNED_BYTE => {
                            let mut byte = [0u8; 1];
                            self.cpu.read_mem(list + i, &mut byte)?;
                            byte[0] as u32
                        }
                        _ => {
                            let mut half = [0u8; 2];
                            self.cpu.read_mem(list + i * 2, &mut half)?;
                            u16::from_le_bytes(half) as u32
                        }
                    });
                }
                self.gles_draw(mode, &indices)?;
            }

            "GetIntegerv" | "GetFixedv" | "GetBooleanv" => {
                let (width, height) = (self.gl.width as i32, self.gl.height as i32);
                let values = gles::integer(a[0], width, height).unwrap_or(&[0]);
                for (i, &value) in values.iter().enumerate() {
                    if a[1] != 0 {
                        self.cpu.write_u32(a[1] + i as u32 * 4, value as u32)?;
                    }
                }
            }
            // Os outros `Get*v` escrevem no ponteiro do segundo argumento; zerar é melhor que
            // deixar lixo, e nenhum jogo depende deles ainda.
            name if name.starts_with("Get") => self.write_at(a[1], 0)?,
            // O resto é atendido com sucesso e não faz nada. Isso é deliberado para o estado que
            // o nosso rasterizador não usa — profundidade, névoa, luz —, e recusar derrubaria
            // jogos por nada. Mas o silêncio esconde as que **mudam o desenho**: o
            // `TexSubImage2D` estava aqui, e o efeito era textura embaralhada sem uma linha de
            // aviso. Registrar não custa, e dá por onde começar a investigar um desenho errado.
            // glReadPixels(x, y, width, height, format, type, pixels)
            //
            // É como o jogo faz a foto do boneco: desenha e lê o quadro de volta. Enquanto isto
            // não existia, ele lia o que estivesse no buffer dele — daí a imagem embaralhada.
            "ReadPixels" => self.gles_read_pixels(&a)?,
            // glColorMask(r, g, b, a) — booleanos, um por canal.
            "ColorMask" => self.gl.set_color_mask(std::array::from_fn(|i| a[i] != 0)),
            outro => {
                if !ATENDIDAS_EM_SILENCIO.contains(&outro) {
                    self.ignored_gl.insert(full);
                }
            }
        }
        match answer {
            Some((_, value)) if legacy => Ok(Some(value)),
            Some((out, value)) => {
                self.write_at(a[out], value)?;
                Ok(Some(SUCCESS))
            }
            None => Ok(Some(SUCCESS)),
        }
    }

    /// Lê os dezesseis números de uma matriz da memória do guest.
    fn read_matrix(&self, address: u32, fixed_point: bool) -> Result<rasterizer::Matrix, CpuError> {
        let mut m = rasterizer::IDENTITY;
        for (i, slot) in m.iter_mut().enumerate() {
            let word = self.cpu.read_u32(address + i as u32 * 4)?;
            *slot = if fixed_point {
                gles::fixed(word)
            } else {
                f32::from_bits(word)
            };
        }
        Ok(m)
    }

    /// `TexImage2D(target, level, internalformat, width, height, border, format, type,
    /// pixels)` — nove argumentos, dos quais a maioria chega pela pilha.
    /// `glCompressedTexImage2D(target, level, internalformat, width, height, border,
    /// imageSize, data)`.
    ///
    /// Só os formatos ATITC, que são os do Adreno 130 e os únicos que os jogos do console usam
    /// — o Boomerang Sports Dodgeball carrega todas as texturas dele assim.
    fn gles_compressed_tex_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, format, width, height) = (a[1], a[2], a[3], a[4]);
        let (size, pixels) = (a[6], a[7]);
        // As texturas paletizadas do OES entram pelo mesmo caminho. O `level` delas é não
        // positivo — zero é só o nível base, e um negativo diz quantos mipmaps vêm depois —,
        // então a checagem de nível abaixo não vale para elas.
        if let Some(palette) = paltex::Format::from_gl(format) {
            if width == 0 || height == 0 || pixels == 0 {
                return Ok(());
            }
            let bytes = self.read_bytes(pixels, size)?;
            let Some(decoded) = paltex::decode(&bytes, width as usize, height as usize, palette)
            else {
                self.bad_pointers.insert(format!(
                    "textura paletizada {format:#x} sem paleta completa"
                ));
                return Ok(());
            };
            let name = self.gl.bound_texture();
            // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
            self.gl.flush();
            let texture = self.gl.textures.entry(name).or_default();
            texture.width = width as usize;
            texture.height = height as usize;
            texture.pixels = decoded;
            return Ok(());
        }
        let explicit_alpha = match format {
            gles::GL_ATC_RGB_AMD => false,
            gles::GL_ATC_RGBA_EXPLICIT_ALPHA_AMD => true,
            _ => {
                self.bad_pointers
                    .insert(format!("textura comprimida no formato {format:#x}"));
                return Ok(());
            }
        };
        // Como no `TexImage2D`, só o nível zero interessa: sem mipmap, carregar os menores por
        // cima do maior apagaria a textura.
        if level != 0 || width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes(pixels, size)?;
        let decoded = atc::decode(&bytes, width as usize, height as usize, explicit_alpha);

        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let texture = self.gl.textures.entry(name).or_default();
        texture.width = width as usize;
        texture.height = height as usize;
        texture.pixels = decoded;
        Ok(())
    }

    fn gles_tex_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, width, height) = (a[1], a[3], a[4]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);

        // Só o nível zero interessa: não fazemos mipmap, e carregar os níveis menores por cima
        // do maior apagaria a textura.
        if level != 0 || width == 0 || height == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let decoded = if pixels == 0 {
            vec![[255; 4]; texels]
        } else {
            let bytes = self.read_bytes(pixels, width * height * bytes_per_texel(format, kind))?;
            decode_texels(&bytes, format, kind, texels)
        };
        // Os parâmetros de repetição e filtro sobrevivem a uma nova imagem: no OpenGL eles são
        // do nome da textura, não do conteúdo, e o jogo costuma defini-los uma vez só.
        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let texture = self.gl.textures.entry(name).or_default();
        texture.width = width as usize;
        texture.height = height as usize;
        texture.pixels = decoded;
        Ok(())
    }

    /// `glReadPixels`: copia um retângulo do quadro para a memória do jogo.
    ///
    /// Atendemos os dois formatos que o OpenGL ES 1.1 obriga: `RGBA` de oito bits por canal e
    /// `RGB` em 565. Qualquer outro é registrado em vez de escrito, porque preencher com o
    /// formato errado dá uma imagem plausível e falsa — pior que não escrever.
    fn gles_read_pixels(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (x, y) = (a[0] as i32, a[1] as i32);
        let (width, height) = (a[2] as usize, a[3] as usize);
        let (format, kind, destino) = (a[4], a[5], a[6]);
        if width == 0 || height == 0 || destino == 0 {
            return Ok(());
        }
        let pixels = self.gl.read_rect(x, y, width, height);
        let bytes: Vec<u8> = match (format, kind) {
            (gles::GL_RGBA, gles::GL_UNSIGNED_BYTE) => pixels.concat(),
            (gles::GL_RGB, gles::GL_UNSIGNED_SHORT_5_6_5) => pixels
                .iter()
                .flat_map(|p| {
                    let v = (u16::from(p[0] >> 3) << 11)
                        | (u16::from(p[1] >> 2) << 5)
                        | u16::from(p[2] >> 3);
                    v.to_le_bytes()
                })
                .collect(),
            _ => {
                self.bad_pointers.insert(format!(
                    "ReadPixels no formato {format:#x}/{kind:#x}, que não sabemos escrever"
                ));
                return Ok(());
            }
        };
        self.cpu.write_mem(destino, &bytes)?;
        Ok(())
    }

    /// `glTexSubImage2D`: troca um retângulo de dentro de uma textura que já existe.
    ///
    /// É como se monta imagem em pedaços — um retrato dentro de um atlas, um número que muda —,
    /// e enquanto isto não existia a chamada era atendida em silêncio: a textura ficava com o
    /// conteúdo antigo e o desenho saía embaralhado.
    ///
    /// Se o retângulo não couber na textura, não escrevemos nada. Recortar seria inventar um
    /// resultado que o OpenGL não define.
    fn gles_tex_sub_image(&mut self, a: &[u32; 10]) -> Result<(), CpuError> {
        let (level, x, y, width, height) = (a[1], a[2], a[3], a[4], a[5]);
        let (format, kind, pixels) = (a[6], a[7], a[8]);
        if level != 0 || width == 0 || height == 0 || pixels == 0 {
            return Ok(());
        }
        let texels = (width * height) as usize;
        let bytes = self.read_bytes(pixels, width * height * bytes_per_texel(format, kind))?;
        let novos = decode_texels(&bytes, format, kind, texels);

        let name = self.gl.bound_texture();
        // Trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado.
        self.gl.flush();
        let Some(texture) = self.gl.textures.get_mut(&name) else {
            return Ok(());
        };
        let (tw, th) = (texture.width as u32, texture.height as u32);
        if x + width > tw || y + height > th {
            self.bad_pointers.insert(format!(
                "TexSubImage2D de {width}x{height} em ({x},{y}) não cabe numa textura {tw}x{th}"
            ));
            return Ok(());
        }
        for linha in 0..height {
            let destino = ((y + linha) * tw + x) as usize;
            let origem = (linha * width) as usize;
            texture.pixels[destino..destino + width as usize]
                .copy_from_slice(&novos[origem..origem + width as usize]);
        }
        Ok(())
    }

    /// Monta os vértices a partir dos vetores do cliente e manda desenhar.
    fn gles_draw(&mut self, mode: u32, indices: &[u32]) -> Result<(), CpuError> {
        if !self.gl_vertices.enabled || self.gl_vertices.address == 0 || indices.is_empty() {
            return Ok(());
        }
        let base = self.gl.current_color();
        let mut vertices = Vec::with_capacity(indices.len());
        for &index in indices {
            let position = self.read_attribute(self.gl_vertices, index, [0.0, 0.0, 0.0, 1.0])?;
            let color = if self.gl_colors.enabled && self.gl_colors.address != 0 {
                self.read_attribute(self.gl_colors, index, [0.0, 0.0, 0.0, 1.0])?
            } else {
                base
            };
            let uv = if self.gl_texcoords.enabled && self.gl_texcoords.address != 0 {
                let t = self.read_attribute(self.gl_texcoords, index, [0.0; 4])?;
                [t[0], t[1]]
            } else {
                [0.0; 2]
            };
            vertices.push(Vertex {
                position,
                color,
                uv,
            });
        }
        self.gl.draw(mode, &vertices);
        Ok(())
    }

    /// Lê um elemento de um vetor do cliente, completando os componentes que faltam.
    fn read_attribute(
        &self,
        pointer: ArrayPointer,
        index: u32,
        default: [f32; 4],
    ) -> Result<[f32; 4], CpuError> {
        let component = component_size(pointer.kind);
        let stride = if pointer.stride == 0 {
            pointer.size * component
        } else {
            pointer.stride
        };
        let base = pointer.address + index * stride;
        let mut out = default;
        for i in 0..pointer.size.min(4) {
            let address = base + i * component;
            out[i as usize] = match pointer.kind {
                gles::GL_FLOAT => f32::from_bits(self.cpu.read_u32(address)?),
                gles::GL_FIXED => gles::fixed(self.cpu.read_u32(address)?),
                gles::GL_SHORT => {
                    let mut half = [0u8; 2];
                    self.cpu.read_mem(address, &mut half)?;
                    i16::from_le_bytes(half) as f32
                }
                gles::GL_UNSIGNED_SHORT => {
                    let mut half = [0u8; 2];
                    self.cpu.read_mem(address, &mut half)?;
                    u16::from_le_bytes(half) as f32
                }
                gles::GL_BYTE => {
                    let mut byte = [0u8; 1];
                    self.cpu.read_mem(address, &mut byte)?;
                    byte[0] as i8 as f32
                }
                // `GL_UNSIGNED_BYTE` só aparece em cor, e ali o valor é normalizado.
                _ => {
                    let mut byte = [0u8; 1];
                    self.cpu.read_mem(address, &mut byte)?;
                    byte[0] as f32 / 255.0
                }
            };
        }
        Ok(out)
    }

    /// N-ésimo argumento da AAPCS: `r0..r3` e, daí em diante, palavras da pilha.
    fn arg(&self, index: usize) -> u32 {
        match index {
            0 => self.cpu.read_reg(Reg::R0),
            1 => self.cpu.read_reg(Reg::R1),
            2 => self.cpu.read_reg(Reg::R2),
            3 => self.cpu.read_reg(Reg::R3),
            n => {
                let sp = self.cpu.read_reg(Reg::Sp);
                self.cpu.read_u32(sp + (n as u32 - 4) * 4).unwrap_or(0)
            }
        }
    }

    /// Copia o quadro do OpenGL para a tela.
    ///
    /// É o que o `eglSwapBuffers` faz no console: o buffer de trás vira o da frente. Aqui a
    /// tela é o framebuffer RGB565 que já sabemos exportar.
    fn present_gl(&mut self) {
        // A tela pode ser a superfície do "device bitmap", quando o jogo pediu uma — é ela que
        // vale, e não o framebuffer de reserva.
        let (width, height) = {
            let target = self.screen();
            (target.width() as usize, target.height() as usize)
        };
        let frame = self.gl.present(width, height);
        let bytes: Vec<u8> = frame.iter().flat_map(|p| p.to_le_bytes()).collect();
        match self.bitmaps.get_mut(&self.device_bitmap) {
            Some(surface) => surface.load_rgb565_bytes(&bytes),
            None => self.screen.load_rgb565_bytes(&bytes),
        }
        self.gl_last_frame = bytes;
    }

    /// Próximo identificador de superfície ou contexto do EGL.
    fn new_egl_handle(&mut self) -> u32 {
        let handle = self.egl_next_handle;
        self.egl_next_handle += 1;
        handle
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

    /// `IThread` (`AEECLSID_THREAD` = `0x01001017`), de `sdk/inc/AEEThread.h`.
    ///
    /// A thread do BREW é cooperativa e se apoia nos callbacks: ela roda até chamar `Suspend`,
    /// e volta quando o `AEECallback` de `GetResumeCBK` é disparado — tipicamente pelo próprio
    /// jogo, via `ISHELL_Resume`. Aqui isso vira salvar e restaurar registradores, com uma
    /// pilha por thread alocada na heap do guest.
    fn thread_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Thread.method(slot) else {
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
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    if let Some(state) = self.threads.remove(&this) {
                        self.resume_callbacks.remove(&state.resume_cb);
                        self.heap.free(state.resume_cb);
                        self.heap.free(state.stack);
                    }
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            "Malloc" => self.malloc(a1)?,
            "Free" => {
                self.heap.free(a1);
                SUCCESS
            }
            // O pool de recursos existe para liberar tudo junto no fim da thread; a nossa heap
            // já sobrevive à thread, então segurar e soltar não muda nada.
            "HoldRsc" => SUCCESS,
            "ReleaseRsc" => SUCCESS,
            // int Start(IThread *, int nStackSz, PFNTHREAD pfStart, void *pvStart)
            "Start" => {
                let state = self.threads.entry(this).or_default();
                if state.started {
                    EALREADY
                } else {
                    let stack = self.malloc(a1.max(THREAD_MIN_STACK))?;
                    if stack == 0 {
                        ENOMEMORY
                    } else {
                        // A pilha do ARM cresce para baixo, então o topo do bloco é o `sp`
                        // inicial; a AAPCS pede alinhamento de 8.
                        let top = (stack + a1.max(THREAD_MIN_STACK)) & !7;
                        let state = self.threads.entry(this).or_default();
                        state.started = true;
                        state.stack = stack;
                        state.resume_pc = a2;
                        state.context = [0; 14];
                        state.context[0] = this;
                        state.context[1] = a3;
                        state.context[13] = top;
                        self.pending_threads.push(this);
                        SUCCESS
                    }
                }
            }
            // int Exit(IThread *, int nRv) — a thread acabou; o controle volta a quem a retomou.
            "Exit" => {
                self.finish_thread(this, a1)?;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // void Join(IThread *, AEECallback *pcb, int *pnRv)
            "Join" => {
                let call = self.resolve_notify(Callback {
                    function: a1,
                    context: a1,
                })?;
                let state = self.threads.entry(this).or_default();
                if state.finished {
                    let (code, joiner) = (state.exit_code, call);
                    if a2 != 0 {
                        self.cpu.write_u32(a2, code)?;
                    }
                    self.queue_call(joiner);
                } else {
                    state.joiners.push((call, a2));
                }
                SUCCESS
            }
            // void Suspend(IThread *) — o único ponto em que a thread devolve o controle.
            //
            // Salvamos os registradores como estão e trocamos o endereço de retorno pelo
            // sentinela: o laço de execução vai parar aí, e quem retomar continua no `lr` que
            // guardamos.
            "Suspend" => {
                let resume_pc = self.cpu.read_reg(Reg::Lr);
                let context = THREAD_REGS.map(|reg| self.cpu.read_reg(reg));
                let state = self.threads.entry(this).or_default();
                state.resume_pc = resume_pc;
                state.context = context;
                state.suspended = true;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // AEECallback *GetResumeCBK(IThread *) — o mesmo callback a cada chamada, porque é
            // por ele que reconhecemos um `ISHELL_Resume` dirigido à thread.
            "GetResumeCBK" => {
                let existing = self.threads.entry(this).or_default().resume_cb;
                if existing != 0 {
                    existing
                } else {
                    let cb = self.malloc(CALLBACK_SIZE)?;
                    self.threads.entry(this).or_default().resume_cb = cb;
                    if cb != 0 {
                        self.resume_callbacks.insert(cb, this);
                    }
                    cb
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Encerra uma thread: solta a pilha e libera quem esperava por ela.
    fn finish_thread(&mut self, thread: u32, code: u32) -> Result<(), CpuError> {
        let Some(state) = self.threads.get_mut(&thread) else {
            return Ok(());
        };
        state.finished = true;
        state.exit_code = code;
        let stack = std::mem::take(&mut state.stack);
        let joiners = std::mem::take(&mut state.joiners);
        self.heap.free(stack);
        self.pending_threads.retain(|&t| t != thread);
        for (call, out) in joiners {
            if out != 0 {
                self.cpu.write_u32(out, code)?;
            }
            self.queue_call(call);
        }
        Ok(())
    }

    /// Dá uma volta a cada thread que pediu para voltar.
    ///
    /// Uma volta por quadro, e só a partir daqui. Uma thread cooperativa cede o controle
    /// esperando ser retomada na próxima passada do laço de eventos, e o laço de eventos deste
    /// emulador é o laço de quadros — retomá-la também a cada fronteira entre chamadas de API
    /// faria o jogo rodar dezenas de quadros internos para cada quadro nosso.
    fn run_pending_threads(&mut self, budget: u64) -> Result<(), CpuError> {
        if self.pending_threads.is_empty() || self.current_thread.is_some() {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        for thread in std::mem::take(&mut self.pending_threads) {
            self.resume_thread(thread, budget)?;
        }
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Retoma uma thread do ponto em que ela parou.
    fn resume_thread(&mut self, thread: u32, budget: u64) -> Result<(), CpuError> {
        let Some(state) = self.threads.get(&thread) else {
            return Ok(());
        };
        if state.finished {
            return Ok(());
        }
        let (context, pc) = (state.context, state.resume_pc);
        for (reg, value) in THREAD_REGS.iter().zip(context) {
            self.cpu.write_reg(*reg, value);
        }
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        self.threads.entry(thread).or_default().suspended = false;
        self.current_thread = Some(thread);
        let outcome = self.execute(pc, budget);
        self.current_thread = None;
        // Voltar sem ter passado por `Suspend` significa que a função de entrada retornou: a
        // thread acabou, mesmo sem `Exit`.
        match outcome? {
            Outcome::Returned { code } => {
                let ceded = self
                    .threads
                    .get(&thread)
                    .is_some_and(|t| t.suspended || t.finished);
                if !ceded {
                    self.finish_thread(thread, code)?;
                }
            }
            // Um desfecho que não é retorno — API faltando, falha de memória — precisa chegar
            // ao relatório: é dentro da thread que o jogo passa a maior parte do tempo.
            other => self.stalled = Some(other),
        }
        Ok(())
    }

    /// Enfileira um callback do guest para a próxima fronteira entre chamadas.
    fn queue_call(&mut self, call: Callback) {
        if call.function != 0 {
            self.pending_calls.push(GuestCall {
                function: call.function,
                args: [call.context, 0, 0, 0],
            });
        }
    }

    /// `IHeap` (`AEECLSID_HEAP` = `0x01001002`), de `sdk/inc/AEEHeap.h`.
    ///
    /// A alocação é a mesma do `MALLOC` dos helpers — é o mesmo heap do guest, só que
    /// alcançado por outra porta. O que faltava de verdade era o `CheckAvail`: o Double Dragon
    /// pergunta se cabe o que ele quer antes de começar e, sem ninguém para responder,
    /// desenhava "Memory is insufficient" em vez do jogo.
    fn heap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Heap.method(slot) else {
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
            "Malloc" => self.malloc(a1 & !ALLOC_NO_ZMEM)?,
            "Realloc" => self.realloc(a1, a2 & !ALLOC_NO_ZMEM)?,
            "Free" => {
                self.heap.free(a1);
                SUCCESS
            }
            "StrDup" => {
                // `AECHAR` é UTF-16: a cópia leva o terminador junto.
                let units = self.read_aechar_units(a1)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // `boolean`: cabe se ainda há esse tanto livre no heap do guest.
            "CheckAvail" => u32::from(self.heap_available() >= a1 as u64),
            // A documentação é explícita: o que sai daqui é o total **em uso**, e o total do
            // aparelho vem do `ISHELL_GetDeviceInfo`.
            "GetMemStats" => self.heap.used(),
            "GetModuleMemStats" => {
                // O pico não é acompanhado; devolver o uso corrente nos dois é a resposta
                // honesta mais próxima, e é o que o jogo compara com o que precisa.
                let used = self.heap.used();
                self.write_at(a2, used)?;
                self.write_at(a3, used)?;
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Quantos bytes o heap do guest ainda pode entregar.
    fn heap_available(&self) -> u64 {
        (loader::HEAP_SIZE as u64).saturating_sub(u64::from(self.heap.used()))
    }

    fn sound_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Sound.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.sounds.remove(&this);
                }
                remaining
            }
            // void RegisterNotify(ISound *, PFNSOUNDSTATUS pfn, const void *pUser)
            "RegisterNotify" => {
                self.sounds.entry(this).or_default().notify = Callback {
                    function: a1,
                    context: a2,
                };
                SUCCESS
            }
            // int Set(ISound *, const AEESoundInfo *) — cinco `int8`: eDevice, eMethod, eAPath,
            // eEarMuteCtl, eMicMuteCtl.
            "Set" => {
                if a1 != 0 {
                    let mut info = [0u8; 5];
                    self.cpu.read_mem(a1, &mut info)?;
                    self.sounds.entry(this).or_default().info = info;
                }
                SUCCESS
            }
            "Get" => {
                if a1 != 0 {
                    let info = self.sounds.entry(this).or_default().info;
                    self.cpu.write_mem(a1, &info)?;
                }
                SUCCESS
            }
            "SetDevice" => SUCCESS,
            // Tocar é instantâneo e mudo, mas o jogo costuma esperar o callback de conclusão
            // antes de liberar o recurso de áudio.
            "PlayTone" | "PlayToneList" | "PlayFreqTone" => {
                self.finish_sound(this, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE);
                SUCCESS
            }
            "Vibrate" => {
                self.finish_sound(this, AEE_SOUND_STATUS_CB, AEE_SOUND_PLAY_DONE);
                SUCCESS
            }
            "StopTone" | "StopVibrate" => SUCCESS,
            "SetVolume" => {
                self.sounds.entry(this).or_default().volume = (a1 as u16).min(AEE_MAX_VOLUME);
                SUCCESS
            }
            // `GetVolume` devolve `void`: o valor chega ao jogo pelo callback, com
            // `AEE_SOUND_VOLUME_CB` e o volume em `dwParam`.
            "GetVolume" => {
                let volume = self.sounds.entry(this).or_default().volume;
                self.finish_sound_with(this, AEE_SOUND_VOLUME_CB, AEE_SOUND_SUCCESS, volume as u32);
                SUCCESS
            }
            // Não há disputa por recurso de áudio num emulador de um jogo só.
            "GetResourceCtl" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, 0)?;
                }
                ECLASSNOTSUPPORT
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Enfileira o callback de `ISound` sem parâmetro extra.
    fn finish_sound(&mut self, sound: u32, kind: u32, status: u32) {
        self.finish_sound_with(sound, kind, status, 0);
    }

    /// Enfileira o `PFNSOUNDSTATUS(pUser, eCBType, eSPStatus, dwParam)` do som.
    fn finish_sound_with(&mut self, sound: u32, kind: u32, status: u32, param: u32) {
        let Some(state) = self.sounds.get(&sound) else {
            return;
        };
        if state.notify.function == 0 {
            return;
        }
        let notify = state.notify;
        self.pending_calls.push(GuestCall {
            function: notify.function,
            args: [notify.context, kind, status, param],
        });
    }

    /// Prepara um bitmap para ser usado como `IDIB`: reserva o buffer de pixels na memória do
    /// guest, copia o conteúdo atual para lá e preenche os campos públicos da struct.
    ///
    /// Layout, de `inc/AEEIDIB.h`: `pvt`, `pPaletteMap`, `pBmp`, `pRGB`, `ncTransparent`,
    /// `cx`, `cy`, `nPitch`, `cntRGB`, `nDepth`, `nColorScheme` e seis bytes reservados.
    fn expose_dib(&mut self, bitmap: u32) -> Result<(), CpuError> {
        if self.dib_buffers.contains_key(&bitmap) {
            return Ok(());
        }
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let (cx, cy) = (fb.width(), fb.height());
        let pitch = cx * 2;
        let Some(buffer) = self.surface_alloc(pitch * cy) else {
            return Ok(());
        };

        self.dib_buffers.insert(bitmap, buffer);
        self.sync_to_guest(bitmap)?;
        self.write_dib_header(bitmap)
    }

    /// Escreve os campos públicos do `IDIB` de um bitmap: tamanho, passo, profundidade e o
    /// ponteiro para os pixels — este último só quando eles já existem.
    ///
    /// Um `IBitmap` de software do BREW **é** um `IDIB`: a struct começa com a vtable de
    /// `IBitmap` e segue com campos públicos, e o jogo lê esses campos direto, sem pedir nada.
    /// O Peggle é o caso: ele decodifica o PNG, pergunta o tamanho pelos campos e imprime
    /// `-size 0/0` no log dele quando não acha. Aí monta cada sprite como um quadrado de lado
    /// zero — 76.618 dos 77.208 triângulos de um quadro saíam degenerados, e a tela ficava
    /// preta com o jogo desenhando o tempo todo.
    ///
    /// Fora do decodificador, os pixels continuam sendo alocados só no `QueryInterface`, que é
    /// quando o jogo declara que vai mexer neles: a região de superfícies não recicla, e toda
    /// superfície publicada entra no laço que sincroniza os pixels a cada chamada que os toca.
    /// Escrever o cabeçalho sem os pixels seria pior que não escrever nada — o jogo passa a
    /// confiar no `pBmp` e desreferencia o zero.
    fn write_dib_header(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let (cx, cy) = (fb.width(), fb.height());
        let pitch = cx * 2;
        let buffer = self.dib_buffers.get(&bitmap).copied().unwrap_or(0);
        let transparent = self.transparency.get(&bitmap).copied().unwrap_or(0) as u32;
        self.cpu.write_u32(bitmap + 4, 0)?; // pPaletteMap
        self.cpu.write_u32(bitmap + 8, buffer)?; // pBmp
        self.cpu.write_u32(bitmap + 12, 0)?; // pRGB: RGB565 não tem paleta
        self.cpu.write_u32(bitmap + 16, transparent)?;
        self.cpu
            .write_mem(bitmap + 20, &(cx as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 22, &(cy as u16).to_le_bytes())?;
        self.cpu
            .write_mem(bitmap + 24, &(pitch as i16).to_le_bytes())?;
        self.cpu.write_mem(bitmap + 26, &0u16.to_le_bytes())?; // cntRGB
        // `nDepth` em bits e `nColorScheme` com o código de `AEEIDIB.h`. Sem o esquema correto o
        // jogo não sabe como interpretar os pixels e desiste.
        self.cpu
            .write_mem(bitmap + 28, &[COLOR_DEPTH as u8, IDIB_COLORSCHEME_565])?;
        self.cpu.write_mem(bitmap + 30, &[0u8; 6])?;
        Ok(())
    }

    /// Reserva espaço na região de superfícies.
    fn surface_alloc(&mut self, bytes: u32) -> Option<u32> {
        let addr = self.surface_next;
        let end = loader::SURFACE_BASE + loader::SURFACE_SIZE as u32;
        if addr.checked_add(bytes)? > end {
            return None;
        }
        // Alinha em 4 bytes para que o jogo possa escrever palavras inteiras.
        self.surface_next = (addr + bytes).div_ceil(4) * 4;
        Some(addr)
    }

    /// Copia os pixels do host para o buffer que o jogo enxerga.
    fn sync_to_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let bytes = fb.to_rgb565_bytes();
        self.cpu.write_mem(buffer, &bytes)
    }

    /// Traz de volta o que o jogo escreveu direto no buffer.
    /// O endereço de um pixel dentro do buffer que o jogo enxerga, quando ele existe.
    ///
    /// É o que permite `DrawPixel` e `GetPixel` mexerem em dois bytes em vez de mandarem a
    /// superfície inteira de um lado para o outro.
    fn dib_pixel(&self, bitmap: u32, x: i32, y: i32) -> Option<u32> {
        let &buffer = self.dib_buffers.get(&bitmap)?;
        let fb = self.bitmaps.get(&bitmap)?;
        let (width, height) = (fb.width() as i32, fb.height() as i32);
        if x < 0 || y < 0 || x >= width || y >= height {
            return None;
        }
        Some(buffer + ((y * width + x) as u32) * 2)
    }

    fn sync_from_guest(&mut self, bitmap: u32) -> Result<(), CpuError> {
        let Some(&buffer) = self.dib_buffers.get(&bitmap) else {
            return Ok(());
        };
        let Some(fb) = self.bitmaps.get(&bitmap) else {
            return Ok(());
        };
        let mut bytes = vec![0u8; (fb.width() * fb.height() * 2) as usize];
        self.cpu.read_mem(buffer, &mut bytes)?;
        if let Some(fb) = self.bitmaps.get_mut(&bitmap) {
            fb.load_rgb565_bytes(&bytes);
        }
        Ok(())
    }

    /// Sincroniza todas as superfícies que o jogo pode ter alterado direto.
    ///
    /// Chamado só nas interfaces que mexem em pixels: um jogo faz dezenas de milhares de
    /// chamadas de outras APIs, e copiar 600 KB em cada uma seria inviável.
    fn sync_surfaces_in(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_from_guest(bitmap)?;
        }
        Ok(())
    }

    fn sync_surfaces_out(&mut self) -> Result<(), CpuError> {
        for bitmap in self.dib_buffers.keys().copied().collect::<Vec<_>>() {
            self.sync_to_guest(bitmap)?;
        }
        Ok(())
    }

    /// Copia de uma superfície para outra, respeitando a cor transparente quando o raster op
    /// pede. Precisa tirar o destino do mapa antes para não ter duas referências mutáveis.
    fn blit(
        &mut self,
        dst: u32,
        dst_pos: (i32, i32),
        size: (i32, i32),
        src: u32,
        src_pos: (i32, i32),
        rop: u32,
    ) {
        if dst == src {
            return;
        }
        let Some(mut target) = self.bitmaps.remove(&dst) else {
            return;
        };
        if let Some(source) = self.bitmaps.get(&src) {
            let transparent = if rop == AEE_RO_TRANSPARENT {
                self.transparency.get(&src).copied()
            } else {
                None
            };
            target.blit(
                dst_pos.0,
                dst_pos.1,
                size.0,
                size.1,
                source,
                src_pos.0,
                src_pos.1,
                transparent,
            );
        }
        self.bitmaps.insert(dst, target);
    }

    /// Argumento além dos quatro registradores, contado a partir do topo da pilha.
    fn stack_arg(&self, index: u32) -> Result<u32, CpuError> {
        self.cpu.read_u32(self.cpu.read_reg(Reg::Sp) + index * 4)
    }

    /// Lê um `AEERect` da memória do guest. `None` quando o ponteiro é nulo.
    fn clip_rect(&self, rect: Rect) -> Option<Rect> {
        clip_rect(self.clip, rect)
    }

    fn clip_blit(&self, dst: (i32, i32), size: (i32, i32), src: (i32, i32)) -> Option<Blit> {
        clip_blit(self.clip, dst, size, src)
    }

    fn read_rect(&self, addr: u32) -> Result<Option<Rect>, CpuError> {
        if addr == 0 {
            return Ok(None);
        }
        let mut bytes = [0u8; 8];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(Some(Rect::from_bytes(bytes)))
    }

    /// Lê uma string `AECHAR` — UTF-16 little-endian terminada em zero, que é como o BREW
    /// representa texto.
    /// Lê uma string larga do guest como as unidades UTF-16 cruas.
    ///
    /// Existe separada da versão em `String` porque `wstrchr` e as comparações trabalham em
    /// cima de `AECHAR`, e converter para `String` perderia a posição de cada caractere.
    fn read_aechar_units(&self, addr: u32) -> Result<Vec<u16>, CpuError> {
        if addr == 0 {
            return Ok(Vec::new());
        }
        let mut units = Vec::new();
        for i in 0..MAX_STRING {
            let mut bytes = [0u8; 2];
            if self.cpu.read_mem(addr + i as u32 * 2, &mut bytes).is_err() {
                break;
            }
            match u16::from_le_bytes(bytes) {
                0 => break,
                unit => units.push(unit),
            }
        }
        Ok(units)
    }

    fn read_aechar(&self, addr: u32) -> Result<String, CpuError> {
        if addr == 0 {
            return Ok(String::new());
        }
        let mut units = Vec::new();
        for i in 0..MAX_STRING {
            let mut bytes = [0u8; 2];
            if self.cpu.read_mem(addr + i as u32 * 2, &mut bytes).is_err() {
                break;
            }
            let unit = u16::from_le_bytes(bytes);
            if unit == 0 {
                break;
            }
            units.push(unit);
        }
        Ok(String::from_utf16_lossy(&units))
    }

    /// Helpers da stdlib do BREW, despachados pelo nome do slot — a tabela vem de
    /// `struct AEEHelperFuncs`, no `AEEStdLib.h` do SDK.
    fn helper_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Helpers.method(slot) else {
            return Ok(None);
        };
        let (a0, a1, a2) = (
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
        );
        let result = match name {
            // void *SetupNativeImage(AEECLSID cls, void *pBuffer, AEEImageInfo *pii,
            //                         boolean *pbRealloc)
            //
            // Converte uma imagem codificada para o formato nativo do aparelho. É o que está
            // por trás do `CONVERTBMP` do SDK, e os dois jogos que a chamam passam
            // `AEECLSID_WINBMP` com um bitmap do Windows.
            "SetupNativeImage" => self.setup_native_image(a1, a2, self.cpu.read_reg(Reg::R3))?,
            "malloc" => {
                let r = self.malloc(a0)?;
                r
            }
            "free" => {
                self.heap.free(a0);
                SUCCESS
            }
            "realloc" => self.realloc(a0, a1 & !ALLOC_NO_ZMEM)?,
            "memmove" => {
                self.copy_guest(a0, a1, a2)?;
                a0
            }
            "memset" => {
                if a2 > 0 {
                    self.cpu.write_mem(a0, &vec![a1 as u8; a2 as usize])?;
                }
                a0
            }
            "memcmp" => {
                let (left, right) = (self.read_bytes(a0, a2)?, self.read_bytes(a1, a2)?);
                cmp_to_int(left.cmp(&right))
            }
            // As funções de string do C contam **bytes**, não caracteres: qualquer byte acima
            // de 0x7f faria a versão em `String` mentir.
            "strlen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "strcpy" => {
                let mut src = self.cpu.read_cbytes(a1, MAX_STRING);
                src.push(0);
                self.cpu.write_mem(a0, &src)?;
                a0
            }
            "strcat" => {
                let base = self.cpu.read_cbytes(a0, MAX_STRING).len() as u32;
                let mut extra = self.cpu.read_cbytes(a1, MAX_STRING);
                extra.push(0);
                self.cpu.write_mem(a0 + base, &extra)?;
                a0
            }
            "strcmp" => {
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                cmp_to_int(left.cmp(&right))
            }
            "strncmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING);
                let right = self.cpu.read_cbytes(a1, MAX_STRING);
                // `strncmp` compara no máximo `n` bytes; uma string mais curta vale inteira.
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "wstrlen" => self.read_aechar(a0)?.encode_utf16().count() as u32,
            "wstrsize" => (self.read_aechar(a0)?.encode_utf16().count() as u32 + 1) * 2,
            // AECHAR *strtowstr(const char *pszIn, AECHAR *pDest, int nSize) — `nSize` é em
            // bytes, não em caracteres.
            "strtowstr" => {
                let text = self.cpu.read_cstring(a0, MAX_STRING);
                self.write_aechar(a1, &text, a2 as usize / 2)?;
                a1
            }
            // char *wstrtostr(const AECHAR *pIn, char *pszDest, int nSize)
            "wstrtostr" => {
                let text = self.read_aechar(a0)?;
                self.write_cstring_limited(a1, &text, a2 as usize)?;
                a1
            }
            "wstrcpy" => {
                let text = self.read_aechar(a1)?;
                self.write_aechar(a0, &text, usize::MAX)?;
                a0
            }
            "wstrcat" => {
                let base = self.read_aechar(a0)?;
                let extra = self.read_aechar(a1)?;
                let offset = base.encode_utf16().count() as u32 * 2;
                self.write_aechar(a0 + offset, &extra, usize::MAX)?;
                a0
            }
            "wstrcmp" => {
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.cmp(&right))
            }
            "wstrncmp" => {
                let take = a2 as usize;
                let (left, right) = (self.read_aechar(a0)?, self.read_aechar(a1)?);
                cmp_to_int(left.get(..take).cmp(&right.get(..take)))
            }
            // char *strncpy(char *dst, const char *src, size_t n) — sem terminador se `src`
            // não couber, como manda a função original.
            "strncpy" => {
                let n = a2 as usize;
                let mut bytes = self.cpu.read_cbytes(a1, MAX_STRING);
                bytes.truncate(n);
                bytes.resize(n, 0);
                if n > 0 {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            "strchr" | "strrchr" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let found = if name == "strchr" {
                    text.iter().position(|&b| b == needle)
                } else {
                    text.iter().rposition(|&b| b == needle)
                };
                found.map(|i| a0 + i as u32).unwrap_or(0)
            }
            "strstr" => {
                let haystack = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                haystack
                    .windows(needle.len().max(1))
                    .position(|w| w == needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            "stricmp" => {
                let fold = |addr| {
                    self.cpu
                        .read_cbytes(addr, MAX_STRING)
                        .iter()
                        .map(u8::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                };
                cmp_to_int(fold(a0).cmp(&fold(a1)))
            }
            "memchr" => {
                let bytes = self.read_bytes(a0, a2)?;
                bytes
                    .iter()
                    .position(|&b| b == a1 as u8)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // --- Strings largas (`AECHAR`, UTF-16) -------------------------------------
            "wstrchr" | "wstrrchr" => {
                let units = self.read_aechar_units(a0)?;
                let needle = a1 as u16;
                let found = if name == "wstrchr" {
                    units.iter().position(|&u| u == needle)
                } else {
                    units.iter().rposition(|&u| u == needle)
                };
                found.map(|i| a0 + i as u32 * 2).unwrap_or(0)
            }
            "wstrdup" => {
                let units = self.read_aechar_units(a0)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            "wstrlower" | "wstrupper" => {
                let units = self.read_aechar_units(a0)?;
                let mapped: Vec<u8> = units
                    .iter()
                    .map(|&u| match u8::try_from(u) {
                        Ok(b) if name == "wstrlower" => b.to_ascii_lowercase() as u16,
                        Ok(b) => b.to_ascii_uppercase() as u16,
                        Err(_) => u,
                    })
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                if !mapped.is_empty() {
                    self.cpu.write_mem(a0, &mapped)?;
                }
                a0
            }
            "wstricmp" | "wstrnicmp" => {
                let take = if name == "wstricmp" {
                    usize::MAX
                } else {
                    a2 as usize
                };
                let left = self.read_aechar_units(a0)?;
                let right = self.read_aechar_units(a1)?;
                cmp_to_int(fold_case(&left, take).cmp(&fold_case(&right, take)))
            }
            // `size_t wstrlcpy/wstrlcat(AECHAR *dst, const AECHAR *src, size_t nSize)`: as
            // versões BSD, que devolvem o tamanho que a origem *teria* ocupado. `nSize` conta
            // caracteres, não bytes.
            "wstrlcpy" | "wstrlcat" => {
                let source = self.read_aechar(a1)?;
                let existing = if name == "wstrlcat" {
                    self.read_aechar_units(a0)?.len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_aechar(a0 + existing as u32 * 2, &source, room)?;
                (existing + source.encode_utf16().count()) as u32
            }
            // `AECHAR *wwritelong(AECHAR *pszBuf, long n)` — escreve o número e devolve o
            // ponteiro para o terminador, para o chamador continuar dali.
            "wwritelong" => {
                let text = (a1 as i32).to_string();
                self.write_aechar(a0, &text, usize::MAX)?;
                a0 + text.len() as u32 * 2
            }
            // `int wstrncopyn(AECHAR *dst, int cbDest, const AECHAR *src, int lenSource)`:
            // `cbDest` conta caracteres e `lenSource` limita a origem (-1 = até o terminador).
            "wstrncopyn" => {
                let mut units = self.read_aechar_units(a2)?;
                let limit = self.cpu.read_reg(Reg::R3) as i32;
                if limit >= 0 {
                    units.truncate(limit as usize);
                }
                let text = String::from_utf16_lossy(&units);
                self.write_aechar(a0, &text, a1 as usize)?;
                text.encode_utf16()
                    .count()
                    .min((a1 as usize).saturating_sub(1)) as u32
            }
            // `void wsprintf(AECHAR *dst, int nSize, const AECHAR *fmt, ...)` — mesma
            // gramática do `snprintf`, com origem e destino em UTF-16.
            "wsprintf" => {
                let fmt = self.read_aechar(a2)?;
                let text = self.format_from(3, &fmt);
                self.write_aechar(a0, &text, a1 as usize / 2)?;
                SUCCESS
            }
            // `void strexpand(const byte *pSrc, int nCount, AECHAR *pDest, int nSize)`
            "strexpand" => {
                let bytes = self.read_bytes(a0, a1)?;
                let text: String = bytes.iter().map(|&b| b as char).collect();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &text, limit)?;
                SUCCESS
            }
            "wstrtofloat" => {
                let text = self.read_aechar(a0)?;
                let (value, _) = parse_leading_double(&text);
                self.return_double(value)
            }
            // `boolean floattowstr(double val, AECHAR *psz, int nSize)`: o `double` ocupa
            // `r0:r1`, então o destino cai em `r2` e o tamanho em `r3`.
            "floattowstr" => {
                let value = fmath::from_words(a0, a1);
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &format!("{value}"), limit)?;
                TRUE
            }
            // `boolean utf8towstr(const byte *pszIn, int nLen, AECHAR *pDest, int nSizeBytes)`
            "utf8towstr" => {
                let bytes = if (a1 as i32) < 0 {
                    self.cpu.read_cbytes(a0, MAX_STRING)
                } else {
                    self.read_bytes(a0, a1)?
                };
                let text = String::from_utf8_lossy(&bytes).into_owned();
                let limit = self.cpu.read_reg(Reg::R3) as usize / 2;
                self.write_aechar(a2, &text, limit)?;
                TRUE
            }
            // `boolean wstrtoutf8(const AECHAR *pszIn, int nLen, byte *pDest, int nSizeBytes)`
            "wstrtoutf8" => {
                let mut units = self.read_aechar_units(a0)?;
                if (a1 as i32) >= 0 {
                    units.truncate(a1 as usize);
                }
                let text = String::from_utf16_lossy(&units);
                self.write_cstring_limited(a2, &text, self.cpu.read_reg(Reg::R3) as usize)?;
                TRUE
            }

            // --- Strings de bytes -----------------------------------------------------
            "strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // void qsort(void *base, size_t nmemb, size_t size, int (*compar)(const void *,
            //            const void *))
            //
            // A comparação é uma função do **jogo**, então ordenar significa reentrar no guest
            // a cada par. É o mesmo desvio que os callbacks pendentes fazem, e no mesmo ponto:
            // a chamada de API terminou e o guest ainda não retomou.
            "qsort" => {
                let compare = self.cpu.read_reg(Reg::R3);
                self.qsort(a0, a1, a2, compare)?;
                SUCCESS
            }
            // `uint32 strtoul(const char *nptr, char **endptr, int base)`
            "strtoul" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_unsigned(&text, a2);
                if a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                value
            }
            "strnicmp" => {
                let take = a2 as usize;
                let left = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let right = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                cmp_to_int(left[..take.min(left.len())].cmp(&right[..take.min(right.len())]))
            }
            "stristr" => {
                let hay = self.cpu.read_cbytes(a0, MAX_STRING).to_ascii_lowercase();
                let needle = self.cpu.read_cbytes(a1, MAX_STRING).to_ascii_lowercase();
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `char *memstr(const char *cpHaystack, const char *cpszNeedle, size_t nLen)` — o
            // palheiro tem tamanho fixo; a agulha continua terminada em NUL.
            "memstr" => {
                let hay = self.read_bytes(a0, a2)?;
                let needle = self.cpu.read_cbytes(a1, MAX_STRING);
                find_subslice(&hay, &needle)
                    .map(|i| a0 + i as u32)
                    .unwrap_or(0)
            }
            // `boolean strbegins(const char *cpszPrefix, const char *psz)` e o par `strends`:
            // o pedaço procurado vem **primeiro**.
            "strbegins" | "strends" | "aee_stribegins" => {
                let part = self.cpu.read_cbytes(a0, MAX_STRING);
                let whole = self.cpu.read_cbytes(a1, MAX_STRING);
                let matched = match name {
                    "strbegins" => whole.starts_with(&part),
                    "strends" => whole.ends_with(&part),
                    _ => whole
                        .to_ascii_lowercase()
                        .starts_with(&part.to_ascii_lowercase()),
                };
                u32::from(matched)
            }
            // `char *strchrend(const char *pszSrc, char c)` — como o `strchr`, mas quando não
            // acha devolve o terminador em vez de zero.
            "strchrend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let needle = a1 as u8;
                let at = text.iter().position(|&b| b == needle).unwrap_or(text.len());
                a0 + at as u32
            }
            // `char *strchrsend(const char *pszSrc, const char *pszChars)` — o primeiro byte
            // que aparecer no conjunto, ou o terminador.
            "strchrsend" => {
                let text = self.cpu.read_cbytes(a0, MAX_STRING);
                let set = self.cpu.read_cbytes(a1, MAX_STRING);
                let at = text
                    .iter()
                    .position(|b| set.contains(b))
                    .unwrap_or(text.len());
                a0 + at as u32
            }
            // A família `mem*` do BREW trabalha sobre um bloco de tamanho fixo: `memrchr` acha
            // a última ocorrência, `memchrend`/`memrchrbegin` devolvem os limites do bloco
            // quando não acham.
            "memrchr" | "memchrend" | "memrchrbegin" => {
                let block = self.read_bytes(a0, a2)?;
                let needle = a1 as u8;
                let at = match name {
                    "memchrend" => block
                        .iter()
                        .position(|&b| b == needle)
                        .unwrap_or(block.len()),
                    "memrchrbegin" => block.iter().rposition(|&b| b == needle).unwrap_or_default(),
                    _ => match block.iter().rposition(|&b| b == needle) {
                        Some(i) => i,
                        None => return Ok(Some(0)),
                    },
                };
                a0 + at as u32
            }
            "strlower" | "strupper" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                if name == "strlower" {
                    bytes.make_ascii_lowercase();
                } else {
                    bytes.make_ascii_uppercase();
                }
                if !bytes.is_empty() {
                    self.cpu.write_mem(a0, &bytes)?;
                }
                a0
            }
            // `size_t strlcpy/strlcat(char *dst, const char *src, size_t nSize)`, à moda BSD.
            "strlcpy" | "strlcat" => {
                let source = self.cpu.read_cstring(a1, MAX_STRING);
                let existing = if name == "strlcat" {
                    self.cpu.read_cbytes(a0, MAX_STRING).len()
                } else {
                    0
                };
                let room = (a2 as usize).saturating_sub(existing);
                self.write_cstring_limited(a0 + existing as u32, &source, room)?;
                (existing + source.len()) as u32
            }
            "OEMStrLen" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32,
            "OEMStrSize" => self.cpu.read_cbytes(a0, MAX_STRING).len() as u32 + 1,
            "swapl" => a0.swap_bytes(),
            "swaps" => (a0 as u16).swap_bytes() as u32,

            // --- Memória, versão e depuração ------------------------------------------
            "sysfree" => {
                self.heap.free(a0);
                SUCCESS
            }
            "err_strdup" => {
                let mut bytes = self.cpu.read_cbytes(a0, MAX_STRING);
                bytes.push(0);
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr == 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_mem(ptr, &bytes)?;
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `int err_realloc(uint32 uSize, void **pp)` — o `realloc` que devolve código de
            // erro e só troca o ponteiro se der certo.
            "err_realloc" => {
                let current = self.cpu.read_u32(a1)?;
                let ptr = self.realloc(current, a0)?;
                if ptr == 0 && a0 != 0 {
                    ENOMEMORY
                } else {
                    self.cpu.write_u32(a1, ptr)?;
                    SUCCESS
                }
            }
            // `uint32 GetAEEVersion(byte *pszFormatted, int nSize, uint16 wFlags)`: byte alto
            // da palavra alta é a versão maior, e assim por diante — daí `4.0.2.0`.
            "GetAEEVersion" => {
                if a0 != 0 {
                    if a2 & GAV_LATIN1 != 0 {
                        self.write_cstring_limited(a0, AEE_VERSION_TEXT, a1 as usize)?;
                    } else {
                        self.write_aechar(a0, AEE_VERSION_TEXT, a1 as usize / 2)?;
                    }
                }
                AEE_VERSION
            }
            // `uint32 GetFSFree(uint32 *pdwTotal)` — não temos cota de sistema de arquivos, e
            // responder um número grande é mais fiel que responder zero.
            "GetFSFree" => {
                if a0 != 0 {
                    self.cpu.write_u32(a0, FS_TOTAL)?;
                }
                FS_TOTAL
            }
            "aee_GetUTCSeconds" => self.elapsed_ms() / 1000,
            // `int32 aee_LocalTimeOffset(boolean *pbDaylightSavings)` — o emulador roda em UTC.
            "aee_LocalTimeOffset" => {
                if a0 != 0 {
                    self.cpu.write_mem(a0, &[0])?;
                }
                0
            }
            // Ganchos de depuração do BREW: existem para o log da plataforma, e o
            // comportamento correto sem ele é não fazer nada.
            "dumpheap" | "dbgevent" => SUCCESS,
            "dbgheapmark" => a0,
            // `int lockmem/unlockmem(void **ppHandle)` — o BREW do aparelho pode mover blocos
            // e por isso trava; a nossa heap é fixa, então travar é sempre um sucesso.
            "lockmem" | "unlockmem" => TRUE,
            // `char *aee_basename(const char *cpszPath)`
            "aee_basename" => {
                let path = self.cpu.read_cbytes(a0, MAX_STRING);
                let at = path
                    .iter()
                    .rposition(|&b| b == b'/' || b == b'\\')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                a0 + at as u32
            }
            "atoi" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                text.trim().parse::<i32>().unwrap_or(0) as u32
            }
            // A família de ponto flutuante da stdlib. Na AAPCS um `double` ocupa um par de
            // registradores com a palavra baixa primeiro, então `v1` vem em `r0:r1`, `v2` em
            // `r2:r3` e o que sobra vai para a pilha.
            "f_op" | "f_cmp" => {
                let (v1, v2) = (
                    fmath::from_words(a0, a1),
                    fmath::from_words(a2, self.cpu.read_reg(Reg::R3)),
                );
                let kind = self.cpu.read_u32(self.cpu.read_reg(Reg::Sp))?;
                if name == "f_cmp" {
                    let Some(answer) = fmath::cmp(v1, v2, kind) else {
                        return Ok(None);
                    };
                    u32::from(answer)
                } else {
                    let Some(value) = fmath::op(v1, v2, kind) else {
                        return Ok(None);
                    };
                    self.return_double(value)
                }
            }
            "f_calc" => {
                let Some(value) = fmath::calc(fmath::from_words(a0, a1), a2) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_get" => {
                let Some(value) = fmath::get(a0) else {
                    return Ok(None);
                };
                self.return_double(value)
            }
            "f_assignint" => self.return_double(a0 as i32 as f64),
            "f_assignstr" | "strtod" => {
                let text = self.cpu.read_cstring(a0, MAX_NUMBER);
                let (value, consumed) = parse_leading_double(&text);
                // O `strtod` ainda devolve, pelo `char **`, onde parou de ler.
                if name == "strtod" && a1 != 0 {
                    self.cpu.write_u32(a1, a0 + consumed as u32)?;
                }
                self.return_double(value)
            }
            // `f_toint` arredonda para zero, como o cast do C; `trunc`/`utrunc` são a mesma
            // conta com o sinal do resultado mudando.
            "f_toint" | "trunc" => fmath::from_words(a0, a1).trunc() as i32 as u32,
            "utrunc" => fmath::from_words(a0, a1).trunc() as u32,
            // int vsnprintf(char *dst, int nSize, const char *fmt, va_list args) e
            // int vsprintf(char *dst, const char *fmt, va_list args).
            //
            // Na AAPCS o `va_list` é um ponteiro para a área de argumentos, então basta ler
            // palavras em sequência a partir dele.
            "vsnprintf" | "vsprintf" => {
                let (fmt_addr, args_addr, limit) = if name == "vsnprintf" {
                    (a2, self.cpu.read_reg(Reg::R3), a1 as usize)
                } else {
                    (a1, a2, usize::MAX)
                };
                let fmt = self.cpu.read_cstring(fmt_addr, MAX_STRING);
                // `AEEOldVaList` é `int **` no ARM (`inc/AEEOldVaList.h`): o que chega é o
                // endereço da variável `va_list`, não a área de argumentos. Sem essa
                // indireção o primeiro `%d` imprime o próprio ponteiro da pilha — foi o que
                // transformou `inva1_shotgun` em `inva537919380_shotgun` no Quake.
                let mut source = GuestArgs {
                    words: Vec::new(),
                    index: 0,
                    stack: self.cpu.read_u32(args_addr)?,
                    cpu: &self.cpu,
                };
                let text = cformat::format(&fmt, &mut source);
                self.write_cstring_limited(a0, &text, limit)?;
                text.len() as u32
            }
            // int snprintf(char *dst, int nSize, const char *fmt, ...)
            "snprintf" => {
                let fmt = self.cpu.read_cstring(a2, MAX_STRING);
                let text = self.format_from(3, &fmt);
                self.write_cstring_limited(a0, &text, a1 as usize)?;
                text.len() as u32
            }
            // `sprintf(char *dst, const char *fmt, ...)`: os variádicos começam em `r2`.
            "sprintf" => {
                let fmt = self.cpu.read_cstring(a1, MAX_STRING);
                let text = self.format_from(2, &fmt);
                self.write_cstring(a0, &text)?;
                text.len() as u32
            }
            "dbgprintf" => {
                let fmt = self.cpu.read_cstring(a0, MAX_STRING);
                let text = self.format_from(1, &fmt);
                self.record_debug(text);
                SUCCESS
            }
            // `IApplet *GetAppInstance(void)` — sem argumentos, o que explica por que o site
            // de chamada podia usar `r0` como rascunho para o ponteiro da função.
            //
            // O jogo chama isto *durante* a própria criação, depois que `AEEApplet_New` já
            // gravou o ponteiro no `void **ppObj` que passamos. Então, enquanto não terminamos
            // de criar o applet, a resposta certa é justamente o conteúdo desse ponteiro.
            "GetAppInstance" => match self.current_applet {
                0 => self.cpu.read_u32(self.module.out_module + 4)?,
                applet => applet,
            },
            // `GETUPTIMEMS` é desde que o aparelho ligou; `GETTIMESECONDS` é o **calendário**,
            // segundos desde 6 de janeiro de 1980. Responder o tempo ligado nos dois era dizer
            // que hoje é o dia da estreia do console: o Z-Wheel calcula o alarme do "próximo
            // dia" a partir daí e ficava girando — quinze milhões de voltas em dois métodos.
            "aee_GetTimeMS" | "aee_GetUpTimeMS" => self.elapsed_ms(),
            "aee_GetSeconds" => self.brew_seconds(),
            // void GETJULIANDATE(uint32 dwSecs, JulianType *pDate)
            //
            // `dwSecs` é o relógio do BREW: segundos desde 6 de janeiro de 1980, GMT. Zero quer
            // dizer "agora", e o nosso agora é o relógio virtual — o mesmo que responde ao
            // `GetSeconds`, para que as duas contas nunca se contradigam.
            "aee_GetJulianDate" => {
                // Helper não tem `this`: o primeiro argumento é o `r0`.
                let segundos = match a0 {
                    0 => self.brew_seconds(),
                    dado => dado,
                };
                if a1 != 0 {
                    let data = julian_date(segundos);
                    for (i, campo) in data.iter().enumerate() {
                        self.cpu
                            .write_mem(a1 + i as u32 * 2, &campo.to_le_bytes())?;
                    }
                }
                SUCCESS
            }
            // Gerador simples e determinístico: repetir a mesma sessão tem que dar o mesmo
            // resultado, senão depurar jogo com aleatoriedade vira loteria.
            "aee_GetRand" => {
                let count = a1;
                for i in 0..count {
                    self.random_state = self
                        .random_state
                        .wrapping_mul(1_103_515_245)
                        .wrapping_add(12_345);
                    self.cpu
                        .write_mem(a0 + i, &[(self.random_state >> 16) as u8])?;
                }
                SUCCESS
            }
            "GetRAMFree" => (loader::HEAP_SIZE as u32).saturating_sub(self.heap.used()),
            // Dormir de verdade só atrasaria o emulador: o tempo do guest anda pelo relógio do
            // host de qualquer jeito.
            // void sleep(uint32 msecs) — o `MSLEEP` do SDK.
            //
            // No console o ARM realmente para e o tempo passa sozinho. Aqui o relógio anda com
            // as instruções, então quem dorme sem que o relógio avance dorme para sempre: o
            // Magical Drop 3 chamava isto cem milhões de vezes num laço que nunca vencia.
            // Adiantar o relógio é o que o aparelho faz.
            "sleep" => {
                self.clock_us += u64::from(a0.min(MAX_SLEEP_MS)) * 1000;
                SUCCESS
            }
            "getlasterror" => self.file_error,
            "aee_IsBadPtr" => u32::from(self.cpu.read_u32(a0).is_err()),
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Prepara o retorno de um `double`: a palavra alta vai para `r1`, e a baixa é o valor
    /// que o despacho grava em `r0`.
    fn return_double(&mut self, value: f64) -> u32 {
        let (low, high) = fmath::to_words(value);
        self.cpu.write_reg(Reg::R1, high);
        low
    }

    /// Milissegundos desde o início da execução.
    fn elapsed_ms(&self) -> u32 {
        self.now_ms()
    }

    /// Formata uma string do guest tomando os variádicos a partir do registrador `first`.
    fn format_from(&self, first: usize, fmt: &str) -> String {
        let words = [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ];
        let mut source = GuestArgs {
            words: words[first..].to_vec(),
            index: 0,
            stack: self.cpu.read_reg(Reg::Sp),
            cpu: &self.cpu,
        };
        cformat::format(fmt, &mut source)
    }

    fn read_bytes(&self, addr: u32, len: u32) -> Result<Vec<u8>, CpuError> {
        let mut buf = vec![0u8; len as usize];
        if len > 0 {
            self.cpu.read_mem(addr, &mut buf)?;
        }
        Ok(buf)
    }

    /// Copia dentro da memória do guest. Passa pelo host, então regiões sobrepostas ficam
    /// corretas — que é justamente o que `memmove` promete.
    fn copy_guest(&mut self, dst: u32, src: u32, len: u32) -> Result<(), CpuError> {
        let bytes = self.read_bytes(src, len)?;
        if len > 0 {
            self.cpu.write_mem(dst, &bytes)?;
        }
        Ok(())
    }

    /// Escreve uma string `AECHAR` (UTF-16 little-endian, terminada em zero).
    ///
    /// `max_units` limita quantos `AECHAR` cabem no destino, terminador incluído.
    fn write_aechar(&mut self, addr: u32, text: &str, max_units: usize) -> Result<(), CpuError> {
        if addr == 0 || max_units == 0 {
            return Ok(());
        }
        let mut units: Vec<u16> = text.encode_utf16().collect();
        units.truncate(max_units.saturating_sub(1));
        units.push(0);
        let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Escreve uma string C respeitando o tamanho do destino.
    fn write_cstring_limited(
        &mut self,
        addr: u32,
        text: &str,
        max_bytes: usize,
    ) -> Result<(), CpuError> {
        if addr == 0 || max_bytes == 0 {
            return Ok(());
        }
        // ISO-8859-1, como na leitura: um caractere, um byte. Além de manter o acento, é o
        // que faz o corte por tamanho cair sempre em fronteira de caractere.
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.truncate(max_bytes.saturating_sub(1));
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes)
    }

    fn write_cstring(&mut self, addr: u32, text: &str) -> Result<(), CpuError> {
        let mut bytes = crate::cpu::latin1_encode(text);
        bytes.push(0);
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} ao escrever {} bytes em {addr:#010x}",
                bytes.len()
            ))
        })
    }

    /// `realloc`: aloca o novo tamanho e copia o conteúdo antigo.
    fn realloc(&mut self, ptr: u32, size: u32) -> Result<u32, CpuError> {
        if ptr == 0 {
            return self.malloc(size);
        }
        let old_size = self.heap.size_of(ptr).unwrap_or(0);
        let new_ptr = self.malloc(size)?;
        if new_ptr != 0 && old_size > 0 {
            self.copy_guest(new_ptr, ptr, old_size.min(size))?;
        }
        self.heap.free(ptr);
        Ok(new_ptr)
    }

    /// Guarda uma linha de log, agrupando repetições em vez de encher o relatório.
    fn record_debug(&mut self, message: String) {
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

    /// `ISHELL_CreateInstance(IShell *po, AEECLSID ClsId, void **ppobj)`.
    ///
    /// É a fábrica de objetos do BREW inteiro. Cada ClassID conhecido vira um objeto nosso com
    /// a vtable da interface correspondente; o que não conhecemos devolve `ECLASSNOTSUPPORT`,
    /// que é uma resposta legítima — o jogo trata classe ausente como plataforma sem aquele
    /// recurso, em vez de quebrar.
    fn shell_create_instance(&mut self) -> Result<u32, CpuError> {
        let clsid = self.cpu.read_reg(Reg::R1);
        let out = self.cpu.read_reg(Reg::R2);

        let iface = match clsid {
            AEECLSID_DISPLAY | AEECLSID_DISPLAY1 => Interface::Display,
            AEECLSID_FILEMGR => Interface::FileMgr,
            AEECLSID_HID => Interface::Hid,
            AEECLSID_SIGNAL_CB_FACTORY => Interface::SignalCbFactory,
            AEECLSID_GRAPHICS => Interface::Graphics,
            AEECLSID_SOUND => Interface::Sound,
            AEECLSID_HEAP => Interface::Heap,
            AEECLSID_UNZIPSTREAM => Interface::UnzipStream,
            AEECLSID_LICENSE => Interface::License,
            AEECLSID_MEMASTREAM => Interface::MemAStream,
            AEECLSID_PNG => Interface::Image,
            AEECLSID_PNGDECODER | AEECLSID_PNGDECODER_BREW => Interface::ImageDecoder,
            AEECLSID_THREAD => Interface::Thread,
            AEECLSID_QEGL => Interface::Egl,
            AEECLSID_EGL => Interface::EglLegacy,
            AEECLSID_GL => Interface::GlLegacy,
            AEECLSID_MEDIAUTIL => Interface::MediaUtil,
            AEECLSID_WEB => Interface::Web,
            AEECLSID_COLLECTION => Interface::Collection,
            AEECLSID_SQLMGR => Interface::SqlMgr,
            AEECLSID_SOURCEUTIL => Interface::SourceUtil,
            _ if FAMILIA_DOS_WIDGETS.contains(&clsid) => Interface::Widget,
            AEECLSID_ZEEBOMCP => Interface::ZeeboMcp,
            AEECLSID_CONFIG => Interface::Config,
            AEECLSID_VETOR => Interface::Vetor,
            AEECLSID_28E3C => Interface::Classe28e3c,
            AEECLSID_CM => Interface::Cm,
            AEECLSID_SYSTEMCTL => Interface::SystemCtl,
            AEECLSID_TYPEFACE => Interface::Typeface,
            AEECLSID_MD5 => Interface::Hash,
            AEECLSID_CIPHER_FACTORY => Interface::CipherFactory,
            AEECLSID_MEDIA | AEECLSID_MEDIAMIDI | AEECLSID_MEDIAMP3 | AEECLSID_MEDIAADPCM
            | AEECLSID_MEDIAPCM => Interface::Media,
            // A sonda entra antes da recusa: o jogo recebe um objeto que não faz nada e segue,
            // e o que ele chamar nele vai para o relatório. É como se descobre que interface a
            // classe é, sem header e sem adivinhação.
            _ if self.probe_classes.contains(&clsid) => {
                let object = self.new_object(Interface::Probe)?;
                if object == 0 {
                    return Ok(ENOMEMORY);
                }
                self.probe_objects.insert(object, clsid);
                if out != 0 {
                    self.cpu.write_u32(out, object)?;
                }
                return Ok(SUCCESS);
            }
            _ => {
                self.unknown_classes.insert(clsid);
                if out != 0 {
                    self.cpu.write_u32(out, 0)?;
                }
                return Ok(ECLASSNOTSUPPORT);
            }
        };

        let Some(obj) = self.objects.create(iface) else {
            return Ok(ENOMEMORY);
        };
        self.cpu.write_u32(obj, loader::vtable_addr(iface))?;
        // Uma coleção nasce vazia e com o cursor no começo. Sem esse registro ela não existiria
        // para os métodos, e um `AtEnd` numa coleção desconhecida responderia "acabou" por
        // acaso — a resposta certa pelo motivo errado.
        if iface == Interface::Collection {
            self.collections.insert(obj, (Vec::new(), 0));
        }
        // Pelo mesmo motivo da coleção: um widget sem registro responderia "não tenho esse
        // filho" por não existir, e não por não ter o filho.
        // Mesma razão da coleção: sem o registro, um `Tamanho` numa lista desconhecida
        // responderia zero por ela não existir, e não por estar vazia.
        if iface == Interface::Vetor {
            self.vetores.insert(obj, (Vec::new(), 0));
        }
        if iface == Interface::Widget {
            // Um widget nasce visível: o jogo só chama o slot 6 para **esconder**.
            self.widgets.insert(obj, Widget { visivel: true, ..Widget::default() });
        }
        if out != 0 {
            self.cpu.write_u32(out, obj)?;
        }
        Ok(SUCCESS)
    }

    /// `MALLOC` do BREW: devolve memória, ou 0 se não houver espaço.
    ///
    /// O tamanho pode vir com `ALLOC_NO_ZMEM` (`0x80000000`) no bit alto, pedindo memória
    /// **sem** zerar — é o que o `malloc` da libc do SDK faz (`MALLOC(size|ALLOC_NO_ZMEM)`).
    /// Ignorar essa flag faz o emulador tentar alocar dois gigabytes e devolver nulo, e o jogo
    /// conclui, corretamente, que ficou sem memória.
    fn malloc(&mut self, size: u32) -> Result<u32, CpuError> {
        let zero = size & ALLOC_NO_ZMEM == 0;
        let size = size & !ALLOC_NO_ZMEM;
        let Some(addr) = self.heap.alloc(size) else {
            return Ok(0);
        };
        // Blocos reaproveitados carregam lixo do dono anterior; zeramos quando pedido.
        if zero && size > 0 {
            self.cpu.write_mem(addr, &vec![0u8; size as usize])?;
        }
        Ok(addr)
    }

    fn args(&self) -> [u32; 4] {
        [
            self.cpu.read_reg(Reg::R0),
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        ]
    }

    /// Cria a instância do applet chamando `IModule::CreateInstance` no módulo carregado.
    ///
    /// Assinatura, de `AEEModGen.c`:
    /// `int AEEMod_CreateInstance(IModule *po, IShell *pIShell, AEECLSID ClsId, void **ppObj)`.
    /// `CreateInstance` é o slot 2 da vtable de `IModule` (depois de `AddRef` e `Release`).
    pub fn create_applet(&mut self, clsid: u32, budget: u64) -> Result<AppletResult, CpuError> {
        self.applet_class = clsid;
        let module_ptr = self.cpu.read_u32(self.module.out_module)?;
        if module_ptr == 0 {
            return Ok(AppletResult::NoModule);
        }
        let vtable = self.cpu.read_u32(module_ptr)?;
        let create_instance = self.cpu.read_u32(vtable + 2 * 4)?;

        // O ponteiro de saída fica logo depois do `IModule*`, na área de objetos.
        let out_applet = self.module.out_module + 4;
        self.cpu.write_u32(out_applet, 0)?;

        let outcome = self.call_guest(
            create_instance,
            [module_ptr, self.module.shell, clsid, out_applet],
            budget,
        )?;
        match outcome {
            Outcome::Returned { code } => Ok(AppletResult::Called {
                code,
                applet: self.cpu.read_u32(out_applet)?,
            }),
            other => Ok(AppletResult::Stopped(other)),
        }
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
    pub fn gl_swaps(&self) -> u32 {
        self.egl_swaps
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

    /// A tela, como está agora.
    pub fn screen(&self) -> &Framebuffer {
        self.bitmaps
            .get(&self.device_bitmap)
            .unwrap_or(&self.screen)
    }

    /// Quantas superfícies de desenho existem.
    pub fn bitmap_count(&self) -> usize {
        self.bitmaps.len()
    }

    /// Textos que o jogo pediu para desenhar mas que ainda não sabemos rasterizar.
    pub fn pending_text(&self) -> &[String] {
        &self.pending_text
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

    pub fn heap_used(&self) -> u32 {
        self.heap.used()
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
        assert_eq!(
            clip_rect(
                None,
                Rect {
                    x: 1,
                    y: 2,
                    width: 3,
                    height: 4
                }
            ),
            None
        );
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
        machine
            .cpu
            .write_reg(Reg::Sp, loader::STACK_BASE + 0x1000);
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
        machine.cpu.write_u32(machine.cpu.read_reg(Reg::Sp), quantos).unwrap();

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
        machine.cpu.write_u32(machine.cpu.read_reg(Reg::Sp), quantos).unwrap();

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
