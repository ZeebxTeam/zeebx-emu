//! Re-exports of the ROM library for the desktop interface.
//!
//! The catalog, `.mif` reading and extensions belong to the engine because `Session` and SQL use
//! them too. The UI keeps this public path during the migration so as not to break its callers.

pub use crate::library::*;
