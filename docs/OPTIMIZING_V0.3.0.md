# Otimização do Zeebx — linha de partida da 0.3.0

Este documento existe por um motivo simples: **os números de desempenho que o projeto publica
não descrevem mais o emulador que está na árvore.** Eles foram medidos com o Unicorn, e o
Unicorn saiu. Antes de otimizar qualquer coisa é preciso voltar a medir, e antes de medir é
preciso dizer em voz alta o que se sabe, o que se supõe e o que se vai conferir.

O alvo desta rodada não é o desktop. É o portátil: **R36S** (Cortex-A35, ArkOS) e **RG40XX-H**
(Cortex-A53, muOS), pelo core Libretro. O desktop entra como bancada de medida, porque nele dá
para instrumentar sem cartão SD no meio.

## 1. O que está desatualizado

O `ARCHITECTURE.md` traz, na seção "Limites conhecidos", uma tabela de custos que vinha do
backend antigo:

| número publicado | onde | estado |
|---|---|---|
| 52% da velocidade do console no Quake | `ARCHITECTURE.md:243` | medido com Unicorn |
| ~68% ARM + despacho, ~28% pixels, ~4% geometria | `ARCHITECTURE.md:243` | medido com Unicorn |
| 1,4 µs por chamada de API | `ARCHITECTURE.md:264` | medido com Unicorn |
| 57 ns por `read_u32` contra 0,3 ns em bloco | `ARCHITECTURE.md:252` | texto diz "FFI do unicorn" |
| 218 M instr/s no laço, ~86 M no jogo | `ARCHITECTURE.md:257` | medido com Unicorn |
| `cargo test --release instrucoes_por_segundo` | `ARCHITECTURE.md:258` | **o teste não existe mais** |

As datas fecham o caso:

```
8af069f  2026-09-11  Atualiza os números de performance medidos
a359183  2026-09-23  Remove o Unicorn e deixa o Dynarmic como backend padrão
```

A medida é doze dias mais velha que a remoção do backend que ela mediu. O documento também
descreve a árvore com `cpu/unicorn.rs` e um `CpuBackend` "sobre o unicorn"
(`ARCHITECTURE.md:59`, `:83`), arquivo que não está mais lá.

**Nada disso quer dizer que as conclusões estavam erradas.** Quer dizer que elas precisam ser
refeitas antes de servirem de base para decisão de otimização.

## 2. O que continua verdadeiro na árvore de hoje

Estes pontos foram conferidos no código, não no documento:

- **O despacho de API ainda para o JIT.** A callback chama `halt(HaltReason::UserDefined1)`
  (`src/cpu/dynarmic.rs:126`), o método é atendido no host, e o laço limpa a razão e volta a
  entrar com `jit.run()` (`src/cpu/dynarmic.rs:587`). A ida e volta por chamada continua
  existindo com o Dynarmic; o que mudou foi o custo dela, que ninguém mediu ainda.
- **A tabela de páginas já é um fastmem parcial.** `config.page_table(...)` com
  `page_table_mask(0)` (`src/cpu/dynarmic.rs:450`), e as entradas nulas caem nas callbacks:
  regiões só de leitura, páginas já executadas, páginas com vigia de escrita e a última página
  de uma região que não a completa (`src/cpu/dynarmic.rs:314`).
- **Há um número recente e útil no próprio código:** sem a tabela de páginas, o Need for Speed
  ficava em 176 milhões de instruções por segundo, 69% da velocidade do console
  (`src/cpu/dynarmic.rs:317`). Esse é um número da era da tabela, não do Unicorn.
- **O rasterizador já é paralelo.** Ele divide o quadro em faixas e usa `std::thread::scope`
  (`src/video/rasterizer.rs:2341`). Quem propuser "paralelizar o rasterizador" está propondo o
  que já existe.
- **O perfil de custo por API existe e é opt-in.** `Machine::enable_api_profile()`
  (`src/machine/diagnostico.rs:87`) e `Machine::api_profile()` (`:92`), ligados pela varredura
  com `ZEEBX_ROM_PERFIL`.
- **A varredura já mede desempenho.** `Desempenho` (`src/varredura.rs:427`) traz `instrucoes`,
  `chamadas`, `quadros`, `pixels`, `voltas`, `virtual_ms`, `laco` e `velocidade()` em por cento
  da velocidade do console.
- **O perfil de release já está apertado.** `lto = "thin"` e `codegen-units = 1`
  (`Cargo.toml:171`). Quem sugerir "ativar LTO" está sugerindo o que já está ligado; o que
  sobra é comparar `thin` com `fat`.

## 3. O que o core Libretro já oferece ao portátil

Treze opções, das quais estas mexem diretamente em custo:

| opção | efeito | vale quando |
|---|---|---|
| `zeebx_frameskip` | pula desenho, sem mexer no relógio | a rasterização domina |
| `zeebx_limite_fps` | freia apresentação em 60/30 | o aparelho corre solto e esquenta |
| `zeebx_resolucao_interna` | supersampling **para cima**, 1..4 | sobra GPU |
| `zeebx_antialias` | amostras por pixel na placa | sobra GPU |
| `zeebx_rasterizador` | força software quando o driver mente | a placa desenha errado |
| `zeebx_perfil` | Portátil: taxa, vozes e cache menores | o aparelho é fraco |
| `zeebx_soundfont_taxa`, `zeebx_midi_vozes`, `zeebx_cache_de_som_mb` | custo do MIDI | a música é pesada |

Repare no que **não** existe: uma resolução interna **abaixo** de 1. Hoje `escala` é um
multiplicador inteiro (`src/video/gpu.rs:207`, `:727`, `:2270`) e só vale no rasterizador de
placa. Para `0,5x` e `0,25x` seria preciso fração, e o ganho maior provavelmente está do lado
do rasterizador de software, onde cada pixel é CPU.

## 4. As hipóteses que vão ser testadas

Nenhuma delas está decidida. São hipóteses, e a medida decide.

1. **O gargalo do portátil é o ARM + HLE, não o preenchimento.** Se for verdade, `0,5x`,
   GLES 2 e frameskip não resolvem; o que resolve é trampolim e leitura em bloco.
2. **O custo por chamada de API caiu com o Dynarmic.** O `1,4 µs` é do Unicorn. Pode ter caído
   muito, pode ter caído pouco. Enquanto não se medir, a prioridade 1 é uma aposta.
3. **A leitura de página só-de-leitura ainda passa por callback.** Se ela pesar, estender o
   fastmem para leitura é barato e rende.
4. **A síntese do SoundFont trava a primeira música.** O custo é de carga, não de quadro, mas
   um engasgo de centenas de ms é sentido no portátil.
5. **`0,5x` no rasterizador de software vale mais que `0,5x` na placa.** O preenchimento por
   CPU escala com área; o Mali-G31 a 640x480 provavelmente não está saturado.
6. **NEON rende no host, não no guest.** O código ARM do jogo é do guest e passa pelo JIT; o
   que dá para vetorizar é o que o host faz: preencher, converter, copiar e limpar.

## 5. O que **não** vai ser feito nesta rodada

Dito agora para não virar discussão depois:

- **Não** se vai copiar o SH4ZAM. Ele é assembly SH-4 do Dreamcast, e o host aqui é ARM64 ou
  x86-64. A ideia aproveitável dele é "rotina especializada por arquitetura do host", e isso
  se escreve com NEON, não com FSCA e XMTRX.
- **Não** se vai baixar o requisito para GLES 2 por desempenho. GLES 2 é assunto de
  **compatibilidade** — Android antigo, driver limitado —, e mudaria VAO, `blit_framebuffer`,
  multisample e versão de shader (`src/video/gpu.rs:284`, `:533`, `:419`, `:1493`).
- **Não** se vai fazer backend GLES 1. Seria um segundo rasterizador de pipeline fixo.
- **Não** se vai criar thread de saída de áudio no core. Em Libretro quem puxa o áudio é o
  frontend; o core que cria thread de saída está brigando com o contrato.
- **Não** se vai ajustar mais timbre nem ganho do MIDI. Isso já foi calibrado por medição.

## 6. Como medir

### 6.1 Na bancada (desktop)

A varredura já faz o trabalho e não precisa de janela:

```sh
ZEEBX_ROM="<jogo>.zip" \
ZEEBX_ROM_PERFIL=1 \
ZEEBX_ROM_MS=15000 \
ZEEBX_ROM_SAIDA=/tmp/perf \
cargo test --release varredura -- --nocapture
```

O que sai dali: velocidade em por cento do console, instruções, chamadas, quadros, pixels e a
lista de métodos de API mais caros, com total.

Regras de medida, herdadas do erro documentado em `8af069f`:

- **medir tempo sem o perfil ligado.** O perfil de blocos custou 24% na medida antiga, e o
  preço cai quase todo na fatia do ARM;
- usar o mesmo `ZEEBX_ROM_MS` em todas as comparações;
- rodar sempre em `--release`;
- comparar proporção com perfil ligado, relógio com ele desligado.

**Cuidado com o que o perfil de API mede.** O `api_time` só soma o tempo de
`dispatch_inner` (`src/machine/mod.rs:3370`), isto é, **o corpo do método**. Ele não inclui o
`halt` do JIT nem a reentrada por `jit.run()`. O custo do trampolim, que é justamente o número
de `1,4 µs` que se quer refazer, **não sai daqui**: precisa de um micro-teste próprio — um laço
do guest chamando um método barato, comparado com o mesmo laço sem a chamada. Confundir os dois
é o erro fácil desta rodada.

### 6.2 Jogos da rodada

| jogo | por que ele |
|---|---|
| Quake | pior caso histórico, meio milhão de draw calls |
| Crash Bandicoot Nitro Kart 3D | usa `glReadPixels`, desliga frameskip |
| Need For Speed Carbon | 3D com muita geometria; deu o número do fastmem |
| Zeebo Extreme Rolimã | já mostrou `memset` como custo |
| Double Dragon | 2D, e é o que se joga nos testes de aparelho |

### 6.3 No aparelho

O que falta é telemetria por quadro no core. O que se quer contar, por segundo:

```
tempo do guest (ARM)
tempo em gles_draw
número de draws e de vértices
número de glReadPixels
tempo em memset/cópia
tempo de conversão do quadro
tempo de áudio
quadros pulados
```

Enquanto isso não existe, o aparelho só responde "quantos FPS", e isso não diz onde o tempo
foi.

## 7. A ordem de ataque

A ordem vale **depois** da medida, e muda se a medida contrariar.

| # | frente | por que agora | risco |
|---|---|---|---|
| 1 | Re-medir e corrigir `ARCHITECTURE.md` | o documento está guiando decisão com número velho | nenhum |
| 2 | Trampolim de API | é o custo fixo por chamada, e o doc já o aponta | alto: mexe no núcleo |
| 3 | Leitura em bloco e fastmem de leitura | a lição "pedir o bloco, nunca o elemento" já é do projeto | médio |
| 4 | NEON no host: preencher, converter, copiar, limpar | escala com pixel e com byte | médio |
| 5 | `0,5x`/`0,25x`, primeiro no software | corta área, e área é o custo do preenchimento por CPU | médio: viewport, tesoura, leitura |
| 6 | Síntese de SoundFont sem travar a carga | tira engasgo sentido no portátil | baixo |
| 7 | `lto = "fat"` medido contra `thin` | é barato de testar | baixo |

## 8. Critério de aceitação

Uma otimização entra quando:

1. há medida **antes e depois**, no mesmo jogo, com o mesmo `ZEEBX_ROM_MS`;
2. a varredura dos 62 jogos não regride comportamento — desempenho fica fora da comparação de
   linha de base de propósito, e é por isso que o número vive aqui e não lá;
3. o ganho aparece também no portátil, ou fica dito que é ganho só de desktop;
4. `cargo test`, `cargo clippy` e o core Libretro continuam verdes;
5. o documento de arquitetura é atualizado junto, com o número novo e a data.


## 10. Referência externa

Uma sessão de consulta com o DeepSeek sobre otimização de emuladores levantou pontos que valem
como leitura, e também mostra onde uma análise de fora erra:

https://chat.deepseek.com/share/yjigu5xc76ahsri24s

O que ela acertou: o trampolim de API é o gargalo estrutural; o rasterizador não é o gargalo; a
lição de pedir bloco em vez de elemento; estender o fastmem para leitura.

O que ela errou: propôs ativar `lto` e `codegen-units = 1`, que já estão ligados
(`Cargo.toml:171`), inferindo o arquivo pelo tamanho em bytes sem abri-lo; propôs paralelizar o
rasterizador, que já é paralelo (`src/video/rasterizer.rs:2341`); propôs thread dedicada de
áudio, que é contra o contrato do Libretro; e tratou os números do `ARCHITECTURE.md` como
atuais, quando são da era do Unicorn — que é exatamente o motivo deste documento existir.

## 11. Registro das medidas

| data | jogo | backend | velocidade | instr/s | chamadas de API | observação |
|---|---|---|---|---|---|---|
| 2026-09-11 | Quake | Unicorn | 52% | 86 M | — | `ARCHITECTURE.md:243`, cena de jogo |
| 2026-09-24 | Quake | Dynarmic | **170%** | **195 M** | 1.323.900 | 15.016 ms virtuais em 8,8 s reais, 583 quadros, rasterizador de software |

### A bancada

Número de desempenho sem a máquina que o produziu não vale nada. Todas as medidas de bancada
deste documento saem daqui, até que se diga o contrário:

| item | valor |
|---|---|
| aparelho | Lenovo IdeaPad 3 |
| processador | AMD Ryzen 7 5700U (Zen 2, 8 núcleos / 16 linhas, até 4,37 GHz) |
| gráficos | Radeon integrado (Vega), Mesa |
| memória | 9 GiB |
| sistema | Debian GNU/Linux 13 (trixie), núcleo 6.19.5-x64v3-xanmod1 |
| compilador | rustc 1.98.1 |
| perfil | `--release`, `lto = "thin"`, `codegen-units = 1` |

Isto é um portátil de escritório, não uma bancada de medição: o relógio varia com temperatura e
com o governador, e a medida do mesmo trabalho oscila entre execuções. Vale para comparar
**antes e depois no mesmo aparelho**, na mesma sessão, e não para publicar número absoluto.

E a diferença para o alvo é enorme, o que é justamente o ponto: o Ryzen 5700U tem 16 linhas de
execução e mais de 4 GHz, enquanto o R36S tem um Cortex-A35 e o RG40XX-H um Cortex-A53. Os 170%
do Quake **não** se transportam para o portátil, e qualquer conclusão sobre gargalo de aparelho
precisa ser medida no aparelho.

**A primeira medida com o Dynarmic desmente a leitura pessimista.** O Quake saiu de 52% para
170% da velocidade do console, e o núcleo de 86 M para 195 M de instruções por segundo — 2,3
vezes. O emulador que o `ARCHITECTURE.md` descreve não é o que está na árvore.

Ressalva honesta, para a medida não virar propaganda: **a cena não é a mesma.** A medida antiga
descrevia a cena de jogo com meio milhão de draw calls; esta rodou 15 segundos a partir da
abertura, com 278.297 `DrawArrays`. O que é diretamente comparável é o ritmo do núcleo
(instruções por segundo), não o relógio de parede de cenas diferentes. Para comparar relógio
será preciso um roteiro de teclas (`ZEEBX_ROM_TECLAS`) que chegue ao mesmo ponto do jogo.

O perfil de chamadas do mesmo relatório mostra onde a conversa com o guest se concentra:

| chamadas | método |
|---:|---|
| 283.982 | `IGLES11::VertexPointer` |
| 282.717 | `IGLES11::TexCoordPointer` |
| 278.297 | `IGLES11::DrawArrays` |
| 137.720 | `AEEHelpers::strcmp` |
| 61.143 | `AEEHelpers::aee_GetRand` |

As três primeiras somam 844.996 das 1.323.900 chamadas — **64% de todas as chamadas de API são
os três pontos de entrada de geometria**, e elas andam juntas: um `VertexPointer`, um
`TexCoordPointer` e um `DrawArrays` por lote. Se o custo do trampolim ainda pesar, é aqui que
ele pesa, e é aqui que agrupar paga.

## 12. Onde o tempo foi, nos cinco jogos (2026-09-24)

Rodada com `ZEEBX_ROM_PERFIL=1`, 15 s virtuais, rasterizador de software, na bancada descrita
acima. **O perfil custa caro**: o Quake sai de 8,8 s (170%) sem perfil para 12,6 s (119%) com
ele, cerca de 43%. Por isso a tabela abaixo serve para **proporção**, e o relógio absoluto vem
da rodada sem perfil.

| jogo | real | velocidade | instr/s | chamadas de API | tempo em API | apresentação |
|---|---:|---:|---:|---:|---:|---:|
| Crash Nitro Kart 3D | 1,5 s | 1009% | 207 M | 44.559 | 1.145 ms | 975 ms (85%) |
| Double Dragon | 2,6 s | 569% | 22 M | 37.808 | 2.010 ms | 1.643 ms (82%) |
| NFS Carbon | 6,3 s | 238% | 51 M | 252.324 | 4.809 ms | 4.062 ms (84%) |
| Quake | 12,6 s | 119% | 136 M | 1.323.900 | 8.109 ms | 5.139 ms (63%) |
| Zeebo Extreme Rolimã | 3,8 s | 396% | 240 M | 176.080 | 2.316 ms | 1.242 ms (54%) |

"Apresentação" é o `eglSwapBuffers`/`SwapBuffers`, e é preciso dizer o que ele **é** por dentro,
senão o número engana:

```rust
"SwapBuffers" => {
    self.sync_egl_color_from_guest()?;
    self.egl_swaps += 1;
    self.present_gl();
    self.wait_for_vsync();
    (2, gles::EGL_TRUE)
}
```
`src/machine/egl.rs:311`

O `present_gl` chama `frame_rgb565` (`src/machine/gl.rs:1051`), e o `frame_rgb565` do software
começa com `self.flush()` (`src/video/rasterizer.rs:2451`) — **é ali que a fila de triângulos
vira pixel**. Ou seja: o tempo do `SwapBuffers` é rasterização, conversão RGBA→RGB565 e cópia,
não "troca de buffer". O `wait_for_vsync` só mexe no relógio virtual e não dorme
(`src/machine/time.rs:115`).

### O que isso muda

**A frase "o rasterizador não é gargalo" do `ARCHITECTURE.md:271` não vale mais nesta
configuração.** Ela foi escrita quando o ARM custava o dobro; com o Dynarmic, a fatia do ARM
encolheu e o preenchimento passou a dominar: 54% a 85% do tempo de API em todos os cinco jogos,
e algo entre 41% e 65% do relógio de parede.

Outros alvos que o perfil mostra:

- **Geometria, no Quake:** `DrawArrays` 876 ms (10,8%), `TexCoordPointer` 431 ms (5,3%),
  `VertexPointer` 427 ms (5,3%) — 1,73 s, 21% do tempo de API. O trio não é caro só por parar o
  JIT; ele custa por dentro também.
- **`memset`, no Rolimã:** 728 ms, 31,4% do tempo de API. Confirma a suspeita antiga.
- **`strcmp`, no Quake:** 203 ms, 2,5%. Menor do que o número de chamadas sugeria — 137 mil
  chamadas, mas o `read_cbytes` já lê em blocos de 64 bytes.
- **Crash Nitro Kart:** 44 mil chamadas em 15 s e 1009% de velocidade. O jogo que motivou a
  proteção do `glReadPixels` é, na bancada, o mais folgado dos cinco.

### Consequência para o plano

A escala interna fracionária (`0,5x`, `0,25x`) **sobe** de prioridade, mas com um alvo
diferente do que se pensava: o caminho de **software**, onde o custo por pixel é CPU e o
`define_escala` hoje é ignorado de propósito (`src/video/rasterizer.rs:756`).

O trampolim **não** sobe nem desce: continua não medido, porque o `api_time` só conta o corpo
do método.

## 13. PDCA 1 — reutilização de índices temporários (rejeitado)

### Plan

`DrawArrays` criava um `Vec<u32>` novo por lote:

```rust
let indices: Vec<u32> = (0..count).map(|i| first + i).collect();
```

O Quake mediu 278.297 chamadas em 15 s. A hipótese era que reaproveitar a capacidade do vetor
reduziria alocações e melhoraria o tempo.

### Do

Foi implementado localmente um `gl_indices_scratch` em `Machine`, reutilizado em
`DrawArrays` e `DrawElements`. O patch não alterava índices, ABI, rasterização nem semântica.

### Check

A comparação foi controlada, sem `ZEEBX_ROM_PERFIL`, mesmo Ryzen 7 5700U, mesmo Quake, mesmo
`ZEEBX_ROM_MS=15000`, e com o perfil release:

| versão | tempo real | velocidade |
|---|---:|---:|
| baseline | 8,8 s | 171% |
| scratch | 8,8 s | 169% |

A variação é ruído de bancada. O patch não produziu ganho mensurável.

### Act

Patch **rejeitado e removido**. Não será commitado como otimização. A conclusão é útil: a
alocação do vetor de índices não é o custo dominante neste caso; a rasterização, conversão e
cópia no `SwapBuffers` são maiores. O próximo patch deve atacar esse caminho ou eliminar o
materializador de índices por completo com um `DrawArrays` contínuo, não apenas reutilizar a
capacidade.

A varredura de 66 jogos foi interrompida para não contaminar o A/B. Ela chegou a Pac-Mania, que
atingiu 52.830 ms virtuais em 240 s reais (22%); não é uma linha de base completa e não deve ser
tratada como resultado final.

## 14. PDCA 2 — exportação direta em palavras RGB565 (aceito com ganho pequeno)

### Plan

`present_gl` fazia o rasterizador gerar bytes RGB565 e depois a superfície convertia cada par de
bytes de volta para `u16`. No caminho de software, isto era trabalho e tráfego duplicados.

### Do

Foi adicionado `frame_rgb565_words()` ao contrato do rasterizador, implementado nos caminhos
software e placa. `present_gl` agora atualiza a superfície com `load_rgb565_words()`. O vetor
`gl_last_frame` também passou a guardar palavras RGB565; a conversão para bytes fica apenas nos
caminhos que pedem bytes, como `eglGetColorBuffer`.

### Check

A/B sequencial, sem perfil, mesma bancada, Quake, 15.016 ms virtuais por rodada:

| rodada | baseline | palavras RGB565 |
|---:|---:|---:|
| 1 | 9,1 s / 165% | 9,0 s / 167% |
| 2 | 9,0 s / 167% | 8,8 s / 170% |
| 3 | 8,8 s / 171% | 8,8 s / 170% |
| média | **8,97 s / 168%** | **8,87 s / 169%** |

Ganho médio: aproximadamente **1,1% no relógio** e **0,6% na velocidade**. É pequeno e fica
próximo do ruído da bancada; não é apresentado como grande salto. Os testes release do núcleo
passaram.

### Act

Patch mantido porque remove uma conversão estruturalmente desnecessária, não porque o Quake
prometeu grande ganho. A próxima confirmação deve ser no rasterizador de placa dos handhelds e
em jogos diferentes. O gargalo dominante continua sendo `flush`/preenchimento, não este loop
isolado.

## 15. PDCA 3 — `memset` direto na memória guest (aceito)

### Plan

O Rolimã já tinha mostrado `memset` como 31% do tempo perfilado. O caminho anterior preenchia
um buffer de pilha de 4 KiB e chamava `write_mem` para cada bloco. Isso repetia borrow, validação
e invalidação de código.

### Do

`GuestMemory::fill` agora preenche diretamente as fatias das regiões, e `DynarmicCpu::fill_mem`
usa esse caminho. A invalidação de código ainda acontece uma vez no intervalo inteiro. Regiões
somente-leitura e endereços não mapeados continuam falhando; testes foram adicionados.

### Check

A/B sequencial no Ryzen 7 5700U, Rolimã, sem perfil, 15.048 ms virtuais:

| rodada | baseline | `GuestMemory::fill` |
|---:|---:|---:|
| 1 | 3,7 s / 408% | 2,8 s / 534% |
| 2 | 3,2 s / 469% | 3,0 s / 495% |
| média | **3,45 s / 439%** | **2,9 s / 515%** |

Ganho médio de aproximadamente **16% no relógio**. A variação da bancada é grande, mas as duas
rodadas apontam na mesma direção e o primeiro ganho foi de 24%.

### Act

Patch mantido. O caminho foi coberto pelos testes de memória e pela suíte release do núcleo.
Ainda falta medir NFS, Quake e os 66 jogos para saber quanto o ganho aparece fora do Rolimã.

## 16. Caminho de GPU nos handhelds: Mali não é AMD

### Hardware correto

- **R36S:** Rockchip **RK3326** (não RK3356), Cortex-A35, Mali-G31 MP2.
- **RG40XX-H:** Allwinner H700/A133, Cortex-A53, Mali-G31.

Ambos anunciam GLES 3.2 no hardware/libMali. Isso não garante que toda imagem com Mesa/Panfrost
exponha 3.2: há drivers G31 que expõem apenas GLES 3.1. O core pede GLES 3.2 e recua para o
software se o frontend recusar.

### Caminho real atual

No core Libretro, o fluxo de hardware é:

```text
retro_run
  -> context_reset / glow::Context
  -> GpuState
  -> gles_draw (lotes no host)
  -> eglSwapBuffers
       -> flush()
       -> glReadPixels (GLES lê RGBA8)
       -> conversão para RGB565
       -> tela CPU
  -> retro_video_refresh(HW_FRAME_BUFFER_VALID)
```

O último passo entrega o framebuffer do frontend ao RetroArch, mas o caminho anterior ainda faz
readback para a superfície CPU. No Mali, esse readback pode forçar a GPU tiled a terminar o quadro
e bloquear a CPU. A AMD do laptop pode esconder esse custo com cache/banda muito maiores; portanto
o benchmark software no Ryzen não estima o custo do Mali.

O patch `64ceaa5` removeu a conversão intermediária bytes→`u16`, mas **não** removeu `glReadPixels`.
O próximo grande patch de GPU deve ser lazy readback/zero-copy:

1. em cena 3D pura, apresentar o FBO do frontend sem ler a GPU para a CPU;
2. só fazer readback quando houver composição 2D, `GetColorBuffer`, `glReadPixels`, dump ou fallback;
3. preservar a tela 640×480 quando o jogo misturar `IDisplay` com GL;
4. manter Crash Nitro Kart no caminho seguro, porque ele lê pixels de volta;
5. medir antes/depois em libMali R36S e no driver do H700.

Não é seguro simplesmente apagar `present_gl`: jogos que desenham 2D depois do GL precisam da
base atualizada. A implementação deve ter um estado `gl_readback_pendente` e um ponto único que
materialize a tela antes de qualquer desenho 2D/leitura.

### Verificação que falta no RG40XX-H

O core agora registra no log, no `context_reset`:

```text
GL_VENDOR
GL_RENDERER
GL_VERSION
GLSL_VERSION
```

O R36S já teve `Mali-G31`/GLES 3.2 observado no teste físico. No RG40XX-H, o `video_driver="gl"`
do muOS prova o tipo de caminho, mas não substitui o log do driver efetivamente carregado. A
próxima instalação deve capturar essa linha e comparar vendor/renderer/version antes de qualquer
conclusão sobre Panfrost ou libMali.

### Primeiro passo seguro do caminho Mali

Antes do zero-copy completo, o core já evita um custo inútil no modo de placa: `retro_run` não
chama `write_rgb565_into()` nem calcula a assinatura CPU quando há FBO de hardware ativo. O
RetroArch recebe `HW_FRAME_BUFFER_VALID` e ignora o ponteiro de pixels nesse modo. Isso remove a
cópia/varredura CPU por quadro sem alterar a composição 2D nem o fallback.

Isso **não** remove ainda o `glReadPixels` que ocorre em `present_gl`; esse é o próximo patch e
precisa de readback preguiçoso para não quebrar jogos que misturam GL, `IDisplay` e leitura de
pixels.

## 17. Referência: ParaLLEl-N64 e Mupen64Plus-Next

Fontes revisados sem copiar código:

- `libretro/parallel-n64` em `6e4c44c`;
- `libretro/mupen64plus-libretro-nx` em `6752836`.

A CPU guest do N64 é MIPS e o Zeebo é ARM, então o lowering dos dynarecs não serve. A arquitetura
serve: fastmem por páginas, block linking, invalidação granular e corpus diferencial. O Zeebx já
delega isso ao Dynarmic; trocar por um JIT MIPS adaptado seria regressão de projeto.

As lições gráficas úteis são mais diretas:

1. **Quadro normal fica na GPU.** Os cores entregam `RETRO_HW_FRAME_BUFFER_VALID`; readback existe
   para a memória guest, screenshot ou compatibilidade, não para apresentar todo quadro.
2. **Readback sob demanda.** GLideN64 marca framebuffer sujo, lê apenas quando a CPU guest observa,
   pode recortar página/faixa e reduz para resolução nativa antes de copiar.
3. **PBO em anel.** Quando readback é inevitável, usa PBO duplo/triplo e consome o anterior; o
   fallback GLES2 é `glReadPixels` síncrono. O Zeebx deve marcar slots válidos e usar fence — o
   código de referência não é seguro para copiar literalmente.
4. **Shadow state.** Cacheia enable, FBO, buffer, textura, viewport, scissor, blend, depth, programa,
   atributos e uniforms. **Feito o programa/VAO/VBO**: os três são criados uma vez e nunca trocam, e
   o `submete_com` os religava a cada lote e os **desligava** no fim de cada um — cinco chamadas de
   driver por lote para reafirmar o que já valia, e desligar programa e VAO é o que faz um driver
   fino de ARM revalidar mais coisa. O Quake chega a 370 lotes por quadro. Agora entram uma vez
   (`GpuState::ligados`), e só o contexto refeito os invalida. O que **falta** do shadow state é o
   resto: o `aplica` ainda reemite enable/blend/depth/textura por lote.
5. **Batch por chave.** Acumula triângulos até mudar FBO/programa/texturas/blend/depth/scissor ou
   aparecer barreira. O Zeebx já agrupa leques/faixas de mesmo `Estado`, então deve melhorar o
   cache de estado antes de construir outro batcher.
6. **Streaming de VBO.** Usa ring persistente quando há `bufferStorage`; senão map unsynchronized.
   No Zeebx, só vale com segmentos e fences para não sobrescrever dados em uso no Mali.
7. **SIMD com oráculo escalar.** Angrylion mantém SSE2/NEON e escalar bit a bit. É o padrão certo
   para RGB565, textura e rasterização; não justifica importar o código específico do RDP/RSP.
8. **Thread GL é opcional.** Mupen avisa que melhora alguns drivers e adiciona latência. Contexto
   Libretro e lifetime dos argumentos tornam isso projeto de alto risco, não quick win.

Não há receita Mali pronta nesses cores. Eles usam capability probing e quirks medidos. Isso levou
a duas correções locais imediatas:

- handheld AArch64 agora pede **GLES 3.0**, suficiente para VAO/FBO blit/MSAA/GLSL 300, em vez de
  recusar Panfrost 3.1 por exigir 3.2;
- `GL_DEPTH_CLAMP` não é mais emitido no GLES, evitando `GL_INVALID_ENUM` e validação inútil por
  lote no driver Mali.

Prioridade resultante: lazy readback/zero-copy, shadow-state/uniform cache, PBO somente para
readback inevitável, e depois NEON nos kernels medidos. Vulkan/ParaLLEl-RDP e o dynarec MIPS ficam
fora do escopo.

## 18. Auditoria posterior: correções e rodada de 63 ROMs

A revisão dos patches encontrou três limites que a primeira implementação não registrou:

1. `GuestMemory::fill` podia alterar a primeira região e falhar na segunda antes da invalidação
   JIT. Agora o intervalo inteiro é validado antes de qualquer escrita, com teste de atomicidade.
2. `GL_DEPTH_CLAMP` deixou de ser ligado no GLES em `aplica`, mas ainda era desligado em
   `devolve_o_contexto`, produzindo o mesmo `GL_INVALID_ENUM`. Os dois lados estão guardados.
3. No FBO Libretro externo, o desenho ia para o framebuffer do frontend, mas `liga_para_leitura`
   ligava o FBO interno. `glReadPixels` e composição CPU podiam ler quadro velho. Em 1x sem MSAA,
   a leitura agora usa o mesmo FBO externo que recebeu o desenho.

Supersampling, MSAA e proporção larga junto do FBO externo ainda precisam de composição explícita
no FBO do frontend; não estão declarados resolvidos por esta correção.

A rodada longa terminou: 63 ROMs reconhecidas, 60 s virtuais, perfil ligado, 1.504 s reais. Casos
mais lentos entre os que avançaram: Action Hero 91%, Quake 93%, Ultimate Chess 114%, NFS Carbon
123%; o perfil encarece fortemente o relógio, portanto esses percentuais não são FPS de uso normal.
Seis casos pedem triagem: Bejeweled, Kingdom Hearts homebrew, Pac-Mania (22% e teto de 240 s),
Prey Evil, Ridge Racer e Opera Mini. Eles não devem ser chamados de regressão sem comparação com
a linha de base e roteiro equivalentes.

## 19. Referência: PCSX-ReARMed, DuckStation e Flycast

Fontes revisados sem copiar código:

- `libretro/pcsx_rearmed` em `ff81ed1`;
- `stenzek/duckstation` em `7326f90`;
- `flyinghead/flycast` em `869038f`.

### O consenso útil

1. **Framebuffer fica na GPU.** Flycast mantém RTT como textura; DuckStation baixa só o retângulo
   solicitado; PCSX-ReARMed escreve direto no software framebuffer oferecido pelo Libretro. O
   Zeebx não deve fazer readback de todo quadro normal.
2. **Readback é uma operação guest, não apresentação.** Quando inevitável: bounding rect/tiles,
   staging/PBO em anel, fence e espera apenas quando a CPU guest realmente consome. DuckStation
   chega a manter rasterizador software paralelo para evitar round-trip em jogos de muita leitura.
3. **Shadow-state antes de mais batching.** DuckStation e Flycast evitam programa, textura,
   blend, depth, stencil, scissor e parâmetros repetidos. O `GpuState::aplica` do Zeebx ainda
   reemite quase tudo a cada flush.
4. **Batch/arena por estado compatível.** DuckStation acumula até mudar textura/blend/máscara ou
   hazard; Flycast envia vértices/índices/uniforms em arena por frame. O Zeebx já agrupa primitivas
   de mesmo `Estado`; deve acrescentar dirty bits e streaming seguro, não reescrever do zero.
5. **Thread de vídeo tem fila limitada.** DuckStation usa FIFO SPSC e no máximo dois frames;
   Flycast aplica back-pressure/deduplica e descarta apresentação velha. Fila ilimitada troca FPS
   por latência e RAM. Em Libretro/GL, ownership do contexto torna isto alto risco.
6. **Raster software em blocos especializados.** PCSX-ReARMed separa setup/textura/shade/blend e
   processa blocos de pixels com NEON; DuckStation compila o mesmo oráculo em escalar e SIMD. É o
   modelo para o `fill_band` do Zeebx: scalar correto, kernels por estado comum, teste diferencial.
7. **Fastmem/JIT não é para copiar.** DuckStation/Flycast usam arena virtual, fault-and-patch,
   block linking e invalidação adaptativa. O Zeebx já recebe JIT ARM e page table do Dynarmic; só
   valem como checklist e telemetria, não como substituto.
8. **Pacing no core Libretro não dorme.** PCSX-ReARMed executa até a fronteira emulada e devolve;
   o frontend controla wall clock. Flycast e DuckStation usam deadline/filas nos frontends próprios.
   O `sleep` atual do Zeebx dentro de `retro_run` merece PDCA separado por risco de double-throttle.

### Achados específicos

- **PCSX-ReARMed:** `GET_CURRENT_SOFTWARE_FRAMEBUFFER` pode remover uma cópia no fallback software;
  exige respeitar pitch/formato/lifetime. Seu GPU GLES é legado e não serve de backend moderno.
- **DuckStation:** dirty rectangles, download textures/PBO com fence, pipeline/shader cache e
  thread de vídeo robusta. Seu caminho AVX2 de rasterização está desativado por defeitos: aviso
  contra assumir que SIMD é automaticamente correto.
- **Flycast:** realmente tem quirks Mali. Testa `glBlitFramebuffer` e cai para quad se o driver
  anuncia mas falha; usa `DEPTH24_STENCIL8` no Mali; no Vulkan acrescenta barreiras ARM específicas.
  O Zeebx já usa `DEPTH24_STENCIL8`, mas deve testar blit no contexto real antes de confiar em MSAA
  e resolução interna.
- **Frameskip Flycast:** reage separadamente a CPU lenta e fila GPU ocupada. O Zeebx hoje reage ao
  aviso de áudio do Libretro; telemetria de backlog/tempo de GPU permitiria decisão melhor.

### Ordem que resulta desta comparação

1. lazy readback/zero-copy;
2. shadow-state + uniform cache;
3. teste real de FBO blit no driver;
4. PBO com fence para leitura inevitável;
5. `GET_CURRENT_SOFTWARE_FRAMEBUFFER` no fallback;
6. NEON/SSE no rasterizador software em blocos;
7. fila de vídeo limitada, somente depois;
8. PDCA do `sleep` dentro de `retro_run`.

### Licenças

PCSX-ReARMed e Flycast são GPL-2.0 ou posterior/compatíveis por arquivo, mas dependências variam.
O DuckStation atual usa **CC-BY-NC-ND-4.0**: não se copia nem porta código para o Zeebx distribuído.
Dele entram somente fatos e ideias, com implementação independente. Nenhum código destes três foi
copiado nesta rodada.

## 20. Instrumentação: o registro do núcleo em cinco níveis

O plano desta rodada começava por "medir antes de otimizar", e o primeiro obstáculo era que o
núcleo **não tinha canal de diagnóstico**: o desktop escrevia por `eprintln!` espalhado, o jogo
tinha um log próprio à parte, o core Libretro tinha um `log()` com o nível fixo, e a varredura
imprimia no relatório dela. Não havia como pedir "o nível de depuração do subsistema de CPU".

Agora há. `src/registro.rs` dá cinco níveis — `Depuracao` (o "verbose/debug"), `Informacao`,
`Aviso`, `Erro`, `Fatal` — com alvo (o subsistema) e um anel de 300 linhas que o frontend drena.

### Decisões que valem registrar

- **O padrão é `Aviso`, e o filtro custa uma leitura atômica.** Quem não mexe em nada continua
  com o silêncio de antes. A macro testa o nível **antes** de montar o texto: uma mensagem de
  depuração filtrada não paga nem o `format!`.
- **O núcleo guarda, o frontend entrega.** O log do frontend é callback variádico do C; chamá-lo
  do fundo de um desenho seria atravessar código do frontend no meio de um estado nosso. O anel
  é o que separa as duas coisas — o despejo acontece no `retro_run`.
- **"Desligado" não é "não entendi".** O primeiro desenho devolvia o mesmo `None` para os dois, e
  um `ZEEBX_LOG=banana` desligaria o registro em silêncio. Hoje há um `Ajuste` que distingue, e o
  token inválido **avisa**.
- **Um vocabulário só** para a opção do core, o `config.ini`, o `settings.json` e a variável de
  ambiente. Quatro tabelas divergiriam; uma função de conversão não.
- **Precedência:** a variável de ambiente vale como ponto de partida e a opção explícita ganha
  dela, nos três frontends.

### Onde liga

| Frontend | Onde |
|---|---|
| Core Libretro | opção `zeebx_log` (categoria Sistema), vale na hora |
| Headless | `[system] log = warn` no `config.ini` |
| Desktop | `debug.nivel_de_log` no `settings.json` |
| Qualquer um | variável `ZEEBX_LOG` |

### O que já se vê

Prova real, Double Dragon, 60 quadros, `ZEEBX_LOG=debug`:

```text
DEBUG session: conteúdo ...Double Dragon (Brazil) (Es,Pt).zip resolvido para .../ddragonz.mod
DEBUG session: 462748 bytes lidos; analisando o módulo
DEBUG cpu: tabela de páginas: 20082 de 1048576 com acesso direto (9 region(oes))
INFO session: abriu Double Dragon (Brazil) (Es,Pt).zip (classe 0x0102f789) com o rasterizador de processador e 0 applet(s) instalado(s)
INFO midi: tabela de timbres: 15051 bytes de SMF -> 47.6s de áudio sintetizados em 187.6ms
```

Os dois números que interessam ao plano:

- **20 082 páginas de 1 048 576 com acesso direto.** A tabela do Dynarmic cobre 1,9% do espaço de
  endereços, que é o esperado — só as regiões mapeadas —, mas é o primeiro número que diz quanto
  do acesso do guest passa direto e quanto passa pelo Rust. Serve de linha de base para discutir
  SMC e páginas quentes.
- **187 ms para sintetizar 47,6 s de música.** Confirma, no caminho de uso, o que a medição de
  bancada dizia: o custo do MIDI é de carga, e é ele que explica a primeira música lenta no
  portátil, não o mixer.

### Achado colateral

O `log()` do core Libretro manda tudo com o nível `3` (`RETRO_LOG_ERROR`), inclusive as mensagens
informativas como "desenhando na placa". É anterior a esta rodada e **não foi mudado**: mexer
nisso altera o que aparece no log de quem filtra por gravidade, e é decisão à parte. O caminho
novo não depende dele — as linhas do núcleo saem com o nível certo.

### Pendências

- o Android ainda usa o `log` do Rust por conta própria e não drena o registro;
- a varredura não drena (o relatório dela já carrega o log do jogo);
- os subsistemas instrumentados nesta primeira passada são sessão, loader, CPU, áudio e MIDI. O
  rasterizador, a rede e o armazenamento ainda não têm linha própria.

## 21. Fechamento da revisão dos emuladores: os adendos

As seções 17 e 19 trazem os **cinco relatórios principais**. Esta seção fecha o que ficou de fora:
**doze adendos e correções** que chegaram depois, e que em vários casos **mudam a conclusão** do
relatório principal. Sem eles a revisão fica pela metade — foi assim que ela ficou até agora.

### Correções que mudam a conclusão

1. **Não construir fastmem novo, e nem priorizá-lo.** O `src/cpu/dynarmic.rs` já entrega ao
   Dynarmic uma tabela direta de 2²⁰ ponteiros de página, bitmap de páginas executadas e
   invalidação por faixa. O segundo adendo é explícito: a tabela atual **já captura o ganho
   principal**, e o que resta é **SMC adaptativo** — página que mistura código e dados sai da
   tabela e passa a ir por callback em toda leitura e escrita. Se algum dia for para valer, a
   rota é a API do próprio Dynarmic (`Config::fastmem`), nunca um manipulador de sinal nosso:
   o handler não pode alocar, travar nem entrar em `RefCell`.
2. **Não criar thread só de apresentação.** O DuckStation **removeu** a apresentação dedicada em
   `1c1b82ed` com a justificativa "Worse frame pacing" e unificou render e present na mesma
   thread. Se o Zeebx paralelizar, é **uma** thread dona de contexto, desenho e apresentação, com
   backpressure de 0 ou 1 quadro no começo.
3. **O batching de baixo risco que falta é o de faixas adjacentes.** O Flycast une *strips*
   vizinhas de estado equivalente (`PolyParam::equivalentIgnoreCullDirection`,
   `makePrimRestartIndex` com índice de reinício `~0` e correção de winding) e ainda emite um
   draw por grupo — não há multidraw. Ordenar globalmente por estado quebra blending,
   translucência e multipasse. O nosso lote hoje fecha por mudança de `Estado`, que é a mesma
   família de chave; o que não temos é o *restart de primitiva* para juntar o que é adjacente.
4. **SIMD do Flycast tem pouca transferibilidade.** O ramo NEON do parser de TA cai em quatro
   `u64` no build ARM64, e `FIPR`/`FTRV` em NEON está **comentado no próprio código por
   precisão**. O único uso real é `shop_frswap`, com `Ld4/St4`. Confirmado: SIMD dos cores
   externos **não** justifica prioridade; continua valendo só como padrão de trabalho
   (escalar-oráculo, kernel por hotspot medido).
5. **O nosso `sleep` no `retro_run` é o desvio.** O `pl_frame_limit` do PCSX-ReARMed, no caminho
   Libretro, **só marca a fronteira**: entrega exatamente um quadro e não dorme, porque o relógio
   de parede é do frontend. O Zeebx dorme (`src/session.rs`), o que soma o nosso freio ao do
   frontend. Não é bug de correção, é risco de double-throttle e de judder — e virou item próprio
   na lista de pendências.
6. **`GL_ARM_shader_framebuffer_fetch` está detectado e inativo** nos cores (o próprio código
   força `ext_fetch = false`). Não contar como ganho que já existe.
7. **O cache binário de shader está desligado justamente no GLES** (`EnableShadersStorage` vive
   sob `#if !defined(HAVE_OPENGLES)`), então o engasgo de compilação continua lá nos cores. Para
   nós isto é **inócuo**: o motor tem **um** shader e o compila uma vez.

### Confirmações no nosso código

- **A guarda do lote existe.** O adendo avisou que o Mupen junta 256 vértices e 1024 índices sem
  guarda aparente. Conferido: o nosso fecha o lote por teto (`VERTICES_NO_LOTE`, 16 384 vértices)
  antes de inserir. Não há o defeito.
- **O consumo parcial do áudio é tratado.** O PCSX ignora o retorno parcial do
  `audio_batch_cb`; o nosso guarda o que sobrou em `audio_pendente` e entrega na chamada seguinte.
- **A latência de áudio é constante, não derivada.** O PCSX recalcula a latência mínima só quando
  muda o tipo de frameskip, e ela pode ficar obsoleta ao mudar intervalo ou NTSC/PAL. O nosso
  pede 96 ms uma vez, e a constante não depende da política de quadro — sem o defeito de valor
  velho, mas com a mesma fragilidade se um dia ela virar função da política.

### Riscos que só valem se mexermos nessas frentes

- **GLSM do `libretro-common`**: ~64 MiB de BSS com índice direto por (programa, location) e
  estouro acima de 1024. Não copiar; cache de uniforme, se houver, com mapa esparso e bit de
  validade.
- **`AHardwareBuffer` do Mupen**: cria com `CPU_READ_OFTEN|GPU_SAMPLED_IMAGE`, usa como alvo de
  desenho **sem** `GPU_COLOR_OUTPUT` e ignora o retorno de `allocate()`. Se um dia formos por
  `EGLImage`, pedir `GPU_COLOR_OUTPUT`, validar a alocação e cair para PBO em qualquer falha.
- **Threaded wrapper do GLideN64**: cresce de 10 MiB até 200 MiB. Ganho opcional de driver com
  custo de RAM e latência; não é o padrão para portátil.
- **`N64DepthCompare` "compatível"**: `glMemoryBarrier` mais um desenho por triângulo. É correção
  com custo extremo em GPU de tiles — serve de aviso contra soluções "por triângulo" no Mali.
- **W^X e flush de I-cache em lotes** (`start_tcache_write`/`do_clear_cache`) só importam quando o
  JIT for nosso; hoje é do Dynarmic.

### Achados Mali que ainda não usamos

- **Testar `glBlitFramebuffer` em execução.** O Flycast não confia no anúncio do driver: faz um
  teste de verdade e, quando ele mente, desenha um quadrilátero. O Zeebx assume que o blit
  funciona — e o usa no `resolve` de MSAA e na leitura do quadro grande.
- **Descartar profundidade e estêncil depois do último passe.** O GLES do Flycast **não** chama
  `glInvalidateFramebuffer` nem `glDiscardFramebufferEXT`: isso seria experimento **novo** nosso,
  não receita copiada. Em GPU de tiles é onde pode render.
- **`DEPTH24_STENCIL8` no Mali** — o Flycast troca o formato por causa de quirk; nós já usamos
  esse formato, então é confirmação, não pendência.
- **Modelo de memória de tile no Vulkan**: `eTransientAttachment`, `LAZILY_ALLOCATED`, storeOp
  `eDontCare` e dependências `eByRegion`. É referência para quando houver Vulkan, não para hoje.
- **Não portar o "tile renderer" do Dreamcast**: o Flycast não reexecuta tile a tile — converte o
  recorte em `scissor` e teste de shader. Seria reescrita enorme.
- **`SwapIntervalDetector`**: mede o intervalo de vblank com média móvel e força intervalo 1
  quando a taxa é instável, para não cair em 30 FPS acidental. É ideia aproveitável no nosso
  limitador e na decisão de frameskip.
- **O frameskip do Flycast separa "CPU lenta" de "fila de GPU cheia".** O nosso só reage ao aviso
  de áudio do frontend; telemetria de GPU permitiria a segunda decisão.

### O que não se aplica

- O caminho `ARMv5_ONLY` dos dynarecs (immediates, pools, sem `IDIV`, VFP condicional) é modelo de
  restrição de ARM11/v6. O nosso **guest** é ARM e o **host** é ARM64; o JIT não é nosso.
- Lightrec é LGPL-2.1+ e o GNU Lightning do `rsp_jit` é LGPL-3.0+: importar elevaria o conjunto de
  licenças do projeto, que é `GPL-2.0-or-later`.

### Licenças, fechadas

| fonte | licença | uso |
|---|---|---|
| PCSX-ReARMed, Flycast | GPL-2.0-or-later nos arquivos citados | compatível com o nosso; com avisos preservados |
| Mupen64Plus-Next, parallel-n64 | agregado: GPL-2.0 (núcleo), LGPLv3 (gles2n64), MIT/LGPLv3 (rsp) | conferir por arquivo antes de qualquer cópia |
| DuckStation | **CC-BY-NC-ND-4.0** | **nenhum código**; só fatos e ideias, com implementação independente |
| dependências vendorizadas (VIXL, Swappy, GNU Lightning) | próprias | algumas elevam o conjunto |

**Nenhuma linha de código destes emuladores foi copiada.** O que entrou no projeto foram fatos,
nomes de função para conferência e as decisões de engenharia listadas acima.

### A ordem revisada

| muda | frente | por quê |
|---|---|---|
| sobe | cache de estado GL | ganho de baixo risco, apontado por duas revisões |
| sobe | juntar lotes adjacentes com restart | batching que ainda não temos, e de baixo risco |
| sobe | testar blit e descarte no driver real | sem isso, MSAA e leitura podem estar mentindo |
| sobe | SMC medido | é o que resta de real na frente de memória |
| entra | PDCA do `sleep` no `retro_run` | desvio do contrato Libretro, medido contra o PCSX |
| desce | fastmem novo | o Dynarmic já entrega |
| desce | SIMD dos cores | pouca transferibilidade confirmada |
| desce | thread de apresentação | o DuckStation removeu por piorar o pacing |

## 22. A partilha do relógio: quanto fica fora do JIT

**O número que faltava desde o começo desta rodada.** O `ARCHITECTURE.md` publicava 1,4 µs por
chamada de API, medidos com o Unicorn; o instrumento de então (`ZEEBX_ROM_PERFIL`) mede só o corpo
do método. Agora o relatório da varredura separa o relógio em duas metades.

### Como é medido

O `run` do Dynarmic já era o lugar por onde tudo passa: cada chamada de API é uma saída e uma
reentrada. O relógio em volta dele dá o tempo **dentro** do JIT — execução do guest mais as
callbacks de memória que o próprio JIT chama. Tudo o que sobra do laço é despacho: o trampolim, o
corpo do método, os callbacks entregues na fronteira e a contabilidade da sessão.

### O erro que a primeira versão cometeu, e o que ele ensinou

Com o relógio em **toda** entrada, o Quake saiu de 8,8 s para 12,3 s: **40% mais lento**. O
`Instant::now` desta máquina é chamada de sistema, e não o caminho rápido do `vDSO` — 1,3 µs por
leitura, 2,6 µs por entrada, num jogo com 1,3 milhão de entradas.

A correção é ler o relógio **uma entrada em cada 64** e multiplicar a média amostrada pelo
contador. E como o custo caiu para cerca de 0,6%, a medida ficou **sempre ligada**: todo relatório
de varredura passa a trazer a partilha, sem depender de alguém lembrar de ligá-la.

### A prova de que não custa

Cinco rodadas do mesmo Quake, 15.016 ms virtuais, intercalando as duas construções:

| construção | rodada | tempo real | velocidade |
|---|---:|---:|---:|
| com a partilha | 1 | 8,9 s | 168% |
| com a partilha | 2 | 8,8 s | 170% |
| sem a partilha | 1 | 8,9 s | 169% |
| sem a partilha | 2 | 8,8 s | 171% |
| com a partilha | 3 | 8,9 s | 169% |

As duas construções são indistinguíveis. O instrumento entra.

### O que ele diz

```text
desempenho:
  abriu em 0.1 s, rodou 8.9 s reais para 15016 ms virtuais (169% da velocidade do console)
  1091 volta(s), 583 quadro(s) (38 fps virtuais)
  1712755541 instruções (193 milhões/s), 1323900 chamada(s) de API
  5434 ms dentro do JIT e 3458 ms fora (38% do laço no despacho); 1324767 entrada(s)
  em 20700 amostra(s), 2612 ns fora do JIT por chamada de API
```

Três leituras, e as três importam:

1. **38% do relógio está fora do JIT.** O despacho não é detalhe: é mais de um terço do trabalho
   do Quake, e é o maior alvo isolado que a árvore tem hoje.
2. **2,6 µs por chamada, fora do JIT.** É o dobro do 1,4 µs que o documento publicava — e o dobro
   de um número que era de outro backend.
3. **Dentro do JIT são 317 milhões de instruções por segundo**, contra 193 milhões no total. O
   recompilador é rápido; o que custa é atravessar a fronteira.

**A ressalva, dita por inteiro:** 2,6 µs é **limite superior** do trampolim. Dentro desses 3458 ms
também estão o corpo de cada método, os callbacks entregues na fronteira e o trabalho do próprio
arranjo de medição. O que separa corpo de trampolim é o perfil de API — e ele confirma o número
por outro caminho: com o perfil ligado, "fora do JIT" subiu para 7272 ms, e o custo do próprio
perfil (1,32 milhão de chamadas × cerca de 2,6 µs) responde por uns 3,4 s disso, o que devolve os
mesmos ~3,9 s. Os dois instrumentos concordam.

### O que isso muda na ordem

**O trampolim sobe ao primeiro lugar**, à frente do cache de estado GL. Ele custa 38% do relógio no
pior caso da árvore; o cache de estado GL ataca um punhado de chamadas por lote de desenho. E há um
caminho de ganho que não exige o redesenho do trampolim: **reduzir o número de fronteiras**. O Quake
atravessa 1,32 milhão de vezes em quinze segundos, e 845 mil delas são os três métodos de geometria
— `VertexPointer`, `TexCoordPointer` e `DrawArrays`, um trio por lote. Enquanto o trio continuar
sendo três chamadas, o custo fixo é pago três vezes por lote.

### Pendências desta frente

- medir a partilha **no portátil**: o desktop diz 38%, e o aparelho tem outro equilíbrio entre
  núcleo e memória. O instrumento agora viaja no relatório de lá.
- o mesmo relatório no Pac-Mania, que é o caso de 48 milhões de chamadas: se a partilha dele for
  ainda maior, ele deixa de ser "o jogo lento" e passa a ser a medida do custo fixo.

## 23. O perfil de API media o próprio relógio

**Esta é a correção mais importante da rodada, e ela invalida leituras anteriores deste próprio
documento.**

### O que estava errado

O perfil de API cronometra cada chamada com um par `Instant::now()`/`elapsed()`. Em Linux isso
costuma ser o caminho rápido do `vDSO` — dezenas de nanossegundos —, e foi por isso que a medida
pareceu aceitável por tanto tempo.

**Nesta máquina não é.** Medido, com prova no teste `quanto_custa_o_relogio`:

```text
relógio: 1318 ns por leitura, 2634 ns por par (now+elapsed); mapa por (interface, slot): 15 ns
```

**1 318 nanossegundos por leitura de relógio** — é chamada de sistema, não `vDSO`. E o perfil lê o
relógio **duas vezes por chamada**. Logo todo método que aparecia custando em torno de 1,4 µs
custava, de fato, **zero**: o número era o instrumento.

Foi assim que o `GetClipRect` do Pac-Mania apareceu com 45 s para 32 milhões de chamadas (1,39 µs
cada) e virou "o hotspot mais claro da árvore". Não era. O `VertexPointer`, o `TexCoordPointer` e o
`strcmp` do Quake estavam na mesma lista, todos com ~1,5 µs de "corpo".

### As duas correções

1. **Amostragem:** o relógio é lido em uma chamada a cada 64, e a média é multiplicada pela
   contagem. A contagem de chamadas continua exata; só o tempo é estimado.
2. **Desconto do instrumento:** na abertura do perfil, o custo de uma leitura de relógio é medido
   nesta máquina (4 096 leituras) e subtraído da média antes de estimar. Um método mais barato que
   o instrumento fica em zero, que é a resposta certa.

Com as duas, o perfil deixou de encarecer a execução: **8,9 s contra 8,7 s** do mesmo Quake, dentro
do ruído — antes, custava 42%.

### O quadro verdadeiro (Quake, 15 s virtuais, software)

```text
onde o tempo foi (5463 ms em chamadas de API):
     4951.8 ms   90.6%  IEGL11::SwapBuffers
      183.6 ms    3.4%  IGLES11::Clear
      125.1 ms    2.3%  IGLES11::DrawElements
      108.8 ms    2.0%  IGLES11::DrawArrays
       47.0 ms    0.9%  AEEHelpers::malloc
```

E os três que dominavam a lista antiga **sumiram**: `TexCoordPointer`, `VertexPointer` e `strcmp`
não têm corpo medível.

### O que isso diz, e o que não diz

**Diz:** o custo real está em **um método**, o `SwapBuffers`, e ele é o caminho de apresentação —
`flush` da fila de triângulos, leitura do quadro pela CPU e conversão para RGB565. Isto confirma,
com o instrumento consertado, o que a primeira leitura já suspeitava por outro caminho: no
caminho de software, quem domina é a rasterização, não o despacho de método.

**Diz também:** o custo dos três métodos de geometria **não é o corpo, é a fronteira**. Eles somam
845 mil das 1,32 milhão de entradas no JIT do Quake, a ~2,6 µs cada. Ou seja, o trio custa o preço
do trampolim, não o preço do que faz — o que reafirma a partilha da seção 22 e explica por que os
dois instrumentos discordavam.

**Não diz:** que estes valores absolutos se somem. Os baldes se sobrepõem — há método de API que
reentra no JIT por dentro —, então a soma (5,46 s) chega a passar do tempo fora do JIT (3,4 s).
**Vale a ordem, não o valor absoluto.** Para valor absoluto, o que se usa é a partilha da seção 22,
que é medida sem nada dentro do laço.

### Consequências para o plano

- o `SwapBuffers` (apresentação: `flush` + leitura + conversão) passa a ser **o** alvo do caminho
  de software, e é onde o readback preguiçoso e a escala interna atacam;
- o trio de geometria **não** pede otimização de corpo: pede fronteira mais barata;
- o Pac-Mania sai da lista de hotspots de método e volta a ser o que é: um jogo com 48 milhões de
  chamadas, ou seja, **48 milhões de fronteiras**;
- e a regra "meça sempre sem o perfil" pode ser revista: com a amostragem, o perfil é barato. O que
  continua valendo é não misturar os dois instrumentos no mesmo número.

## 24. Readback preguiçoso: o quadro da placa só vai para a CPU quando alguém precisa

Frente 1 das treze, e a primeira de todas as revisões externas. Feita no commit `a5acf46`.

### O que era

`eglSwapBuffers` fazia duas coisas de uma vez, e elas não são a mesma coisa:

1. **pintar a fila de triângulos** — a rasterização do quadro, trabalho que o jogo pediu;
2. **ler o quadro de volta para a memória da CPU** — `glReadPixels`, conversão para RGB565 e cópia
   para a tela do console.

A segunda existe para dois consumidores: o **desenho 2D por cima** (HUD pelo `IDisplay`, caixa de
mensagem, a Z-Wheel compondo) e quem **pede os pixels** (o `glReadPixels` do jogo, a gravação de
quadro). Um jogo de 3D puro não faz nenhuma das duas — e pagava a leitura em toda troca de buffer.
Num GPU de tiles, como o Mali dos dois portáteis, isso obriga a GPU a terminar e devolver o quadro.

### O que ficou

`present_gl` agora pinta a fila e marca o quadro como **pendente**; quem precisa dos pixels chama
`materializa_quadro_gl`, que faz a leitura uma vez só:

| quem | materializa? |
|---|---|
| `IDisplay`, `IGraphics`, `IBitmap` (qualquer 2D) | **sim**, antes de escrever — senão o 2D apagaria a cena |
| `glReadPixels` do jogo | não: ele lê o lado do OpenGL, que já está correto |
| janela/core apresentando a **textura da placa** | **não** — é o ganho |
| janela apresentando a **tela da CPU** | sim, antes de converter para bytes |
| despejo de quadro, `gl_frame`, frontend de fora | sim |
| `eglGetColorBufferQUALCOMM` | não: lê o lado do OpenGL |

**E só a placa adia.** No rasterizador de processador a "leitura" é uma conversão em memória: não há
espera a economizar, e o frontend lê a tela todo quadro — adiar ali só criaria a chance de ele
apresentar um quadro velho. O trait ganhou `quadro_espera_pela_placa` para isso.

### A medida

O teste `os_dois_rasterizadores_desenham_o_mesmo_quadro` roda o mesmo jogo nos dois caminhos e
compara os 307 200 pixels. Acrescentei a ele a contagem de trocas de buffer contra leituras:

| jogo | trocas de buffer | leituras (processador) | leituras (placa) |
|---|---:|---:|---:|
| Crash Bandicoot Nitro Kart 3D | 78 | 78 | **1** |
| Need for Speed Carbon | 184 | 184 | **1** |

Uma leitura no caminho de placa, e é a do próprio teste ao pedir os pixels no fim. **De 78 e 184
leituras de quadro para uma.** Os dois jogos desenham tudo pelo OpenGL — inclusive o HUD —, que é o
caso em que o adiamento rende inteiro.

### O erro que eu cometi no caminho, e como ele apareceu

A primeira versão adiava **nos dois** rasterizadores e não corrigia todos os consumidores. O teste
de comparação passou com "0 de 307 200 pixels diferentes" — e esse passe era **vazio**: o caminho de
software nunca mais atualizava a tela, então ele comparava duas telas velhas.

O conserto foi duplo:

- adiar só onde a leitura custa espera de placa (o que também devolve o comportamento antigo ao
  software, sem tocar nele);
- **guarda contra passe vazio** no teste: ele agora exige que a tela tenha conteúdo
  (mais de 1% de pixels acesos) antes de comparar. Um teste que compara duas telas apagadas não
  guarda coisa alguma, e foi preciso um jogo de 3D de verdade para o defeito aparecer.

### Achado colateral: o teste já falhava no Need for Speed, e ninguém sabia

Com NFS, os dois rasterizadores divergem de verdade — 11,54% dos pixels acima de dois passos de
canal, pior caso 88 passos —, e o teste **falha**. Verifiquei com A/B: os números são **idênticos**
com e sem esta mudança, então a divergência é anterior e não tem relação com o readback.

Ela não aparecia porque a suíte roda sem ROM: sem `ZEEBX_TESTE_ROM` o teste se declara dispensado.
No Crash, o mesmo teste dá 0,00% acima de dois passos — a mesma imagem, com o arredondamento
conhecido dos dois conversores. **O NFS é o único dos três com divergência real**, e isso casa com
o que a issue #36 relata sobre ele. Fica registrado como frente própria: ou o rasterizador de placa
está desenhando diferente, ou o de software está.

## 25. PDCA rejeitado: tirar a devolução do contexto do caminho por lote

Frente 2 das treze (cache de estado GL). **Rejeitado**, e o registro vale mais que o código.

### A hipótese, e por que ela parecia boa

O `GpuState::submete_com` — que roda uma vez por lote de desenho — terminava chamando
`devolve_o_contexto`, que faz **doze chamadas de GL** para desfazer o que o lote seguinte refaz:
framebuffer padrão, textura nula, cinco capacidades desligadas, máscara e faixa de profundidade,
máscara de estêncil e máscara de cor. A própria documentação do trait já dizia que ela deveria
acontecer **uma vez por quadro**, e não a cada desenho.

A conta parecia grande: com ~370 lotes por quadro no Quake, tirar doze chamadas por lote são mais
de **quatro mil chamadas de GL por quadro**.

### O que foi tentado, e o que cada tentativa mediu

| tentativa | onde a devolução passou a acontecer | medido |
|---|---|---|
| 1 | fim da volta do guest (`Machine::fecha_volta`) | **2 923** devoluções para **78** quadros (Crash) |
| 2 | só quando houve desenho, mais o quadro apresentado | **0** devoluções |

A primeira tentativa **piorou**: o fim da volta é mais frequente que o lote — 2 923 voltas do guest
para 78 quadros, ou seja 37 por quadro. Trocar "doze chamadas por lote" por "doze chamadas por
volta" com 37 voltas por quadro é pagar mais.

A segunda tentativa não conseguiu nem acender o contador, apesar de o caminho estar inteiro no
papel: a marca é posta no fim do `submete_com`, a devolução é chamada no `present_gl` depois de
pintar a fila, e o `emprestado` é verdadeiro no teste. Não achei a causa dentro do orçamento desta
rodada, e **sem entender por que o contador fica em zero eu não mantenho a mudança**.

### A decisão

**Revertido.** O código voltou ao que estava antes deste PDCA, mantendo todo o resto. O que fica é
a medida dos dois extremos — uma devolução por lote é desperdício, e uma por volta é pior — e o
caminho do meio (por quadro apresentado) segue **não demonstrado**.

### O que este PDCA ensinou

1. **"Uma vez por quadro" precisa de um lugar que aconteça uma vez por quadro**, e o candidato
   óbvio (o fim da volta do guest) não é esse lugar: a volta do guest não tem relação com o quadro
   apresentado. Medido, e não suposto.
2. **Um contador que não acende é informação, não silêncio.** A tentativa 2 parecia certa e não
   moveu o número; manter assim seria trocar comportamento por nada, com risco para o desktop, onde
   é essa devolução que impede a interface do `egui` de sumir.
3. **Falhar num instrumento não invalida o alvo.** O cache de estado GL continua sendo o melhor
   risco/ganho apontado pelas revisões — o que falhou aqui foi o *lugar* da devolução, não a ideia
   de não repetir o que já está na placa. A próxima tentativa deve atacar as dezoito chamadas do
   `aplica` com um espelho do estado, que é um trabalho diferente e mais direto.

## Onde ficou a rodada

| # | frente | estado |
|---|---|---|
| 1 | readback preguiçoso | **feito** — 78 e 184 leituras viraram 1 |
| 2 | cache de estado GL | tentado e **revertido**; o alvo segue aberto |
| 3 | `GET_CURRENT_SOFTWARE_FRAMEBUFFER` | pendente |
| 4 | fastmem do Dynarmic | não priorizar (adendos) |
| 5 | prova do blit do driver | **feito** |
| 6 | PBO + fence | sem razão depois da frente 1 |
| 7 | `0,5x`/`0,25x` | pendente |
| 8 | NEON/SSE | despriorizado |
| 9 | partilha JIT × despacho | **feito** — 38% fora do JIT, 2,6 µs por chamada |
| 10 | PDCA do `sleep` no `retro_run` | pendente |
| 11 | `lto = "fat"` | pendente (precisa de disco) |
| 12 | `glInvalidateFramebuffer` | pendente |
| 13 | Android drenar o registro | pendente, mecânico |

Além das treze: o **perfil de API** foi consertado (media o próprio relógio), o
`ARCHITECTURE.md` saiu da era do Unicorn, e o registro em cinco níveis entrou nos três frontends.

## 26. Espelho de estado: 80% das chamadas de estado saíram

Frente 2 das treze, depois do PDCA rejeitado da seção 25. Commit `f323467`.

### O que é

Um espelho, dentro do `GpuState`, do que já está na placa. **Todo campo começa em `None`, e `None`
quer dizer "não se sabe"** — é o que torna a invalidação trivial: esquecer é voltar ao padrão, e
daí tudo é reenviado uma vez.

Ele cobre as dezoito chamadas de [`GpuState::aplica`]: viewport, tesoura e o liga/desliga dela,
abraço de profundidade, teste, função, máscara e faixa de profundidade, mistura e sua função,
máscara de cor, descarte e seu modo, face frontal, teste, função, máscara e operações de estêncil.

### O erro que a primeira versão cometeu

A primeira versão esquecia o espelho **por inteiro** dentro do `devolve_o_contexto` — que roda a
cada lote. Um espelho zerado a cada lote responde "mudou" dezoito vezes sempre. Medido:

```text
placa: 1602 estado(s) enviado(s) e 0 poupado(s) pelo espelho
```

**Zero.** O espelho existia e não poupava nada.

O conserto é a diferença entre esquecer e **aprender**: em vez de zerar, a devolução registra o
estado **que ela mesma deixa** — tesoura desligada, teste de profundidade, mistura, descarte e
estêncil desligados, máscara de profundidade ligada, faixa (0,1), máscara de estêncil cheia,
máscara de cor toda ligada. As chaves que ela não toca continuam **desconhecidas**, porque o outro
usuário do contexto pode ter mexido nelas — e é por isso que esquecer tudo seria seguro, só que
inútil.

O contador foi o que pegou isso: sem ele, a mudança pareceria certa e não valeria nada.

### A medida

| jogo | envios antes | envios agora | poupados |
|---|---:|---:|---:|
| Crash Bandicoot Nitro Kart 3D | 1 602 | **315** | 1 287 (**80,3%**) |
| Need for Speed Carbon | 5 593 | **1 339** | 4 254 (**76,1%**) |

O número é de **chamadas**, não de tempo: `Session::estado_enviado_e_poupado()` devolve o par, e o
teste de comparação dos dois rasterizadores o imprime. É o que dá para provar no desktop, onde o
driver já cacheia estado e o relógio não separaria nada.

### Onde a invalidação fica

Quatro pontos mexem neste estado **fora** do `aplica`, e cada um avisa o espelho:

| ponto | o que faz | como avisa |
|---|---|---|
| `devolve_o_contexto` | desliga capacidades e mexe em máscaras | **aprende** o estado que deixa |
| `destino` (destino novo) | viewport, máscaras, tesoura desligada | esquece tudo |
| `resolve` | desliga a tesoura para o blit de MSAA | esquece a tesoura |
| `liga_para_leitura` | idem, para a redução do quadro grande | esquece a tesoura |
| `clear` | desliga a tesoura antes do `glClear` | esquece a tesoura |

### O que ficou de fora, e por quê

- **Programa, VAO, buffers e texturas**: o `submete_com` já os liga uma vez por lote e o custo é
  menor que o do estado acima. Um espelho deles exigiria rastrear a unidade de textura ativa, que
  muda dentro do próprio lote.
- **A devolução do contexto**: continua por lote, como estava. Movê-la exigiria um lugar que
  aconteça uma vez por quadro, e a seção 25 mostra por que o candidato óbvio não serve.

### O que fica para o portátil

A contagem é do desktop. **No Mali a mesma chamada custa validação de driver**, e é lá que os 80%
devem aparecer no relógio. O contador viaja no relatório: `Session::estado_enviado_e_poupado()`.

## 27. Frente 7 (`0,5x`/`0,25x`): o desenho, medido antes de escrever

Não implementado nesta rodada. O desenho abaixo sai de leitura do código, e existe para a próxima
retomada começar sabendo onde está a dificuldade — que **não** é onde eu supunha.

### O que eu supunha, e o que é

Supunha ser preciso mexer em viewport, tesoura, anexo, leitura e apresentação. É menos: **o
rasterizador de software já sabe desenhar numa superfície menor e ampliar na apresentação** — é o
caminho que existe para o `EGL_QUALCOMM_surface_scale`, em que o jogo declara uma superfície
pequena e o aparelho estica.

O que faz isso funcionar:

- `GlState::surface()` devolve a superfície declarada, limitada ao quadro, e `frame_rgb565`
  **reamostra** dela para o tamanho de saída. Com 320×240 de superfície e 640×480 de saída o
  reamostrador é vizinho mais próximo com passo 2 — exato, e já escrito;
- `GlState::set_viewport` **deduz a superfície da maior viewport**: `surface = max(x + width)`. Ou
  seja, reduzir a viewport reduz a superfície deduzida sozinho, sem nenhuma outra mudança.

### O que falta, então

1. `GlState`: um campo `reducao: usize` (1, 2, 4) e um `define_reducao`;
2. `set_viewport` e `set_scissor`: dividir a coordenada por `reducao` antes de guardar — e a
   superfície deduzida sai reduzida junto;
3. `read_rect` (`glReadPixels`): o pedido do jogo vem em pixels do console e a superfície está
   reduzida; é preciso mapear o retângulo e **replicar** cada pixel reduzido, porque o chamador
   escreve `width * height` pixels na memória do guest;
4. a opção do core: hoje `zeebx_resolucao_interna` é `1|2|3|4` e vale só na placa. Ela precisa
   aceitar `0.5` e `0.25`, e mandar para o caminho de **software** — na placa quem reduz é outro
   mecanismo, e a placa ignora;
5. `clear`: conferir se limpa o quadro inteiro ou só a superfície. Aparentemente o quadro inteiro,
   o que é inofensivo — a apresentação só lê a região da superfície.

### Onde medir

**No desktop, e sem depender do portátil**: a varredura e o perfil de API rodam no rasterizador de
**software**. Com o perfil de API consertado (seção 23), o custo de `SwapBuffers` — que em software
é o `flush` da fila, ou seja a rasterização — sai direto. Reduzir a área para um quarto deve
aparecer ali, no mesmo instrumento que já mostrou `SwapBuffers` com 90,6% do tempo de método.

### O risco

HUD e alinhamento: qualquer desenho que o jogo faça **fora** da viewport declarada (um HUD em
coordenadas de tela sem `glViewport` próprio) cairia fora da superfície deduzida. É o mesmo risco
que já existe hoje para o `surface_scale`, e a mitigação é a mesma: a redução é opt-in, e o padrão
continua 1×.

### A ordem entre as frentes que sobraram

| frente | por que nesta ordem |
|---|---|
| 7 `0,5x`/`0,25x` | mede-se no desktop, ataca o `SwapBuffers`, que é 90% do tempo de método |
| 10 PDCA do `sleep` | mede-se no desktop, contrato do Libretro |
| 12 `glInvalidateFramebuffer` | só rende no Mali, e não se mede aqui |
| 3 `GET_CURRENT_SOFTWARE_FRAMEBUFFER` | só rende em frontend de framebuffer de software |
| 13 Android drenar o registro | mecânico, e o build Android não se verifica nesta máquina |
| 11 `lto = "fat"` | precisa de disco: 3,4 GB livres e o LTO gordo recompila as dependências |

## 28. Escala interna fracionária: `0,5x` e `0,25x` no rasterizador de software

Frente 7, feita no commit seguinte desta seção. O desenho da seção 27 acertou o diagnóstico: o
caminho já existia, e faltava pouco.

### O que foi feito

1. `GlState` ganhou `reducao` (1, 2 ou 4) e `define_reducao`, que **esquece a superfície** e
   reinicia viewport e tesoura — as coordenadas guardadas eram do tamanho antigo.
2. `set_viewport` e `set_scissor` dividem a coordenada por `reducao` antes de guardar. Como a
   superfície é **deduzida da maior viewport**, ela sai reduzida sozinha: não houve uma segunda
   mudança para manter em dia.
3. `read_rect` (o `glReadPixels` do jogo) mapeia o retângulo do console para a superfície reduzida.
   O chamador escreve `width × height` pixels na memória do guest, e a amostragem por divisão já
   entrega exatamente isso — cada pixel lido vale `reducao` pixels do console.
4. A opção `zeebx_resolucao_interna` passou a aceitar `0.5` e `0.25`, além de `1..4`. **Um número
   só, dois mecanismos:** abaixo de 1 quem reduz é o processador, acima de 1 quem amplia é a placa,
   e cada um usa o que lhe cabe.

### A medida

Quake, 15.016 ms virtuais, rasterizador de software, com o perfil de API ligado (que agora é
barato e honesto — ver a seção 23):

| redução | tempo real | velocidade | `SwapBuffers` |
|---|---:|---:|---:|
| 1× (nativo) | 8,9 s | 168% | 4 867 ms |
| **1/2** (320×240) | **6,9 s** | **216%** | 3 176 ms |
| **1/4** (160×120) | **6,0 s** | **249%** | 2 447 ms |

- `1/2`: **22% menos tempo real**, 29% mais velocidade;
- `1/4`: **33% menos tempo real**, 48% mais velocidade.

O `SwapBuffers` não cai na proporção da área (1/4 da área, 50% do custo) porque ele também carrega
a leitura do quadro e a conversão para RGB565, que continuam em 640×480. Ou seja: o que sobra depois
da redução é, em boa parte, o caminho de apresentação — o mesmo que a frente 1 atacou no lado da
placa.

### A conferência da imagem

O desenho continua o mesmo, só menor. Comparando o relatório de 4 s nas duas pontas:

```text
nativo:  tela: 177 cor(es), dominante 0x220a
1/4:     tela: 177 cor(es), dominante 0x220a
```

Mesma contagem de cores e mesma cor dominante. Uma redução que quebrasse o desenho — HUD fora do
lugar, superfície não descoberta, leitura torta — apareceria aqui como queda de cores ou tela de uma
cor só.

### O que fica dito

- **Só o processador reduz.** Na placa, 640×480 não satura o Mali, e reduzir estragaria a imagem sem
  ganhar nada. A opção diz isso nos rótulos, e o código recusa em silêncio do lado da placa.
- **A redução é a alavanca de portátil mais direta que a rodada achou**: num aparelho fraco, 33% de
  tempo real é a diferença entre 30 e 45 quadros por segundo. É a primeira coisa a ligar lá — e o
  perfil `Portátil` deve passar a oferecê-la.
- O padrão continua 1×: quem não pede nada não perde nitidez.

## 29. Frente 10 fechada sem mudar código: o freio de velocidade entrega 1×

A frente pedia um PDCA do `sleep` dentro do `retro_run`, porque a revisão do PCSX-ReARMed mostrou
que, no caminho Libretro, **o relógio de parede é do frontend** — o `pl_frame_limit` de lá só marca a
fronteira emulada e não dorme. A suspeita era de dupla frenagem: o nosso `sleep` somado ao do
frontend.

### Como foi medido

O teste `a_abi_do_core_roda_uma_rom` roda `retro_run` em laço apertado, com um frontend de mentira
que **não espera retraço nem áudio**. Ali o nosso `sleep` é o único freio que existe — que é
exatamente o que se quer isolar. Acrescentei ao teste a medição do ritmo:

```text
ritmo: 120 quadro(s) — virtual 1983 ms em real 1985 ms (100% da velocidade),
       por quadro: mediana 16.59 ms, p95 17.62 ms, pior 18.60 ms
```

### O que o número diz

1. **100% da velocidade.** 1 983 ms de tempo virtual em 1 985 ms de tempo real, sem o frontend
   ajudar em nada. O freio acerta o alvo.
2. **Mediana 16,59 ms** por quadro, contra os 16,67 ms de um quadro a 60 Hz — um décimo de
   milissegundo de erro sistemático.
3. **p95 17,62 ms e pior 18,60 ms.** A irregularidade fica abaixo de dois milissegundos, ou 12% de
   um quadro. Não é fonte de judder por si só.

### Por que a dupla frenagem não acontece

O `sleep` é um **piso, não um freio a mais**. Ele só dorme o que o relógio virtual está adiantado em
relação ao real (`ahead_ms`), e esse adiantamento é *líquido*: se o frontend já segurou a chamada
por 16 ms, o tempo virtual avançou junto e o adiantamento é negativo — o `sleep` não acontece. Um
frontend que espera retraço não é punido duas vezes; um que não espera é segurado por nós.

### Conclusão

**Nada foi mudado.** A frente fecha com a medida que a própria frente pedia, e o teste fica com uma
prova de ritmo que antes não existia: qualquer mudança futura no laço do core que estrague a
cadência aparece ali como desvio de velocidade ou como p95 acima de dois milissegundos.

## 30. Frente 3: o quadro vai direto ao buffer que o frontend empresta

Frente que parecia depender de aparelho e não depende: `RETRO_ENVIRONMENT_GET_CURRENT_SOFTWARE_FRAMEBUFFER`
é verificável no desktop, porque o teste do core tem um frontend de mentira, e é ele que empresta
o buffer.

### O que mudou

Antes, o core montava o quadro num vetor próprio e o frontend copiava — 600 KB por quadro. Agora o
core **pede o buffer** e escreve onde o quadro vai ficar, entregando o ponteiro emprestado ao
`retro_video_refresh`, como a `libretro.h` exige (sem deslocamento).

Três cuidados, todos conferidos:

1. **O passo de linha é respeitado.** `Framebuffer::write_rgb565_with_pitch` escreve linha a linha
   no passo que o frontend deu, e o que sobra no fim de cada linha fica como estava.
2. **Formato e tamanho são conferidos antes de escrever.** A `libretro.h` diz que o frontend pode
   devolver outro formato; escrever RGB565 num buffer XRGB8888 daria imagem plausível e falsa.
   Qualquer recusa cai no caminho de sempre.
3. **O ponteiro vale só dentro da chamada**, e por isso o pedido e o uso ficam no mesmo lugar, sem
   guardar nada entre quadros.

### A prova

O frontend falso da suíte passou a emprestar um buffer com **folga de passo** (64 bytes) — de
propósito: com passo igual à largura, quem ignorasse o `pitch` acertaria por sorte, e a folga é o
que faz a imagem sair torta se ele for ignorado. O teste confere que o buffer **não ficou apagado**,
isto é, que o core desenhou nele. E a medição de ritmo continua no mesmo teste:

```text
ritmo: 120 quadro(s) — virtual 1983 ms em real 1984 ms (100% da velocidade),
       por quadro: mediana 16.86 ms, p95 17.87 ms, pior 17.97 ms
```

### O que isso rende

Uma cópia de quadro a menos por quadro, no caminho de software. Não é o caminho dos dois portáteis
(lá é placa), mas é o de todo frontend que usa framebuffer de software — RetroArch com driver de
vídeo em software, outros frontends, e a própria suíte.

### Um defeito meu que a suíte pegou

A prova do contador de descartes do registro (`o_contador_de_descartes_aparece_e_zera`) falhou ao
rodar a suíte inteira: ela escrevia no nível `Informacao` sem fixar o nível ativo, e dependia do
que a prova anterior tivesse deixado. Passava até a ordem das provas mudar — que foi o que
aconteceu quando esta rodada acrescentou provas. Corrigida fixando o nível.

## 31. Fim da rodada: as treze frentes, cada uma com o seu desfecho

### Frente 13 — Android drena o registro (feita e verificada)

Faltava o destino: o desktop imprime no `stderr`, o core manda pelo `retro_log`, o headless imprime,
e o Android não tinha para onde. Agora drena no `log`, que o `android_logger` já instala, com a
gravidade preservada — um aviso do núcleo sai como `WARN` no `logcat`, o que faz `adb logcat *:W`
mostrar o que interessa.

**A verificação foi a CI do Android do fork**, disparada à mão (`gh workflow run android.yml`). Ela
não roda sozinha em push: o único gatilho automático do repositório é a tag. Foi ela que verificou
esta frente — e, antes disso, **pegou uma regressão minha que este host não tem como ver**.

### A regressão que a CI do Android pegou

`DebugView` deixou de ser `Copy` quando ganhou o campo de texto do nível de registro, e o frontend
Android faz `let debug = self.settings.debug;` — move de um tipo que já não é `Copy`. O pacote
`zeebx-android` **não compila fora de um alvo Android** (`ndk-sys` recusa: *"only supports compiling
for Android"*), então `cargo check`, `cargo test` e `cargo build` no desktop não veem esse erro. Ele
ficou no repositório desde a instrumentação e só apareceu quando alguém compilou para Android.

Conserto: `clone()` no ponto do movimento — o mesmo que já havia sido feito no desktop, e na mesma
classe de struct (meia dúzia de campos, uma vez por desenho do painel de depuração).

**A lição vale para a rodada inteira:** o único frontend que a suíte local não alcança é o que mais
precisa de CI, e essa CI é manual (`workflow_dispatch`). Quem mexer em `ui::settings` tem de
disparar `android.yml` à mão — nada avisa.

### Frente 12 — descarte de tiles (entregue como experimento, **não medido**)

Opção nova, `zeebx_descarte_de_tiles`, **desligada por padrão**. Ligada, o rasterizador de placa diz
ao driver que profundidade e estêncil podem ser jogados fora depois do quadro — o que num GPU de
tiles evita escrever os dois anexos de volta na memória.

**Não é ganho medido, e não está contado como tal.** Duas razões para vir desligada:

1. um jogo que **não** limpe a profundidade de um quadro para o outro conta com ela — o console é um
   framebuffer de verdade, e lá a profundidade persiste;
2. o ganho é do Mali, onde eu não posso medir.

O que a entrega faz é **tornar o teste possível**: a opção existe, está rotulada como experimental,
e a primeira coisa a fazer no aparelho é ligá-la e olhar a imagem e o relógio.

### Frente 11 — `lto = "fat"` (fechada por decisão, com o motivo)

Não vale o preço, e o motivo é medível sem rodar:

- o código quente **não é Rust de outro crate**. O JIT é C++ (Dynarmic, fora do alcance do LTO do
  Rust) e o rasterizador de software está no próprio `zeebx`;
- `codegen-units = 1` já está ligado, então a inlining dentro do crate já é máxima;
- o `thin` já faz inlining entre crates do que é pequeno;
- o custo é um rebuild do LTO gordo, que recompila **todas** as dependências, com 3,4 GB livres.

Fica registrado como a coisa a testar quando houver disco — e não como pendência esquecida.

### Frente 6 — PBO com fence (sem razão depois da frente 1)

O PBO existe para esconder a latência do `glReadPixels`. Depois da frente 1, o caminho de placa faz
**uma** leitura por sessão, e não uma por quadro: não há latência a esconder. Fechada.

### Extras vindos dos estudos, implementados nesta rodada

- **`GET_CURRENT_SOFTWARE_FRAMEBUFFER`** (do PCSX-ReARMed) — frente 3, feita e provada.
- **Perfil Portátil ligando `0,5x`** — o item de desempenho que faltava no perfil. É a recomendação
  da seção 28, agora de fábrica: quem escolhe Portátil ganha a redução que mediu 22% menos tempo
  real, e quem não gostar da imagem mais quadrada escolhe a opção à mão.
- **Teste do blit do driver** (do Flycast) — frente 5.
- **Espelho de estado** (do DuckStation e do Flycast) — frente 2.
- **Readback preguiçoso** (do Flycast e do DuckStation) — frente 1.

### O placar das treze

| desfecho | frentes |
|---|---|
| feitas e verificadas | 1, 2, 3, 5, 7, 9, 10, 13 |
| entregue como experimento, não medida | 12 |
| fechada por decisão, com motivo | 11 |
| sem razão depois de outra frente | 6 |
| despriorizadas por decisão registrada | 4, 8 |

**Oito das treze com medida.** As cinco restantes têm desfecho escrito, e não ficaram em aberto por
esquecimento: três dependem de aparelho ou de disco, e duas foram despriorizadas pelos próprios
adendos das revisões.

### O que ficou pendente de verdade

1. **Medir no aparelho** — `0,5x` no perfil, o descarte de tiles, a partilha JIT × despacho e a
   contagem de leituras de quadro. O instrumento viaja em todas elas (`Session::leituras_do_quadro_gl`,
   `Session::estado_enviado_e_poupado`, e o relatório de varredura).
2. **Triagem dos seis casos da varredura** — segue sem ser feita.
3. **PR de `perf-v0.3.0` para `development`** — a branch está no fork e não há PR aberto.
4. **`lto = "fat"`** quando houver disco.

## 32. A varredura não é determinística — e o critério de aceitação repousa nela

O critério 2 de aceitação desta rodada diz: *"a varredura dos 62 jogos não regride comportamento"*.
Ao rodar esse critério pela primeira vez com os patches aplicados, o resultado foi **32 de 63 ROMs
com diferença** — e antes de atribuir isso aos patches, fui verificar se a varredura é
reprodutível.

**Não é.**

### A medida

O mesmo binário, a mesma ROM (Action Hero 3D), dois relatórios seguidos:

| | voltas | instruções | chamadas de API | tempo virtual |
|---|---:|---:|---:|---:|
| rodada A | 216 | 1 052 233 | 46 145 | 6 002 ms |
| rodada B | 216 | **1 052 127** | **46 137** | 6 002 ms |

**Mesmo número de voltas, mesmo tempo virtual, e 106 instruções e 8 chamadas de diferença.** O
relatório completo difere em **48 linhas**, e o padrão das diferenças é inconfundível: os mesmos
eventos com endereços de heap deslocados 64 bytes —

```text
-   OpenFile "a3d_sound_effect_04spf.wav"  (0x8c0d8 0x1 0xf0003008) -> 805311000
+   OpenFile "a3d_sound_effect_04spf.wav"  (0x8c0d8 0x1 0xf0003008) -> 805310936
```

Uma alocação a menos no começo desloca tudo o que vem depois. Contra a linha de base, isso vira 814
linhas removidas e 740 adicionadas — que o teste lê como regressão de comportamento.

### O que isso significa

1. **O critério 2 não pode ser cobrado hoje.** Ele acusa ruído como regressão, e um teste que fica
   vermelho sozinho não guarda nada — a mesma lição do `os_dois_rasterizadores_desenham_o_mesmo_quadro`,
   que já falhava no Need for Speed e ninguém sabia porque a CI não tem ROMs.
2. **O quadro "reproduzível bit a bit" tem uma exceção não documentada.** O `ARCHITECTURE.md`
   afirma que duas execuções iguais desenham os mesmos pixels — e elas desenham, porque o desenho
   é função do estado; mas o **estado do guest** já não é o mesmo em duas execuções, e é isso que
   as instruções a mais mostram.

### O que já foi descartado como causa

- **`aee_GetRand`**: é um LCG com semente fixa (`0x1234_5678`) e entra no save state. Determinístico.
- **`aee_GetTimeMS` / `aee_GetUpTimeMS` / `aee_GetSeconds`**: leem o relógio **virtual**, que sai das
  instruções. Determinístico.
- **Ordem de `HashMap` do host chegando ao guest**: `timers` e `pending_calls` são `Vec` (ordem de
  inserção); `threads` e `resume_callbacks` só são acessados por chave. O `call_log` itera um
  `HashMap`, mas é ordenado antes de sair no relatório.

### A próxima etapa, concreta

A divergência é de **106 instruções em 1,05 milhão** com as mesmas 216 voltas. O caminho é
binário: comparar, entre duas execuções, o *número de instruções por volta* e achar a primeira volta
em que os dois números se separam; dali para trás, o evento que decidiu diferente. O instrumento
existe — `Machine::advance` já conta voltas, e `instructions()` já conta instruções.

Suspeito natural, e não conferido: o **adiantamento de relógio por espera** (`note_spin`, em
`machine/time.rs`), que decide *quando* uma volta de laço é espera e adianta o relógio. É a única
peça do emulador que olha para um laço de chamadas em vez de para uma chamada, e um limiar que
dependa de algo fora do estado do guest explicaria exatamente um desvio pequeno e raro.

### O que ficou decidido

O critério 2 fica **suspenso**, e não cumprido: não por os patches terem regredido — nada indica
isso, e as diferenças são de endereço — mas por a régua não medir o que promete. Antes de cobrar
"não regrediu" de uma varredura, ela precisa ser reprodutível, ou a comparação precisa ignorar os
campos que carregam endereços.

O que **pode** ser cobrado hoje, e foi conferido nesta rodada: o **estado** de cada ROM bate com a
linha de base — 63 ROMs, e as categorias são as mesmas (56 rodam, 3 não criam o applet, 2 quebram no
laço, 2 terminam sozinhas). A diferença está no log de eventos, não no desfecho.

## 33. A varredura é determinística — o que não é é o cache de extração

Continuação da seção 32, e o resultado é melhor do que ela dizia. **O emulador é determinístico.**
O que muda a execução do guest é o **estado do cache de extração**: frio (extraindo agora) contra
quente (reaproveitando a extração que já está lá).

### As medidas

| rodada | cache | instruções | chamadas de API |
|---|---|---:|---:|
| quente 1 | reaproveitado | 760 815 | 46 137 |
| quente 2 | reaproveitado | **760 815** | 46 137 |
| quente 3 | reaproveitado | **760 815** | 46 137 |
| frio | recém-extraído | **760 921** | 46 145 |

Três rodadas em cache quente dão **o mesmo número de instruções, dígito por dígito** — 760 815 nas
três. O cache frio dá 106 instruções a mais, uma chamada `CreateInstance` a mais e os endereços de
heap deslocados 64 bytes.

**Determinismo, então, está de pé:** o mesmo estado de cache dá o mesmo resultado. O que não estava
era o controle da variável.

### As duas linhas de tempo

Entre duas rodadas quentes, o relatório difere em **seis linhas** — e todas as seis são do bloco de
desempenho:

```text
-  rodou 0.8 s reais para 6002 ms virtuais (754% da velocidade do console)
+  rodou 0.8 s reais para 6002 ms virtuais (757% da velocidade do console)
-  1052127 instruções (1323430/s), 46137 chamada(s) de API
+  1052127 instruções (1328443/s), 46137 chamada(s) de API
```

São relógio de parede, e não têm o que fazer num arquivo que existe para guardar comportamento. O
resumo comparado (`Relatorio::resumo`) já os exclui — o que os guarda é o relatório completo, que
vai para `ZEEBX_ROM_SAIDA` e **não** é o comparado. Está certo como está; o que faltava era o
controle do cache.

### Por que a linha de base acusou 32 de 63

A rodada do corpus foi feita com um cache **misturado**: 37 entradas já existiam (quentes, de runs
anteriores) e o resto foi extraído na hora (frio). Cada ROM foi medida num dos dois estados, e a
linha de base foi gravada num deles. O resultado é 814 linhas removidas e 740 adicionadas — que o
teste lê como regressão, e não é: é a mesma execução com o heap deslocado.

### O que fazer antes de confiar na comparação

1. **Aquecer o cache antes de comparar.** Rodar o corpus uma vez sem `ZEEBX_ROM_BASE` (o que também
   grava o que falta), e só então rodar contra a linha de base. A primeira passada é o aquecimento;
   a segunda é a medida.
2. **Gravar a linha de base sempre na mesma condição**, e dizer qual é. A de `docs/varredura/` foi
   gravada quente ou fria — não está dito, e é isso que hoje não se sabe.

Fica registrado no procedimento da varredura, que é onde quem for repetir vai olhar.

### O defeito de fidelidade que sobra, e que é real

**O primeiro jogo aberto se comporta diferente dos seguintes.** Isso não é ruído de medição: é o
guest executando 106 instruções a mais por causa de algo que só existe no host. Os suspeitos, em
ordem:

1. **`.zeebx-pacote`, o manifesto, dentro da raiz do jogo.** É um arquivo *nosso*, escrito na pasta
   que o guest enxerga como raiz, e a enumeração de diretório **não o esconde** (`machine/file.rs`
   lista, ordena e deduplica, sem filtro). Um jogo que enumere a própria pasta vê um arquivo que não
   existe no console. Se ele estiver presente numa condição e ausente na outra, a diferença de 106
   instruções e uma `CreateInstance` está explicada.
2. **Ordem de escrita da extração**: mudaria a ordem de `read_dir`, mas a enumeração ordena antes de
   entregar — então não pode ser esta.

O caminho para fechar: abrir o mesmo jogo duas vezes com o cache forçado a cada estado e comparar o
rastreio das primeiras chamadas (`ZEEBX_ROM_TRACO`), que mostra onde as duas execuções se separam.
E, independente disso, **esconder o manifesto do guest é uma correção devida**: é um arquivo do host
numa pasta que o jogo considera dele.

### O critério de aceitação, com o veredito corrigido

Não está suspenso por o emulador ser não-determinístico — ele não é. Está **pendente de controle de
variável**: rodar o corpus com o cache num estado só. Com isso, a comparação volta a poder ser
cobrada, e as diferenças que sobrarem serão regressões de verdade.

### A hipótese do manifesto foi testada — e rejeitada

O `.zeebx-pacote` era o suspeito número um: um arquivo nosso na pasta que o guest enxerga como raiz,
sem filtro na enumeração de diretório. Foi escondido do guest (`machine/file.rs` passa a filtrá-lo) e
o experimento do cache foi repetido:

| rodada | cache | instruções |
|---|---|---:|
| frio | recém-extraído | **760 921** |
| quente 1 | reaproveitado | 760 815 |
| quente 2 | reaproveitado | 760 815 |

**A diferença continua a mesma — 106 instruções, 54 linhas.** O manifesto não é a causa.

O filtro **fica**, porque é correção devida por outro motivo: o jogo não pode ver na própria pasta um
arquivo que o console não tem. Mas ele não explica o que esta seção investiga.

### O novo suspeito, e o que o sustenta

**Leitura preguiçosa a partir do pacote.** Há dois caminhos possíveis para o guest ler um arquivo: da
árvore extraída (cache quente) ou direto do `.zip`, pela rota do `IUnzipAStream` — o próprio acervo
tem essa rota, e há código que extrai **só o arquivo pedido** em vez do pacote inteiro. Se o guest
recebe um fluxo de tipo diferente conforme o cache esteja frio ou quente, o caminho que o jogo toma
muda — e é exatamente isso que 106 instruções e uma `CreateInstance` a mais parecem ser.

O caminho para fechar: abrir o mesmo jogo com `ZEEBX_ROM_TRACO=1` nos dois estados de cache e
comparar o rastreio das primeiras chamadas — o ponto em que os dois se separam nomeia o mecanismo.

### O critério, revisado de novo

A parte que **depende de conserto no procedimento** continua valendo e já está no
`docs/implementacao/11-compatibilidade.md`: aquecer o cache antes de comparar. A parte que depende de
conserto no **código** continua aberta, e agora com um suspeito de outra natureza — não um arquivo
visível, mas um **caminho de leitura diferente**.

## 34. Os extras dos estudos: um a um, com o desfecho

Para não deixar nenhum extra em ambiguidade, o desfecho de cada um.

| extra | vindo de | desfecho |
|---|---|---|
| readback preguiçoso | Flycast, DuckStation | **feito** — 78 e 184 leituras viraram 1 |
| espelho de estado | DuckStation, Flycast | **feito** — 80% e 76% das chamadas de estado |
| buffer emprestado do frontend | PCSX-ReARMed | **feito** — frente 3 |
| prova do blit do driver | Flycast | **feito** — frente 5 |
| escala `0,5x` no perfil Portátil | Flycast/DuckStation (resolução) | **feito** — e medido em 22% |
| coalescer strips adjacentes (restart de primitiva) | Flycast | **sem ganho para nós**: o nosso lote já junta desenhos **consecutivos de mesmo estado** numa chamada só. O truque do Flycast junta strips **dentro** de um desenho, e o guest do Zeebo emite um `DrawArrays` por strip — que já cai no mesmo lote. Não há o que juntar |
| frameskip reagindo a "fila de GPU cheia" | Flycast | **precisa de telemetria de GPU**, que não temos. Fica como pista |
| apresentação numa thread separada | DuckStation (e o adiamento dela) | **não fazer** — o core Libretro entrega um quadro por `retro_run`, e o desktop pinta depois da nossa volta; a thread custaria sincronização sem tirar trabalho do caminho |
| PBO com fence | DuckStation | **fechado** — sem razão depois da frente 1 |
| cache binário de shader | Mupen64Plus-Next | **não se aplica** — o motor tem um shader |
| caminho GLES 2 separado | PCSX-ReARMed | **compatibilidade, não desempenho**; fica para quando houver aparelho que precise |
| `AHardwareBuffer`/`EGLImage` | Mupen64Plus-Next | **Android e coerência de memória**; hoje não temos caminho zero-copy que o justifique |
| NEON nos laços largos | PCSX-ReARMed, Flycast | **despriorizado pelos adendos** — pouca transferibilidade confirmada |
| fastmem próprio / dynarec | Flycast, DuckStation, PCSX | **despriorizado pelos adendos** — o Dynarmic já entrega tabela de páginas, e o que resta é SMC medido |

### O que os logs do RG40XX-H (muOS) já provaram

Medido no aparelho, em 2026-09-24, com o log do núcleo em `depuracao`:

```text
Zeebx: GL real vendor=ARM; renderer=Mali-G31; version=OpenGL ES 3.2 v1.r20p0-01rel0.…
       GLSL=OpenGL ES GLSL ES 3.20
Zeebx: o frontend aceitou render em hardware (OpenGL 3.0); o desenho passa a ser na placa
Geometry: 640x480, Aspect: 1.333
Set video size to: 640x480
```

1. **O driver é o libMali r20p0, não Panfrost** — é a verificação que esta seção pedia e que não
   existia. Com ela, as conclusões abaixo deixam de ser suposição.
2. **A prova do blit passou.** Não há nenhum aviso de `glBlitFramebuffer` no log: neste driver o
   antialias e a resolução interna **não** foram desligados pela frente 5.
3. **O contexto que chega ao núcleo é GL 3.0**, embora o driver anuncie ES 3.2. Fica anotado como
   suspeito do custo: contexto mais baixo costuma fechar caminhos rápidos do driver.
4. **A tela do aparelho é 640×480**, e o RetroArch apresenta nesse tamanho: não há escala
   envolvida, então o que se mede ali é rasterização pura.

E o que o usuário mediu à mão: **reduzir a resolução pela metade e desenhar no processador foi
mais rápido do que no caminho de placa** com as mesmas opções. É essa a diferença que o cache de
programa/VAO/VBO acima veio atacar, e a comparação no aparelho está montada (dois `.so` na pasta
`ports`, e o `testa_zeebx.sh` aceita qual usar).
