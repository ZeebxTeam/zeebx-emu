fn main() {
    let alvo = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if alvo != "vita" {
        return;
    }

    let manifesto = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let fonte = manifesto.join("../vita/compat/posix_faltante.c");
    println!("cargo:rerun-if-changed={}", fonte.display());

    // O `cc` do build script, sem isto, é o do host. O objeto tem de ser ARM de 32 bits para
    // entrar no mesmo executável que o `vita-elf-create` vai converter.
    let triplo = std::env::var("TARGET").unwrap_or_default();
    let chave = format!("CC_{}", triplo.replace('-', "_"));
    let compilador = std::env::var(&chave).unwrap_or_else(|_| {
        panic!("defina {chave} com o arm-vita-eabi-gcc; é o que o compilar.sh exporta");
    });

    cc::Build::new()
        .compiler(compilador)
        .file(&fonte)
        .flag("-march=armv7-a")
        .flag("-mfpu=neon")
        .flag("-mfloat-abi=hard")
        .flag("-mword-relocations")
        .compile("posix_faltante");

    // `-Wl,-q` deixa a relocação no ELF. Sem ela o `vita-elf-create` não monta o `eboot.bin`.
    println!("cargo:rustc-link-arg=-Wl,-q");
    // O heap do newlib é este símbolo. Sem o `-u` o `--gc-sections` joga fora: nenhum código
    // Rust o lê. Ver `_newlib_heap_size_user` em `vita.rs`.
    println!("cargo:rustc-link-arg=-Wl,-u,_newlib_heap_size_user");
    // O `cc` põe o `-lposix_faltante` antes do sqlite. O linker é de uma passada: nesse momento
    // ninguém pediu `fchown`, então o `.o` fica de fora, e o sqlite — que vem depois e toma o
    // endereço da função — fica sem definição. O `--whole-archive` no fim, com o sqlite já na
    // linha, é o que entra.
    let saida = std::env::var("OUT_DIR").unwrap();
    println!("cargo:rustc-link-arg=-Wl,--whole-archive");
    println!("cargo:rustc-link-arg={saida}/libposix_faltante.a");
    println!("cargo:rustc-link-arg=-Wl,--no-whole-archive");
    // Os stubs que o gcc da Vita não puxa sozinho. `SceLibKernel` e o `pthread` já vêm do specs
    // do toolchain; repetir um `.a` estático aqui duplica símbolo.
    for lib in [
        "SceDisplay_stub",
        "SceCtrl_stub",
        "SceAudio_stub",
        "SceSysmem_stub",
        "SceProcessmgr_stub",
        // Leva o quadro pintado na RAM com cache até o framebuffer sem cache. Sem isto a CPU
        // escreve o scanout direto, e cada store vai ao barramento.
        "SceKernelDmacMgr_stub",
        // O aplicativo sobe com o clock que o sistema deixou. O interpretador usa o ARM o
        // tempo todo, e o barramento leva a memória do jogo.
        "ScePower_stub",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
    // O vitaGL é estático e vem depois do nosso código: o linker é de uma passada. O C++ é
    // dele (`__cxa_*`). O `vitashark` é o compilador de shader que o vitaGL chama (`shark_*`);
    // o stub `SceShaccCg` só tem o `sceShaccCg*`, que o shark é quem usa.
    //
    // `SceDisplay` já entrou acima, pelo nosso `sceDisplaySetFrameBuf`. O vitaGL pede outras
    // funções do mesmo stub (`sceDisplayWaitVblankStartMulti`), e a primeira passada não as
    // viu. Repetir o stub puxa só o objeto que faltou: cada função mora no seu `.o`.
    for lib in [
        "vitaGL",
        "vitashark",
        // O `sceShaccCgExt*` não está no stub: é esta biblioteca estática. Ela
        // aplica o patch pelo taiHEN, que o homebrew já tem carregado.
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
        // O shark carrega o módulo do compilador. O specs já ligou este stub para o resto
        // do processo; aqui ele só entra de novo se `sceKernelLoadStartModule` ainda faltar.
        "SceLibKernel_stub",
    ] {
        println!("cargo:rustc-link-lib={lib}");
    }
}
