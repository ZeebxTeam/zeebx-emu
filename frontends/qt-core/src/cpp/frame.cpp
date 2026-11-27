#include "frame.h"

#include <QtGui/QOpenGLContext>
#include <QtGui/QOpenGLFunctions>
#include <QtQuick/QQuickWindow>
#include <QtQuick/QSGSimpleTextureNode>
#include <QtQuick/QSGTexture>

namespace {

#ifndef GL_TEXTURE_SWIZZLE_A
#define GL_TEXTURE_SWIZZLE_A 0x8E45
#endif

// Makes the frame texture read with alpha 1, whatever alpha is stored in it.
//
// **The frame is opaque, but its alpha is not.** Double Dragon and the other Data East games clear
// the background with alpha zero. The wrapper tells Qt the texture is opaque, and Qt draws it
// without blending: the zero alpha ended up in the window. On NVIDIA, on Wayland, the window has an
// alpha channel even when asked not to (see `prepare_gl`), and the compositor showed through those
// pixels whatever was behind it — the "blank bugs". Checked by capturing the window: the background
// came out `(0,0,0,0)`, and the same frame through `zeebx session` came out black. The `swizzle`
// changes only how the texture is read: the rasterizer keeps drawing into it as before. The texture
// bound to Qt's GL is restored to the previous one, because Qt remembers what it left bound and
// does not check.
void read_alpha_as_one(GLuint texture)
{
    QOpenGLContext *context = QOpenGLContext::currentContext();
    if (context == nullptr) {
        return;
    }
    QOpenGLFunctions *gl = context->functions();
    GLint previous = 0;
    gl->glGetIntegerv(GL_TEXTURE_BINDING_2D, &previous);
    gl->glBindTexture(GL_TEXTURE_2D, texture);
    gl->glTexParameteri(GL_TEXTURE_2D, GL_TEXTURE_SWIZZLE_A, GL_ONE);
    gl->glBindTexture(GL_TEXTURE_2D, static_cast<GLuint>(previous));
}

} // namespace

FrameItem::FrameItem(QQuickItem *parent)
    : QQuickItem(parent)
{
    setFlag(ItemHasContents, true);
}

void FrameItem::showTexture(quint32 texture, float clipU, float clipV, QRectF dest,
                                 bool smooth)
{
    m_source = Source::Texture;
    m_texture = texture;
    m_clip = QRectF(0, 0, clipU, clipV);
    m_target = dest;
    m_smooth = smooth;
    m_image = QImage();
    update();
}

void FrameItem::showImage(const QImage &image, QRectF dest, bool smooth)
{
    m_source = Source::Image;
    m_image = image;
    m_newImage = true;
    m_target = dest;
    m_smooth = smooth;
    update();
}

void FrameItem::place(QRectF dest, bool smooth)
{
    if (dest != m_target || smooth != m_smooth) {
        m_target = dest;
        m_smooth = smooth;
        update();
    }
}

void FrameItem::clear()
{
    m_source = Source::Nothing;
    m_texture = 0;
    m_image = QImage();
    update();
}

QSGNode *FrameItem::updatePaintNode(QSGNode *old, UpdatePaintNodeData *)
{
    auto *node = static_cast<QSGSimpleTextureNode *>(old);
    if (m_source == Source::Nothing || window() == nullptr) {
        m_wrapped = 0;
        delete node;
        return nullptr;
    }
    if (node == nullptr) {
        node = new QSGSimpleTextureNode;
        node->setOwnsTexture(true);
    }

    if (m_source == Source::Texture) {
        // The texture belongs to the rasterizer: the wrapper does not own it, and is rebuilt only
        // when the name changes.
        //
        // **The declared size is 1×1 on purpose.** Qt uses the texture size only to convert the
        // `sourceRect` into 0-to-1 coordinates, and the rasterizer already delivers the crop in
        // those coordinates. The real size changes with the internal resolution and the 16:9
        // columns, and the core's GL 3.3 does not ask the driver for it.
        if (node->texture() == nullptr || m_wrapped != m_texture) {
            read_alpha_as_one(m_texture);
            auto *texture = QNativeInterface::QSGOpenGLTexture::fromNative(
                m_texture, window(), QSize(1, 1), QQuickWindow::TextureIsOpaque);
            node->setTexture(texture);
            m_wrapped = m_texture;
        }
        node->setSourceRect(m_clip);
    } else if (m_newImage || node->texture() == nullptr || m_wrapped != 0) {
        // The 2D path: the image is only uploaded when it changed, the same rule as egui.
        node->setTexture(window()->createTextureFromImage(m_image));
        m_wrapped = 0;
        node->setSourceRect(QRectF(QPointF(0, 0), m_image.size()));
        m_newImage = false;
    }

    node->setRect(m_target);
    node->setFiltering(m_smooth ? QSGTexture::Linear : QSGTexture::Nearest);
    return node;
}
