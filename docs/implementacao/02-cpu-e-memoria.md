# 02 — CPU e memória

## O núcleo

`CpuBackend` (`cpu/mod.rs`) é a fronteira: ler e escrever registradores e memória, e um
`run(pc, orçamento) -> StopReason`. A implementação é o **Dynarmic** (`cpu/dynarmic.rs`),
configurado para ARMv6K/A32 e Thumb — o núcleo do MSM7201A é um ARM11, e pedir uma arquitetura
compatível evita que o jogo tropece numa instrução que o console tinha.

A interface existe para o resto do emulador não depender do backend concreto. Ela é fina de propósito:
tudo o que passa por ela são registradores, blocos de bytes e um motivo de parada.

### Motivos de parada

| `StopReason` | Como acontece |
|---|---|
| `ApiCall { addr }` | O jogo saltou para a faixa `0xf000_0000` — é uma chamada de API |
| `Returned` | O jogo voltou para o endereço-sentinela que pusemos em `lr` |
| `MemoryFault` | Leitura ou escrita fora do mapa |
| `Exception` | Instrução inválida |
| `Budget` | Acabou o orçamento de instruções |

Nenhum deles derruba o emulador. Uma falha de memória é **informação**: o relatório mostra o
endereço, o `pc`, o `lr`, os registradores e a pilha. Abortar esconderia justamente o que
interessa.

### Contagem de instruções

Vem dos blocos recompilados pelo Dynarmic. É o relógio virtual do emulador: o tempo que o jogo
enxerga vem daqui, e não do host, para que duas execuções iguais deem o mesmo resultado.

### Vazão

Medida, não estimada — a bancada principal é `zeebx bench-dynarmic <jogo>`:

| | |
|---|---|
| Instruções por segundo | varia por jogo e host; medir com a bancada antes de afirmar |
| Custo de entrar no guest | amortizado pelos blocos recompilados |
| Teto de instruções ligado | cobrado pelo backend sem hook por instrução |

Esses números existem para responder a pergunta "o emulador está lento ou o jogo faz muita
conta?" sem investigação. **Nas vezes em que um jogo pareceu travado, o núcleo nunca foi o
culpado** — foi sempre o nosso lado do despacho, e desde então o `--profile` mede também o tempo
real de cada método de API, que é onde a resposta costuma estar.

## O mapa de memória

| Região | Base | Tamanho | Para quê |
|---|---|---|---|
| nulo | `0x0000_0000` | até o módulo | zeros que se leem, sem escrita nem execução |
| extensões | `0x0800_0000` | 16 MB cada, até 8 | os módulos de extensão do pacote |
| módulo | `0x0001_0000` | imagem + 1 MB | o `.mod` carregado, com prefixo de uma página |
| heap | `0x1000_0000` | 64 MB | o que o `MALLOC` do jogo consome |
| pilha | `0x2000_0000` | 1 MB | `sp` começa no topo; a pilha do ARM cresce para baixo |
| objetos | `0x3000_0000` | 64 KB | os objetos que entregamos ao jogo |
| helpers | `0x3100_0000` | 1 KB | a tabela da stdlib do BREW |
| stubs | `0x3200_0000` | 4 KB | código ARM que **nós** escrevemos |
| superfícies | `0x4000_0000` | 8 MB | os pixels, no espaço do guest |
| vtables | `0xe000_0000` | somente leitura | os ponteiros que levam ao trampolim |

Três decisões que não se deduzem olhando a tabela:

**O prefixo antes do módulo.** O stub que o `elf2mod` põe no início calcula a própria base por
aritmética relativa ao PC e depois lê **duas palavras antes dela**. O carregador do AEE reserva
esse prefixo, e nós também — uma página é folga de sobra.

**A página nula se lê.** No console não há proteção de memória, e os endereços baixos são
legíveis: um jogo que lê por um ponteiro nulo recebe algum valor e segue. O Aviãozinho, um port do
Quake feito por fãs, depende disso ao carregar a primeira fase: o `start.bsp` tem uma das 19
texturas faltando, o motor põe no lugar a textura de reserva `r_notexture_mip`, e esse port
nunca a cria. O nome dela é lido do endereço zero, e parar a execução ali deixava o jogo preso no
menu. Aqui a faixa abaixo do módulo devolve zeros. Escrever nela continua sendo falha, e executar
também — a região é marcada sem execução, e um salto para o endereço zero continua aparecendo no
relatório como antes.

**64 MB de heap.** O Quake mede a memória livre antes de carregar os `.pak` e desiste com "Not
enough free memory" se ela for pequena. O console tem 128 MB; o número aqui é escolha nossa, só
precisa ser folgado o bastante para o jogo reconhecer o aparelho.

**As superfícies ficam no espaço do guest.** Porque o `IDIB` entrega ao jogo o ponteiro do buffer
para ele desenhar direto — é assim que os jogos comerciais escrevem na tela. Guardar os pixels só
do nosso lado tornaria isso impossível. As consequências disso estão em
[05-video-2d.md](05-video-2d.md), e não são pequenas.

## O processador roda em modo usuário

Um applet BREW não é privilegiado, e há código que **confere isso e muda de caminho**. Ficava
como defeito nosso enquanto o `CPSR` não era escrito: quem consultasse o modo tomava o ramo errado.

O motor 3D da Superscape que o Kingdom Hearts traz como extensão é o caso, e ele diz na cara
qual é o modo esperado:

```
0x0803aedc  mrs  r4, apsr
0x0803aee0  tst  r4, #0xf
0x0803aee4  beq  #0x803af74        <- modo usuário: pula tudo isso
0x0803aee8  mrc  p15, #0, r1, c2, c0, #0   <- TTBR0, a base da tabela de páginas
0x0803aef8  ldr  r5, [r7, r6, lsl #2]     <- e caminha nela
```

Privilegiado, ele ia caminhar na tabela de páginas da MMU para traduzir um endereço. Aqui não há
MMU, o `TTBR0` vale zero, e ele morria lendo `0x00000400` — uma falha dentro da extensão, num
endereço que não dizia nada sobre a causa. Com o `CPSR` em `0x10`, ele pula o trecho inteiro e o
jogo passa a rodar.

Oito jogos foram conferidos depois da mudança — Quake, Zeeboids, Crash, Peggle, Prey Evil,
Zuma's Revenge, Bejeweled Twist e Zenonia — e nenhum mudou de comportamento. O semihosting, que
o Peggle e o Zuma usam para log, continua chegando: o `SVC` é atendido pelo mesmo gancho de
interrupção, que não depende de modo.

## Stubs: código nosso, executado pelo jogo

Alguns helpers devolvem sempre a mesma coisa. Atender um deles pelo trampolim custa parar e
religar o núcleo ARM — e o `GetAppInstance` sozinho responde por **mais da metade** das chamadas
de API do Quake, porque os jogos do BREW guardam os globais dentro do applet e cada acesso a um
global passa por ele.

Para esses, `loader/mod.rs` escreve algumas instruções ARM em `0x3200_0000` e aponta a tabela para
lá. A chamada nem sai da CPU.

## Heap e objetos

`brew/heap.rs` é um alocador simples: primeiro ajuste na lista de livres e, quando nada serve, um
ponteiro que avança. A contabilidade fica toda no host — nenhum cabeçalho é escrito na memória do
jogo.

**O bloco devolvido se funde com os vizinhos livres, e isso não é refinamento.** Sem fundir, cada
`FREE` vira um buraco isolado e a fragmentação aparece rápido numa sessão longa: o Treino Cerebral
troca de tela centenas de vezes, nenhum dos pedaços volta a formar região grande, e o mega que a
fase seguinte pede não acha onde caber com o ponteiro já em 64 MB. O jogo não confere o nulo que o
`MALLOC` devolve e morre chamando um método em zero. Com a fusão, a mesma partida fica em **um**
mega de uso em vez de sessenta e seis. O bloco que encosta no topo não vai para a lista: o ponteiro
recua e ele volta a ser espaço novo.

Por isso o `used()` não é "onde o ponteiro chegou": o que está na lista de livres já voltou, e
contá-lo faria o `GetRAMFree` mentir para quem pergunta antes de alocar.

**Há memória que o jogo entrega e não devolve, porque quem devolve é a interface.** O
`IMEMASTREAM_Set` passa o buffer para o stream, e é o stream que o libera ao ser destruído ou ao
receber outro. O Action Hero 3D monta bitmaps de 7616 bytes, vários por quadro, põe num stream,
decodifica numa imagem e solta os dois: com o stream esquecendo o buffer, a fase comia 1,9 MB por
segundo, e em pouco mais de três minutos o `MALLOC` devolvia nulo e o jogo copiava para o endereço
zero. O `SetEx` fica de fora, porque ali quem libera é um `pfnFree` do jogo.

No `realloc`, a falta de espaço deixa o bloco antigo intacto e devolve nulo — liberá-lo junto
tirava do jogo os dados que ele ainda tinha. Tamanho zero é `FREE`: o bloco sai e o ponteiro vira
nulo, e o `ERR_REALLOC(0, &p)` deixa `p` nulo como o jogo espera.

O que **nós** alocamos no heap do jogo também tem dono. O buffer de linha do `IPeek`, do tamanho
do arquivo inteiro, sai com o leitor; o `AEEImageInfo` do aviso de imagem é um bloco por imagem,
reaproveitado e devolvido no `Release`, e não um bloco novo a cada aviso.

**O que só cresce do lado de cá também tem teto.** O log do `DBGPRINTF` agrupa repetições por
índice e guarda até 2000 linhas diferentes — uma linha com fps ou tempo é nova a cada chamada e
crescia pela sessão inteira; passando do teto, sai a metade mais antiga. O semihosting guarda até
256 KB. A fila do `GetNextButtonEvent` guarda 64 eventos por porta: o jogo que lê o controle de
outro jeito nunca a esvaziava.

**O endereço de um objeto volta a ser usado, e o estado do antigo não pode passar para o novo.**
O `Release` do `IHash` e das cifras esquece o estado (um hash novo continuava o MD5 do anterior), o
do aparelho de entrada esquece a porta, o do `ISignal` o tira dos sinais de entrada (o toque
dispararia o callback do sinal que nasceu no lugar) e o do `IImage` esquece o `Notify`. A cor
transparente é esquecida quando o objeto **nasce**, e não quando morre: o `Framebuffer` de um
bitmap ainda é consultado depois do `Release` por quem guardou o endereço sem referência, e mexer
nisso é outro trabalho.

`brew/objects.rs` entrega endereços dentro da região de objetos, cada um começando com o ponteiro de
vtable — que é o que um objeto COM é. Ele mantém **contagem de referências**, e é ela que decide
quando o estado associado (um bitmap, um arquivo aberto, uma enumeração) pode ser descartado.

`adopt` existe para o caso inverso: um objeto que o **jogo** criou e nos entrega. O Bejeweled
Twist implementa o próprio `IBitmap`, com a vtable embutida no objeto — e para desenhar nele é
preciso perguntar a ele onde ficam os pixels.
