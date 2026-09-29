// O item que põe o quadro do jogo na tela, direto no scene graph do Qt Quick.
//
// É a base da `TelaDoJogo`, que é escrita em Rust: o cxx-qt não tem binding de `QSGNode`, e o
// `updatePaintNode` precisa deles. O Rust decide o que mostrar e onde; aqui só se monta o nó.
#pragma once

#include <QtCore/QRectF>
#include <QtGui/QImage>
#include <QtQuick/QQuickItem>

class ItemDoQuadro : public QQuickItem
{
    Q_OBJECT

public:
    explicit ItemDoQuadro(QQuickItem *parent = nullptr);

    // Uma textura de GL do rasterizador na placa, no contexto compartilhado com o do Qt Quick.
    // `recorteU` e `recorteV` são a fração dela que é imagem, com a linha 0 no topo.
    void mostraTextura(quint32 textura, float recorteU, float recorteV, QRectF destino, bool suave);
    // O quadro em RGB565 (`QImage::Format_RGB16`), para o que não passa pela placa.
    void mostraImagem(const QImage &imagem, QRectF destino, bool suave);
    // Muda só onde o quadro fica e o filtro: a janela mudou de tamanho, a imagem não.
    void posiciona(QRectF destino, bool suave);
    // Tira o quadro da tela: o jogo fechou, e a textura dele pode deixar de existir.
    void esvazia();

protected:
    QSGNode *updatePaintNode(QSGNode *antigo, UpdatePaintNodeData *) override;

private:
    enum class Fonte { Nada, Textura, Imagem };

    Fonte m_fonte = Fonte::Nada;
    quint32 m_textura = 0;
    // Qual textura o nó embrulha agora; 0 quando o nó mostra a imagem.
    quint32 m_embrulhada = 0;
    QRectF m_recorte;
    QImage m_imagem;
    bool m_imagemNova = false;
    QRectF m_destino;
    bool m_suave = false;
};
