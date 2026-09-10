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

O levantamento é de leitura dos pacotes, não de suposição: os ports de arcade trazem **um** `.wav`
cada — o efeito — e a música deles chega pelo `IMedia` como MIDI, o que o relatório dizia numa
linha fácil de passar batido, `som recusado (audio/mid)`.

O MP3 já toca (ver abaixo). O MIDI é o assunto do fim desta página.

`wav.rs` lê:

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
IMedia::Play  ──►  decodifica o WAVE ou o MP3  ──►  voz no misturador
```

Os identificadores (`MM_PARM_MEDIA_DATA = 1`, `MM_PARM_VOLUME = 4`, `MM_PARM_MUTE = 5`,
`MM_PARM_PLAY_REPEAT = 11`, `AEE_MAX_VOLUME = 100`, `MMD_BUFFER = 1`) vêm de `AEEIMedia.h`.

O `RegisterNotify` é atendido: o jogo recebe `MM_STATUS_START` quando o som começa e
`MM_STATUS_DONE` quando ele acaba, no `AEEMediaCmdNotify` de 28 bytes de `AEEIMedia.h`. Sem esse
aviso, um jogo que só toca o próximo som quando o anterior termina emudece depois do primeiro.

`IMedia::GetState` devolve o objeto a "pronto" quando o som acabou — é o que o jogo consulta para
saber que pode tocar o próximo. **Quem diz que acabou é o relógio virtual, não o misturador**: o
emulador roda mudo sem deixar de contar o tempo, e há som que toca em silêncio porque sabemos
cronometrá-lo sem saber decodificá-lo.

## O misturador

`audio.rs`. Cada `Voice` tem posição, passo, volume, quanto falta e se está pausada. O passo é a
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

A música é MP3, `wav.rs` recusa, e o `Play` caía no caminho de "sem som legível", que respondia
ao jogo que **o som já tinha acabado**. O jogo consultava o estado, via "pronto", e mandava tocar
de novo. Para sempre.

O primeiro conserto não foi um decodificador. Foi `mp3.rs`, que lê o cabeçalho do primeiro quadro
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
`mp3.rs` ganhou o `decode`, que devolve o mesmo `Sound` que o RIFF/WAVE produz: o misturador não
sabe de onde o som veio, e reamostragem, volume e repetição funcionam iguais.

A decodificação em si é do **symphonia**, Rust puro, só o MP3 habilitado. É a mesma decisão do
SQLite para o `ISQLMgr` e do `ab_glyph` para o `DrawText`, e vale dizer por quê: o Layer III é
Huffman, requantização, estéreo conjunto, IMDCT e banco de síntese polifásico, e escrever isso
sem uma referência para comparar a saída dá o pior resultado possível — som que toca, parece bem e
está errado.

Ordem no `machine.rs`: RIFF/WAVE primeiro, porque é o que quase todo som é e é o mais barato de
reconhecer; MP3 depois; e só então a recusa, que continua nomeando o formato. O caminho da duração
ficou como rede de segurança para um MP3 que o decodificador recuse.

A verificação, medida:

- O tom de teste de 440 Hz decodifica com **pico de 0,761 e altura de 440 Hz** — a altura é
  conferida por cruzamento de zero, porque tom errado passaria por qualquer outra verificação. A
  saída bate com a do decodificador de referência em sete casas decimais.
- O **Tekken 2** saiu de silêncio para **71,6% de amostras não nulas** em oito segundos, com pico
  de 0,698. A hipótese "o Tekken fica mudo" saiu do relatório dele.
- Decodificar é feito **uma vez por trilha**, não por `Play`: o resultado entra no cache de sons
  do `machine.rs`. Uma trilha de trinta e seis segundos a 22 kHz mono são seis megabytes de `f32`,
  e nenhum dos nossos jogos troca de música com frequência que justifique fluxo.

A lição vale além do áudio: **antes de otimizar um jogo lento, conferir se ele está trabalhando
ou insistindo.** O Heavy Weapon era o mesmo caso, com bitmaps que mediam 0×0.

## O que falta

**A música dos ports de arcade.** Onze jogos entregam MIDI, e MIDI não se decodifica: se
sintetiza. Não há amostra dentro do arquivo — há a partitura, e o instrumento vinha do
sintetizador do firmware do console, que não temos. Qualquer som que a gente produza aí é uma
aproximação declarada, e é assim que ela deve aparecer no relatório.
