//! Slots persistentes de save state do frontend Android.
//!
//! O estado da máquina não mora aqui: quem define e valida o formato ZBXS é o núcleo. Este módulo
//! só dá nome aos slots, grava os bytes de forma atômica e guarda uma miniatura opcional. Separar
//! as duas coisas é importante: perder/corromper a miniatura nunca pode tornar um estado válido
//! impossível de carregar.

use std::io::Write;
use std::path::{Path, PathBuf};

use zeebx::session::Session;
use zeebx::video::display::Framebuffer;

pub const SLOTS: usize = 5;
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
    pub miniatura: Option<Miniatura>,
}

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
            miniatura: le_miniatura(&miniatura_path(raiz, id, slot)).ok(),
        })
    })
}

pub fn salva(raiz: &Path, id: &str, slot: usize, sessao: &mut Session) -> Result<Slot, String> {
    valida_slot(slot)?;
    sessao.pode_salvar()?;
    // No caminho de placa, screen() e o framebuffer do aparelho e pode nao ser a textura 3D
    // que o jogador esta vendo. O readback grande existe justamente para capturar o quadro final.
    let miniatura = match sessao.quadro_grande() {
        Some(quadro) => faz_miniatura(&quadro),
        None => faz_miniatura(sessao.screen()),
    };
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
        log::warn!("não gravou a miniatura do save state {}: {erro}", slot + 1);
    }

    Ok(Slot {
        bytes: estado.len() as u64,
        miniatura: Some(miniatura),
    })
}

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
    sessao
        .restaura_estado(&estado)
        .map_err(|e| e.to_string())?;

    Ok(Slot {
        bytes: meta.len(),
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
