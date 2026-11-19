//! An **offscreen** OpenGL context, for the rasterizer on the graphics card.
//!
//! The emulator core has no window. The command line's `run` path has none at all, and that is
//! exactly where surfaces get compared. A GPU backend that only worked with a window would be a
//! backend that cannot be measured.
//!
//! That is why the context is our own, independent of the one the window creates for the UI.
//! Besides giving both modes a single path, it avoids coupling the core to the render thread and
//! to the UI's GL state — the UI draws in that same context and does not expect to find it
//! tampered with.
//!
//! The surface is a 1x1 pbuffer that is never drawn to: the real target is a framebuffer of our
//! own, created by the backend at the size of the console surface. The pbuffer exists only
//! because EGL requires a surface to make a context current.

//!
//! **macOS has no EGL** — `glutin` only offers CGL, which requires a window. There the offscreen
//! context answers that it does not exist, and the windowless path uses the software rasterizer.
//! With a window nothing changes: the backend receives the window's context.

use glow;
#[cfg(not(target_os = "macos"))]
use glutin::config::{ConfigSurfaceTypes, ConfigTemplateBuilder};
#[cfg(not(target_os = "macos"))]
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, Version};
use glutin::context::PossiblyCurrentContext;
#[cfg(not(target_os = "macos"))]
use glutin::display::{DisplayApiPreference, GlDisplay};
use glutin::display::Display;
#[cfg(not(target_os = "macos"))]
use glutin::surface::SurfaceAttributesBuilder;
use glutin::surface::{PbufferSurface, Surface};
#[cfg(not(target_os = "macos"))]
use raw_window_handle::{RawDisplayHandle, XlibDisplayHandle};
#[cfg(not(target_os = "macos"))]
use std::num::NonZeroU32;

/// The context and the function loader, alive for as long as the backend exists.
pub struct GlContext {
    /// The already-resolved GL functions. This is what the backend uses.
    ///
    /// Shareable because the windowed path does **not** create a context: it receives the
    /// window's. See [`crate::video::gpu::GpuState::create`].
    pub gl: std::sync::Arc<glow::Context>,
    /// The current context. Dropped at the end, after `gl`.
    _context: PossiblyCurrentContext,
    /// The minimal surface EGL requires to make the context current.
    _surface: Surface<PbufferSurface>,
    /// The display comes last on purpose: it is what created the two above.
    _display: Display,
}

impl GlContext {
    /// Opens an offscreen context, or says why it could not.
    ///
    /// Failing here is a normal case, not a program error: in a terminal with no reachable EGL
    /// there is no graphics card to use. The caller handles the `Err` by falling back to the
    /// software rasterizer.
    #[cfg(target_os = "macos")]
    pub fn create() -> Result<Self, String> {
        Err("macOS has no EGL for an offscreen context".to_string())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn create() -> Result<Self, String> {
        // `display: None` is `EGL_DEFAULT_DISPLAY`: it asks EGL for the display it considers the
        // default, without needing a window connection opened by us.
        let handle = RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0));
        let display = unsafe { Display::new(handle, DisplayApiPreference::Egl) }
            .map_err(|error| format!("could not open the EGL display: {error}"))?;

        // Depth and stencil are requirements of the console, not decoration: the Z-Wheel stage
        // marks the floor in the stencil to draw the reflection, and without eight bits there the
        // reflection is lost.
        let template = ConfigTemplateBuilder::new()
            .with_depth_size(24)
            .with_stencil_size(8)
            .with_surface_type(ConfigSurfaceTypes::PBUFFER)
            .build();
        let config = unsafe { display.find_configs(template) }
            .map_err(|error| format!("could not list configs: {error}"))?
            .next()
            .ok_or("no config with 24-bit depth and 8-bit stencil")?;

        // The shader is written in GLSL 3.30, so the request is for OpenGL 3.3 core. GLES 3.x is
        // the fallback for cards that only offer the embedded profile — the same pair of attempts
        // the UI painter already makes.
        let context = [
            ContextApi::OpenGl(Some(Version::new(3, 3))),
            ContextApi::Gles(Some(Version::new(3, 0))),
        ]
        .into_iter()
        .find_map(|api| {
            let attrs = ContextAttributesBuilder::new().with_context_api(api).build(None);
            unsafe { display.create_context(&config, &attrs) }.ok()
        })
        .ok_or("neither OpenGL 3.3 nor GLES 3.x was accepted")?;

        let one = NonZeroU32::new(1).expect("1 is not zero");
        let attrs = SurfaceAttributesBuilder::<PbufferSurface>::new().build(one, one);
        let surface = unsafe { display.create_pbuffer_surface(&config, &attrs) }
            .map_err(|error| format!("could not create the pbuffer: {error}"))?;
        let context = context
            .make_current(&surface)
            .map_err(|error| format!("could not make the context current: {error}"))?;

        let gl = unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name).cast())
        };
        Ok(Self {
            gl: std::sync::Arc::new(gl),
            _context: context,
            _surface: surface,
            _display: display,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opens the context and asks who answered.
    ///
    /// On a machine with no reachable EGL — a bare terminal, a container with no graphics card —
    /// having no context **is not a program failure**: the backend falls back to the software
    /// rasterizer. So the test reports the reason and passes, instead of demanding a graphics card
    /// from whoever runs the suite.
    #[test]
    fn the_offscreen_context_opens_or_says_why_not() {
        match GlContext::create() {
            Ok(context) => {
                use glow::HasContext;
                let version = unsafe { context.gl.get_parameter_string(glow::VERSION) };
                let gpu = unsafe { context.gl.get_parameter_string(glow::RENDERER) };
                println!("context opened: {version} — {gpu}");
                assert!(!version.is_empty(), "context without a GL version");
            }
            Err(reason) => println!("no context on this machine: {reason}"),
        }
    }
}


#[cfg(test)]
mod context_tests {
    use super::*;

    /// **The graphics card context opens without a window, and that is the first thing to know.**
    ///
    /// Hardware rendering is only verifiable if the offscreen path works: with a window, the one
    /// measuring is whoever is looking. If EGL is not reachable — a terminal with no graphics
    /// card, a container — the test says so and passes: that is the normal case the module
    /// anticipates, and it is exactly why the emulator falls back to the software rasterizer.
    #[test]
    fn the_offscreen_context_opens_or_says_why_not() {
        match GlContext::create() {
            Ok(context) => {
                let version = unsafe {
                    use glow::HasContext;
                    context.gl.get_parameter_string(glow::VERSION)
                };
                eprintln!("graphics card available: {version}");
            }
            Err(reason) => eprintln!("no offscreen graphics card: {reason}"),
        }
    }
}
