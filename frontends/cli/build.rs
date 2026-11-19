// The icon and the version go into the Windows `.exe`: they are what Explorer and the taskbar
// show.
//
// **It lives in this package because the resource only reaches the executable if it is compiled
// here.** At the root it got stuck in the library, which is not what the user opens. `assets/`
// is still at the repository root, hence the two levels up.
fn main() {
    // `CARGO_CFG_TARGET_OS` is the target's, not the one of the machine compiling: `cfg!` here
    // would answer for the system the build.rs runs on.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/icons/zeebx.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=no icon in the .exe: {error}");

        }
    }
    println!("cargo:rerun-if-changed=../../assets/icons/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
