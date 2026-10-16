# 24 — Velocidade: limite de quadros, frameskip, fast-forward, turbo e rewind

As cinco funções da issue #32 mexem na mesma coisa: o ritmo do jogo contra o relógio do mundo. O
relógio do jogo é virtual (ver [04-tempo.md](04-tempo.md)) e anda com o trabalho que o jogo faz.
Quem o prende ao relógio do mundo é a sessão, comparando os dois a cada volta. Por isso a lógica
das cinco mora no núcleo, em `src/velocidade/` e no `Session::anda`, e os frontends só ligam
teclas e menus.

| Função | Onde vale | Padrão |
|---|---|---|
| Limite de quadros (60 / 30 / desligado) | todos os frontends; no core, a opção `zeebx_limite_fps` | 60 |
| Frameskip (desligado / automático / fixo 1–6) | todos; no core, `zeebx_frameskip` | desligado |
| Fast-forward (2x–10x, ou sem limite) | Qt, egui, Android | 3x, F10 segurado |
| Turbo (clássico, alternar, segurar) | Qt, egui, Android (jogador 1) | desligado |
| Rewind (pontos de retorno) | Qt, egui, Android | desligado, F8 |

O core Libretro não reimplementa fast-forward, turbo nem rewind: o RetroArch tem os três. Ver
[O que o core não faz](#o-que-o-core-não-faz). O iOS fica de fora deste trabalho, e por isso o
`Session::step(budget, bool)` continua existindo: ele é o `anda` com o ritmo de sempre.

## O ritmo de uma volta

O frontend entrega à sessão um `Ritmo`: a proporção (quantas vezes a velocidade do console;
`None` é sem freio), o frameskip, a meia apresentação dos 30 FPS e se o som fica calado. O
`speed_limit: bool` de antes é o `Ritmo::CONSOLE` ou o `Ritmo::SEM_FREIO`.

**Trocar a proporção reancora o relógio.** A 3x o relógio virtual corre na frente do real.
Medido de volta a 1x desde a mesma âncora, ele aparecia adiantado por tudo o que ganhou, e o freio
o segurava em `Step::Ahead` até o mundo alcançar. Era o que já acontecia ao religar o limite
depois de um tempo sem ele. A âncora nova começa no instante da troca.

**Sem freio, a volta corre até a fatia acabar.** Antes, com o limite desligado, o `step` voltava
no primeiro quadro apresentado. O "desligado" da janela ficava preso a um quadro do jogo por
retraço do monitor, e o F10 do PR #83 tinha o mesmo teto. Agora ele roda quadros até gastar
`FATIA_SEM_FREIO` (12 ms), o que deixa 4 ms para a janela desenhar num retraço de 60 Hz. O número é
a conta do retraço, não uma medição. Sem esse teto, uma volta que gastasse a fatia inteira passaria
do retraço, a seguinte receberia o dobro, e em poucas voltas a janela estaria a 10 quadros por
segundo.

**O teto de quadros por volta cresce com a proporção.** A 1x é o `QUADROS_POR_VOLTA` (4) de
sempre. A 10x, uma volta de 16 ms precisa de dez quadros só para andar em dia.

## O frameskip

Pular **não muda a velocidade do jogo**: a lógica roda inteira, e só o desenho 3D e a limpeza
daquele quadro deixam de acontecer (`Machine::define_pula_desenho`). Um jogo que lê a tela de
volta (`glReadPixels`) nunca pula, a mesma regra que o core já tinha.

A decisão é **por quadro, no começo dele**. Um quadro pode atravessar duas voltas da janela, e
decidir de novo no meio dele desenharia metade da cena. Por isso a sessão guarda o número da troca
de buffer em que decidiu, e só a troca seguinte abre a próxima decisão.

O **automático**, fora do core, pula o quadro que não precisa ir à tela:

- **Um quadro desenhado por volta basta.** Depois dele, pular os seguintes deixa na tela o que ele
  desenhou, uns quadros mais velho.
- Antes dele, um quadro só é pulado se, pulado, ainda deixar tempo na fatia para um desenhado.
  Com freio, ele também precisa continuar atrasado e a volta não pode ter batido no teto.
- O custo de um quadro pulado e o de um desenhado são medidos **separados**.

As três regras saíram de medições que derrubaram as versões anteriores:

| Regra | Rolima a 10x | Defeito |
|---|---|---|
| Pular tudo o que não bate no teto da volta | 8,27x | o último quadro da volta saía pulado quase sempre, e a tela parava |
| Pular só se ainda couber um desenhado, com uma medida só (a do último quadro) | sem limite: 3,95x | um jogo pesado nunca via sobra, e desenhava tudo |
| Pular só se ainda couber um desenhado, com as duas medidas | 4,11x | conservador demais |
| As duas medidas, **e** pular tudo depois do primeiro desenhado | 5,76x (média de cenas) | — |

No core, o automático continua sendo o aviso de buffer de áudio do RetroArch, como a `libretro.h`
manda. O contador do fixo e da meia apresentação é o mesmo nos dois (`ContadorDePulo`).

**Durante o fast-forward o automático fica ativo mesmo com o frameskip desligado.** Desenhar 600
quadros por segundo para mostrar 60 é o que impediria o host de chegar à proporção. Um frameskip
fixo escolhido continua valendo.

### O jogo que fica para trás

Um relato da 0.4.2: no Qt, numa máquina com EndeavourOS, o NFS parava (imagem e som, com a janela
respondendo) com qualquer frameskip fora do automático, e voltava ao automático. Não se
reproduziu: aqui o NFS anda a 1x na corrida com todas as escolhas, trocadas no meio. A suspeita é o
jogo rodar abaixo do tempo real naquela máquina, porque o automático é a única escolha que pula
quadro quando o jogo atrasa.

Para o próximo relato trazer o dado, a sessão vigia a velocidade (`VigiaDaVelocidade`). Quando,
num segundo real, o jogo anda menos da metade do que o ritmo pede, o registro diz o quanto andou,
o frameskip, a meia apresentação, o custo do último quadro desenhado e do último pulado, e quantas
voltas houve e quantas estavam adiantadas. Uma linha por episódio, e outra quando ele volta a 90%.
Uma pausa ou o rewind não contam: a medida recomeça depois de um buraco entre as voltas. Lento de
verdade aparece como uma velocidade baixa com quadros caros; parado aparece como zero.

## O fast-forward

A velocidade é uma proporção fixa de 2x a 10x, ou "sem limite". O atalho é F10 (o do PR #83, para
quem já se acostumou), trocável na aba Atalhos, por tecla ou por botão de qualquer controle
ligado. O modo pode ser segurar ou alternar. Pausado, o fast-forward não vale.

Medido em 2026-10-10, rasterizador de processador, janela simulada a 60 Hz, média de 4 rodadas
intercaladas de 1,5 s (intercaladas porque o jogo muda de cena com o tempo, e medir um modo inteiro
depois do outro comparava cenas diferentes):

| Jogo | 3x | 10x | Sem limite | Sem limite, sem pulo |
|---|---|---|---|---|
| Crash Nitro Kart | 2,97x | 9,60x | 23,97x | 8,11x |
| Zeebo Extreme Rolima | 2,95x | 5,76x | 8,48x | 3,97x |
| Double Dragon | 2,98x | 9,92x | 100,55x | 10,38x |

Ao soltar, os três voltaram a 0,99x–1,00x. O caminho da placa e o Android não foram medidos.

### O som

**O som é puxado pelo relógio da placa, e não pelo do jogo.** A placa pede amostras no ritmo do
mundo (`cpal`). A 3x, o jogo tocava a música em 1x e entregava os efeitos três vezes mais juntos.
O fim de cada som, que o jogo mede pelo relógio virtual, chegava antes de o mixer tocá-lo inteiro.
E os fluxos PCM descartavam o excesso no teto de meio segundo, aos estalos.

O mixer agora multiplica o passo de cada voz e de cada fluxo pela velocidade
(`Mixer::define_avanco`). O som acompanha o jogo, mais agudo, como uma fita acelerada. A velocidade
usada é a **alcançada**, numa média móvel de meio segundo, e não a pedida. Com 10x pedidos e o host
chegando a 4x, um passo de 10 deixaria o som duas vezes e meia na frente do jogo. A 1x o ritmo é
exatamente 1, porque seguir a oscilação da medida desafinaria a música de quem nem está avançando.

**A média parte da proporção pedida, e não de 1x.** O jogo acelera no aperto: no Action Hero 3D a
3x, os primeiros 100 ms já andam 2,8x. Partindo de 1x, a média levava um segundo e meio para chegar
perto dos 3x, e o som e o rótulo ▶▶ subiam devagar atrás do jogo. O fast-forward parecia gradual.
Do segundo aperto em diante, com a mesma proporção, ela parte de onde parou da última vez. No mesmo
jogo a 10x o host chega a 6,7x, e partir de 10 a cada aperto fazia a altura escorregar por um
segundo e meio; medido de novo, o segundo aperto começa em 6,56x. O rótulo usa essa mesma média
(`Session::velocidade_alcancada`), e não a amostra de meio segundo do painel.

"Sem som" cala o mixer enquanto avança. "Sem limite" é sempre mudo: a velocidade real varia de
quadro leve para quadro pesado, e o som que a acompanhasse mudaria de altura o tempo todo.

O caminho certo, e maior, é o jogo produzir as amostras pelo relógio virtual num anel que alimenta
a placa, como o core já faz (`render(devidas)`). É a refatoração "um dono só para o áudio" do
[21-refatoracao-do-audio.md](21-refatoracao-do-audio.md), e fica para ela.

## O turbo

**O ritmo é o do relógio virtual, e não o do host.** O frontend entrega o controle uma vez por
volta, e uma volta pode rodar vários quadros: quatro para alcançar o relógio, dez a 10x. Um turbo
que alternasse o botão a cada volta pulsaria dez vezes mais devagar para o jogo no fast-forward. Por
isso o frontend diz **quais** botões pulsam (`Session::define_turbo`), e a sessão decide **quando**
eles estão apertados, antes de cada volta do laço de eventos, pela fase do relógio do jogo: metade
do período apertado, metade solto.

O padrão são 10 toques por segundo, o período de 6 quadros que o RetroArch usa de fábrica, numa
faixa de 5 a 20. Um jogo que lê o controle por `GetState` a 20 quadros por segundo pode perder
toques acima de 10 por segundo; os que leem pela fila de eventos, não.

Os modos são os do RetroArch, por jogador:

- **Clássico**: segurar a tecla de turbo junto com um botão faz aquele botão pulsar.
- **Botão único (alternar)**: um toque na tecla de turbo faz o botão padrão pulsar enquanto ele está
  segurado, e outro toque desfaz. Ele **não** dispara sozinho: um autofire travado atravessaria os
  menus, onde o botão 1 é o "confirma".
- **Botão único (segurar)**: enquanto a tecla de turbo está segurada, o botão padrão pulsa sozinho.

O direcional e o HOME nunca pulsam. O RetroArch exclui o direcional por padrão, e o HOME pulsando
abriria e fecharia o menu do console. A tecla de turbo é uma entrada a mais no mapeamento de cada
jogador (`buttons["turbo"]`), e não um botão do Zeebo.

**A tecla BREW também pulsa.** O controle chega ao jogo por dois caminhos: o estado do `IHID` e as
teclas `EVT_KEY` (o botão 1 é o confirmar, `0xe064`; o 2 é o `AVK_CLR`), que a janela do desktop deriva do
controle. Pulsar só o estado deixaria sem turbo o jogo que lê por tecla. Em vez de levar a
derivação inteira para a sessão, o que mudaria cinco frontends, a sessão **afirma por cima** a tecla
dos botões que pulsam e a devolve ao estado cru quando eles param. Isso só acontece quando o
frontend avisa que deriva as teclas (`Session::turbo_nas_teclas`). O core e o Android não derivam,
e lá a sessão não inventa uma tecla que ninguém solta.

## O rewind

**Pontos de retorno, e não um rewind liso.** Um estado do jogo tem de 15 a 66 MB e leva de 5 a
37 ms para ser gravado. Gravar a cada quadro, como o rewind do RetroArch faz, não cabe em 16 ms. O
rewind marca um ponto a cada intervalo de jogo (padrão 1 s), num anel com teto de memória (256 MB
no desktop, 128 MB no Android). Segurar F8 volta um ponto a cada 250 ms reais e mostra o quadro
dele, com o som calado. Ao soltar, o jogo segue do último ponto mostrado, e os mais novos que ele já
saíram do anel. Pausado, cada toque volta um ponto. O rewind vence o fast-forward: voltando, o jogo
não anda.

**Só a cópia roda no laço do jogo**; a compressão (deflate rápido) vai para uma thread à parte.
Medido em 2026-10-10, um ponto por segundo de jogo:

| Jogo | Caminho | Captura no laço (mediana / máx.) | Por ponto, comprimido | Voltar um ponto |
|---|---|---|---|---|
| Double Dragon | processador | 11,5 / 12,4 ms | 3,39 MB | 23–30 ms |
| Double Dragon | placa | 10,7 / 11,4 ms | 3,18 MB | 22–27 ms |
| Crash Nitro Kart | processador | 12,6 / 15,2 ms | 4,11 MB | 33–39 ms |
| Crash Nitro Kart | placa | 6,7 / 14,6 ms | 3,34 MB | 27–32 ms |
| Zeebo Extreme Rolima | processador | 33,5 / 38,9 ms | 18,34 MB | 133–144 ms |
| Zeebo Extreme Rolima | placa | 19,1 / 19,1 ms | 6,08 MB | 45–55 ms |

A captura é um soluço por ponto; no Rolima no processador, dois quadros perdidos por segundo. É
por isso que o rewind vem desligado. O Quake e o Android não foram medidos.

**Um ponto não nasce com arquivo aberto para escrita.** O estado guarda um arquivo aberto pelo
caminho e pelo deslocamento, e não pelo conteúdo (ver `machine/save.rs`). Voltar para o meio de uma
gravação faria o jogo continuar escrevendo de um deslocamento antigo por cima de um arquivo que já
mudou: o save do jogador. Um ponto que o motor recusa (um arquivo que o jogo apagou depois, por
exemplo) sai do anel, e o anterior é tentado. **O que o jogo gravou no disco não volta**: voltar
para antes de um save não desfaz o save, o mesmo que o save state manual faz.

### O rewind no Qt

O rewind não funcionava na janela Qt. Lá ele é o único caminho que restaura um estado, porque a
janela não tem save state manual, e a leitura do atalho rodava fora do contexto de GL do jogo.
Voltar um ponto refaz as texturas na placa: elas eram apagadas e criadas no contexto que estivesse
corrente, o do Qt Quick ou nenhum. A leitura agora vai com o contexto do jogo corrente, e a
`Partida`, como antes de andar, avisa o espelho do estado da placa
(`Session::retoma_o_contexto`) antes de voltar um ponto. O Android faz o mesmo aviso. Com o
rasterizador de processador o aviso não faz nada, e por isso nenhum teste sem placa o observa.

### A tela preta e o passo para a frente

Pela placa, o Magical Drop III e o Um Jogo de Ovos ficavam com a tela preta durante todo o rewind.
Duas coisas se somavam. O último quadro fica na textura da placa até alguém pedi-lo, e a textura não
entra no estado: o ponto voltava com a tela de um quadro antigo, ou preta num jogo que só desenha
pelo GL. E, restaurado o estado, o contador de escritas da tela voltava igual ao gravado, a tela
parecia intacta, e a janela mostrava a textura da placa, que volta vazia.

O ponto agora traz o quadro da placa para a tela antes de gravar (`materializa_quadro_gl`, com o
contexto do jogo corrente, logo depois de ele andar), e a máquina restaurada diz que a imagem é a
da tela até a próxima troca de buffer (`Machine::quadro_na_placa`). Medido com a `Partida` pela
placa, no Magical Drop III: sem a primeira metade, a tela de cada ponto saía com 0 pixels acesos;
com ela, com cerca de 237 mil.

A mesma medida mostrou o rewind indo **para a frente**: 30.023 ms, volta a 29.000, e o passo
seguinte de volta a 30.023. O ponto que a thread comprimia quando o jogo voltou entrava no anel
depois e virava o mais novo. Tirar um ponto agora muda a geração do anel, como limpar, e o que
estava em voo é descartado: 30.023, 29.000, 27.977, 26.954.

### O som que não voltava

O mixer não entra no save state, e restaurar não mexia nele. Voltar no rewind deixava tocando o
que o mixer tocava no instante de antes: a música da cena seguinte por cima da restaurada, para
sempre quando era um laço, porque o jogo restaurado não sabe daquela voz e nunca a manda parar.
Visto no Action Hero 3D, cuja música é MP3 em laço. O mesmo valia para o save state manual.

Agora o restaurar cala o mixer inteiro e toca de novo o que o estado diz que toca
(`Machine::refaz_o_som`). Um efeito volta do ponto que o relógio virtual marca, contando as
repetições. Um laço volta do começo, porque o instante em que ele começou não é guardado. Um fluxo
PCM volta vazio, e o jogo o enche de novo. Um som cujo cache não está na memória, num estado de
outra sessão, fica calado até o jogo tocá-lo de novo, como já ficava.

### O relógio que não voltava

O rewind expôs um defeito antigo do save state. O relógio do jogo é
`clock_us + instruções / 528`, e o estado gravava só as instruções. O comentário dizia que o
`clock_us` "é o contador de instruções dividido pela taxa", mas não é: ele é o tempo ocioso que o
emulador pula (o vsync, o `skip_idle_time`, o `MSLEEP`), e nos jogos medidos é a maior parte do
relógio. No Double Dragon, um estado gravado em 20.265 ms voltava em 25.408 ms. Os temporizadores e
o próximo vsync, gravados em tempo absoluto, venciam todos de uma vez. O `clock_us` agora entra
numa seção à parte, `relogio.us`, e o mesmo estado volta em 20.032 ms gravado e 20.032 ms
restaurado. A seção é opcional: um estado antigo carrega como carregava. Isso vale também para o
save state do Android e do core.

## Os atalhos

A aba **Atalhos** do Qt junta todas as teclas da janela do jogo num lugar só, como o *Hotkey
Settings* do Dolphin:

- **Trocáveis**: o screenshot (só teclado, F9), o fast-forward (F10), o rewind (F8) e o reiniciar
  (só teclado, R). O fast-forward e o rewind aceitam uma tecla ou um botão de qualquer controle
  ligado.
- **Fixos**, só para leitura: Esc, P, F11.

**O reiniciar abre o mesmo jogo de novo**, na hora, sem confirmação, como o botão de reset de um
console (`Nucleo::reinicia`). Não é um reset da máquina: a abertura é o único caminho que monta
tudo como o jogo espera — as configurações dele, as portas, o som, a Z-Wheel de onde ele veio. O
que o jogo gravou no disco fica. Como o screenshot, ele recusa uma tecla que já é botão do Zeebo, e
é perguntado antes do jogo. Quem já usava o R num botão, num arquivo de antes do atalho, ganha o
reiniciar **sem tecla** (`Settings::adota_o_atalho_de_reiniciar`), em vez de perder o botão. No
Android, o "Reiniciar" fica na caixa do voltar, ao lado de pausar.

A pausa mostra um **⏸** no centro da tela, com a palavra embaixo; continuar mostra um **▶** por meio
segundo, e o reiniciar um **↻**, que somem esmaecendo. São desenhados com `Shape`, e não com
caracteres: nem toda fonte tem o ⏸ e o ↻.

Um botão que é de atalho **não chega ao jogo** (`Atalhos::reservadas`). Se ele também for um botão
do Zeebo, o editor avisa, mas deixa salvar. Os padrões são só de teclado: o controle do Zeebo usa
quase todos os botões de um controle comum, e qualquer botão reservado de fábrica roubaria um de
algum jogo.

No Android não existe F10. O fast-forward, o rewind e o turbo ganham **peças na tela**, ao lado do
HOME e em cima do losango, e um botão do controle físico pode ser ligado a cada um nos ajustes. Os
nomes dos botões são os do desktop (`South`, `LeftTrigger`…), para o mesmo `settings.json` valer
nos dois. Cada peça tem uma chave de mostrar e esconder. O turbo e o rewind, além disso, só
aparecem com a função em uso; o fast-forward aparece sempre.

## O que o core não faz

O core Libretro só ganhou os tipos do núcleo para o limite e o frameskip, com as mesmas chaves de
opção. O fast-forward, o turbo e o rewind são do RetroArch. O rewind do RetroArch, porém,
serializa **a cada quadro**, e com estados de 15 a 66 MB que levam de 5 a 37 ms para gravar ele
não presta com este core. Isso fica dito aqui, e não prometido.
