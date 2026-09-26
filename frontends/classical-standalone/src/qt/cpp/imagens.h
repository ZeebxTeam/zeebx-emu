// As imagens da biblioteca para o QML: capas, logos da Z-Wheel e classificações.
//
// O QML só carrega imagem por endereço, e as capas estão na memória, decodificadas pelo Rust. O
// cxx-qt-lib traz só o tipo base do provedor, sem como escrever o `requestImage` em Rust; este é
// o provedor, e quem responde é a função Rust que ele recebe.
#pragma once

#include <QtGui/QImage>
#include <QtQml/QQmlEngine>

#include "rust/cxx.h"

namespace zeebx {

// Registra o provedor como `image://zeebx/…`. `fonte` recebe o endereço sem o esquema e devolve
// a imagem, ou uma nula quando não há.
void registra_imagens(QQmlEngine &engine, rust::Fn<QImage(rust::Str)> fonte);

} // namespace zeebx
