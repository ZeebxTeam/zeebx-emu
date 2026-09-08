# 09 — Entrada

## Três camadas

```
teclado / controle do host   →   bindings.rs   →   input::Pad   →   IHID / IHIDDevice
     (quem apertou)            (o que aciona     (o estado do      (como o jogo lê)
                                 o quê)            console)
```

Cada camada não sabe da anterior. `bindings.rs` não conhece teclado nem gamepad — ele só diz *o
que* aciona *o quê*; quem sabe se a tecla `Z` está apertada é a interface. É isso que permite
testá-lo sem hardware nenhum.

## O controle do console

`input.rs` tem a tabela real: **18 botões e 4 eixos**, com o UID de cada um. Os UIDs vêm do
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

`X`, `Y`, `Z` e `RZ`, nas palavras 1, 2, 3 e 6 do `AEEHIDPositionInfo`. A faixa é de 16 bits com
sinal: o descritor USB do controle está no dump, mas a parte do report que traria os limites veio
como `** UNAVAILABLE **`, então adotamos o padrão de HID analógico.

**O direcional é reportado como `X` e `Y`** — é o que o arquivo do console diz. Por isso apertar
o direcional mexe nos eixos, e não o contrário.

Quem responde `GetAxesInfo` não devolve valores: devolve, em cada palavra, o **UID do eixo que
ocupa aquela palavra**. É assim que o jogo descobre onde está cada direção, e por isso a tabela
de UIDs precisa estar certa — um UID errado não dá erro nenhum, o jogo só não acha o eixo.

Os quatro UIDs são **transcrição literal** da entrada do controle do Zeebo
(`VID:0x1EAA:PID:0x0135`) no `hid_devices.original.cfg` do console:

```text
AXIS:X:0x0106C40C
AXIS:Y:0x0106C4D1
AXIS:Z:0x0106C4CE
AXIS:RZ:0x0106C4CF
```

O `X` valendo o UID do `Button_3` é esquisito, e a esquisitice é espelhada: o `BUTTON:3` da
mesma entrada vale `0x0106C4D0`, que é UID de eixo. Parece uma troca no arquivo da TecToy — mas
é o arquivo do console, e é o que os jogos viram quando foram feitos.

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

- **Só o eixo, nunca o evento de botão**: os ports de arcade da Data East (Magical Drop 3,
  Karnov's Revenge, Wizard Fire, Street Hoop, Spin Master, Caveman Ninja, Dark Seal, Super
  BurgerTime). Eles chamam `GetPositionState` umas quinhentas vezes e `GetNextButtonEvent`
  **zero**.
- **Os dois, todo quadro**: os jogos da Zeebo Sports, o zeetris, o Zeeboids, a série Extreme.
- **Quase só o botão**: o Quake e o Tork and Kral.

Por isso os dois canais ficam. Desligar qualquer um deles deixa parte da biblioteca sem entrada
nenhuma — foi medido: com o direcional só nos eixos, o menu do Tênis não anda; só nos botões, ele
anda e fica.

`Z` e `RZ` não tinham nada os alimentando até o manche direito ser ligado neles. **O sentido
desses dois é suposição**: o arquivo nomeia os eixos sem dizer o sentido, então seguimos a mesma
convenção do par esquerdo, e a tela de configuração tem uma caixa "Inverter".

## Mapeamento configurável

`bindings.rs`. O mapeamento é guardado **por nome** — o nome da tecla, o do botão do controle do
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

`Y` vai invertido porque no console cima é o valor negativo e na biblioteca de controles cima é
positivo. Errar esse sinal inverte o eixo vertical de todo jogo que o lê — tem teste.

## Migração de configuração

`Controls::adopt` ajusta um arquivo escrito por uma versão anterior: dá eixos a quem tem controle
e ainda não os tinha, e tira o manche de cima dos botões do direcional.

Existe porque **o padrão de um campo novo nem sempre é o vazio**. Sem isso, quem já tinha um
controle configurado ficava com os manches mudos e não teria como adivinhar o motivo.

## Controles de verdade

`gamepads.rs`, sobre `gilrs`. Um computador sem nenhum controle — ou sem permissão para lê-los —
não pode impedir o emulador de abrir: a falha vira "nenhum controle" e o teclado segue.

Os botões vêm antes dos eixos na captura: quem aperta o direcional de cruz de um controle que
também o reporta como eixo quer o botão, que é o mais específico.
