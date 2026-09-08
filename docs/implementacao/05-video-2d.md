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

Mas o `QueryInterface` é permissão, não pré-requisito. No console um `IBitmap` de software
**é** um `IDIB`: a struct começa com a vtable de `IBitmap` e segue com campos públicos —
tamanho, passo, profundidade, ponteiro para os pixels — e nada impede o jogo de lê-los direto.
O Peggle faz exatamente isso com o bitmap que sai do decodificador de PNG, e por isso todo
bitmap decodificado já sai com esses campos preenchidos.

Enquanto eles saíam zerados, o jogo lia `-size 0/0` (a frase é do log dele) e montava cada
sprite como um quadrado de lado zero: **76.618 dos 77.208 triângulos de um quadro** eram
descartados por área nula, e a tela ficava preta com o jogo desenhando o tempo todo. Foi o
contador de triângulos rejeitados, por motivo, que apontou isso — nenhuma chamada de API tinha
falhado.

Só o caminho do decodificador publica os pixels sem ser pedido. Nos outros, o `QueryInterface`
continua sendo a hora de alocar: a região de superfícies não recicla, e toda superfície
publicada entra no laço de sincronização abaixo.

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

### O recorte não é acabamento

`IIMAGE_Draw` percorria a imagem inteira e conferia pixel a pixel, sem olhar o recorte. Isso
está errado duas vezes.

O Pac-Mania desenha a **folha de fontes inteira** e aperta o recorte para que só a letra apareça
— é o desenho de texto dele. Sem o recorte no laço, uma letra custava os 193 mil pixels da folha:
20 mil chamadas de `Draw` liam **3,9 bilhões de pixels** para pôr 315 mil na tela. E, como a
nossa escrita de pixel não conhece o recorte, o que o jogo mandou esconder ia para a tela junto.

Hoje o recorte entra no cálculo dos limites do laço, antes de ler qualquer pixel. Cinco segundos
virtuais do Pac-Mania saíram de **174 para 62 segundos** de relógio, e a tela de escolha de idioma
aparece inteira e certa.

A regra que fica: **num laço de desenho, o recorte decide o tamanho do trabalho, não a aparência
do resultado.** Aplicá-lo por pixel, no fim, é pagar por tudo que se vai jogar fora.

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
