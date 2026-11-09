fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target != "vita" {
        return;
    }

    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let source = manifest.join("../vita/compat/posix_missing.c");
    println!("cargo:rerun-if-changed={}", source.display());

    // Without this, the build script's `cc` is the host's. The object has to be 32-bit ARM to go
    // into the same executable that `vita-elf-create` will convert.
    let triple = std::env::var("TARGET").unwrap_or_default();
    let key_id = format!("CC_{}", triple.replace('-', "_"));
    let compiler = std::env::var(&key_id).unwrap_or_else(|_| {
        panic!("set {key_id} to arm-vita-eabi-gcc; that is what build.sh exports");
    });

    cc::Build::new()
        .compiler(compiler)
        .file(&source)
        .flag("-march=armv7-a")
        .flag("-mfpu=neon")
        .flag("-mfloat-abi=hard")
        .flag("-mword-relocations")
        .compile("posix_faltante");

    // `-Wl,-q` keeps the relocations in the ELF. Without them `vita-elf-create` can't build the
    // `eboot.bin`.
    println!("cargo:rustc-link-arg=-Wl,-q");
    // The newlib heap is this symbol. Without the `-u`, `--gc-sections` throws it away: no Rust
    // code reads it. See `_newlib_heap_size_user` in `vita.rs`.
    println!("cargo:rustc-link-arg=-Wl,-u,_newlib_heap_size_user");
    // `cc` puts `-lposix_faltante` before sqlite. The linker is single-pass: at that point nobody
    // has asked for `fchown`, so the `.o` is left out, and sqlite — which comes later and takes the
    // function's address — ends up without a definition. The `--whole-archive` at the end, with
    // sqlite already on the line, is what gets it in.
    let output = std::env::var("OUT_DIR").unwrap();
    println!("cargo:rustc-link-arg=-Wl,--whole-archive");
    println!("cargo:rustc-link-arg={output}/libposix_faltante.a");
    println!("cargo:rustc-link-arg=-Wl,--no-whole-archive");
    // The stubs the Vita gcc does not pull in on its own. `SceLibKernel` and `pthread` already come
    // from the toolchain's specs; repeating a static `.a` here duplicates symbols.
    for lib in [
        "SceDisplay_stub",
        "SceCtrl_stub",
        "SceAudio_stub",
        "SceSysmem_stub",
        "SceProcessmgr_stub",
        // Carries the frame painted in cached RAM to the uncached framebuffer. Without this the
        // CPU writes the scanout directly, and every store goes to the bus.
        "SceKernelDmacMgr_stub",
        // The app starts with whatever clock the system left. The interpreter uses the ARM the
        // whole time, and the bus carries the game's memory.
        "ScePower_stub",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    // vitaGL is static and comes after our code: the linker is single-pass. The C++ is its own
    // (`__cxa_*`). `vitashark` is the shader compiler vitaGL calls (`shark_*`); the `SceShaccCg`
    // stub only has `sceShaccCg*`, which is what shark uses.
    //
    // `SceDisplay` already went in above, through our `sceDisplaySetFrameBuf`. vitaGL asks for
    // other functions from the same stub (`sceDisplayWaitVblankStartMulti`), and the first pass
    // did not see them. Repeating the stub pulls in only the missing object: each function lives
    // in its own `.o`.
    for lib in [
        "vitaGL",
        "vitashark",
        // `sceShaccCgExt*` is not in the stub: it is this static library. It
        // applies the patch through taiHEN, which homebrew already has loaded.
        "SceShaccCgExt",
        "taihen_stub",
        "mathneon",
        "stdc++",
        "SceDisplay_stub",
        "SceGxm_stub",
        "SceShaccCg_stub",
        "SceAppMgr_stub",
        "SceCommonDialog_stub",
        "SceRtc_stub",
        "SceSysmodule_stub",
        // shark loads the compiler module. The specs already linked this stub for the rest
        // of the process; here it only goes in again if `sceKernelLoadStartModule` is still missing.
        "SceLibKernel_stub",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
