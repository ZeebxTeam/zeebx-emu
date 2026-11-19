/* Checks the structs that `lib.rs` copies by hand: each line asserts where a field is, with the same
 * arithmetic that `lib.rs` asserts on the Rust side. A header that changed stops the build on any
 * target, without running anything. The constants come from `build.rs`, from `constants.rs`. */

#include <stddef.h>

#include "rc_api_request.h"
#include "rc_client.h"
#include "rc_consoles.h"
#include "rc_error.h"

/* When false, the condition becomes an array of negative size. The `extern` declaration may repeat. */
#define ZEEBX_CHECK(condition) extern char zeebx_check[(condition) ? 1 : -1];

#define P sizeof(void*)

ZEEBX_CHECK(offsetof(rc_api_request_t, post_data) == P)

ZEEBX_CHECK(offsetof(rc_api_server_response_t, body_length) == P)
ZEEBX_CHECK(offsetof(rc_api_server_response_t, http_status_code) == 2 * P)
ZEEBX_CHECK(sizeof(rc_api_server_response_t) == 3 * P)

ZEEBX_CHECK(offsetof(rc_client_user_t, token) == 2 * P)
ZEEBX_CHECK(offsetof(rc_client_user_t, score_casual) == 3 * P + 4)

ZEEBX_CHECK(offsetof(rc_client_game_t, title) == 8)
ZEEBX_CHECK(offsetof(rc_client_game_t, badge_url) == 8 + 3 * P)

ZEEBX_CHECK(offsetof(rc_client_achievement_t, measured_progress) == 2 * P + 8)
ZEEBX_CHECK(offsetof(rc_client_achievement_t, id) == 2 * P + 36)
ZEEBX_CHECK(offsetof(rc_client_achievement_t, points) == 2 * P + 40)

/* The summary is allocated on the Rust side. */
ZEEBX_CHECK(offsetof(rc_client_user_game_summary_t, points_unlocked) == 20)
ZEEBX_CHECK(sizeof(rc_client_user_game_summary_t) <= 40)

ZEEBX_CHECK(offsetof(rc_client_event_t, achievement) == P)
