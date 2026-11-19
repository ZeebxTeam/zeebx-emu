> **Registro em português.** Esta é a versão original deste documento, preservada como estava na v0.5.1 e não é mais atualizada. A versão mantida está em inglês: [20-boomerang-and-wii-remote.md](../../implementation/20-boomerang-and-wii-remote.md).

# 20 — O Boomerang, e os controles com sensor no lugar dele

O Boomerang é o controle de movimento do Zeebo: direcional, botões 1 e 2, HOME e acelerômetro,
sem fio, com um receptor USB. Os jogos da Boomerang Sports (Queimada, Tênis, Vôlei, Peteca), o
Crash Nitro Kart e os Zeebo Extreme o reconhecem e passam a jogar pelo movimento. Aqui ele é
emulado, e quem o move é o sensor do controle escolhido na porta: um Wii Remote, um Pro
Controller, um DualShock 4, um DualSense.

Nada disto veio de documentação: o formato do relatório foi lido no código do Zeebo Sports
Queimada (`dodgeball.mod`) e conferido no Crash Nitro Kart 3D (`cnk2.mod`).

## 1. Como o jogo reconhece o aparelho

Pelo par VID/PID do `GetDeviceInfo`. O Queimada classifica em `0x35130`:

| VID:PID | Tipo | Entrada no `hid_devices.cfg` |
|---|---|---|
| `1A5C:3033` | 3 | "Zeebo Game Controller" — o **Z-Pad** |
| `15A2:0003`, `1A5C:7033` | 4 | "Zeebo Accelerometer Controller" — o **Boomerang** |
| `1EAA:0135` | 5 | "New Zeebo Game Controller" — o **Dragon** |

Z-Pad e Dragon têm os mesmos botões e eixos (muda a pegada), então para o jogo a diferença é
só esse par. Qual par é de qual modelo é dedução pela ordem das entradas do arquivo; o
`1EAA:0135` é também o do descritor USB capturado de um console, e é o que o emulador sempre
anunciou. Os três viraram opções de "o console vê" em cada porta (`Aparelho::ZPad`,
`Aparelho::Controle` — o Dragon, que mantém o nome serializado antigo — e `Aparelho::Boomerang`).

Um receptor atende **dois** Boomerangs: o `GetConnectedDevices` lista um aparelho só para todas
as portas com Boomerang.

## 2. O relatório: a estrutura de posição não é de eixos

O `AEEHIDPositionInfo` tem o `bRelativeAxes` e 24 inteiros. No Boomerang eles são o relatório
bruto do receptor, e o jogo o desmonta (`0x4231c`, `0x42458`, `0x40b0c`, `0x413d0`, `0x412f4`).
Contando a partir do primeiro inteiro depois do `bRelativeAxes`:

| Campo | Conteúdo |
|---|---|
| 1, 2, 3 | acelerômetro X, Y, Z: um byte por eixo, centro `0x80` |
| 4 | botões, **lógica invertida** (bit em 0 é apertado): bit 0 botão 1, 1 botão 2, 2 HOME, 3 esquerda, 4 baixo, 5 direita, 6 cima |
| 5 | bit 0: de qual dos dois jogadores é o pacote; o resto é o tipo. Tipo 0 é "desconectado"; 1 a 4 conectam, e o campo 6 leva um dado do aparelho que depende do tipo (o tipo 1 traz, nos bits 4–3, a taxa de amostragem que a calibração usa) |
| 8 | contador de 8 bits. O jogo espera que ande de um em um; um pulo marca pacote perdido |

Na partida o jogo manda um comando pelo `Rumble` (`0x2000|…`, `0x300`) — a entrada do Boomerang
no cfg tem um relatório de saída de vibração — e **espera o contador mudar** para dar o receptor
como vivo (`0x42350`, até 5 milhões de tentativas). Com o contador parado, o jogo congela ali.

### Os botões só contam numa leitura sem pacote novo

O Queimada lê a posição três vezes por quadro e processa os botões de um jogador pela tabela
virtual dele, a cada quadro. Na prática, um único jogador recebendo pacote a cada leitura nunca
tinha o aperto processado; com os dois jogadores alternando, cada um tem leituras "sem pacote
seu" no meio. Por isso o receptor emulado (`Machine::pacote_do_boomerang`):

- produz um pacote novo a cada 10 ms de tempo virtual (`BOOMERANG_PERIODO_US`), e as leituras no
  meio repetem o último;
- **alterna sempre os dois jogadores**; o que não tem controle vai como tipo 0.

### O jogador desconectado leva a aceleração do primeiro

O Crash Nitro Kart lê o acelerômetro **sem olhar o bit de jogador**. Com o segundo jogador indo
parado, metade das leituras era o movimento e metade o repouso: o kart virava a esmo e a
calibração passava na hora. O pacote do jogador desconectado leva a aceleração do primeiro — o
Queimada ignora a aceleração de quem está desconectado.

## 3. O Crash Nitro Kart lê por outro caminho

Ele registra `RegisterForPositionChange` e só lê a posição dentro desse aviso, pelos analógicos
normalizados do SDK (`AEEHIDThumbsticks.c`, `0x496d8`), que acham cada eixo pelo UID do
`GetAxesInfo`. Duas consequências:

- **O receptor acorda o jogo sem parar.** `Machine::relatorio_do_boomerang` dispara o sinal a
  cada 10 ms com uma porta de Boomerang, mesmo com a aceleração igual. Sem isso a calibração
  recebia uma leitura e esperava as outras para sempre.
- **A tabela de eixos é a do Boomerang**, com Z e RZ trocados em relação ao controle
  (`Z=0x0106C4CF`, `RZ=0x0106C4CE`, como no cfg). Com a tabela do controle, a gravidade caía no
  campo errado e a calibração nunca aceitava a leitura.

A calibração dele (`0x45a64`) quer 15 amostras com X e Y entre 101 e 159 e Z entre 162 e 220, e
escreve `ACCELHORIZONTALCENTER`/`ACCELVERTICALCENTER` ao terminar. A faixa do Z centra 1 g perto de
63 unidades; o emulador usa 60 (`ESCALA`). A direção é linear:
`(X−127)·0,9659 + (Y−127)·0,2881` — a gravidade projetada no comprimento do controle, girada uns
17° pela curva do braço.

## 4. O referencial

O Queimada gira X e Y em 20° (`0,94` e `0,342`) antes de usar; o emulador aplica o giro inverso ao
montar o pacote, para o jogo chegar à aceleração entregue.

O CNK manda segurar o Boomerang deitado, com as duas mãos e a face para o jogador, girando como
volante. **O comprimento do Boomerang é o X dele; o do Wii Remote é o Y.** O Wii Remote seguro de
lado (direcional à esquerda) entra no referencial do Boomerang como `[y, x, z]`. O sentido do X foi
acertado na mão: com o sinal oposto, virar para a direita levava o kart para a esquerda.

## 5. O Wii Remote

`input/wiimote.rs`. O driver `hid-wiimote` do Linux divide o controle em dispositivos de entrada:
"Nintendo Wii Remote" (botões) e "Nintendo Wii Remote Accelerometer" (`ABS_RX/RY/RZ`, perto de 100
unidades por g). O gilrs enxerga no máximo os botões, então a leitura é nossa: uma thread por
dispositivo lê o `struct input_event` bruto e uma thread de busca acha controles novos a cada
segundo, agrupando botões e acelerômetro pelo dispositivo HID pai. Não precisa de biblioteca
nem de permissão extra — os nós já têm ACL para o usuário da sessão. Fora do Linux não há leitura.

Na lista de controles aparecem "Wii Remote 1" e "Wii Remote 2", e os botões dele são origens do
mapeamento como as de qualquer controle (ver a seção 6). Sem escolha, o primeiro Wii Remote vai
para o primeiro Boomerang, com os botões somados pelo mapeamento típico.

### Calibração nas configurações

"Calibrar" junta meio segundo de leituras paradas e grava o desvio e a escala em milésimos de g
(`CalibracaoDeMovimento`), para o repouso de face para cima virar `[0, 0, 1]`. Ela **recusa** leitura
que não esteja de face para cima e não corrige desvio maior que 0,25 g — e uma gravada fora disso
é ignorada. Calibrado de lado, o desvio era de 1 g: o CNK via a gravidade dobrada (Z = 249) e nunca
saía da tela de calibração. Os jogos continuam fazendo a calibração deles por cima desta.

### O aviso de calibração

Um toast opcional no canto da janela do jogo mostra o Boomerang inclinando e se o controle está
parado, e fecha quando o jogo confirma. Nenhum jogo avisa o sistema, então os sinais são as
mensagens de depuração: "calib" ou o `ACCEL 1` do CNK abrem; o `ACCEL…CENTER` fecha.

**A primeira leitura do Boomerang não serve de sinal:** todo jogo lê o controle na abertura, e os
da Boomerang Sports só calibram depois da escolha de personagem. Eles também não deixam rastro de
texto na hora (o nome das telas de calibração do Vôlei só passa pelas funções do BREW na
pré-carga), e o estado da calibração fica num objeto cujo layout muda entre os quatro jogos —
detectá-los pede leitura de cada um.

## 6. O sensor é do controle escolhido

A porta de Boomerang procurava sempre um Wii Remote. Com um Pro Controller escolhido nela, o
Boomerang ficava parado: o controle respondia pelos botões e o sensor dele nunca era lido.

Os drivers do Linux põem o sensor num dispositivo irmão do controle, marcado com
`INPUT_PROP_ACCELEROMETER` (o bit 6 do arquivo `properties`): "Pro Controller (IMU)" ao lado de
"Pro Controller", no `hid-nintendo`; "… Motion Sensors" no `hid-playstation`. O
`input/sensores.rs` acha esses nós e os lê como o do Wii Remote. Com giroscópio, a aceleração
vem em `ABS_X/Y/Z` (e a rotação nos `R`); a escala é o `resolution` do `input_absinfo`, que os
drivers preenchem em unidades por g (4096 no Pro Controller).

O sensor se liga ao controle pelo nome **do sistema** (`os_name` do gilrs, e não o nome do
mapeamento, "Nintendo Switch Pro Controller"), pelo par VID/PID e pela ordem entre controles
iguais. Os controles se seguram na pegada normal, que já é a de volante. O `hid-nintendo` põe o
comprimento do Pro Controller no Y, como o Wii Remote: com os eixos passando direto, o controle em
pé, de frente para o jogador, girava a prévia noventa graus para a direita. Os controles da
Nintendo (VID `057e`) trocam X e Y, e com o sentido do X oposto ao do Wii Remote (`[-y, x, z]`):
com o Y passando como veio, virar para a esquerda levava o kart para a direita. Os outros passam
direto até alguém medir.

**O nó do sensor costuma vir sem permissão.** O udev dá acesso ao usuário da sessão (`uaccess`) ao
que marca como joystick, e o sensor é marcado como acelerômetro: aqui o `event23` do Pro
Controller tem a ACL e o `event22`, do IMU, não. A prévia do Boomerang e o `zeebx controles` dizem
quando é isso, com a regra que resolve:

```
SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_ACCELEROMETER}=="1", TAG+="uaccess"
```

A prévia oferece um botão, "Liberar o sensor", que faz isso pelo `pkexec`: a janela de senha do
sistema aparece uma vez, e um comando só grava `/etc/udev/rules.d/70-zeebx-sensores.rules`,
recarrega as regras e repassa os dispositivos de entrada por elas (`udevadm trigger
--action=change`), o que dá a ACL aos sensores já ligados. O número 70 vem antes do
`73-seat-late.rules`, que é quem aplica o `uaccess`. A leitura de um sensor sem permissão é
tentada de novo a cada segundo, então ele volta sozinho, sem reconectar o controle. Se o
`pkexec` falhar (sem agente de senha, senha recusada), a regra aparece para ser criada à mão.

Sem controle escolhido, vale o que valia: o primeiro Wii Remote para o primeiro Boomerang.

### O Wii Remote é um controle mapeável

Os botões dele se somavam aos da porta por uma tabela fixa, e escolhê-lo numa porta deixava o
mapeamento de teclado — como Z-Pad ele só tinha o direcional e dois botões. Agora os botões são
origens como as do gilrs (`WiiUp`, `WiiA`, `Wii1`, `WiiPlus`, `WiiHome`…), capturáveis na tela de
controles, e escolhê-lo traz o mapeamento típico: direcional no direcional, 1 e A no botão 1, 2 e
B no botão 2, menos e mais nos botões 3 e 4, HOME no HOME. Quem já o tinha escolhido ganha esse
mapeamento na primeira vez que o emulador abre. A tabela fixa ficou só para o Boomerang sem
controle escolhido.

## 7. Ferramentas

- `zeebx wiimote [--seconds=N]` mostra o controle ao vivo.
- `zeebx controles` lista o sensor de cada controle, com a aceleração ou o aviso de permissão.
- `zeebx sessao <zip> --boomerang` põe um Boomerang na porta 1; `--movimento=ms:x:y:z,...` diz a
  aceleração a partir de cada instante, em g; `--wiimote` usa o Wii Remote conectado.
- `zeebx run <zip> --portas=boomerang` para rastrear o `IHID` com `--trace=HID`.
