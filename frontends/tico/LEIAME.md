# O Zeebx no tico

O tico é um frontend de Nintendo Switch. Um módulo é a forma de um emulador aparecer
na lista dele: um `.nro`, a descrição do console e a tela de ajustes. Este não passa
pelo RetroArch. O core que o ROMBundler liga continua em
[`frontends/switch`](../switch/compilar.sh), um `.a` estático. Aqui o binário é o
`tico-zeebx.nro`.

## Instalar

O zip tem uma pasta, `zeebx`. Ela vai para o cartão, em `sdmc:/tico/modules/`. O
resultado fica assim:

```
sdmc:/tico/modules/zeebx/module.json
sdmc:/tico/modules/zeebx/tico-zeebx.nro
```

Feche o tico antes de copiar, e abra de novo. Os jogos ficam em `sdmc:/tico/roms/zeebo`.
Um `.mod`, um `.zip` ou um `.7z` servem. Não há lista No-Intro do Zeebo: o tico mostra
o nome do arquivo. A capa, se você tiver, é o nome do arquivo em
`sdmc:/tico/assets/covers/zeebo/`.

O banco de amostras do MIDI, se você o tiver, vai em
`sdmc:/tico/system/zeebx/soundfonts/`, com o nome que o arquivo tiver. Sem ele, a
música usa a tabela de timbres.

Segure − e + por meio segundo para voltar ao tico. O motivo de uma abertura que
falhou fica em `sdmc:/tico/system/zeebx/zeebx.log`, e a tela fica vermelha até um
botão.

Os ajustes da tela do tico valem na abertura seguinte.

## Os botões

| Switch | Zeebo |
|---|---|
| B | Botão 1, embaixo |
| Y | Botão 2, à esquerda |
| X | Botão 3, em cima |
| A | Botão 4, à direita |
| ZL | ZL |
| ZR | ZR |
| L | L2 |
| R | R2 |
| + | Start |
| − | HOME |
| Direcional | Direcional |
| Analógico esquerdo | Manche |
| Analógico direito | O outro manche |

O analógico parado não anda: há uma zona morta em volta do centro.

## O que este binário não faz

O desenho é no processador. O core do RetroArch usa a placa porque o RetroArch
entrega o contexto; este processo não abre um. A velocidade no aparelho não foi
medida. O processador do jogo é o recompilado, o mesmo do core, com o cache de
código de 32 MiB.

O tico não conta o tempo de jogo de um módulo de fora, nem entrega a conta do
RetroAchievements. Os saves ficam na pasta da conta que o tico passou na abertura.

Uma loja do tico, com o zip publicado, ainda não está neste repositório. Para
instalar, copie a pasta. O job `tico.yml` monta o zip quando alguém o dispara.

## Compilar

A cadeia é a imagem `devkitpro/devkita64:20260219`, a mesma do `tico.yml`. No
Mac ela não traz rustc; o script instala o stable no contêiner:

```bash
./frontends/tico/compilar.sh
```

O zip sai em `frontends/tico/saida/zeebx-tico-module.zip`. Dentro da imagem, ou
no job `tico.yml`:

```bash
./frontends/tico/compilar.sh --local
```

`cargo test -p zeebx-tico` prova os argumentos, os ajustes, o controle e o quadro
no desktop. Não sobe o aparelho.
