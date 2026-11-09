#![cfg_attr(target_os = "vita", feature(alloc_error_hook))]
//! Zeebx on the PlayStation Vita, without RetroArch.
//!
//! The emulating is done by [`zeebx::session::Session`]. This binary is what the frontend would do
//! in the core: framebuffer, speaker and controller. `vita.yml` builds the VPK; on the desktop
//! `main` only prints a notice, so that this package's `cargo test` has somewhere to compile it.

mod presentation;
mod host_input;
mod font;
mod list;
mod sound;

fn main() {
    #[cfg(target_os = "vita")]
    vita::run();
    #[cfg(not(target_os = "vita"))]
    eprintln!(
        "zeebx-vita is the PlayStation Vita app. The VPK comes out of frontends/vita-standalone/build.sh."
    );
}

#[cfg(target_os = "vita")]
mod vita;

#[cfg(target_os = "vita")]
#[global_allocator]
static ALLOCATOR: vita::LargeAllocs = vita::LargeAllocs::NEW;
