# 02 — A plataforma: Qualcomm BREW 4.0.2

O Zeebo roda **BREW 4.0.2** com customizações da Zeebo Inc./TecToy. Entender BREW é 90% do
trabalho de um emulador.

## Modelo de execução

BREW é um ambiente **COM-like em C** sobre o AEE (*Application Execution Environment*):

- Toda API é uma interface com **vtable**: `IShell`, `IDisplay`, `IGraphics`, `IBitmap`,
  `IFileMgr`, `IFile`, `ISound`, `ISoundPlayer`, `IMedia`, `IHID`, `IHIDDevice`, `ISignal`,
  `IHeap`, `IDisplay`, `IImage`, `IFont`, `IWeb`, `INetMgr`…
- O aplicativo é uma DLL relocável (`.mod`) cujo **primeiro símbolo do `.text` é `AEEMod_Load()`**.
  O AEE carrega o módulo, chama `AEEMod_Load`, que registra o `CreateInstance` do módulo.
- Objetos nascem por `ISHELL_CreateInstance(shell, ClassID, &obj)` — o ClassID é um número de 32
  bits (ex.: Media Player = `0x01010EF6`). O `.bid` do projeto declara os ClassIDs próprios.
- **Não há `main()` nem loop próprio**: o app é *event-driven*. `HandleEvent(app, eCode, wParam, dwParam)`
  recebe `EVT_APP_START`, `EVT_APP_STOP`, `EVT_APP_SUSPEND/RESUME`, `EVT_KEY`, `EVT_USER`…
  Trabalho contínuo é feito re-agendando callbacks (`ISHELL_SetTimer`, `AEECallback`) — o app
  **devolve o controle ao AEE** a cada frame. Isso é ótimo para o emulador: o "frame loop"
  fica no host.
- Sem libc com I/O ou heap: alocação é `MALLOC`/`FREE` do AEE, arquivo é `IFileMgr`/`IFile`,
  não há `stdio`. Construtores globais C++ não rodam automaticamente.
- Cooperativo/single-threaded no núcleo do app; a documentação (cap. 4.5 do Developer Guide)
  desencoraja multitasking real.

Consequência prática: **um emulador HLE só precisa de um interpretador ARM + implementação das
vtables**. Não há acesso direto a registradores de hardware pelo jogo — se houver, é exceção.

## O que é específico do Zeebo

Do Developer Guide (caps. 6 e 7):

- **`IHID` / `IHIDDevice` + `ISignal`**: gamepads USB. Eventos de botão, eixos, conexão/desconexão,
  rumble, acesso exclusivo, remapeamento. Dois controles simultâneos.
- **OpenGL ES 1.0+ Common Profile via extensão BREW** (`OpenGLES_Extension_1.5.3_For_BREW_SDK_4.x.x`):
  o app cria contexto EGL sobre um `IDisplay`/`IBitmap` do BREW e daí em diante usa `gl*` padrão.
  Extensões relevantes: ATITC, matrix palette, point sprites, `glDrawTexture`.
- **Fixed-point math**: boa parte dos jogos usa `GLfixed` — a implementação precisa ser correta
  em ponto fixo, não só converter para float.
- Áudio: MIDI/wavetable, MP3, PCM, ADPCM, CMX, QCELP via `ISound`/`ISoundPlayer`/`IMedia`.
- Tela: 640×480 RGB565, saída composta (o TV encoder está documentado em `research/tvenc/`).

## Documentação de API disponível localmente

| Arquivo | Para que serve |
|---|---|
| `vendor/tripleoxygen/doc/BREWOemAPIReferenceforMSM.pdf` | **Referência de OEM API** — a mais próxima de uma spec das interfaces |
| `vendor/tripleoxygen/doc/ZeeboDeveloperGuide0.97.pdf` | Guia oficial do Zeebo (IHID, GL ES, memória, eventos) |
| `vendor/tripleoxygen/doc/503611776Inside_BREW.rar` | Livro "Inside BREW" |
| `vendor/tripleoxygen/doc/11715848BREWProgrammingguide.rar` | BREW Programming Guide |
| `vendor/tripleoxygen/doc/BREW_and_OpenGLES-Astle.ppt`, `Tech-303_Ligon.pdf`, `TECH-606.pdf` | Palestras BREW/OpenGL ES |
| `vendor/tripleoxygen/doc/Consideracoes_sobre_o_QDSP5000_e_o_BREW_(v0.1).pdf` | QDSP5000 + BREW (áudio), em português |
| `vendor/sdk/samples/unpacked/MSM7500_OGLES_*` | Amostras oficiais em C, com `.mif`/`.bid`/`.mak` |
| `vendor/tripleoxygen/brew/OpenGL_ES_extension_1.0.3.zip` | Headers da extensão GL ES para BREW |
| `vendor/repos/zeebo_doom/src` | Port real (PrBoom) — mostra o subconjunto de API que um jogo usa de fato |
| `vendor/repos/openzeebo/brew/{intro,backup}` | Apps BREW mínimos completos (`.c`, `.mif`, `.bar`, `.sig`, `.bid`) |
| `vendor/sdk/zeebo-sdk-named/` | **Extensão HID do Zeebo**: `AEEIHID.h`, `AEEIHIDDevice.h`, `AEEHIDButtons.{c,h}` (Tectoy), `AEEHIDThumbsticks.{c,h}` |
| `vendor/sdk/qxengine-named/` | Headers e libs do **QXEngine** + `AEEModGen.c`/`AEEAppGen.c` originais |
| **`vendor/sdk-brew/brew-402/BREW 4.0.2 SP19/sdk/inc/`** | **Headers do BREW SDK 4.0.2 — a versão exata do Zeebo.** É a fonte canônica das vtables |
| `vendor/sdk-brew/brew-402/…/documentation/API Reference/` | Referência de API em HTML |
| `vendor/sdk-brew/toolset/…/elf2mod.exe` + `elf2mod.x` | Gerador de `.mod` e o linker script oficial |

### O que os headers extraídos entregam

- `AEEIHID.h` (340 linhas) e `AEEIHIDDevice.h` (1010 linhas) são a **API de entrada do Zeebo
  na íntegra** — enumeração de dispositivos, eventos de botão/eixo, rumble, acesso exclusivo.
  Dá para escrever a vtable de `IHID`/`IHIDDevice` slot a slot direto daí.
- `AEEModGen.c`/`AEEAppGen.c` mostram exatamente o que o AEE espera de um módulo: `AEEMod_Load()`,
  `IModule` (`CreateInstance`, `FreeResources`), `AEEClsCreateInstance()`, e o `AEEApplet` base
  com `HandleEvent`. É o contrato que o loader do emulador precisa honrar.
- O QXEngine vem como **`.a` ARM** — desmontável, e mostra quais APIs BREW a engine consome.

## Vtables da firmware real (achado importante)

`vendor/tripleoxygen/research/brew/IFILEMGR_VTBL_Z200.txt` e `vtbl.ods` contêm o **dump da vtable
de `IFileMgr`** extraída da firmware do Zeebo (Z200), mapeando cada slot para o endereço da
função na imagem APPS e para o protótipo da API:

```
offset 0x00 -> 0x101e66ad  IFILEMGR_AddRef
offset 0x08 -> 0x10d4c26f  IFILEMGR_OpenFile
offset 0x50 -> 0x105cc5c1  IFILEMGR_GetFreeSpaceEx
...
```

Isso é o gabarito para (a) validar a ordem dos slots de cada interface na sua reimplementação e
(b) desmontar a implementação original em `dump/nand/1.1.2/partitions/1.1.2_APPS.bin` quando o
comportamento de uma API for ambíguo. A **ordem dos slots é ABI**: errar um slot quebra tudo.
