# 12 — Rasterizador: a granularidade do paralelismo

Este documento existe porque a primeira versão do rasterizador paralelo **funcionava e não
servia** para metade dos jogos, e o motivo não é óbvio olhando o código. É sobre onde cortar o
trabalho, não sobre como preencher um triângulo.

## O erro: paralelizar por draw call

A primeira versão dividia o quadro em faixas horizontais **a cada draw call**. Uma faixa por
núcleo, cada uma percorrendo os triângulos daquela chamada. Funcionou bem:

| Jogo | antes | depois |
|---|---:|---:|
| Alpine Racer (15 s virtuais) | 16,92 s | 5,67 s |
| Crash Nitro Kart | 6,15 s | 3,97 s |

E não fez **nada** pelo Quake, que continuou em 28% da velocidade do console.

A medição que explicou:

```
FILL serial=198000 paralelo=2000 custo-medio=5104
```

**99% dos preenchimentos do Quake iam em série.** O custo médio de uma draw call dele é de 5.104
fragmentos — cerca de 15 µs de trabalho. Acordar oito threads custa entre 200 e 400 µs. O limiar
de 64 mil fragmentos, que existia justamente para não pagar esse preço à toa, estava certo: não
havia como paralelizar aquilo.

## Duas formas de desenhar um quadro

O que separa os jogos não é quanto desenham, é **em quantos pedaços**. Cronometrando cada draw
call:

```
Alpine Racer:  >1ms: 22518ms/1840x   200µs-1ms: 441ms/712x   o resto: 0ms/13x
Quake:         556.411 draw calls em 25 segundos virtuais
```

O Alpine concentra 98% do tempo em 1.840 chamadas de ~12 ms cada — cada uma é trabalho de sobra
para oito threads. O Quake espalha o mesmo tipo de cena por meio milhão de chamadas de dois
triângulos. Os dois desenham a mesma quantidade de pixels; só um deles dá para dividir por draw
call.

Isto não é uma esquisitice do Quake. É o que qualquer motor que ordena por textura faz: uma
chamada por material, por sprite, por pedaço de cenário.

## O conserto: acumular o quadro

O desenho passou a ser **acumulado** e pintado uma vez só, no fim do quadro. Duzentos mil
preenchimentos pequenos e serieis viram um grande e paralelo.

A draw call agora tem duas fases:

- **`prepare`** projeta o triângulo, descarta face, calcula a caixa envolvente e os atributos
  divididos por `w`. Tudo que não depende do pixel.
- **`enqueue`** guarda os triângulos na fila e anota, junto, o estado do OpenGL que valia
  naquele momento — porque ele muda entre uma chamada e a seguinte.
- **`flush`** pinta a fila inteira, dividindo a tela em faixas.

```rust
struct Job {
    first: usize,          // faixa dele na fila de triângulos
    last: usize,
    texture: Option<u32>,  // o nome, não a referência
    texture_env: u32,
    depth_test: bool, depth_mask: bool, depth_func: u32,
    blend: bool, blend_src: u32, blend_dst: u32,
    alpha_test: bool, alpha_func: u32, alpha_ref: f32,
}
```

Três decisões que valem registro:

**Uma fila de triângulos, não uma por draw call.** Cada `Job` guarda só os índices da faixa
dele. Um `Vec` por draw call seriam duzentas mil alocações por quadro — trocar um gargalo por
outro.

**A textura fica pelo nome, não por referência.** Entre o `enqueue` e o `flush` o jogo liga
outras texturas, e o que vale é a que estava ligada quando aquele lote foi montado. Guardar
`&Texture` também tornaria impossível emprestar o mapa de texturas para as threads enquanto o
quadro é escrito.

**Os dois vetores vivem entre quadros.** Devolver e repedir a mesma memória sessenta vezes por
segundo não tem por que acontecer.

## A ordem é o que garante o resultado

Cada faixa é um pedaço **exclusivo** do quadro — `chunks_mut` sobre o `color` e o `depth` — e
percorre a fila inteira na ordem em que o jogo desenhou. Como duas faixas nunca tocam o mesmo
pixel, e dentro de uma faixa a ordem é a original, a transparência empilha na mesma sequência.

O resultado não é "parecido": é **igual**. Verificado comparando o quadro do Quake antes e
depois, byte a byte — 921.600 bytes, zero diferenças. O mesmo já valia para a versão por draw
call, conferida em 300 quadros do Alpine Racer.

## Quando a fila precisa ser despejada

A regra: **antes de qualquer coisa que leia o quadro ou mude algo que a fila mencione.**

| Ponto | Por quê |
|---|---|
| `present` | é a entrega do quadro; quem pede o resultado não tem por que saber que o desenho é acumulado |
| `clear` | limpar é uma operação sobre o quadro e entra na mesma ordem: apagaria o que ainda nem foi pintado |
| `glTexImage2D` e `glCompressedTexImage2D` | trocar o conteúdo de uma textura que a fila ainda vai ler mudaria o passado |
| `glDeleteTextures` | idem, pelo mesmo motivo |
| `set_surface` | o que foi desenhado no tamanho antigo precisa virar pixel antes da troca |

O `present` despeja sozinho, e é por isso que ele toma `&mut self`. Foi uma mudança de
assinatura de propósito: com `&self` era possível ler um quadro pela metade, e o compilador não
tinha como avisar.

## Quanto rendeu

Quake, 25 segundos virtuais, medido três vezes:

| | tempo real | velocidade |
|---|---:|---:|
| antes | 89,8 s | 28% |
| reaproveitando o buffer de vértices | 81,0 s | 31% |
| acumulando o quadro | 58,5 s | 43% |

Onde o tempo ia, antes (medido desligando cada etapa):

| | tempo | fatia |
|---|---:|---:|
| preenchimento de pixels | 37,1 s | 38% |
| geometria (transformar e recortar) | 13,5 s | 14% |
| emulação do ARM + despacho de API | 48,3 s | 49% |

Duas correções depois desta medida:

- **A geometria caiu de 14% para ~4%.** O `read_attribute` pedia um `read_u32` por componente,
  e cada pedido atravessa a FFI do unicorn: 57 ns para trazer quatro bytes, contra 0,3 ns
  quando vêm de um `read_mem` de um quilobyte. Dos 3,6 s que as draw calls custavam em 15
  segundos virtuais, 3,2 s eram travessia e 400 ms eram desenho. Hoje é um bloco por array por
  draw call.
- **A tabela acima foi tirada com o `--profile`, que custa 24%** — e o preço cai quase todo na
  fatia do ARM, porque o perfil de blocos faz uma inserção de tabela por bloco de tradução. A
  proporção entre as três fatias serve; o relógio absoluto, não. Meça tempo sem ele.

## Descobrir que nada mudou custava mais que desenhar

O jogo pode escrever direto na superfície que o EGL expõe — é assim que a Z-Wheel compõe o 2D
sobre o palco 3D. Para não perder essas escritas, o `sync_egl_color_from_guest` lia a superfície
inteira de volta do guest e a comparava byte a byte com a nossa cópia. Ele é chamado em **todo**
`Draw*`, `Clear` e `ReadPixels`.

Medido na Z-Wheel, treze segundos virtuais: **93.750 chamadas**, cada uma lendo 400 KB e
comparando 400 KB — perto de 37 GB de tráfego só para descobrir que quase nunca havia mudança.
Eram 3.324 ms de leitura e 2.732 ms de comparação, e o custo aparecia onde ninguém procuraria: no
`glDrawElements`, com 2.883 ms.

O conserto é um **watchpoint de escrita** (`CpuBackend::watch_dirty`/`take_dirty`): o hook do
unicorn liga um `bool` quando o guest escreve na faixa, e o `sync` sai em O(1) enquanto ele estiver
limpo. Depois: leitura 105 ms, comparação 39 ms, `glDrawElements` **275 ms** — dez vezes menos —, e
o total de API caiu de 16.345 ms para algo entre 12.550 e 13.500 ms.

Duas coisas que essa medida ensinou, e que valem além deste caso:

- **A escrita do host não passa pelos hooks do unicorn.** As implementações de API escrevem direto
  na memória do guest, e é por elas que o 2D chega à superfície. A primeira versão do sinalizador
  ignorava isso: o custo sumiu e as capas da roda pararam de aparecer, com a diferença confinada à
  faixa do cilindro. Quem arma um watchpoint precisa marcá-lo também no `write_mem` do próprio
  emulador — o watchpoint de depuração já fazia isso, e foi de lá que veio a pista.
- **Uma execução só não mede nada aqui.** Três execuções do mesmo binário deram 13.903, 14.221 e
  15.845 ms de API: 14% de faixa. Só delta grande conta — e é por isso que a queda do
  `glDrawElements` serve como prova, enquanto o caminho rápido da importação (que é algoritmicamente
  melhor e pixel a pixel idêntico) fica sem número: o efeito dele não sai do ruído.

## O teto que sobra

Vale ter claro para não esperar do rasterizador o que ele não pode dar: **mesmo de graça**, o
Quake ficaria em ~52%. Os 48 s de emulação mais despacho para 25 s virtuais já são o dobro do
relógio. São 3,4 bilhões de instruções de guest por 25 segundos virtuais, e o núcleo entrega 218 milhões por segundo num laço
apertado que não toca memória — mas cerca de 86 milhões no jogo de verdade, onde há tráfego de
memória pela softmmu e uma ida e volta do `emu_start` por chamada de API.

Os dois próximos gargalos, em ordem:

1. **O custo por chamada de API**, medido em 1,4 µs de ida e volta, porque toda chamada é um
   `emu_stop` seguido de um `emu_start` do unicorn. O Quake faz 2,5 milhões delas em 25 segundos
   virtuais. Atender dentro de um hook, sem parar a emulação, é redesenho do trampolim — e é o
   que sobra de maior **do mecanismo**. Antes dele vem o que cada método faz por dentro: o
   perfil de API do `--profile` mede isso, e nas três vezes em que um jogo pareceu preso no
   despacho a causa estava lá, não no trampolim.
2. **O núcleo em si.** Aos ~86 MIPS efetivos, um jogo que use um quarto da capacidade do ARM11
   do console já consome boa parte do nosso relógio só para executar instrução. É aqui que um
   backend sobre `dynarmic` entraria — o `CpuBackend` existe para isso.

## Números de calibração

Faixas, medido no Alpine Racer numa máquina de 24 núcleos: 4 → 10,89 s, 6 → 8,64 s, 12 → 5,59 s,
16 → 5,74 s. Fica uma faixa por núcleo, com um piso de linhas por faixa (`MIN_BAND_ROWS`).

**O piso era 40 linhas, e essa foi a parte errada da calibração.** Ele não é afinação: é o teto
real de paralelismo quando a superfície é baixa. O palco da Z-Wheel tem 640x330, e `330 / 40` dá
oito faixas — numa máquina de 24 núcleos, dois terços dela ficavam paradas durante todo o
preenchimento. O número saiu de medir só cenas de 480 linhas, onde `480 / 40 = 12` já era perto
do que a máquina daria, e o defeito ficou invisível justamente por isso.

Baixando para 8 linhas, tempo de `flush` na Z-Wheel em treze segundos virtuais: **2474 ms com 40,
1938 com 16, 1868 com 8**. E na pista do Crash, em trinta segundos virtuais, o total de API foi de
**4637 para 3842 e 3622 ms** nos mesmos cortes. O receio de que faixas finas custassem caro em 480
linhas não se confirmou, e o motivo é aritmético: ali `480 / 16` e `480 / 8` esbarram no número de
núcleos antes de esbarrar nesta constante, então os dois cortes descrevem a mesma divisão.

Dividir mais fino não muda um pixel — as nove superfícies despejadas da Z-Wheel saem byte a byte
iguais —, porque cada faixa continua sendo região exclusiva e percorre a fila na ordem original.

O limiar de custo (`PARALLEL_COST`, 64 mil fragmentos) foi calibrado quando a divisão era por
draw call, e ali era indiferente entre 16 mil e 256 mil. Com o quadro acumulado ele quase nunca
segura nada — sobrou como proteção para o caso do quadro minúsculo, em que acordar thread ainda
não compensa.

## A lição, curta

Paralelizar não é escolher entre serial e paralelo: é escolher **o tamanho do pedaço**. O mesmo
código, com o mesmo número de threads e o mesmo limiar, ficou entre "3× mais rápido" e "não faz
nada" dependendo só de onde o corte foi feito. E a única forma de descobrir isso foi contar
quantas vezes cada caminho era tomado — o código parecia certo nas duas versões.
