//! The bare window: the game and nothing else.
//!
//! **There is no eframe here, and that is the reason this file exists** — for the same reason the
//! Android frontend has its own. eframe exists to put an interface on screen, and here there is no
//! interface: there is a frame, a window and an OpenGL context the session will use to rasterize
//! 3D. Spinning an egui loop underneath nothing would be paying for the whole interface to draw a
//! rectangle.
//!
//! The context **is the window's**, and not the offscreen one from [`zeebx::video::context`].
//! With a window, the frame the GPU just filled is already in its memory: painting it is binding
//! the texture and drawing a triangle. Through the offscreen context it would have to come back to
//! the CPU and be uploaded again, once per frame.

use std::num::NonZeroU32;
use std::sync::Arc;

use glutin::config::{Config, ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowAttributes};

use zeebx::ui::gpu::GlPainter;
use zeebx::ui::settings::Scaling;

use crate::config::{Headless, Video};

/// The window, the context and the painter that puts the frame in it.
pub struct HeadlessWindow {
    pub window: Window,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    /// The same `glow` the session receives when 3D runs on the GPU.
    pub gl: Arc<glow::Context>,
    gl_painter: Option<GlPainter>,
}

impl HeadlessWindow {
    /// Opens the window and sets up the context. Only works inside winit's `resumed`: before that
    /// there is no active loop to create any window.
    pub fn open(run_loop: &ActiveEventLoop, options: &Headless) -> Result<Self, String> {
        let (width, height) = options.size;
        let attributes = WindowAttributes::default()
            .with_title(options.title.clone())
            .with_inner_size(winit::dpi::PhysicalSize::new(width, height))
            // Fullscreen is the "borderless" kind, not exclusive: switching the monitor's video
            // mode to show an upscaled 640×480 serves no one, and getting out of an exclusive one
            // that froze is much worse than getting out of a window.
            .with_fullscreen(match options.video {
                Video::Fullscreen => Some(Fullscreen::Borderless(None)),
                _ => None,
            });

        // No alpha and no depth on the window: what gets drawn here is an opaque rectangle with a
        // texture. The depth 3D uses is that of the rasterizer's framebuffer, which belongs to it
        // and not to this surface.
        let model = ConfigTemplateBuilder::new().with_alpha_size(0);
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attributes))
            .build(run_loop, model, choose_config)
            .map_err(|error| format!("no OpenGL window: {error}"))?;
        let window = window.ok_or("the system returned no window")?;

        let raw_window = window
            .window_handle()
            .map_err(|error| format!("window without a handle: {error}"))?
            .as_raw();
        let display = config.display();
        // Without asking for a version or profile: glutin resolves that to the best the driver
        // offers, and it is the same eframe did in the egui window — which keeps the
        // [`GlPainter`] shaders working.
        let attributes = ContextAttributesBuilder::new().build(Some(raw_window));
        let context = unsafe { display.create_context(&config, &attributes) }
            .map_err(|error| format!("no GL context: {error}"))?;

        let attributes = window
            .build_surface_attributes(Default::default())
            .map_err(|error| format!("the window describes no surface: {error}"))?;
        let surface = unsafe { display.create_window_surface(&config, &attributes) }
            .map_err(|error| format!("no GL surface: {error}"))?;
        let context = context
            .make_current(&surface)
            .map_err(|error| format!("the context did not become current: {error}"))?;

        // Vsync is the choice of whoever configured it. On, the window swaps the frame at the
        // retrace and the loop sleeps on it; off, it runs free — which is what someone measuring
        // wants.
        let interval = match options.vsync {
            true => SwapInterval::Wait(NonZeroU32::new(1).unwrap()),
            false => SwapInterval::DontWait,
        };
        if let Err(error) = surface.set_swap_interval(&context, interval) {
            eprintln!("warning: the driver refused the requested vsync: {error}");
        }

        let gl = Arc::new(unsafe {
            glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name))
        });
        // The painter is optional: a driver that rejects the shaders leaves the window black
        // instead of bringing down the emulator, and the message says why.
        let gl_painter = match GlPainter::create(&gl) {
            Ok(gl_painter) => Some(gl_painter),
            Err(error) => {
                eprintln!("warning: without the GL painter the window stays black: {error}");
                None
            }
        };

        Ok(Self {
            window,
            surface,
            context,
            gl,
            gl_painter,
        })
    }

    /// The surface size in pixels.
    pub fn size(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width.max(1), size.height.max(1))
    }

    /// Follows the window when it changes size.
    pub fn resize(&self, width: u32, height: u32) {
        let (Some(width), Some(height)) = (NonZeroU32::new(width), NonZeroU32::new(height))
        else {
            return;
        };
        self.surface.resize(&self.context, width, height);
    }

    /// Paints the frame the GPU filled, without it coming back to the CPU.
    /// `native_height` is that of the console frame, which is the unit of the integer scale — the
    /// texture already comes enlarged by the internal resolution, and counting multiples on it
    /// would give a window full of border for no reason.
    pub fn show_texture(&mut self, frame: zeebx::video::rasterizer::GpuFrame, native_height: u32, scale: Scaling, aspect_ratio_4_3: bool, smooth: bool) {
        let aspect = match aspect_ratio_4_3 {
            true => Some(frame.ratio),
            false => None,
        };
        let vp = self.clear_and_fit(native_height, aspect, scale);
        if let Some(gl_painter) = &mut self.gl_painter {
            gl_painter.draw_texture(&self.gl, frame, vp.into(), smooth);
        }
        self.swap();
    }

    /// Paints the device frame, in RGB565 — which is the format it is already in.
    pub fn show_frame(&mut self, bytes: &[u8], width: i32, height: i32, scale: Scaling, aspect_ratio_4_3: bool, smooth: bool) {
        let aspect = match aspect_ratio_4_3 && height > 0 {
            true => Some(width as f32 / height as f32),
            false => None,
        };
        let vp = self.clear_and_fit(height.max(0) as u32, aspect, scale);
        if let Some(gl_painter) = &mut self.gl_painter {
            gl_painter.draw(&self.gl, bytes, width, height, vp.into(), smooth);
        }
        self.swap();
    }

    /// Clears the window to black and returns the rectangle the frame fits in.
    ///
    /// The clear comes first because the leftover border belongs to the window, not the game:
    /// without it the side bands would keep the garbage of the previous frame.
    fn clear_and_fit(&self, native_height: u32, aspect: Option<f32>, scale: Scaling) -> egui::epaint::ViewportInPixels {
        use glow::HasContext;
        let (width, height) = self.size();
        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.viewport(0, 0, width as i32, height as i32);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        fit_viewport(width, height, native_height, aspect, scale)
    }

    /// Gives back to the GPU what was created here. The context is still current at this point.
    pub fn release(self) {
        if let Some(gl_painter) = &self.gl_painter {
            gl_painter.release(&self.gl);
        }
    }

    fn swap(&self) {
        if let Err(error) = self.surface.swap_buffers(&self.context) {
            eprintln!("warning: the buffer swap failed: {error}");
        }
    }
}

/// Where the frame sits inside the window.
///
/// `aspect` is the image width over height; `None` means nobody asked for an aspect ratio and the
/// frame fills everything. `native_height` is the height in pixels of the console frame, and
/// serves **only** the integer scale — which is enlargement by a multiple of the original frame,
/// and therefore needs to know what the original is. The rectangle comes out centered: the
/// leftover border is the same on both sides.
fn fit_viewport(
    width: u32,
    height: u32,
    native_height: u32,
    aspect: Option<f32>,
    scale: Scaling,
) -> egui::epaint::ViewportInPixels {
    let (window_w, window_h) = (width as f32, height as f32);
    let full = || rect(0, 0, width as i32, height as i32, height);
    let Some(aspect) = aspect.filter(|a| *a > 0.0) else {
        return full();
    };
    if scale == Scaling::Stretch {
        return full();
    }

    // The largest size that fits while keeping the aspect ratio. It is `fit`, and it is also where
    // the integer scale starts from when not even one copy of the original frame would fit.
    let (mut frame_w, mut frame_h) = match window_w / window_h > aspect {
        // The window is wider than the image: height rules, and there is room left on the sides.
        true => (window_h * aspect, window_h),
        false => (window_w, window_w / aspect),
    };

    // The integer multiple never blurs, and the price is a bigger border — whoever picks `integer`
    // is picking exactly that. The math is over the **original frame**, not over what fit:
    // starting from the already fitted size would always give one time, and the option would do
    // nothing.
    if scale == Scaling::Integer && native_height > 0 {
        let native_h = native_height as f32;
        let native_w = native_h * aspect;
        let times = (window_w / native_w).min(window_h / native_h).floor();
        // There is no going below one whole copy: in a window smaller than the console frame,
        // `integer` goes back to being `fit`, which at least shows the whole image.
        if times >= 1.0 {
            frame_w = native_w * times;
            frame_h = native_h * times;
        }
    }

    let left = ((window_w - frame_w) / 2.0).round() as i32;
    let top = ((window_h - frame_h) / 2.0).round() as i32;
    rect(left, top, frame_w.round() as i32, frame_h.round() as i32, height)
}

/// The rectangle the way `glViewport` wants it: origin at the bottom.
fn rect(left: i32, top: i32, width: i32, height: i32, window_h: u32) -> egui::epaint::ViewportInPixels {
    egui::epaint::ViewportInPixels {
        left_px: left,
        top_px: top,
        from_bottom_px: window_h as i32 - top - height,
        width_px: width,
        height_px: height,
    }
}

/// The GL configuration to use: the one with the most samples among those that work.
fn choose_config(configs: Box<dyn Iterator<Item = Config> + '_>) -> Config {
    configs
        .reduce(|a, b| match b.num_samples() > a.num_samples() {
            true => b,
            false => a,
        })
        .expect("the driver offered no GL configuration")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a wide window, 4:3 stays in the middle with equal borders on both sides.
    #[test]
    fn four_by_three_stays_centered_in_a_wide_window() {
        let vp = fit_viewport(1600, 900, 480, Some(4.0 / 3.0), Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (1200, 900));
        assert_eq!(vp.left_px, 200);
        assert_eq!(vp.top_px, 0);
        assert_eq!(vp.from_bottom_px, 0);
    }

    /// Stretch ignores the aspect ratio, which is what it promises.
    #[test]
    fn stretch_fills_the_window() {
        let vp = fit_viewport(1600, 900, 480, Some(4.0 / 3.0), Scaling::Stretch);
        assert_eq!((vp.width_px, vp.height_px), (1600, 900));
        assert_eq!((vp.left_px, vp.top_px), (0, 0));
    }

    /// With no aspect ratio requested, the frame takes the whole window.
    #[test]
    fn without_aspect_ratio_the_frame_fills_everything() {
        let vp = fit_viewport(1000, 500, 480, None, Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (1000, 500));
    }

    /// The integer scale enlarges by multiples of the console frame, and leaves a border because
    /// of it.
    #[test]
    fn integer_scale_enlarges_by_frame_multiples() {
        // 640x480 in a 1600x900 window: 1.40 fit in the height, so a double copy does not fit and
        // the single one applies.
        let vp = fit_viewport(1600, 900, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (640, 480));
        assert_eq!(vp.left_px, 480);

        // In a 1920x1080 one, two fit.
        let vp = fit_viewport(1920, 1080, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (1280, 960));
    }

    /// A window smaller than the frame has no integer multiple: then `integer` becomes `fit`,
    /// which at least shows the whole image.
    #[test]
    fn integer_in_a_small_window_fits_again() {
        let vp = fit_viewport(320, 240, 480, Some(4.0 / 3.0), Scaling::Integer);
        assert_eq!((vp.width_px, vp.height_px), (320, 240));
    }

    /// The `glViewport` origin is at the bottom: a band on top becomes a band at the bottom in the
    /// math.
    #[test]
    fn the_origin_is_flipped_upside_down() {
        let vp = fit_viewport(800, 900, 480, Some(4.0 / 3.0), Scaling::Fit);
        assert_eq!((vp.width_px, vp.height_px), (800, 600));
        assert_eq!(vp.top_px, 150);
        assert_eq!(vp.from_bottom_px, 150);
    }
}

// ---------------------------------------------------------------------------------------------
// The window loop.
// ---------------------------------------------------------------------------------------------

use std::path::PathBuf;
use std::process::ExitCode;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::console::{Console, LoopEnd};

/// Opens the window and spins the console in it until it is closed.
pub fn run(console: Console, first: PathBuf) -> ExitCode {
    let run_loop = match EventLoop::new() {
        Ok(run_loop) => run_loop,
        Err(error) => {
            eprintln!("error: no event loop: {error}");
            eprintln!("       on a headless server, use `[video] mode = none`.");
            return ExitCode::FAILURE;
        }
    };
    // Run nonstop: what holds the turn back is the vsync of the frame swap, or the instant of
    // sleep when the game is ahead. Waiting for events would leave the emulator stopped between
    // keys.
    run_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App {
        console,
        first,
        window: None,
        code: ExitCode::SUCCESS,
    };
    if let Err(error) = run_loop.run_app(&mut app) {
        eprintln!("error: the event loop stopped: {error}");
        return ExitCode::FAILURE;
    }
    app.code
}

struct App {
    console: Console,
    first: PathBuf,
    window: Option<HeadlessWindow>,
    code: ExitCode,
}

impl App {
    fn die(&mut self, run_loop: &winit::event_loop::ActiveEventLoop, error: &str) {
        eprintln!("error: {error}");
        self.code = ExitCode::FAILURE;
        run_loop.exit();
    }

    /// Puts on the window the frame the session has now.
    fn paint(&mut self) {
        // The options come out before the session: reading both at the same time would borrow
        // `console` in two ways, and the compiler is right to refuse.
        let g = self.console.settings.graphics.clone();
        let (scale, smooth, aspect_ratio_4_3) = (g.scaling, g.smooth, g.keep_aspect);
        let Some(window) = self.window.as_mut() else {
            return;
        };
        let Some(session) = self.console.session_mut() else {
            return;
        };

        // The "window" aspect ratio follows its size, and so it is set on every frame; any other
        // one was set once, at opening.
        if g.ratio == zeebx::ui::settings::AspectRatio::Window {
            let (width, height) = window.size();
            session.set_aspect_ratio(Some(width as f32 / height.max(1) as f32));
        }

        let native_height = session.screen().height();
        // With 3D on the GPU, the frame that goes to the screen is **not** `screen()`: it is the
        // texture the rasterizer just filled, and it is the only one that can be wider than 4:3 —
        // the wide aspect ratio opens the field of view in rendering, not in composition.
        let on_gpu = g.gpu_rasterizer.then(|| session.gpu_frame()).flatten();
        match on_gpu {
            Some(frame) => window.show_texture(frame, native_height, scale, aspect_ratio_4_3, smooth),
            None => {
                let screen = session.screen();
                let (width, height) = (screen.width() as i32, screen.height() as i32);
                let bytes = screen.to_rgb565_bytes();
                window.show_frame(&bytes, width, height, scale, aspect_ratio_4_3, smooth);
            }
        }
    }
}

impl ApplicationHandler for App {
    /// The window is born here, and **only the first time**.
    ///
    /// winit calls this again when the system brings the app back to life. Remaking the window at
    /// that moment would swap the GL context underneath the session, which keeps textures and
    /// programs created in the old one — the first draw after that would be a crash inside the
    /// driver.
    fn resumed(&mut self, run_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = match HeadlessWindow::open(run_loop, &self.console.options) {
            Ok(window) => window,
            Err(error) => return self.die(run_loop, &error),
        };
        self.console.use_context(Some(window.gl.clone()));
        self.window = Some(window);
        let first = self.first.clone();
        if let Err(error) = self.console.open(&first) {
            return self.die(run_loop, &format!("{}: {error}", first.display()));
        }
    }

    fn window_event(
        &mut self,
        run_loop: &winit::event_loop::ActiveEventLoop,
        _id: winit::window::WindowId,
        win_event: WindowEvent,
    ) {
        match win_event {
            WindowEvent::CloseRequested => run_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(window) = &self.window {
                    window.resize(size.width, size.height);
                }
            }
            // Out of focus, whatever was pressed is not anymore: the key release happens in
            // another window and would never arrive here.
            WindowEvent::Focused(false) => self.console.keyboard.release_all(),
            WindowEvent::KeyboardInput { event, .. } => {
                // `Esc` closes, as in any emulator, and that is why it is not mappable: whoever
                // ran it needs a way out that does not depend on the file being right.
                if event.state.is_pressed()
                    && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                {
                    run_loop.exit();
                    return;
                }
                self.console
                    .keyboard
                    .event(event.physical_key, event.state.is_pressed());
            }
            _ => {}
        }
    }

    /// One console turn per loop turn.
    fn about_to_wait(&mut self, run_loop: &winit::event_loop::ActiveEventLoop) {
        if self.window.is_none() {
            return;
        }
        match self.console.step() {
            LoopEnd::Continue => {}
            LoopEnd::Finished(reason) => {
                eprintln!("stopped: {reason}");
                return run_loop.exit();
            }
            LoopEnd::Error(error) => return self.die(run_loop, &error),
        }
        // Ahead, the game has no new frame to show: sleeping an instant gives the machine back
        // instead of spinning the loop for nothing. With vsync on, the frame swap is what holds
        // it back, but that is not guaranteed — some drivers ignore it.
        if self.console.ahead() {
            std::thread::sleep(std::time::Duration::from_millis(1));
            return;
        }
        self.paint();
    }

    /// The context and the painter are released here, with the loop still standing.
    fn exiting(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        self.console.close();
        if let Some(window) = self.window.take() {
            window.release();
        }
    }
}
