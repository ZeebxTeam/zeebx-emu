# 18 — A roda da Z-Wheel: da tela preta ao desenho

**Para quem é este documento:** quem está implementando a Z-Wheel (o shell/loja do Zeebo, App ID
274755, applet `0x01070798`) em **qualquer** emulador de Zeebo/BREW e está preso na tela preta.

Quase tudo aqui é **fato do lado do guest**: o que o `tectoy.mod` faz, em que endereço, e que
resposta ele exige para continuar. Isso vale independentemente da linguagem e da arquitetura do
emulador. Onde aparece uma decisão que é só nossa, o texto diz que é nossa.

O que **não** é transferível: nomes de arquivo e função do Zeebx. Estão reunidos no apêndice, no
fim, para não poluir o resto.

> Estado (11/09/2026): a roda sobe, o roller é montado, **desenhado a cada quadro e composto na
> tela**, e **gira com a tecla** — medido, ver §5.8. Ela ainda **não funciona por completo**: a
> seção 8 lista o que continua hipótese ou quebrado. Nada aqui é promessa de que o caminho inteiro
> está certo; é o registro do que tirou cada degrau.

---

## 1. Triagem rápida: a tela preta tem várias causas

Se a tela está preta, a primeira pergunta é **em que degrau o app parou**. Cada linha abaixo é um
sintoma que a gente viu, com a causa que estava por trás.

| Sintoma | Causa provável | Como confirmar |
|---|---|---|
| Para em `Could not create root form(20)` | a classe recusada é `0x01028e51`, **não** `0x01001011`; e o retorno dela é **invertido** | ver §3 |
| `SendEvent to get PrefsDB failed`, 11 vezes, e desiste | evento mandado para a própria classe **antes** do `EVT_APP_START` | ver §2.1 |
| `Invalid database version`, `Failed to init Delayed Queue database: 42` | `tt_dlqueue.db` aberto vazio, sem `DBINFO` | ver §2.4 |
| `Arithmetic exception: Divide By Zero` | o "passo de lista" respondido com zero | ver §6.3 |
| `Failure in call to CreateOwnerDrawWidget` | classe `0x01028e14` recusada | ver §4 |
| `Unable to create roller widget in MainMenu form` | **falta o slot 17 do widget** | ver §5.4 |
| Roda o dia todo, sem erro e sem desenhar, num pulso de ~10 s | um slot recusado abortou o callback **calado** | ver §5.1 e §4 |
| A tela mostra um retângulo pequeno no lugar dos 640×480 | a superfície do roller (214×34) sobrescreveu o bitmap da tela | ver §5.6 |
| O roller desenha **uma vez** e nunca mais; a barra de baixo fica vazia | o widget dele nasceu no endereço do bitmap da tela, reciclado depois de um `Release` a mais | ver §5.8 |
| O 3D sai deslocado / cortado na borda | o pbuffer foi criado com o tamanho da tela em vez do dos atributos | ver §4 |
| Monta tudo, mas a tela fica branca ou com a abertura por cima | você está pintando **todas** as raízes vivas | ver §6.1 |
| A roda não gira com nenhuma tecla | a tecla está sendo consumida pelo tratador errado, ou o código da tecla está errado | ver §7 |
| Trava dura, sem nenhuma chamada de API | tratador que desvia para si mesmo: você não devolveu o **anterior** | ver §5.5 |

---

## 2. Antes de sonhar com a roda

Nenhum destes itens é da interface, mas sem cada um a montagem para antes.

### 2.1 O evento que o applet manda para si mesmo antes de existir

A Z-Wheel manda o evento **`0x7b0a`** para a **própria classe**, pedindo o objeto do banco de
preferências, e a resposta volta escrita no `dwParam`. Isso acontece **durante a construção do
applet**, dentro do `CreateInstance` — antes de qualquer `EVT_APP_START`.

Se o teu `ISHELL_SendEvent` procura o applet numa variável que só é preenchida no start, o evento
volta "ninguém tratou" e o app imprime `SendEvent to get PrefsDB failed` onze vezes, depois desiste
da configuração inteira.

**Regra:** para o shell, o applet passa a existir quando é **registrado**, não quando é iniciado. O
`AEEApplet_New` já escreveu o ponteiro de saída antes de o código do jogo rodar — é de lá que se lê.
Isso não é remendo de Z-Wheel: vale para qualquer app que mande evento para si mesmo na construção.

### 2.2 SQLite de verdade

O `AEECLSID_SQLMGR` é SQLite mesmo, não um clone. O `tt_prefs.db` do pacote começa literalmente com
`SQLite format 3`, e as instruções que o módulo carrega em texto são o dialeto (`INSERT OR REPLACE`,
`COLLATE NOCASE`, JOIN entre `GAMEINFO` e `TITLETEXT`, `PRAGMA integrity_check`). Ligue uma
biblioteca SQLite e faça só a ponte: abrir o arquivo nomeado, executar a instrução e devolver as
linhas **uma chamada ao callback do jogo por linha, com os valores já em texto**, como o
`sqlite3_exec`.

O esquema que a interface mostra:

```sql
CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER,
                      dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT,
                      flags INTEGER, size INTEGER, unique(game_id, class_id));
CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT, unique(game_id, lang_id));
CREATE TABLE DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                        titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER);
CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER);
```

### 2.3 O catálogo precisa ser gravável

O `tt_game_info` do pacote é a fonte do catálogo oficial e **não deve ser modificado**. Trabalhe numa
cópia de perfil. Se você for injetar as ROMs locais como itens do carrossel, marque o que é seu (nós
usamos uma tabela própria, `ZEEBX_LIBRARY`, com `class_id`/`game_id`) para que uma nova varredura
apague só os seus registros, sem tocar no que veio do pacote.

### 2.4 `tt_dlqueue.db` chega vazio e precisa de esquema

O arquivo existe com 0 bytes. O `DLQueueDB.c` consulta `DBINFO` e falha com
`Invalid database version` / `Failed to init Delayed Queue database: 42`. Ao abrir esse banco, crie:

```sql
CREATE TABLE IF NOT EXISTS DBINFO(version INTEGER, subversion INTEGER);
INSERT OR IGNORE INTO DBINFO values (1, 0);
CREATE TABLE IF NOT EXISTS DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                                      titletext TEXT, boxart_path TEXT, flags INTEGER,
                                      upgrade_id INTEGER);
```

### 2.5 `preloaded.cfg` é estado do aparelho, não arquivo do pacote

A Z-Wheel faz `IFILEMGR_Test`, depois `GetInfo`, depois `OpenFile` em `preloaded.cfg`. Ele não vem no
pacote: era criado pelo console. Sem NAND, a resposta fiel é **ele existe e está vazio** — lista
vazia de jogos pré-instalados. Responder "não existe" para o `Test` desvia o fluxo.

Detalhe que morde: o tamanho do arquivo é lido em **`+0xc`** da `FileInfo` (medido em `0x89068`).

---

## 3. O retorno invertido — o erro que parece "classe faltando"

`Could not create root form(20)` engana duas vezes. O `20` é `EUNSUPPORTED`, e a classe recusada é a
**`0x01028e51`**, não a `0x01001011`. A mensagem não é referenciada por literal, é por `ADR`: a
`tectoymain.c:1001` sai em `0x7c478` e o `ISHELL_CreateInstance` logo acima carrega o literal de
`0x7c69c`.

Dessa classe o jogo usa **um método só**, o **slot 3**, um acessador genérico chamado por dois
invólucros que fixam o seletor:

| Invólucro | Seletor | O que faz |
|---|---|---|
| `0x3f72c` | `0x800` | **lê** o item de número `id` e escreve no terceiro argumento |
| `0x403c8` | `0x801` | **grava** o item `id` |

E o que decide tudo é o retorno: os invólucros fazem `cmp r0,#0; moveq r0,#3`. **Zero vira erro.**
Responder `SUCCESS` (que é zero no BREW) é responder "falhou" — e a `0x78acc` não trata esse
fracasso: pula para a limpeza e desreferencia o segundo filho, que nunca foi preenchido. Daí o acesso
inválido a zero em `0x78b70`, que **parece** o ponto da falha e é só o ramo de erro.

**Nesta classe, diferente de zero é sucesso.** `AddRef`/`Release` continuam devolvendo a contagem,
como em todo o BREW.

### 3.1 Os itens do acessador são tipados

O seletor `0x800` **não é "pega o filho"**: é "lê o item", e o tipo do que sai depende do número.

- Itens **`>= 0x5000` guardam objetos** (a `0x88338` lê o `0x5000` e o `0x5002` e usa os dois como
  widgets).
- Itens **abaixo de `0x5000` guardam números**. O callback da imagem lê o item `0x347`, **soma dois**
  e grava de volta; a `0x11668` lê o `0x414`, subtrai um e compara.

Se você criar um objeto para qualquer item lido, o jogo passa a somar dois num **ponteiro teu** e a
comparar ponteiro com número. O sintoma não se parece com a causa: a abertura gira para sempre
(vimos doze milhões de idas ao acessador e seis milhões de temporizadores numa execução).

Guardar um objeto num item `>= 0x5000` é **pendurá-lo**: é assim que o formulário recebe o que ele
mostra (a `0x8ed80` grava `0x5000` no formulário do z-pad). Tratar isso como simples "propriedade
numérica" parte a árvore em pedaços, e nenhuma regra sobre "qual é a tela atual" acerta depois.

### 3.2 A família de classes que compartilha esse acessador

São vizinhas de numeração e aparecem juntas numa tabela do firmware em `0x1035cf24`:

| Classe | O que é |
|---|---|
| `0x01028e51` | o widget raiz da interface |
| `0x01028e05` | pedida pelo palco; recusada, o `CreateStageWidget` desiste |
| `0x01028e14` | o `OwnerDrawWidget` do roller |
| `0x01028e19`, `0x01028e2a` | "frame widget" da `AnimationVideo_Form.c:93`; a `2a` é a que **põe texto** |
| `0x01028e26`, `0x01028e36` | acessador com cores e propriedades; sem a `36`, `Couldn't create z-pad instruction form (20)` |
| `0x01028e3f` | **container da barra inferior** — importante, ver §6.1 |
| `0x01028e47` | o **formulário**: tem tratador e pendura conteúdo no item `0x5000` |

Cuidado com um detalhe: **o mesmo número de slot quer dizer coisas diferentes conforme a classe.**
O slot 6 é "mostrar/esconder" na maioria da família, e na `0x01028e2a` é **definir texto**
(`slot6(this, texto AECHAR, tamanho, …)`).

Nenhuma dessas classes está registrada no dump do firmware — elas são de extensões (`widgets.mod`,
`forms.mod`, `framewidget.mod`) que não estão no sistema de arquivos. Tudo acima saiu de leitura do
código do jogo. A hipótese mais forte, que explica de uma vez o retorno invertido e o formato dos
slots, é que isto é o **framework de widgets do BREW 4.0**: o slot 3 seria `IWIDGET_HandleEvent` (que
devolve booleano "tratei"), o slot 4 `SetHandler`, o slot 7/5 `SetExtent`/`GetExtent`. Se o teu
amigo tiver acesso aos cabeçalhos `AEEWidget.h`, `AEEContainer.h`, `AEEForm.h` e `AEEModel.h`, isso
vale mais do que qualquer engenharia reversa nossa — foi assim que a `0x01001011` saiu de "formulário
raiz" para o nome certo, `ISourceUtil` (`AEESource.h`).

---

## 4. O palco (a roda 3D)

O `CreateStageWidget` monta o palco, e ele desenha em GL ES **num pbuffer**, não na janela.

| O que o jogo faz | O que exigir de você |
|---|---|
| `IWidget::slot16(this)` em `0x22d58`, logo depois de o palco existir | aceitar. Recusado, a execução **não volta** para `0x22d5c`: a montagem do menu para ali, e o app fica num pulso de 10 s lendo pontos e fila de download sem desenhar nada |
| cria a `0x01028e14` e depois a `0x01028e05` | atender as duas; recusar leva a roda de jogos junto |
| `eglChooseConfig`, `CreatePbufferSurface`, `CreateContext`, `MakeCurrent` | **o pbuffer tem o tamanho da lista de atributos** (640×330, a área do widget da roda), não o da tela. Com o tamanho errado, o cilindro passa da borda e o painel de baixo é cortado |
| `eglGetColorBufferQUALCOMM` | devolver o **endereço cru dos pixels**, em RGB565. Zero é falha: o `cmp r0,#0` logo depois desvia para `0x76e30` e o palco desiste. Não há saída de tamanho — o jogo já guarda largura/altura em `[r4+0x18]`/`[r4+0x1c]` |
| duas classes `0x01028e3c`, criadas na `tectoymain.c` e guardadas em `+0x354`/`+0x358` | ver abaixo |

A `0x01028e3c` tem dois métodos que **matam o ciclo em silêncio** se recusados:

- **`slot3(this, &saída)`**, chamado em `0x8588c` e `0x858a8`: dois objetos em sequência enchem
  pedaços da mesma estrutura, e o chamador sai fora se qualquer um devolver diferente de zero. Era
  **o fim do ciclo de atração** — 1722 callbacks abortados no primeiro segundo, e depois silêncio.
  Nós zeramos os `0x18` bytes entre os dois destinos (`r4+8` e `r4+0x20`) e devolvemos sucesso. É
  hipótese.
- **`slot5(this, &saída, 0, 0)`**, em `0x86238`: o chamador lê uma **meia palavra** e guarda em
  `[r5+0x10]`. Tem cara de medida. Respondemos zero — e zero já derrubou o jogo uma vez (§6.3), então
  se aparecer laço ou divisão por zero, olhe aqui primeiro.

Outras classes que a `tectoymain.c` exige, com o nome vindo da própria mensagem de erro:
`0x01006c05` (ZEEBOMCP), `0x01006c02` (controle de sistema: seu slot 6 é chamado em laço, e zero é
"siga"), `0x01011810` (`ICM`: o slot 28 recebe `(buffer, 0x340)` e o chamador compara `+0xc` com
**5**, que é `SYS_OPRT_MODE_ONLINE` — responda "o rádio está no ar"), `0x01001027` (`IConfig`),
`0x0100104f` (coleção genérica), `0x01028e35` (lista genérica).

**E uma que você deve recusar de propósito:** `0x01006c01`, o `LCT_SIMCardCtl`. A `0x78544` a cria
para pedir verificação do cartão; quando a criação **falha**, ela põe o estado em `0x27` — e `0x27` é
exatamente o que a `0x82464` encaminha para a transição que abre o menu principal. Implementá-la
quebra a transição. Nós mapeamos a classe num commit e tiramos no seguinte, por isso.

---

## 5. O roller inferior (o slider): os degraus que faltavam

Esta é a parte que resolveu a inicialização. Em ordem de descoberta.

### 5.1 Recusar um slot aborta o callback **calado**

Antes de qualquer coisa, uma lição de método: no nosso emulador, um slot não implementado virava
erro e o retorno de chamada do jogo morria inteiro **sem nada no log** — porque quem abortou fomos
nós, não o jogo. Foi preciso registrar o **desfecho de cada callback** para ver que uma linha
antiga do relatório (`I28e3c::slot[3]`) era quem matava a cadeia.

Se o teu amigo está com tela preta e log limpo, essa é a primeira instrumentação a ter: por callback,
"entrou / voltou / abortou, e por quê".

### 5.2 O slot 13 devolve uma superfície

Em `0x24100..0x24198` do `tectoy.mod`, o `tectoy_rollerwidget.c` chama o **slot 13 de um widget** com
`(&bitmap, largura, altura)` e em seguida faz `QueryInterface` no bitmap devolvido. Ou seja: o widget
é usado como se fosse fábrica de bitmap.

Responder só "sucesso" faz a montagem avançar e o roller ficar **sem superfície**. O certo é tratá-lo
como um **`CreateCompatibleBitmap`**: criar e devolver uma superfície do tamanho pedido. Feito isso,
aparecem as superfícies de **214×34** (roller) e **440×49** (abas).

> Isso só funciona se o teu `QueryInterface` de widget for permissivo (devolver `this` para qualquer
> IID). É o que fazemos, e está anotado como hipótese — é também a suspeita por trás de uma recursão
> antiga em `0x1177c`, porque um grafo auto-referente prende quem caminha procurando pai ou irmão.

### 5.3 O acessador é chamado com o endereço de um filho no lugar do seletor

Algumas classes chamam o slot 3 passando o **endereço de um widget filho** como seletor, para
consultar ou ligar o estado visual dele. Se você tratar isso como "seletor desconhecido" e devolver
zero, lembre-se do §3: zero é **falha** aqui. Reconheça o endereço e responda sucesso.

### 5.4 **O slot 17 — este é o bloqueio principal**

Em `0x23860` e `0x23d0c` do `tectoy.mod`, o roller faz `ldr r3, [r0, #68]`, isto é
**`pWidget->Slot17(0x8000, pFont)`**: associa o modelo de fonte devolvido pelo fallback (ou pelo
tratador de fonte) ao widget do roller.

`68 / 4 = 17`. Se a tua vtable de widget para no slot 16, essa chamada cai em "método não
implementado" e **a montagem do roller aborta** — é o `Unable to create roller widget in MainMenu
form`.

Duas coisas:

1. Aceitar já destrava. Com o slot aceito, o `StageWidget.c:256` passa, o carrossel carrega os **15
   itens**, e o roller e as abas são gerados.
2. **Não é uma notificação:** o módulo **solta `pFont`** logo depois de montar o roller. Quem associa
   precisa segurar uma referência própria (`AddRef` no modelo, `Release` no anterior, e soltar tudo
   quando o widget morrer). Sem isso o objeto de fonte morre e o endereço é reaproveitado por outro
   objeto — um bug que aparece longe daqui.

### 5.5 O slot 4 e o slot 16 **devolvem o anterior**

Os dois registram um callback passando um ponteiro para uma estrutura, e os dois **escrevem o
registro anterior de volta nessa mesma estrutura**. É assim que o BREW encadeia: quem se registra
guarda ali quem estava antes e desvia para lá o que não tratar.

- **Slot 4, tratador de eventos**: `slot4(this, &{função, contexto, liberador})`. O
  `ZPad_Keyboard_Instructions_Form.c` faz exatamente isso — a `0x8f560` lê `[contexto+0x14]` e faz
  salto de cauda para lá quando não trata a tecla. Sem escrever o anterior de volta, a estrutura
  continua descrevendo **o próprio** tratador que acabou de se registrar, e o desvio vira **recursão
  infinita**: 250 milhões de instruções num quadro só e a janela congelada, **sem erro nenhum no
  log** — porque não há erro, há um laço. Sem tratador anterior, devolva zero: a `0x8f564`
  reconhece.
- **Slot 16, retorno de desenho**: o `CreateOwnerDrawWidget` em `0x22cd0` monta o trio na própria
  estrutura (`[r4+0x14] = 0x5250c`, `[r4+0x18] = r4`, `[r4+0x1c] = 0x52574`) e passa `r4+0x14` em
  `r1`. Quem desenha é a `0x5250c`, e ela é **elo de corrente**: chama primeiro
  `[ctx+0x14]([ctx+0x18], r1, r2, r3)` — justamente onde o anterior precisa estar — e depois o
  desenho próprio do roller, em `[ctx+0x00]([ctx+0x08], …)`. Sem a devolução, ela chama a si mesma.

### 5.6 O bitmap da tela não pode voltar para a lista de livres

Sintoma: o roller cria a superfície de 214×34 e **é ela que aparece na janela** no lugar dos 640×480.

Causa: quem pede o bitmap da tela solta o que recebeu, como manda a convenção — mas o **dono** dele é
o display, não quem pediu. Solto, o endereço volta para a lista de livres e o
`CreateCompatibleBitmap` seguinte grava a superfície nova por cima da tela. O bitmap do display
precisa de uma referência sua, que nunca é solta.

**E uma referência permanente não basta**, como descobrimos depois: se qualquer caminho entregar
esse ponteiro **sem** contar — foi o caso do `GetDestination` aqui —, o jogo solta o que nunca foi
contado, a referência permanente é consumida e o endereço volta para a lista de livres assim mesmo.
Ver §5.8, que é a continuação desta.

O mesmo princípio vale em outro ponto: quando o jogo pendura a imagem num widget, ele **solta a
referência dela em seguida**. Se o widget não segurar, o objeto morre com a imagem decodificada
dentro e o que sobra para pintar é nada. **Quem guarda, segura; quem guarda, solta.**

### 5.7 Os eventos que o applet oferece ao root devem voltar "não tratei"

O applet oferece alguns eventos **primeiro ao widget raiz**, em `0x7b418`. Se o root responde
verdadeiro, o despacho **para em `0x7b420`**, antes dos tratadores do próprio applet — que são os que
preenchem as saídas de fonte e de recurso.

Nós respondíamos "tratei" e engolíamos o evento. Hoje `0x101`, `0x7b0a`, `0x7b0e` e `0x7b0f` voltam
**falso** pelo acessador do root, e o applet resolve. (Lembre que aqui "falso" é zero, e neste
acessador zero seria "falhou" para o invólucro — por isso a distinção importa: é o **valor do
booleano do evento**, não o código de erro da criação.)

### 5.8 O bitmap da tela não pode ser reciclado — era isto que apagava a barra

Este foi o último degrau, e é o mais transferível de todos, porque não tem nada de Z-Wheel: é
disciplina de contagem de referência no `IDisplay`.

**Sintoma:** o roller monta, desenha **uma vez** — a superfície de 214×34 sai com o item "Jogar"
renderizado — e nunca mais. A faixa de baixo da tela fica vazia, e nenhuma tecla muda um pixel
sequer.

**A cadeia, medida com a serial e o despejo de superfícies na mesma execução:**

1. `IDISPLAY_GetDestination` devolve o `IBitmap *` do destino. Pela convenção do BREW, **quem
   recebe solta** — e o jogo solta. Nós devolvíamos o ponteiro **sem `AddRef`**, enquanto o
   `GetDeviceBitmap` logo ao lado contava certo. Cada `GetDestination` tirava, portanto, uma
   referência que ninguém tinha posto.
2. Com isso o bitmap da tela **chega a zero**. O endereço volta para a lista de livres — mas a
   superfície continua viva no nosso mapa de bitmaps, e o campo que aponta "a tela" continua
   apontando para lá.
3. O objeto seguinte nasce nesse endereço. Na Z-Wheel, foi exatamente o `OwnerDrawWidget` do
   roller: `<criou 0x01028e14 -> 0x30000290 reuso true superfície-viva true é-a-tela true>`.
4. O roller registra o desenho dele nesse objeto (`Slot16`), e pouco depois o objeto é solto e
   **sai da tabela de widgets** — levando o registro de desenho junto. Daí em diante ninguém mais
   chama o desenho do roller, e a barra some.

**O conserto, nas duas pontas:** `GetDestination` passa a devolver **com contagem** (no `IDisplay`
e no `IGraphics`), e o `Release` do bitmap **não deixa o bitmap da tela chegar a zero** — o dono
dele é o display, não quem pediu. A trava vale mesmo com a contagem certa: é o endereço da tela que
não pode ser reciclado, e qualquer caminho novo que o entregue sem contar reabriria o buraco.

**O que mudou, medido** (Z-Wheel sozinha, 13 s virtuais, sem janela):

| | antes | depois |
|---|---|---|
| superfícies vivas | 7 | 9 (três de 440×49, as abas) |
| cores na tela | 1684 | 4097 |
| instruções em 1765 voltas | 207 M | 674 M — o roller passou a desenhar todo quadro |
| barra inferior | vazia | "Jogar / Ajuda" desenhados |
| uma tecla `0xe034` | **zero** bytes mudam | 25,4% dos pixels da tela mudam |

Com a tecla, o cilindro anda uma posição (`zeebo │ Obrigado │ GAMEVIL` → `Obrigado │ GAMEVIL │ EA`)
e o palco troca para a capa e o render do jogo selecionado. Duas execuções **sem** tecla saem byte a
byte idênticas, então a diferença é da tecla, e não de ruído — esse controle vale a pena rodar antes
de comemorar qualquer coisa.

**A regra que fica**, e que vale para qualquer emulador de BREW: *toda* função que entrega um objeto
por retorno ou por ponteiro de saída conta uma referência, e objetos que o sistema possui — a tela é
o exemplo — não podem morrer por contagem de quem os pediu emprestado. O preço de errar não é um
vazamento: é um endereço servindo a dois donos, e o sintoma aparece longe da causa.

---

## 6. Desenhar: onde a tela preta vira tela certa

**No console quem desenha a interface é a extensão de widgets, que ninguém tem.** Os widgets do
emulador guardam a árvore; alguém tem de decidir quando chamar o jogo para pintar e o que pintar por
conta própria. Esta seção é a parte mais "nossa" do documento — mas os sintomas se repetem em
qualquer implementação.

### 6.1 O jogo **nunca destrói** o que montou, e a barra não é filha do formulário

Dois fatos medidos que quebram as regras ingênuas de desenho:

1. **A Z-Wheel nunca esconde nada** e monta um formulário novo a cada volta do ciclo, sem soltar o
   anterior: **1722 raízes vivas em quinze segundos**, cada uma um formulário inteiro. Pintar todas
   empilha a tela de boas-vindas de 640×480 — que é a raiz mais **velha** — por cima de tudo. O
   console mostra **um** formulário por vez, e é o **último montado**.
2. **A barra inferior não é filha do formulário selecionado.** Ela vive num container da classe
   `0x01028e3f`, que é uma raiz solta. Filtrar o desenho "pela árvore do formulário atual" **apaga a
   barra** mesmo com ela visível. E como o shell remonta a barra a cada volta, incluir *todas* as
   instâncias ressuscita as árvores velhas: só a **mais nova com filhos** serve.

O mesmo vale para o menu principal: o formulário dele **não recebe** o conteúdo pelo item `0x5000`,
fica com zero filho, e o que ele mostra vira raiz solta (container `0x30000c10`, com a barra de
status e o palco dentro).

Nossa regra, depois de tudo isso: **desenho registrado é explícito** — quem registrou o slot 16 quer
ser chamado —, então chamamos o retorno de desenho de **todo widget visível com desenho registrado**,
sem filtrar por árvore. Para imagens e textos (que somos nós que pintamos), aí sim usamos a árvore do
formulário atual **mais** a subárvore do `0x01028e3f` mais novo.

### 6.2 Coordenadas, ordem e o custo de não deduplicar

- A posição vem do `AdicionarFilho` (slot 5), cujo terceiro argumento aponta para seis palavras:
  **`{x, y, sinalizador, largura, altura, objeto}`**. A abertura é pendurada com
  `{0, 0, 1, 640, 480, …}` e os pedaços do z-pad com coordenadas reais (`{100,21}`, `{148,20}`,
  `{365,20}`). Enquanto isso era ignorado, tudo era pintado na origem e a tela era um amontoado no
  canto. Largura e altura vêm **zeradas** quando o widget se mede sozinho — não grave zero por cima
  do que o slot 7 já disse.
- **O mesmo slot 5 atende cinco classes** e nem toda chamada tem essa forma: algumas passam função em
  `r2` e objeto em `r3`. Só leia a posição quando `r2 == 0`.
- A ordem de desenho é **filho por cima de pai** (profundidade na árvore), com a ordem de criação
  como desempate. Usar o endereço do objeto funciona até o alocador ganhar lista de livres — aí o
  fundo de 640×480 volta a cair por cima de tudo.
- Com milhares de árvores vivas, **o mesmo rótulo, no mesmo lugar, na mesma cor, aparece milhares de
  vezes**. Pintar todos dá exatamente o mesmo quadro e fazia um quadro levar mais de um minuto:
  deduplique por `(x, y, texto, cor)`.
- Ao trocar de formulário, **limpe a superfície**; ela é persistente. Mas limpe **só na troca**, não a
  cada quadro: quem desenha pelo `IDisplay` (a maioria dos jogos) não tem formulário nenhum e não
  pode ter a tela apagada por baixo.
- A cor do texto vem da propriedade **`0x140`**, no formato `RRGGBBAA` (a Z-Wheel grava `0x444444ff`,
  o cinza da tela de boas-vindas). O alfa é descartado: a superfície do console não tem canal.

### 6.3 Dois zeros que derrubam

- **O passo de lista.** O slot 5 com **`r2 == 4`** não é "pendurar filho", é um **getter**:
  `slot5(this, &saída, 4)`, e a saída são **duas meias-palavras**. A `0x8fa04` faz a chamada, soma as
  duas em `0x8fa20`, divide por elas em `0x10a18` e multiplica de volta — é o passo de uma lista, e a
  conta `(altura - 20) / passo * passo + 10` diz quantos itens cabem. Não escrever nada dá soma zero
  e **`Arithmetic exception: Divide By Zero`** pelo semihosting. Nós devolvemos a altura de linha da
  fonte; é hipótese, mas não pode ser zero.
- **O `GetExtent` do texto.** Nas classes de texto/imagem, o slot 5 com um ponteiro que não é widget
  nem imagem é `IWIDGET_GetExtent(&{cx, cy})`. A barra de status usa o `cx` do rótulo para posicionar
  os créditos: lê `cx` em `0x857ec` e soma 375 em `0x85828`. Deixar zerado escreve "10" em cima de
  "Meus Z-Credits".

E um ponteiro que não é ponteiro: quando a árvore cresce, o jogo chama o slot 7 (`SetExtent`) com
`r1` apontando para fora do mapa. Ler dali derruba o núcleo ARM. Ignore o que não dá para ler — um
tamanho que não veio é um tamanho que não muda.

---

## 7. A roda só gira se a tecla chegar ao tratador certo

- **Os códigos são `0xe033` (anterior) e `0xe034` (seguinte)**, medidos comparando o quadro com e sem
  cada candidata, com o desenho já determinístico. `0xe064` confirma. Pela numeração dos dígitos,
  `0xe033`/`0xe034` são o `3` e o `4`; **os `AVK_LEFT`/`AVK_RIGHT` (`0xe013`/`0xe014`) não aparecem
  no módulo** — se você mapeou as setas para eles, a roda não gira.
- **A tecla chega como `EVT_KEY` ao applet**, não pelo `IHID`. O formulário de abertura, por exemplo,
  só sai do lugar com `AVK_0` ou `AVK_CLR`, que botão nenhum do controle produz.
- **Ordem de despacho.** Primeiro os tratadores da tela atual, do mais novo para o mais velho, depois
  os de fora dela. O motivo é concreto: o formulário do z-pad traz a **`0x77300`**, um tratador que
  responde "tratei" para **qualquer** tecla. Enquanto ele vinha na frente, nenhuma tecla chegava ao
  menu.
- **Qual é a tela atual**, dado o §6.1: o mais novo entre "o formulário mais novo com filhos" e "a
  raiz mais nova com filhos". Escolher só entre formulários deixa o do z-pad como atual para sempre.
  E raiz **precisa ter filho**: o jogo cria widgets soltos que nunca recebem nada, e o mais novo de
  todos costuma ser um desses — pintar por ele dá tela branca.
- **Uma linha confusa no relatório, que não é o problema.** Mandada a tecla, o applet oferece o
  evento `0x100` ao acessador do widget raiz (`mov lr, pc; bx ip` em `0x7b414`, com `LR = 0x7b41c`) e
  testa `cmp r0,#0; bne` — não-zero quer dizer "já tratei, pare". Recusando, ele segue para o próprio
  `switch` em `0x7b424`, que compara o **evento** contra `0`, `1`, `2`, `3`, `8`, `0x110`, `0x400` e
  `0x4ec`: **`0x100` não está lá**, e o applet descarta. No console quem rotearia a tecla para o
  widget em foco é a extensão de interface. Aqui a roda gira mesmo assim porque **nós entregamos a
  tecla direto aos tratadores dos widgets da tela atual**, sem passar por esse roteamento. Vale saber
  que a linha `IWidget::Acessador seletor 0x100` no relatório é consequência desse desenho, e não a
  causa de tecla que não funciona — eu já tomei uma pela outra.

---

## 8. O que ainda não funciona

Honestidade sobre o estado: a roda sobe, desenha, compõe na tela e gira — mas boa parte do caminho
continua apoiada em hipótese registrada, não em leitura confirmada. O que **ainda não foi
verificado** depois do conserto de §5.8: se a rotação percorre os quinze itens e volta, se a
confirmação (`0xe064`) entra no jogo, e se a seleção da aba acompanha a roda.

- **Slots respondidos sem saber o que fazem**: `0x01028e3c` slots 3 e 5, o seletor `0x711` do
  acessador, o `slot16(this)` de `0x22d58`.
- **O passo de lista** é a altura da fonte. Passo errado erra o layout.
- **A fonte do slot 17 fica guardada e não é usada para desenhar.** Nós desenhamos com a `tectoy.ttf`
  que o próprio pacote traz, num corpo único, porque o **`fontsize.map`** que a Z-Wheel procura **não
  existe em lugar nenhum do dump** — nem no pacote dela. É ele que diria quantos pixels vale cada
  tamanho nomeado do BREW. (A classe de fonte que o roller pede é a **`0x0102f67c`**, não a
  `0x01035156`.)
- **A ligação de pai do formulário do menu nunca foi observada** — ela é inferida pela regra de "tela
  atual", não lida.
- **Árvores antigas nunca são destruídas**; os filtros "só o mais novo" e a deduplicação contornam,
  mas a contagem de objetos vivos cresce. Vaza rápido se `Release` não soltar os filhos guardados: em
  modo de atração, mil e vinte e quatro objetos acabavam numa volta do laço.
- **`QueryInterface` de widget aceita tudo.** O slot 13 depende disso, e é a suspeita por trás da
  recursão em `0x1177c`.
- **O tocador de animação** (slot 8 do widget) continua sem implementação; a abertura sai por atalho.
- **Pintura de imagens/textos é substituto declarado**, não emulação da extensão de widgets.

Duas lições que valem mais do que qualquer slot: **"o jogo andou" não prova que andou pelo caminho
certo** — uma tentativa nossa fez a Z-Wheel montar o menu e o log revelou
`Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **486.101 vezes**. E **um travamento é pior
que uma abertura estável**: desfaça a tentativa.

---

## Apêndice — onde isto está no Zeebx

Para quem trabalha neste repositório.

| O quê | Onde |
|---|---|
| Slots do widget (13, 16, 17, acessador, eventos do root) | [`machine/widget.rs`](../../src/machine/widget.rs), `widget_call` |
| Nomes dos slots | [`brew/aee_slots.rs`](../../src/brew/aee_slots.rs), `WIDGET` |
| Estado do widget (`desenho`, `modelos`, `tratador`, `serial`) | [`machine/mod.rs`](../../src/machine/mod.rs), `struct Widget` |
| Família de widgets, `0x0102f67c`, `0x01028e3c` | [`machine/mod.rs`](../../src/machine/mod.rs) |
| Árvore, formulário atual, barra `0x01028e3f` | `arvore_do_formulario`, `formulario_atual` |
| Partida da abertura e atalho do z-pad | `parte_animacao`, `skip_wheel_instructions` |
| Pbuffer e `GetColorBufferQUALCOMM` | [`machine/egl.rs`](../../src/machine/egl.rs) |
| Bancos de perfil e `tt_dlqueue.db` | [`machine/sql.rs`](../../src/machine/sql.rs), [`brew/sql.rs`](../../src/brew/sql.rs) |
| `preloaded.cfg` | [`machine/file.rs`](../../src/machine/file.rs) |
| Teclas da roda | [`input/mod.rs`](../../src/input/mod.rs), `avk::RODA_*` |
| Bitmap do display | [`machine/bitmap.rs`](../../src/machine/bitmap.rs), `device_bitmap` |
| Evento antes do start | [`machine/signal.rs`](../../src/machine/signal.rs), `send_applet_event` |

Commits, em ordem: `3ca627f` (slot 13 aceito, filho como seletor, eventos do root), `67a2305`
(tira o `SIMCARDCTL` de novo), `8ad6906` (desenho deixa de filtrar por árvore), `794ed80` (container
`0x01028e3f` entra na árvore), `c109a54` (`0x0102f67c` vira fonte), `d7a73d3` (slot 13 →
`CreateCompatibleBitmap`; eventos do root → falso), **`39de79a`** (**slot 17** e esquema do
`tt_dlqueue.db`: o roller monta), `ee63799` (biblioteca local no catálogo), `9e012f6`
(`preloaded.cfg` e `0x7b0e`).

Contexto mais amplo: [14-z-wheel-e-o-efs2.md](14-z-wheel-e-o-efs2.md) (o que veio antes, e por que a
NAND não resolve), [15-o-que-falta-da-nand.md](15-o-que-falta-da-nand.md),
[13-classes-desconhecidas.md](13-classes-desconhecidas.md) (a sonda, que foi como várias vtables
saíram).
