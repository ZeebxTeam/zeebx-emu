# Zeebx

Emulador do Zeebo, o console que a TecToy lançou em 2009 no Brasil e no México.

O Zeebo era digital-only: os jogos vinham da loja da TecToy, que saiu do ar. Sobraram cerca de
60 títulos que só rodam em quem ainda tem o aparelho ou por meio de modding no console.
Este projeto existe para ajudar a preservar essas pérolas que fizeram parte da nossa história.

Em desenvolvimento. Hoje 56 dos 62 títulos de teste passam do carregamento e desenham.

![Screenshot](docs/screenshot.png)

## Links úteis

[Servidor Discord: https://discord.gg/D96HjsKTPa](https://discord.gg/D96HjsKTPa)

[GitHub: https://github.com/ZeebxTeam](https://github.com/ZeebxTeam)

## Instalação

As versões prontas ficam nas [releases](https://github.com/ZeebxTeam/zeebx-emu/releases): o
instalador do Windows, o `.dmg` do macOS, o `.deb` e o AppImage do Linux, a APK do Android e o core
Libretro. No **macOS** e no **Linux** também dá para instalar pelo [Homebrew](https://brew.sh):

```bash
brew install --cask zeebxteam/tap/zeebx-qt
```

Qual arquivo é qual, as atualizações automáticas e o aviso da primeira abertura no macOS estão em
[docs/INSTALACAO.md](docs/INSTALACAO.md).

## Como funciona

Emular Zeebo não é emular um console: é reimplementar o Qualcomm BREW 4.0.2. O jogo é um binário
ARM que nunca toca hardware — ele chama interfaces do sistema por tabelas de ponteiros. Então o
caminho é executar o código ARM num núcleo emulado e atender cada chamada de API no host.

As vtables que entregamos ao jogo apontam para endereços que **não existem** no mapa de memória.
Quando o jogo chama um método, o núcleo aborta a busca de instrução e o endereço nos diz qual
interface e qual método foram pedidos. Não há stub, nem código de cola.

O desenho completo está em [ARCHITECTURE.md](ARCHITECTURE.md).

## Compatibilidade

A maioria das ROMs rodam sem problemas, alguns jogos podem apresentar travamentos antes da inicialização ou durante a execução.

Jogos que utilizam do Boomerang podem ser jogados usando Wii Remote e seus sensores de movimento!

Jogos 3D são compatíveis com recursos de resolução experimentais, podendo atingir resoluções de até 4k em 16:9.

O estado de cada título, com os endereços de cada parada, está em
[COMPATIBILIDADE.md](COMPATIBILIDADE.md).

Para frontends como Android e Libretro, essa listagem de compatibilidade pode não se aplicar. Pedimos que reportem quaisquer problemas nessas versões também.

## Usando

Sem argumentos, abre a interface. Pela linha de comando:

```bash
cargo run --release -- run "roms/Quake.zip" --window
```

Os controles de teclado, as opções da linha de comando e o relatório de depuração estão em
[docs/USO.md](docs/USO.md). Para integrar o emulador ao seu próprio frontend, há um binário sem
interface: ver [docs/HEADLESS.md](docs/HEADLESS.md).

## Compilando

Rust 1.88 ou mais novo.

```bash
cargo build --release
```

As dependências de cada plataforma e os builds da interface, da linha de comando, do Libretro, da
Vita, do Android e do iOS estão em [docs/COMPILANDO.md](docs/COMPILANDO.md). Como os instaladores e
as releases são montados está em [docs/INSTALADORES.md](docs/INSTALADORES.md).

## Plataformas

Linux, Windows e macOS. Mobile está nos planos, mas o foco agora é no desktop!

O core Libretro e o cartão muOS têm a própria página: [docs/CORE-LIBRETRO.md](docs/CORE-LIBRETRO.md).

## Jogos

O repositório não distribui jogos. Coloque os seus em `roms/`, que é ignorada pelo git ou em qualquer outra pasta, e defina nas configurações do programa.

## Documentação

- [ARCHITECTURE.md](ARCHITECTURE.md) — como o emulador é feito
- [docs/INSTALACAO.md](docs/INSTALACAO.md) — instalar, atualizar e qual arquivo da release baixar
- [docs/USO.md](docs/USO.md) — linha de comando, controles e relatório de depuração
- [docs/HEADLESS.md](docs/HEADLESS.md) — o binário sem interface para frontends próprios
- [docs/CORE-LIBRETRO.md](docs/CORE-LIBRETRO.md) — o core Libretro e o muOS
- [docs/COMPILANDO.md](docs/COMPILANDO.md) — dependências e builds por plataforma
- [docs/INSTALADORES.md](docs/INSTALADORES.md) — instaladores e releases
- [docs/](docs/README.md) — a pesquisa: o console, a plataforma BREW, os formatos de arquivo e o
  estado da arte da emulação de Zeebo
- [docs/implementacao/](docs/implementacao/README.md) — cada subsistema, com as decisões e o
  porquê de cada uma
- [TODO.md](TODO.md) — o diário de bordo: o que está pronto, o que falta e o que já custou caro

## Licença

O código do Zeebx é **GPL-2.0-or-later**: o texto da GPLv2 está em [LICENSE](LICENSE).

**Os binários que saem na release são distribuídos sob a GPLv3**, com o texto em
[LICENSE-GPL3](LICENSE-GPL3). Isso não é uma troca de licença: todos eles ligam bibliotecas que
são só Apache-2.0 (`ab_glyph`, `sevenz-rust2`, `zopfli`, `cpal`, `winit`…), e a Apache-2.0 combina
com a GPLv3 mas não com a GPLv2. O "or later" é o que permite essa combinação. O frontend Qt já
seria GPLv3 de qualquer forma, pelo Qt (ver
[21-migracao-para-qt.md](docs/implementacao/21-migracao-para-qt.md)).

Quem quiser o Zeebx sob a GPLv2 pode compilar o código, mas não com essas dependências.


## Quem faz o Zeebx

Todo mundo que já contribuiu com código para o Zeebx. A lista completa está na
[aba de contribuidores](https://github.com/ZeebxTeam/zeebx-emu/graphs/contributors) do GitHub.

<!-- contribuidores -->
<table>
  <tr>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/rebquaker"><img src="https://avatars.githubusercontent.com/u/329117678?v=4&s=100" width="100px;" alt="rebquaker"/><br /><sub><b>rebquaker</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/requeijaum"><img src="https://avatars.githubusercontent.com/u/5564635?v=4&s=100" width="100px;" alt="requeijaum"/><br /><sub><b>requeijaum</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/Meowni2"><img src="https://avatars.githubusercontent.com/u/330453534?v=4&s=100" width="100px;" alt="Meowni2"/><br /><sub><b>Meowni2</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/humbertodias"><img src="https://avatars.githubusercontent.com/u/9255997?v=4&s=100" width="100px;" alt="humbertodias"/><br /><sub><b>humbertodias</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/J0aoSiqueira"><img src="https://avatars.githubusercontent.com/u/107425883?v=4&s=100" width="100px;" alt="J0aoSiqueira"/><br /><sub><b>J0aoSiqueira</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/maskofsin"><img src="https://avatars.githubusercontent.com/u/171617836?v=4&s=100" width="100px;" alt="maskofsin"/><br /><sub><b>maskofsin</b></sub></a></td>
    <td align="center" valign="top" width="16.67%"><a href="https://github.com/lucmsilva651"><img src="https://avatars.githubusercontent.com/u/90426410?v=4&s=100" width="100px;" alt="lucmsilva651"/><br /><sub><b>lucmsilva651</b></sub></a></td>
  </tr>
</table>
<!-- /contribuidores -->

## Menções

- **tripleoxygen** — engenharia reversa de hardware e firmware do Zeebo, e o material público
  que torna este projeto possível :)
- **[Requeijaum](https://github.com/requeijaum)** — grande suporte ao entendimento de boa parte do
  BREW e resolução de bugs
- Os grupos **Zeebo Clube** (Facebook) e **Zeebo Eterno** (Telegram), pelas comparações com o
  console real que trazem fidelidade ao projeto, e toda a comunidade Zeebo que vem apoiando,
  testando e dando feedback desde o início <3