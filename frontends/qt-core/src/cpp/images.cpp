#include "images.h"

#include <QtQuick/QQuickImageProvider>

namespace zeebx {

namespace {

class ImageProvider : public QQuickImageProvider
{
public:
    explicit ImageProvider(rust::Fn<QImage(rust::Str)> source)
        : QQuickImageProvider(QQuickImageProvider::Image)
        , m_source(source)
    {
    }

    // **Must run on the interface thread**, which is where the Rust state lives — in a
    // `thread_local`. A `QImage` provider is synchronous as long as the QML `Image` does not ask
    // for `asynchronous: true`, and none does.
    QImage requestImage(const QString &id, QSize *size, const QSize &) override
    {
        const QByteArray address = id.toUtf8();
        QImage image = m_source(rust::Str(address.constData(), std::size_t(address.size())));
        if (size != nullptr) {
            *size = image.size();
        }
        return image;
    }

private:
    rust::Fn<QImage(rust::Str)> m_source;
};

} // namespace

void register_images(QQmlEngine &engine, rust::Fn<QImage(rust::Str)> source)
{
    // The engine takes ownership of the provider and deletes it on exit.
    engine.addImageProvider(QStringLiteral("zeebx"), new ImageProvider(source));
}

} // namespace zeebx
