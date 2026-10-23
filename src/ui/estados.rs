//! Os slots de save state de cada jogo, gravados no disco.
//!
//! O estado da máquina não mora aqui: quem define e valida o formato ZBXS é o núcleo
//! ([`crate::save_state`]). Este módulo só dá nome aos slots, grava os bytes de forma atômica e
//! guarda uma miniatura opcional. Separar as duas coisas é importante: perder ou corromper a
//! miniatura nunca pode tornar um estado válido impossível de carregar.
//!
//! Nasceu no frontend de Android, na 0.4.0, e veio para cá quando a interface Qt ganhou save
//! state: os dois gravam do mesmo jeito, no mesmo layout, e um estado copiado do celular para o
//! computador aparece no mesmo slot, porque a pasta é a identidade do conteúdo
//! ([`crate::storage::ContentId`]), o hash do mesmo arquivo nos dois.
//!
//! ```text
//! <raiz>/states/<ContentId>/slot-N.zbxstate   o estado
//! <raiz>/states/<ContentId>/slot-N.rgb565     a miniatura, opcional
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::session::Session;
use crate::video::display::Framebuffer;

/// Quantos slots cada jogo tem. Eram cinco no Android; passaram a dez, como no PCSX2, quando a
/// interface Qt ganhou os atalhos de trocar de slot. Os `slot-1` a `slot-5` de antes continuam
/// onde estavam.
pub const SLOTS: usize = 10;
/// O teto do arquivo lido. Um estado de verdade tem dezenas de MB; o teto existe para um arquivo
/// estragado ou estranho não pedir uma alocação absurda.
const MAX_STATE: u64 = 128 * 1024 * 1024;
const MINI_LARGURA: usize = 160;
const MINI_ALTURA: usize = 120;
const MINI_MAGIC: &[u8; 4] = b"ZBXT";

#[derive(Debug, Clone)]
pub struct Miniatura {
    pub largura: usize,
    pub altura: usize,
    pub rgb565: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Slot {
    pub bytes: u64,
    /// Quando o estado foi gravado, pela data do arquivo. `None` onde o sistema não a dá.
    pub gravado: Option<SystemTime>,
    pub miniatura: Option<Miniatura>,
}

/// O que há em cada slot do jogo `id`. Um slot vazio, ilegível ou grande demais é `None`.
pub fn lista(raiz: &Path, id: &str) -> [Option<Slot>; SLOTS] {
    std::array::from_fn(|slot| {
        let path = estado_path(raiz, id, slot);
        recupera_backup(&path);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_STATE {
            return None;
        }
        Some(Slot {
            bytes: meta.len(),
            gravado: meta.modified().ok(),
            miniatura: le_miniatura(&miniatura_path(raiz, id, slot)).ok(),
        })
    })
}

/// Grava o estado da sessão no `slot`, por cima do que houver nele.
///
/// **Com o contexto de GL do jogo corrente**, quando ele desenha pela placa: a miniatura e o
/// quadro que entra no estado são lidos da textura, e a textura só se lê nele.
pub fn salva(raiz: &Path, id: &str, slot: usize, sessao: &mut Session) -> Result<Slot, String> {
    valida_slot(slot)?;
    sessao.pode_salvar()?;
    // Quem divide o contexto com o motor pode ter pintado nele desde o último passo.
    sessao.retoma_o_contexto();
    // No caminho de placa, `screen()` é o framebuffer do aparelho, e pode não ser a textura 3D que
    // o jogador está vendo. O readback grande existe justamente para capturar o quadro final.
    let miniatura = match sessao.quadro_grande() {
        Some(quadro) => faz_miniatura(&quadro),
        None => faz_miniatura(sessao.screen()),
    };
    // **O estado leva a imagem do instante**, como o ponto do rewind: pela placa, o último quadro
    // fica na textura, e a textura não entra no estado. Sem trazê-lo para a tela, carregar
    // voltava com a tela de um quadro antigo, ou preta num jogo que só desenha pelo GL. Ver
    // `docs/implementacao/24-velocidade.md`.
    sessao.materializa_quadro_gl();
    let estado = sessao.grava_estado();
    if estado.is_empty() {
        return Err("o núcleo devolveu um save state vazio".to_string());
    }
    if estado.len() as u64 > MAX_STATE {
        return Err(format!(
            "o save state passou do limite de {} MB",
            MAX_STATE / (1024 * 1024)
        ));
    }

    let dir = pasta_do_jogo(raiz, id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = estado_path(raiz, id, slot);
    recupera_backup(&path);
    grava_atomico(&path, &estado)?;

    // O estado já está seguro no disco. Uma falha na miniatura não pode transformar sucesso em
    // erro: a próxima abertura só mostra o espaço sem imagem.
    if let Err(erro) = grava_miniatura(&miniatura_path(raiz, id, slot), &miniatura) {
        crate::registro!(
            crate::registro::Nivel::Aviso,
            "estados",
            "não gravou a miniatura do save state {}: {erro}",
            slot + 1
        );
    }

    Ok(Slot {
        bytes: estado.len() as u64,
        gravado: Some(SystemTime::now()),
        miniatura: Some(miniatura),
    })
}

/// Põe na sessão o estado do `slot`. Recusado, a sessão continua como estava: o motor lê o
/// arquivo inteiro antes de escrever qualquer coisa.
///
/// Com o contexto de GL do jogo corrente, como o [`salva`]: o estado restaurado refaz as texturas
/// na placa.
pub fn carrega(raiz: &Path, id: &str, slot: usize, sessao: &mut Session) -> Result<Slot, String> {
    valida_slot(slot)?;
    let path = estado_path(raiz, id, slot);
    recupera_backup(&path);
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        return Err("o save state está vazio".to_string());
    }
    if meta.len() > MAX_STATE {
        return Err(format!(
            "o save state passou do limite de {} MB",
            MAX_STATE / (1024 * 1024)
        ));
    }
    let estado = std::fs::read(&path).map_err(|e| e.to_string())?;
    sessao.retoma_o_contexto();
    sessao.restaura_estado(&estado).map_err(|e| e.to_string())?;

    Ok(Slot {
        bytes: meta.len(),
        gravado: meta.modified().ok(),
        miniatura: le_miniatura(&miniatura_path(raiz, id, slot)).ok(),
    })
}

fn valida_slot(slot: usize) -> Result<(), String> {
    (slot < SLOTS)
        .then_some(())
        .ok_or_else(|| "slot de save state inválido".to_string())
}

fn pasta_do_jogo(raiz: &Path, id: &str) -> PathBuf {
    raiz.join("states").join(id)
}

fn estado_path(raiz: &Path, id: &str, slot: usize) -> PathBuf {
    pasta_do_jogo(raiz, id).join(format!("slot-{}.zbxstate", slot + 1))
}

fn recupera_backup(path: &Path) {
    let bak = path.with_extension("zbxstate.bak");
    // Uma queda depois de mover o estado velho, mas antes de publicar o novo, deixa somente o
    // backup. Ele continua sendo o último estado confirmado e deve reaparecer automaticamente.
    if !path.exists() && bak.is_file() {
        let _ = std::fs::rename(bak, path);
    }
}

fn miniatura_path(raiz: &Path, id: &str, slot: usize) -> PathBuf {
    pasta_do_jogo(raiz, id).join(format!("slot-{}.rgb565", slot + 1))
}

fn grava_atomico(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("zbxstate.tmp");
    let bak = path.with_extension("zbxstate.bak");
    let _ = std::fs::remove_file(&tmp);

    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }

    let _ = std::fs::remove_file(&bak);
    if path.exists() {
        std::fs::rename(path, &bak).map_err(|e| e.to_string())?;
    }
    if let Err(erro) = std::fs::rename(&tmp, path) {
        if bak.exists() {
            let _ = std::fs::rename(&bak, path);
        }
        return Err(erro.to_string());
    }
    let _ = std::fs::remove_file(bak);
    Ok(())
}

fn faz_miniatura(frame: &Framebuffer) -> Miniatura {
    let origem_lg = frame.width().max(1) as usize;
    let origem_at = frame.height().max(1) as usize;
    let escala = (MINI_LARGURA as f32 / origem_lg as f32)
        .min(MINI_ALTURA as f32 / origem_at as f32)
        .min(1.0);
    let largura = ((origem_lg as f32 * escala).round() as usize).max(1);
    let altura = ((origem_at as f32 * escala).round() as usize).max(1);
    let mut rgb565 = Vec::with_capacity(largura * altura * 2);

    for y in 0..altura {
        let origem_y = y * origem_at / altura;
        for x in 0..largura {
            let origem_x = x * origem_lg / largura;
            rgb565.extend_from_slice(
                &frame
                    .get_pixel(origem_x as i32, origem_y as i32)
                    .to_le_bytes(),
            );
        }
    }

    Miniatura {
        largura,
        altura,
        rgb565,
    }
}

fn grava_miniatura(path: &Path, miniatura: &Miniatura) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(8 + miniatura.rgb565.len());
    bytes.extend_from_slice(MINI_MAGIC);
    bytes.extend_from_slice(&(miniatura.largura as u16).to_le_bytes());
    bytes.extend_from_slice(&(miniatura.altura as u16).to_le_bytes());
    bytes.extend_from_slice(&miniatura.rgb565);
    let tmp = path.with_extension("rgb565.tmp");
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn le_miniatura(path: &Path) -> Result<Miniatura, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 8 || &bytes[..4] != MINI_MAGIC {
        return Err("miniatura inválida".to_string());
    }
    let largura = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let altura = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    if largura == 0 || altura == 0 || largura > MINI_LARGURA || altura > MINI_ALTURA {
        return Err("dimensões da miniatura inválidas".to_string());
    }
    let esperado = largura * altura * 2;
    if bytes.len() != 8 + esperado {
        return Err("miniatura truncada".to_string());
    }
    Ok(Miniatura {
        largura,
        altura,
        rgb565: bytes[8..].to_vec(),
    })
}

pub fn tamanho(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::sessao_minima_para_save_state;

    const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn raiz(nome: &str) -> PathBuf {
        let raiz = std::env::temp_dir().join(format!(
            "zeebx-testes-estados-{nome}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&raiz);
        raiz
    }

    #[test]
    fn o_estado_vai_ao_slot_e_volta_dele() {
        let raiz = raiz("ida-e-volta");
        let mut sessao = sessao_minima_para_save_state();
        assert!(lista(&raiz, ID).iter().all(Option::is_none));

        let salvo = salva(&raiz, ID, 9, &mut sessao).unwrap();
        assert!(salvo.bytes > 0);
        assert!(
            raiz.join("states")
                .join(ID)
                .join("slot-10.zbxstate")
                .is_file()
        );

        let slots = lista(&raiz, ID);
        assert!(slots[..9].iter().all(Option::is_none));
        let decimo = slots[9]
            .as_ref()
            .expect("o slot 10 devia aparecer na lista");
        assert_eq!(decimo.bytes, salvo.bytes);
        assert!(decimo.miniatura.is_some());

        carrega(&raiz, ID, 9, &mut sessao).unwrap();
        let _ = std::fs::remove_dir_all(&raiz);
    }

    #[test]
    fn slot_fora_da_conta_e_recusado() {
        let raiz = raiz("fora");
        let mut sessao = sessao_minima_para_save_state();
        assert!(salva(&raiz, ID, SLOTS, &mut sessao).is_err());
        assert!(carrega(&raiz, ID, SLOTS, &mut sessao).is_err());
        assert!(!raiz.exists(), "a recusa não pode criar a pasta");
    }

    /// Um estado estragado é recusado pelo motor, e a sessão segue: é o que deixa carregar um
    /// arquivo truncado sem perder o jogo em curso.
    #[test]
    fn estado_estragado_e_recusado() {
        let raiz = raiz("estragado");
        let mut sessao = sessao_minima_para_save_state();
        salva(&raiz, ID, 0, &mut sessao).unwrap();
        let path = estado_path(&raiz, ID, 0);
        let mut bytes = std::fs::read(&path).unwrap();
        let meio = bytes.len() / 2;
        bytes[meio] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        assert!(carrega(&raiz, ID, 0, &mut sessao).is_err());
        let _ = std::fs::remove_dir_all(&raiz);
    }

    /// A miniatura é um enfeite: estragada, o slot aparece sem imagem e carrega igual.
    #[test]
    fn miniatura_estragada_nao_impede_o_estado() {
        let raiz = raiz("miniatura");
        let mut sessao = sessao_minima_para_save_state();
        salva(&raiz, ID, 1, &mut sessao).unwrap();
        std::fs::write(miniatura_path(&raiz, ID, 1), b"lixo").unwrap();

        let slot = lista(&raiz, ID)[1]
            .clone()
            .expect("o estado continua no slot");
        assert!(slot.miniatura.is_none());
        let carregado = carrega(&raiz, ID, 1, &mut sessao).unwrap();
        assert!(carregado.miniatura.is_none());
        let _ = std::fs::remove_dir_all(&raiz);
    }

    /// A queda entre mover o estado velho e publicar o novo deixa só o `.bak`, e ele volta a ser
    /// o estado do slot.
    #[test]
    fn o_backup_de_uma_gravacao_interrompida_volta() {
        let raiz = raiz("backup");
        let mut sessao = sessao_minima_para_save_state();
        salva(&raiz, ID, 2, &mut sessao).unwrap();
        let path = estado_path(&raiz, ID, 2);
        std::fs::rename(&path, path.with_extension("zbxstate.bak")).unwrap();

        assert!(lista(&raiz, ID)[2].is_some());
        assert!(path.is_file());
        let _ = std::fs::remove_dir_all(&raiz);
    }

    #[test]
    fn tamanho_escolhe_a_unidade() {
        assert_eq!(tamanho(512), "512 B");
        assert_eq!(tamanho(3 * 1024), "3 KB");
        assert_eq!(tamanho(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
    }
}
