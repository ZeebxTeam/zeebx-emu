# 05 — Vídeo 2D

## A tela

640×480, RGB565 — a saída de vídeo do Zeebo. `display.rs` tem o `Framebuffer` (pixels em `u16`) e
as operações: `set_pixel_native`, `fill_rect`, `draw_frame`, `draw_line` (Bresenham, só inteiros,
como o hardware da época) e `blit`.

O `IBitmap` do jogo é um objeto nosso com um `Framebuffer` associado. O da tela é o
**device bitmap**, criado na primeira vez que alguém pede `IDISPLAY_GetDeviceBitmap`.

## O DIB: onde tudo fica caro

O BREW deixa o jogo pedir acesso **direto aos pixels**, via `QueryInterface(AEECLSID_DIB)`. A
partir daí o jogo tem um ponteiro para a memória dos pixels e escreve nela sem passar por nós.
Os jogos comerciais fazem isso o tempo todo.

Isso obriga a manter dois lados em dia:

```
  Framebuffer nosso  ◄── sync_from_guest ──  buffer na memória do guest
                     ── sync_to_guest ────►
```

E aí está a armadilha: **fazer isso em volta de toda chamada custa 1,2 MB de ida e volta.**

### O que o Pac-Mania mostrou

O jogo desenha as próprias imagens **pixel a pixel** pela API — 1,16 milhão de `DrawPixel` em dez
quadros — e consulta o recorte 112 mil vezes a cada quinze segundos. Cobrando a superfície
inteira de cada uma dessas chamadas:

| | tempo real |
|---|---|
| 110 quadros, cópia por chamada | **232 s** — dos quais 1,8 s de CPU emulada e 161 s de cópia |
| depois de tratar o pixel no lugar | 3,4 s |
| depois de dispensar a cópia nos ajustes de estado | 20 s virtuais em 26 s reais |

Duas mudanças, mesmas 59 milhões de instruções.

### Como está resolvido

`touches_whole_surface(nome)` decide se a chamada paga a cópia.

- `DrawPixel` e `GetPixel` tratam do **seu** pixel direto no buffer do jogo — dois bytes, não a
  superfície.
- Os ajustes de estado (`SetClipRect`, `SetColor`, `SetFont`, os pares `Get`/`Set` do
  `IGraphics`) não tocam pixel nenhum e não pagam nada.
- Todo o resto paga.

A lista é de **exclusão**, e isso é deliberado: esquecer ali um método que desenha daria pixel
errado, que é difícil de perceber; deixar de fora um que não desenha só custa a cópia, que
aparece na medição. **Na dúvida, copia.**

## O recorte

`IDISPLAY_SetClipRect` era aceito e ignorado, e isso escondia um erro grande.

O Pac-Mania desenha a **folha de fontes inteira** com `BitBlt` e conta com o recorte para que só
a letra apareça. Sem o recorte, a folha toda ia para a tela a cada letra — a abertura do jogo
virava um mosaico de alfabetos sobrepostos.

O corte anda com a **origem na fonte** junto:

```rust
clip_blit(recorte, destino, tamanho, origem) -> Option<(destino, tamanho, origem)>
```

Encolher só o retângulo de destino mostraria o canto errado da imagem — é assim que um atlas de
fontes vira letra trocada. Há teste exatamente sobre essa propriedade.

`None` significa "não sobrou nada": o blit não acontece.

## Blit

`Framebuffer::blit` copia com origem, tamanho e cor transparente opcional. A transparência vem de
`IBITMAP_SetTransparencyColor`, guardada por bitmap.

`IBitmap::BltIn` e `BltOut` invertem os papéis de fonte e destino; `IDISPLAY_BitBlt` desenha no
destino corrente do display, que o `SetDestination` pode ter trocado.

## Texto

Não desenhamos texto. `IDISPLAY_DrawText` guarda a string (aparece no relatório), e
`GetFontMetrics` e `MeasureTextEx` devolvem números coerentes entre si — altura de fonte de tela
pequena, avanço fixo por caractere.

É medida aproximada de propósito: **serve para o jogo posicionar o que ele mesmo desenha**, e um
número plausível o deixa seguir. Sem nenhum, o Pac-Mania nem monta a tela. Uma fonte de verdade
continua na lista do que falta.
