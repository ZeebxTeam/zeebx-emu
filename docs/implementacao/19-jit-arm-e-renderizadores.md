# 19 — JIT ARM e renderizadores 3D

## O sintoma

Alguns jogos 3D permaneciam entre 10 e 15 FPS mesmo quando o trabalho do host era pequeno. O
caso que revelou a causa foi a abertura de Kingdom Hearts: ela carrega `swv21brew.mod`, uma
extensão Superscape que implementa o próprio rasterizador RGB565/depth em ARM. Não passa pelo
nosso EGL/OpenGL ES; cada pixel da cena é um laço de instruções do guest.

O perfil da abertura mostrou que o problema não era o laço BREW:

| Intervalo | Timers/voltas | Instruções ARM |
|---|---:|---:|
| ~2 s virtuais | 50 timers, 150 voltas | ~25 milhões |
| ~8 s virtuais | 495 voltas | ~762 milhões |

Timers, sinais e callbacks não tinham volume para explicar a queda. A CPU era executada no TCG
do Unicorn, que preserva bem a compatibilidade mas não era rápido o bastante para esse
rasterizador escrito em ARM.

## O que não foi alterado

Trocar desempenho por alterar o tempo do jogo esconderia o defeito e quebraria física, animação
e input. Portanto o JIT usa o mesmo `Machine` e a mesma sequência da sessão:

```text
advance → timers vencidos → callback do timer → sinais → callbacks pendentes
```

O relógio virtual também permanece igual:

```text
utime = clock_us + instruções / 528
```

`528` é a frequência, em MHz, do ARM11 do MSM7201A. O salto de ociosidade continua limitado a
um vblank; ele só representa tempo em que o console estaria esperando. O JIT reduz tempo de
parede do host, não cria tempo virtual nem dispara callbacks extras.

## Backend Dynarmic

`cpu/dynarmic.rs` implementa o mesmo `CpuBackend` do Unicorn, agora com Dynarmic A32 ARMv6K.
Ele é o backend da sessão gráfica normal; `zeebx bench arquivo.zip --seconds=N` mantém uma
bancada sem janela para medir a ROM inteira.

O contrato com o restante do emulador é preservado:

- registradores, memória e contador de instruções continuam acessados pelo `CpuBackend`;
- as vtables BREW (`0xf0000000..0xf0ffffff`) e o retorno-sentinela continuam endereços sem
  código; o fetch do JIT para ali para que o despachante Rust atenda a API;
- a parada na vtable não conta como instrução ARM executada e o PC volta a ficar no endereço que
  o Unicorn exporia;
- o CPSR começa em modo usuário (`0x10`), como no applet do Zeebo;
- `SVC #0xAB` atende `SYS_WRITEC` e `SYS_WRITE0`, mantendo o semihosting de Peggle e Zuma.

## Coerência de código e de superfícies

O JIT pode reutilizar um bloco compilado. Quando o guest ou uma API host escreve uma página que
já foi buscada como código, o bloco precisa ser invalidado antes da próxima entrada. O backend
rastreia somente páginas executadas e invalida apenas essas páginas; invalidar por qualquer
escrita seria desastroso, pois um renderizador RGB565 escreve milhões de pixels por quadro.

Os buffers de bitmap/EGL continuam no espaço de memória do guest. O contrato de `watch_dirty` é
conservador no Dynarmic enquanto não há um hook de sujeira específico: uma sincronização pode
copiar mais dados, mas nunca deixa de importar um pixel alterado. Esse detalhe favorece correção
gráfica, e não é o gargalo do renderizador ARM do Kingdom Hearts.

## Validação feita

O primeiro quadro de Kingdom Hearts em 2 s virtuais foi gerado pelos dois núcleos e os BMPs têm
o mesmo SHA-256:

```text
d5be7b5c0589e5d8af8bb2d0d8652d745449262d65b34675133384136566b4bc
```

Na mesma máquina de desenvolvimento, a bancada Dynarmic executou:

| Carga | Tempo virtual | Tempo real | Velocidade |
|---|---:|---:|---:|
| Kingdom Hearts | 2,001 s | 0,305–0,331 s | 604–657% |
| Kingdom Hearts | 8,004 s | 3,555 s | 225% |

No trecho de 8 s foram cerca de 762 milhões de instruções, a 214,5 MIPS. A folga acima de 100%
é intencional: a sessão aplica o limitador de velocidade contra o relógio real, como fazia antes.

Além dos testes de unidade de CPU (execução, parada em API, retorno e semihosting), a suíte
completa deve passar com:

```bash
cargo test --release
```

## Como investigar uma regressão visual

1. Reproduzir no mesmo instante virtual, de preferência com `--keys` e dump de BMP.
2. Comparar Unicorn e Dynarmic, não apenas FPS: hash idêntico prova o estado de pixels daquele
   instante; hash diferente pede captura do PC/rotina que escreve a superfície.
3. Se a diferença aparece só após trocar de menu, conferir primeiro invalidação de código em
   RAM e leitura de memória atualizada pelo host; não compensar mexendo em timers ou `utime`.
4. Manter a equivalência antes de otimizar a sincronização de superfícies.

O próximo trabalho é ampliar esses testes para roteiros interativos de CNK3D, Tekken e Kingdom
Hearts, especialmente menus e telas de opção, onde código e tabelas mutáveis são mais comuns.
