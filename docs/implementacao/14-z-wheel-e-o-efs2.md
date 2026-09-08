# 14 — Z-Wheel e o EFS2: onde paramos

Este documento existe para que a próxima leva sobre a Z-Wheel comece do ponto certo, e não do
começo. Ele registra o que está feito, o que está medido e qual é o único elo que falta.

## O que a Z-Wheel é

Não é um jogo: é o **aplicativo de loja do console** — catálogo, fila de download, pontos e
telemetria. O App ID é 274755 e o ClassID do applet é `0x01070798`. O pacote traz o servidor em
texto puro, no `tectoy.cfg`:

```
credit_server_url=https://aquila.tectoy.com.br:8443/WSM/wsm?wsdl
```

Isso faz dela o alvo natural de um servidor privado: apontar a linha para outro lugar basta, sem
patch em runtime. E ela funciona **offline** no console — o servidor era só para comprar e baixar.

## Até onde ela chega hoje

Ela cria o applet e roda o `EVT_APP_START`. Lê o banco de preferências, lê o `tectoy.cfg`, e para
em:

```
tectoymain.c:1001   Could not create root form(20)
```

O `20` é `ECLASSNOTSUPPORT`.

O que foi conseguido no caminho, e está no emulador: o `AEECLSID_SQLMGR` sobre SQLite de verdade
([`sql.rs`](../../src/sql.rs)), a coleção genérica `0x0100104f`, o `ISHELL_SendEvent`, o
`GETJULIANDATE`, o `IDisplay::Clone` e o `DrawText` desenhando com a `tectoy.ttf` que ela mesma
empacota.

## As classes que faltam, e onde elas estão

| Classe | O que é | Como foi identificada |
|---|---|---|
| `0x01001011` | formulário raiz | a mensagem de erro que o app imprime ao recusá-la |
| `0x0100104f` | coleção genérica | **implementada** |
| `0x01035156` | fonte TrueType | fica entre `Unable to create instance of TrueType TYPEFACE` e `TrueType Dictionary` no pool de literais |
| `0x01028e51` | sem nome | o slot 3 dela precisa responder **verdadeiro**: o envoltório em `0x3f72c` converte booleano em erro, com `moveq r0, #3` |
| `0x01028e35`, `0x01028e3c`, `0x01011810` | sem nome | aparecem quando as anteriores são atendidas |

Elas **não são API do BREW que precisamos escrever**. São extensões que o console carregava —
`widgets.mod`, `forms.mod`, `framewidget.mod`, `imenu.mod`, `icontrols.mod` —, e neste firmware
estão compiladas dentro do `1.1.2_APPS.bin`: o `0x01001011` aparece 26 vezes ali e nenhuma no
sistema de arquivos.

## A corrente até a vtable, e o elo que falta

```
EFS2  →  MIFs dos módulos estáticos  →  registro de classes  →  vtables
```

O primeiro elo é o que falta, e ele é o primeiro da corrente.

**O que está resolvido do EFS2** (ver [`ferramentas/nand.py`](../../ferramentas/nand.py)):

- A tabela de partições, conferida: com blocos de 128 KB, a partição APPS calculada bate byte a
  byte com o `1.1.2_APPS.bin` publicado à parte.
- O dump tem dois arquivos; o `_spare` é o mesmo conteúdo intercalado em setores de 512 + 16. A
  área fora de banda só tem ECC — o mapa é interno.
- O formato da entrada de diretório: `<u8 tamanho> <u8 tipo> <u32 data> <u8 zero> <nome> <'i'>
  <u32 ref>`, com o tamanho contando o nome mais cinco.
- O mapa é uma tabela de páginas: `tabela[ref + n]` é a página física do n-ésimo pedaço de 2 KB.
  **Confirmado**: montando a `tectoy.ttf` pela tabela em `0x4c23104`, as treze primeiras páginas
  saem idênticas à cópia conhecida.

**O que falta**: a geração corrente da tabela. A entrada 13 daquela cópia aponta para `0x14f5` e
a correta é `0x17a6` — não é corrupção, é uma versão antiga. E a corrente não existe como bloco
contíguo: procurando a lista real de 94 páginas da fonte, o prefixo de 5 aparece uma vez e o de
16 não aparece nenhuma. O mapa corrente é a tabela base **mais um diário**, e reproduzi-lo é o
trabalho que falta.

## Duas armadilhas que já custaram caro

**Verificação que concorda com a hipótese não é verificação.** Isto pegou três vezes, em formas
diferentes: uma base de mapeamento calibrada por uma string que era a ocorrência errada, um
desmontado que produziu instruções válidas a partir de texto, e um rastro de execução que
guardava as primeiras instruções quando eu lia como se fossem as últimas. A desmontagem quase
nunca se recusa a produzir código, e o `find` num dump devolve a primeira ocorrência, não a
corrente.

**Fonte primária ganha de inferência.** Deduzi dos binários dos jogos que o eixo `X` do controle
era `0x0106c4ce`, com evidência coerente em dezenas de títulos. O `hid_devices.original.cfg` do
console diz `0x0106C40C`. A dedução descrevia bem **outro aparelho** — o controle de PC, cuja
entrada no mesmo arquivo é de fato `c4d0`/`c4d1`.

## Material

Fica em `vendor/`, ignorado pelo git — firmware e conteúdo do console não entram no repositório.
Origem: `tripleoxygen.net/files/devices/zeebo/`.
