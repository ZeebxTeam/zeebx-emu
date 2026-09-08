fn main() {
    // O unicorn-engine-sys traz o cputlb do QEMU, que usa atômicos de 128 bits
    // (__atomic_load_16, __atomic_store_16, __atomic_compare_exchange_16). No
    // x86-64 o compilador não os gera inline sem cmpxchg16b, então eles vivem
    // na libatomic do GCC e precisam ser linkados explicitamente.
    if cfg!(target_os = "linux") {
        println!("cargo:rustc-link-lib=atomic");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
