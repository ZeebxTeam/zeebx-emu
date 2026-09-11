# 11 — Compatibilidade

Levantada rodando **as 62 ROMs** por seis segundos virtuais cada, sem janela. O procedimento está
no fim deste documento e dá para repetir a qualquer momento.

> "Roda" quer dizer **não quebrou em seis segundos**. Não quer dizer que a tela esteja certa nem
> que o jogo seja jogável. É o piso, não o teto.

## Placar

| Estado | Antes | Agora |
|---|---:|---:|
| roda | 33 | **50** |
| falha no `EVT_APP_START` | 10 | **0** |
| não cria o applet | 8 | 5 |
| para no laço de quadros | 7 | 6 |
| lento demais | 3 | 1 |

Doze jogos mudaram de estado de uma vez, e a causa foi uma só: **o sistema de arquivos do console
não distingue maiúsculas de minúsculas, e o nosso distinguia.** Os dez ports de arcade pedem
`font.fnz` e trazem `font.FNZ` no pacote; nenhum deles passava do `EVT_APP_START`, porque a fonte
não abria e o ponteiro nulo vinha logo depois. Raging Thunder 2 e Reckless Racing caíam no mesmo
buraco em outros arquivos. No Windows e no macOS isso funcionava por acaso.

Vale registrar como o defeito foi encontrado, porque o caminho não era óbvio: os dez pediam seis
ClassIDs que não temos, o que parecia ser a causa. **Não era** — eles seguem sem essas classes e
rodam assim mesmo. Quem entregou o problema foi o `DBGPRINTF` do próprio jogo, com um
`Failed to open font.fnz file!!!` na última linha do log, e a lista de "arquivos não encontrados"
do relatório logo abaixo confirmando.

## Por jogo

### Roda limpo (8)

Passou os seis segundos e o relatório não tem nada a apontar: nenhuma classe desconhecida, nenhuma
API atendida por hipótese, nenhum ponteiro recusado, nenhum arquivo não encontrado, nenhum texto
que não saibamos desenhar.

- Alien Breaker Deluxe
- Alpine Racer
- Treino Cerebral
- Zeebo F.C. Super League
- Zeebo Sports Peteca
- Zeebo Sports Queimada
- Zeebo Sports Tenis
- Zeebo Sports Volei

### Roda com ressalvas (37)

Passou os seis segundos, mas o relatório apontou alguma coisa. O balde é conservador: o
`config.cfg` que o Quake não acha é normal, porque só existe depois de salvar.

| Jogo | Classes que pede e não temos | Outras ressalvas |
|---|---|---|
| Armageddon Squadron |  | arquivo não encontrado, ponteiro recusado |
| Bad Dudes vs. DragonNinja | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Caveman Ninja | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Crash Bandicoot Nitro Kart 3D |  | arquivo não encontrado |
| Dark Seal | as seis do grupo de extensões | ponteiro recusado |
| Disney All Star Cards |  | API por hipótese, ponteiro recusado |
| Double Dragon | `0x0102f679` `0x0102f681` `0x01030852` | ponteiro recusado, texto sem fonte |
| FIFA 09 | `0x01001029` | arquivo não encontrado |
| Galaxy on Fire | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Heavy Barrel | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Iron Sight |  | arquivo não encontrado, ponteiro recusado |
| Karnovs Revenge | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Magical Drop 3 | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Pac-Mania |  | API por hipótese |
| Powerboat Challenge | `0x01001039` + as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Quake | as seis do grupo de extensões | arquivo não encontrado |
| Quake 2 | as seis do grupo de extensões | arquivo não encontrado |
| Raging Thunder 2 |  | arquivo não encontrado, ponteiro recusado |
| Rally Master Pro | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Reckless Racing |  | arquivo não encontrado, ponteiro recusado |
| Resident Evil 4 - Zeebo Edition | `0x0102f681` | texto sem fonte |
| Ridge Racer | as seis do grupo de extensões |  |
| Spin Master | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Street Hoop | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Super BurgerTime | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Tekken 2 |  | API por hipótese |
| Ultimate Chess 3D | `0x01002000` `0x01005503` | arquivo não encontrado, ponteiro recusado |
| Um Jogo de Ovos | as seis do grupo de extensões | arquivo não encontrado |
| Wizard Fire | as seis do grupo de extensões | arquivo não encontrado, ponteiro recusado |
| Zeebo Clube | `0x0100110a` |  |
| Zeebo Extreme Baja |  | arquivo não encontrado |
| Zeebo Extreme Boia Cross |  | arquivo não encontrado |
| Zeebo Extreme Corrida Aerea |  | arquivo não encontrado |
| Zeebo Extreme Jetboard |  | arquivo não encontrado |
| Zeebo Extreme Rolima |  | arquivo não encontrado |
| Zeebo F.C. Foot Camp |  | arquivo não encontrado |
| Zeebo Family Pack |  | arquivo não encontrado, ponteiro recusado |

### Não roda (12)

| Jogo | Onde para |
|---|---|
| Action Hero 3D - Wild Dog and IMICRO3D | para no laço — acesso inválido a 0x00000000 (pc 0x00055568) |
| Alice no Pais das Maravilhas | para no laço na volta 204 — acesso inválido a 0x00000000 (pc 0x000105ac) |
| Bejeweled Twist | para no laço — acesso inválido a 0x00000024 (pc 0x00032b78) |
| Need For Speed - Carbon - Domine a Cidade | não chega a criar o applet |
| Prey Evil | para no laço — salta para o endereço zero (lr 0x000161d8) |
| Turma da Monica em Vamos Brincar Vol. 1 | para no laço na volta 4 — acesso inválido a 0x00000000 (pc 0x0008a5a0) |
| Z-Wheel | não chega a criar o applet — pede `AEECLSID_SQLMGR` |
| Zeebo App | não chega a criar o applet — pede `0x01028e51` |
| Zeebo Channels - Opera Mini | não chega a criar o applet — pede a classe de rede `0x0100102e` |
| Zenonia | não chega a criar o applet — pede `0x01003109`, do subsistema de texto dele |
| Pac-Mania | lento demais — desenha, mas pixel a pixel pela API |
| Zumas Revenge | para no laço na volta 55 — acesso inválido a 0x0000000c (pc 0x00046b94) |

> **Esta tabela é de uma medição anterior.** A varredura de abertura de hoje mostra que
> Z-Wheel, Zeebo App, Action Hero 3D, Alice, Prey Evil, Turma da Mônica e Zuma's Revenge **criam
> o applet** — os quatro últimos já criavam, e os dois primeiros deixaram de estar presos aí. Onde
> cada um para depois disso é medição de execução, e ainda não foi refeita. As três que continuam
> sem criar o applet estão detalhadas em [As quatro que não abrem](#as-quatro-que-não-abrem-uma-a-uma).

### O `nResID` que não valia nada

`ISHELL_LoadResObject` recebe o arquivo, o **número do recurso** dentro dele e a **classe** que o
jogo quer de volta. Ignorávamos as duas últimas: pegávamos o caminho, tentávamos decodificar o
arquivo inteiro como PNG e devolvíamos nulo quando não era.

O Tekken 2 pede a entrada **5034** do `tekken2.bar`, um BMP de 308 KB. Nós tentávamos ler os 734
KB do `.bar` inteiro como se fosse a imagem. O jogo seguia com uma imagem sem tamanho, e imagem
sem tamanho é divisão por zero na hora de montar a tela — era o que enchia o log dele de
`Arithmetic exception: Divide By Zero` no menu e na seleção de personagem.

O relatório dizia isso o tempo todo, numa linha fácil de ler errado:

```
hipóteses em uso:
  um recurso pedido por LoadResObject não é um PNG que saibamos ler
```

Não era "não é um PNG". Era **não é uma imagem** — era o arquivo de recursos inteiro. É a terceira
vez neste documento que a resposta estava no relatório antes de alguém entender a pergunta.

Três coisas mudaram: o `nResID` passou a valer, o cabeçalho `AEEResBlob` é pulado (o que o
`RESBLOB_DATA()` do SDK faz), e a classe pedida passou a valer — o Tekken pede `AEEIID_IBITMAP` e
recebia um `IImage`, o que o fazia chamar um método de `IBitmap` numa vtable de `IImage`.

Três jogos mudaram de estado:

- **Toy Raid** parava no laço de quadros e agora abre com o menu inteiro desenhado.
- **Pac-Mania** era "roda e não mostra nada"; agora mostra a tela de escolha de idioma inteira e
  certa. Ficou lento porque passou a ter o que desenhar, e o perfil de API apontou onde: cinco
  segundos virtuais dele saíram de 174 para 62 de relógio quando o `IIMAGE_Draw` passou a honrar
  o recorte, e o que sobra é a sincronização de superfície em chamada que não desenha.
- **Tekken 2** parou de estourar divisão por zero.

### O jogo que insistia

O **Tekken 2** era o último "lento demais": **766 mil `Play` e 766 mil `GetState` em quatro
segundos virtuais**, e seis segundos de jogo não terminavam em cinco minutos de máquina. Não era
carga de trabalho. A música dele é MP3, o `IMEDIA_Play` respondia "esse som já acabou", o jogo
consultava o estado, via "pronto" e mandava tocar de novo. Para sempre.

Com a duração lida do cabeçalho do MP3 — sem decodificá-lo, ver
[07-audio.md](07-audio.md) —, o som "toca" em silêncio pelo tempo certo e ele anda: **9,7
segundos** para os mesmos seis virtuais, com a tela de título desenhada. Ele continua sem passar
dali, e isso é entrada, não desempenho: o jogo registra o sinal de botão, drena dois toques e
para de responder.

### O `IDIB` que ninguém pediu

Duas entradas saíram desta lista de uma vez, e pelo mesmo motivo. Um `IBitmap` de software do
BREW **é** um `IDIB`: a struct segue com campos públicos, e o jogo lê o tamanho direto deles,
sem `QueryInterface`. Nós só preenchíamos esses campos quando a interface era pedida.

O **Peggle** montava cada sprite como um quadrado de lado zero — 76.618 dos 77.208 triângulos de
um quadro descartados por área nula, tela preta com o jogo desenhando o tempo todo. O log dele
dizia `-size 0/0`, e a lista de rejeições por motivo do rasterizador confirmou. Hoje ele desenha
geometria, mas ainda fica preso no carregamento e os sprites saem sem textura.

O **Heavy Weapon** era o "lento demais" de 6 milhões de chamadas de API por quadro. Não era carga
de trabalho: era o jogo repetindo contra bitmaps que mediam 0×0. Seis segundos virtuais saíram de
**mais de cinco minutos** (estourando o `timeout`) para **2,1 segundos**, e ele desenha — com as
cores erradas, o que é o próximo passo dele.

### As seis classes do grupo de extensões

```
0x0103d8de  0x0103d8eb  0x0103d8ef  0x0103d8f0  0x0103def1  0x010426e3
```

Pedidas por dezessete jogos, e **nenhum deles precisa delas para rodar**: todos recebem
`ECLASSNOTSUPPORT` e caem no caminho alternativo. O log do Quake e o dos ports de arcade mostram
de onde vêm — o `GLES_ext.c` do próprio SDK do BREW, que os jogos linkam:

```
eglGetProcAddress (NBI) - got V2 EGLSurfaceManip interface
eglGetProcAddress (NBI) - got V2 GLESImageonExt interface
```

Ou seja: são as extensões de OpenGL ES do console, e as duas que os jogos anunciam ter conseguido
são justamente as que já implementamos. O que essas seis fazem, e se alguma muda o desenho na
tela, ainda não foi levantado — mas nenhuma delas impede jogo nenhum de rodar hoje.

## As quatro que não abrem, uma a uma

Levantadas pela varredura de abertura (`a_rom_indicada_abre`, ver [17-testes.md](17-testes.md)):
**60 das 63 ROMs criam o applet**; três não. A quarta — Kingdom Hearts — foi corrigida no caminho,
e vale contar o que era.

### Kingdom Hearts: o manifesto que estava no lugar "errado" — corrigido

`nenhum .mif ao lado do módulo diz qual applet criar`. O `.mif` **estava** no pacote:

```
Kingdon Hearts/Kingdon Hearts.mif          <- o manifesto
Kingdon Hearts/Kingdon Hearts_/swv21brew.mod   <- o módulo, numa pasta própria
```

Nós procurávamos o `.mif` em dois lugares calculados por nome: ao lado do `.mod`, e no
`<Título>/mif/<id>.mif` da disposição do console. Nenhum dos dois é este. Agora a busca também
**varre** a pasta do módulo e a de cima, o mais perto primeiro, com ordem estável.

Ele abre, e o obstáculo seguinte já apareceu: o jogo pede `0x0100a004` e `0x0102bbfc`, não as
encontra, desenha `The application has failed and will close (Error 14)` e sai sozinho. O 14 é
`0x14` — vinte, o `ECLASSNOTSUPPORT` que devolvemos. Nenhuma das duas está na tabela de classes da
partição APPS.

### Opera Mini: `CreateInstance` recusa com `EFAILED`

O applet não nasce porque o construtor dele pede a `0x0100102e` e nós não a temos. A interface
dela agora está **lida**, não suposta: 59 métodos, vtable `0x10a783e4` — ver
[15-o-que-falta-da-nand.md](15-o-que-falta-da-nand.md), inclusive por que ela não saía antes.

### Zenonia: orçamento esgotado dentro do `CreateInstance`

Mesma família do Need For Speed, e o mesmo desfecho: `b #0x84634`, um salto para si mesmo logo
depois do `dbgprintf`. Ele também **para de propósito**, e o orçamento acaba em cima disso.

A diferença é a causa: aqui falta mesmo a classe `0x01003109`, o subsistema de texto dele. O
`ISHELL_CreateInstance` dela devolve `ECLASSNOTSUPPORT` e o log do próprio jogo nomeia o que
quebrou:

```
:AF![B:\WIPI\WBLIB\src\WBLCore.cpp:241]:(-268435448):
:FALSE && "CWBLText::Create() failed!"(0):
```

A vtable dela também está lida agora: 40 métodos, em `0x10e15c04`. E a sonda mostra que ele usa
**quatro** deles — os slots 4, 6, 8 e 18. Com a classe respondida por observação
(`--sonda=0x01003109`), o Zenonia cria o applet, recebe o `EVT_APP_START` e roda o laço de
quadros; a tela ainda sai preta, que é o próximo passo dele.

### Need For Speed Carbon: não era lentidão, era uma parada de propósito — corrigido

Este é o mais interessante dos quatro, porque o rótulo estava errado. "Orçamento de instruções
esgotado" soa como jogo pesado; o perfil diz outra coisa — **97,8% das 500 milhões de instruções
num único bloco**, o `0x001a3c80`. Aquele endereço é isto:

```
0x1a3c78  mov  r0, #0x32
0x1a3c7c  blx  r1              <- helper 39, dbgprintf
0x1a3c80  b    #0x1a3c80       <- e fica aqui para sempre
```

Um `b` para si mesmo. O jogo **decidiu** parar, e as duas linhas do log são as que ele imprime
antes: `source\mediacenter\ZeeboSnd.cpp:327` e `BREAKPOINT!`.

Logo acima está o motivo. Ele varre até setenta entradas de uma tabela de sons chamando
`strstr(entrada, "snd/skid/skid.wav")`, e o rastreamento mostra as setenta devolvendo zero — a
agulha é sempre o mesmo ponteiro, `0x1a40ec`, e as entradas preenchidas são todas
`snd/carbon_fe/*.wav`. Não achando o som, ele assume defeito de programação e trava.

E o arquivo **existe** no pacote, e a tabela **também** tem o `snd/skid/skid.wav`, no índice 53.
A busca é que não achava — e não por causa do skid.

O jogo registra cinco sons de jogo em sequência, e **o terceiro nome da lista é a string vazia**.
Em C, `strstr(qualquer_coisa, "")` devolve o próprio texto: toda string contém a string vazia, e a
busca acha o índice 0 na hora. A nossa devolvia "não achou" — o `needle.len().max(1)` procurava
uma janela de um byte igual a uma agulha de zero bytes, que nunca casa. Aí ele varria as setenta
entradas, concluía que o som não existe, e travava de propósito.

Uma linha de semântica do C, e o jogo aparecia no relatório como "lento demais". **Hoje ele cria
o applet, recebe o `EVT_APP_START` e apresenta 181 quadros de OpenGL em seis segundos virtuais**,
com a abertura da EA na tela.

Vale a lição de método, que é a terceira vez que aparece nesta página: o `--profile` **não
imprimia nada** quando o jogo morria antes do laço de quadros, que é justamente o caso do Need
For Speed. Enquanto o perfil só existia no laço, "orçamento esgotado" não tinha como virar
"parada de propósito na linha tal".

## A largura do `printf` era um defeito invisível

Vale registrar porque não aparece como quebra nenhuma nesta tabela.

O `cformat` lia as flags e a largura do especificador e **descartava** — o que serve para o log do
`DBGPRINTF`, que era o uso original, mas o mesmo formatador atende o `snprintf` e o `vsnprintf`
do guest. O Resident Evil 4 monta o nome dos arquivos de estágio com `%s_%02d.h2z`; sem a
largura, o `%02d` de zero saía como `0`, ele procurava `3d_stg02_0.h2z` e o arquivo é
`3d_stg02_00.h2z`. **Os doze estágios não eram encontrados**, e o jogo rodava sem textura
nenhuma — sem quebrar, sem erro, sem nada no relatório além de uma lista de "arquivos não
encontrados" que ninguém tinha lido.

Foi a mesma lista que entregou o `font.fnz` dos ports de arcade, e nas duas vezes ela estava lá
desde o começo. A lição: **a lista de arquivos não encontrados do relatório é sinal, não ruído.**

## Como repetir, por teste

O mesmo levantamento roda dentro do `cargo test`, no [`src/varredura.rs`](../../src/varredura.rs).
A diferença que importa não é a comodidade: é que uma regressão passa a aparecer **no commit que
a causou**, e não na próxima vez que alguém repetir a varredura à mão.

```bash
# um jogo, com o relatório inteiro na tela
ZEEBX_ROM="roms/Quake.zip" cargo test --release varredura -- --nocapture

# a varredura inteira, gravando um relatório por jogo e cobrando a linha de base
ZEEBX_ROM=roms ZEEBX_ROM_SAIDA=saida ZEEBX_ROM_BASE=docs/varredura \
  cargo test --release varredura -- --nocapture
```

| Variável | O que faz |
|---|---|
| `ZEEBX_ROM` | um `.mod`/`.zip`, uma lista separada por vírgula, ou um diretório |
| `ZEEBX_ROM_MS` | quanto tempo **virtual** rodar, em ms (padrão 6000, o desta página) |
| `ZEEBX_ROM_TETO` | teto de tempo **real** por jogo, em segundos (padrão 90) |
| `ZEEBX_ROM_SAIDA` | diretório onde gravar o relatório completo de cada jogo |
| `ZEEBX_ROM_BASE` | diretório da linha de base: o que falta é gravado, o que existe é cobrado |

Nenhuma ROM está na árvore, então sem `ZEEBX_ROM` os dois testes avisam e passam — `cargo test`
não vira uma varredura de sessenta e cinco jogos sem alguém ter pedido.

São dois testes, e a diferença entre eles é o custo:

- **`a_rom_indicada_abre`** é a pergunta barata: carrega o `.mod`, roda o `AEEMod_Load`, lê o
  `.mif` e chama o `CreateInstance`. Quem não cria applet falha aqui sem gastar os seis segundos
  virtuais que nunca iria rodar.
- **`a_rom_indicada_avanca`** entrega o `EVT_APP_START`, roda o laço de quadros e monta o
  relatório: estado, desempenho, o que faltou e o log do jogo. A partida vem separada da primeira
  volta, e é o que distingue as duas linhas desta página — "quebrou no `EVT_APP_START`" e
  "quebrou no laço de quadros" apontam para trechos de código diferentes.

O teste falha por duas razões, e só por essas duas:

1. **O jogo não chegou ao fim do tempo pedido.** Não carrega, não cria o applet, quebra, ou não
   cumpre o tempo virtual dentro do teto de tempo real. Pendência não é falha: um jogo que roda
   pedindo seis classes que não temos continua passando, porque é assim que ele se comporta.
2. **O resumo mudou em relação à linha de base**, quando ela existe. É o que cobra pendência: uma
   API que deixou de ser atendida, uma classe que passou a ser pedida ou um arquivo que sumiu
   aparecem como `+`/`-` na saída do teste.

A linha de base guarda o estado e as pendências, e **nada de desempenho** — nem tempo, nem
instrução, nem quadro. Se guardasse, trocar de máquina acusaria regressão em todos os jogos de
uma vez, e um teste que acusa sempre não é lido nunca. Por isso o `--release` também não é
enfeite: em depuração o núcleo roda uma ordem de grandeza mais devagar, e o teto de tempo real
classificaria jogo bom como "lento demais".

## Como repetir, à mão

```bash
for z in roms/*.zip; do
  n=$(basename "$z" .zip)
  timeout 90 ./target/release/zeebx run "$z" --seconds=6 > "saida/$n.txt" 2>&1
done
```

O emulador extrai o `.zip` sozinho e escolhe o `.mod` certo, então não é mais preciso desempacotar
à mão. Quem estourar o `timeout` é "lento demais".

O que ler em cada saída, **nesta ordem** — é ela que separa as categorias:

| Linha | Significa |
|---|---|
| `applet: nenhum .mif encontrado` | nem chegou a criar o applet |
| `start: EVT_APP_START → retornou …` | o applet foi criado e recebeu o evento inicial |
| `start: EVT_APP_START → acesso inválido …` | quebrou no evento inicial, antes do primeiro quadro |
| `parou na volta N em …` | quebrou já dentro do laço de quadros, na volta N |
| `arquivos não encontrados:` | **leia sempre** — um nome errado aqui vira tela preta ou quebra, sem erro nenhum |
| `classes desconhecidas: …` | ClassIDs que o jogo pediu e não temos |
| `chamadas:` | quantas vezes cada método foi chamado — é o que aponta gargalo |
| `log do jogo:` | os `DBGPRINTF` do próprio jogo, que costumam nomear o problema |
