//! Zeebx — the command line of the Zeebo / Qualcomm BREW emulator.
//!
//! Run, measure and inspect a game without the interface: `run`, `session`, `bench`, `info`,
//! `controls` and `wiimote`. The interface is `zeebx-qt`, in `frontends/qt-standalone`.

// The binary consumes the engine as a library: nothing here redeclares modules, and `desktop`
// decides which of them go into the build.
use zeebx::{audio, cpu, input, library, loader, machine, session, ui};

use std::process::ExitCode;

use zeebx::brew::aee;
use zeebx::cpu::{DefaultBackend, CpuBackend, dynarmic::DynarmicCpu};
use zeebx::input::bindings;
use zeebx::loader::archive;
use zeebx::loader::modfile::{ModImage, Variant};
use zeebx::machine::{AppletResult, Machine, Outcome};
use zeebx::ui::window;

/// Cap on instructions per slice between two API calls — keeps an infinite loop in the guest
/// from hanging the emulator. It has to be generous: Bejeweled Twist's initialization goes tens
/// of millions of instructions without calling any API.
const INSTRUCTION_BUDGET: u64 = 500_000_000;

/// How much game time to run when nobody asks for something else.
const DEFAULT_SECONDS: u32 = 10;
/// Cap on event loop passes — a safety net, not a usage limit: what decides when to stop is
/// `--seconds` or closing the window.
const MAX_ROUNDS: u32 = 10_000_000;

/// How often, in wall-clock time, the window is refreshed at the least.
const REFRESH_PERIOD: std::time::Duration = std::time::Duration::from_millis(8);

/// Rate at which `--dump-audio` records. It does not need to be the sound card's: the point is
/// to check what the game asked to play, and 44100 covers the highest rate games use.
const RECORD_RATE: u32 = 44_100;

/// How many entries the session profile shows.
const PROFILE_LINES: usize = 20;

/// How many lines of the semihosting log the report shows.
const SEMIHOSTING_LINES: usize = 40;

fn main() -> ExitCode {
    // **The log level comes in here too.** The window reads it on startup (`zeebx-qt`), and the
    // command-line commands did not: `ZEEBX_LOG=informacao` was ignored in `run`, `bench` and
    // `session`, and the instrumentation lines -- all of them `Info` -- did not show up. That is
    // how two of my measurements came out empty before I noticed.
    zeebx::logging::from_env();
    let args: Vec<String> = cli_args();
    match args.first().map(String::as_str) {
        Some("info") if args.len() == 2 => report(info(&args[1])),
        Some("run") if args.len() >= 2 => {
            let tracing = args.iter().any(|a| a.starts_with("--trace"));
            let trace_filter = args
                .iter()
                .find_map(|a| a.strip_prefix("--trace="))
                .map(str::to_owned);
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok());
            let rounds = args
                .iter()
                .find_map(|a| a.strip_prefix("--frames="))
                .and_then(|n| n.parse().ok())
                .unwrap_or(MAX_ROUNDS);
            // `--probe=0xCLSID[,0xCLSID...]` serves unknown classes with an observation object,
            // instead of refusing them, and says at the end what the game called on it.
            let probe: Vec<u32> = args
                .iter()
                .filter_map(|a| a.strip_prefix("--probe="))
                .flat_map(|list| list.split(','))
                .filter_map(|n| u32::from_str_radix(n.trim().trim_start_matches("0x"), 16).ok())
                .collect();
            // `--probe-answer=0xCLSID:SLOT=VALUE` sets what a probe slot answers. Without it
            // there is no way out of a loop where the game waits for "done" — the probe says
            // success forever and it never leaves.
            let probe_answers: Vec<(u32, u32, u32)> = args
                .iter()
                .filter_map(|a| a.strip_prefix("--probe-answer="))
                .filter_map(|spec| {
                    let (class_id, rest) = spec.split_once(':')?;
                    let (slot, value) = rest.split_once('=')?;
                    let number = |t: &str| {
                        let t = t.trim();
                        match t.strip_prefix("0x") {
                            Some(hex) => u32::from_str_radix(hex, 16).ok(),
                            None => t.parse().ok(),
                        }
                    };
                    Some((number(class_id)?, number(slot)?, number(value)?))
                })
                .collect();
            // `--serial=PATH` records what would come out of the console's debug UART: each
            // `DBGPRINTF` in order, with the virtual clock instant. The report groups
            // repetitions; this does not, which is what makes it useful for analyzing a
            // sequence.
            let serial = args
                .iter()
                .find_map(|a| a.strip_prefix("--serial="))
                .map(std::path::PathBuf::from);
            let dump_heap = args.iter().any(|a| a == "--dump-heap");
            let dump_surfaces = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-surfaces="))
                .map(str::to_owned);
            let dump_gl = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-gl="))
                .map(str::to_owned);
            let dump_audio = args
                .iter()
                .find_map(|a| a.strip_prefix("--dump-audio="))
                .map(str::to_owned);
            let window = args.iter().any(|a| a == "--window");
            let keys = args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose();
            let keys = match keys {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            };
            // With a window, the default is to run until it is closed; without one, someone has
            // to say when to stop, and the default is a few seconds of game time.
            let seconds = match (seconds, window) {
                (limit @ Some(_), _) => limit,
                (None, true) => None,
                (None, false) => Some(DEFAULT_SECONDS),
            };
            report(run(
                &args[1],
                Options {
                    tracing,
                    trace_filter,
                    rounds,
                    seconds,
                    serial,
                    dump_heap,
                    dump_surfaces,
                    probe,
                    probe_answers,
                    window,
                    keys,
                    dump_gl,
                    dump_audio,
                    network: !args.iter().any(|a| a == "--no-network"),
                    network_to: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--server="))
                        .map(str::to_owned),
                    bridge: args.iter().any(|a| a == "--bridge"),
                    key_events: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--key-names="))
                        .map(keyboard)
                        .unwrap_or_default(),
                    installed: args
                        .iter()
                        .find_map(|a| a.strip_prefix("--installed="))
                        .map(installed)
                        .unwrap_or_default(),
                    ports: match args.iter().find_map(|a| a.strip_prefix("--ports=")) {
                        Some(list) => match devices(list) {
                            Some(ports) => ports,
                            None => {
                                eprintln!(
                                    "error: --ports expects comma-separated names, among \
                                     'pad', 'keyboard' and 'none'"
                                );
                                return ExitCode::FAILURE;
                            }
                        },
                        None => zeebx::DEFAULT_PORTS,
                    },
                },
            ))
        }
        // What the emulator sees of controllers, for when input does not respond and there is
        // no telling whether the problem is the device, the saved name or the mapping.
        Some("controls") => {
            let pads = input::gamepads::Gamepads::new();
            let names = pads.names();
            match names.is_empty() {
                true => println!("no controller seen by the system"),
                false => {
                    println!("controllers seen, in system order:");
                    for (i, name) in names.iter().enumerate() {
                        println!("  {i}: {name}");
                    }
                }
            }
            // Each controller's motion sensor: it is what feeds the port's Boomerang, and a
            // controller with a sensor that does not move the Boomerang is usually the device
            // node without permission.
            let sensors = input::sensors::Sensors::start();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            for (i, name) in names.iter().enumerate() {
                let sensor =
                    pads.identity(Some(name), i)
                        .and_then(|(os_name, vendor, product, order)| {
                            sensors.for_controller(&os_name, vendor, product, order)
                        });
                match sensor {
                    Some(s) if s.permission_denied => {
                        println!(
                            "  sensor of {name}: {} NO PERMISSION — the udev rule:",
                            s.name
                        );
                        println!("    {}", input::sensors::UDEV_RULE);
                    }
                    Some(s) if s.has_reading => println!(
                        "  sensor of {name}: {}, [{:+.2} {:+.2} {:+.2}] g",
                        s.name, s.acceleration[0], s.acceleration[1], s.acceleration[2]
                    ),
                    Some(s) => println!("  sensor of {name}: {}, no reading yet", s.name),
                    None => {}
                }
            }
            let settings = ui::settings::Settings::load();
            for port in 0..input::PORTS {
                let Some(player) = settings.controls.player(port) else {
                    continue;
                };
                let chosen = player.device.as_deref();
                let found = match chosen {
                    Some(name) => match names.iter().position(|n| n == name) {
                        Some(i) => format!("position {i}"),
                        None => "NOT CONNECTED".to_string(),
                    },
                    None => match names.len() > port {
                        true => format!("position {port}, for being port {}", port + 1),
                        false => "none at that position".to_string(),
                    },
                };
                println!(
                    "port {}: {}, device {:?}, chosen {:?} -> {found}",
                    port + 1,
                    match player.enabled {
                        true => "on",
                        false => "off",
                    },
                    player.device_kind,
                    chosen.unwrap_or("none"),
                );
            }
            // With `--read`, it stays a while showing what comes in: it is the only way to tell
            // "the controller is not seen" from "the controller is seen and the mapping does not
            // match".
            if args.iter().any(|a| a == "--read") {
                let mut pads = pads;
                println!("reading for 15 seconds — press the buttons");
                let end = std::time::Instant::now() + std::time::Duration::from_secs(15);
                let mut before: Vec<String> = vec![String::new(); input::PORTS];
                let mut raw_before: Vec<String> = vec![String::new(); names.len()];
                while std::time::Instant::now() < end {
                    pads.poll();
                    // Raw, per controller: separates "the system delivers nothing" from "it
                    // delivers and the port's mapping does not use it".
                    for (i, name) in names.iter().enumerate() {
                        let raw = match pads.first_active(Some(name), i) {
                            Some(source) => format!("{source:?}"),
                            None => String::new(),
                        };
                        if raw != raw_before[i] {
                            match raw.is_empty() {
                                true => println!("controller {i} ({name}): nothing"),
                                false => println!("controller {i} ({name}): {raw}"),
                            }
                            raw_before[i] = raw;
                        }
                    }
                    for port in 0..input::PORTS {
                        let Some(player) = settings.controls.player(port) else {
                            continue;
                        };
                        let device = player.device.clone();
                        let pad = player.pad(
                            |source| pads.is_active(device.as_deref(), port, source),
                            |axis| pads.value(device.as_deref(), port, axis),
                        );
                        let pressed_buttons: Vec<&str> = input::BUTTON_NAMES
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| pad.is_down(*i))
                            .map(|(_, name)| *name)
                            .collect();
                        let pressed_now = pressed_buttons.join(" ");
                        if pressed_now != before[port] {
                            match pressed_now.is_empty() {
                                true => println!("port {}: nothing", port + 1),
                                false => println!("port {}: {pressed_now}", port + 1),
                            }
                            before[port] = pressed_now;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
            }
            ExitCode::SUCCESS
        }
        Some("bench") if args.len() >= 2 => {
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(DEFAULT_SECONDS);
            let dump = args.iter().find_map(|a| a.strip_prefix("--dump="));
            let keys = match args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose()
            {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let key_events = args
                .iter()
                .find_map(|a| a.strip_prefix("--key-names="))
                .map(keyboard)
                .unwrap_or_default();
            let installed = args
                .iter()
                .find_map(|a| a.strip_prefix("--installed="))
                .map(installed)
                .unwrap_or_default();
            let surfaces = args.iter().find_map(|a| a.strip_prefix("--dump-surfaces="));
            report(bench_dynarmic(
                &args[1],
                seconds,
                dump,
                &keys,
                &key_events,
                installed,
                surfaces,
            ))
        }
        // Shows the Wii Remote live: buttons and acceleration, to check the reading without a
        // window.
        Some("wiimote") => {
            let wiimote_seconds: u64 = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            let wiimotes = input::wiimote::Wiimotes::start();
            let end = std::time::Instant::now() + std::time::Duration::from_secs(wiimote_seconds);
            while std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(250));
                match wiimotes.state(0) {
                    Some(e) => {
                        let pressed_buttons: Vec<&str> = input::wiimote::BUTTONS
                            .iter()
                            .map(|(name, _)| *name)
                            .filter(|name| e.pressed(name))
                            .collect();
                        let [x, y, z] = e.acceleration;
                        println!(
                            "x {x:+.2} y {y:+.2} z {z:+.2} g  accelerometer: {}  buttons: {pressed_buttons:?}",
                            if e.with_accelerometer {
                                "yes"
                            } else {
                                "not yet"
                            }
                        );
                    }
                    None => println!("no Wii Remote"),
                }
            }
            ExitCode::SUCCESS
        }
        Some("session") if args.len() >= 2 => {
            let seconds = args
                .iter()
                .find_map(|a| a.strip_prefix("--seconds="))
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(DEFAULT_SECONDS);
            let dump = args.iter().find_map(|a| a.strip_prefix("--dump="));
            let keys = match args
                .iter()
                .find_map(|a| a.strip_prefix("--keys="))
                .map(input::Script::parse)
                .transpose()
            {
                Ok(keys) => keys.unwrap_or_default(),
                Err(err) => {
                    eprintln!("error: {err}");
                    return ExitCode::FAILURE;
                }
            };
            let shots: Vec<u32> = args
                .iter()
                .find_map(|a| a.strip_prefix("--shots="))
                .map(|list| {
                    list
                        .split(',')
                        .filter_map(|t| t.trim().parse().ok())
                        .collect()
                })
                .unwrap_or_default();
            let gpu = args.iter().any(|a| a == "--gpu");
            let serial = args.iter().find_map(|a| a.strip_prefix("--serial="));
            // As in `run`: the filter alone turns nothing on, and turning it on without a filter
            // shows everything. Before, this branch did not even read the option, and the
            // `--trace` in `session`'s usage did nothing.
            let tracing = args.iter().any(|a| a.starts_with("--trace"));
            let trace_filter = args
                .iter()
                .find_map(|a| a.strip_prefix("--trace="))
                .map(str::to_owned);
            // With nothing, the saved Z-Wheel preferences apply; `--factory` uses the package's
            // cfg as it came, and the other two change a single option.
            let mut z_wheel = match args.iter().any(|a| a == "--factory") {
                true => ui::settings::ZWheel {
                    end_of_life: true,
                    always_transitions: false,
                },
                false => ui::settings::Settings::load().z_wheel,
            };
            if args.iter().any(|a| a == "--no-end-of-life") {
                z_wheel.end_of_life = false;
            }
            if args.iter().any(|a| a == "--no-transitions") {
                z_wheel.always_transitions = false;
            }
            let scale = args
                .iter()
                .find_map(|a| a.strip_prefix("--scale="))
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(1);
            let number = |prefix: &str| {
                args.iter()
                    .find_map(|a| a.strip_prefix(prefix))
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(1)
            };
            let enhancements = (number("--msaa="), number("--aniso="));
            // `--profile` measures everything; `--profile=MS` only from that virtual instant on.
            let profile = args.iter().find_map(|a| match a.as_str() {
                "--profile" => Some(0),
                other => other
                    .strip_prefix("--profile=")
                    .and_then(|n| n.parse::<u32>().ok()),
            });
            // `--boomerang` puts a Boomerang on port one; `--motion=ms:x:y:z,...` gives its
            // acceleration from each instant on, in g.
            let motion: Vec<(u32, [f32; 3])> = args
                .iter()
                .find_map(|a| a.strip_prefix("--motion="))
                .map(|list| {
                    list
                        .split(',')
                        .filter_map(|item| {
                            let parts: Vec<&str> = item.split(':').collect();
                            let [when, x, y, z] = parts.as_slice() else {
                                return None;
                            };
                            Some((
                                when.parse().ok()?,
                                [x.parse().ok()?, y.parse().ok()?, z.parse().ok()?],
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let boomerang = (args.iter().any(|a| a == "--boomerang") || !motion.is_empty())
                .then_some(motion);
            // The ports, to test what only shows up with two players. Without the option the
            // usual default stays: a controller on the first and the second free.
            let ports = match args.iter().find_map(|a| a.strip_prefix("--ports=")) {
                Some(list) => match devices(list) {
                    Some(ports) => Some(ports),
                    None => {
                        eprintln!(
                            "error: --ports expects comma-separated names, among \
                             'pad', 'zpad', 'keyboard', 'boomerang' and 'none'"
                        );
                        return ExitCode::FAILURE;
                    }
                },
                None => None,
            };
            report(windowless_session(
                &args[1], seconds, dump, &keys, &shots, gpu, serial, z_wheel, scale, enhancements,
                profile, boomerang, ports, tracing, trace_filter,
            ))
        }
        _ => {
            eprintln!("usage: zeebx info <file.mod>     (the interface is zeebx-qt)");
            eprintln!(
                "     zeebx run <file.mod> [--window] [--seconds=N] [--keys=ms:key,...]
                             [--dump-gl=DIR] [--dump-audio=FILE.wav]
                             [--trace[=filter]] [--serial=PATH] [--dump-heap]
                             [--dump-surfaces=DIR]
                             [--frames=N] [--probe=0xCLSID,...]
                             [--no-network] [--server=HOST[:PORT]] [--bridge]
                             [--ports=pad|keyboard|none,...] [--key-names=ms:name,...]"
            );
            eprintln!(
                "     zeebx session <file.zip> [--seconds=N] [--keys=ms:button,...] [--dump=FRAME.bmp] [--shots=ms,...] [--gpu] [--serial=PATH] [--factory] [--no-end-of-life] [--no-transitions] [--scale=N] [--msaa=N] [--aniso=N] [--profile[=MS]] [--boomerang] [--motion=ms:x:y:z,...] [--wiimote] [--aspect=16:9]
                             [--ports=pad,pad] [--dpad-on-axes]  (the window's session, without a window)"
            );
            eprintln!(
                "     zeebx bench <file.mod|zip> [--seconds=N] [--keys=ms:key,...] [--dump=FRAME.bmp] [--key-names=ms:name,...] [--installed=0xCLSID[:id],...] [--dump-surfaces=DIR] [--dpad-on-axes]  (Dynarmic, without a window)"
            );
            ExitCode::FAILURE
        }
    }
}


/// Subcommands and options that had Portuguese names before the codebase moved to English:
/// `(legacy, current)`. The legacy spelling is still accepted so existing scripts keep running.
const LEGACY_ARGS: &[(&str, &str)] = &[
    ("sessao", "session"),
    ("controles", "controls"),
    ("--dpad-nos-eixos", "--dpad-on-axes"),
    ("--sonda-resposta", "--probe-answer"),
    ("--sem-fim-de-vida", "--no-end-of-life"),
    ("--sem-transicoes", "--no-transitions"),
    ("--sem-rede", "--no-network"),
    ("--instalados", "--installed"),
    ("--movimento", "--motion"),
    ("--proporcao", "--aspect"),
    ("--servidor", "--server"),
    ("--fabrica", "--factory"),
    ("--escala", "--scale"),
    ("--portas", "--ports"),
    ("--teclas", "--key-names"),
    ("--perfil", "--profile"),
    ("--placa", "--gpu"),
    ("--ponte", "--bridge"),
    ("--sonda", "--probe"),
    ("--fotos", "--shots"),
    ("--ler", "--read"),
];

/// The command line without the program name, with legacy spellings rewritten to the current
/// ones. Everything that reads the command line goes through here, so the aliases live in one
/// place.
fn cli_args() -> Vec<String> {
    std::env::args()
        .skip(1)
        .map(|arg| {
            let (name, value) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (arg.as_str(), None),
            };
            match LEGACY_ARGS.iter().find(|(legacy, _)| *legacy == name) {
                Some((_, current)) => match value {
                    Some(value) => format!("{current}={value}"),
                    None => current.to_string(),
                },
                None => arg,
            }
        })
        .collect()
}

fn report(result: Result<(), Box<dyn std::error::Error>>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Loads the module and runs from `AEEMod_Load` until an outcome.
/// The command-line options of `run`, gathered together because nearly every function of the
/// run needs a handful of them.
struct Options {
    tracing: bool,
    trace_filter: Option<String>,
    rounds: u32,
    seconds: Option<u32>,
    /// Path of the serial capture, with `--serial=PATH`.
    serial: Option<std::path::PathBuf>,
    dump_heap: bool,
    /// Where to write one BMP per live surface, with `--dump-surfaces=DIR`.
    dump_surfaces: Option<String>,
    probe: Vec<u32>,
    probe_answers: Vec<(u32, u32, u32)>,
    window: bool,
    keys: input::Script,
    dump_gl: Option<String>,
    dump_audio: Option<String>,
    /// Whether the game may talk to the network. On by default; `--no-network` turns it off.
    network: bool,
    /// Where to redirect connections, with `--server=HOST[:PORT]`.
    network_to: Option<String>,
    /// Whether the module's bridge delivers the response to the game, with `--bridge`.
    bridge: bool,
    /// What the console sees on each port, with `--ports=pad,keyboard`.
    ports: [Option<bindings::Device>; input::PORTS],
    /// Keys to deliver, with `--key-names=ms:name[,...]`.
    key_events: Vec<(u32, u32)>,
    /// Classes the shell treats as installed, with `--installed=0xCLSID[:id][,...]` — the id is
    /// the module's folder, which `EnumNextApplet` hands out. In the window
    /// the library says so; without it, this is what lets a game launch through the Z-Wheel be
    /// tested.
    installed: Vec<(u32, String)>,
}

/// Reads `1000:select,2000:down` and returns `(instant in ms, AVK code)`.
///
/// The names are those of [`input::avk::by_name`]: `up`, `down`, `left`, `right`, `select`,
/// `clr`, `star`, `pound` and the digits. Whatever is not recognized is dropped with a warning,
/// because a script with one wrong key is still good for the others.
/// Reads `0xCLSID[:id][,...]`. Without an id, the module id is the class itself in hexadecimal.
fn installed(list: &str) -> Vec<(u32, String)> {
    list
        .split(',')
        .filter_map(|item| {
            let (class_id, id) = item.split_once(':').unwrap_or((item, ""));
            let class_id = u32::from_str_radix(class_id.trim().trim_start_matches("0x"), 16).ok()?;
            let id = match id.is_empty() {
                true => format!("{class_id:x}"),
                false => id.to_string(),
            };
            Some((class_id, id))
        })
        .collect()
}

/// `--aspect=16:9`: the experimental wide aspect ratio for 3D, width over height.
fn aspect_ratio_from_cli() -> Option<f32> {
    cli_args().into_iter()
        .find_map(|a| a.strip_prefix("--aspect=").map(str::to_string))
        .and_then(|p| {
            let (width, height) = p.split_once(':')?;
            Some(width.parse::<f32>().ok()? / height.parse::<f32>().ok()?)
        })
}

fn keyboard(list: &str) -> Vec<(u32, u32)> {
    list
        .split(',')
        .filter(|item| !item.is_empty())
        .filter_map(|item| {
            let (when, name) = item.split_once(':')?;
            let when = when.parse().ok()?;
            match input::avk::by_name(name.trim()) {
                Some(avk) => Some((when, avk)),
                None => {
                    eprintln!("warning: unknown key {name:?}");
                    None
                }
            }
        })
        .collect()
}

/// Reads `pad,keyboard` and the like. `None` when some name does not exist.
///
/// It exists so that both ports are **testable without a window**, which is how everything here
/// gets verified: without it, the only way to know whether a game sees the second controller
/// would be to open the interface and look.
fn devices(list: &str) -> Option<[Option<bindings::Device>; input::PORTS]> {
    let mut ports = [None; input::PORTS];
    for (n, name) in list.split(',').enumerate() {
        let port = ports.get_mut(n)?;
        *port = match name.trim() {
            "controle" | "pad" | "dragon" => Some(bindings::Device::Controller),
            "zpad" | "z-pad" => Some(bindings::Device::ZPad),
            "teclado" | "keyboard" => Some(bindings::Device::Keyboard),
            "boomerang" => Some(bindings::Device::Boomerang),
            "nenhum" | "none" | "" => None,
            _ => return None,
        };
    }
    Some(ports)
}

fn run(path: &str, options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let Options {
        tracing,
        trace_filter,
        rounds,
        seconds,
        serial,
        dump_heap,
        dump_surfaces,
        probe,
        probe_answers,
        window,
        keys,
        dump_gl,
        dump_audio,
        network,
        network_to,
        bridge,
        ports,
        key_events,
        installed,
    } = options;
    // A game in `.zip` or `.7z` is extracted to the cache and run from there, as in the interface.
    let extracted;
    let path = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("zip" | "7z") => {
            extracted = archive::extract(std::path::Path::new(path))?;
            println!("extracted: {}", extracted.display());
            extracted.to_str().unwrap_or(path)
        }
        _ => path,
    };
    let image = ModImage::parse(std::fs::read(path)?)?;
    let extensions = zeebx::session::extensions_of(std::path::Path::new(path));
    for extension in &extensions {
        println!(
            "extension: provides {}",
            extension
                .classes
                .iter()
                .map(|c| format!("{c:#010x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let module = loader::load_with(&image, &extensions)?;

    println!("loaded:    {path}");
    println!("entry:     {:#010x}", module.entry);
    for region in module.mem.regions() {
        println!(
            "  {:<8} {:#010x} .. {:#010x} {}",
            region.name,
            region.base,
            region.base as u64 + region.bytes.len() as u64,
            if region.writable { "rw" } else { "ro" }
        );
    }

    // The root of the game's file system is the directory where the `.mod` is: that is where
    // the console keeps the title's files.
    let root = std::path::Path::new(path)
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut machine = Machine::new(DefaultBackend::new()?, module, root);
    println!("files:     {}", machine.file_root().display());
    machine.set_tracing(tracing);
    machine.set_trace_filter(trace_filter);
    if !probe.is_empty() {
        machine.probe_classes(&probe);
    }
    for (class_id, slot, value) in &probe_answers {
        machine.probe_answer(*class_id, *slot, *value);
    }
    machine.set_ports(ports);
    machine.set_installed_applets(installed);
    machine.set_network(network);
    if network_to.is_some() {
        machine.set_network_to(network_to);
    }
    if bridge {
        machine.set_bridge(true);
    }
    if let Some(serial_path) = &serial {
        machine.enable_serial(serial_path)?;
        println!("serial:    {}", serial_path.display());
    }
    let outcome = machine.run(INSTRUCTION_BUDGET)?;

    print!("stopped:   ");
    describe_outcome(&outcome);
    if let Outcome::Returned { .. } = outcome {
        let out = machine.module().out_module;
        println!(
            "           IModule* = {:#010x}",
            machine.cpu().read_u32(out)?
        );
    }

    // With the module loaded, the next step is to instantiate the applet. The ClassID comes
    // from the `.mif` that accompanies the module.
    let mut ran_frames = false;
    if let Outcome::Returned { code: 0 } = outcome {
        match library::applet_clsid(std::path::Path::new(path)) {
            Some(clsid) => {
                println!("applet:    ClassID {clsid:#010x} (from the .mif)");
                match machine.create_applet(clsid, INSTRUCTION_BUDGET)? {
                    AppletResult::Called { code, applet } => {
                        println!(
                            "           CreateInstance returned {code}, IApplet* = {applet:#010x}"
                        );
                        if code == 0 && applet != 0 {
                            print!("start:     EVT_APP_START → ");
                            let started =
                                machine.start_applet(applet, clsid, INSTRUCTION_BUDGET)?;
                            describe_outcome_with_state(&started, &machine);
                            // `HandleEvent`'s return code belongs to the applet, not to the
                            // dispatcher: Kingdom Hearts returns 1 on EVT_APP_START and goes on
                            // setting up its loop normally. The Session already accepts any
                            // return; the command line has to do the same, otherwise it never
                            // reaches the code that lets the intro be profiled.
                            if matches!(started, Outcome::Returned { .. } | Outcome::Budget) {
                                ran_frames = true;
                                run_frames(
                                    &mut machine,
                                    rounds,
                                    seconds,
                                    path,
                                    dump_heap,
                                    window,
                                    &keys,
                                    &key_events,
                                    dump_gl.as_deref(),
                                    dump_audio.as_deref(),
                                )?;
                            }
                        }
                    }
                    AppletResult::NoModule => println!("           no IModule* to call"),
                    AppletResult::Stopped(stop) => {
                        print!("           stopped at ");
                        describe_outcome(&stop);
                    }
                }
            }
            None => println!("applet:    no .mif found next to the module"),
        }
    }
    // Without a frame loop nobody dumped the memory, and it is precisely when the game spent its
    // budget before even existing that this snapshot helps most.
    if dump_heap && !ran_frames {
        dump_memory(&machine)?;
    }
    #[cfg(debug_assertions)]
    {
        let (total, largest, duplicates) = machine.widgets_snapshot();
        println!(
            "widgets:   {total} alive, largest attachment list {largest}, repeated attachments {duplicates}"
        );
    }
    if let Some(dir) = &dump_surfaces {
        write_surfaces(&machine, dir)?;
    }

    let (heap, objects) = (machine.heap_used(), machine.live_objects());
    println!("heap:      {heap} bytes in use, {objects} live objects");
    // **How much is left does not say how it is left.** A heap with 30 MB free in 400 holes
    // cannot serve a 2 MB allocation, and the game reports that as out of memory; the line below
    // is what separates the two cases in the report.
    let snapshot = machine.heap_snapshot();
    println!(
        "           {} hole(s), largest free {} of {} free ({}% of the free space stuck in holes), {} live blocks of {}",
        snapshot.holes,
        snapshot.largest_hole,
        snapshot.free,
        u64::from(snapshot.lost_in_holes()) * 100 / u64::from(snapshot.free.max(1)),
        snapshot.live,
        snapshot.cap,
    );
    let refusals = machine.refused_allocations();
    if let Some((size, lr)) = refusals.first() {
        println!(
            "refused:   {} malloc request(s) with no room; the first: {} bytes, requested at {lr:#010x}",
            refusals.len(),
            size
        );
    }
    let checks = machine.refused_availability_checks();
    if let Some((size, lr)) = checks.first() {
        println!(
            "refused:   {} available-memory question(s) answered with \"does not fit\"; the first: {} bytes at {lr:#010x}",
            checks.len(),
            size
        );
    }
    if !machine.suspicious_objects().is_empty() {
        println!(
            "warning:   {} calls with an unexpected `this` pointer",
            machine.suspicious_objects().len()
        );
    }
    let presses = machine.pad_log();
    if !presses.is_empty() {
        println!("presses:   {} delivered to the game", presses.len());
        for (ms, port, entry_name, down) in &presses {
            let action = match down {
                true => "press  ",
                false => "release",
            };
            println!("  {ms:>7} ms  port {}  {action} {entry_name}", port + 1);
        }
    }
    if let Some(class_id) = machine.take_launch_request() {
        println!("launch:    the shell asked to open {class_id:#010x}");
    }
    let media = machine.media_log();
    if !media.is_empty() {
        println!(
            "sound:     {} line(s) of what the game did with media",
            media.len()
        );
        for (ms, object, call, times) in &media {
            let repeats = match times {
                1 => String::new(),
                n => format!("  ({n}x)"),
            };
            println!("  {ms:>7} ms  {object:#010x}  {call}{repeats}");
        }
    }
    let urls = machine.web_requests();
    if !urls.is_empty() {
        println!(
            "network:   the game requested {} address(es) through IWeb",
            urls.len()
        );
        for url in &urls {
            println!("  {url}");
        }
    }
    let response = machine.web_response();
    if !response.is_empty() {
        let as_text = String::from_utf8_lossy(response);
        println!("  response: {} bytes, {as_text:?}", response.len());
    }
    let ignored = machine.ignored_gl();
    if !ignored.is_empty() {
        println!("gl:        served without doing anything ({})", ignored.len());
        println!("  {}", ignored.join(" "));
    }
    let delivered_log = machine.delivered();
    if !delivered_log.is_empty() {
        println!("bridge:    what it did with the response");
        for log_line in delivered_log {
            println!("  {log_line}");
        }
    }
    let plaintexts = machine.plaintexts();
    if !plaintexts.is_empty() {
        println!("encrypted: what the game encrypted, in plaintext");
        for block in &plaintexts {
            let hex: String = block.iter().map(|b| format!("{b:02x}")).collect();
            println!("  {} bytes  {hex}", block.len());
            println!("    {:?}", String::from_utf8_lossy(block));
        }
    }
    let key_ids = machine.cipher_keys();
    if !key_ids.is_empty() {
        println!("cipher:    the game encrypted data with");
        for key_id in &key_ids {
            println!("  {key_id}");
        }
    }
    if let Some(source) = machine.font_source() {
        println!("font:      {source}, from the game's own package");
    }
    if !machine.pending_text().is_empty() {
        println!("text on screen (no font to draw it yet):");
        for text in machine.pending_text() {
            println!("  {text:?}");
        }
    }
    if machine.screen().is_dirty() {
        let out = std::path::Path::new(path).with_extension("bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, machine.screen().to_bmp())?;
        println!(
            "screen:    {} ({}x{}, {} surfaces)",
            out.display(),
            machine.screen().width(),
            machine.screen().height(),
            machine.bitmap_count()
        );
    }
    if let Some(frame) = machine.gl_frame() {
        let out = std::path::Path::new(path).with_extension("gl.bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, frame.to_bmp())?;
        println!(
            "opengl:    {} ({} frames presented)",
            out.display(),
            machine.gl_swaps()
        );
    }
    // ARM semihosting is the other log path games use: the `SVC #0xAB` that Peggle and Zuma's
    // Revenge emit comes out here, not through `DBGPRINTF`.
    let semihosting = machine.cpu().semihosting();
    if !semihosting.trim().is_empty() {
        println!("semihosting log:");
        for log_line in semihosting.lines().take(SEMIHOSTING_LINES) {
            println!("  {log_line}");
        }
    }
    if !machine.debug_output().is_empty() {
        println!("game log:");
        for (line, count) in machine.debug_output() {
            match count {
                1 => println!("  {line}"),
                n => println!("  {line}   ({n}x)"),
            }
        }
    }
    // Delivers whatever is in the signal queue: the game registers for events and waits for the
    // callbacks. With no host input yet, the queue is usually empty.
    let delivered = machine.deliver_signals(INSTRUCTION_BUDGET)?;
    if !delivered.is_empty() {
        println!("signals:   {} callback(s) delivered", delivered.len());
    }
    let played = machine.deliver_callbacks(INSTRUCTION_BUDGET)?;
    if !played.is_empty() {
        println!("callbacks: {} delivered", played.len());
    }
    // The object region has a ceiling, and a game that gets close to it is leaking references —
    // the symptom is some arbitrary class no longer being created, which looks nothing like the
    // cause. The list only shows up when there are many live objects.
    let live = machine.live_objects_by_kind();
    let total: usize = live.iter().map(|(_, n)| n).sum();
    if total >= 256 {
        println!("live objects per interface ({total} in total):");
        for (entry_name, live_count) in live.iter().take(8) {
            println!("  {live_count:6}x {entry_name}");
        }
    }
    if !machine.swallowed_faults().is_empty() {
        println!("invalid accesses the game carried on past:");
        for diagnostic in machine.swallowed_faults() {
            println!("  {diagnostic}");
        }
    }
    if !machine.missing_apis().is_empty() {
        println!("missing APIs:");
        for diagnostic in machine.missing_apis() {
            println!("  {diagnostic}");
        }
    }
    if !machine.missing_files().is_empty() {
        println!("files not found:");
        for name in machine.missing_files() {
            println!("  {name}");
        }
    }
    if !machine.bad_pointers().is_empty() {
        println!("invalid pointers received:");
        for line in machine.bad_pointers() {
            println!("  {line}");
        }
    }
    if !machine.assumptions().is_empty() {
        println!("assumptions in use:");
        for note in machine.assumptions() {
            println!("  {note}");
        }
    }
    if tracing {
        println!("trace:");
        for line in machine.trace() {
            println!("  {line}");
        }
    }
    let unknown = machine.unknown_classes();
    if !unknown.is_empty() {
        print!("unknown classes:");
        for clsid in unknown {
            print!(" {clsid:#010x}");
        }
        println!();
    }
    let probe_calls = machine.probe_log();
    if !probe_calls.is_empty() {
        println!("probe:     what the games called on the classes served by observation");
        let mut current_object = 0;
        for (clsid, object, slot, args, texts, times) in probe_calls {
            if *object != current_object {
                current_object = *object;
                println!("  class {clsid:#010x}, object {object:#x}:");
            }
            let show = |i: usize| match &texts[i] {
                Some(as_text) => format!("{as_text:?}"),
                None => format!("{:#x}", args[i]),
            };
            let repeated = match times {
                1 => String::new(),
                n => format!("   ({n}x)"),
            };
            println!(
                "    slot[{slot:>2}] ({}, {}, {}){repeated}",
                show(1),
                show(2),
                show(3)
            );
        }
    }
    let log = machine.call_log();
    if !log.is_empty() {
        println!("calls:");
        for (name, count) in log {
            println!("  {count:>4}x {name}");
        }
    }
    Ok(())
}

/// Measures a whole ROM on the default backend, without a window.
///
/// This is not a second implementation of the `run` command: it is a narrow bench for measuring
/// how many virtual milliseconds the recompiled ARM can deliver per wall-clock second.
fn bench_dynarmic(
    path: &str,
    seconds: u32,
    dump: Option<&str>,
    keys: &input::Script,
    key_events: &[(u32, u32)],
    installed: Vec<(u32, String)>,
    surfaces: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let extracted;
    let path = match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("zip" | "7z") => {
            extracted = archive::extract(std::path::Path::new(path))?;
            extracted.as_path()
        }
        _ => std::path::Path::new(path),
    };
    let image = ModImage::parse(std::fs::read(path)?)?;
    let extensions = zeebx::session::extensions_of(path);
    let module = loader::load_with(&image, &extensions)?;
    let root = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default();
    let mut machine = Machine::new(DynarmicCpu::new()?, module, root);
    machine.set_installed_applets(installed);
    let boot = machine.run(INSTRUCTION_BUDGET)?;
    if !matches!(boot, Outcome::Returned { code: 0 }) {
        return Err(format!("loading stopped at {boot:?}").into());
    }
    let clsid = library::applet_clsid(path).ok_or("no .mif found the applet")?;
    let applet = match machine.create_applet(clsid, INSTRUCTION_BUDGET)? {
        AppletResult::Called { code: 0, applet } if applet != 0 => applet,
        other => return Err(format!("CreateInstance stopped at {other:?}").into()),
    };
    let start = machine.start_applet(applet, clsid, INSTRUCTION_BUDGET)?;
    if !matches!(start, Outcome::Returned { .. } | Outcome::Budget) {
        return Err(format!("EVT_APP_START stopped at {start:?}").into());
    }

    let wall = std::time::Instant::now();
    let base_clock = machine.clock_ms();
    let base_instructions = machine.instructions();
    let until = base_clock.saturating_add(seconds.saturating_mul(1000));
    let mut turns = 0u64;
    let mut pad = input::Pad::default();
    // Each key becomes a press and a release when the clock passes its instant. With `--dump`,
    // the frame half a second after each one is also written, numbered: it is what shows the
    // navigation step by step, and not only where it ended.
    let mut pending: std::collections::VecDeque<(u32, u32)> = {
        let mut sorted = key_events.to_vec();
        sorted.sort_by_key(|&(when, _)| when);
        sorted.into()
    };
    let mut shots: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut shot_number = 0;
    // See the comment on the same switch in the loop: it is the core's `zeebx_dpad_to_analog_p1`.
    let dpad_on_axes = cli_args().into_iter().any(|arg| arg == "--dpad-on-axes");
    while machine.clock_ms() < until && !machine.is_idle() {
        while pending
            .front()
            .is_some_and(|&(when, _)| machine.clock_ms() >= when)
        {
            let (when, avk) = pending.pop_front().unwrap_or_default();
            machine.set_key(avk, true);
            machine.set_key(avk, false);
            shots.push_back(when.saturating_add(500));
        }
        if let Some(path) = dump {
            while shots
                .front()
                .is_some_and(|&when| machine.clock_ms() >= when)
            {
                shots.pop_front();
                shot_number += 1;
                let shot_file = match path.strip_suffix(".bmp") {
                    Some(base) => format!("{base}.{shot_number}.bmp"),
                    None => format!("{path}.{shot_number}"),
                };
                std::fs::write(shot_file, machine.screen().to_bmp())?;
            }
        }
        // `--dpad-on-axes` is the same thing the core option does every frame, to measure issue
        // #39 without a window. **Before** the script, on purpose: a script axis step is a real
        // stick, and it gets the last word — see the mirror in `Player::pad`.
        if dpad_on_axes {
            pad.mirror_dpad_on_axes();
        }
        keys.apply(machine.clock_ms(), &mut pad);
        machine.set_pad(pad);
        let outcomes = machine.advance(INSTRUCTION_BUDGET)?;
        machine.deliver_signals(INSTRUCTION_BUDGET)?;
        machine.deliver_callbacks(INSTRUCTION_BUDGET)?;
        turns += 1;
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            return Err(format!("loop stopped at {bad:?}").into());
        }
    }
    let elapsed = wall.elapsed();
    let virtual_ms = machine.clock_ms().saturating_sub(base_clock);
    let instructions = machine.instructions().saturating_sub(base_instructions);
    let ratio = virtual_ms as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE) / 10.0;
    println!("backend:   Dynarmic ARMv6K");
    println!(
        "time:      {virtual_ms} virtual ms in {:.3} real s ({ratio:.1}% of speed)",
        elapsed.as_secs_f64()
    );
    println!(
        "cpu:       {} million instructions ({:.1} MIPS)",
        instructions / 1_000_000,
        instructions as f64 / elapsed.as_secs_f64().max(f64::MIN_POSITIVE) / 1_000_000.0
    );
    println!(
        "loop:      {turns} passes, {} timer(s), {} GL frame(s)",
        machine.armed_timers(),
        machine.gl_swaps()
    );
    let media: Vec<_> = machine
        .call_log()
        .into_iter()
        .filter(|(name, _)| name.starts_with("IMedia::"))
        .collect();
    if !media.is_empty() {
        println!("media:");
        for (name, count) in media {
            println!("  {count:>4}x {name}");
        }
    }
    // **The core's log, which in the window goes to the frontend's log.** In the harness it is
    // the reason to exist: these lines are how you read what the engine decided -- the sound
    // delivered and the decoded duration, the stream the game stops feeding, and who silenced
    // each sound. Without them, measuring that required opening RetroArch, and with it someone's
    // navigation.
    //
    // The level comes in through `ZEEBX_LOG` (`warning` is the default): the instrumentation
    // lines are `info`, and without the variable they do not show up.
    let logging = zeebx::logging::drain();
    if !logging.is_empty() {
        println!("log:");
        for line in logging {
            println!("  [{}] {}: {}", line.level.tag(), line.target, line.text);
        }
    }
    // **Say what was lost.** An instrument that drops lines silently lies by omission: that is how
    // a measurement reported "zero sounds decoded" with 123 sounds requested in the log.
    let dropped = zeebx::logging::dropped();
    if dropped > 0 {
        println!("log:       {dropped} line(s) were dropped by the ring before being read");
    }
    // The input calls say whether the game ever queries the controller and through which
    // channel: button events (`GetNextButtonEvent`) or position (`GetPositionState`). It is the
    // question that decides whether mirroring the D-pad onto the axes changes anything for this
    // game.
    let entry: Vec<_> = machine
        .call_log()
        .into_iter()
        .filter(|(name, _)| {
            name.contains("Position") || name.contains("Button") || name.contains("HID")
        })
        .collect();
    if !entry.is_empty() {
        println!("input:");
        for (name, count) in entry {
            println!("  {count:>4}x {name}");
        }
    }
    // The call count says the game **asks**; this one says the answer **arrived**. Without it,
    // an arcade port that queries the axis every frame looks the same with the D-pad released
    // and pressed — and it is the number that proves the D-pad mirror (issue #39).
    let off_center = machine.off_center_axis_reads();
    if off_center > 0 {
        // **Which** axes, not just how many: `X` and `Y` belong to the left stick, `Z` and `RZ`
        // to the right. It is what answers "did the D-pad reach the stick the game reads?".
        let names = machine.off_center_axes_seen().join(", ");
        println!("axis:      {off_center} off-center read(s), on {names}");
    }
    if let Some(path) = dump {
        std::fs::write(path, machine.screen().to_bmp())?;
        println!("frame:     {path}");
    }
    if let Some(dir) = surfaces {
        write_surfaces(&machine, dir)?;
    }
    if let Some(class_id) = machine.take_launch_request() {
        println!("launch:    the shell asked to open {class_id:#010x}");
    }
    Ok(())
}

/// Runs a game along the same path as the window, without opening it.
///
/// The bench builds the machine on its own, and what it shows may not be what the window shows:
/// the window installs every game in the library, passes the controller through the session and
/// translates the D-pad into keys. Here the same pieces come in — `Session`, the settings'
/// library and [`input::controller_key_events`] — and the script is of **controller buttons**,
/// like someone playing. With `--dump`, a frame comes out half a second after each press; the
/// whole report goes to the output at the end.
fn windowless_session(
    path: &str,
    seconds: u32,
    dump: Option<&str>,
    keys: &input::Script,
    instants: &[u32],
    gpu: bool,
    serial: Option<&str>,
    z_wheel: ui::settings::ZWheel,
    scale: usize,
    enhancements: (usize, usize),
    profile: Option<u32>,
    boomerang: Option<Vec<(u32, [f32; 3])>>,
    requested_ports: Option<[Option<bindings::Device>; input::PORTS]>,
    tracing: bool,
    trace_filter: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let serial = serial.map(std::path::Path::new);
    let settings = ui::settings::Settings::load();
    let games = settings
        .roms_dir
        .as_deref()
        .map(library::scan)
        .unwrap_or_default();
    // `--wiimote` uses the connected Wii Remote instead of the script.
    let wiimote = cli_args().into_iter()
        .any(|a| a == "--wiimote")
        .then(input::wiimote::Wiimotes::start);
    let boomerang = boomerang.or(wiimote.as_ref().map(|_| Vec::new()));
    let ports = match (requested_ports, &boomerang) {
        (Some(ports), _) => ports,
        (None, Some(_)) => [Some(bindings::Device::Boomerang), None],
        (None, None) => zeebx::DEFAULT_PORTS,
    };
    let mut session = session::Session::start_with(
        std::path::Path::new(path),
        ports,
        serial,
        gpu,
        None,
        z_wheel,
    )
    .map_err(|err| format!("{err:?}"))?;
    session.set_internal_resolution(scale);
    session.set_aspect_ratio(aspect_ratio_from_cli());
    session.set_enhancements(enhancements.0, enhancements.1);
    session.set_tracing(tracing);
    session.set_trace_filter(trace_filter);
    let mut profile_enabled_at: Option<(u32, std::time::Instant, u64)> = None;
    session.set_installed_applets(
        games
            .iter()
            .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?))),
    );
    let real_start = std::time::Instant::now();
    let mut end = seconds.saturating_mul(1000);
    let mut reopened = false;
    let mut pad = input::Pad::default();
    // `--dump-audio=A.wav` here too: the session is the window's, and that is where sound
    // happens.
    let recording = cli_args().into_iter()
        .find_map(|a| a.strip_prefix("--dump-audio=").map(str::to_string))
        .map(|wav_path| (wav_path, session.record_audio(RECORD_RATE)));
    let mut recorded: Vec<f32> = Vec::new();
    let mut recorded_ms = 0u64;
    let mut shots: std::collections::VecDeque<u32> = instants.iter().copied().collect();
    let mut number = 0;
    let dpad_on_axes = cli_args().into_iter().any(|arg| arg == "--dpad-on-axes");
    while session.clock_ms() < end {
        if let (Some(since), None) = (profile, profile_enabled_at) {
            if session.clock_ms() >= since {
                session.enable_api_profile();
                profile_enabled_at = Some((
                    session.clock_ms(),
                    std::time::Instant::now(),
                    session.instructions(),
                ));
            }
        }
        let before = pad;
        if !reopened {
            // The same switch as in `bench`, and for the same reason: measuring issue #39 in the
            // windowless session. Before the script, so the script's axis step wins.
            if dpad_on_axes {
                pad.mirror_dpad_on_axes();
            }
            keys.apply(session.clock_ms(), &mut pad);
        } else {
            pad = input::Pad::default();
        }
        session.set_port_pad(0, pad);
        if let Some(wiimotes) = &wiimote {
            // The same rotation into the Boomerang's frame of reference that the window applies.
            let [x, y, z] = wiimotes
                .state(0)
                .filter(|e| e.with_accelerometer)
                .map_or([0.0, 0.0, 1.0], |e| e.acceleration);
            session.set_port_motion(0, [y, x, z]);
        } else if let Some(script) = &boomerang {
            // The last motion whose instant has already passed; before the first one, at rest
            // face up.
            let current = script
                .iter()
                .rev()
                .find(|(when, _)| session.clock_ms() >= *when)
                .map_or([0.0, 0.0, 1.0], |(_, g)| *g);
            session.set_port_motion(0, current);
        }
        for (avk, pressed) in input::controller_key_events(&before, &pad) {
            session.set_key(avk, pressed);
        }
        // Intermediate screens go by as in the window, without advancing the clock; they do not
        // become shots.
        if session.show_intermediate_frame() {
            continue;
        }
        if (0..input::BUTTONS).any(|b| pad.is_down(b) && !before.is_down(b)) {
            shots.push_back(session.clock_ms().saturating_add(500));
            shots.make_contiguous().sort_unstable();
        }
        let stopped = matches!(
            session.step(std::time::Duration::from_millis(16), false),
            session::Step::Stopped
        );
        if let Some((_, mixer)) = &recording {
            let current = u64::from(session.clock_ms());
            let frames = (current.saturating_sub(recorded_ms) * u64::from(RECORD_RATE)) / 1000;
            if frames > 0 {
                recorded.extend(mixer.render(frames as usize));
                recorded_ms = current;
            }
        }
        if let Some(path) = dump {
            while shots.front().is_some_and(|&t| session.clock_ms() >= t) {
                shots.pop_front();
                number += 1;
                let name = match path.strip_suffix(".bmp") {
                    Some(base) => format!("{base}.{number}.bmp"),
                    None => format!("{path}.{number}"),
                };
                if let Some(gl) = session.frame_gl() {
                    std::fs::write(name.replace(".bmp", ".gl.bmp"), gl.to_bmp())?;
                }
                println!(
                    "shot {number}:    {} virtual ms, {} real ms, {} GL frames so far",
                    session.clock_ms(),
                    real_start.elapsed().as_millis(),
                    session.presented_frames()
                );
                let in_window = session.gpu_frame().is_some();
                if let Some(large) = session.large_frame() {
                    std::fs::write(name.replace(".bmp", ".grande.bmp"), large.to_bmp())?;
                    println!(
                        "large:     {}x{} at {} ms, {}",
                        large.width(),
                        large.height(),
                        session.clock_ms(),
                        match in_window {
                            true => "the window shows this one",
                            false => "the window shows the 640×480 screen (there is 2D on top)",
                        }
                    );
                }
                std::fs::write(name, session.screen().to_bmp())?;
            }
        }
        if let Some(class_id) = session.take_launch_request() {
            let matching_game = games.iter().find(|game| game.clsid == Some(class_id));
            println!(
                "launch:    {class_id:#010x} at {} ms → {}",
                session.clock_ms(),
                matching_game.map_or(
                    "no game in the library with that class".to_string(),
                    |g| g.path.display().to_string()
                )
            );
            break;
        }
        if stopped && session.class_id() == session::Z_WHEEL && session.exited_on_its_own() {
            // The same as the window does: see `session::Z_WHEEL`.
            println!(
                "reopen:    the Z-Wheel exited at {} ms; reopening",
                session.clock_ms()
            );
            let offset = session.clock_ms();
            session = session::Session::start_with(
                std::path::Path::new(path),
                zeebx::DEFAULT_PORTS,
                serial,
                gpu,
                None,
                z_wheel,
            )
            .map_err(|err| format!("{err:?}"))?;
            session.set_internal_resolution(scale);
            session.set_aspect_ratio(aspect_ratio_from_cli());
            session.set_enhancements(enhancements.0, enhancements.1);
            session.set_installed_applets(
                games
                    .iter()
                    .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?))),
            );
            let _ = offset;
            reopened = true;
            end = 4000;
            continue;
        }
        if stopped {
            println!("stopped:   {:?}", session.stopped_reason());
            let (regs, stack, lr) = session.failure();
            if let Some(lr) = lr {
                let regs: Vec<String> = regs
                    .iter()
                    .enumerate()
                    .map(|(i, v)| format!("r{i}={v:#x}"))
                    .collect();
                println!("           {} lr={lr:#x}", regs.join(" "));
                let stack: Vec<String> = stack.iter().map(|v| format!("{v:#x}")).collect();
                println!("           stack: {}", stack.join(" "));
            }
            break;
        }
    }
    if let Some(path) = dump {
        std::fs::write(path, session.screen().to_bmp())?;
    }
    println!("time:      {} virtual ms", session.clock_ms());
    let (heap, objects) = session.memory();
    println!("heap:      {heap} bytes in use, {objects} live objects");
    // **How much is left does not say how it is left.** A heap with 30 MB free in 400 holes
    // cannot serve a 2 MB allocation, and the game reports that as out of memory: the two lines
    // below are what separate "filled up" from "fell to pieces" in the report.
    let snapshot = session.heap_snapshot();
    println!(
        "           {} hole(s), largest free {} of {} free ({}% of the free space stuck in holes), {} live blocks of {}",
        snapshot.holes,
        snapshot.largest_hole,
        snapshot.free,
        u64::from(snapshot.lost_in_holes()) * 100 / u64::from(snapshot.free.max(1)),
        snapshot.live,
        snapshot.cap,
    );
    let (refusals, checks) = session.heap_refusals();
    if let Some((size, lr)) = refusals.first() {
        println!(
            "refused:   {} malloc request(s) with no room; the first: {} bytes, requested at {lr:#010x}",
            refusals.len(),
            size
        );
    }
    if let Some((size, lr)) = checks.first() {
        println!(
            "refused:   {} available-memory question(s) answered with \"does not fit\"; the first: {} bytes at {lr:#010x}",
            checks.len(),
            size
        );
    }
    if let (Some((wav_path, _)), false) = (&recording, recorded.is_empty()) {
        std::fs::write(wav_path, audio::to_wav(&recorded, RECORD_RATE))?;
        let peak = recorded.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("audio:     {wav_path} (peak {peak:.3})");
    }
    // **The core's log, with the sound alongside.** This function is the only one that writes
    // the mixed audio to WAV, so this is where the two meet: the WAV says *what* played, the log
    // says *why* -- the sound delivered and the decoded duration, the stream the game stops
    // feeding, and who silenced each sound. Keeping them apart is what had me chasing a cut-off
    // voice without knowing whether it had been cut off.
    let logging = zeebx::logging::drain();
    if !logging.is_empty() {
        println!("log:");
        for log_line in logging {
            println!("  [{}] {}: {}", log_line.level.tag(), log_line.target, log_line.text);
        }
    }
    // **Say what was lost.** An instrument that drops lines silently lies by omission: that is how
    // a measurement reported "zero sounds decoded" with 123 sounds requested in the log.
    let dropped = zeebx::logging::dropped();
    if dropped > 0 {
        println!("log:       {dropped} line(s) were dropped by the ring before being read");
    }
    if let Some((since, started_at, instructions_before)) = profile_enabled_at {
        let real = started_at.elapsed();
        let virtual_ms = session.clock_ms().saturating_sub(since);
        let api = session.api_profile();
        let total: u64 = api.iter().map(|(_, ns)| ns).sum();
        println!(
            "profile:   since {since} ms: {} real ms for {virtual_ms} virtual ms ({:.0}% of the console), {} ms serving the API ({:.0}%)",
            real.as_millis(),
            f64::from(virtual_ms) / real.as_millis().max(1) as f64 * 100.0,
            total / 1_000_000,
            total as f64 / real.as_nanos().max(1) as f64 * 100.0,
        );
        let instructions = session.instructions().saturating_sub(instructions_before);
        println!(
            "           {:.0} million ARM instructions: {:.0} per real second, {:.0} per virtual second (the console does 528)",
            instructions as f64 / 1e6,
            instructions as f64 / 1e6 / real.as_secs_f64().max(1e-9),
            instructions as f64 / 1e6 / (f64::from(virtual_ms) / 1000.0).max(1e-9),
        );
        for (name, ns) in api.iter().take(PROFILE_LINES) {
            println!(
                "  {:5.1}%  {:>8} ms  {name}",
                *ns as f64 / total.max(1) as f64 * 100.0,
                ns / 1_000_000
            );
        }
    }
    for log_line in session.log() {
        println!("{log_line}");
    }
    if tracing {
        println!("trace:");
        for line in session.trace() {
            println!("  {line}");
        }
    }
    Ok(())
}

/// Writes the heap and the module image as they are in memory.
///
/// The module image is worth having alongside the heap: at runtime it has already gone through
/// the relocations the `elf2mod` stub applies on its own, and the file on disk has not.
///
/// Like the profile, this also helps when the game **never** gets to start: Need For Speed builds
/// its sound table before the applet exists, and that table holds the answer to why it stops.
fn dump_memory(
    machine: &machine::Machine<DefaultBackend>,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(
        "heap.bin",
        machine.dump(loader::HEAP_BASE, loader::HEAP_SIZE)?,
    )?;
    let (base, len) = {
        let region = &machine.module().mem.regions()[0];
        (region.base, region.bytes.len())
    };
    std::fs::write("module.bin", machine.dump(base, len)?)?;
    println!(
        "           heap.bin (base {:#010x}) and module.bin (base {:#010x})",
        loader::HEAP_BASE,
        base
    );
    Ok(())
}

/// Writes one BMP per live surface.
///
/// "The game draws and the screen stays black" has two possible causes, and only the contents of
/// the surfaces tell them apart: either it drew onto something that does not go to the screen,
/// or it did not draw. Looking at their contents solved Tekken 2's text in minutes after hours of
/// guessing.
fn write_surfaces<C: cpu::CpuBackend>(
    machine: &machine::Machine<C>,
    dir: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let surfaces = machine.surfaces();
    for (addr, width, height, bmp) in &surfaces {
        std::fs::write(format!("{dir}/{addr:08x}-{width}x{height}.bmp"), bmp)?;
    }
    println!("surfaces: {} in {dir}/", surfaces.len());
    Ok(())
}

/// Writes the outcome and, when it is a memory fault, the registers and the stack.
///
/// Without the registers, "invalid access at 0x00000000" says something was null and does not
/// say **which** — and that is exactly the question. They were already being kept; what was
/// missing was showing them for the faults inside `EVT_APP_START` too, which is where apps die.
fn describe_outcome_with_state<C: cpu::CpuBackend>(
    outcome: &Outcome,
    machine: &machine::Machine<C>,
) {
    describe_outcome(outcome);
    if !matches!(outcome, Outcome::Fault { .. } | Outcome::Exception { .. }) {
        return;
    }
    let regs = machine.fault_regs();
    print!("           ");
    for (i, value) in regs.iter().enumerate() {
        print!("r{i}={value:#x} ");
    }
    println!();
    let stack = machine.fault_stack();
    if !stack.is_empty() {
        let items: Vec<String> = stack.iter().map(|v| format!("{v:#x}")).collect();
        println!("           stack: {}", items.join(" "));
    }
}

fn describe_outcome(outcome: &Outcome) {
    match outcome {
        Outcome::Returned { code } => println!("returned {code}"),
        Outcome::Unimplemented { addr, args, caller } => {
            println!("unimplemented API — {}", aee::describe(*addr));
            println!(
                "           args r0={:#x} r1={:#x} r2={:#x} r3={:#x}  (called from {caller:#010x})",
                args[0], args[1], args[2], args[3]
            );
        }
        Outcome::Fault { addr, pc, lr } => {
            println!("invalid access at {addr:#010x} (pc {pc:#010x}, lr {lr:#010x})")
        }
        Outcome::Exception { pc } => println!("ARM core exception at pc {pc:#010x}"),
        Outcome::Budget => println!("instruction budget exhausted"),
        Outcome::CallLimit { calls } => {
            println!("{calls} API calls without yielding — probably a busy loop")
        }
    }
}

/// Runs the frame loop: advances the virtual clock, fires the timers that expired and delivers
/// the pending callbacks.
///
/// The game draws inside the callback of the timer it re-arms itself, so it is the timer firing
/// that produces each frame. Stops when the game has nothing else armed — with no timer or
/// callback pending, nothing else is going to happen.
#[allow(clippy::too_many_arguments)]
fn run_frames(
    machine: &mut Machine<DefaultBackend>,
    rounds: u32,
    seconds: Option<u32>,
    path: &str,
    dump_heap: bool,
    show: bool,
    keys: &input::Script,
    key_events: &[(u32, u32)],
    dump_gl: Option<&str>,
    dump_audio: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // With a window, sound goes out through the sound card; without one, the goal is dumping
    // frames, and an open audio stream would only get in the way.
    let speaker = match show {
        true => match audio::Output::open(1.0, false) {
            Ok(output) => {
                machine.set_audio(Some(output.mixer()));
                Some(output)
            }
            Err(err) => {
                println!("sound:     no audio output ({err})");
                None
            }
        },
        false => None,
    };
    let mut window = match show {
        true => Some(window::Window::open(
            "Zeebx",
            machine.screen().width(),
            machine.screen().height(),
        )?),
        false => None,
    };
    let mut drawn = 0;
    let mut pending_key_events: std::collections::VecDeque<(u32, u32)> = {
        let mut sorted = key_events.to_vec();
        sorted.sort_by_key(|&(when, _)| when);
        sorted.into()
    };
    let mut turns = 0;
    let mut stopped = None;
    let mut pad = input::Pad::default();
    let deadline = seconds.map(|s| s.saturating_mul(1000));
    if let Some(dir) = dump_gl {
        std::fs::create_dir_all(dir)?;
    }
    let mut dumped = 0;
    // Sound is mixed right here, at the pace of the virtual clock, instead of going out through
    // the sound card: that is how the audio can be **checked** without listening to it — and the
    // file that comes out is comparable with the sounds the game delivered.
    let recorder = dump_audio.map(|_| {
        let mixer = audio::Mixer::silent(RECORD_RATE);
        machine.set_audio(Some(mixer.clone()));
        mixer
    });
    let mut recorded: Vec<f32> = Vec::new();
    let mut recorded_ms = 0u64;
    let mut shown = 0;
    let mut refreshed = std::time::Instant::now();
    // The virtual clock runs on its own: when the game sleeps, `skip_idle_time` jumps time ahead
    // instead of spending it, and an idle stretch goes by in almost no real time. That is what
    // keeps the time the game *measures* right, but nothing held the emulator to the world's
    // pace — Crash ran 12 virtual seconds in 1.5 real ones. Only the window needs the brake;
    // without it, finishing fast is exactly the point.
    let started = std::time::Instant::now();
    let clock_base = u64::from(machine.clock_ms());
    for round in 0..rounds {
        if let Some(window) = &mut window {
            if !window.is_open() {
                break;
            }
            let ahead = u64::from(machine.clock_ms())
                .saturating_sub(clock_base)
                .saturating_sub(started.elapsed().as_millis() as u64);
            if ahead > 0 {
                std::thread::sleep(std::time::Duration::from_millis(ahead));
            }
            // One loop pass is not a frame: the game can run its whole main loop inside a
            // single callback, and each pass here only returns the slice of instructions that
            // fit. In Peteca that is a million and a half passes for a few dozen frames —
            // redrawing on all of them drowns the window, which never gets composited and shows
            // up blank. What drives redrawing is `eglSwapBuffers`.
            let swaps = machine.gl_swaps();
            let presented = swaps != shown;
            // A 2D game never presents through OpenGL, and even 3D has to handle the keyboard
            // and the close request between one frame and the next: hence the wall-clock
            // interval.
            if presented || refreshed.elapsed() >= REFRESH_PERIOD {
                shown = swaps;
                refreshed = std::time::Instant::now();
                window.show(machine.screen())?;
                pad = window.pad();
            }
        }
        // The script's keys come in when the game clock passes the marked instant. Each one goes
        // as a press followed by a release, which is what a tap is.
        while pending_key_events
            .front()
            .is_some_and(|&(when, _)| machine.clock_ms() >= when)
        {
            let (_, avk) = pending_key_events.pop_front().unwrap_or_default();
            machine.set_key(avk, true);
            machine.set_key(avk, false);
        }
        if deadline.is_some_and(|limit| machine.clock_ms() >= limit) {
            break;
        }
        // The script goes on top of the keyboard: that way input can be checked without a
        // window and, with one, you can see what the script does.
        keys.apply(machine.clock_ms(), &mut pad);
        machine.set_pad(pad);
        turns += 1;
        let outcomes = machine.advance(INSTRUCTION_BUDGET)?;
        machine.deliver_signals(INSTRUCTION_BUDGET)?;
        machine.deliver_callbacks(INSTRUCTION_BUDGET)?;

        // A slice's instruction cap is a request to yield, not a bad outcome. See the same
        // decision in `session.rs`.
        if let Some(bad) = outcomes
            .iter()
            .find(|outcome| !matches!(outcome, Outcome::Returned { .. } | Outcome::Budget))
        {
            stopped = Some((round, bad.clone()));
            break;
        }
        // One frame per file: comparing two neighboring frames is what tells whether an artifact
        // comes from the drawing or from state left stuck from one frame to the next.
        if let Some(dir) = dump_gl {
            while dumped < machine.gl_swaps() {
                dumped += 1;
                if let Some(frame) = machine.gl_frame() {
                    let out = format!("{dir}/{dumped:05}.bmp");
                    std::fs::write(out, frame.to_bmp())?;
                }
            }
        }
        // Records the sound for the virtual time that passed since the previous pass.
        if let Some(mixer) = &recorder {
            let now = u64::from(machine.clock_ms());
            let frames = (now.saturating_sub(recorded_ms) * u64::from(RECORD_RATE)) / 1000;
            if frames > 0 {
                recorded.extend(mixer.render(frames as usize));
                recorded_ms = now;
            }
        }
        drawn += outcomes.len();
        if outcomes.is_empty() && machine.is_idle() {
            break;
        }
    }

    drop(speaker);
    if let (Some(path), false) = (dump_audio, recorded.is_empty()) {
        std::fs::write(path, audio::to_wav(&recorded, RECORD_RATE))?;
        let loudest = recorded.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!(
            "audio:     {path} ({:.1} s, peak {loudest:.3})",
            recorded.len() as f32 / (RECORD_RATE * 2) as f32
        );
    }
    println!(
        "time:      {drawn} timer firing(s) in {} virtual ms, {} timer(s) armed",
        machine.clock_ms(),
        machine.armed_timers()
    );
    println!(
        "           {} million instructions in {turns} loop pass(es)",
        machine.instructions() / 1_000_000,
    );
    if let Some((frame, outcome)) = stopped {
        print!("           stopped on pass {frame} at ");
        describe_outcome(&outcome);
        if let Outcome::Fault { .. } = outcome {
            let regs = machine.fault_regs();
            let dump: Vec<String> = regs
                .iter()
                .enumerate()
                .map(|(i, value)| format!("r{i}={value:#x}"))
                .collect();
            println!("           {}", dump.join(" "));
            let stack = machine.fault_stack();
            if !stack.is_empty() {
                let trail: Vec<String> = stack.iter().map(|a| format!("{a:#x}")).collect();
                println!("           stack: {}", trail.join(" "));
            }
            // What explains a null pointer is the struct that led to it, and it only exists in
            // guest memory — hence being able to take it out and analyze it at leisure.
            if dump_heap {
                dump_memory(machine)?;
            }
        }
    }
    if drawn > 0 && machine.screen().is_dirty() {
        let out = std::path::Path::new(path).with_extension("frames.bmp");
        let out = out.file_name().map(std::path::Path::new).unwrap_or(&out);
        std::fs::write(out, machine.screen().to_bmp())?;
        println!("           last frame in {}", out.display());
    }
    Ok(())
}

/// Prints what we know how to read from a `.mod`.
fn info(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let data = std::fs::read(path)?;
    let size = data.len();
    let image = ModImage::parse(data)?;

    println!("file:     {path}");
    println!("size:     {size} bytes ({size:#x})");
    println!(
        "variant:  {}",
        match image.variant() {
            Variant::Raw => "no header (raw ARM code)",
            Variant::BrewHeader => "BREW header",
        }
    );
    println!("entry:    {:#010x}  (AEEMod_Load)", image.entry());

    if let Some(header) = image.header() {
        println!("version:  {}", header.version);
        println!("code_off: {:#x}", header.code_offset);
        print!("undeciphered fields (0x14..0x3c):");
        for word in header.unknown {
            print!(" {word:#x}");
        }
        println!();
    }
    Ok(())
}
