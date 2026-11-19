//! Zeebx without an interface: the emulator on the command line, configured by a `config.ini`.
//!
//! It exists for whoever already has a frontend. A program that simulates the console's shell, a
//! game shelf, an arcade cabinet — none of them wants the Zeebx library on top of the screen it
//! drew itself. This binary opens the game it is given, obeys the configuration file and shows
//! nothing else.
//!
//! **The core is the same.** Graphics, audio and controller mapping are the fields the desktop
//! interface writes to `settings.json`; here they arrive through INI, which is what gets edited
//! by hand. See [`config`].

mod config;
mod console;
mod dump;
mod host_input;
mod ini;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;

use crate::config::Video;
use crate::console::{Console, LoopEnd};

const HELP: &str = "\
Zeebx headless — the Zeebo emulator on the command line.

  zeebx-headless [OPTIONS] GAME

GAME is required: a `.mod`, or the `.zip` that holds it. To boot into the
Z-Wheel, pass the Z-Wheel — it is a game like any other.

Options:
  --config=PATH    the configuration file to use. Without it, looks for a
                   `config.ini` next to the executable and then in the system
                   configuration directory. If none exists, writes one there.
  --controllers    list the controllers the system can see, with the names
                   `[port1] controller` expects, and exit.
  --example        write a commented `config.ini` with the factory values to
                   standard output, and exit.
  --soundfont=PATH the `.sf2` bank MIDI music plays with, overriding
                   `[audio] soundfont`.
  --help           this.
  --version        the version.

Everything else — video, audio, controls — lives in the `config.ini`.
";

fn main() -> ExitCode {
    // **`ZEEBX_LOG` counts as the starting point, and the file wins over it.** The `config.ini` is
    // the explicit choice of whoever edited it; the variable is what serves someone debugging a
    // run without touching any file. Read before the file so the order is that one.
    zeebx::logging::from_env();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version") {
        println!("zeebx-headless {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--example") {
        print!("{}", config::model());
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--controllers") {
        return list_controllers();
    }

    let file = args
        .iter()
        .find_map(|a| a.strip_prefix("--config="))
        .map(PathBuf::from);
    let soundfont = args
        .iter()
        .find_map(|a| a.strip_prefix("--soundfont="))
        .map(PathBuf::from);
    // Whatever does not start with `--` is the game. Only one: opening two means nothing.
    let game = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from);
    if let Some(unknown) = args.iter().find(|a| {
        a.starts_with("--") && !a.starts_with("--config=") && !a.starts_with("--soundfont=")
    }) {
        eprintln!("error: unknown option `{unknown}`. `--help` lists what exists.");
        return ExitCode::FAILURE;
    }

    let mut loaded = config::load(file.as_deref());
    for warning in &loaded.warnings {
        eprintln!("config: {warning}");
    }
    // The command line wins over the file: it is the caller's choice, right now.
    if soundfont.is_some() {
        loaded.settings.audio.soundfont = soundfont;
    }
    let origin = loaded.origin;
    let mode = loaded.headless.video;
    let console = Console::create(loaded.settings, loaded.headless);

    // First run: leave a factory `config.ini` written, commented line by line, instead of just
    // running with the defaults and not saying where anything is changed.
    if let config::ConfigOrigin::Missing(location) = &origin {
        match config::create(location) {
            Ok(()) => eprintln!("config: none found; wrote one at {}", location.display()),
            Err(error) => eprintln!("config: no file, using factory settings ({error})"),
        }
    }

    // **The game is required, and has no default.** This binary is called by another program,
    // which knows what it wants to open; guessing something when it says nothing would be opening
    // what nobody asked for. Whoever wants the Z-Wheel passes it like any other game.
    let Some(path) = game else {
        eprintln!(
            "error: nothing to run.\n\
             \n\
             Pass the `.mod`, or the `.zip` that holds it:\n\
             \x20   zeebx-headless PATH/TO/GAME.zip"
        );
        return ExitCode::FAILURE;
    };

    match mode {
        Video::Off => windowless(console, &path),
        _ => window::run(console, path),
    }
}

/// The no-video mode: nothing on screen, and the frame goes out through `[dump]`.
fn windowless(mut console: Console, game_path: &std::path::Path) -> ExitCode {
    // Without a window there is no GL context coming from one, but 3D on the GPU is still
    // possible: the core can open an offscreen context, which is what `zeebx run` already uses to
    // measure.
    let context = match console.settings.graphics.gpu_rasterizer {
        true => match zeebx::video::context::GlContext::create() {
            Ok(context) => Some(context),
            Err(error) => {
                eprintln!("warning: no reachable GPU, 3D stays in software: {error}");
                None
            }
        },
        false => None,
    };
    console.use_context(context.as_ref().map(|c| c.gl.clone()));

    let mut output = match dump::Output::open(&console.options.dump.clone()) {
        Ok(output) => output,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = console.open(game_path) {
        eprintln!("error: {}: {error}", game_path.display());
        return ExitCode::FAILURE;
    }

    // The frame is only written when it changes: the emulator loop spins many more times than the
    // game draws, and repeating the same frame would fill the pipe with identical copies.
    let mut seen = None;
    loop {
        // The core's log goes out here, once per turn. With the ring empty — the common case, at
        // the default level — this is a lock and a read.
        zeebx::logging::dump_to_stderr();
        match console.step() {
            LoopEnd::Continue => {}
            LoopEnd::Finished(reason) => {
                eprintln!("stopped: {reason}");
                return ExitCode::SUCCESS;
            }
            LoopEnd::Error(error) => {
                eprintln!("error: {error}");
                return ExitCode::FAILURE;
            }
        }
        if console.ahead() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let large = console.options.dump.format;
        let Some(session) = console.session_mut() else {
            return ExitCode::SUCCESS;
        };
        // **The GPU frame has to be brought back before leaving here.** With deferred readback
        // (`Session::materialize_gl_frame`), the CPU screen only receives the frame when someone
        // asks for it — and this loop is exactly who asks: it compares the screen with itself so
        // as not to repeat a frame, and writes the bytes to the pipe.
        session.materialize_gl_frame();
        let now = {
            let screen = session.screen();
            (screen.generation(), screen.writes())
        };
        if seen == Some(now) {
            continue;
        }
        seen = Some(now);
        // **The raw formats always go out at the console's frame size, and only `.png` follows
        // the internal resolution.** A headerless byte stream has to keep the frame the same size
        // from start to end: 3D on the GPU comes out large and the 2D menu comes out small, and
        // whoever counts bytes on the other side would lose step at the first switch. `.png`
        // states its own size in every file, so there the larger image gets in no one's way.
        let frame = match large == config::Format::Png {
            true => session.large_frame(),
            false => None,
        };
        let frame = frame.as_ref().unwrap_or_else(|| session.screen());
        if let Err(error) = output.write(frame) {
            // The other end closed the pipe. It is not the caller's failure: it is the normal end
            // of `zeebx-headless ... | frontend` when the frontend exits.
            eprintln!("stopped: {error} ({} frames)", output.counted());
            return ExitCode::SUCCESS;
        }
        if output.full() {
            eprintln!("stopped: wrote the {} frames requested", output.counted());
            return ExitCode::SUCCESS;
        }
    }
}

/// The controllers the system sees, by the names `config.ini` expects.
fn list_controllers() -> ExitCode {
    let pads = zeebx::input::gamepads::Gamepads::new();
    let names = pads.names();
    if names.is_empty() {
        println!("no controllers visible to the system");
        return ExitCode::SUCCESS;
    }
    println!("controllers seen, in system order:");
    for (i, name) in names.iter().enumerate() {
        println!("  {i}: {name}");
    }
    println!("\nin [port1] of config.ini, for example:\n  controller = \"{}\"", names[0]);
    ExitCode::SUCCESS
}
