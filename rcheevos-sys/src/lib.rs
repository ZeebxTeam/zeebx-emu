//! Declarações do rcheevos, escritas à mão (o `bindgen` pediria o libclang em cada alvo), só com o
//! que o Zeebx chama. A posição dos campos é afirmada aqui e em `confere.c` com a mesma conta, e o
//! `build.rs` confere as constantes. Um struct "só o começo" nunca é alocado do lado do Rust.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_void};
use std::mem::{offset_of, size_of};

macro_rules! copia {
    ($($nome:ident: $tipo:ty = $valor:expr;)*) => {
        $(pub const $nome: $tipo = $valor;)*
    };
}
include!("constantes.rs");

const P: usize = size_of::<*const c_char>();

/// `rc_client_t`, opaco.
#[repr(C)]
pub struct rc_client_t {
    _privado: [u8; 0],
}

/// `rc_client_async_handle_t`, opaco.
#[repr(C)]
pub struct rc_client_async_handle_t {
    _privado: [u8; 0],
}

/// `rc_api_request_t` (`include/rc_api_request.h`), **só o começo**.
#[repr(C)]
pub struct rc_api_request_t {
    pub url: *const c_char,
    pub post_data: *const c_char,
}
const _: () = assert!(offset_of!(rc_api_request_t, post_data) == P);

/// `rc_api_server_response_t` (`include/rc_api_request.h`), inteiro: o Zeebx monta as respostas.
#[repr(C)]
pub struct rc_api_server_response_t {
    pub body: *const c_char,
    pub body_length: usize,
    pub http_status_code: c_int,
}
const _: () = assert!(offset_of!(rc_api_server_response_t, body_length) == P);
const _: () = assert!(offset_of!(rc_api_server_response_t, http_status_code) == 2 * P);
const _: () = assert!(size_of::<rc_api_server_response_t>() == 3 * P);

/// `rc_client_user_t` (`include/rc_client.h`), **só o começo**.
#[repr(C)]
pub struct rc_client_user_t {
    pub display_name: *const c_char,
    pub username: *const c_char,
    pub token: *const c_char,
    pub score: u32,
    pub score_casual: u32,
}
const _: () = assert!(offset_of!(rc_client_user_t, token) == 2 * P);
const _: () = assert!(offset_of!(rc_client_user_t, score_casual) == 3 * P + 4);

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
const _: () = assert!(offset_of!(rc_client_game_t, title) == 8);
const _: () = assert!(offset_of!(rc_client_game_t, badge_url) == 8 + 3 * P);

/// `rc_client_achievement_t` (`include/rc_client.h`), **só o começo**: para antes do `time_t`.
#[repr(C)]
pub struct rc_client_achievement_t {
    pub title: *const c_char,
    pub description: *const c_char,
    pub badge_name: [c_char; 8],
    pub measured_progress: [c_char; 24],
    pub measured_percent: f32,
    pub id: u32,
    pub points: u32,
}
const _: () = assert!(offset_of!(rc_client_achievement_t, measured_progress) == 2 * P + 8);
const _: () = assert!(offset_of!(rc_client_achievement_t, id) == 2 * P + 36);
const _: () = assert!(offset_of!(rc_client_achievement_t, points) == 2 * P + 40);

/// `rc_client_user_game_summary_t` (`include/rc_client.h`), com os dois `time_t` em 64 bits.
#[repr(C)]
#[derive(Default)]
pub struct rc_client_user_game_summary_t {
    pub num_promoted_achievements: u32,
    pub num_unpromoted_achievements: u32,
    pub num_unlocked_achievements: u32,
    pub num_unsupported_achievements: u32,
    pub points_available: u32,
    pub points_unlocked: u32,
    pub beaten_time: i64,
    pub completed_time: i64,
}
const _: () = assert!(offset_of!(rc_client_user_game_summary_t, points_unlocked) == 20);
const _: () = assert!(size_of::<rc_client_user_game_summary_t>() == 40);

/// `rc_client_event_t` (`include/rc_client.h`), **só o começo**.
#[repr(C)]
pub struct rc_client_event_t {
    pub type_: u32,
    pub achievement: *const rc_client_achievement_t,
}
const _: () = assert!(offset_of!(rc_client_event_t, achievement) == P);

pub type rc_client_read_memory_func_t =
    Option<unsafe extern "C" fn(u32, *mut u8, u32, *mut rc_client_t) -> u32>;
pub type rc_client_server_callback_t =
    Option<unsafe extern "C" fn(*const rc_api_server_response_t, *mut c_void)>;
pub type rc_client_server_call_t = Option<
    unsafe extern "C" fn(
        *const rc_api_request_t,
        rc_client_server_callback_t,
        *mut c_void,
        *mut rc_client_t,
    ),
>;
pub type rc_client_callback_t =
    Option<unsafe extern "C" fn(c_int, *const c_char, *mut rc_client_t, *mut c_void)>;
pub type rc_client_message_callback_t =
    Option<unsafe extern "C" fn(*const c_char, *const rc_client_t)>;
pub type rc_client_event_handler_t =
    Option<unsafe extern "C" fn(*const rc_client_event_t, *mut rc_client_t)>;

unsafe extern "C" {
    // include/rc_hash.h
    pub fn rc_hash_generate_from_buffer(
        hash: *mut c_char,
        console_id: u32,
        buffer: *const u8,
        buffer_size: usize,
    ) -> c_int;

    // include/rc_client.h
    pub fn rc_client_create(
        read_memory_function: rc_client_read_memory_func_t,
        server_call_function: rc_client_server_call_t,
    ) -> *mut rc_client_t;
    pub fn rc_client_destroy(client: *mut rc_client_t);
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
    pub fn rc_client_set_allow_background_memory_reads(client: *mut rc_client_t, allowed: c_int);
    pub fn rc_client_set_hardcore_enabled(client: *mut rc_client_t, enabled: c_int);

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

    pub fn rc_client_begin_load_game(
        client: *mut rc_client_t,
        hash: *const c_char,
        callback: rc_client_callback_t,
        callback_userdata: *mut c_void,
    ) -> *mut rc_client_async_handle_t;
    pub fn rc_client_unload_game(client: *mut rc_client_t);
    pub fn rc_client_reset(client: *mut rc_client_t);
    pub fn rc_client_get_game_info(client: *const rc_client_t) -> *const rc_client_game_t;
    pub fn rc_client_get_user_game_summary(
        client: *const rc_client_t,
        summary: *mut rc_client_user_game_summary_t,
    );
    pub fn rc_client_achievement_get_image_url(
        achievement: *const rc_client_achievement_t,
        state: c_int,
        buffer: *mut c_char,
        buffer_size: usize,
    ) -> c_int;

    pub fn rc_client_do_frame(client: *mut rc_client_t);
    pub fn rc_client_idle(client: *mut rc_client_t);
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
