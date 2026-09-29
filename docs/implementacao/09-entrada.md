# 09 — Entrada

## Três camadas

```
teclado / controle do host   →   input/bindings.rs   →   input::Pad   →   IHID / IHIDDevice
     (quem apertou)            (o que aciona     (o estado do      (como o jogo lê)
                                 o quê)            console)
```

Cada camada não sabe da anterior. `input/bindings.rs` não conhece teclado nem gamepad — ele só diz *o
que* aciona *o quê*; quem sabe se a tecla `Z` está apertada é a interface. É isso que permite
testá-lo sem hardware nenhum.

## O controle do console

`input/mod.rs` tem a tabela real: **18 botões e 4 eixos**, com o UID de cada um. Os UIDs vêm do
`hid_devices.cfg` do console, e há três acréscimos deliberados, cada um com sua razão:

- **Os quatro sentidos do direcional como botões.** O arquivo do console os traz só como eixos
  `X`/`Y`, mas é como botão que os jogos os leem: com esses UIDs presentes o menu do Quake anda;
  sem eles o cursor não sai do lugar por mais que o eixo mude. Um direcional digital em USB HID
  costuma ser reportado das duas formas.
- **`Button_1` e `Button_3`.** Faltam na lista do console, que traz o `2` e o `4` mas põe um UID
  de eixo no lugar de um deles. A tela de ajuda do próprio Quake nomeia os quatro — "aperte 1
  para pular" —, então eles existem no controle.
- **`HOME` é o `Back` do BREW.** O controle tem HOME impresso na carcaça, e o `hid_devices.cfg`
  mapeia esse botão físico para `AEEUID_HIDJoystick_Back`. O Double Dragon pede "APERTE O BOTÃO
  HOME" e quem responde é o `back`.

### Eixos

`X`, `Y`, `Z` e `RZ`, nas palavras 1, 2, 3 e 6 do `AEEHIDPositionInfo`. A faixa é de **um byte
sem sinal, `0..=255`, com o repouso em 128**.

Isso não é escolha: está escrito no jogo. O Zeebo F.C. Super League converte cada eixo assim, e
o mesmo trecho está no Tênis, no Zeeboids e em todo título que usa essa camada do SDK:

```text
mvn   r0, #0x7f        ; r0 = -128
sxtah r4, r0, r4       ; valor = (int16)eixo - 128
```

Ele subtrai 128 para achar o centro, então o centro do aparelho é 128 — e é também o que
reportam os manches USB que o próprio `hid_devices.original.cfg` lista, o Logitech Dual Action e
o RumblePad2. O descritor do controle do Zeebo está no dump, mas a parte do report que traria os
limites veio como `** UNAVAILABLE **`; antes disso valia um palpite de 16 bits com sinal, e o
palpite estava errado.

**O sintoma de mandar zero era o boneco andando sozinho.** Zero não é o centro, é o batente: com
o manche parado, todo jogo dessa camada lia `-128` nos quatro eixos e caminhava para um canto. O
Zeeboids escapava por acidente — ele trata "os quatro eixos exatamente no mínimo" como "não há
manche aqui" e zera tudo, que é uma defesa contra exatamente o que fazíamos.

Pelo mesmo motivo, as vinte palavras de eixo que o controle **não** usa vão no centro, e não em
zero: um jogo que leia uma delas encontra um eixo parado, não um encostado no batente. A palavra
zero da struct continua zerada, porque ela não é eixo — é o `bRelativeAxes`.

Dentro do emulador o eixo é guardado **centrado no zero** (`Pad::axes`, com curso `±128`), que é
a forma com que o jogo trabalha depois daquela subtração. Quem traduz para a faixa do aparelho é
o `IHIDDevice`, no único lugar em que o console é quem lê.

O `Y` vai **invertido** em relação à biblioteca de controles: no HID o eixo vertical cresce para
baixo, e cima é o valor baixo. O par da direita segue a mesma convenção.

**O direcional e o manche são canais distintos.** Embora o descritor enumere `X` e `Y`, o
direcional digital chega como botões `DPad_*`; o manche esquerdo alimenta `X` e `Y`. **Não
espelhamos um no outro por padrão**: soltar uma seta enviaria uma falsa variação analógica de
retorno ao centro, e jogos que usam variação em vez de estado passariam a navegar duas vezes. O
espelho existe como opção desligada, para os jogos que só escutam o eixo — ver *O espelho do
direcional, e por que ele é opção*, adiante.

Quem responde `GetAxesInfo` não devolve valores: devolve, em cada palavra, o **UID do eixo que
ocupa aquela palavra**. É assim que o jogo descobre onde está cada direção, e por isso a tabela
de UIDs precisa estar certa — um UID errado não dá erro nenhum, o jogo só não acha o eixo.

Três dos quatro UIDs são **transcrição literal** da entrada do controle do Zeebo
(`VID:0x1EAA:PID:0x0135`) no `hid_devices.original.cfg` do console:

```text
AXIS:X:0x0106C40C      <- este não
AXIS:Y:0x0106C4D1
AXIS:Z:0x0106C4CE
AXIS:RZ:0x0106C4CF
```

O `X` do arquivo vale o UID do `Button_3`, e a troca é espelhada: o `BUTTON:3` da mesma entrada
vale `0x0106C4D0`, que é o `LeftThumb_X` das outras entradas do próprio arquivo.

**Desfazemos essa troca, e quem decidiu foi medida.** Com `0x0106C40C` no `X`, o manche não move
esquerda e direita em jogo nenhum — o jogo varre a tabela do `GetAxesInfo` procurando UID de
eixo, não acha nenhum para o `X` e nunca guarda o campo dele; `Y`, `Z` e `RZ` andam e só o
horizontal fica morto. Com `0x0106C4D0`, o manche anda inteiro.

Isto não contradiz a lição da seção seguinte. A pergunta ali era "quais são os UIDs do
aparelho", e a fonte é o arquivo. A pergunta aqui é "o que o jogo procura na tabela", e a única
fonte possível é o jogo.

### O conserto que não era

Vale registrar porque é um erro de método, não de código.

Eu "consertei" o `X` uma vez, deduzindo dos binários dos jogos: os quatro valores `c4ce`, `c4cf`,
`c4d0` e `c4d1` aparecem em dezenas de títulos, sempre em pares dentro do mesmo pool de
constantes, o que é a cara de dois manches — e `0x0106c40c` não aparece em jogo nenhum. A
conclusão foi que `X`/`Y` eram `c4ce`/`c4cf`.

A dedução estava certa **para as outras entradas do arquivo**, as dos controles de PC, onde de
fato `AXIS:X:0x0106c4d0` e `AXIS:Y:0x0106c4d1`. Para o controle do Zeebo, não. Era plausível,
coerente e errada, e só caiu quando o arquivo apareceu.

A lição: **fonte primária ganha de inferência**. Uma dedução bem construída a partir de evidência
indireta pode explicar tudo o que se observou e ainda assim descrever outro aparelho.

### Quem lê o quê

Contando as chamadas de `IHIDDevice` em dez segundos de cada uma das 62 ROMs:

- **Só o eixo e a varredura, nunca o evento de botão**: os ports de arcade da Data East (Magical Drop 3,
  Karnov's Revenge, Wizard Fire, Street Hoop, Spin Master, Caveman Ninja, Dark Seal, Super
  BurgerTime). Eles chamam `GetPositionState` umas quinhentas vezes e `GetNextButtonEvent`
  **zero**.
- **Os dois, todo quadro**: os jogos da Zeebo Sports, o zeetris, o Zeeboids, a série Extreme.
- **Quase só o botão**: o Quake e o Tork and Kral.

Por isso os dois canais ficam. Desligar qualquer um deles deixa parte da biblioteca sem entrada
nenhuma — foi medido: com o direcional só nos eixos, o menu do Tênis não anda; só nos botões, ele
anda e fica.

### O espelho do direcional, e por que ele é opção

A seção anterior nomeia os jogos que só escutam o eixo: os ports de arcade da Data East chamam
`GetPositionState` mais de duas mil vezes em quarenta segundos e `GetNextButtonEvent` **zero**. Para
esses, o direcional de um controle não faz nada — no console ele é botão, e botão nenhum chega ao
canal que eles leem. Medido, com o direcional apertado nos quatro sentidos, cinco segundos cada:

| jogo | eixo fora do centro, sem a opção | com a opção | com o manche de verdade |
|---|---|---|---|
| Rally Master Pro | 0 | 4 | 4 |
| Magical Drop III | 0 | 1212 | — |
| Zeeboids | 0 | 600 | — |

A coluna do meio é a que importa, e ela vem de `Machine::leituras_com_eixo_deslocado`: **a contagem
de chamadas diz que o jogo pergunta; esta diz que a resposta chegou.** Sem ela, um port que consulta
o eixo todo quadro parece igual com o direcional solto e apertado — e foi o que quase fez esta
mudança ficar sem prova, porque a contagem de `GetPositionState` não muda com o direcional.

**É opção, desligada por padrão, e são duas.** No núcleo, `zeebx_dpad_to_analog_p1` e
`zeebx_dpad_to_analog_p2` (`disabled|enabled`), uma por porta; no standalone, a caixa equivalente na
tela de controles, que também é por porta. Duas e não uma porque o console tem duas portas
(`input::PORTAS = 2`) e são dois jogadores: quem joga de manche no controle 1 não decide pelo dono do
controle 2. No standalone o ajuste mora em `Player::direcional_nos_eixos` e desce para o
`settings.json` sozinho.

A razão de não ser o padrão é a mesma que tirou o direcional dos eixos: **o defeito é por jogo**. O
Zeeboids consulta os dois canais a cada quadro — 1.212 `GetPositionState` e 1.219
`GetNextButtonEvent` em quarenta segundos —, então com a opção ligada ele recebe o mesmo aperto duas
vezes; foi o que desfez a tentativa de `e705840` em `4418fe9`. Quem lê os dois canais deixa
desligado; quem só lê o eixo liga.

**A ordem importa, e é o detalhe que não se adivinha.** O espelho escreve o eixo *e o devolve ao
centro* quando a direção não está apertada, porque é isso que um manche faz — e é a mesma "falsa
variação de retorno ao centro" que o `Pad::press` anota como o risco da ideia. Isso obriga o espelho
a rodar **antes** do laço do analógico, no `Player::pad`, e antes do roteiro do harness: o manche de
verdade, quando existe e está fora da zona morta, precisa ter a última palavra. Escrito na ordem
contrária, o espelho apagaria o manche parado no centro e o direcional venceria o analógico.

Os três frontends que passam pelo [`Player::pad`] obedecem à opção: o desktop, o sem janela (pela
chave `dpad_to_analog` de cada seção `[portN]` do `config.ini`) e o núcleo Libretro. **O frontend
Android monta o próprio `Pad`** — o controle de tela e o gamepad dele não passam por aqui —, e por
isso a opção não o alcança.

Para medir sem janela:

```sh
zeebx bench "<jogo.zip>" --seconds=40 --keys=15000:up:5000,20000:down:5000 \\
    --dpad-nos-eixos
```

O resumo traz a linha `eixo:` com as leituras fora do centro, e a linha `entrada:` com as chamadas.

### Há um terceiro canal, e nele a ordem da lista é tudo

Os ports da Data East não param no eixo: eles chamam `GetButtonInfo` **dezesseis vezes por
quadro**, com os índices de 0 a 15, e guardam o estado de cada um num vetor indexado pela
posição. O laço está em `0x1f2a8` no módulo do Bad Dudes, e é literal — `mov r1, r4`, chamada,
`ldr r3, [sp, #4]`, `strb r3, [r5, #0x13c]`, `add r4, r4, #1`, `cmp r4, #0x10`.

Para quem lê assim, **o UID não importa: importa a posição**. Enquanto a lista abria com o
`Button_2` e trazia o `Right_Shoulder_Upper` logo atrás, o botão 2 do controle chegava ao arcade
como o gatilho direito e o 4 como o 3 — e o `b2` e o `b3`, que estavam nas posições 16 e 17, não
chegavam a ser lidos. Por isso os quatro botões de face abrem a lista, na ordem 1, 2, 3, 4, e o
que não é botão do aparelho (o `LeftThumb_X` que o arquivo do console deixou no meio, e a segunda
aparição do `Right_Shoulder_Upper`) foi para o fim, fora da faixa que o arcade varre.

O direcional segue a mesma lógica: **fica na ordem dos UIDs, cima, esquerda, baixo, direita**
(posições 12 a 15). O `GamepadMgr` das amostras do SDK lê o `id` do `GetNextButtonEvent`, que é a
posição na lista, e guarda o estado num vetor indexado por ela (`0x16b68` no Dragon Vs Chicken).
Na ordem cima, baixo, esquerda, direita, a demo andava para baixo com a esquerda e vice-versa. O
Tênis, o Rolima e o Bad Dudes não usam esse laço.

`Z` e `RZ` não tinham nada os alimentando até o manche direito ser ligado neles. **O sentido
desses dois é suposição**: o arquivo nomeia os eixos sem dizer o sentido, então seguimos a mesma
convenção do par esquerdo, e a tela de configuração tem uma caixa "Inverter".

### O `type` do `AEEHIDDeviceInfo` é um UID

Antes de criar o aparelho, alguns jogos conferem o que o `IHID::GetDeviceInfo` diz dele. O
primeiro campo da struct é o **UID do tipo de dispositivo** — o mesmo `0x0106c3fd` de joystick
que o jogo passa ao `GetConnectedDevices`, e `0x0106c3fc` para teclado —, e não um número
pequeno. Nós respondíamos `1`.

O **Bad Dudes vs. DragonNinja** compara esse campo com `0x0106c3fd`; com `1` ele nunca chamava o
`CreateDevice` e ficava parado no aviso inicial, sem ver botão nenhum. Ele também pula o
aparelho de produto `3` e fabricante `0x15a2`, que é o receptor do Boomerang. Os outros ports da
Data East não conferem o tipo, e por isso funcionavam.

**E a fila de eventos de conexão vazia responde `EFAILED`.** O `GetNextConnectEvent` respondia
sucesso com os campos zerados, o que para o jogo é "houve um evento" — um aparelho de identificador
zero conectando, a cada pergunta. Enquanto o `type` era `1`, os Zeebo Extreme descartavam o evento
e seguiam; com o UID certo eles passaram a tratá-lo, e ficaram em `GetNextConnectEvent` e
`GetDeviceInfo` para sempre, sem armar timer nem desenhar. A sessão então terminava sozinha, por
falta do que fazer, e o jogo "não abria". Fila vazia é `EFAILED`, como no `GetNextButtonEvent`.

## A numeração dos botões é a do aparelho

O losango do controle **não** é numerado na ordem em que os olhos leem: **1 fica embaixo, 2 à
esquerda, 3 no topo e 4 à direita** (conferido nas imagens oficiais do controle). O mapeamento para
o RetroPad preserva a **posição da mão**, e não o número:

| aparelho | onde fica | RetroPad |
|---|---|---|
| Botão 1 | embaixo | `B` |
| Botão 2 | esquerda | `Y` |
| Botão 3 | topo | `X` |
| Botão 4 | direita | `A` |
| HOME | no meio | `Select`; no standalone, o `Start` do host também cai nele |
| ZL / ZR | ombros | `L` / `R` |
| direcional | cruz | `D-Pad` |
| dois manches | — | analógicos esquerdo e direito |

**Isto já esteve errado de duas maneiras ao mesmo tempo** (issue #41): a tela de mapeamento do
núcleo rotulava `B` como Botão 1 enquanto a leitura entregava `B` como Botão 2 — duas listas
paralelas que divergiram em silêncio —, e o mapa da arte em `assets/controller-map.svg` numerava o
losango como 1 embaixo, 2 à direita, 3 à esquerda e 4 no topo. Agora a tabela do núcleo é **uma
só** (`BOTOES_DO_RETROPAD`: quem lê e quem rotula bebem da mesma), a arte segue a numeração do
aparelho, e um teste prende as duas coisas — descritores e leitura não podem mais discordar.

**Quem já tinha mapeamento salvo também é alcançado.** O `settings.json` manda mais que o padrão, e
um mapeamento antigo continuaria entregando leste no `b2` — o defeito inteiro. O
`Player::migrate_action_buttons` troca os quatro botões de ação pelos novos **quando eles ainda são
exatamente os antigos**; quem mexeu em qualquer um deles fica com o que escreveu, que é a mesma
regra da migração da convenção dos eixos. No RetroArch não há migração a fazer: o mapeamento padrão
de lá é por botão físico, e a correção vale assim que o núcleo novo entra.

## Mapeamento configurável

`input/bindings.rs`. O mapeamento é guardado **por nome** — o nome da tecla, o do botão do controle do
host, o do botão do Zeebo — e não por índice. Índices mudam quando uma tabela muda; nomes
sobrevivem, e é o que faz um arquivo de configuração escrito hoje continuar valendo depois.

- Um botão do Zeebo aceita **mais de uma origem**: é o que permite `Espaço` e `X` fazerem a mesma
  coisa, e o teclado continuar valendo com um controle ligado.
- `Source::Axis { name, positive }` é "o jogador empurrou para este lado?", e serve para acionar
  um **botão**.
- `AxisSource { name, invert }` traz o **valor inteiro** do eixo, que é o que um analógico é.

O manche **não** aperta o direcional. São controles diferentes, e ligar os dois faria os dois
agirem juntos e nenhum deles sozinho. O valor analógico entra **depois** dos botões e só fora da
zona morta: assim ele acrescenta curso ao que o direcional escreveu, em vez de apagá-lo quando o
manche está em repouso.

`Y` vai invertido porque no HID o eixo vertical cresce para baixo e na biblioteca de controles
cima é positivo. Errar esse sinal inverte o eixo vertical de todo jogo que o lê — tem teste.

**O aviso de evento é por aparelho, não por tipo de evento.** O jogo registra o `ISignal` do
`RegisterForButtonEvent` e do `RegisterForPositionChange` **no objeto do aparelho**, um por
controle ligado. Guardar o sinal só pelo nome do registro fazia o segundo apagar o primeiro: toda
mudança acordava o callback do controle dois, o jogo perguntava ao aparelho errado, não achava
evento nenhum e o controle um não fazia nada. O Treino Cerebral ficava preso no "aperte botão 1"
assim que a porta dois entrava. A chave é o par `(registro, porta)`.

**O `aparelho` da porta é o que o console enumera, e o controle do host é outra coisa.** São dois
campos: o aparelho diz se aquela porta é um Z-Pad, um controle, um Boomerang ou um teclado — e o
`GetConnectedDevices` de joystick só lista os três primeiros —, enquanto o `device` diz de qual
aparelho do host ela lê. Marcar o segundo controle numa porta de teclado deixava a porta fora da
conta dos jogos, e a opção de dois jogadores ficava apagada com as duas portas ligadas. Escolher
um controle na lista agora ajusta o aparelho junto, e uma configuração antiga é corrigida ao
carregar.

**O controle do host também é guardado por nome, e dois iguais têm o mesmo nome.** Quem liga dois
aparelhos do mesmo modelo e marca o segundo na porta 2 gravava a mesma string da porta 1, e a
busca — que varre a lista do sistema procurando o nome — devolvia sempre o primeiro: a porta 2
respondia ao controle 1. A lista numera as repetições (` #2`, ` #3`) e a busca usa a mesma lista,
então continua sendo nome, que sobrevive a desligar e religar um aparelho.

## Migração de configuração

`Controls::adopt` ajusta um arquivo escrito por uma versão anterior: dá eixos a quem tem controle
e ainda não os tinha, tira o manche de cima dos botões do direcional e devolve a inversão
vertical a quem a perdeu — houve uma versão que salvou os quatro eixos retos, e o padrão voltar
ao certo não conserta um arquivo já gravado.

Existe porque **o padrão de um campo novo nem sempre é o vazio**. Sem isso, quem já tinha um
controle configurado ficava com os manches mudos e não teria como adivinhar o motivo.

## Controles de verdade

`input/gamepads.rs`, sobre `gilrs`. Um computador sem nenhum controle — ou sem permissão para lê-los —
não pode impedir o emulador de abrir: a falha vira "nenhum controle" e o teclado segue.

Os botões vêm antes dos eixos na captura: quem aperta o direcional de cruz de um controle que
também o reporta como eixo quer o botão, que é o mais específico.

## As duas portas

O Zeebo tem **duas portas USB**, e o console as enumera: o `GetConnectedDevices` da Z-Wheel
passa capacidade 2. Antes disso o emulador tinha um controle e pronto — `input::PORTAS` agora
vale 2, e cada porta é configurada separadamente.

Cada porta pode ter um destes aparelhos:

| Escolha | UID do aparelho | VID:PID | O que o jogo vê |
|---|---|---|---|
| `dragon` (`controle`) | `0x0106c3fd` | `1EAA:0135` | O Dragon, com os 18 botões e os 4 eixos |
| `zpad` | `0x0106c3fd` | `1A5C:3033` | O Z-Pad: os mesmos botões e eixos, outro identificador |
| `boomerang` | `0x0106c3fd` | `15A2:0003` | O Boomerang, com o relatório de movimento — ver o [20](20-boomerang-e-wii-remote.md) |
| `teclado` | `0x0106c3fc` | — | Um teclado USB |
| `nenhum` | — | — | A porta não é enumerada |

Os UIDs e os pares são do `hid_devices.cfg` do console, como o resto da tabela. Z-Pad e Dragon só
diferem pelo par VID/PID, que os jogos da Boomerang Sports usam para escolher o tratamento.

Na linha de comando: `--portas=dragon,teclado`. Na interface, cada porta tem sua aba de
mapeamento. Só a primeira porta nasce ligada — um arquivo de configuração escrito antes das
portas existirem continua valendo com o controle na porta 1, que é o que ele descrevia.

### O teclado é um aparelho, não um atalho

Isto merece a distinção porque as duas coisas existem e são diferentes:

- **O teclado do host simulando o controle** é o mapeamento de sempre, em `input/bindings.rs`. O jogo
  vê um controle.
- **O teclado como aparelho** é uma porta ocupada por um teclado USB. O jogo o enumera, e a
  Z-Wheel escreve `Keyboard Connected.` no log dela.

Quem quiser as duas coisas pode: controle na porta 1 com mapeamento de teclado, teclado de
verdade na porta 2.

### A ordem importa

A Z-Wheel enumera os aparelhos HID durante o `EVT_APP_START`. Configurar as portas **depois** de
começar o jogo faz o console nunca ver o teclado, por mais certa que a configuração esteja — foi
o que aconteceu, e o sintoma era "o teclado não funciona" sem nada no relatório. Daí o
`Session::start_with(caminho, portas)`: não há um caminho para começar um jogo e outro para
configurá-lo.

## Teclas de verdade

Além do controle, o emulador entrega `EVT_KEY` (`0x100`) com os códigos AVK do BREW:
`AVK_SELECT = 0xE015`, `AVK_0..9 = 0xE030..`, `AVK_CLR = 0xE04A`, e os quatro sentidos.

**Uma tecla vai primeiro aos tratadores de widget e só depois ao applet.** É a ordem do BREW: a
interface tem a primeira chance, e o que ela não consumir sobe. Entregar direto ao applet faz
menu nenhum responder.

## Dois roteiros, para testar sem janela

Existem porque sem eles a única forma de saber se a entrada funciona é apertar a tecla e olhar,
e isso não cabe num teste nem numa execução automática.

- `--keys=ms:botão[:duração]` move o **controle**. `--keys=3000:b1,6000:start`.
- `--teclas=ms:nome` entrega **teclas** AVK. `--teclas=1000:select,2000:down`.
- `--boomerang` e `--movimento=ms:x:y:z` (no `sessao`) movem um **Boomerang**; `--wiimote` usa o
  Wii Remote conectado. Ver o [20](20-boomerang-e-wii-remote.md).

O instante é o do relógio virtual do jogo, não o número de voltas do laço: uma volta não dura
sempre o mesmo tanto. Um botão fica apertado se **algum** passo o quer apertado agora — aplicar
passo a passo parecia igual e não era, porque todo roteiro que navega um menu usa o mesmo botão
duas vezes, e o segundo passo desfazia o primeiro antes da hora.

O relatório lista os toques entregues, com a porta de cada um:

```text
toques:    6 entregue(s) ao jogo
     3001 ms  porta 1  aperta b1
     3134 ms  porta 1  solta  b1
```
