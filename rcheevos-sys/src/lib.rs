//! Declarações do rcheevos, escritas à mão.
//!
//! **Sem `bindgen`.** Ele precisa do libclang em cada máquina que compila, e o Zeebx compila para
//! Windows, macOS, Android, iOS e o Switch: hoje só o `coreaudio-sys` do macOS o puxa. Aqui entra
//! só o que o Zeebx chama, e cada declaração copia a do header citado. Uma função nova entra aqui
//! junto com o primeiro uso dela.
//!
//! **Os structs copiados são conferidos contra o C.** O `src/tamanhos.c` compila junto e devolve
//! o `sizeof` e o `offsetof` de cada um; o teste `os_structs_batem_com_o_c` compara. Um struct
//! marcado "só o começo" copia os campos até o último que o Zeebx lê e nunca é alocado do lado do
//! Rust — só lido por ponteiro que o rcheevos entregou —, e por isso pode parar antes do fim.
//!
//! O `RC_CCONV` dos headers é `__cdecl` no Windows de 32 bits e vazio no resto: em todo alvo do
//! Zeebx ele é a convenção de `extern "C"`.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};

/// O `time_t` do C. Tem 64 bits em todo alvo do Zeebx (Linux, Windows, macOS, Android e iOS de 64
/// bits, o Switch e o Emscripten); um alvo de 32 bits com `time_t` de 32 faria o teste de layout
/// falhar antes de alguém ler um campo errado.
pub type time_t = i64;

/// `RC_CONSOLE_ZEEBO`, de `include/rc_consoles.h`. O número é do rcheevos oficial desde a v10.3.0;
/// não é provisório.
pub const RC_CONSOLE_ZEEBO: u32 = 70;

// include/rc_error.h
pub const RC_OK: c_int = 0;
pub const RC_NO_GAME_LOADED: c_int = -29;
pub const RC_ABORTED: c_int = -31;

// include/rc_api_request.h
pub const RC_API_SERVER_RESPONSE_CLIENT_ERROR: c_int = -1;
pub const RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR: c_int = -2;

// include/rc_client.h — níveis de registro
pub const RC_CLIENT_LOG_LEVEL_NONE: c_int = 0;
pub const RC_CLIENT_LOG_LEVEL_ERROR: c_int = 1;
pub const RC_CLIENT_LOG_LEVEL_WARN: c_int = 2;
pub const RC_CLIENT_LOG_LEVEL_INFO: c_int = 3;
pub const RC_CLIENT_LOG_LEVEL_VERBOSE: c_int = 4;

// include/rc_client.h — estados do carregamento do jogo
pub const RC_CLIENT_LOAD_GAME_STATE_NONE: c_int = 0;
pub const RC_CLIENT_LOAD_GAME_STATE_DONE: c_int = 5;
pub const RC_CLIENT_LOAD_GAME_STATE_ABORTED: c_int = 6;

// include/rc_client.h — conquistas
pub const RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED: u8 = 2;
pub const RC_CLIENT_ACHIEVEMENT_STATE_DISABLED: u8 = 3;
pub const RC_CLIENT_ACHIEVEMENT_CATEGORY_PROMOTED: c_int = 1;
pub const RC_CLIENT_ACHIEVEMENT_LIST_GROUPING_LOCK_STATE: c_int = 0;
pub const RC_CLIENT_ACHIEVEMENT_UNLOCKED_HARDCORE: u8 = 2;

// include/rc_client.h — eventos
pub const RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED: u32 = 1;
pub const RC_CLIENT_EVENT_LEADERBOARD_STARTED: u32 = 2;
pub const RC_CLIENT_EVENT_LEADERBOARD_FAILED: u32 = 3;
pub const RC_CLIENT_EVENT_LEADERBOARD_SUBMITTED: u32 = 4;
pub const RC_CLIENT_EVENT_LEADERBOARD_TRACKER_SHOW: u32 = 10;
pub const RC_CLIENT_EVENT_LEADERBOARD_TRACKER_HIDE: u32 = 11;
pub const RC_CLIENT_EVENT_LEADERBOARD_TRACKER_UPDATE: u32 = 12;
pub const RC_CLIENT_EVENT_RESET: u32 = 14;
pub const RC_CLIENT_EVENT_GAME_COMPLETED: u32 = 15;
pub const RC_CLIENT_EVENT_SERVER_ERROR: u32 = 16;
pub const RC_CLIENT_EVENT_DISCONNECTED: u32 = 17;
pub const RC_CLIENT_EVENT_RECONNECTED: u32 = 18;

/// `rc_client_t`, opaco (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_t {
    _privado: [u8; 0],
}

/// `rc_client_async_handle_t`, opaco (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_async_handle_t {
    _privado: [u8; 0],
}

/// `rc_buffer_chunk_t` (`include/rc_util.h`).
#[repr(C)]
pub struct rc_buffer_chunk_t {
    pub write: *mut u8,
    pub end: *mut u8,
    pub start: *mut u8,
    pub next: *mut rc_buffer_chunk_t,
}

/// `rc_buffer_t` (`include/rc_util.h`).
#[repr(C)]
pub struct rc_buffer_t {
    pub chunk: rc_buffer_chunk_t,
    pub data: [u8; 256],
}

/// `rc_api_request_t` (`include/rc_api_request.h`), inteiro: o Zeebx monta pedidos com ele.
///
/// **Não pode mudar de lugar entre o `rc_api_init_*` e o [`rc_api_destroy_request`].** O
/// `rc_buffer_t` aponta para o próprio `data`, e as strings moram ali: um `rc_api_request_t`
/// movido aponta para a memória de onde ele estava.
#[repr(C)]
pub struct rc_api_request_t {
    pub url: *const c_char,
    pub post_data: *const c_char,
    pub content_type: *const c_char,
    pub buffer: rc_buffer_t,
}

/// `rc_api_host_t` (`include/rc_api_request.h`). Nulo é o servidor oficial.
#[repr(C)]
pub struct rc_api_host_t {
    pub host: *const c_char,
    pub media_host: *const c_char,
}

/// `rc_api_award_achievement_request_t` (`include/rc_api_runtime.h`).
#[repr(C)]
pub struct rc_api_award_achievement_request_t {
    pub username: *const c_char,
    pub api_token: *const c_char,
    pub achievement_id: u32,
    pub hardcore: u32,
    pub game_hash: *const c_char,
    pub seconds_since_unlock: u32,
}

/// `rc_api_submit_lboard_entry_request_t` (`include/rc_api_runtime.h`).
#[repr(C)]
pub struct rc_api_submit_lboard_entry_request_t {
    pub username: *const c_char,
    pub api_token: *const c_char,
    pub leaderboard_id: u32,
    pub score: i32,
    pub game_hash: *const c_char,
    pub seconds_since_completion: u32,
}

/// `rc_api_server_response_t` (`include/rc_api_request.h`).
#[repr(C)]
pub struct rc_api_server_response_t {
    pub body: *const c_char,
    pub body_length: usize,
    pub http_status_code: c_int,
}

/// `rc_client_user_t` (`include/rc_client.h`), **só o começo**: falta o `avatar_last_updated`.
#[repr(C)]
pub struct rc_client_user_t {
    pub display_name: *const c_char,
    pub username: *const c_char,
    pub token: *const c_char,
    pub score: u32,
    pub score_casual: u32,
    pub num_unread_messages: u32,
    pub avatar_url: *const c_char,
}

/// `rc_client_game_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_game_t {
    pub id: u32,
    pub console_id: u32,
    pub title: *const c_char,
    pub hash: *const c_char,
    pub badge_name: *const c_char,
    pub badge_url: *const c_char,
}

/// `rc_client_achievement_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_achievement_t {
    pub title: *const c_char,
    pub description: *const c_char,
    pub badge_name: [c_char; 8],
    pub measured_progress: [c_char; 24],
    pub measured_percent: f32,
    pub id: u32,
    pub points: u32,
    pub unlock_time: time_t,
    pub state: u8,
    pub category: u8,
    pub bucket: u8,
    pub unlocked: u8,
    pub rarity: f32,
    pub rarity_hardcore: f32,
    pub type_: u8,
    pub badge_url: *const c_char,
    pub badge_locked_url: *const c_char,
}

/// `rc_client_user_game_summary_t` (`include/rc_client.h`). Inteiro: o rcheevos escreve nele.
#[repr(C)]
#[derive(Default)]
pub struct rc_client_user_game_summary_t {
    pub num_promoted_achievements: u32,
    pub num_unpromoted_achievements: u32,
    pub num_unlocked_achievements: u32,
    pub num_unsupported_achievements: u32,
    pub points_available: u32,
    pub points_unlocked: u32,
    pub beaten_time: time_t,
    pub completed_time: time_t,
}

/// `rc_client_achievement_bucket_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_achievement_bucket_t {
    pub achievements: *const *const rc_client_achievement_t,
    pub num_achievements: u32,
    pub label: *const c_char,
    pub subset_id: u32,
    pub bucket_type: u8,
}

/// `rc_client_achievement_list_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_achievement_list_t {
    pub buckets: *const rc_client_achievement_bucket_t,
    pub num_buckets: u32,
}

/// `rc_client_leaderboard_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_leaderboard_t {
    pub title: *const c_char,
    pub description: *const c_char,
    pub tracker_value: *const c_char,
    pub id: u32,
    pub state: u8,
    pub format: u8,
    pub lower_is_better: u8,
    pub category: u8,
}

/// `RC_CLIENT_LEADERBOARD_DISPLAY_SIZE`.
pub const RC_CLIENT_LEADERBOARD_DISPLAY_SIZE: usize = 24;

/// `rc_client_leaderboard_tracker_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_leaderboard_tracker_t {
    pub display: [c_char; RC_CLIENT_LEADERBOARD_DISPLAY_SIZE],
    pub id: u32,
}

/// `rc_client_server_error_t` (`include/rc_client.h`).
#[repr(C)]
pub struct rc_client_server_error_t {
    pub error_message: *const c_char,
    pub api: *const c_char,
    pub result: c_int,
    pub related_id: u32,
}

/// `rc_client_event_t` (`include/rc_client.h`). Os ponteiros que o Zeebx não lê ficam opacos.
#[repr(C)]
pub struct rc_client_event_t {
    pub type_: u32,
    pub achievement: *mut rc_client_achievement_t,
    pub leaderboard: *mut rc_client_leaderboard_t,
    pub leaderboard_tracker: *mut rc_client_leaderboard_tracker_t,
    pub leaderboard_scoreboard: *mut c_void,
    pub server_error: *mut rc_client_server_error_t,
    pub subset: *mut c_void,
}

/// `rc_client_read_memory_func_t`.
pub type rc_client_read_memory_func_t = Option<
    unsafe extern "C" fn(
        address: u32,
        buffer: *mut u8,
        num_bytes: u32,
        client: *mut rc_client_t,
    ) -> u32,
>;

/// `rc_client_server_callback_t`.
pub type rc_client_server_callback_t = Option<
    unsafe extern "C" fn(
        server_response: *const rc_api_server_response_t,
        callback_data: *mut c_void,
    ),
>;

/// `rc_client_server_call_t`.
pub type rc_client_server_call_t = Option<
    unsafe extern "C" fn(
        request: *const rc_api_request_t,
        callback: rc_client_server_callback_t,
        callback_data: *mut c_void,
        client: *mut rc_client_t,
    ),
>;

/// `rc_client_callback_t`.
pub type rc_client_callback_t = Option<
    unsafe extern "C" fn(
        result: c_int,
        error_message: *const c_char,
        client: *mut rc_client_t,
        userdata: *mut c_void,
    ),
>;

/// `rc_client_message_callback_t`.
pub type rc_client_message_callback_t =
    Option<unsafe extern "C" fn(message: *const c_char, client: *const rc_client_t)>;

/// `rc_client_event_handler_t`.
pub type rc_client_event_handler_t =
    Option<unsafe extern "C" fn(event: *const rc_client_event_t, client: *mut rc_client_t)>;

unsafe extern "C" {
    // include/rc_consoles.h
    pub fn rc_console_name(console_id: u32) -> *const c_char;

    // include/rc_api_request.h e include/rc_api_runtime.h
    pub fn rc_api_destroy_request(request: *mut rc_api_request_t);
    pub fn rc_api_init_award_achievement_request_hosted(
        request: *mut rc_api_request_t,
        api_params: *const rc_api_award_achievement_request_t,
        host: *const rc_api_host_t,
    ) -> c_int;
    pub fn rc_api_init_submit_lboard_entry_request_hosted(
        request: *mut rc_api_request_t,
        api_params: *const rc_api_submit_lboard_entry_request_t,
        host: *const rc_api_host_t,
    ) -> c_int;

    // include/rc_hash.h
    pub fn rc_hash_generate_from_buffer(
        hash: *mut c_char,
        console_id: u32,
        buffer: *const u8,
        buffer_size: usize,
    ) -> c_int;

    // include/rc_client.h — ciclo de vida e configuração
    pub fn rc_client_create(
        read_memory_function: rc_client_read_memory_func_t,
        server_call_function: rc_client_server_call_t,
    ) -> *mut rc_client_t;
    pub fn rc_client_destroy(client: *mut rc_client_t);
    pub fn rc_client_set_hardcore_enabled(client: *mut rc_client_t, enabled: c_int);
    pub fn rc_client_get_hardcore_enabled(client: *const rc_client_t) -> c_int;
    pub fn rc_client_set_userdata(client: *mut rc_client_t, userdata: *mut c_void);
    pub fn rc_client_get_userdata(client: *const rc_client_t) -> *mut c_void;
    pub fn rc_client_set_host(client: *mut rc_client_t, hostname: *const c_char);
    pub fn rc_client_get_user_agent_clause(
        client: *mut rc_client_t,
        buffer: *mut c_char,
        buffer_size: usize,
    ) -> usize;
    pub fn rc_client_enable_logging(
        client: *mut rc_client_t,
        level: c_int,
        callback: rc_client_message_callback_t,
    );
    pub fn rc_client_set_event_handler(
        client: *mut rc_client_t,
        handler: rc_client_event_handler_t,
    );

    // include/rc_client.h — usuário
    pub fn rc_client_begin_login_with_password(
        client: *mut rc_client_t,
        username: *const c_char,
        password: *const c_char,
        callback: rc_client_callback_t,
        callback_userdata: *mut c_void,
    ) -> *mut rc_client_async_handle_t;
    pub fn rc_client_begin_login_with_token(
        client: *mut rc_client_t,
        username: *const c_char,
        token: *const c_char,
        callback: rc_client_callback_t,
        callback_userdata: *mut c_void,
    ) -> *mut rc_client_async_handle_t;
    pub fn rc_client_logout(client: *mut rc_client_t);
    pub fn rc_client_get_user_info(client: *const rc_client_t) -> *const rc_client_user_t;

    // include/rc_client.h — jogo
    pub fn rc_client_begin_load_game(
        client: *mut rc_client_t,
        hash: *const c_char,
        callback: rc_client_callback_t,
        callback_userdata: *mut c_void,
    ) -> *mut rc_client_async_handle_t;
    pub fn rc_client_get_load_game_state(client: *const rc_client_t) -> c_int;
    pub fn rc_client_is_game_loaded(client: *const rc_client_t) -> c_int;
    pub fn rc_client_unload_game(client: *mut rc_client_t);
    pub fn rc_client_get_game_info(client: *const rc_client_t) -> *const rc_client_game_t;
    pub fn rc_client_get_user_game_summary(
        client: *const rc_client_t,
        summary: *mut rc_client_user_game_summary_t,
    );
    pub fn rc_client_create_achievement_list(
        client: *mut rc_client_t,
        category: c_int,
        grouping: c_int,
    ) -> *mut rc_client_achievement_list_t;
    pub fn rc_client_destroy_achievement_list(list: *mut rc_client_achievement_list_t);
    pub fn rc_client_get_achievement_info(
        client: *mut rc_client_t,
        id: u32,
    ) -> *const rc_client_achievement_t;

    // include/rc_client.h — processamento
    pub fn rc_client_set_allow_background_memory_reads(client: *mut rc_client_t, allowed: c_int);
    pub fn rc_client_do_frame(client: *mut rc_client_t);
    pub fn rc_client_idle(client: *mut rc_client_t);
    pub fn rc_client_reset(client: *mut rc_client_t);
    pub fn rc_client_progress_size(client: *mut rc_client_t) -> usize;
    pub fn rc_client_serialize_progress_sized(
        client: *mut rc_client_t,
        buffer: *mut u8,
        buffer_size: usize,
    ) -> c_int;
    pub fn rc_client_deserialize_progress_sized(
        client: *mut rc_client_t,
        serialized: *const u8,
        serialized_size: usize,
    ) -> c_int;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::mem::{offset_of, size_of};

    /// Prova que a biblioteca foi compilada e ligada, e que o número do console é o do header.
    #[test]
    fn o_console_70_e_o_zeebo() {
        let nome = unsafe { CStr::from_ptr(rc_console_name(RC_CONSOLE_ZEEBO)) };
        assert_eq!(nome.to_str().unwrap(), "Zeebo");
    }

    /// O trecho que o rcheevos acrescenta ao user agent diz a versão dele. O RA cobra um user
    /// agent único, e este trecho é a metade que não é do Zeebx.
    #[test]
    fn o_cliente_nasce_e_diz_a_versao_do_rcheevos() {
        unsafe {
            let cliente = rc_client_create(None, None);
            assert!(!cliente.is_null());
            let mut texto = [0 as c_char; 64];
            let n = rc_client_get_user_agent_clause(cliente, texto.as_mut_ptr(), texto.len());
            let clausula = CStr::from_ptr(texto.as_ptr()).to_str().unwrap();
            assert_eq!(n, clausula.len());
            assert!(clausula.starts_with("rcheevos/"), "{clausula}");
            rc_client_destroy(cliente);
        }
    }

    macro_rules! confere {
        (tamanho $tipo:ident) => {{
            unsafe extern "C" {
                #[link_name = concat!("zeebx_rc_tamanho_", stringify!($tipo))]
                fn c() -> usize;
            }
            assert_eq!(size_of::<$tipo>(), unsafe { c() }, "sizeof({})", stringify!($tipo));
        }};
        (posicao $tipo:ident . $campo:ident = $campo_c:ident) => {{
            unsafe extern "C" {
                #[link_name = concat!(
                    "zeebx_rc_posicao_",
                    stringify!($tipo),
                    "_",
                    stringify!($campo_c)
                )]
                fn c() -> usize;
            }
            assert_eq!(
                offset_of!($tipo, $campo),
                unsafe { c() },
                "offsetof({}, {})",
                stringify!($tipo),
                stringify!($campo_c)
            );
        }};
        (posicao $tipo:ident . $campo:ident) => {
            confere!(posicao $tipo . $campo = $campo)
        };
    }

    /// As constantes copiadas, contra o valor que o C enxerga.
    #[test]
    fn as_constantes_batem_com_o_c() {
        macro_rules! constante {
            ($nome:ident) => {{
                unsafe extern "C" {
                    #[link_name = concat!("zeebx_rc_valor_", stringify!($nome))]
                    fn c() -> c_int;
                }
                assert_eq!($nome as c_int, unsafe { c() }, "{}", stringify!($nome));
            }};
        }
        constante!(RC_OK);
        constante!(RC_NO_GAME_LOADED);
        constante!(RC_ABORTED);
        constante!(RC_API_SERVER_RESPONSE_CLIENT_ERROR);
        constante!(RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR);
        constante!(RC_CLIENT_LOG_LEVEL_NONE);
        constante!(RC_CLIENT_LOG_LEVEL_ERROR);
        constante!(RC_CLIENT_LOG_LEVEL_WARN);
        constante!(RC_CLIENT_LOG_LEVEL_INFO);
        constante!(RC_CLIENT_LOG_LEVEL_VERBOSE);
        constante!(RC_CLIENT_LOAD_GAME_STATE_NONE);
        constante!(RC_CLIENT_LOAD_GAME_STATE_DONE);
        constante!(RC_CLIENT_LOAD_GAME_STATE_ABORTED);
        constante!(RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED);
        constante!(RC_CLIENT_ACHIEVEMENT_STATE_DISABLED);
        constante!(RC_CLIENT_ACHIEVEMENT_CATEGORY_PROMOTED);
        constante!(RC_CLIENT_ACHIEVEMENT_LIST_GROUPING_LOCK_STATE);
        constante!(RC_CLIENT_ACHIEVEMENT_UNLOCKED_HARDCORE);
        constante!(RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_STARTED);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_FAILED);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_SUBMITTED);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_SHOW);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_HIDE);
        constante!(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_UPDATE);
        constante!(RC_CLIENT_EVENT_RESET);
        constante!(RC_CLIENT_EVENT_GAME_COMPLETED);
        constante!(RC_CLIENT_EVENT_SERVER_ERROR);
        constante!(RC_CLIENT_EVENT_DISCONNECTED);
        constante!(RC_CLIENT_EVENT_RECONNECTED);
        constante!(RC_CLIENT_LEADERBOARD_DISPLAY_SIZE);
        constante!(RC_CONSOLE_ZEEBO);
    }

    #[test]
    fn os_structs_batem_com_o_c() {
        confere!(tamanho rc_api_request_t);
        confere!(posicao rc_api_request_t.content_type);
        confere!(posicao rc_api_request_t.buffer);
        confere!(tamanho rc_buffer_t);
        confere!(posicao rc_buffer_t.data);
        confere!(tamanho rc_api_host_t);
        confere!(tamanho rc_api_award_achievement_request_t);
        confere!(posicao rc_api_award_achievement_request_t.seconds_since_unlock);
        confere!(tamanho rc_api_submit_lboard_entry_request_t);
        confere!(posicao rc_api_submit_lboard_entry_request_t.seconds_since_completion);
        confere!(tamanho rc_api_server_response_t);
        confere!(posicao rc_api_server_response_t.http_status_code);
        confere!(posicao rc_client_user_t.num_unread_messages);
        confere!(posicao rc_client_user_t.avatar_url);
        confere!(tamanho rc_client_game_t);
        confere!(posicao rc_client_game_t.badge_url);
        confere!(tamanho rc_client_achievement_t);
        confere!(posicao rc_client_achievement_t.measured_percent);
        confere!(posicao rc_client_achievement_t.unlock_time);
        confere!(posicao rc_client_achievement_t.state);
        confere!(posicao rc_client_achievement_t.rarity);
        confere!(posicao rc_client_achievement_t.type_ = type);
        confere!(posicao rc_client_achievement_t.badge_url);
        confere!(posicao rc_client_achievement_t.badge_locked_url);
        confere!(tamanho rc_client_user_game_summary_t);
        confere!(posicao rc_client_user_game_summary_t.points_unlocked);
        confere!(posicao rc_client_user_game_summary_t.completed_time);
        confere!(tamanho rc_client_achievement_bucket_t);
        confere!(posicao rc_client_achievement_bucket_t.label);
        confere!(posicao rc_client_achievement_bucket_t.bucket_type);
        confere!(tamanho rc_client_achievement_list_t);
        confere!(posicao rc_client_achievement_list_t.num_buckets);
        confere!(tamanho rc_client_leaderboard_t);
        confere!(posicao rc_client_leaderboard_t.id);
        confere!(posicao rc_client_leaderboard_t.category);
        confere!(tamanho rc_client_leaderboard_tracker_t);
        confere!(posicao rc_client_leaderboard_tracker_t.id);
        confere!(tamanho rc_client_server_error_t);
        confere!(posicao rc_client_server_error_t.related_id);
        confere!(tamanho rc_client_event_t);
        confere!(posicao rc_client_event_t.achievement);
        confere!(posicao rc_client_event_t.subset);
    }
}
