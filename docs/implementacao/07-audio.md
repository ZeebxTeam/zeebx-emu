# 07 — Áudio

## O que os jogos entregam

Quase tudo é RIFF/WAVE: efeito e trilha chegam num buffer de memória, e é o que `wav.rs` lê.

A exceção é o **Tekken 2**, cuja música é um MP3 de 140 KB — MPEG-2, Layer III, 64 kbps, 22.050 Hz,
mono, com a etiqueta `Info` do codificador. Ele custou caro justamente por ser exceção, e a
história está mais abaixo.

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
IMedia::Play  ──►  decodifica o WAVE  ──►  voz no misturador
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

O conserto não é um decodificador de MP3. É `mp3.rs`, que lê o cabeçalho do primeiro quadro e a
etiqueta `Xing`/`Info` do codificador e devolve **só a duração** — quando a etiqueta traz a
contagem de quadros o número é exato mesmo com taxa variável; sem ela, sobra a conta do tamanho
pela taxa de bits. Com a duração, o som "toca" em silêncio pelo tempo certo do relógio virtual, o
`GetState` responde "tocando" enquanto isso, e o jogo segue.

É uma troca declarada, e ela sai no relatório como hipótese em uso: **o Tekken fica mudo, mas
anda**. Seis segundos virtuais saíram de mais de cinco minutos para **9,7 segundos**, e ele
desenha a tela de título. O que sobra nele agora é outro assunto: 1,08 milhão de `DrawPixel` por
quadro, que é custo de despacho de API.

A lição vale além do áudio: **antes de otimizar um jogo lento, conferir se ele está trabalhando
ou insistindo.** O Heavy Weapon era o mesmo caso, com bitmaps que mediam 0×0.

## O que falta

**Tocar MP3 de verdade.** Hoje só a duração é honrada. Um decodificador de Layer III é trabalho
de gente grande, e o único jogo que cobra é o Tekken 2.
