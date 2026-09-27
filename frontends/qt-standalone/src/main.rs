//! Zeebx Qt — o emulador de Zeebo com a interface em Qt Quick.
//!
//! Um frontend à parte do `egui-standalone`: `zeebx-qt` abre a biblioteca, e `zeebx-qt <jogo>` abre
//! direto o jogo. A linha de comando de depuração (`run`, `info`, `bench`...) continua no `zeebx`.

// No Windows, a versão de distribuição abre sem a janela de console atrás da interface.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod qt;

use std::process::ExitCode;

/// O nome com que o Wayland casa a janela com o `.desktop` instalado, de onde tira o ícone. É o do
/// binário, que é o nome que o `cargo packager` dá ao `.desktop`.
const APP_ID: &str = "zeebx-qt";

fn main() -> ExitCode {
    zeebx::registro::le_do_ambiente();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        // Sai antes de criar o `QGuiApplication`, sem tela: o CI roda isto para provar que o
        // binário acha as bibliotecas do Qt, que o sistema carrega antes do `main`.
        Some("--version") => {
            println!("zeebx-qt {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        jogo => qt::launch(jogo),
    }
}
