# 21 — A refatoração do subsistema de áudio

Este documento abre a refatoração pedida depois do #40. Ele existe porque o defeito do #43 — as
vozes da Turma da Mônica — **continua aberto**, e a investigação provou que a causa não é uma linha
de código: é a **estrutura** do subsistema.

## O que já está descartado, com número

| elo | medida | veredito |
|---|---|---|
| decodificação | 122 sons, taxa, canais e duração iguais aos arquivos do `pakz` (conferido com `ffprobe`) | correto |
| mixer | `passo 1.0000` em toda voz (som de 44.100 num mixer de 44.100) | correto |
| entrega ao frontend | **98–101% de 44.100 quadros por segundo real**, com o frontend aceitando 100% | correto |
| RetroArch | entrada 44.100 (`Set audio input rate to: 44100.00 Hz`), reamostrador CC, saída em 48.000 | correto |

Nenhum elo do caminho dobra a velocidade — e o sintoma existe. Isso só é possível porque a estrutura
permite **medir um caminho e tocar outro**.

## O que a estrutura permite (o 5-why de três pernas, publicado no #43)

1. **O mixer é ligado em três lugares** (`--dump-audio`, a saída da interface e o core) e **a taxa é
   decidida em dois** (44.100 no core, a do aparelho na saída). Quatro medições seguidas mentiram
   porque o caminho medido não era o caminho jogado: uma corrida sem `--dump-audio` **não tinha
   mixer nenhum**, e relatou "zero sons decodificados" com 123 sons pedidos.
2. **A fronteira com o aparelho não tinha instrumento nenhum**: ninguém contava quadros entregues
   contra aceitos, nem registrava a taxa do dispositivo.
3. **O registro era um anel de 300 linhas despejado no fim da função**: o jogo fala muito no boot, as
   linhas do áudio eram sobrescritas, e o fechamento abrupto da janela perdia o resto. Pelo libretro
   funciona, porque o core entrega as linhas ao frontend por quadro; no standalone, não.
4. **O contrato do BREW não é honrado por inteiro.** A SDK diz que **um só `ISoundPlayer_Play` fica
   ativo por objeto, e um `Play` novo no mesmo objeto é recusado** (`AEESoundPlayer.H:1224-1226`, com
   "You need to issue a stop before you can call another"); o nosso mixer **substitui** a voz. Faltam
   também as decisões de dono do buffer e o significado exato de `MM_PARM_PLAY_REPEAT`.

## O desenho

1. **Um dono só para o áudio**: um `AudioHost` que possui a saída, a taxa canônica e o registro. Os
   frontends deixam de montar o mixer: pedem o host e lhe entregam a saída.
2. **Uma fronteira instrumentada**: quadros produzidos, quadros aceitos e a taxa do aparelho, sempre
   visíveis e sempre em arquivo. A instrumentação deixa de depender de onde o mixer nasceu.
3. **O contrato do BREW como a SDK descreve**: voz por objeto, `Play` recusado quando já há som
   tocando, dono do buffer respeitado, fim de som pelo **relógio virtual** e não pelo mixer.
4. **Decodificadores validados contra a fonte**: taxa, canais e duração conferidos por som, e o passo
   da voz calculado a partir deles.
5. **O registro com saída de arquivo**, escrita conforme as linhas nascem, para sobreviver ao
   fechamento abrupto da janela.
6. **Testes que prendem o contrato**: `Play` recusado, duração da voz igual à do som, mono contra
   estéreo, a bomba de descompressão, e um consumidor falso que mede a fronteira em qualquer frontend.

## Critério de aceitação

- o sintoma do #43 deixa de reproduzir, com a evidência de **antes e depois** na mesma sessão;
- a fronteira medida em qualquer frontend, com um número por segundo no log;
- os três comandos de CI verdes, e cada teste novo falhando **antes** do conserto correspondente.
