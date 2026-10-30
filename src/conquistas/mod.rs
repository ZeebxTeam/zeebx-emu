//! Conquistas do RetroAchievements, pelo `rc_client` do rcheevos. **Só o modo casual:** o hardcore
//! exige um emulador aprovado pelo RA.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rcheevos_sys as rc;

pub mod http;

/// O cliente que a janela guarda e empresta à sessão do jogo aberto. Um só: a conta é uma só.
pub type Compartilhado = Rc<RefCell<Conquistas>>;

pub trait Memoria {
    /// Copia `destino.len()` bytes a partir de `endereco`; `false` se a faixa não está mapeada.
    fn le(&self, endereco: u32, destino: &mut [u8]) -> bool;
}

/// As faixas do espaço de endereços do RA: o começo no RA, o começo no guest e o tamanho.
/// **Provisório: a identidade**, até o mapa do Zeebo ser combinado com os mantenedores do RA.
pub const MAPA: &[(u32, u32, u64)] = &[(0, 0, 1 << 32)];

/// Onde `ra` está no guest, e quantos dos `quantos` bytes cabem antes de a faixa acabar.
fn traduz(ra: u32, quantos: u32) -> Option<(u32, u32)> {
    let &(inicio, guest, tamanho) = MAPA
        .iter()
        .find(|&&(inicio, _, tamanho)| inicio <= ra && u64::from(ra - inicio) < tamanho)?;
    let deslocamento = ra - inicio;
    let cabe = (tamanho - u64::from(deslocamento)).min(u64::from(quantos)) as u32;
    Some((guest.checked_add(deslocamento)?, cabe))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pedido {
    pub url: String,
    /// O formulário de um POST; `None` é um GET.
    pub corpo: Option<String>,
}

pub trait Servidor {
    /// Começa o pedido. A resposta vai pela `entrega`, de qualquer thread.
    fn pede(&mut self, pedido: Pedido, entrega: Entrega);
    fn define_user_agent(&mut self, _user_agent: &str) {}
}

type Callback = unsafe extern "C" fn(*const rc::rc_api_server_response_t, *mut c_void);

struct Chegada {
    callback: Callback,
    dados: *mut c_void,
    status: c_int,
    corpo: Vec<u8>,
}

// SAFETY: o callback só é chamado na thread do cliente, em `Conquistas::entrega_respostas`.
unsafe impl Send for Chegada {}

type Fila = Arc<Mutex<Vec<Chegada>>>;

/// Onde a resposta de um pedido é deixada. O rcheevos espera **exatamente uma** resposta por
/// pedido, e por isso uma entrega descartada responde sozinha, como falha.
pub struct Entrega {
    fila: Fila,
    callback: Option<(Callback, *mut c_void)>,
}

// SAFETY: como a `Chegada`.
unsafe impl Send for Entrega {}

impl Entrega {
    pub fn responde(mut self, status: u16, corpo: Vec<u8>) {
        self.deixa(c_int::from(status), corpo);
    }

    /// O pedido não chegou ao servidor: o rcheevos tenta de novo quando faz sentido.
    pub fn falha_de_rede(mut self, mensagem: &str) {
        self.deixa(
            rc::RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR,
            mensagem.as_bytes().to_vec(),
        );
    }

    fn deixa(&mut self, status: c_int, corpo: Vec<u8>) {
        let Some((callback, dados)) = self.callback.take() else {
            return;
        };
        let chegada = Chegada {
            callback,
            dados,
            status,
            corpo,
        };
        // Um cadeado envenenado não estraga a fila, e perder a resposta penduraria o pedido.
        self.fila
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(chegada);
    }
}

impl Drop for Entrega {
    fn drop(&mut self) {
        self.deixa(
            rc::RC_API_SERVER_RESPONSE_CLIENT_ERROR,
            b"o pedido foi descartado sem resposta".to_vec(),
        );
    }
}

/// O que aconteceu, para a interface mostrar. Ver [`Conquistas::eventos`].
#[derive(Clone, Debug, PartialEq)]
pub enum Evento {
    Entrou(Result<(), String>),
    /// `Ok(None)` é um hash que o RA não conhece: o jogo roda sem conquistas.
    JogoCarregado(Result<Option<Jogo>, String>),
    Desbloqueou(Conquista),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conquista {
    pub id: u32,
    pub titulo: String,
    pub pontos: u32,
    pub icone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jogo {
    pub titulo: String,
    pub icone: Option<String>,
    pub total: u32,
    pub desbloqueadas: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usuario {
    pub nome: String,
    /// O que se guarda no lugar da senha.
    pub token: String,
    /// Os pontos do modo casual.
    pub pontos: u32,
}

/// O hash do RA para um jogo do Zeebo, pelo `rc_hash`: o MD5 do `.mod` principal.
pub fn hash_do_modulo(modulo: &[u8]) -> Option<String> {
    let mut hash = [0 as c_char; 33];
    // SAFETY: `hash` tem os 33 bytes que o header pede, e o rcheevos termina a string.
    unsafe {
        let ok = rc::rc_hash_generate_from_buffer(
            hash.as_mut_ptr(),
            rc::RC_CONSOLE_ZEEBO,
            modulo.as_ptr(),
            modulo.len(),
        );
        (ok != 0).then(|| texto(hash.as_ptr()))
    }
}

/// O que os callbacks alcançam pelo `userdata` do cliente. Acessado **só por ponteiro cru**: uma
/// chamada ao rcheevos pode voltar a um callback nosso, e um `&mut` vivo de cada lado seria UB.
struct Estado {
    servidor: Box<dyn Servidor>,
    fila: Fila,
    /// A memória do quadro em andamento. Ver [`Conquistas::com_memoria`].
    memoria: Option<*const (dyn Memoria + 'static)>,
    eventos: Vec<Evento>,
}

/// O que a [`crate::session::Session`] guarda das conquistas.
#[derive(Default)]
pub struct DaSessao {
    /// O hash do `.mod` que a sessão carregou.
    pub hash: Option<String>,
    pub cliente: Option<Compartilhado>,
    /// O último quadro de 60 Hz avaliado, no relógio virtual. `None` reancora no próximo.
    pub ultimo_quadro: Option<u64>,
}

pub struct Conquistas {
    cliente: *mut rc::rc_client_t,
    estado: *mut Estado,
}

impl Conquistas {
    /// Um cliente desconectado, em modo casual. `ZEEBX_RA_HOST` troca o servidor.
    pub fn novo(servidor: Box<dyn Servidor>) -> Self {
        let estado = Box::into_raw(Box::new(Estado {
            servidor,
            fila: Arc::default(),
            memoria: None,
            eventos: Vec::new(),
        }));
        // SAFETY: os callbacks têm as assinaturas do header. O `estado` vive até o `Drop`.
        let cliente = unsafe {
            let cliente = rc::rc_client_create(Some(le_memoria), Some(chama_servidor));
            assert!(!cliente.is_null(), "o rcheevos não criou o cliente");
            rc::rc_client_set_userdata(cliente, estado.cast());
            rc::rc_client_set_event_handler(cliente, Some(ao_evento));
            rc::rc_client_enable_logging(cliente, rc::RC_CLIENT_LOG_LEVEL_INFO, Some(registra));
            // A memória só existe dentro do quadro.
            rc::rc_client_set_allow_background_memory_reads(cliente, 0);
            rc::rc_client_set_hardcore_enabled(cliente, 0);
            cliente
        };
        let conquistas = Self { cliente, estado };
        if let Ok(host) = std::env::var("ZEEBX_RA_HOST")
            && let Ok(host) = CString::new(host.trim())
            && !host.is_empty()
        {
            // SAFETY: o rcheevos copia a string.
            unsafe { rc::rc_client_set_host(conquistas.cliente, host.as_ptr()) };
        }
        let user_agent = conquistas.user_agent();
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { (*conquistas.estado).servidor.define_user_agent(&user_agent) };
        conquistas
    }

    /// `Zeebx/<versão> (<sistema>) rcheevos/<versão>`: o RA cobra um user agent por emulador.
    pub fn user_agent(&self) -> String {
        let mut clausula = [0 as c_char; 64];
        // SAFETY: o tamanho é o do buffer, e o rcheevos termina a string.
        let clausula = unsafe {
            rc::rc_client_get_user_agent_clause(self.cliente, clausula.as_mut_ptr(), 64);
            texto(clausula.as_ptr())
        };
        let sistema = std::env::consts::OS;
        format!("Zeebx/{} ({sistema}) {clausula}", env!("CARGO_PKG_VERSION"))
    }

    /// Entra com usuário e senha. O resultado chega como [`Evento::Entrou`].
    pub fn entra_com_senha(&mut self, usuario: &str, senha: &str) {
        self.entra(usuario, senha, false);
    }

    /// Entra com o [`Usuario::token`] de um login anterior.
    pub fn entra_com_token(&mut self, usuario: &str, token: &str) {
        self.entra(usuario, token, true);
    }

    fn entra(&mut self, usuario: &str, segredo: &str, com_token: bool) {
        let (Ok(usuario), Ok(segredo)) = (CString::new(usuario), CString::new(segredo)) else {
            return self.anota(Evento::Entrou(Err(
                "byte nulo no usuário ou na senha".into()
            )));
        };
        let comeca = match com_token {
            true => rc::rc_client_begin_login_with_token,
            false => rc::rc_client_begin_login_with_password,
        };
        let nulo = std::ptr::null_mut();
        // SAFETY: o rcheevos copia as strings.
        unsafe {
            comeca(
                self.cliente,
                usuario.as_ptr(),
                segredo.as_ptr(),
                Some(ao_entrar),
                nulo,
            )
        };
        self.entrega_respostas();
    }

    /// Sai da conta. O jogo carregado vai junto.
    pub fn sai(&mut self) {
        self.descarrega_jogo();
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_logout(self.cliente) };
    }

    pub fn usuario(&self) -> Option<Usuario> {
        // SAFETY: o ponteiro vale até o login mudar, e é copiado aqui.
        unsafe {
            let u = rc::rc_client_get_user_info(self.cliente).as_ref()?;
            Some(Usuario {
                nome: texto(u.username),
                token: texto(u.token),
                pontos: u.score_casual,
            })
        }
    }

    /// Começa a identificar o jogo pelo hash. O [`Evento::JogoCarregado`] chega no primeiro
    /// [`Conquistas::quadro`] depois da resposta: o rcheevos valida os endereços nele.
    pub fn carrega_jogo(&mut self, hash: &str) {
        let Ok(hash) = CString::new(hash) else {
            return;
        };
        let nulo = std::ptr::null_mut();
        // SAFETY: o rcheevos copia o hash.
        unsafe {
            rc::rc_client_begin_load_game(self.cliente, hash.as_ptr(), Some(ao_carregar), nulo)
        };
        self.entrega_respostas();
    }

    /// Antes, o `rc_client_reset` solta a maestria pendente (a última conquista que ainda não
    /// teve quadro): o `rc_client_unload_game` a soltaria com o jogo já nulo, e o rcheevos cairia.
    pub fn descarrega_jogo(&mut self) {
        // SAFETY: cliente válido até o `Drop`.
        unsafe {
            rc::rc_client_reset(self.cliente);
            rc::rc_client_unload_game(self.cliente);
        }
    }

    /// Avalia as conquistas de **um** quadro do console.
    pub fn quadro(&mut self, memoria: &dyn Memoria) {
        self.entrega_respostas();
        // SAFETY: cliente válido até o `Drop`.
        self.com_memoria(memoria, |cliente| unsafe {
            rc::rc_client_do_frame(cliente)
        });
        self.entrega_respostas();
    }

    /// Nenhum quadro anda: só os pedidos. Sem `memoria`, o `rc_client_idle` não é chamado: ele
    /// pode validar os endereços de um jogo recém-carregado e, sem memória, desligaria todos.
    pub fn ocioso(&mut self, memoria: Option<&dyn Memoria>) {
        self.entrega_respostas();
        if let Some(memoria) = memoria {
            // SAFETY: cliente válido até o `Drop`.
            self.com_memoria(memoria, |cliente| unsafe { rc::rc_client_idle(cliente) });
            self.entrega_respostas();
        }
    }

    /// Os contadores de acerto, para o save state. Vazio sem jogo.
    pub fn progresso(&mut self) -> Vec<u8> {
        // SAFETY: o buffer tem o tamanho que o rcheevos pediu.
        unsafe {
            let tamanho = rc::rc_client_progress_size(self.cliente);
            let mut dados = vec![0u8; tamanho];
            let r =
                rc::rc_client_serialize_progress_sized(self.cliente, dados.as_mut_ptr(), tamanho);
            if r != rc::RC_OK {
                dados.clear();
            }
            dados
        }
    }

    /// Volta ao progresso de um save state. Vazio zera os contadores.
    pub fn restaura_progresso(&mut self, dados: &[u8]) -> bool {
        let ponteiro = match dados.is_empty() {
            true => std::ptr::null(),
            false => dados.as_ptr(),
        };
        // SAFETY: o rcheevos lê no máximo `dados.len()` bytes; nulo é o "sem progresso" dele.
        unsafe {
            rc::rc_client_deserialize_progress_sized(self.cliente, ponteiro, dados.len())
                == rc::RC_OK
        }
    }

    /// Os eventos desde a última chamada, do mais velho ao mais novo.
    pub fn eventos(&mut self) -> Vec<Evento> {
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { std::mem::take(&mut (*self.estado).eventos) }
    }

    fn anota(&mut self, evento: Evento) {
        // SAFETY: como acima.
        unsafe { (*self.estado).eventos.push(evento) };
    }

    /// Roda `f` com a `memoria` à disposição do [`le_memoria`].
    fn com_memoria(&mut self, memoria: &dyn Memoria, f: impl FnOnce(*mut rc::rc_client_t)) {
        // SAFETY: o tempo de vida apagado não escapa: as leituras em segundo plano estão
        // desligadas, e o ponteiro é recolhido antes de `memoria` sair de escopo.
        unsafe {
            let ponteiro: *const (dyn Memoria + '_) = memoria;
            (*self.estado).memoria = Some(std::mem::transmute::<
                *const (dyn Memoria + '_),
                *const (dyn Memoria + 'static),
            >(ponteiro));
            f(self.cliente);
            (*self.estado).memoria = None;
        }
    }

    /// Passa ao rcheevos as respostas que chegaram. Uma resposta pode gerar o pedido seguinte, que
    /// um servidor imediato põe na fila durante o laço; o teto é só contra um servidor quebrado.
    fn entrega_respostas(&mut self) {
        // SAFETY: o `Arc` é clonado sem segurar referência ao `Estado`.
        let fila = unsafe { Arc::clone(&(*self.estado).fila) };
        for _ in 0..1024 {
            let chegadas = std::mem::take(&mut *fila.lock().unwrap_or_else(|e| e.into_inner()));
            if chegadas.is_empty() {
                return;
            }
            for chegada in chegadas {
                let resposta = rc::rc_api_server_response_t {
                    body: chegada.corpo.as_ptr().cast(),
                    body_length: chegada.corpo.len(),
                    http_status_code: chegada.status,
                };
                // SAFETY: o callback que o rcheevos deu para este pedido, chamado uma vez.
                unsafe { (chegada.callback)(&resposta, chegada.dados) };
            }
        }
    }
}

impl Drop for Conquistas {
    fn drop(&mut self) {
        self.descarrega_jogo();
        // SAFETY: o cliente morre antes do estado, e nenhum callback é chamado depois.
        unsafe {
            rc::rc_client_destroy(self.cliente);
            drop(Box::from_raw(self.estado));
        }
    }
}

/// Uma string do C, copiada; vazia para nulo.
///
/// # Safety
/// `p` é nulo ou aponta para uma string terminada em zero.
unsafe fn texto(p: *const c_char) -> String {
    match p.is_null() {
        true => String::new(),
        // SAFETY: pelo contrato acima.
        false => unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned(),
    }
}

/// Como [`texto`], com `None` para nulo e vazio.
///
/// # Safety
/// Como em [`texto`].
unsafe fn talvez(p: *const c_char) -> Option<String> {
    // SAFETY: repassado.
    Some(unsafe { texto(p) }).filter(|t| !t.is_empty())
}

/// # Safety
/// `cliente` é um dos nossos, e o Rust não segura referência ao `Estado` enquanto o C roda.
unsafe fn estado_de<'a>(cliente: *const rc::rc_client_t) -> &'a mut Estado {
    // SAFETY: pelo contrato acima.
    unsafe { &mut *rc::rc_client_get_userdata(cliente).cast::<Estado>() }
}

unsafe extern "C" fn le_memoria(
    endereco: u32,
    destino: *mut u8,
    quantos: u32,
    cliente: *mut rc::rc_client_t,
) -> u32 {
    // SAFETY: `cliente` é nosso, e o `destino` tem `quantos` bytes.
    unsafe {
        let estado = estado_de(cliente);
        let (Some(memoria), Some((guest, cabe))) = (estado.memoria, traduz(endereco, quantos))
        else {
            return 0;
        };
        let destino = std::slice::from_raw_parts_mut(destino, cabe as usize);
        if (*memoria).le(guest, destino) {
            cabe
        } else {
            0
        }
    }
}

unsafe extern "C" fn chama_servidor(
    pedido: *const rc::rc_api_request_t,
    callback: rc::rc_client_server_callback_t,
    dados: *mut c_void,
    cliente: *mut rc::rc_client_t,
) {
    // SAFETY: `cliente` é nosso, e o `pedido` vale durante a chamada.
    unsafe {
        let Some(callback) = callback else {
            return;
        };
        let estado = estado_de(cliente);
        let pedido = Pedido {
            url: texto((*pedido).url),
            corpo: talvez((*pedido).post_data),
        };
        let fila = Arc::clone(&estado.fila);
        let callback = Some((callback, dados));
        estado.servidor.pede(pedido, Entrega { fila, callback });
    }
}

unsafe extern "C" fn ao_entrar(
    resultado: c_int,
    mensagem: *const c_char,
    cliente: *mut rc::rc_client_t,
    _: *mut c_void,
) {
    // SAFETY: `cliente` é nosso, e a mensagem vale durante a chamada.
    unsafe {
        let evento = match resultado {
            rc::RC_OK => Ok(()),
            _ => Err(texto(mensagem)),
        };
        estado_de(cliente).eventos.push(Evento::Entrou(evento));
    }
}

unsafe extern "C" fn ao_carregar(
    resultado: c_int,
    mensagem: *const c_char,
    cliente: *mut rc::rc_client_t,
    _: *mut c_void,
) {
    // SAFETY: como em `ao_entrar`; o jogo e o resumo são copiados aqui.
    unsafe {
        let evento = match resultado {
            rc::RC_OK => rc::rc_client_get_game_info(cliente).as_ref().map(|jogo| {
                let mut resumo = rc::rc_client_user_game_summary_t::default();
                rc::rc_client_get_user_game_summary(cliente, &mut resumo);
                Jogo {
                    titulo: texto(jogo.title),
                    icone: talvez(jogo.badge_url),
                    total: resumo.num_promoted_achievements,
                    desbloqueadas: resumo.num_unlocked_achievements,
                }
            }),
            rc::RC_NO_GAME_LOADED => None,
            // Outro carregamento tomou o lugar deste, e é ele que avisa.
            rc::RC_ABORTED => return,
            _ => {
                return estado_de(cliente)
                    .eventos
                    .push(Evento::JogoCarregado(Err(texto(mensagem))));
            }
        };
        estado_de(cliente)
            .eventos
            .push(Evento::JogoCarregado(Ok(evento)));
    }
}

unsafe extern "C" fn ao_evento(
    evento: *const rc::rc_client_event_t,
    cliente: *mut rc::rc_client_t,
) {
    // SAFETY: `cliente` é nosso, e o evento e o que ele aponta valem durante a chamada.
    unsafe {
        let e = &*evento;
        let Some(c) = e.achievement.as_ref() else {
            return;
        };
        if e.type_ != rc::RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED {
            return;
        }
        let mut url = [0 as c_char; 256];
        let estado = rc::RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED;
        let r = rc::rc_client_achievement_get_image_url(c, estado, url.as_mut_ptr(), 256);
        estado_de(cliente)
            .eventos
            .push(Evento::Desbloqueou(Conquista {
                id: c.id,
                titulo: texto(c.title),
                pontos: c.points,
                icone: (r == rc::RC_OK).then(|| texto(url.as_ptr())),
            }));
    }
}

unsafe extern "C" fn registra(mensagem: *const c_char, _: *const rc::rc_client_t) {
    // SAFETY: a mensagem vale durante a chamada.
    let mensagem = unsafe { texto(mensagem) };
    crate::registro!(
        crate::registro::Nivel::Informacao,
        "conquistas",
        "{mensagem}"
    );
}

#[cfg(test)]
mod teste;
