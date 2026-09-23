# 07 — Áudio

## O que os jogos entregam

**Efeito é WAVE; música não.** Essa é a divisão, e ela explica um sintoma que durou muito tempo:
efeito sonoro tocava em todo jogo e trilha não tocava em nenhum. Não era um defeito no
misturador — eram dois formatos inteiros faltando.

Contado nos pacotes dos sessenta e três títulos:

| Formato | Onde aparece |
|---|---|
| RIFF/WAVE | efeito sonoro, em 14 jogos — sempre tocou |
| **MP3** | a música de 9 jogos: Quake, Quake 2, Galaxy on Fire, Rally Master Pro, Powerboat Challenge, Action Hero 3D, Need For Speed, zeetris e a Z-Wheel |
| **MIDI** | a música dos ports de arcade: Double Dragon, Bad Dudes, Caveman Ninja, Dark Seal, Heavy Barrel, Karnov's Revenge, Magical Drop 3, Spin Master, Street Hoop, Super BurgerTime e Wizard Fire |
| **PCM gerado pelo jogo** | os ports de arcade da Data East de novo: o som da placa emulada sai por um `ISource`, ver abaixo |

O levantamento é de leitura dos pacotes, não de suposição: os ports de arcade trazem **um** `.wav`
cada — o efeito — e a música deles chega pelo `IMedia` como MIDI, o que o relatório dizia numa
linha fácil de passar batido, `som recusado (audio/mid)`.

O MP3 já toca (ver abaixo). O MIDI é o assunto do fim desta página.

`audio/wav.rs` lê:

- PCM de 8 bits (sem sinal, centrado em 128) e de 16 bits (com sinal);
- **IMA ADPCM** (formato 17), que é o que o Pac-Mania usa.

Formato desconhecido é recusado **pelo número**: `WavError::NotPcm(tag)`. Um som que some sem
explicação vira uma caçada; um erro que diz "formato 85" vira uma linha de pesquisa.

## O caminho do BREW

```
IMediaUtil::CreateMedia(AEEMediaData { clsData, pData, dwSize })
        │
IMedia::SetMediaParm(MM_PARM_MEDIA_DATA | VOLUME | MUTE | PLAY_REPEAT)
        │
        ├──► na volta seguinte do laço, lê os bytes e decodifica o WAVE, o MP3 ou o MIDI
        │
IMedia::Play  ──►  voz no misturador
```

Os identificadores (`MM_PARM_MEDIA_DATA = 1`, `MM_PARM_VOLUME = 4`, `MM_PARM_MUTE = 5`,
`MM_PARM_PLAY_REPEAT = 11`, `AEE_MAX_VOLUME = 100`, `MMD_FILE_NAME = 0`, `MMD_BUFFER = 1`) vêm de
`AEEIMedia.h`.

**O buffer é lido na volta seguinte do laço de eventos** (`Machine::resolve_midia`, chamado no
`deliver_signals`), e o cache (`CargaDeMidia`) é pela chave do **conteúdo**. Os dois extremos
quebram jogos diferentes:

- **Lido no `Play`**, com cache por `(endereço, tamanho)` — como foi por muito tempo —, um jogo que
  carrega vários sons pelo mesmo buffer de rascunho já o reaproveitou quando toca, e um som novo no
  mesmo endereço e com o mesmo tamanho tocava o antigo.
- **Lido na entrega** — a primeira correção, vinda da comparação com o zeebulator3 —, o Zeebo F.C.
  Super League entregava o som de navegação do menu com só o cabeçalho WAV escrito: ele entrega,
  manda tocar e copia as amostras em seguida, no mesmo tratador. Saía um chiado com o conteúdo antigo
  do buffer, onde havia até o cabeçalho de uma textura ATC.

A volta do laço fica entre os dois: o tratador do jogo já terminou de escrever e ainda não
reaproveitou o buffer. No aparelho o `Play` também só lê depois, numa tarefa separada. Um `Play`
sobre um som ainda não lido marca o objeto como tocando, avisa o início e começa a voz quando o
buffer é lido; o `GetTotalTime` força a leitura na hora, porque quem pergunta precisa da duração.

Três formas de entrega:

- `MMD_BUFFER`: memória. Um buffer que começa com a assinatura do gzip (`1f 8b`) é descomprimido
  antes — o `sound.ggz` do Double Dragon guarda assim.
- `MMD_FILE_NAME`: o nome de um arquivo do pacote, lido pelo sistema de arquivos virtual. O
  Galaxy on Fire entrega as sete músicas dele assim (`GalaxyOnFire1_Theme.mp3` e as outras), e
  antes elas eram recusadas em silêncio. Um arquivo que não existe responde `EFAILED` e entra no
  relatório.
- `MMD_ISOURCE` só é tocado como PCM cru: um `ISource` que entregue um arquivo comprimido fica
  mudo.

As classes da família `AEECLSID_MULTIMEDIA` do SDK criam todas o mesmo objeto: QCP, PMD,
MIDIOUTMSG, MIDIOUTQCP, MPEG4, MMF, PHR, AAC, IMELODY, AMR, XMF e DLS, além de MEDIA, MIDI, MP3,
ADPCM e PCM. O conteúdo passa pelos decodificadores que temos, e o que nenhum lê termina na hora.
QCELP e EVRC não são decodificados aqui nem no zeebulator3.

O `RegisterNotify` é atendido: o jogo recebe `MM_STATUS_START` quando o som começa e
`MM_STATUS_DONE` quando ele acaba, no `AEEMediaCmdNotify` de 28 bytes de `AEEIMedia.h`. Sem esse
aviso, um jogo que só toca o próximo som quando o anterior termina emudece depois do primeiro.

**O `START` e o fim natural saem na volta do laço, e não na saída do `Play`**
(`Machine::entrega_avisos_de_midia`, no `deliver_signals`). Os Zeebo Extreme dependem disso. O
gerenciador de som deles (`SoundMgr`/`SoundPlayer` da biblioteca TTD) marca o som como "pedido"
(2) **depois** do `Play`, e o `START` o passa a "tocando" (1). Com o aviso na saída da chamada, o
`START` chegava antes da marca e o som ficava em 2 para sempre. O `Stop` deles só age em 1, então
a música do menu nunca parava. A música da pista ficava na fila esperando o canal (`+0x3634` do
tocador), e com música na fila o tocador recusa todo efeito: o Bóia Cross corria só com a trilha,
sem uma chamada de som sequer. O bloco do aviso é escrito na hora de cada entrega, porque um
`START` e um `DONE` do mesmo objeto na mesma volta dividem o bloco.

**Todo aviso sai assim, inclusive o `DONE` do `Stop`, e ele não some com o `Release`.** O
Double Dragon repete a música pelo `DONE`: se o objeto ainda está marcado, agenda um `Play` para
dali a 100 ms. Ele desmarca e solta o objeto logo depois do `Stop`; com o `DONE` na saída do
`Stop`, o tratador ainda via o objeto marcado, e o `Play` agendado caía num `IMedia` já liberado —
endereço zero, ao apertar voltar. O Zeebo F.C. Super League, ao contrário, para e solta e **espera**
o `DONE` daquele som: se o aviso fosse descartado com o objeto, a abertura parava na tela de aviso.
Por isso o tratador é guardado quando o aviso nasce, e a entrega não depende de o objeto existir.

**Um som entregue por memória é relido a cada `Play`.** O `IMedia` do aparelho não copia o buffer.
O Super League tem um objeto só para os efeitos da partida, com um buffer de 500 KB: escreve o
chute ou o passo e manda tocar de novo, sem outro `SetMediaParm`. Guardado da primeira leitura,
todo efeito saía com o som de seleção do menu, o primeiro a passar por ali. A releitura segue a
regra de cima — na volta do laço — e o cache pelo conteúdo evita decodificar de novo o que não
mudou.

O `Stop` de um som que tocava também avisa, **com `MM_STATUS_DONE`**. Era o que deixava as corridas do Crash Nitro Kart mudas. Ele conta os
sons ativos e só toca a música da pista quando a conta zera; o tratador de aviso dele (`0x11a80`)
desconta no `DONE` (2) e no status 9 e **ignora o `ABORT` (3)**. Na entrada da corrida ele para as
músicas do menu com `Stop`: sem aviso nenhum, e depois com o `ABORT` que o zeebulator3 usa, elas
nunca saíam da conta, e a corrida inteira — contagem, motor e a trilha de 642 KB — ficava sem um
único `Play`. Com `DONE`, a contagem toca aos 31 s, a trilha entra em repetição e o motor troca de
som com a rotação.

**O WAVE acaba onde o `RIFF` diz.** O Zeebo F.C. Super League monta sons num buffer de rascunho de
500 KB e grava no bloco `data` o tamanho do buffer inteiro, mas no `RIFF` o tamanho do som (14 KB).
Seguir o `data` tocava onze segundos — o efeito e depois lixo de memória, alto, por cima da música.
Quando o `data` passa mais de 4 KB do fim que o `RIFF` declara, vale o `RIFF`; uma diferença de
poucos bytes é arquivo editado com o `RIFF` desatualizado, e não corta nada. Enquanto os sons eram
lidos no `Play`, esse buffer já tinha outro conteúdo e o defeito não aparecia.

Para conferir sem ouvir, o `zeebx sessao <zip> --dump-audio=A.wav` grava a mistura da sessão da
janela, no ritmo do relógio virtual.

Um `Play` sobre um som que ainda toca **não** avisa. Avisar fazia um ciclo nos jogos que tocam de
novo dentro do tratador: o novo `Play` caía sobre o som que acabara de começar, gerava outro aviso,
e o som reiniciava a cada quadro — o Zeebo F.C. Super League saía estourado e picotado.

**Um `Play` sobre a música que já toca em laço não a recomeça.** O gerenciador de som dos Zeebo
Extreme manda tocar a trilha da pista de novo toda vez que um efeito acaba: no Rolima, o efeito
de 10 KB toca, recebe `Stop`, e o `Play` cai no MP3 de 807 KB que já está em repetição infinita.
A voz recomeçava do início, e a música reiniciava a cada turbo e a cada derrapagem. Recusar com
`EBADSTATE` não serve: o jogo entende que a música parou e repete o `Play` a cada quadro. O que
ele espera é o `START`, que o passa a "tocando"; a voz segue de onde está. A regra vale só para o
laço infinito (`MM_PARM_PLAY_REPEAT` zero), porque um efeito tocado de novo por cima de si mesmo
é o que o Zeebo F.C. faz, e ele precisa recomeçar.

**O cache dos sons decodificados tem teto.** Passando de 64, saem os que nenhum `IMedia` usa. Sem
isso, cada conteúdo novo escrito no buffer de rascunho do Zeebo F.C. ficava decodificado para
sempre. Uma voz tocando não perde nada: o PCM dela está num `Arc` que o misturador também segura.

O `GetMediaParm` devolve o volume e o mudo guardados (antes, zero: um jogo que lê o volume e grava
de volta se emudecia), e um `IMedia` liberado para a voz dele no misturador — uma música em
repetição seguia tocando depois de o objeto sumir.

`IMedia::GetState` devolve o objeto a "pronto" quando o som acabou — é o que o jogo consulta para
saber que pode tocar o próximo. **Quem diz que acabou é o relógio virtual, não o misturador**: o
emulador roda mudo sem deixar de contar o tempo, e há som que toca em silêncio porque sabemos
cronometrá-lo sem saber decodificá-lo.

## O misturador

`audio/mod.rs`. Cada `Voice` tem posição, passo, volume, quanto falta e se está pausada. O passo é a
razão entre a taxa do som e a da placa: é assim que um WAVE de 8 kHz sai certo numa saída de
48 kHz. Há teste de cruzamento por zero provando que o reamostrar **preserva a altura** — errar
isso dá um som que toca, parece bem, e está no tom errado.

Vozes terminadas são recolhidas dentro do `fill`, no mesmo lugar em que são consumidas. A saída é
`cpal`.

## Como conferir sem ouvir

`--dump-audio=arquivo.wav` grava a mistura. Foi assim que a implementação foi verificada: pico de
0,700 no Quake, 0,807 no Double Dragon, 1,000 no Peteca, 0,586 no Pac-Mania, com a proporção
esperada entre blocos com som e blocos em silêncio.

Não substitui ouvir, mas separa "não sai som" de "sai som errado".

## O MP3 que não tocava, e o jogo que insistia

O Tekken 2 estava classificado como "lento demais": ele fazia **766 mil `Play` e 766 mil
`GetState` em quatro segundos virtuais**, e seis segundos de jogo não terminavam em cinco minutos
de máquina. Parecia um jogo pesado.

Não era. O rastro mostra o par se repetindo sem nada no meio:

```
IMedia::GetState → 2   (MM_STATE_READY)
IMedia::Play     → 0
IMedia::GetState → 2
IMedia::Play     → 0
```

A música é MP3, `audio/wav.rs` recusa, e o `Play` caía no caminho de "sem som legível", que respondia
ao jogo que **o som já tinha acabado**. O jogo consultava o estado, via "pronto", e mandava tocar
de novo. Para sempre.

O primeiro conserto não foi um decodificador. Foi `audio/mp3.rs`, que lê o cabeçalho do primeiro quadro
e a etiqueta `Xing`/`Info` do codificador e devolve **só a duração** — quando a etiqueta traz a
contagem de quadros o número é exato mesmo com taxa variável; sem ela, sobra a conta do tamanho
pela taxa de bits. Com a duração, o som "toca" em silêncio pelo tempo certo do relógio virtual, o
`GetState` responde "tocando" enquanto isso, e o jogo segue.

Era uma troca declarada, e saía no relatório como hipótese em uso: **o Tekken andava, mudo**. Seis
segundos virtuais saíram de mais de cinco minutos para **9,7 segundos**, e ele desenha a tela de
título. O que sobra nele agora é outro assunto: 1,08 milhão de `DrawPixel` por quadro, que é custo
de despacho de API.

## O MP3 que agora toca

A duração resolvia o travamento e não a música, e a música é de nove jogos — não de um. Então
`audio/mp3.rs` ganhou o `decode`, que devolve o mesmo `Sound` que o RIFF/WAVE produz: o misturador não
sabe de onde o som veio, e reamostragem, volume e repetição funcionam iguais.

A decodificação em si é do **symphonia**, Rust puro, só o MP3 habilitado. É a mesma decisão do
SQLite para o `ISQLMgr` e do `ab_glyph` para o `DrawText`, e vale dizer por quê: o Layer III é
Huffman, requantização, estéreo conjunto, IMDCT e banco de síntese polifásico, e escrever isso
sem uma referência para comparar a saída dá o pior resultado possível — som que toca, parece bem e
está errado.

Ordem no `machine/media.rs`: RIFF/WAVE primeiro, porque é o que quase todo som é e é o mais barato de
reconhecer; MP3 depois; e só então a recusa, que continua nomeando o formato. O caminho da duração
ficou como rede de segurança para um MP3 que o decodificador recuse.

A verificação, medida:

- O tom de teste de 440 Hz decodifica com **pico de 0,761 e altura de 440 Hz** — a altura é
  conferida por cruzamento de zero, porque tom errado passaria por qualquer outra verificação. A
  saída bate com a do decodificador de referência em sete casas decimais.
- O **Tekken 2** saiu de silêncio para **71,6% de amostras não nulas** em oito segundos, com pico
  de 0,698. A hipótese "o Tekken fica mudo" saiu do relatório dele.
- Decodificar é feito **uma vez por conteúdo**, na entrega: o resultado entra no cache de sons
  do `machine/media.rs`. Uma trilha de trinta e seis segundos a 22 kHz mono são seis megabytes de `f32`,
  e nenhum dos nossos jogos troca de música com frequência que justifique fluxo.

A lição vale além do áudio: **antes de otimizar um jogo lento, conferir se ele está trabalhando
ou insistindo.** O Heavy Weapon era o mesmo caso, com bitmaps que mediam 0×0.

## O MIDI, que não se decodifica: se sintetiza

`audio/midi.rs`. Onze jogos entregam a trilha como MIDI, e **MIDI não é som, é partitura**: não há
amostra dentro do arquivo, há "toque a nota 69 no instrumento 25 com força 100". Quem virava isso
em som era o sintetizador do firmware do console, com o banco de instrumentos dele — que está na
parte da NAND que ainda não lemos.

Então o que sai daqui é uma **aproximação declarada**, e ela aparece no relatório como hipótese em
uso. Dizer isso na cara importa mais que o de costume, porque neste caso o erro é silencioso: uma
música sintetizada errado *toca*, e soa como se fosse assim mesmo.

Por isso o módulo cobra de si só o que dá para verificar sem ouvir — **a nota certa, na hora
certa, pelo tempo certo**:

- **A partitura, como a especificação manda.** Formato 0 e 1, várias trilhas juntadas por pulso,
  status corrente, `Note On` de força zero valendo como `Note Off`, meta e SysEx pulados pelo
  tamanho declarado. O status corrente não é otimização: uma trilha de notas seguidas é quase toda
  assim, e ignorá-lo não perde um evento, perde a trilha inteira a partir do primeiro.
- **Pulso para segundo**, honrando a divisão do cabeçalho e cada `Set Tempo` do ponto dele para
  frente — recalcular a música toda com o tempo novo põe tudo o que vem depois de um
  *accelerando* no lugar errado sem nada parecer quebrado. A divisão SMPTE também conta: o byte
  alto é o número de quadros em complemento de dois, e negar os dezesseis bits juntos dá 24 onde
  deveria dar 25.
- **A afinação.** A nota 69 sai em 440 Hz, e o teste mede isso por correlação com o tom certo e
  com os dois semitons vizinhos — contar cruzamento de zero depende da forma da onda, correlação
  não. Afinação errada é o defeito que mais facilmente passa por "o sintetizador é assim".

O que é palpite, e é palpite por falta de dado: **o timbre**. A tabela mapeia família do General
MIDI para forma de onda e envoltória, tentando acertar o *comportamento* — piano decai, órgão
sustenta, metal ataca devagar —, porque é isso que faz a melodia ser reconhecível mesmo com o
instrumento errado. Percussão é ruído filtrado, com duas famílias de decaimento separando bombo
de prato.

Dois cuidados que não são detalhe:

- **Quadrada e dente são somadas por harmônicos** até abaixo de Nyquist, não geradas pela forma
  crua. Uma quadrada crua a 22 kHz rebate: os harmônicos acima da metade da taxa voltam como
  frequências que não estão na partitura, e a nota sai acompanhada de um assobio que sobe quando
  ela desce.
- **O pico é normalizado em 0,8.** Somar dezenas de vozes passa de 1,0 com facilidade, e o que
  passa corta — e corte soa exatamente como "o sintetizador é ruim", sem ser.

Medido no Double Dragon: a linha `som recusado (audio/mid)` saiu do relatório, e dez segundos de
jogo saíram de silêncio para **74,7% de amostras não nulas**, com pico de 0,807.

### O MIDI do Double Dragon, medido contra o Zeebulator

O Double Dragon foi a primeira exceção séria a "a aproximação basta". Comparado com o outro
emulador, o som dele é pobre — e a investigação mostrou que a causa é estrutural, não um ajuste.

**O pacote traz MIDI, e ele estava escondido.** Um `grep` por `MThd` no zip não acha nada: o
`sound.ggz` (1.928.097 B) é um cabeçalho de 592 B com **74 pares** `(offset, tamanho)` e 74
membros gzip contíguos. Conferidos os 74, o tamanho declarado bate em todos, e o conteúdo é
**62 `RIFF`/WAVE** (efeitos, mono 16 bits 22.050 Hz) e **12 `MThd`** (músicas). A BGM principal
tem 1.788 notas, 47,47 s, 9 canais e 404 eventos de percussão; os 12 somam 22.729 notas.

**O que a partitura pede** (e é aqui que a aproximação mais dói):

| família | fatia das notas |
|---|---:|
| pianos (0–7) | 28,7% |
| **guitarras (24–31)** | **23,2%** |
| percussão (canal 10) | 20,6% (4.681 notas) |
| metais | 9,2% |

**O Zeebulator vira isso com amostras, não com onda.** Ele carrega um soundfont General MIDI real
(GeneralUser GS, **32.319.396 B**) e toca com o **TinySoundFont** (MIT, `tsf.h`), em mono a
22.050 Hz e com **−16 dB** de folga antes do corte interno do `tsf`. Note On/Off, `Program
Change` e percussão vão para o banco: o canal 10 usa o kit do próprio soundfont (banco 128), não
uma classificação nossa.

**A comparação controlada**, mesma nota e mesma duração, passando o mesmo arquivo pelas duas
implementações (e por uma referência independente, `timidity` + `FluidR3_GM`):

| render | H2..H8 / H1 | centroide | rolloff 85% |
|---|---|---:|---:|
| Zeebulator com soundfont | 1 · .27 · .015 · .072 · .018 | 1383 Hz | 2647 Hz |
| `timidity` + FluidR3 | — | 1638 Hz | 3140 Hz |
| **Zeebx (`midi.rs`)** | 1 · **0** · .012 · **0** · .002 | **1065 Hz** | **1174 Hz** |

Ou seja: o nosso sai **mais escuro e mais pobre em harmônicos** que os dois renders de soundfont,
com afinação certa (441,0 Hz medidos contra 440,0 — erro de +0,23%, sem erro de oitava). O
problema é o **material**, não a altura.

E o defeito mais visível é este, medido programa a programa: **29 (guitarra overdrive), 30
(distorcida), 42 (violoncelo) e 48 (cordas) saem com a mesma onda** — a tabela mapeia família, e
a família `24–31` é "dente com decaimento". O 81 (lead saw) sai **quadrada** onde a partitura pede
dente de serra. O baixo (33) fica **plano em 0,65** até o fim, onde o soundfont decai para 0,27.
Nas guitarras isso é 23,2% das notas do jogo.

Medido na música inteira:

| render | pico | RMS |
|---|---:|---:|
| Zeebulator com soundfont | 25.727 (78,5% FS) | 4.877 |
| Zeebulator sem soundfont (fallback) | 32.767 (**100% FS**, corta) | 8.401 (+4,72 dB) |
| Zeebx (`midi.rs`) | 0,799988 (= 0,8 por construção) | ≈4.782 |

**Uma ressalva que evita uma conclusão errada:** no Zeebulator **upstream o soundfont só está
ligado em `tools/game_probe.cpp`**. O `frontends/standalone/main.cpp` constrói o `MediaHle` sem
ele, então cai no sintetizador tosco — que é *mais escuro* ainda (rolloff 880 Hz). Quem comparar
pelo standalone não está ouvindo soundfont nenhum; o binário com o banco é o `game_probe`.

**Licença: o código do Zeebulator não serve.** Ele é **GPLv3** e o Zeebx é **GPL-2.0-only**; as
duas não se combinam. O que serve é o que está sob licença própria: o **TinySoundFont** (MIT), o
`rustysynth` (MIT, Rust puro) e o próprio banco GeneralUser GS (licença permissiva, embora o texto
admita origem desconhecida de parte das amostras). O `oxisynth` é LGPL-2.1 e fica de fora.

**O custo, medido:** o banco tem **10,3×** o tamanho da árvore inteira do Zeebulator, pede
**+64 MiB de RSS** na carga (o `tsf` converte tudo para `float`) e levaria o `.so` do core de
**15,6 MB para ~48 MB** em seis alvos de CI. O caminho absoluto compilado do Zeebulator não serve
para o core: não é relocalizável, vaza o `$HOME` do build e, faltando o arquivo, a música degrada
em silêncio.

**Dois fatos que mudam o risco da troca:**

1. Trocar o sintetizador **não acusa regressão na linha de base** das 62 ROMs: pico e RMS ficam no
   relatório completo e **não** no resumo, por decisão registrada em `varredura.rs` — conferido,
   `0` dos 62 arquivos em `docs/varredura/` tem a linha `áudio:`.
2. Em compensação, **17 testes** dentro de `src/audio/midi.rs` travam propriedades da síntese
   atual (pico em `0,79..=0,81`, altura por correlação, brilho da percussão). Uma troca de motor
   os reescreve.

O plano, em duas etapas, com a medição no meio:

- **Etapa 1, barata:** corrigir na tabela exatamente o que foi medido — guitarras 24–31
  sustentadas (29 e 30 são 23,2% da partitura), 81 como dente de serra, baixo sem sustentação
  plana, e separar 42 de 48.
- **Etapa 2, fiel:** wavetable GM com licença compatível (`rustysynth`, MIT), com o banco como
  arquivo **opcional ao lado do core** e recuo para o sintetizador atual quando ele faltar — o
  core Libretro não pode ganhar dependência de host, e 32 MB embutidos em cada `.so` não se pagam.

O harness de comparação vive fora do repositório, em `~/zeebx-midi-harness`: `zbfont/` (C++ com o
`tsf.h` e o banco), `zxmidi/` (o nosso `midi.rs` num crate mínimo) e `abtest/` (os WAVs de cada
caso). É ele que mede cada passo em vez de opinar.

## O som que o jogo gera enquanto toca

Os ports de arcade da Data East não entregam um arquivo: eles emulam o chip de som da placa e
produzem as amostras quadro a quadro. A entrega é um `AEEMediaDataEx` com `clsData = MMD_ISOURCE`
(`0x01001012`), `bRaw` ligado, um `ISource` que o próprio jogo implementa e um `AEEMediaWaveSpec`
com o formato — 11025 Hz, mono, 16 bits com sinal, nos que foram medidos.

Enquanto só sabíamos ler memória e arquivo, a entrega era recusada e o log do jogo dizia
`Failed to SetMediaDataEx!!!!`: **nenhum** dos onze ports tinha som. Agora a entrega é
reconhecida, e o `Play` abre uma voz de fluxo no misturador.

**Quem marca o ritmo é o relógio virtual.** A cada volta do laço o emulador calcula quantos
quadros o relógio já deve, com um décimo de segundo de adiantamento para a placa não esvaziar
entre duas voltas, e chama o `ISource::Read` do jogo até juntar isso — no máximo algumas leituras
por volta, porque um `Read` que devolve menos do que foi pedido é o jogo sem amostras prontas. É a
mesma escolha do resto do som: sem placa nenhuma o emulador continua pedindo, e o jogo, que emula
o chip de som dentro do `Read`, anda do mesmo jeito.

A voz de fluxo vive em `audio/mod.rs`, ao lado das vozes de som pronto. Ela reamostra para a taxa
da placa interpolando entre dois quadros, guarda meio segundo no máximo — se o jogo entrega mais
rápido do que a placa consome, o excesso mais antigo sai em vez de o atraso crescer — e, faltando
amostra, segura o último valor em vez de estalar para o zero.

### Etapa 1 feita: a tabela de timbres refeita contra a medição

O plano era corrigir o que a comparação mediu. O que estava errado não era só um valor: era o
**modelo**. A tabela mapeava **família de oito** para uma forma fixa e uma envoltória fixa, e o
resultado medido é que 29 (*overdrive*), 30 (distorcida), 42 (violoncelo) e 48 (cordas) saíam com a
**mesma onda** — 23,2% das notas do Double Dragon entre as três primeiras.

Três mudanças, todas com a medição como alvo:

1. **A forma virou contínua.** Em vez de quatro degraus fixos, a série harmônica é `1/k^expoente`, e
   o expoente de cada família foi **ajustado por varredura**: renderiza, mede o centroide, compara
   com o do soundfont. Os degraus fixos erravam justamente porque os alvos caíam **entre** eles.
2. **O teto de harmônicos passou a ser o de Nyquist.** Era 12 para o dente e 9 para quadrada e
   triângulo. Com `1/k^e` pequeno o bastante os harmônicos altos **são** o timbre: cortados em 24, o
   lead de dente de serra media 2823 Hz onde a amostra real mede 3497 Hz.
3. **A normalização passou a ser calculada por nota**, varrendo 96 fases (20 µs por nota), porque o
   pico da série depende do expoente **e** do número de harmônicos que a altura da nota permite.
   Sem isso, o baixo (`e = 2,45`) sairia quase três vezes mais alto que o lead (`e = 0,70`), e o
   timbre mudaria o balanço da música junto.

Erro de centroide contra o soundfont, nos nove casos de referência:

| caso | soundfont | Zeebx antes | Zeebx agora | erro antes | erro agora |
|---|---:|---:|---:|---:|---:|
| piano (0) | 755 Hz | 398 | 695 | 357 | **60** |
| baixo (33) | 467 Hz | 396 | 465 | 71 | **2** |
| guitarra *over* (29) | 1379 Hz | 1011 | 1398 | 368 | **19** |
| guitarra dist. (30) | 2195 Hz | 1011 | 2233 | 1185 | **38** |
| cordas (48) | 1707 Hz | 1009 | 1717 | 697 | **10** |
| violoncelo (42) | 1573 Hz | 1009 | 1498 | 563 | **75** |
| metais (61) | 1926 Hz | 730 | 1988 | 1196 | **62** |
| lead serra (81) | 3497 Hz | 730 | 3492 | 2767 | **5** |
| bateria | 7092 Hz | 6661 | 6661 | 430 | 430 |
| **total** | | | | **7635** | **701** |

E as envoltórias, que é o que mais se ouve numa linha de baixo:

```text
baixo     soundfont 1.00 0.74 0.60 0.53 0.48
          antes     1.00 0.69 0.65 0.65 0.64   ← travava em 0,65: sustentava como órgão
          agora     1.00 0.93 0.86 0.79 0.71
piano     soundfont 1.00 0.82 0.69 0.60 0.54
          antes     1.00 0.65 0.34 0.29 0.29   ← decaía rápido demais e travava
          agora     1.00 0.90 0.80 0.70 0.59
bateria   soundfont 1.00 0.90 0.32 0.28 0.60
          antes     1.00 0.80 0.60 0.30 0.42   ← chimbau alto demais, condução baixa demais
          agora     1.00 0.90 0.35 0.44 0.64
```

Na música real do Double Dragon (o SMF de 47,5 s): pico 0,8 (por construção, o mesmo `normaliza()`)
e RMS **0,159**, contra **0,1488** do soundfont — era 0,1459.

**Um defeito de robustez apareceu no caminho.** Ao medir o SMF extraído do `.ggz`, a duração saiu
**168 s** onde o jogo toca 47,5 s: o parser varria todos os blocos `MTrk` do buffer, e o membro
extraído traz as músicas seguintes coladas. O jogo entrega um SMF por `Play`, então o defeito não
aparece em jogo — mas qualquer arquivo com duas músicas concatenadas rendia duração errada, que é o
sintoma que menos se nota. Agora a leitura para no número de trilhas que o cabeçalho declara.

Quatro testes travam o que foi corrigido, cada um falhando no código anterior: a distorcida não
pode ser a mesma onda da *overdrive*; o lead 81 tem harmônico par (dente de serra) e o 80 não
(quadrada); o baixo decai em vez de ficar plano; e duas músicas coladas não viram uma.

**O que continua de fora, medido:** a razão de harmônicos das cordas (8,37 na amostra real contra
0,6 do que uma série `1/k^e` consegue), a da distorção (4,58 contra 0,84), o centroide da bateria
(6661 contra 7092 Hz, porque ruído filtrado não tem a ressonância de um tambor) e a ondulação de
naipe de cordas (tremolo e coro do banco). Isso é material de amostra, e é o assunto da etapa
seguinte — o wavetable com `rustysynth`.

### Etapa 2 feita: o banco de amostras, opcional, com recuo

A tabela de timbres chegou ao limite do que uma soma de harmônicos alcança. O que falta é
**material de amostra**, e para isso entrou um sintetizador de SoundFont: `rustysynth`, MIT e Rust
puro.

**Por que `rustysynth` e não o código do Zeebulator.** O Zeebulator é **GPLv3** e o Zeebx é
`GPL-2.0-only`; as duas licenças não se combinam, então nada dele pode ser copiado — nem o
invólucro em volta do TinySoundFont. O `rustysynth` é MIT, então pode.

**Por que o banco não vem embutido.** São 32 MB (o GeneralUser GS mede 32.319.396 B) e a carga pede
+64 MiB de RSS, porque as amostras viram `float`. Embutido, o `.so` do core iria de 15,6 MB para
~48 MB em seis alvos de CI — 48 MB para rodar um jogo de 1 MB não se paga. O banco é procurado em
`<raiz do aparelho>/soundfonts/*.sf2`, e `ZEEBX_SOUNDFONT` aponta um arquivo direto.

**Sem banco, nada muda.** O MIDI volta para a tabela de timbres, e o relatório diz qual dos dois
caminhos tocou, pela hipótese em uso. O `rustysynth` é Rust puro: o core continua com zero
dependências de host, e o `ldd` só mostra libstdc++, libgcc, libm e libc.

Medido **nas doze músicas** do pacote, e não numa só: cada SMF foi extraído do `sound.ggz` e
renderizado pelos três caminhos, com a janela de 1 s a 40% da duração de cada música.

| render | erro médio de centroide | pior caso | melhor caso |
|---|---:|---:|---:|
| **Zeebx com o banco de amostras** | **11,3%** | 27,2% | 0,2% |
| Zeebx com a tabela de timbres | **40,3%** | 115,9% | 1,5% |

Na música principal e mais longa (47,5 s) os números são melhores que a média — **2,5%** com o
banco contra **38%** com a tabela —, e é por isso que a medição nas doze importa: a média é a
resposta honesta, e ela diz que o banco é **3,6× mais próximo** da referência, não que seja igual.

O que sobra dos 11,3% é do **sintetizador**, não do material: os dois tocam as mesmas amostras, mas
o `rustysynth` e o TinySoundFont interpolam e tratam envoltória de formas diferentes, e o caminho
do Zeebulator ainda passa por mono com −16 dB de folga. Também é honesto registrar que a tabela
acerta em cheio em duas músicas (1,7% e 1,5%) e erra feio em duas outras (114,7% e 115,9%): a
aproximação não é uniformemente ruim, ela depende de quais instrumentos a música usa.

Dois defeitos apareceram na medição, os dois por comparação com a referência:

1. **O teto de duração era 36 s num caminho e 300 s no outro**, e a música de 47,5 s saía cortada
   pelo caminho do banco. Dois caminhos que dizem tocar a mesma partitura não podem ter tetos
   diferentes. Agora os dois usam o mesmo.
2. **O nível não era o mesmo.** A tabela deixa o pico em 0,8 (ver `midi::normaliza`) e o banco
   saía em 0,53: a mesma música trocava de volume conforme o aparelho tivesse ou não um `.sf2`
   instalado, e no jogo isso mexe no balanço entre a trilha e os efeitos, que passam pelo mesmo
   misturador. Agora o banco normaliza igual.

Custo medido: a carga do banco mais a renderização da música de 47,5 s levam **295 ms**, uma vez
por música e com o banco guardado por caminho (o jogo toca doze).

#### Qual banco usar: medido, não escolhido no gosto

Quatro candidatos, com o que foi verificado em fonte primária (tamanho por `HEAD` no arquivo, licença
pelo `copyright` do pacote Debian ou pelo repositório do autor):

| banco | tamanho | licença | presets | centroide contra a referência |
|---|---:|---|---:|---:|
| **GeneralUser GS** | 32,3 MB | permissiva, mas o texto admite origem desconhecida de parte das amostras | **287** | **+2,5%** |
| FluidR3_GS | 2,4 MB | **MIT** (Frank Wen, no `copyright` do Debian) | **33** | +7,3%, com RMS 3× menor |
| FluidR3_GM | 114 MB | **MIT** | — | não medido: grande demais para distribuir |
| MuseScore_General (Lite) | 32,6 MB | **MIT** | — | **não é lido**: é `.sf3`, e o `rustysynth` rejeita SF3 (há teste no próprio crate) |

O banco pequeno **não resolve**, e o motivo é medido: o FluidR3_GS tem 33 presets porque é um
subconjunto GS, e a música do Double Dragon usa **25 programas GM** diferentes. Com 33 presets, a
maior parte cai no preset padrão, e o resultado é uma mistura mais fina (RMS 0,055 contra 0,143 do
GeneralUser, com o mesmo pico 0,8). Ele ainda é 7,3% melhor que a tabela, mas fica longe do banco
completo.

Ou seja: **fidelidade e licença limpa não cabem juntas no mesmo tamanho**. O GeneralUser GS é a
referência que o Zeebulator usa e o que chega a 2,5%; a licença dele permite redistribuir, mas o
próprio texto diz que não se sabe a origem de todas as amostras. Quem publica tem de decidir isso.
Um caminho aberto: o `rustysynth-ext` lê SF3, e aí o MuseScore_General (MIT, 32,6 MB) entra — não
foi testado aqui.

#### O caminho de produção, provado pelo relatório

O módulo tem teste próprio, mas o que importa é o caminho que o jogo percorre:
`Machine::decodifica_som` → `toca_com_banco` → `soundfont::toca`. Medido com a varredura apontada
para o Double Dragon, 30 s virtuais, mesma ROM:

```text
sem banco        hipótese: "a música MIDI é sintetizada aqui, com timbre aproximado"
                 áudio: rms 0,1511
com banco        hipótese: "a música MIDI é tocada com o banco de amostras do aparelho"
                 áudio: rms 0,1375
```

A linha de hipótese é a prova: ela só aparece quando o banco **entrou**, e é escrita no relatório do
jogo, não no teste. E o RMS muda junto — 9% menor, que é o banco trocando o material.

**Uma nota que evita confusão na próxima varredura:** a varredura das 62 ROMs roda com
`--no-default-features`, e a feature `soundfont` **não** está ligada ali. É de propósito: o resumo da
linha de base não tem áudio, e ligar o banco mudaria só os números do relatório completo. Quem
quiser ver o efeito do banco na varredura precisa passar `--features soundfont` **e** apontar
`ZEEBX_SOUNDFONT`; sem uma das duas coisas, o caminho é o da tabela de timbres.

#### O core diz onde o banco deve ficar

A busca é por diretório, e a pasta sai da raiz de sistema que o frontend entrega — quem instala o
core não tem como adivinhar. Então o core **diz**, no log, em uma linha. Medido no RetroArch desta
máquina (1.20, driver `glcore`):

```text
sem banco   Zeebx: sem banco de amostras do MIDI; a trilha toca com a tabela de timbres.
            Para ouvir com amostras, ponha um .sf2 em
            /home/…/RetroArch/system/zeebx/aparelho/soundfonts (ou aponte ZEEBX_SOUNDFONT)

com banco   Zeebx: banco de amostras do MIDI em
            /home/…/RetroArch/system/zeebx/aparelho/soundfonts/GeneralUser-GS.sf2;
            a trilha toca com as amostras
```

A escolha de **avisar** em vez de acrescentar mais um diretório de busca é deliberada: um caminho a
mais é um palpite, e palpite em caminho de arquivo se paga com "não funciona e não diz por quê". A
compilação sem a feature também responde — dizendo que não tem o sintetizador —, porque silêncio
aqui vira a mesma conclusão errada.

#### O custo de renderizar a música, e o defeito que ele revelou

Medido com a mesma música (Double Dragon, 47,5 s), em `--release`, nesta máquina:

| caminho | carga do banco | renderizar a música inteira |
|---|---:|---:|
| tabela de timbres | — | **6,25 s** |
| banco de amostras (GeneralUser GS, 32 MB) | 23 ms | **289 ms** |

O banco é **21× mais rápido**, e isso não era esperado. O motivo é o trabalho por amostra: a tabela
soma `sin()` por harmônico **em cada amostra** (até 48 harmônicos × 1.057.823 amostras), enquanto o
banco toca amostras gravadas com interpolação.

**O defeito que isso revelou é mais sério que a diferença de fidelidade.** A síntese acontece dentro
do despacho de API, na chamada `Play` do jogo: com a tabela, cada música que começa **trava o jogo
por 6,25 s** nesta máquina — e o aparelho é bem mais lento que ela. O Double Dragon toca doze
músicas. Com o banco o mesmo trecho leva 289 ms, o que ainda se nota, mas é outra ordem.

**Corrigido, e a correção foi a clássica da síntese aditiva**: uma volta de seno é calculada
**uma vez**, e cada harmônico é lido dessa mesma volta a `k` vezes a velocidade — então cada
harmônico custa uma leitura, e não um `sin()`. A onda fica guardada por `(expoente, número de
harmônicos)`, e as vozes de uma música compartilham as mesmas tabelas (algumas centenas de pares
distintos para 1788 notas).

| | antes | depois |
|---|---:|---:|
| renderizar 47,5 s de música | 6,25 s | **183 ms** |
| erro de centroide contra o soundfont | 701 Hz | **699 Hz** |

Ou seja: **34× mais rápido**, com o timbre intacto — a diferença de 2 Hz é a quantização de uma
tabela de 1024 pontos, e a interpolação linear entre pontos vizinhos mantém a nota aguda sem
chiado. E o caminho da tabela ficou **mais rápido que o do banco** (183 ms contra 289 ms), o que
inverte a conta de custo entre os dois: o banco continua ganhando em fidelidade, não em tempo.

Uma segunda correção possível ficou de fora: renderizar em pedaços, espalhando o custo pelos
quadros seguintes. O misturador já tem voz de fluxo (`open_stream`/`feed_stream`) para som que o
jogo entrega aos poucos, mas o sequenciador teria de sobreviver entre chamadas, e isso mexe no save
state. Com 183 ms por música, não se paga.

#### Os outros dois jogos que usam MIDI, e uma hipótese que a medição derrubou

Três jogos do acervo entregam a trilha como MIDI: Double Dragon, Ultimate Chess 3D e Zuma's
Revenge. Os dois últimos trazem **um** SMF curto cada (326 B em formato 1 com seis trilhas, e 228 B
em formato 0), escondidos em contêineres próprios — `.sar` e `.dat`, os dois com `MThd` no meio.

Medido contra a mesma referência:

| música | referência | tabela de timbres | erro |
|---|---:|---:|---:|
| Ultimate Chess 3D | 583 Hz | 499 Hz | 14,4% |
| Zuma's Revenge | 3703 Hz | 3373 Hz | 8,9% |

Ou seja, **9% a 14%** nestes dois, contra a média de 40% no Double Dragon. Não é contradição: o
erro da aproximação depende de **quais instrumentos a música usa**, e o Double Dragon usa 25
programas GM, com 23% de guitarras — justamente onde a tabela confundia 29, 30, 42 e 48.

**A hipótese que caiu.** No Chess a energia (RMS) do nosso render saiu **duas vezes** a da
referência, e a leitura fácil era "a normalização de pico está comprimindo tudo para o mesmo
volume". Medido: **não**. O pico bruto da soma sai em 2,64 e 6,59 nesses dois jogos, então a
normalização **sempre atenua** (fatores 0,30 e 0,12) — ela nunca amplifica. O que difere é o nível
da referência, que varia por música (0,78 no Double Dragon, cerca de 0,42 no Chess), enquanto o
nosso é fixo em 0,8 por construção.

Fica como está, e o motivo é honesto: o nível do hardware não é conhecido — o caminho do Zeebulator
aplica −16 dB de folga por escolha dele, não por medida do console —, e um pico fixo é o que evita
corte quando a música entra no mesmo misturador que os efeitos. Trocar isso por um palpite não
seria fidelidade, seria outra escolha.

### Um `IAStream` do jogo como fonte

O Aviãozinho, um port do Quake feito por fãs, monta o `ISource` de outro jeito: escreve um
`IAStream` próprio, cujo `Read` devolve o que o mixer do Quake acabou de misturar, e pede ao
`ISourceUtil` (slot 6) que o transforme em `ISource`. Esse slot só aceitava um `IFile` aberto e
recusava o resto; o `SNDDMA_Init` desistia, e o `S_Init` lia `shm->speed` do buffer nulo e
derrubava o jogo aos 24 ms. O `Read` do `IAStream` e o do `ISource` estão no mesmo slot, com os
mesmos argumentos e o mesmo retorno, então o próprio stream é devolvido como fonte, e a voz de
fluxo chama o código do jogo como já fazia com os ports de arcade. O formato vem no
`AEEMediaWaveSpec` de sempre: 22050 Hz, estéreo, 16 bits.

## O que falta

**O banco de instrumentos do console.** Com ele, o MIDI deixa de ser aproximação e passa a soar
como soava — e o caminho para ele é o mesmo das classes que faltam: o leitor de EFS2 (ver
[14](14-z-wheel-e-o-efs2.md) e [15](15-o-que-falta-da-nand.md)).

Enquanto o banco do console não aparece, a referência de fidelidade é a comparação medida: o
wavetable com `rustysynth` e um banco General MIDI instalado ao lado do core. Falta **um banco
padrão** para distribuir — o GeneralUser GS serve e a licença permite, mas o texto dele admite
origem desconhecida de parte das amostras, então quem publica precisa decidir isso com os olhos
abertos.
