# Otimizações para handhelds fracos (RK3326/H700, Mali-G31)

Branch: `handheld-optimizations`. Aparelhos-alvo: R36S (RK3326, 4× Cortex-A35, Mali-G31
MP2, 1 GB) e RG40XX-H (H700, 4× Cortex-A53, Mali-G31 MP2, 1 GB).

A referência é o port nativo de Halo CE para o RK3326
([halo-ce-rk3326-Port](https://github.com/kattimasen-ops/halo-ce-rk3326-Port)), em especial a
[história de otimização com números por passo](https://github.com/kattimasen-ops/halo-ce-rk3326-Port/blob/main/docs/PERFORMANCE.md)
e as [lições medidas no Mali-G31](https://github.com/kattimasen-ops/halo-ce-rk3326-Port/blob/main/docs/MALI-G31-NOTES.md).
A regra metodológica vem de lá e vale aqui: **cada técnica entra com número medido no
aparelho; comparação só dentro do mesmo passo; nada se extrapola de desktop.** É também a
regra da casa ("afirmação vem com medida").

## 0. Estado de partida (não refazer)

- `target-cpu=cortex-a35` no `.so` linux-aarch64 (`libretro.yml`) — o mesmo tuning do port.
- Batching de draws adjacentes (`src/video/gpu.rs`, lote de 16384 floats; ~370 lotes/quadro no
  Quake) — o paralelo do `HALO_BATCH_QUADS` deles.
- Espelho de estado (−80% envios) — o paralelo da redução de state-changes.
- Readback preguiçoso (`materializa_quadro_gl`, `src/session.rs`; o `present_gl` só marca
  pendente) — a filosofia do readback assíncrono deles.
- Programa/VAO/VBO únicos cacheados, anel de VBO, cache de uniforms com dirty-bit, relógio
  amostrado 1/64.
- Leitura RGB565 direta quando o driver aceita (`frame_rgb565`); descarte de depth/stencil
  pós-quadro (`define_descarte_de_tiles`).
- LTO thin + `codegen-units=1` no release.

## Fase 0 — instrumento e bancada (pré-requisito de todo o resto)

**0.1 Telemetria por segundo** (`docs/OPTIMIZING_V0.3.0.md §6.3`, pendente). Estender
`leituras_do_quadro_gl` e `estado_enviado_e_poupado` (`src/session.rs`) com: tempo guest vs
`gles_draw`, draws/vértices por quadro, `glReadPixels` (contagem + bytes), memset/conversão,
áudio, skips. Uma linha por segundo no log; o `sessao` imprime o resumo. Critério:
`--seconds=60` relata sem mudar comportamento.

**0.2 Alavancas de diagnóstico** (todas desligadas por padrão, custo zero quando off —
instrumento caro contamina a medida: o par `Instant::now` já custou +40% uma vez). FPS e pior
quadro no relatório do `sessao`; log de hitch (quadros acima do limiar, com o que fizeram:
programas ligados, texturas decodificadas, geometria, leituras); estatística de emissores de
draw. Modelos: `HALO_FPS_LOG`, `debug.hitch_log`, `HALO_DEBUG_DRAW_CALLERS` do port.

**0.3 Bancada nos dois aparelhos** (pendente dos aparelhos — o roteiro fica pronto aqui, os
números vêm depois). Roteiro determinístico via `sessao --seconds/--keys`: Quake (3D pesado),
NFS (readback), Double Dragon (leve), Crash. Partir da mesma temperatura, anotar clocks (o
throttle a 70 °C move o resultado em vários fps, medido lá). Entrega: tabela-base por
aparelho. **É ela que decide o que a Fase 1 ataca.**

## Fase 1 — barato, alta confiança

**1.1 Cache de binário de programa.** `compila()/liga_programa` (`src/video/gpu.rs`) compila
e linka em runtime, sem cache. Guardar `glGetProgramBinary` em disco com chave = hash das
fontes + versão/renderer do driver (o método do port; link ~60 ms no Mali; cada programa novo
deles custava 220–250 ms). A chave inclui a variante de 2.1 quando ela existir. Critério: some
o engasgo do primeiro quadro 3D.

**1.2 Escala fracionária opt-in (ex. 0,75).** `define_escala` só aceita inteiro ≥1; o caminho
de redução já existe (`liga_para_leitura` faz blit LINEAR). Renderizar o 3D em 480×360 e
ampliar corta ~44% do trabalho de fragmento — a maior alavanca do port (pixels eram grande
parte do trabalho da GPU a 640×480). HUD em resolução cheia sobre o 3D reduzido, em vez de
forçar tudo a 640×480 + readback. Opt-in nas configurações + opção do core; padrão 1,0.

**1.3 Texturas 16-bit até a GPU.** Os tipos nativos (565/4444/5551) sobem nativos em vez
de RGBA8: metade do upload e da VRAM, com volta exata provada por teste exaustivo
(`volta_dos_canais_e_exata`) e fotos iguais a menos de 1 LSB (`formatos_compactos_sobem_sem_erro`
garante que o driver aceita os três). Medido: Crash 19 MB → 9,5 MB em 16-bit; Quake 15,6 MB
→ 6,3 MB. A telemetria conta os bytes compactos em separado. Comprimidos e paletizados
continuam subindo RGBA8 (decodificação direta para 16-bit fica para depois). Lição de
implementação: o interno do 4444/5551 muda entre desktop (`RGBA4`/`RGB5_A1`) e GLES
(0x8D64/0x8D65) — o desktop recusava em silêncio e a textura saía preta.

**1.4 PGO no `.so` + duelo thin vs fat.** Duelo medido no desktop (62 s virtuais: Double
Dragon, Crash, Quake): thin 1/3/16 s, fat 2/3/17 s, PGO com treino local 2/4/16 s — tudo
dentro do ruído, então fica o thin (o fat ainda custa 9 min de build e só emagrece 4 MB).
Nada disso responde no aparelho (é no A35 de cache pequeno que layout conta), então o treino
de verdade fica pendente dos handhelds. O que ficou pronto é o mecanismo, validado de ponta
a ponta com o toolchain da casa (`llvm-tools-preview`): build com
`RUSTFLAGS="-Cprofile-generate=/tmp/pgo-raw"`, treino (`sessao` do Quake gerou 36 MB de
`.profraw`), `llvm-profdata merge` e rebuild com `-Cprofile-use`. Receita do treino no
aparelho: mesmo roteiro da bancada 0.3, dez minutos de jogo real, juntar os `.profraw` e
rebuildar o `.so` — sem commitar perfil nem mudar `Cargo.toml` (só `RUSTFLAGS`).

## Fase 2 — só com a bancada apontando o limite

**2.1 Variante sem `discard` + opacidade na decodificação.** O `FRAGMENTO` tem
`if(!passa){discard;}` sempre — em GPU tiled isso desliga o descarte oculto de fragmentos.
Registrar na decodificação se todos os texels têm alfa 1 (custo ~zero, como no port) e
escolher a variante por lote. Custa um 2º programa + pressão de trocas — fazer junto com
2.3. Referência de ganho deles: 36→40 fps.

**2.2 Conversão de pixel com NEON.** `frame_rgb565` converte por pixel em closure quando o
driver não entrega RGB565, e aloca o índice de colunas por quadro (dá para içar). Com
`target-cpu=cortex-a35` o autovetor deve resolver; conferir o codegen, senão intrínsecos.
Precedente: o `fill_mem` vetorizado deu +16%.

**2.3 Ordenação global por shader.** O lote só junta adjacentes; ordenar os opacos por
(programa, textura, estado). O port cortou trocas de 190→109 — mas o fps só mexeu após outro
stall sair, por isso é Fase 2 e se mede pelas contadoras do Espelho.

**2.4 HUD sem readback total.** `quadro_na_placa` só vale com as escritas intactas
(`src/machine/gl.rs`); qualquer 2D materializa. Compor o HUD em cima do 3D ampliado sem volta
à CPU. Desenho detalhado após 1.2.

## Fase 3 — condicional, se o perfil pedir

**3.1 Uniform blocks** (só se o tempo de driver/draw no Mali for alto — a régua do port:
~17 µs/draw, 8 µs por bind, 4,5 µs por troca). Os uniforms individuais com dirty-bit já
cobrem o caso comum.
**3.2 Poda de outputs de vértice + mediump** (`VERTICE` tem saídas fixas hoje). Pequeno
sozinho; combina com as variantes de 2.1.
**3.3 Sampler objects** (um por configuração; hoje só há `sampler2D` declarados).
**3.4 Buffers mapeados de uma vez** (o anel remapeia ao encher; o port: mapear uma vez +
escrever menos).
**3.5 I/O de save fora do quadro** (o checkpoint de 16 MB do port: 80 ms→14 ms em memória).
Só se o hitch log acusar I/O em cartão FUSE/exFAT.

## Não-metas

Medido que não ajuda no port: `-O3` no guest, três framebuffers, 2 quadros à frente / GL
thread, fp16 como limite, banda de textura como culpada, adiar flushes (revertido lá).
Restrição nossa: `retro_run` é single-thread por contrato (thread de áudio já recusada);
pinar clocks no código não dá a partir do core (vira documentação); biblioteca nova de host
no core o `ldd` do `libretro.yml` barra; dependência GPL-2.0-only não entra (binários são
GPLv3 na prática).

## Roteiro da bancada (0.3) — validado localmente, números do aparelho pendentes

Quatro jogos, roteiro determinístico de toques (`b1` a cada 2500 ms a partir de 5000 ms),
caminho de software (determinístico em qualquer lugar, inclusive via SSH no aparelho):

```bash
KEYS=$(python3 -c "print(','.join(f'{t}:b1:200' for t in range(5000,32000,2500)))")
zeebx sessao "<jogo>.zip" --seconds=32 --keys="$KEYS" --telemetria   # Double Dragon, Crash
zeebx sessao "<jogo>.zip" --seconds=62 --keys="$KEYS" --telemetria   # Quake, NFS Carbon
```

Validação local (binário debug, desktop Mesa, software — **não** são números de handheld,
servem só para provar que o roteiro é determinístico e a telemetria fecha):

| Jogo | Desenhos | Vértices | Subidas | Maior emissor |
|---|---|---|---|---|
| Double Dragon (2 rodadas) | 9732, idêntico | 1878562, idêntico | 10 (4800 KiB) | `IGL::glDrawArrays` |
| Crash Nitro Kart 3D | 25166 | 1143165 | 111 (19507 KiB) | `IGL::glDrawElements` |
| Quake | 1808143 | 30895914 | 962 (15791 KiB) | `IGLES11::DrawArrays` |
| NFS Carbon | 49851 | 1535477 | 451 (16670 KiB) | `IGL::glDrawArrays` |

Dois fatos que o roteiro já mostra: jogos diferentes usam interfaces diferentes (`IGL` vs
`IGLES11` — os contadores pegam todas, porque o gancho é no `gles_draw`); e o Quake emite
~29 mil draws/s de leque face a face, que é o candidato natural da ordenação (2.3). Cada
roteiro precisa de validação de fluxo no aparelho (chegar ao gameplay representativo), com
temperatura e clocks anotados, antes de qualquer comparação entre passos.

## Regras da casa

Docs e commits em português, frase declarativa, sem prefixo de conventional commit;
`Cargo.lock` versionado (`--locked` no CI); `THIRD-PARTY-NOTICES.txt` acompanha dependência
nova; nada de `#[cfg(feature = "desktop")]` onde não é desktop — o core não linka host.
