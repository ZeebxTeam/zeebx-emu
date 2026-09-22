//! Zeebx — emulador de Zeebo / Qualcomm BREW.

// No Windows, a versão de distribuição abre sem a janela de console atrás da interface. O preço é
// a linha de comando (`zeebx run`, `zeebx sessao`) não escrever no terminal nessa versão; para
// ela, o build de desenvolvimento continua com console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    zeebx::run()
}
