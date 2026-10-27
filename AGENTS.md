# Guia para agentes

Este arquivo é para uma IA que vai mexer neste repositório. Ele diz o que não se descobre lendo
um arquivo por vez: onde as coisas moram, o que não pode quebrar, e como este projeto escreve.

Humanos também podem ler — não há nada aqui que seja segredo do código.

## O que é o Zeebx

Um emulador do Zeebo, o console que a TecToy lançou em 2009. O aparelho roda **Qualcomm BREW
4.0.2**, e por isso a emulação é de alto nível: o jogo é um binário ARM que nunca toca hardware,
ele conversa com o sistema por tabelas de ponteiros de função. O que precisa ser fiel é a
conversa, não o silício. O desenho geral está em [`ARCHITECTURE.md`](ARCHITECTURE.md), e cada
subsistema tem o seu em [`docs/implementacao/`](docs/implementacao/README.md).

## O mapa

```
Cargo.toml          a biblioteca `zeebx` — o emulador inteiro, sem interface
src/                BREW, CPU, vídeo, áudio, carregador, sessão, save state
src/ui/             telas e estado compartilhados entre frontends (ver o aviso abaixo)
frontends/
  qt-standalone/          a interface: o binário `zeebx-qt`, em Qt Quick
  cli/                    a linha de comando: o binário `zeebx` (`run`, `sessao`, `bench`...), sem Qt
  headless/               sem interface, configurado por `config.ini`
  libretro/               o core do RetroArch
  android/                o aplicativo, sem uma linha de Java
  ios/                    o aplicativo de iOS: UIKit por cima de uma biblioteca estática
  switch/                 o `.a` estático que o ROMBundler liga
  vita/                   o `.a` estático que o RetroArch da Vita liga
  vita-standalone/        o VPK que abre sozinho, sem RetroArch
ferramentas/        scripts Python: catálogo, varredura, instalação do core
docs/               documentação; `patch-notes/` guarda as notas de cada versão
assets/             ícones, fontes e `lang/` — os idiomas de fábrica
```

**`src/ui/` não é só interface.** O `library`, o `settings`, o `saves`, o `acervo` e o `i18n`
moram ali mas são lidos **pelo núcleo** — o `machine`, o `loader` e a `session` falam com eles.
Só `ui::window` (a janela crua do `zeebx run --window`), `ui::atualizacao` e `ui::discord` são de
desktop de verdade; a janela da interface mora em `frontends/qt-standalone`.

## As features, e por que elas existem

| Feature | O que traz | Quem usa |
|---|---|---|
| `desktop` | gilrs, minifb, rfd, Discord, glutin — implica `gpu`, `audio`, `soundfont` | Qt, linha de comando, headless |
| `audio` | só o `cpal`, que no Android fala com o Oboe | Android, e o `desktop` |
| `gl` | o desenho na placa; **não puxa nada de host** | core, Android, e o `desktop` |
| `gpu` | criar contexto próprio (glutin) — implica `gl` | desktop |
| `soundfont` | MIDI por banco de amostras (rustysynth, Rust puro) | core e desktop |

**A regra da casa: o core Libretro não linka biblioteca de host.** Sem janela, sem placa de som,
sem controle. Quem entrega vídeo, áudio e entrada é o frontend. O `libretro.yml` cobra isso com
`ldd`, e um `dep:` novo no lugar errado quebra o build do core.

Antes de pôr um `#[cfg(feature = "desktop")]` em algo, pergunte se aquilo é **mesmo** de desktop.
Três gates errados já quebraram o build do Android sem ninguém perceber, porque o APK só é montado
na tag.

## Compilar e provar

A interface é o frontend Qt (`zeebx-standalone-qt`, binário `zeebx-qt`), e é ela que sai nos
instaladores. A linha de comando (`zeebx-cli`, binário `zeebx`) é a bancada de medir e depurar, e
não pede Qt. A interface do egui saiu depois da 0.4.2. O Qt pede o **Qt 6** (6.4 ou mais novo),
que o build acha pelo `qmake6`, pelo `qmake` ou pelo `QMAKE`.
`python3 ferramentas/prepara_build.py` diz o que falta. A interface Qt está em
[`docs/implementacao/21-migracao-para-qt.md`](docs/implementacao/21-migracao-para-qt.md).

```bash
cargo build --release --locked -p zeebx-cli                 # o binário `zeebx`
cargo build --release --locked -p zeebx-standalone-qt       # o binário `zeebx-qt`
cargo test  --release --locked -p zeebx -p zeebx-cli -p zeebx-standalone-qt
timeout 60 ./target/release/zeebx controles                 # sobe de verdade, sem tela

cargo build --release --locked -p zeebx-headless
cargo build --release --locked -p zeebx-libretro            # sozinho: ver abaixo
python3 ferramentas/verifica_core.py target/release/libzeebx_libretro.so
```

**O core Libretro compila num comando só dele.** Junto com outro pacote, o Cargo une as features
do núcleo que os dois pedem, e o core herda o `audio` do outro: um código que só compila com som
passa no comando conjunto e quebra no `libretro.yml`, que o compila sozinho. Já aconteceu com o
`set_audio` da `ui::partida`.

O `verifica_core.py` abre a biblioteca como o frontend a abre (`dlopen`) e chama a ABI. Não é
`nm` numa lista de símbolos: ele pega dependência que faltou no link, que a leitura de símbolos
não pega.

**Android** precisa da cadeia no `$HOME`, sem sudo — a tabela está em
[`frontends/android/LEIAME.md`](frontends/android/LEIAME.md):

```bash
export PATH="$HOME/.cargo/bin:$HOME/Android/gradle/bin:$PATH"
export JAVA_HOME="$HOME/Android/jdk"
./frontends/android/compilar.sh --apk
```

**Switch** (o `.a` estático que o ROMBundler liga) mora em
[`frontends/switch/compilar.sh`](frontends/switch/compilar.sh). O `cfg(zeebx_switch)` desse
script é o que reduz o cache de código do dynarmic e aumenta o número de sons guardados. No
Mac a imagem Docker `rombundler-switch` traz o devkitA64 e o rustc; o job `core-switch` do
`libretro.yml` roda o mesmo script com `--local` dentro da imagem `devkitpro/devkita64`. A
release faz o mesmo e publica `zeebx_libretro-switch.zip`. O `.a` não liga sozinho: os
`switch_jit_*` e `switch_spinlock_*` que o patch do dynarmic chama vêm do ROMBundler.

**Vita** (o `.a` estático que o RetroArch liga) mora em
[`frontends/vita/compilar.sh`](frontends/vita/compilar.sh). O alvo é o
`armv7-sony-vita-newlibeabihf`, tier 3: a std é recompilada com `-Z build-std` e o
vitasdk fornece o `arm-vita-eabi-gcc`. O dynarmic fica de fora — ele não emite ARMv7 —,
então o núcleo é o interpretador, e o desenho fica no processador porque o vitaGL não
entrega OpenGL ES 3. No Mac a imagem Docker `vitasdk/vitasdk` traz o SDK; o job
`core-vita` do `libretro.yml` roda o mesmo script com `--local` dentro dessa imagem. A
release faz o mesmo e publica `zeebx_libretro-vita.zip`. O `.a` não abre sozinho: o
RetroArch o consome renomeado para `libretro_vita.a`.

O aplicativo, que não passa pelo RetroArch, mora em
[`frontends/vita-standalone`](frontends/vita-standalone/LEIAME.md). É um VPK: o mesmo alvo e a
mesma imagem Docker, outro binário. O `vita.yml` é `workflow_dispatch`, como o APK; a release
publica `zeebx-vita.zip`.

**A memória na Vita é apertada e não foi medida no aparelho.** O heap do jogo lá é de 48 MiB
(64 no resto), e a região interna soma mais 16 MiB que a newlib reserva de verdade. Uma região
nova ou maior em `loader/mod.rs` sai dessa mesma conta. O heap de outro tamanho também faz o
save state da Vita não abrir nas outras plataformas, e vice-versa. Ver
[`frontends/vita-standalone/LEIAME.md`](frontends/vita-standalone/LEIAME.md).

**iOS** precisa de um Mac com Xcode e dos alvos `aarch64-apple-ios` e
`aarch64-apple-ios-sim` no rustup. O passo a passo está em
[`frontends/ios/LEIAME.md`](frontends/ios/LEIAME.md):

```bash
./frontends/ios/compilar.sh --app
```

## O CI

**A tag é o único gatilho automático.** O `release.yml` dispara em `v0.0.0` e monta a release como
rascunho. O `ci.yml`, o `libretro.yml`, o `headless.yml`, o `android.yml`, o `ios.yml`, o
`vita.yml` e o `qt.yml` são `workflow_dispatch`: o CI padrão compila a linha de comando — e com ela o núcleo e os
testes — nas seis plataformas e chama o `qt.yml`, que compila a interface e monta os instaladores. A release chama o mesmo `qt.yml`:
os passos do Qt moram num lugar só. Onze jobs por execução é caro demais
para gastar em cada push, e quem decide é quem pede. A exceção é o `discord-issues.yml`, que não
compila nada: avisa no Discord quando uma issue abre, fecha ou muda de responsável.

Se você mexeu em algo que só um deles cobre — o APK, o core num alvo ARM —, diga ao humano que
vale disparar aquele workflow antes da tag. Você não consegue dispará-lo.

## O que não pode quebrar

- **Os nomes dos binários não mudam.** A interface é o `zeebx-qt` — é o que o atalho, o `.desktop`
  e o atualizador de quem já a tem instalada apontam —, e a linha de comando é o `zeebx`, que os
  scripts, o CI e a documentação digitam. O pacote pode mudar de lugar; o que o usuário digita, não.
- **O `.so` do core e o `zeebx_libretro.info` andam em par.** Um `.info` velho ao lado de um core
  novo faz o scan do RetroArch marcar `??` em tudo. E os campos de capacidade do `.info` têm de
  casar com o que a ABI faz.
- **O código do Zeebx é GPL-2.0-or-later**. O backend de CPU padrão é o Dynarmic, para que
  frontends GPLv3 como Qt 6 possam linkar o núcleo sem carregar uma dependência GPLv2-only. Antes
  de propor uma biblioteca nova, cheque a licença dela e as features do binário que vai linká-la.
  **Os binários já são GPLv3 na prática**: todos ligam crates que são só Apache-2.0, que não
  combinam com a GPLv2. Por isso uma dependência GPL-2.0-only não entra em binário nenhum.
- **A `CHAVE_PUBLICA` de `src/ui/atualizacao.rs` e o segredo `ZEEBX_ATUALIZADOR_CHAVE` do GitHub
  andam em par.** O segredo assina os pacotes da release; a chave embutida é o que o emulador
  aceita. Trocar um sem o outro faz toda atualização automática ser recusada — e quem já tem a
  versão velha instalada só sai dela baixando à mão.
- **O `THIRD-PARTY-NOTICES.txt` acompanha o `Cargo.lock`.** Mudou dependência, rode
  `python3 ferramentas/avisos_de_terceiros.py` e commite junto; a release confere.
- **O heap do guest é só do jogo.** Memória que o emulador pede para si no espaço do guest sai
  do `aloca_interno` (região `0x3400_0000`), nunca do `malloc` nem do `heap.alloc`. Um bloco
  nosso no heap muda o endereço de tudo o que o jogo aloca depois, e o RetroAchievements lê o
  heap por endereço. As bases e tamanhos de `loader/mod.rs` são o mesmo contrato.
- **O `Cargo.lock` é versionado** e o CI usa `--locked`. Membro novo no workspace entra no lock,
  no mesmo commit.

## Como este projeto escreve

**Comentário explica o porquê, não o quê.** O código já diz o que faz. O que se perde é a razão:
a medição que derrubou a hipótese, o defeito que a linha previne, o caminho que foi tentado e não
servia. Há comentários longos aqui, e eles são assim de propósito.

**Afirmação vem com medida.** "34× mais rápido", "de 79% para 284%", "1214 quadros por
milissegundo virtual". Se você não mediu, diga que não mediu — não arredonde para uma impressão.

**Mensagem de commit é uma frase declarativa em português**, dizendo o que mudou e por quê. Sem
prefixo de conventional commit no histórico recente. O corpo explica a razão, não repete o diff.

**Português no código e na documentação; inglês onde o usuário estrangeiro lê** — as opções e
mensagens do headless, as chaves do `config.ini`, os arquivos de `assets/lang/`.

## A lista de contribuidores do README

A tabela "Quem faz o Zeebx" do README é escrita, e não buscada: o README não roda nada. Ela sai
de `python3 ferramentas/contribuidores.py`, que lê a mesma API do site (`docs/site.js`) e reescreve
o trecho entre os marcadores `<!-- contribuidores -->`. Refaça quando entrar gente nova.

## Notas de versão

Cada versão deixa a sua em [`docs/patch-notes/`](docs/patch-notes/), escrita para quem usa o
emulador: o que foi corrigido, em linguagem de quem joga, e não de quem programa.

## Se você não tem certeza

Este repositório prefere uma pergunta a um palpite bem escrito. Se a leitura não fecha — se não dá
para saber se um controle manda o direcional como tecla ou como eixo, por exemplo —, diga qual
instrumento resolveria em vez de escolher o caminho mais provável.
