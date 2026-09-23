# Proposta de inclusão no banco de DATs do libretro

Cinco títulos de Zeebo do acervo **não estão** no `Mobile - Zeebo.dat` que o RetroArch usa para
nomear as ROMs e para achar as capas. Sem entrada no DAT, esses títulos aparecem com o nome do
arquivo e ficam sem capa, mesmo com o dump em mãos.

Medido: o DAT em uso tem **57 títulos** e **nenhum** destes cinco.

| Título | Por que está fora |
|---|---|
| Bad Dudes vs. DragonNinja | jogo oficial, lançado no console e ausente do banco |
| Caveman Ninja | idem |
| Dark Seal | idem |
| Karnov's Revenge | idem |
| Kingdom Hearts V CAST | homebrew, capítulo 1 + Agrabah |

## O que este diretório tem

- `Mobile - Zeebo (proposta).dat` — os blocos prontos para acrescentar ao DAT do banco, no **formato
  dele**: cabeçalho `clrmamepro` com os mesmos quatro campos do arquivo oficial (`name`,
  `description`, `version` entre aspas no formato `AAAA.MM.DD`, `homepage`), e um `game` por título,
  com `region "Brazil"` e os hashes de cada arquivo.
- `proposta.txt` — o mesmo, para leitura humana: pacote, tamanho, CRC32 e SHA1 do `.zip`, e todos os
  arquivos do módulo com tamanho, CRC32, MD5 e SHA1.

Os nomes dos arquivos seguem a convenção do banco: o No-Intro grava `mod274259font.bar` para o que
está em `mod/274259/font.bar`. Cada `game` traz **vários** `rom` de propósito: qual deles é o dump é
decisão de quem mantém o banco, e a proposta leva todos os hashes em vez de apostar num.

## Para onde enviar (conferido na API do GitHub, não suposto)

| Destino | Estado | O que fazer |
|---|---|---|
| `datomatic.no-intro.org` (`?page=download&op=daily`) | é a **fonte** do banco | é aqui que o título entra primeiro, com o dump anexado |
| `libretro/libretro-database` | 53 DATs em `dat/`, sem `Mobile - Zeebo.dat` | PR acrescentando o arquivo |
| `robloach/libretro-dats` | é o **construtor** (Node.js), `database/` é submódulo do de cima | não recebe DAT à mão |
| `libretro-thumbnails/Mobile_-_Zeebo` | **não existe** (404) | as 58 capas vão junto do pedido de criação do repositório |

O caminho de nome do repositório de capas é o nome do banco com `_` nos espaços — conferido num
repositório que existe: `libretro-thumbnails/Nintendo_-_Nintendo_Entertainment_System`.

## As capas

São 58, em PNG, no formato que o repositório de thumbnails aceita (ele **recusa** JPEG), no layout
`Named_Boxarts/` com o nome exato de cada título no DAT. Medido: `Named_Boxarts` com 58 PNG e
`Named_Titles` com 44, na pasta que o RetroArch já usa.

As capas **não** ficam versionadas aqui: são regeneráveis e pesam alguns megabytes. Para refazer,
`ferramentas/catalogo.py --roms <zips> --saida <destino> --png`.

## O que falta

Só o envio, que precisa das contas de quem mantém o fork. Nada mais.
