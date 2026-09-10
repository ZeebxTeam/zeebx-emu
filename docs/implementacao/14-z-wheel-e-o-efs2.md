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

### Por que ela fica na tela de boas-vindas

A resposta curta: **a abertura nunca recebe a partida**, e quando eu a dou, o jogo pede o tocador,
que não temos.

A resposta longa está no retorno de chamada da imagem, em `0x4d484`, e ela corrige uma leitura
minha. Aquele trecho lê o tamanho natural do widget, compara com `0x280` — seiscentos e quarenta,
a largura da tela — e, sendo igual ou maior, segue pelo caminho de tela cheia: liga o bit 2 do
item `0x347`, lê o item `0x414` para `[formulário+0x24]` e **põe o estado em 1**.

Ou seja, estado 1 não é acidente nosso: é o estado certo para uma abertura que ocupa a tela
inteira. E o estado 1 é justamente o que pede o tocador.

### O acessador do widget é tipado, e nós não temos a tabela de tipos

Isto foi um erro de verdade, encontrado aqui. O seletor `0x800` **não é "pega o filho"**: é "lê o
item", e o que sai depende do número do item. O retorno de chamada da imagem lê o item `0x347`,
soma dois e grava de volta — é número. Enquanto a leitura criava um filho para qualquer item, o
que o jogo somava dois era um **ponteiro nosso**, e o que ele gravava em `[formulário+0x24]` pelo
item `0x414` era outro.

O corte está em `0x5000`, e sabe-se onde ele erra: o `0x414` está abaixo da linha e mesmo assim
guarda um widget. Subir a linha faria o `0x347` voltar a receber ponteiro. Os itens são tipados e
a tabela de tipos é do console.

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

## O EFS2, e por que ele não era o caminho

A ideia era: se as classes de interface não estão na tabela do firmware, elas devem estar em
módulos de extensão do sistema de arquivos — `widgets.mod`, `forms.mod`, `framewidget.mod`. Para
lê-los faltava decifrar o EFS2. Ao atacá-lo, três coisas apareceram, e as três mudam o plano.

### O sistema de arquivos está listado, e não tem extensão nenhuma

Os nós de diretório estão em claro no dump — não precisam do mapa de páginas. Varrendo tudo e
juntando (`nand.py nomes`) sai a lista completa: **317 nomes**, e nenhum é extensão do BREW. Só os
módulos dos jogos — `tectoy.mod`, `reksio.mod` — e dados: `.qxt`, `.qxm`, `.brf`, `.html`, fontes
BDF, perfis do modem, logs de erro.

A contagem confirma o modelo log-estruturado: `tectoy.mod` aparece **400 vezes**, uma por
regravação.

### As classes não estão registradas em lugar nenhum do dump

Procurando `0x01028e51`, `0x0100104f` e `0x01035156` como entrada de registro — em qualquer dos
formatos plausíveis, de oito, doze ou dezesseis bytes — nos **128 MB inteiros**: zero. Elas
aparecem como literal quatro ou cinco vezes cada, sempre dentro de pool de código ou de tabela de
strings, nunca como registro.

### A tabela de classes era parcial porque a líamos errado — e mesmo inteira não tem as nossas

O `registro()` procurava **entrada isolada**, filtrando por uma lista de sinalizadores. Só que
sinalizador não é um punhado de valores: no `1.1.2_APPS.bin` aparecem `0x1`, `0x4`, `0x5`, `0x8`,
`0x20004`, `0x2000004`, `0xffff0000`. Com o filtro saíam 105 entradas.

O que identifica a tabela é a **corrida**: quatro ou mais entradas seguidas cujo primeiro campo
cai na faixa dos ClassIDs. Assim saem **84 tabelas, 704 entradas, 425 classes distintas** — a
maior em `0x10c3a018`, com 62. O `AEECLSID_FILEMGR` está lá, com os **dois** ponteiros da entrada
preenchidos, o que também esclarece a forma: `<CLSID> <sinalizadores> <a> <b>`, construtor em `b`
e às vezes um segundo ponteiro em `a`.

E aí vem a parte que resolve a pergunta. Com as 84 tabelas lidas, continuam **ausentes**:

| Classe | O que é |
|---|---|
| `0x01028e51`, `0x01028e35`, `0x01028e3c`, … | a família de widgets |
| `0x0100104f` | a coleção genérica |
| `0x01035156` | a fonte TrueType |
| `0x0102c4e8` | `AEECLSID_SQLMGR` |
| `0x0106c411` | `AEECLSID_HID` — o gamepad |
| `0x01041207` | `AEECLSID_SignalCBFactory` |

Não é coincidência: **é a camada específica do Zeebo inteira**. O SQLite, o gamepad, os sinais, a
interface. Nada disso está no `1.1.2_APPS.bin`, e nada disso está em nenhuma das outras partições
do dump.

Isso explica retroativamente de onde veio cada identificação que temos dessas classes: o
`AEECLSID_SQLMGR` saiu do log do próprio jogo, o `AEECLSID_HID` saiu do `hid_devices.cfg` e da
`IHID.dll` do SDK, os widgets saíram do código da Z-Wheel. **Nenhuma veio do firmware, porque
nenhuma está nele.**

### O que isso provavelmente é: o framework de widgets do BREW 4.0

Juntando o que foi medido slot a slot, aparece um desenho que não é do Zeebo — é do BREW:

| o que medimos | o que é no BREW |
|---|---|
| slot 3 com `(0x800 ou 0x801, id, valor)` | `IWIDGET_HandleEvent(evt, wParam, dwParam)`, com os eventos de ler e gravar propriedade |
| retorno invertido, zero é erro | `HandleEvent` devolve **booleano**: tratei ou não tratei |
| evento `0x100` com códigos `0xe030`, `0xe04a` | evento de tecla, com os `AVK_` do BREW |
| slot 4 com `{função, contexto}` | `IWIDGET_SetHandler` |
| slot 7 com `{largura, altura}`, slot 5 lendo de volta | `IWIDGET_SetExtent` / `GetExtent` |
| a lista com tamanho, pegar-em, inserir-em, remover-em | `IVectorModel` |

Cada uma dessas leituras foi feita isolada, e todas caem no mesmo lugar. Isso é o que dá força à
hipótese: ela **explica de uma vez** o que vinha sendo explicado peça por peça — inclusive o
retorno invertido, que era a esquisitice mais difícil de justificar.

**O que falta para sair de "provável" não é firmware, é documentação**: os cabeçalhos
`AEEWidget.h`, `AEEContainer.h`, `AEEForm.h` e `AEEModel.h` do SDK do BREW 4.0 dariam a ordem
exata dos slots, em vez de a deduzirmos um por execução. Foi assim que a `ISourceUtil` saiu de
"formulário raiz" para o nome certo, com o `AEESource.h`.

E vale dizer o que **não** ajudaria: outro dump de NAND. O dump que temos já tem, no próprio
APPS, código que usa `0x01028e35` e as vizinhas — o console usa essas classes e não as
implementa em lugar nenhum que se possa achar. Procurar outro dump é apostar que o problema é de
versão; a evidência aponta para uma camada que simplesmente não é despejada por este método.

### Conferência independente: o `IFileMgr`

O tripleoxygen tem, em `research/brew/`, um `IFILEMGR_VTBL_Z200.txt` e um `vtbl.ods` — trabalho de
outra pessoa mapeando vtable do mesmo firmware. Vale como conferência de fora, e ela passou em
duas frentes:

- o endereço da vtable do `IFileMgr` que a nossa leitura corrigida encontra é `0x113e0438`, o
  mesmo que está no arquivo deles;
- os **vinte e um** nomes da nossa tabela `FILEMGR` batem, na ordem, com o `INHERIT_IFileMgr` do
  cabeçalho do SDK que eles transcreveram — do `AddRef` ao `GetFreeSpaceEx`.

Não é confirmação do que falta, mas é confirmação do **método**: ler vtable no firmware, do jeito
que está no `firmware.py` hoje, dá o mesmo resultado que outra pessoa obteve por outro caminho.

### O SDK do Zeebo não traz os cabeçalhos do BREW

O `ZeeboSDKPackage-1.2.4.zip` (50 MB) tem o instalador do Zeebo, o do OpenGL ES, o Adreno
Profiler, o driver USB, o guia do desenvolvedor e os exemplos — inclusive o fonte do `conftest`.
**Nenhum cabeçalho `AEE*.h`**: o SDK do Zeebo se apoia no SDK do BREW da Qualcomm, que é instalado
à parte e não está lá.

O material de BREW da pasta `doc/` é da era 2 e 3 — os exemplos trazem `AEE.h`, `AEENTP.h`,
`AEEAddrBookExt.h`, e nada do framework de widgets, que é do BREW 3.1 em diante. O
`BREWOemAPIReferenceforMSM.pdf` também não menciona `IWidget` nenhuma vez.

Ou seja: o que falta é o **SDK do BREW 4.x da Qualcomm**, com o `AEEWidget.h` e companhia. Não é
material de Zeebo, é da Qualcomm, e não está neste acervo.

### O que se confirmou de graça: o `ICM`

Com a leitura corrigida, o `AEECLSID_CM` (`0x01011810`) ganhou vtable de verdade — `0x10a5e1f0`,
com pelo menos trinta e quatro slots. O **slot 28** existe e é `0x10ca927c`, e o que ele faz é:

```
0x10ca92b6  bl   #0x10ca46d0
0x10ca92c0  str  r0, [r2, #0xc]      ; o valor vai para o deslocamento 0xc
0x10ca92ca  blx  #0x10b967f8         ; copia a estrutura para o buffer de quem chamou
0x10ca92ce  movs r0, #0              ; zero é sucesso
```

O deslocamento `0xc` que a nossa implementação escreve **está confirmado pelo firmware**. O valor
cinco continua sendo inferência do lado do jogo, que compara com cinco em `0x77564` — mas agora só
metade da hipótese é hipótese.

### E a Z-Wheel do console é outro build

O `tectoy.mod` instalado no dump tem strings que o nosso não tem: `SpinStage.c`, `StageWidget.c`,
`spin_ui_utils.c`, `tectoy_maskededit.c`. Comparando a página que contém o `0x01028e51` nos dois,
elas diferem — o deslocamento dentro da página nem é o mesmo.

Ou seja, o dump é de 2011 e o nosso pacote é de outra geração. Mesmo com o EFS2 lido por inteiro,
o que sairia dele não é necessariamente o que o nosso pacote espera.

## O que ainda falta do EFS2, se um dia precisarmos

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

## O "objeto 10" do evento 0x7b0a

O `0x885a4` pede ao próprio applet, por `SendEvent(cls=0x01070798, evt=0x7b0a,
wParam=0xa, dwParam=&saída)`, um objeto que ele guarda em `[app+0x34e4]`. Sem esse
objeto o `0x89518` sai cedo, o `0x8f700` recebe zero e o jogo escreve
`Couldn't create z-pad instruction form (6)` — para sempre, milhares de vezes por
minuto.

O tratador está em `0x7bd4c`, que separa por `wParam`: `1 → 0x7bdac`,
`2 → 0x7bebc`, `4 → 0x7c134` (PrefsDB) e `0xa` caindo em `0x7bdc0`. O ramo do 10 lê
a preferência **`Lang`** e entra num laço:

```asm
0x7bdf4  ldr r5,[r5,#0x4e4]   ; cabeça da lista de idiomas
0x7bdf8  ldr r0,[sp,#8]       ; o valor de Lang
0x7bea4  cmp r5,#0            ; fim da lista -> desiste
0x7bea8  bne 0x7be00
0x7be00  ldr r1,[r5]          ; etiqueta do idioma da vez
0x7be04  cmp r1,r0
0x7be08  bne 0x7bea0          ; não é este, próximo
```

`[r5]` não é um índice: são **quatro caracteres** lidos como uma palavra. O rastro
mostra `0x20206e65`, `0x20207365` e `0x20207470` — `"en  "`, `"es  "` e `"pt  "`.
A preferência `Lang` guarda a etiqueta no mesmo formato empacotado.

O `tt_prefs.db` do pacote nasce com `Lang = 0`, junto de `TermsAccepted = 0` e
`IsRegistered = 0`: é um perfil que nunca passou pela primeira configuração — a tela
que oferece `language_english.bmp`, `language_portugese.bmp`, `language_spanish.bmp`
e `language_mexico.bmp`. Zero não casa com etiqueta nenhuma, e é por isso que o laço
sempre termina em nada.

Enquanto não alcançamos essa tela, o `escolhe_idioma` grava `"pt  "` na primeira vez
que o banco é aberto sem escolha, e o relatório anota a hipótese. Com isso o erro do
z-pad desaparece e o jogo passa a pedir `tectoy_pt.brf`.

A parede seguinte é outra: `Service status is NOT available!`, no
`tectoy_ui_internal_utils.c:647`, repetido no mesmo compasso do
`Unable to create instance of AEECLSID_LCT_SIMCARDCTL`.

## O estado do serviço, e o laço que sobra

O slot 28 do `ICM` não é `GetPhoneInfo`: é o `ICM_GetSSInfo`, e enche um `AEECMSSInfo`
de 0x340 bytes. Duas partes do jogo leem o mesmo resultado por caminhos diferentes —
a `0x87c90` quer o modo de operação em `+0xc`, a `0x696a0` quer o estado do serviço em
`+0` e, quando ele é 2, a intensidade do sinal em `+0x28`, que a `0x69830` traduz em
barras por faixas de nove. Respondíamos só o primeiro, e o segundo repetia
`Service status is NOT available!` no compasso do ciclo de atração.

O que ainda não anda é o próprio ciclo. A Z-Wheel se conduz por `ISHELL_Resume`: cada
volta chama `0x82464`, que faz a verificação de cartão em `0x78544`, recebe `0x27`
("não deu para verificar"), passa por `0x1f7b4` e se re-agenda. São 1736 voltas dentro
de **um** quadro, com um punhado de widgets vazando em cada uma.

O que quebraria o laço é o estado `0x28`, e ele exige que o `AEECLSID_LCT_SIMCARDCTL`
exista: o `0x78580` chama o slot 3 dele e, se a resposta for zero, `0x82470` sai sem se
re-agendar. Oferecer a classe foi testado de novo aqui, e continua pior: o jogo cria um
controle por volta e nunca o solta — 65 506 objetos vivos, o pote esgotado, e o z-pad
passando a falhar com código 3. O slot 3 registra um par de retorno de chamada em
`+0xc4`/`+0xc8`, e sem alguém respondendo "não há cartão" o objeto nunca é liberado.

Duas ideias foram testadas e desfeitas:

- **Adiar as retomadas** que passassem de uma cota por quadro. O laço de fato caiu de
  1736 voltas para a cota, e o heap de 4 MB para 907 KB — mas o jogo passou a repetir
  `ISHELL_Resume` 576 mil vezes no quadro, esperando a retomada que não vinha. Adiar
  não pausa o jogo; só troca um laço por outro.
- **Vencer os timers na fronteira de callback**, e não só entre quadros. A motivação é
  real — um quadro pode durar dois minutos virtuais, e nesse tempo nenhum timer vencia.
  Mas a reentrância quebra quem não a espera: o Crash Bandicoot passou a morrer num
  acesso a nulo em `0x435b0`.

## A cadeia da tela de boas-vindas

Levou três voltas para ficar clara, com uma correção minha no meio.

1. O `AnimationVideo_Form` toca o `opening_low.gif`, que tem **um quadro só**: é a
   própria tela de boas-vindas, 640×480.
2. Terminado, a máquina de estado em `0x114ac` vai ao estado 3 — o terminal — e a cada
   volta chama `HandleEvent(0x801)` no widget e se re-agenda por `ISHELL_Resume`.
3. A retomada entra em `0x11750`, que chama `[app+0x24]->slot6(1)` e cai no `0x82464`:
   a verificação de cartão SIM.
4. O desfecho `0x27` leva à `0x1f7b4`, que passa pelo `0x1f85c` e chega ao
   `0x7ed10(app, 1)` — **o lançamento do formulário de instruções do z-pad**. A
   mensagem `Unable to launch z-pad intructions form: %d` sai daí.

O ponto que me custou uma reversão: dos três desfechos da verificação, só o `0x27`
avança. O `0x28` compara, desvia para o fim e retorna — calado porque não faz nada. Ao
oferecer o `AEECLSID_LCT_SIMCARDCTL` e responder "cartão em ordem", o log ficou limpo e
a tela ficou parada para sempre. O `Unable to create instance of
AEECLSID_LCT_SIMCARDCTL` repetido é o jogo tomando o caminho certo muitas vezes.

O estado 0 da mesma máquina é o que espera a imagem carregar — trinta tentativas de
segundo em segundo, e `waiting for image load to complete...` quando desiste. Nunca é
alcançado aqui: o GIF carrega de primeira.

### O relógio e a entrada

O `skip_idle_time` saltava direto para o timer mais próximo. Com os 120 000 ms de
inatividade que a tela de boas-vindas arma, o relógio ia a dois minutos de uma vez e
disparava o tempo de ocioso antes de qualquer tecla ter chance de chegar: o laço dava
**seis** voltas em noventa segundos de relógio real, e um roteiro de teclas nunca
encontrava o instante marcado. O salto agora é de um quadro por vez — 8398 voltas.

### A posição dos widgets

O terceiro argumento do `AdicionarFilho` sempre trouxe a posição, e nós sempre a
jogamos fora. São seis palavras, `{x, y, sinalizador, largura, altura, objeto}`: a
imagem de abertura entra com `{0, 0, 1, 640, 480}` e os pedaços do formulário do z-pad
com `{100, 21}`, `{148, 20}`, `{365, 20}`. O mesmo slot atende cinco classes da família,
e nem toda chamada tem esta forma — algumas passam uma função em `r2` e um objeto em
`r3` —, então a posição só é lida quando `r2` é zero.

As propriedades que o jogo grava pelo seletor `0x801`, medidas: `0x130` (0 ou 255),
`0x140` (uma cor, `0x444444ff`), `0x152` (objetos), `0x153` (`0x10`, `0x410`, `0x420`,
`0x440`), `0x216` (0 ou 1), `0x347` (2) e a faixa `0x5000` (objetos). Nenhuma é
coordenada.

## De onde vêm os widgets

A pergunta é de onde tirar a implementação, e a resposta tem uma parte boa e uma ruim.

**A ruim: o binário não está no dump.** O APPS traz as strings
`fs:/mod/widgets/widgets.mod`, `fs:/mif/widgets.mif` e `fs:/mod/htmlwidget/htmlwidget.mod`
— os caminhos existem, os arquivos não. Procurados na NAND inteira, os 128 MB, a
cadeia `widgets` aparece treze vezes e todas dentro do APPS, como constante. O
`part_EFS2APPS.bin` tem dois módulos, `reksio.mod` e `tectoy.mod`, e mais nada.

As classes também não estão na tabela de classes: `0x01028e19`, `0x01028e2a`,
`0x01028e35`, `0x01028e3f`, `0x01028e47` e `0x01028e51` não aparecem em nenhuma das 84
tabelas, e nas cinco ocorrências que têm na NaND nenhuma tem a forma de entrada de
registro. São todas pool de literais — o firmware **pede** essas classes tanto quanto a
Z-Wheel.

**A boa: o firmware é um usuário enorme delas.** A `0x01028e2a` aparece quarenta vezes,
a `0x01028e3f` e a `0x01028e47` dezenove, a `0x01028e35` dezesseis. Cada uso é um call
site com a assinatura escrita, e ler call site é o que temos feito com a Z-Wheel — só
que aqui há muito mais código, e ele é da própria TecToy. A versão está gravada ali
perto: `RMUI Ver: V0.1.196`.

Duas confirmações já saíram disso, e as duas batem com o que tínhamos deduzido:

- Em `0x103587be`, o firmware chama `slot7(this, &{600, 40})` — o `DefinirTamanho`, com
  a forma que já usávamos.
- O `0x1035f222` é o ajustador de propriedade: `acessador(this, 0x801, id, valor)`, com
  a convenção de retorno invertida. Uma das chamadas é `(0x153, 0x10)`, exatamente o par
  que medimos saindo da Z-Wheel.

E saiu uma coisa nova: o irmão dele em `0x1035f23c` usa um **terceiro seletor**, o
`0x711`, na forma `acessador(this, 0x711, 0, valor)`. Era ele o "seletor de widget que
não conhecemos" das hipóteses. Continuamos recusando — o que ele quer dizer ainda não
sabemos —, mas agora o relatório diz o número em vez de dizer "um seletor".

O caminho para implementar, então, não é copiar uma vtable: é ler os call sites do
firmware, que são muitos e estão todos no `1.1.2_APPS.bin`.
