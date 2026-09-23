//! O que a interface guarda, separado em dois: o que precisa de uma janela do host e o que não
//! precisa.
//!
//! A biblioteca, os ajustes, os saves e o catálogo de idiomas são lidos **pelo núcleo** — o
//! `machine`, o `loader` e a [`crate::session`] falam com eles —, então existem em toda
//! plataforma. A [`app::App`], a vitrine, o pintor de GL e a janela sem interface só existem
//! onde há uma janela de desktop: no Android quem monta a tela é outro frontend, sobre o mesmo
//! núcleo.

/// O repositório do projeto.
///
/// Mora aqui, e não em cada frontend, porque é o mesmo projeto visto de telas diferentes. A
/// primeira versão da tela "sobre" do Android trazia a própria cópia destes dois endereços, e o
/// do Discord estava simplesmente errado — um convite inventado, que não levava a lugar nenhum.
/// Uma constante duplicada não avisa quando as cópias divergem; uma constante só, sim.
pub const REPOSITORIO: &str = "https://github.com/ZeebxTeam/zeebx-emu";

/// O convite do servidor de conversa.
pub const DISCORD: &str = "https://discord.gg/D96HjsKTPa";

pub mod acervo;
// Sem gate, como os irmãos abaixo: o painel só fala com a `session`, o `i18n` e o `settings`,
// e é o mesmo que o frontend de Android desenha. Não há nada de desktop aqui.
pub mod depuracao;
// **O pintor é da feature `gl`, e não do `desktop`.** Ele é o backend de OpenGL do motor: o
// frontend de Android desenha com ele e não tem eframe nenhum. O `desktop` continua trazendo-o,
// porque implica `gpu`, que implica `gl`.
#[cfg(feature = "gl")]
pub mod gpu;
pub mod i18n;
pub mod library;
pub mod saves;
pub mod settings;

#[cfg(feature = "desktop")]
pub mod app;
#[cfg(feature = "desktop")]
pub mod atualizacao;
#[cfg(feature = "desktop")]
pub mod discord;
#[cfg(feature = "desktop")]
pub mod window;

#[cfg(feature = "desktop")]
pub use app::App;
