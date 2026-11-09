fn main() {
    // **The `.exe` icon does not live here.** A Windows resource only reaches the executable if
    // it is compiled in the package that produces it: `frontends/qt-standalone` and `frontends/cli`.
    println!("cargo:rerun-if-changed=build.rs");
    // `frontends/switch/build.sh` passes `--cfg zeebx_switch`. Without this declaration rustc
    // treats the name as unknown.

    println!("cargo::rustc-check-cfg=cfg(zeebx_switch)");
}
