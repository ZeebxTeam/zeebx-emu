#include "paleta.h"

#include <QtGui/QGuiApplication>
#include <QtGui/QPalette>
#include <QtGui/QStyleHints>

namespace zeebx {

namespace {

// A paleta em montagem entre um `paleta_cor` e o `paleta_aplica`.
QPalette montada;

struct Papel {
    const char *nome;
    QPalette::ColorRole papel;
};

// Os nomes do `palette` do QML, os mesmos do arquivo do tema.
constexpr Papel papeis[] = {
    {"window", QPalette::Window},
    {"windowText", QPalette::WindowText},
    {"base", QPalette::Base},
    {"alternateBase", QPalette::AlternateBase},
    {"text", QPalette::Text},
    {"placeholderText", QPalette::PlaceholderText},
    {"button", QPalette::Button},
    {"buttonText", QPalette::ButtonText},
    {"brightText", QPalette::BrightText},
    {"light", QPalette::Light},
    {"midlight", QPalette::Midlight},
    {"mid", QPalette::Mid},
    {"dark", QPalette::Dark},
    {"shadow", QPalette::Shadow},
    {"highlight", QPalette::Highlight},
    {"highlightedText", QPalette::HighlightedText},
    {"link", QPalette::Link},
    {"linkVisited", QPalette::LinkVisited},
    {"toolTipBase", QPalette::ToolTipBase},
    {"toolTipText", QPalette::ToolTipText},
};

} // namespace

bool paleta_cor(bool desabilitada, rust::Str papel, std::uint32_t rgb)
{
    const QByteArrayView nome(papel.data(), qsizetype(papel.size()));
    for (const Papel &p : papeis) {
        if (nome == QByteArrayView(p.nome)) {
            const QColor cor = QColor::fromRgb(QRgb(rgb));
            if (desabilitada) {
                montada.setColor(QPalette::Disabled, p.papel, cor);
            } else {
                montada.setColor(p.papel, cor);
            }
            return true;
        }
    }
    return false;
}

void paleta_aplica(bool escuro)
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 6, 0)
    // O papel de destaque novo do Qt 6.6, que o Fusion usa em alguns controles. O tema não o
    // traz, porque o Qt 6.4 do Ubuntu 24.04 não o tem: vale o mesmo do `highlight`.
    montada.setColor(QPalette::Accent, montada.color(QPalette::Highlight));
#endif
    QGuiApplication::setPalette(montada);
    montada = QPalette();
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    QGuiApplication::styleHints()->setColorScheme(escuro ? Qt::ColorScheme::Dark : Qt::ColorScheme::Light);
#else
    Q_UNUSED(escuro);
#endif
}

void paleta_do_sistema()
{
    // Uma paleta vazia devolve a do sistema: medido no Qt 6.4 e no 6.12, com as janelas abertas.
    QGuiApplication::setPalette(QPalette());
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    QGuiApplication::styleHints()->unsetColorScheme();
#endif
}

} // namespace zeebx
