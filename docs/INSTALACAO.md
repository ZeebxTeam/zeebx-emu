# Instalação

As versões prontas ficam nas [releases](https://github.com/ZeebxTeam/zeebx-emu/releases): o
instalador do Windows, o `.dmg` do macOS, o `.deb` e o AppImage do Linux, a APK do Android e o core
Libretro.

## Homebrew

No **macOS** e no **Linux** dá para instalar pelo [Homebrew](https://brew.sh), com o
[tap do projeto](https://github.com/ZeebxTeam/homebrew-tap):

```bash
brew install --cask zeebxteam/tap/zeebx-qt
```

No macOS vai para Aplicativos (pede o macOS 13 ou mais novo); no Linux, o AppImage vai para
`~/Applications/zeebx-qt.AppImage`. A interface antiga, em egui, foi até a 0.4.2: o cask
`zeebxteam/tap/zeebx` fica nela.

## Qual arquivo baixar

São dois formatos em cada um dos quatro sistemas, e o nome do arquivo diz qual é qual:

| | |
|---|---|
| `zeebx-standalone-qt-linux-x86_64.deb`, `.AppImage` | o emulador com a interface, para instalar |
| `zeebx-standalone-qt-windows-x86_64-setup.exe` | idem, no Windows |
| `zeebx-standalone-qt-macos-arm64.dmg`, `-x86_64.dmg` | idem, nos dois Macs |
| `zeebx-headless-<sistema>.zip` | o binário sem interface, com o `config.ini` e o leia-me |
| `zeebx-android-arm64-v8a.apk` | o aplicativo de Android |
| `zeebx-ios-simulator.zip` | o simulador e o leia-me |
| `zeebx-ios.zip` | o IPA do AltStore clássico e o leia-me |

## Atualizações

O emulador se atualiza sozinho no AppImage, no instalador do Windows e no `.app` do macOS: quando
sai uma versão nova, ele avisa e oferece "Atualizar agora". Nas configurações, em Atualizações, dá
para desligar a procura ao abrir e escolher se as versões de pré-lançamento contam.

## macOS: primeira abertura

No macOS, a primeira abertura avisa que a Apple não pôde verificar o Zeebx: esta build é assinada
ad-hoc, e não pela Apple. Ela se libera em Ajustes do Sistema > Privacidade e Segurança > "Abrir
Mesmo Assim", ou com `xattr -dr com.apple.quarantine "/Applications/Zeebx.app"` depois de arrastar
para Aplicativos (o do Qt se chama `Zeebx Qt.app`). A imagem traz um `LEIA-ME.txt` ao lado do aplicativo. Em Mac com chip da Apple,
use o `macos-arm64`: o `macos-x86_64` roda pelo Rosetta, e foi nele que os jogos pararam na issue 53.

## Android

A APK sai assinada com a **chave de depuração**, que é a que o Gradle gera sozinho: serve para
instalar de lado (`adb install`), não para a Play Store — aquela pede a chave de publicação, que
não pode morar num repositório público.

## iOS

O simulador é arm64. No aparelho, o `zeebx-ios.zip` traz o IPA para o AltStore clássico assinar com
um Apple ID. Cada zip traz o seu `LEIA-ME.txt`: o do simulador e o do aparelho. Para montar o iOS a
partir do código, ver [COMPILANDO.md](COMPILANDO.md#ios).