# 07 — Áudio

## O que os jogos entregam

Nenhum decodificador exótico foi preciso: **todos os jogos testados passam RIFF/WAVE**. A
suspeita inicial de que a trilha viria em MP3 não se confirmou — o `IMedia` recebe um buffer com
um WAVE dentro.

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

`IMedia::GetState` devolve o objeto a "pronto" quando a voz acabou — é o que o jogo consulta para
saber que o som terminou.

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

## O que falta

**A notificação de fim não é entregue.** `IMedia::RegisterNotify` é aceito e o callback nunca é
chamado. Um jogo que só toca o próximo som quando avisamos que o anterior acabou fica em silêncio
depois do primeiro.

A peça que faltava já está levantada, em `inc/AEEIMedia.h`:

```c
typedef struct AEEMediaCmdNotify {
   AEECLSID clsMedia;   // +0
   IMedia * pIMedia;    // +4
   int      nCmd;       // +8    MM_CMD_PLAY = 4
   int      nSubCmd;    // +12
   int      nStatus;    // +16   MM_STATUS_DONE = 2
   void *   pCmdData;   // +20
   uint32   dwSize;     // +24
} AEEMediaCmdNotify;    // 28 bytes

typedef void (*PFNMEDIANOTIFY)(void *pUser, AEEMediaCmdNotify *pCmdNotify);
```

É a explicação mais provável do áudio que "funciona por um tempo e para".
