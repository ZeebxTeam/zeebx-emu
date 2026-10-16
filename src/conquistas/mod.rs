//! Conquistas do RetroAchievements, pelo `rc_client` do rcheevos.
//!
//! O [`Conquistas`] é o cliente: login, o jogo carregado, as conquistas avaliadas a cada quadro e
//! os desbloqueios enviados. Ele vive **acima** da [`crate::session::Session`] — a conta continua
//! conectada de um jogo para o outro — e a sessão o recebe emprestado por
//! [`crate::session::Session::liga_conquistas`]. O desenho todo, e o que o RA exige de um
//! emulador, está em `docs/implementacao/25-conquistas.md`.
//!
//! # As três peças que o emulador entrega
//!
//! - **A memória** ([`Memoria`]), lida só dentro de [`Conquistas::quadro`] e
//!   [`Conquistas::ocioso`]. O `rc_client` é configurado para **não** ler fora deles
//!   (`rc_client_set_allow_background_memory_reads`): a validação dos endereços de um jogo que
//!   acabou de carregar fica para o quadro seguinte, quando a memória está à mão.
//! - **O servidor** ([`Servidor`]), que recebe cada pedido HTTP e devolve a resposta por uma
//!   [`Entrega`]. A resposta pode chegar de outra thread; ela só é passada ao rcheevos na thread
//!   do cliente, no próximo [`Conquistas::quadro`] ou [`Conquistas::ocioso`]. Os testes respondem
//!   na hora, com JSON escrito à mão.
//! - **O mapa** ([`Mapa`]), que traduz o endereço que uma conquista usa no endereço do guest. O
//!   mapa de memória do Zeebo ainda não foi combinado com os mantenedores do RA; o
//!   [`Mapa::provisorio`] é a identidade, e a decisão final muda só ele.

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rcheevos_sys as rc;

pub mod http;
pub mod icones;
pub mod pendentes;
pub mod resumos;

use pendentes::Pendentes;

/// O cliente que a janela guarda e a sessão usa. Um só por processo: a conta é uma só.
pub type Compartilhado = Rc<RefCell<Conquistas>>;

/// Quem sabe ler a memória do guest.
pub trait Memoria {
    /// Copia `destino.len()` bytes a partir do endereço `endereco` do guest. `false` quando a
    /// faixa não está toda mapeada; aí nada é garantido sobre o `destino`.
    fn le(&self, endereco: u32, destino: &mut [u8]) -> bool;
}

/// Um trecho contíguo do espaço de endereços do RA e onde ele está no guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Faixa {
    /// Onde a faixa começa, no endereço que as conquistas usam.
    pub ra: u32,
    /// Onde a faixa começa no guest.
    pub guest: u32,
    /// Quantos bytes. `u64` para que a faixa possa cobrir os 4 GiB inteiros.
    pub tamanho: u64,
}

/// A tradução dos endereços do RA para os do guest.
///
/// **É um contrato.** Uma conquista publicada guarda endereços; mudar o mapa depois quebra os
/// sets que já existem. Por isso ele mora num lugar só, e por isso o definitivo espera a conversa
/// com os mantenedores do RA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapa {
    faixas: Vec<Faixa>,
}

impl Mapa {
    /// A identidade sobre os 4 GiB: o endereço do RA é o do guest.
    ///
    /// Serve para desenvolver e **não** vai para o usuário. O que o guest não mapeia não se lê, e
    /// o `rc_client` desliga a conquista que depende de um endereço assim — sem mapa no
    /// `consoleinfo.c`, essa leitura é a única validação que ele faz.
    pub fn provisorio() -> Self {
        Self::de(vec![Faixa {
            ra: 0,
            guest: 0,
            tamanho: 1 << 32,
        }])
    }

    /// Um mapa com estas faixas. Elas não podem se sobrepor no espaço do RA.
    pub fn de(mut faixas: Vec<Faixa>) -> Self {
        faixas.sort_by_key(|f| f.ra);
        debug_assert!(
            faixas
                .windows(2)
                .all(|par| u64::from(par[0].ra) + par[0].tamanho <= u64::from(par[1].ra)),
            "faixas sobrepostas no espaço do RA: {faixas:?}"
        );
        Self { faixas }
    }

    /// Onde `ra` está no guest, e quantos bytes de `quantos` cabem antes de a faixa acabar.
    pub fn traduz(&self, ra: u32, quantos: u32) -> Option<(u32, u32)> {
        let faixa = self
            .faixas
            .iter()
            .rev()
            .find(|f| f.ra <= ra && u64::from(ra) < u64::from(f.ra) + f.tamanho)?;
        let deslocamento = u64::from(ra - faixa.ra);
        let cabe = (faixa.tamanho - deslocamento).min(u64::from(quantos)) as u32;
        let guest = u64::from(faixa.guest) + deslocamento;
        let guest = u32::try_from(guest).ok()?;
        Some((guest, cabe))
    }
}

/// Um pedido ao servidor do RA, como o rcheevos o montou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pedido {
    /// O endereço completo, com protocolo e host.
    pub url: String,
    /// O corpo de um POST (`application/x-www-form-urlencoded`). `None` pede um GET.
    pub corpo: Option<String>,
    /// O `Content-Type` do corpo.
    pub tipo: Option<String>,
}

/// Quem leva os pedidos ao servidor.
///
/// A implementação de verdade (HTTP, numa thread à parte) chega com a interface. Os testes usam
/// uma que responde na hora.
pub trait Servidor {
    /// Começa o pedido. A resposta vai pela `entrega`, agora ou depois, desta thread ou de outra.
    fn pede(&mut self, pedido: Pedido, entrega: Entrega);

    /// O user agent a mandar em cada pedido. Ver [`Conquistas::user_agent`].
    fn define_user_agent(&mut self, _user_agent: &str) {}
}

/// O retorno que o rcheevos deu para um pedido.
struct Retorno {
    callback: unsafe extern "C" fn(*const rc::rc_api_server_response_t, *mut c_void),
    dados: *mut c_void,
    /// A chave do pedido na fila em disco, se ele é um dos que não podem se perder. Ver
    /// [`pendentes`].
    pendente: Option<String>,
}

// SAFETY: o retorno só é **chamado** na thread do cliente, em `Conquistas::entrega_respostas`.
// As outras threads só o carregam de volta para a fila.
unsafe impl Send for Retorno {}

struct Chegada {
    retorno: Retorno,
    status: c_int,
    corpo: Vec<u8>,
}

type Fila = Arc<Mutex<Vec<Chegada>>>;

/// O lugar onde a resposta de um pedido é deixada.
///
/// Cada pedido do rcheevos espera **exatamente uma** resposta: a memória dele só é liberada no
/// retorno. Por isso uma entrega descartada sem resposta responde sozinha como falha de rede.
pub struct Entrega {
    fila: Fila,
    retorno: Option<Retorno>,
}

impl Entrega {
    /// A resposta do servidor: o código HTTP e o corpo.
    pub fn responde(mut self, status: u16, corpo: Vec<u8>) {
        self.deixa(c_int::from(status), corpo);
    }

    /// O pedido não chegou ao servidor. O rcheevos tenta de novo sozinho quando faz sentido —
    /// um desbloqueio, por exemplo, fica na fila até a rede voltar.
    pub fn falha_de_rede(mut self, mensagem: &str) {
        self.deixa(
            rc::RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR,
            mensagem.as_bytes().to_vec(),
        );
    }

    fn deixa(&mut self, status: c_int, corpo: Vec<u8>) {
        let Some(retorno) = self.retorno.take() else {
            return;
        };
        let chegada = Chegada {
            retorno,
            status,
            corpo,
        };
        // Um cadeado envenenado é de uma thread que já morreu no meio de um `push`; a fila
        // continua inteira, e perder a resposta deixaria o pedido pendurado para sempre.
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
    /// O login terminou. O erro vem no texto do servidor ("senha errada", "token expirado").
    Entrou(Result<(), String>),
    /// O jogo terminou de ser identificado. [`ErroAoCarregar::Desconhecido`] é o hash que o RA
    /// não conhece: o jogo roda, só sem conquistas.
    JogoCarregado(Result<(), ErroAoCarregar>),
    /// Uma conquista foi desbloqueada.
    Desbloqueou(Conquista),
    /// Uma tentativa de placar começou.
    PlacarComecou { id: u32, titulo: String },
    /// Uma tentativa de placar falhou.
    PlacarFalhou { id: u32, titulo: String },
    /// Um placar foi enviado, com o valor já formatado.
    PlacarEnviado {
        id: u32,
        titulo: String,
        valor: String,
    },
    /// O contador de um placar em andamento apareceu ou mudou.
    Contador { id: u32, texto: String },
    /// O contador de um placar sumiu.
    ContadorSumiu { id: u32 },
    /// O jogo tem de recomeçar do zero: o hardcore foi ligado com ele aberto.
    Reiniciar,
    /// Todas as conquistas do jogo foram desbloqueadas.
    JogoCompleto,
    /// O servidor recusou algo e o rcheevos não vai tentar de novo.
    ErroDoServidor { api: String, mensagem: String },
    /// Um desbloqueio não chegou ao servidor e ficou pendente.
    Desconectado,
    /// Os desbloqueios pendentes chegaram.
    Reconectado,
}

/// Por que o jogo não carregou.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErroAoCarregar {
    /// O RA não conhece este hash.
    Desconhecido,
    /// Outro carregamento tomou o lugar deste.
    Cancelado,
    /// Qualquer outra falha, com o texto do rcheevos.
    Outro(String),
}

/// Uma conquista, como a interface a mostra.
#[derive(Clone, Debug, PartialEq)]
pub struct Conquista {
    pub id: u32,
    pub titulo: String,
    pub descricao: String,
    pub pontos: u32,
    /// O ícone desbloqueado, no servidor de mídia do RA.
    pub icone: Option<String>,
    /// O ícone apagado, de enquanto ela não sai.
    pub icone_bloqueado: Option<String>,
    pub desbloqueada: bool,
    /// Desbloqueada no hardcore, e não só no casual.
    pub no_hardcore: bool,
    /// O progresso das que medem algo ("12/30"); vazio nas outras.
    pub progresso: String,
    /// O mesmo progresso, de 0 a 100.
    pub percentual: f32,
    /// O emulador não lê algum endereço dela, e ela nunca vai disparar. Ver [`Mapa`].
    pub sem_suporte: bool,
}

/// Os grupos da lista, na ordem em que o rcheevos os entrega.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grupo {
    Bloqueadas,
    Desbloqueadas,
    SemSuporte,
    NaoOficiais,
    RecemDesbloqueadas,
    DesafioEmAndamento,
    QuaseLa,
    /// Desbloqueadas aqui e ainda não confirmadas pelo servidor.
    NaoEnviadas,
    Outras,
}

impl Grupo {
    fn de(tipo: u8) -> Self {
        match tipo {
            1 => Self::Bloqueadas,
            2 => Self::Desbloqueadas,
            3 => Self::SemSuporte,
            4 => Self::NaoOficiais,
            5 => Self::RecemDesbloqueadas,
            6 => Self::DesafioEmAndamento,
            7 => Self::QuaseLa,
            8 => Self::NaoEnviadas,
            _ => Self::Outras,
        }
    }
}

/// Quantas conquistas o jogo tem e quantas o jogador já tem. É o "12 de 40" do aviso de abertura.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resumo {
    pub total: u32,
    pub desbloqueadas: u32,
    pub sem_suporte: u32,
    pub pontos: u32,
    pub pontos_desbloqueados: u32,
}

/// A conta conectada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Usuario {
    pub nome: String,
    pub nome_de_exibicao: String,
    /// O que se guarda no lugar da senha. Ver [`Conquistas::entra_com_token`].
    pub token: String,
    pub pontos: u32,
    pub pontos_casual: u32,
}

/// O jogo que o RA reconheceu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jogo {
    pub id: u32,
    pub titulo: String,
    pub hash: String,
    pub icone: Option<String>,
}

/// O hash do RA para um jogo do Zeebo: o MD5 do `.mod` principal.
///
/// É o `rc_hash` do próprio rcheevos que faz a conta, e não um MD5 nosso: o hash de referência do
/// console mora lá, e assim os dois não têm como divergir. `None` se o rcheevos ligado não sabe o
/// Zeebo — o oficial ainda não sabe; o fork no submódulo sabe.
pub fn hash_do_modulo(modulo: &[u8]) -> Option<String> {
    let mut hash = [0 as c_char; 33];
    // SAFETY: `hash` tem os 33 bytes que o header pede, e o buffer vale durante a chamada.
    let ok = unsafe {
        rc::rc_hash_generate_from_buffer(
            hash.as_mut_ptr(),
            rc::RC_CONSOLE_ZEEBO,
            modulo.as_ptr(),
            modulo.len(),
        )
    };
    // SAFETY: o rcheevos sempre termina a string, mesmo quando falha.
    (ok != 0).then(|| unsafe { texto(hash.as_ptr()) })
}

/// O que os retornos do C alcançam pelo `userdata` do cliente.
///
/// Mora numa caixa à parte e é acessado **só por ponteiro cru**, dos dois lados. Uma chamada ao
/// rcheevos pode voltar a um retorno nosso, e um `&mut` vivo de um lado com o outro lado mexendo
/// no mesmo lugar seria comportamento indefinido.
struct Estado {
    servidor: Box<dyn Servidor>,
    fila: Fila,
    /// A memória do quadro em andamento. Só existe dentro de [`Conquistas::com_memoria`].
    memoria: Option<*const (dyn Memoria + 'static)>,
    mapa: Mapa,
    eventos: Vec<Evento>,
    pendentes: Pendentes,
    /// O login deu certo e a fila da execução anterior ainda não foi mandada de novo.
    reenviar: bool,
    /// O servidor em uso, quando não é o oficial: os pedidos remontados vão para ele.
    host: Option<CString>,
}

/// O que a [`crate::session::Session`] guarda das conquistas.
#[derive(Default)]
pub struct DaSessao {
    /// O hash do `.mod` que a sessão carregou. Ver [`hash_do_modulo`].
    pub hash: Option<String>,
    /// O cliente emprestado pela janela, quando há conta e o jogo foi mandado carregar.
    pub cliente: Option<Compartilhado>,
    /// O último quadro de 60 Hz avaliado, contado no relógio virtual. `None` reancora no
    /// próximo: no começo e depois de um save state, que move o relógio para onde quiser.
    pub ultimo_quadro: Option<u64>,
}

/// O cliente do RetroAchievements. Ver o topo do módulo.
pub struct Conquistas {
    cliente: *mut rc::rc_client_t,
    estado: *mut Estado,
}

impl Conquistas {
    /// Um cliente desconectado, em modo **casual**: o hardcore só vale para emuladores aceitos
    /// pelo RA, e o Zeebx ainda não é um deles.
    ///
    /// A fila de desbloqueios pendentes fica em [`pendentes::caminho_padrao`]; nos testes, que não
    /// podem tocar a configuração de quem os roda, ela fica desligada. Ver [`Conquistas::com_fila`].
    ///
    /// **`ZEEBX_RA_HOST`** troca o servidor, para desenvolver contra o
    /// `ferramentas/servidor_ra_simulado.py` (ver `docs/implementacao/25-conquistas.md`).
    pub fn novo(servidor: Box<dyn Servidor>) -> Self {
        #[cfg(not(test))]
        let fila = Pendentes::em(pendentes::caminho_padrao());
        #[cfg(test)]
        let fila = Pendentes::desligada();
        let mut conquistas = Self::com_fila(servidor, fila);
        if let Some(host) = std::env::var("ZEEBX_RA_HOST")
            .ok()
            .filter(|h| !h.trim().is_empty())
        {
            crate::registro!(
                crate::registro::Nivel::Aviso,
                "conquistas",
                "ZEEBX_RA_HOST: falando com {host}, e não com o RetroAchievements"
            );
            conquistas.define_host(Some(host.trim()));
        }
        conquistas
    }

    /// Como o [`Conquistas::novo`], com a fila de pendentes dada.
    pub fn com_fila(servidor: Box<dyn Servidor>, pendentes: Pendentes) -> Self {
        let estado = Box::into_raw(Box::new(Estado {
            servidor,
            fila: Arc::default(),
            memoria: None,
            mapa: Mapa::provisorio(),
            eventos: Vec::new(),
            pendentes,
            reenviar: false,
            host: None,
        }));
        // SAFETY: os retornos têm as assinaturas dos tipos do header. O `estado` vive até o
        // `Drop`, que destrói o cliente antes de soltá-lo.
        let cliente = unsafe {
            let cliente = rc::rc_client_create(Some(le_memoria), Some(chama_servidor));
            assert!(
                !cliente.is_null(),
                "o rcheevos não conseguiu criar o cliente"
            );
            rc::rc_client_set_userdata(cliente, estado.cast());
            rc::rc_client_set_event_handler(cliente, Some(ao_evento));
            rc::rc_client_enable_logging(cliente, rc::RC_CLIENT_LOG_LEVEL_INFO, Some(registra));
            rc::rc_client_set_allow_background_memory_reads(cliente, 0);
            rc::rc_client_set_hardcore_enabled(cliente, 0);
            cliente
        };
        let conquistas = Self { cliente, estado };
        let user_agent = conquistas.user_agent();
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { (*conquistas.estado).servidor.define_user_agent(&user_agent) };
        conquistas
    }

    /// Troca o mapa de endereços. Ver [`Mapa`].
    pub fn define_mapa(&mut self, mapa: Mapa) {
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { (*self.estado).mapa = mapa };
    }

    /// Aponta para outro servidor, como o de testes do RA. `None` volta ao oficial.
    pub fn define_host(&mut self, host: Option<&str>) {
        let host = host.and_then(|h| CString::new(h).ok());
        // SAFETY: o rcheevos copia a string; nenhuma chamada ao C em andamento ao mexer no
        // `Estado` (ver [`Estado`]).
        unsafe {
            rc::rc_client_set_host(
                self.cliente,
                host.as_ref().map_or(std::ptr::null(), |h| h.as_ptr()),
            );
            (*self.estado).host = host;
        }
    }

    /// O user agent que o servidor de HTTP deve mandar: `Zeebx/<versão> (<sistema>)` e o trecho
    /// do rcheevos. O RA cobra um user agent único e estável de cada emulador.
    pub fn user_agent(&self) -> String {
        let mut clausula = [0 as c_char; 64];
        // SAFETY: o tamanho passado é o do buffer, e o rcheevos termina a string.
        let clausula = unsafe {
            rc::rc_client_get_user_agent_clause(
                self.cliente,
                clausula.as_mut_ptr(),
                clausula.len(),
            );
            texto(clausula.as_ptr())
        };
        format!(
            "Zeebx/{} ({}) {clausula}",
            env!("CARGO_PKG_VERSION"),
            nome_do_sistema()
        )
    }

    /// Entra com usuário e senha. O resultado chega como [`Evento::Entrou`]; a senha não fica
    /// guardada em lugar nenhum, e o que se guarda é o [`Usuario::token`].
    pub fn entra_com_senha(&mut self, usuario: &str, senha: &str) {
        let (Ok(usuario), Ok(senha)) = (CString::new(usuario), CString::new(senha)) else {
            self.anota(Evento::Entrou(Err("usuário ou senha com byte nulo".into())));
            return;
        };
        // SAFETY: o rcheevos copia as strings antes de voltar.
        unsafe {
            rc::rc_client_begin_login_with_password(
                self.cliente,
                usuario.as_ptr(),
                senha.as_ptr(),
                Some(ao_entrar),
                std::ptr::null_mut(),
            );
        }
        self.entrega_respostas();
    }

    /// Entra com o token de um login anterior.
    pub fn entra_com_token(&mut self, usuario: &str, token: &str) {
        let (Ok(usuario), Ok(token)) = (CString::new(usuario), CString::new(token)) else {
            self.anota(Evento::Entrou(Err("usuário ou token com byte nulo".into())));
            return;
        };
        // SAFETY: como em `entra_com_senha`.
        unsafe {
            rc::rc_client_begin_login_with_token(
                self.cliente,
                usuario.as_ptr(),
                token.as_ptr(),
                Some(ao_entrar),
                std::ptr::null_mut(),
            );
        }
        self.entrega_respostas();
    }

    /// Sai da conta. O jogo carregado, se houver, é descarregado junto.
    pub fn sai(&mut self) {
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_logout(self.cliente) };
    }

    /// A conta conectada, se houver.
    pub fn usuario(&self) -> Option<Usuario> {
        // SAFETY: o ponteiro vale até a próxima chamada que mude o login, e é copiado aqui.
        unsafe {
            let u = rc::rc_client_get_user_info(self.cliente);
            let u = u.as_ref()?;
            Some(Usuario {
                nome: texto(u.username),
                nome_de_exibicao: texto(u.display_name),
                token: texto(u.token),
                pontos: u.score,
                pontos_casual: u.score_casual,
            })
        }
    }

    /// Começa a identificar o jogo pelo hash.
    ///
    /// O resultado chega como [`Evento::JogoCarregado`] **no primeiro [`Conquistas::quadro`]**
    /// depois de o servidor responder, e não antes: é ali que o rcheevos valida os endereços, e
    /// ele só lê memória dentro do quadro. Um hash desconhecido, que não tem o que validar, chega
    /// na hora.
    pub fn carrega_jogo(&mut self, hash: &str) {
        let Ok(hash) = CString::new(hash) else {
            self.anota(Evento::JogoCarregado(Err(ErroAoCarregar::Outro(
                "hash com byte nulo".into(),
            ))));
            return;
        };
        // SAFETY: o rcheevos copia o hash.
        unsafe {
            rc::rc_client_begin_load_game(
                self.cliente,
                hash.as_ptr(),
                Some(ao_carregar),
                std::ptr::null_mut(),
            );
        }
        self.entrega_respostas();
    }

    /// Descarrega o jogo: a sessão acabou.
    pub fn descarrega_jogo(&mut self) {
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_unload_game(self.cliente) };
    }

    /// O jogo carregado e reconhecido, se houver.
    pub fn jogo(&self) -> Option<Jogo> {
        // SAFETY: o ponteiro vale enquanto o jogo estiver carregado, e é copiado aqui.
        unsafe {
            if rc::rc_client_is_game_loaded(self.cliente) == 0 {
                return None;
            }
            let j = rc::rc_client_get_game_info(self.cliente).as_ref()?;
            Some(Jogo {
                id: j.id,
                titulo: texto(j.title),
                hash: texto(j.hash),
                icone: talvez(j.badge_url),
            })
        }
    }

    /// O resumo do jogo carregado, se houver um reconhecido.
    pub fn resumo(&self) -> Option<Resumo> {
        self.jogo()?;
        let mut r = rc::rc_client_user_game_summary_t::default();
        // SAFETY: o rcheevos escreve no struct inteiro, que tem o tamanho do C (teste de layout).
        unsafe { rc::rc_client_get_user_game_summary(self.cliente, &mut r) };
        Some(Resumo {
            total: r.num_promoted_achievements,
            desbloqueadas: r.num_unlocked_achievements,
            sem_suporte: r.num_unsupported_achievements,
            pontos: r.points_available,
            pontos_desbloqueados: r.points_unlocked,
        })
    }

    /// As conquistas oficiais do jogo carregado, nos grupos e na ordem do rcheevos.
    pub fn lista(&mut self) -> Vec<(Grupo, Vec<Conquista>)> {
        // SAFETY: a lista é do rcheevos e é destruída aqui mesmo, depois de copiada.
        unsafe {
            let lista = rc::rc_client_create_achievement_list(
                self.cliente,
                rc::RC_CLIENT_ACHIEVEMENT_CATEGORY_PROMOTED,
                rc::RC_CLIENT_ACHIEVEMENT_LIST_GROUPING_LOCK_STATE,
            );
            let Some(l) = lista.as_ref() else {
                return Vec::new();
            };
            let grupos = match l.buckets.is_null() {
                true => &[][..],
                false => std::slice::from_raw_parts(l.buckets, l.num_buckets as usize),
            };
            let copia = grupos
                .iter()
                .map(|g| {
                    let itens = match g.achievements.is_null() {
                        true => &[][..],
                        false => {
                            std::slice::from_raw_parts(g.achievements, g.num_achievements as usize)
                        }
                    };
                    let itens = itens.iter().filter_map(|c| conquista_de(*c)).collect();
                    (Grupo::de(g.bucket_type), itens)
                })
                .collect();
            rc::rc_client_destroy_achievement_list(lista);
            copia
        }
    }

    /// A conquista `id` do jogo carregado.
    pub fn conquista(&self, id: u32) -> Option<Conquista> {
        // SAFETY: o ponteiro vale enquanto o jogo estiver carregado, e é copiado aqui.
        unsafe { conquista_de(rc::rc_client_get_achievement_info(self.cliente, id)) }
    }

    /// Se o hardcore está ligado.
    pub fn hardcore(&self) -> bool {
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_get_hardcore_enabled(self.cliente) != 0 }
    }

    /// Se as regras do hardcore valem agora: o hardcore está ligado **e** há um jogo com
    /// conquistas, ou um a caminho.
    ///
    /// **A caminho conta.** Entre o pedido e a resposta do servidor o jogo ainda não tem
    /// conquistas, mas pode vir a ter: carregar um estado nesse intervalo e só depois ver o set
    /// chegar seria entrar no hardcore com a partida adulterada. Um jogo que o RA não conhece
    /// solta as regras, porque não há o que proteger.
    pub fn protege_o_jogo(&self) -> bool {
        if !self.hardcore() {
            return false;
        }
        // SAFETY: cliente válido até o `Drop`.
        let (estado, carregado) = unsafe {
            (
                rc::rc_client_get_load_game_state(self.cliente),
                rc::rc_client_is_game_loaded(self.cliente) != 0,
            )
        };
        match estado {
            rc::RC_CLIENT_LOAD_GAME_STATE_NONE
            | rc::RC_CLIENT_LOAD_GAME_STATE_DONE
            | rc::RC_CLIENT_LOAD_GAME_STATE_ABORTED => carregado,
            _ => true,
        }
    }

    /// Liga ou desliga o hardcore. Ligar com um jogo aberto pede o [`Evento::Reiniciar`]: o RA
    /// não aceita entrar no hardcore no meio de uma partida.
    pub fn define_hardcore(&mut self, ligado: bool) {
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_set_hardcore_enabled(self.cliente, c_int::from(ligado)) };
        self.entrega_respostas();
    }

    /// O jogo foi reiniciado: as conquistas e os placares voltam ao estado inicial.
    pub fn reinicia(&mut self) {
        // SAFETY: cliente válido até o `Drop`.
        unsafe { rc::rc_client_reset(self.cliente) };
    }

    /// Avalia as conquistas de **um** quadro do console, lendo a `memoria`.
    pub fn quadro(&mut self, memoria: &dyn Memoria) {
        self.entrega_respostas();
        // SAFETY: cliente válido até o `Drop`.
        self.com_memoria(memoria, |cliente| unsafe {
            rc::rc_client_do_frame(cliente)
        });
        self.entrega_respostas();
    }

    /// O jogo está parado (pausa, menu): os pedidos pendentes andam, e nenhuma conquista é
    /// avaliada. Sem `memoria` — nenhum jogo aberto —, só as respostas são entregues: o
    /// `rc_client_idle` pode validar os endereços de um jogo recém-carregado, e sem memória ele
    /// desligaria todas as conquistas.
    pub fn ocioso(&mut self, memoria: Option<&dyn Memoria>) {
        self.entrega_respostas();
        if let Some(memoria) = memoria {
            // SAFETY: cliente válido até o `Drop`.
            self.com_memoria(memoria, |cliente| unsafe { rc::rc_client_idle(cliente) });
            self.entrega_respostas();
        }
    }

    /// O progresso das conquistas (contadores, placares em andamento), para o save state.
    /// Vazio sem jogo carregado.
    pub fn progresso(&mut self) -> Vec<u8> {
        // SAFETY: o buffer tem o tamanho que o próprio rcheevos pediu.
        unsafe {
            let tamanho = rc::rc_client_progress_size(self.cliente);
            let mut dados = vec![0u8; tamanho];
            if tamanho == 0
                || rc::rc_client_serialize_progress_sized(self.cliente, dados.as_mut_ptr(), tamanho)
                    != rc::RC_OK
            {
                return Vec::new();
            }
            dados
        }
    }

    /// Volta ao progresso de um save state. Vazio (um estado de antes das conquistas) zera os
    /// contadores, como o rcheevos manda.
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
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { (*self.estado).eventos.push(evento) };
    }

    /// Roda `f` com a `memoria` à disposição do [`le_memoria`], e a recolhe na volta.
    fn com_memoria(&mut self, memoria: &dyn Memoria, f: impl FnOnce(*mut rc::rc_client_t)) {
        // SAFETY: apaga o tempo de vida para guardar o ponteiro no `Estado`. Ele só é lido
        // durante `f` — o rcheevos não lê memória fora de `do_frame`/`idle`, porque as leituras
        // em segundo plano estão desligadas — e é recolhido antes de `memoria` sair de escopo.
        unsafe {
            let ponteiro: *const (dyn Memoria + '_) = memoria;
            (*self.estado).memoria = Some(std::mem::transmute::<
                *const (dyn Memoria + '_),
                *const (dyn Memoria + 'static),
            >(ponteiro));
        }
        f(self.cliente);
        // SAFETY: como acima.
        unsafe { (*self.estado).memoria = None };
    }

    /// Quantos desbloqueios e placares esperam o servidor, na fila em disco.
    pub fn pendentes(&self) -> usize {
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        unsafe { (*self.estado).pendentes.quantos() }
    }

    /// Manda de novo, depois do login, o que uma execução anterior deixou na fila. A resposta
    /// não volta ao rcheevos — ele não fez esses pedidos —, só tira o pedido da fila.
    ///
    /// O pedido é **remontado** pelo rcheevos, com o token do login de agora, o servidor de agora
    /// e há quantos segundos o desbloqueio aconteceu: ver [`pendentes`].
    fn reenvia_pendentes(&mut self) {
        // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
        let estado = unsafe { &mut *self.estado };
        if !std::mem::take(&mut estado.reenviar) {
            return;
        }
        let Some((usuario, token)) = self.usuario().map(|u| (u.nome, u.token)) else {
            return;
        };
        let pendentes = estado.pendentes.da_execucao_anterior(&usuario);
        if !pendentes.is_empty() {
            crate::registro!(
                crate::registro::Nivel::Informacao,
                "conquistas",
                "mandando de novo {} pedido(s) que ficaram de uma execução anterior",
                pendentes.len()
            );
        }
        let agora = agora();
        for (chave, pendente) in pendentes {
            let Some(pedido) = monta(&pendente, &usuario, &token, estado.host.as_ref(), agora)
            else {
                continue;
            };
            let entrega = Entrega {
                fila: Arc::clone(&estado.fila),
                retorno: Some(Retorno {
                    callback: ignora_resposta,
                    dados: std::ptr::null_mut(),
                    pendente: Some(chave),
                }),
            };
            estado.servidor.pede(pedido, entrega);
        }
    }

    /// Passa ao rcheevos as respostas que chegaram, uma de cada vez.
    ///
    /// Uma resposta pode gerar o pedido seguinte (o login termina, o carregamento continua), e um
    /// servidor que responde na hora o põe na fila durante este laço. O teto só existe para que um
    /// servidor quebrado não prenda a thread.
    fn entrega_respostas(&mut self) {
        for _ in 0..1024 {
            // SAFETY: a fila é um `Arc` clonado do `Estado` sem segurar referência a ele.
            let fila = unsafe { Arc::clone(&(*self.estado).fila) };
            let chegadas = std::mem::take(&mut *fila.lock().unwrap_or_else(|e| e.into_inner()));
            if chegadas.is_empty() {
                self.reenvia_pendentes();
                return;
            }
            for chegada in chegadas {
                if let Some(chave) = &chegada.retorno.pendente {
                    // SAFETY: nenhuma chamada ao C em andamento; ver [`Estado`].
                    unsafe { (*self.estado).pendentes.resolve(chave, chegada.status) };
                }
                let resposta = rc::rc_api_server_response_t {
                    body: chegada.corpo.as_ptr().cast(),
                    body_length: chegada.corpo.len(),
                    http_status_code: chegada.status,
                };
                // SAFETY: o retorno é o que o rcheevos deu para este pedido, chamado uma vez; a
                // resposta vale durante a chamada, e o rcheevos copia o que guarda dela.
                unsafe { (chegada.retorno.callback)(&resposta, chegada.retorno.dados) };
            }
        }
        crate::registro!(
            crate::registro::Nivel::Aviso,
            "conquistas",
            "mais de 1024 voltas de respostas seguidas; o resto fica para o próximo quadro"
        );
    }
}

impl Drop for Conquistas {
    fn drop(&mut self) {
        // SAFETY: o cliente é destruído antes do estado, e nenhum retorno é chamado depois: as
        // respostas que chegarem ficam numa fila que ninguém mais esvazia.
        unsafe {
            rc::rc_client_destroy(self.cliente);
            drop(Box::from_raw(self.estado));
        }
    }
}

/// O nome do sistema no user agent.
fn nome_do_sistema() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        "android" => "Android",
        "ios" => "iOS",
        "horizon" => "Switch",
        "emscripten" => "Web",
        outro => outro,
    }
}

/// Uma string do C, copiada. Vazia para nulo.
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

/// Como [`texto`], com `None` para nulo e para vazio.
///
/// # Safety
/// Como em [`texto`].
unsafe fn talvez(p: *const c_char) -> Option<String> {
    // SAFETY: repassado.
    Some(unsafe { texto(p) }).filter(|t| !t.is_empty())
}

/// # Safety
/// `c` é nulo ou aponta para um `rc_client_achievement_t` válido.
unsafe fn conquista_de(c: *const rc::rc_client_achievement_t) -> Option<Conquista> {
    // SAFETY: pelo contrato acima.
    unsafe {
        let c = c.as_ref()?;
        Some(Conquista {
            id: c.id,
            titulo: texto(c.title),
            descricao: texto(c.description),
            pontos: c.points,
            icone: talvez(c.badge_url),
            icone_bloqueado: talvez(c.badge_locked_url),
            desbloqueada: c.state == rc::RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED,
            no_hardcore: c.unlocked & rc::RC_CLIENT_ACHIEVEMENT_UNLOCKED_HARDCORE != 0,
            progresso: texto(c.measured_progress.as_ptr()),
            percentual: c.measured_percent,
            sem_suporte: c.state == rc::RC_CLIENT_ACHIEVEMENT_STATE_DISABLED,
        })
    }
}

/// O `Estado` de um cliente.
///
/// # Safety
/// `cliente` é um dos nossos: o `userdata` dele é o `Estado` que o [`Conquistas`] criou.
unsafe fn estado_de<'a>(cliente: *const rc::rc_client_t) -> &'a mut Estado {
    // SAFETY: pelo contrato acima. Quem chama isto é um retorno do C, e o lado do Rust não
    // segura referência ao `Estado` enquanto o C roda.
    unsafe { &mut *rc::rc_client_get_userdata(cliente).cast::<Estado>() }
}

unsafe extern "C" fn le_memoria(
    endereco: u32,
    destino: *mut u8,
    quantos: u32,
    cliente: *mut rc::rc_client_t,
) -> u32 {
    // SAFETY: `cliente` é nosso; o `destino` tem `quantos` bytes, pelo contrato do header.
    unsafe {
        let estado = estado_de(cliente);
        let Some(memoria) = estado.memoria else {
            return 0;
        };
        let Some((guest, cabe)) = estado.mapa.traduz(endereco, quantos) else {
            return 0;
        };
        let destino = std::slice::from_raw_parts_mut(destino, cabe as usize);
        match (*memoria).le(guest, destino) {
            true => cabe,
            false => 0,
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
        let estado = estado_de(cliente);
        let Some(callback) = callback else {
            return;
        };
        let pedido = &*pedido;
        let pedido = Pedido {
            url: texto(pedido.url),
            corpo: talvez(pedido.post_data),
            tipo: talvez(pedido.content_type),
        };
        let pendente = estado.pendentes.anota(&pedido, agora());
        let entrega = Entrega {
            fila: Arc::clone(&estado.fila),
            retorno: Some(Retorno {
                callback,
                dados,
                pendente,
            }),
        };
        estado.servidor.pede(pedido, entrega);
    }
}

/// O relógio do mundo, em segundos desde 1970: o instante de um desbloqueio na fila.
fn agora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Remonta um pedido da fila pelo rcheevos: a URL, o corpo e a assinatura `v` são os dele.
fn monta(
    pendente: &pendentes::Pendente,
    usuario: &str,
    token: &str,
    host: Option<&CString>,
    agora: u64,
) -> Option<Pedido> {
    let usuario = CString::new(usuario).ok()?;
    let token = CString::new(token).ok()?;
    let hash = CString::new(pendente.hash.as_str()).ok()?;
    let atras = u32::try_from(agora.saturating_sub(pendente.quando)).unwrap_or(u32::MAX);
    let host = rc::rc_api_host_t {
        host: host.map_or(std::ptr::null(), |h| h.as_ptr()),
        media_host: std::ptr::null(),
    };
    // SAFETY: o pedido é montado e lido aqui, sem sair do lugar entre o `init` e o `destroy` (ver
    // [`rc::rc_api_request_t`]); as strings de entrada vivem até o fim da função.
    unsafe {
        let mut pedido = std::mem::MaybeUninit::<rc::rc_api_request_t>::zeroed();
        let resultado = match pendente.o_que {
            pendentes::OQue::Desbloqueio {
                conquista,
                hardcore,
            } => {
                let parametros = rc::rc_api_award_achievement_request_t {
                    username: usuario.as_ptr(),
                    api_token: token.as_ptr(),
                    achievement_id: conquista,
                    hardcore: u32::from(hardcore),
                    game_hash: hash.as_ptr(),
                    seconds_since_unlock: atras,
                };
                rc::rc_api_init_award_achievement_request_hosted(
                    pedido.as_mut_ptr(),
                    &parametros,
                    &host,
                )
            }
            pendentes::OQue::Placar { placar, valor } => {
                let parametros = rc::rc_api_submit_lboard_entry_request_t {
                    username: usuario.as_ptr(),
                    api_token: token.as_ptr(),
                    leaderboard_id: placar,
                    score: valor,
                    game_hash: hash.as_ptr(),
                    seconds_since_completion: atras,
                };
                rc::rc_api_init_submit_lboard_entry_request_hosted(
                    pedido.as_mut_ptr(),
                    &parametros,
                    &host,
                )
            }
        };
        let p = pedido.as_mut_ptr();
        let montado = (resultado == rc::RC_OK).then(|| Pedido {
            url: texto((*p).url),
            corpo: talvez((*p).post_data),
            tipo: talvez((*p).content_type),
        });
        rc::rc_api_destroy_request(p);
        montado
    }
}

/// O retorno dos pedidos mandados de novo pelo [`Conquistas`], e não pelo rcheevos: a resposta só
/// importa à fila, que o [`Conquistas::entrega_respostas`] já atualizou.
unsafe extern "C" fn ignora_resposta(_: *const rc::rc_api_server_response_t, _: *mut c_void) {}

unsafe extern "C" fn ao_entrar(
    resultado: c_int,
    mensagem: *const c_char,
    cliente: *mut rc::rc_client_t,
    _dados: *mut c_void,
) {
    // SAFETY: `cliente` é nosso, e a mensagem vale durante a chamada.
    unsafe {
        let evento = match resultado {
            rc::RC_OK => Ok(()),
            _ => Err(texto(mensagem)),
        };
        let estado = estado_de(cliente);
        // A fila da execução anterior vai ao servidor depois do login, que é quando há token.
        estado.reenviar |= evento.is_ok();
        estado.eventos.push(Evento::Entrou(evento));
    }
}

unsafe extern "C" fn ao_carregar(
    resultado: c_int,
    mensagem: *const c_char,
    cliente: *mut rc::rc_client_t,
    _dados: *mut c_void,
) {
    // SAFETY: como em `ao_entrar`.
    unsafe {
        let evento = match resultado {
            rc::RC_OK => Ok(()),
            rc::RC_NO_GAME_LOADED => Err(ErroAoCarregar::Desconhecido),
            rc::RC_ABORTED => Err(ErroAoCarregar::Cancelado),
            _ => Err(ErroAoCarregar::Outro(texto(mensagem))),
        };
        estado_de(cliente)
            .eventos
            .push(Evento::JogoCarregado(evento));
    }
}

unsafe extern "C" fn ao_evento(
    evento: *const rc::rc_client_event_t,
    cliente: *mut rc::rc_client_t,
) {
    // SAFETY: `cliente` é nosso, e o evento e o que ele aponta valem durante a chamada.
    unsafe {
        let e = &*evento;
        let placar = || {
            let p = e.leaderboard.as_ref();
            (
                p.map_or(0, |p| p.id),
                p.map(|p| texto(p.title)).unwrap_or_default(),
            )
        };
        let traduzido = match e.type_ {
            rc::RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED => {
                conquista_de(e.achievement).map(Evento::Desbloqueou)
            }
            rc::RC_CLIENT_EVENT_LEADERBOARD_STARTED => {
                let (id, titulo) = placar();
                Some(Evento::PlacarComecou { id, titulo })
            }
            rc::RC_CLIENT_EVENT_LEADERBOARD_FAILED => {
                let (id, titulo) = placar();
                Some(Evento::PlacarFalhou { id, titulo })
            }
            rc::RC_CLIENT_EVENT_LEADERBOARD_SUBMITTED => {
                let (id, titulo) = placar();
                let valor = e
                    .leaderboard
                    .as_ref()
                    .map(|p| texto(p.tracker_value))
                    .unwrap_or_default();
                Some(Evento::PlacarEnviado { id, titulo, valor })
            }
            rc::RC_CLIENT_EVENT_LEADERBOARD_TRACKER_SHOW
            | rc::RC_CLIENT_EVENT_LEADERBOARD_TRACKER_UPDATE => {
                e.leaderboard_tracker.as_ref().map(|t| Evento::Contador {
                    id: t.id,
                    texto: texto(t.display.as_ptr()),
                })
            }
            rc::RC_CLIENT_EVENT_LEADERBOARD_TRACKER_HIDE => e
                .leaderboard_tracker
                .as_ref()
                .map(|t| Evento::ContadorSumiu { id: t.id }),
            rc::RC_CLIENT_EVENT_RESET => Some(Evento::Reiniciar),
            rc::RC_CLIENT_EVENT_GAME_COMPLETED => Some(Evento::JogoCompleto),
            rc::RC_CLIENT_EVENT_SERVER_ERROR => {
                e.server_error.as_ref().map(|s| Evento::ErroDoServidor {
                    api: texto(s.api),
                    mensagem: texto(s.error_message),
                })
            }
            rc::RC_CLIENT_EVENT_DISCONNECTED => Some(Evento::Desconectado),
            rc::RC_CLIENT_EVENT_RECONNECTED => Some(Evento::Reconectado),
            // Os indicadores de desafio e de progresso ficaram para depois da primeira fase.
            _ => None,
        };
        if let Some(traduzido) = traduzido {
            estado_de(cliente).eventos.push(traduzido);
        }
    }
}

unsafe extern "C" fn registra(mensagem: *const c_char, _cliente: *const rc::rc_client_t) {
    // SAFETY: a mensagem vale durante a chamada.
    let mensagem = unsafe { texto(mensagem) };
    crate::registro!(
        crate::registro::Nivel::Informacao,
        "conquistas",
        "{mensagem}"
    );
}

#[cfg(test)]
pub(crate) mod testes;
