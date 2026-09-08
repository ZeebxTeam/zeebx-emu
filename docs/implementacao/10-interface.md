# 10 — A interface

`egui`/`eframe`. Três janelas de sistema separadas, não abas: a biblioteca é a janela principal,
e as configurações e o jogo abrem em **viewports imediatas** próprias.

## Biblioteca

`library.rs` varre a pasta de ROMs procurando `.mod` e `.zip` até quatro níveis, com teto de
2000 jogos para uma pasta escolhida sem querer não travar a interface. A varredura ordena, porque
`read_dir` não promete ordem e uma lista que muda de posição entre duas aberturas é confusa.

Duas disposições convivem: a do console instala como `<Título>/mod/<id>/<nome>.mod` com o `.mif`
num `mif/` irmão; os exemplos do SDK deixam os dois lado a lado.

O título vem da **pasta**, não do arquivo: no layout do console o nome do arquivo é um
identificador numérico sem graça.

### A imagem de cada cartão

Nesta ordem: uma imagem de mesmo nome ao lado do jogo (`Quake.png` junto de `Quake.zip`), o maior
ícone do `.mif`, e por último o logo embutido.

A ordem existe porque **não há capa dentro das ROMs** — o maior ícone dos títulos antigos é 65×42,
que é ícone de menu. A imagem ao lado do jogo é a saída para quem quiser arte de verdade.

O cartão **amplia sem interpolar** quando a imagem é menor que o quadro: um ícone de 26 pixels
aparece ampliado quatro vezes, e interpolar viraria um borrão — o bloco quadrado é o que o console
mostrava. Uma imagem grande já entra reduzida, e aí a interpolação é que evita o serrilhado.

## Configurações

Quatro abas: geral, controles, gráficos e áudio. Tudo é gravado em JSON no diretório de
configuração do sistema, escrito à mão (`settings::config_dir`) porque são três regras conhecidas
e cada uma cabe numa linha.

Cada campo é `#[serde(default)]`: **um arquivo faltando, truncado ou de uma versão mais nova
precisa deixar o programa abrir, não impedi-lo.** No pior caso, volta o padrão.

## Idiomas

`i18n.rs`. Dois idiomas embutidos no binário (en, pt-BR) e a possibilidade de acrescentar outros
por JSON solto — ao lado do executável, para uma cópia portátil, ou no diretório de configuração,
para quem quer traduzir sem mexer na instalação. Um arquivo com o código de um embutido o
substitui, que é como corrigir uma tradução sem esperar versão nova.

Um teste compara as chaves dos dois idiomas: **chave que existe só num deles é texto que aparece
em inglês no meio do português.**

## O mapa visual do controle

`padview.rs`. São **duas** peças:

| | |
|---|---|
| `assets/controller.png` | a arte — só precisa ser bonita |
| `assets/controller-map.svg` | o mapa: um SVG invisível do mesmo tamanho, em que o `id` de cada forma é o nome de um botão |

Separar as duas permite redesenhar o controle sem tocar em código, e poupa a arte de ter que
carregar um `id` em cada traço. Botão que a arte não desenha simplesmente não tem forma no mapa:
não acende, e continua configurável pela lista.

O mapa é rasterizado uma vez, cada forma virando uma **silhueta recortada**. Dela saem as duas
coisas que interessam: acender o botão certo e saber em qual deles o cursor está. Testar o clique
pela silhueta, e não por uma caixa retangular, é o que faz um direcional em cruz e um botão
redondo responderem só onde existe botão.

O recorte não é economia de disco: cada silhueta vira uma textura na placa de vídeo, e guardá-las
do tamanho do controle inteiro custaria dezenas de megabytes para desenhar botões de poucos
pixels.

O fundo escuro da arte é recortado **pela borda** (inundação a partir das margens), não pela cor:
o traço preto de dentro do desenho é tão escuro quanto o fundo, e apagá-lo pela cor deixaria o
controle sem contorno nenhum.

A tela mostra ainda os **quatro eixos ao vivo**. Um manche que não chega ao emulador aparece ali
como um zero teimoso — é o que separa "não mapeado" de "mapeado no eixo errado".

## Ícone da janela

A logo (`assets/zeebx.png`) é reduzida a 256 pixels de lado no arranque — ela tem mais de mil, e
um ícone desse tamanho é megabytes de textura para desenhar algo que nunca passa de alguns pixels
na barra.

A redução faz a média com a cor **premultiplicada** pelo alfa. Sem isso, um pixel transparente
entra na conta com a cor que ele guarda — que costuma ser preta — e a borda de um logo com fundo
transparente fica com um halo escuro.

Uma logo que não abra não impede a janela de abrir: o ícone simplesmente não é definido.

**No macOS o ícone da janela vem do pacote `.app`, não desta chamada** — o `with_icon` vale em
Windows e Linux. Um `.app` de verdade ainda não é montado por nós.

## Linha de comando

```bash
zeebx                                   # abre a interface
zeebx info jogo.mod                     # o que o módulo declara
zeebx run jogo.mod --seconds=6 --trace  # roda sem janela e resume as chamadas
```

| Opção | Para quê |
|---|---|
| `--seconds=N` / `--frames=N` | até onde ir |
| `--trace[=filtro]` | as últimas chamadas, e o resumo por método no fim |
| `--dump-gl=DIR` | um arquivo por quadro do OpenGL |
| `--dump-audio=A.wav` | a mistura de áudio |
| `--keys=ROTEIRO` | entrada sem janela |
| `--window` | a janela antiga, de `minifb` |
| `--watch=ENDEREÇO` | quem escreveu e quem leu uma faixa de memória |

O `--watch` é a ferramenta para "quem deveria ter preenchido este campo?": quando o jogo quebra
num ponteiro nulo, ele diz se alguém chegou a escrever ali — e de qual instrução partiu a escrita.
