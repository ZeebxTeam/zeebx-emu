//! O resumo de cada jogo já aberto, guardado em disco: o "Conquistas: 12/40" dos detalhes da
//! biblioteca.
//!
//! **Sem consulta ao servidor ao abrir os detalhes.** O número é o da última vez em que o jogo foi
//! aberto com a conta conectada, e é de **uma** conta: trocar de conta esconde os resumos da outra
//! em vez de mostrá-los como se fossem desta.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guardado {
    pub usuario: String,
    pub desbloqueadas: u32,
    pub total: u32,
}

fn caminho() -> PathBuf {
    crate::config::config_dir()
        .join("conquistas")
        .join("resumos.json")
}

fn le_todos() -> HashMap<String, Guardado> {
    std::fs::read(caminho())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// O resumo do jogo de `chave` (ver [`crate::library::chave_do_jogo`]), se ele foi aberto com a
/// conta de `usuario`.
pub fn le(chave: &str, usuario: &str) -> Option<Guardado> {
    le_todos().remove(chave).filter(|g| g.usuario == usuario)
}

/// Guarda o resumo do jogo de `chave`. Uma falha de disco só fica no registro: é um número de
/// conveniência, e não vale derrubar nada por ele.
pub fn grava(chave: &str, guardado: Guardado) {
    let mut todos = le_todos();
    if todos.get(chave) == Some(&guardado) {
        return;
    }
    todos.insert(chave.to_string(), guardado);
    let resultado = serde_json::to_vec_pretty(&todos)
        .map_err(|erro| erro.to_string())
        .and_then(|bytes| {
            let caminho = caminho();
            if let Some(pasta) = caminho.parent() {
                std::fs::create_dir_all(pasta).map_err(|erro| erro.to_string())?;
            }
            std::fs::write(caminho, bytes).map_err(|erro| erro.to_string())
        });
    if let Err(erro) = resultado {
        crate::registro!(
            crate::registro::Nivel::Aviso,
            "conquistas",
            "o resumo de {chave} não foi guardado: {erro}"
        );
    }
}
