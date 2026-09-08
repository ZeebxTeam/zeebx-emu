# 04 — Tempo, temporizadores e callbacks

## O relógio é virtual

```
agora_us = clock_us + instruções_executadas / 528
```

528 é o clock do ARM11 do MSM7201A em MHz. O tempo do jogo, então, **anda com o trabalho que ele
faz** — não com o relógio do host. Isso torna a execução repetível: o mesmo jogo, com a mesma
entrada, produz a mesma sequência de quadros, esteja o host ocupado ou não.

`clock_us` é a parcela que **adiantamos**, e ela existe por dois motivos.

## Adiantar o tempo ocioso

`skip_idle_time` — quando não há callback, thread, blit nem sinal pendente, o relógio salta para
o vencimento do próximo temporizador. É o que o console faz: sem trabalho pendente ele dorme e
acorda no prazo. Emular o sono instrução por instrução seria emular o nada.

## Pular a espera ocupada

Alguns jogos não armam temporizador para o próximo quadro: ficam num laço lendo o relógio e
cedendo a vez até o prazo chegar. O Zeebo Sports Peteca faz **69 mil leituras por quadro**,
14 milhões de instruções gastas só em esperar. No console isso não custa nada, porque o tempo
passa sozinho enquanto o ARM gira; aqui cada volta é emulada de verdade, e o jogo rodava seis
vezes mais devagar que o aparelho.

`note_spin` reconhece o padrão. Ele conta chamadas consecutivas que são **só** consulta de
relógio (`aee_GetTimeMS`, `aee_GetUpTimeMS`, `aee_GetSeconds`) ou cessão da vez
(`IThread::Suspend`, `IShell::Resume`). Qualquer outra chamada é sinal de trabalho e **zera a
contagem**. Passadas 64 dessas, cada consulta seguinte adianta o relógio em 250 µs.

O critério importa: um quadro que lê o relógio no meio do que faz continua custando o que custa.
Só quem *não está progredindo* ganha tempo de graça.

Resultado no Peteca: 101 s reais → 12,4 s, para os mesmos 428 quadros de 17 segundos virtuais.

## Dormir

`MSLEEP` — o `sleep(msecs)` da stdlib — **adianta o relógio**. No console o ARM realmente para e o
tempo passa sozinho; aqui o relógio anda com as instruções, então quem dorme sem que o relógio
avance dorme para sempre. O Magical Drop 3 chamava `sleep` cem milhões de vezes num laço que
nunca vencia.

Há um teto: um valor absurdo, pedido por engano, não pode levar o relógio junto.

## Temporizadores

`ISHELL_SetTimer` guarda `(prazo, callback)`. A cada volta, `advance` recolhe os vencidos e
chama cada um.

O detalhe que não é óbvio: os vencidos saem da lista **antes** de rodar. O callback tipicamente
rearma o timer, e o rearmado não pode disparar já no mesmo quadro — seria um laço que nunca
devolve o controle.

## Reentrar no guest

Vários pontos precisam chamar uma função **do jogo** de dentro de uma chamada de API: um
callback de timer, um comparador de `qsort`, o `PFNIMAGEINFO` de uma imagem pronta.

Isso funciona porque o despacho acontece **fora** do `emu_start` — o núcleo já parou quando o
atendemos. Reentrar é seguro desde que:

- os registradores vivos sejam salvos e restaurados (`SAVED_REGS`);
- haja um teto de aninhamento (`MAX_NESTING`), senão um ciclo prenderia o emulador;
- a reentrada aconteça numa **fronteira de chamada**, nunca no meio de uma.

`run_pending_callbacks` roda na fronteira entre duas chamadas de API — o único ponto em que dá
para entrar no guest sem interromper nada pela metade. `CALLBACK_ROUNDS` limita quantas rodadas
entregar, porque um callback pode enfileirar outro.

## Threads cooperativas

`IThread` do BREW não tem preempção: o guest só perde o controle quando chama `Suspend`. É
exatamente aí que salvamos os registradores, e retomar é restaurá-los e continuar de onde o
`Suspend` voltaria. É a razão de isto ser viável num emulador de um núcleo só.

## `qsort`

A stdlib do BREW tem `qsort`, e o comparador é código do jogo. Ordenamos **na memória do guest**,
trocando os elementos de lugar de verdade, para que os ponteiros que o comparador recebe sejam os
endereços reais dentro do vetor — que é o que o `qsort` do C faz e o que um comparador pode
observar.

O algoritmo é heapsort: ordena no lugar e faz `n log n` comparações. Como **cada comparação custa
uma entrada no guest**, o número delas é o que importa; uma ordenação por inserção seria simples
e quadrática, e um vetor grande custaria caro.
