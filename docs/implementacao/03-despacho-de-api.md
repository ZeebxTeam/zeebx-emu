# 03 — Despacho de API

## O trampolim

Um objeto BREW é um ponteiro para uma struct cuja primeira palavra aponta para a vtable. Chamar
`ISHELL_CreateInstance(shell, …)` vira, em ARM, um load do ponteiro de vtable, um load do slot e
um `blx` para o endereço lido.

Aproveitamos isso: os endereços que colocamos nos slots ficam numa faixa **não mapeada**,
escolhida de forma que o próprio endereço codifique qual método foi chamado.

```
endereço = 0xf000_0000 + (interface << 12) + (slot * 4)
```

O núcleo aborta o fetch, `aee::decode` desfaz a conta, e o despacho sabe exatamente o que
chamar. **Não existe uma linha de código ARM de cola em lugar nenhum** — a falha de busca *é* o
mecanismo de chamada.

Decodificar à mão, olhando um log:

```
0xf000_7038 - 0xf000_0000 = 0x7038
0x7038 >> 12              = 7      -> Interface::Bitmap
(0x7038 & 0xfff) / 4      = 14     -> slot 14 = SetTransparencyColor
```

`aee::describe` faz isso e devolve `"IBitmap::SetTransparencyColor"`, que é o que aparece no
rastreamento e nas mensagens de erro.

## Os slots

`brew/aee_slots.rs` lista o nome de cada método **na ordem em que ocupa a vtable**. A ordem vem das
macros `INHERIT_IXxx` dos headers do SDK — é a única fonte que garante o índice certo, e um
deslocamento de um slot faz o jogo chamar o método errado sem nenhum aviso.

`brew/aee_helpers.rs` faz o mesmo para a stdlib: `struct AEEHelperFuncs`, campo por campo. O índice é
o deslocamento em palavras — `malloc` é o slot 26, offset `0x68`, que é o valor que os módulos
reais pedem.

## `Machine::dispatch`

```rust
dispatch(addr)
  ├─ aee::decode(addr)                    -> (interface, slot)
  ├─ contabiliza a chamada                -> o resumo do fim
  ├─ note_spin(iface, slot)               -> ver 04-tempo.md
  ├─ rastreamento, se ligado
  ├─ dispatch_inner(iface, slot)          -> o trabalho de verdade
  └─ ponteiro ruim vira EBADPARM, não pânico
```

Um ponteiro ruim vindo do jogo **não pode derrubar o emulador**: vira `EBADPARM`, que é o que o
BREW responde, e fica registrado para aparecer no relatório. Um jogo com um bug não é motivo
para o emulador cair.

`dispatch_inner` reparte por interface. Algumas famílias têm função própria (`file_call`,
`gles_call`, `media_call`, `bitmap_call`…), e o braço de `IDisplay`/`IGraphics`/`IBitmap` tem uma
particularidade cara descrita em [05-video-2d.md](05-video-2d.md).

Devolver `None` significa **"ainda não implementado"** — o laço para com `Outcome::Unimplemented`,
que nomeia o método, os argumentos e de onde veio a chamada. É essa mensagem que transforma
"o jogo não abre" em "falta o `IFileMgr::EnumInit`".

## Como um método novo entra

1. Rodar o jogo com `--seconds=6` e ler a linha `parou na volta N em API não implementada`.
2. Achar a assinatura — nesta ordem: header do SDK em `docs/vendor/.../sdk/inc/`, depois a
   referência HTML em `documentation/API Reference/`.
3. Implementar no `*_call` da família, devolvendo o que o BREW devolveria (inclusive o erro).
4. Teste. Se o método tem geometria ou formato, o teste é sobre isso e não sobre a chamada.
5. Rodar o jogo de novo e conferir que ele passou daquele ponto.

O passo 2 não é opcional. **Chutar uma assinatura dá um emulador que funciona por acidente** —
e o acidente termina três jogos depois.

## Quando o BREW não tem resposta boa

Nem todo método precisa fazer o que promete. Vale responder o que é verdade:

- `ISHELL_Prompt` devolve **falso**: "não criei o diálogo". O BREW prevê isso, e o jogo segue
  pelo caminho de quem não pôde perguntar.
- `IBitmap::QueryInterface` de uma classe que não temos devolve `ECLASSNOTSUPPORT`, que é a
  resposta correta — o BREW espera que o app tenha caminho alternativo.
- O que não muda nada (`Backlight`, `SetAnnunciators`) devolve `SUCCESS` e ignora. Recusar faria
  jogos desistirem por causa de um ajuste que não muda o som nem a imagem.

O que **não** vale é inventar dado. Uma classe desconhecida entra em `unknown_classes` e aparece
no relatório; uma suposição entra em `assumptions` e aparece também.

## A classe que não é nossa: módulos de extensão

Há um caso em que a resposta certa não é implementar a classe nem recusá-la: quando ela vem
**dentro do próprio pacote do jogo**, escrita em ARM. É o mecanismo de módulo de extensão do
BREW, e dois títulos que temos dependem dele:

| título | applet | extensão | classe |
|---|---|---|---|
| Action Hero 3D | `mod/274259/a3d.mod` | `mod/12875/imicro3d.mod` | `0x010292c3` |
| Kingdom Hearts | `Kingdon Hearts/kh.mod` | `Kingdon Hearts_/swv21brew.mod` | `0x0102bbfc` |

Os dois apareciam no levantamento como "não roda", e a causa era a mesma: pediam uma classe que
o console não tem em partição nenhuma — porque ela não é do console, é do jogo.

### Como a extensão é reconhecida

O `.mif` guarda um registro de 8 bytes por classe, `<u32 ClassID> <u32 zero>`, e ele aparece
**idêntico nos dois lados da relação**: no manifesto do jogo, declarando a dependência, e no da
extensão, declarando o que fornece. Quem distingue é o arquivo em volta: um `.mif` **sem**
registro de applet não é de um título, é de uma extensão. Sem essa regra o manifesto do jogo se
ofereceria para atender a classe que ele mesmo está pedindo.

A pareação `.mod` ↔ `.mif` é pelo **nome da pasta** do módulo, que é a disposição do BREW —
`<nome>/` com o módulo dentro e `<nome>.mif` ao lado. Ela é mais estreita que a busca que acha o
manifesto do applet, e de propósito: ali, errar dá "manifesto não encontrado"; aqui, daria a
classe atendida pelo módulo errado.

### O que acontece na hora do pedido

As extensões são **mapeadas na carga**, cada uma em `0x0800_0000 + i * 0x0100_0000`, com as duas
palavras de helper antes da base — as mesmas que o `AEEStdLib.h` lê. Mapear antes, e não na hora
do pedido, é porque o núcleo não tem API de mapear região depois do `reset`; os módulos são
pequenos (90 KB e 164 KB) e o custo é irrelevante.

O que fica para a hora do pedido são os dois passos que o console dá:

1. `AEEMod_Load(shell, helpers, &saída)` na primeira vez, e a extensão entrega o `IModule*` dela.
2. `IModule::CreateInstance(módulo, shell, clsid, &saída)` — o slot 2, o mesmo do applet.

O objeto que volta é implementado em ARM pela extensão. Daí em diante o jogo conversa direto com
ele e **nós não precisamos saber que interface é** — nem `IMICRO3D` nem o motor da Superscape
foram implementados aqui; eles simplesmente rodam.

### O desvio de regra, e a regra que vem com ele

Isto acontece **dentro** do despacho de uma chamada de API, que é justamente o lugar onde entrar
no guest era proibido — o `call_guest` normal escreve em `r0..r3` e no `lr`, e quem está
despachando vai ler o `lr` depois para saber onde retomar o jogo. Perdê-lo manda a execução para
o lugar errado, em geral para o endereço zero.

O `IIMAGE_Draw` numa superfície do jogo resolveu isso adiando a chamada para a fila da fronteira,
e aqui não dá: o jogo espera o ponteiro do objeto escrito antes do retorno. Então a chamada é
aninhada de verdade, com **todo o contexto salvo e devolvido** — `r0..r12`, `sp` e `lr`. A pilha
não precisa de cuidado, porque a chamada aninhada empilha abaixo do `sp` corrente, que é espaço
que ninguém está usando: a mesma garantia que uma interrupção tem.

### Onde os dois chegaram

Nenhum dos dois estava a uma classe de distância só — e vale registrar, porque é a diferença
entre "destravou" e "roda".

O **Action Hero 3D** criou o objeto de `IMICRO3D` e o obstáculo seguinte apareceu na hora:
`0x01004001`, pedida logo depois de um `IMemAStream`. É `AEECLSID_BMP`, e a identificação não é
palpite — os recursos dele são 67 BMPs dentro de um `.res` cuja primeira entrada se chama
`66.bmp`. O decodificador de imagem do `IImage` só tratava PNG; hoje ele olha a magia dos bytes
e trata PNG, BMP e JPEG pelo mesmo caminho, que é o certo de todo jeito: o console tem uma
classe por formato, todas alimentadas por `IAStream`, e o formato está escrito no arquivo. Com
isso ele cria o applet, recebe o `EVT_APP_START` e roda o laço com o relatório limpo — nenhuma
classe desconhecida, nenhum arquivo faltando, nenhuma hipótese. Ele ainda não põe nada na tela.

O **Kingdom Hearts** precisou de mais uma coisa, e ela não era de classe nenhuma: o processador
em **modo usuário**, ver [02-cpu-e-memoria.md](02-cpu-e-memoria.md). Com os dois, ele roda **405
voltas do laço e 418 milhões de instruções** em seis segundos virtuais, com a tela desenhada.
Sobra a `0x0100a004`, que não está em `.mif` nenhum do pacote nem na tabela de classes do
firmware, e que ele tolera não ter.
