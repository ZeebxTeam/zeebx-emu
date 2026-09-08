# 01 — Arquitetura

## A ideia em uma frase

O jogo é um binário ARM que nunca fala com hardware: ele chama vtables. O emulador executa esse
binário num núcleo ARM de verdade e **intercepta as chamadas**, atendendo cada uma no host. Não
há firmware do console envolvida em momento nenhum.

## O caminho de uma chamada

```
  jogo (ARM)                      emulador (Rust)
      │
      │ ldr r12, [vtable + 0x30]
      │ blx r12
      ▼
  0xf000_7038  ── endereço não mapeado ──▶  o núcleo para com FETCH_UNMAPPED
                                                    │
                                     aee::decode(0xf000_7038)
                                                    │
                                        (Interface::Bitmap, slot 14)
                                                    │
                                          Machine::dispatch
                                                    │
                                     bitmap_call("SetTransparencyColor")
                                                    │
                                      r0 = resultado, pc = lr, continua
```

O endereço **não existe**: é uma falha de busca de instrução tratada como chamada. É isso que
dispensa qualquer código ARM de cola — o detalhe está em [03-despacho-de-api.md](03-despacho-de-api.md).

## Os módulos

### Núcleo de execução

| Arquivo | Papel |
|---|---|
| `cpu/mod.rs` | `CpuBackend`, a fronteira com o núcleo ARM: registradores, memória, `run` |
| `cpu/unicorn.rs` | A implementação sobre o unicorn-engine, configurada como ARM1176 |
| `mem.rs` | O mapa de memória do guest, em regiões nomeadas |
| `loader.rs` | Monta o ambiente do módulo e chama `AEEMod_Load` |
| `modfile.rs` | Parser do `.mod` |
| `machine.rs` | O laço: roda, atende a chamada, continua. É onde vive quase toda a API do BREW |

### API do BREW

| Arquivo | Papel |
|---|---|
| `aee.rs` | O trampolim: converte endereço ↔ (interface, slot) |
| `aee_slots.rs` | O nome de cada método, na ordem em que ocupa a vtable |
| `aee_helpers.rs` | A tabela da stdlib do BREW (`memcpy`, `malloc`, `sprintf`…) |
| `objects.rs` | Os objetos que entregamos ao jogo, com contagem de referências |
| `heap.rs` | O heap que o `malloc` do jogo consome |

### Saídas

| Arquivo | Papel |
|---|---|
| `display.rs` | O framebuffer e as operações 2D |
| `rasterizer.rs` | O OpenGL ES 1.1 em software |
| `gles.rs`, `atc.rs`, `paltex.rs` | Estado do GL e os formatos de textura comprimida |
| `audio.rs`, `wav.rs` | Mistura e decodificação de som |
| `input.rs`, `bindings.rs`, `gamepads.rs` | Entrada |

### Fora do emulador

| Arquivo | Papel |
|---|---|
| `session.rs` | Um jogo em execução, do arquivo aos quadros — o que a interface usa |
| `ui.rs`, `i18n.rs`, `settings.rs`, `library.rs`, `padview.rs` | A interface |
| `main.rs` | A linha de comando, e o `launch()` que abre a interface quando não há argumentos |
| `vfs.rs`, `archive.rs`, `miffile.rs`, `resfile.rs`, `icon.rs` | Arquivos e recursos |

## O laço

`Machine::advance(budget)` é uma volta. Ela:

1. **adianta o relógio** se o jogo só está esperando (`skip_idle_time`);
2. **dispara os temporizadores vencidos** — tirando-os da lista *antes* de chamar o callback,
   porque o callback tipicamente rearma o timer e o rearmado não pode disparar no mesmo quadro;
3. **entrega os callbacks e as threads pendentes**.

Uma volta **não é um quadro**. O jogo pode rodar o laço principal inteiro dentro de um callback,
e cada volta devolve apenas a fatia de instruções que coube. No Peteca são um milhão e meio de
voltas para algumas dezenas de quadros. Quem manda no redesenho é o `eglSwapBuffers` (3D) ou o
relógio real (2D).

## O emulador roda na linha da interface

Não há thread separada. O núcleo do unicorn não atravessa linhas de execução, e o
`Session::step` já devolve o controle a cada fatia de tempo real — que é o que mantém a janela
viva enquanto o jogo corre. Uma thread traria sincronização sem trazer nada em troca.
