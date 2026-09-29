fn main() {
    // Um `staticlib` não repassa `cargo:rustc-link-lib` ao ligador do `.app`. O Xcode tem de
    // nomear os frameworks na linha dele — ver `OTHER_LDFLAGS` do projeto. Estas linhas existem
    // para o `cargo test` deste pacote, que liga o binário de teste no host e precisa dos mesmos
    // frameworks que o `cpal` e o SQLite embutido pedem no Apple. Fora da Apple o rustc recusa
    // `framework=`, e um `cargo test --workspace` no Linux ou no Windows pararia aqui.
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() != Ok("apple") {
        return;
    }
    println!("cargo:rustc-link-lib=framework=AudioToolbox");
    println!("cargo:rustc-link-lib=framework=CoreAudio");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
}
