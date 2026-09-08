# 02 — CPU e memória

## O núcleo

`CpuBackend` (`cpu/mod.rs`) é a fronteira: ler e escrever registradores e memória, e um
`run(pc, orçamento) -> StopReason`. A implementação é o **unicorn-engine 2.1.5** configurado como
**ARM1176** (`cpu/unicorn.rs`) — o núcleo do MSM7201A é um ARM11, e pedir o modelo certo evita
que o jogo tropece numa instrução que o console tinha.

A interface existe para o resto do emulador não depender do unicorn. Ela é fina de propósito:
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

Vem de um hook por **bloco de tradução**, não por instrução: é ordens de grandeza mais barato e
dá o mesmo número, porque no ARM toda instrução tem quatro bytes.

### Vazão

Medida, não estimada — `cargo test --release cpu::unicorn::speed -- --ignored --nocapture`:

| | |
|---|---|
| Instruções por segundo | ~200–540 M/s |
| Custo de entrar no guest | ~0,7 µs |
| Teto de instruções ligado (hook por instrução do unicorn) | ~25% mais lento |

Esses números existem para responder a pergunta "o emulador está lento ou o jogo faz muita
conta?" sem investigação. **Nas duas vezes em que um jogo pareceu travado, o núcleo não era o
culpado** — era o nosso lado do despacho.

## O mapa de memória

| Região | Base | Tamanho | Para quê |
|---|---|---|---|
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

**64 MB de heap.** O Quake mede a memória livre antes de carregar os `.pak` e desiste com "Not
enough free memory" se ela for pequena. O console tem 128 MB; o número aqui é escolha nossa, só
precisa ser folgado o bastante para o jogo reconhecer o aparelho.

**As superfícies ficam no espaço do guest.** Porque o `IDIB` entrega ao jogo o ponteiro do buffer
para ele desenhar direto — é assim que os jogos comerciais escrevem na tela. Guardar os pixels só
do nosso lado tornaria isso impossível. As consequências disso estão em
[05-video-2d.md](05-video-2d.md), e não são pequenas.

## Stubs: código nosso, executado pelo jogo

Alguns helpers devolvem sempre a mesma coisa. Atender um deles pelo trampolim custa parar e
religar o núcleo ARM — e o `GetAppInstance` sozinho responde por **mais da metade** das chamadas
de API do Quake, porque os jogos do BREW guardam os globais dentro do applet e cada acesso a um
global passa por ele.

Para esses, `loader.rs` escreve algumas instruções ARM em `0x3200_0000` e aponta a tabela para
lá. A chamada nem sai da CPU.

## Heap e objetos

`heap.rs` é um alocador simples com lista de livres e reuso por tamanho exato. Não compacta:
os jogos alocam blocos grandes e poucos, e a fragmentação nunca apareceu como problema.

`objects.rs` entrega endereços dentro da região de objetos, cada um começando com o ponteiro de
vtable — que é o que um objeto COM é. Ele mantém **contagem de referências**, e é ela que decide
quando o estado associado (um bitmap, um arquivo aberto, uma enumeração) pode ser descartado.

`adopt` existe para o caso inverso: um objeto que o **jogo** criou e nos entrega. O Bejeweled
Twist implementa o próprio `IBitmap`, com a vtable embutida no objeto — e para desenhar nele é
preciso perguntar a ele onde ficam os pixels.
