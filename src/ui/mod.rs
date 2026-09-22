//! A interface do emulador: biblioteca de jogos, configurações e a tela do console.

pub mod i18n;
pub mod library;
pub mod saves;
pub mod settings;

#[cfg(feature = "desktop")]
pub mod acervo;
#[cfg(feature = "desktop")]
pub mod atualizacao;
#[cfg(feature = "desktop")]
pub mod discord;
#[cfg(feature = "desktop")]
pub mod gpu;
#[cfg(feature = "desktop")]
pub mod window;
#[cfg(feature = "desktop")]
mod shell;

#[cfg(feature = "desktop")]
pub use shell::App;
