//! O aparelho: framebuffer, controle, alto-falante e a pasta `ux0:`.
//!
//! O preenchimento vai para a placa quando o vitaGL sobe. O vitaGL traduz OpenGL ES 2, e o
//! shader do núcleo tem essa versão. Se ele não sobe, o quadro volta a ser o RGB565 pintado no
//! framebuffer do kernel. O dynarmic não emite ARMv7 — o núcleo aqui é o interpretador.

#[path = "placa.rs"]
mod placa;

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use zeebx::audio::MidiBackend;
use zeebx::config::ZWheel;
use zeebx::input::{self, Pad};
use zeebx::library::{self, Game};
use zeebx::session::{self, FATIA_MAXIMA, Session, Step};
use zeebx::storage::StoragePaths;
use zeebx::velocidade::{Frameskip, Ritmo};

use crate::apresentacao::{self, Quadro};
use crate::entrada::{self, Leitura};
use crate::lista;
use crate::som::{self, Anel};

const RAIZ: &str = "ux0:data/zeebx";
const ROMS: &str = "ux0:data/zeebx/roms";
const LOG: &str = "ux0:data/zeebx/zeebx.log";
const LOG_C: &[u8] = b"ux0:data/zeebx/zeebx.log\0";

const LARGURA: usize = 960;
const ALTURA: usize = 544;
const PASSO: usize = 960;
/// Um quadro de 960×544 em 32 bits cabe em 2 MiB. O `PHYCONT` sem cache só existe em múltiplos
/// de 1 MiB; a CDRAM, em múltiplos de 256 KiB. 2 MiB serve para os dois.
const ALINHAMENTO: u32 = 1024 * 1024;

/// Scanout fisicamente contínuo e sem cache. A CPU não pinta aqui: cada store sem cache
/// atravessa o barramento. O desenho vai para `Tela::composto`, com cache, e o `sceDmacMemcpy`
/// entrega.
const MEMORIA_DA_TELA: u32 = 0x0D80_8060;
/// Onde o quadro da Vita costuma morar. Neste aparelho há 112 MiB livres.
const CDRAM_RW: u32 = 0x0940_8060;

/// 1×, e o pulo automático de desenho quando a sessão mede atraso.
///
/// Sem o pulo, um quadro que passa de 16 ms atrasa o jogo na mesma medida: a volta desenha
/// todos. O automático deixa de preencher o quadro que não vai à tela, desde que ainda caiba
/// um desenhado na fatia — é a conta que a sessão já faz. A meia resolução do perfil Portátil
/// fica de fora: mediu 22% naquele outro aparelho e deixa o 3D quadrado.
const RITMO: Ritmo = Ritmo {
    proporcao: Some(1.0),
    frameskip: Frameskip::Automatico,
    meia_apresentacao: false,
    abafa_o_som: false,
};

/// O newlib lê este símbolo para dimensionar o `malloc`. O bloco é pedido uma vez e não cresce.
/// 192 MiB cabem no modo estendido que o `ATTRIBUTE2=12` do `param.sfo` pede (365 MiB de MAIN).
///
/// O que passa de 1 MiB não mora aqui: o heap do guest na Vita tem 48 MiB, e o Double Dragon
/// morreu com `sem memoria` com este número. Esses blocos saem por [`Grandes`].
///
/// Nada neste binário lê o valor: o `#[used]` impede o LLVM de apagar o símbolo, e o `-Wl,-u` do
/// `build.rs` impede o `--gc-sections` de apagá-lo de novo. `#[used(linker)]` faria os dois, e
/// no rustc 1.99 ainda é instável.
#[used]
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static _newlib_heap_size_user: u32 = 192 * 1024 * 1024;

#[repr(C)]
#[allow(dead_code)]
struct SceKernelAllocMemBlockOpt {
    size: u32,
    attr: u32,
    alignment: u32,
    uid_base: u32,
    nome: *const c_char,
    flags: i32,
    reserved: [i32; 10],
}

#[repr(C)]
struct SceDisplayFrameBuf {
    size: u32,
    base: *mut c_void,
    pitch: u32,
    pixelformat: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct SceCtrlData {
    time_stamp: u64,
    buttons: u32,
    lx: u8,
    ly: u8,
    rx: u8,
    ry: u8,
    resto: [u8; 16],
}

unsafe extern "C" {
    fn sceCtrlSetSamplingMode(mode: i32) -> i32;
    fn sceCtrlPeekBufferPositive(port: i32, pad: *mut SceCtrlData, count: i32) -> i32;
    fn sceCtrlReadBufferPositive(port: i32, pad: *mut SceCtrlData, count: i32) -> i32;
    fn sceDisplaySetFrameBuf(fb: *const SceDisplayFrameBuf, sync: i32) -> i32;
    fn sceDisplayWaitVblankStart() -> i32;
    fn sceAudioOutOpenPort(tipo: i32, len: i32, freq: i32, mode: i32) -> i32;
    fn sceAudioOutOutput(port: i32, buf: *const c_void) -> i32;
    fn sceAudioOutSetVolume(port: i32, ch: i32, vol: *mut i32) -> i32;
    fn sceKernelAllocMemBlock(
        name: *const c_char,
        tipo: u32,
        size: u32,
        opt: *mut SceKernelAllocMemBlockOpt,
    ) -> i32;
    fn sceKernelGetMemBlockBase(uid: i32, base: *mut *mut c_void) -> i32;
    fn sceKernelFreeMemBlock(uid: i32) -> i32;
    fn sceKernelGetFreeMemorySize(info: *mut Livre) -> i32;
    fn sceKernelExitProcess(res: i32) -> i32;
    fn sceDmacMemcpy(dst: *mut c_void, src: *const c_void, size: u32) -> i32;
    fn scePowerGetArmClockFrequency() -> i32;
    fn scePowerGetBusClockFrequency() -> i32;
    fn scePowerSetArmClockFrequency(freq: i32) -> i32;
    fn scePowerSetBusClockFrequency(freq: i32) -> i32;
    fn sceKernelGetThreadId() -> i32;
    fn sceKernelChangeThreadPriority(thid: i32, priority: i32) -> i32;
}

/// O máximo que o sistema aceita. Sem isto o processo fica no clock em que o shell o deixou.
const CLOCK_ARM_MHZ: i32 = 444;
const CLOCK_BARRAMENTO_MHZ: i32 = 222;

/// Abaixo do padrão da thread principal, e igual nas duas threads: o jogo não passa na frente
/// do som. Número menor é mais urgente.
const PRIORIDADE: i32 = 64;

const _: () = assert!(std::mem::size_of::<SceDisplayFrameBuf>() == 0x18);
const _: () = assert!(std::mem::size_of::<SceCtrlData>() == 0x20);
const _: () = assert!(std::mem::size_of::<SceKernelAllocMemBlockOpt>() == 0x40);
const _: () = assert!(cfg!(target_pointer_width = "32"));

fn morre(codigo: i32) -> ! {
    unsafe { sceKernelExitProcess(codigo) };
    loop {}
}

fn sem_memoria(layout: std::alloc::Layout) {
    // Sem `std::fs`: o gancho roda porque uma alocação já falhou, e abrir o arquivo pelo Rust
    // alocaria de novo. O `open` do newlib, se não couber, devolve erro em vez de reentrar aqui.
    // O número é o tamanho que não coube, para o próximo log não dizer só "acabou".
    let mut msg = [0u8; 48];
    let prefixo = b"sem memoria ";
    msg[..prefixo.len()].copy_from_slice(prefixo);
    let mut n = layout.size();
    let mut digitos = [0u8; 20];
    let mut i = 0usize;
    if n == 0 {
        digitos[0] = b'0';
        i = 1;
    } else {
        while n > 0 && i < digitos.len() {
            digitos[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
    }
    let mut p = prefixo.len();
    while i > 0 {
        i -= 1;
        msg[p] = digitos[i];
        p += 1;
    }
    msg[p] = b'\n';
    p += 1;
    unsafe {
        let fd = open(LOG_C.as_ptr().cast(), 1 | 0x8 | 0x200, 0o644i32);
        if fd >= 0 {
            write(fd, msg.as_ptr().cast(), p);
            close(fd);
        }
    }
}

#[repr(C)]
struct Livre {
    size: i32,
    user: i32,
    cdram: i32,
    phycont: i32,
}

fn anota_livre() {
    let mut info = Livre {
        size: std::mem::size_of::<Livre>() as i32,
        user: 0,
        cdram: 0,
        phycont: 0,
    };
    let rc = unsafe { sceKernelGetFreeMemorySize(&mut info) };
    if rc < 0 {
        anota(&format!("livre recusado ({rc})"));
        return;
    }
    anota(&format!(
        "livre user {} cdram {} phycont {}",
        info.user, info.cdram, info.phycont
    ));
}

const BLOCOS: usize = 256;
const PAGINA: usize = 256 * 1024;
/// `SCE_KERNEL_MEMBLOCK_TYPE_USER_RW`. Cacheada: o interpretador lê isto o tempo todo.
/// O quadro na tela usa outro tipo, sem cache, porque o display lê por DMA.
const USER_RW: u32 = 0x0c20_d060;
const NOME_DO_BLOCO: &[u8] = b"zeebx\0";

#[derive(Clone, Copy)]
struct Vaga {
    ptr: usize,
    uid: i32,
}

/// Alocações de 1 MiB ou mais, fora do heap fixo do newlib.
///
/// O heap de 192 MiB é pedido inteiro na largada e não devolve o que sobra ao sistema. O mapa
/// do guest (48 MiB de heap, 16 da região interna, 8 de superfície, 4 de objetos) cabe nele e
/// deixa pouco contíguo para a textura e para o som. Cada bloco grande pede a sua memória ao
/// kernel; o `malloc` fica com o que é pequeno. Se o kernel recusar, cai no `malloc` como antes.
pub struct Grandes {
    trava: std::sync::atomic::AtomicBool,
    vagas: std::cell::UnsafeCell<[Vaga; BLOCOS]>,
}

unsafe impl Sync for Grandes {}

impl Grandes {
    pub const NOVO: Self = Self {
        trava: std::sync::atomic::AtomicBool::new(false),
        vagas: std::cell::UnsafeCell::new([Vaga { ptr: 0, uid: 0 }; BLOCOS]),
    };

    fn trava(&self) {
        while self.trava.swap(true, std::sync::atomic::Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }

    fn solta(&self) {
        self.trava
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn bloco(&self, tamanho: u32) -> Option<*mut u8> {
        let uid = unsafe {
            sceKernelAllocMemBlock(
                NOME_DO_BLOCO.as_ptr().cast(),
                USER_RW,
                tamanho,
                std::ptr::null_mut(),
            )
        };
        if uid < 0 {
            return None;
        }
        let mut base: *mut c_void = std::ptr::null_mut();
        if unsafe { sceKernelGetMemBlockBase(uid, &mut base) } < 0 || base.is_null() {
            unsafe { sceKernelFreeMemBlock(uid) };
            return None;
        }
        self.trava();
        let vagas = unsafe { &mut *self.vagas.get() };
        let vaga = vagas.iter_mut().find(|vaga| vaga.ptr == 0);
        let Some(vaga) = vaga else {
            self.solta();
            unsafe { sceKernelFreeMemBlock(uid) };
            return None;
        };
        vaga.ptr = base as usize;
        vaga.uid = uid;
        self.solta();
        Some(base.cast())
    }

    fn solta_bloco(&self, ptr: *mut u8) -> bool {
        self.trava();
        let vagas = unsafe { &mut *self.vagas.get() };
        let Some(vaga) = vagas.iter_mut().find(|vaga| vaga.ptr == ptr as usize) else {
            self.solta();
            return false;
        };
        let uid = vaga.uid;
        vaga.ptr = 0;
        vaga.uid = 0;
        self.solta();
        unsafe { sceKernelFreeMemBlock(uid) };
        true
    }
}

unsafe impl std::alloc::GlobalAlloc for Grandes {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.size() >= 1024 * 1024
            && layout.align() <= PAGINA
            && let Some(tamanho) = arredonda(layout.size())
            && let Some(ptr) = self.bloco(tamanho)
        {
            return ptr;
        }
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        if self.solta_bloco(ptr) {
            return;
        }
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

fn arredonda(n: usize) -> Option<u32> {
    let cheio = n.checked_add(PAGINA - 1)?;
    u32::try_from(cheio & !(PAGINA - 1)).ok()
}

unsafe extern "C" {
    fn open(path: *const c_char, flags: i32, ...) -> i32;
    fn write(fd: i32, buf: *const c_void, n: usize) -> isize;
    fn close(fd: i32) -> i32;
}

fn anota(texto: &str) {
    if let Ok(mut arquivo) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG)
    {
        use std::io::Write;
        let _ = writeln!(arquivo, "{texto}");
    }
}

fn despeja() {
    let linhas = zeebx::registro::drena();
    if linhas.is_empty() {
        return;
    }
    if let Ok(mut arquivo) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG)
    {
        use std::io::Write;
        for linha in linhas {
            let _ = writeln!(
                arquivo,
                "[{}] {}: {}",
                linha.nivel.etiqueta(),
                linha.alvo,
                linha.texto
            );
        }
    }
}

fn leitura_de(dados: &SceCtrlData) -> Leitura {
    Leitura {
        botoes: dados.buttons,
        lx: dados.lx,
        ly: dados.ly,
        rx: dados.rx,
        ry: dados.ry,
    }
}

fn le_bloqueante() -> Leitura {
    let mut dados = SceCtrlData {
        time_stamp: 0,
        buttons: 0,
        lx: 128,
        ly: 128,
        rx: 128,
        ry: 128,
        resto: [0; 16],
    };
    let n = unsafe { sceCtrlReadBufferPositive(0, &mut dados, 1) };
    if n < 1 {
        Leitura {
            lx: 128,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Leitura::default()
        }
    } else {
        leitura_de(&dados)
    }
}

fn le_agora() -> Leitura {
    let mut dados = SceCtrlData {
        time_stamp: 0,
        buttons: 0,
        lx: 128,
        ly: 128,
        rx: 128,
        ry: 128,
        resto: [0; 16],
    };
    let n = unsafe { sceCtrlPeekBufferPositive(0, &mut dados, 1) };
    if n < 1 {
        Leitura {
            lx: 128,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Leitura::default()
        }
    } else {
        leitura_de(&dados)
    }
}

struct Tela {
    base: *mut u32,
    /// O quadro que a CPU pinta. O início útil é alinhado a 16: o DMA pede isso, e o `Vec<u32>`
    /// só garante 4.
    composto: Vec<u32>,
    inicio: usize,
    indice: usize,
    salto: usize,
    largura: usize,
    passo: usize,
    altura: usize,
    ja_avisou: bool,
    ja_avisou_dma: bool,
    /// O vitaGL tomou a tela. O composto sobe como textura; não há scanout do kernel.
    na_placa: bool,
}

fn composto_alinhado(palavras: usize) -> (Vec<u32>, usize) {
    let composto = vec![apresentacao::PRETO; palavras + 3];
    let addr = composto.as_ptr() as usize;
    let falta = (16 - (addr & 15)) & 15;
    (composto, falta / 4)
}

fn pede_tela(tipo: u32, tamanho: u32) -> Result<i32, i32> {
    // Sem o `opt`. Pedir alinhamento na estrutura (`attr` 0x4) devolve
    // `0x80020005` no aparelho, com 256 KiB e com 1 MiB. O Vita3K aceita a
    // estrutura. O kernel já alinha o bloco pelo tipo quando o ponteiro é nulo.
    let uid = unsafe {
        sceKernelAllocMemBlock(c"zeebx-fb".as_ptr(), tipo, tamanho, std::ptr::null_mut())
    };
    if uid < 0 { Err(uid) } else { Ok(uid) }
}

impl Tela {
    fn abre(na_placa: bool) -> Result<Self, String> {
        unsafe { sceCtrlSetSamplingMode(1) };
        if na_placa {
            let (composto, inicio) = composto_alinhado(PASSO * ALTURA);
            let mut tela = Self {
                base: std::ptr::null_mut(),
                composto,
                inicio,
                indice: 0,
                salto: 0,
                largura: LARGURA,
                passo: PASSO,
                altura: ALTURA,
                ja_avisou: false,
                ja_avisou_dma: false,
                na_placa: true,
            };
            tela.apresenta(|q| apresentacao::limpa(q, apresentacao::PRETO));
            tela.apresenta(|q| apresentacao::limpa(q, apresentacao::PRETO));
            return Ok(tela);
        }
        let bytes = ((PASSO * ALTURA * 4) as u32).div_ceil(ALINHAMENTO) * ALINHAMENTO;
        let tamanho = bytes * 2;
        let uid = match pede_tela(MEMORIA_DA_TELA, tamanho) {
            Ok(uid) => uid,
            Err(phy) => match pede_tela(CDRAM_RW, tamanho) {
                Ok(uid) => {
                    anota(&format!("tela na cdram; phycont recusada ({phy})"));
                    uid
                }
                Err(cdram) => {
                    return Err(format!(
                        "não consegui a memória da tela (phycont {phy}, cdram {cdram})"
                    ));
                }
            },
        };
        let mut base: *mut c_void = std::ptr::null_mut();
        let rc = unsafe { sceKernelGetMemBlockBase(uid, &mut base) };
        if rc < 0 || base.is_null() {
            return Err(format!("a memória da tela não tem endereço ({rc})"));
        }
        let (composto, inicio) = composto_alinhado(PASSO * ALTURA);
        let mut tela = Self {
            base: base.cast(),
            composto,
            inicio,
            indice: 0,
            salto: bytes as usize,
            largura: LARGURA,
            passo: PASSO,
            altura: ALTURA,
            ja_avisou: false,
            ja_avisou_dma: false,
            na_placa: false,
        };
        tela.apresenta(|q| apresentacao::limpa(q, apresentacao::PRETO));
        tela.apresenta(|q| apresentacao::limpa(q, apresentacao::PRETO));
        Ok(tela)
    }

    fn apresenta(&mut self, desenha: impl FnOnce(&mut Quadro<'_>)) {
        let n = self.passo * self.altura;
        let inicio = self.inicio;
        {
            let pixels = &mut self.composto[inicio..inicio + n];
            let mut quadro = Quadro {
                pixels,
                largura: self.largura,
                passo: self.passo,
                altura: self.altura,
            };
            desenha(&mut quadro);
        }
        if self.na_placa {
            placa::mostra_rgba(
                &self.composto[inicio..inicio + n],
                self.largura as i32,
                self.altura as i32,
            );
            return;
        }
        let deslocamento = self.indice * (self.salto / 4);
        let dst = unsafe { self.base.add(deslocamento) };
        let src = unsafe { self.composto.as_ptr().add(inicio) };
        let bytes = (n * 4) as u32;
        let rc = unsafe { sceDmacMemcpy(dst.cast(), src.cast(), bytes) };
        if rc < 0 {
            if !self.ja_avisou_dma {
                self.ja_avisou_dma = true;
                anota(&format!("o DMA da tela falhou ({rc}); copiando pela CPU"));
            }
            unsafe { std::ptr::copy_nonoverlapping(src, dst, n) };
        }
        let fb = SceDisplayFrameBuf {
            size: 0x18,
            base: dst.cast(),
            pitch: self.passo as u32,
            pixelformat: 0,
            width: self.largura as u32,
            height: self.altura as u32,
        };
        let rc = unsafe { sceDisplaySetFrameBuf(&fb, 1) };
        if rc < 0 && !self.ja_avisou {
            self.ja_avisou = true;
            anota(&format!("a tela recusou o quadro: {rc}"));
        }
        unsafe { sceDisplayWaitVblankStart() };
        self.indice ^= 1;
    }
}

struct Som {
    anel: Arc<Anel>,
    porta: i32,
}

impl Som {
    fn abre() -> Self {
        let anel = Arc::new(Anel::novo(8_192));
        let porta = unsafe { sceAudioOutOpenPort(0, som::GRAO as i32, som::TAXA as i32, 1) };
        if porta < 0 {
            anota(&format!("sem som: a porta devolveu {porta}"));
            return Self { anel, porta: -1 };
        }
        let mut volume = [0x8000i32, 0x8000];
        unsafe { sceAudioOutSetVolume(porta, 3, volume.as_mut_ptr()) };
        let fio = Arc::clone(&anel);
        let criada = std::thread::Builder::new()
            .name("som".into())
            .stack_size(64 * 1024)
            .spawn(move || fio_de_som(porta, fio));
        if let Err(erro) = criada {
            anota(&format!("sem som: {erro}"));
            return Self { anel, porta: -1 };
        }
        Self { anel, porta }
    }
}

fn fio_de_som(porta: i32, anel: Arc<Anel>) {
    ajusta_prioridade();
    // Dois blocos: a porta lê o que acabou de receber até a chamada seguinte voltar. Encher o
    // mesmo bloco de novo, antes disso, troca o som que ainda está tocando.
    let mut blocos = [
        som::Bloco {
            amostras: [0; som::GRAO * 2],
        },
        som::Bloco {
            amostras: [0; som::GRAO * 2],
        },
    ];
    let mut qual = 0;
    loop {
        anel.puxa(&mut blocos[qual].amostras);
        let rc = unsafe { sceAudioOutOutput(porta, blocos[qual].amostras.as_ptr().cast()) };
        if rc < 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        qual ^= 1;
    }
}

struct Jogo {
    sessao: Session,
    mixer: zeebx::audio::Mixer,
    pad_antes: Pad,
    ultimo: Instant,
    relogio: u32,
    ultimo_som: Instant,
    mix: Vec<f32>,
    pcm: Vec<i16>,
    corda: Option<Instant>,
    pela_z_wheel: bool,
    primeiro: bool,
    /// O menu ainda está no quadro composto. A primeira apresentação do jogo limpa a faixa.
    faixa_suja: bool,
}

struct Aplicativo {
    jogos: Vec<Game>,
    instalados: Vec<(u32, String)>,
    storage: StoragePaths,
    cursor: usize,
    erro: String,
    jogo: Option<Jogo>,
    viu_quadro: bool,
    saidas_sem_quadro: u32,
}

impl Aplicativo {
    fn novo() -> Self {
        let storage =
            StoragePaths::for_frontend(Path::new("ux0:data"), Some(Path::new("ux0:data")));
        let mut erro = String::new();
        if let Err(falha) = storage.create_dirs() {
            erro = format!("não consegui criar as pastas: {falha}");
        }
        let _ = std::fs::create_dir_all(ROMS);
        Self {
            jogos: Vec::new(),
            instalados: Vec::new(),
            storage,
            cursor: 0,
            erro,
            jogo: None,
            viu_quadro: false,
            saidas_sem_quadro: 0,
        }
    }

    fn varre(&mut self) {
        self.jogos = library::scan(Path::new(ROMS));
        self.instalados = self
            .jogos
            .iter()
            .filter_map(|jogo| Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?)))
            .collect();
        if self.jogos.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.jogos.len() {
            self.cursor = self.jogos.len() - 1;
        }
    }

    fn caminho_da_classe(&self, classe: u32) -> Option<PathBuf> {
        self.jogos
            .iter()
            .find(|jogo| jogo.clsid == Some(classe))
            .map(|jogo| jogo.path.clone())
    }

    fn abre_caminho(
        &mut self,
        caminho: &Path,
        pela_z_wheel: bool,
        som: &Som,
    ) -> Result<(), String> {
        let instalados = self.instalados.clone();
        // Com o vitaGL no ar a lista aparece. O quadro do jogo, neste Vita3K,
        // não volta do framebuffer: desenhar na placa e só trocar os buffers
        // republica a lista. Sem a devolução, o jogo sai no processador.
        let guarda = placa::guarda_o_quadro();
        let contexto = guarda.then(placa::contexto).flatten();
        let na_placa = contexto.is_some();
        match Session::start_with_storage_installed_policy(
            caminho,
            zeebx::PORTAS_PADRAO,
            None,
            na_placa,
            contexto,
            ZWheel::default(),
            &self.storage,
            &instalados,
            MidiBackend::Timbres,
        ) {
            Ok(mut sessao) => {
                let relogio = sessao.clock_ms();
                let mixer = sessao.grava_audio(som::TAXA);
                som.anel.esvazia();
                self.viu_quadro = false;
                self.jogo = Some(Jogo {
                    sessao,
                    mixer,
                    pad_antes: Pad::default(),
                    ultimo: Instant::now(),
                    relogio,
                    ultimo_som: Instant::now(),
                    mix: Vec::new(),
                    pcm: Vec::new(),
                    corda: None,
                    pela_z_wheel,
                    primeiro: true,
                    faixa_suja: true,
                });
                anota(&format!("abriu {}", caminho.display()));
                Ok(())
            }
            Err(erro) => Err(erro.to_string()),
        }
    }

    fn abre_cursor(&mut self, som: &Som) -> Result<(), String> {
        let caminho = self
            .jogos
            .get(self.cursor)
            .ok_or("não há jogo escolhido")?
            .path
            .clone();
        self.abre_caminho(&caminho, false, som)
    }

    fn volta_ao_menu(&mut self, som: &Som, erro: String) {
        self.jogo = None;
        som.anel.esvazia();
        self.erro = erro;
        self.viu_quadro = false;
        // A contagem é de saídas seguidas, no meio da Z-Wheel reabrindo. Voltar à lista zera:
        // o próximo jogo que o jogador escolhe começa limpo.
        self.saidas_sem_quadro = 0;
    }

    fn menu(&mut self, tela: &mut Tela) {
        let mut antes = 0u32;
        let mut dir_antes = 0i32;
        let mut dir_desde = Instant::now();
        let mut dir_passo = Instant::now();
        let mut sujo = true;
        loop {
            despeja();
            if sujo {
                let titulos: Vec<&str> =
                    self.jogos.iter().map(|jogo| jogo.title.as_str()).collect();
                let erro = self.erro.clone();
                let cursor = self.cursor;
                tela.apresenta(|q| apresentacao::desenha_menu(q, &titulos, cursor, &erro));
                sujo = false;
            }
            let leitura = le_bloqueante();
            let acao = lista::toque(leitura.botoes, antes);
            let dir = lista::direcao(leitura.botoes);
            let borda = if dir != dir_antes { dir } else { 0 };
            if dir != dir_antes {
                dir_antes = dir;
                dir_desde = Instant::now();
                dir_passo = Instant::now();
            }
            let delta = lista::passo_da_lista(borda, dir, dir_desde.elapsed(), dir_passo.elapsed());
            if delta != 0 && borda == 0 {
                dir_passo = Instant::now();
            }
            antes = leitura.botoes;
            match acao {
                lista::Toque::Sai => morre(0),
                lista::Toque::Atualiza => {
                    tela.apresenta(|q| apresentacao::desenha_menu(q, &[], 0, "Lendo a pasta..."));
                    self.erro.clear();
                    self.varre();
                    sujo = true;
                }
                lista::Toque::Confirma if !self.jogos.is_empty() => return,
                _ => {}
            }
            if delta != 0 {
                self.cursor = lista::move_cursor(self.cursor, self.jogos.len(), delta);
                sujo = true;
            }
        }
    }

    fn quadro(&mut self, tela: &mut Tela, som: &Som) {
        let mut sair = false;
        let mut passo = Step::Running;
        let mut pedido = None;
        let mut normal = false;
        let mut motivo = String::new();
        let mut classe = 0u32;
        let mut pela = false;
        let mut apresentou = false;
        {
            let jogo = self.jogo.as_mut().expect("quadro sem jogo");
            let leitura = le_agora();
            if entrada::sai_do_jogo(leitura.botoes) {
                if jogo.corda.is_none() {
                    jogo.corda = Some(Instant::now());
                }
                if jogo
                    .corda
                    .is_some_and(|desde| entrada::tempo_de_saida(desde.elapsed()))
                {
                    sair = true;
                }
            } else {
                jogo.corda = None;
            }
            if !sair {
                let mut para_o_jogo = leitura;
                para_o_jogo.botoes = entrada::filtra_saida(leitura.botoes);
                let pad = entrada::para_pad(&para_o_jogo);
                if jogo.primeiro {
                    jogo.pad_antes = pad;
                    jogo.primeiro = false;
                }
                let antes = jogo.pad_antes;
                jogo.sessao.set_port_pad(0, pad);
                for (avk, apertada) in input::teclas_do_controle(&antes, &pad) {
                    jogo.sessao.set_key(avk, apertada);
                }
                jogo.pad_antes = pad;

                let agora = Instant::now();
                let fatia = agora
                    .saturating_duration_since(jogo.ultimo)
                    .min(FATIA_MAXIMA);
                jogo.ultimo = agora;
                if jogo.faixa_suja {
                    jogo.faixa_suja = false;
                    tela.apresenta(|q| apresentacao::limpa(q, apresentacao::PRETO));
                    // A limpeza troca viewport e programa. O espelho da placa ainda acha que
                    // o estado de antes está valendo, e o quadro seguinte sairia na tela inteira.
                    if placa::ativa() {
                        jogo.sessao.retoma_o_contexto();
                    }
                }
                if placa::ativa() {
                    placa::prepara_o_quadro();
                }
                let intermediario = jogo.sessao.mostra_quadro_intermediario();
                if !intermediario {
                    passo = jogo.sessao.anda(fatia, &RITMO);
                }
                if passo == Step::Presented {
                    apresentou = true;
                }
                pedido = jogo.sessao.take_launch_request();
                if passo == Step::Stopped {
                    normal = jogo.sessao.saiu_sozinho();
                    motivo = jogo.sessao.stopped_reason().unwrap_or_default();
                    classe = jogo.sessao.classe();
                    pela = jogo.pela_z_wheel;
                }
                // A imagem sai antes do mix. O som não muda o quadro, e misturá-lo
                // antes da troca atrasava o comando pelo tempo do render.
                if intermediario || passo == Step::Presented {
                    // A textura do destino, 1:1 no centro. Sem a devolução do quadro a
                    // sessão está no processador, e a imagem sai pela CPU.
                    let textura = match placa::guarda_o_quadro() && !intermediario {
                        true => jogo.sessao.textura_do_quadro(),
                        false => None,
                    };
                    if let Some(id) = textura {
                        placa::mostra_textura(id, 160, 32, 640, 480);
                        jogo.sessao.retoma_o_contexto();
                    } else {
                        tela.apresenta(|q| {
                            // A cópia só escreve o centro. Sem isto a lista fica na faixa.
                            apresentacao::limpa(q, apresentacao::PRETO);
                            let screen = jogo.sessao.screen();
                            apresentacao::copia_rgb565(
                                q,
                                screen.pixels(),
                                screen.width(),
                                screen.height(),
                            );
                        });
                    }
                } else if passo == Step::Ahead {
                    // Adiantado, a volta volta na hora. Sem esta espera o laço gira no vazio.
                    unsafe { sceDisplayWaitVblankStart() };
                }
                if som.porta >= 0 {
                    let agora_ms = jogo.sessao.clock_ms();
                    let virt = u64::from(agora_ms.wrapping_sub(jogo.relogio));
                    jogo.relogio = agora_ms;
                    let real = jogo.ultimo_som.elapsed().as_millis() as u64;
                    jogo.ultimo_som = Instant::now();
                    let devidas = som::amostras_devidas(real, virt, som::TAXA);
                    jogo.mixer.render_into(devidas, &mut jogo.mix);
                    jogo.pcm.clear();
                    jogo.pcm.extend(
                        jogo.mix
                            .iter()
                            .map(|amostra| (amostra.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16),
                    );
                    som.anel.empurra(&jogo.pcm);
                }
            }
        }
        despeja();
        if sair {
            anota("voltou à lista");
            self.volta_ao_menu(som, String::new());
            return;
        }
        if let Some(classe) = pedido {
            match self.caminho_da_classe(classe) {
                Some(caminho) => {
                    let pela_z_wheel = classe != session::Z_WHEEL;
                    if let Err(erro) = self.abre_caminho(&caminho, pela_z_wheel, som) {
                        anota(&erro);
                        self.volta_ao_menu(som, erro);
                    }
                }
                None => {
                    let erro = format!("O shell pediu {classe:#010x}, que não está na pasta");
                    anota(&erro);
                    self.volta_ao_menu(som, erro);
                }
            }
            return;
        }
        if apresentou {
            self.viu_quadro = true;
            self.saidas_sem_quadro = 0;
        }
        if passo != Step::Stopped {
            return;
        }
        if !normal {
            anota(&motivo);
            self.volta_ao_menu(som, motivo);
            return;
        }
        if !self.viu_quadro {
            self.saidas_sem_quadro += 1;
        } else {
            self.saidas_sem_quadro = 0;
        }
        let reabre = classe == session::Z_WHEEL || pela;
        if reabre && self.saidas_sem_quadro < 3 {
            if let Some(caminho) = self.caminho_da_classe(session::Z_WHEEL) {
                if let Err(erro) = self.abre_caminho(&caminho, false, som) {
                    anota(&erro);
                    self.volta_ao_menu(som, erro);
                }
                return;
            }
        }
        self.volta_ao_menu(som, String::new());
    }
}

fn sobe_o_clock() {
    unsafe {
        let antes_arm = scePowerGetArmClockFrequency();
        let antes_bus = scePowerGetBusClockFrequency();
        let arm = scePowerSetArmClockFrequency(CLOCK_ARM_MHZ);
        let bus = scePowerSetBusClockFrequency(CLOCK_BARRAMENTO_MHZ);
        let agora_arm = scePowerGetArmClockFrequency();
        let agora_bus = scePowerGetBusClockFrequency();
        anota(&format!(
            "clock {antes_arm}/{antes_bus} MHz -> {agora_arm}/{agora_bus} MHz (pedidos {arm}/{bus})"
        ));
    }
}

fn ajusta_prioridade() {
    unsafe {
        let id = sceKernelGetThreadId();
        let rc = sceKernelChangeThreadPriority(id, PRIORIDADE);
        if rc < 0 {
            anota(&format!("prioridade {PRIORIDADE} recusada ({rc})"));
        }
    }
}

pub fn roda() -> ! {
    let _ = std::fs::create_dir_all(RAIZ);
    sobe_o_clock();
    ajusta_prioridade();
    let log = LOG.to_string();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut arquivo) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            use std::io::Write;
            let _ = writeln!(arquivo, "pânico: {info}");
        }
    }));
    // Falta de memória aborta sem passar pelo pânico. O gancho é `fn`, não closure: no meio
    // de uma alocação recusada ele não pode capturar nada.
    std::alloc::set_alloc_error_hook(sem_memoria);
    unsafe { std::env::set_var("HOME", RAIZ) };
    zeebx::registro::le_do_ambiente();
    // 8 MiB de som já decodificado, o teto do perfil Portátil. Acima disso o cache compete com
    // o jogo pela RAM do aparelho.
    zeebx::machine::define_teto_do_cache_de_som(8 * 1024 * 1024);
    anota("Zeebx");
    anota_livre();

    let na_placa = match placa::abre() {
        placa::Abertura::Pronta => true,
        placa::Abertura::SemPlaca => false,
        placa::Abertura::TomouATela(erro) => {
            anota(&erro);
            morre(1);
        }
    };
    let mut tela = match Tela::abre(na_placa) {
        Ok(tela) => tela,
        Err(erro) => {
            anota(&erro);
            morre(1);
        }
    };
    let som = Som::abre();
    let mut app = Aplicativo::novo();
    tela.apresenta(|q| apresentacao::desenha_menu(q, &[], 0, "Lendo a pasta..."));
    app.varre();
    despeja();
    loop {
        app.menu(&mut tela);
        if let Err(erro) = app.abre_cursor(&som) {
            anota(&erro);
            app.erro = erro;
            continue;
        }
        while app.jogo.is_some() {
            app.quadro(&mut tela, &som);
        }
    }
}
