#![cfg_attr(target_os = "vita", feature(alloc_error_hook))]
//! O Zeebx na PlayStation Vita, sem RetroArch.
//!
//! Quem emula é a [`zeebx::session::Session`]. Este binário é o que no core o frontend faria:
//! framebuffer, alto-falante e controle. O `vita.yml` monta o VPK; no desktop o `main` só avisa,
//! para o `cargo test` deste pacote ter onde compilá-lo.

mod apresentacao;
mod entrada;
mod fonte;
mod lista;
mod som;

fn main() {
    #[cfg(target_os = "vita")]
    vita::roda();
    #[cfg(not(target_os = "vita"))]
    eprintln!(
        "zeebx-vita é o aplicativo da PlayStation Vita. O VPK sai de frontends/vita-standalone/compilar.sh."
    );
}

#[cfg(target_os = "vita")]
mod vita;

#[cfg(target_os = "vita")]
#[global_allocator]
static ALOCADOR: vita::Grandes = vita::Grandes::NOVO;
