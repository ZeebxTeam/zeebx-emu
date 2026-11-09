//! Compiles rcheevos with `cc`, which knows each target's compiler (its Makefile only knows x86).
//! `RCHEEVOS_DIR` points to another copy of it.

use std::fmt::Write;
use std::path::{Path, PathBuf};

/// The `src/` folders that go in; `libretro/` is RetroArch's.
const DIRS: [&str; 5] = ["api", "client", "hash", "runtime", "util"];

macro_rules! copied {
    ($($name:ident: $ty:ty = $value:expr;)*) => {
        const COPIES: &[(&str, i64)] = &[$((stringify!($name), $value as i64)),*];
    };
}
include!("src/constants.rs");

fn main() {
    println!("cargo:rerun-if-env-changed=RCHEEVOS_DIR");
    let root = match std::env::var_os("RCHEEVOS_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("rcheevos"),
    };
    let include = root.join("include");
    if !include.join("rc_client.h").is_file() {
        panic!(
            "rcheevos is not in {}. Run `git submodule update --init rcheevos-sys/rcheevos`, \
             or point RCHEEVOS_DIR to a copy of it.",
            root.display()
        );
    }

    // The check of the copies (see `src/layout_check.c`): only declarations, which C refuses if wrong.
    let mut check = String::from("#include \"layout_check.c\"\n");
    for (name, value) in COPIES {
        writeln!(check, "ZEEBX_CHECK({name} == {value})").unwrap();
    }
    let generated = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("check_constants.c");
    std::fs::write(&generated, check).unwrap();
    println!("cargo:rerun-if-changed=src/layout_check.c");
    println!("cargo:rerun-if-changed=src/constants.rs");

    let mut build = cc::Build::new();
    build.include(&include).include("src").warnings(false);
    for name in DIRS {
        let dir = root.join("src").join(name);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut sources: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "c"))
            .collect();
        sources.sort();
        build.files(sources);
    }
    println!("cargo:rerun-if-changed={}", include.display());
    build.file(&generated);

    // The Zeebo hash lives with the zip one: it is the directory records of the `.mod` and the `.mif`.
    // Disc and encrypted ROM aren't needed.
    build
        .define("RC_STATIC", None)
        .define("RC_HASH_NO_DISC", None)
        .define("RC_HASH_NO_ENCRYPTED", None);
    build.compile("rcheevos");
}
