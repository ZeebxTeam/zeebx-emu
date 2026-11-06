# Core Libretro e muOS

O core Libretro é empacotado com o `.info` e pode ser instalado no RetroArch. O cartão muOS usa o
core AArch64 em `opt/muos/share/core` e o banco MIDI é opcional. A instalação documentada está em
[`libretro/LIBRETRO_PLAN.md`](libretro/LIBRETRO_PLAN.md). A playlist, o DAT e as capas de
Zeebo são gerados pelas ferramentas da pasta `ferramentas/`.

Para compilar o core:

```bash
cargo build --release -p zeebx-libretro
```

O core não linka a interface desktop nem bibliotecas de áudio/controle do host. Mais detalhes em
[COMPILANDO.md](COMPILANDO.md#libretro).

A listagem de compatibilidade do [COMPATIBILIDADE.md](../COMPATIBILIDADE.md) pode não se aplicar ao
Libretro. Reporte os problemas que encontrar.