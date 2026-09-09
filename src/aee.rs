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
    /// `0x01001011` = **`AEECLSID_SOURCEUTIL`**, a fábrica de `ISource` do BREW.
    ///
    /// Ela se chamava `RootForm` aqui, e o nome estava errado: veio da mensagem
    /// `Could not create root form` da Z-Wheel, que na verdade é sobre a [`Interface::Widget`].
    /// A identificação certa saiu de dois jogos que a usam de maneiras que pareciam
    /// incompatíveis — e não são, quando os slots têm os nomes do `AEESource.h`:
    ///
    /// | slot | método | quem usa |
    /// |---|---|---|
    /// | 3 | `PeekSourceFromSource(po, ISource*, nMax, IPeek**)` | Z-Wheel, para ler o `tectoy.cfg` linha a linha |
    /// | 5 | `SourceFromMemory(po, pBuf, nSize, pfn, pUser, ISource**)` | Zeeboids, para embrulhar o corpo do POST |
    /// | 6 | `SourceFromFile(po, IFile*, ISource**)` | Z-Wheel, sobre o arquivo recém-aberto |
    ///
    /// O `SourceFromMemory` é o que fecha a conta: são **seis** parâmetros, e o ponteiro de
    /// saída é o segundo da pilha — exatamente onde o código do Zeeboids o lia, num trecho que
    /// tínhamos batizado de "envio". Ele não envia nada: embrulha o corpo para entregar à
    /// `IWeb`. É de lá que a ponte do Zeeboids pega o corpo, e é por isso que ela funciona.
    ///
    /// São sete métodos, e a vtable do firmware em `0x10a785e4` também tem sete.
    SourceUtil = 39,
    /// `0x01028e51`, o widget da interface da Z-Wheel — inclusive o formulário raiz.
    ///
    /// A classe não está na tabela do `1.1.2_APPS.bin`, então não há vtable de firmware para
    /// copiar. O que se sabe dela veio do código do jogo, e é pouco e claro: o único método
    /// usado é o **slot 3**, um acessador genérico `slot3(this, seletor, id, valor)`. O jogo o
    /// chama por dois invólucros, e os dois dizem qual é o seletor:
    ///
    /// - `0x3f72c(obj, id, saida)` chama `slot3(obj, 0x800, id, saida)` — **pega o filho** de
    ///   número `id` e escreve o ponteiro em `saida`.
    /// - `0x403c8(obj, valor)` chama `slot3(obj, 0x801, 0x130, valor)` — **grava** a
    ///   propriedade `0x130`.
    ///
    /// O retorno é ao contrário do BREW: **diferente de zero é sucesso**. Os dois invólucros
    /// fazem `cmp r0,#0; moveq r0,#3`, ou seja, transformam zero em `EBADCLASS`. Responder
    /// `SUCCESS` aqui — que vale zero — é dizer "falhou", e foi exatamente o que fez a
    /// `tectoymain.c:1001` imprimir `Could not create root form(20)` e depois morrer num nulo.
    Widget = 40,
    /// `0x01006c05`, o **ZEEBOMCP** — o objeto único que a Z-Wheel pede a cada partida.
    ///
    /// O nome sai do próprio jogo: a `Tectoy.c` imprime `Cannot create instance of ZEEBOMCP`
    /// quando a criação falha. No firmware ele é o singleton de `0x11085cb8`, que aloca oito
    /// bytes — vtable e contagem — e, se já existir, só incrementa a contagem.
    ///
    /// A tabela de classes do `1.1.2_APPS.bin` engana aqui: a entrada de `0x01006c05` aponta
    /// para `0x11267d04`, que é o `LCT_SIMCardCtl_New` e **só aceita `0x01006c01`**. A vtable
    /// de verdade é a `0x102d47a8`, oito métodos, achada ao lado do construtor — no mesmo
    /// trecho que carrega `fs:/card3` e `fs:/mcp/`. Copiar a vtable da entrada teria dado uma
    /// tabela de quatro métodos que não é desta classe.
    ///
    /// Os três primeiros slots foram lidos: contagem, contagem, e um `QueryInterface` que
    /// compara o IID com `0x01000001` e com `0x01006c05`. Os cinco restantes ficam sem nome de
    /// propósito — a Z-Wheel, até agora, só cria e solta o objeto, e uma chamada num deles é
    /// coisa para aparecer no relatório, não para ser atendida por adivinhação.
    ZeeboMcp = 41,
    /// `0x01001027`, a `IConfig` do BREW — os itens de configuração do aparelho.
    ///
    /// A `Tectoy_SetLanguagePref` da Z-Wheel chama o **slot 3** com `(0x3f, ponteiro, 4)`, que
    /// é a forma do `ICONFIG_SetItem(pMe, nItem, pBuff, nSize)` do SDK; o slot 2 é o
    /// `GetItem` correspondente. É isso que está implementado: os itens ficam guardados por
    /// número, e quem grava relê o que gravou.
    ///
    /// **A vtable do firmware não serviu, e é bom dizer por quê.** A entrada desta classe no
    /// `1.1.2_APPS.bin` existe (construtor `0x1125fbbc`, vtable `0x1086b554`, doze métodos),
    /// mas nove desses doze são literalmente `movs r0,#0x14; bx lr` — devolvem `EUNSUPPORTED`
    /// e nada mais, o `SetItem` inclusive. Copiar aquilo daria um `IConfig` que faz o console
    /// falhar: a Z-Wheel imprime `Unable to set language to config, error 20` e desiste. A
    /// leitura mais provável é que aquela entrada seja um registro de fachada da partição de
    /// aplicativos, e que a `IConfig` de verdade viva no lado do BREW, que não temos.
    ///
    /// Por isso só os quatro primeiros slots têm nome. Os oito de cima ficam de fora de
    /// propósito: sobre eles a única fonte seria a tabela que já se mostrou errada.
    Config = 42,
    /// Um `ISource` do BREW: bytes com um cursor. Criado pela [`Interface::SourceUtil`].
    Source = 43,
    /// Um `IPeek` do BREW: a leitura por linhas sobre um [`Interface::Source`].
    ///
    /// Do `IPeek` só conhecemos o **slot 8**, porque é o único que a Z-Wheel chama: ela passa o
    /// endereço de um par `{ponteiro, tamanho}` e o número 3, e espera receber a próxima linha.
    /// Os outros ficam sem nome — uma chamada neles precisa aparecer no relatório.
    Peek = 44,
    /// `0x01028e35`, a lista genérica da Z-Wheel — o que o jogo chama de "vector model".
    ///
    /// Não está na tabela de classes do firmware, então os slots saíram do código do jogo, e
    /// cada um tem duas leituras que concordam: o carregador do `tectoy.cfg` em `0x88338`
    /// enche a lista, e o laço em `0x7f164` a percorre.
    ///
    /// | slot | método | onde se lê |
    /// |---|---|---|
    /// | 5 | tamanho | `0x7f170`, e o resultado vira o teto do laço |
    /// | 6 | pegar em | `0x7f190`, com `(índice, &saída)`; o jogo testa se o texto começa com `#` |
    /// | 8 | inserir em | `0x884f0`, com índice `-1` — inserir no fim |
    /// | 9 | remover em | `0x7d788`, com índice `0`, no laço que esvazia a lista item a item |
    /// | 10 | esvaziar | `0x80010`, uma vez, logo antes do `Release` |
    /// | 12 | definir liberador | `0x88458`, recebendo **ponteiro de função do módulo** |
    ///
    /// Os slots sem nome nunca foram chamados. Deixá-los sem nome é o que faz uma chamada
    /// inesperada aparecer no relatório em vez de passar por implementada.
    Vetor = 45,
    /// `0x01028e3c`, da mesma família das outras duas extensões da Z-Wheel, e a mais modesta
    /// delas: a `tectoymain.c` cria **duas** logo no começo e guarda em `+0x354` e `+0x358`, e
    /// até agora não chama método nenhum em nenhuma das duas.
    ///
    /// Por isso só o `AddRef` e o `Release` existem aqui. Não é preguiça: é que qualquer outro
    /// nome seria invenção, e do jeito que está a primeira chamada de verdade vai aparecer no
    /// relatório em vez de ser atendida por acaso.
    Classe28e3c = 46,
    /// `0x01011810`, o que a Z-Wheel chama de **ICM** — o gerenciador de chamadas do BREW.
    ///
    /// A `tectoymain.c:1037` desiste da inicialização se não conseguir criá-lo. O que ela quer
    /// dele é uma coisa só, e o código diz qual: `0x87c40` zera um buffer de `0x340` bytes,
    /// chama o **slot 28** com `(buffer, 0x340)` e devolve a palavra em `+0xc`. Em `0x77564` o
    /// chamador compara essa palavra com **5**.
    ///
    /// Cinco é o `SYS_OPRT_MODE_ONLINE` do modo de operação do rádio da Qualcomm, e o campo
    /// bate com o `oprt_mode` do `AEECMPhInfo`. Ou seja: a pergunta é "o rádio está no ar?", e
    /// aqui a resposta é sim. É hipótese, e está registrada como tal no relatório — mas é
    /// hipótese com dois apoios independentes, o valor e a posição.
    Cm = 47,
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
    pub const ALL: [Interface; 48] = [
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
        Self::SourceUtil,
        Self::Widget,
        Self::ZeeboMcp,
        Self::Config,
        Self::Source,
        Self::Peek,
        Self::Vetor,
        Self::Classe28e3c,
        Self::Cm,
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
            Self::SourceUtil => "ISourceUtil",
            Self::Widget => "IWidget",
            Self::ZeeboMcp => "IZeeboMCP",
            Self::Config => "IConfig",
            Self::Source => "ISource",
            Self::Peek => "IPeek",
            Self::Vetor => "IVetor",
            Self::Classe28e3c => "I28e3c",
            Self::Cm => "ICM",
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
            Self::SourceUtil => aee_slots::SOURCE_UTIL,
            Self::Widget => aee_slots::WIDGET,
            Self::ZeeboMcp => aee_slots::ZEEBO_MCP,
            Self::Config => aee_slots::CONFIG,
            Self::Source => aee_slots::SOURCE,
            Self::Peek => aee_slots::PEEK,
            Self::Vetor => aee_slots::VETOR,
            Self::Classe28e3c => aee_slots::CLASSE_28E3C,
            Self::Cm => aee_slots::CM,
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
            39 => Self::SourceUtil,
            40 => Self::Widget,
            41 => Self::ZeeboMcp,
            42 => Self::Config,
            43 => Self::Source,
            44 => Self::Peek,
            45 => Self::Vetor,
            46 => Self::Classe28e3c,
            47 => Self::Cm,
            6 => Self::Helpers,
            _ => return None,
        })
    }
}

/// Se o nome de um slot é um marcador de posição — `slot7` e parecidos.
///
/// A tabela precisa desses marcadores quando um slot **de cima** é conhecido: sem eles, o slot
/// 28 do `ICM` não teria como ficar no índice 28. Mas marcador não é implementação, e atendê-lo
/// com sucesso seria justamente a mentira que estas tabelas existem para evitar. Quem despacha
/// usa isto para recusar, e aí a chamada aparece no relatório com o número do slot.
pub fn e_marcador(name: &str) -> bool {
    name.strip_prefix("slot")
        .is_some_and(|n| n.parse::<u32>().is_ok())
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
