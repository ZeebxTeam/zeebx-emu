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
  classical-standalone/   o binário `zeebx`: janela Qt (a do egui em `zeebx egui`) e linha de comando
  headless/               sem interface, configurado por `config.ini`
  libretro/               o core do RetroArch
  android/                o aplicativo, sem uma linha de Java
  ios/                    o aplicativo de iOS: UIKit por cima de uma biblioteca estática
ferramentas/        scripts Python: catálogo, varredura, instalação do core
docs/               documentação; `patch-notes/` guarda as notas de cada versão
assets/             ícones, fontes e `lang/` — os idiomas de fábrica
```

**`src/ui/` não é só interface.** O `library`, o `settings`, o `saves`, o `acervo` e o `i18n`
moram ali mas são lidos **pelo núcleo** — o `machine`, o `loader` e a `session` falam com eles.
Só `ui::app`, `ui::window`, `ui::atualizacao` e `ui::discord` são janela de verdade.

## As features, e por que elas existem

| Feature | O que traz | Quem usa |
|---|---|---|
| `desktop` | eframe, gilrs, minifb, rfd, Discord, glutin — implica `gpu`, `audio`, `soundfont` | standalone, headless |
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

O standalone pede o **Qt 6** (6.4 ou mais novo): a feature `ui-qt` vem ligada, e o build acha o Qt
pelo `qmake6`, pelo `qmake` ou pelo `QMAKE`. `python3 ferramentas/prepara_build.py` diz o que falta.
A interface Qt e a migração estão em
[`docs/implementacao/21-migracao-para-qt.md`](docs/implementacao/21-migracao-para-qt.md).

```bash
cargo build --release --locked -p zeebx-classical-standalone   # o binário `zeebx`
cargo test  --release --locked -p zeebx -p zeebx-classical-standalone
timeout 60 ./target/release/zeebx controles                    # sobe de verdade, sem tela

cargo build --release --locked -p zeebx-headless -p zeebx-libretro
python3 ferramentas/verifica_core.py target/release/libzeebx_libretro.so
```

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

**iOS** precisa de um Mac com Xcode e dos alvos `aarch64-apple-ios` e
`aarch64-apple-ios-sim` no rustup. O passo a passo está em
[`frontends/ios/LEIAME.md`](frontends/ios/LEIAME.md):

```bash
./frontends/ios/compilar.sh --app
```

## O CI

**A tag é o único gatilho automático.** O `release.yml` dispara em `v0.0.0` e monta a release como
rascunho. O `ci.yml`, o `libretro.yml`, o `headless.yml`, o `android.yml`, o `ios.yml` e o
`qt.yml` são `workflow_dispatch`: o CI padrão já compila o standalone clássico e, quando
aplicável, o Qt nas seis plataformas, enquanto `ios.yml` e `qt.yml` ficam disponíveis para
builds específicos e para montar os instaladores antes da tag. Esses workflows não precisam
rodar em cada push: seis runners por execução, ou até doze jobs quando o Qt entra em cena, é
caro demais para gastar automaticamente, e quem decide é quem pede. A exceção é o
`discord-issues.yml`, que não compila nada: avisa no Discord quando uma issue abre, fecha ou
muda de responsável.

Se você mexeu em algo que só um deles cobre — o APK, o core num alvo ARM —, diga ao humano que
vale disparar aquele workflow antes da tag. Você não consegue dispará-lo.

## O que não pode quebrar

- **O nome do binário é `zeebx`.** O pacote mudou de lugar; o que o usuário digita, não.
- **O `.so` do core e o `zeebx_libretro.info` andam em par.** Um `.info` velho ao lado de um core
  novo faz o scan do RetroArch marcar `??` em tudo. E os campos de capacidade do `.info` têm de
  casar com o que a ABI faz.
- **O código do Zeebx é GPL-2.0-or-later**. O backend de CPU padrão é o Dynarmic, para que
  frontends GPLv3 como Qt 6 possam linkar o núcleo sem carregar uma dependência GPLv2-only. Antes
  de propor uma biblioteca nova, cheque a licença dela e as features do binário que vai linká-la.
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

## Notas de versão

Cada versão deixa a sua em [`docs/patch-notes/`](docs/patch-notes/), escrita para quem usa o
emulador: o que foi corrigido, em linguagem de quem joga, e não de quem programa.

## Se você não tem certeza

Este repositório prefere uma pergunta a um palpite bem escrito. Se a leitura não fecha — se não dá
para saber se um controle manda o direcional como tecla ou como eixo, por exemplo —, diga qual
instrumento resolveria em vez de escolher o caminho mais provável.
