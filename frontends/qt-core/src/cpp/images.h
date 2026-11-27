// The library's images for QML: covers, Z-Wheel logos and ratings.
//
// QML only loads images by address, and the covers are in memory, decoded by Rust. cxx-qt-lib
// brings only the provider's base type, with no way to write `requestImage` in Rust; this is the
// provider, and the one that answers is the Rust function it receives.
#pragma once

#include <QtGui/QImage>
#include <QtQml/QQmlEngine>

#include "rust/cxx.h"

namespace zeebx {

// Registers the provider as `image://zeebx/…`. `source` receives the address without the scheme
// and returns the image, or a null one when there is none.
void register_images(QQmlEngine &engine, rust::Fn<QImage(rust::Str)> source);

} // namespace zeebx
