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
