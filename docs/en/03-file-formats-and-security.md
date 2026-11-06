# 03 — File formats and security

## Anatomy of a title (observed in `roms/Z-Wheel`)

Z-Wheel — the console's menu/shell, App ID **274755** — has the standard structure of a Zeebo BREW app:

```
mif/274755.mif              <- Module Information File (metadata + icons)
mod/274755/                 <- module directory, named after the App ID
    tectoy.mod              <- relocatable ARM binary (the “executable”)
    tectoy.sig              <- signature / execution authorization
```

The original research also inventories `.bar`, `.bid`, GGZ and `61u.key`; see the Portuguese source for the complete dump-specific findings.
