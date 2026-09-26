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
    #[cfg(feature = "ui-qt")]
    interface_qt();
    println!("cargo:rerun-if-changed=../../assets/icones/zeebx.ico");
    println!("cargo:rerun-if-changed=build.rs");
}

/// A ponte com o Qt: os `QObject`s escritos em Rust, a cola em C++ e o módulo QML. Ver
/// `docs/implementacao/21-migracao-para-qt.md`.
#[cfg(feature = "ui-qt")]
fn interface_qt() {
    use cxx_qt_build::{CxxQtBuilder, QmlModule};

    CxxQtBuilder::new_qml_module(
        QmlModule::new("zeebx")
            .qml_files(["qml/Principal.qml", "qml/Jogo.qml", "qml/GradeDaBiblioteca.qml", "qml/SliderDaBiblioteca.qml", "qml/JanelaDeConfiguracoes.qml", "qml/JanelaDeSaves.qml", "qml/JanelaDeLog.qml"])
            .depend("QtQuick"),
    )
    .files(["src/qt/ponte.rs", "src/qt/biblioteca.rs", "src/qt/configuracoes.rs", "src/qt/auxiliares.rs"])
    .include_dir("src/qt/cpp")
    .cpp_files([
        "src/qt/cpp/gl_qt.cpp",
        // O cabeçalho entra para o moc: o `ItemDoQuadro` tem `Q_OBJECT`.
        "src/qt/cpp/quadro.h",
        "src/qt/cpp/quadro.cpp",
        "src/qt/cpp/imagens.cpp",
    ])
    // A imagem do aviso de calibração e a logo do ícone, em `qrc:/zeebx/`. O `assets/` é do
    // projeto, e não deste pacote.
    .qrc_resources(cxx_qt_build::QResources::new().resource(
        cxx_qt_build::QResource::new()
            .prefix("/zeebx")
            .file(qt_build_utils::QResourceFile::new("../../assets/boomerang.png").alias("boomerang.png"))
            .file(qt_build_utils::QResourceFile::new("../../assets/zeebx.png").alias("zeebx.png")),
    ))
    .qt_module("Quick")
    // O Qt Qml pede o Network no macOS.
    .qt_module("Network")
    .build();
    println!("cargo:rerun-if-changed=src/qt/cpp/gl_qt.h");
    println!("cargo:rerun-if-changed=src/qt/cpp/imagens.h");
}
