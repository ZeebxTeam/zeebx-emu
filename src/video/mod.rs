//! A saída gráfica: do framebuffer do console ao rasterizador do OpenGL ES.

pub mod atc;
#[cfg(feature = "desktop")]
pub mod contexto;
pub mod display;
pub mod font;
pub mod gif;
pub mod gles;
#[cfg(feature = "desktop")]
pub mod gpu;
pub mod icon;
pub mod paltex;
pub mod rasterizer;
