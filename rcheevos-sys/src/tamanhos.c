/* Os tamanhos e posições dos structs do rcheevos que o `lib.rs` copia à mão.
 *
 * Uma cópia errada não dá erro de compilação: lê o campo vizinho e segue. O teste
 * `os_structs_batem_com_o_c` compara cada número daqui com o `size_of`/`offset_of!` do Rust, no
 * alvo em que se compila. O `time_t` no meio do `rc_client_achievement_t` é o motivo principal:
 * ele tem 32 bits em alguns alvos e 64 em outros, e tudo o que vem depois dele muda de lugar. */

#include <stddef.h>

#include "rc_client.h"
#include "rc_api_request.h"
#include "rc_api_runtime.h"
#include "rc_consoles.h"
#include "rc_error.h"

#define ZEEBX_TAMANHO(tipo) size_t zeebx_rc_tamanho_##tipo(void) { return sizeof(tipo); }
#define ZEEBX_POSICAO(tipo, campo) size_t zeebx_rc_posicao_##tipo##_##campo(void) { return offsetof(tipo, campo); }

ZEEBX_TAMANHO(rc_api_request_t)
ZEEBX_POSICAO(rc_api_request_t, content_type)
ZEEBX_POSICAO(rc_api_request_t, buffer)
ZEEBX_TAMANHO(rc_buffer_t)
ZEEBX_POSICAO(rc_buffer_t, data)
ZEEBX_TAMANHO(rc_api_host_t)
ZEEBX_TAMANHO(rc_api_award_achievement_request_t)
ZEEBX_POSICAO(rc_api_award_achievement_request_t, seconds_since_unlock)
ZEEBX_TAMANHO(rc_api_submit_lboard_entry_request_t)
ZEEBX_POSICAO(rc_api_submit_lboard_entry_request_t, seconds_since_completion)

ZEEBX_TAMANHO(rc_api_server_response_t)
ZEEBX_POSICAO(rc_api_server_response_t, http_status_code)

ZEEBX_TAMANHO(rc_client_user_t)
ZEEBX_POSICAO(rc_client_user_t, num_unread_messages)
ZEEBX_POSICAO(rc_client_user_t, avatar_url)

ZEEBX_TAMANHO(rc_client_game_t)
ZEEBX_POSICAO(rc_client_game_t, badge_url)

ZEEBX_TAMANHO(rc_client_achievement_t)
ZEEBX_POSICAO(rc_client_achievement_t, measured_percent)
ZEEBX_POSICAO(rc_client_achievement_t, unlock_time)
ZEEBX_POSICAO(rc_client_achievement_t, state)
ZEEBX_POSICAO(rc_client_achievement_t, rarity)
ZEEBX_POSICAO(rc_client_achievement_t, type)
ZEEBX_POSICAO(rc_client_achievement_t, badge_url)
ZEEBX_POSICAO(rc_client_achievement_t, badge_locked_url)

ZEEBX_TAMANHO(rc_client_user_game_summary_t)
ZEEBX_POSICAO(rc_client_user_game_summary_t, points_unlocked)
ZEEBX_POSICAO(rc_client_user_game_summary_t, completed_time)

ZEEBX_TAMANHO(rc_client_achievement_bucket_t)
ZEEBX_POSICAO(rc_client_achievement_bucket_t, label)
ZEEBX_POSICAO(rc_client_achievement_bucket_t, bucket_type)

ZEEBX_TAMANHO(rc_client_achievement_list_t)
ZEEBX_POSICAO(rc_client_achievement_list_t, num_buckets)

ZEEBX_TAMANHO(rc_client_leaderboard_t)
ZEEBX_POSICAO(rc_client_leaderboard_t, id)
ZEEBX_POSICAO(rc_client_leaderboard_t, category)

ZEEBX_TAMANHO(rc_client_leaderboard_tracker_t)
ZEEBX_POSICAO(rc_client_leaderboard_tracker_t, id)

ZEEBX_TAMANHO(rc_client_server_error_t)
ZEEBX_POSICAO(rc_client_server_error_t, related_id)

ZEEBX_TAMANHO(rc_client_event_t)
ZEEBX_POSICAO(rc_client_event_t, achievement)
ZEEBX_POSICAO(rc_client_event_t, subset)

/* As constantes que o `lib.rs` copia. */
#define ZEEBX_VALOR(nome) int zeebx_rc_valor_##nome(void) { return (int)(nome); }

ZEEBX_VALOR(RC_OK)
ZEEBX_VALOR(RC_NO_GAME_LOADED)
ZEEBX_VALOR(RC_ABORTED)
ZEEBX_VALOR(RC_API_SERVER_RESPONSE_CLIENT_ERROR)
ZEEBX_VALOR(RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR)
ZEEBX_VALOR(RC_CLIENT_LOG_LEVEL_NONE)
ZEEBX_VALOR(RC_CLIENT_LOG_LEVEL_ERROR)
ZEEBX_VALOR(RC_CLIENT_LOG_LEVEL_WARN)
ZEEBX_VALOR(RC_CLIENT_LOG_LEVEL_INFO)
ZEEBX_VALOR(RC_CLIENT_LOG_LEVEL_VERBOSE)
ZEEBX_VALOR(RC_CLIENT_LOAD_GAME_STATE_NONE)
ZEEBX_VALOR(RC_CLIENT_LOAD_GAME_STATE_DONE)
ZEEBX_VALOR(RC_CLIENT_LOAD_GAME_STATE_ABORTED)
ZEEBX_VALOR(RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED)
ZEEBX_VALOR(RC_CLIENT_ACHIEVEMENT_STATE_DISABLED)
ZEEBX_VALOR(RC_CLIENT_ACHIEVEMENT_CATEGORY_PROMOTED)
ZEEBX_VALOR(RC_CLIENT_ACHIEVEMENT_LIST_GROUPING_LOCK_STATE)
ZEEBX_VALOR(RC_CLIENT_ACHIEVEMENT_UNLOCKED_HARDCORE)
ZEEBX_VALOR(RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_STARTED)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_FAILED)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_SUBMITTED)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_SHOW)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_HIDE)
ZEEBX_VALOR(RC_CLIENT_EVENT_LEADERBOARD_TRACKER_UPDATE)
ZEEBX_VALOR(RC_CLIENT_EVENT_RESET)
ZEEBX_VALOR(RC_CLIENT_EVENT_GAME_COMPLETED)
ZEEBX_VALOR(RC_CLIENT_EVENT_SERVER_ERROR)
ZEEBX_VALOR(RC_CLIENT_EVENT_DISCONNECTED)
ZEEBX_VALOR(RC_CLIENT_EVENT_RECONNECTED)
ZEEBX_VALOR(RC_CLIENT_LEADERBOARD_DISPLAY_SIZE)
ZEEBX_VALOR(RC_CONSOLE_ZEEBO)
