#include "palette.h"

#include <QtGui/QGuiApplication>
#include <QtGui/QPalette>
#include <QtGui/QStyleHints>

namespace zeebx {

namespace {

// The palette being assembled between a `palette_color` and the `palette_apply`.
QPalette assembled;

struct Role {
    const char *name;
    QPalette::ColorRole role;
};

// The names of QML's `palette`, the same as in the theme file.
constexpr Role roles[] = {
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

bool palette_color(bool disabled, rust::Str role, std::uint32_t rgb)
{
    const QByteArrayView name(role.data(), qsizetype(role.size()));
    for (const Role &p : roles) {
        if (name == QByteArrayView(p.name)) {
            const QColor color = QColor::fromRgb(QRgb(rgb));
            if (disabled) {
                assembled.setColor(QPalette::Disabled, p.role, color);
            } else {
                assembled.setColor(p.role, color);
            }
            return true;
        }
    }
    return false;
}

void palette_apply(bool dark)
{
#if QT_VERSION >= QT_VERSION_CHECK(6, 6, 0)
    // The new accent role from Qt 6.6, which Fusion uses in some controls. The theme does not
    // carry it, because Ubuntu 24.04's Qt 6.4 does not have it: it takes the same as `highlight`.
    assembled.setColor(QPalette::Accent, assembled.color(QPalette::Highlight));
#endif
    QGuiApplication::setPalette(assembled);
    assembled = QPalette();
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    QGuiApplication::styleHints()->setColorScheme(dark ? Qt::ColorScheme::Dark : Qt::ColorScheme::Light);
#else
    Q_UNUSED(dark);
#endif
}

void system_palette()
{
    // An empty palette restores the system one: measured on Qt 6.4 and 6.12, with the windows open.
    QGuiApplication::setPalette(QPalette());
#if QT_VERSION >= QT_VERSION_CHECK(6, 8, 0)
    QGuiApplication::styleHints()->unsetColorScheme();
#endif
}

} // namespace zeebx
