//! Compila o rcheevos com o `cc`, que sabe o compilador de cada alvo (o Makefile dele só sabe x86).
//! `RCHEEVOS_DIR` aponta para outra cópia dele.

use std::fmt::Write;
use std::path::{Path, PathBuf};

/// As pastas de `src/` que entram; `libretro/` é do RetroArch.
const PASTAS: [&str; 5] = ["api", "client", "hash", "runtime", "util"];

macro_rules! copia {
    ($($nome:ident: $tipo:ty = $valor:expr;)*) => {
        const COPIAS: &[(&str, i64)] = &[$((stringify!($nome), $valor as i64)),*];
    };
}
include!("src/constantes.rs");

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

    // A conferência das cópias (ver `src/confere.c`): só declarações, que o C recusa se erradas.
    let mut confere = String::from("#include \"confere.c\"\n");
    for (nome, valor) in COPIAS {
        writeln!(confere, "ZEEBX_CONFERE({nome} == {valor})").unwrap();
    }
    let gerado = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("confere_constantes.c");
    std::fs::write(&gerado, confere).unwrap();
    println!("cargo:rerun-if-changed=src/confere.c");
    println!("cargo:rerun-if-changed=src/constantes.rs");

    let mut build = cc::Build::new();
    build.include(&include).include("src").warnings(false);
    for pasta in PASTAS {
        let dir = raiz.join("src").join(pasta);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut fontes: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "c"))
            .collect();
        fontes.sort();
        build.files(fontes);
    }
    println!("cargo:rerun-if-changed={}", include.display());
    build.file(&gerado);

    // O hash do Zeebo é o MD5 do `.mod`: disco, ROM cifrada e zip não fazem falta.
    build
        .define("RC_STATIC", None)
        .define("RC_HASH_NO_DISC", None)
        .define("RC_HASH_NO_ENCRYPTED", None)
        .define("RC_HASH_NO_ZIP", None);
    build.compile("rcheevos");
}
