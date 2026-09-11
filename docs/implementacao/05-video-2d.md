# 05 — Vídeo 2D

## A tela

640×480, RGB565 — a saída de vídeo do Zeebo. `video/display.rs` tem o `Framebuffer` (pixels em `u16`) e
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

### O endereço volta a ser usado, e o cabeçalho tem de acompanhar

Publicar o `IDIB` era feito **uma vez por endereço de objeto** — e endereço de objeto é
reciclado: liberado o anterior, o próximo bitmap nasce no mesmo lugar, com outro tamanho.

O Tekken 2 decodifica nove imagens em sequência, liberando cada uma antes da seguinte; todas
nasceram no mesmo endereço. O `IDIB` anunciava para as nove o tamanho da primeira, 200×112. O
jogo então criava uma página de 200×112 para uma folha de letras de 360×280, guardava só o canto
dela, e depois pedia cada glifo por coordenada da folha inteira: o que caía fora virava um bloco
preenchido com a textura. O menu inteiro saía com as palavras como retângulos laranja.

Hoje o cabeçalho é reescrito a cada exposição, com o tamanho e o passo da imagem que está lá. O
buffer é reaproveitado quando cabe — reservar outro a cada vez também acertaria o tamanho, mas a
região de superfícies não recicla e um jogo que decodifique centenas de imagens a esgotaria.

A lição é a mesma de outras vezes: **o que um jogo lê de uma struct nossa vale tanto quanto o
que devolvemos de uma chamada.** Aqui nenhuma chamada falhou, e o relatório saiu limpo.

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

### Recorte nenhum é a tela inteira, e não "nada passa"

O `clip_blit` sempre soube disso — sem recorte, o blit passa inteiro. O `clip_rect`, ao lado,
fazia o contrário: um `let clip = clip?;` na primeira linha, e todo retângulo de quem **não**
define recorte era descartado.

O efeito é silencioso e grande: `IDISPLAY_DrawRect` nunca desenhou nada em jogo nenhum que não
chamasse `SetClipRect` antes. Levou tempo para aparecer porque quase todo desenho 2D é blit, e
blit tomava o caminho certo. E havia um teste afirmando o engano — `clip_rect(None, r) == None`.
Teste errado protege defeito, e este protegeu.

## Limpar a tela é um `DrawRect`

Não existe `IDISPLAY_ClearScreen` na vtable: no SDK ele é uma macro que chama

```c
IDISPLAY_DrawRect(p, NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)
```

Três leituras precisam estar certas ao mesmo tempo, e as três estavam erradas aqui:

| O que chega | O que quer dizer | O que fazíamos |
|---|---|---|
| `pRect == NULL` | a superfície inteira | nada a desenhar |
| `RGB_NONE` | **a cor corrente** do `SetColor` | "não pinte" |
| `flags` | quem manda em moldura e preenchimento | ignorado |

O Tekken 2 limpa a tela assim **uma vez por quadro**: `SetColor(CLR_USER_BACKGROUND, preto)` e
`ClearScreen`. Enquanto a limpeza não acontecia, o menu dele era desenhado por cima do que já
estava na tela — o "APERTE ❶ PARA COMEÇAR" da abertura ficava aparecendo por baixo de "MODO
VERSUS". O `flags` também não é detalhe: o Quake pede moldura sozinha (`IDF_RECT_FRAME`) passando
preto no preenchimento, e honrar a cor ignorando o sinalizador punha um retângulo preto que ele
não pediu.

Sinalizador nenhum (`flags == 0`) mantém o comportamento antigo, desenhar os dois. Nenhum jogo do
acervo chama assim, e na dúvida é melhor continuar desenhando do que apagar uma tela por causa de
uma leitura que não deu para conferir.

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
