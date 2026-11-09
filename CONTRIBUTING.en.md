# Notes for contributors

Portuguese version: [CONTRIBUTING.md](CONTRIBUTING.md).

When you open a pull request, target the `development` branch, or the branch that matches the change. Do not open the pull request against `master`: that is where the release-build CI is concentrated.

A new frontend lives under `frontends/`, next to the ones that already exist:

- `frontends/android/`
- `frontends/cli/`
- `frontends/headless/`
- `frontends/ios/`
- `frontends/libretro/`
- `frontends/qt-standalone/`
- `frontends/switch/`
- `frontends/vita/`
- `frontends/vita-standalone/`

It also needs a build target in [`.github/workflows/release.yml`](.github/workflows/release.yml), so the artifact is published with the release.

These are the details that keep the repository organized.
