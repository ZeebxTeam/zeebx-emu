//! O Zeebx no iOS: a mesma sessão, outra tela.
//!
//! Quem emula é a [`zeebx::session::Session`]. Este arquivo é a fronteira com a UIKit: a Swift
//! pede uma varredura, um passo e um quadro, e devolve botão. A janela, o toque e o controle
//! de verdade ficam no projeto Xcode, porque é ele quem tem o ciclo de vida do UIKit.
//!
//! **Não há Dynarmic neste alvo.** O alocador de código do JIT pede uma página executável, e o
//! iOS — o simulador inclusive — recusa isso a um aplicativo de terceiro. O núcleo já tem o
//! interpretador para o caso em que o JIT não pode rodar; aqui é ele.
//!
//! A chamada é de uma linha só. O mixer de áudio é que atravessa threads, e isso já é o
//! contrato do `cpal` no desktop.

use std::ffi::{CStr, CString, c_char};
use std::path::PathBuf;
use std::ptr;
use std::time::Instant;

use zeebx::input::{self, Pad};
use zeebx::library;
use zeebx::session::{self, FATIA_MAXIMA, Session, Step};
use zeebx::storage::StoragePaths;

/// Os mesmos números de `include/zeebx_ios.h`. O teste abaixo confere o que dá para conferir
/// deste lado; o header é o que a Swift inclui.
const SEM_SESSAO: i32 = -1;
const QUADRO: i32 = 0;
const RODANDO: i32 = 1;
const ADIANTADO: i32 = 2;
const PAROU: i32 = 3;

/// O aplicativo, do lado de cá da fronteira.
///
/// Público porque a assinatura `extern "C"` o nomeia. Os campos ficam privados: a Swift só vê
/// o ponteiro, e não há construtor fora das funções daqui.
pub struct Aplicativo {
    roms: PathBuf,
    jogos: Vec<library::Game>,
    /// Um `CString` por título, para o ponteiro devolvido continuar válido.
    titulos: Vec<CString>,
    sessao: Option<Session>,
    pad: Pad,
    pad_anterior: Pad,
    ultimo: Instant,
    /// BGRA8 do quadro corrente. A Swift copia antes da próxima volta.
    quadro: Vec<u8>,
    largura: u32,
    altura: u32,
    erro: CString,
    storage: StoragePaths,
}

impl Aplicativo {
    fn define_erro(&mut self, texto: &str) {
        self.erro = CString::new(texto).unwrap_or_else(|_| CString::new("erro").expect("sem nulo"));
    }

    fn varre(&mut self) -> i32 {
        self.jogos = library::scan(&self.roms);
        self.titulos = self
            .jogos
            .iter()
            .map(|jogo| {
                CString::new(jogo.title.as_str())
                    .unwrap_or_else(|_| CString::new("jogo").expect("sem nulo"))
            })
            .collect();
        self.titulos.len() as i32
    }

    fn publica(&mut self) {
        let Some(sessao) = self.sessao.as_ref() else {
            self.quadro.clear();
            self.largura = 0;
            self.altura = 0;
            return;
        };
        let tela = sessao.screen();
        self.largura = tela.width();
        self.altura = tela.height();
        self.quadro = bgra_de_rgb565(tela.pixels());
    }
}

/// RGB565 do console para BGRA8, que é o que um `CGImage` little-endian com alfa na frente pede.
fn bgra_de_rgb565(pixels: &[u16]) -> Vec<u8> {
    let mut saida = Vec::with_capacity(pixels.len() * 4);
    for &pixel in pixels {
        let r5 = (pixel >> 11) & 0x1f;
        let g6 = (pixel >> 5) & 0x3f;
        let b5 = pixel & 0x1f;
        // O canal de 5 bits vira 8 repetindo os bits altos nos baixos: senão o branco do
        // console (31) sai 248 e a tela fica lavada.
        let r = ((r5 << 3) | (r5 >> 2)) as u8;
        let g = ((g6 << 2) | (g6 >> 4)) as u8;
        let b = ((b5 << 3) | (b5 >> 2)) as u8;
        saida.extend_from_slice(&[b, g, r, 255]);
    }
    saida
}

fn caminho(ptr: *const c_char) -> Option<PathBuf> {
    if ptr.is_null() {
        return None;
    }
    // O ponteiro veio da Swift nesta chamada e permanece válido até ela voltar.
    let texto = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
    if texto.is_empty() {
        return None;
    }
    Some(PathBuf::from(texto.as_ref()))
}

fn aplicativo<'a>(ptr: *mut Aplicativo) -> Option<&'a mut Aplicativo> {
    if ptr.is_null() {
        None
    } else {
        // O ponteiro é o que `zeebx_ios_cria` devolveu e que `zeebx_ios_destroi` ainda não
        // consumiu. A Swift o usa de uma linha só.
        Some(unsafe { &mut *ptr })
    }
}

fn c_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned(),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_cria(
    documentos: *const c_char,
    suporte: *const c_char,
) -> *mut Aplicativo {
    let (Some(documentos), Some(suporte)) = (caminho(documentos), caminho(suporte)) else {
        return ptr::null_mut();
    };

    // **O iOS não é `macos` para o `config_dir`.** Sem um `HOME` dentro do sandbox, o núcleo
    // cai no diretório corrente — que aqui não é gravável — e uma busca de SoundFont ou de
    // catálogo parece erro de ROM. A pasta Documents mora na raiz do container; o pai dela é
    // o lugar que o sistema trata como casa do aplicativo.
    //
    // Só neste alvo. Num `cargo test` do host o processo é o do cargo, e mudar o `HOME` dele
    // mudaria o de todo mundo que rodasse junto.
    #[cfg(target_os = "ios")]
    if let Some(casa) = documentos.parent() {
        unsafe { std::env::set_var("HOME", casa) };
    }

    let roms = documentos.join("roms");
    let _ = std::fs::create_dir_all(&roms);
    let storage = StoragePaths::for_frontend(&suporte, Some(&suporte));
    let erro_de_pasta = storage.create_dirs().err().map(|erro| erro.to_string());

    let mut app = Aplicativo {
        roms,
        jogos: Vec::new(),
        titulos: Vec::new(),
        sessao: None,
        pad: Pad::default(),
        pad_anterior: Pad::default(),
        ultimo: Instant::now(),
        quadro: Vec::new(),
        largura: 0,
        altura: 0,
        erro: CString::new("").expect("vazio"),
        storage,
    };
    if let Some(texto) = erro_de_pasta {
        app.define_erro(&texto);
    }
    app.varre();
    Box::into_raw(Box::new(app))
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_destroi(app: *mut Aplicativo) {
    if !app.is_null() {
        drop(unsafe { Box::from_raw(app) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_varre(app: *mut Aplicativo) -> i32 {
    aplicativo(app).map(|app| app.varre()).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_titulo(app: *mut Aplicativo, indice: i32) -> *const c_char {
    let Some(app) = aplicativo(app) else {
        return ptr::null();
    };
    let Ok(indice) = usize::try_from(indice) else {
        return ptr::null();
    };
    app.titulos
        .get(indice)
        .map(|titulo| titulo.as_ptr())
        .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_comeca(app: *mut Aplicativo, indice: i32) -> i32 {
    let Some(app) = aplicativo(app) else {
        return -1;
    };
    let Ok(indice) = usize::try_from(indice) else {
        app.define_erro("jogo fora da lista");
        return -1;
    };
    let Some(jogo) = app.jogos.get(indice) else {
        app.define_erro("jogo fora da lista");
        return -1;
    };
    let caminho = jogo.path.clone();
    let instalados: Vec<(u32, String)> = app
        .jogos
        .iter()
        .filter_map(|jogo| Some((jogo.clsid?, library::id_do_modulo(&jogo.path)?)))
        .collect();

    // Sem placa: no iOS o quadro é o do rasterizador de software, pintado pela UIKit. O
    // contexto de GL do núcleo pede EGL, e aqui não há.
    match Session::start_software_with_storage_installed(
        &caminho,
        zeebx::PORTAS_PADRAO,
        zeebx::config::ZWheel::default(),
        &app.storage,
        &instalados,
    ) {
        Ok(mut sessao) => {
            // Um aparelho sem saída de som não pode impedir o jogo. O motivo fica no erro só
            // quando a sessão em si não sobe; o som ausente vai para o log.
            if let Some(motivo) = sessao.set_audio(true, 100) {
                eprintln!("sem som: {motivo}");
            }
            app.sessao = Some(sessao);
            app.pad = Pad::default();
            app.pad_anterior = Pad::default();
            app.ultimo = Instant::now();
            app.erro = CString::new("").expect("vazio");
            app.publica();
            0
        }
        Err(erro) => {
            app.define_erro(&erro.to_string());
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_para(app: *mut Aplicativo) {
    let Some(app) = aplicativo(app) else {
        return;
    };
    app.sessao = None;
    app.pad = Pad::default();
    app.pad_anterior = Pad::default();
    app.quadro.clear();
    app.largura = 0;
    app.altura = 0;
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_passo(app: *mut Aplicativo) -> i32 {
    let Some(app) = aplicativo(app) else {
        return SEM_SESSAO;
    };
    if app.sessao.is_none() {
        return SEM_SESSAO;
    }

    let antes = app.pad_anterior;
    let pad = app.pad;
    {
        let sessao = app.sessao.as_mut().expect("conferido acima");
        sessao.set_port_pad(0, pad);
        for (avk, apertada) in input::teclas_do_controle(&antes, &pad) {
            sessao.set_key(avk, apertada);
        }
    }
    app.pad_anterior = pad;

    // O orçamento é o tempo real desde a volta anterior, preso ao teto: é quanto o jogo
    // precisa emular para acompanhar o relógio do mundo. O teto existe para uma volta que
    // demorou — o aplicativo voltou do segundo plano — não virar um salto de minutos.
    let agora = Instant::now();
    let fatia = agora
        .saturating_duration_since(app.ultimo)
        .min(FATIA_MAXIMA);
    app.ultimo = agora;

    let intermediario = app
        .sessao
        .as_mut()
        .expect("conferido acima")
        .mostra_quadro_intermediario();
    if intermediario {
        app.publica();
        return QUADRO;
    }

    let passo = app
        .sessao
        .as_mut()
        .expect("conferido acima")
        .step(fatia, true);
    if passo == Step::Stopped {
        let normal = app
            .sessao
            .as_ref()
            .is_some_and(session::Session::saiu_normalmente);
        let motivo = app
            .sessao
            .as_ref()
            .and_then(session::Session::stopped_reason)
            .unwrap_or_default();
        app.sessao = None;
        app.pad = Pad::default();
        app.pad_anterior = Pad::default();
        if !normal {
            app.define_erro(&motivo);
        }
        return PAROU;
    }
    app.publica();
    match passo {
        Step::Presented => QUADRO,
        Step::Ahead => ADIANTADO,
        Step::Running => RODANDO,
        Step::Stopped => PAROU,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_quadro(
    app: *mut Aplicativo,
    largura: *mut u32,
    altura: *mut u32,
) -> *const u8 {
    let Some(app) = aplicativo(app) else {
        return ptr::null();
    };
    if app.quadro.is_empty() || app.largura == 0 || app.altura == 0 {
        return ptr::null();
    }
    if !largura.is_null() {
        unsafe { *largura = app.largura };
    }
    if !altura.is_null() {
        unsafe { *altura = app.altura };
    }
    app.quadro.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_botao(app: *mut Aplicativo, indice: u32, apertado: i32) {
    let Some(app) = aplicativo(app) else {
        return;
    };
    let indice = indice as usize;
    if indice >= input::BUTTONS {
        return;
    }
    app.pad.press(indice, apertado != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_eixo(app: *mut Aplicativo, indice: u32, valor: i32) {
    let Some(app) = aplicativo(app) else {
        return;
    };
    app.pad.set_axis(indice as usize, valor);
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_botao_por_nome(nome: *const c_char) -> i32 {
    let Some(nome) = c_string(nome) else {
        return -1;
    };
    Pad::button_by_name(&nome)
        .map(|indice| indice as i32)
        .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn zeebx_ios_erro(app: *mut Aplicativo) -> *const c_char {
    aplicativo(app)
        .map(|app| app.erro.as_ptr())
        .unwrap_or(ptr::null())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branco_e_preto_do_rgb565_viram_bgra_cheio() {
        let pixels = [0u16, 0xffff, 0xf800];
        let bgra = bgra_de_rgb565(&pixels);
        assert_eq!(&bgra[0..4], &[0, 0, 0, 255]);
        assert_eq!(&bgra[4..8], &[255, 255, 255, 255]);
        assert_eq!(bgra[8], 0, "azul");
        assert_eq!(bgra[9], 0, "verde");
        assert_eq!(bgra[10], 255, "vermelho");
        assert_eq!(bgra[11], 255);
    }

    #[test]
    fn ponteiro_nulo_nao_cria() {
        assert!(zeebx_ios_cria(ptr::null(), ptr::null()).is_null());
        let vazio = CString::new("").unwrap();
        assert!(zeebx_ios_cria(vazio.as_ptr(), vazio.as_ptr()).is_null());
    }

    #[test]
    fn os_nomes_dos_botoes_seguem_o_controle() {
        assert_eq!(zeebx_ios_botao_por_nome(c"b1".as_ptr()), 0);
        assert_eq!(
            zeebx_ios_botao_por_nome(c"up".as_ptr()) as usize,
            Pad::button_by_name("up").unwrap()
        );
        assert_eq!(
            zeebx_ios_botao_por_nome(c"home".as_ptr()),
            zeebx_ios_botao_por_nome(c"back".as_ptr())
        );
        assert_eq!(zeebx_ios_botao_por_nome(ptr::null()), -1);
        assert_eq!(zeebx_ios_botao_por_nome(c"nao-existe".as_ptr()), -1);
    }

    fn pasta_de_teste(nome: &str) -> (PathBuf, CString, CString) {
        let raiz = std::env::temp_dir().join(format!("zeebx-ios-{nome}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raiz);
        let documentos = raiz.join("Documents");
        let suporte = raiz.join("Library/Application Support");
        std::fs::create_dir_all(&documentos).unwrap();
        std::fs::create_dir_all(&suporte).unwrap();
        let documentos_c = CString::new(documentos.to_str().unwrap()).unwrap();
        let suporte_c = CString::new(suporte.to_str().unwrap()).unwrap();
        (raiz, documentos_c, suporte_c)
    }

    #[test]
    fn pasta_vazia_lista_zero_e_nao_comeca_jogo() {
        let (raiz, documentos, suporte) = pasta_de_teste("vazia");
        let app = zeebx_ios_cria(documentos.as_ptr(), suporte.as_ptr());
        assert!(!app.is_null());
        assert_eq!(zeebx_ios_varre(app), 0);
        assert!(zeebx_ios_titulo(app, 0).is_null());
        assert_eq!(zeebx_ios_comeca(app, 0), -1);
        assert!(!zeebx_ios_erro(app).is_null());
        assert_eq!(zeebx_ios_passo(app), SEM_SESSAO);
        assert!(zeebx_ios_quadro(app, ptr::null_mut(), ptr::null_mut()).is_null());
        zeebx_ios_botao(app, 0, 1);
        zeebx_ios_eixo(app, 0, 40);
        zeebx_ios_para(app);
        zeebx_ios_destroi(app);
        let _ = std::fs::remove_dir_all(raiz);
    }
}
