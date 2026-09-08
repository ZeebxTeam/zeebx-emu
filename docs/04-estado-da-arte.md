# 04 — Estado da arte

## Infuse (Tuxality) — o único emulador funcional

- Repositório: <https://github.com/Tuxality/Infuse> — **o repo está vazio** (só `LICENSE`,
  `README.md` e `.gitignore`; 1 KB, último push 31/03/2024). O código **não é público**;
  o GitHub serve como vitrine/issue tracker. A licença declarada pelo GitHub é "Other (NOASSERTION)".
- Site/blog: <https://tuxality.net/projects/infuse_zeebo_emulator>
- Arquitetura declarada pelo autor:
  - **HLE**: reimplementação do subsistema BREW escrita do zero, por engenharia reversa limpa;
  - CPU: **[dynarmic](https://github.com/merryhime/dynarmic)** (JIT ARM);
  - Gráficos: **OpenGL ES 1.x**;
  - Áudio: MIDI básico, PCM/ADPCM, MP3, mixagem de múltiplos streams com resampling
    independente do host; backends waveOut (Windows), Core Audio (macOS), PulseAudio (Linux),
    Media Kit (Haiku);
  - Entrada: HID, até dois gamepads;
  - Entrada de dados: `.mod` + `.mif` + `.sig`. **Não precisa de BIOS.**
  - Plataformas: Windows x86_64, Linux x86_64, macOS arm64, Steam Deck, Haiku, ARM (RG353v, exp.).
- Compatibilidade declarada: **jogáveis** — Double Dragon, Crash Nitro Kart 3D, Zeebo Family Pack.
  **Em progresso** — Quake II, Reckless Racing, série Asphalt, Kingdom Hearts V-Cast.
- Ferramenta aberta do mesmo autor: [`ggzbrewtools`](https://github.com/Tuxality/ggzbrewtools) (C++17, unpacker de assets GGZ).

O ponto: o Infuse **prova que HLE de BREW funciona**, e o release A1 (maio/2024) é a referência
de comportamento correto. Mas nada dele é reutilizável como código.

## tripleoxygen / OpenZeebo — a base de conhecimento aberta

Pesquisador brasileiro que fez o trabalho de hardware/firmware e publicou tudo:

- Arquivo completo: <https://www.tripleoxygen.net/files/devices/zeebo/> (espelhado em `vendor/tripleoxygen/`)
- [`openzeebo`](https://github.com/tripleoxygen/openzeebo) — apps BREW de exemplo (`intro`, `backup`)
  com Makefile, `.mif`, `.bar`, `.sig`; código ARM de JTAG; `zloader` (bootloader alternativo,
  protocolo fastboot); `nand_util` (leitura/escrita de NAND em Python); `qcserial.py`; `ggzupk.py`.
- [`zeebo_doom`](https://github.com/tripleoxygen/zeebo_doom) — port do PrBoom 2.5.0 para Zeebo.
  **A camada de portabilidade é a melhor documentação viva de "qual API BREW um jogo real usa".**
- [`zeeutils`](https://github.com/tripleoxygen/zeeutils) — utilitário BREW de configuração no console.
- [`kernel_zeebo`](https://github.com/tripleoxygen/kernel_zeebo) — árvore de kernel Linux para o MSM do Zeebo
  (tentativa de rodar Linux; útil como referência de registradores/periféricos do MSM7201A).

## Preservação de software

Coleções públicas no Internet Archive (usar como fonte de material de teste):

- <https://archive.org/details/zeebo-romset-and-devtools> — ~945 MB, 68 jogos + ferramentas de dev
- <https://archive.org/details/zeebo-game-app-compilation-open-zeebo>
- <https://archive.org/details/zeebo-games-app-compilation>

A biblioteca comercial do Zeebo tem ~57–68 títulos. Como a loja era digital e os servidores da
TecToy saíram do ar, o que existe hoje veio de dumps de consoles físicos feitos pela comunidade
(grupo Zeebo Club, GBAtemp, OpenZeebo).

## Comunidade / threads úteis

- GBAtemp — [Why is not there a Zeebo Emulator for PC?](https://gbatemp.net/threads/why-is-not-there-a-zeebo-emulator-for-pc.529860/)
- GBAtemp — [Jailbreak / 61u.key](https://gbatemp.net/threads/im-trying-to-figure-out-a-more-practical-and-easy-way-to-unlock-jailbreak-zeebo-consoles.653809/)
- GBAtemp — [Every game ever made for the Zeebo](https://gbatemp.net/threads/every-game-ever-made-for-the-zeebo.635635/)
- ResetEra — [Finally Zeebo emulation starts to get somewhere](https://www.resetera.com/threads/finally-zeebo-emulation-starts-to-get-somewhere-infuse-brew-emulator.749431/)
