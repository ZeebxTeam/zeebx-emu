#include "gl_qt.h"

#include <QtCore/QCoreApplication>
#include <QtGui/QGuiApplication>
#include <QtGui/QIcon>
#include <QtGui/QImage>
#include <QtGui/QOffscreenSurface>
#include <QtGui/QPixmap>
#include <QtGui/QOpenGLContext>
#include <QtGui/QSurfaceFormat>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGRendererInterface>

#include <memory>

namespace zeebx {

namespace {
// Vivem até o `gl_destroi`, no fim do `launch`: o contexto não pode morrer com a janela do jogo,
// porque a Z-Wheel reabre jogos e as texturas do rasterizador moram nele.
std::unique_ptr<QOpenGLContext> contexto;
std::unique_ptr<QOffscreenSurface> superficie;
} // namespace

void prepara_gl()
{
    // O `threaded`, padrão no Linux com Mesa, desenharia o scene graph noutra thread enquanto a
    // principal escreve o quadro seguinte. Quem define a variável à mão continua mandando.
    if (!qEnvironmentVariableIsSet("QSG_RENDER_LOOP")) {
        qputenv("QSG_RENDER_LOOP", "basic");
    }
    // O estilo dos controles do Qt Quick: o Fusion segue a paleta do sistema em todos eles. O
    // Basic, que é o padrão, pinta os campos e os textos com a paleta dele, e num tema escuro o
    // texto solto saía escuro sobre o fundo escuro da janela.
    if (!qEnvironmentVariableIsSet("QT_QUICK_CONTROLS_STYLE")) {
        qputenv("QT_QUICK_CONTROLS_STYLE", "Fusion");
    }
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts);
    // No macOS o padrão é Metal e no Windows é D3D; o rasterizador é GL.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    // O contrato que o eframe entregava, e que os shaders do `Pintor` e do `GpuState` esperam.
    QSurfaceFormat formato;
    // GL de desktop, dito explicitamente. Sem isto, no Wayland com o driver da NVIDIA, o pedido
    // de 3.3 core volta EGL_BAD_MATCH e o Qt Quick aborta ao abrir a janela; no X11 passava.
    formato.setRenderableType(QSurfaceFormat::OpenGL);
    formato.setVersion(3, 3);
    formato.setProfile(QSurfaceFormat::CoreProfile);
    formato.setDepthBufferSize(24);
    formato.setStencilBufferSize(8);
    // A janela é opaca. É o que o Qt no Wayland consulta para dizer ao compositor que ela não se
    // mistura com o que está atrás. **Não basta sozinho**: a NVIDIA não tem configuração sem alfa
    // e entrega 8 bits mesmo assim — medido pelo `QSG_INFO`. O que garante o quadro opaco é o
    // `le_alfa_como_um`, em `quadro.cpp`.
    formato.setAlphaBufferSize(0);
    QSurfaceFormat::setDefaultFormat(formato);
}

void aplica_icone()
{
    // A logo tem mais de mil pixels de lado. O egui a reduzia a 256 pelo mesmo motivo: guardar a
    // imagem inteira para desenhar algo que nunca passa de alguns pixels na barra.
    const QImage logo(QStringLiteral(":/zeebx/zeebx.png"));
    if (logo.isNull()) {
        return;
    }
    const QImage icone = logo.scaled(256, 256, Qt::KeepAspectRatio, Qt::SmoothTransformation);
    QGuiApplication::setWindowIcon(QIcon(QPixmap::fromImage(icone)));
}

bool gl_cria()
{
    if (contexto) {
        return true;
    }
    auto novo = std::make_unique<QOpenGLContext>();
    novo->setFormat(QSurfaceFormat::defaultFormat());
    novo->setShareContext(QOpenGLContext::globalShareContext());
    if (!novo->create()) {
        return false;
    }
    auto fora_de_tela = std::make_unique<QOffscreenSurface>();
    fora_de_tela->setFormat(novo->format());
    fora_de_tela->create();
    if (!fora_de_tela->isValid()) {
        return false;
    }
    contexto = std::move(novo);
    superficie = std::move(fora_de_tela);
    return true;
}

bool gl_torna_corrente()
{
    return contexto && contexto->makeCurrent(superficie.get());
}

void gl_solta()
{
    if (contexto) {
        contexto->doneCurrent();
    }
}

void gl_destroi()
{
    if (contexto) {
        contexto->doneCurrent();
    }
    contexto.reset();
    superficie.reset();
}

std::size_t gl_funcao(rust::Str nome)
{
    if (!contexto) {
        return 0;
    }
    return reinterpret_cast<std::size_t>(contexto->getProcAddress(QByteArray(nome.data(), qsizetype(nome.size()))));
}

} // namespace zeebx
