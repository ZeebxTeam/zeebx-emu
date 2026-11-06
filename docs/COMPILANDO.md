# Compilando

Rust 1.88 ou mais novo.

```bash
cargo build --release
```

## O que mais precisa estar instalado

A interface é o `zeebx-qt`, em **Qt 6** (`frontends/qt-standalone`, Qt 6.4 ou mais novo). A linha de
comando — `zeebx run`, `zeebx sessao`, `zeebx bench`, para medir e depurar — é o `zeebx`
(`frontends/cli`), sem Qt. Os dois usam dependências nativas para `dynarmic`, áudio e controles; o
Qt só a interface pede. A interface em egui saiu depois da 0.4.2. Debian, Ubuntu e derivados:

```bash
sudo apt install build-essential cmake ninja-build pkg-config python3 clang libclang-dev \
    libglib2.0-dev libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev \
    qt6-base-dev qt6-base-dev-tools qt6-declarative-dev qt6-declarative-dev-tools qmake6 \
    qt6-wayland qml6-module-qtquick qml6-module-qtquick-controls qml6-module-qtquick-layouts \
    qml6-module-qtquick-shapes \
    qml6-module-qtquick-templates qml6-module-qtquick-window qml6-module-qtqml-workerscript
```

No Arch: `qt6-base qt6-declarative qt6-wayland`. O build acha o Qt pelo `qmake6` ou pelo `qmake`;
para usar outro, aponte `QMAKE` para ele.

```bash
cargo build --release -p zeebx-standalone-qt     # target/release/zeebx-qt, a interface
cargo build --release -p zeebx-cli               # target/release/zeebx, a linha de comando
```

Para conferir dependências antes do build standalone:

```bash
python3 ferramentas/prepara_build.py
```

## Libretro

O core Libretro não linka a interface desktop nem bibliotecas de áudio/controle do host:

```bash
cargo build --release -p zeebx-libretro
```

Para instalar o core no RetroArch ou no muOS, ver [CORE-LIBRETRO.md](CORE-LIBRETRO.md).

## Sem interface

O binário para quem já tem um frontend próprio:

```bash
cargo build --release -p zeebx-headless
```

Ver [HEADLESS.md](HEADLESS.md).

## PlayStation Vita

O core da PlayStation Vita é uma biblioteca estática, no mesmo molde do Switch. O script e o
que o RetroArch espera dele estão em [`frontends/vita/compilar.sh`](../frontends/vita/compilar.sh).

O aplicativo que abre sozinho na Vita, sem RetroArch, é o VPK de
[`frontends/vita-standalone`](../frontends/vita-standalone/LEIAME.md).

## Android

O mesmo [`compilar.sh`](../frontends/android/compilar.sh) que se usa na máquina é o que roda no CI;
ele aceita o `ANDROID_SDK_ROOT` que os runners exportam e o `gradle` que estiver no caminho. A APK
sai assinada com a chave de depuração (ver [INSTALACAO.md](INSTALACAO.md#android)).

## iOS

O iOS sai de um Mac com Xcode. O núcleo vira uma biblioteca estática e o Xcode monta o `.app`:

```bash
./frontends/ios/compilar.sh --app
```

O simulador é arm64. No aparelho o mesmo script com `--app-aparelho` pede um time de
desenvolvimento (`DEVELOPMENT_TEAM`) para instalar direto; sem ele, o `.app` sai sem assinatura
e o `--pacote` o coloca, como `zeebx-ios.ipa`, dentro de `zeebx-ios.zip`, para o AltStore clássico
assinar com um Apple ID. Cada zip traz o seu `LEIA-ME.txt`: o do simulador e o do aparelho. O iOS não deixa o
processo mapear código executável, então nesse alvo o núcleo é o interpretador, não o Dynarmic.
Ver [`frontends/ios/LEIAME.md`](../frontends/ios/LEIAME.md).

## Instaladores

Para montar os instaladores e as releases, ver [INSTALADORES.md](INSTALADORES.md).