// The item that puts the game frame on screen, directly in the Qt Quick scene graph.
//
// It is the base of `GameScreen`, which is written in Rust: cxx-qt has no `QSGNode` binding, and
// `updatePaintNode` needs them. Rust decides what to show and where; here the node is only built.
#pragma once

#include <QtCore/QRectF>
#include <QtGui/QImage>
#include <QtQuick/QQuickItem>

class FrameItem : public QQuickItem
{
    Q_OBJECT

public:
    explicit FrameItem(QQuickItem *parent = nullptr);

    // A GL texture from the GPU rasterizer, in the context shared with Qt Quick's.
    // `clipU` and `clipV` are the fraction of it that is image, with row 0 at the top.
    void showTexture(quint32 texture, float clipU, float clipV, QRectF dest, bool smooth);
    // The frame in RGB565 (`QImage::Format_RGB16`), for what does not go through the GPU.
    void showImage(const QImage &image, QRectF dest, bool smooth);
    // Changes only where the frame sits and the filter: the window changed size, the image did not.
    void place(QRectF dest, bool smooth);
    // Takes the frame off the screen: the game closed, and its texture may cease to exist.
    void clear();

protected:
    QSGNode *updatePaintNode(QSGNode *old, UpdatePaintNodeData *) override;

private:
    enum class Source { Nothing, Texture, Image };

    Source m_source = Source::Nothing;
    quint32 m_texture = 0;
    // Which texture the node wraps now; 0 when the node shows the image.
    quint32 m_wrapped = 0;
    QRectF m_clip;
    QImage m_image;
    bool m_newImage = false;
    QRectF m_target;
    bool m_smooth = false;
};
