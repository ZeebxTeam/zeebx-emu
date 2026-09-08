# Inventário de `docs/vendor/`

Material de terceiros espelhado localmente (~1,3 GB). **Não versionado** — veja o `.gitignore`.

## `tripleoxygen/` — espelho de <https://www.tripleoxygen.net/files/devices/zeebo/>

| Pasta | Destaques |
|---|---|
| `doc/` | **ZeeboDeveloperGuide0.97.pdf**, **BREWOemAPIReferenceforMSM.pdf**, Inside BREW, BREW Programming Guide, TRMs ARM1136/ARM926, palestras BREW+OpenGL ES, CodeAuth por assinatura digital, **ZeeboSDKPackage-0.93/1.2.4.zip** |
| `brew/` | Samples OpenGL ES 1.0.3 + headers da extensão, QLib, demos Qualcomm, `starting_brew.pdf`, `conftest.sig` |
| `dump/nand/1.1.2/partitions/` | **Firmware completa 1.1.2**: PBL-adjacentes, QCSBL, OEMSBL1/2, APPSBL, AMSS, APPS, MIBIB (+ md5) |
| `dump/ram/1.1.2/` | Dumps de RAM por região (0x80000000, 0xA0400000, 0xA8600000, 0xE0000000, 0xFFFF0000…) |
| `dump/regs/` | Dumps de blocos de registradores |
| `mod/` | OEMSBL2 patchado (UART/GPIO, MPU da NAND), `zloader` assinado |
| `research/` | **`brew/vtbl.ods` + `IFILEMGR_VTBL_Z200.txt` (vtables reais da firmware)**, `Analysis 0x00000000.txt`, `memory_map.ods`, `mainboard_pinout.ods`, `jtag arm11.txt`, `tvenc/` (framebuffers capturados), `peteca/` (saves) |
| `hardware/` | Configs OpenOCD MSM7201A (ARM11/ARM9), adaptador `zeebojtag_r2`, painel EL, `peripheral/joystick_descriptor.txt`, docs do controle Boomerang |
| `content/` | Pacotes do Z-Wheel (`274755*.7z`, incl. versão MX), `hid_devices.cfg`, `ids.txt`, `flixfile.dat` |
| `datasheet/` | NAND Hynix/Hynix MCP, TUSB2036 (hub USB), driver de painel EL |
| `driver/` | Drivers USB do console para Windows |
| `code/`, `homebrew/`, `media/` | Blinkenlichten, arte do OpenZeebo, pinout de UART, splash RGB565 |

## `repos/` — clones

| Repo | Nota |
|---|---|
| `openzeebo` | Apps BREW de exemplo, zloader, nand_util, `ggzupk.py`, JTAG ARM11 em asm |
| `zeebo_doom` | PrBoom portado (camada de portabilidade BREW real) |
| `zeeutils` | Só LICENSE no branch padrão |
| `ggzbrewtools` | Unpacker/packer GGZ (C++17 + Boost) |
| `kernel_zeebo` | Árvore de kernel Linux MSM (~900 MB) |

## `sdk/` — Zeebo SDK 1.2.4 extraído

- `zeebo-sdk-1.2.4/ZeeboSDKPackage-1.2.4/` — MSIs (`ZeeboSDKInstaller.msi`, `QXBinaryDistribInstaller.msi`
  = QXEngine), Adreno Profiler, extensão OpenGL ES 1.5.3 para BREW SDK 4.x, driver, Developer Guide.
- `samples/unpacked/MSM7500_OGLES_qcom_sdk_samples.release_02.15.07.beta1/` — **amostras oficiais
  em C** com `.mif`/`.bid`/`.mak`: `simple_vbo`, `simple_dot3`, `simple_atitc`, `simple_shadow`,
  `simple_spotlight`, `simple_point_sprites`, `simple_drawtexture`, `simple_matrix_palette`.
- `samples/unpacked_conftest/` — fonte do conftest.

### MSIs extraídos (via p7zip)

Fluxo usado: `7z x <installer>.msi` produz as tabelas MSI e um CAB de payload com nomes GUID;
extrair o CAB e renomear os arquivos pelo comentário `FILE:` / include guard de cada um.

- `zeebo-sdk-named/` — payload do **ZeeboSDKInstaller.msi**. É só o *delta* do Zeebo sobre o BREW
  SDK: a **extensão HID** (`AEEIHID.h`, `AEEIHIDDevice.h`, `AEEHIDButtons.{c,h}`,
  `AEEHIDThumbsticks.{c,h}`, `AEEHIDDevice_{Joystick,Keyboard,Mouse}.h`), mais recursos do
  BREW Device Configurator, bitmaps e DLLs do simulador. `AEEHIDButtons.h` é da **Tectoy Digital**
  e mapeia UIDs de botão para os IDs padrão do gamepad Zeebo.
- `qxengine-named/` — payload do **QXBinaryDistribInstaller.msi** (786 arquivos): 62 headers do
  **QXEngine** (`QXModel.h`, `QXRenderMgr.h`, `QXTexture.h`, `QXAnimation.h`, `QXMathFixed.h`,
  `QXPack.h`, `QXFileManager.h`…), 43 bibliotecas estáticas ARM (`ar archive`), samples completos
  (QCastle, CrateHunt, LightingSample, ProjectedShadow, AnimatedModel) e — importante —
  **`AEEModGen.c` e `AEEAppGen.c`** originais da Qualcomm, que documentam o ABI exato de
  `AEEMod_Load` / `IModule` / `AEEClsCreateInstance`.
- `msi-zeebosdk/`, `msi-qxengine/` — tabelas MSI cruas; `zeebo-sdk-files/`, `qxengine-files/` —
  payload com nomes GUID (mantidos para rastreabilidade).

## `brew-src/` — fontes e exemplos BREW extraídos dos RARs

- `43957791BREWSourceCode/BookDemo/Extension/AEE.h` — **`AEE.h` completo** (72 KB, era BREW 2.x)
- `78575279brewexamples/examples/` — exemplos BREW variados

## `sdk-brew/` — **BREW SDK 4.0.2 completo** (o que faltava)

Origem: [Zeebo SDK + BREW SDK 4.0.2 + BREW MP SDK — Internet Archive](https://archive.org/details/zeebo-sdk-brew-sdk-4.0.2-brew-mp-sdk)
(1,17 GB). `brew-sdk-402-bundle.zip` é o download bruto; `bundle/` é ele extraído.

| Caminho | Conteúdo |
|---|---|
| `brew-402/BREW 4.0.2 SP19/sdk/inc/` | **158 headers do BREW SDK 4.0.2**: `AEE.h`, `AEEShell.h`, `AEEDisp.h`, `AEEFile.h`, `AEEStdLib.h`, `AEEGraphics.h`, `AEESound.h`, `AEEMedia.h`, `AEEBitmap.h`, `AEECallback.h`… — **a versão exata que o Zeebo roda** |
| `brew-402/…/sdk/inc/gles/` | `egl.h`, `gl.h`, `glESext.h`, `glQUALCOMM.h`, `gles_1_0/`, `gles_1_1/` |
| `brew-402/…/inc/` | +146 headers e `.bid` (ClassIDs de todas as classes do sistema) |
| `brew-402/…/sdk/src/` | `AEEModGen.c`, `AEEAppGen.c`, `EGL_1x.c`, `GLES_1x.c`, `GLES_ext.c`, `mod_malloc.c`, `glibc_stubs.c`, thrdutil |
| `brew-402/…/documentation/API Reference/` | **Referência de API em HTML** (todas as interfaces, assinaturas e semântica) |
| `brew-402/…/bin/Simulator.exe` | Simulador BREW da Qualcomm (x86) — referência de comportamento das APIs |
| `brew-402/…/devicepacks/` | Device packs (perfis de aparelho para o simulador) |
| `toolset/…/bin/elf2mod.exe` | **O `elf2mod`** que gera os `.mod` a partir de ELF ARM |
| `toolset/…/bin/elf2mod/src/gnu/elf2mod.x` | **Linker script GNU oficial** — confirma `ro-base = 0x0`, `__module_start__`, `.text.AEEMod_Load` forçado no início, layout ER_RO/ER_RW |
| `bundle/BMP_SDKMP_7.12.5_SETUP_3.exe` | BREW MP SDK 7.12.5 (892 MB) — **ainda não extraído**, opcional |

### Pendente
Dois RARs usam método RAR5, não suportado pelo p7zip instalado
(`503611776Inside_BREW.rar`, `11715848BREWProgrammingguide.rar`). Para abrir:

```bash
brew install unar && unar -o brew-src docs/vendor/tripleoxygen/doc/503611776Inside_BREW.rar
```

(resolvido com `unar` — os dois arquivos já estão extraídos em `brew-src/`.)

O BREW MP SDK 7.12.5 dentro de `bundle/` continua sem extrair; só vale a pena se quisermos
comparar APIs entre BREW 4.0.2 e BREW MP.
