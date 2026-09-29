# 10 — A interface

`egui`/`eframe`. Três janelas de sistema separadas, não abas: a biblioteca é a janela principal,
e as configurações e o jogo abrem em **viewports imediatas** próprias.

## Biblioteca

`ui/library.rs` varre a pasta de ROMs procurando `.mod` e `.zip` até quatro níveis, com teto de
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

### A Z-Wheel empresta as capas

As ROMs não trazem capa, mas o pacote da Z-Wheel traz as de 59 jogos, em
`mod/274755/assets/games/<game_id>/` (`boxartlg.jpg`, `boxart.bmp`, `rating.jpg` e a descrição em
três idiomas). `ui/acervo.rs` as lê direto do `.zip` ou da pasta, sem extrair:

- o banco `tt_game_info` (SQLite, copiado para o cache para abrir) liga `game_id` ao `class_id` do
  applet — o mesmo ClassID que a biblioteca usa como chave — e à pasta da capa; `TITLETEXT` dá o
  nome oficial por idioma (o `lang_id` é o código de letras do BREW em little-endian, `"pt  "`);
- o banco `asset_cache` liga as cenas do palco (`assets/stage_slides/<dslid>/`, tipo 5 na tabela
  `ASSETS`) a um `owner`, o `game_id`; a cena traz o logo do rolo de cima como textura
  `slidebanner.qxt` (QXEngine: cabeçalho de 40 bytes, formato `0x0c` = ATITC só de cor). São 12
  jogos com logo.

A capa ao lado do jogo continua valendo mais que a da Z-Wheel, e a da Z-Wheel mais que o ícone do
`.mif`. A descrição perde o aviso da loja que fechou em 2011, que abre quase todas entre `**`.

A Z-Wheel se configura na aba Geral (arquivo, pasta ou "Detectar", que confere o ClassID do `.mif`
em pastas com "wheel" ou "tectoy" no nome), sai da lista e abre pelo botão **▶ Z-Wheel** da barra
de cima; um jogo aberto pela biblioteca que sai sozinho fecha a janela dele.

### Grade e slider

`ui/vitrine.rs`. Em grade, cartões em pé na proporção das caixas; em slider, um jogo por vez no
jeito da Z-Wheel: o rolo de logos girando como cilindro, o nome, as caixas deslizando (a posição
corre atrás da escolha com uma mola amortecida, e a lista dá a volta) e a classificação com a
descrição embaixo.

Os dois modos andam pelo controle: direcional, manche e setas escolhem, com repetição ao segurar;
botão 1, Start, Enter ou espaço abrem; HOME abre a Z-Wheel. Teclado, direcional e manche viram um
estado só antes de detectar o aperto, para uma seta mapeada também no controle não andar dois. A
biblioteca não escuta com jogo ou configurações abertos, e como o controle não gera evento no egui
a janela se redesenha sozinha para lê-lo.

### Busca

O campo na barra de cima filtra a lista dos dois modos. Um jogo fica se o título tiver todas as
palavras digitadas, em qualquer ordem. Caixa, acento e pontuação não contam, e as siglas com
ponto se juntam: "cnk" acha "C.N.K.", "boia" acha "Bóia". Mudar a busca põe a escolha no primeiro
resultado, sem a mola do slider atravessar a lista. Com o campo em foco, setas e espaço são do
texto e o teclado não navega; o controle continua navegando. O Enter tira o foco do campo antes
de a biblioteca ler o quadro, e por isso abre o jogo escolhido. Ctrl+F leva ao campo e Esc limpa.

## Configurações

Quatro abas: geral, controles, gráficos e áudio. Tudo é gravado em JSON no diretório de
configuração do sistema, escrito à mão (`settings::config_dir`) porque são três regras conhecidas
e cada uma cabe numa linha.

Cada campo é `#[serde(default)]`: **um arquivo faltando, truncado ou de uma versão mais nova
precisa deixar o programa abrir, não impedi-lo.** No pior caso, volta o padrão.

Toda aba rola: a gráfica cresceu além da altura da janela. A janela principal e a do jogo abrem
em janela, maximizadas ou em tela cheia (padrão maximizada), e `F11` ou `Alt+Enter` alterna a tela
cheia a qualquer momento. A ampliação padrão é caber na janela: numa janela maximizada o pixel
inteiro deixava uma moldura larga à toa.

Na aba de controles, uma porta com Boomerang troca o desenho do controle pela prévia do Boomerang
girando com o Wii Remote, o botão de calibrar e a opção do aviso de calibração no jogo — ver o
[20](20-boomerang-e-wii-remote.md).

## Idiomas

`ui/i18n.rs`. Quatro idiomas embutidos no binário (en, es, es-MX, pt-BR) e a possibilidade de acrescentar
outros por JSON solto — ao lado do executável, para uma cópia portátil, ou no diretório de
configuração, para quem quer traduzir sem mexer na instalação. Um arquivo com o código de um
embutido o substitui, que é como corrigir uma tradução sem esperar versão nova.

Um teste compara as chaves dos idiomas embutidos: **chave que existe só num deles é texto que
aparece em inglês no meio do português.**

## O mapa visual do controle

`input/padview.rs`. São **duas** peças:

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

## Título da janela do jogo

A sessão só conhece a pasta de extração, que leva a impressão digital do pacote
(`Zeebo-Extreme-Boia-Cross-21503726-1788761080`). O título da janela sai da biblioteca pelo
ClassID — o nome da Z-Wheel quando ela descreve o jogo, o do pacote quando não —, e só na falta
dele a pasta, sem os dois números do fim (`library::sem_impressao_digital`).

## Aviso de abertura

Na partida, um modal diz que o emulador está em desenvolvimento e pede para configurar o
controle antes de jogar. "Não mostrar de novo" grava a versão em `aviso_dispensado_na_versao`; uma
versão nova mostra o aviso outra vez.

## Atualizações

`ui/atualizacao.rs`. Na abertura (opção nas configurações, ligada por padrão), uma thread pergunta
ao GitHub pela `releases/latest` de `ZeebxTeam/zeebx-emu`. A tag vale com ou sem `v`; rascunhos e
pré-lançamentos ficam de fora. Havendo versão maior que a do binário, um modal oferece abrir a
página da release — depois do aviso de abertura, para os dois não se empilharem. "Procurar agora"
repete a pergunta pelas configurações. Um 404 é "em dia": é o que vem enquanto não há release.

## Rich Presence do Discord

`ui/discord.rs`. O Discord fala por um soquete local que pode sumir a qualquer hora, então a
conexão mora numa thread: a interface diz o que mostrar a cada quadro, e só uma mudança vira
pedido. A thread conecta, reenvia e tenta de novo a cada 15 s enquanto o Discord estiver fechado.

| Onde | Linha de cima | Imagem grande | Ícone pequeno |
|---|---|---|---|
| biblioteca | "No menu" | `zeebx` | — |
| Z-Wheel | "Na Z-Wheel" | `zeebx` | — |
| jogo | "Jogando {nome}" | a capa | `zeebx` |

O nome que aparece no perfil é o do **aplicativo** Zeebx do Developer Portal, cujo ID está em
`discord::APLICATIVO`. As imagens também não passam pelo soquete: o Discord só
mostra uma chave cadastrada no aplicativo ou uma URL `https`. A exportação e o endereço das capas
existem no código mas estão **fora das configurações** por enquanto. A exportação grava
`zeebx.png` e `jogo_<clsid>.png` (512×512, a capa inteira centrada) com o nome igual à chave,
prontos para as Art Assets. Quem publicar as capas pode dar o endereço com `{chave}` ou `{clsid}`.

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
| `zeebx sessao <zip> --boomerang --movimento=ms:x:y:z` | a sessão da janela com um Boomerang roteirizado |
| `zeebx wiimote` | o Wii Remote ao vivo |
