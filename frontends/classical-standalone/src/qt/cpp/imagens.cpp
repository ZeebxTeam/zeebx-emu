#include "imagens.h"

#include <QtQuick/QQuickImageProvider>

namespace zeebx {

namespace {

class ProvedorDeImagens : public QQuickImageProvider
{
public:
    explicit ProvedorDeImagens(rust::Fn<QImage(rust::Str)> fonte)
        : QQuickImageProvider(QQuickImageProvider::Image)
        , m_fonte(fonte)
    {
    }

    // **Tem de rodar na thread da interface**, que é onde o estado do Rust mora — num
    // `thread_local`. Um provedor de `QImage` é síncrono enquanto o `Image` do QML não pede
    // `asynchronous: true`, e nenhum pede.
    QImage requestImage(const QString &id, QSize *tamanho, const QSize &) override
    {
        const QByteArray endereco = id.toUtf8();
        QImage imagem = m_fonte(rust::Str(endereco.constData(), std::size_t(endereco.size())));
        if (tamanho != nullptr) {
            *tamanho = imagem.size();
        }
        return imagem;
    }

private:
    rust::Fn<QImage(rust::Str)> m_fonte;
};

} // namespace

void registra_imagens(QQmlEngine &engine, rust::Fn<QImage(rust::Str)> fonte)
{
    // O engine fica com o provedor e o apaga ao sair.
    engine.addImageProvider(QStringLiteral("zeebx"), new ProvedorDeImagens(fonte));
}

} // namespace zeebx
