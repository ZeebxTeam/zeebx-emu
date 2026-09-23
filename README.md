# Zeebx 

Emulador do Zeebo, o console que a TecToy lançou em 2009 no Brasil e no México.

O Zeebo era digital-only: os jogos vinham da loja da TecToy, que saiu do ar. Sobraram cerca de
60 títulos que só rodam em quem ainda tem o aparelho ou por meio de modding no console. 
Este projeto existe para ajudar a preservar essas pérolas que fizeram parte da nossa história.

Em desenvolvimento. Hoje 56 dos 62 títulos de teste passam do carregamento e desenham.

## Links úteis

[Servidor Discord: https://discord.gg/D96HjsKTPa](https://discord.gg/D96HjsKTPa)

[GitHub: https://github.com/ZeebxTeam](https://github.com/ZeebxTeam)

# Notas para Colaboradores

Por favor, ao abrir uma PR, sempre aponte para a branch development ou a branch correspondente ao ajuste que está sendo feito.
Não abra PR para a branch master, visto que é onde organizamos e concentramos nossos CI de build de relases.

Para novos targets de frontend, siga sempre a regrinha de mantê-lo dentro da pasta "frontends", exemplo:
frontends/android/
frontends/headless/
frontends/libretro/
frontends/standalone-qt/

E também ajuste o [.github/workflows/release.yml](release.yml) para apontar um alvo de build durante nosso CI, assim garante que o target seja fornecido junto durante a criação da release!

Esses são detalhes sugeridos apenas para manter a organização do nosso repositório!

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

## Compilando

Rust 1.88 ou mais novo.

```bash
cargo build --release
```

### O que mais precisa estar instalado

O standalone usa dependências nativas para `unicorn-engine`, `dynarmic`, áudio, janela e controles.
Debian, Ubuntu e derivados:

```bash
sudo apt install build-essential cmake ninja-build pkg-config python3 clang libclang-dev \
    libglib2.0-dev libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev
```

O core Libretro não linka a interface desktop nem bibliotecas de áudio/controle do host:

```bash
cargo build --release -p zeebx-libretro
```

Para conferir dependências antes do build standalone:

```bash
python3 ferramentas/prepara_build.py
```

### Instaladores e releases

Os instaladores saem do [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), com a
configuração em `[package.metadata.packager]` no `Cargo.toml`:

```bash
cargo install cargo-packager --locked
cargo packager --release --formats deb,appimage   # Linux
cargo packager --release --formats nsis           # Windows
cargo packager --release --formats dmg            # macOS
```

Os arquivos ficam em `target/pacotes/`. No Arch, o AppImage precisa de `NO_STRIP=1`: o `strip` do
linuxdeploy não reconhece as bibliotecas do sistema.

Uma tag de versão (`v0.1.0` ou `0.1.0`) enviada ao GitHub dispara o
[`release.yml`](.github/workflows/release.yml), que monta a release como rascunho, com o título
igual à tag. O emulador procura versões novas nessas releases ao abrir.

São dois formatos em cada um dos quatro sistemas, e o nome do arquivo diz qual é qual:

| | |
|---|---|
| `zeebx-standalone-linux-x86_64.deb`, `.AppImage` | o emulador com a interface, para instalar |
| `zeebx-standalone-windows-x86_64-setup.exe` | idem, no Windows |
| `zeebx-standalone-macos-arm64.dmg`, `-x86_64.dmg` | idem, nos dois Macs |
| `zeebx-headless-<sistema>.zip` | o binário sem interface, com o `config.ini` e o leia-me |
| `zeebx-android-arm64-v8a.apk` | o aplicativo de Android |

A APK sai assinada com a **chave de depuração**, que é a que o Gradle gera sozinho: serve para
instalar de lado (`adb install`), não para a Play Store — aquela pede a chave de publicação, que
não pode morar num repositório público. O mesmo
[`compilar.sh`](frontends/android/compilar.sh) que se usa na máquina é o que roda no CI; ele
aceita o `ANDROID_SDK_ROOT` que os runners exportam e o `gradle` que estiver no caminho.

## Usando

Sem argumentos, abre a interface. Pela linha de comando:

```bash
cargo run --release -- run "roms/Quake.zip" --window
```

Zips são extraídos para um cache e o `.mod` certo é escolhido sozinho. `--seconds=N` define
quantos segundos de tempo virtual emular quando não há janela; com janela, roda até você fechar.

Os controles no teclado:

| Tecla | Controle do Zeebo |
|---|---|
| Setas | direcional |
| Z, X ou Espaço, C, V | botões 1, 2, 3 e 4 |
| Q, W | ZL e ZR |
| F, G | analógico esquerdo, direito |
| H, Backspace, Enter | HOME |

O `run` informa onde o jogo parou, o que ele pediu e não temos, e o log que os próprios
desenvolvedores deixaram no binário — por `DBGPRINTF` e por semihosting do ARM. Esse relatório é
o backlog do projeto. As opções de depuração estão em [ARCHITECTURE.md](ARCHITECTURE.md).

### Com um frontend seu

Quem já tem um frontend — um que simula a carcaça do console, uma estante de jogos, um gabinete
de fliperama — não quer a interface do Zeebx por cima da tela que ele mesmo montou. Para isso há
um binário sem interface nenhuma, configurado por um `config.ini`:

```bash
cargo build --release -p zeebx-headless
./target/release/zeebx-headless "roms/Quake.zip"
```

O jogo é obrigatório e não há padrão: este binário é chamado por outro programa, que sabe o que
quer abrir. Na primeira execução ele escreve um `config.ini` completo e comentado, e diz onde.
As opções e as chaves do arquivo são em inglês, como os comandos; os comentários são em
português.

Ele abre uma janela só com o jogo — ou nenhuma, mandando os quadros por um cano para o seu
programa pintar. Gráficos, áudio e controles saem dos mesmos campos que a interface grava, só
que em INI. Ver [`frontends/headless/LEIAME.md`](frontends/headless/LEIAME.md).

## Core Libretro e muOS

O core Libretro é empacotado com o `.info` e pode ser instalado no RetroArch. O cartão muOS usa o
core AArch64 em `opt/muos/share/core` e o banco MIDI é opcional. A instalação documentada está em
[`docs/libretro/LIBRETRO_PLAN.md`](docs/libretro/LIBRETRO_PLAN.md). A playlist, o DAT e as capas de
Zeebo são gerados pelas ferramentas da pasta `ferramentas/`.

## Plataformas

Linux, Windows e macOS. Mobile está nos planos, mas o foco agora é no desktop!

## Jogos

O repositório não distribui jogos. Coloque os seus em `roms/`, que é ignorada pelo git ou em qualquer outra pasta, e defina nas configurações do programa.

## Documentação

- [ARCHITECTURE.md](ARCHITECTURE.md) — como o emulador é feito
- [docs/](docs/README.md) — a pesquisa: o console, a plataforma BREW, os formatos de arquivo e o
  estado da arte da emulação de Zeebo
- [docs/implementacao/](docs/implementacao/README.md) — cada subsistema, com as decisões e o
  porquê de cada uma
- [TODO.md](TODO.md) — o diário de bordo: o que está pronto, o que falta e o que já custou caro

## Licença

GPL-2.0, o texto completo em [LICENSE](LICENSE).


## Menções

- **tripleoxygen** — engenharia reversa de hardware e firmware do Zeebo, e o material público
  que torna este projeto possível :)
