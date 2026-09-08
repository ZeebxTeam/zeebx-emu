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

O preço disso está medido, e a medida mudou quando ficou mais fina. A volta pelo núcleo — sair
do `emu_start` e entrar de novo — custa **1,4 µs**, e é o que se paga por chamada de API mesmo
quando o método não faz nada. O que sobra depende do método, e às vezes é muito mais que isso.
Num jogo que faz milhões delas por quadro isso vira o gargalo, como está descrito no fim deste
documento.

## Camadas

```
  interface (egui)  ─┐
  linha de comando  ─┴─▶  Session  ─▶  Machine  ─▶  CpuBackend  ─▶  unicorn (TCG)
                                          │
                                          ├─▶ rasterizador de software (OpenGL ES 1.1)
                                          ├─▶ framebuffer 2D
                                          ├─▶ mixer de áudio ─▶ cpal
                                          ├─▶ VFS ─▶ diretório do módulo
                                          └─▶ heap, objetos, temporizadores
```

O `CpuBackend` isola o núcleo ARM. Hoje a implementação é sobre o unicorn; se a performance
exigir, um backend sobre `dynarmic` entra no lugar sem tocar no resto do código.

A `Machine` é o emulador propriamente dito: o laço, o despacho e a implementação das APIs. Ela
não sabe que existe janela.

A `Session` é um jogo em execução, do arquivo aos quadros. É o que a interface usa, e é onde
ficam o freio de velocidade e a medição.

### Os módulos

Execução:

| | |
|---|---|
| `cpu/mod.rs` | O trait `CpuBackend`: registradores, memória, `run` |
| `cpu/unicorn.rs` | A implementação sobre o unicorn, configurada como ARM1176 |
| `mem.rs` | O mapa de memória do guest, em regiões nomeadas |
| `loader.rs`, `modfile.rs` | Carga do `.mod` e montagem do ambiente |
| `machine.rs` | O laço, o despacho e quase toda a API do BREW |

API do BREW:

| | |
|---|---|
| `aee.rs` | O trampolim: endereço ↔ (interface, slot) |
| `aee_slots.rs` | O nome de cada método, na ordem da vtable |
| `aee_helpers.rs` | A stdlib do BREW: `memcpy`, `malloc`, `sprintf` e companhia |
| `objects.rs`, `heap.rs` | Objetos com contagem de referências, e o heap do guest |
| `cformat.rs` | O `printf` do guest |
| `crypto.rs` | AES e MD5, para o `ICipher1` e o `IHash` |
| `font.rs` | O texto do `IDISPLAY_DrawText`, com a fonte que o jogo empacota |
| `sql.rs` | Os bancos SQLite do `ISQLMgr`, sobre o `rusqlite` |

Saída:

| | |
|---|---|
| `rasterizer.rs` | OpenGL ES 1.1 em software |
| `gles.rs`, `atc.rs`, `paltex.rs` | Estado do GL e as texturas comprimidas |
| `display.rs` | Framebuffer e operações 2D |
| `audio.rs`, `wav.rs` | Mistura e decodificação de som |
| `input.rs`, `bindings.rs`, `gamepads.rs` | Entrada |

Arquivos e recursos:

| | |
|---|---|
| `vfs.rs` | Os caminhos do guest, presos ao diretório do módulo |
| `archive.rs` | Jogos em `.zip`, extraídos para um cache |
| `miffile.rs`, `resfile.rs`, `icon.rs` | `.mif`, `.bar` e ícones |

Fora do emulador:

| | |
|---|---|
| `session.rs` | Um jogo em execução |
| `ui.rs`, `i18n.rs`, `settings.rs`, `library.rs`, `padview.rs` | A interface |
| `main.rs` | Linha de comando e abertura da interface |

## O laço

`Machine::advance` é uma volta. Ela adianta o relógio se o jogo só está esperando, dispara os
temporizadores vencidos e entrega os callbacks e as threads pendentes.

Uma volta não corresponde a um quadro. O jogo pode rodar o laço principal inteiro dentro de um
callback, e cada volta devolve apenas a fatia de instruções que coube; no Zeebo Sports Peteca são
um milhão e meio de voltas para algumas dezenas de quadros. Quem manda no redesenho é o
`eglSwapBuffers` no 3D e o relógio real no 2D.

O emulador roda na mesma linha de execução da interface. O núcleo do unicorn não atravessa
threads, e o `Session::step` já devolve o controle a cada fatia de tempo real, que é o que mantém
a janela viva enquanto o jogo corre. Uma thread separada traria sincronização sem trazer nada em
troca.

## O que o projeto mantém de propósito

O quadro é reproduzível bit a bit: duas execuções iguais desenham os mesmos pixels. Foi isso que
permitiu provar que a paralelização do rasterizador e o despejo por quadro não mudaram nada, com
921.600 bytes comparados e zero diferenças. Um backend de GPU real custaria essa garantia, e é
uma das razões de ele não estar na frente da fila.

O relógio é do guest, não do host. O tempo que o jogo mede vem das instruções executadas, e o
tempo ocioso é adiantado em vez de gasto. Sem isso um jogo que espera em laço gasta a espera de
verdade; com isso, `--seconds=N` significa a mesma coisa em qualquer máquina.

Palpite fica marcado como palpite. Uma API atendida sem documentação entra no conjunto
`assumptions` e sai no relatório. A ordem dos slots do `IHash` é um exemplo: o SDK 4.0.2 não traz
o header, então a ordem é hipótese, e isso está dito no código e na saída do programa.

## Dependências

A decodificação de imagem, o som, a rasterização, o `printf`, o AES, o MD5 e o inflate de
recursos são escritos aqui. As dependências cobrem o que é do host ou o que seria reinventar mal.

O `unicorn-engine` é o núcleo ARM, atrás do `CpuBackend`. O `eframe` com o `egui` é a interface,
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

O `build.rs` existe por um motivo só: linkar a `libatomic`. O `cputlb` do QEMU que o
`unicorn-engine-sys` embute usa atômicos de 128 bits que o x86-64 não gera inline, e o crate não
declara essa dependência.

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

São 241 funções de teste, todas junto do código que testam.

Uma parte verifica algoritmos contra a especificação: os vetores da RFC 1321 no MD5, os do
FIPS-197 no AES. Outra verifica os parsers de formato contra os arquivos reais, o `.mod`, o
`.mif` e o `.bar`.

A terceira parte é a mais útil no dia a dia: cada teste carrega no comentário o defeito que o
gerou. O `%02d` que fazia o Resident Evil 4 procurar `3d_stg02_0.h2z` quando o arquivo é
`3d_stg02_00.h2z`. O ClassID fora da faixa da Qualcomm que impedia o Zenonia de abrir. O
orçamento de instruções que precisa ser conferido antes de somar o bloco, e não depois.

Além disso existe a varredura de compatibilidade, que roda as 62 ROMs e classifica cada uma. O
procedimento está em
[docs/implementacao/11-compatibilidade.md](docs/implementacao/11-compatibilidade.md).

## Limites conhecidos

Os números abaixo são medidos, não estimados, em 25 segundos virtuais de Quake: 49% do tempo em
emulação do ARM e despacho de API, 38% em preenchimento de pixels, 14% em geometria.

O núcleo faz cerca de 110 milhões de instruções por segundo. Um jogo que use um quarto da
capacidade do ARM11 do console já consome 80% do nosso relógio só para executar instrução.

O próximo gargalo é o custo por chamada de API, 1,4 µs de ida e volta pelo núcleo, porque toda
chamada para e reinicia a emulação. Atendê-las dentro de um hook é um redesenho do trampolim, e é
a maior melhoria estrutural que resta. Vale, porém, a lição do perfil de API: antes de atacar o
mecanismo, conferir o que cada método custa por dentro — no Pac-Mania a média de 8 µs por chamada
não vinha do trampolim, vinha de dois métodos que faziam trabalho demais.

O rasterizador não é gargalo hoje. Ele já divide o quadro em faixas paralelas, e mesmo que fosse
instantâneo o Quake ficaria em torno de 52% da velocidade do console.
