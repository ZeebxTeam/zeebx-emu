//! O que o tico põe na linha de comando, e o que vale quando um pedaço não veio.
//!
//! A ordem está no `launch` de `module.json`. O `{nro}` é o argv[0] e não chega aqui: o
//! hospedeiro em C já separou o resto. Um caminho vazio vira o de fábrica, para um teste
//! manual do `.nro` sem o tico ainda achar as pastas.

use std::path::{Path, PathBuf};

pub const CONFIG: &str = "sdmc:/tico/config/cores/zeebx.jsonc";
pub const SYSTEM: &str = "sdmc:/tico/system/zeebx";
pub const SAVES: &str = "sdmc:/tico/saves/zeebo";
pub const STATES: &str = "sdmc:/tico/states/zeebo";
pub const ROMS: &str = "sdmc:/tico/roms/zeebo";

/// Os pedaços como o hospedeiro os leu. `None` é ponteiro nulo ou texto vazio.
pub struct Pedido {
    pub rom: Option<String>,
    pub titulo: String,
    pub saves: Option<String>,
    pub states: Option<String>,
    pub system: Option<String>,
    pub roms: Option<String>,
    pub config: Option<String>,
}

/// O jogo e as pastas, já com os vazios preenchidos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lancamento {
    pub rom: PathBuf,
    pub titulo: String,
    pub saves: PathBuf,
    pub states: PathBuf,
    pub system: PathBuf,
    pub roms: PathBuf,
    pub config: PathBuf,
}

pub fn resolve(pedido: Pedido) -> Result<Lancamento, String> {
    let rom = pedido
        .rom
        .filter(|texto| !texto.is_empty())
        .ok_or_else(|| "o tico não disse qual jogo abrir".to_string())?;
    let rom = PathBuf::from(rom);
    let titulo = if pedido.titulo.is_empty() {
        rom.file_stem()
            .and_then(|nome| nome.to_str())
            .unwrap_or("Zeebo")
            .to_string()
    } else {
        pedido.titulo
    };
    let saves = caminho(pedido.saves.as_deref(), SAVES);
    let system = caminho(pedido.system.as_deref(), SYSTEM);
    // Sem a pasta de roms, a Z-Wheel não acha os outros jogos. A pasta do arquivo
    // aberto é o mínimo: é onde o tico deixou este, e costuma ser onde estão os outros.
    let roms = match pedido.roms.as_deref() {
        Some(texto) if !texto.is_empty() => PathBuf::from(texto),
        _ => rom.parent().unwrap_or(Path::new(ROMS)).to_path_buf(),
    };
    Ok(Lancamento {
        rom,
        titulo,
        saves,
        states: caminho(pedido.states.as_deref(), STATES),
        system,
        roms,
        config: caminho(pedido.config.as_deref(), CONFIG),
    })
}

fn caminho(texto: Option<&str>, fabrica: &str) -> PathBuf {
    match texto {
        Some(texto) if !texto.is_empty() => PathBuf::from(texto),
        _ => PathBuf::from(fabrica),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pedido() -> Pedido {
        Pedido {
            rom: Some("sdmc:/tico/roms/zeebo/Double Dragon.mod".into()),
            titulo: "Double Dragon".into(),
            saves: Some("sdmc:/tico/saves/conta/zeebo".into()),
            states: Some("sdmc:/tico/states/conta/zeebo".into()),
            system: Some("sdmc:/tico/system/zeebx".into()),
            roms: Some("sdmc:/tico/roms/zeebo".into()),
            config: Some("sdmc:/tico/config/cores/zeebx.jsonc".into()),
        }
    }

    #[test]
    fn a_linha_do_tico_vira_as_pastas_da_conta() {
        let jogo = resolve(pedido()).unwrap();
        assert_eq!(jogo.titulo, "Double Dragon");
        assert_eq!(
            jogo.saves,
            PathBuf::from("sdmc:/tico/saves/conta/zeebo")
        );
        assert_eq!(jogo.roms, PathBuf::from("sdmc:/tico/roms/zeebo"));
    }

    #[test]
    fn sem_pastas_fica_o_caminho_de_fabrica_e_a_pasta_do_arquivo() {
        let mut cru = pedido();
        cru.titulo.clear();
        cru.saves = None;
        cru.states = Some(String::new());
        cru.system = None;
        cru.roms = None;
        cru.config = None;
        let jogo = resolve(cru).unwrap();
        assert_eq!(jogo.titulo, "Double Dragon");
        assert_eq!(jogo.saves, PathBuf::from(SAVES));
        assert_eq!(jogo.states, PathBuf::from(STATES));
        assert_eq!(jogo.system, PathBuf::from(SYSTEM));
        assert_eq!(jogo.config, PathBuf::from(CONFIG));
        assert_eq!(jogo.roms, PathBuf::from("sdmc:/tico/roms/zeebo"));
    }

    #[test]
    fn sem_jogo_nao_ha_o_que_abrir() {
        let mut cru = pedido();
        cru.rom = None;
        assert!(resolve(cru).is_err());
    }
}
