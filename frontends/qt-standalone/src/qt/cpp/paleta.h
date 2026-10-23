// As cores dos temas da interface: a paleta do Qt por cima do estilo Fusion.
//
// Montada aqui porque o cxx-qt-lib não traz o QPalette. O Rust lê o tema (`zeebx::ui::temas`) e
// passa cor por cor; a aplicação é da janela inteira, pelo QGuiApplication::setPalette, que as
// janelas do Qt Quick e os controles do Fusion acompanham na hora. Ver
// `docs/implementacao/26-temas.md`.
#pragma once

#include <cstdint>

#include "rust/cxx.h"

namespace zeebx {

// Uma cor da paleta em montagem, pelo nome que o QML usa (`windowText`). `desabilitada` põe a cor
// só no grupo dos controles desabilitados; sem ela, a cor vale para todos. Devolve falso para um
// papel que o Qt não tem.
bool paleta_cor(bool desabilitada, rust::Str papel, std::uint32_t rgb);

// Aplica a paleta montada a todas as janelas, e recomeça a montagem. `escuro` diz ao sistema que
// o tema é escuro, para a barra de título acompanhar onde o Qt sabe pedir isso.
void paleta_aplica(bool escuro);

// Volta à paleta do sistema, que era a única antes dos temas.
void paleta_do_sistema();

} // namespace zeebx
