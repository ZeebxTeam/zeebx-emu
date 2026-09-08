# 03 — Formatos de arquivo e segurança

## Anatomia de um título (observado em `roms/Z-Wheel`)

O Z-Wheel — o menu/shell do console, App ID **274755** — tem a estrutura padrão de um app BREW
do Zeebo:

```
mif/274755.mif              <- Module Information File (metadados + ícones)
mod/274755/                 <- diretório do módulo, nome = App ID
    tectoy.mod              <- binário ARM relocável (o "executável")
    tectoy.sig              <- assinatura / autorização de execução
    tectoy.mif              <- referenciado de dentro do .sig
    tectoy.bar / *.brf      <- recursos (strings, imagens, i18n: _pt, _es, _esmx)
    *.qxt / *.qxm / *.qxa   <- assets do QXEngine (texturas, modelos, animações)
    *.bmp *.png *.gif *.mp3 *.wav
    preload.xml, uiconfig.xml, tectoy.cfg, tt_prefs.db, rootca.cer
```

Note `rootca.cer` e `tt_prefs.db`: o Z-Wheel falava com os servidores da TecToy (loja/OTA),
que estão fora do ar. Um emulador precisará **stub/mock dessa camada de rede** para o menu
não travar — ou um servidor local que finja ser a loja.

### `.mod` — módulo BREW

Observado no cabeçalho de `tectoy.mod`:

```
00000000: 0e00 00ea  0f00 00ea   b <entry> ; b <entry2>   (2 branches ARM)
00000008: "BREW"                 magic
0000000c: 01 00 00 00            versão do formato
00000010: 40 00 00 00            offset 0x40 (início do código real / AEEMod_Load)
00000014: 01 01 00 00
00000018: 00 02 00 00            0x200
0000001c: f0 fd 08 00            0x0008FDF0  (tamanho de código+dados)
00000020: 20/a0/40/90            tabela de offsets internos
00000034: f0 fd 08 00            repete 0x0008FDF0
00000040: 17 40 2d e9 ...        primeiro código ARM  -> AEEMod_Load
```

Ou seja: `.mod` **não é ELF**. É o produto do `elf2mod` da Qualcomm, que pega um ELF ARM
(`.text` baseado em 0, `.data`/`.bss` logo após, `AEEMod_Load` como primeiro símbolo do `.text`,
linkado com `-Wl,--emit-relocs`) e gera um binário **auto-relocável** com tabela de relocações
embutida. O carregador do emulador precisa: mapear o blob, aplicar as relocações para a base
escolhida e saltar em `AEEMod_Load`.

O **linker script oficial** (`vendor/sdk-brew/toolset/…/bin/elf2mod/src/gnu/elf2mod.x`) confirma
o layout esperado pelo `elf2mod`:

```
. = 0x0;                        /* ro-base em zero */
__module_start__ = .;
.text : { "*.o"(.text.AEEMod_Load)   /* AEEMod_Load forçado no início */
          *(.text .text.* ...) }
PHDRS { ER_RO PT_LOAD; ER_RW PT_LOAD; }
```

O próprio `elf2mod.exe` está em `vendor/sdk-brew/toolset/…/bin/` — dá para rodar sob Wine e,
principalmente, **desmontar para descobrir o formato exato da tabela de relocação** do `.mod`,
que é o único ponto do formato que ainda é inferência.

Referência de toolchain: o [BREW Development Guide do usernameak](https://gist.github.com/usernameak/f7315e9a00ebc4febac5a16687b2409e)
descreve o fluxo `arm-none-eabi-gcc -mword-relocations -fshort-wchar ... → elf2mod → .mod`,
que é o caminho inverso — útil para produzir **binários de teste próprios** com fonte conhecida.

### `.mif` — Module Information File

Binário com tabela de offsets no início, seguida de records. Em `274755.mif` dá para ver
`image/jpg` e um JPEG embutido logo em seguida (ícone/boxart), além de referências de applet
(ClassID, tipo, flags) e privilégios. O AEE lê o `.mif` **antes** de carregar o `.mod` para
saber quais applets o módulo expõe.

### `.sig` — assinatura

`tectoy.sig` (2712 bytes) é legível em parte:

```
0100 0200 8004 4400 ...  "Zeebo, Inc."  ...  "tectoy.mif"
```

É a autorização de execução emitida para aquele módulo/dispositivo. O **Infuse exige `.mod`, `.mif`
e `.sig`** juntos, mas por ser HLE ele provavelmente não valida criptografia — só usa os metadados.
Um emulador HLE pode simplesmente **ignorar a verificação**; isso não é um obstáculo.

### `.bar` / `.brf` — recursos

`.bar` é o resource file padrão BREW (strings, bitmaps, i18n) gerado pelo BREW Resource Editor.
No Zeebo aparecem também `.brf` por idioma (`tectoy_pt.brf`, `tectoy_es.brf`…).

### `.qxt` / `.qxm` / `.qxa` — QXEngine

Assets do **QXEngine**, a engine 3D da Qualcomm distribuída com o SDK do Zeebo
(`QXBinaryDistribInstaller.msi`, em `vendor/sdk/zeebo-sdk-1.2.4/`). Textura, modelo e animação.
Se um jogo usa QXEngine, ele carrega uma **biblioteca binária** — que também precisa rodar
sobre a sua reimplementação de BREW (ela é código ARM comum, então roda no JIT sem tratamento
especial, desde que as APIs BREW abaixo dela existam).

### GGZ — arquivos de asset comprimidos

Vários jogos (Double Dragon, Sonic BREW, DMC) empacotam assets em GGZ. Existe unpacker aberto:
[`Tuxality/ggzbrewtools`](https://github.com/Tuxality/ggzbrewtools) (clonado em `vendor/repos/ggzbrewtools`)
e um script Python em `vendor/repos/openzeebo/tools/ggzupk.py`.

## Segurança do console (só importa para rodar no hardware real)

- Cada Zeebo tem um **`61u.key`** único (14 caracteres alfanuméricos) que ativa o *Rear Diagnostic
  Port* / modo dev via SD card. Era gerado por uma ferramenta interna Qualcomm/TecToy a partir do
  IMEI; essa ferramenta nunca vazou. Sem ela, o único caminho conhecido é **soldar JTAG** e extrair
  a chave do console (adaptador e configs OpenOCD em `vendor/tripleoxygen/hardware/`).
- Assinatura digital de código: `vendor/tripleoxygen/doc/CodeAuthOnBrewMPThruDigitalSigning.pdf`.
- Cadeia de boot e patches de bootloader assinado: `vendor/tripleoxygen/mod/dwnmode_bl/` e
  `mod/nand/mod_1.1.2_OEMSBL2_*` (OEMSBL2 modificado para habilitar UART/GPIO e desabilitar MPU da NAND).

**Nada disso bloqueia um emulador.** É relevante só para dumpar jogos de um console físico ou
rodar homebrew no hardware.
