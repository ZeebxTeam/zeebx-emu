//! Trampolim de chamadas para as APIs do BREW.
//!
//! Um objeto BREW é um ponteiro para uma struct cuja primeira palavra aponta para a vtable.
//! Chamar `ISHELL_CreateInstance(shell, ...)` no guest vira, em código ARM, um load do
//! ponteiro de vtable, um load do slot e um `blx` para o endereço lido.
//!
//! Aproveitamos isso: os endereços que colocamos nos slots ficam numa faixa **não mapeada**,
//! escolhida de forma que o próprio endereço codifique qual método foi chamado. O núcleo ARM
//! aborta o fetch, e [`decode`] devolve a interface e o slot.

use crate::brew::aee_helpers;
use crate::brew::aee_slots;
use crate::cpu::unicorn::API_BASE;

/// Quantos bits do endereço identificam a interface.
const IFACE_SHIFT: u32 = 12;
/// Espaço reservado a cada interface — 1024 slots, muito além do necessário.
const IFACE_STRIDE: u32 = 1 << IFACE_SHIFT;

/// Interfaces que o emulador conhece. O valor numérico entra no endereço do trampolim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// `AEECLSID_SQLMGR` do console: abre bancos SQLite. Ver [`crate::brew::sql`].
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
    ///
    /// **O retorno invertido vale só para o acessador.** O slot 2 é um `QueryInterface` comum,
    /// e ali zero é sucesso: em `0x11c58` o jogo faz `movs r5,r0; bne <erro>`. Misturar as duas
    /// convenções seria fácil, e por isso elas estão escritas lado a lado aqui. O slot 12 segue
    /// a mesma convenção do 2, e a mesma forma `(IID, &saída)`.
    Widget = 40,
    /// Controle básico usado pelo Zenonia (`AEECLSID 0x01003109`).
    Control = 51,
    /// `0x01006c05`, o **ZEEBOMCP** — o objeto único que a Z-Wheel pede a cada partida.
    ///
    /// O nome sai do próprio jogo: a `Tectoy.c` imprime `Cannot create instance of ZEEBOMCP`
    /// quando a criação falha. No firmware ele é o singleton de `0x11085cb8`, que aloca oito
    /// bytes — vtable e contagem — e, se já existir, só incrementa a contagem.
    ///
    /// O construtor é `0x11085cb8` e a vtable é `0x102d47a8`, com oito métodos — no mesmo
    /// trecho que carrega `fs:/card3` e `fs:/mcp/`, que é o que um MCP faria.
    ///
    /// Foi esta classe que revelou um erro na leitura da tabela de classes do firmware. O
    /// `firmware.py` lia as entradas deslocadas de uma palavra e devolvia, para cada CLSID, o
    /// construtor da entrada **anterior** — aqui, o `LCT_SIMCardCtl_New`, que começa comparando
    /// o CLSID recebido com `0x01006c01`. Foi essa comparação que denunciou o deslocamento. A
    /// ferramenta está corrigida, e a tabela agora aponta para o mesmo construtor que eu tinha
    /// achado a pé.
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
    /// **A vtable do firmware não serviu, e a razão mudou depois que eu a entendi.** Eu tinha
    /// copiado uma tabela de doze métodos em que nove eram `movs r0,#0x14; bx lr` — devolvem
    /// `EUNSUPPORTED` e nada mais, o `SetItem` inclusive —, e o resultado foi o jogo trocar
    /// `Unable to create instance of IConfig, error 20` por `Unable to set language to config,
    /// error 20`: o mesmo vinte, um passo adiante.
    ///
    /// Aquela tabela era de outra classe: o `firmware.py` lia as entradas deslocadas de uma
    /// palavra. Corrigida a leitura, o que sobra para a `0x01001027` é uma entrada com
    /// sinalizadores `0xffff0008` e um "construtor" cuja vtable tem um método só, que nem
    /// endereço é — ou seja, **falso positivo**: esta classe não está registrada neste
    /// firmware.
    ///
    /// Por isso os nomes vêm do SDK, e só os quatro slots que a Z-Wheel exercita. Os oito de
    /// cima ficam de fora: sobre eles não há fonte nenhuma.
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
    /// `0x01006c02`, o **controle de sistema** do console — o `OEM_LCTSystemCtl.c` do firmware.
    ///
    /// A `tectoymain.c:1759` imprime `ERROR: Failed to create system control` sem ele. O que
    /// ele controla está escrito nas strings ao lado da implementação: `Pipe OFF`,
    /// `Pipe Half Bright`, `Pipe Slow Pulsing`, `Button ON` — as luzes do aparelho — e um
    /// `LCT_SystemCtl_SystemControl nSystemMode=%d, nDownloadMode=%d`.
    ///
    /// A vtable é a `0x10691ea8`, com sete métodos, e o construtor `0x10e9f93e` confere o
    /// CLSID recebido contra `0x01006c02` — é dela mesma. Os três primeiros estão
    /// implementados; os que mexem em luz ficam como marcador até alguém chamá-los.
    ///
    /// O slot 6 é a exceção, porque a Z-Wheel o chama num laço. No firmware ele é uma casca de
    /// três instruções sobre uma chamada de hardware, sem argumento nenhum; do lado do jogo, em
    /// `0x81768`, zero é "siga" e diferente de zero desvia. Respondemos zero — o aparelho que
    /// não temos não tem o que reclamar.
    SystemCtl = 48,
    /// `0x01035156`, a fonte TrueType do console.
    ///
    /// O nome sai da mensagem que a `tectoymain.c:1269` imprime quando a criação falha:
    /// `Unable to create instance of TrueType TYPEFACE`. Ela não está na tabela de classes
    /// deste firmware.
    ///
    /// Como a [`Interface::Classe28e3c`], a Z-Wheel cria e guarda — em `+0x34b8` — e até agora
    /// não chama método nenhum. Aqui isso é menos surpreendente do que parece: o emulador já
    /// desenha texto com a `tectoy.ttf` que o próprio pacote traz, então o caminho de
    /// renderização não passa por este objeto.
    Typeface = 49,
    /// `0x01006c01`, o `LCT_SIMCardCtl` — o controle do cartão SIM do console.
    ///
    /// **Está implementada e não é oferecida, e o motivo é o jogo.** A `0x78544` cria esta
    /// classe para pedir a verificação do cartão; quando a criação **falha**, ela põe o estado
    /// em `0x27` — e `0x27` é justamente o que a `0x82464` encaminha para a transição que abre
    /// o menu principal. Recusar **é** o caminho, e por um motivo que levou três voltas para
    /// ficar claro.
    ///
    /// O `0x78544` distingue três desfechos, e o `0x82464` faz coisas diferentes com cada um:
    ///
    /// - `CreateInstance` falha → estado `0x27`. O `0x82464` chama a `0x1f7b4`, que é a rotina
    ///   que **avança a interface**: ela chega ao `0x7ed10`, o lançamento do formulário de
    ///   instruções do z-pad. É o único dos três que leva o jogo adiante.
    /// - Slot 3 devolve zero → estado `0x28`. O `0x82464` **não faz nada** com ele: compara,
    ///   desvia para o fim e retorna. A tela fica onde está, calada.
    /// - Slot 3 devolve não-zero → estado `1`, e o jogo mostra `Showing SIM Error dialog`.
    ///
    /// O `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` repetido é o jogo tomando o
    /// caminho certo muitas vezes, não um erro a calar. Oferecer a classe silencia a mensagem
    /// e, junto com ela, o avanço: foi assim que a tela de boas-vindas ficou parada.
    ///
    /// Há ainda o vazamento, se um dia isto for oferecido: o `0x785a8` volta sem `Release` e o
    /// ponteiro morre na pilha. No console quem solta o objeto é a resposta da verificação
    /// assíncrona. Sem soltar, o jogo criava um controle por volta do laço e esgotava o pote em
    /// 65 506 objetos, derrubando junto o formulário do z-pad.
    ///
    /// A vtable é a `0x113cf854`, com quatro métodos, e o construtor `0x11267d04` confere o
    /// CLSID recebido contra `0x01006c01` — é dela mesma. Foi este construtor, aliás, que
    /// denunciou a leitura deslocada da tabela de classes do firmware.
    ///
    /// O slot 3 recebe `(this, texto, applet)` e, no firmware, guarda um par em `+0xc4` e
    /// `+0xc8`: é registro de retorno de chamada, do tipo "verifique o cartão e me avise".
    SimCardCtl = 50,
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
    pub const ALL: [Interface; 52] = [
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
        Self::SystemCtl,
        Self::Typeface,
        Self::SimCardCtl,
        Self::Control,
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
            Self::Control => "IControl",
            Self::ZeeboMcp => "IZeeboMCP",
            Self::Config => "IConfig",
            Self::Source => "ISource",
            Self::Peek => "IPeek",
            Self::Vetor => "IVetor",
            Self::Classe28e3c => "I28e3c",
            Self::Cm => "ICM",
            Self::SystemCtl => "ILCTSystemCtl",
            Self::Typeface => "ITypeface",
            Self::SimCardCtl => "ILCTSimCardCtl",
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
            Self::Control => aee_slots::CONTROL,
            Self::ZeeboMcp => aee_slots::ZEEBO_MCP,
            Self::Config => aee_slots::CONFIG,
            Self::Source => aee_slots::SOURCE,
            Self::Peek => aee_slots::PEEK,
            Self::Vetor => aee_slots::VETOR,
            Self::Classe28e3c => aee_slots::CLASSE_28E3C,
            Self::Cm => aee_slots::CM,
            Self::SystemCtl => aee_slots::SYSTEM_CTL,
            Self::Typeface => aee_slots::TYPEFACE,
            Self::SimCardCtl => aee_slots::SIM_CARD_CTL,
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
            48 => Self::SystemCtl,
            49 => Self::Typeface,
            50 => Self::SimCardCtl,
            51 => Self::Control,
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
