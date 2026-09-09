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

## A tabela de classes estava sendo lida deslocada de uma palavra

Esta custou horas, e vale contar direito porque a lição é o contrário da que eu escrevi primeiro.

O `firmware.py` lia as entradas da tabela de classes como
`{construtor, CLSID, sinalizadores, zero}`. A ordem verdadeira é
`{CLSID, sinalizadores, zero, construtor}` — o construtor vem **depois**. Lendo deslocado, cada
CLSID recebia o construtor da entrada anterior.

O erro não quebrava nada visivelmente: devolvia um construtor de verdade, apontando para uma
vtable de verdade, da classe errada. Duas vezes eu concluí que a tabela era "de fachada" quando o
problema era meu:

- Para a `0x01006c05` ela dava o `LCT_SIMCardCtl_New`, que começa comparando o CLSID recebido com
  `0x01006c01`. **Foi essa comparação que denunciou o deslocamento**: um construtor que recusa a
  própria classe é sinal de que ele não é dela.
- Para a `0x01001027` ela dava uma vtable de doze métodos em que nove eram `movs r0,#0x14`.
  Copiada, fez o jogo trocar `Unable to create instance of IConfig` por `Unable to set language to
  config` — o mesmo erro 20, um passo adiante.

Corrigida a leitura, a tabela aponta para o construtor `0x11085cb8` da `0x01006c05`, que é
exatamente o singleton que eu tinha achado a pé pelo `fs:/mcp/`. E a `0x01001027` deixa de ter
entrada: o que sobra para ela é um falso positivo, com sinalizadores `0xffff0008` e um
"construtor" cuja vtable tem um método que nem endereço é.

**A regra que fica**: desmontar o construtor antes de copiar a vtable. Se ele testa um CLSID, tem
de ser o que você pediu.

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

## Onde a Z-Wheel está agora

Sem classe desconhecida nenhuma. Ela monta o `PrefsDB`, abre catorze bancos, lê as setenta linhas
do `tectoy.cfg`, conecta o joystick pelo `IHID`, registra a leitura de posição, cria os sinais e
começa a montar a interface.

As classes que entraram, e de onde veio cada nome:

| Classe | O que é | Fonte |
|---|---|---|
| `0x01001011` | `ISourceUtil` | `AEESource.h` — ver abaixo |
| `0x01001027` | `IConfig` | assinatura do `ICONFIG_SetItem` do SDK |
| `0x01006c02` | controle de sistema | `OEM_LCTSystemCtl.c`, nas strings do firmware |
| `0x01006c05` | ZEEBOMCP | mensagem do próprio jogo |
| `0x01011810` | `ICM` | `SYS_OPRT_MODE_ONLINE` e a posição do campo |
| `0x01028e35` | lista genérica | código do jogo, duas leituras que concordam |
| `0x01028e3c` | criada e guardada | — |
| `0x01035156` | fonte TrueType | mensagem do próprio jogo |
| `0x01028e19/2a/3f/47/51` | família de widgets | um acessador só, medido |

## A `0x01001011` não era formulário raiz: é a `ISourceUtil`

Dois jogos a usavam de jeitos que pareciam incompatíveis. Com os nomes do `AEESource.h`, os três
usos encaixam de primeira:

```
slot 3  PeekSourceFromSource(po, ISource*, nMax, IPeek**)          Z-Wheel, lê o tectoy.cfg
slot 5  SourceFromMemory(po, pBuf, nSize, pfn, pUser, ISource**)   Zeeboids, embrulha o POST
slot 6  SourceFromFile(po, IFile*, ISource**)                      Z-Wheel
```

O `SourceFromMemory` é o que fecha a conta: **seis** parâmetros, com o ponteiro de saída no
segundo lugar da pilha — exatamente onde o Zeeboids o lia, num trecho que tínhamos batizado de
"envio". Ele não envia nada; embrulha o corpo do POST para entregar à `IWeb`. É de lá que a ponte
pega o corpo, e é por isso que ela sempre funcionou.

## O evento que a Z-Wheel manda para si mesma antes de existir

Ela imprimia `SendEvent to get PrefsDB failed` onze vezes e desistia da configuração. O evento é o
`0x7b0a`, mandado **para a própria classe**, e o tratador nunca era chamado: o `current_applet` só
é preenchido quando o `EVT_APP_START` é despachado, e este evento acontece antes, durante a
construção do applet. Nesse instante o objeto já existe — o `AEEApplet_New` escreveu o ponteiro de
saída antes de o código do jogo rodar. Para o shell, o applet passa a existir quando é
**registrado**, não quando é iniciado.

Não é remendo de Z-Wheel: qualquer jogo que mande evento para si mesmo na construção estava sendo
ignorado.

## O que falta para o menu, e o que já foi tentado

A abertura aparece. O menu não, e a razão está lida, não suposta.

### A máquina de estados da abertura

A `AnimationVideo_Form` **se sustenta sozinha**: a `0x11528` arma um `ISHELL_SetTimer` de mil
milissegundos com o retorno de chamada `0x114ac`, que é o próprio tique. O console não fica
cutucando a animação; ele dá **um** aviso e o resto é do jogo. O aviso é o par `(0x801, 0x5064)`
no tratador que o slot 4 registrou — a forma que o `0x11828` desvia para o `0x114ac`.

O estado fica em `[formulário+0x2c]` e o contador em `+0x30`:

| estado | o que acontece |
|---|---|
| 0 | conta até trinta tiques de um segundo, depois vira estado 3 |
| 1 | pede o **tocador** pelo slot 8 do widget e manda tocar |
| 2 | arma temporizador e espera |
| 3 | agenda um `ISHELL_Resume` — **é por aqui que o menu entra** |

Medido: quando o aviso é entregue, o estado **já é 1**. Quem o põe em 1 é o retorno de chamada
da imagem, em `0x4d484`: imagem carregada é imagem pronta para tocar. Ou seja, o caminho do
console passa pelo tocador, e o estado 0 (contar trinta segundos) é o caminho de quando não há
imagem.

### As duas tentativas, e por que as duas foram desfeitas

**Aviso repetido.** Entreguei o par a cada cem milissegundos. A máquina saiu do estado de
contagem, pediu o tocador, seguiu pelo caminho do nulo e acabou num `malloc` com **ponteiro no
lugar do tamanho**, em laço infinito.

**Aviso único, com o slot 8 devolvendo o pai.** O aviso único é a leitura certa — a máquina se
rearma sozinha. E devolver o pai tem apoio: o objeto que sai do slot 8 recebe em seguida um
`slot3(widget, 0, 0)` e é solto, e slot 3 num widget é o acessador, que recusa seletor
desconhecido sem estragar nada. Com isso a Z-Wheel **começou a montar o menu** — 178 objetos
vivos, 723 milhões de instruções.

Mas o log conta o resto: `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **486.101 vezes**,
`SendEvent to get Tectoy Font failed`, `Couldn't create z-pad instruction form (6)`. Implementar o
`LCT_SIMCardCtl` — que é classe de firmware, com construtor conferido — tirou aquele erro e trocou
o laço por um **travamento duro**: giro puro, sem uma única chamada de API, indefinidamente.

As duas foram desfeitas. "O jogo andou" não é prova de que andou pelo caminho certo, e um
travamento é pior do que uma abertura estável.

### O que falta, com nome e endereço

1. **O tocador de animação**, do slot 8 do widget. Não é um widget: o que sai dali recebe
   `slot3(widget, 0, 0)`, assinatura que o acessador não tem.
2. **A fonte.** A `0x7bfc8` chama o **slot 4** da `0x01035156` esperando um objeto de fonte no
   ponteiro de saída, e em seguida o entrega ao slot 9 de outro objeto. Sem isso, o
   `SendEvent to get Tectoy Font failed` e os formulários que dependem de texto falham com erro 6.
3. **O widget de rolagem** — a `MainMenu_Form.c` imprime `Unable to create roller widget in
   MainMenu form`, e é ele que desenha o carrossel.
4. **Pintura de verdade**, com texto e recorte. O `pinta_widgets` só joga na tela uma imagem
   pendurada.

Os quatro são pedaços da extensão de interface do console. Não é mais um slot por vez: é
reconstruir um subsistema, e cada objeto que se inventa no meio do caminho leva o jogo para um
estado que o aparelho nunca alcança.

## Onde ela para hoje, e por quê

Numa **recursão infinita** em `0x1177c`, que é o despachante de tratadores: ele lê a função em
`+0x20` e o contexto em `+0x1c` — a mesma estrutura que o slot 4 do widget registra — e a chama.
A pilha enche de `0x1179c` repetido.

A suspeita mais provável é nossa, e é específica: o grafo de widgets que devolvemos é
**auto-referente**. O `QueryInterface` do widget devolve `this` para qualquer IID, e o acessador
devolve um filho da mesma classe; um jogo que caminhe por essa estrutura procurando um pai ou um
irmão nunca sai do lugar.

Isso marca o fim do que se resolve descobrindo um slot por vez. Daqui em diante é preciso saber
**quais** interfaces cada widget realmente implementa — e essa é uma pergunta sobre a extensão de
interface do console, não sobre a Z-Wheel.

## O modelo de dados da Z-Wheel

É o catálogo que a interface mostra:

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
