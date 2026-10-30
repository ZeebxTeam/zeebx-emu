// As constantes copiadas do rcheevos. O `lib.rs` as declara, e o `build.rs` confere cada uma contra
// o header, no compilador de C.
copia! {
    // include/rc_error.h
    RC_OK: c_int = 0;
    RC_NO_GAME_LOADED: c_int = -29;
    RC_ABORTED: c_int = -31;

    // include/rc_api_request.h
    RC_API_SERVER_RESPONSE_CLIENT_ERROR: c_int = -1;
    RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR: c_int = -2;

    // include/rc_client.h
    RC_CLIENT_LOG_LEVEL_INFO: c_int = 3;
    RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED: c_int = 2;
    RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED: u32 = 1;

    // include/rc_consoles.h
    RC_CONSOLE_ZEEBO: u32 = 70;
}
