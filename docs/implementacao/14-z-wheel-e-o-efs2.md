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

Ela cria o applet, roda o `EVT_APP_START`, monta o banco de preferências, abre os bancos do
catálogo e chega em:

```
tectoy_prefsDB.c:605   Unable to create vector model PREFSDB_GetRecords
tectoymain.c:338       Failed to load config lines.
```

As duas dependem da mesma classe que falta, a `0x01028e35`.

O `Could not create root form(20)` que parava tudo antes **saiu**. O `20` era `EUNSUPPORTED`, e a
classe recusada era a `0x01028e51` — não a `0x01001011`. Ver a seção sobre o retorno invertido.

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
| `0x01028e51` | widget da interface | **implementada** — é ela que a `tectoymain.c:1001` pede, e o slot 3 precisa responder **verdadeiro**: o envoltório em `0x3f72c` converte booleano em erro, com `moveq r0, #3` |
| `0x01006c05` | ZEEBOMCP | **implementada** — o nome sai da `Tectoy.c`, que imprime `Cannot create instance of ZEEBOMCP` |
| `0x01001027` | `IConfig` | **implementada** — `GetItem`/`SetItem` |
| `0x01028e35`, `0x01028e3c`, `0x01011810` | sem nome | o que falta hoje |

Elas **não são API do BREW que precisamos escrever**. São extensões que o console carregava —
`widgets.mod`, `forms.mod`, `framewidget.mod`, `imenu.mod`, `icontrols.mod` —, e neste firmware
estão compiladas dentro do `1.1.2_APPS.bin`: o `0x01001011` aparece 26 vezes ali e nenhuma no
sistema de arquivos.

## O retorno invertido da `0x01028e51`

A mensagem `Could not create root form` não é referenciada por literal, é por `ADR` — buscar o
ponteiro dela no módulo não acha nada. Varrendo os `ADD rD, pc, #imm`, a `tectoymain.c:1001` sai
em `0x7c478`, e o `ISHELL_CreateInstance` logo acima carrega o literal de `0x7c69c`: `0x01028e51`.

A classe não está na tabela do firmware, mas o jogo ensina o que precisa dela. O único método
usado é o **slot 3**, um acessador genérico chamado por dois invólucros que fixam o seletor:
`0x3f72c` passa `0x800` (pega o filho de número `id`) e `0x403c8` passa `0x801` (grava a
propriedade).

O detalhe que decidia tudo é o **retorno**: `cmp r0,#0; moveq r0,#3`. Zero vira erro. Responder
`SUCCESS`, que vale zero, era responder "falhou" — e a `0x78acc` não trata esse fracasso: pula
para a limpeza e desreferencia o segundo filho, que nunca foi preenchido. Era daí que vinha o
acesso inválido a zero em `0x78b70`.

## Duas vtables de fachada, e como se reconhece uma

A tabela de classes do `1.1.2_APPS.bin` respondeu duas vezes com uma vtable que **não é da
classe**. As duas custaram tempo, e as duas se reconhecem pelo mesmo teste: **ler o construtor
antes de copiar a vtable**.

- `0x01006c05` (ZEEBOMCP): a entrada aponta para `0x11267d04`, que é o `LCT_SIMCardCtl_New` e
  começa comparando o CLSID recebido com `0x01006c01` — recusa a própria classe sob a qual está
  registrado. A implementação de verdade é o singleton `0x11085cb8`, com a vtable `0x102d47a8` de
  oito métodos, no mesmo trecho que carrega `fs:/card3` e `fs:/mcp/`.
- `0x01001027` (`IConfig`): a entrada existe e o construtor é limpo, mas **nove dos doze métodos
  são `movs r0,#0x14; bx lr`** — devolvem `EUNSUPPORTED` e nada mais, o `SetItem` inclusive.
  Copiar aquilo fez o jogo trocar `Unable to create instance of IConfig, error 20` por `Unable to
  set language to config, error 20`: o mesmo 20, um passo adiante. A leitura mais provável é que
  seja um registro de fachada da partição de aplicativos, e que a `IConfig` de verdade viva no
  lado do BREW, que não temos.

## O evento que chega antes do start

A Z-Wheel imprimia `SendEvent to get PrefsDB failed` onze vezes e desistia da configuração
inteira. O evento é o `0x7b0a`, que ela manda **para a própria classe** pedindo o objeto do banco
de preferências; a resposta volta escrita no `dwParam`.

O tratador nunca era chamado. O `send_applet_event` lia o `current_applet`, e o `current_applet`
só é preenchido quando o `EVT_APP_START` é despachado — mas este evento acontece **antes disso**,
durante a construção do applet, dentro do próprio `CreateInstance`. Nesse instante o objeto já
existe: o `AEEApplet_New` escreveu o ponteiro de saída antes de o código do jogo rodar. Para o
shell, o applet passa a existir quando é **registrado**, não quando é iniciado.

Isso não é remendo de Z-Wheel: qualquer jogo que mande evento para si mesmo na construção estava
sendo ignorado.

## O modelo de dados da Z-Wheel

Com o PrefsDB de pé, o jogo abre os bancos e o esquema inteiro aparece nas strings do módulo. É o
catálogo que a interface mostra:

```sql
CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER,
                      dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT,
                      flags INTEGER, size INTEGER, unique(game_id, class_id))
CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT, unique(game_id, lang_id))
CREATE TABLE ASSETS(owner INTEGER, dslid INTEGER PRIMARY KEY, type INTEGER, version INTEGER,
                    path TEXT, language INTEGER, title TEXT, startdate INTEGER, enddate INTEGER)
CREATE TABLE DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                        titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER)
CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER)
```

## O próximo obstáculo: a `0x01028e35`

É a última classe entre a Z-Wheel e a tela. Ela aparece nos dois lugares que ainda falham — o
`PREFSDB_GetRecords`, que a chama de "vector model", e o carregador do `tectoy.cfg` em `0x88338`.

Atendendo-a com `--sonda`, as duas mensagens somem e o jogo passa a abrir **nove bancos e dez
consultas**. O que a sonda mostrou dela:

| slot | argumentos | leitura |
|---|---|---|
| 8 | `(0xffffffff, "x8", …)` | inserir no fim — o `-1` é a posição |
| 12 | `(0x52c64, …)` e `(0x87b2c, …)` | recebe **ponteiro de função do módulo**: registra um retorno de chamada |
| 5, 10 | — | ainda sem leitura |

**Cuidado com a `0x01001011` daqui.** No `0x88338` ela é usada como **fábrica de leitores**, não
como formulário: `slot6(arquivo, &saída)` e `slot3(objeto, n+1, &saída)`, cada um produzindo um
objeto novo. A nossa implementação, herdada do Zeeboids, trata o slot 3 como um `SetHandler` que
guarda um par e não escreve saída nenhuma — por isso, com a sonda ligada, o jogo morre num nulo
em `0x88470`. Ou a tabela do Zeeboids está errada, ou as duas classes têm o mesmo número e usos
diferentes; decidir isso é parte do próximo passo, não de um palpite.

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
