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

## Apresentação

`eglSwapBuffers` é o que marca um quadro. A interface só redesenha quando o contador de trocas
muda — ou, num jogo 2D que nunca apresenta pelo GL, a cada intervalo de relógio real.

Redesenhar a cada volta do laço afogava a janela: no Peteca são um milhão e meio de voltas para
algumas dezenas de quadros, e a janela nunca chegava a ser composta.

## O que falta

- **Mipmap.** Nada gera nem consome níveis além do base.
- **Iluminação.** Nenhum jogo testado pediu, mas não está lá.
- **`--dump-gl=DIR`** grava os quadros do GL um a um; é a forma de conferir 3D sem janela.
