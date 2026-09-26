# Arquitetura

Como o Zeebx é feito por dentro. Cada subsistema tem um documento próprio em
[`docs/implementacao/`](docs/implementacao/README.md); aqui está o desenho geral.

## O ponto de partida

Emular Zeebo é reimplementar o Qualcomm BREW 4.0.2. O jogo é um binário ARM que nunca toca
hardware: ele conversa com o sistema por tabelas de ponteiros de função, o `IShell`, o
`IDisplay`, o `IFileMgr`, o `IHID`, o OpenGL ES. Não existe registrador de vídeo para escrever,
nem interrupção de timer para atender, nem firmware do console envolvida em momento nenhum.

Por isso a emulação é de alto nível. O código ARM do jogo roda num núcleo emulado e as chamadas
de API são interceptadas e atendidas no host. O que precisa ser fiel é a conversa, não o
silício.

## O trampolim

As vtables que entregamos ao jogo apontam para uma faixa de endereços que não é mapeada. Quando
o jogo chama um método, o núcleo aborta a busca de instrução e nós lemos o endereço: ele codifica
qual interface e qual método foram pedidos.

```
  jogo (ARM)                          emulador (Rust)
      │ ldr r12, [vtable + 0x30]
      │ blx r12
      ▼
  0xf000_7038  ── não mapeado ──▶  FETCH_UNMAPPED
                                        │  aee::decode → (Bitmap, slot 14)
                                        │  Machine::dispatch
                                        ▼  r0 = resultado, pc = lr, continua
```

Não há código ARM de cola, stub compilado nem tabela de saltos dentro do guest. O endereço é o
identificador da chamada. O retorno funciona igual: `lr` recebe um endereço-sentinela também não
mapeado, e chegar nele significa que o módulo retornou.

O preço disso está medido, **mas a medida é da era do Unicorn**. A volta pelo núcleo — sair do
JIT e entrar de novo — custava **1,4 µs** por chamada mesmo quando o método não fazia nada. Com o
Dynarmic esse número **não foi refeito**: o instrumento de hoje (`ZEEBX_ROM_PERFIL`) cronometra
só o corpo do método, e não o `halt` mais a reentrada, que é onde mora o custo fixo. Enquanto o
micro-teste não existir, o custo por chamada é desconhecido, e não 1,4 µs.

O que sobra depende do método, e às vezes é muito mais que isso: no Pac-Mania a média de 8 µs por
chamada não vinha do trampolim, vinha de dois métodos que faziam trabalho demais. Ver
[`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md).

## Camadas

```
  interface (egui)        ─┐
  linha de comando        ─┤
  frontends/android       ─┼─▶  Session  ─▶  Machine  ─▶  CpuBackend  ─▶  dynarmic (A32)
  frontends/libretro      ─┘                                            └─▶ interpretador (wasm32)
  frontends/headless      ─┘
                                              │
                                              ├─▶ rasterizador de software (OpenGL ES 1.1)
                                              ├─▶ framebuffer 2D
                                              ├─▶ mixer de áudio ─▶ cpal
                                              ├─▶ VFS ─▶ diretório do módulo
                                              └─▶ heap, objetos, temporizadores
```

O `CpuBackend` isola o núcleo ARM. No nativo a implementação é o **Dynarmic** (A32, ARMv6K), e
no `wasm32` é o interpretador: o Dynarmic não compila para WebAssembly. **O Unicorn foi
removido**; a troca de backend que este parágrafo previa aconteceu, e sem tocar no resto do
código — que era exatamente o que o trait existia para garantir.

A `Machine` é o emulador propriamente dito: o laço, o despacho e a implementação das APIs. Ela
não sabe que existe janela.

A `Session` é um jogo em execução, do arquivo aos quadros. É o que a interface usa, e é onde
ficam o freio de velocidade e a medição.

Os frontends são pacotes próprios ao lado do núcleo, cada um com a sua janela e o seu laço de
eventos: [`frontends/android`](frontends/android/LEIAME.md), sobre a `NativeActivity`,
[`frontends/ios`](frontends/ios/LEIAME.md), sobre a UIKit, e
[`frontends/headless`](frontends/headless/LEIAME.md), sem interface nenhuma, para quem já tem um
frontend seu e quer só a emulação por linha de comando com um `config.ini`. Nenhuma linha de
emulação sabe em qual deles está rodando.

### Os módulos

A árvore segue os subsistemas: uma pasta por assunto, e dentro dela um arquivo por peça.

Execução:

| | |
|---|---|
| `cpu/mod.rs` | O trait `CpuBackend`: registradores, memória, `run` |
| `cpu/dynarmic.rs` | A implementação sobre o Dynarmic, com tabela de páginas direta |
| `cpu/interpretador.rs` | O backend do `wasm32`, onde o Dynarmic não existe |
| `cpu/mem.rs` | O mapa de memória do guest, em regiões nomeadas |
| `loader/` | Carga do `.mod` e montagem do ambiente, mais os formatos `.mif`, `.bar` e `.zip` |
| `machine/mod.rs` | O laço, o despacho e o estado da máquina |
| `machine/*.rs` | Um submódulo por interface do BREW — ver [`docs/implementacao/01-arquitetura.md`](docs/implementacao/01-arquitetura.md) |

API do BREW (`brew/`):

| | |
|---|---|
| `brew/aee.rs` | O trampolim: endereço ↔ (interface, slot) |
| `brew/aee_slots.rs` | O nome de cada método, na ordem da vtable |
| `brew/aee_helpers.rs` | A stdlib do BREW: `memcpy`, `malloc`, `sprintf` e companhia |
| `brew/objects.rs`, `brew/heap.rs` | Objetos com contagem de referências, e o heap do guest |
| `brew/cformat.rs`, `brew/fmath.rs` | O `printf` e o ponto flutuante do guest |
| `brew/crypto.rs` | AES e MD5, para o `ICipher1` e o `IHash` |
| `brew/sql.rs` | Os bancos SQLite do `ISQLMgr`, sobre o `rusqlite` |
| `brew/vfs.rs` | Os caminhos do guest, presos ao diretório do módulo |

Saída:

| | |
|---|---|
| `video/rasterizer.rs` | OpenGL ES 1.1 em software |
| `video/gles.rs`, `video/atc.rs`, `video/paltex.rs` | Estado do GL e as texturas comprimidas |
| `video/display.rs` | Framebuffer e operações 2D |
| `video/font.rs` | O texto do `IDISPLAY_DrawText`, com a fonte que o jogo empacota |
| `video/icon.rs`, `video/gif.rs` | As imagens que vêm dentro dos jogos |
| `audio/` | Mistura de som, e os formatos WAV, MP3 e MIDI |
| `input/` | O controle do Zeebo, os gamepads do host e o mapa de botões |

Fora do emulador:

| | |
|---|---|
| `session.rs` | Um jogo em execução |
| `ui/` | A interface: tela, biblioteca, preferências, saves e tradução |
| `main.rs` | Linha de comando e abertura da interface |

## O laço

`Machine::advance` é uma volta. Ela adianta o relógio se o jogo só está esperando, dispara os
temporizadores vencidos e entrega os callbacks e as threads pendentes.

Uma volta não corresponde a um quadro. O jogo pode rodar o laço principal inteiro dentro de um
callback, e cada volta devolve apenas a fatia de instruções que coube; no Zeebo Sports Peteca são
um milhão e meio de voltas para algumas dezenas de quadros. Quem manda no redesenho é o
`eglSwapBuffers` no 3D e o relógio real no 2D.

O emulador roda na mesma linha de execução da interface. O Dynarmic não atravessa threads — o
`Jit` é preso à linha em que nasceu —, e o `Session::step` já devolve o controle a cada fatia de
tempo real, que é o que mantém a janela viva enquanto o jogo corre. Uma thread separada traria
sincronização sem trazer nada em troca. É a mesma razão para o core Libretro entregar um quadro
por `retro_run`, e para o registro do núcleo ser drenado ali e não de dentro do desenho.

## O que o projeto mantém de propósito

O quadro é reproduzível bit a bit: duas execuções iguais desenham os mesmos pixels. Foi isso que
permitiu provar que a paralelização do rasterizador e o despejo por quadro não mudaram nada, com
921.600 bytes comparados e zero diferenças. Um backend de GPU real custaria essa garantia, e é
uma das razões de ele não estar na frente da fila.

**Uma ressalva, medida em 2026-09-24: a reprodução exige o mesmo estado de cache de extração.**
Três execuções com a extração já no cache dão **760 815 instruções, dígito por dígito**; a mesma
ROM com o cache recém-extraído dá **760 921**, com uma `CreateInstance` a mais e o heap deslocado 64
bytes. O emulador é determinístico — o que muda é o que o guest observa do host, e a causa ainda
não está fechada: a hipótese do manifesto `.zeebx-pacote` foi testada e **rejeitada**. Ver
[`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md), seções 32 e 33. Quem for comparar duas
execuções precisa aquecer o cache antes, ou vai medir isto em vez do que queria.

O relógio é do guest, não do host. O tempo que o jogo mede vem das instruções executadas, e o
tempo ocioso é adiantado em vez de gasto. Sem isso um jogo que espera em laço gasta a espera de
verdade; com isso, `--seconds=N` significa a mesma coisa em qualquer máquina.

Palpite fica marcado como palpite. Uma API atendida sem documentação entra no conjunto
`assumptions` e sai no relatório. A ordem dos slots do `IHash` é um exemplo: o SDK 4.0.2 não traz
o header, então a ordem é hipótese, e isso está dito no código e na saída do programa.

## Dependências

A decodificação de imagem, o som, a rasterização, o `printf`, o AES, o MD5 e o inflate de
recursos são escritos aqui. As dependências cobrem o que é do host ou o que seria reinventar mal.

O `dynarmic` é o núcleo ARM, atrás do `CpuBackend`, e só existe no nativo: o `wasm32` usa o
interpretador de `cpu/interpretador.rs`. O `eframe` com o `egui` é a interface,
e o `minifb` é a janela do modo `--window`, que é separada dela. O `cpal` só põe som na placa; a
decodificação é nossa. O `gilrs` dá controles de verdade nos três sistemas.

O `ab_glyph` rasteriza os glifos do `IDISPLAY_DrawText`. Ele já vinha na árvore por causa do
`eframe`, então usá-lo direto não acrescenta compilação — e escrever um interpretador de TrueType
seria reinventar mal. A fonte em si nunca é nossa: é a que o jogo empacota.

O `rusqlite`, com a build embutida, atende o `AEECLSID_SQLMGR`. Não é conveniência: o console
usava SQLite mesmo — o `tt_prefs.db` que a Z-Wheel traz no pacote começa com `SQLite format 3`, e
as instruções que o módulo carrega em texto usam `INSERT OR REPLACE`, `COLLATE NOCASE` e JOIN.
Escrever um motor para isso seria reinventar mal.

O `flate2` faz o inflate do `IUnzipAStream` do BREW. O `zip` entra só para leitura e só com
`deflate`, que é o que os empacotadores usam. O `png`, o `zune-jpeg` e o `resvg` cobrem os
formatos que os `.mif` e os recursos trazem. O `rfd` é só o seletor de pastas, por `xdg-portal`
para não depender do GTK no Linux. O `serde` e o `serde_json` guardam a configuração.

O `build.rs` da raiz existia por um motivo só: linkar a `libatomic` que o `unicorn-engine-sys`
precisava, e esse motivo **saiu da árvore com o Unicorn**. O que sobrou dele é o
`cargo:rerun-if-changed` e o registro de que o ícone do `.exe` passou a ser compilado pelo
`frontends/classical-standalone`, que é quem produz o executável. Quem tem um `build.rs` com
trabalho de verdade hoje é o core Libretro, e por outro motivo: o shim de compatibilidade com
glibc anterior a 2.34
([`frontends/libretro/src/r36s_compat.c`](frontends/libretro/src/r36s_compat.c)), que define
`__libc_single_threaded` e o par `__dso_handle`/`__cxa_finalize` como símbolos fracos.

## Ferramentas de depuração

Quase todo defeito difícil deste projeto foi achado por uma delas, e não pela leitura do código.

O `--trace[=trecho]` mostra cada chamada de API com argumentos e retorno. O `--watch=ENDEREÇO`
diz quem escreveu e quem leu um campo, que é como se responde "quem deveria ter preenchido isto?".
O `--code=INI:FIM` lista as instruções executadas numa faixa.

O `--profile` mostra onde o tempo é gasto pelos **dois lados**: no guest, por bloco de tradução
em vez de por instrução, para não alterar o que está sendo medido; e no host, o tempo real de
cada método de API que o emulador atendeu. O segundo existe porque a média mente — "8 µs por
chamada de API" escondia que o `IIMAGE_Draw` e o `IIMAGE_SetParm` do Pac-Mania sozinhos eram 99%
do despacho dele. O `--wall=SEGUNDOS` interrompe por tempo real,
inclusive de dentro de uma fatia, e é o que torna possível perfilar um jogo que nunca termina.

Fora do emulador há o `ferramentas/desmonta.py`, que desmonta um trecho do módulo do jogo
resolvendo os literais e as strings. Ele responde a pergunta que nenhuma das ferramentas de dentro
responde — **por que** o jogo desviou —, e a diferença é grande: "o app morre em `0x78b70`" e
"`0x78b70` é o ramo de erro que só imprime a mensagem" são indistinguíveis sem ele.

O `--sonda=0xCLSID` atende uma classe que não conhecemos com um objeto de observação, em vez de
recusá-la, e diz no fim que slots o jogo chamou, com que argumentos e com que texto. É como se
descobre que interface é uma classe sem ter o header — o `ISQLMgr` do console saiu daí inteiro,
numa execução. O procedimento está em
[docs/implementacao/13-classes-desconhecidas.md](docs/implementacao/13-classes-desconhecidas.md).

O `--keys=ms:tecla` roteiriza a entrada em tempo virtual, de modo que a mesma sequência acontece
em toda execução. O `--dump-gl=DIR` grava um `.bmp` por quadro apresentado. O `--seconds` e o
`--frames` limitam quanto rodar, em tempo virtual ou em voltas do laço.

O instrumento principal, porém, é o relatório que o `run` imprime no fim: onde parou, com
registradores e pilha; as classes que o jogo pediu e não temos; os arquivos que ele não achou; as
APIs atendidas por hipótese; e o log do próprio jogo, que sai por `DBGPRINTF` e por semihosting
do ARM.

## Testes

São 308 funções de teste, todas junto do código que testam. Os comandos estão em
[docs/implementacao/17-testes.md](docs/implementacao/17-testes.md).

Uma parte verifica algoritmos contra a especificação: os vetores da RFC 1321 no MD5, os do
FIPS-197 no AES. Outra verifica os parsers de formato contra os arquivos reais, o `.mod`, o
`.mif` e o `.bar`.

A terceira parte é a mais útil no dia a dia: cada teste carrega no comentário o defeito que o
gerou. O `%02d` que fazia o Resident Evil 4 procurar `3d_stg02_0.h2z` quando o arquivo é
`3d_stg02_00.h2z`. O ClassID fora da faixa da Qualcomm que impedia o Zenonia de abrir. O
orçamento de instruções que precisa ser conferido antes de somar o bloco, e não depois.

Além disso existe a varredura de compatibilidade, que roda as ROMs de verdade e classifica cada
uma. Ela vive no [`src/varredura.rs`](src/varredura.rs) e é **dirigida por ambiente**: sem
`ZEEBX_ROM` os testes avisam e passam, porque ROM nenhuma está na árvore. Com ele, cada jogo
avança em tempo virtual e sai um relatório — onde parou, quanto custou, que API e que classe
faltaram, que arquivo não foi achado, e os registradores de quem quebrou.

O que torna isso teste, e não relatório, é a **linha de base**: o resumo de cada jogo é comparado
com o que já se sabia dele, e o que mudou aparece como `+`/`-` na falha. Desempenho fica de fora
da comparação de propósito — número de máquina não é comportamento de emulador. O procedimento
está em [docs/implementacao/11-compatibilidade.md](docs/implementacao/11-compatibilidade.md).

## Limites conhecidos

**Os números abaixo têm data, e a data importa.** Tudo o que está marcado como *era do Unicorn*
foi medido antes de o Dynarmic virar o backend, e **não descreve o emulador de hoje**. Quando o
Unicorn saiu da árvore ninguém refez as medidas; refazê-las é o item 1 de
[`docs/OPTIMIZING_V0.3.0.md`](docs/OPTIMIZING_V0.3.0.md), onde vivem os números de agora.

### O que se sabe hoje (Dynarmic)

Medido em bancada x86-64, `--release`, sem o perfil ligado, 15.016 ms virtuais de Quake:

| | |
|---|---|
| velocidade | **170%** da do console |
| ritmo do núcleo | **195 milhões** de instruções por segundo |

Nos dois números, 2,3 vezes o que o Unicorn entregava. **A ressalva está no documento:** a cena
não é a mesma da medida antiga, então o que compara direto é o ritmo do núcleo, não o relógio de
parede de duas cenas diferentes.

O que a rodada longa mostrou, com 63 ROMs e 60 s virtuais cada, **com o perfil ligado** — que
encarece o relógio em cerca de 43% e por isso serve para ordenar, não para publicar: Action Hero
3D 91%, Quake 93%, Ultimate Chess 3D 114%, Need for Speed Carbon 123%. O Pac-Mania não fecha 60 s
nem em 240 s de relógio real: gasta 32 milhões de `GetClipRect` e 16 milhões de `SetClipRect` no
caminho de desenho, e é o candidato mais claro a otimização que a árvore tem hoje.

### O que ficou da era do Unicorn

Está aqui porque ainda é verdade como **lição**, e não como medida atual.

**Meça sempre sem o `--profile`.** Ele custava 24% na medição antiga — 36 s contra 29 s no mesmo
trabalho —, porque o perfil de blocos faz uma inserção de tabela por bloco de tradução. O preço
caía quase todo na fatia do ARM, então com ele ligado a emulação parecia maior do que era. O
mesmo cuidado vale hoje, por outro motivo: o `ZEEBX_ROM_PERFIL` cronometra cada método de API.

**Pedir o bloco, nunca o elemento.** A geometria foi 14% do tempo numa medição antiga e caiu
quando o `read_attribute` deixou de pedir um `read_u32` por componente: cada pedido atravessava a
FFI do Unicorn, que procurava a região antes de copiar quatro bytes — **57 ns**, contra **0,3 ns**
quando os mesmos bytes vinham de um `read_mem` de um quilobyte. Com 6,65 milhões de vértices em
15 segundos, isso eram 3,2 s dos 3,6 s que as draw calls custavam, contra 400 ms do desenho de
fato. A lição continua valendo para todo dado que o guest entrega em array, e é ela que sustenta
a leitura em bloco de hoje.

**O custo por chamada de API é desconhecido, não 1,4 µs.** Aquele número media a volta pelo
`emu_start` do Unicorn. O instrumento atual não mede o `halt` mais a reentrada do JIT: mede o
corpo do método. Enquanto o micro-teste não existir, tratar 1,4 µs como fato é tratar uma medida
de um backend removido como se fosse do atual — e foi exatamente esse erro que a revisão externa
cometeu.

**O rasterizador *é* gargalo no caminho de software.** A frase "não é gargalo hoje" também é da
era do Unicorn. Com o Dynarmic, a fatia do ARM encolheu e a apresentação passou a dominar: nos
cinco jogos perfilados ela ficou entre 54% e 85% do tempo de método de API, e o `eglSwapBuffers`
carrega rasterização, conversão para RGB565 e cópia, porque é ali que a fila de triângulos vira
pixel (`frame_rgb565` chama `flush`). No caminho de placa, quem manda é outro conjunto de custos
— e é o que a telemetria no portátil ainda precisa mostrar.

### O que ninguém mediu ainda

- o custo real de uma chamada de API com o Dynarmic, trampolim incluído;
- quanto do tempo do portátil é ARM, quanto é rasterização e quanto é `glReadPixels`;
- quantas páginas de código+código se misturam e fazem o SMC pesar.

O primeiro número sai de um micro-teste; os outros dois, da telemetria por quadro no core.
