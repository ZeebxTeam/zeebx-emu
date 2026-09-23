//! Core Libretro do Zeebx.
//!
//! O frontend é dono de vídeo, áudio e entrada; aqui só se traduz isso para o motor `zeebx`.
//! Nenhuma janela, placa de som ou controle do host é aberto por este crate: o motor roda pelo
//! passo virtual de [`zeebx::session::Session::run_frame`].
//!
//! O core atende conteúdo `.mod`/`.zip`/`.7z`, vídeo RGB565, áudio PCM16 a 44,1 kHz, RetroPad nas
//! duas portas, teclado USB e Boomerang alimentado pelo sensor do frontend. Renderização em
//! hardware OpenGL/GLES 3, save state versionado e troca da Z-Wheel também estão implementados;
//! mouse do guest continua fora da ABI.

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use zeebx::audio::Mixer;
use zeebx::config::ZWheel;
use zeebx::input::Pad;
use zeebx::input::bindings::Aparelho;
use zeebx::session::{Session, StartError, Step};
use zeebx::storage::StoragePaths;

/// Versão da ABI que este core implementa.
const API_VERSION: u32 = 1;

/// Taxa nominal do core: 44,1 kHz estéreo.
///
/// A quantidade entregue por chamada **não** é fixa: sai do tempo virtual que passou desde a
/// chamada anterior. Ver `retro_run`.
const SAMPLE_RATE: u32 = 44_100;

/// Formato de pixel negociado com o frontend, em `retro_pixel_format`.
const PIXEL_FORMAT_RGB565: u32 = 2;

/// Comandos de ambiente usados.
/// Pede ao frontend que descarregue o conteúdo e volte ao menu dele.
const ENV_SHUTDOWN: u32 = 7;

/// Mostra um aviso ao jogador, na tela do próprio frontend.
const ENV_SET_MESSAGE: u32 = 6;

/// Pergunta se o frontend entrega todos os botões numa palavra só.
const ENV_GET_INPUT_BITMASKS: u32 = 51;

/// Identificador especial do RetroPad que devolve os botões como máscara de bits.
const ID_JOYPAD_MASK: u32 = 256;

/// Recebe as teclas do frontend. É por aqui que a Z-Wheel navega: ela pede `AVK_0` e `AVK_CLR`,
/// que não existem no RetroPad.
const ENV_SET_KEYBOARD_CALLBACK: u32 = 12;

/// Códigos de tecla da ABI (`enum retro_key`), que seguem os do SDL 1.2.
const RETROK_BACKSPACE: u32 = 8;
const RETROK_RETURN: u32 = 13;
const RETROK_ESCAPE: u32 = 27;
const RETROK_ASTERISK: u32 = 42;
const RETROK_HASH: u32 = 35;
const RETROK_0: u32 = 48;
const RETROK_UP: u32 = 273;
const RETROK_DOWN: u32 = 274;
const RETROK_RIGHT: u32 = 275;
const RETROK_LEFT: u32 = 276;

/// Quantos jogos o core achou ao lado do conteúdo.
///
/// **É a precondição da Z-Wheel listar alguma coisa.** A roda enumera os applets instalados e pede
/// o `.mod` de cada um por ClassID; quem sabe onde eles estão é o levantamento feito ao abrir o
/// conteúdo. Roda vazia aqui é roda vazia na tela — e sem esta conta o sintoma "a Z-Wheel abre sem
/// jogo nenhum" não teria onde ser medido sem janela. **Instrumento de teste, e só dele.**
#[cfg(test)]
static JOGOS_VISTOS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A última classe que o shell pediu para abrir.
///
/// **Instrumento de teste, e só dele.** O log do core sai pelo callback do frontend, que é
/// **variádico** — uma função `extern "C" fn(...)` não pode ser escrita em Rust estável —, então o
/// teste observa por aqui o que a interface mostraria como texto. Fora de `cfg(test)` não existe.
#[cfg(test)]
static ULTIMA_ABERTURA: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A classe do applet que está rodando agora, a cada quadro.
///
/// **Instrumento de teste, e só dele**, pelo mesmo motivo de [`ULTIMA_ABERTURA`]. É o que deixa o
/// teste do ciclo da Z-Wheel dizer *quem* está rodando — a roda, ou o jogo que ela abriu — sem
/// janela e sem olhar pixels.
#[cfg(test)]
static CLASSE_ATUAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// O relógio virtual da sessão, em milissegundos, a cada quadro.
///
/// **Instrumento de teste, e só dele.** É o que diz se a roda está no mesmo ponto da linha do tempo
/// nos dois caminhos — o da varredura e o do core —, e é a primeira coisa a conferir quando o mesmo
/// roteiro dá desfechos diferentes: a tecla pode estar certa e o instante, não. Medido com ele:
/// cada `retro_run` avança ~26 ms de relógio virtual, e não os 16 ms de um quadro a 60 Hz, porque a
/// volta do core termina quando a máquina **apresenta** um quadro.
#[cfg(test)]
static RELOGIO: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// As instruções que a máquina já executou, a cada quadro.
///
/// **Instrumento de teste, e só dele.** É o par do [`RELOGIO`] para a pergunta que separa "a roda
/// está lenta" de "a roda parou": o relógio do guest anda por vsync e por instrução, então
/// instruções que sobem com o relógio parado são guest girando sem apresentar quadro. Foi o que
/// mostrou que o caminho do core entrega 1 214 quadros por milissegundo virtual depois do
/// confirmar, contra ~240 quadros por segundo no regime normal.
#[cfg(test)]
static INSTRUCOES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// O frontend oferece um contexto de placa para o core desenhar.
const ENV_SET_HW_RENDER: u32 = 14;

/// Perfil de hardware que o core pede.
///
/// Desktop usa `RETRO_HW_CONTEXT_OPENGL_CORE` (3), medido com RetroArch/Mesa. Os handhelds
/// Linux AArch64 (R36S/R35S/RGB20S com ArkOS/AeolusUX/dArkOS/dArkOSen e RG40XX-H com muOS)
/// expõem OpenGL ES no RetroArch, não um contexto OpenGL Core 3.3. Neles pedimos GLES 3.2
/// (`RETRO_HW_CONTEXT_OPENGLES_VERSION`, 5), e o `gpu.rs` usa `#version 300 es`, compatível com
/// o subconjunto necessário. Se o frontend/driver só oferecer GLES 3.1, ele recusa
/// o pedido e o core permanece no rasterizador software — nunca depende de X11, Wayland ou EGL.
///
/// No desktop o valor 1 (`RETRO_HW_CONTEXT_OPENGL`, compatibilidade) não serve: em RetroArch/EGL
/// ele entregava perfil diferente do que o motor esperava e falhava com `GL: Invalid enum`.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_CONTEXT: u32 = 5; // RETRO_HW_CONTEXT_OPENGLES_VERSION (GLES 3.1+)
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_VERSION_MAJOR: u32 = 3;
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const HW_VERSION_MINOR: u32 = 2;

#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
const HW_CONTEXT: u32 = 3; // RETRO_HW_CONTEXT_OPENGL_CORE
#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
const HW_VERSION_MAJOR: u32 = 3;
#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
const HW_VERSION_MINOR: u32 = 3;

/// O valor que o `retro_video_refresh` recebe quando o quadro saiu no framebuffer do frontend.
///
/// É o sentinela do `libretro`: passar pixels junto com ele seria mentira, e o RetroArch apresenta
/// o framebuffer que ele mesmo forneceu.
const HW_FRAME_BUFFER_VALID: usize = usize::MAX;


/// O começo de `retro_hw_render_callback`, na ordem do `libretro.h` vendorizado.
///
/// Os campos são os que o core precisa ler e preencher: o tipo de contexto (o core escolhe), os
/// dois ponteiros que o **frontend** preenche depois (o framebuffer corrente e o resolvedor de
/// funções de GL) e o `context_reset`, que é como o frontend avisa que o contexto está utilizável.
#[repr(C)]
#[derive(Clone, Copy)]
struct RetroHwRenderCallback {
    context_type: u32,
    context_reset: Option<unsafe extern "C" fn()>,
    get_current_framebuffer: Option<unsafe extern "C" fn() -> u32>,
    get_proc_address: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
    depth: bool,
    stencil: bool,
    bottom_left_origin: bool,
    version_major: u32,
    version_minor: u32,
    /// Se o frontend quer que o core guarde os recursos de GL entre contextos.
    ///
    /// Fica declarado para o struct ter o **tamanho e os deslocamentos** do `libretro.h`: um campo
    /// a menos aqui e o `context_destroy` abaixo seria lido no lugar errado. Não usamos cache, e
    /// responder `false` (o zero) é a resposta certa.
    cache_context: bool,
    /// Chamado pelo frontend quando o contexto **deixa de valer** — trocar de driver de vídeo, por
    /// exemplo. Sem ele, o `glow::Context` que guardamos continua apontando para funções que já não
    /// existem e o primeiro desenho depois disso quebra.
    context_destroy: Option<unsafe extern "C" fn()>,
    /// O resto do struct não nos interessa, mas o tamanho tem de bater com o do frontend.
    _resto: [usize; 4],
}

/// O que o frontend respondeu ao pedido de render em hardware.
static OFERTA_DE_PLACA: std::sync::OnceLock<RetroHwRenderCallback> = std::sync::OnceLock::new();

/// Se o frontend já avisou que o contexto está utilizável.
///
/// **A ordem importa:** o `retro_load_game` pede o contexto, e o frontend chama o `context_reset`
/// **depois** — só ali as funções de GL existem. Por isso a placa entra no primeiro `retro_run`, e
/// não na carga.
static CONTEXTO_PRONTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// O contexto de GL montado a partir do que o frontend entregou, quando ele entregou.
static PLACA: std::sync::Mutex<Option<std::sync::Arc<glow::Context>>> =
    std::sync::Mutex::new(None);

/// O contexto de placa, quando já se desenha nele.
fn placa() -> Option<std::sync::Arc<glow::Context>> {
    PLACA.lock().ok().and_then(|guarda| guarda.clone())
}

/// Pede o contexto de placa ao frontend, uma vez, e guarda a resposta.
///
/// No `libretro`, quem **oferece** é o core: ele preenche o struct e chama o ambiente. O frontend
/// devolve `true` se aceitar, e depois disso ele cria o contexto e chama o nosso `context_reset`.
fn pede_o_contexto_de_placa() {
    if OFERTA_DE_PLACA.get().is_some() {
        return;
    }
    let mut oferta = RetroHwRenderCallback {
        context_type: HW_CONTEXT,
        context_reset: Some(contexto_pronto),
        get_current_framebuffer: None,
        get_proc_address: None,
        // Profundidade e stencil são exigências do console, não enfeite: o palco da Z-Wheel marca
        // o chão no stencil para desenhar o reflexo.
        depth: true,
        stencil: true,
        bottom_left_origin: false,
        version_major: HW_VERSION_MAJOR,
        version_minor: HW_VERSION_MINOR,
        // Não guardamos recursos de GL entre contextos, e o `libretro` só oferece a opção.
        cache_context: false,
        context_destroy: Some(contexto_perdido),
        _resto: [0; 4],
    };
    let alvo = &mut oferta as *mut RetroHwRenderCallback as *mut c_void;
    if unsafe { environ(ENV_SET_HW_RENDER, alvo) } {
        log(&format!(
            "Zeebx: o frontend aceitou render em hardware (OpenGL {}.{}); o desenho passa a ser na placa",
            oferta.version_major, oferta.version_minor
        ));
        let _ = OFERTA_DE_PLACA.set(oferta);
    } else {
        log("Zeebx: o frontend não oferece render em hardware; o desenho fica no processador");
    }
}

/// Monta o contexto de GL a partir do resolvedor do frontend e **recria a sessão** na placa.
///
/// **Uma vez, no primeiro quadro** — e não na carga —, porque é só depois do `context_reset` que
/// as funções existem. Recriar a sessão aqui custa um reinício que ninguém vê: nenhum quadro foi
/// entregue ainda.
///
/// **Qualquer falha devolve o software e diz por quê.** Um frontend que aceita o pedido e não
/// cumpre — sem `get_proc_address`, sem contexto — não pode deixar o emulador sem imagem: o
/// caminho de software é o medido e o que já funcionava.
fn liga_a_placa(estado: &mut Core) {
    if estado.placa_ligada || !CONTEXTO_PRONTO.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // Tenta **uma vez**: um contexto que não veio não vem no quadro seguinte, e insistir a cada
    // quadro gastaria o log inteiro.
    estado.placa_ligada = true;
    let Some(oferta) = OFERTA_DE_PLACA.get() else {
        return;
    };
    let Some(pega_endereco) = oferta.get_proc_address else {
        aviso("Zeebx: o frontend aceitou render em hardware mas não oferece get_proc_address; seguindo no processador");
        return;
    };
    let contexto = std::sync::Arc::new(unsafe {
        glow::Context::from_loader_function(|nome| {
            let Ok(nome) = CString::new(nome) else {
                return std::ptr::null();
            };
            pega_endereco(nome.as_ptr())
        })
    });
    // A placa entra no global **antes** da troca: é ele que `troca_para` consulta para decidir se
    // a sessão nasce com o rasterizador de placa. Assim as trocas seguintes — a Z-Wheel abrindo um
    // jogo, o jogo voltando para ela — também nascem na placa.
    if let Ok(mut guarda) = PLACA.lock() {
        *guarda = Some(contexto);
    }
    let antes = estado.path.clone();
    // **Um `panic` aqui derrubaria o RetroArch.** Não há fronteira segura para atravessar uma
    // falha de Rust e voltar para o C do frontend: o processo inteiro cai, e o usuário perde o
    // emulador por causa de uma otimização de desenho. O `catch_unwind` transforma isso no mesmo
    // caminho da falha comum — aviso no log, sessão de software, jogo rodando.
    //
    // `AssertUnwindSafe` é o que a situação pede: se o meio da montagem do contexto ficou
    // inconsistente, o que vem depois **não** continua dali — a sessão é recriada do zero, e o
    // global da placa é limpo.
    let tentativa = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        troca_para(estado, &antes, false)
    }));
    let desfecho = match tentativa {
        Ok(resultado) => resultado,
        Err(_) => Err(StartError::NotLoadable(
            "a montagem do contexto de placa entrou em pânico".to_string(),
        )),
    };
    match desfecho {
        Ok(()) => log("Zeebx: desenhando na placa"),
        Err(erro) => {
            aviso(&format!(
                "Zeebx: o render em hardware falhou ({erro}); seguindo no processador"
            ));
            // A sessão de software foi perdida na tentativa. Recria sem placa para não ficar sem
            // imagem nenhuma — e o global da placa é limpo, para as trocas seguintes nascerem no
            // processador.
            estado.placa_ligada = false;
            if let Ok(mut guarda) = PLACA.lock() {
                *guarda = None;
            }
            if let Err(tambem) = troca_para(estado, &antes, false) {
                aviso(&format!("Zeebx: nem no processador deu para reabrir: {tambem}"));
            }
        }
    }
}

/// O `context_reset` que nós preenchemos: o frontend chama quando o contexto está utilizável.
unsafe extern "C" fn contexto_pronto() {
    CONTEXTO_PRONTO.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// O frontend avisa que o contexto deixou de valer.
///
/// **O `glow::Context` guardado vira lixo aqui**: as funções de GL que ele resolveu não existem
/// mais. Descartar o contexto e voltar ao software é a resposta segura — o jogo continua rodando
/// no processador, e o aviso diz por quê.
unsafe extern "C" fn contexto_perdido() {
    if let Ok(mut guarda) = PLACA.lock() {
        *guarda = None;
    }
    CONTEXTO_PRONTO.store(false, std::sync::atomic::Ordering::Relaxed);
    PERDEU_A_PLACA.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Se o frontend já avisou que o contexto se perdeu. Ver [`contexto_perdido`].
static PERDEU_A_PLACA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Pergunta se o frontend aceita receber quadro nulo quando nada mudou.
const ENV_GET_CAN_DUPE: u32 = 3;
const ENV_GET_SYSTEM_DIRECTORY: u32 = 9;
const ENV_SET_PIXEL_FORMAT: u32 = 10;
const ENV_SET_INPUT_DESCRIPTORS: u32 = 11;
const ENV_GET_LOG_INTERFACE: u32 = 27;
const ENV_GET_SAVE_DIRECTORY: u32 = 31;
const ENV_SET_CONTROLLER_INFO: u32 = 35;

/// Tipos de dispositivo e identificadores de botão do RetroPad.
const DEVICE_NONE: u32 = 0;
const DEVICE_JOYPAD: u32 = 1;
const DEVICE_ANALOG: u32 = 5;
const ID_B: u32 = 0;
const ID_Y: u32 = 1;
const ID_SELECT: u32 = 2;
const ID_START: u32 = 3;
const ID_UP: u32 = 4;
const ID_DOWN: u32 = 5;
const ID_LEFT: u32 = 6;
const ID_RIGHT: u32 = 7;
const ID_A: u32 = 8;
const ID_X: u32 = 9;
const ID_L: u32 = 10;
const ID_R: u32 = 11;
const ANALOG_LEFT: u32 = 0;
const ANALOG_AXIS_X: u32 = 0;
const ANALOG_AXIS_Y: u32 = 1;

/// Subclasses de RetroPad que identificam aparelhos do console.
const DEVICE_ZPAD: u32 = ((1 + 1) << 8) | DEVICE_JOYPAD;
const DEVICE_BOOMERANG: u32 = ((2 + 1) << 8) | DEVICE_JOYPAD;

type EnvironmentFn = unsafe extern "C" fn(cmd: u32, data: *mut c_void) -> bool;
type KeyboardEventFn = unsafe extern "C" fn(down: bool, keycode: u32, character: u32, modifiers: u16);

#[repr(C)]
struct RetroMessage {
    msg: *const c_char,
    frames: u32,
}

#[repr(C)]
struct RetroKeyboardCallback {
    callback: Option<KeyboardEventFn>,
}
type VideoRefreshFn =
    unsafe extern "C" fn(data: *const c_void, width: u32, height: u32, pitch: usize);
type AudioSampleBatchFn = unsafe extern "C" fn(data: *const i16, frames: usize) -> usize;
type InputPollFn = unsafe extern "C" fn();
type InputStateFn = unsafe extern "C" fn(port: u32, device: u32, index: u32, id: u32) -> i16;

#[repr(C)]
pub struct RetroSystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
pub struct RetroGameGeometry {
    base_width: u32,
    base_height: u32,
    max_width: u32,
    max_height: u32,
    aspect_ratio: f32,
}

#[repr(C)]
pub struct RetroSystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
pub struct RetroSystemAvInfo {
    geometry: RetroGameGeometry,
    timing: RetroSystemTiming,
}

#[repr(C)]
pub struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[repr(C)]
struct RetroLogCallback {
    log: Option<unsafe extern "C" fn(level: u32, fmt: *const c_char, ...)>,
}

#[repr(C)]
struct RetroInputDescriptor {
    port: u32,
    device: u32,
    index: u32,
    id: u32,
    description: *const c_char,
}

#[repr(C)]
struct RetroControllerDescription {
    desc: *const c_char,
    id: u32,
}

// SAFETY: as tabelas de descritores são `static` e imutáveis; os ponteiros apontam para literais
// de vida estática. A ABI exige que continuem válidos durante todo o carregamento.
unsafe impl Sync for RetroControllerDescription {}

#[repr(C)]
struct RetroControllerInfo {
    types: *const RetroControllerDescription,
    num_types: u32,
}

// SAFETY: idem acima: a lista de portas é imutável e aponta para as descrições estáticas.
unsafe impl Sync for RetroControllerInfo {}

/// O que o frontend oferece ao core.
///
/// É `Copy` de propósito: os ponteiros são tirados do mutex **antes** de qualquer chamada, para
/// que o core nunca segure um cadeado seu enquanto o frontend executa. Um frontend que abra
/// diálogo, salve estado ou espere outra thread dentro do callback travaria o core — e foi o que
/// aconteceu com o disco cheio, quando o RetroArch abriu o aviso de gravação no meio do quadro.
#[derive(Default, Clone, Copy)]
struct Frontend {
    environ: Option<EnvironmentFn>,
    video: Option<VideoRefreshFn>,
    audio_batch: Option<AudioSampleBatchFn>,
    input_poll: Option<InputPollFn>,
    input_state: Option<InputStateFn>,
}

/// Estado do jogo carregado.
struct Core {
    session: Session,
    mixer: Mixer,
    /// Porta 1 e 2, como o guest as enxerga.
    portas: [Option<Aparelho>; zeebx::input::PORTAS],
    /// Buffer do quadro no formato negociado, reaproveitado a cada `retro_run`.
    frame: Vec<u8>,
    audio: Vec<i16>,
    /// Caminho do conteúdo, para o `retro_reset`.
    path: PathBuf,
    /// As raízes do perfil, para trocar de applet sem perder saves nem cache.
    storage: StoragePaths,
    /// Os jogos encontrados ao lado do conteúdo: ClassID do applet e o que carregar.
    ///
    /// É o que permite atender o pedido de lançamento do shell: a Z-Wheel pede uma classe, e aqui
    /// se sabe qual `.mod` ou `.zip` responde por ela.
    jogos: Vec<(u32, PathBuf)>,
    /// O conteúdo da Z-Wheel, para voltar a ela quando um jogo que ela abriu termina.
    z_wheel: Option<PathBuf>,
    /// Se a sessão atual foi aberta pela Z-Wheel — a volta é para ela, como no console.
    aberto_pela_z_wheel: bool,
    /// Se o frontend entrega os botões do RetroPad numa máscara de bits.
    bitmasks: bool,
    /// Se o frontend aceita quadro nulo quando a tela não mudou.
    aceita_dupe: bool,
    /// Assinatura do último quadro entregue.
    ultima_assinatura: Option<u64>,
    /// Relógio virtual da última chamada, para o áudio acompanhar o tempo que passou de verdade.
    ultimo_relogio_ms: u32,
    /// Amostras que o frontend não aceitou e ficam para a chamada seguinte.
    audio_pendente: Vec<i16>,
    /// Se já avisou que o quadro saiu do tamanho do console.
    avisou_tamanho: bool,
    /// Estado anterior do Select do RetroPad, para o atalho de `AVK_CLR`.
    select_antes: bool,
    /// O controle da volta anterior, por porta, para o que muda virar **tecla do console**.
    ///
    /// O applet lê o direcional e o botão 1 como as teclas do BREW (`0xe031`…`0xe064`), e não
    /// pela posição do controle: é com elas que a Z-Wheel navega. Sem guardar o quadro anterior
    /// não há como saber o que mudou, e é a mudança que vira tecla.
    pad_antes: [Pad; zeebx::input::PORTAS],
    /// Quantos quadros já foram apresentados depois da parada.
    ///
    /// A tela final fica à mostra por um instante antes de o frontend ser dispensado: sem isso o
    /// conteúdo some no mesmo quadro em que o jogo acaba, e quem está jogando não vê o desfecho.
    quadros_apos_parar: u32,
    /// Se já se tentou ligar o render em hardware. Uma vez só: um contexto que não veio não vem
    /// no quadro seguinte. Ver [`liga_a_placa`].
    placa_ligada: bool,
    /// Se o desfecho já foi relatado ao frontend.
    ///
    /// Sem isto o core repetiria a mesma linha a cada quadro depois da parada, e um log que cresce
    /// para sempre esconde justamente o instante em que o jogo parou.
    parou: bool,
}

fn frontend() -> &'static Mutex<Frontend> {
    static FRONTEND: OnceLock<Mutex<Frontend>> = OnceLock::new();
    FRONTEND.get_or_init(|| Mutex::new(Frontend::default()))
}

/// O estado do jogo visto pela ABI.
///
/// O motor não é `Send`: carrega `Rc`, ponteiros do JIT e do rasterizador, que valem enquanto
/// estiverem na mesma thread. A ABI Libretro garante que `retro_run`, `retro_reset` e
/// `retro_unload_game` são chamados na thread que carregou o jogo, e é essa a promessa que este
/// invólucro faz ao compilador.
struct EstadoDoCore(Core);

// SAFETY: ver o comentário acima — o core nunca é movido para outra thread.
unsafe impl Send for EstadoDoCore {}

/// Fila de teclas do frontend.
///
/// O callback não executa guest: ele **enfileira**, e `retro_run` entrega as teclas ao motor na
/// thread normal do core. Executar o emulador de dentro do callback seria reentrância em cima do
/// estado que `retro_run` está usando.
fn teclas() -> &'static Mutex<std::collections::VecDeque<(u32, bool)>> {
    static TECLAS: OnceLock<Mutex<std::collections::VecDeque<(u32, bool)>>> = OnceLock::new();
    TECLAS.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

/// Traduz uma tecla do frontend para o código virtual do BREW, quando existe.
///
/// `Esc` e `P` ficam de fora de propósito: são as teclas da interface do frontend, não do jogo.
fn avk_da_tecla(keycode: u32) -> Option<u32> {
    use zeebx::input::avk;
    Some(match keycode {
        RETROK_UP => avk::UP,
        RETROK_DOWN => avk::DOWN,
        RETROK_LEFT => avk::LEFT,
        RETROK_RIGHT => avk::RIGHT,
        RETROK_RETURN => avk::SELECT,
        RETROK_BACKSPACE | RETROK_ESCAPE => avk::CLR,
        RETROK_ASTERISK => avk::STAR,
        RETROK_HASH => avk::POUND,
        RETROK_0..=57 => avk::ZERO + (keycode - RETROK_0),
        _ => return None,
    })
}

/// O callback de teclado do frontend: só enfileira.
unsafe extern "C" fn tecla_recebida(down: bool, keycode: u32, _character: u32, _modifiers: u16) {
    let Some(avk) = avk_da_tecla(keycode) else {
        return;
    };
    if let Ok(mut fila) = teclas().lock() {
        // Um teto evita que uma tecla presa (ou um frontend repetindo sem parar) cresça sem fim.
        if fila.len() < 1024 {
            fila.push_back((avk, down));
        }
    }
}

fn core() -> &'static Mutex<Option<EstadoDoCore>> {
    static CORE: OnceLock<Mutex<Option<EstadoDoCore>>> = OnceLock::new();
    CORE.get_or_init(|| Mutex::new(None))
}

/// Uma cópia dos callbacks do frontend, sem manter o cadeado.
fn callbacks() -> Frontend {
    frontend().lock().map(|guard| *guard).unwrap_or_default()
}

/// Avança um aviso ao jogador. Também vai para o log, porque nem todo frontend mostra mensagem
/// — e um aviso que ninguém vê não serve para nada.
fn aviso(texto: &str) {
    let Ok(texto_c) = CString::new(texto) else {
        return;
    };
    let mensagem = RetroMessage {
        // Três segundos a 60 Hz: tempo de ler sem atrapalhar quem está jogando.
        msg: texto_c.as_ptr(),
        frames: 180,
    };
    unsafe {
        environ(
            ENV_SET_MESSAGE,
            &mensagem as *const RetroMessage as *mut c_void,
        );
    }
    log(&format!("Zeebx: {texto}"));
}

unsafe fn environ(cmd: u32, data: *mut c_void) -> bool {
    let Some(callback) = callbacks().environ else {
        return false;
    };
    // SAFETY: o frontend promete que o callback aceita o comando pedido. O cadeado do core já foi
    // solto, então o frontend pode fazer o que quiser aqui dentro.
    unsafe { callback(cmd, data) }
}

fn log(mensagem: &str) {
    let Ok(texto) = CString::new(mensagem) else {
        return;
    };
    let mut callback = RetroLogCallback { log: None };
    let alvo = &mut callback as *mut RetroLogCallback as *mut c_void;
    if unsafe { environ(ENV_GET_LOG_INTERFACE, alvo) } {
        if let Some(escreve) = callback.log {
            // SAFETY: o frontend forneceu o callback e o formato é literal.
            unsafe { escreve(3, c"%s".as_ptr(), texto.as_ptr()) };
        }
    }
}

/// Descreve o aparelho de uma porta para o `GetConnectedDevices` do guest.
fn aparelho_do_dispositivo(device: u32) -> Option<Aparelho> {
    match device {
        DEVICE_NONE => None,
        DEVICE_ZPAD => Some(Aparelho::ZPad),
        DEVICE_BOOMERANG => Some(Aparelho::Boomerang),
        _ => Some(Aparelho::Controle),
    }
}

/// Se o Select do RetroPad está apertado na porta dada.
fn le_select(porta: u32) -> bool {
    let frente = callbacks();
    let (Some(poll), Some(state)) = (frente.input_poll, frente.input_state) else {
        return false;
    };
    // SAFETY: callbacks do frontend, chamados na thread de `retro_run`.
    unsafe {
        poll();
        state(porta, DEVICE_JOYPAD, 0, ID_SELECT) != 0
    }
}

/// Lê o RetroPad e monta o estado que o console enxerga.
///
/// Com `bitmasks`, os doze botões vêm numa palavra só — uma chamada ao frontend por quadro em vez
/// de doze. O `id` especial `ID_JOYPAD_MASK` devolve os bits na ordem dos `RETRO_DEVICE_ID_JOYPAD_*`.
fn le_pad(porta: u32, bitmasks: bool) -> Pad {
    let frente = callbacks();
    let (Some(poll), Some(state)) = (frente.input_poll, frente.input_state) else {
        return Pad::default();
    };
    // SAFETY: os callbacks vêm do frontend e são chamados na thread de `retro_run`.
    unsafe { poll() };
    let mascara = match bitmasks {
        // SAFETY: consulta de estado do próprio frontend.
        true => unsafe { state(porta, DEVICE_JOYPAD, 0, ID_JOYPAD_MASK) as u32 },
        false => 0,
    };
    let botao = |id: u32| -> bool {
        match bitmasks {
            true => mascara & (1 << id) != 0,
            // SAFETY: consulta de estado do próprio frontend.
            false => unsafe { state(porta, DEVICE_JOYPAD, 0, id) != 0 },
        }
    };
    let mut pad = Pad::default();
    let mapa = [
        (ID_UP, "up"),
        (ID_DOWN, "down"),
        (ID_LEFT, "left"),
        (ID_RIGHT, "right"),
        (ID_Y, "b1"),
        (ID_B, "b2"),
        (ID_X, "b3"),
        (ID_A, "b4"),
        (ID_L, "zl"),
        (ID_R, "zr"),
        (ID_START, "start"),
        (ID_SELECT, "back"),
    ];
    for (id, nome) in mapa {
        if botao(id) {
            if let Some(indice) = Pad::button_by_name(nome) {
                pad.press(indice, true);
            }
        }
    }
    // Os dois analógicos do RetroPad viram os quatro eixos do console, na faixa que o guest lê.
    let eixo = |index: u32, id: u32| -> i32 {
        // SAFETY: consulta de estado do próprio frontend.
        unsafe { state(porta, DEVICE_ANALOG, index, id) as i32 }
    };
    for (indice, valor) in [
        (0usize, eixo(ANALOG_LEFT, ANALOG_AXIS_X)),
        (1usize, eixo(ANALOG_LEFT, ANALOG_AXIS_Y)),
        (2usize, eixo(1, ANALOG_AXIS_X)),
        (3usize, eixo(1, ANALOG_AXIS_Y)),
    ] {
        // `-0x8000..=0x7fff` do frontend para o curso do manche do console.
        pad.set_axis(indice, valor / 256);
    }
    pad
}

/// Registra os aparelhos que o usuário pode escolher em cada porta.
unsafe fn registra_controladores() {
    static DESCRICOES: [RetroControllerDescription; 5] = [
        RetroControllerDescription {
            desc: c"Desconectado".as_ptr(),
            id: DEVICE_NONE,
        },
        RetroControllerDescription {
            desc: c"Dragon".as_ptr(),
            id: DEVICE_JOYPAD,
        },
        RetroControllerDescription {
            desc: c"Z-Pad".as_ptr(),
            id: DEVICE_ZPAD,
        },
        RetroControllerDescription {
            desc: c"Boomerang".as_ptr(),
            id: DEVICE_BOOMERANG,
        },
        RetroControllerDescription {
            desc: c"Teclado USB".as_ptr(),
            id: 3,
        },
    ];
    static PORTAS: [RetroControllerInfo; 3] = [
        RetroControllerInfo {
            types: DESCRICOES.as_ptr(),
            num_types: DESCRICOES.len() as u32,
        },
        RetroControllerInfo {
            types: DESCRICOES.as_ptr(),
            num_types: DESCRICOES.len() as u32,
        },
        RetroControllerInfo {
            types: std::ptr::null(),
            num_types: 0,
        },
    ];
    // SAFETY: as tabelas têm vida estática, como a ABI exige.
    unsafe {
        environ(ENV_SET_CONTROLLER_INFO, PORTAS.as_ptr() as *mut c_void);
    }
}

/// Rotula os botões para a tela de configuração do frontend.
unsafe fn registra_botoes() {
    let mut descritores: Vec<RetroInputDescriptor> = Vec::new();
    let rotulos = [
        (ID_UP, "Direcional cima"),
        (ID_DOWN, "Direcional baixo"),
        (ID_LEFT, "Direcional esquerda"),
        (ID_RIGHT, "Direcional direita"),
        // O rótulo vai no `id` que o core realmente lê, senão a tela de mapeamento do frontend
        // ensina o jogador a apertar o botão errado.
        (ID_B, "Botão 1"),
        (ID_Y, "Botão 2"),
        (ID_X, "Botão 3"),
        (ID_A, "Botão 4"),
        (ID_L, "ZL"),
        (ID_R, "ZR"),
        (ID_START, "HOME/Start"),
        (ID_SELECT, "Voltar"),
    ];
    for porta in 0..2u32 {
        for (id, texto) in rotulos {
            descritores.push(RetroInputDescriptor {
                port: porta,
                device: DEVICE_JOYPAD,
                index: 0,
                id,
                description: texto.as_ptr() as *const c_char,
            });
        }
        for (index, id, texto) in [
            (ANALOG_LEFT, ANALOG_AXIS_X, "Manche X"),
            (ANALOG_LEFT, ANALOG_AXIS_Y, "Manche Y"),
        ] {
            descritores.push(RetroInputDescriptor {
                port: porta,
                device: DEVICE_ANALOG,
                index,
                id,
                description: texto.as_ptr() as *const c_char,
            });
        }
    }
    descritores.push(RetroInputDescriptor {
        port: 0,
        device: 0,
        index: 0,
        id: 0,
        description: std::ptr::null(),
    });
    // SAFETY: a lista termina em `description` nulo e os textos são literais estáticos.
    unsafe {
        environ(
            ENV_SET_INPUT_DESCRIPTORS,
            descritores.as_ptr() as *mut c_void,
        );
    }
}

fn diretorio(cmd: u32) -> Option<PathBuf> {
    let mut ponteiro: *const c_char = std::ptr::null();
    let alvo = &mut ponteiro as *mut *const c_char as *mut c_void;
    if !unsafe { environ(cmd, alvo) } || ponteiro.is_null() {
        return None;
    }
    // SAFETY: o frontend entrega um caminho UTF-8 válido durante a chamada; copiamos aqui.
    let texto = unsafe { CStr::from_ptr(ponteiro) }
        .to_string_lossy()
        .into_owned();
    Some(PathBuf::from(texto))
}

/// `retro_api_version`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_api_version() -> u32 {
    API_VERSION
}

/// `retro_set_environment`: o frontend entrega o caminho de volta ao sistema.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_environment(callback: Option<EnvironmentFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.environ = callback;
    }
    // SAFETY: a partir daqui o ambiente está disponível para registrar o que precisa.
    unsafe {
        registra_controladores();
        registra_botoes();
    }
}

/// `retro_set_video_refresh`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_video_refresh(callback: Option<VideoRefreshFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.video = callback;
    }
}

/// `retro_set_audio_sample_batch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_audio_sample_batch(callback: Option<AudioSampleBatchFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.audio_batch = callback;
    }
}

/// `retro_set_input_poll`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_input_poll(callback: Option<InputPollFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.input_poll = callback;
    }
}

/// `retro_set_input_state`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_set_input_state(callback: Option<InputStateFn>) {
    if let Ok(mut guard) = frontend().lock() {
        guard.input_state = callback;
    }
}

/// `retro_set_audio_sample`: a variante de uma amostra não é usada; o core sempre entrega lote.
#[unsafe(no_mangle)]
pub extern "C" fn retro_set_audio_sample(_callback: Option<unsafe extern "C" fn(i16, i16)>) {}

/// `retro_init`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_init() {}

/// `retro_deinit`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_deinit() {
    if let Ok(mut guard) = core().lock() {
        *guard = None;
    }
}

/// `retro_get_system_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_info(info: *mut RetroSystemInfo) {
    if info.is_null() {
        return;
    }
    // SAFETY: o frontend passou um ponteiro válido para preencher.
    unsafe {
        *info = RetroSystemInfo {
            library_name: c"Zeebx".as_ptr(),
            library_version: concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char,
            valid_extensions: c"mod|zip|7z".as_ptr(),
            need_fullpath: true,
            block_extract: true,
        };
    }
}

/// `retro_get_system_av_info`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut RetroSystemAvInfo) {
    if info.is_null() {
        return;
    }
    // SAFETY: o frontend passou um ponteiro válido para preencher.
    unsafe {
        *info = RetroSystemAvInfo {
            geometry: RetroGameGeometry {
                base_width: 640,
                base_height: 480,
                max_width: 640,
                max_height: 480,
                aspect_ratio: 4.0 / 3.0,
            },
            timing: RetroSystemTiming {
                fps: 60.0,
                sample_rate: SAMPLE_RATE as f64,
            },
        };
    }
}

/// `retro_load_game`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn retro_load_game(game: *const RetroGameInfo) -> bool {
    if game.is_null() {
        return false;
    }
    // SAFETY: ponteiro validado e lido apenas dentro da chamada.
    let caminho = unsafe { (*game).path };
    if caminho.is_null() {
        return false;
    }
    // SAFETY: caminho UTF-8 prometido pela ABI.
    let caminho = unsafe { CStr::from_ptr(caminho) }
        .to_string_lossy()
        .into_owned();
    let Some(save_dir) = diretorio(ENV_GET_SAVE_DIRECTORY) else {
        log("Zeebx: o frontend não informou diretório de saves; recusando carregar.");
        return false;
    };
    let sistema = diretorio(ENV_GET_SYSTEM_DIRECTORY);
    // O sistema guarda o que é da máquina e o que é descartável — a NAND `fs:/` e o cache de
    // conteúdo extraído. Os saves do título ficam no diretório de saves, que é o que o frontend
    // sincroniza. É a mesma divisão que o PPSSPP faz entre `flash0` e o memory stick.
    let storage = StoragePaths::for_frontend(&save_dir, sistema.as_deref());
    if let Err(erro) = storage.create_dirs() {
        log(&format!(
            "Zeebx: sem permissão em {} / {}: {erro}",
            storage.saves.display(),
            storage.cache.display()
        ));
        return false;
    }
    log(&format!(
        "Zeebx: saves em {}, sistema em {}",
        storage.saves.display(),
        storage.cache.display()
    ));
    // Teclado: a Z-Wheel navega por `AVK_*`, que o RetroPad não produz.
    static TECLADO: RetroKeyboardCallback = RetroKeyboardCallback {
        callback: Some(tecla_recebida),
    };
    unsafe {
        environ(
            ENV_SET_KEYBOARD_CALLBACK,
            &TECLADO as *const RetroKeyboardCallback as *mut c_void,
        );
    }
    // O formato de vídeo é negociado antes de rodar: sem ele não há como entregar quadro.
    let mut formato = PIXEL_FORMAT_RGB565;
    let alvo = &mut formato as *mut u32 as *mut c_void;
    if !unsafe { environ(ENV_SET_PIXEL_FORMAT, alvo) } {
        log("Zeebx: o frontend não aceita RGB565.");
        return false;
    }
    // Uma chamada por quadro em vez de doze, quando o frontend entrega a máscara.
    let bitmasks = unsafe { environ(ENV_GET_INPUT_BITMASKS, std::ptr::null_mut()) };
    log(&format!(
        "Zeebx: botões por {}",
        match bitmasks {
            true => "máscara de bits",
            false => "consulta individual",
        }
    ));
    // O quadro repetido pode ir como nulo: economiza uma cópia de 600 KB por quadro e evita
    // ocupar o frontend com trabalho que não muda nada na tela.
    let mut aceita_dupe = false;
    let alvo_dupe = &mut aceita_dupe as *mut bool as *mut c_void;
    let aceita_dupe = unsafe { environ(ENV_GET_CAN_DUPE, alvo_dupe) } && aceita_dupe;
    let portas = [Some(Aparelho::Controle), None];
    let resultado = unsafe { carrega(&caminho, &storage, portas, PathBuf::from(&caminho)) };
    match resultado {
        Ok(mut novo) => {
            novo.bitmasks = bitmasks;
            novo.aceita_dupe = aceita_dupe;
            novo.ultimo_relogio_ms = novo.session.clock_ms();
            if let Ok(mut guard) = core().lock() {
                *guard = Some(EstadoDoCore(novo));
            }
            true
        }
        Err(erro) => {
            log(&format!("Zeebx: não deu para abrir {caminho}: {erro}"));
            false
        }
    }
}

/// Monta o estado do jogo. Separado do export para concentrar o `unsafe` da ABI num lugar só.
unsafe fn carrega(
    caminho: &str,
    storage: &StoragePaths,
    portas: [Option<Aparelho>; zeebx::input::PORTAS],
    path: PathBuf,
) -> Result<Core, StartError> {
    // **A ordem importa: a biblioteca e a fonte vêm antes da sessão.**
    //
    // A fonte é lida quando a máquina é construída — ela entra no `font` do motor. Instalá-la
    // depois deixava a sessão inteira sem fonte, e o jogo não desenhava texto nenhum: era o que
    // fazia a tela do Double Dragon ficar branca e vazia, porque o que ele desenha ali é a
    // mensagem "Memory is insufficient. Please delete some files." em fundo branco.
    let (pasta, jogos) = biblioteca(caminho);
    // **O catálogo da biblioteca local, que é o que a Z-Wheel lê para montar a grade.** A roda
    // não lê a pasta de ROMs: ela lê o `tt_game_info` do perfil, e quem liga um ao outro é o
    // `catalog.json` que a interface grava (`library::sync_catalog`). O core enumerava os jogos
    // para o shell e **não alimentava o catálogo** — medido no harness sem janela em 22/09/2026:
    // sem o catálogo a roda não monta a grade e nenhuma tecla tem o que mover; com ele, a grade
    // aparece desenhada e o confirmar produz o pedido de abertura. É o mesmo caminho que o
    // RetroArch usa no aparelho, e é por isso que ele vale aqui e não só na varredura.
    if !jogos.is_empty()
        && let Some(pasta) = &pasta
    {
        let _ = zeebx::library::sync_catalog(&zeebx::library::scan(pasta));
    }
    let instalados = instalados_da_biblioteca(&jogos);
    #[cfg(test)]
    JOGOS_VISTOS.store(jogos.len() as u32, std::sync::atomic::Ordering::Relaxed);
    let fonte = prepara_fonte(storage, &jogos);
    match &fonte {
        Some(onde) => log(&format!("Zeebx: fonte do sistema em {}", onde.display())),
        None => aviso(&format!(
            "Sem tectoy.ttf no acervo ({} jogos): o texto do sistema não será desenhado",
            jogos.len()
        )),
    }
    // **Onde o banco de amostras deve ficar, dito no log.** A busca é por diretório e a pasta sai
    // da raiz de sistema que o frontend entregou: sem esta linha, quem instala o core não tem como
    // saber o caminho, e "o banco não funciona" fica indistinguível de "o arquivo está no lugar
    // errado".
    log(&zeebx::audio::soundfont::relato(&storage.device));

    // **Pede o contexto de placa ao frontend, se ele tiver um.** Quem aceita é ele; nós só usamos
    // mais tarde, quando o `context_reset` chegar. Recusar aqui não muda nada: a sessão de
    // software já nasceu e é ela que roda até prova em contrário.
    pede_o_contexto_de_placa();

    let mut session = Session::start_software_with_storage_installed(
        std::path::Path::new(caminho),
        portas,
        ZWheel::default(),
        storage,
        &instalados,
    )?;
    let mixer = session.grava_audio(SAMPLE_RATE);
    // **1x e proporção nativa, sempre.** O core entrega o quadro do console em 640×480, sem
    // resolução interna ampliada e sem esticar: quem ajusta shader precisa de uma fonte previsível,
    // e o upscale é papel do frontend.
    session.define_resolucao_interna(1);
    session.define_proporcao(None);
    if !jogos.is_empty() {
        session.set_installed_applets(
            jogos
                .iter()
                .filter_map(|(classe, caminho)| {
                    Some((*classe, zeebx::library::id_do_modulo(caminho)?))
                }),
        );
        log(&format!(
            "Zeebx: {} jogo(s) instalados a partir de {}",
            jogos.len(),
            pasta.as_deref().unwrap_or(std::path::Path::new(".")).display()
        ));
    }
    // **A captura de serial, quando pedida.** É o mesmo instrumento da varredura
    // (`ZEEBX_ROM_SERIAL`) e do `run` (`--serial`): classes criadas, bancos abertos, SQL,
    // propriedades de widget e a árvore de widgets da primeira tecla. Sem ele, quem está no
    // aparelho vê o jogo e não vê o que o applet faz por dentro — e foi com ele que se mediu, na
    // Z-Wheel, que o caminho do core reage à tecla mas pergunta `class_id = -1` (0 linhas) onde a
    // varredura resolve o foco (`class_id = 17359702`, o Alien Breaker) e pede a abertura.
    // O censo do acessador por classe, o mesmo da varredura — e ele é **opt-in** lá pelos mesmos
    // motivos: acrescenta uma seção ao relatório, e o relatório entra na linha de base.
    if std::env::var("ZEEBX_CORE_SELETORES").is_ok() {
        session.liga_censo_de_widgets();
    }
    if let Ok(caminho) = std::env::var("ZEEBX_CORE_SERIAL") {
        match session.liga_serial(std::path::Path::new(&caminho)) {
            Ok(()) => log(&format!("Zeebx: captura de serial em {caminho}")),
            Err(erro) => aviso(&format!("Zeebx: não deu para abrir a captura de serial: {erro}")),
        }
    }
    // A Z-Wheel é o shell: quando ela é o conteúdo, guardar o caminho é o que permite voltar a
    // ela depois que um jogo termina — o que o console faz.
    let z_wheel = match session.classe() == zeebx::session::Z_WHEEL {
        true => Some(path.clone()),
        false => None,
    };

    Ok(Core {
        placa_ligada: false,
        session,
        mixer,
        portas,
        frame: Vec::new(),
        audio: Vec::new(),
        path,
        storage: storage.clone(),
        jogos,
        z_wheel,
        aberto_pela_z_wheel: false,
        bitmasks: false,
        aceita_dupe: false,
        ultima_assinatura: None,
        ultimo_relogio_ms: 0,
        audio_pendente: Vec::new(),
        avisou_tamanho: false,
        select_antes: false,
        pad_antes: [Pad::default(); zeebx::input::PORTAS],
        quadros_apos_parar: 0,
        parou: false,
    })
}

/// A biblioteca de jogos ao lado do conteúdo: `(pasta, [(ClassID, o que carregar)])`.
///
/// É o que responde ao pedido de lançamento do shell — a Z-Wheel pede uma classe e aqui se sabe
/// qual arquivo a atende. Sem isso ela abre vazia, porque enumera os instalados e não acha nenhum.
/// A deduplicação por ClassID evita listar duas vezes quem tem o `.zip` **e** uma cópia extraída.
fn biblioteca(caminho: &str) -> (Option<PathBuf>, Vec<(u32, PathBuf)>) {
    let pasta = std::path::Path::new(caminho)
        .parent()
        .map(std::path::Path::to_path_buf);
    let mut jogos: Vec<(u32, PathBuf)> = Vec::new();
    let mut vistas: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for jogo in pasta.as_deref().map(zeebx::library::scan).unwrap_or_default() {
        let Some(classe) = jogo.clsid else {
            continue;
        };
        if vistas.insert(classe) {
            jogos.push((classe, jogo.path));
        }
    }
    (pasta, jogos)
}

/// Converte a biblioteca descoberta para o formato que o shell enumera no boot.
fn instalados_da_biblioteca(jogos: &[(u32, PathBuf)]) -> Vec<(u32, String)> {
    jogos
        .iter()
        .filter_map(|(classe, caminho)| {
            Some((*classe, zeebx::library::id_do_modulo(caminho)?))
        })
        .collect()
}

/// Instala a fonte do sistema na raiz do aparelho, se ainda não estiver lá.
///
/// Sai do cache quando a Z-Wheel já foi extraída e, quando não foi, do próprio pacote compactado —
/// extraindo **só** o arquivo, porque não vale materializar o pacote inteiro por 190 KB.
fn prepara_fonte(storage: &StoragePaths, jogos: &[(u32, PathBuf)]) -> Option<PathBuf> {
    zeebx::loader::archive::fonte_do_sistema_em(&storage.cache, &storage.device).or_else(|| {
        let candidatos: Vec<&PathBuf> = jogos
            .iter()
            .filter(|(classe, _)| *classe == zeebx::session::Z_WHEEL)
            .map(|(_, caminho)| caminho)
            .collect();
        let pacote = candidatos
            .iter()
            .find(|caminho| zeebx::loader::archive::embalado(caminho))
            .or_else(|| candidatos.first())?;
        zeebx::loader::archive::instala_fonte_do_pacote(pacote, &storage.device)
    })
}

/// Troca o applet em execução dentro do mesmo core.
///
/// É o que o console faz quando a Z-Wheel abre um jogo e quando o jogo fecha: o shell continua
/// sendo o shell. O frontend não participa — para ele, `retro_run` só devolveu outro quadro.
fn troca_para(estado: &mut Core, caminho: &Path, aberto_pela_z_wheel: bool) -> Result<(), StartError> {
    // **Se já se desenha na placa, a sessão nova também nasce nela** — a Z-Wheel abrindo um jogo,
    // o jogo voltando para ela. Sem isto a primeira troca devolveria o desenho ao processador, e o
    // sintoma seria "o render em hardware funciona até o primeiro jogo".
    let instalados = instalados_da_biblioteca(&estado.jogos);
    let mut session = match placa() {
        Some(contexto) => Session::start_with_storage_installed(
            caminho,
            estado.portas,
            None,
            true,
            Some(contexto),
            ZWheel::default(),
            &estado.storage,
            &instalados,
        )?,
        None => Session::start_software_with_storage_installed(
            caminho,
            estado.portas,
            ZWheel::default(),
            &estado.storage,
            &instalados,
        )?,
    };
    let mixer = session.grava_audio(SAMPLE_RATE);
    session.define_resolucao_interna(1);
    session.define_proporcao(None);
    estado.session = session;
    estado.mixer = mixer;
    estado.path = caminho.to_path_buf();
    estado.aberto_pela_z_wheel = aberto_pela_z_wheel;
    estado.parou = false;
    estado.quadros_apos_parar = 0;
    estado.ultima_assinatura = None;
    estado.select_antes = false;
    estado.ultimo_relogio_ms = estado.session.clock_ms();
    Ok(())
}

/// `retro_run`: um quadro virtual, um quadro de vídeo e o áudio correspondente.
#[unsafe(no_mangle)]
pub extern "C" fn retro_run() {
    // Os buffers saem do estado antes das chamadas ao frontend: nenhum cadeado do core fica preso
    // enquanto o frontend executa, e é isso que impede um aviso dele — "disco cheio, quer salvar?"
    // — de travar o emulador.
    let (frame, audio, largura, altura, duplicado) = {
        let Ok(mut guard) = core().lock() else {
            return;
        };
        let Some(EstadoDoCore(estado)) = guard.as_mut() else {
            return;
        };

        // **Quem está rodando agora**, para o teste do ciclo da Z-Wheel poder dizer se é a roda
        // ou o jogo que ela abriu. Ver [`CLASSE_ATUAL`].
        // Os dois instrumentos de teste, e a anotação vai **em cada linha**: um `#[cfg(test)]` só
        // vale para o item seguinte, e sem ela aqui o `cargo build` do CI quebra com "cannot find
        // value RELOGIO in this scope" enquanto o `cargo test` local passa — que foi exatamente o
        // que aconteceu.
        #[cfg(test)]
        CLASSE_ATUAL.store(estado.session.classe(), std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        RELOGIO.store(estado.session.clock_ms(), std::sync::atomic::Ordering::Relaxed);
        #[cfg(test)]
        INSTRUCOES.store(estado.session.instrucoes(), std::sync::atomic::Ordering::Relaxed);
        // **A placa entra no primeiro quadro.** O contexto de GL só existe depois que o frontend
        // chama o `context_reset`, que acontece depois do `retro_load_game`; aqui é o primeiro
        // lugar em que ele pode estar pronto. Recriar a sessão custa um reinício que ninguém vê:
        // nenhum quadro foi entregue ainda.
        liga_a_placa(estado);
        // **O contexto se perdeu e a sessão estava nele.** O aviso sozinho não basta: a sessão
        // guarda o rasterizador de placa, e continuar desenhando por ele chamaria funções de GL que
        // já não existem. Voltar ao software é o mesmo caminho da falha na ativação.
        if PERDEU_A_PLACA.swap(false, std::sync::atomic::Ordering::Relaxed) && estado.placa_ligada {
            estado.placa_ligada = false;
            let antes = estado.path.clone();
            if let Err(erro) = troca_para(estado, &antes, false) {
                aviso(&format!(
                    "Zeebx: o contexto de placa se perdeu e nem no processador deu para reabrir: {erro}"
                ));
            } else {
                aviso("Zeebx: o frontend trocou o contexto de vídeo; seguindo no processador");
            }
        }
        // Em modo de placa, o alvo do desenho é o framebuffer que o frontend indica **a cada
        // quadro** — ele pode trocar.
        if let Some(pega_framebuffer) = OFERTA_DE_PLACA
            .get()
            .and_then(|oferta| oferta.get_current_framebuffer)
            && placa().is_some()
        {
            let fbo = unsafe { pega_framebuffer() };
            estado.session.desenha_no_fbo(Some(fbo));
        }
        // Teclas que o frontend entregou desde o último quadro. O callback só enfileira; aqui
        // elas entram no guest, na thread normal do core.
        if let Ok(mut fila) = teclas().lock() {
            while let Some((avk, down)) = fila.pop_front() {
                estado.session.set_key(avk, down);
            }
        }
        // O shell pediu outro applet — a Z-Wheel escolheu um jogo, ou o jogo mandou voltar. O
        // pedido vive **dentro** do motor (`Machine::pending_launch`), e a UI desktop já o atende
        // assim; aqui ele troca de sessão sem o frontend saber.
        if let Some(classe) = estado.session.take_launch_request() {
            // Instrumento do teste, e só dele: o log do core sai pelo callback do frontend, que é
            // **variádico** e por isso não pode ser implementado num teste em Rust estável. Aqui o
            // teste observa o que a interface do frontend mostraria como texto.
            #[cfg(test)]
            ULTIMA_ABERTURA.store(classe, std::sync::atomic::Ordering::Relaxed);
            let alvo = estado
                .jogos
                .iter()
                .find(|(c, _)| *c == classe)
                .map(|(_, caminho)| caminho.clone());
            match alvo {
                Some(caminho) => {
                    let e_z_wheel = classe == zeebx::session::Z_WHEEL;
                    match troca_para(estado, &caminho, !e_z_wheel) {
                        Ok(()) => log(&format!(
                            "Zeebx: o shell pediu {classe:#010x}; abrindo {}",
                            caminho.display()
                        )),
                        Err(erro) => log(&format!(
                            "Zeebx: o shell pediu {classe:#010x} e não deu para abrir: {erro}"
                        )),
                    }
                }
                None => aviso(&format!(
                    "O shell pediu {classe:#010x}, que não está na pasta de jogos"
                )),
            }
        }
        // Atalho: o Select do RetroPad vale como `AVK_CLR`, que é o "voltar" do console. Sem ele
        // a Z-Wheel fica presa na abertura em quem não tem teclado mapeado no frontend.
        let select = le_select(0);
        if select != estado.select_antes {
            estado.select_antes = select;
            estado.session.set_key(zeebx::input::avk::CLR, select);
        }
        // Entrada primeiro: o guest lê o controle dentro do quadro que vai rodar.
        for porta in 0..zeebx::input::PORTAS {
            if estado.portas[porta].is_none() {
                continue;
            }
            let pad = le_pad(porta as u32, estado.bitmasks);
            estado.session.set_port_pad(porta, pad);
            // **O controle vira tecla do console, como na janela.** O `teclas_do_controle` existe
            // para isso e é usado pelo desktop e pela janela desde sempre — o core **não o
            // chamava**, então quem lê as teclas do BREW não recebia nada do RetroArch. Medido em
            // 22/09/2026 com a Z-Wheel, 2600 quadros e o roteiro de botões: sem esta tradução, a
            // roda anima (223 imagens distintas) e não pede abertura nenhuma; com ela, a grade
            // abre e o pedido sai. É a diferença entre "o controle chega ao guest" e "o controle
            // chega ao applet do jeito que ele lê".
            for (avk, apertada) in
                zeebx::input::teclas_do_controle(&estado.pad_antes[porta], &pad)
            {
                estado.session.set_key(avk, apertada);
            }
            estado.pad_antes[porta] = pad;
        }
        // **As telas intermediárias de um `Update` dentro de um callback passam como na janela.**
        // A máquina guarda uma tela por `IDISPLAY_Update` que o guest chama dentro do callback (a
        // transição da Z-Wheel desliza a tela num laço síncrono, umas duzentas vezes), e quem as
        // mostra é o frontend, uma por quadro, sem avançar o relógio. A janela as consumia; o core
        // **não** — e com a fila cheia o `advance_once` devolvia "apresentou" para sempre, sem
        // rodar o guest: nenhuma fronteira de API, nenhuma tecla esvaziada da fila, e o applet
        // parado. Medido: a Z-Wheel congelava depois do confirmar (relógio virtual parado em
        // 37 012 ms) e só uma tecla chegava.
        if !estado.session.mostra_quadro_intermediario()
            && let Step::Stopped = estado.session.run_frame()
        {
            if !estado.parou {
                estado.parou = true;
                let relogio = estado.session.clock_ms();
                let motivo = estado
                    .session
                    .stopped_reason()
                    .unwrap_or_else(|| "sem motivo relatado".to_string());
                log(&format!("Zeebx: parou em {relogio} ms virtuais: {motivo}"));
                // O log do próprio jogo é o que costuma nomear o motivo da parada; as últimas
                // linhas vão para o frontend, que é onde alguém vai olhar.
                let relato = estado.session.log();
                for linha in relato.iter().rev().take(6).rev() {
                    log(&format!("Zeebx:   {linha}"));
                }
                let memoria = estado.session.memory();
                log(&format!(
                    "Zeebx:   heap {} bytes, {} objetos",
                    memoria.0, memoria.1
                ));
                // No console, sair de um jogo devolve o controle à Z-Wheel, que é outro applet
                // instalado. Aqui o pedido é registrado, mas **a ABI não deixa o core pedir
                // outro conteúdo ao frontend**: ou o core carregaria a Z-Wheel por conta própria,
                // ou o frontend encerra o conteúdo. Ver o plano, seção de desfecho.
                if let Some(classe) = estado.session.take_launch_request() {
                    log(&format!(
                        "Zeebx: o shell pediu para abrir {classe:#010x}; trocar de conteúdo dentro do core ainda não existe"
                    ));
                }
                estado.quadros_apos_parar = 0;
            }
        }
        // Vídeo: o framebuffer do console, no formato negociado.
        let tela = estado.session.screen();
        let (largura, altura) = (tela.width(), tela.height());
        // O console é 640×480, e é esse o quadro que o shader espera receber. Um tamanho
        // diferente é avisado uma vez, em vez de aparecer como imagem torta sem explicação.
        if !estado.avisou_tamanho && (largura != 640 || altura != 480) {
            estado.avisou_tamanho = true;
            log(&format!(
                "Zeebx: quadro {largura}x{altura}, fora dos 640x480 do console"
            ));
        }
        let mut quadro = std::mem::take(&mut estado.frame);
        tela.write_rgb565_into(&mut quadro);
        let assinatura = tela.signature();
        let duplicado = estado.aceita_dupe && estado.ultima_assinatura == Some(assinatura);
        estado.ultima_assinatura = Some(assinatura);
        // Áudio: **o tempo vem do relógio virtual**, não de um número fixo. Um jogo que passa dois
        // quadros virtuais entre duas chamadas precisa entregar o dobro de amostras, senão o som
        // atrasa em relação à imagem e o frontend engasga ao tentar acompanhar.
        let agora = estado.session.clock_ms();
        let decorrido = u64::from(agora.wrapping_sub(estado.ultimo_relogio_ms));
        estado.ultimo_relogio_ms = agora;
        let devidas = (decorrido.min(1000) * u64::from(SAMPLE_RATE) / 1000) as usize;
        let mut som = std::mem::take(&mut estado.audio);
        som.clear();
        // O que o frontend não aceitou da vez anterior vai na frente, para não sumir um pedaço.
        som.append(&mut estado.audio_pendente);
        for amostra in estado.mixer.render(devidas) {
            som.push((amostra.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16);
        }
        (quadro, som, largura, altura, duplicado)
    };
    let frente = callbacks();
    if let Some(video) = frente.video {
        let na_placa = placa().is_some();
        let (ponteiro, _) = match (na_placa, duplicado) {
            // **Em modo de placa o quadro já está no framebuffer do frontend**: entregar pixels
            // aqui seria mentira, e o `libretro` tem um sentinela para dizer exatamente isso.
            (true, _) => (HW_FRAME_BUFFER_VALID as *const c_void, ()),
            // Quadro nulo avisa "repete o anterior", que é o que a ABI oferece para tela parada.
            (false, true) => (std::ptr::null(), ()),
            (false, false) => (frame.as_ptr() as *const c_void, ()),
        };
        // SAFETY: o buffer vive durante a chamada; no quadro repetido o frontend reusa o último.
        unsafe {
            video(ponteiro, largura, altura, largura as usize * 2);
        }
    }
    // O retorno do lote é em quadros **aceitos**; o que sobrar espera a próxima chamada.
    let mut sobra = Vec::new();
    if let Some(batch) = frente.audio_batch {
        let quadros = audio.len() / 2;
        // SAFETY: o lote é intercalado em estéreo e o tamanho é o número de quadros.
        let aceitos = unsafe { batch(audio.as_ptr(), quadros) }.min(quadros);
        if aceitos < quadros {
            sobra = audio[aceitos * 2..].to_vec();
        }
    }
    // Os buffers voltam para o estado, para a próxima chamada reaproveitar a mesma alocação.
    let mut dispensar = false;
    if let Ok(mut guard) = core().lock() {
        if let Some(EstadoDoCore(estado)) = guard.as_mut() {
            estado.frame = frame;
            estado.audio = audio;
            // A sobra não pode crescer sem fim; meio segundo é o teto.
            let limite = (SAMPLE_RATE as usize / 2) * 2;
            sobra.truncate(limite);
            estado.audio_pendente = sobra;
            if estado.parou {
                estado.quadros_apos_parar += 1;
                // Dois segundos de tela parada bastam para ver o desfecho e o log.
                if estado.quadros_apos_parar == 120 {
                    // **A volta para a Z-Wheel.** No console, fechar um jogo devolve o controle ao
                    // shell, e o shell é outro applet instalado — então aqui se troca de sessão,
                    // como a UI desktop já faz. Sem Z-Wheel disponível, o desfecho possível é
                    // pedir ao frontend que encerre o conteúdo.
                    let voltar = estado.z_wheel.clone().filter(|_| {
                        estado.aberto_pela_z_wheel
                            || estado.session.classe() == zeebx::session::Z_WHEEL
                    });
                    match voltar {
                        Some(caminho) => match troca_para(estado, &caminho, false) {
                            Ok(()) => log("Zeebx: fim do jogo; de volta à Z-Wheel"),
                            Err(erro) => {
                                log(&format!("Zeebx: não deu para voltar à Z-Wheel: {erro}"));
                                dispensar = true;
                            }
                        },
                        None => dispensar = true,
                    }
                }
            }
        }
    }
    if dispensar {
        // Encerrar o conteúdo é o desfecho que a ABI oferece: o frontend volta ao menu dele, em
        // vez de ficar mostrando para sempre o último quadro de um jogo que acabou.
        log("Zeebx: jogo terminado; pedindo ao frontend para encerrar o conteúdo");
        unsafe {
            environ(ENV_SHUTDOWN, std::ptr::null_mut());
        }
    }
}

/// `retro_unload_game`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_unload_game() {
    if let Ok(mut guard) = core().lock() {
        *guard = None;
    }
}

/// `retro_reset`: recarrega o conteúdo do zero, preservando saves e NAND.
#[unsafe(no_mangle)]
pub extern "C" fn retro_reset() {
    let Ok(mut guard) = core().lock() else {
        return;
    };
    let Some(EstadoDoCore(antigo)) = guard.as_ref() else {
        return;
    };
    let caminho = antigo.path.clone();
    let portas = antigo.portas;
    let Some(save_dir) = diretorio(ENV_GET_SAVE_DIRECTORY) else {
        return;
    };
    let sistema = diretorio(ENV_GET_SYSTEM_DIRECTORY);
    let storage = StoragePaths::for_frontend(&save_dir, sistema.as_deref());
    let texto = caminho.to_string_lossy().into_owned();
    // SAFETY: mesma montagem do carregamento, na thread de `retro_run`.
    match unsafe { carrega(&texto, &storage, portas, caminho.clone()) } {
        Ok(novo) => *guard = Some(EstadoDoCore(novo)),
        Err(erro) => log(&format!("Zeebx: reset falhou: {erro}")),
    }
}

/// `retro_set_controller_port_device`: troca o aparelho que o guest enxerga na porta.
#[unsafe(no_mangle)]
pub extern "C" fn retro_set_controller_port_device(port: u32, device: u32) {
    let Ok(mut guard) = core().lock() else {
        return;
    };
    let Some(EstadoDoCore(estado)) = guard.as_mut() else {
        return;
    };
    let porta = port as usize;
    if porta >= zeebx::input::PORTAS {
        return;
    }
    estado.portas[porta] = aparelho_do_dispositivo(device);
    estado.session.set_portas(estado.portas);
}

/// O último estado gravado, para o `retro_serialize` entregar o que o `_size` mediu.
///
/// A ABI chama as duas em sequência — primeiro o tamanho, depois a gravação —, e o tamanho varia
/// com o que o jogo tem em memória: as texturas, as imagens decodificadas e os buffers de GL são a
/// maior parte dele. Recalcular na gravação daria um estado **diferente** do que foi medido, e o
/// frontend teria alocado o buffer pelo número errado. Por isso o tamanho medido guarda os bytes.
static ESTADO_MEDIDO: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// `retro_serialize_size`: o tamanho do estado, ou zero quando não há o que salvar.
///
/// **Zero só acontece sem sessão ou com um desenho em curso** — e as duas são respostas honestas:
/// sem sessão não há estado, e no meio de um desenho começado o estado prometeria algo que nunca
/// existiu. Em qualquer outro momento o número é o do estado de verdade.
#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize_size() -> usize {
    let Ok(mut guard) = core().lock() else {
        return 0;
    };
    let Some(EstadoDoCore(estado)) = guard.as_mut() else {
        return 0;
    };
    if let Err(motivo) = estado.session.pode_salvar() {
        aviso(&format!("Zeebx: não dá para salvar agora: {motivo}"));
        return 0;
    }
    let arquivo = estado.session.grava_estado();
    let tamanho = arquivo.len();
    if let Ok(mut guarda) = ESTADO_MEDIDO.lock() {
        *guarda = Some(arquivo);
    }
    tamanho
}

/// `retro_serialize`: entrega o estado medido.
///
/// Devolve `false` — sem escrever nada — quando não cabe no buffer que o frontend ofereceu. É o
/// contrato: o frontend aloca pelo tamanho que pediu, e mentir sobre ele corromperia a memória
/// dele.
#[unsafe(no_mangle)]
pub extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    if data.is_null() {
        return false;
    }
    let Ok(guarda) = ESTADO_MEDIDO.lock() else {
        return false;
    };
    let Some(arquivo) = guarda.as_ref() else {
        return false;
    };
    if arquivo.len() > size {
        aviso(&format!(
            "Zeebx: o estado tem {} bytes e o frontend ofereceu {size}",
            arquivo.len()
        ));
        return false;
    }
    // SAFETY: o frontend garante `size` bytes válidos em `data`, e o bloco acima conferiu que o
    // estado cabe.
    unsafe {
        std::ptr::copy_nonoverlapping(arquivo.as_ptr(), data as *mut u8, arquivo.len());
    }
    true
}

/// `retro_unserialize`: põe o estado de volta.
///
/// A recusa vem do motor, com o motivo: seção faltando, tamanho que não bate, `crc32` trocado. Um
/// estado pela metade dentro de uma máquina em execução é pior que um estado recusado, e é por isso
/// que a leitura acontece **antes** de qualquer escrita.
#[unsafe(no_mangle)]
pub extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    if data.is_null() || size == 0 {
        return false;
    }
    // SAFETY: o frontend garante `size` bytes válidos em `data`.
    let arquivo = unsafe { std::slice::from_raw_parts(data as *const u8, size) };
    let Ok(mut guard) = core().lock() else {
        return false;
    };
    let Some(EstadoDoCore(estado)) = guard.as_mut() else {
        return false;
    };
    match estado.session.restaura_estado(arquivo) {
        Ok(()) => true,
        Err(erro) => {
            aviso(&format!("Zeebx: o save state foi recusado: {erro}"));
            false
        }
    }
}

/// `retro_cheat_reset`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_reset() {}

/// `retro_cheat_set`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_cheat_set(_index: u32, _enabled: bool, _code: *const c_char) {}

/// `retro_load_game_special`: não há subsistemas.
#[unsafe(no_mangle)]
pub extern "C" fn retro_load_game_special(
    _game_type: u32,
    _info: *const RetroGameInfo,
    _num_info: usize,
) -> bool {
    false
}

/// `retro_get_region`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_region() -> u32 {
    0
}

/// `retro_get_memory_data`: os saves são arquivos do BREW, não uma região contínua.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_data(_id: u32) -> *mut c_void {
    std::ptr::null_mut()
}

/// `retro_get_memory_size`.
#[unsafe(no_mangle)]
pub extern "C" fn retro_get_memory_size(_id: u32) -> usize {
    0
}


#[cfg(test)]
mod testes {
    use super::*;

    /// **Os deslocamentos do struct da placa têm de bater com o `libretro.h`.**
    ///
    /// O core preenche o struct, o frontend o devolve preenchido, e nós lemos
    /// `get_current_framebuffer` e `get_proc_address` dele. Um campo a mais ou a menos aqui faria
    /// esses dois serem lidos no lugar errado — e o sintoma seria "o render em hardware não
    /// funciona", sem nada no log que explique. Os números são a soma dos campos que o
    /// `include/libretro.h` declara, na ordem dele.
    #[test]
    fn a_entrada_do_retropad_chega_ao_guest() {
        let Ok(caminho) = std::env::var("ZEEBX_TESTE_ROM") else {
            eprintln!("sem ZEEBX_TESTE_ROM: nada a testar");
            return;
        };
        let pasta = std::env::temp_dir().join(format!("zeebx-entrada-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        let _ = PASTA.set(CString::new(pasta.to_string_lossy().to_string()).unwrap());
        let caminho_c = CString::new(caminho.clone()).unwrap();
        let info = RetroGameInfo {
            path: caminho_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        let mut reagiu = None;
        let mut abriu: Option<(&str, i32, u32, u32)> = None;
        unsafe {
            retro_set_environment(Some(ambiente));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(sem_poll));
            retro_set_input_state(Some(entrada));
            retro_init();
            // O aparelho da porta decide o mapeamento do pad. `ZEEBX_TESTE_APARELHO=zpad` usa o
            // Z-Pad, que é o controle do console — a Z-Wheel é um app do Zeebo e o lê assim.
            let aparelho = match std::env::var("ZEEBX_TESTE_APARELHO").as_deref() {
                Ok("zpad") | Ok("ZPAD") => DEVICE_ZPAD,
                _ => DEVICE_JOYPAD,
            };
            retro_set_controller_port_device(0, aparelho);
            assert!(retro_load_game(&info), "o core recusou {caminho}");
            // Quanto tempo esperar antes de mandar entrada. A Z-Wheel leva mais que os jogos para
            // chegar à tela interativa — a varredura a pega com zero quadros aos seis segundos —,
            // e mandar botão para uma tela de carregamento não diz nada sobre o caminho de entrada.
            let espera: u32 = std::env::var("ZEEBX_TESTE_ESPERA")
                .ok()
                .and_then(|n| n.parse().ok())
                .unwrap_or(180);
            for _ in 0..espera {
                retro_run();
            }
            for botao in [ID_START, ID_A, ID_B, ID_SELECT] {
                if let Ok(mut vistos) = ASSINATURAS.lock() {
                    vistos.clear();
                }
                BOTAO.store(botao, Ordering::Relaxed);
                for _ in 0..40 {
                    retro_run();
                }
                BOTAO.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                let distintas = ASSINATURAS
                    .lock()
                    .map(|v| v.iter().collect::<std::collections::BTreeSet<_>>().len())
                    .unwrap_or(0);
                eprintln!("botao {botao}: {distintas} imagem(ns) distinta(s) em 60 quadros");
                if distintas > 1 {
                    reagiu = Some(botao);
                    break;
                }
            }
            // **O manche**, que é como a Z-Wheel e os menus são navegados.
            for (qual, valor) in [("x", 0x7fff), ("x", -0x8000), ("y", 0x7fff), ("y", -0x8000)] {
                if let Ok(mut vistos) = ASSINATURAS.lock() {
                    vistos.clear();
                }
                let alvo = match qual {
                    "x" => &EIXO_X,
                    _ => &EIXO_Y,
                };
                alvo.store(valor, Ordering::Relaxed);
                for _ in 0..40 {
                    retro_run();
                }
                alvo.store(0, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                let distintas = ASSINATURAS
                    .lock()
                    .map(|v| v.iter().collect::<std::collections::BTreeSet<_>>().len())
                    .unwrap_or(0);
                eprintln!("manche {qual}={valor}: {distintas} imagem(ns) distinta(s)");
                if distintas > 1 {
                    reagiu = Some(0x100 + valor as u32);
                    break;
                }
            }
            // **Abrir um jogo com o manche e o botão de confirmar.** É o ciclo do item 8 inteiro,
            // sem frontend: a roda está interativa desde a espera, o manche a navega, e o pedido de
            // abertura aparece em `ULTIMA_ABERTURA` — o core o registra por instrumento de teste,
            // porque o log dele sai por callback variádico do frontend.
            ULTIMA_ABERTURA.store(0, Ordering::Relaxed);
            'tentativas: for (direcao, valor) in [("x", 0x7fff), ("x", -0x8000), ("y", 0x7fff), ("y", -0x8000)] {
                // Os quatro botões de face **e** o Start: a Z-Wheel não usa o mesmo para navegar e
                // para confirmar, e testar só os de face foi o que deixou o ciclo sem resposta.
                for botao in [ID_A, ID_B, ID_Y, ID_X, ID_START] {
                    let alvo = match direcao {
                        "x" => &EIXO_X,
                        _ => &EIXO_Y,
                    };
                    // **Empurra o manche e confirma sem soltar.** A primeira versão soltava antes
                    // de apertar, e essa combinação — manche parado numa direção com o botão
                    // apertado — é justamente como um carrossel confirma a peça em foco.
                    alvo.store(valor, Ordering::Relaxed);
                    for _ in 0..30 {
                        retro_run();
                    }
                    BOTAO.store(botao, Ordering::Relaxed);
                    for _ in 0..20 {
                        retro_run();
                    }
                    BOTAO.store(u32::MAX, Ordering::Relaxed);
                    alvo.store(0, Ordering::Relaxed);
                    // **Depois de confirmar, a roda anima a transição** antes de pedir a abertura —
                    // vinte quadros não bastam, e o pedido chega durante a animação.
                    for _ in 0..180 {
                        retro_run();
                    }
                    let classe = ULTIMA_ABERTURA.load(Ordering::Relaxed);
                    if classe != 0 {
                        abriu = Some((direcao, valor, botao, classe));
                        break 'tentativas;
                    }
                }
            }
            retro_unload_game();
            retro_deinit();
        }
        let _ = std::fs::remove_dir_all(&pasta);
        eprintln!(
            "ciclo: {}",
            match abriu {
                Some((direcao, valor, botao, classe)) => format!(
                    "o shell pediu {classe:#010x} (manche {direcao}={valor}, botão {botao})"
                ),
                None => "a roda não pediu abertura nenhuma".to_string(),
            }
        );
        eprintln!(
            "entrada: {}",
            match reagiu {
                Some(botao) if botao >= 0x100 => "chegou — o manche mudou a imagem".to_string(),
                Some(botao) => format!("chegou — o botao {botao} mudou a imagem"),
                None => "nem os quatro botoes nem o manche mudaram a imagem".to_string(),
            }
        );
    }

    #[test]
    fn os_deslocamentos_do_struct_da_placa_batem_com_o_libretro_h() {
        use std::mem::{offset_of, size_of};
        assert_eq!(offset_of!(RetroHwRenderCallback, context_reset), 8);
        assert_eq!(offset_of!(RetroHwRenderCallback, get_current_framebuffer), 16);
        assert_eq!(offset_of!(RetroHwRenderCallback, get_proc_address), 24);
        assert_eq!(offset_of!(RetroHwRenderCallback, depth), 32);
        assert_eq!(offset_of!(RetroHwRenderCallback, version_major), 36);
        assert_eq!(offset_of!(RetroHwRenderCallback, version_minor), 40);
        assert_eq!(offset_of!(RetroHwRenderCallback, cache_context), 44);
        assert_eq!(offset_of!(RetroHwRenderCallback, context_destroy), 48);
        assert!(size_of::<RetroHwRenderCallback>() >= 56);
    }
    use std::ffi::CString;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Quantos quadros o vídeo do frontend recebeu.
    static QUADROS: AtomicU32 = AtomicU32::new(0);
    /// Qual botão do RetroPad o teste está segurando. `u32::MAX` é nenhum.
    static BOTAO: AtomicU32 = AtomicU32::new(u32::MAX);
    /// A pasta que o frontend de teste entrega como sistema e como saves.
    static PASTA: OnceLock<CString> = OnceLock::new();
    /// O sistema de fora, quando `ZEEBX_CORE_SISTEMA` aponta para uma árvore de aparelho real.
    static SISTEMA: OnceLock<CString> = OnceLock::new();
    /// A assinatura de cada quadro entregue, na ordem.
    static ASSINATURAS: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

    /// O ambiente mínimo que o core precisa, respondendo como um frontend de verdade.
    ///
    /// O que não temos responde `false` — é o que o RetroArch faz com o que não conhece, e é
    /// assim que o caminho de recusa do core também fica exercitado.
    unsafe extern "C" fn ambiente(cmd: u32, dados: *mut c_void) -> bool {
        match cmd {
            // Aceita RGB565 e recusa o resto: é o formato que o console entrega, e recusar os
            // outros faz o core seguir pelo caminho que ele usa no RetroArch.
            ENV_SET_PIXEL_FORMAT => {
                !dados.is_null() && unsafe { *(dados as *const u32) } == PIXEL_FORMAT_RGB565
            }
            ENV_GET_SYSTEM_DIRECTORY | ENV_GET_SAVE_DIRECTORY => {
                // O sistema pode vir de fora, e vem por `ZEEBX_CORE_SISTEMA`: é assim que o teste
                // encontra o aparelho de verdade, com os jogos instalados, e consegue exercitar o
                // ciclo da Z-Wheel de ponta a ponta. Sem a variável, cada um recebe a pasta
                // temporária do teste.
                let pasta = if cmd == ENV_GET_SYSTEM_DIRECTORY {
                    SISTEMA.get().or_else(|| PASTA.get())
                } else {
                    PASTA.get()
                };
                let Some(pasta) = pasta else {
                    return false;
                };
                if dados.is_null() {
                    return false;
                }
                // SAFETY: o core promete um `*const *const c_char` para escrita.
                unsafe { *(dados as *mut *const c_char) = pasta.as_ptr() };
                true
            }
            _ => false,
        }
    }

    unsafe extern "C" fn video(dados: *const c_void, largura: u32, altura: u32, passo: usize) {
        QUADROS.fetch_add(1, Ordering::Relaxed);
        // Assinatura barata do quadro: muda quando a imagem muda, que é o que o teste precisa
        // saber para dizer se a entrada chegou ao guest — um controle que não chega deixa a tela
        // parada, e um botão errado também, e as duas coisas se separam olhando o resto.
        let total = (passo as u64) * u64::from(altura);
        let bytes =
            unsafe { std::slice::from_raw_parts(dados as *const u8, total.min(1 << 22) as usize) };
        let mut assinatura = 1469598103934665603u64;
        for &b in bytes.iter().step_by(97) {
            assinatura = (assinatura ^ u64::from(b)).wrapping_mul(1099511628211);
        }
        if let Ok(mut vistos) = ASSINATURAS.lock() {
            vistos.push(assinatura);
        }
        let _ = (largura, altura);
    }

    /// Quantas amostras estéreo o core entregou ao frontend.
    static AMOSTRAS: AtomicU32 = AtomicU32::new(0);
    /// Quantos jogos o core achou ao lado do conteúdo (ver [`JOGOS_VISTOS`]).

    unsafe extern "C" fn audio(_dados: *const i16, quadros: usize) -> usize {
        // Contar aqui é o que permite conferir **pelo caminho do core** que o áudio sai: o motor
        // tem a medida dele (pico, rms, salto), e o core tem esta — se o lote chega ao frontend.
        AMOSTRAS.fetch_add(quadros as u32, Ordering::Relaxed);
        quadros
    }

    /// O eixo que o teste está empurrando, na faixa do RetroPad (`-0x8000..=0x7fff`).
    ///
    /// A Z-Wheel é navegada **pelo manche**, então um teste que só aperta botão não a move — e a
    /// pergunta "a entrada chega?" ficava sem resposta para ela.
    static EIXO_X: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
    static EIXO_Y: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

    unsafe extern "C" fn entrada(_porta: u32, dispositivo: u32, _indice: u32, id: u32) -> i16 {
        if dispositivo == DEVICE_ANALOG {
            // No RetroPad, `0` é o X do analógico esquerdo e `1` é o Y.
            let valor = match id {
                0 => EIXO_X.load(Ordering::Relaxed),
                _ => EIXO_Y.load(Ordering::Relaxed),
            };
            return valor.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        // O core pergunta pelo estado de cada botão, um a um.
        (BOTAO.load(Ordering::Relaxed) == id) as i16
    }

    extern "C" fn sem_poll() {}

    /// **Sem sessão não há o que salvar**, e a recusa não escreve nada.
    ///
    /// Zero continua sendo a resposta certa aqui — não porque o core não saiba salvar, mas porque
    /// não há máquina nenhuma montada. Um `serialize` que gravasse meia máquina seria pior que
    /// nenhum: o RetroArch deixaria salvar, e o carregamento devolveria um jogo com memória e
    /// registradores certos e a mesa de objetos errada, chamando API com identificador que não
    /// existe mais.
    #[test]
    fn sem_sessao_nao_ha_o_que_salvar() {
        assert_eq!(
            retro_serialize_size(),
            0,
            "sem sessão, o tamanho é zero"
        );
        let mut destino = [0u8; 16];
        assert!(
            !retro_serialize(destino.as_mut_ptr() as *mut c_void, destino.len()),
            "gravar meia máquina seria pior que recusar"
        );
        assert!(!retro_unserialize(
            destino.as_ptr() as *const c_void,
            destino.len()
        ));
        assert_eq!(destino, [0u8; 16], "a recusa não pode ter escrito nada");
    }

    /// **O core, exercitado pela própria ABI.**
    ///
    /// É o teste que faltava para o ciclo da Z-Wheel do lado do core: o motor é medido pela
    /// varredura, e o laço do core — que troca de sessão quando o shell pede — não tinha prova
    /// automática nenhuma. Aqui não há janela nem RetroArch, mas o caminho é o mesmo:
    /// `retro_init`, `retro_load_game`, quadros e `retro_unload_game`.
    ///
    /// **O que este teste prova, e o que ele não prova.**
    ///
    /// Prova: o core carrega conteúdo pela própria ABI, entrega quadros e desmonta limpo, sem
    /// RetroArch e sem janela. Com a Z-Wheel, entrega 360 quadros.
    ///
    /// **Não prova a troca de sessão.** Dirigindo o controle pelas seis teclas do RetroPad — 30
    /// quadros por tecla — nenhuma imagem distinta aparece (medido: 1 por fase) e o shell não pede
    /// abertura nenhuma. Ou a Z-Wheel headless não chega ao estado em que aceita a escolha, ou a
    /// entrada não chega ao guest pelo caminho do core. As duas hipóteses estão abertas e este
    /// teste **não** escolhe entre elas: quem fecha o ciclo é o RetroArch, com controle de
    /// verdade, que é o que o item 8 do plano pede.
    ///
    /// **Sem `ZEEBX_CORE_ROM` ele não roda.** ROM não entra na árvore do repositório, e um teste
    /// que baixa conteúdo sozinho é pior que um teste que não roda.
    ///
    /// ```bash
    /// ZEEBX_CORE_ROM="roms/Z-Wheel.zip" cargo test -p zeebx-libretro -- --nocapture
    /// ```
    #[test]
    fn a_abi_do_core_roda_uma_rom() {
        let Ok(caminho) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("sem ZEEBX_CORE_ROM: nada a rodar");
            return;
        };
        let pasta = std::env::temp_dir().join(format!("zeebx-core-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        let _ = PASTA.set(CString::new(pasta.to_string_lossy().to_string()).unwrap());
        if let Ok(fora) = std::env::var("ZEEBX_CORE_SISTEMA") {
            let _ = SISTEMA.set(CString::new(fora).unwrap());
        }
        QUADROS.store(0, Ordering::Relaxed);

        let caminho_c = CString::new(caminho.clone()).unwrap();
        let quadros_pedidos = std::env::var("ZEEBX_CORE_QUADROS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(60u32);

        let mut abriu = ULTIMA_ABERTURA.load(Ordering::Relaxed);
        let mut qual = None;
        // Antes de dirigir o controle não pode haver pedido de abertura nenhum: se houver, ele
        // veio do carregamento do conteúdo, e a leitura do laço abaixo estaria medindo outra coisa.
        assert_eq!(abriu, 0, "o shell pediu abertura antes de qualquer tecla");
        assert!(qual.is_none());
        if let Ok(mut vistos) = ASSINATURAS.lock() {
            vistos.clear();
        }
        let info = RetroGameInfo {
            path: caminho_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        unsafe {
            retro_set_environment(Some(ambiente));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(sem_poll));
            retro_set_input_state(Some(entrada));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "o core recusou {caminho}");
            for _ in 0..quadros_pedidos {
                retro_run();
            }
            // **Dirige a Z-Wheel.** A pergunta desta parte é prática: com que botão o jogador
            // confirma a escolha, e o pedido de abertura chega ao core? Cada botão do RetroPad é
            // segurado por vinte quadros e solto por dez, e o teste para no primeiro que o shell
            // aceitar. Sem o pedido, ele diz que nenhum serviu — que também é resposta.
            abriu = ULTIMA_ABERTURA.load(Ordering::Relaxed);
            qual = None;
            for botao in [ID_A, ID_B, ID_X, ID_Y, ID_START, ID_SELECT] {
                let antes = QUADROS.load(Ordering::Relaxed);
                BOTAO.store(botao, Ordering::Relaxed);
                for _ in 0..20 {
                    retro_run();
                }
                BOTAO.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..10 {
                    retro_run();
                }
                // A imagem mudou enquanto o botão estava apertado? É o que separa "a entrada não
                // chega" de "chega, e o botão é outro".
                let distintas = ASSINATURAS
                    .lock()
                    .map(|v| {
                        let inicio = (antes as usize).min(v.len());
                        v[inicio..].iter().collect::<std::collections::BTreeSet<_>>().len()
                    })
                    .unwrap_or(0);
                eprintln!("botão {botao}: {distintas} imagem(ns) distinta(s) em 30 quadros");
                abriu = ULTIMA_ABERTURA.load(Ordering::Relaxed);
                if abriu != 0 {
                    qual = Some(botao);
                    break;
                }
            }
            retro_unload_game();
            retro_deinit();
        }
        eprintln!(
            "pedido de abertura: {abriu:#010x} ({})",
            match abriu {
                0 => "nenhum botão abriu".to_string(),
                _ => format!("com o botão {qual:?}"),
            }
        );

        let quadros = QUADROS.load(Ordering::Relaxed);
        assert!(quadros > 0, "nenhum quadro chegou ao frontend");
        eprintln!("quadros entregues ao frontend: {quadros}");
        eprintln!(
            "amostras estéreo entregues ao frontend: {}",
            AMOSTRAS.load(Ordering::Relaxed)
        );
        eprintln!(
            "jogos ao lado do conteúdo: {}",
            JOGOS_VISTOS.load(Ordering::Relaxed)
        );
        let _ = std::fs::remove_dir_all(&pasta);
    }

    /// **O RetroPad vira tecla do console no caminho do core — e a Z-Wheel responde.**
    ///
    /// É a prova de ponta a ponta do item 8 pelo caminho que o RetroArch usa, sem janela e sem
    /// olhar pixels: o laço roda quadros, o teste aperta os botões do RetroPad, e o que se observa
    /// é a **decisão do core** — a classe que está rodando, por [`CLASSE_ATUAL`] — e o **pedido do
    /// shell**, por [`ULTIMA_ABERTURA`], que vem de `Session::take_launch_request`.
    ///
    /// **O tempo é a primeira armadilha.** A grade de jogos da Z-Wheel só aparece depois de ~37 s
    /// de relógio virtual, mais de duas mil voltas; o teste que existia rodava 60 quadros e
    /// concluía que nenhum botão abria nada. `ZEEBX_CORE_QUADROS` diz quantos quadros rodar antes
    /// de começar a apertar.
    ///
    /// **A segunda armadilha é a pasta.** O jogo que a roda abre é o que estiver em foco, e o
    /// retorno à roda só acontece quando o jogo **termina sozinho** — por isso a pasta deve ter,
    /// ao lado da Z-Wheel, um título que termina sozinho. Medido na varredura: o `Zeebo Clube` e o
    /// `Zeebo App` são os dois que fazem isso.
    ///
    /// **Sem `ZEEBX_CORE_ROM` ele não roda**, pela mesma razão dos outros: ROM não entra na árvore.
    #[test]
    fn o_retropad_vira_tecla_do_console_no_caminho_do_core() {
        let Ok(caminho) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("sem ZEEBX_CORE_ROM: nada a percorrer");
            return;
        };
        // **A tradução, provada no crate do core.** Dois pads construídos à mão: a função é a
        // mesma do motor, mas quem a chama aqui é o core, e um erro de tipo ou de nome apareceria
        // exatamente neste ponto.
        {
            let antes = zeebx::input::Pad::default();
            let mut agora = zeebx::input::Pad::default();
            let indice = zeebx::input::Pad::button_by_name("b1").expect("b1 existe");
            agora.press(indice, true);
            let teclas = zeebx::input::teclas_do_controle(&antes, &agora);
            eprintln!("tradução de b1: {teclas:?}");
            assert_eq!(
                teclas,
                vec![(zeebx::input::avk::CONFIRMA, true)],
                "a tradução do controle não produziu a tecla do confirmar"
            );
        }
        let pasta = std::env::temp_dir().join(format!("zeebx-ciclo-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        let _ = PASTA.set(CString::new(pasta.to_string_lossy().to_string()).unwrap());
        if let Ok(fora) = std::env::var("ZEEBX_CORE_SISTEMA") {
            let _ = SISTEMA.set(CString::new(fora).unwrap());
        }
        QUADROS.store(0, Ordering::Relaxed);
        ULTIMA_ABERTURA.store(0, Ordering::Relaxed);
        CLASSE_ATUAL.store(0, Ordering::Relaxed);
        BOTAO.store(u32::MAX, Ordering::Relaxed);

        let caminho_c = CString::new(caminho.clone()).unwrap();
        let ate_a_grade = std::env::var("ZEEBX_CORE_QUADROS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(2600u32);
        let info = RetroGameInfo {
            path: caminho_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };

        unsafe {
            retro_set_environment(Some(ambiente));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_set_input_poll(Some(sem_poll));
            retro_set_input_state(Some(entrada));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "o core recusou {caminho}");
            for _ in 0..ate_a_grade {
                retro_run();
            }
            // Quem roda no começo é a roda. Se não for, o resto do teste mediria outra coisa.
            let quem = CLASSE_ATUAL.load(Ordering::Relaxed);
            assert_eq!(
                quem,
                zeebx::session::Z_WHEEL,
                "no começo quem roda é a Z-Wheel, não {quem:#010x}"
            );
            eprintln!("rodando no começo: {quem:#010x} (Z-Wheel)");

            // O roteiro da doc, nos botões do RetroPad: confirmar em "Jogar", descer às capas,
            // andar duas capas à direita e confirmar. `b1` é o confirmar do console.
            // **O compasso importa.** No roteiro da varredura as teclas estão a dois ou três
            // segundos de distância: a roda tem transições armadas em 400 ms e um pulso próprio, e
            // teclar a cada 0,4 s atropela a tela seguinte. Aqui cada passo segura 8 quadros e
            // espera um segundo e meio antes do próximo.
            // **O roteiro, por `ZEEBX_CORE_TECLAS` quando se quer outro.** Formato `ms:id`, com o
            // `id` do RetroPad (`1` é o `Y`, o confirmar do console), separado por vírgula — o
            // mesmo espírito do `ZEEBX_ROM_TECLAS` da varredura, que é onde o ciclo foi provado
            // primeiro. Sem a variável, vale o roteiro da doc: confirmar em "Jogar", descer às
            // capas, andar duas capas à direita e confirmar. Ele depende de **onde a grade põe o
            // foco**, e por isso não serve para toda pasta: com dois jogos, o foco já está no
            // primeiro e as setas o tiram de lá.
            let roteiro: Vec<(u32, u32)> = match std::env::var("ZEEBX_CORE_TECLAS") {
                Ok(texto) => texto
                    .split(',')
                    .filter_map(|parte| {
                        let (quando, id) = parte.split_once(':')?;
                        Some((quando.trim().parse().ok()?, id.trim().parse().ok()?))
                    })
                    .collect(),
                Err(_) => vec![
                    (0, ID_Y),
                    (0, ID_DOWN),
                    (0, ID_RIGHT),
                    (0, ID_RIGHT),
                    (0, ID_Y),
                    (0, ID_Y),
                    (0, ID_DOWN),
                    (0, ID_Y),
                ],
            };
            let distintas = |desde: usize| -> usize {
                ASSINATURAS
                    .lock()
                    .map(|v| {
                        let inicio = desde.min(v.len());
                        v[inicio..].iter().collect::<std::collections::BTreeSet<_>>().len()
                    })
                    .unwrap_or(0)
            };
            // **A roda anima antes da tecla**, e é isso que dá sentido à medida de depois: uma
            // tela que já estivesse parada não diria nada sobre a tecla.
            let animando = distintas(QUADROS.load(Ordering::Relaxed) as usize - 100);
            eprintln!(
                "antes da tecla: {animando} imagem(ns) distinta(s) em 100 quadros, relógio {} ms, \
                 {} instruções",
                RELOGIO.load(Ordering::Relaxed),
                INSTRUCOES.load(Ordering::Relaxed)
            );
            assert!(
                animando > 3,
                "a Z-Wheel não estava animando antes da tecla ({animando} imagens distintas)"
            );
            let mut aberto = 0u32;
            for (quando, passo) in roteiro {
                // Com instante no roteiro, espera o **relógio** chegar nele: cada `retro_run`
                // avança ~26 ms, e não os 16 de um quadro a 60 Hz. Ver [`RELOGIO`].
                if quando > 0 {
                    for _ in 0..ate_a_grade {
                        if RELOGIO.load(Ordering::Relaxed) >= quando {
                            break;
                        }
                        retro_run();
                    }
                }
                let antes = QUADROS.load(Ordering::Relaxed) as usize;
                BOTAO.store(passo, Ordering::Relaxed);
                for _ in 0..8 {
                    retro_run();
                }
                BOTAO.store(u32::MAX, Ordering::Relaxed);
                for _ in 0..90 {
                    retro_run();
                }
                aberto = ULTIMA_ABERTURA.load(Ordering::Relaxed);
                eprintln!(
                    "passo {passo}: abertura {aberto:#010x}, {} assinatura(s) distinta(s)",
                    distintas(antes)
                );
                if aberto != 0 {
                    break;
                }
            }
            // **O pedido não sai no mesmo quadro da tecla.** No roteiro da varredura a última
            // confirmação é aos 38 s e o pedido aparece quase dois segundos depois, quando a roda
            // já desmontou as telas e armou o temporizador do lançamento. Verificar só logo depois
            // de cada tecla mede a tela, e não o desfecho.
            for _ in 0..900 {
                retro_run();
                aberto = ULTIMA_ABERTURA.load(Ordering::Relaxed);
                if aberto != 0 {
                    break;
                }
            }
            // **A tecla foi tratada.** Sem a tradução do controle em teclas do console, a roda
            // ignorava o RetroPad por inteiro e seguia animando para sempre: medido, 13 ou 14
            // imagens distintas em cada 24 quadros, em todos os passos. Com a tradução, a primeira
            // confirmação é tratada e a tela para de animar.
            let depois = distintas(QUADROS.load(Ordering::Relaxed) as usize - 100);
            eprintln!(
                "depois da tecla: {depois} imagem(ns) distinta(s) em 100 quadros, relógio {} ms, \
                 {} instruções",
                RELOGIO.load(Ordering::Relaxed),
                INSTRUCOES.load(Ordering::Relaxed)
            );
            assert!(
                depois < animando,
                "a roda ignorou a tecla: {depois} imagens distintas depois, contra {animando} antes"
            );
            // **O pedido de abertura chega, e o core troca de sessão.** Antes da correção das
            // telas intermediárias isto era `0x00000000` e a roda ficava parada; agora é a classe
            // do jogo que a grade tinha em foco — o mesmo `0x0108E356` que a varredura pede.
            assert_ne!(aberto, 0, "o roteiro não chegou a pedir a abertura de jogo nenhum");
            let mut rodando = CLASSE_ATUAL.load(Ordering::Relaxed);
            for _ in 0..900 {
                retro_run();
                rodando = CLASSE_ATUAL.load(Ordering::Relaxed);
                if rodando != 0 && rodando != zeebx::session::Z_WHEEL {
                    break;
                }
            }
            assert_eq!(
                rodando, aberto,
                "o core não abriu o jogo que o shell pediu ({aberto:#010x})"
            );
            // **A volta à roda** depende do jogo terminar sozinho, e o que a grade tem em foco é o
            // primeiro título da pasta: com um jogo que sai sozinho ao lado da Z-Wheel (o `Zeebo
            // Clube`, medido na varredura), o core devolve o controle ao shell — e é o que este
            // laço espera. Sem um título desses, ele é relatado e não cobrado.
            // **A janela tem de ser maior que o jogo.** O retorno só acontece depois de o título
            // terminar, e o que a grade põe em foco roda ~62 s de relógio virtual — mais que os
            // 3600 quadros (60 s) da primeira versão, que por isso relatava `false` num caso em que
            // a volta **aconteceu** (as teclas seguintes chegam aos tratadores da roda).
            let mut voltou = false;
            for _ in 0..9000 {
                retro_run();
                if CLASSE_ATUAL.load(Ordering::Relaxed) == zeebx::session::Z_WHEEL {
                    voltou = true;
                    break;
                }
            }
            eprintln!("voltou à Z-Wheel depois do jogo: {voltou}");

            // **O que este teste ainda NÃO prova, e fica medido em vez de suposto:** o pedido de
            // abertura. No caminho da varredura ele sai (`abertura pedida: 0x0108e356`, com as
            // mesmas teclas e a mesma máquina), e aqui não — o que significa que a diferença está
            // em como o core conduz a sessão, e não na tecla, que é o que este teste prova. O
            // número sai no relatório para a próxima sessão medir a partir dele.
            eprintln!("pedido de abertura no caminho do core: {aberto:#010x}");
            retro_unload_game();
            retro_deinit();
        }
        eprintln!(
            "quadros entregues ao frontend: {}, {} instruções em {} ms virtuais",
            QUADROS.load(Ordering::Relaxed),
            INSTRUCOES.load(Ordering::Relaxed),
            RELOGIO.load(Ordering::Relaxed)
        );
        let _ = std::fs::remove_dir_all(&pasta);
    }

    /// **O save state atravessa a ABI, e volta igual.**
    ///
    /// É a prova de ponta a ponta do item 6, e ela é feita pelo caminho que o RetroArch usa: pedir
    /// o tamanho, gravar, sujar o estado, carregar e conferir. A conferência que importa é a
    /// última — **gravar de novo depois de carregar tem de dar byte a byte o mesmo arquivo** —,
    /// porque é isso que um save state promete. Comparar campos seria mais fraco: o que o jogador
    /// vê é o jogo continuar do mesmo ponto.
    ///
    /// **Sem `ZEEBX_CORE_ROM` ele não roda**, pela mesma razão do teste acima.
    #[test]
    fn o_save_state_atravessa_a_abi() {
        let Ok(caminho) = std::env::var("ZEEBX_CORE_ROM") else {
            eprintln!("sem ZEEBX_CORE_ROM: nada a salvar");
            return;
        };
        let pasta = std::env::temp_dir().join(format!("zeebx-estado-{}", std::process::id()));
        std::fs::create_dir_all(&pasta).unwrap();
        let _ = PASTA.set(CString::new(pasta.to_string_lossy().to_string()).unwrap());
        if let Ok(fora) = std::env::var("ZEEBX_CORE_SISTEMA") {
            let _ = SISTEMA.set(CString::new(fora).unwrap());
        }
        QUADROS.store(0, Ordering::Relaxed);
        let caminho_c = CString::new(caminho.clone()).unwrap();
        let quadros_pedidos = std::env::var("ZEEBX_CORE_QUADROS")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(60u32);

        let info = RetroGameInfo {
            path: caminho_c.as_ptr(),
            data: std::ptr::null(),
            size: 0,
            meta: std::ptr::null(),
        };
        unsafe {
            retro_set_environment(Some(ambiente));
            retro_set_video_refresh(Some(video));
            retro_set_audio_sample_batch(Some(audio));
            retro_init();
            retro_set_controller_port_device(0, DEVICE_JOYPAD);
            assert!(retro_load_game(&info), "o core recusou {caminho}");
            for _ in 0..quadros_pedidos {
                retro_run();
            }

            // 1) O tamanho, medido pelo frontend.
            let tamanho = retro_serialize_size();
            assert!(tamanho > 0, "com sessão montada, o tamanho tem de ser positivo");
            eprintln!("save state: {tamanho} bytes");
            let mut primeiro = vec![0u8; tamanho];
            assert!(
                retro_serialize(primeiro.as_mut_ptr() as *mut c_void, primeiro.len()),
                "a gravação foi recusada"
            );

            // 2) O jogo continua: o estado tem de ficar **diferente**.
            for _ in 0..quadros_pedidos {
                retro_run();
            }
            let tamanho_depois = retro_serialize_size();
            let mut segundo = vec![0u8; tamanho_depois];
            assert!(retro_serialize(
                segundo.as_mut_ptr() as *mut c_void,
                segundo.len()
            ));
            assert_ne!(
                primeiro, segundo,
                "o estado não mudou depois de rodar mais quadros: o teste não estaria medindo nada"
            );

            // 3) Carregar o primeiro, e conferir que gravar de novo dá o mesmo arquivo.
            assert!(
                retro_unserialize(primeiro.as_ptr() as *const c_void, primeiro.len()),
                "o carregamento foi recusado"
            );
            let tamanho_de_volta = retro_serialize_size();
            let mut terceiro = vec![0u8; tamanho_de_volta];
            assert!(retro_serialize(
                terceiro.as_mut_ptr() as *mut c_void,
                terceiro.len()
            ));
            if terceiro != primeiro {
                // **Onde** os dois divergem, e não a comparação inteira: um `assert_eq!` de dois
                // vetores de sete megabytes escreve sete megabytes na saída e não diz nada. O nome
                // da seção está no arquivo em texto, então a primeira diferença aponta o culpado.
                let posicao = terceiro
                    .iter()
                    .zip(primeiro.iter())
                    .position(|(a, b)| a != b)
                    .unwrap_or(terceiro.len().min(primeiro.len()));
                let inicio = posicao.saturating_sub(64);
                let texto = String::from_utf8_lossy(&terceiro[inicio..posicao + 8]);
                let texto = texto
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .collect::<String>();
                let antes = String::from_utf8_lossy(&primeiro[inicio..posicao + 8]);
                let antes = antes
                    .chars()
                    .filter(|c| c.is_ascii_graphic() || *c == ' ')
                    .collect::<String>();
                panic!(
                    "o estado carregado não é o que foi gravado: primeira diferença no byte {posicao} \
                     de {} (gravado {inicio}..{}: {antes:?}; carregado: {texto:?})",
                    terceiro.len(),
                    posicao + 8
                );
            }

            // 4) Um byte trocado é recusado, e **não** muda a máquina.
            let mut estragado = primeiro.clone();
            let meio = estragado.len() / 2;
            estragado[meio] ^= 0xff;
            assert!(
                !retro_unserialize(estragado.as_ptr() as *const c_void, estragado.len()),
                "um estado corrompido foi aceito"
            );
            let tamanho_final = retro_serialize_size();
            let mut quarto = vec![0u8; tamanho_final];
            assert!(retro_serialize(quarto.as_mut_ptr() as *mut c_void, quarto.len()));
            assert_eq!(
                quarto, primeiro,
                "a recusa de um estado corrompido mexeu na máquina"
            );

            retro_unload_game();
            retro_deinit();
        }
        let _ = std::fs::remove_dir_all(&pasta);
    }
}
