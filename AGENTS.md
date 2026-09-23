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
  classical-standalone/   o binário `zeebx`: janela do egui e linha de comando
  headless/               sem interface, configurado por `config.ini`
  libretro/               o core do RetroArch
  android/                o aplicativo, sem uma linha de Java
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
| `desktop` | eframe, gilrs, minifb, rfd, Discord, glutin — implica `gpu`, `audio`, `soundfont`, `unicorn` | standalone, headless |
| `audio` | só o `cpal`, que no Android fala com o Oboe | Android, e o `desktop` |
| `gl` | o desenho na placa; **não puxa nada de host** | core, Android, e o `desktop` |
| `gpu` | criar contexto próprio (glutin) — implica `gl` | desktop |
| `soundfont` | MIDI por banco de amostras (rustysynth, Rust puro) | core e desktop |
| `unicorn` | o backend QEMU da CPU | desktop |

**A regra da casa: o core Libretro não linka biblioteca de host.** Sem janela, sem placa de som,
sem controle. Quem entrega vídeo, áudio e entrada é o frontend. O `libretro.yml` cobra isso com
`ldd`, e um `dep:` novo no lugar errado quebra o build do core.

Antes de pôr um `#[cfg(feature = "desktop")]` em algo, pergunte se aquilo é **mesmo** de desktop.
Três gates errados já quebraram o build do Android sem ninguém perceber, porque o APK só é montado
na tag.

## Compilar e provar

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

## O CI

**A tag é o único gatilho automático.** O `release.yml` dispara em `v0.0.0` e monta a release como
rascunho. O `ci.yml`, o `libretro.yml`, o `headless.yml` e o `android.yml` são `workflow_dispatch`:
seis runners por execução é caro demais para gastar em cada push, e quem decide é quem pede.

Se você mexeu em algo que só um deles cobre — o APK, o core num alvo ARM —, diga ao humano que
vale disparar aquele workflow antes da tag. Você não consegue dispará-lo.

## O que não pode quebrar

- **O nome do binário é `zeebx`.** O pacote mudou de lugar; o que o usuário digita, não.
- **O `.so` do core e o `zeebx_libretro.info` andam em par.** Um `.info` velho ao lado de um core
  novo faz o scan do RetroArch marcar `??` em tudo. E os campos de capacidade do `.info` têm de
  casar com o que a ABI faz.
- **A licença é GPL-2.0-only**, porque o `unicorn-engine` é GPLv2 e vai compilado dentro do
  binário. Isso **exclui** qualquer dependência LGPLv3 ou GPLv3 — Qt 6, por exemplo. Antes de
  propor uma biblioteca nova, cheque a licença dela.
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
