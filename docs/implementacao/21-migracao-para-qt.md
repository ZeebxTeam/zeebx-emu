# 21 — A migração da interface para Qt

Plano de troca do `egui`/`eframe` por Qt 6, com o [cxx-qt](https://github.com/KDAB/cxx-qt) como
ponte entre Rust e Qt. Este documento é **plano**, não descrição do que existe: cada fase, quando
concluída, deve ser movida para o documento do assunto (o [10](10-interface.md) em especial) e
marcada aqui como feita.

## Estado

| Fase | Situação |
|---|---|
| 0 — prova de viabilidade | feita no Linux (`frontends/classical-standalone/src/qt/`, feature `ui-qt`); no `qt.yml`, Linux, Windows x64 e macOS Intel passaram, e o macOS ARM64 espera a nova execução com o Qt 6.11 |
| 1 — desacoplar | feita: `Viewport` do pintor, tradução de teclas, `ui::partida::Partida` e `ui::entrada::EntradaDoDesktop` |
| 2 — estrutura | feita: núcleo da interface Qt, `Biblioteca` como modelo, janela principal e janela do jogo |
| 3 — janela do jogo | feita: textura da placa sem cópia, enquadramento, faixas de parada e de depuração, aviso de calibração, título e modo da janela |
| 4 — biblioteca | feita: grade e slider, capas e nomes do acervo, busca, navegação pelo controle |
| 5 — configurações | feita: as seis abas, o desenho do controle com clique pela silhueta, captura, eixos, Boomerang, Discord e atualizações |
| 6 — janelas auxiliares | feita: saves, log da execução, aviso de abertura e aviso de versão nova |
| 7 — conferência | feita: as chaves de texto das duas interfaces comparadas, teste de que toda chave usada existe, e o que faltava no Qt (ícone, `app_id`, tamanhos mínimos, tela cheia na biblioteca) |
| 8 — corte do eframe | segunda etapa feita: a feature `ui-qt` vem ligada, o Qt é a interface padrão e o egui fica em `zeebx egui`. O corte espera uma release com o Qt |
| 9 — build e empacotamento | feita: o `release.yml` monta o AppImage, o NSIS e o `.dmg` com o Qt embutido, e o `.deb` com o Qt do sistema; o `ci.yml` instala o Qt nos seis alvos; o `qt.yml` faz o mesmo que a release e abre cada instalador antes de sair |

## Onde o egui está de verdade

O ponto de partida era a suposição de que o egui mora só em `src/ui/`. Não mora — e o que vaza é
o que mais pesa na migração:

| Acoplamento | Onde | Peso |
|---|---|---|
| ~~`eframe::glow` no núcleo~~ | resolvido na 0.3.0: o núcleo usa o `glow` direto | — |
| **O rasterizador na placa roda no contexto de GL do eframe** | `App::gl` → `Session::start_with` → `GpuState::novo` | estrutural: ver abaixo |
| Nomes de tecla | `Source::Key { name }` guarda o `egui::Key::name()` | compatibilidade do `settings.json` |
| `egui::ViewportBuilder`/`ViewportCommand` | `ui/settings.rs` | mover para o lado do egui |
| `egui::epaint::ViewportInPixels` | `ui/gpu.rs` (`Pintor`) | trocar por um tipo próprio |
| O desenho em si | `ui/app.rs` (3.400 linhas), `ui/app/vitrine.rs` | reescrita |

`input/bindings.rs` **não** usa `egui::Key`: as teclas já são guardadas por nome. Mas o nome é o
do egui, e é o que está gravado na configuração de quem já usa o
emulador. O Qt precisa de uma tabela `Qt::Key → nome do egui`, com teste, e não de um formato
novo. **E a comparação é pela tecla, nunca pelo texto:** o mesmo botão aparece com duas grafias
no arquivo — o mapeamento de fábrica grava `"ArrowUp"`, e a tela de controles grava o
`egui::Key::name()`, que é `"Up"`. O egui e o headless convertem os dois lados com
`egui::Key::from_name`, que aceita as duas. A ponte Qt comparava o texto, e o direcional do teclado
não chegava aos jogos — só à Z-Wheel, que navega pelas teclas do BREW.

O `glutin` e o `ab_glyph` **ficam**. O primeiro dá o pbuffer do `run` e do `bench`, onde toda a
medição é feita; o segundo é o `IDISPLAY_DrawText`. Nenhum dos dois é da interface.

### O contexto emprestado

`GpuState::novo` recebe o contexto da janela em vez de criar um, e o comentário ali diz por quê:
o núcleo roda na thread da interface, e um contexto próprio tornado corrente nela desliga o do
eframe — a janela congela. No Qt a mesma coisa vale, com uma saída melhor: o `QOpenGLContext`
tem `makeCurrent`/`doneCurrent` explícitos, e o Qt Quick torna o contexto dele corrente antes de
desenhar. Então o rasterizador pode ter um contexto **próprio, fora de tela e compartilhado** com
o do Qt Quick (`Qt::AA_ShareOpenGLContexts`):

- ele sobrevive à janela do jogo fechar, o que a Z-Wheel reabrindo jogos agradece;
- a textura grande do `quadro_na_placa()` é visível ao scene graph sem cópia;
- o passo fica `makeCurrent → Session::step → doneCurrent`, sem disputar contexto com ninguém.

## Por que Qt Quick e não Widgets

O cxx-qt gera `QObject`s em Rust — propriedades, sinais, slots, herança de `QAbstractListModel` e
de `QQuickPaintedItem` — para serem usados a partir de **QML**. Não há bindings de QtWidgets: cada
`QDialog` ou `QListWidget` seria C++ escrito à mão. O desenho fica em QML, o estado e a lógica em
Rust, e a cola em C++ se limita ao que o cxx-qt-lib não cobre:

1. **A preparação do GL**, antes do `QGuiApplication`: `QQuickWindow::setGraphicsApi(OpenGL)` (no
   macOS o padrão é Metal, no Windows D3D), `QSurfaceFormat` 3.3 core — o mesmo contrato que o
   eframe entrega hoje e que os shaders do `Pintor` e do `GpuState` esperam — e
   `Qt::AA_ShareOpenGLContexts`.
2. **O carregador de funções de GL** (`QOpenGLContext::getProcAddress`), para o
   `glow::Context::from_loader_function_cstr`.
3. **A tela do jogo com textura nativa**: um `QQuickItem` com `updatePaintNode`, porque não há
   binding de `QSGNode`.
4. **O provedor de capas** (`QQuickImageProvider`).

### Uma thread só, e o render loop `basic`

O emulador continua na thread da interface ([01](01-arquitetura.md#o-emulador-roda-na-linha-da-interface)).
No Qt Quick isso tem uma consequência: o render loop `threaded`, padrão no Linux com Mesa, desenha
o scene graph em outra thread — que leria a textura do rasterizador enquanto a thread principal já
escreve o quadro seguinte nela. O `basic` desenha na thread principal, como o egui fazia. Fica
fixado até haver motivo medido para trocar; se houver, a textura precisa ser copiada no
`updatePaintNode`, o único instante em que as duas threads estão paradas juntas.

O passo do jogo acompanha o quadro da janela — um `FrameAnimation` do QML chama o passo a cada
quadro, no ritmo do vsync, como o `request_repaint` do egui. Um `QTimer` de intervalo zero seria
uma CPU a 100% à toa.

### Licença

O projeto saiu do `GPL-2.0-only` para o **`GPL-2.0-or-later`**. A dependência que prendia a
licença na versão 2, o unicorn, foi retirada, e o backend de CPU padrão é o Dynarmic; o
`Cargo.toml` de cada pacote e o [`AGENTS.md`](../../AGENTS.md) já dizem isso. O `LICENSE` continua
sendo o texto da GPLv2, que é o que o "ou posterior" pede.

O "ou posterior" é o que libera a migração: o binário que linka o Qt é distribuído, como um todo,
sob a GPLv3, que o código do Zeebx permite. E com GPLv3 a questão acaba para os módulos em uso: `QtCore`, `QtGui`, `QtQuick`, `QtQml` e
`QtNetwork` declaram, no Qt 6.11 instalado,

```text
SPDX-License-Identifier: LicenseRef-Qt-Commercial OR LGPL-3.0-only OR GPL-2.0-only OR GPL-3.0-only
```

e tanto a LGPLv3 quanto a GPLv3 combinam com GPLv3. Cada módulo novo ainda precisa ter o cabeçalho
conferido, porque nem todo módulo Qt tem a mesma lista. O cxx-qt é MIT/Apache.

## As fases

### 0 — Prova de viabilidade

Antes de migrar tela nenhuma, provar os quatro pontos que podem inviabilizar o plano:

1. uma janela QML com texto vindo de uma propriedade Rust;
2. o quadro RGB565 de uma `Session` desenhado por um `QQuickPaintedItem` com
   `QImage::Format_RGB16` — que é exatamente RGB565, sem conversão na CPU;
3. um `glow::Context` tirado de um contexto do Qt, com o `GpuState` rodando nele;
4. o build nos três sistemas.

Fica atrás da feature `ui-qt` e do subcomando `zeebx qt [jogo]`, sem tocar no caminho
do egui:

```bash
cargo run --release -p zeebx-classical-standalone --features ui-qt -- \
    qt "Crash Bandicoot Nitro Kart 3D (Brazil) (Es,Pt).zip"
```

O código mora em `frontends/classical-standalone/` (`src/qt/`, `qml/`), e não no núcleo: o núcleo
é também o do core Libretro e do Android, e janela é coisa de frontend. A feature `ui-qt` é do
standalone e fica fora de todo job do `ci.yml` e do `release.yml`, que compilam por `-p` sem
`--all-features`.

O que a prova mostrou, no Linux com Wayland e NVIDIA (driver 615, Qt 6.11, cxx-qt 0.10):

- **Os pontos 1 a 3 funcionam.** O Pac-Mania aparece pelo `Format_RGB16`, e o Crash Nitro Kart
  roda com o `GpuState` num contexto fora de tela do Qt, compartilhado, passando pelo `glow` —
  o mesmo `GpuState`, sem mudança nenhuma no núcleo.
- **O formato precisa dizer `setRenderableType(OpenGL)`.** Sem isso, o pedido de 3.3 core volta
  `EGL_BAD_MATCH` no Wayland e o Qt Quick aborta ao abrir a janela; no X11 passava. O `qFatal`
  vai para o journal, não para o terminal — `QT_FORCE_STDERR_LOGGING=1` o traz de volta.
- **O construtor de um item precisa de `impl cxx_qt::Initialize`.** Sem ele o cxx-qt gera um
  construtor que repassa o pai `QObject*` à base, e o `QQuickPaintedItem` quer um `QQuickItem*`.
- **A cola em C++ foi pequena:** a preparação do GL e o contexto fora de tela, umas 80 linhas em
  `src/qt/cpp/`. As funções de GL passam ao Rust como `rust::Str`, não `const char*`.
- **O contexto fora de tela precisa ser destruído à mão, e na ordem certa.** Deixado para os
  destrutores estáticos do C++, ele morria depois do `QGuiApplication`, e **fechar a janela
  terminava em SIGSEGV** dentro do `~QOffscreenSurface` — nenhum teste pegava, porque todos
  terminavam pelo `timeout`. A ordem agora é: a engine (a tela solta a sessão com o contexto
  vivo), o contexto (`gl_destroi`), o `QGuiApplication`. Medido fechando a janela pelo QML: saída
  0 nos dois rasterizadores.

**O `qt.yml`** (ponto 4) passou no Linux, no Windows x64 e no macOS Intel com o Qt 6.8.3. O macOS
ARM64 quebrou dentro de um cabeçalho do próprio Qt: o `qyieldcpu.h` do 6.8.3 chama `__yield()`, que
o clang do Xcode novo só aceita com o `<arm_acle.h>`. O 6.11 tenta `__builtin_arm_yield` antes, e o
workflow passou a usá-lo — **menos no Windows, que fica no 6.10**. A partir do 6.11 o repositório do
Qt para Windows tem uma pasta por arquitetura (`qt6_6112/qt6_6112_msvc2022_64/`), e o índice único
que o `aqtinstall` 3.3.0 procura dá 404; o passo falhava com "Failed to locate XML data". O 6.10
ainda tem o índice antigo, e o `__yield` é defeito só do ARM. **Isso vale para a fase 9 também:**
empacotar para Windows com o `aqtinstall` prende o Qt no 6.10 até ele entender o formato novo.

Falta a entrada, que existe — teclado pelo nome do
egui e controles pelo `gilrs`, com o mapeamento do `settings.json` — mas não foi exercitada à
mão. O quadro grande do `quadro_na_placa()` ainda passa pela leitura em RGB565: embrulhar a
textura sem cópia é da fase 3.

### 1 — Desacoplar o núcleo do eframe

**O escopo mudou com a 0.3.0.** O `egui` não sai do núcleo: o frontend Android desenha com ele, e o
headless usa o `egui::Key` como nome canônico de tecla. O que a migração troca é a janela do
desktop — o `eframe` do `classical-standalone` —, e o egui que o núcleo carrega para os outros
frontends não atrapalha o Qt. Dois itens do plano original deixaram de valer por isso:
`no_construtor`/`comandos` no `settings.rs` não custam nada ao Qt, que só não os chama; e a lista
canônica de teclas já é o `egui::Key`, conferida pelo teste da ponte.

Feito:

- ~~`glow` como dependência direta~~ — veio com a 0.3.0.
- ~~`Viewport` próprio no `Pintor`~~. `ui::gpu::Viewport`, com `From<ViewportInPixels>` para quem
  tem egui na tela. O headless montava um `ViewportInPixels` à mão sem ter egui nenhum; agora só
  converte na chamada, e a lógica dele (e os testes) ficaram como estavam.
- ~~A tradução de teclas no núcleo~~. `input::avks_ativos` e `input::transicoes` saíram
  do `App`, que tinha também uma cópia idêntica do `input::avk_de`. A
  interface Qt passou a entregar `EVT_KEY` com elas, como a do egui.

- ~~O laço do jogo fora do `App`~~. **`ui::partida::Partida`**, no núcleo e sem toolkit: a sessão,
  as teclas entregues, o controle do quadro anterior, o relógio da fatia, a pausa e o "aberta pela
  Z-Wheel". `teclado_mudou` recebe cada evento de teclado — um toque que começa e termina entre dois
  quadros precisa chegar como aperto e soltura —, `avanca` entrega a entrada e faz o passo, e
  `pedido_de_lancamento`/`saida` dizem o que vem depois: segue, lança um ClassID, reabre a Z-Wheel
  ou fecha. `Partida::abre` monta a sessão a partir do `Settings` — resolução interna, proporção,
  melhorias, neblina, som, os instalados para a Z-Wheel, a tela herdada. A regra de onde abrir a
  Z-Wheel virou `library::z_wheel_de`.

  As duas janelas usam a mesma partida. A do egui ficou só com ler o teclado do `egui::Context`,
  achar o caminho do ClassID na biblioteca e desenhar; a Qt passou a atender o lançamento pela
  Z-Wheel e a volta para ela, e fecha a janela quando o jogo sai sem ter para onde voltar.

  **O que não mudou de propósito.** Uma abertura que falha com outro jogo rodando zerava a entrada
  e a pausa do jogo que fica: continua zerando (`Partida::esquece_entrada`), por mais que seja
  discutível — o objetivo do corte é não mudar comportamento.

Falta:

- ~~A entrada do desktop fora do `App`~~. **`ui::entrada::EntradaDoDesktop`**: os controles do
  host (`gilrs`), os Wii Remotes e os sensores de movimento, lidos pelo mapeamento de cada porta —
  o controle de cada porta, o sensor que alimenta o Boomerang dela, e a troca de eixos do Wii
  Remote e dos controles da Nintendo. O teclado é a única parte que cada janela lê: ela entrega o
  conjunto de `egui::Key` apertadas — o egui pelo `keys_down`, que é exatamente o que o `key_down`
  consulta; o Qt pela tabela `Qt::Key → egui::Key` —, e `tecla_apertada` confere o mapeamento
  contra ele, nas duas grafias. Fica no desktop porque o `gilrs` é do desktop, mas sem egui de
  janela. O `App` manteve os métodos como repasses, e a tela de controles não mudou; a janela Qt
  passou a mandar o movimento de verdade em vez do repouso fixo.
- O resto do `App` — biblioteca, configurações, saves — não se parte: cada tela é reescrita em QML
  nas fases 4 a 6, sobre as mesmas peças do núcleo (`library`, `settings`, `saves`, `acervo`).

**Medido:** os testes do núcleo e do standalone passam, com e sem `ui-qt` (a única falha,
`video::gpu::tests::save_state_recria_texturas_e_a_segunda_unidade`, falha igual no HEAD limpo e
não é desta mudança); na janela Qt, Crash Nitro Kart pela placa, Pac-Mania em software e a Z-Wheel
pela placa rodam. **Não conferido:** o Boomerang pela janela Qt — um Wii Remote ou um controle com
sensor na porta de Boomerang, no Crash Nitro Kart. **Conferido à mão**, nas duas janelas: a Z-Wheel lançando um jogo, o jogo
voltando para ela pelo menu, e o jogo aberto pela biblioteca fechando a janela ao sair.

### 2 — Estrutura Qt ao lado do egui

`zeebx qt` abre a biblioteca, e `zeebx qt <jogo>` abre o jogo direto; o `zeebx` sem argumento
continua sendo o egui. O código é do `frontends/classical-standalone/`, atrás da feature `ui-qt`.

- **`qt/nucleo.rs`**: o estado da interface — configurações, jogos, Z-Wheel, a entrada do desktop,
  a partida e o contexto de GL — num `thread_local` com `RefCell`. Não é um `Rc<RefCell<…>>`
  passado de mão em mão, como o plano dizia: o cxx-qt cria cada `QObject` a partir do QML, sem
  argumentos, e não há por onde entregá-lo. A regra que isso impõe está no topo do arquivo:
  **nunca emitir sinal de dentro do `com`**, porque o QML pode chamar de volta e o segundo
  empréstimo derruba o programa.
- **`QObject`s finos**: a `TelaDoJogo` só guarda o quadro que está na tela e repassa teclas; a
  `Biblioteca` é um `QAbstractListModel` com os papéis `titulo` e `caminho`, e abre jogos.
- **Duas janelas do sistema**, como no egui: `Principal.qml` com a lista e o botão da Z-Wheel, e
  `Jogo.qml`, mostrada quando um jogo abre e que fecha o jogo ao ser fechada. `Esc` fecha e `P`
  pausa, como no egui.
- **O contexto de GL existe sempre**, criado na primeira abertura, e é a configuração
  (`graphics.gpu_rasterizer`) que decide se o rasterizador vai para a placa — como no egui, que
  sempre tem o contexto da janela. O `--placa` da prova de viabilidade saiu.

Fica para depois o que o plano previa aqui: a feature `ui-egui`, que tiraria o `eframe` do
standalone. O `eframe` vem pela feature `desktop` do núcleo, e separá-lo é a fase 8.

### 3 — A janela do jogo

**O quadro vai ao scene graph sem voltar à CPU.** A `TelaDoJogo` tem como base o `ItemDoQuadro`
(`src/qt/cpp/quadro.h`), um `QQuickItem` em C++ com `updatePaintNode` — o cxx-qt não tem binding de
`QSGNode`. Com o rasterizador na placa, a textura do `quadro_na_placa()` entra embrulhada
(`QSGOpenGLTexture::fromNative`), na resolução interna e com as colunas do 16:9; no resto — 2D,
software, ou a placa com 2D por cima —, a tela RGB565 sobe como imagem, só quando mudou. Medido
no Crash Nitro Kart com resolução interna 3×: praticamente todos os quadros do menu vão pela
textura, e a imagem sai na orientação certa.

- **O embrulho declara a textura como 1×1.** O Qt usa o tamanho só para converter o recorte em
  coordenadas de 0 a 1, e o rasterizador já entrega o recorte assim; o GL 3.3 do núcleo não
  pergunta o tamanho real ao driver.
- **Um `glFinish` depois de cada passo**, no contexto do rasterizador: a especificação só garante
  a outro contexto o que o primeiro terminou. O custo não foi medido — o `eglSwapBuffers` do jogo
  já lê o quadro de volta, o que espera a placa do mesmo jeito.
- **O enquadramento é um só.** `ui::partida::enquadra` saiu do `App`, com os quatro testes; o
  egui e o Qt calculam o mesmo retângulo, e a proporção "da janela" acompanha o tamanho dela nos
  dois.

O que mais a janela do egui fazia e a do Qt passou a fazer:

- **A faixa de parada**: o motivo traduzido (`play.failed`) e o que fazer (`play.stop.hint`).
- **O painel de depuração**, como faixa que encolhe o jogo. Os textos saíram para
  `depuracao::Textos`, sem egui, que o `painel` do núcleo também passou a usar; a linha do tempo
  é um `Canvas` com a mesma escala.
- **O aviso de calibração do Boomerang.** A lógica — quando abre, se o controle está parado,
  quando conclui e some, o giro do volante — saiu do `App` para `ui::calibracao`, com três testes;
  a descrição do sensor, para `SensorDaPorta::descreve`. A imagem entra nos recursos do QML pelo
  `qt-build-utils`, que dá apelido a um arquivo de `assets/`.
- **O título da biblioteca**, sem a impressão digital da pasta; **o modo da janela** configurado
  (`graphics.janela_do_jogo`, e o `graphics.janela` para a principal); **F11 e Alt+Enter**.
- **"Pausado" traduzido**, com a chave `play.paused` nos quatro idiomas — o egui não mostrava nada.

Falta o nome oficial da Z-Wheel no título, que depende do acervo (fase 4). **Não conferido:** o
aviso de calibração com um Boomerang de verdade.

**O alfa do quadro, achado depois das fases.** O Double Dragon e os outros da Data East mostravam
"bugs em branco" no Qt e não no egui. O jogo limpa o fundo com alfa zero; o quadro vai ao scene
graph como textura opaca, desenhado sem mistura, e esse alfa ia parar na janela. Na NVIDIA, no
Wayland, a janela tem canal alfa — 8 bits, mesmo com o `prepara_gl` pedindo zero, medido pelo
`QSG_INFO` —, e o compositor mostrava por esses pixels o que estava atrás dela. Conferido
capturando a janela: o fundo da tela de título saía `(0,0,0,0)`, e o mesmo quadro pela
`zeebx sessao` saía preto. Agora a textura é lida com alfa 1 (`GL_TEXTURE_SWIZZLE_A`, em
`quadro.cpp`), o que só muda a leitura, e não o rasterizador que desenha nela; e o `prepara_gl`
pede a janela sem alfa, que é o que o Qt no Wayland consulta para marcá-la como opaca. Depois da
correção, a área do jogo na captura sai toda opaca. O egui não tinha isto porque a janela do eframe
é opaca.

### 4 — Biblioteca

**As regras são uma só; o desenho é de cada janela.** Saíram do `vitrine.rs` do egui para o núcleo:
a busca (`library::casa_com_a_busca`, com os testes), a navegação pelo controle
(`ui::navegacao`, com a repetição ao segurar e três testes), o nome oficial da Z-Wheel
(`acervo::titulo_de`) e a escolha da imagem de cada jogo (`acervo::imagem_do_jogo` — capa ao lado,
capa da Z-Wheel, ícone do `.mif`, reserva — e `amplia_sem_interpolar`). O egui passou a usá-las sem
mudar de comportamento.

- **O modelo.** A `Biblioteca` tem os papéis `titulo`, `descricao`, `classe`, `capa`,
  `capaNitida`, `logo` e `classificacao`, e a lista vem do núcleo da interface já ordenada, sem a
  Z-Wheel e filtrada pela busca. O `item(linha)` serve o slider, que mostra os jogos em volta da
  escolha dando a volta na lista, e não uma linha por delegado.
- **As imagens chegam por `image://zeebx/…`**, de um `QQuickImageProvider` em C++
  (`cpp/imagens.h`) que chama o Rust por um `rust::Fn`. O endereço leva um número de geração: o Qt
  guarda as imagens pelo endereço, e sem ele o "procurar de novo" mostraria as capas antigas. O
  provedor é síncrono de propósito — o estado mora num `thread_local` da thread da interface.
- **A capa ao lado do jogo é perguntada ao disco uma vez por varredura.** O egui só não sentia o
  custo porque guardava a textura de cada jogo; o `data()` de um modelo Qt é chamado a cada linha
  que aparece. A regra passou a receber a resposta de quem chama (`acervo::capa_ao_lado`).
- **O teclado da biblioteca é do QML**, que já anda pela grade com as setas; o controle chega pela
  `Biblioteca.comandos`, lido a cada 33 ms com a janela principal em foco e sem jogo aberto — como
  no egui, em que o controle não gera evento. Passar o teclado também pelo Rust faria uma seta
  mapeada no controle andar dois passos, o que o egui evita juntando os dois antes.
- **A grade** (`GradeDaBiblioteca.qml`) e **o slider** (`SliderDaBiblioteca.qml`) seguem as medidas
  e as curvas do egui: o cartão de 136 com o quadro de 112×145, o rolo em cilindro com os painéis a
  0,42 radiano, as caixas encolhendo com a distância, a mola de `1 − e^(−12·dt)`. Os nomes não são
  `Grade`/`Slider` porque `Slider` já é um tipo do Qt Quick Controls.
- **O estilo é o Fusion.** O Basic, padrão do Qt Quick Controls, pinta os campos e o texto com a
  paleta dele, e o fundo da janela seguia a do sistema: num tema escuro, texto escuro sobre fundo
  escuro. O Fusion segue a paleta do sistema em tudo. `QT_QUICK_CONTROLS_STYLE` continua mandando.
- **A roda anda o slider por uma `MouseArea`, e não por um `WheelHandler`.** O `WheelHandler` só
  aceita o mouse por padrão — `acceptedDevices` 1, conferido no Qt 6.11 — e só um eixo: num
  notebook, a rolagem de dois dedos do touchpad não andava o slider. Reproduzido com o
  `qmltestrunner`, com o slider de verdade e uma biblioteca falsa; o evento sintético é sempre de
  mouse, então o touchpad em si foi conferido à mão.
- A barra de cima tem a Z-Wheel, a busca (Ctrl+F, Esc limpa, Enter joga o escolhido) e o "liberar
  sincronização" do Zeeboids. As configurações e os saves entram com as janelas deles.

Conferido na tela, pelo `grabToImage` de uma cópia da configuração: a grade e o slider, com as capas
e os logos da Z-Wheel, os nomes oficiais e a classificação. **Não conferido:** a navegação por um
controle de verdade.

**As setas, achadas depois das fases.** Dois defeitos deixavam as setas sem mexer na lista, e o
teclado simulado numa tela virtual mediu os dois, pelo `currentIndex` da grade e pelo `cursor` do
slider:

- **No modo slider, as setas andavam a grade escondida.** A grade e o slider declaravam
  `focus: true`, e o Qt dá o foco a um só, o primeiro. Agora o foco é de quem está à vista, e trocar
  o modo nas configurações o passa à vista nova.
- **Um botão clicado ficava com o teclado.** Depois de "Procurar de novo" ou das configurações, as
  setas iam para o botão. Os botões da barra não pegam mais o foco, e o ✕ da busca e o clique numa
  capa devolvem as setas à lista, como o Esc da busca já fazia.


### 5 — Configurações

Uma janela do sistema (`JanelaDeConfiguracoes.qml`), com as seis abas do egui. O nome não é
`Configuracoes.qml` porque `Configuracoes` já é o `QObject` Rust registrado no mesmo módulo.

- **Uma chave, e não uma propriedade por opção.** São dezenas de opções; o `Configuracoes`
  (`src/qt/configuracoes.rs`) lê e grava pela chave do `settings.json` (`graphics.smooth`,
  `audio.volume`), dá as escolhas das listas já traduzidas, e a cada gravação salva e aplica o
  efeito na hora, como o egui: o volume e a névoa no jogo aberto, e a resolução interna, a proporção
  e as melhorias refazendo o destino na placa — com o contexto de GL corrente, porque é GL.
- **O idioma troca na hora.** O `tr()` do QML é uma chamada, e uma ligação só se refaz quando lê
  uma propriedade que mudou. O singleton `Idioma` tem uma `versao` que o `tr()` de cada arquivo lê;
  trocar o idioma a muda, e todo texto se refaz. A lista da biblioteca é refeita junto, porque os
  nomes oficiais da Z-Wheel são por idioma.
- **O que era do `App` e as duas janelas precisam** foi para o núcleo: a presença do Discord
  (`discord::Acompanha`), os rótulos da resolução e dos níveis (`settings::rotulo_*`) e a troca de
  controle de uma porta (`Player::troca_controle`, com teste). A janela Qt passou a ter a presença no
  Discord e a procura por versão nova.
- **A aba de controles** tem o seletor de portas e de aparelho, o controle do host — com o
  configurado na lista mesmo desligado, como o egui mostra —, o mapeamento com captura por tecla ou
  por botão, os eixos com o valor ao vivo, e o desenho. O desenho vem pelo provedor de imagens: a
  arte de base e cada silhueta já tingida na cor da vez; o clique é testado pela silhueta no
  `PadArt::hit`. Com uma porta de Boomerang, a prévia gira com o sensor, com a calibração e a
  liberação dos sensores pelo `pkexec`. O estado ao vivo é relido a cada 33 ms com a aba à vista, e
  a `versao` só muda quando a configuração muda — a janela não se refaz a cada leitura do controle.
- **Uma ligação depende da versão por argumento, e não por uma expressão solta.** O QML é
  compilado antecipadamente (`qmlcachegen`), e o compilador descarta uma leitura cujo valor não é
  usado: `(cfg.versao, cfg.aparelho())` perdia a leitura da versão, e a ligação deixava de depender
  dela. O sintoma foi trocar o aparelho de Boomerang para Z-Pad e a aba continuar na prévia do
  Boomerang — e o mesmo defeito deixava o idioma sem trocar na hora. Medido pela ligação, no binário:
  depois da troca, o Rust respondia Z-Pad e a ligação continuava em Boomerang; com o QML
  interpretado (`QML_DISABLE_DISK_CACHE=1 QV4_FORCE_INTERPRETER=1`), acompanhava. O `qmltestrunner`
  interpreta, e por isso o teste da roda não pegaria nada disto. A forma que ficou é
  `depende([versoes], valor)`: o argumento é avaliado na chamada, e a leitura acontece.
- **Com as configurações abertas, a biblioteca não escuta o controle nem o teclado** — nem se a
  principal voltar a ter o foco. Mapeando o controle, o botão apertado abria um jogo sem querer. A
  regra é uma só, `Principal.sobreposta`, verdadeira com qualquer janela por cima (a do jogo, a de
  configurações, a de saves e os dois avisos); o mouse continua valendo, como no egui. Ao voltar
  a escutar, o que já estava apertado não conta: a navegação fica silenciada enquanto não escuta.
  Conferido no binário, abrindo as configurações e devolvendo o foco à principal.
- **Os botões da aba não pegam o foco.** Com ele, capturar o espaço clicaria o "atribuir" de novo e
  desistiria da captura.
- **O "pôr o quadro na tela pelo GL" não aparece.** A janela Qt sempre põe o quadro pelo scene
  graph, com a textura da placa quando há uma.
- Os seletores de pasta e de arquivo continuam no `rfd`, como no egui.

**Uma mudança de comportamento, de propósito:** no egui, o "restaurar" de uma porta com Wii Remote
aplicava o mapeamento de controle comum ao Wii Remote, enquanto escolhê-lo na lista aplicava o dele.
O `Player::padrao_do_controle` usa a mesma regra nos dois, e o restaurar volta ao do Wii Remote.

Conferido na tela, por captura da janela numa cópia da configuração: as seis abas, a prévia do
Boomerang lendo o sensor de um Pro Controller, e o desenho do controle com a captura pulsando e os
manches. **Não conferido:** a calibração e a liberação dos sensores de ponta a ponta.

### 6 — Janelas auxiliares

Os `QObject`s ficam em `src/qt/auxiliares.rs` — `Saves`, `Avisos` e `Log` —, e a regra de cada
um, no núcleo, para as duas interfaces usarem a mesma:

- **O que era do `App` foi para o núcleo.** `saves::todos` refaz os manifestos dos caches antigos e
  lista os saves dos jogos e os do aparelho; `partida::Relatorio` grava o relatório da execução a
  cada dois segundos, e `partida::caminho_da_serial` diz onde vai a captura da serial.
- **Saves** (`JanelaDeSaves.qml`) é uma janela do sistema, com as duas seções do egui — os jogos, e
  o aparelho com a dica de que é de todos. A lista toca o disco: é relida ao abrir e depois de cada
  exclusão, e a `versao` do `Saves` refaz a janela. A exclusão pede confirmação num diálogo modal, e
  o recado do que aconteceu fica no alto.
- **Log** (`JanelaDeLog.qml`) acompanha o jogo: aparece com ele aberto e o `debug.log` ligado, e
  ligar a opção no meio do jogo a abre. Fechar dispensa o log **desta** execução, e não a
  preferência. As linhas são relidas a cada 250 ms, só quando mudam, e a lista fica presa no fim;
  quem sobe para ler a solta, e só o usuário muda isso. A primeira leitura acontece antes de a
  janela aparecer, sem altura para ir ao fim, e por isso a ida ao fim se repete depois do layout. A
  barra tem o exportar, pelo `rfd`, e o caminho do relatório à vista.
- **Quem fica por cima de quem.** No QML, uma janela declarada dentro de outra ganha ela como
  `transientParent`, e o gerenciador de janelas mantém a filha sempre acima do pai. A do jogo era
  filha da principal, e clicar na principal durante o jogo não a trazia para a frente; agora não tem
  pai, como no egui. A do log é filha da do jogo, e fica por cima dele — **mas só se for mostrada
  depois de a janela do jogo existir no compositor.** Mostrada junto com ela, chegava ao KWin sem
  pai e ficava atrás do jogo: um script do KWin que lista a pilha a via como janela solta
  (`transient=false`). Agora ela espera o jogo ficar ativo. E ela não aceita o foco
  (`Qt.WindowDoesNotAcceptFocus`): o KWin dava o foco ao log ao aparecer, e no Wayland o foco só
  volta a outra janela em resposta a uma ação do usuário. Medido pela pilha do KWin: o log por
  cima, filho do jogo, e o jogo como janela ativa.
- **Os avisos** são diálogos modais por cima da biblioteca. O de abertura aparece enquanto a versão
  não foi dispensada; "Configurar controle" abre as configurações na aba de controles. O de versão
  nova vem depois dele, nunca junto, e "Baixar" abre a página no navegador.
- **A resposta da procura por versão nova é lida pelo relógio da biblioteca**, e não devolvida por
  `qt_thread().queue(...)`, como este plano previa. O relógio de 33 ms já existe para o controle, e
  a leitura é um `try_recv`; um sinal vindo de outra thread não economizaria nada.

Conferido na tela, por captura dos diálogos e das janelas numa cópia da configuração, com saves
falsos: as duas seções, a confirmação, a exclusão tirando o save do disco e da lista, os dois
avisos, e as configurações abrindo em Controles. O log conferido com o Alien Breaker Deluxe aberto:
as linhas, a lista presa no fim, o relatório e a serial gravados, e a janela sumindo ao fechar com
o jogo ainda aberto. **Não conferidos:** o exportar, que abre o diálogo do sistema, e o aviso de
versão nova vindo de uma procura de verdade.

### 7 — Conferência

`discord.rs`, `i18n.rs`, `saves.rs`, `acervo.rs` e `library.rs` não mudam. Os textos continuam
vindo do `Catalog` — **não** do `qsTr` —, para os JSON soltos e o teste de chaves seguirem
valendo.

- **Os módulos compartilhados.** Contra o `master`, o `i18n.rs` está intacto, e os outros quatro só
  ganharam linhas, nenhuma alterada: é o que saiu do `App` para as duas interfaces usarem — a
  presença do Discord, a lista de saves, e funções da biblioteca e do acervo. Nenhum `qsTr` no QML.
- **As duas interfaces, comparadas pelas chaves de texto.** Uma chave que o egui usa e o Qt não é
  uma função que ficou para trás. Sobraram só três: o "pôr o quadro na tela pelo GL", tirado de
  propósito na fase 5; a exportação das imagens do Discord, que no egui também está fora da tela
  (`#[allow(dead_code)]`); e o `settings.title`, que o Qt passou a usar no título das
  configurações. As chaves que nenhuma das duas usa são montadas em tempo de execução
  (`button.*`, `axis.*`) ou são do frontend Android (`play.*`).
- **Toda chave usada existe no catálogo** — `tests/chaves_da_interface.rs`, no pacote raiz: no
  do standalone, com a interface Qt ligada, o cxx-qt liga o módulo QML em todo alvo, e um teste de
  integração, sem as pontes que são do binário, não linkava. O teste de `i18n.rs` confere que
  os idiomas têm as mesmas chaves entre si; faltava o outro lado. Uma chave errada não quebra nada:
  o `Catalog` a devolve como ela mesma, e a tela mostra `saves.confirm.yes` no lugar de "Excluir".
  No QML vale todo literal com cara de chave, menos os das configurações (`chave:`, `v()`,
  `define()`, `opcoes()`), que também têm pontos; no Rust, o primeiro argumento das chamadas ao
  catálogo em `src/qt/`, `src/ui/` e no frontend Android. Conferido trocando uma chave por uma que
  não existe: o teste aponta o arquivo e a linha.
- **O que não tem texto**, comparado pelo que o `eframe` recebia e pelo `update` do `App`:
  - O ícone e o `app_id`. O Qt não dava nenhum dos dois. Agora a logo, reduzida a 256 pixels como
    no egui, é o ícone de todas as janelas (`aplica_icone`, em `gl_qt.cpp`), e o
    `setDesktopFileName("zeebx")` faz o papel do `app_id`: no Wayland é por ele que o compositor
    acha o `zeebx.desktop` e o ícone. Rodando de `target/`, sem o `.desktop` instalado, o portal
    avisa que não achou o aplicativo; é só o aviso.
  - Os tamanhos mínimos do egui: a biblioteca, 480×360; as configurações, 420×320; o jogo, 320×240.
    A de saves e a de log já tinham.
  - F11 e Alt+Enter também na biblioteca. No egui a tela cheia vale na janela principal e na do
    jogo; o Qt só tinha na do jogo.

### 8 — Corte do eframe

**Sai o `eframe` do standalone, não o egui do projeto** — ver a fase 1. `ui-qt` como padrão por uma
versão, depois saem o `eframe`, a `ui::App` e, se trocado pelo
`QtQuick.Dialogs`, o `rfd`. O `--window` (`minifb`) é decisão pendente do `TODO.md`,
independente desta. O [10](10-interface.md), o diagrama de camadas do
[`ARCHITECTURE.md`](../../ARCHITECTURE.md) e os comentários de `video/contexto.rs` e
`video/gpu.rs` que falam do eframe são reescritos.

**A primeira etapa, feita.** Compilado com a feature `ui-qt`, `zeebx` sem argumentos abre o Qt, e
`zeebx egui` abre a interface antiga — para quem achar uma diferença ter como comparar. O
`zeebx qt [jogo]` continua valendo. Conferido pelo log de imports do QML
(`QT_LOGGING_RULES="qt.qml.import=true"`): o `zeebx` carrega o QML, o `zeebx egui` não.

**A segunda etapa, feita: a feature `ui-qt` vem ligada por padrão.** Ela esperou duas coisas: a
fase 9, que ensinou o CI a instalar e empacotar o Qt, e a licença sair do GPL-2.0-only (ver
*Licença*), que agora é `GPL-2.0-or-later`. O `ci.yml` instala o Qt nos seis alvos, e o
`release.yml` monta os instaladores com ele. Sem o Qt, `--no-default-features` ainda compila a
interface do egui, e o `ferramentas/prepara_build.py` confere o Qt junto com o resto. A ordem que
sobra é:

1. a fase 9 — feita;
2. a licença trocada e a feature ligada por padrão — feito;
3. uma release com o Qt padrão e o `zeebx egui` ainda lá;
4. na seguinte, o corte: saem o `eframe`, o `ui::App`, o `zeebx egui` e os caminhos `cfg` da
   feature, e são reescritos o [10](10-interface.md), o `ARCHITECTURE.md` e os comentários do
   `video/`.

### 9 — Build e empacotamento

O CI instala o Qt nos três sistemas. O `cargo packager` não implanta Qt: cada sistema ganha o
passo dele (`linuxdeploy-plugin-qt`, `windeployqt`, `macdeployqt`), e o `.deb` passa a depender
de `libqt6gui6`, `libqt6qml6`, `libqt6quick6` e dos módulos QML usados.

**Primeiro no `qt.yml`, depois no `release.yml`.** Enquanto a licença prendia, os instaladores
saíam só como artefatos do `qt.yml`, com uma configuração à parte, passada por `--config`. Com a
feature ligada por padrão, a configuração passou ao `[package.metadata.packager]` do `Cargo.toml`
do standalone, e os mesmos passos estão no `release.yml`: o AppImage no Ubuntu 22.04, o `.deb` num
job à parte no 24.04, o NSIS e os dois `.dmg`. O `qt.yml` faz o mesmo e, além disso, abre cada
instalador numa tela virtual antes de deixá-lo como artefato — é onde testar antes de uma tag. O
`--config` trazia dois tropeços, que sumiram com ele: sem ler o `Cargo.toml`, o `cargo packager`
não sabia a versão, e o 0.11.8 entrava no caminho do *arquivo* como se fosse uma pasta, parando
com "Not a directory".

- **A chave do cache leva o sistema.** O job do AppImage passou do Ubuntu 24.04 para o 22.04 com a
  mesma chave, e o "Compila" saiu com 101 — enquanto o mesmo build passava num contêiner 22.04
  limpo. A causa provável, não confirmada sem o log: o cache restaurou os scripts de build
  compilados no 24.04, ligados a uma glibc mais nova. A chave agora é `nome-sistema`, no `qt.yml`
  e no `release.yml`, o que vale de qualquer forma.
- **O AppImage**, montado no Ubuntu 22.04 como o da release, leva o Qt 6.11.2 pelo
  `linuxdeploy-plugin-qt`. O Wayland não vem sozinho: é preciso pedir o plugin de plataforma
  (`libqwayland.so`, um só a partir do Qt 6.10) e o módulo `waylandcompositor`, sem o qual a
  integração gráfica (`wayland-graphics-integration-client`) fica de fora e a janela não tem GL no
  Wayland — conferido desmontando o AppImage. O ALSA e o `libgpg-error` ficam de fora de
  propósito, pela lista de exclusão do AppImage: todo desktop os tem. Conferido num Ubuntu 22.04,
  num 24.04 e num Debian 13 limpos, numa tela virtual, com um jogo aberto. Montado no Arch, o
  `linuxdeploy` não serve de prova: o `strip` embutido não conhece as bibliotecas de lá, e o Qt de
  lá traz plugins de imagem do KDE com dependências que o do `aqtinstall` não tem.
- **O `.deb` usa o Qt do sistema**, como o plano previa, e por isso é compilado contra ele, num job
  à parte no Ubuntu 24.04: um binário do Qt 6.11 não roda num Qt mais velho, e o 24.04 traz o 6.4.
  Isso deixa o Ubuntu 22.04 de fora do `.deb` com Qt (o Qt dele é o 6.2), mas não do AppImage. As
  dependências são as bibliotecas que o binário linka, conferidas com `ldd` e `dpkg -S`, o plugin
  de plataforma, o `qt6-wayland` e os módulos QML que os `import` pedem. O `gui`, o `network` e o
  `opengl` vão com os dois nomes: o Ubuntu 24.04 os chama com `t64`, e o Debian 13 sem. O job
  instala o `.deb` num Ubuntu 24.04 e num Debian 13 limpos, com o `apt` resolvendo tudo, e o abre
  numa tela virtual.
- **O Windows**: o `windeployqt` junta o Qt numa pasta, e a configuração manda o NSIS instalá-la ao
  lado do `zeebx.exe`, com as subpastas.
- **O macOS**: o `cargo packager` monta o `.app`, o `macdeployqt` põe o Qt dentro, a assinatura
  ad-hoc é refeita — no ARM, um binário alterado sem assinatura válida é morto ao abrir — e o
  `hdiutil` faz o `.dmg`.
- **O que o Windows e o macOS ainda não tiveram:** uma execução. Os passos não rodam fora dos
  runners deles, e o primeiro `qt.yml` com eles é a prova.

**Três coisas que o empacotamento achou fora dele:**

- **O ALSA do `.deb` do egui, que já sai na release**, instalava uma imitação num sistema mínimo.
  A dependência era `libasound2 | libasound2t64`, e no Ubuntu 24.04 o `libasound2` é cumprido pelo
  `liboss4-salsa-asound2`, um ALSA de mentira sobre o OSS4: o `apt` ficava com ele, e o binário
  parava ao abrir com "undefined symbol: snd_pcm_status_get_trigger_htstamp". Com o
  `libasound2t64` primeiro, o `apt` escolhe o de verdade. Corrigido nos dois `.deb`.
- **O `zeebx qt <jogo>` no Qt 6.4 não mostrava a janela do jogo.** Ela era aberta de dentro do
  `onCompleted` da principal, e o 6.4 a deixava sem mapear; o 6.11 mostrava. Agora é aberta
  depois da montagem, com `Qt.callLater`. Aberto pela biblioteca, os dois já funcionavam.
- **O atalho de busca** usava `sequence: StandardKey.Find`, e onde o sistema dá mais de uma tecla
  ao "procurar" o Qt 6.11 liga só a primeira e avisa. Passou a `sequences`.

O Qt do Ubuntu foi compilado com o linker `gold`, e o cxx-qt repassa o `-fuse-ld=gold` ao link: o
`rustc` avisa que o `gold` está obsoleto. É só o aviso — o binário liga e roda.

## Riscos, do maior para o menor

1. O contexto compartilhado entre o `GpuState` e o scene graph, sobretudo no macOS, onde o OpenGL
   está obsoleto mas ainda atrás do RHI. É a razão de a fase 0 existir.
2. O Qt no CI e nos pacotes dos três sistemas.
3. A fidelidade do slider cilíndrico.
4. O `settings.json` antigo: os nomes de tecla do egui precisam continuar valendo.
