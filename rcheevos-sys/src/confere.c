/* Confere os structs que o `lib.rs` copia à mão: cada linha afirma onde um campo está, com a mesma
 * conta que o `lib.rs` afirma do lado do Rust. Um header que mudou para a compilação em qualquer
 * alvo, sem rodar nada. As constantes vêm do `build.rs`, a partir do `constantes.rs`. */

#include <stddef.h>

#include "rc_api_request.h"
#include "rc_client.h"
#include "rc_consoles.h"
#include "rc_error.h"

/* Falsa, a condição vira um array de tamanho negativo. A declaração `extern` pode se repetir. */
#define ZEEBX_CONFERE(condicao) extern char zeebx_confere[(condicao) ? 1 : -1];

#define P sizeof(void*)

ZEEBX_CONFERE(offsetof(rc_api_request_t, post_data) == P)

ZEEBX_CONFERE(offsetof(rc_api_server_response_t, body_length) == P)
ZEEBX_CONFERE(offsetof(rc_api_server_response_t, http_status_code) == 2 * P)
ZEEBX_CONFERE(sizeof(rc_api_server_response_t) == 3 * P)

ZEEBX_CONFERE(offsetof(rc_client_user_t, token) == 2 * P)
ZEEBX_CONFERE(offsetof(rc_client_user_t, score_casual) == 3 * P + 4)

ZEEBX_CONFERE(offsetof(rc_client_game_t, title) == 8)
ZEEBX_CONFERE(offsetof(rc_client_game_t, badge_url) == 8 + 3 * P)

ZEEBX_CONFERE(offsetof(rc_client_achievement_t, measured_progress) == 2 * P + 8)
ZEEBX_CONFERE(offsetof(rc_client_achievement_t, id) == 2 * P + 36)
ZEEBX_CONFERE(offsetof(rc_client_achievement_t, points) == 2 * P + 40)

/* O resumo é alocado do lado do Rust. */
ZEEBX_CONFERE(offsetof(rc_client_user_game_summary_t, points_unlocked) == 20)
ZEEBX_CONFERE(sizeof(rc_client_user_game_summary_t) <= 40)

ZEEBX_CONFERE(offsetof(rc_client_event_t, achievement) == P)
