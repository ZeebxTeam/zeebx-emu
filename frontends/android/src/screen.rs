//! The Android window as an OpenGL ES surface, and the egui painter on top of it.
//!
//! Without eframe, the context is ours. What Android hands over is an `ANativeWindow`; the rest
//! is EGL, and what speaks EGL here is glutin — the same crate the desktop already uses for the
//! rasterizer's offscreen context.
//!
//! **The context and the surface have different lifetimes, and that is what separates the two
//! types here.** The Android window is born and dies many times: switching apps, turning off the
//! screen, rotating the device. The surface goes with it. The context does not — and cannot,
//! because the GPU rasterizer keeps textures, buffers and programs created in it. Destroying the
//! context when switching apps would leave the session with a pile of object names that no
//! longer exist, and the first draw after coming back would be a crash inside the driver.
//!
//! That is why the [`Gpu`] is created once and lives until the end, and the [`Screen`] is the
//! surface, made and unmade with the window.

use std::num::NonZeroU32;
use std::sync::Arc;

use android_activity::AndroidApp;
use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext,
    PossiblyCurrentGlContext, Version,
};
use glutin::display::{Display, DisplayApiPreference, GlDisplay};
use glutin::surface::{
    GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface,
};
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, RawDisplayHandle, RawWindowHandle,
};

/// What outlives the window: the EGL display, the context, `glow` and the egui painter.
pub struct Gpu {
    display: Display,
    config: Config,
    context: PossiblyCurrentContext,
    /// The same `glow` the session receives when 3D runs on the GPU.
    pub gl: Arc<glow::Context>,
    pub painter: egui_glow::Painter,
    /// Swap interval applied to the current surface. `None` means the driver refused explicit
    /// control and should keep its own default policy.
    interval: Option<bool>,
}

/// The surface of the current window.
pub struct Screen {
    surface: Surface<WindowSurface>,
    /// The surface size in pixels. egui works in points; the conversion is the density.
    pub size: [u32; 2],
}

impl Gpu {
    /// Builds the context on the first window the activity creates.
    ///
    /// It needs a window **in this** call because EGL picks the config by the surface format and
    /// because a context only becomes current with a surface; from then on the window can come and
    /// go as it pleases.
    pub fn create(app: &AndroidApp, throttle: bool) -> Result<(Self, Screen), String> {
        let window = app.native_window().ok_or("the activity has no window")?;
        let width = NonZeroU32::new(window.width() as u32).ok_or("window with no width")?;
        let height = NonZeroU32::new(window.height() as u32).ok_or("window with no height")?;

        let raw_window =
            RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window.ptr().cast()));
        let raw_display = RawDisplayHandle::Android(AndroidDisplayHandle::new());

        // On Android there is only EGL: saying so explicitly keeps glutin from trying GLX.
        let display = unsafe { Display::new(raw_display, DisplayApiPreference::Egl) }
            .map_err(|error| format!("no EGL display: {error}"))?;

        let model = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .compatible_with_native_window(raw_window)
            .build();
        // The best config is the one with the most samples that still matches the window.
        let config = unsafe { display.find_configs(model) }
            .map_err(|error| format!("no EGL config: {error}"))?
            .reduce(|a, b| if b.num_samples() > a.num_samples() { b } else { a })
            .ok_or("no EGL config fits this window")?;

        // The manifest asks for GLES 3.0, and that is what the core's rasterizer expects when the
        // GPU is turned on. Asking explicitly avoids falling into a 2.0 context by default.
        let attributes = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(3, 0))))
            .build(Some(raw_window));
        let context = unsafe { display.create_context(&config, &attributes) }
            .map_err(|error| format!("no GL context: {error}"))?;

        let surface = create_surface(&display, &config, raw_window, width, height)?;
        let context = context
            .make_current(&surface)
            .map_err(|error| format!("the context did not become current: {error}"))?;
        let interval = set_vsync(&context, &surface, throttle).then_some(throttle);

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        });
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false)
            .map_err(|error| format!("egui_glow failed to set up: {error}"))?;

        log::info!("screen of {}x{}", width.get(), height.get());
        Ok((
            Self {
                display,
                config,
                context,
                gl,
                painter,
                interval,
            },
            Screen {
                surface,
                size: [width.get(), height.get()],
            },
        ))
    }

    /// Makes the surface of the window the activity just created, with the context that already
    /// exists.
    pub fn recreate_screen(&mut self, app: &AndroidApp, throttle: bool) -> Result<Screen, String> {
        let window = app.native_window().ok_or("the activity has no window")?;
        let width = NonZeroU32::new(window.width() as u32).ok_or("window with no width")?;
        let height = NonZeroU32::new(window.height() as u32).ok_or("window with no height")?;
        let raw_window =
            RawWindowHandle::AndroidNdk(AndroidNdkWindowHandle::new(window.ptr().cast()));

        let surface =
            create_surface(&self.display, &self.config, raw_window, width, height)?;
        self.context
            .make_current(&surface)
            .map_err(|error| format!("the context did not become current: {error}"))?;
        self.interval =
            set_vsync(&self.context, &surface, throttle).then_some(throttle);
        log::info!("screen remade at {}x{}", width.get(), height.get());
        Ok(Screen {
            surface,
            size: [width.get(), height.get()],
        })
    }

    /// The window changed size — it rotated, or the system bar showed up.
    pub fn resize(&self, screen: &mut Screen, app: &AndroidApp) {
        let Some(window) = app.native_window() else {
            return;
        };
        let (Some(width), Some(height)) = (
            NonZeroU32::new(window.width() as u32),
            NonZeroU32::new(window.height() as u32),
        ) else {
            return;
        };
        screen.surface.resize(&self.context, width, height);
        screen.size = [width.get(), height.get()];
    }

    /// Draws what egui produced and swaps the buffers.
    pub fn paint(
        &mut self,
        screen: &Screen,
        primitives: &[egui::ClippedPrimitive],
        textures: &egui::TexturesDelta,
        points_per_pixel: f32,
        throttle: bool,
    ) {
        if self.interval.is_some_and(|current| current != throttle) {
            self.interval =
                set_vsync(&self.context, &screen.surface, throttle).then_some(throttle);
        }
        self.painter.clear(screen.size, [0.0, 0.0, 0.0, 1.0]);
        self.painter
            .paint_and_update_textures(screen.size, points_per_pixel, primitives, textures);
        if let Err(error) = screen.surface.swap_buffers(&self.context) {
            log::error!("the buffer swap failed: {error}");
        }
    }

    /// Becomes the current context again after the activity has been resumed.
    pub fn resume(&self, screen: &Screen) {
        if !self.context.is_current() {
            if let Err(error) = self.context.make_current(&screen.surface) {
                log::error!("the context did not become current again: {error}");
            }
        }
    }
}

fn set_vsync(
    context: &PossiblyCurrentContext,
    surface: &Surface<WindowSurface>,
    throttle: bool,
) -> bool {
    let interval = match throttle {
        true => SwapInterval::Wait(NonZeroU32::new(1).expect("one is nonzero")),
        false => SwapInterval::DontWait,
    };
    if let Err(error) = surface.set_swap_interval(context, interval) {
        // Some Android drivers ignore or refuse an explicit interval. In that case the swap still
        // works with EGL's default policy, so this is no reason to lose the screen.
        log::warn!("could not set the EGL swap interval: {error}");
        false
    } else {
        true
    }
}

fn create_surface(
    display: &Display,
    config: &Config,
    window: RawWindowHandle,
    width: NonZeroU32,
    height: NonZeroU32,
) -> Result<Surface<WindowSurface>, String> {
    let attributes = SurfaceAttributesBuilder::<WindowSurface>::new().build(window, width, height);
    unsafe { display.create_window_surface(config, &attributes) }
        .map_err(|error| format!("no surface: {error}"))
}
