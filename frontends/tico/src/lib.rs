//! O Zeebx como módulo do tico, no Nintendo Switch.
//!
//! O tico não carrega uma biblioteca: ele abre um `.nro` com o jogo na linha de comando e,
//! quando o jogo acaba, espera que esse `.nro` devolva o controle com `--resume`. Quem emula
//! é a [`zeebx::session::Session`]. Este crate é o laço — controle, quadro e som passam pelo
//! hospedeiro em C, que é quem fala com o libnx. No desktop o `cargo test` prova o contrato
//! (argumentos, ajustes, botões, quadro) sem o aparelho.

mod ajustes;
mod apresentacao;
mod entrada;
mod lancamento;
mod partida;
mod som;

#[cfg(test)]
mod manifesto;

use std::ffi::{c_char, CStr};

use entrada::Leitura;

/// O que o `.nro` oferece ao laço. Ponteiros nulos são "não tenho isto": o jogo segue sem
/// imagem, sem som ou sem o aviso na tela, em vez de derrubar o processo.
#[repr(C)]
pub struct ZeebxHospedeiro {
    pub apresenta: Option<unsafe extern "C" fn(*const u16, u32, u32)>,
    pub audio: Option<unsafe extern "C" fn(*const i16, u32)>,
    pub entrada: Option<unsafe extern "C" fn(*mut Leitura)>,
    pub erro: Option<unsafe extern "C" fn(*const c_char)>,
}

/// Abre o jogo e roda até sair ou até o jogador pedir a volta ao tico.
///
/// A ordem dos ponteiros é a do `launch` em `module.json`: jogo, título, saves, states,
/// system, roms. `config` é o `zeebx.jsonc` que o tico grava; nulo usa o caminho de fábrica.
/// Devolve 0 quando a volta ao tico é o fim esperado, e outro valor quando o jogo nem abriu.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeebx_tico_roda(
    rom: *const c_char,
    titulo: *const c_char,
    saves: *const c_char,
    states: *const c_char,
    system: *const c_char,
    roms: *const c_char,
    config: *const c_char,
    hospedeiro: *const ZeebxHospedeiro,
) -> i32 {
    let Some(hospedeiro) = (unsafe { hospedeiro.as_ref() }) else {
        return 2;
    };
    let pedido = lancamento::Pedido {
        rom: texto(rom),
        titulo: texto(titulo).unwrap_or_default(),
        saves: texto(saves),
        states: texto(states),
        system: texto(system),
        roms: texto(roms),
        config: texto(config),
    };
    match lancamento::resolve(pedido) {
        Ok(args) => partida::roda(&args, hospedeiro),
        Err(erro) => {
            partida::avisa(hospedeiro, &erro);
            1
        }
    }
}

/// Copia o quadro do console para o framebuffer linear do Switch.
///
/// `passo_bytes` é a distância entre linhas que o `framebufferBegin` devolve, em bytes, e não
/// em pixels: no modo linear do libnx a linha vem alinhada, e pode ser mais larga que a tela.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zeebx_copia_rgb565(
    destino: *mut u16,
    passo_bytes: u32,
    dest_w: u32,
    dest_h: u32,
    origem: *const u16,
    origem_w: u32,
    origem_h: u32,
) {
    if destino.is_null() || origem.is_null() || passo_bytes < 2 || dest_h == 0 {
        return;
    }
    let passo = passo_bytes as usize;
    let linhas = dest_h as usize;
    let pixels = passo / 2 * linhas;
    let destino = unsafe { std::slice::from_raw_parts_mut(destino, pixels) };
    let origem_n = (origem_w as usize).saturating_mul(origem_h as usize);
    let origem = unsafe { std::slice::from_raw_parts(origem, origem_n) };
    apresentacao::copia_rgb565(
        destino,
        passo,
        dest_w as usize,
        dest_h as usize,
        origem,
        origem_w as usize,
        origem_h as usize,
    );
}

fn texto(ponteiro: *const c_char) -> Option<String> {
    if ponteiro.is_null() {
        return None;
    }
    let texto = unsafe { CStr::from_ptr(ponteiro) }.to_string_lossy();
    let texto = texto.trim().trim_end_matches('/').trim_end_matches('\\');
    if texto.is_empty() {
        None
    } else {
        Some(texto.to_string())
    }
}
