# 17 — Testes

Como conferir que o emulador faz o que dizemos que faz, e quais comandos usar para isso.

A meta do projeto é reproduzir o comportamento do BREW 4.0.2 como o Zeebo o apresentava. Isso é
uma afirmação sobre **comportamento observável**, não sobre código bonito — então a única forma de
saber que continuamos certos é medir. E medir uma vez não serve: o que interessa é saber **no
commit que causou** que alguma coisa deixou de funcionar.

Daí três camadas, com custos bem diferentes:

| Camada | Custo | O que responde |
|---|---|---|
| Testes junto do código | segundos | um algoritmo, um parser, um método de API está certo? |
| Varredura de abertura | ~15 s para a biblioteca inteira | a ROM abre, ou estoura antes de inicializar? |
| Varredura de execução | segundos a minutos por jogo | o jogo anda, quanto custa, e o que ainda falta para ele? |

## O básico

```bash
# tudo
cargo test

# um módulo, ou um nome
cargo test rasterizer
cargo test recorte

# os que ficam de fora por padrão: medem tempo, ou dependem da máquina de quem roda
cargo test --release -- --ignored --nocapture
```

São 308 funções de teste, todas junto do código que testam, e 5 marcadas com `#[ignore]`.
Elas ficam de fora porque **tempo não é resultado reproduzível**: quatro medem a vazão do núcleo
ARM (`cpu::unicorn::speed`) e uma olha o cache de saves do sistema de arquivos de verdade.

O `--nocapture` mostra o que os testes imprimem. Sem ele, `cargo test` engole a saída de quem
passou — e nas varreduras é justamente a saída que interessa.

## Por que não existe `tests/`

Rust tem duas camadas de teste: as de integração, em `tests/` na raiz, cada arquivo compilado como
um binário separado que só vê a **API pública** do crate; e as unitárias, junto do código em
`#[cfg(test)] mod tests`, que podem tocar em campo privado.

Aqui só existe a segunda, e não por descuido: **o crate é só binário**, não tem `src/lib.rs`. Um
arquivo em `tests/` não conseguiria nem `use zeebx::…`, porque não há biblioteca para importar.

Isso combina com o que estes testes precisam fazer. Um teste que confere o recorte de um blit
precisa montar uma `Machine`, chamar um método por slot e ler a superfície de dentro — nada disso
é, nem deveria ser, interface pública do emulador. Ter de tornar público o que os testes usam
deixaria a fronteira do código pior para melhorar a de teste nenhum.

O preço é que a varredura de ROMs também vive em `src/`, sob `#[cfg(test)]` — ou seja, ela não
existe no binário de release. É o mesmo efeito dos testes que só rodam em desenvolvimento em
outras linguagens, obtido por compilação condicional em vez de por diretório.

## A varredura de ROMs

`src/varredura.rs`. É o levantamento de compatibilidade de
[11-compatibilidade.md](11-compatibilidade.md) transformado em teste. Ela é **dirigida por
ambiente**: nenhuma ROM está na árvore, então sem `ZEEBX_ROM` os dois testes avisam e passam. É
de propósito — `cargo test` não pode virar uma varredura de sessenta e três jogos porque alguém
clonou o repositório.

| Variável | O que faz |
|---|---|
| `ZEEBX_ROM` | um `.mod`/`.zip`, uma lista separada por vírgula, ou um diretório |
| `ZEEBX_ROM_MS` | quanto tempo **virtual** rodar, em ms (padrão 6000) |
| `ZEEBX_ROM_TETO` | teto de tempo **real** por jogo, em segundos (padrão 90) |
| `ZEEBX_ROM_SAIDA` | diretório onde gravar o relatório completo de cada jogo |
| `ZEEBX_ROM_BASE` | diretório da linha de base: o que falta é gravado, o que existe é cobrado |
| `ZEEBX_ROM_TECLAS` | roteiro de controle no **relógio virtual**: `ms:botão` separado por vírgula, e `ms:eixo=valor` para o manche |
| `ZEEBX_ROM_PERFIL` | liga o perfil de custo por método de API (encarece a execução: não use para medir velocidade) |

### Roteiro de controle

Havia perguntas que só se respondiam com alguém apertando o controle — se a entrada chega ao
guest, se um menu aceita a escolha. Com o roteiro, a varredura responde sozinha:

```bash
# empurra o manche para a esquerda aos 4 s, devolve ao repouso aos 6 s, confirma aos 10 s
ZEEBX_ROM="roms/Z-Wheel.zip" ZEEBX_ROM_MS=12000 \
  ZEEBX_ROM_TECLAS="4000:x=-128,6000:x=0,7000:y=-128,9000:y=0,10000:b1,11000:" \
  cargo test --release varredura -- --nocapture
```

Os nomes de botão são os da tabela do aparelho (`b1`…`b4`, `start`, `up`, `left`, `down`,
`right`), e os eixos são `x`, `y`, `z` e `rz` no curso `-128..=128`. **O passo é no relógio
virtual**, então o mesmo roteiro vale igual com a máquina a 30% ou a 300% da velocidade do
console. Um passo com dois-pontos e nada depois (`11000:`) solta tudo.

Vale saber de um detalhe que já custou uma investigação: **a Z-Wheel é navegada pelo manche**,
não pelos botões (ela registra `RegisterForPositionChange`). Um roteiro só de botões não move a
roda um pixel — e o sintoma é uma imagem parada, que parece defeito de entrada.

### O que a varredura **não** consegue verificar, medido

Tentativa de exercitar o ciclo da Z-Wheel por aqui, com o roteiro acima: **não funciona**, e vale
saber por quê antes de tentar de novo. Doze segundos virtuais, com e sem manche:

| | com roteiro | sem roteiro |
|---|---:|---:|
| voltas | 1547 | 1543 |
| instruções | 133.567.092 | 133.566.074 |
| chamadas de API | 264.953 | 264.931 |

A diferença é de mil instruções em 133 milhões: a roda **não reage** e não desenha nada neste
caminho (zero quadros apresentados, zero texto desenhado nos dois casos). A Z-Wheel monta a
interface pelo caminho de placa, que o laço da varredura não exercita; conclusão prática: **o ciclo
dela só se verifica no RetroArch, com controle na mão** — e é o que o plano já dizia no item 8.

### Um jogo, com o relatório inteiro

```bash
ZEEBX_ROM="roms/Quake.zip" cargo test --release varredura -- --nocapture
```

Sai o estado, o desempenho, as APIs que faltaram, as classes que o jogo pediu e não temos, os
arquivos que ele não achou, os ponteiros recusados, as APIs atendidas por hipótese, os
registradores e a pilha de quem quebrou, e o fim do log do próprio jogo.

### Só a abertura, na biblioteca inteira

```bash
ZEEBX_ROM=roms cargo test --release varredura::tests::a_rom_indicada_abre -- --nocapture
```

A pergunta mais barata que se pode fazer de um jogo: carrega o `.mod`, roda o `AEEMod_Load`, lê o
`.mif` e chama o `CreateInstance` — e para aí. Sessenta e três ROMs em cerca de quinze segundos.
Quem não cria applet aparece na hora, sem gastar os seis segundos virtuais que nunca iria rodar.

O teste não para no primeiro erro: ele anota cada um e segue, e só falha no fim com a lista
inteira. Numa varredura o que se quer ver é o placar, não a primeira desistência.

### O placar, para conferir um ajuste

```bash
ZEEBX_ROM=roms cargo test --release placar -- --nocapture
```

Cada ROM roda poucos segundos virtuais e vira uma linha de tabela: estado, tempo virtual
cumprido, desenho e o motivo de quem parou. É o teste de ida e volta rápida depois de mexer no
emulador — a pergunta é "quem continua abrindo", não o relatório de cada jogo.

```
| Jogo | Estado | Tempo virtual | Desenho | Velocidade | Motivo |
|---|---|---|---|---|---|
| Disney All Star Cards | roda | 2500 ms | 382200 pixel(s) | 905% |  |
| Bejeweled Twist | quebrou no laço de quadros | 11 ms | tela preta | 24% | acesso inválido a 0x00000024 |
| Zeeboids | roda | 3148 ms | 41 quadro(s) | 80% |  |
```

A coluna de desenho conta o que houver: quadro apresentado quando o jogo apresenta, escritas na
tela quando ele desenha em 2D sem apresentar, e "tela preta" quando não houve nem uma coisa nem
outra — que é o sintoma que não aparece no estado.

| Variável | O que faz |
|---|---|
| `ZEEBX_PLACAR_MS` | tempo **virtual** por ROM, em ms (padrão 5000) |
| `ZEEBX_PLACAR_TETO` | teto de tempo **real** por ROM, em segundos (padrão 30) |
| `ZEEBX_PLACAR_SAIDA` | arquivo onde gravar a tabela |

**Este teste não falha por jogo quebrado.** A biblioteca tem jogos sabidamente incompatíveis, e
falhar neles apagaria a tabela, que é o que interessa. Quem cobra regressão é a linha de base,
abaixo.

### A execução, com linha de base

```bash
ZEEBX_ROM=roms ZEEBX_ROM_SAIDA=saida ZEEBX_ROM_BASE=docs/varredura \
  cargo test --release varredura -- --nocapture
```

O `ZEEBX_ROM_BASE` é o que torna isso um teste e não um relatório. Na primeira vez, o resumo de
cada jogo é **gravado**; nas seguintes, é **cobrado**, e o que mudou sai como `+`/`-` na falha:

```
FALHA Quake     roda     6016 ms virtuais em 4.1 s, 41 quadro(s), 146%, 13 pendência(s)
  mudou desde docs/varredura/quake.txt:
  - arquivos não encontrados:
  +   0x0103d8de
```

Gravar o que falta, em vez de exigir que alguém escreva à mão, é o que permite adotar a linha de
base de um jogo novo numa execução — e a diferença fica visível no `git diff`, que é onde ela deve
ser revisada, jogo por jogo.

### O que a linha de base guarda, e o que não guarda

Guarda **estado e pendências**. Não guarda **nada de desempenho** — nem tempo, nem instrução, nem
quadro, nem o caminho do arquivo, nem os registradores da falha.

O motivo é prático: se guardasse, trocar de máquina — ou rodar em depuração em vez de `--release`
— acusaria regressão em todos os jogos de uma vez, e um teste que acusa sempre não é lido nunca.
Há um teste só para isso, o `o_resumo_nao_muda_com_o_desempenho`.

Por isso o `--release` também não é enfeite: em depuração o núcleo emulado roda uma ordem de
grandeza mais devagar, e o teto de tempo real classificaria jogo bom como "lento demais".

### Os estados, e o que cada um significa

| Estado | Onde parou | Falha o teste? |
|---|---|---|
| roda | cumpriu o tempo virtual pedido, de pé | não |
| terminou sozinho | sem timer armado e sem trabalho pendente | não |
| não carrega | não é um `.mod` que saibamos ler | sim |
| quebrou antes de criar o applet | no `AEEMod_Load` ou dentro do `CreateInstance` | sim |
| não cria o applet | sem `.mif`, ou `CreateInstance` recusando | sim |
| quebrou no `EVT_APP_START` | o applet nasceu e morreu no evento inicial | sim |
| quebrou no laço de quadros | já dentro do laço | sim |
| lento demais | não cumpriu o tempo virtual dentro do teto real | sim |

**Pendência não é falha.** Um jogo que roda pedindo as seis classes do grupo de extensões continua
passando, porque é assim que ele se comporta no console também: recebe `ECLASSNOTSUPPORT` e segue
pelo caminho alternativo. Quem cobra pendência é a linha de base, e só quando ela existe.

A separação entre "quebrou no `EVT_APP_START`" e "quebrou no laço de quadros" custou uma mudança no
`session.rs` — a partida virou um passo próprio, o `Session::parte`. Sem isso as duas caíam na
mesma volta e um jogo que morre no laço aparecia como morto na partida, apontando para o trecho
errado do código.

## As outras medições

```bash
# vazão do núcleo ARM e custo de entrar no guest
cargo test --release cpu::unicorn::speed -- --ignored --nocapture

# um jogo sem janela, com o relatório completo e o resumo de chamadas
cargo run --release -- run "roms/Quake.zip" --seconds=6

# só as chamadas de uma interface
cargo run --release -- run "roms/Quake.zip" --seconds=6 --trace=Bitmap

# um quadro por arquivo, para comparar dois vizinhos
cargo run --release -- run "roms/Quake.zip" --seconds=6 --dump-gl=quadros
```

O relatório do `run` e o da varredura dizem a mesma coisa, e por construção: os dois saem dos
mesmos contadores da `Machine`. O do `run` é para investigar um jogo; o da varredura é para cobrar
o que já se sabia dele.

## Como escrever um teste aqui

Duas regras, e as duas vêm de defeito que passou.

**O comentário do teste carrega o defeito que o gerou.** Não "testa o formatador", e sim: o `%02d`
que fazia o Resident Evil 4 procurar `3d_stg02_0.h2z` quando o arquivo é `3d_stg02_00.h2z`. Um
teste sem essa frase é um teste que alguém vai apagar quando ele der trabalho, sem saber o que
estava sendo protegido.

**O teste mede comportamento, não implementação.** Chamar o método pelo slot, como o jogo chama, e
conferir o que saiu na superfície ou no registrador de retorno — não conferir que uma função
interna foi chamada. É o que permite reescrever o miolo sem reescrever o teste.

## A varredura precisa de cache limpo, e o motivo não é óbvio

A varredura das 62 ROMs compara o resumo de cada jogo com a linha de base commitada. Ela falhou em
seis lotes de oito depois de uma mudança de áudio, e o susto era falso: **o jogo grava os próprios
arquivos dentro do pacote extraído** — `udata/dooopt.sav`, `udata/options`, `default.opt` — e o
cache de extração sobrevive entre execuções.

O efeito é exatamente o inverso do que parece: com o arquivo já gravado por uma execução anterior,
o `open` que antes falhava passa a funcionar, o relatório **perde** uma pendência, e a linha de base
acusa diferença. Cinco jogos "mudaram" por causa de save deixado para trás:

```text
Iron Sight        - udata/dooopt.sav
Karnov's Revenge  - open falhou: .../udata/options
                  - open falhou: .../default.opt
```

Limpar o cache **antes** de cada lote devolve o mesmo resultado de sempre: nenhuma diferença.

Duas coisas ficam desta medição:

1. O script de varredura limpa o cache nas duas pontas do lote, não só no fim.
2. Nem toda falha do teste é regressão. O teste falha por dois motivos — categoria que não passa
   (jogo sabidamente fora da lista) e diferença de linha de base —, e só a segunda interessa. O
   resumo do script separa as duas, senão seis jogos conhecidos escondem a resposta que se quer.

O script é `ferramentas/varredura_por_lotes.py`, e ele carrega as duas lições:

```bash
python3 ferramentas/varredura_por_lotes.py --roms /caminho/das/roms --ms 6000
```

O `--ms` tem de ser o mesmo com que a linha de base foi gravada (6000). Medido: com 3000 o mesmo
jogo aparece como diferença de linha de base — o resumo tem a tela e as pendências do instante em
que a medição parou, e parar noutro instante muda os dois.
