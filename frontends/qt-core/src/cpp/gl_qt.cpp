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
// They live until `gl_destroy`, at the end of `launch`: the context cannot die with the game window,
// because the Z-Wheel reopens games and the rasterizer's textures live in it.
std::unique_ptr<QOpenGLContext> context;
std::unique_ptr<QOffscreenSurface> surface;
} // namespace

void prepare_gl()
{
    // `threaded`, the default on Linux with Mesa, would draw the scene graph on another thread
    // while the main one writes the next frame. Whoever sets the variable by hand still wins.
    if (!qEnvironmentVariableIsSet("QSG_RENDER_LOOP")) {
        qputenv("QSG_RENDER_LOOP", "basic");
    }
    // The style of the Qt Quick controls: Fusion follows the system palette in all of them. Basic,
    // which is the default, paints the fields and texts with its own palette, and in a dark theme
    // the loose text came out dark on the window's dark background.
    if (!qEnvironmentVariableIsSet("QT_QUICK_CONTROLS_STYLE")) {
        qputenv("QT_QUICK_CONTROLS_STYLE", "Fusion");
    }
    QCoreApplication::setAttribute(Qt::AA_ShareOpenGLContexts);
    // On macOS the default is Metal and on Windows it is D3D; the rasterizer is GL.
    QQuickWindow::setGraphicsApi(QSGRendererInterface::OpenGL);
    // The contract eframe used to deliver, and that the shaders of `GlPainter` and `GpuState` expect.
    QSurfaceFormat format;
    // Desktop GL, stated explicitly. Without this, on Wayland with the NVIDIA driver, the request
    // for 3.3 core comes back EGL_BAD_MATCH and Qt Quick aborts when opening the window; on X11 it
    // went through.
    format.setRenderableType(QSurfaceFormat::OpenGL);
    format.setVersion(3, 3);
    format.setProfile(QSurfaceFormat::CoreProfile);
    format.setDepthBufferSize(24);
    format.setStencilBufferSize(8);
    // The window is opaque. This is what Qt on Wayland checks to tell the compositor that it does
    // not blend with what is behind it. **It is not enough on its own**: NVIDIA has no config
    // without alpha and delivers 8 bits anyway — measured with `QSG_INFO`. What guarantees the
    // opaque frame is `read_alpha_as_one`, in `frame.cpp`.
    format.setAlphaBufferSize(0);
    QSurfaceFormat::setDefaultFormat(format);
}

void apply_icon()
{
    // The logo is more than a thousand pixels on a side. egui scaled it down to 256 for the same
    // reason: keeping the whole image to draw something that never exceeds a few pixels in the bar.
    const QImage logo(QStringLiteral(":/zeebx/zeebx.png"));
    if (logo.isNull()) {
        return;
    }
    const QImage iconName = logo.scaled(256, 256, Qt::KeepAspectRatio, Qt::SmoothTransformation);
    QGuiApplication::setWindowIcon(QIcon(QPixmap::fromImage(iconName)));
}

bool gl_create()
{
    if (context) {
        return true;
    }
    auto create = std::make_unique<QOpenGLContext>();
    create->setFormat(QSurfaceFormat::defaultFormat());
    create->setShareContext(QOpenGLContext::globalShareContext());
    if (!create->create()) {
        return false;
    }
    auto offscreen = std::make_unique<QOffscreenSurface>();
    offscreen->setFormat(create->format());
    offscreen->create();
    if (!offscreen->isValid()) {
        return false;
    }
    context = std::move(create);
    surface = std::move(offscreen);
    return true;
}

bool gl_make_current()
{
    return context && context->makeCurrent(surface.get());
}

void gl_done_current()
{
    if (context) {
        context->doneCurrent();
    }
}

void gl_destroy()
{
    if (context) {
        context->doneCurrent();
    }
    context.reset();
    surface.reset();
}

std::size_t gl_function(rust::Str name)
{
    if (!context) {
        return 0;
    }
    return reinterpret_cast<std::size_t>(context->getProcAddress(QByteArray(name.data(), qsizetype(name.size()))));
}

} // namespace zeebx
