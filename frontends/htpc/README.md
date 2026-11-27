# Zeebx HTPC (`zeebx-htpc`)

A dedicated 10-foot, gamepad-first frontend for living room TVs, HTPCs, Steam Deck, and Steam Big Picture / Gamescope.

It shares the same configuration folder, ROM library, saves, save states, and color palettes (`temas/`) with `zeebx-qt`, so you can install and switch between both binaries freely.

## Launching & Steam Integration

- **Open the HTPC library:**
  ```bash
  zeebx-htpc
  ```
- **Launch a game directly (Steam ROM Manager / ES-DE / Playnite):**
  ```bash
  zeebx-htpc "/path/to/game.zip"
  ```
  When launched with a ROM path, exiting the game from the Quick Menu closes `zeebx-htpc` cleanly so focus returns to Steam Big Picture or your launcher.

## Default Gamepad Controls

Any standard controller (Xbox, PlayStation, Nintendo, Steam Input) navigates the HTPC interface out of the box:

- **D-Pad / Left Stick:** Navigate
- **South (`A` / `✕`):** Play / Confirm / Save State
- **East (`B` / `○`):** Back / Resume
- **West (`X` / `□`):** Load State / Choose Folder
- **North (`Y` / `△`):** Game Details
- **L1 / R1 (`LB` / `RB`):** Switch Tabs / Settings Categories
- **Start (`Menu` / `Options`):** Open HTPC Settings
- **Select (`View` / `Share`):** Launch Z-Wheel (when installed)
- **Quick Menu in-game:** `L3 + R3` by default (configurable in *Settings → Controllers & HTPC* to `Start + Select` or `Guide / Home`, avoiding collisions with the Zeebo's `HOME` button)

## Community HTPC Themes (`htpc-themes/`)

The built-in default theme is **Z-Wheel HD**. Extra themes (official or community-made) are discovered automatically in:

```text
<executable folder>/htpc-themes/<theme-id>/    # portable install
<config folder>/htpc-themes/<theme-id>/        # user themes
```

Each theme folder contains a `theme.json` manifest (format `1`) and a QML entry file:

```json
{
  "format": 1,
  "name": {
    "en": "My Theme",
    "pt-BR": "Meu Tema"
  },
  "description": {
    "en": "A custom 10-foot showcase for Zeebx HTPC."
  },
  "author": "Your Name",
  "version": "1.0.0",
  "entry": "Main.qml",
  "default_palette": "laranja",
  "palettes": {
    "custom_dark": {
      "name": { "en": "Custom Dark", "pt-BR": "Escuro Custom" },
      "dark": true,
      "background": "#10141c",
      "background_alt": "#1a2233",
      "surface": "#212b40",
      "border": "#4a5c7d",
      "text": "#eef3fa",
      "muted": "#93a3bd",
      "accent": "#ff6a2b"
    }
  }
}
```

### Theme QML Contract

Every theme is loaded inside the host shell (`HtpcMain.qml`) and receives:

- `property var library`: the `Library` model (`lines()`, `item(row)`, `hasZWheel()`, `count`, `empty`, `onModelReset`)
- `property var htpc`: the `HtpcController` singleton exposing resolved per-theme colors (`bgColor`, `bgAltColor`, `surfaceColor`, `surfaceSolidColor`, `borderColor`, `textColor`, `mutedColor`, `accentColor`, `accentTextColor`, `accentSoftColor`, `accentGlowColor`, `reelBgColor`, `warningColor`, `errorColor`, `darkTheme`), controller prompts (`promptOk`, `promptBack`, `promptX`, `promptY`, `promptL1`, `promptR1`, `promptStart`, `promptSelect`, `promptQuick`), and `controllerStatus`
- `property var host`: shell actions (`launchGame(line)`, `launchZWheel()`, `openDetails(line)`, `openSettings()`, `showToast(msg, isError)`)
- `function handleCommand(code)`: receives unified gamepad/keyboard navigation codes (`0` Up, `1` Down, `2` Left, `3` Right, `4` Confirm, `5` Back, `6` ActionX, `7` ActionY, `8` TabPrev, `9` TabNext, `10` Start, `11` Select)

See [`qml/themes/zwheel-hd/Main.qml`](qml/themes/zwheel-hd/Main.qml) as the complete reference implementation. If an external theme fails to load, `zeebx-htpc` automatically falls back to `zwheel-hd` so the user is never locked out.

### Per-Theme Color Configuration

Color palette and accent choices are stored per HTPC theme under `htpc.theme_colors` in `settings.json`. Each theme can use its own `theme.json` palettes, follow the global Zeebx color theme, or use any palette from `temas/*.json` — plus an optional accent color override.

## Building & Testing

```bash
cargo build --release --locked -p zeebx-htpc
cargo test  --release --locked -p zeebx-htpc
QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1 timeout 8 ./target/release/zeebx-htpc
```
