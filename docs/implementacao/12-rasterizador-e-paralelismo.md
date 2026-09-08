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

## O teto que sobra

Vale ter claro para não esperar do rasterizador o que ele não pode dar: **mesmo de graça**, o
Quake ficaria em ~52%. Os 48 s de emulação mais despacho para 25 s virtuais já são o dobro do
relógio. São 3,4 bilhões de instruções de guest por 25 segundos virtuais e o núcleo faz cerca de
110 milhões por segundo.

Os dois próximos gargalos, em ordem:

1. **O custo por chamada de API**, hoje de ~7 µs, porque toda chamada é um `emu_stop` seguido de
   `emu_start` do unicorn. O Quake faz 2,5 milhões delas em 25 segundos virtuais; o Heavy Weapon
   faz 6 milhões para desenhar **um** quadro. Atender dentro de um hook, sem parar a emulação,
   é redesenho do trampolim — e é o que sobra de maior.
2. **O núcleo em si.** A 110 MIPS, um jogo que use um quarto da capacidade do ARM11 do console
   já consome 80% do nosso relógio só para executar instrução.

## Números de calibração

Faixas, medido no Alpine Racer numa máquina de 24 núcleos: 4 → 10,89 s, 6 → 8,64 s, 12 → 5,59 s,
16 → 5,74 s. Fica uma faixa por núcleo com piso de 40 linhas por faixa (`MIN_BAND_ROWS`): mais
fina que isso, quase todo triângulo cruza fronteira e o preparo por faixa come o ganho.

O limiar de custo (`PARALLEL_COST`, 64 mil fragmentos) foi calibrado quando a divisão era por
draw call, e ali era indiferente entre 16 mil e 256 mil. Com o quadro acumulado ele quase nunca
segura nada — sobrou como proteção para o caso do quadro minúsculo, em que acordar thread ainda
não compensa.

## A lição, curta

Paralelizar não é escolher entre serial e paralelo: é escolher **o tamanho do pedaço**. O mesmo
código, com o mesmo número de threads e o mesmo limiar, ficou entre "3× mais rápido" e "não faz
nada" dependendo só de onde o corte foi feito. E a única forma de descobrir isso foi contar
quantas vezes cada caminho era tomado — o código parecia certo nas duas versões.
