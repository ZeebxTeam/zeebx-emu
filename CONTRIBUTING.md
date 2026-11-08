# Notas para colaboradores

Versão em inglês: [CONTRIBUTING.en.md](CONTRIBUTING.en.md).

Ao abrir uma pull request, aponte para a branch `development`, ou para a branch do ajuste em questão. Não abra a pull request contra `master`: é nela que o CI de build das releases se concentra.

Um frontend novo fica em `frontends/`, ao lado dos que já existem:

- `frontends/android/`
- `frontends/cli/`
- `frontends/headless/`
- `frontends/ios/`
- `frontends/libretro/`
- `frontends/qt-standalone/`
- `frontends/switch/`
- `frontends/vita/`
- `frontends/vita-standalone/`

Ele também entra no [`.github/workflows/release.yml`](.github/workflows/release.yml), com um alvo de build no CI, para o artefato sair junto na release.

São detalhes para manter o repositório organizado.
