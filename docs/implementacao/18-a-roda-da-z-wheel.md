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

- **Os códigos são os do `AEEVCodes.h`**: `0xe030` `CLR`, `0xe031`/`0xe032` cima/baixo,
  `0xe033`/`0xe034` esquerda/direita, `0xe035` `SELECT`; `0xe064` é o confirmar do controle. A prova
  está no módulo: a `0x44914` é a tradução que a própria Z-Wheel faz do analógico em teclas, e leva
  cima, baixo, esquerda e direita em `0xe031` a `0xe034`; os tratadores de `0x11888` e `0x158c0`
  tratam `0xe030` igual a `0xe04a` e `0xe035` igual a `0xe064`. O direcional do controle manda as
  quatro setas.
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

## 7.1 Confirmar: o foco dos containers, e a lista de jogos que abre

Confirmar em "Jogar" não fazia nada. A cadeia que faltava, em ordem de descoberta:

| Degrau | O que é | Como se leu |
|---|---|---|
| seletor `0x711` | define o **filho em foco** de um container | grava um widget (`0x30000d90`) no container do roller |
| seletor `0x713` | **lê** o filho em foco | é feito no mesmo container, com `&saída`; `0x4eb5c` compara com `[[r4+0x24]+0xb0]` e só age se bater |
| seletor `0x700` | marca foco num widget, e com isso **no pai dele** | a grade nunca grava `0x711`; chama `0x700` no item (`0x399d0`) e pergunta o `0x713` ao pai do item |
| seletor `0x702` | "habilitado para foco", **um byte** | lido com `ldrb`; a ação e o `0x700` só acontecem se for verdadeiro. **Hipótese** |
| `aee_makepath` | junta diretório e arquivo | usado em duas passadas: mede, aloca, monta |
| `IVector` slot 7 | `ReplaceAt(índice, item)` | ordenamento por inserção em `0x38e5c`; é o slot do BREW entre `GetAt` e `InsertAt` |
| interface `0x0101e443` | canvas de um bitmap; o slot 7 entrega um `IDisplay` que desenha nele | o que sai é usado com `SetDestination`/`GetDestination`/`GetClipRect`; recusada, `0x41d68` desreferenciava nulo em `0x41dbc`. **Leitura pelo uso** |

Com isso, confirmar em "Jogar" abre o anel "Novos / Recentes" e a **grade de jogos da biblioteca
local**, com as capas.

### 7.2 Da grade ao lançamento

Na lista, cima e baixo passam as páginas e levam o foco entre a barra de abas e as capas;
esquerda e direita trocam a aba ou andam nas capas, conforme o foco. `0xe035` ou `0xe064` abre.
O caminho até o jogo, degrau
por degrau:

| Degrau | O que é |
|---|---|
| `0x01028e3c` = `IValueModel` | a grade faz `SetValue(modelo, item)`, e o ouvinte `0x37704` lê o item com `GetValue`. O slot 3 (`AddListener`) era uma "consulta" que zerava 0x18 bytes e **apagava função e contexto de todo ouvinte** |
| getter `0x713` com `AddRef` | o tratador da grade solta o que recebe; sem a referência, a contagem ia a zero |
| `IContainer` nos containers | slot 6 `Remove(filho)` e slot 7 `GetWidget(ref, bNext, bWrap)`, separados das leituras "visível" e "tamanho" pelos argumentos. Sem eles, `0x80490` rodava para sempre |
| `IROOTFORM_RemoveForm(raiz, FORM_LAST)` | slot 6 da raiz com `1`. Tira o formulário do topo **sem** `Release` e sem zerar o `pai` |
| `CanStartApplet` booleano | respondia `SUCCESS`, que é "não pode" |
| `EnumAppletInit`/`EnumNextApplet` | a Z-Wheel usa o nome do `.mif` (o id do módulo) do jogo escolhido |
| `ZeeboMCP` slots 3, 5 e 7 | `ModDataCopyFromENAND`, `ModDataRemoveFromMCP`, `UserDataCopyToENAND` — a cópia entre `fs:/card3/mod/` e `fs:/mcp/mod/`, que aqui não existe |
| `ObjectStore::add_ref` | não ressuscita objeto solto: duplicava o endereço na lista de livres, e o `IGraphics` do aviso de lançamento nasceu em cima de um bitmap |
| `EVT_WDG_MOVEFOCUS` (`0x711`) com `1` a `4` | `FIRST`, `LAST`, `NEXT`, `PREV`: o foco anda entre os filhos que aceitam foco (`0x702`), e quem sai e quem entra recebe `EVT_WDG_SETFOCUS` (`0x700`). É esse aviso que desenha a seleção. A tecla não vai mais a um ramo sem foco — sem isso a seta andava nas capas e trocava a aba junto |
| `IDISPLAY_DrawText` | honra `nChars`, o retângulo e os alinhamentos (`0x20` centro, `0x200` meio; conferidos no uso em `0x30434`). A barra de abas é um carrossel de rótulos de 214 px centralizados |
| `utf8towstr` | o terminador só entra se couber: a Z-Wheel passa o tamanho sem ele (`0x8afc8`), e reservá-lo comia a última letra de todo nome |
| BMP de 16 bits | as capas de nove jogos são 5-5-5 sem máscara |

Sem janela, `--instalados=0xCLSID:id` registra jogos instalados para testar o lançamento.

O que ainda falta nessa tela:

- **Abrir o jogo — resolvido.** O último degrau era o slot 3 do `ILCTSystemCtl` (`0x01006c02`): lê o
  modo pelo slot 6 e grava pelo 3 (modos 0 a 3, sinalizadores de hardware no firmware). Com ele, a
  Z-Wheel limpa os formulários, grava o marcador `ttgmrun.tmp`, arma o timer de 2 s para `0x81ebc` e
  chama `ISHELL_StartApplet` com a classe do jogo escolhido. Medido sem janela: com
  `bench roms/Z-Wheel.zip --seconds=43 --instalados=0x010a2337:279369
  --teclas=30500:0xe064,33000:0xe032,35000:0xe034,36500:0xe034,38000:0xe064` (abrir a lista, descer
  às capas, duas vezes à direita, confirmar), o relatório diz `lançar: o shell pediu para abrir
  0x010a2337` — o Alien Breaker. A bancada grava com `--dump` um quadro meio segundo depois de cada
  tecla, e aceita `--dump-surfaces`.
- **Na janela, escolher um jogo fechava a Z-Wheel e nada abria.** Três causas, achadas repetindo
  sem janela os toques gravados no relatório da janela (`zeebx sessao`, que usa a mesma `Session`, a
  mesma biblioteca e o mesmo mapeamento do controle):
  1. **A Z-Wheel lança saindo.** Ao confirmar, grava `ttgmrun.tmp` e `StringLastAppRan`, desmonta
     as telas e sai no timer de `0x81c38`. O console a reabre, e na partida (`0x81904`) ela vê o
     marcador e abre o jogo dois segundos depois. A janela agora reabre a Z-Wheel quando ela sai
     sozinha (`session::Z_WHEEL`). Um marcador deixado por uma execução que caiu é retomado na
     partida seguinte — é por isso que a Z-Wheel às vezes abria um jogo sozinha.
  2. **Widgets morriam sem os liberadores.** O trio `{função, contexto, liberador}` perdia a terceira
     palavra, o `RemoveForm` não soltava a referência da raiz e os filhos eram soltos só pela
     contagem. O roller da barra de abas seguia vivo com o timer de 40 ms (`0x2fc88`) armado sobre
     memória que a Z-Wheel já tinha solto, e o tique seguinte saltava para o endereço zero.
  3. **A região de superfícies não reciclava.** Capas e bitmaps de 440/441×49 por quadro esgotavam
     os 8 MB aos 27 s de navegação; daí em diante canvas e caixas de mensagem ficavam sem pixels.
- **O palco 3D saía cinza, e depois com riscos.** Dois defeitos em sequência:
  1. A Z-Wheel compõe o palco copiando o pbuffer para a tela com o `MEMMOVE` do helper, e a vigia de
     escrita só enxergava escrita do código ARM. As cópias pedidas ao helper (`memmove`, `memset`)
     agora marcam a faixa (`CpuBackend::marca_sujo`).
  2. O pbuffer tem 640×330, mas o rasterizador ficava em 640×480 e a Z-Wheel nunca chama
     `glViewport`. A exportação e a importação do `eglGetColorBufferQUALCOMM` passavam por escala e
     não voltavam às mesmas linhas: o fundo que o jogo copia a cada quadro não chegava a um terço
     delas, e a espada animada deixava rastros pontilhados. No `eglMakeCurrent` de uma superfície
     de outro tamanho que não o da tela, a superfície e a viewport do GL passam a ser as dela.
- **A roda da tela inicial tem dois itens nesta ROM, e está certo.** O `tectoy.cfg` traz `EOL=1` e
  `zeebomenu_hide=1`, que ligam os bits `0x2000` e `0x4000` de `app+0x3614`; a montagem em `0x4f620`
  tira os itens Zeebo e Configurações e troca o último por "Ajuda". Capturas com mais itens são de
  versões anteriores ao fim de vida.
- **Moldura de foco.** O `MOVEFOCUS` com um widget explícito (`0x711`) agora avisa quem sai e quem
  entra com `EVT_WDG_SETFOCUS`, pelo tratador do jogo. O do roller (`0x6089c`) grava o foco em
  `[+0xac]`, que é o que o `DrawRollerExt` testa para desenhar a moldura azul.
- **"Ajuda" abre, com placeholder.** A tela de FAQ precisa do widget de HTML (`0x0102dd32`). A classe
  entrou na família de widgets, e a montagem só passou quando a propriedade `0x161` do widget
  passou a devolver um objeto: a `0x31c34` faz nele um `QueryInterface(0x102d691)` sem conferir o
  zero, e abortava calada. Não há renderizador de HTML: o widget mostra o texto de
  `aparelho/z-wheel/ajuda.html`, criado com um texto padrão e editável à vontade (tags de bloco
  viram parágrafo, o resto some).
- **Formulário coberto não desenha.** Os widgets de desenho próprio eram chamados todos, e o palco
  do menu caía por cima da ajuda e aparecia atrás da lista de jogos. Quem pertence a um formulário
  que não é o do topo agora fica de fora; o que não está sob formulário, como a barra de status,
  segue.
- **Voltar é o botão 2**, como a ajuda da Z-Wheel diz, e manda `AVK_CLR` — medido na tela de ajuda.
- **Cor de fundo dos widgets.** A propriedade `0x130` é o fundo, em `RRGGBBAA` como a `0x140` do
  texto. A Z-Wheel grava alfa zero na maioria dos widgets e `0xd0d0d0ff` no container que cada
  formulário pendura em `0x5000`: é o cinza claro atrás do palco, da roda e da lista de jogos. Ele
  passou a ser pintado a cada quadro, antes das imagens, e isso também cobriu o que sobrava de um
  quadro para o outro — o quadrado cinza onde fica a seta de página da grade e os tracinhos azuis
  sob a barra de abas.
- **A moldura da capa só nas laterais está certa.** A imagem de seleção no `tectoyli.brf`
  (178×267) é isso mesmo: duas colunas azuis com o miolo transparente. A moldura da roda (214×47)
  também confere.
- **A tela de "Aguarde" no lançamento.** Ao sair para abrir um jogo, a Z-Wheel troca arquivos no
  próprio diretório (`0x81db0`): se existe `gamestartrgb.sav`, renomeia `zeebosplash.rgb565.raw`
  para `.sav` e o `gamestartrgb.sav` para `zeebosplash.rgb565.raw`. Na reabertura desfaz a troca no
  primeiro milissegundo (`0x822bc`). Quem lê o arquivo nesse intervalo é o console, ao reabri-la, e
  a imagem (640×480 RGB565) é "Aguarde enquanto o aplicativo é carregado". A Z-Wheel reaberta não
  desenha nos dois segundos até o `StartApplet`, e o jogo começa com a tela que ela deixou. A sessão
  agora pinta esse arquivo ao abrir a Z-Wheel, e o jogo aberto por ela herda a última tela
  (`Session::herda_tela`) — antes, os dois intervalos eram tela preta. No boot o arquivo é o
  "Bem-Vindo ao Zeebo", e a abertura da Z-Wheel desenha a própria versão por cima.

### 7.3 Roda inferior, abas, volta ao menu e transições

- **Os itens da roda inferior são imagens 214×34 dos recursos**, montadas em `0x4f620`: Jogar
  (`0x138f`), o logo zeebo (`0x138e`, no `tectoyli.brf`, se `0x4000` apagado), Comprar (`0x138d`,
  se `0x2000` apagado), Navegar (`0x13ef`, bit `1`, do `enable_browser`), Desligar (`0x13f0`, bit
  `0x40`) e, por último, Ajuda (`0x13f1`) com `0x2000` ou Configurar (`0x13a2`) sem. A leitura da
  cfg em `0x7fe80` confirma: `EOL` liga o `0x2000`, `zeebomenu_hide` liga o `0x4000`. A opção "Z-Wheel
  de fim de vida" (aba Geral) entrega à Z-Wheel uma cópia da `tectoy.cfg` no perfil do aparelho com os
  dois em zero.
- **O `SourceFromFile` relia o arquivo pelo nome.** A cfg é lida linha a linha por um `ISource`
  feito do `IFile`, e o `ISource` era criado resolvendo de novo o nome do guest — que apontava para a
  cfg do pacote, não para a cópia aberta. O `OpenFile` agora guarda o caminho real.
- **Moldura de seleção translúcida.** A imagem de seleção (`214×47`) tem o miolo azul com alfa 51. O
  PNG era reduzido a opaco/transparente pelo corte em 128; o alfa com meio-tom agora é guardado e
  misturado no desenho da imagem.
- **Voltar da ajuda travava o carrossel de cima.** O tratador do menu (`0x4e0e8`) lê o
  `SETPROPERTY(0x5064)`, o `FID_ACTIVE`: com zero para palco e roda (`0x4e3d0`), com um devolve foco,
  palco e som. A ajuda sai com `RemoveForm(raiz, formulário)` explícito, e o formulário que voltava ao
  topo nunca recebia o `1`. Os dois caminhos do `RemoveForm` agora avisam o novo topo, **na volta
  seguinte do laço** e só se ele ainda estiver no topo: ao lançar um jogo, a `0x78a94` tira o
  formulário de cima e a Z-Wheel desmonta o resto na mesma chamada. Avisado na hora, o menu se
  reativava no meio da desmontagem, a lista de jogos abria uma caixa de mensagem vazia e o jogo não
  lançava. Na janela, o pedido de lançar é conferido antes da saída da Z-Wheel, pela mesma razão: a
  reaberta pede o jogo e sai na mesma volta.
- **As abas mostram a página de cada uma.** Na troca de aba, a `0x3220c` abre o arquivo, faz dele um
  `ISource` e o entrega pelo slot 5 (`0x322e0`) do objeto da propriedade `0x161`. O conteúdo é guardado
  por widget e desenhado no lugar do placeholder, com entidades de acento decodificadas; sem página,
  vale o placeholder. As setas rolam o painel quando ele tem foco, como o widget de HTML do firmware
  faria — o container da ajuda grava o próprio widget de HTML (neto dele) como foco.
- **Transições.** A `0x798c4(app, tipo, 1)` decide se a troca desliza: tipo no `SlideOnceToForm` só
  desliza se ainda não estiver no `HasSlidToForm` das preferências, que ela marca na primeira vez. A
  cfg traz `31` e o `tt_prefs.db` do dump já vem com `14` (Jogar, Configurar, zeebo), então nada
  deslizava. A opção "Transições da Z-Wheel em toda troca de tela" (ligada por padrão) põe
  `SlideOnceToForm=0` na cópia da cfg. A animação em si:
  1. `0x74e20` fotografa a tela, cria um bitmap fora da tela, faz `SetDestination` para ele, solta a
     própria referência e empilha o formulário; arma 400 ms para `0x74434`.
  2. `0x74468` pega o destino (a tela nova), monta 640×1440 (antiga, fundo da `transitions.png` com o
     título, nova), volta o destino para a tela.
  3. `0x74258` desliza de 5 em 5 pixels **num laço só**, com `IDISPLAY_Update` a cada passo; no fim
     manda `0x7001` e reativa a raiz.
  Faltavam duas coisas: o `SetDestination` não segurava referência (o bitmap morria e o destino virava
  outro objeto — a transição parava no menu antigo para sempre), e as telas de cada `Update` dentro de
  um callback nunca chegavam à janela. A máquina agora guarda as telas de uma volta com mais de um
  `Update` e a sessão as mostra, uma por quadro, antes de o jogo seguir.
- **`ISHELL_CloseApplet`** anota o pedido; a sessão entrega o `EVT_APP_STOP` e termina como saída
  normal. Um jogo aberto pela Z-Wheel que sai assim devolve a janela à Z-Wheel.

## 7.5 A prova roteirizada do lançamento (22/09/2026)

O ciclo passou a fechar **na varredura**, sem janela e sem RetroArch, com um comando só:

```bash
ZEEBX_ROM="Z-Wheel (Brazil) (Es,Pt).zip" \
ZEEBX_ROM_MS=70000 ZEEBX_ROM_TETO=300 ZEEBX_ROM_INSTALADOS=auto \
ZEEBX_ROM_TECLAS=30500:k0xe064,33000:k0xe032,35000:k0xe034,36500:k0xe034,38000:k0xe064,45000:k0xe032,48000:k0xe064 \
cargo test --locked a_rom_indicada_avanca -- --nocapture
```

e o relatório responde com o desfecho que não engana — `Session::take_launch_request`, o mesmo
gancho que o core usa para trocar de sessão:

```text
estado: terminou sozinho
abertura pedida: 0x0108e356        (Alien Breaker Deluxe)
```

O roteiro faz o caminho inteiro: confirmar em "Jogar" (`0xe064`), descer às capas (`0xe032`),
andar duas capas à direita (`0xe034`), confirmar de novo. A grade aparece desenhada a partir de
37,6 s, com os jogos da biblioteca local em três colunas.

**Duas armadilhas do instrumento, medidas aqui:**

- **O `id` do `--instalados` é obrigatório**, e é o **número da pasta do módulo** dentro do pacote
  (`fs:/mod/N`). Sem ele a Z-Wheel registra `Tectoy.c:2925 No mod number for this game!!!` e
  desiste — foi o que aconteceu com a lista de classes sem id. É o que `ZEEBX_ROM_INSTALADOS=auto`
  resolve: varre a pasta da ROM e monta os pares `0xCLSID:id` dos pacotes.
- **O cache é estado do jogo.** A Z-Wheel grava `zeeboprefs.dat` e `ttgmrun.tmp` **dentro do pacote
  extraído**, e no boot seguinte reage a eles — lançando o último jogo em 2 s, sem passar pelo
  roteiro. Medir o caminho fresco exige limpar `~/.config/zeebx/cache` **e** o perfil do aparelho
  antes de cada execução; sem isso o mesmo comando dá resultados diferentes em execuções seguidas.

**O que ainda não fecha, medido:** a Z-Wheel registra `check_malloc: Malloc failed in Tectoy.c at
line 570` depois de alocar ~64 MiB no próprio pool (o `memcheck` dela imprime
`Free(67001392)`), e sai. O pedido que falha **acompanha o tamanho do heap** — 67 001 488 com
64 MB, 134 110 352 com 128 MB —, então aumentar o heap só move o alvo: ela pede o que sobra, e
não uma necessidade própria. O que ficou dessa medição é a contabilidade honesta do "quanto há
livre" (`Heap::maior_bloco`), que antes somava buracos com o que resta à frente e respondia um
número que nenhuma alocação consegue.

### 7.6 O widget proprietário `0x01028e19`, medido pelo uso

A família dos widgets é atendida em bloco (por vizinhança de numeração), e o que distingue uma
classe da outra não está em header nenhum. O censo do acessador por classe
(`ZEEBX_ROM_SELETORES=1`) responde **pelo uso**, e na Z-Wheel a `0x01028e19` — a única do trio
`0x01028e19` / `0x01028e2a` / `0x01028e4b` que este roteiro cria — recebe dois seletores, e só:

```text
classe 0x01028e19  seletor 0x800  (5x)
classe 0x01028e19  seletor 0x801  (10x)
```

`0x800` é leitura de item e `0x801` a gravação; os itens são **tipados**, com o corte medido em
`0x5000` — de lá para cima o item guarda objeto, abaixo guarda número. É o mesmo caminho que a
família inteira usa, então neste ciclo **o que a classe proprietária pede é o comportamento
genérico** — e é o que o emulador faz.

**O que não se reproduz, e fica registrado:**

1. **O desenho próprio da classe.** Pintamos imagens e textos por substituto declarado, e não
   emulando a extensão de widgets — o que a classe faria de diferente ao desenhar não é medido
   aqui.
2. **Qualquer comportamento dela fora dos caminhos que este roteiro exercita.** O censo mede o
   *uso*; ausência de uso não é ausência de comportamento.

### 7.7 O core não traduzia o controle em teclas do console

`input::teclas_do_controle` existe desde sempre e é o que transforma o **controle** nas **teclas do
BREW** que os aplicativos leem (`0xe031` a `0xe034` para o direcional, `0xe064` para o botão 1). A
janela e o desktop a usam. O core Libretro **nunca a chamava** — entregava o controle pelo
`set_port_pad` e parava ali. Medido em 22/09/2026, pelo caminho que o RetroArch usa:

```text
sem a tradução   a roda anima para sempre: 13 a 14 imagens distintas em cada 24 quadros,
                 em todos os passos do roteiro, e nenhuma tecla é tratada
com a tradução   55 imagens distintas nos 100 quadros antes da tecla (animando) e
                 1 imagem distinta nos 100 quadros depois do confirmar (tratada)
```

É a diferença entre "o controle chega ao guest" — que estava medido — e "o controle chega ao
applet do jeito que ele lê". No aparelho isso é o RetroArch entregando o controle a um aplicativo
que só entende teclas do console.

**O que este caminho ainda não fecha, medido.** Com as **mesmas teclas** e a mesma máquina, a
varredura pede (`abertura pedida: 0x0108e356`) e o core não. Três medições estreitaram onde está a
diferença, e nenhuma delas é a tecla:

1. **Não é paciência.** Depois do roteiro, mais 6 000 quadros (100 s de relógio virtual) e o pedido
   continua zero: a roda fica **presa** depois do confirmar (1 imagem distinta em 100 quadros), não
   lenta.
2. **O compasso não é a contagem de quadros.** Medido com um relógio de teste: cada `retro_run`
   avança **~26 ms** de relógio virtual, e não os 16 ms de um quadro a 60 Hz, porque a volta do
   core termina quando a máquina **apresenta** um quadro. Contando quadros, o roteiro começava aos
   **50,6 s** em vez dos 30,5 s do roteiro da varredura — outro ponto da linha do tempo, outra
   tela, outro desfecho.
3. **E o mesmo instante não é o mesmo estado.** Conduzindo pelo relógio, aos **30,5 s** a roda no
   caminho do core está **parada** (1 imagem distinta em 100 quadros) enquanto no da varredura
   anima. Ou seja: os dois caminhos não estão no mesmo ponto da linha do tempo quando o relógio
   marca o mesmo número — e é isso que o próximo experimento tem de separar (o que a varredura
   apresenta que o core não apresenta, ou o contrário).

Os instrumentos ficam no teste: `CLASSE_ATUAL` (quem roda) e `RELOGIO` (em que instante), e os
mesmos instantes do roteiro da varredura (30 500 / 33 000 / 35 000 / 36 500 / 38 000 ms) valem para
os dois caminhos.

**4. O relógio virtual para, e é isto que prende a roda.** Medido com um roteiro que tecla de dois
em dois segundos e meio entre 25 s e 75 s: **517 036 quadros entregues ao frontend com o relógio
virtual em ~25,4 s** — 426 ms de tempo virtual em meio milhão de quadros. O `run_frame` fecha cada
volta quando a máquina apresenta um quadro *ou* quando gasta `MAX_STEPS_PER_FRAME` (200 000) voltas;
com o guest num laço que não avança o relógio, o orçamento de voltas acaba **antes** do próximo
temporizador vencer, e o relógio — que é quem dispara os temporizadores da roda — nunca anda. A
roda não fica lenta: ela para.

**5. A soltura da tecla não é o fator.** No roteiro da varredura a tecla é **presa e nunca solta**
(não há passo `Solto` no roteiro que pede a abertura, e o `Passo::Tecla` empurra só `(avk, true)`),
enquanto no caminho do core o botão era apertado e solto 300 ms depois. Mantendo o botão preso no
teste do core, o desfecho é **o mesmo**: 1 imagem distinta em 100 quadros e pedido zero. A
diferença não está no par aperta/solta.

**6. Com a captura de serial no core, a pergunta virou o foco.** O core ganhou o mesmo
instrumento da varredura (`ZEEBX_CORE_SERIAL`), e ele mostra o que a roda faz por dentro nos dois
caminhos. A diferença está numa consulta:

```text
varredura   [  30508 ms] ... AND GAMEINFO.class_id = -1 -> 0 linha(s)
            [  38022 ms] ... AND GAMEINFO.class_id = 17359702 -> 2 linha(s)   <- o foco, e o pedido sai
core        [  50611 ms] ... AND GAMEINFO.class_id = -1 -> 0 linha(s)
            [  53033 ms] ... AND GAMEINFO.class_id = -1 -> 0 linha(s)        <- a tecla é tratada, o foco não vem
```

`17359702` é `0x0108E356`, o Alien Breaker — o mesmo que a varredura pede para abrir. No core a
tecla **é tratada** (a roda cria o `IValueModel` em `0x01028e3c`, pendura o `0x5000` no formulário e
liga o `FID_ACTIVE`), mas o foco nunca vira um jogo: a consulta continua com `-1`. É esta a
pergunta do próximo experimento — **de onde a roda tira a classe do item em foco, e por que ela sai
`-1` no caminho do core** —, e agora há como medi-la, porque o instrumento existe nos dois lados.

**7. A árvore de widgets é a mesma, e o primeiro confirmar faz o mesmo — a divergência está depois.**
Comparando as duas capturas no instante da primeira tecla, as árvores são **idênticas**, linha por
linha: mesmos endereços, mesmas classes, mesmos tratadores. O `0x300007d0` que a §7.6 mede
(`classe 0x1028e19`, 39×25 em (100,21), dois filhos) está nos dois, pendurado na barra
`0x1028e3f`, cujo tratador é a `0x77300` — a que responde "tratei" para qualquer tecla.

E o **primeiro** confirmar faz o mesmo nos dois: `IWeb` (`0x0100550d`), display, e
`prop 0x5064=0x1` (o `FID_ACTIVE`) — na varredura aos 30 910 ms, no core aos 53 434 ms.

A divergência aparece **na tecla seguinte**: na varredura, a seta para baixo (33 023 ms) e a da
direita (35 023 ms) produzem atividade (`IWeb` + display, uma vez cada); no core, as teclas
seguintes não produzem **nada** na captura. Mesma árvore, mesmo primeiro efeito, e a segunda tecla
se perde — o que aponta para a **entrega do evento**, e não para o estado da tela. É a única
variável que difere entre os dois caminhos: lá a tecla entra como tecla do console, aqui ela é
traduzida do controle.

**8. E só **uma** tecla chega à máquina.** O instrumento que faltava — a linha da entrega, no
próprio despacho — respondeu de imediato. Na execução inteira do caminho do core, a máquina recebeu
**um único par**:

```text
[36612 ms] <tecla 0xe064 aperta para 2 tratador(es) na tela e 0 fora, tratada>
[36771 ms] <tecla 0xe064 solta  para 2 tratador(es) na tela e 4 fora, tratada>
```

As **sete teclas seguintes do roteiro não chegam à máquina** — nem como aperta, nem como solta. Isso
não é o applet ignorando: é o evento não chegando. Com a primeira tecla tratada e as demais ausentes,
a pergunta muda de lado: ou a **tradução do controle** não produz as bordas seguintes, ou a **fila**
da máquina não é esvaziada depois desse despacho. É o próximo experimento, e ele é de código, não de
comportamento do applet — a instrumentação para ele já está na captura.

**9. E só uma tecla chega, em três variações.** O mesmo instrumento, rodado nos dois caminhos,
compara o que entra na máquina:

```text
varredura  [30510] 0xe064 aperta   [33023] 0xe032 aperta   [35023] 0xe034 aperta
           [36508] 0xe034 aperta   [38024] 0xe064 aperta      <- as cinco do roteiro, todas tratadas
core       [36612] 0xe064 aperta   [36771] 0xe064 solta      <- um único par, e mais nada
```

E o fato se sustenta em três variações, todas medidas: botões diferentes, o **mesmo** botão quatro
vezes, e um aperto de **120 quadros** (contra 8) — em todas, a máquina recebe **uma** tecla e as
seguintes não aparecem, nem como aperta nem como solta. Na varredura, a captura mostra as cinco, uma
a uma, cada uma tratada por dois tratadores.

Isso não é o applet: é a **entrega**. Depois da primeira tecla o caminho do core não leva mais
nenhuma à máquina, e é essa a pergunta do próximo experimento — com o instrumento já nos dois lados
(a linha da entrega na captura) e o roteiro da varredura como referência do que deveria aparecer.

**10. O pad muda, a entrega não chega — e há um atalho que consome setas.** Medindo direto a
tradução (impressão temporária do que `le_pad` devolve por quadro), o pad muda **corretamente** em
todos os passos:

```text
0x0 -> 0x1       aperta o botão 1 (o confirmar)
0x1 -> 0x1  ...  segura
0x1 -> 0x0       solta
0x0 -> 0x4000    aperta o direcional para baixo
```

Ou seja: a leitura do controle está certa e as bordas existem. O que não acontece é a **entrega** —
a captura registra uma tecla e as outras não aparecem. E no despacho das teclas há um caminho que
**consome as setas antes de entregá-las**: o rolamento do HTML em foco, que devolve "tratei" para
cima/baixo quando há um widget de HTML com foco, e sai do laço antes de chamar tratador nenhum.
Também merece olhar: o `continue` dele não passa pela linha da captura, então uma seta consumida ali
**não deixa rastro** — o que faz "a tecla não chegou" e "a tecla foi consumida pelo rolamento" se
parecerem.

**11. O que ficou refutado, para não repetir o caminho.** Duas hipóteses caíram por medição, e as
duas eram boas:

- **"a sessão parou depois da primeira tecla, e por isso a fila não é esvaziada"** — refutada: a
  máquina continua executando **84 360 instruções por quadro** depois do confirmar. Sessão parada não
  executa nada.
- **"o atalho do rolamento do HTML come as setas"** — insuficiente: ele só consome **cima e baixo**,
  e a seta da **direita** também não chega à captura.

O que sobra, medido: o pad **muda** em todo passo (`0x0->0x1`, `0x1->0x0`, `0x0->0x4000`), as
bordas existem, `Session::set_key` delega para a fila da máquina sem intermediário — e a captura
registra **uma** tecla. Entre "a fila recebeu" e "o despacho entregou" não há mais nada no caminho
além dessas duas linhas. O próximo experimento é instrumentar exatamente esses dois pontos, num
commit só: uma linha em `Session::set_key` (entrou na fila) e a linha que já existe em `flush_keys`
movida para o **topo** do laço (saiu da fila, antes de qualquer atalho consumir).

**12. O quadro que fecha, com dois fatos que ainda não encaixam.** Três medições da mesma execução:

- o pad **muda** em todos os sete passos (`0x1`, `0x4000`, `0x8000`, `0x8000`, `0x1`, `0x1`, `0x4000`);
- a tradução **funciona no crate do core** — provado com pads construídos à mão:
  `tradução de b1: [(57444, true)]`, que é `0xe064`, o confirmar;
- e a captura registra **um** par de teclas, com o relógio virtual **parando em 37 012 ms** depois do
  confirmar.

O que **encaixa**: com o relógio parado, o guest deixou de chegar às fronteiras de API, e é nelas
que a fila de teclas é esvaziada — as teclas seguintes ficam **presas na fila**, e é por isso que a
captura não as mostra. Isso explica o sintoma inteiro (tela parada, teclas sem efeito) sem precisar de
nenhum defeito novo.

O que **não encaixa ainda**, e é o próximo experimento: o guest executa 84 360 instruções por quadro
depois do confirmar, e uma volta com esse trabalho deveria voltar de `advance` e passar por
`deliver_signals`. Se ele não passa, a pergunta é **onde a volta fica presa** — e a medição é uma
linha em `advance_once_inner`, dizendo se `deliver_signals` roda depois do confirmar.

### 7.8 A causa estava no core: as telas intermediárias do `Update` não eram consumidas

Quando o guest chama `IDISPLAY_Update` **dentro de um callback** — e a transição da Z-Wheel faz isso
num laço síncrono, umas duzentas vezes —, a máquina guarda uma tela por chamada
(`quadros_do_update`). Quem as mostra é o frontend, **uma por quadro**, sem avançar o relógio: a
janela as consome (`ui/mod.rs`) e a sessão sem janela também (`main.rs`). **O core Libretro não as
consumia.**

Com a fila cheia, `advance_once` devolvia "apresentou" **para sempre** — sem rodar o guest, sem
fronteira de API, sem esvaziar a fila de teclas. Medido, na Z-Wheel pelo caminho do core:

```text
antes    relógio parado em 37 012 ms · 1 par de teclas em toda a execução · pedido de abertura 0x0
depois   relógio chega a 78 983 ms  · as 8 teclas do roteiro, uma a uma, cada uma tratada
```

E o ciclo fecha, no caminho que o RetroArch usa:

```text
passo 1: abertura 0x0108e356, 11 assinatura(s) distinta(s)
depois da tecla: 11 imagens distintas em 100 quadros, relógio 566 ms   <- a sessão nova, o jogo rodando
pedido de abertura no caminho do core: 0x0108e356
6886 quadros, 19 088 154 instruções em 61 814 ms virtuais
```

`0x0108E356` é o **Alien Breaker**, o mesmo jogo que a varredura pede — e o relógio em 566 ms é a
sessão do jogo, que começou do zero: o core **abriu e executou**. A volta à roda fica medida e não
cobrada: depende de o jogo terminar sozinho, e o que a grade põe em foco é o primeiro título da
pasta.

**A volta à roda, medida e ainda não provada.** O roteiro do teste do core virou configurável
(`ZEEBX_CORE_TECLAS=ms:id`, com o `id` do RetroPad — `1` é o `Y`, o confirmar), para a prova poder
ser roteirizada como na varredura. Com ele, a tentativa óbvia — uma pasta com a Z-Wheel e **um jogo
que termina sozinho** (o `Zeebo Clube`, medido na varredura) — **não lança**: a roda reage ao
confirmar (50 imagens distintas) e o pedido fica em zero. A grade depende do **conteúdo** para pôr o
foco, e com dois títulos o roteiro de navegação tira o foco do único que existe.

Com a pasta cheia (62 jogos), o roteiro da doc lança e executa (o `0x0108E356`), e a volta fica em
`false` porque **o jogo em foco — o Alien Breaker, o primeiro em ordem depois da navegação — não sai
sozinho**. Fica medido e registrado: falta um roteiro que abra, na pasta cheia, um dos dois títulos
que terminam sozinhos.

**A volta à roda: o que a medição delimitou.** Três pastas, o mesmo teste, o mesmo roteiro
padrão — e só uma lança:

```text
62 jogos (a pasta cheia)   lança e executa (0x0108E356); a volta fica em false porque o jogo em
                           foco nao sai sozinho
3 jogos (Action Hero 3D, Alice, Zeebo Clube)   nao lanca: a primeira tecla produz 50 imagens
                           distintas (uma transicao) e o pedido fica em zero
2 jogos (Zeebo Clube)       nao lanca, com o mesmo sintoma
```

Ou seja: **a grade só põe foco alcançável com catálogo grande**, e é isso que a navegação do roteiro
precisa para escolher — e não a quantidade de títulos em si, porque a lista é montada pelo catálogo.
A volta à roda fica, portanto, dependente de duas coisas ao mesmo tempo: um catálogo onde o roteiro
ache o foco, e um jogo que **saia sozinho** nesse foco. O `Zeebo Clube` e o `Zeebo App` são os dois
que saem (medidos na varredura), e nenhum dos dois é alcançado pelo roteiro na pasta cheia.

Quatro tentativas, e todas com o mesmo desfecho — o lançamento **só acontece com a pasta cheia**:

```text
2 jogos, roteiro da doc                     nao lanca
3 jogos (Action Hero 3D, Alice, Zeebo Clube) nao lanca
3 jogos com um titulo que PARA NA HORA       nao lanca
4 jogos, instantes da varredura (30,5 a 38 s) nao lanca
```

A terceira foi a mais promissora: o `Zeebo Channels - Opera Mini` **não cria o applet** e a sessão
para logo, então a volta à roda seria exercitada de verdade. E o título da grade vem do **nome do
pacote** (é o que a varredura grava no catálogo), então renomeei a cópia para `Alien Z` para ela cair
na terceira posição alfabética — que é onde o roteiro põe o foco na pasta cheia. Mesmo assim, sem
lançamento.

O que isso delimita: **não é o título nem o número de arquivos** — é o que a grade monta. Com poucos
títulos ela não oferece o foco que o roteiro alcança, e com muitos ela oferece, mas aí o foco cai num
jogo que não sai sozinho. As duas condições precisam valer ao mesmo tempo, e é isso que falta montar.

**A barra pergunta à lista todo quadro, e nós respondemos zero.** Na captura, o par `(barra,
endereço da lista)` aparece **14 319 vezes**, uma a cada 5 ms — um laço de consulta da barra
`0x01028e3f` ao roller `0x30000ad0`, com o **endereço do filho no lugar do seletor**. É o caso que a
§5.3 já conhece ("o acessador é chamado com o endereço de um filho no lugar do seletor"), e a nossa
resposta é o `OK` seco daquele ramo.

O que faz disso a próxima pista: **é uma pergunta repetida**, e o que a barra faz com a resposta não
está medido. Se ela pergunta a cada quadro para saber *qual item está escolhido* — ou *quantos* a
lista tem —, o zero que devolvemos é a diferença entre a grade escolher e não escolher.

**Onde está a lista de jogos, medido na árvore.** A captura de serial despeja a árvore de widgets na
primeira tecla, e nela a roda tem **dois** rollers (classe `0x01028e14`):

```text
0x30000ad0  tratador 0x75724  640x330 em (0,50)     <- a LISTA de jogos
0x30000cd0  tratador 0x6089c  440x49 em (100,405)   <- a roda de baixo (Jogar, Comprar, ...)
```

O de baixo é o que a doc §7.3 já conhece (é o `0x6089c` que desenha a moldura azul ao receber
`SETFOCUS`). O de cima, `0x30000ad0`, com `640x330` — a área inteira da grade — e tratador `0x75724`,
é a **lista de jogos**: é nele que vive a escolha que o confirmar transforma no `class_id` do
lançamento.

É o próximo experimento, e agora com endereço e tratador na mão: registrar o que o `0x30000ad0`
recebe enquanto o roteiro navega (o censo já mostra a classe `0x01028e14` recebendo `0x801` dez vezes)
e o que ele faz no confirmar.

**A hipótese do último jogado caiu.** Zerando o cache (onde a roda grava o estado dela, dentro do
pacote extraído) o lançamento continua a ser `0x0108E356` — o mesmo Alien Breaker. A escolha é
**determinística** e não depende do estado que as execuções anteriores deixaram.

O que continua de pé, medido: a consulta da grade devolve 120 linhas fundindo oficiais e locais, e o
jogo que o roteiro alcança é o mesmo com ou sem a cópia renomeada que eu inseri. Ou seja, o roteiro
chega a um **jogo**, e não a uma **posição** da lista — o que só fecha com uma leitura da própria
grade, que fica como próximo experimento.

**A lista é fundida, e a ordem não é a alfabética.** Medido no catálogo que a roda abre: **59
títulos oficiais** (curtos — `Alice`, `Alien Breaker`) mais **61 locais** (o nome do pacote —
`Alice no Pais das Maravilhas (Brazil) (Es,Pt)`), **120 linhas** na consulta da grade. Ordenados por
título, os seis primeiros são:

```text
Action Hero 3D                                  (oficial)
Action Hero 3D - Wild Dog & IMICRO3D (...)      (local)
Alice                                           (oficial)
Alice no Pais das Maravilhas (...)              (local)
Alien A (Brazil) (Es,Pt)                        (a copia quebrada, renomeada por mim)
Alien Breaker                                   (oficial)
```

O lançamento, mesmo assim, foi `0x0108E356` — **o Alien Breaker**, o sexto. Logo o foco da grade
**não é a terceira linha da lista fundida**. O que sobra como explicação medida é a ordem da própria
roda: ela guarda `dt_lastplayed` e tem a aba "Novos / Recentes", e `Alien Breaker` é exatamente o
jogo que **estas execuções vêm abrindo** — o mais recente é o primeiro.

Quinta tentativa, com a pasta **cheia** e o título quebrado encomendado para a terceira posição:
ligações simbólicas para os 62 pacotes (sem duplicar 705 MB) mais uma cópia real do `Opera Mini`
renomeada para `Alien A`, que ordena **antes** do `Alien Breaker`. Resultado: o pedido continua a ser
`0x0108E356` — **o Alien Breaker**, não o `Alien A`. O foco da grade **não segue a ordem de título**
que eu supus: o `class_id` do relatório (`17359702`) é o que a grade escolhe por conta própria.

Isso fecha o que dá para medir sem entrar na grade: lançamento e execução provados nos dois caminhos,
e a **volta** dependendo de um item em foco que saia sozinho — com a grade não obedecendo à ordem que
se vê de fora.

É o próximo experimento, com o alvo já estreito: **descobrir como a grade escolhe o item em foco na
pasta cheia** (o relatório diz `class_id = 17359702`, o terceiro em ordem de título) e reproduzir essa
condição numa pasta onde o terceiro seja um título que sai — ou aceitar, com a medição na mão, que a
volta à roda fica demonstrada só no aparelho.

**13. O armazenamento muda o instante, e não a prisão.** O caminho da varredura usa o armazenamento
padrão (a config do usuário) e o do core usa o que o frontend entrega; apontando o teste do core
para a árvore do usuário (`ZEEBX_CORE_SISTEMA=$HOME/.config/zeebx`), o mesmo número de quadros leva
a roda a **36,6 s** em vez de **50,6 s** — o estado do aparelho muda quando as coisas acontecem, o
que confirma que os dois caminhos não se comparam por relógio. Mas a prisão depois do confirmar
continua igual (1 imagem distinta em 100 quadros, pedido zero). Não é artefato da pasta.

A conta, com os números que saíram (o teste ganhou `INSTRUCOES` ao lado do `RELOGIO`; o acesso
`Session::instrucoes` já existia):

```text
antes da tecla    1 060 495 350 instruções · 55 088 ms virtuais · 55 imagens distintas em 100 quadros
depois da tecla   1 068 931 395 instruções · 51 012 ms virtuais ·  1 imagem distinta em 100 quadros
                  -> +8 436 045 instruções em 100 quadros = 84 360 por quadro, e 4,2 ms de relógio
```

**A roda não está presa nem ociosa: ela roda e não apresenta.** 84 mil instruções por quadro é
trabalho de verdade (o mesmo patamar da varredura, ~17 milhões por segundo virtual), e o relógio
anda 4,2 ms por quadro em vez dos 16,67 de um quadro cheio — porque a volta termina antes, com a
máquina devolvendo o controle sem trocar o buffer. O que ela faz nesse estado é **esperar um
evento**, e é isso que as teclas do RetroPad não produzem no caminho do core — e que o roteiro da
varredura produz, porque lá as teclas entram como teclas do console e não como controle.

Some-se a isso o teto de voltas por quadro (o item 4 acima): com o guest num laço que devolve o
controle sem apresentar, o `run_frame` gasta o orçamento antes do próximo temporizador vencer.

O próximo experimento é este, e ele é de uma pergunta só: **quantas voltas cada caminho gasta por
quadro virtual, e quantas delas avançam o relógio**. A varredura dá uma volta por iteração e o core
acumula até o teto; se o teto é atingido em laço sem relógio, a diferença entre os dois caminhos
está aí, e não no roteiro nem na tecla. O instrumento para isso (contar voltas do motor por quadro)
ainda não existe no core: `Session::machine` é privado do motor, e expor um acesso público é
mudança no `src/`, que custa os doze trabalhos de CI — vale a pena fazer isso junto de outra
mudança, não sozinho.

### 7.9 Retorno à roda, observado na captura serial

A captura do caminho do core registra o retorno que o teste antigo não conseguia afirmar pelo
`CLASSE_ATUAL`: depois da sessão do jogo, a Z-Wheel volta a construir seus objetos. Em
`/tmp/quem.txt`, medido no roteiro completo, aos `78611 ms` aparecem de novo
`0x01028e14`/`0x01028e05` e os `IValueModel`; aos `85451 ms` ela reconstrói a barra, lê a lista e
consulta `GAMEINFO.class_id = 17359702`. O mesmo trecho grava `StringLastAppRan = 279369`, atualiza
`playcount/dt_lastplayed` e destrói o contexto EGL do jogo:

```text
78611 ms  classe 0x01028e14 · classe 0x01028e05 · IValueModel · GAMEINFO
85452 ms  StringLastAppRan = 279369 · UPDATE GAMEINFO · eglDestroyContext · eglDestroySurface
```

Isso é a volta ao shell observada no guest: o jogo escolhido sai, o core reabre a Z-Wheel e ela
monta a biblioteca outra vez. O teste do core ainda relata `voltou=false` quando a janela de espera
termina antes desse desfecho ou quando o foco cai num título que não sai; a captura é a evidência
mais direta do caminho que realmente aconteceu.

## 8. O que ainda não funciona

### 8.1 Medido em 22/09/2026: a roda não reage a tecla, e a causa era uma classe

Com o roteiro da varredura (`ZEEBX_ROM_TECLAS=30500:k0xe064,33000:k0xe032,…`) a roda sobe, desenha
e **não reage**: os cinco quadros gravados um meio segundo depois de cada tecla saem **idênticos**
(mesmo `sha1`), 86% brancos, e o log dela termina em
`Shop_Action.c:77 Unable to create instance of IDOWNLOAD in ShopAction_Init` seguido de
`GameLib_Form.c:1315 Unable to init ShopAction in Gamelib_CheckPreLoaded` — a biblioteca de jogos
não monta, e sem grade nenhuma tecla tem o que mover.

O `IDOWNLOAD` é a classe **`0x01000000`**, que estava recusada como "só o `QVERSION`". O
`AEEClassIDs.h` do SDK 4.0.2 diz, em três linhas seguidas: `QVERSION` é `0x01000000`,
`AEECLSID_PRIV` é `QVERSION` e **`AEECLSID_DOWNLOAD` é `AEECLSID_PRIV`**. A leitura anterior parou
na primeira das três.

Atendida por sonda, o abort **desaparece** do log e a execução sobe (473 828 → 474 692
instruções; 7 353 → 7 394 chamadas de API), mas a roda ainda **não monta o menu**: ela fica no
pulso descrito em §4 — um timer, 2 582 voltas de 16,6 ms lendo pontos e fila de download, sem
desenhar nada de novo. É ali que a investigação está agora.

**A árvore de widgets na primeira tecla explica o "não reage".** Com a captura de serial ligada na
varredura (`ZEEBX_ROM_SERIAL`), o despejo da primeira tecla (30 533 ms) mostra **três** widgets: o
formulário raiz `0x1028e51` com dois filhos, e mais dois objetos de classe `0x0`, sem tratador, sem
tamanho e sem texto. Não há palco, roller, barra nem grade — não existe o que uma tecla mova. O que
se vê na tela é o *splash* herdado da sessão, e não desenho da roda.

E a captura diz o que ela faz no lugar disso, na ordem:

```text
  0 ms  classe 0x0100104f  ... abriu tt_prefs.db, asset_cache, tt_game_info, tt_dlqueue.db
  0 ms  sql SELECT * FROM GAMEINFO, TITLETEXT WHERE ... AND GAMEINFO.class_id = -1 -> 0 linha(s)
  0 ms  classe 0x01028e47 (formulário) · 0x01028e19 · 0x01028e3f (barra) · 0x0100550d (IWeb) 2x
  0 ms  prop 0x216=0x1 em 0x30000650
  6000 ms  prop 0x216=0x0
  7000 ms  prop 0x216=0x0 · classe 0x01006c01 (cartão SIM)
30533 ms  primeira tecla: 3 widgets
```

Ou seja: a consulta ao `tt_game_info` para `class_id = -1` volta **vazia**, o `IWeb` é criado duas
vezes e não é chamado, e o sinalizador `0x216` é ligado e desligado entre 6 s e 7 s. Depois disso a
roda fica parada. É ali — e não na entrada — que o item 8 está preso.

### 8.1.1 O slot 5 do controle de sistema: a primeira tecla não é inerte

Pressionar o confirmar **cedo** (3 s, e não os 30 s do roteiro da doc) não é inerte: a varredura
abortava com

```text
erro: laço parou em Unimplemented { addr: 0xf0030014, args: [0x300006d0, 2, 0, 0xf0030014], caller: 0x81898 }
```

e o endereço decodifica pelo próprio esquema do emulador — `API_BASE + interface*0x1000 + slot*4`,
com `API_BASE = 0xf0000000` — em **`Interface::SystemCtl` (48), slot 5**. Ou seja: no confirmar, a
Z-Wheel chama o slot 5 do controle de sistema, com o modo em `r1 = 2`, e nós não o tínhamos.

A tabela de slots traz `"slot5"`, que é **marcador**: `aee::e_marcador` faz o despacho devolver
`None`, e a execução para ali de propósito — "um travamento é melhor que uma abertura errada". O
nome veio do firmware, desmontado de `1.1.2_APPS.bin` (`ferramentas/firmware.py`, Thumb):

```text
vtable 0x10691ea8     slot[3] = 0x10e9fdb6   (DefinirModo)
                      slot[4] = 0x10e9fe7a
                      slot[5] = 0x10e9fe92   <- o que a roda chama
                      slot[6] = 0x10e9ff84   (Consultar)
```

O slot 5 recebe `(this, modo, opção)` — a opção com `-1` valendo "a do aparelho", lida de
`0x114287ec` — e a **última coisa que faz é `bl 0x10e9fdb6`**, que é o corpo do `DefinirModo`. É o
slot 3 com um argumento a mais. Implementado assim, o abort **desaparece** e a roda passa a receber
as sete teclas do roteiro sem parar — mas **não redesenha**: os sete quadros continuam idênticos
(`sha1` igual), e o pedido de abertura não aparece.

### 8.1.2 A causa: a classe do cartão SIM estava sendo oferecida

O `268d6fb` (21/09) pôs `AEECLSID_SIMCARDCTL` (`0x01006c01`) na fábrica, para calar o
`tectoymain.c:1668 ERROR: Unable to create instance of AEECLSID_LCT_SIMCARDCTL, cannot do SIM
check`. O comentário longo de `Interface::SimCardCtl` já dizia que **aquele log é o jogo tomando o
caminho certo**: recusada a classe, a Z-Wheel põe o estado em `0x27` e o `0x82464` chama a
`0x1f7b4`, que avança a interface; oferecida, o slot 3 responde zero, o estado vira `0x28`, e o
`0x82464` **não faz nada com ele**. Era o `0x28` — "a tela fica onde está, calada".

O A/B, medido no `bench` sem janela, 43 s de relógio virtual, mesmo roteiro nos dois:

```text
com a classe oferecida    474 692 instruções · 2582 voltas · 1 timer · 6 chamadas de IMedia
                          5 quadros IDÊNTICOS (sha1 igual) · 0 quadro apresentado
com a classe recusada     750 milhões de instruções · 6572 voltas · 4 timers · 32 chamadas de IMedia
                          5 quadros DIFERENTES · 1 quadro apresentado
```

E a varredura passa a mostrar o que a roda desenha — a **grade da biblioteca local**, com os
nossos jogos em três colunas, a partir de 37,6 s, que é depois do confirmar em "Jogar":

```text
texto desenhado na tela:
    37605 ms  (427, 272)  Alien Breaker
    37622 ms  (42, 272)  Action Hero 3D
    37622 ms  (271, 272)  Alice
```

Falta o **pedido de abertura**: ele ainda não aparece, e o novo candidato está no próprio
relatório — `IWidget::Acessador seletor 0x7001` entrou na lista de APIs que faltaram, e a `0x7001`
é o evento que a transição manda no fim (§7.3).

**A ponte do catálogo, e o que ela não resolve.** A roda não lê a pasta de ROMs: lê o
`tt_game_info` do perfil, e quem liga um ao outro é o `catalog.json` que a interface grava
(`library::sync_catalog`). A varredura não o alimentava — medido: com `ZEEBX_ROM_INSTALADOS`, o
banco do perfil abria com as 59 linhas oficiais do pacote e a `ZEEBX_LIBRARY` com **zero**. A
varredura agora varre a pasta da ROM e sincroniza (63 linhas passam a entrar), o que é necessário
— mas **não suficiente**: a árvore de widgets na primeira tecla continua com os mesmos três, e a
roda continua sem montar o palco (`0x01028e05`) nem o roller (`0x01028e14`), que são os dois
objetos por onde passa a montagem do menu (§4 a §5.4). O `0x01028e19` que a §2.2 cita **é** criado,
uma vez, logo depois do formulário — e é o único do trio.

Duas armadilhas de instrumento saíram desta medição, e as duas estão consertadas no código:

- **O passo do roteiro sem o `k` era descartado em silêncio.** Escrito na forma da bancada
  (`30500:0xe064`), o roteiro inteiro virou "nenhuma tecla" sem uma linha de aviso, e a medida
  dizia que a roda não responde — quando quem não apertou nada foi o roteiro. Agora o `k` é
  opcional (o botão vem primeiro, porque `up` e `down` são nomes dos dois) e o que for descartado
  sai como aviso.
- **A varredura não tinha captura de serial.** A árvore de widgets da primeira tecla, as classes
  criadas e os bancos abertos vão para a captura de serial, que é onde a instrumentação escreve sem
  se misturar com o log do jogo — e ela só existia no `run` (`--serial=`). Sem ela, "para quem foi
  esta tecla?" só se respondia com janela aberta, que é o que a varredura existe para não exigir.
  Agora é `ZEEBX_ROM_SERIAL=<arquivo>`.
- **A varredura não tinha sonda nem a mostrava.** `ZEEBX_ROM_SONDA=0xCLSID,…` responde a classe
  com um objeto de observação e o relatório ganhou a seção "o que o jogo chamou nas classes de
  sonda", slot a slot, com os textos que os argumentos apontam. Foi assim que se viu que a roda
  pega o objeto, pergunta o slot 2 (`QueryInterface`), usa e solta.

### 8.2 O que continua apoiado em hipótese

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
| Teclas da roda | [`input/mod.rs`](../../src/input/mod.rs), `avk::LEFT`/`RIGHT` (o direcional, em `ui/mod.rs`) |
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
