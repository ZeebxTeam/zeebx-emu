fn main() {
    // A `staticlib` does not pass `cargo:rustc-link-lib` on to the `.app` linker. Xcode has to
    // name the frameworks on its own command line — see the project's `OTHER_LDFLAGS`. These
    // lines exist for this package's `cargo test`, which links the test binary on the host and
    // needs the same frameworks that `cpal` and the bundled SQLite require on Apple. Outside
    // Apple rustc rejects `framework=`, and a `cargo test --workspace` on Linux or Windows would
    // stop here.

    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() != Ok("apple") {
        return;
    }
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
}
