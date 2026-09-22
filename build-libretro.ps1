# Compila o núcleo Libretro no Windows (MSVC): zeebx_libretro.dll
$ErrorActionPreference = "Stop"

cargo build --lib --release
if (-not $?) { exit $LASTEXITCODE }

$origem = Join-Path "target" "release" "zeebx.dll"
if (-not (Test-Path $origem)) {
    Write-Error "não achei $origem — a compilação do cdylib falhou"
}

Copy-Item $origem "zeebx_libretro.dll" -Force
Write-Host "zeebx_libretro.dll"

$prefixo = Join-Path $env:APPDATA "RetroArch"
if ($args -contains "install") {
    $cores = Join-Path $prefixo "cores"
    $info = Join-Path $prefixo "info"
    New-Item -ItemType Directory -Force -Path $cores, $info | Out-Null
    Copy-Item "zeebx_libretro.dll" $cores -Force
    Copy-Item "zeebx_libretro.info" $info -Force
    Write-Host "instalado em $prefixo"
}
