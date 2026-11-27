// The C++ glue that cxx-qt-lib does not cover: the GL setup before QGuiApplication, the offscreen
// context the GPU rasterizer borrows, and the windows' icon.
#pragma once

#include <cstddef>

#include "rust/cxx.h"

namespace zeebx {

// Before QGuiApplication: GL as the Qt Quick API, 3.3 core, shared contexts, the `basic` render
// loop, which draws on the main thread — where the emulator runs —, and the Fusion style for the
// controls.
void prepare_gl();

// The rasterizer's context: its own, offscreen and shared with Qt Quick's. After
// QGuiApplication. Returns false if the GPU can't provide it.
bool gl_create();
bool gl_make_current();
void gl_done_current();

// Destroys the context. Must happen after the session is gone — it releases textures in it — and
// before QGuiApplication: left to the static destructors, the QOffscreenSurface dies after Qt and
// the process crashes with SIGSEGV when the window closes.
void gl_destroy();

// The icon of every window: the logo, from `qrc:/zeebx/zeebx.png`, scaled down to icon size.
// After QGuiApplication. On Wayland the `.desktop` is what counts, found by the name given to
// `setDesktopFileName`; the icon in pixels applies on X11, Windows and macOS.
void apply_icon();

// The address of a GL function, for `glow`. Zero if there is none.
std::size_t gl_function(rust::Str name);

} // namespace zeebx
