# Instaladores e releases

Os instaladores saem do [cargo-packager](https://github.com/crabnebula-dev/cargo-packager), com a
configuração em `[package.metadata.packager]` no `Cargo.toml` de `frontends/qt-standalone/`, rodado
de dentro da pasta dele. **Ele não implanta o Qt**: cada formato tem o passo dele, e o
[`release.yml`](../.github/workflows/release.yml) é a receita completa.

- **`.deb`**: usa o Qt do sistema, então precisa ser montado contra ele — no Ubuntu 24.04, com os
  pacotes de [COMPILANDO.md](COMPILANDO.md): `cargo packager --release --formats deb`.
- **AppImage**: leva o próprio Qt, pelo `linuxdeploy-plugin-qt`, com `QMAKE` apontando o Qt,
  `QML_SOURCES_PATHS` para `frontends/qt-standalone/qml`, `EXTRA_PLATFORM_PLUGINS=libqwayland.so`
  e `EXTRA_QT_MODULES=waylandcompositor`. Monte num Ubuntu 22.04 com o Qt do `aqtinstall`, como o
  CI: no Arch, o `strip` do linuxdeploy não reconhece as bibliotecas do sistema, e o Qt de lá traz
  plugins com dependências que o linuxdeploy não acha.
- **Windows**: `windeployqt --release --no-translations --qmldir frontends/qt-standalone/qml
  --dir target/qt-implantado target/release/zeebx-qt.exe`, e depois `--formats nsis`.
- **macOS**: `--formats app`, `macdeployqt` no `.app`, a assinatura ad-hoc refeita com
  `codesign --force --deep --sign -`, e o `.dmg` pelo `hdiutil`.

Os arquivos ficam em `target/pacotes/`.

## Releases

Uma tag de versão (`v0.1.0` ou `0.1.0`) enviada ao GitHub dispara o
[`release.yml`](../.github/workflows/release.yml), que monta a release como rascunho, com o título
igual à tag. O emulador procura versões novas nessas releases ao abrir.

A tabela de qual arquivo da release é qual está em [INSTALACAO.md](INSTALACAO.md#qual-arquivo-baixar).