# 01 — Hardware do Zeebo

Fonte primária: `vendor/tripleoxygen/doc/ZeeboDeveloperGuide0.97.pdf` (Zeebo Inc., 2009),
seção 2.1 e capítulo 3. Complementado por dumps e pesquisa do OpenZeebo.

## Especificações (do Developer Guide, verbatim resumido)

| Item | Valor |
|---|---|
| Chipset | **Qualcomm MSM7201A** + RTR6280 (RF) + PM7540 (PMIC) |
| CPU de aplicação | **ARM11 (ARMv6, ARM1136)** @ 528 MHz + **QDSP-5** |
| CPU de modem | ARM9 + QDSP-4 (integrados no MSM7201A) |
| GPU | **Adreno 130** (linhagem ATI/AMD Imageon Z430) + MDP |
| RAM | 128 MB DDR SDRAM + 32 MB DDR empilhado no MSM7201A |
| ROM | 1 GB NAND Flash |
| Vídeo | Composto (RCA), PAL-M/NTSC, **VGA 640×480, 4:3** |
| Áudio | MIDI (72 vozes, wavetable 512 kB, 44 kHz), MP3, PCM, ADPCM, CMX, QCELP |
| 3D API | **OpenGL ES 1.0+ Common Profile** (via extensão BREW) |
| 2D API | BREW 2D (IDisplay / IGraphics / IBitmap) |
| I/O | 3× USB 2.0 host (acessórios), 1× USB OTG mini-B (dev/serviço), SD, RCA |
| Rede | GSM/GPRS/EDGE quad band + UMTS/HSDPA/HSUPA tri band |
| SO | **BREW 4.0.2** (customizado) |
| ID do console | IMEI |
| Performance declarada | 1,6 M triângulos/s; fill rate texturizado 63 M px/s (2 texturas) |

Observação importante para emulação: **os gamepads são USB HID** e aparecem para o jogo via
`IHID` / `IHIDDevice` (botões, eixos, rumble, hot-plug) — não é um teclado BREW de celular.
O arquivo `vendor/tripleoxygen/content/hid_devices.cfg` e
`vendor/tripleoxygen/hardware/peripheral/joystick_descriptor.txt` descrevem o mapeamento real.

## Arquitetura gráfica

O Developer Guide (cap. 3) documenta o pipeline do Adreno 130:
- Renderização **em tiles/binning** (bin de geometria → render por bin), com *ring buffer* de
  comandos entre CPU e GPU;
- **MDP** (Mobile Display Processor) faz composição/scan-out concorrente com o 3D;
- Interfaces de memória (SMI interno vs EBI externo) com impacto grande em performance —
  irrelevante para HLE, relevante se algum jogo assumir tempos/latências.
- Extensões OpenGL ES suportadas e limitações estão nos capítulos 7.2 a 7.4 — inclui
  **ATITC** (compressão de textura ATI), matrix palette, point sprites, DrawTexture.
  As amostras oficiais estão em `vendor/sdk/samples/unpacked/MSM7500_OGLES_*`.

Para HLE, o que importa é a **API OpenGL ES 1.x + extensões**, não o Adreno em si:
a tradução natural no host é GL ES 1.1 → OpenGL/ANGLE, ou uma reimplementação de pipeline fixo
sobre GL ES 2/Vulkan. Texturas ATITC precisam de decodificação por software no host.

## Memória, boot e JTAG (material de baixo nível)

De `vendor/tripleoxygen/research/` e `vendor/tripleoxygen/dump/`:

- Cadeia de boot na NAND: **PBL (ROM) → QCSBL → OEMSBL1/OEMSBL2 → APPSBL → APPS/AMSS**.
  Todas as partições estão dumpadas em `dump/nand/1.1.2/partitions/` (firmware 1.1.2),
  com `md5.txt`.
- `research/Analysis 0x00000000.txt`: notas de RE do bootloader — offsets de QCSBL/OEMSBL,
  GPIO do painel EL frontal (`GPIO_OUT_0` 0xA9200800, bit 15, ativo baixo), detecção de SD card,
  protocolo de leitura da NAND por registradores (0xA0A00000/04/10/100), e — muito útil —
  o endereço `0x11428800` com a **lista de App IDs** do sistema
  (DEFAULT `0x1008000`, EM `0x1007001`, CONFTEST `0x1A2345F`, TECTOY `0x1070798`)
  e flags de modo de sistema em `0x114287F4/F5`.
- `research/memory_map.ods`: mapa de memória MSM7201A (derivado de HTC Dream/RAPH, mesma
  família de chipset) — SRAM/SMI em `0xA8000000`, blocos de periféricos, etc.
- `research/jtag arm11.txt`, `hardware/msm7201a_arm11.cfg`, `msm7201a_arm9.cfg`,
  `hardware/zeebojtag_r2.zip`: configs OpenOCD e adaptador JTAG para o console.
- `dump/ram/1.1.2/` e `dump/regs/`: dumps de regiões de RAM e de blocos de registradores
  (0x80000000, 0xA8600000, 0xB8000900, 0xFFFEF000).
- `research/tvenc/`: framebuffers capturados do encoder de TV (útil como referência de saída
  esperada: formato RGB565, resolução, overscan).

Esse material só é necessário se você quiser **LLE** (rodar a firmware real). Para HLE ele serve
como *ground truth*: dá para desmontar `APPS.bin` e ver como a Qualcomm implementou cada API.
