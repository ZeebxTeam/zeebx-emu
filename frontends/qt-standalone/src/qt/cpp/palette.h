// The colors of the interface themes: the Qt palette on top of the Fusion style.
//
// Assembled here because cxx-qt-lib does not bring QPalette. Rust reads the theme
// (`zeebx::ui::themes`) and passes it color by color; it is applied to the whole window, through
// QGuiApplication::setPalette, which the Qt Quick windows and the Fusion controls follow right
// away. See `docs/implementation/26-themes.md`.
#pragma once

#include <cstdint>

#include "rust/cxx.h"

namespace zeebx {

// A color of the palette being assembled, by the name QML uses (`windowText`). `disabled` puts the
// color only in the disabled controls' group; without it, the color applies to all. Returns false
// for a role Qt does not have.
bool palette_color(bool disabled, rust::Str role, std::uint32_t rgb);

// Applies the assembled palette to all windows, and restarts the assembly. `dark` tells the system
// the theme is dark, so the title bar follows along where Qt knows how to ask for that.
void palette_apply(bool dark);

// Goes back to the system palette, which was the only one before themes.
void system_palette();

} // namespace zeebx
