# 06 — Vídeo 3D

## O contrato

O console tem uma **Adreno 130** (ex-ATI Imageon). Aqui a GPU é a CPU do host. O que importa é o
contrato: o jogo entrega vértices em coordenadas de objeto, matrizes, texturas e um punhado de
estados fixos, e espera um quadro de volta.

`video/rasterizer.rs` é o pipeline fixo clássico, **sem iluminação**: transforma, recorta contra o
plano próximo, divide pela perspectiva, mapeia para a tela e preenche triângulos com interpolação
corrigida pela perspectiva, teste de profundidade, mistura e teste de alfa.

## EGL e as duas formas

O BREW expõe o OpenGL ES de **duas** maneiras, e os jogos do console usam as duas:

| | Como é |
|---|---|
| `IEGL`/`IGLES` | interface COM normal, com `this` no primeiro argumento |
| forma antiga de `AEEGL.h` | sem `this`, retorno direto — `Interface::EglLegacy` e `GlLegacy` |

`eglGetProcAddress` devolve `aee::encode(Interface::Gles, slot)` depois de tirar o prefixo `gl`,
de modo que uma função obtida em tempo de execução cai no mesmo despacho das outras.

## A superfície

Deduzida do maior `glViewport` que o jogo pediu. E há uma regra que custou caro descobrir:

> **Quem nunca chama `glViewport` fica com a viewport padrão, que o OpenGL define como a
> superfície inteira.**

Deduzir a superfície de um conjunto vazio de viewports dava 1×1, e a apresentação esticava um
único pixel por toda a tela — foi a causa da "tela branca" do Zeebo Sports Peteca, e eu culpei a
janela antes de achar isso.

A segunda regra: **o `y` do `glViewport` conta a partir de baixo.** Os dois rasterizadores guardam
o quadro de cima para baixo, e por muito tempo usaram o `y` como se contasse do topo. Com viewport
de tela cheia (`y = 0`, altura igual à da superfície) as duas leituras coincidem, e por isso quase
nenhum jogo mostrava diferença. O Crash Nitro Kart mostrava: ele desenha o trecho seguinte da pista
através de um portal, com a viewport e o recorte no retângulo do portal. O trecho ia parar
espelhado na metade de baixo da tela, a área do portal ficava vazia, e quando o kart atravessava
e o portal virava tela cheia, o cenário "surgia de baixo para cima". Parecia distância de desenho,
e o limite de profundidade da recursão de portais do jogo não tinha nada a ver com isso.

A conversão fica num lugar só, `viewport_do_topo()`, com `y_topo = altura_da_superfície − y −
altura`, usada no desenho e no `draw_texture`. A viewport interna que o `import_rgb565_changes` da
placa monta já é contada do topo e não passa por ela.

`glDepthRange` também é atendido nos dois rasterizadores: o `z` normalizado vai para
`perto + (longe − perto) × (z/w × 0,5 + 0,5)`. O Crash alterna faixas para pôr o brilho do kart
por cima do resto, e antes a chamada era ignorada em silêncio.

## Duas unidades de textura — nos dois rasterizadores

A Adreno 130 tem duas unidades, e o OpenGL ES 1.1 exige ao menos duas. Por muito tempo o
pipeline desenhou só a unidade 0 e respondia `GL_MAX_TEXTURE_UNITS` = 1; hoje ele tem as duas
(`UnidadeDeTextura`): cada uma com textura ligada, liga-desliga, ambiente (`TexEnv`) e vetor de
coordenadas, e o vértice carrega a `uv1`. A unidade 1 age sobre o que saiu da 0 — no `GL_COMBINE`,
`GL_PREVIOUS` é a saída da 0 e `GL_PRIMARY_COLOR` a cor do vértice. Ela é amostrada sem mipmap.

**A ligação e os parâmetros são da unidade ativa.** O Resident Evil 4 termina cada bloco na
unidade 1, desligando a textura e trocando a ligada; quando a placa aplicava isso na textura base,
a vila saía branca. O `bound_texture` responde pela unidade ativa, que é a que um `glTexImage2D`
alcança.

Responder uma só era o que escondia os personagens do Dragon Vs Chicken — ver a seção dele.

## A matriz de textura

`GL_TEXTURE` **precisa** ser aplicada às coordenadas `uv`. Os jogos mandam UV em ponto fixo
(32767 é comum) e contam com a matriz para trazer ao intervalo certo. Sem isso as texturas do
Peteca viravam ruído — e o primeiro diagnóstico que dei, "falta mipmap", estava errado.

## Formatos de textura

| Módulo | Formato | Quem usa |
|---|---|---|
| `video/atc.rs` | ATITC (`GL_AMD_compressed_ATC_texture`) | o formato nativo do Adreno; o Boomerang Sports Dodgeball carrega **tudo** assim, sem uma única `glTexImage2D` |
| `video/paltex.rs` | `OES_compressed_paletted_texture` | paleta de 16 ou 256 cores seguida dos índices |

Duas armadilhas registradas em teste:

- **A paleta do `paltex` é little-endian.** Lê-la como big-endian dava faixas de arco-íris no
  logo do Double Dragon — plausível o bastante para passar despercebido.
- **O `level` dos formatos paletizados é não positivo**: zero é só o nível base, negativo diz
  quantos mipmaps vêm depois. Como usamos só o base, o que interessa é sempre o primeiro trecho
  de índices depois da paleta.

Os blocos ATITC são 4×4 texels, como o DXT1: duas cores e dois bits de índice por texel. A
diferença está no bit mais alto da primeira cor, que escolhe entre interpolar as quatro cores da
paleta ou reservar a primeira para o preto.

### O PNG do decodificador chega à textura no formato dele

Muitos jogos montam as texturas a partir do `IImageDecoder` de PNG: pegam o bitmap, leem os
campos do `IDIB` e copiam `pBmp` para um `glTexImage2D`. O **Alien Breaker Deluxe** mostra o
contrato na desmontagem: com `nDepth` 8 ele usa a paleta; fora isso copia `nDepth / 8` bytes por
pixel e escolhe `GL_RGB` para 3 e `GL_RGBA` para os outros. O nosso bitmap anunciava RGB565, 16
bits, e o jogo copiava dois bytes por pixel achando que eram quatro: os logos saíam brancos.

O bitmap do decodificador agora publica o `IDIB` no formato do próprio PNG — 32 bits em RGBA
quando a imagem tem alfa, 24 em RGB quando não tem, linhas contíguas e 8 bits por canal. A cópia
em RGB565 continua no mapa de superfícies para os nossos blits, e esse buffer fica fora da
sincronização. O mesmo erro era o "imagens lotadas de glitch" do **Heavy Weapon**, que agora mostra
o mapa da missão, e as imagens erradas do **Tork and Kral**.

### A cor corrente é limitada a [0, 1]

O OpenGL ES 1.1 limita a cor de `glColor4f`/`glColor4x` a [0, 1] no momento em que ela é
definida. O Alien Breaker Deluxe pinta com `glColor4f(255, 255, 255, a)` — é o valor de byte num
parâmetro de ponto flutuante —, e no console isso é branco puro, que deixa a textura intacta.
Guardando 255, o nosso modulador multiplicava a textura por 255, e o título e os menus saíam
estourados para o branco, com só as bordas escuras aparecendo.

### O `glScissor` recorta de verdade

Era aceito e ignorado, com a justificativa de que desenhar demais é o erro menos visível. Não é,
quando o jogo **conta** com o recorte: o Peggle desenha a folha de fontes inteira e aperta a
tesoura em volta de uma letra, o mesmo truque que o Pac-Mania faz com o recorte do `IDisplay`.
Ignorado, cada letra punha a folha inteira na tela — o menu e a mesa do jogo saíam cobertos de
alfabetos.

Nos dois motores o retângulo entra junto com a viewport, e com a mesma convenção: o `y` do
`glScissor` conta de baixo para cima, e a nossa superfície conta do topo. No rasterizador de
software ele aperta a caixa de cada triângulo, e por isso não custa nada por pixel; na placa é o
`glScissor` dela, multiplicado pela escala do anexo.

**E ele passa pela mesma conversão da viewport na proporção larga.** A tesoura chega em pixels do
console; o anexo é mais largo. Enquanto ela ia crua, uma tesoura na tela inteira — que é o que o
Resident Evil 4 e o Crash Nitro Kart ligam em jogo — cortava tudo além dos 640 do console, e os
lados que a proporção larga acabara de abrir ficavam com a cor de fundo do anexo. Quem não usa
tesoura, como o Raging Thunder 2, nunca viu a faixa.

**Na placa, o `y` da tesoura também é convertido para o topo** (`tesoura_no_anexo`, em
`video/gpu.rs`). A viewport passava por `viewport_do_topo` e a tesoura ia crua; na tela inteira
as duas leituras coincidem, e só um retângulo mostrava a diferença. O portal do Crash Nitro Kart
é um: viewport e tesoura no retângulo dele, e a tesoura caía na faixa espelhada — o portal saía
vazio e o cenário voltou a "subir do nada" ao atravessá-lo, o mesmo sintoma que a viewport já
teve, desde que a tesoura passou a ser respeitada. O rasterizador de software já convertia.

E, na proporção larga, a tesoura **se desloca** de `extra` e só cresce até as bordas que já
tocava. Alargá-la pela razão da viewport, como a primeira versão fazia, dava certo na tela inteira
e fazia o retângulo de um portal vazar para fora da moldura.

## Névoa

O `glFog*` era atendido em silêncio e o `GL_FOG`, ignorado. O Resident Evil 4 pede névoa linear
de dez a setecentas e oitenta unidades para separar o que está perto do que está longe, e sem ela
a cena saía toda com o mesmo brilho.

O fator sai da **distância em coordenadas de olho** — `|z|`, como o OpenGL permite em vez do
comprimento do vetor, e é o que toda implementação de função fixa faz. Ele é calculado na etapa de
vértice, que é comum aos dois rasterizadores, e viaja no `Vertex` como os outros atributos; no
fragmento entra **depois da textura e antes do teste de alfa**, mexendo só no RGB, que é a ordem
do OpenGL ES 1.1. Na placa é mais um atributo do vértice, e quem mistura é o shader.

Nas formas `x` o `GL_FOG_MODE` vem **inteiro**, não em ponto fixo: é uma enumeração, e convertê-la
como escala daria `0x2601/65536`, que não é modo nenhum.

O `GL_FOG_COLOR` tem **quatro** componentes e precisa estar no `gles::componentes`: fora dali o
`glFogxv` lia só o primeiro, e a névoa bege do Resident Evil 4 chegava com verde e azul zerados —
vermelha.

**A chave dos ajustes gráficos é de quem joga, não do jogo.** No console a névoa costuma esconder
o que a distância de desenho não alcançava, e aqui a cena chega inteira; quem prefere ver longe
desliga. Fica ligada por omissão — o jogo pediu a névoa, e em muitos ela é o efeito, não o
remendo.

## O Dragon Vs Chicken, a amostra do SDK

O `conftest.mod` do SDK 1.2.4 é um jogo de demonstração sobre o motor QX (malhas `.qxm`,
texturas `.qxt`, animações `.qxa`, partículas `.qxp`), e o motor escreve um `data/qx.log` com o
que carregou. Ele fala com o GL **só pela `IGL`**, a interface antiga sem `this`, e pede as
funções de buffer pelo `eglGetProcAddress`. Três defeitos nossos apareceram com ele:

- **O `eglGetProcAddress` devolvia função com `this`.** Um nome `gl*` resolvia para o slot da
  `IGLES11`, que lê os argumentos a partir do `r1`; quem chama um ponteiro de função C manda do
  `r0`. O `glGenBuffers(1, &nome)` tomava o ponteiro pela contagem e escrevia fora da memória. Hoje
  o nome é procurado na tabela da `IGL`, que ganhou no fim as funções que o `AEEGL.h` não tem (as
  de ponto flutuante, as de buffer e as de extensão); o começo, que é a vtable dela, não mudou.
- **O sufixo da extensão cai na função do núcleo.** Anunciamos `GL_ARB_vertex_buffer_object`, e o
  QX, ao achá-lo, pede `glBindBufferARB` e as outras cinco. O ponteiro era nulo; agora é o do
  `glBindBuffer`. O mesmo vale para `OES`.
- **Deslocamento zero num buffer é um vetor de verdade.** O `gles_draw` desistia quando o ponteiro
  de vértices valia zero, e com um buffer ligado o ponteiro é deslocamento — o QX dá
  `glVertexPointer(3, GL_FIXED, 0, 0)` para toda malha. Passando a usar buffer, o cenário inteiro
  sumiu, até isto (`ArrayPointer::em_uso`).

E o `glTexEnv` ganhou o **`GL_COMBINE`** (`TexEnv`, nos dois rasterizadores): função, três fontes e
operandos para o RGB e para o alfa, escalas e a `GL_TEXTURE_ENV_COLOR`, na unidade 0. Antes ele
caía no `GL_MODULATE`. O QX configura a combinação com a fonte na textura. Função e fonte
inválidas são recusadas, e as fontes aceitam o crossbar — ver a seção do Bóia Cross.

**Os personagens sumiam porque respondíamos uma unidade de textura só.** O motor pergunta
`GL_MAX_TEXTURE_UNITS` na abertura e guarda a resposta no gerenciador de extensões; o gerenciador
de iluminação confere o número (`0x2df08`: se for menor que dois, liga o bit `0x100`, que desliga
a iluminação), e os desenhos dele (`0x1ec20`) desistem com o bit ligado. Sem iluminação, cada
malha caía no caminho da cor por vértice — apontada para dentro da própria malha carregada, onde o
`dragon.qxm` guarda zeros —, e a textura em `GL_MODULATE` saía preta e transparente, reprovada no
teste de alfa. As sombras apareciam porque não passam por ali.

Com duas unidades, o motor ilumina pela textura: `DOT3_RGB` entre o mapa de relevo (`*_b.qxt`) e
a cor do vértice, que passa a levar a direção da luz, na unidade 0; e `ADD_SIGNED` com a textura
de cor, alfa pela `GL_TEXTURE_ENV_COLOR`, na unidade 1. O dragão, as galinhas e o cenário inteiro
— cachoeira, rochas, a iluminação — aparecem, nos dois rasterizadores.

## A água do Bóia Cross: enum inválido e o crossbar

A água da primeira pista (Santo André) saía azul-roxa e opaca; no longplay capturado do console
ela é marrom e turva. O rastreio do desenho dela (`sessao --trace=IGLES11`, código do jogo em
`0x3cf64`) mostra o QX montando um relevo em duas unidades:

```
unidade 0  textura 0xca (mapa de normais, o azul-lilás)
           GL_COMBINE_RGB = GL_DOT3_RGB
           GL_COMBINE_RGB = GL_TEXTURE0        <- queria GL_SRC0_RGB
           GL_SRC1_RGB    = GL_PRIMARY_COLOR   (o vetor da luz, ~(0,5; 0,95; 0,7))
unidade 1  textura 0xc9 (a cor da água, marrom)
           GL_COMBINE_RGB = GL_COMBINE         <- não é função nenhuma
           GL_SRC0_RGB    = GL_PREVIOUS
           GL_SRC1_RGB    = GL_TEXTURE1
           GL_COMBINE_ALPHA = GL_REPLACE, GL_SRC0_ALPHA = GL_TEXTURE1
```

Dois defeitos nossos se somavam:

- **Aceitávamos qualquer valor.** No driver, função ou fonte que não existe é
  `GL_INVALID_ENUM` e o valor de antes fica: a unidade 0 continua no `DOT3`, a 1 no `MODULATE`
  padrão. Aqui o `GL_TEXTURE0` virava a função e caía na multiplicação — o mapa de normais vezes o
  vetor da luz, que é a cor azul-roxa da água. O `TexEnv::define` agora recusa os dois.
- **`GL_TEXTURE0` e `GL_TEXTURE1` como fonte** (o `ARB_texture_env_crossbar`) caíam no
  `GL_PREVIOUS`. A unidade 1 multiplicava o anterior por ele mesmo, e a textura marrom nunca era
  amostrada. O crossbar não é do OpenGL ES 1.1, mas o aparelho o aceita: com a fonte recusada a
  unidade 1 ficaria no `GL_PREVIOUS` padrão, sem marrom nenhum, e a água do vídeo é marrom.

Nos dois rasterizadores as duas unidades agora amostram antes de qualquer uma combinar, e uma
fonte que aponta para unidade que não desenha faz a unidade passar o anterior adiante, que é a
regra do crossbar. Essa última não foi medida no aparelho — nenhum jogo testado a exercita.

## A distância de desenho dos Zeebo Extreme

O cenário que aparece do nada no Rolima **é decisão do jogo, não recorte nosso.** Medido na
corrida da primeira pista:

- A projeção é `glFrustumx` com `near = 1` e `far = 500`. Com o plano distante quatro vezes maior,
  as fotos dos mesmos instantes saem idênticas pixel a pixel: o jogo não manda nada além dos 500.
- A corrida não lê nada de volta do GL — nenhum `glGet*` —, então a escolha do que desenhar é toda
  da matemática dele, igual no aparelho.
- São uns 58 `Draw*` e 3 mil triângulos por quadro, estáveis.
- O rasterizador de software recorta no plano próximo e resolve o distante pixel a pixel, na
  profundidade; a placa recorta os dois. Nenhum dos caminhos descarta triângulo inteiro.

O critério está no `pak0.pakz` (um `PACK` de arquivos LZMA, com o índice no fim em entradas de 64
bytes: 56 de nome, a posição e o tamanho). O `infos/gamecartsinfo1p.txt` divide cada pista em
segmentos (`models/pista_serra0.xtreme` a `24`, cada um com a sua `.octree`) e diz o último
waypoint de cada um: o jogo desenha os segmentos em volta de onde o jogador está. Os níveis de
detalhe declarados ali (`400` perto, `1000` longe) são das malhas dos corredores, três por
corredor (`pers1_lod1` a `lod3`); a pista não tem uma versão de baixa qualidade separada.

Estender a distância seria um ajuste nosso por cima do jogo — achar e mexer na janela de
segmentos —, e não correção de fidelidade.

## O `glReadPixels` e o lado de cima

O Zeeboids desenha o boneco e **lê o quadro de volta** para montar a foto do perfil. A origem do
`glReadPixels` é o canto inferior esquerdo, e a primeira linha do resultado é a de baixo da tela;
as nossas superfícies contam do topo, então a leitura inverte a linha.

Isso vale para os dois motores, e é fácil de errar na placa: lá o quadro **também** está guardado
com a linha 0 no topo, porque o Y é virado no shader de vértice — é o que faz a leitura da tela
não precisar de espelho na CPU. Quem ler direto do `glReadPixels` da placa recebe, portanto, a
imagem já na convenção da superfície, que é a errada para o jogo. Lida crua, a foto do Zeeboids
saía de cabeça para baixo, e com ela o rosto do boneco em todo jogo que o usa depois. Tem teste
comparando a orientação nos dois rasterizadores.

## `GL_OES_draw_texture`

O blit de tela: um retângulo desenhado **em coordenadas de janela**, sem passar pelas matrizes.
É o caminho que um emulador usa para pôr a tela dele na tela do aparelho, e o console tem a
extensão.

O pedaço da textura vem do `GL_TEXTURE_CROP_RECT_OES`, quatro inteiros **com sinal** — largura ou
altura negativa espelha o eixo, que é como a extensão vira a imagem. Recorte zerado é a textura
inteira: desenhar nada seria pior que adotar o padrão óbvio.

A janela do OpenGL tem o zero **embaixo**, ao contrário da nossa superfície. Errar essa inversão
põe a imagem de cabeça para baixo, e num emulador de arcade isso passa por "funcionou" — daí o
teste ser sobre exatamente essa orientação.

As oito formas (`s`, `i`, `x`, `f` e as vetoriais) desenham a mesma coisa; só muda como o número
chega. Elas **não** fazem parte da vtable do `IGLES`: ficam no fim da tabela de nomes, e o jogo
chega a elas pelo `eglGetProcAddress`.

## As extensões do console

Dez portes de arcade — todos sobre o mesmo motor, um emulador de Neo Geo — desistiam da
inicialização gráfica e escreviam **"InitGLExtensions failed"** na tela. Eles não usam
`eglGetProcAddress` por nome: pedem as extensões por **`QueryInterface` no objeto EGL**, do jeito
que o shim `GLES_ext.c` do SDK faz.

| Interface | O que dá |
|---|---|
| `IEGLSurfaceManip` | escala, rotação, transparência e sobreposição de superfície |
| `IGLESImageonExt` | os extras do ATI Imageon sobre o OpenGL ES 1.0 |

As duas têm header completo no SDK vendorizado, com a tabela de métodos inteira. A V2 é
superconjunto da V1 **com o mesmo prefixo de vtable**, então uma tabela de nomes serve às duas
IIDs.

Quase tudo ali responde "consegui" sem fazer nada, e isso é deliberado: rotação, transparência e
camadas não mudam o que o jogo desenha, só como o console compõe o resultado. Recusar faria o
jogo desistir por causa de um recurso que ele nem chega a usar. Os buffers de vértice da ATI e da
Qualcomm são a exceção — ali responder sucesso sem guardar nada faria o desenho seguinte sair de
lixo, e recusar é mais honesto.

**A escala é a que faz trabalho de verdade.** No `SetSurfaceScale` o jogo declara o retângulo em
que desenha, e isso vale mais que a dedução por viewport: aqui ele *diz* o tamanho. Um emulador
de arcade desenha em 320×224 e deixa o console ampliar.

### E as extensões precisam ser anunciadas por nome também

Ter a interface não bastou. Os jogos também procuram, **por substring**, três nomes:

```
GL_OES_draw_texture          em glGetString(GL_EXTENSIONS)
GL_ATI_imageon_misc          idem
EGL_QUALCOMM_surface_scale   em eglQueryString(EGL_EXTENSIONS)
```

No binário deles as três aparecem **com espaço no fim** — o delimitador da busca. Enquanto o
`eglQueryString` devolvia vazio, o vídeo não inicializava por mais que as interfaces existissem.

### O nome que faltava valia um jogo inteiro

A lista tem uma quarta entrada hoje, `GL_ATI_texture_compression_atitc`, e ganhou uma quinta:
`GL_ARB_vertex_buffer_object`. Essa última custou um jogo enquanto ficou de fora, e vale contar
como, porque o sintoma não apontava para o GL em nada.

O **Prey Evil** aparecia no levantamento como "para no laço — salta para o endereço zero
(lr 0x000161d8)". Aquele endereço é um `blx r1` depois de `ldr r1, [r0, #0x274]`: ele chama um
ponteiro de função guardado num campo de um objeto dele. O campo é nulo, e quem o deixa nulo é o
próprio jogo, no `0x1ddc4`:

```
0x1ddd4  bl   …            <- monta a lista de extensões
0x1dde4  blx  r2           <- strstr(lista, "GL_OES_draw_texture")     -> achou
0x1de04  blx  r2           <- strstr(lista, "ARB_vertex_buffer_object") -> 0
0x1de0c  beq  #0x1de3c     <- e aí ele **não instala** a função
0x1de1c  str  r0, [r4, #0x274]
```

Duas buscas, e ele só instala a função de desenho se achar **as duas**. Depois chama o ponteiro
sem conferir. Ou seja: o jogo não tolera a extensão faltar, ele assume que ela existe — o
console a tem.

Com os objetos de buffer implementados e o nome na lista, ele sai de "quebra na volta 0" para
**358 quadros em seis segundos virtuais**, com o controle detectado
(`gamepadmgr.cpp:319 — 1 Joysticks connected`). Ele ainda não põe geometria na tela; isso é o
passo seguinte dele, e é outro problema.

O que a lista **não** ganhou é igualmente parte da decisão: o `point_size_array`, que vários
jogos também procuram, continua de fora porque dele só existe um `SUCCESS` que não faz nada.
Anunciar o que não existe é o que produz exatamente o defeito acima, de cabeça para baixo.

### Os objetos de buffer

`glGenBuffers`, `glBindBuffer`, `glBufferData`, `glBufferSubData`, `glDeleteBuffers`,
`glIsBuffer` e `glGetBufferParameteriv`. O conteúdo fica **no host**, num mapa por nome, e não
na memória do jogo: é onde um driver de verdade o guarda, e depois do `glBufferData` o jogo tem
o direito de reaproveitar o ponteiro que passou — guardar cópia nossa é o que faz esse direito
valer.

Duas regras decidem de onde um vetor vem, e elas **não são a mesma**:

- Para os vetores de vértice, cor, normal e coordenada, vale a ligação do `GL_ARRAY_BUFFER` no
  instante do `glVertexPointer`, não a do desenho. É por isso que o `ArrayPointer` guarda o
  nome do buffer: um jogo que sobe três malhas liga cada buffer, dá os ponteiros dela e só
  depois desenha — lendo a ligação corrente no desenho, as três sairiam do último buffer.
- Para a lista de índices do `glDrawElements`, vale a ligação **corrente** do
  `GL_ELEMENT_ARRAY_BUFFER`: é ela que decide se o último argumento é ponteiro ou deslocamento.

Com um buffer ligado, o "ponteiro" do vetor não é endereço nenhum — é deslocamento dentro do
buffer. Confundir os dois tem um sintoma característico e enganoso: `glVertexPointer(…, 0)` vira
leitura do endereço zero, ou seja, aparece como acesso inválido a `0x00000000` e não como erro
de GL. Um pedido que passe do fim do buffer sai **zerado** em vez de falhar, porque o OpenGL
deixa o resultado indefinido ali e derrubar o `glDrawElements` inteiro por causa de um vértice
seria pior — ver `fatia_do_buffer` e os testes dela.

## Os nomes de textura e de buffer: o Rally Master Pro

O Rally Master Pro parava depois de algumas corridas, na troca de cena, com "acesso inválido a
`0x0101010d`". A parada era determinística: um roteiro sem janela que abandona a etapa e a
recarrega a cada 30 s parava sempre no mesmo ponto, aos 194 s, no quinto ou sexto ciclo. O
endereço é `0x01010101 + 0x0c`: um campo lido de memória coberta de bytes `0x01`.

O que achou o culpado foi uma guarda temporária: 64 bytes preenchidos depois de cada bloco do
heap, e uma vigia de escrita nas guardas. Todas as escritas fora do lugar caíam no mesmo bloco,
um vetor de 248 bytes, e eram bytes `0x01` logo depois do fim dele. Na parada provocada pela
vigia, os registradores mostravam um `glBindTexture(GL_TEXTURE_2D, …)` a caminho, e a escrita
anterior tinha sido no índice `0xf8`: o jogo guarda uma flag por textura, indexada pelo **nome**
dela.

Os nomes saíam de um contador que só crescia. A especificação permite: o `glGen*` só precisa
entregar nomes fora de uso. Mas cada etapa apaga e recria as texturas da cena, então o nome
passava de 248 em algumas corridas, e a flag ia parar no objeto vizinho do heap. Com a guarda no
caminho, as escritas ficavam nela e o jogo não parava, o que fechou o diagnóstico.

Agora os nomes em uso ficam num `NomesGl`, um para texturas e outro para buffers, e o `glGen*`
entrega o **menor nome livre**: nome apagado volta a sair. O `glBind*` de um nome nunca gerado o
marca como usado, porque o OpenGL cria o objeto na primeira ligação. Com isso, o mesmo roteiro
rodou 680 s, cerca de 21 ciclos, sem parar e sem acesso inválido nenhum.

**Qual nome o driver do Zeebo entrega não foi medido.** Sabe-se só que ele reaproveita, porque o
jogo não para no console. O menor livre é o que mantém os nomes baixos, e nomes baixos são o que
o vetor de 248 supõe. O Mesa, por exemplo, entrega o maior em uso mais um, o que também
reaproveita quando a cena inteira sai.

O save state guarda os dois conjuntos. Um estado anterior a isso só tem o contador, e então os
nomes de 1 até ele ficam em uso, que é o que o contador garantia. O campo do contador continua
no arquivo, com o maior nome em uso: um motor antigo que abra um estado novo continua a contar
dali, sem repetir um nome vivo.

## Apresentação

`eglSwapBuffers` é o que marca um quadro. A interface só redesenha quando o contador de trocas
muda — ou, num jogo 2D que nunca apresenta pelo GL, a cada intervalo de relógio real.

Redesenhar a cada volta do laço afogava a janela: no Peteca são um milhão e meio de voltas para
algumas dezenas de quadros, e a janela nunca chegava a ser composta.

## O que falta

- **Mipmap.** Nada gera nem consome níveis além do base.
- **Iluminação.** Nenhum jogo testado pediu, mas não está lá.
- **`--dump-gl=DIR`** grava os quadros do GL um a um; é a forma de conferir 3D sem janela.
