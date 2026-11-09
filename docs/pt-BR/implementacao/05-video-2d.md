> **Registro em português.** Esta é a versão original deste documento, preservada como estava na v0.5.1 e não é mais atualizada. A versão mantida está em inglês: [05-video-2d.md](../../implementation/05-video-2d.md).

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
continua sendo a hora de alocar, e toda superfície publicada entra no laço de sincronização
abaixo.

### O endereço volta a ser usado, e o cabeçalho tem de acompanhar

Publicar o `IDIB` era feito **uma vez por endereço de objeto** — e endereço de objeto é
reciclado: liberado o anterior, o próximo bitmap nasce no mesmo lugar, com outro tamanho.

O Tekken 2 decodifica nove imagens em sequência, liberando cada uma antes da seguinte; todas
nasceram no mesmo endereço. O `IDIB` anunciava para as nove o tamanho da primeira, 200×112. O
jogo então criava uma página de 200×112 para uma folha de letras de 360×280, guardava só o canto
dela, e depois pedia cada glifo por coordenada da folha inteira: o que caía fora virava um bloco
preenchido com a textura. O menu inteiro saía com as palavras como retângulos laranja.

Hoje o cabeçalho é reescrito a cada exposição, com o tamanho e o passo da imagem que está lá. O
buffer é reaproveitado quando cabe, o que poupa uma troca de buffer a cada imagem.

**A região de superfícies usa o mesmo alocador do heap do jogo.** Primeiro ela não reciclava, e
a Z-Wheel a esgotava em 27 s de navegação. Depois passou a reciclar blocos inteiros — o menor
livre que coubesse —, sem dividir nem fundir vizinhos: com tamanhos variados, os buracos pequenos
não serviam a pedidos grandes e os 8 MB acabavam do mesmo jeito, só mais devagar, com o bitmap
nascendo sem pixels. Com o `Heap`, blocos devolvidos se fundem e um bloco grande serve a um pedido
pequeno. O color buffer do EGL também passa por ele e devolve o antigo quando cresce.

A lição é a mesma de outras vezes: **o que um jogo lê de uma struct nossa vale tanto quanto o
que devolvemos de uma chamada.** Aqui nenhuma chamada falhou, e o relatório saiu limpo.

**E a anotação de capacidade tem de morrer junto com o buffer.** Ao desenhar numa superfície do
jogo montamos um `IBitmap` nosso como origem e o descartamos em seguida. O descarte tirava o
buffer da anotação, mas deixava a capacidade para trás; o bitmap seguinte a nascer naquele
endereço lia a sobra, concluía que o buffer dele já cabia e publicava um `IDIB` com `pBmp`
**nulo**. Quem lesse os pixels por ali recebia zero: o `BltIn` do Bejeweled Twist monta a tabela
de linhas a partir do `pBmp`. Hoje a origem temporária é devolvida pelo `solta_dib`, que solta as
duas coisas.

**O buffer reaproveitado ainda tem os pixels do morto.** Reescrever só o cabeçalho deixa no
buffer a imagem do objeto anterior, e o bitmap novo só tem os pixels certos do nosso lado. No
Unicorn isso não aparecia: a vigia de escrita dizia "o jogo não mexeu aqui", a importação não
acontecia e a nossa cópia prevalecia. O Dynarmic não tem vigia e responde sempre "sujo" — o que
só é seguro se o buffer nunca estiver atrás do host. Aqui estava, e a importação trazia a imagem
anterior por cima da folha de glifos: no motor da janela, as letras do Tekken 2 voltaram a sair
como blocos, agora escuros.

Por isso um objeto novo num endereço que já tem buffer entra em `dib_herdados`: enquanto estiver
lá, a importação é pulada e a primeira exposição publica os pixels do host, mesmo cabendo. A
regra é de ciclo de vida — **o buffer pertence ao objeto**, e bytes de um objeto que morreu não são
do jogo para serem trazidos de volta.

### Só o retângulo que mudou volta para o jogo

Publicar a superfície inteira depois de cada desenho era o maior custo do emulador em jogos de
sprite: o Pac-Mania desenha 160 mil sprites em cinco segundos virtuais, e cada `IIMAGE_Draw`
reescrevia os 600 KB da superfície — 15 dos 16 segundos de API. O `Framebuffer` agora guarda a
caixa suja desde a última publicação e um número de série; enquanto a série for a mesma que o
jogo já recebeu, só a caixa é escrita (146 ms no mesmo trecho). A importação zera a caixa, porque
depois dela os dois lados são iguais.

Um diff das chamadas de API entre os dois motores foi o que separou isto de um erro de CPU: 2,2
milhões de chamadas iguais (342 linhas diferentes, todas de atraso de timer). Se o jogo pede a
mesma coisa e o quadro sai diferente, quem difere é o nosso lado.

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

### O BMP comprimido em RLE

O decodificador de BMP recusava tudo que viesse comprimido, e essa recusa custava um jogo: o
**Disney All Star Cards** guarda quase todas as imagens dele em `BI_RLE8` e `BI_RLE4`. Sem elas o
jogo mostrava o mapa com um retângulo vazio no lugar da arte e, logo depois, ficava sem nada para
fazer — a sessão terminava sozinha.

O esquema é o do Windows: um par `(contagem, valor)` repete o valor; contagem zero abre uma fuga —
`0` fim de linha, `1` fim da imagem, `2` um salto `(dx, dy)` e, de `3` para cima, uma sequência
literal, sempre terminada em fronteira de palavra. No `RLE4` cada byte traz dois pixels, alternando
o nibble alto e o baixo. O que a imagem não cobrir fica no índice zero da paleta, como o Windows
faz.

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

### O bitmap compatível já é `IDIB`, e o Zenonia apresenta por `ITransform`

O Zenonia ficava com a tela preta desenhando o tempo todo, e eram dois erros encadeados.

**O canvas nascia sem tamanho.** O jogo cria um canvas 320x240 com `CreateCompatibleBitmap` e lê
`cx`, `cy` e `pBmp` direto da struct, sem `QueryInterface` — no BREW o bitmap compatível já é
`IDIB`. O nosso só publicava esses campos quando alguém pedia a interface, então o canvas era
0x0 para o jogo. Sem `pBmp`, o motor dele caía no caminho lento, pixel a pixel por `SetPixels`
(197 mil chamadas em trinta segundos). Agora o `CreateCompatibleBitmap` já expõe a DIB.

**O quadro ia para a tela por uma interface que não existia.** Para apresentar, o jogo pede ao
bitmap da tela o IID `0x01001029` e chama o slot 4 do objeto com `(x, y, pSrc, xSrc, ySrc, dx,
dy, pMatrix, nComposite)` — o `ITransform::TransformBltComplex`. Esse IID estava tratado como "o
terceiro IID do `IDIB`", por ser vizinho do `AEEIID_DIB_20`, e era respondido com o próprio
bitmap: o slot 4 caía no `NativeToRGB`.

A matriz é `{A, B, C, D}` em 8.8 aplicada em volta do centro do retângulo de origem, com `(x, y)`
no canto que ele teria sem transformação. O Zenonia passa `x = 160`, `y = 120`, canvas 320x240 e
escala 1,9: o resultado, 608x456, fica centrado na tela 640x480. A amostragem é pelo pixel mais
próximo, percorrendo o destino para não deixar buracos.

### O Action Hero 3D: `TransformBltSimple`, o magenta e o pedaço da `IImage`

O jogo parava na primeira apresentação pedindo o `TransformBltSimple`, e passando dele, o menu
saía coberto de folhas de fonte inteiras sobre fundo magenta. Eram três coisas.

**`TransformBltSimple`.** Mesmos argumentos do `Complex` até `dy`, e no lugar da matriz um
`uint16 unTransform`: rotação em quartos de volta nos bits 0-1, espelho em X no bit 2 e escala a
partir do bit 3. O jogo passa `unTransform = 8` com o mesmo `x = 160`, `y = 120` e canvas 320x240
do Zenonia — o quadro dobrado centrado na tela —, então `8` é a escala 2x. A matriz sai das flags e
o desenho é o do `Complex`. Escalas além de 1x e 2x ainda não foram vistas e ficam anotadas nas
suposições.

**A cor transparente padrão é o magenta.** O jogo desenha as palavras prontas de um atlas de
fundo magenta com `BitBlt(..., AEE_RO_TRANSPARENT)` sem nunca chamar `SetTransparencyColor`. É o
`RGB_MASK_COLOR` do BREW. Um bitmap sem cor pedida agora usa `0xF81F` no blit transparente e
anuncia esse valor no `ncTransparent` do `IDIB`. O `FillRect` continua comparando com zero, que é
o que o Bejeweled Twist precisa.

**`IPARM_SIZE`, `IPARM_OFFSET` e `IPARM_ROP`.** O texto dinâmico vem de uma `IImage` sobre o BMP
de 201x37 do `font.bar`, três linhas de 12 pixels. Cada letra é `SetParm(0, cx, 12)`,
`SetParm(1, x, 0|12|24)` e `Draw(x, y)`, e a imagem é preparada com `SetParm(3, 7)`. Os números
saíram desse uso — `SIZE` 0, `OFFSET` 1, `ROP` 3, vizinhos do `CXFRAME` 2 e do `NFRAMES` 4 que já
conhecíamos. Ignorados, cada letra punha a folha inteira na tela; agora o `Draw` lê só o pedaço, e
com o `ROP` transparente pula o magenta.

**O passo das linhas do `IDIB` é alinhado em 4 bytes.** Depois disso o logo da tela de título e
os retratos da história ainda saíam cortados. O jogo copia essas imagens pixel a pixel pelo
`pBmp`, e o laço dele anda de uma linha para a outra com `cx + 1` pixels quando `cx` é ímpar
(`tst cx, #1` antes do laço). O logo tem 149 de largura e os retratos 179, e o nosso buffer
guardava as linhas coladas, com `cx * 2` bytes: cada linha lida escorregava um pixel. Agora o
`nPitch` e o buffer usam a largura em bytes arredondada para múltiplo de 4, com um pixel de
enchimento no fim da linha de largura ímpar. Em largura par o passo continua `cx * 2`, então os
jogos que já funcionavam leem o mesmo buffer de antes.

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

`IDISPLAY_DrawText` escreve com uma fonte TrueType, escolhida nesta ordem:

1. **A do jogo**, quando o pacote traz um `.ttf` — a Z-Wheel traz a `tectoy.ttf`.
2. **A do aparelho**, em `fs:/shared/fonts/tectoy.ttf`, que é onde o firmware a procura.

A segunda não vem com o emulador, porque é da TecToy; ela vem no pacote da Z-Wheel, que é o
sistema do console. Quando o aparelho ainda não a tem e a Z-Wheel já foi aberta alguma vez, ela
é copiada do cache para lá (`archive::fonte_do_sistema`). Sem fonte nenhuma, o texto continua
indo só para o relatório.

A maioria dos jogos não traz fonte porque no console não precisava. O Kingdom Hearts pede
`0x8000`, o `AEE_FONT_NORMAL` do BREW, e sem a fonte do aparelho desenhava as cinco caixas do
menu e nenhuma das palavras — "New Game", "Load Game" e as outras só apareciam no relatório.

`GetFontMetrics` e `MeasureTextEx` devolvem números coerentes entre si. Sem nenhum, o Pac-Mania
nem monta a tela.
