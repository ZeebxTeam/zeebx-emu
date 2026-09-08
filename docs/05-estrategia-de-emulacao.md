# 05 — Estratégia de emulação proposta

## HLE, não LLE

| | LLE (rodar a firmware) | **HLE (reimplementar BREW)** |
|---|---|---|
| Precisa da NAND/AMSS do console | sim (problema legal e de distribuição) | não |
| Precisa emular MSM7201A, Adreno 130, QDSP-5, MDP, modem | sim | não |
| Precisa emular o ARM11 | sim | sim (só o código do jogo) |
| Esforço | anos | meses até "algo roda" |
| Precisão | alta | depende da fidelidade das APIs |

Vamos de **HLE**. O jogo só fala com vtables BREW; abaixo delas somos nós. Os dumps de NAND
servem como referência de desassembly quando o comportamento de uma API for ambíguo, não como
dependência de runtime.

## Blocos do emulador

```
      .mod / .mif / .sig  em roms/<Título>/
                │
       ┌────────▼─────────┐
       │  Module loader   │  parse do header BREW, relocação, mapa de memória do módulo
       └────────┬─────────┘
                │  entry = AEEMod_Load
       ┌────────▼─────────┐        ┌────────────────────────┐
       │  Núcleo ARM      │◄──────►│  Memória guest (flat)   │  heap, stack, .text/.data do módulo
       │  (ARMv6/ARM11)   │        └────────────────────────┘
       └────────┬─────────┘
                │  chamada a ponteiro de vtable em endereço "mágico"
       ┌────────▼───────────────────────────────────────────┐
       │  AEE / camada de HLE: despacho de interface+slot     │
       │  IShell IDisplay IGraphics IBitmap IFileMgr IFile   │
       │  ISound ISoundPlayer IMedia IHID ISignal IHeap ...  │
       │  + extensão OpenGL ES 1.x / EGL                     │
       └────────┬───────────────┬───────────────┬────────────┘
                │               │               │
          vídeo (GL/…)     áudio (mixer)    input (SDL gamepad)
                │
        VFS: mapeia caminhos BREW ("fs:/~/…") para roms/<Título>/mod/<id>/
```

### 1. Núcleo ARM
Alvo: **ARMv6 (ARM1136), ARM + Thumb, sem NEON, com VFP** (checar se os jogos usam VFP ou só
ponto fixo — o Developer Guide empurra ponto fixo).
Opções: começar com **interpretador próprio** (mais fácil de debugar e de instrumentar traces de
chamadas), com `dynarmic` ou `unicorn` como plano B/otimização. O Infuse usa dynarmic.

### 1.5. A tabela de helpers (descoberto na prática)

Além das vtables de objeto, módulos dinâmicos chamam a **stdlib do BREW** por uma tabela de
ponteiros de função que o carregador entrega — `MALLOC`, `STRLEN` e companhia não são métodos de
interface. O ponteiro dessa tabela chega ao módulo por duas palavras gravadas **antes** da imagem
carregada, que o stub do `elf2mod` copia para dentro do módulo. Consequência prática: o módulo
não pode ser carregado no endereço 0, e a tabela de helpers precisa dos mesmos trampolins que as
vtables.

### 2. Trampolim de API — o truque central
Interfaces BREW são structs com ponteiro para vtable. A técnica usual:
alocar as vtables em uma região de endereços **inválida/sentinela** no espaço guest; quando o
core ARM tentar executar um `BLX` para lá, o emulador captura, decodifica *(interface, slot)*
pelo endereço, lê os argumentos de `r0..r3`/pilha pela AAPCS, executa a implementação nativa e
devolve o retorno em `r0`. Toda API nova vira "mais uma entrada na tabela".
A **ordem dos slots é ABI** — usar os headers do SDK BREW (`AEE*.h`) e conferir contra
`vendor/tripleoxygen/research/brew/vtbl.ods`, que tem a vtable real do console.

### 3. Ordem de implementação (do menor caminho até a primeira tela)
1. Loader `.mod` + relocação + `AEEMod_Load` retornando com sucesso.
2. `IShell` mínimo, `MALLOC/FREE/REALLOC`, `IModule`, ciclo `EVT_APP_START`.
3. `IFileMgr`/`IFile` sobre um VFS apontado para o diretório do módulo.
4. `IDisplay`/`IBitmap`/`IGraphics` → framebuffer 640×480 RGB565 numa janela SDL.
5. Timers/callbacks (`ISHELL_SetTimer`, `AEECallback`) e o loop de frame.
6. `IHID`/`ISignal` → gamepad.
7. EGL + OpenGL ES 1.x (via ANGLE ou tradução para GL 2+/gl4es).
8. Áudio: PCM/ADPCM → MP3 → MIDI wavetable (o mais chato; deixar por último).
9. Rede: stubs que falham "graciosamente" (`IWeb`, `INetMgr`) — e, depois, um servidor local
   fingindo a loja da TecToy para o Z-Wheel.

### 4. Estratégia de testes
- **`roms/Z-Wheel`** (App ID 274755) é o shell do console: exercita 2D, fontes, áudio, i18n,
  QXEngine e rede. É um alvo ambicioso para o primeiro boot, mas é um excelente *stress test*.
- Alvo inicial mais fácil: compilar apps BREW próprios com toolchain aberta
  (`arm-none-eabi-gcc` + `elf2mod`, ver `vendor/repos/openzeebo/brew/intro`) — assim você tem
  **fonte + binário** e sabe exatamente qual API foi chamada.
- Segundo alvo: amostras oficiais do SDK (`vendor/sdk/samples/unpacked/`), que cobrem GL ES
  isoladamente (VBO, dot3, point sprites, matrix palette, ATITC…).
- **Trace-first**: antes de implementar, logar toda chamada não implementada com
  `(interface, slot, args)` e deixar o app quebrar. A lista de faltantes vira o backlog.

### 5. Escolhas a definir
- **Linguagem**: C++17/20 ou Rust. C++ facilita reaproveitar dynarmic; Rust facilita não errar
  na aritmética de ponteiros do guest.
- **Frontend**: SDL3 (janela, input, áudio) é o menor caminho para multiplataforma.
- **GL**: ANGLE (GL ES sobre Metal/D3D/Vulkan) resolve macOS, onde GL ES nativo não existe.

## Riscos reais
1. **Áudio MIDI/wavetable** — a tabela de 512 kB é da firmware; reproduzir o timbre exato exige
   extrair o wavetable do dump ou aceitar um soundfont substituto (diferença audível).
2. **QXEngine** é binário fechado; se ele tocar APIs BREW obscuras ou fizer suposições de
   layout de memória, vai doer.
3. **Formato `.mod`** não está documentado publicamente — a tabela de relocação precisa ser
   inferida do `elf2mod` e validada com binários de fonte conhecida.
4. **Camada de rede/loja morta**: vários títulos podem checar ativação/DRM online.
5. Fidelidade de ponto fixo em GL ES (`GLfixed`) — erro sutil aqui vira geometria torta.
