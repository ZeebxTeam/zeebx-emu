//! O pacote que o tico lê em `sdmc:/tico/modules/zeebx/`. O teste trava o contrato: o id não
//! muda, o `.nro` tem o nome que o `launch` abre, e toda chave da tela de ajustes tem texto
//! em inglês e em português.

use serde_json::Value;

fn le(nome: &str) -> String {
    let raiz = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("modulo");
    std::fs::read_to_string(raiz.join(nome)).unwrap_or_else(|erro| panic!("{nome}: {erro}"))
}

fn chaves(valor: &Value, saida: &mut Vec<String>) {
    let Some(objeto) = valor.as_object() else {
        return;
    };
    for campo in ["name", "title", "label"] {
        if let Some(texto) = objeto.get(campo).and_then(Value::as_str) {
            saida.push(texto.to_string());
        }
    }
    for filho in objeto.values() {
        match filho {
            Value::Array(lista) => {
                for item in lista {
                    chaves(item, saida);
                }
            }
            Value::Object(_) => chaves(filho, saida),
            _ => {}
        }
    }
}

#[test]
fn o_modulo_aponta_para_o_nro_e_para_as_pastas_que_o_laço_le() {
    let modulo: Value = serde_json::from_str(&le("module.json")).unwrap();
    assert_eq!(modulo["id"], "zeebx");
    assert_eq!(modulo["nro"], "tico-zeebx.nro");
    assert_eq!(modulo["is_libretro"], false);
    assert_eq!(modulo["system_dir"], "zeebx");
    assert_eq!(modulo["config_file"], "zeebx.jsonc");
    assert_eq!(modulo["bool_true_value"], "enabled");
    assert_eq!(modulo["bool_false_value"], "disabled");
    let launch = modulo["launch"].as_str().unwrap();
    for pedaco in ["{nro}", "{rom}", "{title}", "{saves}", "{states}", "{system}", "{roms}"] {
        assert!(launch.contains(pedaco), "{pedaco} falta no launch");
    }
    let sistema = &modulo["systems"][0];
    assert_eq!(sistema["slug"], "zeebo");
    assert!(sistema["extensions"].as_array().unwrap().iter().any(|v| v == ".mod"));
    assert!(sistema["extensions"].as_array().unwrap().iter().any(|v| v == ".zip"));
    assert!(sistema["extensions"].as_array().unwrap().iter().any(|v| v == ".7z"));
    for cor in ["start", "end", "start_dark", "end_dark"] {
        let texto = sistema["colors"][cor].as_str().unwrap();
        assert_eq!(texto.len(), 6, "{cor}");
        assert!(texto.chars().all(|c| c.is_ascii_hexdigit()));
    }
    let lista = le("gamelists/zeebo.dat");
    assert!(lista.contains("<datafile"));
}

#[test]
fn cada_chave_da_tela_tem_texto_em_ingles_e_em_portugues() {
    let ajustes: Value = serde_json::from_str(&le("settings.json")).unwrap();
    assert_eq!(ajustes["core_id"], "zeebx");
    assert_eq!(ajustes["config_file"], "zeebx.jsonc");
    let mut chaves_da_tela = Vec::new();
    chaves(&ajustes, &mut chaves_da_tela);
    assert!(!chaves_da_tela.is_empty());
    for idioma in ["lang/en.json", "lang/pt.json"] {
        let textos: Value = serde_json::from_str(&le(idioma)).unwrap();
        for chave in &chaves_da_tela {
            let texto = textos[chave].as_str().unwrap_or("");
            assert!(!texto.is_empty(), "{chave} sem texto em {idioma}");
        }
    }
}
