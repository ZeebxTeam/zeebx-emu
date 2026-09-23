# O Zeebx no Android

O núcleo é o mesmo. O que muda é quem o gira.

A emulação — o BREW, a CPU, o rasterizador, o carregador — virou a biblioteca `zeebx`, e três
coisas a consomem: o binário de desktop (`frontends/classical-standalone`, com a janela do egui e a linha de
comando), os testes, e o `.so` deste diretório. Nenhuma linha de emulação sabe em qual dos três
está rodando.

Este diretório é `frontends/android/` porque ele é **um** frontend, não *o* frontend do Android:
quando houver outro alvo — um console, um navegador —, ele entra ao lado, e o núcleo continua
onde está.

## O corte

O que depende de uma janela de desktop está atrás de `cfg(not(target_os = "android"))`:

| Fica em toda plataforma | Só no desktop |
|---|---|
| `audio`, `brew`, `cpu`, `input`, `loader`, `machine`, `video`, `session` | `ui::app` (a janela do egui), `ui::window`, `ui::atualizacao`, `ui::discord` |
| `ui::settings`, `ui::library`, `ui::acervo`, `ui::saves`, `ui::i18n`, `ui::gpu` | `glutin`, `gilrs`, `minifb`, `rfd`, `ureq`, `discord-rich-presence` |

A segunda linha da primeira coluna é o ponto: a biblioteca de jogos, os ajustes e os saves são
lidos **pelo núcleo** — o `machine`, o `loader` e a `session` falam com eles —, então não são
interface, apesar de morarem em `src/ui/`.

## O frontend

Nenhuma linha de Java ou Kotlin: a `NativeActivity` do sistema carrega a `libzeebx_android.so`
e chama o `android_main` dela.

| Arquivo | O que é |
|---|---|
| `src/lib.rs` | o laço de eventos, o estado do aplicativo e por onde cada tela entra |
| `src/tela.rs` | a `ANativeWindow` virando superfície EGL, o contexto que sobrevive a ela, e o `egui_glow` |
| `src/entrada.rs` | o evento do Android virando botão do Zeebo e ponteiro do egui |
| `src/biblioteca.rs` | a grade de jogos, com as capas que o `library::scan` do núcleo já lê |
| `src/ajustes.rs` | as configurações, sobre o mesmo `Settings` e o mesmo catálogo de idiomas do desktop |
| `src/tema.rs` | o tamanho das coisas numa tela que se segura com as mãos |
| `src/widgets.rs` | as peças de toque: a chave de luz, o segmentado, a faixa de opção |
| `src/seletor.rs` | o navegador de pastas |
| `src/jogo.rs` | o quadro na tela, o painel de depuração e a pergunta do "voltar" |
| `src/sistema.rs` | as duas chamadas de JNI: a permissão de arquivos e abrir um endereço |

**O conteúdo é o mesmo do desktop; a forma não.** A grade usa o `library::scan`, que é o mesmo
que acha os jogos no desktop; os ajustes escrevem o mesmo `Settings`, no mesmo formato; os
rótulos saem do mesmo `ui::i18n`, cujos idiomas de fábrica vêm embutidos no binário; e o
painel de velocidade é literalmente o `ui::depuracao::painel`. O que ficou de fora ficou por não
existir aqui: as opções de janela não valem numa tela só, e os controles, o Discord e as
atualizações ainda não estão ligados neste frontend.

A forma, essa é outra — e a primeira tentativa, que era a janela do desktop encolhida, não
servia. Quatro coisas mudaram, e as quatro por causa do aparelho:

- **Tamanho.** Corpo de texto em 17 e dica em 14, não 12 e 9; alvo de toque de 48 pontos, não
  18. Um portátil fica a meio braço do rosto, não a trinta centímetros, e quem aponta é o
  polegar, não uma seta de um pixel.
- **A caixinha virou chave.** Um `checkbox` de 18 pontos não se lê nem se acerta; uma chave de
  luz diz o estado pela posição e pela cor. As bolinhas do `radio_button` viraram botões
  segmentados pelo mesmo motivo.
- **As dicas ficam fechadas.** Os textos de ajuda do Zeebx são longos — alguns têm cinco linhas
  —, e todos abertos viram uma parede cinza em que não se acha o que se procurava. Cada linha
  tem um `?`, e só uma dica abre por vez.
- **O direcional anda na grade.** Num aparelho com botões, chegar ao jogo sem encostar na tela é
  o caminho normal e não a alternativa: um cartão fica marcado, o direcional o move, a rolagem o
  acompanha e o botão 1 abre.

**Não há eframe aqui, e é de propósito.** Pelo winit — que é o que o eframe gira — o controle se
perde duas vezes: os códigos de tecla de um `Gamepad` viram `Key::Unidentified`, que o egui
descarta, e os eixos do manche são `MotionEvent`, que o winit só lê quando são toque. Interceptar
em Java também não resolve: a `NativeActivity` entrega a entrada pela fila nativa e **não passa
pela hierarquia de views**, então um `dispatchKeyEvent` nosso nunca seria chamado — foi medido.
Girando a `android-activity` direto, chega o que o Android mandou: `Keycode::ButtonA`, `Axis::X`,
os gatilhos e o "voltar".

O mapa de botões é o mesmo que o desktop dá a um controle moderno: sul no `b1`, leste no `b2`,
oeste no `b3`, norte no `b4`, os gatilhos superiores no `zl`/`zr`, o Start no `back` — o controle
do Zeebo não tem Start, e quem ocupa o lugar dele é o HOME.

### O 3D na placa

O contexto de GL do `tela.rs` é o mesmo que a sessão recebe, então o "preencher o 3D na placa de
vídeo" do desktop vale aqui. **E é ele que dá vida a quatro outras opções:** no rasterizador de
software o `define_proporcao`, o `define_escala`, o `define_antialias` e o anisotrópico são
funções de corpo vazio — só a placa sabe fazer aquilo. Com o 3D na CPU, mexer na proporção larga
ou na resolução interna não fazia absolutamente nada, e por isso as quatro ficam apagadas
enquanto a placa está desligada.

**O contexto e a superfície têm vidas diferentes, e o `tela.rs` as separa por isso.** A janela do
Android nasce e morre a cada troca de aplicativo; o contexto não pode ir junto, porque o
rasterizador guarda nele texturas, buffers e programas. Destruí-lo ao ir para o segundo plano
deixaria a sessão com nomes de objetos que não existem mais, e o primeiro desenho na volta seria
uma falha dentro do driver. Então a `Placa` nasce com a primeira janela e vive até o fim; a
`Tela` é só a superfície, refeita a cada janela.

### A pasta de ROMs

Ela é escolhida no aplicativo e guardada nos mesmos ajustes do desktop. O padrão é
`/sdcard/zeebo/roms`, e lê-lo exige o "acesso a todos os arquivos", que o próprio aplicativo pede.
Quem prefere não conceder nada usa o diretório privado, que sempre funciona:

```
adb push jogo.mod /sdcard/Android/data/io.github.zeebxteam.zeebx/files/roms/
```

O seletor de pastas é nosso, sobre o `std::fs`, e não o do sistema: o `ACTION_OPEN_DOCUMENT_TREE`
devolve um `content://`, e o carregador do núcleo abre caminho de arquivo.

## Compilar

```
./frontends/android/compilar.sh          # só o .so
./frontends/android/compilar.sh --apk    # o .so e a APK
```

A APK sai em `apk/app/build/outputs/apk/debug/`, com dois nomes: o `app-debug.apk` do Gradle,
que é de onde o `installDebug` e o Android Studio se servem, e uma cópia como
`zeebx-android-<abi>.apk`, que é o nome que a release usa — ao lado de um
`zeebx-standalone-linux-x86_64.deb`, um `app-debug.apk` não diz de qual programa é.

Este mesmo script é o que roda no CI, sem uma segunda cópia dos caminhos dentro do workflow: ele
aceita o `ANDROID_SDK_ROOT` que os runners do GitHub exportam e o `gradle` que estiver no
caminho, caindo no `$HOME` quando não há nenhum dos dois. Ver
[`release.yml`](../../.github/workflows/release.yml).

O script espera a toolchain no `$HOME`, sem `sudo`:

| O quê | Onde | De onde |
|---|---|---|
| rustup + alvo `aarch64-linux-android` | `~/.cargo` | `https://sh.rustup.rs` |
| JDK 17 | `~/Android/jdk` | Adoptium |
| SDK (platform 35, build-tools 35) e NDK r27.3 | `~/Android/sdk` | `sdkmanager` |
| Gradle 8.11 | `~/Android/gradle` | `services.gradle.org` |
| `cargo-ndk` | `~/.cargo/bin` | `cargo install cargo-ndk` |

Os caminhos são ajustáveis por ambiente: `ANDROID_SDK_HOME`, `ANDROID_NDK_VERSAO`, `JAVA_HOME`,
`GRADLE`, `ZEEBX_ANDROID_ABI`.

## Duas pedras no caminho, e por que elas existem

**O CMake não achava o NDK.** O `unicorn` e o `dynarmic` compilam C e C++ por CMake, e o
`cmake-rs` monta a linha de comando sozinho: ele põe `CMAKE_SYSTEM_NAME=Android` e
`--target=aarch64-linux-android35` nas flags, mas não diz a ABI ao toolchain do NDK. Sem ela o
NDK assume `armeabi-v7a` e acrescenta `-march=armv7-a`, que o clang recusa junto de um alvo
aarch64 — e o build morre no teste do compilador, antes de qualquer código nosso. O
`ndk-toolchain.cmake` daqui fixa a ABI e a API antes de ler o toolchain do NDK.

**`-lpthread` não existe.** No bionic a pthread e a rt vivem dentro da libc; não há
`libpthread.so` nem `librt.so`. O `unicorn-engine-sys` pede as duas por nome, sem olhar o
sistema. O `compilar.sh` gera arquivos vazios com esses nomes em `target/android-libs-vazias` e
os põe no caminho do ligador: ele acha o nome, não acha símbolo nenhum, e segue.

## O que ainda não está aqui

- **Som.** O `cpal` tem backend de AAudio e já compila, mas ninguém o liga ainda.
- **Controle remapeável.** O mapa de botões é fixo no `entrada.rs`. O `input::bindings`, que é
  quem o desktop usa para deixar o usuário remapear, ainda não está ligado aqui.
- **Sensores e Wii Remote.** Continuam atrás de `cfg(target_os = "linux")`.
