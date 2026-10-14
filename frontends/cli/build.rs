// O ícone e a versão entram no `.exe` do Windows: é o que o Explorer e a barra de tarefas
// mostram.
//
// **Mora neste pacote porque o recurso só chega ao executável se for compilado aqui.** Na raiz
// ele ficava preso na biblioteca, que não é o que o usuário abre. O `assets/` continua na raiz
// do repositório, por isso os dois níveis acima.
fn main() {
    // O `CARGO_CFG_TARGET_OS` é o do alvo, e não o da máquina que compila: o `cfg!` aqui
    // responderia pelo sistema em que o build.rs roda.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut recurso = winresource::WindowsResource::new();
        recurso.set_icon("../../assets/icones/zeebx.ico");
        if let Err(erro) = recurso.compile() {
            println!("cargo:warning=sem ícone no .exe: {erro}");
        }
    }
    println!("cargo:rerun-if-changed=../../assets/icones/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}
