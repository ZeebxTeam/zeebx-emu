# Zeebx — andamento

Emulador HLE de Zeebo / Qualcomm BREW 4.0.2, em Rust, para Linux, Windows e macOS.
Contexto técnico em [docs/](docs/README.md).

## Pronto

- Levantamento e espelhamento da documentação (ver [docs/07-inventario-vendor.md](docs/07-inventario-vendor.md))
- **BREW SDK 4.0.2 SP19 completo** — headers, referência de API em HTML, `elf2mod` e o linker script
- Esqueleto do projeto em Rust: parser de `.mod`, memória do guest, trait `CpuBackend`
- `zeebx info <arquivo.mod>` lê as duas variantes de `.mod` que encontramos
- Núcleo ARM1176 sobre `unicorn-engine`, atrás do trait
- Trampolim de APIs: vtables em faixa não mapeada, com decodificação de interface e slot
- `zeebx run <arquivo.mod>` executa `AEEMod_Load` de módulos reais (`helloworld.mod`,
  `tectoy.mod`, `bjt.mod`) — os três **retornam SUCCESS** e entregam um `IModule*` válido
- Laço de despacho: atende a chamada, escreve o retorno em `r0` e retoma em `lr`
- Heap do guest com `MALLOC`/`FREE`, e `IShell::AddRef`/`Release`
- Log de chamadas por método — o backlog de APIs, medido em vez de suposto
- Parser de `.mif` com extração do ClassID do applet
- `Machine::call_guest` — chamadas no sentido inverso, do emulador para o guest
- `IModule::CreateInstance` é chamado com o ClassID do `.mif`; os três módulos entram no código
  de criação do applet
- `ISHELL_CreateInstance` com fábrica de objetos por ClassID, `AddRef`/`Release` por objeto
- **`helloworld.mod` cria o applet com sucesso** (`IApplet*` válido)
- `DBGPRINTF` implementado — o log do próprio jogo aparece no terminal
- Nomes de método nas vtables, gerados dos headers do SDK: o log diz `IShell::CreateInstance`
  em vez de `slot[2]`
- Teto global de chamadas de API, para laço de repetição não travar o emulador
- `ISHELL_GetDeviceInfo` com a tela 640×480 do Zeebo
- **`EVT_APP_START` entregue ao applet** — o ciclo de vida completo do BREW: carga, criação do
  applet, evento de início e desenho
- Framebuffer 640×480 RGB565 com `SetColor`, `DrawRect`, `Update` e exportação para BMP
- **`helloworld.mod` roda até o fim e pede para desenhar `"Hello World"`**
- `IBitmap` completo: superfícies, `RGBToNative`/`NativeToRGB`, `DrawPixel`, `GetPixel`,
  `DrawHScanline`, `FillRect`, `BltIn`/`BltOut` com cor transparente, `GetInfo`,
  `CreateCompatibleBitmap`, transparência
- `IDisplay::GetDeviceBitmap`, `BitBlt`, `SetDestination` — o desenho vai para a superfície
  corrente, não mais direto na tela
- `ISignal`, `ISignalCtl` e `ISignalCBFactory` — o jogo já registra os callbacks de botão
- `AEECLSID_SignalCBFactory` = `0x01041207`, confirmado no `AEESignalCBFactory.bid` do SDK
- Helpers de string: `strtowstr`, `wstrtostr`, `wstrcpy`, `wstrcat`, `wstrcmp`, `strncpy`,
  `strchr`, `strstr`, `stricmp`, `memchr`, `atoi`, `snprintf` e companhia
- **O Bejeweled Twist passa da inicialização** e chega a abrir arquivos — 1,5 MB de heap
- `IGraphics` completo: cores, modo de preenchimento, translação, ponto, linha, retângulo,
  círculo, triângulo, polígono e polilinha, com Bresenham e ponto médio no framebuffer
- Rastreamento passa a registrar o valor de retorno de cada chamada
- `IFileMgr`/`IFile` sobre um VFS ancorado no diretório do módulo, com recusa de qualquer
  caminho que tente sair dele
- **O Bejeweled Twist abre e lê os arquivos dele** e chega a pedir `IGraphics`
- `IHID`/`IHIDDevice` com um gamepad sempre conectado, usando o VID/PID reais do controle do
  console e os UIDs de botão do `hid_devices.cfg`
- Ponteiro inválido vindo do guest vira `EBADPARM` e entra no relatório, em vez de derrubar o
  emulador
- Aviso de "hipóteses em uso" no relatório, para não confundir palpite com comportamento
  conhecido
- **A tabela de helpers inteira, com nomes** — 117 slots, de `struct AEEHelperFuncs`
- Helpers implementados: `malloc`/`free`/`realloc` (com `ALLOC_NO_ZMEM`), `memmove`, `memset`,
  `memcmp`, `strlen`, `strcpy`, `strcat`, `strcmp`, `strncmp`, `wstrlen`, `sprintf`,
  `dbgprintf`, `GetAppInstance`, os helpers de tempo, `aee_GetRand`, `GetRAMFree`
- **A duração de um MP3 sem decodificá-lo** (`mp3.rs`) — a música do Tekken 2 é MP3, o `Play`
  respondia "esse som já acabou" e o jogo mandava tocar de novo, 766 mil vezes em quatro segundos
  virtuais. Com a duração vinda do cabeçalho e da etiqueta `Xing`/`Info`, o som toca em silêncio
  pelo tempo certo do relógio virtual: seis segundos virtuais saíram de mais de cinco minutos
  para 9,7 segundos, e ele desenha a tela de título
- **Os campos do `IDIB` em todo bitmap que sai do decodificador** — um `IBitmap` de software do
  BREW *é* um `IDIB`, e o jogo lê o tamanho direto dos campos públicos, sem `QueryInterface`.
  Enquanto saíam zerados, o Peggle montava cada sprite como um quadrado de lado zero (76.618 dos
  77.208 triângulos de um quadro descartados por área nula, tela preta) e o Heavy Weapon repetia
  o quadro contra bitmaps 0×0 — seis segundos virtuais dele caíram de mais de cinco minutos para
  2,1 segundos. **49 dos 62 rodando**
- **A tabela de UIDs dos eixos, conferida nos binários dos jogos** — o `X` trazia o UID do
  `Button_3`, que não aparece em jogo nenhum: quem procurava o eixo horizontal não achava eixo, e
  quem procurava o `Y` caía no `RZ`. Com os quatro UIDs certos o menu do Zeebo Sports Tênis anda
  um item por toque e fica, em vez de voltar na soltura
- `zeebx run <mod> --trace` — registro de cada chamada na ordem, com argumentos
- **O Bejeweled Twist inicializa**: cria o applet, cria a classe do jogo e entra na
  inicialização, com 6993 alocações e 640 KB de heap em uso
- `IDIB` (`AEECLSID_DIB` = `0x01001045`): a superfície ganha um buffer de pixels na memória do
  guest (região `surfaces`, 8 MB em `0x40000000`), os 36 bytes de campos públicos são
  preenchidos e o `IBITMAP_QueryInterface` devolve o próprio objeto. Host e guest são
  sincronizados nas interfaces que mexem em pixels
- `IFILEMGR_OpenFile` com `_OFM_CREATE` cria os diretórios que faltam — vários jogos escrevem
  em `udata\...` sem chamar `MkDir` antes
- `IFILE_Seek` devolve `SUCCESS`, e não o deslocamento (conferido em `IFILE_Seek.htm`)
- **O Bejeweled Twist carrega os recursos**: 840 leituras no `resources.dat`, 1,6 MB de heap
- `ISound` (`AEECLSID_SOUND` = `0x01001056`) mudo: guarda o `AEESoundInfo` e o volume, aceita
  tons e vibração, e enfileira o `PFNSOUNDSTATUS` com `AEE_SOUND_PLAY_DONE`
- `ILicense` (`AEECLSID_LICENSE` = `0x0100100f`): módulo sem expiração (`LT_NONE`) e comprado
  (`PT_PURCHASE`), que é o que o console responderia para um jogo que o dono comprou
- `ISHELL_GetClassItemID` devolve o número do item da loja do BREW, lido do diretório em que o
  `.mod` mora
- **O Bejeweled Twist termina a inicialização** (`INITIALIZATION DONE!`) e recebe `EVT_APP_START`
- **Laço de quadros**: relógio virtual, `ISHELL_SetTimer`/`CancelTimer`/`GetTimerExpiration` e
  disparo dos timers vencidos a cada quadro, junto com os sinais e os callbacks de som
- `zeebx run <mod> [--seconds=N] [--frames=N] [--watch=0xADDR] [--dump-heap]`
- **Watchpoint de escrita e de leitura** (`--watch`): registra todo acesso numa faixa, com o PC
  de origem — inclusive os que o próprio emulador faz, que não passam pelos hooks do unicorn.
  Foi o que permitiu ver o jogo lendo o cabeçalho do pacote: a leitura acontecia dentro do
  nosso `memmove`, invisível para o hook
- **Rastreio de código** (`--code=0xINI:0xFIM`): cada instrução executada dentro da faixa, com o
  `r0` de cada uma. A desmontagem mostra os caminhos possíveis; isto mostra o percorrido
- O watchpoint registra também o `lr` de cada escrita, e o rastreio de código registra o `lr`
  de cada instrução — na entrada de uma função, é o endereço de retorno do chamador
- **Varredura de pilha na falha**: os endereços de retorno que sobraram na pilha, filtrados por
  "tem um `bl` logo antes", reconstroem a cadeia de chamadas sem tabela de desenrolamento
- Falha de memória reporta `r0..r11`, e `--dump-heap` salva a heap e a imagem do módulo
- `IDISPLAY_GetDeviceBitmap` passa a devolver uma referência (`AddRef`) — o jogo dá `Release`
  uma vez por quadro, e a contagem zerava
- **Correção do `strlen`** (e de toda a família de funções de string): elas contam *bytes*, não
  caracteres. A versão antiga passava pela conversão para `String`, e cada byte inválido em
  UTF-8 virava `U+FFFD`, de três bytes — `strlen("\x89PNG")` devolvia 6 em vez de 4
- `IMemAStream` (`AEECLSID_MEMASTREAM` = `0x0100100c`) — um bloco de memória servido como stream
- `IImage` sobre `AEECLSID_PNG` (`0x01004004`), com decodificação de PNG de verdade (crate
  `png`), expansão de paleta e `tRNS`, e desenho respeitando o alfa
- **`IIMAGE_Notify` disparado na fronteira entre duas chamadas de API** — o único ponto em que
  dá para entrar no guest sem interromper nada pela metade. Com ele o jogo passa a chamar
  `IIMAGE_Draw`, o que não acontecia antes
- **Chamada ao guest com argumentos na pilha** (`call_guest_with_stack`), seguindo a AAPCS
- **O Quake desenha**: a tela de abertura da TecToy aparece com a barra de progresso do
  carregamento — a primeira imagem de verdade que o emulador produz
- `ISHELL_LoadResObject` carrega um arquivo inteiro como recurso e devolve um `IImage`
- `IIMAGE_Notify` numa imagem já decodificada notifica na hora, sem esperar stream
- **`ISHELL_SetTimerEx`**: a macro do SDK passa o mesmo ponteiro como `pfn` e `pUser`, e o que
  chega é um `AEECallback *` — quem chamar está em `pfnNotify`/`pNotifyData`, nos offsets 16 e
  20. Enquanto tratávamos o ponteiro como função, o Quake saltava para a heap
- O VFS sobe um nível com `..`, até a raiz de módulos e nem um passo além: o Quake guarda os
  dados dele em `mod/id1/`, ao lado do diretório do próprio módulo
- Instrução inválida vira desfecho relatado, em vez de derrubar o emulador
- `ISHELL_GetDeviceInfo` preenche os campos estendidos quando o chamador pede: o Bejeweled
  Twist manda `wStructSize = 64` nas duas chamadas, e recebia vinte bytes de zeros — inclusive
  `wMaxPath = 0`, que diz que nenhum caminho de arquivo cabe
- `IBITMAP_FillRect` respeita o `AEERasterOp`: preencher com a própria cor transparente sob
  `AEE_RO_TRANSPARENT` não escreve nada, e `AEE_RO_XOR` inverte. Ignorar o `rop` custava caro —
  o Bejeweled Twist chama exatamente assim, com cor zero, e a tela inteira era apagada uma vez
  por quadro
- **`IIMAGE_Draw` numa superfície do próprio jogo, pelo `BltIn` dela** — montamos um `IBitmap`
  nosso com a imagem, o `BltIn` do jogo pede `QueryInterface(AEECLSID_DIB)` nele e lê os pixels
  pelos campos públicos. O ciclo fecha: são 18 blits por execução, todos aceitos

- **Ponto flutuante da stdlib** (`src/fmath.rs`): `f_op`, `f_cmp`, `f_calc`, `f_get`,
  `f_toint`, `f_assignint`/`f_assignstr`, `strtod`, `trunc`/`utrunc`. Os códigos de operação
  saem de `AEEStdLib.h`, onde `FO_*`, `FCALC_*` e `FGET_*` dividem a mesma numeração; o Quake
  monta as tabelas de seno e tangente com 182 chamadas de `f_calc` logo na inicialização
- **Mais 60 helpers da stdlib**: a família larga (`wstrchr`, `wstrrchr`, `wstrdup`,
  `wstrlower`/`wstrupper`, `wstricmp`/`wstrnicmp`, `wstrlcpy`/`wstrlcat`, `wwritelong`,
  `wstrncopyn`, `wsprintf`, `strexpand`, `utf8towstr`/`wstrtoutf8`, `wstrtofloat`,
  `floattowstr`), a de bytes (`strdup`, `strtoul`, `strnicmp`, `stristr`, `memstr`,
  `strbegins`/`strends`, `strchrend`/`strchrsend`, `memrchr`/`memchrend`/`memrchrbegin`,
  `strlower`/`strupper`, `strlcpy`/`strlcat`, `swapl`/`swaps`) e o resto (`GetAEEVersion`,
  `GetFSFree`, `sysfree`, `err_strdup`/`err_realloc`, `lockmem`/`unlockmem`, `aee_basename`)
- **Correção do `vsprintf`/`vsnprintf`**: `AEEOldVaList` é `int **` no ARM
  (`inc/AEEOldVaList.h`) — o que chega é o endereço da variável `va_list`, não a área de
  argumentos. Sem a indireção o primeiro `%d` imprimia o próprio ponteiro da pilha, e o Quake
  procurava o recurso `inva537919380_shotgun` em vez de `inva1_shotgun`
- Heap de 64 MB: o Quake mede a memória livre antes de carregar os `.pak` e desistia com
  "Not enough free memory"
- **`IThread` (`AEECLSID_THREAD` = `0x01001017`) — threads cooperativas de verdade.** O laço
  principal do Quake roda dentro de uma. Como não há preempção, o guest só perde o controle no
  `Suspend`, e é exatamente aí que salvamos os registradores; retomar é restaurá-los e
  continuar do endereço de retorno guardado. Cada thread tem a própria pilha na heap do guest
- `ISHELL_Resume` (slot 36) enfileira um `AEECallback` para a volta seguinte do laço de
  eventos — é por ele que a thread pede a própria retomada, via `GetResumeCBK`
- **O relógio do guest anda durante a execução**, e não só entre quadros: soma o tempo que o
  laço de quadros adianta com o tempo real gasto executando. Sem isso um laço de espera do
  jogo ("fique aqui até passarem 1,5 s") nunca terminava — ele lia o relógio, nada avançava e
  lia de novo
- `--trace=trecho` filtra o rastreamento por nome de método: num jogo que faz milhares de
  `strlen` por quadro, o que interessa era empurrado para fora da janela
- **O Quake roda a engine**: carrega `pak0.pak` e `pak1.pak`, resolve os lumps do WAD,
  inicializa o cliente (`CL_Disconnect`) e chega à inicialização do vídeo

- **`IEGL11` e `IGLES11`** (`AEECLSID_QEGL` = `0x0103d8ec`): um objeto só que responde por EGL
  e por OpenGL ES pelas interfaces novas do BREW — `this` no primeiro argumento, código de
  erro no retorno e o resultado por ponteiro de saída. A `IEGL`/`IGL` antigas de `AEEGL.h`
  têm outra convenção e não são estas
- **Rasterizador de software** (`src/rasterizer.rs`): pipeline fixo do OpenGL ES 1.1 — pilhas
  de matriz, recorte contra o plano próximo, divisão pela perspectiva, preenchimento de
  triângulos com interpolação corrigida pela perspectiva, textura, teste de profundidade,
  mistura, teste de alfa e face traseira
- `TexImage2D` com os formatos compactos do OpenGL ES (5_6_5, 4_4_4_4, 5_5_5_1) e os de byte
- O Quake do Zeebo desenha em 320×400 e conta com a extensão de escala da Qualcomm para chegar
  aos 640×480 da tela; como ele nunca diz o tamanho da superfície, ele é deduzido da maior
  viewport usada
- `IMediaUtil`/`IMedia` mudos: o Quake cria o tocador da trilha na inicialização do áudio e,
  se ela falha, segue em frente e depois chama `Play` num ponteiro nulo sem conferir
- **O relógio virtual vem da contagem de instruções**, medida por bloco de tradução: duas
  execuções da mesma ROM dão exatamente o mesmo resultado, o que é o que torna possível
  comparar dois quadros e saber que a diferença foi a mudança que fizemos
- `zeebx run <mod>` grava também o quadro do OpenGL (`<nome>.gl.bmp`), separado da tela: os
  dois desenhos convivem, e ver o do OpenGL sozinho é o que diz se a renderização 3D está certa
- **O Quake roda**: carrega o mapa, toca a demo e desenha o mundo em 3D, com o console, a
  barra de status e as texturas. A demo avança sozinha — o jogador leva dano e a barra de vida
  cai
- Uma thread cooperativa é retomada **uma vez por quadro**, e não a cada fronteira entre
  chamadas de API: ela cede o controle esperando a próxima passada do laço de eventos, e o
  laço de eventos deste emulador é o laço de quadros
- O laço de quadros só encerra quando não há mais nada a fazer — timer, callback ou thread.
  Olhar só para os timers encerrava a execução com o jogo no meio, porque o Quake cancela o
  timer assim que passa a viver dentro da thread

- **Janela ao vivo** (`--window`): o quadro aparece enquanto o jogo roda, em vez de só sair
  como `.bmp` no fim. `Esc` ou fechar a janela encerram. Com `--window` o padrão é rodar até
  fecharem — o `--frames` continua valendo como limite de segurança

- **Entrada do teclado** (`src/input.rs`): o controle do Zeebo com os doze botões e os quatro
  eixos, alimentado pelas teclas do host. Cada aperto e cada soltura entra numa fila que o
  `GetNextButtonEvent` esvazia, e a mudança acorda o jogo pelos `ISignal` que ele registrou —
  sem isso o jogo só veria a tecla na próxima vez que resolvesse perguntar, e alguns nunca
  perguntam
- `--keys=ms:tecla[:duração_ms],...` roteiriza a entrada, para exercitá-la sem janela
- **`GetAxesInfo` devolve os UIDs dos eixos**, não valores: é assim que o
  `AEEHIDThumbsticks.c` do SDK descobre em qual campo está cada direção, e enquanto
  respondíamos zeros ele não achava nenhuma
- **O `AEEHIDPositionInfo` tem 25 palavras, não 50.** Escrevíamos o dobro do tamanho da struct,
  passando cem bytes por cima da memória do jogo. Era o que derrubava o Zeebo Sports Peteca no
  primeiro quadro
- **O Quake abre o menu** (JOGAR / OPÇÕES / AJUDA / SAIR) e o **Crash entra numa corrida** e
  esterça

- **O direcional também é botão.** O `hid_devices.cfg` do console descreve o direcional só como
  os eixos `X` e `Y`, mas é como botão que os jogos o leem: com os quatro UIDs `DPad_*`
  presentes, o menu do Quake anda; sem eles, o cursor não sai do lugar por mais que o eixo
  mude. Um direcional digital em USB HID costuma ser reportado das duas formas, e é o que
  fazemos — apertar uma seta mexe no botão e no eixo
- `Button_1` e `Button_3` também faltavam na lista do console. A tela de ajuda do próprio Quake
  nomeia os quatro ("aperte 1 para pular", "aperte 3 para ativar mira"), então eles existem
- `GetButtonInfo` responde o estado atual do botão, e não zero: um jogo que consulta em vez de
  esperar o evento só enxerga a tecla por ali
- A janela lê também os eventos de borda do teclado: um toque que começasse e terminasse entre
  dois quadros não aparecia em `is_key_down`, e o emulador só olha o teclado uma vez por quadro
- **`glTexEnvxv` — a forma vetorial — era ignorada.** O Crash pede `GL_REPLACE` por ela, e o
  modo ficava preso no `GL_MODULATE`: cada textura saía multiplicada pela cor do vértice, o que
  apagava os ícones da tela para preto
- **Modos de repetição e filtro por textura**: `GL_CLAMP_TO_EDGE` prende na borda em vez de dar
  a volta, e `GL_LINEAR` interpola em vez de pegar o vizinho mais próximo. Os dois são por
  nome de textura e sobrevivem a uma nova imagem, como no OpenGL
- **O plano próximo é `z >= -w`, não `w > 0`.** Recortar pelo sinal de `w` deixava passar
  vértices logo atrás do plano próximo, e a divisão pela perspectiva multiplicava as
  coordenadas deles por dezenas de milhares: o triângulo virava um bloco cobrindo a tela, com
  a textura tão esticada que saía como cor chapada. Era o que sujava o cenário do Crash. O
  plano é inclusivo — a interface do Crash é desenhada em ortográfica bem em cima dele, e
  recortar com `> 0` apagava o logo e as placas do menu
- **O recorte percorria o polígono ao contrário.** Com dois vértices à frente, o que sobra é
  `a → b → eb → ea`; montávamos `a → ea → eb → b`, que é a mesma volta invertida. A orientação
  saía trocada e o descarte de faces jogava fora justamente os triângulos recortados
- `--dump-gl=DIR` grava um arquivo por quadro apresentado: comparar dois quadros vizinhos é o
  que separa um artefato do desenho de um estado preso de um quadro para o outro
- **O relógio passou a ser de microssegundos e a andar com o jogo.** O laço avançava 33 ms
  fixos por volta, independentemente de quanto o jogo tivesse executado: o Crash pedia 510 ms
  de tempo virtual por quadro desenhado, ou seja, o jogo se via rodando a 2 fps e acelerava
  tudo para compensar. Agora o relógio é `clock_us + instruções/528`, e quando não há nada
  pendente ele pula direto para o vencimento do próximo timer, em vez de queimar voltas
- **`eglSwapBuffers` espera o retraço vertical** a 60 Hz. Sem isso o Quake passou a rodar a
  418 fps — o jogo não tem limite próprio, quem limita é a tela do console
- **`GetAppInstance` virou um trecho de código ARM**, gravado numa região nova (`STUB_BASE`)
  com a tabela de helpers agora gravável. Ele respondia 2,85 milhões das 4,86 milhões de
  chamadas de API de uma execução de 20 s: cada uma custava sair do unicorn e voltar, só para
  devolver um ponteiro constante

- **`ISHELL_GetDeviceInfoEx` responde o IMEI** (item 28). O `pnSize` é de entrada e saída, e
  devolver sucesso sem escrever nele deixava o Zeebo Sports Peteca alocar com lixo. O IMEI é
  sintético — não conhecemos o de nenhum Zeebo —, mas com os 15 dígitos e o verificador de
  Luhn certos, porque quem pede um IMEI costuma conferir

- **O Zeebo Sports Peteca chega à tela de título**, com a quadra, a arquibancada, a plateia e
  o logo. Três coisas faltavam:
  - **Rede e criptografia como objetos que existem.** Ele cria `AEECLSID_WEB` (`0x01005000`),
    `AEECLSID_MD5` (`0x01001015`) e `AEECLSID_CipherFactory` (`0x0102cce1`) em sequência e
    **não confere o retorno**: a primeira recusa fazia o código pular as outras duas criações e
    usar o ponteiro que nunca foi escrito
  - **`ICipher1` com AES-128 em CBC de verdade** (`src/crypto.rs`), com os vetores do FIPS-197
    e do NIST SP 800-38A como teste
  - **Texturas ATITC** (`src/atc.rs`): ele não faz **uma única** chamada a `glTexImage2D` —
    todas as 300 e tantas texturas entram por `glCompressedTexImage2D` nos formatos
    `GL_ATC_RGB_AMD` e `GL_ATC_RGBA_EXPLICIT_ALPHA_AMD`, que são os do Adreno 130
- **Recursos `.bar` lidos** (`src/resfile.rs`), com `LoadResString`, `LoadResData`,
  `LoadResDataEx` e `FreeResData`. O `.bar` é o **mesmo contêiner do `.mif`**; o que ele tem a
  mais é um índice de entradas de 8 bytes, `(tipo, id, a, b)`, em que os ids `id..=id + a`
  moram nas seções `b..=b + a`. A regra foi confirmada nos dois arquivos reais: em ambos o
  número de recursos bate exatamente com o número de seções (143 e 308) e a maior seção usada é
  a última, então o índice cobre tudo uma vez cada
- Quando o jogo passa nome de arquivo nulo — o Peggle passa —, vale o único `.bar` ao lado do
  módulo. Não há convenção de nome que sirva: o Peggle chama o dele de `resources.bar` e o
  Pac-Mania de `pacmania.bar`
- **`IDisplay::GetClipRect` e `GetFontMetrics`**. O recorte é ignorado no desenho, então o
  retângulo corrente é a tela inteira; as métricas de fonte são números coerentes para o jogo
  medir e posicionar, já que texto ainda não é desenhado
- **O Double Dragon chega à tela de título** — a quarta ROM a rodar. Ele desenhava "Memory is
  insufficient", que é o caminho de erro genérico dele, não falta de memória: o `dwRAM` já
  reportava 64 MB e o jogo usava 18 KB. A recusa de verdade era o `AEECLSID_UNZIPSTREAM`, que
  ele usa para abrir o `data.ggz`. Três peças faltavam:
  - **`IUnzipAStream`** (`AEECLSID_UNZIPSTREAM` = `0x01001014`), com a vtable documentada em
    `sdk/inc/AEEUnzipStream.h`. A descompressão é feita de uma vez, na primeira leitura: o BREW
    descomprime conforme se lê, mas o efeito visível é o mesmo e isso dispensa manter estado de
    inflate parcial entre chamadas
  - **Ler de um `IAStream` que é um `IFile`.** O jogo entrega o arquivo aberto direto ao unzip —
    no BREW um `IFile` também é um `IAStream` —, e a origem não é o `IMemAStream` que
    esperávamos
  - **O botão HOME é o `Back` do BREW.** O controle do Zeebo tem HOME impresso na carcaça, e é
    esse botão que o `hid_devices.cfg` do console mapeia no `AEEUID_HIDJoystick_Back` — não
    existe UID de "Home" no `AEEHIDDevice_Joystick.h`. Ele responde pelos dois nomes no
    `--keys`, e a tecla `H` foi para o mapa junto do `Backspace`: o jogo pede "APERTE O BOTÃO
    HOME", e ninguém adivinharia que o botão se chama `back` aqui dentro. Com ele, o jogo passa
    da tela de título para o menu — Batalha 1 Jogador, 2 Jogadores, Ajuda, Opções, Sair
  - **Texturas paletizadas do OES** (`src/paltex.rs`), do
    `OES_compressed_paletted_texture`. As entradas de 16 bits valem na ordem de bytes do
    aparelho, little-endian: lidas ao contrário, o logo sai com serrilhado de arco-íris no lugar
    do dourado, e foi assim que a ordem se decidiu
- **`IHeap` implementado** (`AEECLSID_HEAP` = `0x01001002`), com a vtable documentada em
  `sdk/inc/AEEHeap.h`. A alocação é a mesma do `MALLOC` dos helpers — o mesmo heap do guest por
  outra porta —, e o que faltava de verdade era o `CheckAvail`
- **A superfície de desenho, sem `glViewport`, é a tela inteira.** Ela é deduzida da maior
  viewport que o jogo usa — é assim que o Quake, que desenha em 320×400 numa tela de 640×480,
  aparece no tamanho certo. O Peteca não chama `glViewport` uma vez sequer: o conjunto vazio
  dava 1×1, e a apresentação esticava um pixel branco por toda a tela
- **A matriz de textura é aplicada aos `uv`.** O Peteca manda as coordenadas em ponto fixo — a
  quadra chega com 32767, o extremo de um inteiro de 16 bits — e é a matriz de textura que as
  traz de volta para `0..1`. Ignorá-la fazia o `GL_REPEAT` dar a volta na textura a cada pixel,
  e a quadra e a arquibancada saíam como confete das cores certas. Custou uma caçada longa:
  as texturas decodificavam limpas, o filtro era bilinear e o passo era de ampliação, não de
  redução — tudo apontava para lugar nenhum até os `uv` crus aparecerem
- **A espera ocupada não é emulada instrução por instrução.** O Peteca não arma timer para o
  próximo quadro: ele lê o relógio e cede a vez até o prazo chegar, 69 mil leituras por quadro.
  No console isso não custa nada, porque o tempo passa sozinho; aqui eram 14 milhões de
  instruções emuladas por quadro, e o jogo rodava seis vezes mais devagar que o aparelho.
  Quando o jogo só lê o relógio e cede a vez, o emulador adianta o relógio — 17 segundos
  virtuais caíram de 101 para 12 segundos reais, com os mesmos 428 quadros
- **A janela tem freio de tempo real.** O relógio virtual adianta o tempo ocioso em vez de
  gastá-lo, o que mantém honesto o tempo que o jogo *mede*; sem freio, o Crash rodava 12
  segundos virtuais em 1,5 real
- A ordem da vtable de `IWeb` não está no SDK 4.0.2 — a interface foi aposentada. Ela saiu do
  que o jogo chama: `IWeb` usa `DECLARE_IBASE`, com só `AddRef` e `Release` antes dos métodos
  próprios, e o slot 3 recebe um ponteiro para um vetor na pilha, que é a assinatura do
  `AddOpt`. Pelo `IQI` esse slot seria o `GetResponse`, cujo segundo argumento é um ponteiro
  de saída — e ali chegava um endereço de trampolim
- **A largura do `printf` é respeitada.** O `cformat` lia flags e largura e descartava, o que
  bastava para o `DBGPRINTF` — mas o mesmo formatador atende o `snprintf` do guest. O Resident
  Evil 4 monta o nome dos estágios com `%s_%02d.h2z`; sem a largura ele procurava
  `3d_stg02_0.h2z` e o arquivo é `3d_stg02_00.h2z`. Os doze estágios não eram achados e o jogo
  rodava sem textura nenhuma, sem quebrar e sem erro
- **Strings `char` do guest são ISO-8859-1**, que é o que o BREW usa. Eram lidas com
  `from_utf8_lossy`: o `0xE7` de um "ç" não é UTF-8 válido, virava `U+FFFD` e voltava para a
  memória do jogo como três bytes de lixo. A ida e a volta agora preservam byte a byte
- **O rasterizador roda em várias threads.** Cada draw call vira um lote de triângulos já
  projetados; acima de 64 mil fragmentos de caixa envolvente o quadro é dividido em faixas
  horizontais, uma por núcleo. Cada faixa percorre o lote na ordem em que o jogo desenhou, então
  a transparência empilha igual e o resultado é bit a bit o mesmo da versão serial
- **O orçamento de instruções não custa mais um hook por instrução.** Passar `count` para o
  `uc_emu_start` faz o unicorn instalar um `UC_HOOK_CODE`, chamado em *toda* instrução. O teto
  passou para o hook de bloco que já existia para contar, e o tempo gasto dentro do unicorn caiu
  36%
- **A interface não limita mais a velocidade do jogo.** Ela dava uma fatia fixa de 16 ms por
  quadro desenhado; com a janela sincronizada ao monitor, bastava emulação mais desenho passarem
  de um retraço para o período dobrar, e o jogo ficava com 16 de cada 33 ms — travado em 50%.
  O orçamento agora é o tempo real decorrido desde o quadro anterior
- **O sistema de arquivos não distingue maiúsculas de minúsculas**, como o do console. Os dez
  ports de arcade pedem `font.fnz` e trazem `font.FNZ` no pacote: nenhum deles passava do
  `EVT_APP_START`, porque a fonte não abria e o ponteiro nulo vinha logo depois. Com Raging
  Thunder 2 e Reckless Racing, que caíam no mesmo buraco em outros arquivos, **doze jogos
  passaram a rodar de uma vez**. No Windows e no macOS isso funcionava por acaso
- **`IHash` e o MD5**, que era a única API faltando em todo o acervo: o Zeeboids passou a rodar
- **O applet é reconhecido pela forma do registro no `.mif`**, e não pela faixa de ClassID da
  Qualcomm — o Zenonia usa `0xbf2e2021` e um homebrew usa `0x12345678`
- **O módulo escolhido num pacote com mais de um é o que declara applet**, o que destravou o
  Action Hero 3D, cujo pacote traz dois jogos
- **O semihosting do ARM é atendido** em vez de virar exceção: o Peggle e o Zuma's Revenge, que
  paravam nele, passaram a rodar, e o log que eles emitem por esse caminho agora aparece
- **A carga de recurso não é mais recusada por um `pnBufSize` não inicializado**, que era o que
  deixava o Peggle com sessenta mil bytes de zeros no lugar da imagem
- **A velocidade mostrada é a de agora**, medida sobre os últimos 500 ms. Era a média desde o
  início da sessão: ao entrar numa tela mais pesada, o número escorregava por minutos rumo ao
  novo valor e parecia uma piora contínua onde o emulador já estava estável

## Áudio

- **Os jogos tocam.** O caminho é `IMediaUtil::CreateMedia` com um `AEEMediaData`, depois
  `SetMediaParm`, `Play`. Os três que têm som — Quake, Zeebo Sports Peteca e Double Dragon —
  entregam **RIFF/WAVE de PCM em memória**, mono, de 8 ou 16 bits, em taxas de 11025 a 44100 Hz.
  Nenhum entrega MP3, apesar de o Quake ter oito soltos na pasta: não foi preciso decodificador
  de formato comprimido
- **`src/wav.rs`** lê o RIFF e **`src/audio.rs`** mistura as vozes e fala com a placa pelo
  `cpal`. Cada voz é reamostrada para a taxa da placa, porque o Peteca toca sons de três taxas
  diferentes na mesma sessão
- Três valores não estão em header nenhum do SDK 4.0.2 e saíram da observação:
  - `MM_PARM_MEDIA_DATA = 1` e `MM_PARM_VOLUME = 4`, que batem com a ordem da tabela
    `MM_PARM_XXX` da documentação — o 1 recebe ponteiro para struct, o 4 recebe números de 0 a
    100
  - `AEE_MAX_VOLUME = 100`: o Double Dragon manda 0 e 100, o Quake manda 70 e 80, e nada em
    nenhuma ROM passa de 100
  - `MMD_BUFFER = 1`, o `clsData` que os três jogos usam
- O Pac-Mania usa **IMA ADPCM**, não PCM: quatro bits por amostra, em blocos com a primeira
  amostra e o índice da tabela de passos no cabeçalho. Ele apareceu porque o leitor **recusa
  com nome** em vez de recusar em silêncio
- **`--dump-audio=ARQUIVO.wav`** grava o que sairia pelo alto-falante, misturado no ritmo do
  relógio virtual. Existe porque conferir áudio de ouvido não cabe num teste: com ele dá para
  medir pico e blocos com som sem depender de escutar
- O aviso de fim de som (`MM_STATUS_DONE`) **não é entregue** ao jogo. O `AEEMediaCmdNotify`
  teria de ser montado na memória do guest e o layout dele não está nos headers que temos; o
  `GetState` já responde certo, que é o que os jogos consultam

## Interface

- **Biblioteca de jogos, com varredura da pasta de ROMs** (`src/library.rs`). Ela cobre as
  duas disposições que aparecem na prática: a do console, `<Título>/mod/<id>/<nome>.mod` com o
  `.mif` num `mif/` irmão, e a dos exemplos do SDK, com os dois lado a lado. O título vem da
  pasta no primeiro caso, porque ali o nome do arquivo é um identificador numérico
- **Configurações guardadas em disco** (`src/settings.rs`), na pasta que cada sistema reserva
  para isso. Todo campo tem padrão e a leitura nunca falha: arquivo ausente, truncado ou de uma
  versão mais nova precisa deixar o emulador abrir, não impedi-lo
- **Idiomas por arquivo JSON** (`src/i18n.rs`). Português e inglês vêm embutidos no binário,
  para o emulador funcionar sozinho; qualquer outro entra como arquivo numa pasta `lang/`, sem
  recompilar. Um arquivo com o código de um embutido o substitui, que é como corrigir uma
  tradução sem esperar versão nova. Um teste compara as chaves dos dois idiomas: chave que
  existe só num deles é texto que aparece em inglês no meio do português
- **A sessão de jogo saiu da linha de comando** (`src/session.rs`) — carregar o módulo, criar o
  applet, entregar o `EVT_APP_START` e girar o laço de eventos agora é uma coisa só, que a
  linha de comando roda até um limite e a interface toca um pedaço por quadro desenhado
- A interface é `egui`/`eframe`, e o emulador roda **na mesma linha de execução dela**: o
  núcleo do unicorn não atravessa linhas de execução, e o `Session::step` já devolve o controle
  a cada fatia de tempo real
- **Aba de controles**, com o mapeamento em `src/bindings.rs` e a leitura dos controles de
  verdade em `src/gamepads.rs`, pelo `gilrs`. O mapeamento é guardado **por nome** — da tecla,
  do botão do host, do botão do Zeebo — e não por índice: índices mudam quando uma tabela muda,
  nomes sobrevivem, e é isso que faz um arquivo de configuração escrito hoje continuar valendo
- O teclado e o controle valem **juntos**: cada botão do Zeebo aceita mais de uma origem, que é
  o mesmo mecanismo que já fazia `Espaço` e `X` serem o mesmo botão
- **Mapa visual do controle** (`src/padview.rs`): a arte é um PNG (`assets/controller.png`) e as
  regiões, um SVG invisível do mesmo tamanho (`assets/controller-map.svg`) em que o `id` de cada
  forma é o nome de um botão. Separar os dois deixa o desenho ser trocado sem tocar em código.
  Cada forma vira uma silhueta recortada, e dela saem o realce e o teste de clique — que assim
  respeita o formato do botão em vez de uma caixa retangular. `back`, `l2` e `r2` não existem
  nessa arte: não acendem, e seguem configuráveis pela lista
- **Analógicos** (`AxisSource` em `src/bindings.rs`): o console tem quatro eixos, e o
  direcional é reportado como `X` e `Y` — o manche esquerdo cai neles, o direito em `Z` e `RZ`,
  que antes não tinham nada os alimentando. O valor analógico entra **depois** dos botões e só
  fora da zona morta: assim ele acrescenta curso ao que o direcional escreveu em vez de apagá-lo
  quando o manche está em repouso. `Y` vai invertido porque no console cima é negativo; o
  sentido de `Z`/`RZ` é suposição, o arquivo do console nomeia os dois sem dizer o sentido
- O manche **não** aperta o direcional: são controles diferentes, e ligar os dois faria os dois
  agirem juntos e nenhum deles sozinho. Um arquivo de configuração escrito antes dos eixos é
  ajustado ao abrir (`Controls::adopt`), porque o padrão de um campo novo nem sempre é o vazio
- **Falta o jogador 2.** A estrutura já é uma lista de jogadores e a tela avisa, mas ligar o
  segundo exige que o emulador alimente dois controles — hoje o `Machine` tem um `set_pad` só
- **Biblioteca em cartões**, com a imagem de cada jogo. O `.mif` guarda os ícones do título em
  seções que começam com `u16` de comprimento **do próprio cabeçalho** seguido do tipo MIME; os
  formatos que aparecem nas ROMs reais são PNG, BMP paletado e JPEG, e `src/icon.rs` lê os três
  (o BMP à mão, porque o que os `.mif` trazem é o subconjunto mais simples do formato)
- **Não existe capa dentro das ROMs.** O maior ícone dos títulos antigos é 65×42 — é ícone de
  menu, não arte. O Resident Evil 4 é a exceção, com 192×192. Por isso a biblioteca também
  procura uma imagem de mesmo nome ao lado do jogo, e cai no logo embutido quando não há nenhuma
- **Falta decidir o que fazer com o `--window`.** Ele é a janela antiga, de `minifb`, e faz o
  mesmo que a interface nova — são dois sistemas de janela no mesmo binário

- **A superfície não é mais copiada por chamada.** Quando o jogo pede o `IDIB`, ele passa a
  enxergar os pixels na memória dele, e a superfície precisava ser copiada nos dois sentidos em
  volta de cada chamada de desenho — 1,2 MB de ida e volta. O Pac-Mania desenha **pixel a pixel**
  pela API (1,16 milhão de `DrawPixel` em dez quadros) e pagava a superfície inteira por pixel:
  110 quadros levavam 232 s, dos quais 1,8 s eram CPU emulada e 161 s eram cópia. Agora
  `DrawPixel` e `GetPixel` mexem nos dois bytes do pixel direto no buffer do jogo, e só quem pode
  tocar mais de um pixel paga a cópia. Mesmos 59 milhões de instruções, **232 s → 3,4 s**
- **A cópia era cobrada até de quem não desenha.** `SetClipRect`/`GetClipRect` e o resto dos
  ajustes de estado de `IDisplay`/`IGraphics` pagavam a superfície inteira: o Pac-Mania consulta
  o recorte 112 mil vezes a cada quinze segundos. Vinte segundos virtuais caíram de mais de
  500 s para 26 s
- **O recorte de `IDisplay` passou a valer.** Ele era aceito e ignorado. O Pac-Mania desenha a
  folha de fontes **inteira** com `BitBlt` e conta com o recorte para que só a letra apareça —
  sem ele, a folha toda ia para a tela, que era aquele "debug de fontes" na abertura. O corte
  anda com a origem na fonte junto: encolher só o destino mostraria o canto errado da imagem

- **`IFileMgr::EnumInit`/`EnumNext`** (`src/machine.rs`, `Vfs::resolve_dir`): a listagem sai de
  uma vez no `EnumInit` e a fila fica **dentro do `IFileMgr`**, como o BREW faz — dois
  gerenciadores enumeram diretórios diferentes ao mesmo tempo. O nome devolvido é o caminho
  completo, com o prefixo que o jogo passou: é ele que volta para o `OpenFile` logo em seguida.
  Depois de um `EnumNext` falso o `GetLastError` devolve `EFAILED` mesmo tendo dado tudo certo —
  a documentação chama isso de compatibilidade com o cliente 1.0
- **`IDisplay::MeasureTextEx`**: largura por avanço fixo de caractere, coerente com a altura que
  o `GetFontMetrics` informa. Como não desenhamos texto, o número serve para o jogo centralizar
  e quebrar linha; sem nenhum, o Pac-Mania parava na escolha de idioma
- **`ISHELL_GetPrefs`/`SetPrefs`**: guardadas em memória, por classe e versão. Gravar em disco
  exigiria inventar um formato — o console tinha um, e não sabemos qual
- **`AEEHelpers::SetupNativeImage`** (o `CONVERTBMP` do SDK): decodifica e devolve um `IBitmap`.
  O ponteiro vai direto para o `IDISPLAY_BitBlt`, que recebe um `IBitmap *` — devolver um bloco
  solto de pixels obrigaria o desenho a ter um segundo caminho. Quem diz quanto ler é o
  cabeçalho da imagem; por ora só o BMP, que é o que os jogos passam
- **`AEECLSID_PNGDecoderBREW`** (`0x01030766`), com o `IImageDecoder` e o `IForceFeed` que o
  alimenta. O jogo cria o decodificador, pede a ele a interface de entrada, escreve o arquivo em
  pedaços e busca o bitmap. O `IForceFeed` é um objeto **separado**: as duas interfaces têm
  métodos diferentes no mesmo slot, e devolver o mesmo ponteiro faria o jogo chamar o método
  errado. A IID `AEEIID_FORCEFEED = 0x0101eb0b` **não está nos headers que temos** — foi
  identificada pelo uso, e o comportamento confirma
- **`ISHELL_Prompt`** responde falso: "não criei o diálogo" é a verdade, o BREW prevê essa
  resposta, e o jogo segue pelo caminho de quem não pôde perguntar

## Jogos em `.zip`

- A varredura reconhece `.zip` e o emulador **extrai para um cache** antes de rodar, em vez de
  ler de dentro do pacote. Os jogos gravam — o Peteca tem um `.sav` —, e escrever de volta num
  zip não é coisa que se queira fazer; extrair resolve isso e deixa o resto do emulador sem
  saber que o zip existe. A pasta do cache é nomeada pelo arquivo mais o tamanho e a data, de
  modo que reabrir o mesmo jogo reaproveita a extração
- Havendo mais de um `.mod` dentro, ganha o que está na disposição do console
  (`<Título>/mod/<id>/<nome>.mod`), mesmo sendo o mais fundo: um pacote com o jogo e um extra
  tem o módulo do jogo ali, e o extra é que costuma estar solto

## Onde cada ROM que ainda não roda está parada

O levantamento completo está em
[docs/implementacao/11-compatibilidade.md](docs/implementacao/11-compatibilidade.md) e é gerado
por uma varredura que dá para repetir. O placar de hoje: **45 rodam**, 7 não criam o applet, 6
param no laço de quadros e 3 são lentos demais. A categoria "falha no `EVT_APP_START`", que tinha
dez jogos, está zerada.

Os dezesseis que sobraram:

- **Bejeweled Twist**: não falta API nenhuma — todas as classes que ele pede existem. Ele lê um
  campo do próprio objeto (`[this+0x20]`, em `0x00032b60`) que o construtor em `0x00017eb8` zera
  e ninguém preenche depois. O objeto vizinho (`[this+0x40]`) é um `IDIB`, pelos campos que a
  função lê: `nPitch` como `int16` em `+0x18` e `nDepth` em `+0x1c`
- **Peggle**, **Prey Evil**, **Toy Raid**: ponteiro nulo no laço de quadros, cada um num
  endereço próprio. Sem causa comum aparente; é caso a caso, com o `--watch`
- **Zumas Revenge**: exceção do núcleo ARM em `0x00010f94` — o único do conjunto
- **Zeeboids**: `IHash::slot[4]`, a **única API faltando** em todas as 61 ROMs
- **Action Hero 3D e Zenonia** não acham o `.mif` ao lado do módulo — problema nosso de layout de
  pacote, não do jogo
- **Z-Wheel** segue no `AEECLSID_SQLMGR` (`0x0102c4e8`). O `ISQLMgr` é a API antiga de banco do
  BREW (o `deprecated_replacements.htm` do SDK a aponta para `dbc_IFactory`), e o SDK 4.0.2 não
  traz mais o header dela — a vtable teria de sair de engenharia reversa do binário. Do lado do
  conteúdo, o `doom_insert.sql` do dump mostra o formato: tabelas `GAMEINFO` e `TITLETEXT`
- **Zeebo App** (`0x01028e51`), **Zeebo Channels** (`0x0100102e`), **Tork and Kral** e
  **Need For Speed Carbon**: param antes de criar o applet, os dois últimos sem classe
  desconhecida e sem causa levantada
- **Alice no País das Maravilhas**, **Heavy Weapon** e **Turma da Mônica**: lentos demais
- **Pac-Mania** roda, mas a tela sai preta. O relatório dele diz "um recurso pedido por
  `LoadResObject` não é um PNG que saibamos ler"; o levantamento anterior identificou esses
  recursos como `.tga`, e não há decodificador de TGA no código

## Em andamento

- Nada no momento.

## Próximos passos

O placar é de **50 dos 62** rodando. A fila mudou de natureza: por muito tempo o trabalho era
fazer o jogo abrir, e agora a maior parte do valor está em fazer bem o que já abre.

### Jogar direito o que já roda

1. **O direcional dos jogos que leem os dois canais.** Com a tabela de UIDs corrigida, o menu do
   Tênis anda um item por toque e fica — a medição está no
   [09-entrada.md](docs/implementacao/09-entrada.md). O que sobra é conferir jogo a jogo se
   alguém ainda anda dois: a matriz de quem lê o quê está levantada, e os candidatos são os que
   chamam `GetNextButtonEvent` **e** `GetPositionState` todo quadro — Zeebo Sports, zeetris,
   Zeeboids e a série Extreme
2. **Fonte.** `DrawText` recebe o texto certo e não tem com o que desenhá-lo. Duas origens
   possíveis, ambas verificáveis: o dump da firmware
   (`docs/vendor/tripleoxygen/dump/nand/1.1.2/partitions/1.1.2_APPS.bin`), que é a fonte que o
   console usa de verdade, ou os recursos do simulador do SDK (`bin/BrewRes.dat`,
   `bin/SimulatorRes.dll`). A primeira é a fiel; a segunda deve ser mais fácil de achar
3. **Peggle e Pac-Mania rodam e não mostram nada.** O Peggle já desenha a geometria certa desde
   os campos do `IDIB`, mas fica preso no carregamento e os sprites saem sem textura — ele
   redecodifica as mesmas nove imagens em volta. O Pac-Mania diz "um recurso pedido por
   `LoadResObject` não é um PNG que saibamos ler"; o levantamento antigo apontou `.tga`, e não há
   decodificador de TGA no código
4. **O Heavy Weapon desenha com as cores erradas.** Ele parou de ser lento e passou a desenhar:
   a tela de idioma sai em verde e preto, com as formas no lugar certo. É formato de textura, não
   geometria

### Fazer abrir o que não abre (12)

5. **Os sete que param no laço**: Action Hero 3D (`0x00055568`), Alice, Turma da Mônica,
   Bejeweled Twist (já tem análise pronta na seção própria abaixo), Prey Evil (salta para um
   ponteiro de função nulo logo depois de chamadas GL), Toy Raid e Zuma's Revenge
   (`0x0000000c`). Caso a caso, com o `--watch` e o `--profile`
6. **Os cinco que não criam o applet**: Zenonia pede `0x01003109`, que é do subsistema de texto
   dele (o log diz `CWBLText::Create() failed!`); o Opera Mini pede `0x0100102e`, que é de rede
   — o módulo dele traz `socket://zeebo-cust.opera-mini.net:1080/`; o Zeebo App pede
   `0x01028e51`; o Z-Wheel segue no `AEECLSID_SQLMGR`; o Need For Speed Carbon não pede classe
   nenhuma e não tem causa levantada
7. **O Tekken 2 não passa da tela de título**, e agora é entrada, não desempenho: ele registra o
   sinal de botão, drena dois toques com `GetNextButtonEvent` e para de responder. Depois disso,
   o que sobra nele é despacho — 1,08 milhão de `DrawPixel` mais outro tanto de `RGBToNative`
   por quadro, duas chamadas de API por pixel, que são quase todo o tempo dos 9,7 segundos que
   ele leva para rodar seis. Essa parte é o item 8

### Desempenho

8. **O custo por chamada de API**, hoje ~7 µs, porque toda chamada é um `emu_stop` seguido de um
   `emu_start`. Atendê-las dentro de um hook, sem parar a emulação, é redesenho do trampolim e é
   a maior melhoria que resta. Vale 2,5 milhões de chamadas em 25 s de Quake e 6 milhões por
   quadro no Heavy Weapon
9. **O alocador do guest.** A `free_list` é varrida linearmente e o `free` nunca junta blocos
   vizinhos: ela chega a 830 entradas no Crash e 1125 no Bejeweled, e cada `malloc` percorre
   isso. Não é gargalo hoje, mas é fragmentação que só cresce

### Baixa prioridade, com o porquê

10. **As seis classes de `0x0103d8de` a `0x010426e3`**, pedidas por dezessete jogos. Elas
    pareciam ser o que travava os ports de arcade e **não eram** — nenhum dos dezessete precisa
    delas, todos recebem `ECLASSNOTSUPPORT` e seguem pelo caminho alternativo. Vêm do
    `GLES_ext.c` do SDK, ao lado das extensões que já temos. O que falta saber é se alguma muda
    o que aparece na tela, e isso se descobre olhando o desenho
11. **`AEECLSID_SQLMGR`** e um motor de SQL mínimo atrás dele: um jogo só, o Z-Wheel
12. **Rede**, para o Opera Mini. Faria ele abrir, não funcionar: o `zeebo-cust.opera-mini.net`
    saiu do ar com os servidores da TecToy. Envolve dar acesso à rede a um binário de origem
    externa, então é decisão de projeto antes de ser tarefa

## O `resources.dat` do Bejeweled Twist

Decifrado. O formato:

  | Campo | Formato |
  |---|---|
  | Assinatura | `PPCPRCON` (8 bytes) |
  | Cabeçalho | três `uint32`: nº de entradas + 1, fim da tabela, e um terceiro campo |
  | Entrada | `id: uint16`, `tipo: uint8`, `offset: uint32`, `tamanho: uint32` — 11 bytes |
  | Última entrada | id 10000, inline: `English^Español^Deutsch^Français^Italiano^Brasileiro~` |

  São 206 entradas, em quatro tipos: **0 = PNG**, **1 = WAV** (RIFF), **2 = texto** (campos
  separados por `^`) e **3 = formato próprio**, em pedaços de 32 KB. As faixas de id são
  1000–1003, 3000–3066, 5000–5007, 9000–9125 e 10000.

O tipo 3 é **um único arquivo de 4,1 MB fatiado em pedaços de 32 KB**, não um formato
proprietário: o jogo remonta os pedaços e o que sai são PNGs comuns. A entropia alta era só a
compressão do próprio PNG.

O pipeline completo, que agora funciona: o jogo lê os pedaços, reconhece a assinatura com
`memcmp(buffer, "\x89PNG", 4)`, monta um `IMemAStream` sobre o buffer, cria um `IImage` com
`AEECLSID_PNG` e entrega o stream. São 443 imagens por execução.

## Ideias para estudar (sem compromisso)

- **Servidor alternativo para o Z-Wheel.** O menu do console falava com os servidores da TecToy
  (loja, OTA, ativação), que saíram do ar. Existe a possibilidade de escrever um servidor
  próprio que responda no lugar deles, devolvendo o catálogo local. Ainda é só uma
  possibilidade: falta estudar o protocolo, se é viável e quais as implicações. Anotado aqui
  para não se perder.

## O objeto de texto do Bejeweled Twist

Depois de carregar e blitar 18 glifos — as imagens têm 14×22 e 1×22 pixels, é a fonte do jogo —
ele monta um contexto de desenho (`SetClip`/`SetColor`/`SetFillColor`) e quebra em `0x32b78`:

```
00032b60  push {r0-r8, sb, sl, fp, lr}
00032b64  mov  r5, r0
00032b68  ldr  r0, [r0, #0x20]     ; ← zero
00032b74  bl   #0x3a7c8            ; add r0, r0, #0x10; bx lr
00032b78  ldr  r2, [r0, #0x14]     ; falha em 0x24
```

O `--watch` sobre `r5 + 0x20` mostra cinco escritas, e a **última é o construtor zerando o
campo** (`0x17ef0`, dentro de `0x17eb8`, que zera `+0x10` até `+0x28` em sequência). As quatro
anteriores são de outros objetos que ocuparam o mesmo endereço antes. Ou seja: o objeto foi
construído e o passo que deveria configurá-lo nunca veio.

O chamador (`0x32dac`) só entra nessa rotina quando `[r4+0x24]` também é zero — e `+0x24` é
zerado pelo mesmo construtor, duas instruções depois. Os dois campos vazios são o mesmo bloco
recém-inicializado.

O que se sabe do objeto, pelo mapa de escritas campo a campo:

| Campo | Valor | Quem escreve |
|---|---|---|
| `+0x00` | `0x10fc8` | construtor (vtable primária) |
| `+0x08` | `0x11004` | construtor (vtable secundária — herança múltipla) |
| `+0x10` | `1` | `0x6ecf4`, um método virtual da segunda interface |
| `+0x14`, `+0x18` | `0x85` | `0x6edb8` |
| `+0x38` | zerado ao fim | `0x85c98`, o `operator=` de um ponteiro contado |
| `+0x0c`, `+0x1c`, `+0x20` | nunca preenchidos depois do construtor | — |

O `+0x38` chegou a apontar para um objeto de vtable `0x1104c` — a mesma família de classes que
aparecia no primeiro travamento investigado, o do sistema de recursos.

### De onde o objeto vem

`0x15634` aloca 0x44 bytes, constrói com `0x16890` e chama o slot 3 da vtable. A vtable `0x10fc8`
guarda **deslocamentos relativos à própria vtable**, não endereços: o slot 3 vale `0x565ac`, e a
função é `0x10fc8 + 0x565ac = 0x67574` — a rotina grande de carregar imagem.

### O carregamento de imagem funciona

O `--code` sobre `0x37a60` (a validação do PNG) mostra a trilha real: ela percorre
`IHDR → PLTE → tRNS → IDAT → IEND`, dezenove vezes seguidas, e sai por `0x37bec` com
`mov r0, #1`. **`1` é o retorno de sucesso**, e `0x156a8` — para onde o chamador desvia — é o
epílogo que guarda o objeto criado, não um caminho de erro. As duas leituras anteriores estavam
invertidas: nada falha aí.

### O que realmente trava

A classe do objeto tem a vtable `0x10fc8`, com deslocamentos relativos. Os slots que importam:

| Slot | Função | Papel | Executa? |
|---|---|---|---|
| 3 | `0x67574` | decodifica a imagem | sim, com sucesso nas 18 |
| 5 | `0x32d04` | desenha | sim — é onde trava |
| 6 | `0x285b8` | carrega pelo ID do recurso | sim, 8 vezes, **nenhuma falha** |
| 10 | `0x6edb4` | preenche `+0x14`/`+0x18` | sim |
| 12 | `0x6ed60` | preenche `+0x20`/`+0x24` | **nunca** |

O desenho é uma cascata de três campos, e basta o do meio para o travamento não acontecer:

```
0x32d14  ldr r0, [this+0x28]   ; zero → desvia
0x32d70  ldr r0, [this+0x24]   ; zero → desvia
0x32b68  ldr r0, [this+0x20]   ; zero → estoura
```

### A cadeia inteira, de cima a baixo

A varredura de pilha dá o caminho, e seguindo cada elo até o topo chega-se ao **callback do
timer** — o mesmo que registramos pelo `ISHELL_SetTimer`:

```
0x12fec  callback do timer (PFNNOTIFY)
0x7c348  atualiza o quadro (mede o tempo com aee_GetUpTimeMS)
0x719a4  chamada virtual do slot 2 da cena
0x304c4 → 0x30a80
0x12c9c → 0x2eac0 → 0x2e828
slot 5   desenhar
0x32b60  estouro
```

Ou seja: **não falta evento nenhum**. É o laço de quadros normal do jogo desenhando a interface,
e nele o recurso de tema chega sem os ponteiros que o desenho procura. (Um dos elos que a
varredura devolveu, `0x30890`, era falso positivo — é o início de uma função, não um retorno.
O ruído é o preço de não ter tabela de desenrolamento.)

Em `0x30a74` o objeto a desenhar é buscado assim:

```
ldr r0, [r4, #4]          ; contexto
add r0, r0, #0x1000
ldr r0, [r0, #0x180]      ; G — a estrutura de tema
add r1, r0, #0x24         ; G+0x24: o recurso a desenhar
bl  #0x12c9c
```

E `G+0x24` é instalado em `0x656c8`, dentro da rotina que monta o tema (`0x6569c`):

```
000656ac  ldr r0, [r4, #0x28]   ; o ID do recurso
000656bc  bl  #0x244ec          ; carrega por ID
000656c4  add r0, r4, #0x24
000656c8  bl  #0x85c2c          ; G->campo24 = recurso
```

`0x244ec` cria o objeto (`0x15634`) e chama o slot 6 para carregar pelo ID. **As oito chamadas
têm sucesso**, inclusive a do objeto que depois trava. Antes delas, `0x65704` faz o mesmo em
laço para a lista de recursos do tema.

Os IDs pedidos são **5000 e 5002–5007** — a faixa de **tipo 0 do `resources.dat`, que é PNG**.
Todos existem no arquivo, todos carregam, e o objeto que trava é o do ID 5000. O rastreio
confirma que ele passa pelo slot 3 (decodificar) como os outros dezessete.

O mapa completo de escritas no objeto fecha o caso: `+0x1c`, `+0x20`, `+0x24` e `+0x28` recebem
exatamente uma escrita cada — o zero do construtor. Tudo o que aparece antes disso no mesmo
endereço pertencia a uma **superfície do jogo** que o ocupava antes (dá para reconhecer a vtable
inline sendo montada: `0x139d8` em `+0x24` é o `BltIn`, `0x13b0c` em `+0x30` o
`CreateCompatibleBitmap`).

Ou seja: o recurso é encontrado, decodificado e instalado; o que nunca acontece é a chamada do
slot 12, que instalaria os dois ponteiros que o desenho procura. Ele não aparece em nenhum
caminho executado, e como a chamada seria virtual, não há como localizá-la por busca estática —
os três lugares do módulo que chamam um slot 12 são de outras classes e não rodam.

## O pacote de imagens dentro do `resources.dat`

O rastreio dos IDs de recurso pedidos pelo tema (5000, 5002–5007) não bate com as leituras de
arquivo: o jogo só lê os **128 pedaços do tipo 3**, e nenhum recurso de tipo 0, 1 ou 2. A
explicação é que o tipo 3 é um **pacote com índice próprio** — os "IDs" do tema são resolvidos
dentro dele, não pela tabela do `resources.dat`.

O pacote tem 12 bytes de cabeçalho e, antes de cada PNG, outros 12 com o mesmo formato:

```
00 00 00 00 | 00 00 00 10 | 00 00 20 e0
   quatro zeros   flags       tamanho (big-endian)
```

Os valores de flags observados são `0x01`, `0x10` e `0x40`. São 235 imagens no pacote.

Os flags, nas 235 entradas: `0x10` em 190, `0x01` em 26, `0x40` em 10, `0x04` em 7, e um caso
de `0x11` e outro de `0x50`.

**A hipótese de que esses metadados alimentam os setters foi testada.** O watchpoint de leitura
mostra o jogo lendo os 12 bytes de cada entrada, em duas partes — oito bytes e depois quatro:

```
0x10199ef0 lido (8 bytes)
0x10199ef8 lido (4 bytes)
```

Então ele **é** lido e interpretado. Mas isso não explica o travamento: nós apenas copiamos
bytes do arquivo, de modo que o jogo recebe daí exatamente o que receberia no console. Se o
flag `0x10` das imagens do tema significa "sem cor própria", não chamar o slot 13 é o
comportamento correto — e o problema está noutro lugar.

## A comparação de cor está certa; falta um evento

O slot 5 compara, em `0x32d64`, a cor do recurso (`+0x2c`) com uma cor construída por
`0x143e4`. Desmontando os dois construtores de cor:

- `0x18530` — o padrão, que o construtor do objeto usa: zera os 12 bytes.
- `0x184c0` — o com componentes: recebe R, G, B em `r1`–`r3` e o alfa na pilha, e calcula
  também o RGB565 e o RGB888. `0x143e4` o chama com `(0, 0, 0, 0)`.

Ou seja, os dois lados são `(0,0,0,0)` **por construção**, e a comparação dá igual porque
deve dar. A semântica é "sem tingimento definido → desenhe o recurso original", e o caminho
tomado é o normal. **A cor não é o problema.**

O problema é o par `+0x20`/`+0x24`, e agora se sabe de onde ele viria. O **slot 7**
(`0x28640`) é "criar a superfície": recebe largura, altura e formato, instala a superfície em
`+0x20`, zera `+0x24` e, em caso de sucesso, escreve `{1, largura, altura}` em `+0x10`,
`+0x14` e `+0x18` — que é exatamente o padrão que o objeto tem hoje (`1`, `0x85`, `0x85`),
só que escrito pelo slot 10.

O par `+0x20`/`+0x24` vem do **slot 7** (`0x28640`), que é "criar a superfície": recebe largura,
altura e formato, instala a superfície em `+0x20`, zera `+0x24` e, em caso de sucesso, escreve
`{1, largura, altura}` em `+0x10`, `+0x14` e `+0x18` — que é o padrão que o objeto tem hoje
(`1`, `0x85`, `0x85`), só que escrito pelo slot 10. **Nenhuma das chamadas ao slot 7 executa.**

### Uma pista falsa, registrada para não ser seguida de novo

O manipulador de eventos em `0x120a8` — que chama um "slot 7" e um "slot 12" conforme o subtipo
do evento — **não é do recurso gráfico**. É do subsistema de **mídia**: o pool literal ao lado
dele carrega `AEECLSID_MEDIA` (`0x01005500`), e a função que o registra (`0x67cc0`) compara o
resultado de uma detecção de tipo com `AEECLSID_MEDIAMIDI`, `AEECLSID_MEDIAMP3` e
`AEECLSID_MEDIAADPCM`, definindo 1, 2 ou 3 num campo de tipo.

Os `ldr r2, [r1, #0x1c]` e `[r1, #0x30]` daquele dispatcher são slots de **outra vtable**, e
foram lidos como se fossem os da nossa classe. Não existe "evento de tipo 4 subtipo 3" faltando.

Fica dali uma informação para quando chegarmos ao áudio: o jogo identifica o formato com
**`ISHELL_GetHandler`** (slot 32 do `IShell`), que ainda não implementamos.

## O pacote de imagens dentro do `resources.dat`

O rastreio dos IDs de recurso pedidos pelo tema (5000, 5002–5007) não bate com as leituras de
arquivo: o jogo só lê os **128 pedaços do tipo 3**, e nenhum recurso de tipo 0, 1 ou 2. A
explicação é que o tipo 3 é um **pacote com índice próprio** — os "IDs" do tema são resolvidos
dentro dele, não pela tabela do `resources.dat`.

O pacote tem 12 bytes de cabeçalho e, antes de cada PNG, outros 12 com o mesmo formato:

```
00 00 00 00 | 00 00 00 10 | 00 00 20 e0
   quatro zeros   flags       tamanho (big-endian)
```

Os valores de flags observados são `0x01`, `0x10` e `0x40`. São 235 imagens no pacote.

Os flags, nas 235 entradas: `0x10` em 190, `0x01` em 26, `0x40` em 10, `0x04` em 7, e um caso
de `0x11` e outro de `0x50`.

**A hipótese de que esses metadados alimentam os setters foi testada.** O watchpoint de leitura
mostra o jogo lendo os 12 bytes de cada entrada, em duas partes — oito bytes e depois quatro:

```
0x10199ef0 lido (8 bytes)
0x10199ef8 lido (4 bytes)
```

Então ele **é** lido e interpretado. Mas isso não explica o travamento: nós apenas copiamos
bytes do arquivo, de modo que o jogo recebe daí exatamente o que receberia no console. Se o
flag `0x10` das imagens do tema significa "sem cor própria", não chamar o slot 13 é o
comportamento correto — e o problema está noutro lugar.

## A comparação de cor está certa; falta um evento

O slot 5 compara, em `0x32d64`, a cor do recurso (`+0x2c`) com uma cor construída por
`0x143e4`. Desmontando os dois construtores de cor:

- `0x18530` — o padrão, que o construtor do objeto usa: zera os 12 bytes.
- `0x184c0` — o com componentes: recebe R, G, B em `r1`–`r3` e o alfa na pilha, e calcula
  também o RGB565 e o RGB888. `0x143e4` o chama com `(0, 0, 0, 0)`.

Ou seja, os dois lados são `(0,0,0,0)` **por construção**, e a comparação dá igual porque
deve dar. A semântica é "sem tingimento definido → desenhe o recurso original", e o caminho
tomado é o normal. **A cor não é o problema.**

O problema é o par `+0x20`/`+0x24`, e agora se sabe de onde ele viria. O **slot 7**
(`0x28640`) é "criar a superfície": recebe largura, altura e formato, instala a superfície em
`+0x20`, zera `+0x24` e, em caso de sucesso, escreve `{1, largura, altura}` em `+0x10`,
`+0x14` e `+0x18` — que é exatamente o padrão que o objeto tem hoje (`1`, `0x85`, `0x85`),
só que escrito pelo slot 10.

E slot 7 e slot 12 são chamados juntos, pelo **manipulador de eventos do recurso**:

```
000120a8  push {r3, r4, r5, lr}   ; (this, evento)
000120b0  ldr  r0, [r1, #8]       ; tipo do evento
000120c0  cmp  r0, #4
000120c8  ldr  r1, [r1, #0x10]    ; subtipo, 0..13
000120d4  addls pc, pc, r1, lsl #2 ; tabela de saltos
…
00012158  ldrb r0, [r4, #0x34]    ; subtipo 3
00012160  bne  #0x12188
00012190  ldr  r2, [r1, #0x1c]    ; ← slot 7: cria a superfície
000121b4  ldr  r2, [r1, #0x30]    ; ← slot 12: configura
```

**Esse manipulador nunca é executado** — zero instruções no rastreio. O que falta não é uma
chamada perdida: é um **evento de tipo 4, subtipo 3**, que o jogo nunca entrega ao recurso.
Descobrir quem o envia é o próximo passo.

## Como o desenho chega às superfícies do jogo

O ciclo completo, e vale a pena registrar porque é bonito: o jogo pede uma imagem, nós
decodificamos o PNG, e quando ele manda desenhar numa superfície própria montamos um `IBitmap`
**nosso** com a imagem e chamamos o `BltIn` **dele**. O `BltIn` então faz
`QueryInterface(AEECLSID_DIB)` no nosso bitmap e lê os pixels pelos campos públicos — o mesmo
`IDIB` que implementamos meses antes para o outro lado da relação.

Conferido na desmontagem (`0x139d8`): ele carrega `pSrc` de `[sp, #0x24]`, chama o slot 2 da
vtable com `0x01001045` e depois lê `cx` (`+0x14`), `cy` (`+0x16`) e `nColorScheme` (`+0x1d`).

## As superfícies do Bejeweled Twist não são DIBs

O jogo faz `SetDestination(superfície própria)` antes de cada `IIMAGE_Draw`. Perguntamos a ela
onde ficam os pixels, do jeito que o BREW faria — `QueryInterface` com `AEECLSID_DIB` e, por
garantia, com o `AEEIID_DIB_20` do BREW 2.0. A resposta é `ECLASSNOTSUPPORT` nas duas.

Não é um acidente nosso: o `QueryInterface` dela é, literalmente,

```
00013b64  mov  r0, #0x14      ; ECLASSNOTSUPPORT
00013b68  bx   lr
```

Desmontando a vtable inteira, quase tudo é stub do mesmo feitio (`DrawPixel`, `FillRect`,
`GetPixel`…). Só dois slots têm código de verdade: `AddRef`/`Release` e o **`BltIn`**. É por
ele que o desenho tem de passar.

A consulta ficou no código: custa uma entrada no guest por superfície nova, é o caminho certo,
e uma superfície de outro jogo pode muito bem responder que sim.

## O custo de ligar o `IIMAGE_Notify`

Vale registrar o trade-off, porque ele não é óbvio:

| | Notify desligado | Notify ligado (atual) |
|---|---|---|
| Quadros | 44 | 0 |
| PNGs decodificados | 443 | 18 |
| `IIMAGE_Draw` | **nenhum** | 18 |

Sem a notificação o jogo fica num laço de carregamento que nunca desenha — ele avança mais
quadros, mas não sai do lugar. Com ela, entra no caminho de desenho e esbarra logo no
`IBitmap` próprio. Ficou ligado porque é o comportamento do BREW, e porque o obstáculo que ele
revela é o real.

## Aprendizados que valem registro

- **A pilha guarda o caminho, mesmo sem tabela de desenrolamento.** Ler as palavras da pilha e
  ficar com as que apontam para logo depois de um `bl` reconstrói a cadeia de chamadas com
  falsos positivos toleráveis. Foram nove elos de uma vez, contra um por rodada de desmontagem.

- **Desmontagem estática mostra caminhos; só a execução mostra o percorrido.** Duas rodadas de
  investigação concluíram que a validação do PNG do jogo estava falhando, lendo o `mov r0, #1`
  como erro. O rastreio de código mostrou em dez segundos que ela percorre os chunks até o
  `IEND` e devolve sucesso. Quando a pergunta é "o que aconteceu", ler o código não substitui
  segui-lo.

- **O `IDIB` serve nos dois sentidos.** Foi implementado para o jogo escrever direto na tela;
  acabou sendo como o jogo *lê* as imagens que entregamos a ele. É a mesma interface, com os
  papéis trocados.

- **Há um único ponto seguro para reentrar no guest**: a fronteira entre duas chamadas de API,
  depois que o resultado já está em `r0` e antes de o guest retomar. Disparar o callback de
  dentro do `SetStream` ou acumulá-lo para o fim do quadro derruba o jogo — no primeiro caso
  porque a estrutura dele ainda não está montada, no segundo porque o contexto já passou.
- **Nem todo `IBitmap` é nosso.** O BREW deixa o aplicativo implementar a interface, com a
  vtable embutida no objeto, e o Bejeweled Twist faz isso: o primeiro campo aponta para
  `this + 4`, onde começam os ponteiros para o código do próprio jogo.
- **Uma vtable cheia não quer dizer uma implementação cheia.** A superfície do Bejeweled Twist
  preenche os 16 slots, mas quase todos são `mov r0, #0x14; bx lr` — devolvem
  `ECLASSNOTSUPPORT` e nada mais. Ler os ponteiros não bastava; foi preciso desmontar cada um
  para descobrir que só o `BltIn` faz alguma coisa.

- **`strlen` conta bytes, não caracteres.** Implementá-lo sobre `String` do Rust parece
  inofensivo até aparecer um byte acima de `0x7f`: `from_utf8_lossy` troca cada um por `U+FFFD`,
  que ocupa três bytes. `strlen("\x89PNG")` devolvia **6** em vez de 4 — e como o jogo usa esse
  valor como tamanho do `memcmp` da assinatura PNG, ele comparava dois bytes a mais, concluía
  que os próprios recursos não eram PNG e abandonava o carregamento. Um erro de tipo no host
  virou "o jogo não reconhece o formato dele mesmo".
- **Entropia alta não quer dizer formato proprietário.** Os pedaços tipo 3 do `resources.dat`
  pareciam criptografados; eram PNGs fatiados em blocos de 32 KB. Foi a desmontagem do
  carregador, não a estatística, que respondeu.

- **Os hooks do unicorn não veem as escritas do host.** O `mem_write` que a implementação de uma
  API faz não dispara `MEM_WRITE`, então um watchpoint que só usa hook mente por omissão — foi
  preciso instrumentar o `write_mem` do backend também.
- **Watchpoint de uma palavra só perde escritas.** Um `stm`/`strd` dispara o hook com o endereço
  *inicial* do bloco; se ele começa antes da palavra vigiada, a escrita não aparece. Vigiar uma
  faixa larga em volta foi o que revelou quem zerava o campo.
- **Uma ferramenta que trunca a saída mente.** O relatório do `--watch` mostrava só as últimas
  40 escritas, e as do construtor são as primeiras — o que levou à conclusão errada de que um
  campo "nunca era escrito" quando ele era, logo no começo. Ferramenta de diagnóstico não corta
  resultado sem dizer que cortou.

- **O relógio do guest tem de ser virtual.** O jogo pede um timer de 33 ms esperando um quadro;
  se o tempo vier do relógio do host, o ritmo passa a depender de quão rápido o emulador
  interpreta as instruções, e duas execuções nunca dão o mesmo resultado. Com o relógio
  avançando um passo por quadro, a falha do Bejeweled Twist é idêntica com 1, 10, 33 ou 100 ms
  por quadro — o que já prova que não é problema de tempo.
- **Timer do BREW é de um disparo só.** Quem quer periodicidade rearma dentro do próprio
  callback, e é assim que o jogo monta o laço dele. Por isso o timer vencido sai da lista
  *antes* de rodar: senão o rearmado dispararia de novo no mesmo quadro.
- **`IDISPLAY_GetDeviceBitmap` devolve uma referência.** O jogo dá `Release` na superfície da
  tela uma vez por quadro; sem o `AddRef` correspondente a contagem chegava a zero.

- **O diretório do `.mod` é o item ID da loja do BREW.** `mod/277083/bjt.mod` — 277083 é o
  número que o `ISHELL_GetClassItemID` tem de devolver. Enquanto ele devolvia zero, o Bejeweled
  Twist abortava a inicialização logo depois de consultar a licença.
- **`ILicense` não é proteção contra cópia.** É consulta: o jogo pergunta o tipo de licença para
  decidir se é demo, se conta usos ou se expira em data. `LT_NONE` + `PT_PURCHASE` é a resposta
  que o console dá para um jogo comprado — e a resposta certa para uma ROM legitimamente
  extraída.

- **`ISound` não toca arquivos.** É o gerador de tons, a vibração e o volume do aparelho; quem
  toca áudio é o `ISoundPlayer` (`AEECLSID_SOUNDPLAYER`). Os dois jogos que pediam
  `AEECLSID_SOUND` nem chegaram a chamar um método — bastava a classe existir.
- **`ISound` responde por callback, não por retorno.** Quase toda a vtable devolve `void`:
  `GetVolume` entrega o valor pelo `PFNSOUNDSTATUS` com `AEE_SOUND_VOLUME_CB`, e o fim da
  reprodução chega como `AEE_SOUND_PLAY_DONE`.

- **Um `IDIB` *é* um `IBitmap`.** A struct de `inc/AEEIDIB.h` começa com `AEEVTBL(IBitmap)*` e
  só acrescenta campos públicos, então `QueryInterface(AEECLSID_DIB)` pode devolver o próprio
  objeto — não é preciso um segundo objeto nem uma segunda vtable.
- **O jogo lê `nColorScheme`, não `nDepth`.** Desmontando o `bjt.mod` em `0x6ab28`:
  `ldrb r0, [r0, #0x1d]` seguido de comparações com `0x10`, `0x12` e `0x18` — os valores de
  `IDIB_COLORSCHEME_565`, `_666` e `_888`. Qualquer outra coisa vira `0xff` e aborta a
  inicialização. Ou seja: preencher a profundidade em bits não basta, o esquema de cor precisa
  estar certo.

- **O módulo não pode ser carregado no endereço 0.** O stub que o `elf2mod` coloca no início
  calcula a própria base por aritmética relativa ao PC e depois lê duas palavras *antes* dela
  (`ldr r0, [r1, #-8]` e `[r1, #-4]`). O carregador do AEE reserva um prefixo antes da imagem.
- **Módulos dinâmicos não linkam contra libc.** Eles chamam a stdlib do BREW por uma tabela de
  ponteiros de função entregue pelo carregador — e é justamente o ponteiro dessa tabela que vai
  nas duas palavras do prefixo. O padrão `ldr r1, [r0, #offset]` + `bx r1` aparece idêntico em
  `helloworld.mod` (do SDK) e em `tectoy.mod` (do console).
- **O slot 26 da tabela de helpers (offset 0x68) é `MALLOC`.** Não foi palpite: `AEEModGen.c` do
  BREW SDK 4.0.2 mostra `AEEStaticMod_New` chamando `MALLOC(nSize + sizeof(IModuleVtbl))`, com
  `nSize = sizeof(AEEMod)` = 20 e a vtable de `IModule` = 16 — os 0x24 exatos que o guest pede.
  O mesmo arquivo confirma que o segundo argumento de `AEEMod_Load` é `AEEHelperFuncs *`.

## Formato do `.mif` (levantado nesta sessão)

Não é público. O que está em `src/miffile.rs` saiu da comparação de 24 arquivos reais. O
registro de applet tem 20 bytes e começa com o `AEECLSID`; em todo `.mif` de applet existe
exatamente uma seção desse tamanho. A regra foi validada contra duas fontes independentes:
`mediaplayer.mif` devolve `0x01010EF6`, o mesmo ClassID que a engenharia reversa da firmware
registrou para o Media Player, e o Z-Wheel devolve `0x01070798`, o App ID que a firmware chama
de "TECTOY".

## Aprendizados desta rodada

- **`DBGPRINTF` é o slot 39 da tabela de helpers** (offset 0x9C). Não precisou de desmontagem:
  o Z-Wheel passa em `r0` a string `"*dbgprintf-%d* %s:%d"` e em `r2` o caminho de um arquivo
  `.c` dele. Implementá-lo abriu o log interno dos jogos — o Bejeweled Twist anuncia
  "CREATING GAME APPLET... ...SUCCESS!" com os caminhos originais do build da desenvolvedora.
- **A ordem dos slots gerada dos headers do SDK bate com a firmware.** A tabela de `IFileMgr`
  extraída das macros `INHERIT_*` é idêntica à vtable que o tripleoxygen dumpou do console.
- **O orçamento de instruções não segura laço que chama API.** Ele é reiniciado a cada chamada
  atendida, então um jogo que repete uma chamada com erro roda para sempre. Daí o teto global.

## A vtable de `IHIDDevice` não estava errada — o bug era nosso

Suspeitávamos que a firmware do console tivesse um método a mais do que os headers do SDK, porque
o Bejeweled Twist chamava o "slot 7 de `IHIDDevice`" passando `(0x0106c3fd, ponteiro, 2)`, que
não casava com `GetNumberOfButtons(pnButtons)`.

Duas descobertas desfizeram a suspeita:

1. `0x0106c3fd` é `AEEUID_HID_Joystick_Device` (em `AEEHIDDevice_Joystick.h`) — um **tipo de
   dispositivo**, não um botão. Com isso os argumentos passam a descrever perfeitamente
   `IHID::GetConnectedDevices(nDeviceType, pnHandles, nLen, pnLenReq)`, que é o slot 7 **de
   `IHID`**.
2. O objeto era mesmo um `IHID`. Quem estava errado era o emulador: `build_vtables` montava as
   vtables numa lista escrita à mão, enquanto `vtable_addr` endereçava pelo valor do enum. Ao
   acrescentar `IBitmap` o enum ganhou o valor 7 mas a lista não, e a partir dali tudo
   escorregou — o objeto `IHID` recebeu a vtable de `IHIDDevice`.

A lista agora vive num lugar só (`Interface::ALL`), com teste que confere índice contra valor do
enum e outro que confere que cada vtable aponta para os próprios trampolins. **Os headers do SDK
descrevem o console corretamente; a ordem dos slots está certa.**

## `AEECLSID_HID` = `0x0106c411` (identificado)

Era o ClassID que fazia o Bejeweled Twist imprimir "INITIALIZATION FAILED!". A cadeia de
evidências, toda em material que já tínhamos:

- o valor aparece três vezes dentro da `IHID.dll` do SDK do Zeebo — a DLL cujo instalador só
  continha a extensão de HID, e cujas strings são `"Failed to open HID device"`,
  `\builds\p4_depot\...\Hid\OEMHID.c` e `fs:/sys/hid_devices.cfg`;
- o `conftest` do SDK chama `ISHELL_CreateInstance(shell, AEECLSID_HID, &pIHID)` no
  `GamepadMgr.c`, e o `conftest.elf` contém a mesma constante;
- `AEEIID_IHID` é `0x0106c38d` e os UIDs de botão do `hid_devices.cfg` são `0x0106c40a` e
  vizinhos — mesma faixa.

Ou seja: o jogo não inicializava porque não conseguia criar o gerenciador de gamepad.

## `IDISPLAY_GetDeviceBitmap` devolve código de erro, não ponteiro

O Bejeweled Twist criava o `IGraphics`, pegava a superfície da tela e desistia sem chamar mais
nada. A resposta veio de desmontar o trecho do `bjt.mod` entre uma chamada e outra (o
rastreamento passou a registrar o endereço de retorno, o que dá o ponto exato):

```asm
0x678f0: cmp r0, #0
0x678f4: bne 0x67934      ; se != SUCCESS, falha
0x678f8: add r1, r4, #0x24 ; ponteiro de saída
0x678fc: ldr r0, [r5, #20]
0x67900: bl  0x13ba0       ; embrulho que faz tail call em GetDeviceBitmap
0x67904: cmp r0, #0
0x67908: bne 0x67934      ; <<< retorno diferente de zero = falha
```

Ou seja, o jogo trata o retorno como **código de erro**. E está certo: em `inc/AEEIDisplay.h`,
`int (*GetDeviceBitmap)(iname *po, IBitmap **ppIBitmap)` — devolve erro e entrega a superfície
pelo ponteiro de saída. Eu tinha implementado como se devolvesse o `IBitmap *` direto, igual ao
`GetDestination` (que aí sim devolve o ponteiro). Um ponteiro válido virava "erro diferente de
zero".

## Panorama das ROMs de teste

Esta seção trazia uma tabela de cinco linhas, escrita quando eram cinco as ROMs de teste. Hoje são
61 e o levantamento é automático: veja
[docs/implementacao/11-compatibilidade.md](docs/implementacao/11-compatibilidade.md), que traz o
estado de cada uma e o comando para regerar o placar.

## Sobre desmontar a firmware

A tentativa de localizar a vtable de `IHIDDevice` no `1.1.2_APPS.bin` não deu certo, e vale
registrar o que se aprendeu para quem tentar de novo:

- A imagem é carregada em `0x10000000` — a distribuição dos ponteiros de código bate exatamente
  com o tamanho do arquivo (22 MB), o que dá `VA = offset + 0x10000000`.
- `AEEIID_IHID` (`0x0106c38d`) e `AEEIID_IHIDDevice` (`0x0106c38e`) **não aparecem em nenhuma
  partição**, então não dá para ancorar a busca pelo IID.
- As strings do módulo de HID estão em `0x659c94` (`"Failed to open HID device"`), mas os
  ponteiros da tabela vizinha apontam para `0x11b2xxxx`, fora da imagem — o módulo parece ser
  carregado em outra região, e a vtable não está armazenada como array contíguo no dump.
- Não temos desmontador de ARM no ambiente, o que torna o passo seguinte caro.

No fim não foi preciso: a resposta veio dos headers do SDK do Zeebo mais um bug nosso.

## A tabela de helpers (resolvida sem desmontar nada)

A intenção era desmontar `1.1.2_APPS.bin` para mapear a tabela. Não foi preciso: ela está
declarada em `struct AEEHelperFuncs`, no `sdk/inc/AEEStdLib.h` do próprio BREW SDK 4.0.2.

O mesmo header define `GET_HELPER() = *((AEEHelperFuncs **)AEEMod_Load - 1)` e
`GET_HELPER_VER()` uma palavra antes disso — exatamente o prefixo de duas palavras que a
engenharia reversa do `tectoy.mod` tinha revelado sessões atrás, agora com nome e propósito.
Corrigimos o carregador: `base - 8` é a versão, `base - 4` é o ponteiro da tabela.

Os dois slots que já conhecíamos bateram na mosca: 26 (`0x68`) é `malloc` e 39 (`0x9C`) é
`dbgprintf`. E o slot 48, que tínhamos chutado como alocador, é `GetAppInstance` — uma função
**sem argumentos**, o que explica por que o site de chamada podia usar `r0` como rascunho para
o ponteiro da função, coisa que tinha nos confundido.

## Detalhes que custaram tempo (e a resposta)

- **`EVT_APP_START` vale 0.** Não está em `AEE.h` como a documentação diz; está em
  `inc/AEEEvent.h`, que o `AEE.h` inclui por caminho relativo.
- **`ALLOC_NO_ZMEM` viaja dentro do tamanho.** `MALLOC` aceita `size | 0x80000000` para pedir
  memória sem zerar, e o `malloc` da libc do SDK sempre usa essa forma. Sem mascarar a flag, o
  emulador tentava alocar dois gigabytes, devolvia nulo, e o jogo concluía — corretamente — que
  estava sem memória. Foi o `--trace` que mostrou: `malloc (r0=0x8000003c)`.
- **`AEEDeviceInfo` tem bitfields no meio.** Preenchemos só os oito `uint16` iniciais, que têm
  posição inequívoca; adivinhar o empacotamento do resto seria pior do que deixar zerado.

## Perguntas em aberto

- **Formato da tabela de relocação do `.mod`** — carregar em base 0 evita o problema hoje, mas
  vai voltar se algum título precisar de outra base. O `elf2mod.exe` está em
  `docs/vendor/sdk-brew/toolset/` e é a fonte da verdade quando chegarmos lá.
- **Campos 0x14..0x3C do cabeçalho BREW** — só temos duas amostras (`tectoy.mod`, `bjt.mod`) e os
  valores estão registrados em `BrewHeader::unknown`. Mais amostras devem revelar o padrão.
- **Acentuação quebrada em alguma tela do Resident Evil 4 e do Double Dragon**, relatada mas ainda
  não reproduzida. O caminho óbvio foi conferido e não é ele: as strings `char` do guest eram
  lidas como UTF-8 e isso destruía acento, mas o conserto **não muda essas telas** — o A/B com o
  comportamento antigo sai idêntico, e nenhum dos dois jogos chama `strtowstr` ou `snprintf` nas
  telas que alcançamos. O menu do RE4 ("Não pode ser escolhida", "MERCENÁRIO") e a abertura do
  Double Dragon ("APERTE O BOTÃO HOME") saem certos nos dois builds. Falta saber **em qual tela**
