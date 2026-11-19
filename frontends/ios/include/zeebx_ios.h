#ifndef ZEEBX_IOS_H
#define ZEEBX_IOS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * The border between Swift and the Rust library. Every call belongs to the UI thread.
 *
 * Two kinds of text cross it. A `const char *` is borrowed: it stays valid until the next call
 * that the comment names. A `char *` is owned: Swift copies it and hands it back to
 * `zeebx_ios_free`. Both are UTF-8, zero-terminated.
 */

/*
 * What `zeebx_ios_step` returns. The numbers are the contract with Swift. No session is the only
 * negative one.
 */
#define ZEEBX_NO_SESSION (-1)
#define ZEEBX_FRAME 0
#define ZEEBX_RUNNING 1
#define ZEEBX_AHEAD 2
#define ZEEBX_STOPPED 3

/*
 * The physical controller's buttons: each is the position of its bit in the mask
 * `zeebx_ios_controller` takes, so South pressed is `1 << ZEEBX_PAD_SOUTH`. The order is
 * `controller::NAMES` on the Rust side, and the names are the ones desktop stores in
 * `settings.json`.
 *
 * Plain numbers, and not the masks themselves, because a plain number is the only kind of macro
 * Swift is sure to import.
 */
#define ZEEBX_PAD_SOUTH 0
#define ZEEBX_PAD_EAST 1
#define ZEEBX_PAD_WEST 2
#define ZEEBX_PAD_NORTH 3
#define ZEEBX_PAD_LEFT_TRIGGER 4
#define ZEEBX_PAD_RIGHT_TRIGGER 5
#define ZEEBX_PAD_LEFT_TRIGGER2 6
#define ZEEBX_PAD_RIGHT_TRIGGER2 7
#define ZEEBX_PAD_LEFT_THUMB 8
#define ZEEBX_PAD_RIGHT_THUMB 9
#define ZEEBX_PAD_START 10
#define ZEEBX_PAD_SELECT 11
#define ZEEBX_PAD_MODE 12
#define ZEEBX_PAD_DPAD_UP 13
#define ZEEBX_PAD_DPAD_DOWN 14
#define ZEEBX_PAD_DPAD_LEFT 15
#define ZEEBX_PAD_DPAD_RIGHT 16

/* The kinds of piece of the on-screen controls: the position in the core's `TouchControl::ALL`. */
#define ZEEBX_PIECE_DPAD 0
#define ZEEBX_PIECE_LEFT_STICK 1
#define ZEEBX_PIECE_RIGHT_STICK 2

/* The phases of a touch, for `zeebx_ios_touch` and `zeebx_ios_editor_touch`. */
#define ZEEBX_TOUCH_DOWN 0
#define ZEEBX_TOUCH_MOVED 1
#define ZEEBX_TOUCH_UP 2

/* Opaque pointer. Swift does not look inside. */
typedef struct ZeebxIos ZeebxIos;

/* What the game screen needs after each turn. */
typedef struct ZeebxView {
    /* Changes when the frame's pixels change: only then is the image rebuilt. */
    uint32_t frame_serial;
    /* The size the frame is drawn at, in points, centered on the screen. */
    float frame_width;
    float frame_height;
    /* Whether enlarging interpolates. */
    int32_t smooth;
    /* Changes when the on-screen controls would be drawn differently: only then are they asked
     * for again and redrawn. */
    uint32_t overlay_revision;
    /* From 0 to 100. */
    int32_t overlay_opacity;
} ZeebxView;

/* A piece of the on-screen controls, placed and ready to draw, in the screen's points. */
typedef struct ZeebxElement {
    int32_t kind;
    float center_x;
    float center_y;
    float half_width;
    float half_height;
    /* A circle, or a pill. */
    int32_t round;
    /* Whether it is pressed. On the D-pad, one bit per arm: up, down, left, right. */
    uint32_t pressed;
    /* Where a stick's knob is, from -1 to 1, with y growing downward. */
    float knob_x;
    float knob_y;
    /* The layout editor's selection. */
    int32_t highlighted;
} ZeebxElement;

/*
 * `documents` is the app's Documents folder: the ROMs live in `documents/roms`, which the
 * Files app can see. `support` is Library/Application Support: settings, cache, saves, save
 * states and the virtual device. `language` is the device's, like `pt-BR`; it only counts until
 * the player chooses one. Returns null if either folder is null or empty.
 *
 * The first scan of the ROM folder starts here, in the background.
 */
ZeebxIos *zeebx_ios_create(const char *documents, const char *support, const char *language);
void zeebx_ios_destroy(ZeebxIos *app);

/* Gives back a `char *` this library returned. Null is fine. */
void zeebx_ios_free(char *text);

/* The interface text of `key`, in the language in use. Owned. */
char *zeebx_ios_tr(ZeebxIos *app, const char *key);

/*
 * Moves whenever something the interface shows may have changed: a setting, the language, the
 * scan, a controller connecting. Comparing it is cheaper than asking for everything again.
 */
uint32_t zeebx_ios_revision(ZeebxIos *app);

/* Scans the ROM folder again, in the background. Does nothing if a scan is already running. */
void zeebx_ios_scan(ZeebxIos *app);

/* 1 when a scan finished and the list changed in this call, 0 otherwise. */
int32_t zeebx_ios_scan_poll(ZeebxIos *app);

/* `{"scanning": bool, "games": [{"title": string, "cover": bool}]}`. Owned. */
char *zeebx_ios_library_json(ZeebxIos *app);

/*
 * The cover of game `index`, in premultiplied BGRA8, origin at the top-left corner. Valid until
 * the next `zeebx_ios_cover` or `zeebx_ios_slot_thumbnail`. Null if the game has none.
 */
const uint8_t *zeebx_ios_cover(ZeebxIos *app, int32_t index, uint32_t *width, uint32_t *height);

/*
 * Starts game `index`. 0 when the session came up; -1 when it did not, and the reason stays in
 * `zeebx_ios_error` until the next call that replaces it.
 */
int32_t zeebx_ios_start(ZeebxIos *app, int32_t index);
void zeebx_ios_stop(ZeebxIos *app);

/* Closes the open game and opens it again from zero. 0, or -1 with the reason in the error. */
int32_t zeebx_ios_restart(ZeebxIos *app);

/*
 * One turn of the loop: delivers the input, advances the game by the real time that has passed
 * and publishes the frame. With the menu open or the game paused it advances nothing, and is
 * still the call that keeps the texts and the frame current.
 *
 * `ZEEBX_STOPPED` means the game is over and closed. `zeebx_ios_error` is empty when it left on
 * its own, and carries the reason when it broke.
 */
int32_t zeebx_ios_step(ZeebxIos *app);

/*
 * The current frame, in BGRA8, with the origin at the top-left corner. The pointer is valid
 * until the next `zeebx_ios_step` or `zeebx_ios_stop`. `width` and `height` may be null.
 * Returns null when there is no frame.
 */
const uint8_t *zeebx_ios_frame(ZeebxIos *app, uint32_t *width, uint32_t *height);

/*
 * The screen, in points: its whole size, the safe area inside it and how many pixels a point
 * is. The frame is fitted to the whole screen; the on-screen controls are laid out in the safe
 * area, so no piece ends up under the notch.
 */
void zeebx_ios_layout(ZeebxIos *app, float width, float height, float safe_x, float safe_y,
                      float safe_width, float safe_height, float scale);

/* What the game screen and the layout editor draw now. */
void zeebx_ios_view(ZeebxIos *app, ZeebxView *out);

/*
 * The pieces of the on-screen controls as they are to be drawn now, as of the last
 * `zeebx_ios_view`. Writes at most `capacity` of them and returns how many; zero when they are
 * hidden. There are never more than 16.
 */
int32_t zeebx_ios_elements(ZeebxIos *app, ZeebxElement *out, int32_t capacity);

/* What is written on a piece of `kind`. Lives as long as the process. Null for an unknown kind. */
const char *zeebx_ios_element_label(int32_t kind);

/*
 * A finger on the game. `id` tells fingers apart for as long as each one is down; `phase` is
 * `ZEEBX_TOUCH_*`; `x` and `y` are in the screen's points. A cancelled touch is an `UP`.
 */
void zeebx_ios_touch(ZeebxIos *app, int64_t id, int32_t phase, float x, float y);

/*
 * The physical controller, read once per turn. `buttons` has one bit per `ZEEBX_PAD_*`. The
 * sticks are as `GCController` gives them: from -1 to 1, with y growing **upward**. `connected` 0 says
 * there is none, and releases everything.
 *
 * It is also what the settings screen listens to while a row waits for a button.
 */
void zeebx_ios_controller(ZeebxIos *app, int32_t connected, uint32_t buttons, float left_x,
                          float left_y, float right_x, float right_y);

/* A key of a keyboard. `index` is the position in the core's `BUTTON_NAMES`; `pressed` is 0/1. */
void zeebx_ios_button(ZeebxIos *app, uint32_t index, int32_t pressed);

/* -1 when the name is not a Zeebo button. */
int32_t zeebx_ios_button_by_name(const char *name);

/* Releases every button, key and finger. For when the app leaves the screen. */
void zeebx_ios_release_input(ZeebxIos *app);

/* The pause menu went up (1) or down (0). While it is up the game is still and silent. */
void zeebx_ios_set_menu(ZeebxIos *app, int32_t open);

/* Pauses the game by choice, or releases it. */
void zeebx_ios_set_paused(ZeebxIos *app, int32_t paused);

/*
 * `{"available": bool, "paused": bool, "slots": [{"used": bool, "size": string,
 * "thumbnail": bool}]}`, one entry per save state slot of the open game. Owned.
 */
char *zeebx_ios_slots_json(ZeebxIos *app);

/* The picture of a save state, in BGRA8. Same lifetime as `zeebx_ios_cover`. Null if none. */
const uint8_t *zeebx_ios_slot_thumbnail(ZeebxIos *app, int32_t slot, uint32_t *width,
                                        uint32_t *height);

/* 0, or -1 with the reason in `zeebx_ios_error`, already in the player's language. */
int32_t zeebx_ios_save_state(ZeebxIos *app, int32_t slot);

/* Like `zeebx_ios_save_state`. On success the menu is down and the game is running. */
int32_t zeebx_ios_load_state(ZeebxIos *app, int32_t slot);

/*
 * What is written over the game — rewinding, fast-forwarding, turbo, paused — or an empty
 * string. Valid until the next `zeebx_ios_step`.
 */
const char *zeebx_ios_indicator(ZeebxIos *app);

/* The debug panel's line, or an empty string when it is off. Same lifetime. */
const char *zeebx_ios_debug_line(ZeebxIos *app);

/*
 * The settings screen: `{"tab": id, "tabs": [{"id", "title"}], "rows": [row]}`. A row has a
 * `kind` and, by kind:
 *
 *   section  label
 *   note     label, warn
 *   toggle   id, label, hint, enabled, on
 *   choice   id, label, hint, enabled, options, selected
 *   slider   id, label, hint, enabled, value, min, max, format ("{n}" is the number)
 *   action   id, label, enabled, detail, role
 *
 * An action's `role` is `call` (tap it and call `zeebx_ios_settings_set`), `editor` (open the
 * layout editor) or `url` (open the address in `detail`). Owned.
 */
char *zeebx_ios_settings_json(ZeebxIos *app, const char *tab);

/*
 * A row changed. `value` is the switch (0/1), the index of the choice or the slider's number;
 * an action ignores it. The change is saved and applied before this returns.
 */
void zeebx_ios_settings_set(ZeebxIos *app, const char *id, int32_t value);

/* The layout editor went up (1) or down (0). While it is up, the pieces always show. */
void zeebx_ios_editor(ZeebxIos *app, int32_t open);

/* The one finger the editor follows: down grabs and selects, moved drags, up drops and saves. */
void zeebx_ios_editor_touch(ZeebxIos *app, int32_t phase, float x, float y);

/* Grows (positive) or shrinks the selected piece, ten percent per step. */
void zeebx_ios_editor_resize(ZeebxIos *app, int32_t steps);

/* Every piece back to where and how it shipped. */
void zeebx_ios_editor_reset(ZeebxIos *app);

/* The selected piece and its size, like "D-pad  120%". Owned. Null when none is selected. */
char *zeebx_ios_editor_selection(ZeebxIos *app);

/* Text of the last failure, or an empty string. Valid until the next call that replaces it. */
const char *zeebx_ios_error(ZeebxIos *app);

#ifdef __cplusplus
}
#endif

#endif
