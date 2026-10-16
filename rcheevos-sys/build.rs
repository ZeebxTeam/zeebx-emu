//! Compila o rcheevos como biblioteca estática, com o `cc`.
//!
//! **Não é CMake nem o Makefile do rcheevos.** O `Makefile.common` dele só sabe x86 e x64 e exige
//! `ARCH`; o Zeebx compila também para Android, iOS e o Switch. O `cc` já sabe o compilador e as
//! flags de cada alvo, e já está no `Cargo.lock` (o core Libretro usa). A lista de pastas abaixo é
//! a mesma do `Makefile.common`.
//!
//! O código vem do submódulo `rcheevos/`, que aponta para o fork com o que o Zeebo precisa. Quem
//! está mexendo no fork pode apontar `RCHEEVOS_DIR` para a própria cópia e compilar o Zeebx contra
//! ela sem empurrar nada.

use std::path::{Path, PathBuf};

/// As pastas de `src/` que entram. Fica de fora `libretro/`, que é a ponte do RetroArch com os
/// cores e não serve a um frontend próprio.
const PASTAS: [&str; 5] = ["api", "client", "hash", "runtime", "util"];

fn main() {
    println!("cargo:rerun-if-env-changed=RCHEEVOS_DIR");
    let raiz = match std::env::var_os("RCHEEVOS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("rcheevos"),
    };
    let include = raiz.join("include");
    if !include.join("rc_client.h").is_file() {
        panic!(
            "o rcheevos não está em {}. Rode `git submodule update --init rcheevos-sys/rcheevos`, \
             ou aponte RCHEEVOS_DIR para uma cópia dele.",
            raiz.display()
        );
    }

    let mut build = cc::Build::new();
    build.include(&include).warnings(false);
    for pasta in PASTAS {
        let dir = raiz.join("src").join(pasta);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut fontes: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "c"))
            .collect();
        // `read_dir` não promete ordem; a ordem dos objetos no `.a` não muda o resultado, mas
        // uma compilação repetível é mais fácil de comparar.
        fontes.sort();
        build.files(fontes);
    }
    println!("cargo:rerun-if-changed={}", include.display());
    // Os tamanhos dos structs copiados no `lib.rs`, para o teste de layout. São umas dezenas de
    // funções de uma linha; o ligador descarta as que ninguém chama fora dos testes.
    build.file("src/tamanhos.c");
    println!("cargo:rerun-if-changed=src/tamanhos.c");

    // O hash de disco (CD, CHD), o de ROM cifrada (3DS) e o de zip (DOS, arcade) são de consoles
    // que o Zeebx não emula. O do Zeebo é o MD5 do arquivo inteiro, que não depende de nenhum dos
    // três.
    build
        .define("RC_STATIC", None)
        .define("RC_HASH_NO_DISC", None)
        .define("RC_HASH_NO_ENCRYPTED", None)
        .define("RC_HASH_NO_ZIP", None);
    build.compile("rcheevos");
}
