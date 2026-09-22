//! Núcleo Libretro: o mesmo [`Session`] da interface, no laço que o frontend manda.
//!
//! O RetroArch chama [`retro_run`] a 60 Hz. Cada volta lê o controle, anda o jogo até um quadro
//! (ou o orçamento de tempo real) e devolve pixels RGB565 640×480 — o framebuffer nativo do
//! Zeebo — e 735 quadros de áudio a 44100 Hz. O som não passa pelo `cpal`: o mixer silencioso
//! já existe para gravar, e aqui ele alimenta o callback em lote.

use std::cell::RefCell;
use std::ffi::{CStr, c_char, c_uint, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::audio::Mixer;
use crate::input::bindings::Aparelho;
use crate::input::{self, Pad};
use crate::session::Session;

const LARGURA: u32 = 640;
const ALTURA: u32 = 480;
const FPS: f64 = 60.0;
const TAXA_AUDIO: u32 = 44_100;
const QUADROS_DE_AUDIO: usize = (TAXA_AUDIO as f64 / FPS) as usize;
const FATIA: Duration = Duration::from_millis(16);

const RETRO_API_VERSION: c_uint = 1;
const RETRO_DEVICE_JOYPAD: c_uint = 1;
const RETRO_DEVICE_ANALOG: c_uint = 5;
const RETRO_DEVICE_INDEX_ANALOG_LEFT: c_uint = 0;
const RETRO_DEVICE_INDEX_ANALOG_RIGHT: c_uint = 1;
const RETRO_DEVICE_ID_ANALOG_X: c_uint = 0;
const RETRO_DEVICE_ID_ANALOG_Y: c_uint = 1;
const RETRO_DEVICE_ID_JOYPAD_MASK: c_uint = 256;

const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
const RETRO_ENVIRONMENT_GET_INPUT_BITMASKS: c_uint = 51;
const RETRO_PIXEL_FORMAT_XRGB8888: c_uint = 1;
const RETRO_PIXEL_FORMAT_RGB565: c_uint = 2;

type EnvironCb = Option<unsafe extern "C" fn(c_uint, *mut c_void) -> bool>;
type VideoCb = Option<unsafe extern "C" fn(*const c_void, c_uint, c_uint, usize)>;
type AudioCb = Option<unsafe extern "C" fn(i16, i16)>;
type AudioBatchCb = Option<unsafe extern "C" fn(*const i16, usize) -> usize>;
type InputPollCb = Option<unsafe extern "C" fn()>;
type InputStateCb = Option<unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16>;

#[repr(C)]
struct RetroSystemInfo {
    library_name: *const c_char,
    library_version: *const c_char,
    valid_extensions: *const c_char,
    need_fullpath: bool,
    block_extract: bool,
}

#[repr(C)]
struct RetroGameGeometry {
    base_width: c_uint,
    base_height: c_uint,
    max_width: c_uint,
    max_height: c_uint,
    aspect_ratio: f32,
}

#[repr(C)]
struct RetroSystemTiming {
    fps: f64,
    sample_rate: f64,
}

#[repr(C)]
struct RetroSystemAvInfo {
    geometry: RetroGameGeometry,
    timing: RetroSystemTiming,
}

#[repr(C)]
struct RetroGameInfo {
    path: *const c_char,
    data: *const c_void,
    size: usize,
    meta: *const c_char,
}

#[derive(Clone, Copy)]
enum PixelFormat {
    Rgb565,
    Xrgb8888,
}

#[derive(Clone, Copy)]
struct Host {
    environ: EnvironCb,
    video: VideoCb,
    audio: AudioCb,
    audio_batch: AudioBatchCb,
    input_poll: InputPollCb,
    input_state: InputStateCb,
    bitmasks: bool,
    formato: PixelFormat,
}

struct Core {
    session: Session,
    mixer: Mixer,
    path: PathBuf,
    pads: [Pad; input::PORTAS],
    xrgb: Vec<u32>,
    pcm: Vec<i16>,
}

thread_local! {
    static HOST: RefCell<Host> = const {
        RefCell::new(Host {
            environ: None,
            video: None,
            audio: None,
            audio_batch: None,
            input_poll: None,
            input_state: None,
            bitmasks: false,
            formato: PixelFormat::Rgb565,
        })
    };
    static CORE: RefCell<Option<Core>> = const { RefCell::new(None) };
}

/// Botão do Z-Pad para cada id do joypad Libretro, na ordem dos ids 0..=15.
const JOYPAD_PARA_BOTAO: [Option<&str>; 16] = [
    Some("b1"),     // B — sul, o botão 1 do Zeebo
    Some("b3"),     // Y — oeste
    Some("back"),   // Select — HOME
    Some("start"),  // Start
    Some("up"),
    Some("down"),
    Some("left"),
    Some("right"),
    Some("b2"),     // A — leste, o botão 2
    Some("b4"),     // X — norte
    Some("zl"),     // L
    Some("zr"),     // R
    Some("l2"),
    Some("r2"),
    Some("lthumb"),
    Some("rthumb"),
];

fn com_host<R>(f: impl FnOnce(&mut Host) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

fn com_core<R>(f: impl FnOnce(&mut Option<Core>) -> R) -> R {
    CORE.with(|c| f(&mut c.borrow_mut()))
}

fn abre(path: &Path) -> Result<Core, String> {
    let mut session = Session::start(path).map_err(|e| e.to_string())?;
    session.set_portas([Some(Aparelho::Controle), Some(Aparelho::Controle)]);
    let mixer = session.grava_audio(TAXA_AUDIO);
    Ok(Core {
        session,
        mixer,
        path: path.to_path_buf(),
        pads: [Pad::default(); input::PORTAS],
        xrgb: Vec::new(),
        pcm: vec![0; QUADROS_DE_AUDIO * 2],
    })
}

fn le_caminho(info: *const RetroGameInfo) -> Option<PathBuf> {
    if info.is_null() {
        return None;
    }
    let path = unsafe { (*info).path };
    if path.is_null() {
        return None;
    }
    let texto = unsafe { CStr::from_ptr(path) }.to_str().ok()?;
    if texto.is_empty() {
        return None;
    }
    Some(PathBuf::from(texto))
}

fn aplica_entrada(core: &mut Core, host: &Host) {
    let Some(state) = host.input_state else {
        return;
    };
    for porta in 0..input::PORTAS {
        let mut pad = Pad::default();
        let mask = if host.bitmasks {
            let bits = unsafe {
                state(
                    porta as c_uint,
                    RETRO_DEVICE_JOYPAD,
                    0,
                    RETRO_DEVICE_ID_JOYPAD_MASK,
                )
            };
            bits as u32
        } else {
            0
        };
        for id in 0..16u32 {
            let down = if host.bitmasks {
                mask & (1 << id) != 0
            } else {
                let valor = unsafe { state(porta as c_uint, RETRO_DEVICE_JOYPAD, 0, id) };
                valor != 0
            };
            if down {
                if let Some(Some(nome)) = JOYPAD_PARA_BOTAO.get(id as usize) {
                    if let Some(indice) = Pad::button_by_name(nome) {
                        pad.press(indice, true);
                    }
                }
            }
        }
        let eixo = |index: c_uint, id: c_uint, inverter: bool| -> i32 {
            let cru = unsafe { state(porta as c_uint, RETRO_DEVICE_ANALOG, index, id) };
            let mut n = cru as f32 / 32767.0;
            if inverter {
                n = -n;
            }
            (n.clamp(-1.0, 1.0) * input::AXIS_CURSO as f32) as i32
        };
        // No console, cima no analógico é valor negativo. No Libretro, Y positivo é baixo:
        // os sinais já coincidem, então o Y não inverte. O X segue o manche.
        pad.set_axis(0, eixo(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_X, false));
        pad.set_axis(1, eixo(RETRO_DEVICE_INDEX_ANALOG_LEFT, RETRO_DEVICE_ID_ANALOG_Y, false));
        pad.set_axis(2, eixo(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_X, false));
        pad.set_axis(3, eixo(RETRO_DEVICE_INDEX_ANALOG_RIGHT, RETRO_DEVICE_ID_ANALOG_Y, false));

        for (avk, apertada) in crate::ui::App::teclas_do_controle(&core.pads[porta], &pad) {
            core.session.set_key(avk, apertada);
        }
        core.session.set_port_pad(porta, pad);
        core.pads[porta] = pad;
    }
}

fn mostra_quadro(core: &mut Core, host: &Host) {
    let Some(video) = host.video else {
        return;
    };
    let tela = core.session.screen();
    let largura = tela.width();
    let altura = tela.height();
    match host.formato {
        PixelFormat::Rgb565 => {
            let pixels = tela.pixels();
            unsafe {
                video(
                    pixels.as_ptr().cast(),
                    largura,
                    altura,
                    (largura as usize) * 2,
                );
            }
        }
        PixelFormat::Xrgb8888 => {
            core.xrgb.clear();
            core.xrgb.extend(tela.pixels().iter().map(|&p| {
                let c = crate::video::display::Rgb::from_rgb565(p);
                (u32::from(c.r) << 16) | (u32::from(c.g) << 8) | u32::from(c.b)
            }));
            unsafe {
                video(
                    core.xrgb.as_ptr().cast(),
                    largura,
                    altura,
                    (largura as usize) * 4,
                );
            }
        }
    }
}

fn mostra_audio(core: &mut Core, host: &Host) {
    let amostras = core.mixer.render(QUADROS_DE_AUDIO);
    core.pcm.clear();
    core.pcm.extend(amostras.iter().map(|s| {
        (s.clamp(-1.0, 1.0) * 32767.0) as i16
    }));
    if let Some(batch) = host.audio_batch {
        unsafe {
            batch(core.pcm.as_ptr(), QUADROS_DE_AUDIO);
        }
        return;
    }
    if let Some(sample) = host.audio {
        for par in core.pcm.chunks_exact(2) {
            unsafe {
                sample(par[0], par[1]);
            }
        }
    }
}

fn pede_formato(environ: EnvironCb) -> PixelFormat {
    let Some(environ) = environ else {
        return PixelFormat::Rgb565;
    };
    let mut fmt = RETRO_PIXEL_FORMAT_RGB565;
    if unsafe { environ(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, (&mut fmt as *mut c_uint).cast()) } {
        return PixelFormat::Rgb565;
    }
    fmt = RETRO_PIXEL_FORMAT_XRGB8888;
    if unsafe { environ(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, (&mut fmt as *mut c_uint).cast()) } {
        return PixelFormat::Xrgb8888;
    }
    PixelFormat::Rgb565
}

fn aceita_bitmasks(environ: EnvironCb) -> bool {
    let Some(environ) = environ else {
        return false;
    };
    unsafe { environ(RETRO_ENVIRONMENT_GET_INPUT_BITMASKS, std::ptr::null_mut()) }
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_environment(cb: EnvironCb) {
    com_host(|host| {
        host.environ = cb;
        host.bitmasks = aceita_bitmasks(cb);
    });
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_video_refresh(cb: VideoCb) {
    com_host(|host| host.video = cb);
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_audio_sample(cb: AudioCb) {
    com_host(|host| host.audio = cb);
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_audio_sample_batch(cb: AudioBatchCb) {
    com_host(|host| host.audio_batch = cb);
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_input_poll(cb: InputPollCb) {
    com_host(|host| host.input_poll = cb);
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_input_state(cb: InputStateCb) {
    com_host(|host| host.input_state = cb);
}

#[unsafe(no_mangle)]
extern "C" fn retro_init() {}

#[unsafe(no_mangle)]
extern "C" fn retro_deinit() {
    com_core(|core| *core = None);
}

#[unsafe(no_mangle)]
extern "C" fn retro_api_version() -> c_uint {
    RETRO_API_VERSION
}

#[unsafe(no_mangle)]
extern "C" fn retro_get_system_info(info: *mut RetroSystemInfo) {
    if info.is_null() {
        return;
    }
    unsafe {
        *info = RetroSystemInfo {
            library_name: c"Zeebx".as_ptr(),
            library_version: CStr::from_bytes_with_nul(
                concat!(env!("CARGO_PKG_VERSION"), "\0").as_bytes(),
            )
            .expect("versão do pacote")
            .as_ptr(),
            valid_extensions: c"mod|zip".as_ptr(),
            need_fullpath: true,
            block_extract: true,
        };
    }
}

#[unsafe(no_mangle)]
extern "C" fn retro_get_system_av_info(info: *mut RetroSystemAvInfo) {
    if info.is_null() {
        return;
    }
    unsafe {
        *info = RetroSystemAvInfo {
            geometry: RetroGameGeometry {
                base_width: LARGURA,
                base_height: ALTURA,
                max_width: LARGURA,
                max_height: ALTURA,
                aspect_ratio: 4.0 / 3.0,
            },
            timing: RetroSystemTiming {
                fps: FPS,
                sample_rate: f64::from(TAXA_AUDIO),
            },
        };
    }
}

#[unsafe(no_mangle)]
extern "C" fn retro_set_controller_port_device(_port: c_uint, _device: c_uint) {}

#[unsafe(no_mangle)]
extern "C" fn retro_reset() {
    let caminho = com_core(|core| core.as_ref().map(|c| c.path.clone()));
    let Some(path) = caminho else {
        return;
    };
    match abre(&path) {
        Ok(novo) => com_core(|core| *core = Some(novo)),
        Err(erro) => eprintln!("zeebx: reset falhou: {erro}"),
    }
}

#[unsafe(no_mangle)]
extern "C" fn retro_run() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let host = com_host(|h| *h);
        if let Some(poll) = host.input_poll {
            unsafe {
                poll();
            }
        }
        com_core(|guarda| {
            let Some(emu) = guarda.as_mut() else {
                return;
            };
            aplica_entrada(emu, &host);
            if !emu.session.mostra_quadro_intermediario() {
                let _ = emu.session.step(FATIA, false);
            }
            mostra_quadro(emu, &host);
            mostra_audio(emu, &host);
        });
    }));
}

#[unsafe(no_mangle)]
extern "C" fn retro_serialize_size() -> usize {
    0
}

#[unsafe(no_mangle)]
extern "C" fn retro_serialize(_data: *mut c_void, _size: usize) -> bool {
    false
}

#[unsafe(no_mangle)]
extern "C" fn retro_unserialize(_data: *const c_void, _size: usize) -> bool {
    false
}

#[unsafe(no_mangle)]
extern "C" fn retro_cheat_reset() {}

#[unsafe(no_mangle)]
extern "C" fn retro_cheat_set(_index: c_uint, _enabled: bool, _code: *const c_char) {}

#[unsafe(no_mangle)]
extern "C" fn retro_load_game(game: *const RetroGameInfo) -> bool {
    let Some(path) = le_caminho(game) else {
        eprintln!("zeebx: o núcleo precisa do caminho do .mod ou .zip");
        return false;
    };
    com_host(|host| {
        host.formato = pede_formato(host.environ);
        host.bitmasks = aceita_bitmasks(host.environ);
    });
    match abre(&path) {
        Ok(novo) => {
            com_core(|core| *core = Some(novo));
            true
        }
        Err(erro) => {
            eprintln!("zeebx: não carregou {}: {erro}", path.display());
            false
        }
    }
}

#[unsafe(no_mangle)]
extern "C" fn retro_load_game_special(
    _tipo: c_uint,
    _info: *const RetroGameInfo,
    _num: usize,
) -> bool {
    false
}

#[unsafe(no_mangle)]
extern "C" fn retro_unload_game() {
    com_core(|core| *core = None);
}

#[unsafe(no_mangle)]
extern "C" fn retro_get_region() -> c_uint {
    0
}

#[unsafe(no_mangle)]
extern "C" fn retro_get_memory_data(_id: c_uint) -> *mut c_void {
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
extern "C" fn retro_get_memory_size(_id: c_uint) -> usize {
    0
}
