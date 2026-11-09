#ifndef ZEEBX_IOS_H
#define ZEEBX_IOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* What `zeebx_ios_step` returns. The numbers are the contract with Swift. */
#define ZEEBX_NO_SESSION (-1)
#define ZEEBX_FRAME 0
#define ZEEBX_RUNNING 1
#define ZEEBX_AHEAD 2
#define ZEEBX_STOPPED 3

/* Opaque pointer. Swift does not look inside. */
typedef struct ZeebxIos ZeebxIos;

/*
 * `documents` is the app's Documents folder: the ROMs live in `documents/roms`, which the
 * Files app can see. `support` is Library/Application Support: cache, saves and the virtual
 * device. Both are file paths, in UTF-8, zero-terminated.
 * Returns null if either pointer is null.
 */
ZeebxIos *zeebx_ios_create(const char *documents, const char *support);
void zeebx_ios_destroy(ZeebxIos *app);

/* How many games the last scan found. Scans the ROM folder again. */
int32_t zeebx_ios_scan(ZeebxIos *app);

/* Title of game `index`, valid until the next scan. Null if the index does not exist. */
const char *zeebx_ios_title(ZeebxIos *app, int32_t index);

/*
 * Starts game `index`. 0 when the session came up; -1 when it did not, and the reason stays in
 * `zeebx_ios_error` until the next call that replaces it.
 */
int32_t zeebx_ios_start(ZeebxIos *app, int32_t index);
void zeebx_ios_stop(ZeebxIos *app);

/*
 * One turn of the loop: delivers the controller, advances the game by the real time that has
 * passed and publishes the frame. The call belongs to the UI thread; it is not safe from
 * another thread.
 */
int32_t zeebx_ios_step(ZeebxIos *app);

/*
 * The current frame, in BGRA8, with the origin at the top-left corner. The pointer is valid
 * until the next `zeebx_ios_step` or `zeebx_ios_stop`. `width` and `height` may be null.
 * Returns null when there is no frame.
 */
const uint8_t *zeebx_ios_frame(ZeebxIos *app, uint32_t *width, uint32_t *height);

/* `index` is the position in the core's `BUTTON_NAMES` (`b1` is 0, `up` is 12). `pressed` is 0/1. */
void zeebx_ios_button(ZeebxIos *app, uint32_t index, int32_t pressed);

/* Joystick axis, centered on zero. `index` 0 is X, 1 is Y. The travel is -128..=128. */
void zeebx_ios_axis(ZeebxIos *app, uint32_t index, int32_t value);

/* -1 when the name is not a Zeebo button. */
int32_t zeebx_ios_button_by_name(const char *name);

/* Text of the last failure, or an empty string. Valid until the next call that replaces it. */

const char *zeebx_ios_error(ZeebxIos *app);

#ifdef __cplusplus
}
#endif

#endif
