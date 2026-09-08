# Zeebx — documentação de pesquisa para um emulador de Zeebo

Levantamento do que existe hoje, publicamente, sobre o Zeebo (TecToy, 2009) e sobre a
plataforma Qualcomm BREW / MSM7201A que ele usa — com o material relevante espelhado
localmente em [`vendor/`](vendor/).

## Conclusão curta

Emular Zeebo **não é emular um console**, é **reimplementar o Qualcomm BREW 4.0.2**.
O jogo é um binário ARM (`.mod`) que não fala com hardware: ele chama interfaces COM-like
(`IShell`, `IDisplay`, `IGraphics`, `IHID`, `ISound`, `IFileMgr`, OpenGL ES via extensão BREW).
Isso favorece **HLE** — executar o código ARM do jogo num interpretador/JIT e interceptar as
chamadas de API, traduzindo para o host. É exatamente o caminho do Infuse, do Tuxality.

O material público é surpreendentemente bom: SDK oficial do Zeebo, Developer Guide,
referência de OEM API do BREW, dumps completos da NAND (bootloaders + AMSS + APPS),
dumps de RAM/registradores e pesquisa de vtables das interfaces BREW na firmware real.

Esta série `0x` é **pesquisa**: o que se sabe do console e da plataforma. Como o emulador está
escrito está em [`implementacao/`](implementacao/).

## Índice

| Doc | Conteúdo |
|---|---|
| [01-hardware.md](01-hardware.md) | Especificações do console, MSM7201A, Adreno 130, memória, JTAG |
| [02-plataforma-brew.md](02-plataforma-brew.md) | BREW 4.0.2, AEE, modelo de execução, APIs que importam |
| [03-formatos-e-seguranca.md](03-formatos-e-seguranca.md) | `.mod`, `.mif`, `.bar`, `.sig`, `.bid`, GGZ, assinatura, `61u.key` |
| [04-estado-da-arte.md](04-estado-da-arte.md) | Infuse, OpenZeebo, ports homebrew, romsets, comunidade |
| [05-estrategia-de-emulacao.md](05-estrategia-de-emulacao.md) | Arquitetura proposta, fases, riscos, decisões técnicas |
| [06-fontes.md](06-fontes.md) | Todas as fontes com link |
| [07-inventario-vendor.md](07-inventario-vendor.md) | O que foi baixado e onde está |
| [implementacao/](implementacao/) | **Como o emulador é feito por dentro** — módulo a módulo, com as decisões e o porquê de cada uma |

## Material de teste

`roms/` guarda os títulos usados no desenvolvimento (também fora do git). Já posicionado:
**Z-Wheel** (App ID `274755`), o shell/menu do próprio console — ver
[03-formatos-e-seguranca.md](03-formatos-e-seguranca.md) para a anatomia dele.

## Aviso

`vendor/` contém material de terceiros (documentação Qualcomm/Zeebo confidencial na origem,
dumps de firmware, SDKs) espelhado para pesquisa e preservação. Não redistribua; e não commite
essa pasta num repositório público — o `.gitignore` já a exclui.
