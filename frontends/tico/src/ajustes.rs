//! O `zeebx.jsonc` que o tico grava a partir de `settings.json`.
//!
//! O formato exato do arquivo não está no guia do tico — só o caminho e que ele aceita
//! comentário. Lemos um objeto, as chaves na raiz e, se não estiverem lá, um nível abaixo,
//! pulando um mapa por jogo: um ajuste de um título não pode valer para o seguinte só porque
//! apareceu primeiro no arquivo. Sem arquivo, vale o padrão.

use std::path::Path;

use serde_json::Value;
use zeebx::audio::MidiBackend;
use zeebx::velocidade::{Frameskip, LimiteFps};

#[derive(Debug, Clone, PartialEq)]
pub struct Ajustes {
    pub limite: LimiteFps,
    pub frameskip: Frameskip,
    pub reducao: usize,
    pub audio: bool,
    pub volume: f32,
    pub midi: MidiBackend,
    pub efeitos: bool,
    pub direcional_no_manche: bool,
}

impl Default for Ajustes {
    fn default() -> Self {
        Self {
            limite: LimiteFps::Sessenta,
            // Não foi medido no Switch. O automático só pula o quadro que já perdeu o
            // retraco, que é o que a Vita usa no processador quando a volta passa de 16 ms.
            frameskip: Frameskip::Automatico,
            reducao: 1,
            audio: true,
            volume: 1.0,
            midi: MidiBackend::Auto,
            efeitos: true,
            direcional_no_manche: false,
        }
    }
}

pub fn carrega(caminho: &Path) -> Ajustes {
    let Ok(texto) = std::fs::read_to_string(caminho) else {
        return Ajustes::default();
    };
    de_texto(&texto).unwrap_or_default()
}

pub fn de_texto(texto: &str) -> Option<Ajustes> {
    let limpo = sem_comentarios(texto);
    let valor: Value = serde_json::from_str(&limpo).ok()?;
    let mut ajustes = Ajustes::default();
    if let Some(texto) = chave(&valor, "zeebx_limite") {
        if let Some(limite) = LimiteFps::de_texto(&texto) {
            ajustes.limite = limite;
        }
    }
    if let Some(texto) = chave(&valor, "zeebx_frameskip") {
        if let Some(frameskip) = Frameskip::de_texto(&texto) {
            ajustes.frameskip = frameskip;
        }
    }
    if let Some(texto) = chave(&valor, "zeebx_resolucao") {
        ajustes.reducao = reducao_de_texto(&texto);
    }
    if let Some(texto) = chave(&valor, "zeebx_audio") {
        ajustes.audio = ligado(&texto);
    }
    if let Some(texto) = chave(&valor, "zeebx_volume") {
        ajustes.volume = volume_de_texto(&texto);
    }
    if let Some(texto) = chave(&valor, "zeebx_midi") {
        if let Ok(midi) = texto.parse() {
            ajustes.midi = midi;
        }
    }
    if let Some(texto) = chave(&valor, "zeebx_midi_efeitos") {
        ajustes.efeitos = ligado(&texto);
    }
    if let Some(texto) = chave(&valor, "zeebx_direcional_manche") {
        ajustes.direcional_no_manche = ligado(&texto);
    }
    Some(ajustes)
}

fn chave(valor: &Value, nome: &str) -> Option<String> {
    fn anda(valor: &Value, nome: &str, profundidade: u32) -> Option<String> {
        let objeto = valor.as_object()?;
        if let Some(direto) = objeto.get(nome).and_then(como_texto) {
            return Some(direto);
        }
        if profundidade == 0 {
            return None;
        }
        for (chave, filho) in objeto {
            // Um mapa por jogo não entra na leitura global. Se o tico gravar o ajuste
            // deste lançamento na raiz, é essa raiz que vale.
            if chave == "games" || chave == "per_game" {
                continue;
            }
            if let Some(achou) = anda(filho, nome, profundidade - 1) {
                return Some(achou);
            }
        }
        None
    }
    anda(valor, nome, 2)
}

fn como_texto(valor: &Value) -> Option<String> {
    match valor {
        Value::String(texto) => Some(texto.clone()),
        Value::Bool(true) => Some("enabled".to_string()),
        Value::Bool(false) => Some("disabled".to_string()),
        Value::Number(numero) => Some(numero.to_string()),
        _ => None,
    }
}

fn ligado(texto: &str) -> bool {
    matches!(
        texto.trim().to_ascii_lowercase().as_str(),
        "enabled" | "true" | "1" | "on" | "ligado" | "sim"
    )
}

fn volume_de_texto(texto: &str) -> f32 {
    let texto = texto.trim().trim_end_matches('%');
    let Ok(numero) = texto.parse::<f32>() else {
        return 1.0;
    };
    (numero / 100.0).clamp(0.0, 1.0)
}

fn reducao_de_texto(texto: &str) -> usize {
    match texto.trim().to_ascii_lowercase().as_str() {
        "0.5" | "0,5" | "0.5x" => 2,
        "0.25" | "0,25" | "0.25x" => 4,
        _ => 1,
    }
}

/// Tira `//` e `/* */`. Uma barra dentro de string fica: o caminho de um banco pode tê-la.
pub fn sem_comentarios(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    let mut chars = texto.chars().peekable();
    let mut string = false;
    let mut escape = false;
    while let Some(c) = chars.next() {
        if string {
            saida.push(c);
            if escape {
                escape = false;
                continue;
            }
            if c == '\\' {
                escape = true;
                continue;
            }
            if c == '"' {
                string = false;
            }
            continue;
        }
        if c == '"' {
            string = true;
            saida.push(c);
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for d in chars.by_ref() {
                if d == '\n' {
                    saida.push('\n');
                    break;
                }
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            while let Some(d) = chars.next() {
                if d == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        saida.push(c);
    }
    saida
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comentario_nao_apaga_barra_de_caminho_nem_o_valor() {
        let texto = r#"
        {
            // o pulo
            "zeebx_frameskip": "2",
            "zeebx_volume": "50", /* metade */
            "nota": "sdmc://banco"
        }
        "#;
        let ajustes = de_texto(texto).unwrap();
        assert_eq!(ajustes.frameskip, Frameskip::Fixo(2));
        assert!((ajustes.volume - 0.5).abs() < f32::EPSILON);
        assert!(sem_comentarios(texto).contains("sdmc://banco"));
    }

    #[test]
    fn o_ajuste_de_um_jogo_nao_vence_o_global_e_a_falta_fica_no_padrao() {
        let texto = r#"
        {
            "zeebx_audio": "disabled",
            "zeebx_resolucao": "0.5",
            "zeebx_direcional_manche": "enabled",
            "zeebx_midi": "timbres",
            "zeebx_limite": "30",
            "games": { "outro": { "zeebx_audio": "enabled", "zeebx_frameskip": "6" } }
        }
        "#;
        let ajustes = de_texto(texto).unwrap();
        assert!(!ajustes.audio);
        assert_eq!(ajustes.reducao, 2);
        assert!(ajustes.direcional_no_manche);
        assert_eq!(ajustes.midi, MidiBackend::Timbres);
        assert_eq!(ajustes.limite, LimiteFps::Trinta);
        assert_eq!(ajustes.frameskip, Frameskip::Automatico);
    }

    #[test]
    fn um_nivel_abaixo_ainda_conta_quando_a_raiz_nao_tem_a_chave() {
        let texto = r#"{ "options": { "zeebx_midi_efeitos": "disabled" } }"#;
        let ajustes = de_texto(texto).unwrap();
        assert!(!ajustes.efeitos);
    }

    #[test]
    fn json_invalido_nao_e_ajuste() {
        assert!(de_texto("{").is_none());
    }
}
