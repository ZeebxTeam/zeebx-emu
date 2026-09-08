//! Trampolim de chamadas para as APIs do BREW.
//!
//! Um objeto BREW é um ponteiro para uma struct cuja primeira palavra aponta para a vtable.
//! Chamar `ISHELL_CreateInstance(shell, ...)` no guest vira, em código ARM, um load do
//! ponteiro de vtable, um load do slot e um `blx` para o endereço lido.
//!
//! Aproveitamos isso: os endereços que colocamos nos slots ficam numa faixa **não mapeada**,
//! escolhida de forma que o próprio endereço codifique qual método foi chamado. O núcleo ARM
//! aborta o fetch, e [`decode`] devolve a interface e o slot.

use crate::cpu::unicorn::API_BASE;
use crate::{aee_helpers, aee_slots};

/// Quantos bits do endereço identificam a interface.
const IFACE_SHIFT: u32 = 12;
/// Espaço reservado a cada interface — 1024 slots, muito além do necessário.
const IFACE_STRIDE: u32 = 1 << IFACE_SHIFT;

/// Interfaces que o emulador conhece. O valor numérico entra no endereço do trampolim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Interface {
    Shell = 0,
    Module = 1,
    Applet = 2,
    FileMgr = 3,
    File = 4,
    Display = 5,
    Bitmap = 7,
    /// Gamepad do Zeebo — extensão do console, declarada no SDK dele.
    Hid = 8,
    HidDevice = 9,
    /// Sinais: o mecanismo pelo qual o BREW avisa o app de que algo aconteceu.
    Signal = 10,
    SignalCtl = 11,
    SignalCbFactory = 12,
    /// API 2D do BREW: linhas, círculos, polígonos, viewport.
    Graphics = 13,
    /// Som básico do BREW: tons, vibração e volume.
    Sound = 14,
    /// Licença do módulo em execução: tipo, expiração e forma de compra.
    License = 15,
    /// Stream assíncrono sobre um bloco de memória — é como o BREW alimenta os decodificadores
    /// de imagem.
    MemAStream = 16,
    /// Imagem decodificada de um stream (PNG, no caso dos jogos do console).
    Image = 17,
    /// Thread cooperativa do BREW: não há preempção, o guest cede o controle sozinho.
    Thread = 18,
    /// EGL — a ponte entre o OpenGL ES e a tela do console.
    Egl = 19,
    /// OpenGL ES 1.1, na forma que o BREW expõe.
    Gles = 20,
    /// Fábrica de objetos de mídia.
    MediaUtil = 21,
    /// Reprodução de áudio e vídeo — no console, os `.mp3` da trilha sonora.
    Media = 22,
    /// EGL na forma antiga de `AEEGL.h`, sem `this` e com retorno direto.
    EglLegacy = 23,
    /// OpenGL ES 1.0 na forma antiga de `AEEGL.h`.
    GlLegacy = 24,
    /// HTTP do BREW. O console está sempre offline aqui, mas o objeto precisa existir: os
    /// jogos criam os três de rede em sequência e não conferem o retorno.
    Web = 25,
    /// Resumo criptográfico — `AEECLSID_MD5`.
    Hash = 26,
    /// Fábrica de cifradores, de `inc/AEEICipherFactory.h`.
    CipherFactory = 27,
    /// Cifrador de bloco, de `inc/AEEICipher1.h`.
    Cipher = 28,
    /// Memória do sistema, de `sdk/inc/AEEHeap.h`.
    Heap = 29,
    /// Stream que descomprime deflate, de `sdk/inc/AEEUnzipStream.h`.
    UnzipStream = 30,
    /// Decodificador de imagem, de `inc/AEEIImageDecoder.h`. É a interface padrão das classes
    /// de decodificação — `AEECLSID_PNGDecoderBREW` entre elas.
    ImageDecoder = 31,
    /// Entrada de dados de um decodificador, de `inc/AEEIForceFeed.h`. O jogo pede esta
    /// interface ao decodificador e escreve o arquivo nela, pedaço a pedaço.
    ForceFeed = 32,
    /// Escala, rotação e transparência da superfície EGL, de `sdk/inc/AEEEGLSurfaceManip.h`.
    /// É o `EGL_QUALCOMM_surface_scale` do console.
    EglSurfaceManip = 33,
    /// Os extras do ATI Imageon sobre o OpenGL ES, de `sdk/inc/AEEGLESImageonEXT.h`.
    GlesImageonExt = 34,
    /// `AEECLSID_SQLMGR` do console: abre bancos SQLite. Ver [`crate::sql`].
    SqlMgr = 36,
    /// Um banco aberto pelo [`Interface::SqlMgr`].
    SqlDatabase = 37,
    /// A coleção genérica da Z-Wheel (`0x0100104f`): guarda itens e é percorrida.
    Collection = 38,
    /// `0x01001011`, o formulário raiz — a classe que a Z-Wheel e o Zeeboids pedem para abrir.
    ///
    /// Os sete métodos vêm da vtable do firmware, em `0x10a785e4`, achada pela tabela de
    /// registro do `1.1.2_APPS.bin`: entradas de dezesseis bytes `{função, CLSID, sinalizadores,
    /// 0}`, e a da `0x01001011` aponta para o construtor em `0x112e399c`. Ele aloca vinte e
    /// quatro bytes, grava a vtable em `+0`, o `IShell` em `+8` e cria em `+0xc` um objeto da
    /// classe `0x0103475a`, que é para quem os métodos delegam.
    ///
    /// São sete e não mais: o slot 7 não é endereço Thumb e o 8 é zero. Isso casa com o que a
    /// sonda viu os aplicativos chamarem — os slots 3 e 6, os dois últimos.
    RootForm = 39,
    /// Objeto de uma classe que ainda não conhecemos, criado a pedido do `--sonda`.
    ///
    /// Não implementa interface nenhuma: existe para **descobrir qual é**. Toda chamada é
    /// registrada com o slot e os argumentos, e responde `SUCCESS`, de modo que o jogo siga o
    /// máximo que conseguir e mostre o que espera do objeto. Foi assim que o `IHID` do console
    /// foi identificado, na mão; isto é a mesma ideia com ferramenta.
    Probe = 35,
    /// Tabela de funções da stdlib do BREW (`MALLOC`, `STRLEN`, …), que os módulos dinâmicos
    /// acessam por um ponteiro entregue pelo carregador — não por vtable de objeto.
    Helpers = 6,
}

impl Interface {
    /// Todas as interfaces, na ordem exata dos valores do enum.
    ///
    /// A tabela de vtables é montada a partir desta lista e endereçada pelo valor do enum, então
    /// as duas coisas precisam concordar — daí a lista existir num lugar só, com teste que
    /// confere a correspondência. Quando elas divergiram, um objeto recebeu a vtable de outra
    /// interface e a chamada foi parar no método errado, com sintoma a quilômetros da causa.
    pub const ALL: [Interface; 40] = [
        Self::Shell,
        Self::Module,
        Self::Applet,
        Self::FileMgr,
        Self::File,
        Self::Display,
        Self::Helpers,
        Self::Bitmap,
        Self::Hid,
        Self::HidDevice,
        Self::Signal,
        Self::SignalCtl,
        Self::SignalCbFactory,
        Self::Graphics,
        Self::Sound,
        Self::License,
        Self::MemAStream,
        Self::Image,
        Self::Thread,
        Self::Egl,
        Self::Gles,
        Self::MediaUtil,
        Self::Media,
        Self::EglLegacy,
        Self::GlLegacy,
        Self::Web,
        Self::Hash,
        Self::CipherFactory,
        Self::Cipher,
        Self::Heap,
        Self::UnzipStream,
        Self::ImageDecoder,
        Self::ForceFeed,
        Self::EglSurfaceManip,
        Self::GlesImageonExt,
        Self::Probe,
        Self::SqlMgr,
        Self::SqlDatabase,
        Self::Collection,
        Self::RootForm,
    ];

    /// Nome usado nos logs — casa com a nomenclatura do SDK.
    pub fn name(self) -> &'static str {
        match self {
            Self::Shell => "IShell",
            Self::Module => "IModule",
            Self::Applet => "IApplet",
            Self::FileMgr => "IFileMgr",
            Self::File => "IFile",
            Self::Display => "IDisplay",
            Self::Bitmap => "IBitmap",
            Self::Hid => "IHID",
            Self::HidDevice => "IHIDDevice",
            Self::Signal => "ISignal",
            Self::SignalCtl => "ISignalCtl",
            Self::SignalCbFactory => "ISignalCBFactory",
            Self::Graphics => "IGraphics",
            Self::Sound => "ISound",
            Self::License => "ILicense",
            Self::MemAStream => "IMemAStream",
            Self::Image => "IImage",
            Self::Thread => "IThread",
            Self::Egl => "IEGL11",
            Self::Gles => "IGLES11",
            Self::MediaUtil => "IMediaUtil",
            Self::Media => "IMedia",
            Self::EglLegacy => "IEGL",
            Self::GlLegacy => "IGL",
            Self::Web => "IWeb",
            Self::Hash => "IHash",
            Self::CipherFactory => "ICipherFactory",
            Self::Cipher => "ICipher1",
            Self::Heap => "IHeap",
            Self::UnzipStream => "IUnzipAStream",
            Self::ImageDecoder => "IImageDecoder",
            Self::ForceFeed => "IForceFeed",
            Self::EglSurfaceManip => "IEGLSurfaceManip",
            Self::GlesImageonExt => "IGLESImageonExt",
            Self::SqlMgr => "ISQLMgr",
            Self::SqlDatabase => "ISQLDatabase",
            Self::Collection => "IColecao",
            Self::RootForm => "IFormRaiz",
            Self::Probe => "ClasseDesconhecida",
            Self::Helpers => "AEEHelpers",
        }
    }

    /// Nomes dos métodos desta interface, na ordem da vtable.
    fn slot_names(self) -> &'static [&'static str] {
        match self {
            Self::Shell => aee_slots::SHELL,
            Self::Module => aee_slots::MODULE,
            Self::Applet => aee_slots::APPLET,
            Self::FileMgr => aee_slots::FILEMGR,
            Self::File => aee_slots::FILE,
            Self::Display => aee_slots::DISPLAY,
            Self::Bitmap => aee_slots::BITMAP,
            Self::Hid => aee_slots::HID,
            Self::HidDevice => aee_slots::HIDDEVICE,
            Self::Signal => aee_slots::SIGNAL,
            Self::SignalCtl => aee_slots::SIGNALCTL,
            Self::SignalCbFactory => aee_slots::SIGNALCBFACTORY,
            Self::Graphics => aee_slots::GRAPHICS,
            Self::Sound => aee_slots::SOUND,
            Self::License => aee_slots::LICENSE,
            Self::MemAStream => aee_slots::MEMASTREAM,
            Self::Image => aee_slots::IMAGE,
            Self::Thread => aee_slots::THREAD,
            Self::Egl => aee_slots::EGL,
            Self::Gles => aee_slots::GLES,
            Self::MediaUtil => aee_slots::MEDIAUTIL,
            Self::Media => aee_slots::MEDIA,
            Self::EglLegacy => aee_slots::EGL_LEGACY,
            Self::GlLegacy => aee_slots::GL_LEGACY,
            Self::Web => aee_slots::WEB,
            Self::Hash => aee_slots::HASH,
            Self::CipherFactory => aee_slots::CIPHER_FACTORY,
            Self::Cipher => aee_slots::CIPHER,
            Self::Heap => aee_slots::HEAP,
            Self::UnzipStream => aee_slots::UNZIP_STREAM,
            Self::ImageDecoder => aee_slots::IMAGE_DECODER,
            Self::ForceFeed => aee_slots::FORCE_FEED,
            Self::EglSurfaceManip => aee_slots::EGL_SURFACE_MANIP,
            Self::GlesImageonExt => aee_slots::GLES_IMAGEON_EXT,
            Self::SqlMgr => aee_slots::SQL_MGR,
            Self::SqlDatabase => aee_slots::SQL_DATABASE,
            Self::Collection => aee_slots::COLLECTION,
            Self::RootForm => aee_slots::ROOT_FORM,
            // A sonda não tem tabela: `method` responde por ela antes de chegar aqui.
            Self::Probe => &[],
            Self::Helpers => aee_helpers::HELPERS,
        }
    }

    /// Nome do método num slot, quando conhecido.
    pub fn method(self, slot: u32) -> Option<&'static str> {
        // A sonda aceita qualquer slot: o que interessa dela é o número, não o nome, e recusar
        // faria o jogo parar justamente no que queremos observar.
        if matches!(self, Self::Probe) {
            return Some("sonda");
        }
        self.slot_names().get(slot as usize).copied()
    }

    /// O mesmo que `from_index`, para quem só tem o número guardado — o perfil de API.
    pub fn from_index_public(index: u32) -> Option<Self> {
        Self::from_index(index)
    }

    fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::Shell,
            1 => Self::Module,
            2 => Self::Applet,
            3 => Self::FileMgr,
            4 => Self::File,
            5 => Self::Display,
            7 => Self::Bitmap,
            8 => Self::Hid,
            9 => Self::HidDevice,
            10 => Self::Signal,
            11 => Self::SignalCtl,
            12 => Self::SignalCbFactory,
            13 => Self::Graphics,
            14 => Self::Sound,
            15 => Self::License,
            16 => Self::MemAStream,
            17 => Self::Image,
            18 => Self::Thread,
            19 => Self::Egl,
            20 => Self::Gles,
            21 => Self::MediaUtil,
            22 => Self::Media,
            23 => Self::EglLegacy,
            24 => Self::GlLegacy,
            25 => Self::Web,
            26 => Self::Hash,
            27 => Self::CipherFactory,
            28 => Self::Cipher,
            29 => Self::Heap,
            30 => Self::UnzipStream,
            31 => Self::ImageDecoder,
            32 => Self::ForceFeed,
            33 => Self::EglSurfaceManip,
            34 => Self::GlesImageonExt,
            35 => Self::Probe,
            36 => Self::SqlMgr,
            37 => Self::SqlDatabase,
            38 => Self::Collection,
            39 => Self::RootForm,
            6 => Self::Helpers,
            _ => return None,
        })
    }
}

/// Endereço-trampolim de um método.
pub fn encode(iface: Interface, slot: u32) -> u32 {
    API_BASE + (iface as u32) * IFACE_STRIDE + slot * 4
}

/// Versão de [`encode`] que aceita o índice numérico da interface, para quem guardou só o
/// número (o log de chamadas, por exemplo).
pub fn encode_raw(iface_index: u32, slot: u32) -> u32 {
    API_BASE + iface_index * IFACE_STRIDE + slot * 4
}

/// Inverte [`encode`]. Devolve `None` se o endereço não corresponder a uma interface conhecida.
pub fn decode(addr: u32) -> Option<(Interface, u32)> {
    let offset = addr.checked_sub(API_BASE)?;
    let iface = Interface::from_index(offset / IFACE_STRIDE)?;
    Some((iface, (offset % IFACE_STRIDE) / 4))
}

/// Descrição legível de uma chamada ainda não implementada, para o log que vira nosso backlog.
pub fn describe(addr: u32) -> String {
    match decode(addr) {
        Some((iface, slot)) => match iface.method(slot) {
            Some(name) => format!("{}::{name}", iface.name()),
            None => format!("{}::slot[{slot}]", iface.name()),
        },
        None => format!("endereço de API desconhecido {addr:#010x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codifica_e_decodifica_ida_e_volta() {
        for (iface, slot) in [
            (Interface::Shell, 0),
            (Interface::FileMgr, 2),
            (Interface::File, 31),
        ] {
            assert_eq!(decode(encode(iface, slot)), Some((iface, slot)));
        }
    }

    #[test]
    fn interfaces_diferentes_nao_colidem() {
        assert_ne!(encode(Interface::Shell, 1023), encode(Interface::Module, 0));
    }

    #[test]
    fn endereco_fora_das_interfaces_conhecidas_nao_decodifica() {
        assert_eq!(decode(API_BASE + 99 * IFACE_STRIDE), None);
        assert_eq!(decode(0x1000), None);
    }

    #[test]
    fn descreve_chamada_com_nome_do_sdk() {
        assert_eq!(
            describe(encode(Interface::FileMgr, 2)),
            "IFileMgr::OpenFile"
        );
        assert_eq!(
            describe(encode(Interface::Shell, 2)),
            "IShell::CreateInstance"
        );
    }

    #[test]
    fn slot_sem_nome_conhecido_aparece_pelo_numero() {
        assert_eq!(describe(encode(Interface::File, 99)), "IFile::slot[99]");
    }

    #[test]
    fn a_ordem_dos_slots_bate_com_o_sdk() {
        // Conferido contra as macros INHERIT_* do BREW SDK 4.0.2.
        assert_eq!(Interface::Module.method(2), Some("CreateInstance"));
        assert_eq!(Interface::Applet.method(2), Some("HandleEvent"));
        assert_eq!(Interface::Display.method(7), Some("Update"));
    }
}
