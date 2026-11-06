# Zeebx on iOS

The emulation core is shared with the other frontends; this directory provides the iOS application wrapper.

Building requires a Mac with Xcode and the Rust targets `aarch64-apple-ios` and `aarch64-apple-ios-sim`:

```bash
./frontends/ios/compilar.sh --app
```

The build details and device signing options are in [`LEIAME.md`](LEIAME.md).
