# 15 — O que ainda falta tirar da NAND

Esta é a lista pedida: **o que o firmware do console tem e o emulador ainda não**. Ela ficou
possível de responder rápido depois que o `ferramentas/firmware.py` ganhou o comando `classe`;
antes, cada classe custava uma dúzia de passos manuais.

## Como a lista é levantada

O `1.1.2_APPS.bin` tem uma tabela de classes com entradas de dezesseis bytes:

```
<u32 construtor> <u32 CLSID> <u32 sinalizadores> <u32 zero>
```

O comando acha a entrada, desmonta o construtor atrás do literal que ele grava em `[obj]` — a
vtable — e lista os métodos até o primeiro que não é endereço Thumb:

```
$ python3 ferramentas/firmware.py classe 0x01001011
  construtor  0x112e399c  (sinalizadores 0x8)
  vtable      0x10a785e4  (7 métodos)
```

Três filtros separam a entrada real das outras citações do mesmo CLSID — a `0x01001011` aparece
vinte e seis vezes no arquivo: o construtor tem o bit 0 do Thumb, cai num segmento carregável, e
começa com `push`. Os dois primeiros sozinhos ainda deixavam passar texto ASCII lido como
número.

**Um resto de dúvida, dito na cara**: entradas com sinalizadores `0xffff0000` são suspeitas. As
confirmadas trazem valores pequenos (`0x4`, `0x8`, `0x9`, `0x20000`), e as duas com `0xffff0000`
podem ser casamento por acaso. Onde isso aparece abaixo, está marcado.

## O que já está lido e implementado

| Classe | O que é | Estado |
|---|---|---|
| `0x01001011` | despachante com fila (não é formulário) | **implementada** — 7 métodos, e o slot 5 é o envio que fez a rede funcionar |
| `0x0100104f` | coleção genérica | implementada |
| `0x0102c4e8` | `SQLMGR`, sobre SQLite de verdade | implementada |

## O que o firmware entrega e falta implementar

| Classe | Métodos | O que destrava |
|---|---:|---|
| `0x01006c05` | 8 | pedida pela Z-Wheel; construtor `0x11267d04`, vtable `0x102d47a8` |
| `0x0103475a` | 39 | o objeto **interno** da `0x01001011` — é para ele que os slots 4, 5 e 6 delegam. Sinalizadores `0xffff0000`: conferir antes de confiar |
| `0x01003109` | 40 | o subsistema de texto do Zenonia (`CWBLText::Create() failed!`). Construtor `0x108ee2ec`, vtable `0x10e15c04` |
| `0x0100102e` | 59 | a classe de rede que o Opera Mini pede. Construtor `0x10b9b778`, vtable `0x10a783e4` |

### O construtor que não grava a vtable

As duas últimas linhas ficaram por muito tempo com `?` em "métodos", e não porque a informação
faltasse: **o construtor delas não grava a vtable.** Ele aloca o objeto e passa o serviço a uma
função de inicialização, alcançada por salto longo — `ldr pc, [pc, #-4]`, o trampolim que o
linker põe entre segmentos distantes. O construtor inteiro se desmonta sem um único `str` em
`[obj]`, e seguir a chamada à mão levava ao trampolim, cujo desmontado é lixo.

A `0x0100102e` mostra o padrão inteiro:

```
0x10b9b786  ldr  r1, [pc, #0x44]   ; = 0x01039270   <- cria antes a classe de que depende
0x10b9b790  blx  r3                                 <- ISHELL_CreateInstance
0x10b9b798  movs r0, #0x30
0x10b9b79a  bl   #0x10d6f944                        <- malloc(0x30)
0x10b9b7b0  blx  #0x10b98744                        <- init, e é aqui que a vtable entra
0x10b9b7b4  str  r5, [r4]                           <- *out = obj
```

O `0x10b98744` é o trampolim; atrás dele, em `0x105c7dd4`, está o init de verdade:

```
0x105c7ddc  ldr  r0, [pc, #0x20]   ; = 0x10a783e4   <- a vtable
0x105c7dde  str  r0, [r4]
```

O `firmware.py` faz isso sozinho agora: quando o começo do construtor não entrega o literal, ele
segue as chamadas — atravessando o trampolim — **um nível**. Um nível cobre estas classes, e cada
nível a mais aumenta a chance de achar a vtable *de outra coisa*: o construtor também chama
`malloc` e o `CreateInstance` do objeto de que depende.

Vale registrar de onde vinha o outro número errado: a contagem de métodos parava num **teto de
16** que ninguém via, e o teto entrava na tabela como se fosse o tamanho da interface. A
`0x0103475a` tem 39 métodos, não 16. Hoje o teto é 64, ajustável (`classe <clsid> <teto>`), e a
ferramenta avisa quando a contagem bate nele.

A leitura entrega mais uma coisa de graça: a `0x0100102e` **cria a `0x01039270` antes de
qualquer outra coisa** e devolve o erro dela se falhar. Isso é pista sobre o que a classe é, e
não um obstáculo nosso — o firmware não é executado aqui, nós implementamos a classe no host. A
`0x01039270`, por sua vez, não está na tabela desta partição, o que a põe na mesma lista das
outras que faltam, abaixo.

E vale ser exato sobre o erro do Opera Mini: o `EFAILED` que ele leva no `CreateInstance` do
applet vem de **nós** não implementarmos a `0x0100102e` — o `ISHELL_CreateInstance` dele
recusa, e o construtor do applet desiste. Não é o construtor do firmware falhando; esse a gente
só leu.

## O que o firmware **não** tem

| Classe | Quem pede |
|---|---|
| `0x01028e35` | Z-Wheel — e é o próximo obstáculo dela, chamada no slot 12 |
| `0x01028e51` | Z-Wheel e Zeebo App |
| `0x01035156` | a fonte TrueType da Z-Wheel |
| `0x01001031` | aparece no `QueryInterface` do `IWeb` |
| `0x01039270` | a classe de que a `0x0100102e` depende — o Opera Mini, por consequência |

Nenhuma está na tabela da partição APPS, e a explicação provável é a mesma de sempre: extraímos
só essa partição. O resto do sistema de arquivos está atrás do EFS2, cujo leitor parou na
geração corrente da tabela de páginas (ver [14](14-z-wheel-e-o-efs2.md)).

## O que não é problema de classe

Vale separar, porque muda a ordem do trabalho:

- **O leitor de EFS2** é o que destrava as quatro classes acima de uma vez, e também os módulos
  de extensão (`widgets.mod`, `forms.mod`, `isql.mod`) que o console carregava. O que falta é
  reproduzir o diário de alterações do sistema log-estruturado.
- **A `0x01001011` não era um formulário**, apesar da mensagem de erro da Z-Wheel dizer
  "root form". A vtable mostrou um despachante com fila. Isso vale como aviso: o nome que o
  aplicativo dá a uma classe descreve o uso dele, não a classe.

## Ordem sugerida

1. **`0x0103475a`**, o objeto interno — é o que falta para a `0x01001011` deixar de ser esboço,
   e ela é a classe que os dois alvos usam.
2. **`0x01006c05`**, que já está lida e é pequena (4 métodos).
3. **O leitor de EFS2**, que destrava as quatro que faltam de uma vez em vez de uma a uma.
