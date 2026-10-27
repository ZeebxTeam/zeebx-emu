# O Zeebx na PlayStation Vita

O núcleo é o mesmo. O que muda é quem pinta, quem toca e quem aperta o botão.

Este aplicativo não passa pelo RetroArch. O core que o RetroArch liga continua em
[`frontends/vita`](../vita/compilar.sh), um `.a` estático. Aqui o binário é o `eboot.bin` de um
VPK: a lista de jogos, o quadro e o som são deste processo.

## O que ele pede

Um Vita com Homebrew (Henkaku ou o equivalente). O VPK não instala num aparelho de fábrica.

A imagem entra pixel a pixel, no centro. Esticar 640×480 até encher a tela repetia uma coluna
e pulava a outra, e o 3D em meia resolução deixava o desenho quadrado. Quando um quadro não
cabe no retraço, os seguintes rodam sem desenhar até o relógio alcançar. Na entrada o
aplicativo pede 444 MHz no ARM e 222 no barramento; o que o aparelho aceitou fica no log. A
velocidade neste aparelho não foi medida. O processador aqui é o interpretador: o dynarmic não
emite ARMv7. A música usa a tabela de timbres.

## O que ainda não foi medido no aparelho

Vale para este aplicativo e para o core do RetroArch, que usam o mesmo núcleo.

- **A memória do jogo.** Na Vita o heap do jogo é de 48 MiB, e não de 64 como no desktop: com 64
  o vitaGL não achava o bloco temporário do quadro. A região que o emulador reserva para si
  (`INTERNAL_BASE`, em `src/loader/mod.rs`) soma mais 16 MiB, e aqui a newlib reserva o bloco
  inteiro, sem o overcommit do desktop. Ninguém mediu ainda se 48 + 16 deixam o quadro caber,
  nem se o Quake roda com 48. Se um jogo fechar sem aviso ou disser que ficou sem
  memória, este é o primeiro suspeito.
- **Save state entre plataformas não vale.** Com o heap de outro tamanho, um estado gravado no
  desktop, no Android ou no core de outra plataforma é recusado aqui, e o daqui é recusado lá.
  O emulador diz que a região não bate e o jogo segue como estava; nada é corrompido.

## Onde ficam os jogos

`ux0:data/zeebx/roms`, no VitaShell. Um `.zip` ou um `.mod` servem, na mesma varredura do desktop.
Os saves ficam em `ux0:data/zeebx/saves`. O que o aplicativo anota — inclusive um pânico — vai
para `ux0:data/zeebx/zeebx.log`.

## Os botões

| Vita | Zeebo |
|---|---|
| X | Botão 1, embaixo |
| Quadrado | Botão 2, à esquerda |
| Triângulo | Botão 3, em cima |
| Círculo | Botão 4, à direita |
| L | ZL |
| R | ZR |
| Start | Start |
| Select | HOME |
| Direcional | Direcional |
| Analógico esquerdo | Manche |
| Analógico direito | O outro manche |

Na lista, o direcional escolhe, X inicia, O fecha o aplicativo e Triângulo relê a pasta. Segurar
Select e Start por meio segundo sai do jogo e volta à lista. O manche parado não anda: há uma
zona morta pequena em volta do centro.

## Compilar

A cadeia é a imagem `vitasdk/vitasdk`. No Mac:

```bash
./frontends/vita-standalone/compilar.sh
```

O VPK sai em `frontends/vita-standalone/saida/zeebx.vpk`. Dentro da imagem, ou no job `vita.yml`:

```bash
./frontends/vita-standalone/compilar.sh --local
```

`cargo test -p zeebx-vita` prova o mapeamento, a lista e o desenho do quadro no desktop. Não sobe
o aparelho.
