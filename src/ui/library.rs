//! Reexports da biblioteca de ROMs para a interface desktop.
//!
//! O catálogo, leitura de `.mif` e extensões pertencem ao motor porque `Session` e SQL também os
//! usam. A UI mantém este caminho público durante a migração para não quebrar seus chamadores.

pub use crate::library::*;
