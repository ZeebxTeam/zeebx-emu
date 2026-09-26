# O Zeebx no iOS

O núcleo é o mesmo. O que muda é quem pinta e quem aperta o botão.

A emulação — o BREW, a CPU, o rasterizador, o carregador — é a biblioteca `zeebx`. Este
diretório é o frontend: uma biblioteca estática em Rust e um projeto Xcode em Swift. A Swift
não emula nada. Ela pede uma volta, recebe um quadro em BGRA e devolve o toque.

## Por que não é o Dynarmic

No desktop o núcleo recompila os blocos ARM para o código da máquina. No iOS isso não cabe. O
alocador que o Dynarmic usa num host ARM64, o oaknut, mapeia a página como executável e depois
troca a proteção com `mprotect`. O iOS recusa essa troca para um aplicativo de terceiro — a
página executável é do sistema. O simulador também é `TARGET_OS_IPHONE`, então a recusa vale
nos dois.

O interpretador em `src/cpu/interpretador.rs` ocupa esse lugar, o mesmo que já ocupa no
`wasm32`. Não foi medido contra o JIT. Um jogo que no desktop corre folgado aqui anda mais
devagar; o que não muda é a conversa com o BREW.

## O corte

| Fica | Não entra neste frontend |
|---|---|
| `audio` (o `cpal`, que no iOS é Core Audio) | `desktop`: eframe, glutin, gilrs, rfd |
| rasterizador de software | rasterizador na placa — o contexto fora de tela do núcleo pede EGL, e aqui não há |

O quadro que a sessão publica já é o do aparelho, em RGB565. A biblioteca converte para BGRA8
uma vez por volta, que é o layout que um `CGImage` little-endian com o alfa na frente aceita.

## Onde ficam as ROMs

`Documents/roms`, dentro do sandbox. O `Info.plist` liga o compartilhamento de arquivos, então
a pasta aparece no app Arquivos e, com o aparelho ligado no Finder, na aba do Zeebx. Um `.mod`
solto ou um `.zip` servem: quem escolhe o módulo dentro do pacote é o mesmo `library::scan` do
desktop.

Cache, saves e o aparelho virtual ficam em `Library/Application Support`, no layout de
`StoragePaths` — o pacote não é onde o jogo grava.

## Compilar

Mac com Xcode, e os dois alvos no rustup:

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
```

```bash
./frontends/ios/compilar.sh              # a biblioteca do simulador
./frontends/ios/compilar.sh --aparelho   # a biblioteca do aparelho
./frontends/ios/compilar.sh --app        # o .app do simulador
./frontends/ios/compilar.sh --app-aparelho
```

O `.app` do simulador sai em `frontends/ios/build/iphonesimulator/`. O do aparelho, em
`frontends/ios/build/iphoneos/`. Sem `DEVELOPMENT_TEAM` o do aparelho não é assinado. Esse
binário é o `zeebx-ios.ipa`, dentro de `zeebx-ios.zip`: o AltStore clássico assina com o Apple ID
de quem instala. `./frontends/ios/compilar.sh --pacote` faz dois zips, cada um com o seu leia-me:
[`LEIA-ME-simulador.txt`](LEIA-ME-simulador.txt) no do simulador e
[`LEIA-ME-altstore.txt`](LEIA-ME-altstore.txt) no do aparelho. Para instalar com a sua conta de desenvolvedor:

```bash
DEVELOPMENT_TEAM=XXXXXXXXXX ./frontends/ios/compilar.sh --app-aparelho
```

O Xcode chama o mesmo script na fase de build (`--na-fase`). Não há uma segunda lista de alvos
dentro do `project.pbxproj`.

O simulador que este script produz é arm64, o do Mac com Apple Silicon. O simulador Intel
(`x86_64-apple-ios`) ficou de fora: o Xcode atual já não o oferece como destino.

## O que a tela faz

A lista é o que a varredura achou. Abrir um jogo entrega o controle na primeira porta — o mesmo
`PORTAS_PADRAO` do desktop — e liga o som. O limite de velocidade fica ligado: este frontend
ainda não tem a tela de ajustes, e um telefone que corre adiantado só gasta bateria.

O toque cobre o direcional, os quatro botões de face e os gatilhos L e R. Um controle de verdade
entra pelo `GCController`, no mesmo mapa do Android: sul no `b1`, leste no `b2`, oeste no `b3`,
norte no `b4`. No simulador, o teclado é o da janela de desktop: setas, Z/X/C/V, Q/W, e
H/Backspace/Enter no HOME.

## O que ainda não está aqui

- **A tela de ajustes.** Resolução, velocidade e o mapa de botões são os padrões.
- **O 3D na placa.** Não há contexto de GL nosso. O jogo desenha no rasterizador de software.
- **JIT.** Depende de uma permissão que a Apple não dá a aplicativo de terceiro.
