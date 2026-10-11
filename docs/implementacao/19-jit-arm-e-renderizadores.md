# 19 — JIT ARM e renderizadores 3D

## O sintoma

Alguns jogos 3D permaneciam entre 10 e 15 FPS mesmo quando o trabalho do host era pequeno. O
caso que revelou a causa foi a abertura de Kingdom Hearts: ela carrega `swv21brew.mod`, uma
extensão Superscape que implementa o próprio rasterizador RGB565/depth em ARM. Não passa pelo
nosso EGL/OpenGL ES; cada pixel da cena é um laço de instruções do guest.

O perfil da abertura mostrou que o problema não era o laço BREW:

| Intervalo | Timers/voltas | Instruções ARM |
|---|---:|---:|
| ~2 s virtuais | 50 timers, 150 voltas | ~25 milhões |
| ~8 s virtuais | 495 voltas | ~762 milhões |

Timers, sinais e callbacks não tinham volume para explicar a queda. A CPU era executada no TCG
do Unicorn, que preserva bem a compatibilidade mas não era rápido o bastante para esse
rasterizador escrito em ARM.

## O que não foi alterado

Trocar desempenho por alterar o tempo do jogo esconderia o defeito e quebraria física, animação
e input. Portanto o JIT usa o mesmo `Machine` e a mesma sequência da sessão:

```text
advance → timers vencidos → callback do timer → sinais → callbacks pendentes
```

O relógio virtual também permanece igual:

```text
utime = clock_us + instruções / 528
```

`528` é a frequência, em MHz, do ARM11 do MSM7201A. O salto de ociosidade continua limitado a
um vblank; ele só representa tempo em que o console estaria esperando. O JIT reduz tempo de
parede do host, não cria tempo virtual nem dispara callbacks extras.

## Backend Dynarmic

`cpu/dynarmic.rs` implementa o mesmo `CpuBackend` do Unicorn, agora com Dynarmic A32 ARMv6K.
Ele é o backend da sessão gráfica normal; `zeebx bench arquivo.zip --seconds=N` mantém uma
bancada sem janela para medir a ROM inteira.

O contrato com o restante do emulador é preservado:

- registradores, memória e contador de instruções continuam acessados pelo `CpuBackend`;
- as vtables BREW (`0xf0000000..0xf0ffffff`) e o retorno-sentinela continuam endereços sem
  código; o fetch do JIT para ali para que o despachante Rust atenda a API;
- a parada na vtable não conta como instrução ARM executada e o PC volta a ficar no endereço que
  o Unicorn exporia;
- o CPSR começa em modo usuário (`0x10`), como no applet do Zeebo;
- `SVC #0xAB` atende `SYS_WRITEC` e `SYS_WRITE0`, mantendo o semihosting de Peggle e Zuma.

## Coerência de código e de superfícies

O JIT pode reutilizar um bloco compilado. Quando o guest ou uma API host escreve uma página que
já foi buscada como código, o bloco precisa ser invalidado antes da próxima entrada. O backend
rastreia somente páginas executadas e invalida apenas essas páginas; invalidar por qualquer
escrita seria desastroso, pois um renderizador RGB565 escreve milhões de pixels por quadro.

Os buffers de bitmap/EGL continuam no espaço de memória do guest. O Dynarmic tem a mesma vigia
de escrita do Unicorn (`watch_dirty`): a callback de escrita do guest liga o sinalizador da faixa,
escrita do host não liga. Um envoltório com o menor intervalo que contém todas as faixas deixa a
escrita comum — a imensa maioria — em duas comparações.

Por um tempo a vigia respondeu sempre "sujo", o que parecia só conservador e não era:

- **Custava o jogo inteiro.** Cada chamada que desenha importava todas as superfícies. No
  Pac-Mania, 100 mil `IIMAGE_Draw` somavam 22 s só lendo buffers intocados; 8 s virtuais não
  terminavam em um minuto. Com a vigia, levam 6,3 s.
- **Escondia um erro de correção.** "Sempre sujo" só é seguro se o buffer do guest nunca estiver
  atrás da cópia do host, e estava: um bitmap novo no endereço de um liberado herdava o buffer
  com os pixels do morto (ver `dib_herdados` no doc de vídeo 2D). As letras do Tekken 2 saíam
  como blocos só neste motor.

## Interworking: o bit 0 do endereço

O despachante retoma o guest no `lr`, e o `lr` de uma chamada feita de código Thumb traz o bit 0
ligado. O Unicorn trata isso no `emu_start`; o Dynarmic não, e o `run` precisa traduzir: bit 0
ligado liga o `T` do `CPSR` e sai do endereço. Sem isso o Zenonia, que é Thumb, voltava de toda
API um byte adiante, e o núcleo abortava com `Unhandled instruction 0xF8F9F5F0` — metade de um
`bl` com metade do seguinte.

## Validação feita

O primeiro quadro de Kingdom Hearts em 2 s virtuais foi gerado pelos dois núcleos e os BMPs têm
o mesmo SHA-256:

```text
d5be7b5c0589e5d8af8bb2d0d8652d745449262d65b34675133384136566b4bc
```

Na mesma máquina de desenvolvimento, a bancada Dynarmic executou:

| Carga | Tempo virtual | Tempo real | Velocidade |
|---|---:|---:|---:|
| Kingdom Hearts | 2,001 s | 0,305–0,331 s | 604–657% |
| Kingdom Hearts | 8,004 s | 3,555 s | 225% |

No trecho de 8 s foram cerca de 762 milhões de instruções, a 214,5 MIPS. A folga acima de 100%
é intencional: a sessão aplica o limitador de velocidade contra o relógio real, como fazia antes.

Além dos testes de unidade de CPU (execução, parada em API, retorno e semihosting), a suíte
completa deve passar com:

```bash
cargo test --release
```

## Como investigar uma regressão visual

1. Reproduzir no mesmo instante virtual, de preferência com `--keys` e dump de BMP.
2. Comparar Unicorn e Dynarmic, não apenas FPS: hash idêntico prova o estado de pixels daquele
   instante; hash diferente pede captura do PC/rotina que escreve a superfície.
3. Se a diferença aparece só após trocar de menu, conferir primeiro invalidação de código em
   RAM e leitura de memória atualizada pelo host; não compensar mexendo em timers ou `utime`.
4. Manter a equivalência antes de otimizar a sincronização de superfícies.

O próximo trabalho é ampliar esses testes para roteiros interativos de CNK3D, Tekken e Kingdom
Hearts, especialmente menus e telas de opção, onde código e tabelas mutáveis são mais comuns.

## Resolução interna do 3D (upscale)

Com o rasterizador da placa ligado, a aba gráfica oferece **resolução interna de 1x a 6x**
(`graphics.resolucao_interna`). O jogo continua vendo 640×480; só o anexo de cor e profundidade do
`GpuState` cresce.

- **Desenho.** `destino()` cria o anexo com `medida × escala`, e `aplica()` multiplica a viewport
  pelo fator, depois de convertê-la para contada do topo (`viewport_do_topo()`; o `y` do
  `glViewport` conta de baixo, ver o [06](06-video-3d.md#a-superfície)). Tudo o que o jogo passa em pixels — viewport, `draw_texture`, o quadrilátero do
  `import_rgb565_changes` — continua em pixels do console e é escalado só na hora de ir para a placa.
- **Leitura.** `liga_para_leitura()` reduz o quadro grande na placa (`glBlitFramebuffer` com filtro
  linear) para um framebuffer do tamanho do console antes de qualquer leitura: `frame_rgb565`
  (o `GetColorBufferQUALCOMM` e a cópia para a tela) e `read_rect` (o `glReadPixels`). Ler o quadro
  grande seria mover o quadrado do fator em bytes para jogar quase tudo fora.
- **Janela.** `Machine::quadro_na_placa()` devolve a textura grande **só quando a tela é exatamente o
  que o último `eglSwapBuffers` pôs lá**: a contagem de escritas da tela é guardada no `present_gl`, e
  qualquer desenho 2D depois disso a muda. Nesse caso — HUD pelo `IDisplay`, caixa de mensagem, a
  Z-Wheel, que compõe o 3D na CPU — a janela fica com a tela de 640×480. O `Pintor` desenha a textura
  do rasterizador direto, no mesmo contexto que o egui usa, com o recorte da superfície.
- **Teto.** O fator é limitado pelo `GL_MAX_TEXTURE_SIZE`/`GL_MAX_RENDERBUFFER_SIZE` da placa.
- **Sem janela.** `zeebx sessao <zip> --placa --escala=N --dump=…` grava também `*.grande.bmp` e diz se
  a janela mostraria o quadro grande. Medido no Crash Nitro Kart: 20 s virtuais em ~2,9 s reais nas
  escalas 1 e 3, e o contorno dos modelos sai liso em 1920×1440.

O rasterizador de software ignora a opção: o custo cresceria com o quadrado do fator. Jogos 2D não
ganham nada, porque a imagem já sai pronta em 640×480.

## Proporção larga, experimental

`graphics.proporcao` (Nativa, 16:9, 16:10, a da janela), só no rasterizador da placa. Não estica:
**renderiza** a cena em perspectiva mais larga, como o hack de widescreen do Dolphin.

- **O anexo ganha colunas dos lados** (`GpuState::extra`, 106 por lado em 16:9 com 480 linhas),
  só quando a superfície vai à tela inteira — um pbuffer menor não tem lados para abrir.
- **A superfície esticada também abre.** O Quake desenha em 320×400 e declara isso pelo
  `EGL_QUALCOMM_surface_scale`, que o aparelho amplia até 640×480. O `GlState` guarda essa
  diferença (`superficie_esticada`, ligada pelo `SetSurfaceScale` e desligada por um pbuffer), as
  colunas a mais são contadas em pixels da superfície (53 por lado em 16:9) e a proporção entregue à
  janela é a da tela. Antes, a superfície menor que o quadro não abria nada, e com a resolução
  interna acima de 1 o quadro ia à janela na proporção da superfície — 0,8, estreito e menor, nos
  dois modos.
- **Perspectiva abre, o resto se desloca.** Um lote com projeção em perspectiva
  (`GlState::projecao_em_perspectiva`: `p[11] ≠ 0` e `p[15] = 0`) tem o `x` de recorte
  multiplicado por `k = 640 / (640 + 2·extra)` e a viewport alargada na razão inversa, em torno do
  mesmo centro deslocado. O que estava na tela cai no mesmo pixel de antes, e o que o recorte
  cortava aparece nos lados. HUD em ortográfica, `draw_texture` e a composição 2D só vão para o
  centro.
- **O jogo continua vendo 640×480.** A leitura (`liga_para_leitura`) copia só o centro, então
  `glReadPixels` e a cópia para a tela saem idênticos ao nativo — conferido no Crash Nitro Kart.
- **A janela** pinta o quadro largo na proporção dele (`QuadroNaPlaca::proporcao`) quando a tela é
  3D pura; com 2D por cima, volta ao 4:3.
- Sem janela: `zeebx sessao <zip> --placa --proporcao=16:9 --dump=…` grava o `*.grande.bmp` largo.

Problemas esperados, e por isso experimental: objetos que surgem nas bordas (o jogo não desenha o
que acha que está fora da tela), a imagem alternando entre largo e 4:3 quando há 2D por cima, e
jogos que desenham o HUD em perspectiva, que se deformam junto com a cena.

## Antialias (MSAA) e filtro anisotrópico

Também só no rasterizador da placa, na aba gráfica, desligados por padrão.

- **MSAA 2x/4x/8x** (`graphics.antialias`). Com amostras, o `destino()` monta um segundo framebuffer
  de desenho com cor e profundidade multiamostradas; a textura `cor` passa a ser só o resultado.
  `resolve()` copia as amostras para ela antes de qualquer leitura (`liga_para_leitura`, o quadro
  grande) — e a janela, que pinta a `cor`, a recebe resolvida porque o `present_gl` lê o quadro a
  cada `eglSwapBuffers`. O número de amostras é limitado pelo `GL_MAX_SAMPLES`. Combina com a
  resolução interna. Transparência recortada por teste de alfa não é suavizada.
- **Anisotrópico 2x–16x** (`graphics.anisotropico`), pelo `GL_TEXTURE_MAX_ANISOTROPY` da extensão
  `EXT/ARB_texture_filter_anisotropic`, aplicado em `parametros()` e reaplicado a todas as texturas
  quando muda. Sem a extensão, fica desligado.
- Sem janela: `zeebx sessao <zip> --placa --msaa=4 --aniso=16`.

Medido: no Crash Nitro Kart, MSAA 4x suaviza visivelmente o contorno dos modelos. No Need for Speed
em 640×480 o anisotrópico quase não muda a imagem (diferença média de 0,2 nível): as texturas dele
vêm sem mipmaps, e sem cadeia o filtro tem pouco a fazer.

## Por que não há "overclock" da CPU emulada — nem "underclock"

O relógio virtual anda com as instruções executadas (528 por microssegundo, o ARM11 do console), e
o desenho em GL não custa tempo virtual. Medido no Need for Speed, na corrida: ~60 quadros por
segundo virtual a 100% e a 200% de CPU — o jogo já bate no teto do retraço de 60 Hz, porque o que o
deixava lento no aparelho era a GPU, que aqui não é emulada. Uma opção de CPU mais rápida foi
experimentada e retirada por não mudar nada. O que limita a fluidez no emulador é o host conseguir
manter a velocidade real (50 s virtuais em ~44 s reais no `sessao`, sem janela).

O underclock (menos instruções por microssegundo, como o controle de clock dos cores de PS1)
cai pelo mesmo motivo, na direção oposta: aqui não existe orçamento de ciclos por quadro — o jogo
executa até apresentar, e o trabalho por quadro é determinado por ele. Cortar o divisor não corta
trabalho do host; só recarimba o tempo. Jogo de quantum fixo não muda nada; jogo por delta de
tempo dá passos maiores (mais rápido, não mais leve). E ainda quebraria dois contratos: os
save-states guardam prazos absolutos (taxa nova os desloca) e o freio `Ahead`/`run_frame` passaria
a dormir pelo motivo errado. O que alivia host fraco do lado da CPU continua sendo gastar menos
por quadro (laço de espera pulado, JIT, chamadas) — nunca fingir um clock menor.

## Desempenho: o Quake em câmera lenta

O Quake executa 132 milhões de instruções por segundo virtual, um quarto da CPU do console, e faz
umas 370 draw calls por quadro: um `glDrawArrays` em leque para cada face. Medido com amostras de
pilha pelo `gdb` na fase, metade do tempo era o JIT, um terço mandar desenho à placa e um sexto ler
o quadro de volta. Sem janela o jogo ficava em 0,88× do console, e três coisas mudaram:

- **Anel de vértices.** Cada desenho redefinia o buffer (`buffer_data`) e os ponteiros de
  atributo, 4 µs por chamada. Agora o buffer é um anel (`GpuState::anel`): o desenho grava na faixa
  seguinte com `buffer_sub_data` e desenha a partir dela pelo `first`; os ponteiros ficam no VAO.
- **Lotes.** Desenhos seguidos com o mesmo `fill` e a mesma perspectiva viram triângulos soltos
  num lote (`GpuState::lote`) que vai à placa numa chamada só. Leque e faixa são desmontados na
  ordem do OpenGL, o que mantém a orientação e o descarte por face. O lote é descarregado com o
  estado em que foi juntado sempre que o `fill` muda e antes de tudo que mexe na placa fora do
  desenho — limpar, subir ou apagar textura, mudar parâmetro de textura, ler o quadro, trocar
  destino, escala, proporção ou superfície.
- **Uniformes em cache.** A posição de cada uniforme é procurada uma vez e o valor só é reenviado
  quando muda (`Uniformes`). Rende perto de um microssegundo por desenho.

Com isso o Quake foi a 1,25× sem janela, com a mesma imagem — conferido também no Dragon Vs
Chicken e no 16:9.

**E a janela rodava um quadro do jogo por quadro dela.** O `Session::step` voltava no primeiro
`eglSwapBuffers`, e com a janela abaixo de 60 quadros por segundo o jogo andava na mesma proporção:
liso, sem engasgo, em câmera lenta. Atrasado mais de um quadro em relação ao relógio do mundo, ele
agora roda até quatro quadros por volta e só o último vai à tela. Atraso acima de 250 ms é perdoado
em vez de recuperado — pausa, carregamento e janela arrastada param o relógio virtual, e correr
atrás disso depois faria o jogo disparar.

## Desempenho: a corrida do Need for Speed abaixo da velocidade do console

Medido com `zeebx sessao <zip> --placa --escala=3 --perfil=45000`, que liga o perfil de API só a
partir do instante dado e conta as instruções ARM no intervalo. Na corrida, de 45 s a 59 s virtuais:

| | velocidade | tempo real | instruções/s real | troca de quadro |
|---|---|---|---|---|
| antes | 69% do console | 20,3 s | 132 milhões | 4,9 ms/quadro |
| leitura em RGB565 | — | — | — | 2,1 ms/quadro |
| + tabela de páginas | **108%** | 12,9 s | 207 milhões | 2,6 ms/quadro |

O jogo executa 192 milhões de instruções por segundo virtual na corrida — 36% da CPU do console; o
que o deixava lento no aparelho era a GPU. As mesmas 2.682 milhões de instruções e os mesmos quadros
nas duas medições: o comportamento não mudou, só o custo.

- **Leitura do quadro em RGB565.** O `eglSwapBuffers` copia o 3D para a tela do console. A leitura em
  RGBA com conversão na CPU custava 1,8 ms por quadro, seis vezes a leitura em si; agora a placa
  entrega `GL_RGB`/`GL_UNSIGNED_SHORT_5_6_5` (no GLES, onde isso não é garantido, fica o RGBA), a
  mesma medida vira cópia, e o `present_gl` reaproveita o buffer do quadro anterior.
- **Tabela de páginas do Dynarmic.** Toda leitura e escrita do código recompilado saía para uma
  callback Rust com `RefCell` e busca de região. A tabela (`DynarmicCpu::tabela`, 2^20 ponteiros)
  aponta direto para as páginas das regiões graváveis. Continuam nas callbacks, com a entrada nula:
  regiões só de leitura (as vtables, que são como o JIT devolve o controle nas APIs), páginas já
  executadas (anuladas na primeira busca de código, para a escrita nelas invalidar o bloco), páginas
  com vigia de escrita (anuladas no `watch_dirty` e restauradas no `unwatch_dirty`) e a página
  parcial do fim de uma região. A execução ARM foi de ~176 para ~350 milhões de instruções por
  segundo. Crash Nitro Kart saiu idêntico pixel a pixel, e a Z-Wheel segue lançando jogos.
- **Janela sem `gpu_present`.** A textura do egui é subida só quando a tela muda (série e escritas), e
  numa passada só; antes eram duas chamadas a `to_argb` e duas cópias por repaint.
