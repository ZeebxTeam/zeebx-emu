fn main() {
    // O unicorn-engine-sys traz o cputlb do QEMU, que usa atômicos de 128 bits
    // (__atomic_load_16, __atomic_store_16, __atomic_compare_exchange_16). No
    // x86-64 o compilador não os gera inline sem cmpxchg16b, então eles vivem
    // na libatomic do GCC e precisam ser linkados explicitamente.
    if cfg!(target_os = "linux") && std::env::var_os("CARGO_FEATURE_UNICORN").is_some() {
        // Só o backend QEMU precisa destes atômicos de 128 bits. O core Libretro não habilita
        // unicorn e, portanto, não pode exigir libatomic do sistema do handheld.
        println!("cargo:rustc-link-lib=atomic");
    }
    // **O ícone do `.exe` não mora mais aqui.** Um recurso do Windows só chega ao executável
    // se for compilado no pacote que o produz, e desde a 0.3.0 quem produz o `zeebx` é o
    // `frontends/classical-standalone`. O `rustc-link-lib` acima continua valendo daqui: aquilo
    // atravessa da biblioteca para quem a linka, um recurso do Windows não.
    println!("cargo:rerun-if-changed=build.rs");
}
