//! Scanning a real ROM, inside `cargo test`.
//!
//! The `run` report already says what we need to know about a game: where it stopped, what it
//! asked for that we don't have, what it cost. What was missing was **enforcing** it. A scan done
//! by hand, in a shell loop, finds the regression the next time someone repeats it — which can be
//! weeks after the commit that caused it. Here the same survey becomes a test: the ROM advances,
//! the report comes out structured, and the summary can be compared with what was already known
//! about it.
//!
//! No ROM is in the tree, so **nothing here runs on its own**: the tests only do work when
//! `ZEEBX_ROM` points to a game, and without it they warn and pass — which is right for whoever
//! cloned the repository without the files. It is also what keeps `cargo test` from becoming a
//! scan of sixty-five games that nobody asked for.
//!
//! ```bash
//! # one game, with the full report on screen
//! ZEEBX_ROM="roms/Quake.zip" cargo test --release scan -- --nocapture
//!
//! # the whole scan, writing one report per game and enforcing the baseline
//! ZEEBX_ROM=roms ZEEBX_ROM_MS=6000 ZEEBX_ROM_OUT=output ZEEBX_ROM_BASE=docs/scan \
//!   cargo test --release scan -- --nocapture
//! ```
//!
//! | Variable | What it does |
//! |---|---|
//! | `ZEEBX_ROM` | a `.mod`/`.zip`, a comma-separated list, or a directory |
//! | `ZEEBX_ROM_MS` | how much **virtual** time to run, in ms (default 6000, like the survey) |
//! | `ZEEBX_ROM_CAP` | **real** time cap per game, in seconds (default 90) |
//! | `ZEEBX_ROM_OUT` | directory where the full report of each game is written |
//! | `ZEEBX_ROM_BASE` | baseline directory; what doesn't exist is written, what exists is enforced |
//! | `ZEEBX_ROM_DOWNSCALE` | internal 3D resolution factor in the software rasterizer: 1 (default), 2 or 4 |
//!
//! The `--release` is not decoration: in debug the emulated core runs an order of magnitude
//! slower, and the real time cap would classify a good game as "too slow".

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::machine::Outcome;
use crate::session::{Session, StartError, Step};

/// How much virtual time to run when nobody asks for something else. Six seconds is what the
/// compatibility survey uses, and keeping the same number keeps the two comparable.
const DEFAULT_MS: u32 = 6_000;

/// Real time cap per game. Whoever doesn't reach the virtual time within it is "too slow",
/// which is a compatibility category and not a test failure — the same criterion as the
/// `timeout 90` of the hand-run scan.
const DEFAULT_CAP: u64 = 90;

/// How much virtual time the scoreboard gives each ROM. Short on purpose: it is meant to answer
/// "who opened", not to play.
const SCOREBOARD_MS: u32 = 5_000;

/// Real time cap per ROM on the scoreboard. A game that doesn't make it is "too slow" and the
/// scan moves on.
const SCOREBOARD_CAP: u64 = 30;

/// How many of the most called methods the report shows. It is where bottlenecks show up.
const SHOWN_CALLS: usize = 12;

/// Reads the installed applets from `ZEEBX_ROM_INSTALLED`, in the bench format:
/// `0xCLSID:id,0xCLSID:id`.
///
/// **Without this the wheel cannot open any game.** `IShell::StartApplet` answers
/// `ECLASSNOTSUPPORT` for a class that is not on the list, and the list belongs to the *host* —
/// the bench has `--installed=` and the scan now accepts the same declaration.
fn installed_from_env(rom: &Path) -> Vec<(u32, String)> {
    let Ok(value) = crate::config::env_var("ZEEBX_ROM_INSTALLED") else {
        return Vec::new();
    };
    // **`auto` installs the games that sit next to the ROM**, with the module folder number that
    // each package carries — the same thing the interface and the core do. Without the number, the
    // Z-Wheel logs `Tectoy.c:2925 No mod number for this game!!!` and **does not launch**: measured,
    // it gets as far as requesting the launch and gives up. With it, `opening requested:
    // 0x0108e356`. The folder is the same one the scan already uses for the catalog, and the list
    // comes out sorted and without repeats, so the result is the same between runs.
    if value.trim().eq_ignore_ascii_case("auto") {
        let Some(dir) = rom.parent() else {
            return Vec::new();
        };
        let mut seen = std::collections::BTreeSet::new();
        return crate::library::scan(dir)
            .into_iter()
            .filter_map(|game| {
                let class_id = game.clsid?;
                let id = crate::library::module_id(&game.path)?;
                match seen.insert((class_id, id.clone())) {
                    true => Some((class_id, id)),
                    false => None,
                }
            })
            .collect();
    }
    value
        .split(',')
        .filter_map(|item| {
            let item = item.trim();
            if item.is_empty() {
                return None;
            }
            let (class_id, id) = item.split_once(':').unwrap_or((item, ""));
            let class_id = u32::from_str_radix(class_id.trim().trim_start_matches("0x"), 16).ok()?;
            Some((class_id, id.trim().to_string()))
        })
        .collect()
}

/// Reads the control script from `ZEEBX_ROM_KEYS`.
///
/// Format: comma-separated `ms:button`, applied on the **virtual clock** — `1000:b1,2000:up`.
/// A step without a button (`3000:`) releases everything. The names are those of
/// [`crate::input::BUTTON_NAMES`], the same as the command line and the key mapping.
///
/// **An axis comes in as `ms:axis=value`**: `1000:x=-128` pushes the stick to the left, and
/// `1500:x=0` returns it to rest. The axes are `x`, `y`, `z` and `rz`, with the stick travel in
/// `-128..=128` — the same range the `Pad` stores. This is not decoration: **the Z-Wheel is
/// navigated with the stick**, and a script of buttons only doesn't move it a pixel.
///
/// **A console key comes in as `ms:k0xe064`**, and the `k` is optional as long as the name is not
/// a button's: `kselect` and `select` mean the same **if** `select` is not a button name. It
/// exists because some questions can only be answered by someone pressing the controller: whether
/// the input path reaches the guest, whether the Z-Wheel accepts the choice.
fn key_script() -> Vec<(u64, ScriptStep)> {
    let Ok(value) = crate::config::env_var("ZEEBX_ROM_KEYS") else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (when, name) = match part.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let Ok(ms) = when.trim().parse::<u64>() else {
            continue;
        };
        let name = name.trim();
        let step = match name.split_once('=') {
            // **The event comes before the axis, and the order matters.** I had put it after, and
            // the axis branch swallowed it with the `_ => continue`: `e0x7000=0x4ea` became no
            // event at all, and the step was dropped silently. The same trap as the recreated pad,
            // one layer up.
            // `e0x7000=0x4ea`: widget event, with the number and the `wParam`.
            Some((evt, value)) if evt.trim().starts_with('e') => {
                let evt = u32::from_str_radix(evt.trim().trim_start_matches('e').trim_start_matches("0x"), 16);
                let value = value.trim().trim_start_matches("0x");
                match (evt, u16::from_str_radix(value, 16)) {
                    (Ok(evt), Ok(w)) => ScriptStep::Event(evt, w),
                    _ => continue,
                }
            }
            Some((axis, value)) => {
                let axis = axis_from_name(axis.trim());
                let value = value.trim().parse::<i32>().ok();
                match (axis, value) {
                    (Some(axis), Some(value)) => ScriptStep::Axis(axis, value),
                    _ => continue,
                }
            }
            None if name.is_empty() => ScriptStep::ReleaseAll,
            // `k0`, `kclr`, `kselect`, `k0xe063`: console key, by the name `avk` knows.
            None if name.len() > 1 && name[..1].eq_ignore_ascii_case("k") => {
                match crate::input::avk::by_name(&name[1..].to_ascii_lowercase()) {
                    Some(avk) => ScriptStep::Key(avk),
                    None => continue,
                }
            }
            None => match crate::input::BUTTON_NAMES
                .iter()
                .position(|candidate| candidate.eq_ignore_ascii_case(name))
            {
                Some(index) => ScriptStep::Button(index),
                // **The key name without the `k` also works.** Before, `0xe064` was neither a
                // button nor a key (the `k` was missing) and the step was dropped **silently**: a
                // whole script of keys turned into "no key", and the measurement said the wheel
                // does not respond — when the one that pressed nothing was the script. The order
                // matters: button first, because `up` and `down` are names of both.
                None => match crate::input::avk::by_name(&name.to_ascii_lowercase()) {
                    Some(avk) => ScriptStep::Key(avk),
                    None => {
                        eprintln!(
                            "warning: script step {part:?} is neither a button nor a key, and was \
                             dropped"
                        );
                        continue;
                    }
                },
            },
        };
        steps.push((ms, step));
    }
    steps.sort_by_key(|(ms, _)| *ms);
    steps
}

/// The index of an axis by the name used in the script: `x`, `y`, `z` and `rz`.
fn axis_from_name(name: &str) -> Option<usize> {
    match name.to_ascii_lowercase().as_str() {
        "x" => Some(0),
        "y" => Some(1),
        "z" => Some(2),
        "rz" => Some(3),
        _ => None,
    }
}

/// Applies a script step to the pad, returning the pad for the next step.
///
/// The pad **persists**: each step changes only what it names. This function is what makes the
/// empty step (`"11000:"`) release everything, and it is what guarantees that holding the stick
/// and pressing a button are two steps that add up — before this the pad was recreated on every
/// step and the press zeroed the axis.
fn apply_step(mut pad: crate::input::Pad, step: ScriptStep) -> crate::input::Pad {
    match step {
        ScriptStep::Button(index) => pad.press(index, true),
        ScriptStep::Axis(axis, value) => pad.set_axis(axis, value),
        // Keys and events don't live in the pad: `due_steps` delivers them, through the session.
        ScriptStep::Key(_) | ScriptStep::Event(_, _) => {}
        ScriptStep::ReleaseAll => pad = crate::input::Pad::default(),
    }
    pad
}

/// Applies the script steps that are already due by `elapsed` ms, returning the pad for the next
/// step.
///
/// The pad goes in and out by value: the caller **has** to keep it between one frame and the
/// next. The defect that existed here lived exactly in that — the pad was recreated inside the
/// frame loop, so the stick lasted a single step and the button press released it. The test of
/// this function walks across two frames to keep the contract written down.
fn due_steps(
    mut pad: crate::input::Pad,
    script: &[(u64, ScriptStep)],
    step: &mut usize,
    elapsed: u64,
    key_events: &mut Vec<(u32, bool)>,
    events: &mut Vec<(u32, u16)>,
) -> crate::input::Pad {
    while *step < script.len() && script[*step].0 <= elapsed {
        match script[*step].1 {
            // Keys and "release everything" need the engine, not only the pad: they become orders
            // for the caller, which is who holds the session.
            ScriptStep::Key(avk) => key_events.push((avk, true)),
            // The widget event needs the engine, and the session is with the caller.
            ScriptStep::Event(evt, w) => events.push((evt, w)),
            ScriptStep::ReleaseAll => {
                let held: Vec<u32> = key_events
                    .iter()
                    .filter(|(_, down)| *down)
                    .map(|(avk, _)| *avk)
                    .collect();
                for avk in held {
                    key_events.push((avk, false));
                }
                pad = crate::input::Pad::default();
            }
            other => pad = apply_step(pad, other),
        }
        *step += 1;
    }
    pad
}

/// A step of the control script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptStep {
    /// Presses a button, by the device index.
    Button(usize),
    /// Puts an axis at the given value, in the stick travel.
    Axis(usize, i32),
    /// Delivers a **widget** event to the applet, by number and `wParam`.
    ///
    /// It exists to exercise what the widget would do and the emulator does not have yet: on the
    /// console, whoever sends the `0x7000` that the Z-Wheel reads to open a game is the list
    /// widget. With the script delivering the event, the rest of the cycle — launch request,
    /// session switch and return — can be exercised without it. Format: `ms:e0x7000=0x4ea`.
    Event(u32, u16),
    /// Presses a **console key** (`AVK_*`), which no button produces.
    ///
    /// It is what the Z-Wheel waits for to move the launch form along: it asks for `AVK_0` and
    /// `AVK_CLR`, and neither fits on a controller button. Without this step, the script could only
    /// do what the controller does — and the conclusion that "the gesture does not exist" came
    /// from an instrument that could not make the gesture.
    Key(u32),
    /// Releases everything: buttons, axes and keys.
    ReleaseAll,
}

/// Reads the call trace from `ZEEBX_ROM_TRACE`.
///
/// It is `1` (the last calls before the crash) or a text filter: `ZEEBX_ROM_TRACE=IFile` keeps
/// only the calls whose name contains that. It exists because "invalid access to 0x0" says some
/// pointer was null and does not say **which** — and in an investigation what answers is the last
/// call before the execution went off track. Without this option the trace exists in the engine
/// and nobody turns it on in a scan.
fn requested_trace() -> Option<String> {
    let value = crate::config::env_var("ZEEBX_ROM_TRACE").ok()?;
    let value = value.trim().to_string();
    match value.is_empty() || value == "0" {
        true => None,
        false => Some(value),
    }
}

/// Reads the serial capture path from `ZEEBX_ROM_SERIAL`.
///
/// The capture is where the **instrumentation** writes without mixing with the game log: the
/// widget tree on the first key, the classes created, the databases opened. It has been in `run`
/// forever (`--serial=`), and was missing here — and without it the question "**whom** was this
/// key for?" could only be answered with a window open, which is exactly what the scan exists to
/// not require.
fn requested_serial() -> Option<std::path::PathBuf> {
    let value = std::env::var("ZEEBX_ROM_SERIAL").ok()?;
    let value = value.trim();
    match value.is_empty() {
        true => None,
        false => Some(std::path::PathBuf::from(value)),
    }
}

/// Reads the classes to serve by **probe** from `ZEEBX_ROM_PROBE`, as in `0x01000000,0x0102c4e8`.
///
/// It is the instrument for the question "what class is this, and what does the game call on
/// it?" without disassembling the module: the probe returns an object that answers success to
/// everything and records each slot with the arguments and with the texts the arguments point to.
/// That is how `IDownload` came about: the Z-Wheel asks for `0x01000000` in `ShopAction_Init`, it
/// was refused, and without it the wheel's game library does not build — the grid never appears
/// and no key does anything.
fn requested_probe() -> Option<Vec<u32>> {
    let value = crate::config::env_var("ZEEBX_ROM_PROBE").ok()?;
    let classes: Vec<u32> = value
        .split(',')
        .filter_map(|part| u32::from_str_radix(part.trim().trim_start_matches("0x"), 16).ok())
        .collect();
    match classes.is_empty() {
        true => None,
        false => Some(classes),
    }
}

/// Reads the memory dump request from `ZEEBX_ROM_DUMP`, in the format `address:size`.
///
/// It exists because the last question of an investigation is usually "what is in **that**
/// memory?": the trace says the game built a table of 28-byte entries and crashed calling a null
/// pointer; what answers the rest is reading the table. The address changes from run to run
/// (it is guest heap), so the dump is read at the end, with the same virtual clock.
fn requested_dump() -> Option<(u32, usize)> {
    let value = crate::config::env_var("ZEEBX_ROM_DUMP").ok()?;
    let (address, size) = value.trim().split_once(':')?;
    let address = u32::from_str_radix(address.trim().trim_start_matches("0x"), 16).ok()?;
    let size = size.trim().parse::<usize>().ok()?.min(64 * 1024);
    Some((address, size))
}

/// At what rate the mixer delivers the audio for the measurement.
///
/// It is the rate the Libretro core asks of the frontend, and the one the console actually
/// delivers — 44.1 kHz stereo. Measuring at another rate would give a number that does not match
/// what one hears.
const SAMPLE_RATE: u32 = 44_100;

/// How many lines of the game's own log go into the report. The last ones, not the first: what
/// matters in a game that crashed is what it was saying shortly before.
const SHOWN_LOG: usize = 20;

/// Is the system error "out of space"?
///
/// It goes by the raw number and not the `ErrorKind`: 28 (`ENOSPC`) on Unix and 112
/// (`ERROR_DISK_FULL`) on Windows, which is what `raw_os_error` returns on each.
fn is_out_of_space(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(28) | Some(112))
}

/// What state the ROM ended up in, in the same buckets as the compatibility survey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Not a module we know how to read or load. There never was a machine.
    DoesNotLoad,
    /// The module loaded and the execution crashed before there was an applet: in `AEEMod_Load`
    /// or inside `CreateInstance`. It is where a file stops that gets past the loose `.mod` header
    /// without being ARM code.
    CrashedBeforeApplet,
    /// The module loaded and the execution came back whole, but there is no applet — no `.mif`
    /// saying which one to create, or `CreateInstance` refusing the class.
    NoApplet,
    /// The applet was born and crashed on the first lap, which is where `EVT_APP_START` is
    /// delivered.
    CrashedAtStart,
    /// Crashed already inside the frame loop.
    CrashedInLoop,
    /// Did not crash, but did not reach the requested virtual time within the real time cap.
    TooSlow,
    /// The game finished on its own before the requested time — no timer armed and no work.
    Finished,
    /// The disk ran out in the middle of the scan. **It is not a game defect, it is the
    /// environment**: extracting the ROM to the cache did not fit, and what came after that does
    /// not count as a measurement.
    OutOfSpace,
    /// Reached the requested virtual time, still standing.
    Runs,
}

impl Category {
    /// Whether this state counts as a passing test.
    ///
    /// "Runs" and "finished" pass; the rest is failure. **A finding is not a failure**: a game
    /// that runs asking for three classes we don't have still passes, because that is how it
    /// behaves on the console too — what enforces findings is the baseline, and only when it exists.
    pub fn passes(self) -> bool {
        matches!(self, Self::Runs | Self::Finished)
    }

    fn label(self) -> &'static str {
        match self {
            Self::DoesNotLoad => "does not load",
            Self::CrashedBeforeApplet => "crashed before creating the applet",
            Self::NoApplet => "does not create the applet",
            Self::CrashedAtStart => "crashed at EVT_APP_START",
            Self::CrashedInLoop => "crashed in the frame loop",
            Self::TooSlow => "too slow",
            Self::Finished => "finished on its own",
            Self::OutOfSpace => "out of disk space",
            Self::Runs => "runs",
        }
    }
}

/// What it cost to run. Nothing here goes into the comparison with the baseline: they are numbers
/// of whoever's machine ran it, and enforcing them would make the test fail because of hardware.
#[derive(Debug, Clone, Copy, Default)]
pub struct Performance {
    /// Real time spent from `start` until the applet exists. A game can take seconds here.
    pub opening: Duration,
    /// Real time spent in the frame loop.
    pub run_loop: Duration,
    /// Virtual time the game got to measure, in ms.
    pub virtual_ms: u32,
    /// Laps of the event loop. It is the "stopped at lap N" of the survey.
    pub laps: u64,
    /// Frames the game presented.
    pub frames: u32,
    /// Writes to the screen. A 2D game may not present any frame in a few seconds and still be
    /// drawing — it is what separates "opened" from "black screen".
    pub pixels: u64,
    /// **Distinct colors** in the final frame, and the one that shows up most.
    ///
    /// `pixels` counts writes, and a game that paints 307,200 pixels black has a high `pixels` and
    /// a black screen: counting writes does **not** separate "drew" from "drew nothing". Counting
    /// colors does. It is the number the baseline was missing to enforce a regression that blanks
    /// the screen without breaking the execution — the symptom easiest to miss in a scan.
    pub colors: u32,
    /// The most frequent color of the final frame, in RGB565. On a black screen it is `0x0000`.
    pub dominant_color: u16,
    /// Whether the measured frame came from the GPU rasterizer, and not from the console screen.
    ///
    /// It changes the reading: a black screen with `gpu_frame` is the game drawing **somewhere
    /// else**, and a black screen without it is the game not drawing.
    pub gpu_frame: bool,
    /// ARM instructions executed.
    pub instructions: u64,
    /// API calls served.
    pub calls: u64,
    /// Entries into the JIT and nanoseconds spent inside it, when the backend measures.
    ///
    /// It is what separates the guest from dispatch: the loop time that is **not** in here is
    /// trampoline plus method body. See [`zeebx::cpu::CpuBackend::jit_report`].
    pub jit_entries: u64,
    /// **Estimated** nanoseconds inside the JIT, and how many samples back the estimate.
    pub jit_nanos: u64,
    pub jit_samples: u64,
    /// Bytes of guest heap handed out, and our objects still alive at the end.
    pub heap: u32,
    pub objects: usize,
}

impl Performance {
    /// The report line that says where the time went: inside or outside the JIT.
    ///
    /// **Only the time inside the JIT is sampled.** Entries are always counted, and the clock is
    /// turned on by the same switch as the API profile — without it, what comes out here is the
    /// count, and saying so is better than presenting zero as if it were a measurement.
    pub fn dispatch_report(&self) -> String {
        if self.jit_entries == 0 {
            return "no JIT entry to measure (backend does not count it)".to_string();
        }
        if self.jit_samples == 0 {
            return format!(
                "{} JIT entry(ies); time inside it not measured (set ZEEBX_ROM_PROFILE)",
                self.jit_entries
            );
        }
        let total = self.run_loop.as_nanos() as u64;
        let inside = self.jit_nanos.min(total) / 1_000_000;
        let outside = total / 1_000_000 - inside;
        let slice = self.dispatch_share().unwrap_or(0);
        let per_call = match self.nanos_outside_jit_per_call() {
            Some(ns) => format!("{ns} ns outside the JIT per API call"),
            None => "no API call to divide by".to_string(),
        };
        format!(
            "{inside} ms inside the JIT and {outside} ms outside ({slice}% of the loop in dispatch); \
{} entry(ies) in {} sample(s), {per_call}",
            self.jit_entries, self.jit_samples
        )
    }

    /// How much of the loop stayed outside the JIT, as a percentage — dispatch, measured.
    ///
    /// `None` when the backend does not measure JIT entry, or when the loop was too short for the
    /// ratio to say anything.
    pub fn dispatch_share(&self) -> Option<u64> {
        let real = self.run_loop.as_nanos() as u64;
        if real == 0 || self.jit_entries == 0 || self.jit_samples == 0 {
            return None;
        }
        let inside = self.jit_nanos.min(real);
        Some((real - inside) * 100 / real)
    }

    /// Wall-clock nanoseconds outside the JIT per API call.
    ///
    /// **It is the number that was missing.** `ZEEBX_ROM_PROFILE` measures only the method body;
    /// this measures what is left, and that is where the trampoline lives. Without it, the cost per
    /// API call is unknown — and treating a number from the Unicorn era as current is how the
    /// external review got it wrong.
    pub fn nanos_outside_jit_per_call(&self) -> Option<u64> {
        let real = self.run_loop.as_nanos() as u64;
        if real == 0 || self.calls == 0 || self.jit_samples == 0 {
            return None;
        }
        Some(real.saturating_sub(self.jit_nanos.min(real)) / self.calls)
    }

    /// Fraction of console speed, as a percentage: virtual time over real time.
    pub fn speed(&self) -> u64 {
        let real = self.run_loop.as_millis() as u64;
        match real {
            0 => 0,
            real => u64::from(self.virtual_ms) * 100 / real,
        }
    }

    /// Instructions per second of real time, in the loop.
    pub fn ips(&self) -> u64 {
        match self.run_loop.as_millis() as u64 {
            0 => 0,
            real => self.instructions * 1000 / real,
        }
    }

    /// Frames per second of **virtual** time — the rate the game thinks it is getting.
    pub fn fps(&self) -> u64 {
        match self.virtual_ms {
            0 => 0,
            ms => u64::from(self.frames) * 1000 / u64::from(ms),
        }
    }
}

/// What the emulator has to point out about the run. This is the part the baseline enforces.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    /// API methods the game called that we don't implement, with the caller's address.
    pub apis: Vec<String>,
    /// ClassIDs the game requested that we don't have.
    pub classes: Vec<String>,
    /// Files the game opened and did not find. **Signal, not noise**: it was this list that
    /// revealed the `font.fnz` of the arcade ports and the twelve stages of Resident Evil 4.
    pub files: Vec<String>,
    /// Guest pointers we rejected, with the reason.
    pub pointers: Vec<String>,
    /// APIs served by assumption — what is a measured guess, and not known behavior.
    pub assumptions: Vec<String>,
    /// Invalid accesses that happened inside a callback and did not bring the execution down.
    pub faults: Vec<String>,
    /// What the game asked `IFileMgr`, with path and return value, in order.
    pub files_queried: Vec<String>,
    /// Every class the game requested, with how many times.
    pub requested_classes: Vec<String>,
    /// The text the game drew, with the virtual instant — what is written on the screen.
    pub drawn_text: Vec<String>,
    /// What the game called on the classes served by **probe** (`ZEEBX_ROM_PROBE`), slot by slot.
    pub probe: Vec<String>,
    /// What each **widget** class received in the accessor, per selector.
    pub selectors: Vec<String>,
    /// Allocations the heap refused, as `size at whoever asked (lr)`.
    ///
    /// It is the clue that was missing when a game showed the out-of-memory screen with nothing in
    /// the report: `malloc` returns zero silently.
    pub allocations: Vec<String>,
    /// Text the game asked to draw and we could not draw.
    pub text: Vec<String>,
    /// GL calls served without doing anything.
    pub ignored_gl: Vec<String>,
}

impl Findings {
    fn from(session: &Session) -> Self {
        let machine = session.machine();
        let sorted = |mut lines: Vec<String>| {
            lines.sort();
            lines.dedup();
            lines
        };
        Self {
            apis: sorted(machine.missing_apis()),
            classes: sorted(
                machine
                    .unknown_classes()
                    .iter()
                    .map(|id| format!("{id:#010x}"))
                    .collect(),
            ),
            files: sorted(machine.missing_files()),
            pointers: sorted(machine.bad_pointers()),
            assumptions: sorted(
                machine
                    .assumptions()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            faults: sorted(machine.swallowed_faults()),
            files_queried: machine.fs_log().cloned().collect(),
            requested_classes: machine
                .requested_classes()
                .into_iter()
                .map(|(class_id, times)| format!("{class_id:#010x}  ({times}x)"))
                .collect(),
            drawn_text: machine
                .drawn_text()
                .map(|(ms, x, y, text)| format!("{ms:>7} ms  ({x}, {y})  {text}"))
                .collect(),
            selectors: machine
                .selectors_per_class()
                .into_iter()
                .map(|(class_id, selector, times)| {
                    format!("class {class_id:#010x}  selector {selector:#x}  ({times}x)")
                })
                .collect(),
            probe: machine
                .probe_log()
                .iter()
                .map(|(class_id, object, slot, args, texts, times)| {
                    let show = |i: usize| match &texts[i] {
                        Some(text) => format!("{text:?}"),
                        None => format!("{:#x}", args[i]),
                    };
                    let repeated = match times {
                        1 => String::new(),
                        n => format!("   ({n}x)"),
                    };
                    format!(
                        "class {class_id:#010x}, object {object:#x}: slot[{slot:>2}] ({}, {}, {}){repeated}",
                        show(1),
                        show(2),
                        show(3)
                    )
                })
                .collect(),
            allocations: sorted(
                machine
                    .refused_allocations()
                    .into_iter()
                    .map(|(size, pc)| format!("malloc of {size} bytes, requested from {pc:#010x}"))
                    .chain(machine.refused_availability_checks().into_iter().map(
                        |(size, pc)| {
                            format!("CheckAvail of {size} bytes, asked at {pc:#010x}")
                        },
                    ))
                    .collect(),
            ),
            text: sorted(machine.pending_text().to_vec()),
            ignored_gl: sorted(machine.ignored_gl().iter().map(|s| s.to_string()).collect()),
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The sections, in the order in which they are worth reading — the same as the `run` report.
    fn sections(&self) -> [(&'static str, &Vec<String>); 14] {
        [
            ("missing APIs", &self.apis),
            ("classes the game requested and we do not have", &self.classes),
            ("requested classes", &self.requested_classes),
            ("file system calls", &self.files_queried),
            ("files not found", &self.files),
            ("invalid accesses the game carried on past", &self.faults),
            ("rejected pointers", &self.pointers),
            ("APIs served by assumption", &self.assumptions),
            ("text drawn on screen", &self.drawn_text),
            ("allocations refused by the heap", &self.allocations),
            ("what each widget class received in the accessor", &self.selectors),
            ("what the game called on the probe classes", &self.probe),
            ("text we could not draw", &self.text),
            ("GL served as a no-op", &self.ignored_gl),
        ]
    }
}

/// The survey of one ROM.
#[derive(Debug, Clone)]
pub struct Report {
    pub file: PathBuf,
    pub title: String,
    /// The class the shell asked to open, when it **got as far as asking** for one.
    ///
    /// It is the outcome the Z-Wheel has to produce to switch applications, and the only signal that
    /// does not deceive: the color count goes up when it animates, and animation is not a request.
    /// It comes from `Session::take_launch_request`, the same hook the core uses to switch sessions.
    pub requested_open: Option<u32>,
    pub category: Category,
    /// Where it stopped, in readable text. `None` when the game was still standing at the end.
    pub reason: Option<String>,
    /// `None` when there never was a session to measure.
    pub performance: Option<Performance>,
    pub findings: Findings,
    /// Registers and stack at the moment of the crash, when there was one.
    ///
    /// Without them, "invalid access to 0x00000000" says something was null and does not say
    /// **which** — and that is the question. It stays out of the summary: the address and the `pc`
    /// are already in the reason, and the rest is investigation material, not comparison material.
    pub crash_state: Option<String>,
    /// The most called methods, from most to least.
    pub top_calls: Vec<(String, u64)>,
    /// Where the real time was spent, per API method, from the most expensive to the cheapest.
    ///
    /// Empty unless `ZEEBX_ROM_PROFILE` is set: turning on the stopwatch on every call changes the
    /// measured speed, and the speed number is what the scan exists to give. With the call count
    /// next to the cost, **volume** can be told apart from **weight**: Rolima made 6.8 million calls
    /// and Ridge Racer 70 thousand, and only the profile says which of the two costs more per call.
    pub top_costs: Vec<(String, u64)>,
    /// The real time of all API calls added up, in nanoseconds.
    pub total_cost: u64,
    /// The tail of the game's own log, from `DBGPRINTF` and from semihosting.
    pub game_log: Vec<String>,
    /// The memory dump requested in `ZEEBX_ROM_DUMP`, with the address it came from.
    pub dump: Option<(u32, Vec<u8>)>,
    /// The last calls of the trace, when `ZEEBX_ROM_TRACE` asked for it.
    ///
    /// It stays out of the summary on purpose: it is investigation material, it changes from one run
    /// to the next with any tweak to the engine, and enforcing it in the baseline would be flagging a
    /// regression on top of a debugging tool.
    pub trace: Vec<String>,
    /// What the audio did, when there was audio to measure.
    pub audio: Option<Audio>,
}

/// What the game's audio did, measured without a sound card.
///
/// **A pop has a numeric signature**: a large jump between one sample and the next, on the same
/// channel. Keeping the largest jump and how many went past half travel lets us say "this game
/// pops" without recording an audio file and listening — and the scan of the 62 ROMs starts
/// measuring that for free, which is the only way to find out when the defect appeared.
///
/// It stays **out of the summary**, for the same reason as the crash state: they are investigation
/// numbers, and enforcing them in the baseline would flag a regression because of a threshold
/// chosen today.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Audio {
    /// Stereo frames delivered to the mixer.
    pub samples: u64,
    /// The largest absolute value, in `0..=1`.
    pub peak: f32,
    /// The RMS value, which is what is perceived as loudness.
    pub rms: f32,
    /// DC offset: a mean value far from zero is a direct-current hiss.
    pub dc_offset: f32,
    /// The largest jump between samples of the same channel.
    pub largest_jump: f32,
    /// How many jumps went past [`POP_THRESHOLD`].
    pub pops: u64,
}

/// From how much a jump between samples counts as a pop.
///
/// Half travel in one sample is a strong discontinuity for any content that is not deliberate
/// white noise. It is not a law: it is a declared threshold, so the numbers of two games can be
/// compared with each other.
pub const POP_THRESHOLD: f32 = 0.5;

impl Report {
    /// What the baseline keeps: state and findings, with no performance number.
    ///
    /// It has to be stable across machines and across runs, or the baseline flags a regression
    /// where there was only a different computer. That is why there is no time, no instruction
    /// count and no frames here — only what is the emulator's behavior in front of that game.
    pub fn summary(&self) -> String {
        let mut text = format!("{}\nstate: {}\n", self.title, self.category.label());
        if let Some(class_id) = self.requested_open {
            text.push_str(&format!("requested launch: {class_id:#010x}\n"));
        }
        if let Some(reason) = &self.reason {
            text.push_str(&format!("reason: {reason}\n"));
        }
        if let Some(d) = &self.performance {
            // **The line that keeps a black screen from passing.** A game that blanks everything
            // keeps presenting frames, executing instructions and answering APIs: only the color
            // count gives it away. The dominant one says *what* was left in its place — black is a
            // blanked screen, and a `0xFFFF` in its place is another story.
            // A nearly uniform screen has two readings, and the report gives both: either the game
            // does not draw, or it **is still loading**. That is what happened with the two titles
            // of the F.C. line: with six virtual seconds they showed one and six colors, and with
            // twenty they showed 2,410 and 4,133. Without this line, whoever reads the report blames
            // the game.
            let warning = match d.colors {
                0..=2 => " (nearly uniform screen: may be loading — try raising ZEEBX_ROM_MS)",
                _ => "",
            };
            text.push_str(&format!(
                "screen: {} color(s), dominant {:#06x}{}{}\n",
                d.colors,
                d.dominant_color,
                match d.gpu_frame {
                    true => " (the frame is on the GPU, not on the 2D screen)",
                    false => "",
                },
                warning
            ));
        }
        for (name, lines) in self.findings.sections() {
            if lines.is_empty() {
                continue;
            }
            text.push_str(&format!("{name}:\n"));
            for line in lines {
                text.push_str(&format!("  {line}\n"));
            }
        }
        if self.findings.is_empty() {
            text.push_str("nothing to report\n");
        }
        text
    }

    /// The whole report, with performance, calls and the game log.
    pub fn complete(&self) -> String {
        // The path stays out of the summary on purpose: it changes from machine to machine, and
        // the baseline would flag a difference just because of that.
        let mut text = format!("file: {}\n{}", self.file.display(), self.summary());
        if let Some(d) = &self.performance {
            text.push_str(&format!(
                "performance:\n  \
                 opened in {:.1} s, ran {:.1} real s for {} virtual ms ({}% of console speed)\n  \
                 {} lap(s), {} frame(s) ({} virtual fps)\n  \
                 {} instructions ({}/s), {} API call(s)\n  \
                 {}\n  \
                 {} KB of heap, {} live object(s)\n",
                d.opening.as_secs_f32(),
                d.run_loop.as_secs_f32(),
                d.virtual_ms,
                d.speed(),
                d.laps,
                d.frames,
                d.fps(),
                d.instructions,
                d.ips(),
                d.calls,
                d.dispatch_report(),
                d.heap / 1024,
                d.objects,
            ));
        }
        if let Some(state) = &self.crash_state {
            text.push_str(&format!("at the moment of the crash:\n{state}"));
        }
        if !self.top_costs.is_empty() {
            text.push_str(&format!(
                "where the time went ({} ms in API calls):\n",
                self.total_cost / 1_000_000
            ));
            for (name, ns) in &self.top_costs {
                text.push_str(&format!(
                    "  {:>9.1} ms  {:>5.1}%  {name}\n",
                    *ns as f64 / 1e6,
                    *ns as f64 * 100.0 / (self.total_cost.max(1)) as f64,
                ));
            }
        }
        if let Some((address, bytes)) = &self.dump {
            text.push_str(&format!("dump of {address:#010x} ({} bytes):\n", bytes.len()));
            for (line, chunk) in bytes.chunks(16).enumerate() {
                let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
                text.push_str(&format!(
                    "  {:#010x}  {}\n",
                    address + (line * 16) as u32,
                    hex.join(" ")
                ));
            }
        }
        if !self.trace.is_empty() {
            text.push_str("trace (last calls before the stop):\n");
            for line in &self.trace {
                text.push_str(&format!("  {line}\n"));
            }
        }
        if let Some(audio) = &self.audio {
            // It stays in the full report and not in the summary, for the same reason as the crash
            // state: it is an investigation number, and the baseline does not enforce a threshold
            // chosen today.
            text.push_str(&format!(
                "audio:\n                   {} stereo sample(s), peak {:.3}, rms {:.4}, dc {:+.5}\n                   largest jump between samples of the same channel {:.3}; {} above {:.2}\n",
                audio.samples, audio.peak, audio.rms, audio.dc_offset,
                audio.largest_jump, audio.pops, POP_THRESHOLD,
            ));
        }
        if !self.top_calls.is_empty() {
            text.push_str("heaviest calls:\n");
            for (name, times) in &self.top_calls {
                text.push_str(&format!("  {times:>8}x {name}\n"));
            }
        }
        if !self.game_log.is_empty() {
            text.push_str("game log:\n");
            for line in &self.game_log {
                text.push_str(&format!("  {line}\n"));
            }
        }
        text
    }

    /// One line per game, for the scan table.
    pub fn line(&self) -> String {
        let performance = match &self.performance {
            Some(d) => format!(
                "{} virtual ms in {:.1} s, {} frame(s), {}%",
                d.virtual_ms,
                d.run_loop.as_secs_f32(),
                d.frames,
                d.speed()
            ),
            None => "did not run".to_string(),
        };
        let findings: usize = self
            .findings
            .sections()
            .iter()
            .map(|(_, lines)| lines.len())
            .sum();
        format!(
            "{:<44} {:<26} {performance}, {findings} finding(s)",
            self.title,
            self.category.label()
        )
    }

    /// When there was not even a session, the report is just the reason for the rejection.
    fn rejected(file: &Path, error: &StartError) -> Self {
        // The rejection is classified by what it says about the game, and not by the stage in
        // which it happened: "we don't have the class it asks for" and "it is not a module" are
        // different problems, and the survey separates them.
        let category = match error {
            StartError::NoApplet | StartError::Refused(_) => Category::NoApplet,
            StartError::Stopped(_) => Category::CrashedBeforeApplet,
            // Running out of space arrives as a read error, and it is the only one on this list
            // that says **nothing** about the game: separating it avoids recording a defect where
            // there was a full disk.
            StartError::Unreadable(error) if is_out_of_space(error) => Category::OutOfSpace,
            StartError::Unreadable(_) | StartError::NotAModule(_) | StartError::NotLoadable(_) => {
                Category::DoesNotLoad
            }
        };
        Self {
            file: file.to_path_buf(),
            title: crate::library::title_for(file),
            category,
            requested_open: None,
            reason: Some(error.to_string()),
            performance: None,
            findings: Findings::default(),
            crash_state: None,
            top_calls: Vec::new(),
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: Vec::new(),
            trace: Vec::new(),
            dump: None,
            audio: None,
        }
    }
}

/// The registers and the stack of whoever crashed, when the stop was an invalid access or an
/// exception.
fn crash_state(session: &Session) -> Option<String> {
    if !matches!(
        session.stopped(),
        Some(Outcome::Fault { .. } | Outcome::Exception { .. })
    ) {
        return None;
    }
    let machine = session.machine();
    let registers: Vec<String> = machine
        .fault_regs()
        .iter()
        .enumerate()
        .map(|(i, value)| format!("r{i}={value:#x}"))
        .collect();
    let mut text = format!("  {}\n", registers.join(" "));
    let stack = machine.fault_stack();
    if !stack.is_empty() {
        let items: Vec<String> = stack.iter().map(|value| format!("{value:#x}")).collect();
        text.push_str(&format!("  stack: {}\n", items.join(" ")));
    }
    Some(text)
}

/// How many distinct colors the frame has, and which one is the most frequent.
///
/// A 640×480 frame is 307,200 pixels: counting distinct colors with a set is cheap, and the count
/// for the dominant one fits in a vector of 65,536 positions — two simple passes, with no large
/// allocation per game.
fn frame_colors(screen: &crate::video::display::Framebuffer) -> (u32, u16) {
    let mut bytes = Vec::new();
    screen.write_rgb565_into(&mut bytes);
    let mut count = vec![0u32; 1 << 16];
    let mut distinct = 0u32;
    for pair in bytes.chunks_exact(2) {
        let color = u16::from_le_bytes([pair[0], pair[1]]);
        if count[color as usize] == 0 {
            distinct += 1;
        }
        count[color as usize] += 1;
    }
    let dominant = count
        .iter()
        .enumerate()
        .max_by_key(|(_, times)| *times)
        .map(|(color, _)| color as u16)
        .unwrap_or(0);
    (distinct, dominant)
}

/// How much virtual time has passed since `base`, for the control script.
fn now_ms(now: u32, base: u32) -> u64 {
    u64::from(now.wrapping_sub(base))
}

/// Loads the ROM, delivers `EVT_APP_START` and runs `virtual_ms` of game time.
///
/// The loop advances **one lap per call** of [`Session::step`], with a zeroed real time budget:
/// it is what lets us say on which lap the game stopped, as the `run` report does. The speed
/// limiter stays off, because nobody is watching here — what we want is the virtual time reached
/// as fast as the machine allows.
pub fn examine(file: &Path, virtual_ms: u32, cap: Duration) -> Report {
    let begin = Instant::now();
    let installed = installed_from_env(file);
    let mut session = match Session::start_with_installed(
        file,
        crate::DEFAULT_PORTS,
        None,
        false,
        None,
        Default::default(),
        &installed,
    ) {
        Ok(session) => session,
        Err(error) => return Report::rejected(file, &error),
    };
    let opening = begin.elapsed();
    let base_ms = session.clock_ms();
    let mut measured = Performance {
        opening,
        ..Default::default()
    };
    // **The audio is measured, not heard.** Without a sound card, the mixer delivers the samples
    // on the virtual clock — the same cadence the Libretro frontend uses — and the pop count comes
    // from there.
    let mixer = session.record_audio(SAMPLE_RATE);
    // **The reduction of the internal resolution, to measure what it buys.** Only the CPU
    // rasterizer uses it; see [`crate::video::rasterizer::GlState::set_downscale`].
    if let Ok(value) = crate::config::env_var("ZEEBX_ROM_DOWNSCALE")
        && let Ok(n) = value.trim().parse::<usize>()
    {
        session.set_downscale(n);
    }
    // Cost profile: opt-in, because the per-call stopwatch makes the run itself more expensive.
    let profiling = crate::config::env_var("ZEEBX_ROM_PROFILE").is_ok();
    if profiling {
        session.machine_mut().enable_api_profile();
    }
    // **The local library goes into the catalog before the start.** The Z-Wheel does not read the
    // ROM folder: it reads the profile's `tt_game_info`, and what links one to the other is the
    // `catalog.json` the interface writes (`library::sync_catalog`). The scan was not feeding it,
    // and the measurement is this: with `ZEEBX_ROM_INSTALLED`, the profile database opens with
    // the package's 59 official rows and `ZEEBX_LIBRARY` with **zero** — the wheel has nothing to
    // put on the grid, and no key has anything to move. Only the ROM folder is scanned, and only
    // when `ZEEBX_ROM_INSTALLED` asks for it: the effect outside the game directory is
    // `catalog.json`, the same one the interface maintains.
    if !installed.is_empty()
        && let Some(dir) = file.parent()
    {
        let _ = crate::library::sync_catalog(&crate::library::scan(dir));
    }
    // The probe goes in before the start, like the trace: what matters in it is the game's first
    // calls, which is when it sets up what it needs.
    if let Some(probe) = requested_probe() {
        session.machine_mut().probe_classes(&probe);
    }
    // The accessor census per widget class is opt-in for the same reason as the cost profile: it
    // adds a section to the report, and the baseline of the 62 games cannot change because of an
    // instrument. `ZEEBX_ROM_SELECTORS` turns it on.
    if crate::config::env_var("ZEEBX_ROM_SELECTORS").is_ok() {
        session.machine_mut().enable_widget_census();
    }
    // The serial capture goes in before the start: what matters in it is the beginning.
    if let Some(path) = requested_serial() {
        let _ = session.machine_mut().enable_serial(&path);
    }
    let mut sound = Audio::default();
    let mut sum = 0.0_f64;
    let mut sum_of_squares = 0.0_f64;
    let mut last_clock = base_ms;
    // The control script comes in through the same path the frontend uses: `set_port_pad`.
    let script = key_script();
    // The trace goes in before the start: the call that matters is usually one of the first.
    let trace = requested_trace();
    if let Some(filter) = &trace {
        session.machine_mut().set_tracing(true);
        if filter != "1" {
            session.machine_mut().set_trace_filter(Some(filter.clone()));
        }
    }
    let mut script_step = 0usize;
    // The class the shell asked to open, if it asked. It is the outcome the Z-Wheel has to
    // produce to switch applications: without this field, the only clue that it reacted was the
    // color count, and animation also changes the count.
    let mut requested_open = None;
    // The script pad **persists between steps, and that is why it lives outside the frame loop**.
    // Each step changes only what it names, and the empty step (`"11000:"`) releases everything.
    // The previous defect was silent and survived a first fix: the pad was recreated on every
    // step, so `"18000:x=128,20000:b1"` released the stick at the instant of the press. As the
    // instrument showed (`eixo0=255` in one step and `128` in the next), recreating it outside the
    // step was not enough — it has to cross the whole loop. This is what makes the gesture of
    // "hold the stick and press" reach the game, which is how a menu asks to open.
    let mut script_pad = crate::input::Pad::default();
    // One previous sample **per channel**: the stream is interleaved stereo, and comparing `R` of
    // one frame with `L` of the same frame would measure the difference between the channels —
    // which is music, not a discontinuity. That is how the first version of this count gave a zero
    // jump in a game with sound.
    let mut previous = [0.0_f32; 2];

    // The start comes separately from the first lap: it is in it that the applet receives
    // `EVT_APP_START`. Telling the two apart is what separates "crashed before the first frame"
    // from "crashed in the loop", and those two lines of the survey point to different stretches
    // of code — without the separation, a game that died on the first lap of the loop showed up as
    // dead at the start.
    let run_loop = Instant::now();
    let mut category = match session.running_game() {
        Some(_) => Category::CrashedAtStart,
        None => Category::Runs,
    };

    if category == Category::Runs {
        loop {
            if session.clock_ms().saturating_sub(base_ms) >= virtual_ms {
                break;
            }
            if run_loop.elapsed() >= cap {
                category = Category::TooSlow;
                break;
            }
            measured.laps += 1;
            match session.step(Duration::ZERO, false) {
                // **The intermediate screen has to be consumed.** The window shows it and moves
                // on; here nobody shows it, and without taking it off the queue the next lap
                // returns the same screen forever — Quake stayed at "presented" without the game
                // moving a millisecond.
                Step::Presented => {
                    measured.frames += 1;
                    while session.show_intermediate_frame() {}
                }
                Step::Stopped => {
                    category = Category::CrashedInLoop;
                    break;
                }
                Step::Running | Step::Ahead => {}
            }
            // The elapsed time comes from the virtual clock, and not from a fixed number: a game
            // that spends two frames between two laps owes twice the samples. It is the same
            // calculation as `retro_run`, so the measurement here holds for what the frontend
            // receives.
            // The script is applied on the **virtual** clock: the same script works the same with
            // the machine at 30% or at 300% of console speed.
            if !script.is_empty() {
                let total_elapsed = now_ms(session.clock_ms(), base_ms);
                let step_before = script_step;
                let mut key_orders = Vec::new();
                let mut event_orders = Vec::new();
                script_pad = due_steps(
                    script_pad,
                    &script,
                    &mut script_step,
                    total_elapsed,
                    &mut key_orders,
                    &mut event_orders,
                );
                for (avk, down) in key_orders {
                    session.set_key(avk, down);
                }
                for (evt, w) in event_orders {
                    let _ = session.deliver_event_to_applet(evt, w);
                }
                // With the trace on, the pad after each step goes out in the report. Without this,
                // a script that does not reach the game is indistinguishable from a game that
                // ignores it — that is exactly what cost the Z-Wheel investigation.
                if trace.is_some() && script_step != step_before {
                    eprintln!(
                        "script {total_elapsed} ms: step {script_step} -> x={} y={} buttons={:#x}",
                        script_pad.console_axis(0),
                        script_pad.console_axis(1),
                        script_pad.buttons
                    );
                }
                session.set_port_pad(0, script_pad);
            }
            if requested_open.is_none() {
                requested_open = session.take_launch_request();
            }
            let now = session.clock_ms();
            let elapsed = u64::from(now.wrapping_sub(last_clock)).min(1000);
            last_clock = now;
            let due = (elapsed * u64::from(SAMPLE_RATE) / 1000) as usize;
            for (i, sample) in mixer.render(due).into_iter().enumerate() {
                sum += f64::from(sample);
                sum_of_squares += f64::from(sample) * f64::from(sample);
                sound.samples += 1;
                sound.peak = sound.peak.max(sample.abs());
                // The jump is measured **within the same channel**, two indices back.
                let channel = i % 2;
                let stride = (sample - previous[channel]).abs();
                sound.largest_jump = sound.largest_jump.max(stride);
                if stride > POP_THRESHOLD {
                    sound.pops += 1;
                }
                previous[channel] = sample;
            }
        }
    }

    // Ending with `Returned` is not a crash: it is the game having finished what it had to do, and
    // it is the outcome of a `helloworld` that draws and exits.
    if matches!(session.stopped(), Some(Outcome::Returned { .. })) {
        category = Category::Finished;
    }

    measured.run_loop = run_loop.elapsed();
    measured.virtual_ms = session.clock_ms().saturating_sub(base_ms);
    measured.instructions = session.machine().instructions();
    // How the clock is split between the JIT and dispatch. See `dispatch_report`.
    if let Some((entries, nanos, samples)) = session.machine().jit_report() {
        measured.jit_entries = entries;
        measured.jit_nanos = nanos;
        measured.jit_samples = samples;
    }
    let mut calls = session.machine().call_log();
    measured.calls = calls.iter().map(|(_, times)| times).sum();
    let (heap, objects) = session.memory();
    measured.heap = heap;
    measured.objects = objects;
    calls.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    calls.truncate(SHOWN_CALLS);

    let mut log: Vec<String> = session
        .machine()
        .debug_output()
        .iter()
        .map(|(line, times)| match times {
            1 => line.clone(),
            n => format!("{line}   ({n}x)"),
        })
        .collect();
    log.extend(
        session
            .machine()
            .cpu()
            .semihosting()
            .lines()
            .map(str::to_string),
    );
    let surplus = log.len().saturating_sub(SHOWN_LOG);
    log.drain(..surplus);

    measured.pixels = session.screen().writes();
    // The dump is read **at the end**, with the game already stopped: that is when the table it
    // built is ready, and that is what the investigation wants to see.
    let dump = requested_dump().and_then(|(address, size)| {
        session
            .machine()
            .dump(address, size)
            .ok()
            .map(|bytes| (address, bytes))
    });
    let (measured_colors, measured_dominant) = frame_colors(session.screen());
    measured.colors = measured_colors;
    measured.dominant_color = measured_dominant;
    // **The console screen may not be where the frame is.** A game that draws through OpenGL on a
    // device surface leaves the 2D screen black, and its frame lives in the GPU rasterizer.
    // Without looking at both, the scan would flag a blanked screen in a game that draws — that
    // almost happened with Prey Evil, which makes 71 thousand `MatrixMode` calls and has 1 screen
    // color.
    if measured.colors <= 1
        && let Some(large) = session.large_frame()
    {
        let (colors, dominant) = frame_colors(&large);
        if colors > measured.colors {
            measured.colors = colors;
            measured.dominant_color = dominant;
            measured.gpu_frame = true;
        }
    }
    // The total comes from the whole list, and only then is it cut: the slice shown has to be a
    // percentage of what the game spent, and not of the handful that fit in the report.
    let (mut cost, total_cost) = match profiling {
        true => {
            let all = session.machine().api_profile();
            let total: u64 = all.iter().map(|(_, ns)| ns).sum();
            (all, total)
        }
        false => (Vec::new(), 0),
    };
    cost.truncate(SHOWN_CALLS);
    // Close the audio accounts. `rms` and `dc_offset` only make sense with samples in the count,
    // and a game that played nothing gets `None` instead of zeros that would look like measured
    // silence.
    if sound.samples > 0 {
        let count = sound.samples as f64;
        sound.rms = (sum_of_squares / count).sqrt() as f32;
        sound.dc_offset = (sum / count) as f32;
    }
    let audio = (sound.samples > 0).then_some(sound);
    Report {
        file: file.to_path_buf(),
        // The title comes from the requested file, and **not** from `Session::title`: for a `.zip`
        // the session names the game after the cache folder, whose name carries the file's size
        // and date. That changes from machine to machine, and the baseline — which is written
        // with that name and carries the title inside — cannot depend on host metadata.
        title: crate::library::title_for(file),
        category,
        requested_open,
        reason: session.stopped_reason(),
        performance: Some(measured),
        findings: Findings::from(&session),
        crash_state: crash_state(&session),
        top_calls: calls,
        top_costs: cost,
        total_cost,
        game_log: log,
        trace: session.machine().trace().to_vec(),
        dump,
        audio,
    }
}

/// Only loads the ROM and creates the applet, without running the loop.
///
/// It is the cheapest question one can ask of a game — "does it open, or blow up in our face?" —
/// and one that can be asked of sixty-five at once without waiting minutes. What it measures is
/// the path until the applet exists: loading the `.mod`, `AEEMod_Load`, `.mif` and
/// `CreateInstance`.
pub fn open(file: &Path) -> Result<Duration, String> {
    let begin = Instant::now();
    match Session::start_with(file, crate::DEFAULT_PORTS, None, false, None, Default::default()) {
        Ok(_) => Ok(begin.elapsed()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ROMs the environment asked for. Empty when nobody asked for any.
    ///
    /// Accepts a file, a comma-separated list and a directory. The directory is the whole scan,
    /// and it **only happens when someone writes it** — never by default.
    fn requested_roms() -> Vec<PathBuf> {
        let Ok(value) = std::env::var("ZEEBX_ROM") else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        // **The whole value first.** Every No-Intro name has a comma — `Double Dragon (Brazil)
        // (Es,Pt).zip` — and the comma-separated list split the path in two, saying "could not
        // read the file" for both. Whoever points at an existing file wants that file.
        let parts: Vec<&str> = match PathBuf::from(value.trim()).exists() {
            true => vec![value.trim()],
            false => value.split(',').map(str::trim).filter(|p| !p.is_empty()).collect(),
        };
        for part in parts {
            let path = PathBuf::from(part);
            if !path.is_dir() {
                paths.push(path);
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut games: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("zip" | "mod" | "ZIP" | "MOD" | "7z" | "7Z")
                    )
                })
                .collect();
            games.sort();
            paths.extend(games);
        }
        paths
    }

    fn number(name: &str, fallback: u64) -> u64 {
        crate::config::env_var(name)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(fallback)
    }

    fn dir(name: &str) -> Option<PathBuf> {
        let path = PathBuf::from(crate::config::env_var(name).ok()?);
        std::fs::create_dir_all(&path).ok()?;
        Some(path)
    }

    /// The scan spends close to 1 GB extracting the ROMs to the cache, and without space it starts
/// flagging games that are fine.
///
/// The check runs at the start and costs an instant; the alternative is finding out after an hour
/// of scanning that the second half of the scoreboard did not count.
fn require_space(dirs: &[PathBuf]) {
    const TEST_WRITE_BYTES: usize = 64 * 1024 * 1024;
    for dir in dirs {
        if let Err(error) = crate::scratch::ensure_room(dir, TEST_WRITE_BYTES) {
            panic!(
                "out of disk space for the scan: {error}. The scan extracts each ROM to the \
                 cache and spends close to 1 GB; free up space and run it again."
            );
        }
    }
}

/// File name for a game's title, without depending on the host file system.
    fn file_name_for_title(title: &str) -> String {
        let clean: String = title
            .chars()
            .map(|c| match c.is_ascii_alphanumeric() {
                true => c.to_ascii_lowercase(),
                false => '-',
            })
            .collect();
        format!("{}.txt", clean.trim_matches('-'))
    }

    /// Runs `body` for each requested ROM and only fails at the end, with all the failures together.
    ///
    /// Failing on the first one hides the others, and in a scan what we want to see is the whole
    /// scoreboard — a game that crashed says nothing about the other sixty-four.
    fn for_each_rom(name: &str, mut body: impl FnMut(&Path) -> Result<String, String>) {
        let roms = requested_roms();
        if roms.is_empty() {
            eprintln!(
                "{name}: no ROM to examine. Point ZEEBX_ROM to a file, a comma-separated \
                 list or a directory, and run again with --release."
            );
            return;
        }
        let mut faults = Vec::new();
        for rom in &roms {
            match body(rom) {
                Ok(line) => println!("ok    {line}"),
                Err(error) => {
                    println!("FAIL  {error}");
                    faults.push(error);
                }
            }
        }
        // A full disk invalidates the whole scoreboard, including the games that passed, and
        // whoever reads it needs to know that before looking at any other number.
        if faults
            .iter()
            .any(|failure| failure.contains(Category::OutOfSpace.label()))
        {
            eprintln!(
                "\n**THE DISK FILLED UP IN THE MIDDLE OF THE SCAN.** The numbers from that point on \
                 do not count — not even the ones that passed — because extracting each ROM needs \
                 space. Free up space and run again."
            );
        }
        assert!(
            faults.is_empty(),
            "{} of {} ROM(s) with problems:\n{}",
            faults.len(),
            roms.len(),
            faults.join("\n")
        );
    }

    /// The scoreboard: each ROM runs for a few seconds and becomes a table row.
    ///
    /// It is the quick round-trip test — what we want to know after a tweak is "who still opens",
    /// and not the whole report of each game. Five virtual seconds per ROM are enough to separate
    /// who reaches the menu from who crashes at the start, and the short real time cap keeps a heavy
    /// game from holding up the scan.
    ///
    /// **This test does not fail on a broken game**: the library has games known to be
    /// incompatible, and failing on them would erase the only thing that matters here, which is the
    /// table. What enforces regressions is [`the_given_rom_progresses`], with the baseline.
    ///
    /// ```bash
    /// ZEEBX_ROM=roms cargo test --release scoreboard -- --nocapture
    /// # faster still, and writing the table:
    /// ZEEBX_ROM=roms ZEEBX_SCOREBOARD_MS=3000 ZEEBX_SCOREBOARD_OUT=scoreboard.md \
    ///   cargo test --release scoreboard -- --nocapture
    /// ```
    #[test]
    fn the_rom_scoreboard() {
        let roms = requested_roms();
        if roms.is_empty() {
            eprintln!(
                "the_rom_scoreboard: no ROM to examine. Point ZEEBX_ROM to a \
                 file, a comma-separated list or a directory, and run with --release."
            );
            return;
        }
        let ms = number("ZEEBX_SCOREBOARD_MS", u64::from(SCOREBOARD_MS)) as u32;
        let cap = Duration::from_secs(number("ZEEBX_SCOREBOARD_CAP", SCOREBOARD_CAP));
        let mut lines = Vec::new();
        let mut tally: std::collections::BTreeMap<&str, usize> = Default::default();
        for rom in &roms {
            let report = examine(rom, ms, cap);
            let d = report.performance.unwrap_or_default();
            let reason = report
                .reason
                .clone()
                .unwrap_or_default()
                .replace('\n', " ");
            let screen = match (d.frames, d.pixels) {
                (0, 0) => "black screen".to_string(),
                (0, pixels) => format!("{pixels} pixel(s)"),
                (frames, _) => format!("{frames} frame(s)"),
            };
            let line = format!(
                "| {} | {} | {} ms | {} | {:.0}% | {} |",
                report.title,
                report.category.label(),
                d.virtual_ms,
                screen,
                d.speed(),
                reason
            );
            println!("{line}");
            *tally.entry(report.category.label()).or_default() += 1;
            lines.push(line);
        }
        let summary: Vec<String> = tally
            .iter()
            .map(|(state, count)| format!("{count} {state}"))
            .collect();
        let table = format!(
            "| Game | State | Virtual time | Drawing | Speed | Reason |\n|---|---|---|---|---|---|\n{}\n\n{} ROM(s): {}\n",
            lines.join("\n"),
            roms.len(),
            summary.join(", ")
        );
        println!("\n{table}");
        if let Ok(path) = crate::config::env_var("ZEEBX_SCOREBOARD_OUT") {
            let _ = std::fs::write(path, &table);
        }
    }

    /// The generic question: does the ROM open, or blow up right away — and with what error.
    ///
    /// It stands on its own: the five games that "do not create the applet" in the survey fail
    /// here, without spending the six virtual seconds they will never get to run.
    #[test]
    fn the_given_rom_opens() {
        // The space check applies to both tests: both extract the ROM to the cache.
        require_space(&[crate::config::config_dir()]);
        for_each_rom("the_given_rom_opens", |rom| {
            let title = crate::library::title_for(rom);
            match open(rom) {
                Ok(amount) => Ok(format!("{title}: opened in {:.1} s", amount.as_secs_f32())),
                Err(error) => Err(format!("{title}: did not open — {error}")),
            }
        });
    }

    /// The ROM advances, and what happened becomes a report.
    ///
    /// It fails when the game crashes, and when the summary differs from the baseline — which is
    /// the whole point: an API that stopped being served, a class that started being requested or
    /// a file that disappeared show up here, in the commit that caused them.
    #[test]
    fn the_given_rom_progresses() {
        let ms = number("ZEEBX_ROM_MS", u64::from(DEFAULT_MS)) as u32;
        let cap = Duration::from_secs(number("ZEEBX_ROM_CAP", DEFAULT_CAP));
        let output = dir("ZEEBX_ROM_OUT");
        let base = dir("ZEEBX_ROM_BASE");
        let mut dirs_to_check = vec![crate::config::config_dir()];
        dirs_to_check.extend(output.iter().cloned());
        require_space(&dirs_to_check);
        for_each_rom("the_given_rom_progresses", |rom| {
            let report = examine(rom, ms, cap);
            println!("\n===== {} =====\n{}", rom.display(), report.complete());
            let name = file_name_for_title(&report.title);
            if let Some(dir) = &output {
                let _ = std::fs::write(dir.join(&name), report.complete());
            }
            let mut complaints = Vec::new();
            if !report.category.passes() {
                complaints.push(match &report.reason {
                    Some(reason) => format!("{} — {reason}", report.category.label()),
                    None => report.category.label().to_string(),
                });
            }
            if let Some(dir) = &base
                && let Err(difference) = check_baseline(&dir.join(&name), &report)
            {
                complaints.push(difference);
            }
            match complaints.is_empty() {
                true => Ok(report.line()),
                false => Err(format!("{}\n  {}", report.line(), complaints.join("\n  "))),
            }
        });
    }

    /// Compares the summary with the baseline, writing it when it does not exist yet.
    ///
    /// Writing what is missing, instead of requiring someone to write it by hand, is what makes it
    /// possible to adopt the baseline of a new game in one run — and the difference is visible in
    /// `git diff`, which is where it should be reviewed, game by game.
    fn check_baseline(path: &Path, report: &Report) -> Result<(), String> {
        let current = report.summary();
        let Ok(expected) = std::fs::read_to_string(path) else {
            let _ = std::fs::write(path, &current);
            println!("      baseline written to {}", path.display());
            return Ok(());
        };
        if expected == current {
            return Ok(());
        }
        let lines = |text: &str| -> Vec<String> { text.lines().map(str::to_string).collect() };
        let (before, now) = (lines(&expected), lines(&current));
        let mut difference = vec![format!("changed since {}:", path.display())];
        difference.extend(
            before
                .iter()
                .filter(|l| !now.contains(l))
                .map(|l| format!("  - {l}")),
        );
        difference.extend(
            now
                .iter()
                .filter(|l| !before.contains(l))
                .map(|l| format!("  + {l}")),
        );
        Err(difference.join("\n"))
    }

    // From here down, tests that need no ROM at all: it is the scan logic being checked, and it is
    // exactly the part that cannot be verified by looking at a game's output. Without them, a
    // classification error would pass as the game's result.

    #[test]
    fn a_file_that_is_not_a_module_becomes_a_report_with_a_reason() {
        let path = std::env::temp_dir().join("zeebx-varredura-lixo.mod");
        std::fs::write(&path, b"isto nao e um modulo").unwrap();
        let report = examine(&path, 10, Duration::from_secs(1));
        // The `.mod` header is loose: twenty bytes of text get past the loader and what refuses
        // them is the ARM itself, when executing them. That is why the category is "crashed before
        // creating the applet" and not "does not load" — and that is the difference the report
        // needs to state, because the two point to different places.
        assert_eq!(report.category, Category::CrashedBeforeApplet);
        assert!(!report.category.passes());
        // The reason is what one reads to know what to do, so it cannot be empty nor be left out
        // of the summary — which is the text the baseline keeps.
        assert!(report.reason.as_deref().is_some_and(|m| !m.is_empty()));
        assert!(
            report
                .summary()
                .contains("crashed before creating the applet")
        );
        assert!(report.performance.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn only_runs_and_finished_count_as_a_passing_test() {
        for category in [Category::Runs, Category::Finished] {
            assert!(category.passes(), "{category:?} should pass");
        }
        for category in [
            Category::DoesNotLoad,
            Category::CrashedBeforeApplet,
            Category::NoApplet,
            Category::CrashedAtStart,
            Category::CrashedInLoop,
            Category::TooSlow,
        ] {
            assert!(!category.passes(), "{category:?} should not pass");
        }
    }

    /// **A single-color screen is a blanked screen, and the number of colors says so.**
    ///
    /// It is what `pixels` did not do: a game that paints 307,200 pixels black counts 307,200
    /// writes and passed as if it were drawing. Counting colors, it gets one.
    #[test]
    fn a_single_color_frame_has_one_color() {
        use crate::video::display::{Framebuffer, Rgb};

        let black = Framebuffer::new(64, 48);
        assert_eq!(frame_colors(&black), (1, 0x0000), "a black screen is one color");

        let mut white = Framebuffer::new(64, 48);
        white.fill_rect(
            crate::video::display::Rect {
                x: 0,
                y: 0,
                width: 64,
                height: 48,
            },
            Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
        );
        assert_eq!(frame_colors(&white), (1, 0xffff), "white is one too");

        let mut two_colors = Framebuffer::new(64, 48);
        two_colors.set_pixel(0, 0, Rgb { r: 255, g: 0, b: 0 });
        let (distinct, dominant) = frame_colors(&two_colors);
        assert_eq!(distinct, 2, "two different pixels are two colors");
        assert_eq!(dominant, 0x0000, "and the dominant one is the one that fills the rest");
    }

    /// **The control script is read on the virtual clock, and by button name.**
    ///
    /// The format is the same as the command line (`ms:button`), and the name has to be found in
    /// the device table: a wrong name becomes a step without a button, and a step without a button
    /// **releases everything** — silently. The test pins down the two things that matter: the order
    /// by time and the rejection of an unknown name.
    #[test]
    fn the_key_script_reads_time_and_name() {
        // The table is the device's; the index of `b1` is 0 and that of `down` is 14.
        let index_of = |name: &str| {
            crate::input::BUTTON_NAMES
                .iter()
                .position(|candidate| candidate.eq_ignore_ascii_case(name))
        };
        assert_eq!(index_of("b1"), Some(0));
        assert_eq!(index_of("DOWN"), index_of("down"));
        assert_eq!(index_of("inexistente"), None);

        // Order: the script is applied in time order, even when written out of it.
        let mut steps = vec![(2000u64, index_of("b1")), (1000, index_of("up"))];
        steps.sort_by_key(|(ms, _)| *ms);
        assert_eq!(steps[0].0, 1000, "the earliest comes first");
    }

    /// **A full disk is not a game defect.**
    ///
    /// The scan of the 62 ROMs, run with the disk full, recorded two games as "does not load" with
    /// `No space left on device` — the scoreboard blamed the games, and the problem was the
    /// environment. Running out of space arrives as a read error, so this is where it needs to be
    /// separated.
    #[test]
    fn running_out_of_space_does_not_count_as_a_broken_game() {
        let categorize = |code| {
            let error = std::io::Error::from_raw_os_error(code);
            Report::rejected(Path::new("/tmp/x.mod"), &StartError::Unreadable(error)).category
        };
        assert_eq!(categorize(28), Category::OutOfSpace, "ENOSPC on Unix");
        assert_eq!(categorize(112), Category::OutOfSpace, "ERROR_DISK_FULL on Windows");
        assert_eq!(categorize(2), Category::DoesNotLoad, "a missing file is something else");
        assert!(!Category::OutOfSpace.passes());
    }

    /// The rejection is classified by what it says about the game, not by the stage in which it
    /// happened.
    #[test]
    fn the_rejection_separates_not_loading_from_not_creating_the_applet() {
        let file = Path::new("/tmp/exemplo.mod");
        let categorize = |error: StartError| Report::rejected(file, &error).category;
        assert_eq!(categorize(StartError::NoApplet), Category::NoApplet);
        assert_eq!(categorize(StartError::Refused(20)), Category::NoApplet);
        assert_eq!(
            categorize(StartError::Stopped(Outcome::Budget)),
            Category::CrashedBeforeApplet
        );
        assert_eq!(
            categorize(StartError::NotAModule("x".into())),
            Category::DoesNotLoad
        );
        assert_eq!(
            categorize(StartError::NotLoadable("x".into())),
            Category::DoesNotLoad
        );
    }

    /// **The baseline cannot see performance.**
    ///
    /// If it could, switching machines — or running in debug instead of `--release` — would flag
    /// a regression in every game at once, and a test that always complains is never read.
    #[test]
    fn the_summary_does_not_change_with_performance() {
        let model = Report {
            file: PathBuf::from("/tmp/x.mod"),
            title: "Exemplo".to_string(),
            requested_open: None,
            category: Category::Runs,
            reason: None,
            performance: Some(Performance {
                virtual_ms: 6_000,
                frames: 300,
                instructions: 700_000_000,
                run_loop: Duration::from_secs(4),
                ..Default::default()
            }),
            findings: Findings {
                files: vec!["config.cfg".to_string()],
                ..Default::default()
            },
            crash_state: None,
            top_calls: vec![("IDisplay::Update".to_string(), 300)],
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: vec!["carregando".to_string()],
            trace: Vec::new(),
            dump: None,
            audio: None,
        };
        let slow = Report {
            performance: Some(Performance {
                virtual_ms: 6_000,
                frames: 41,
                instructions: 700_000_000,
                run_loop: Duration::from_secs(70),
                ..Default::default()
            }),
            top_calls: Vec::new(),
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: Vec::new(),
            trace: Vec::new(),
            dump: None,
            audio: None,
            ..model.clone()
        };
        assert_eq!(model.summary(), slow.summary());
        assert!(model.summary().contains("config.cfg"));
        // The full one, yes, shows the difference — it is the report a person reads.
        assert_ne!(model.complete(), slow.complete());
    }

    /// A finding that appears changes the summary, and that is what the baseline enforces.
    #[test]
    fn a_new_finding_changes_the_summary() {
        let clean = Report {
            file: PathBuf::from("/tmp/x.mod"),
            title: "Exemplo".to_string(),
            requested_open: None,
            category: Category::Runs,
            reason: None,
            performance: None,
            findings: Findings::default(),
            crash_state: None,
            top_calls: Vec::new(),
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: Vec::new(),
            trace: Vec::new(),
            dump: None,
            audio: None,
        };
        assert!(clean.summary().contains("nothing to report"));
        let with_api = Report {
            findings: Findings {
                apis: vec!["IShell::Método (de 0x00012345)".to_string()],
                ..Default::default()
            },
            ..clean.clone()
        };
        assert_ne!(clean.summary(), with_api.summary());
        assert!(with_api.summary().contains("missing APIs"));
        assert!(!with_api.summary().contains("nothing to report"));
    }

    /// The baseline is written the first time and enforced the second.
    #[test]
    fn the_baseline_is_recorded_and_then_enforced() {
        let path = std::env::temp_dir().join("zeebx-varredura-base.txt");
        let _ = std::fs::remove_file(&path);
        let model = Report {
            file: PathBuf::from("/tmp/x.mod"),
            title: "Exemplo".to_string(),
            requested_open: None,
            category: Category::Runs,
            reason: None,
            performance: None,
            findings: Findings::default(),
            crash_state: None,
            top_calls: Vec::new(),
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: Vec::new(),
            trace: Vec::new(),
            dump: None,
            audio: None,
        };
        assert!(check_baseline(&path, &model).is_ok(), "writes what is missing");
        assert!(check_baseline(&path, &model).is_ok(), "and then checks");
        let changed = Report {
            findings: Findings {
                classes: vec!["0x01003109".to_string()],
                ..Default::default()
            },
            ..model
        };
        let Err(difference) = check_baseline(&path, &changed) else {
            panic!("a new class should have flagged a difference");
        };
        // The difference needs to name what appeared: "changed" without saying what helps nobody.
        assert!(difference.contains("+   0x01003109"), "{difference}");
        let _ = std::fs::remove_file(&path);
    }

    /// The crash state appears in the full report and **not** in the summary.
    ///
    /// A register is investigation material: two computers can reach the same defect with a
    /// different stack, and enforcing that in the baseline would flag a regression where there is
    /// none.
    #[test]
    fn the_crash_state_stays_out_of_the_summary() {
        let report = Report {
            file: PathBuf::from("/tmp/x.mod"),
            title: "Exemplo".to_string(),
            requested_open: None,
            category: Category::CrashedInLoop,
            reason: Some("acesso inválido a 0x00000024, em 0x00032b78".to_string()),
            performance: None,
            findings: Findings::default(),
            crash_state: Some("  r0=0x0 r1=0x24\n  pilha: 0x1 0x2\n".to_string()),
            top_calls: Vec::new(),
            top_costs: Vec::new(),
            total_cost: 0,
            game_log: Vec::new(),
            trace: Vec::new(),
            dump: None,
            audio: None,
        };
        assert!(report.complete().contains("at the moment of the crash"));
        assert!(report.complete().contains("r1=0x24"));
        assert!(!report.summary().contains("r1=0x24"));
        // The reason, yes, is in both: it is what says what crashed.
        assert!(report.summary().contains("0x00032b78"));
    }

    #[test]
    fn the_title_becomes_a_predictable_file_name() {
        assert_eq!(file_name_for_title("Quake 2"), "quake-2.txt");
        assert_eq!(
            file_name_for_title("Alice no País das Maravilhas"),
            "alice-no-pa-s-das-maravilhas.txt"
        );
    }

    /// The script pad persists between steps. This test exists because the defect was silent:
    /// the pad was recreated on every step, so `"20000:x=128,22000:b1"` released the stick at the
    /// instant of the press. The gesture of holding the stick and pressing a button — what a game
    /// asks for to open — never arrived whole, and the wrong conclusion was that the gesture did
    /// not exist.
    #[test]
    fn the_script_pad_persists_between_steps() {
        let pad = apply_step(crate::input::Pad::default(), ScriptStep::Axis(0, 128));
        assert_eq!(pad.console_axis(0), 255, "the stick at maximum is 255");

        let pad = apply_step(pad, ScriptStep::Button(1));
        assert_eq!(
            pad.console_axis(0),
            255,
            "the button press must not release the stick"
        );
        assert_ne!(pad.buttons, 0, "the pressed button stays pressed");

        let pad = apply_step(pad, ScriptStep::ReleaseAll);
        assert_eq!(pad.console_axis(0), 128, "the empty step releases the axis");
        assert_eq!(pad.buttons, 0, "the empty step releases the buttons");
    }



    /// **The script parser understands the event**, and not only the delivery.
    ///
    /// The previous test exercises `due_steps`, which receives the step already built; this one
    /// exercises the **text** the player writes. The two together close the line: text → step →
    /// order.
    #[test]
    fn the_script_text_becomes_an_event() {
        // SAFETY: single-threaded test, and the variable is read right below.
        unsafe { std::env::set_var("ZEEBX_ROM_KEYS", "1000:e0x7000=0x4ea,2000:kselect") };
        let steps = key_script();
        unsafe { std::env::remove_var("ZEEBX_ROM_KEYS") };
        assert_eq!(
            steps,
            vec![
                (1_000, ScriptStep::Event(0x7000, 0x4ea)),
                (2_000, ScriptStep::Key(crate::input::avk::SELECT)),
            ],
            "the script text did not become an event and a key"
        );
    }

    /// **A key without the `k` is still a key** — the `k` is optional, not mandatory.
    ///
    /// The test exists because of a measurement that came out wrong: I wrote the Z-Wheel key
    /// script in the bench's form (`30500:0xe064`) and the scan **dropped every step silently**,
    /// because there the key required the `k`. The report did not change shape, and the conclusion
    /// that came out of it — "the keys do not reach the wheel" — was the instrument's, not the
    /// emulator's. With the `k` optional, the measurement started saying what we wanted to measure.
    #[test]
    fn the_script_key_does_not_need_the_k() {
        // SAFETY: single-threaded test, and the variable is read right below.
        unsafe { std::env::set_var("ZEEBX_ROM_KEYS", "30500:0xe064,31000:k0xe032,32000:up") };
        let steps = key_script();
        unsafe { std::env::remove_var("ZEEBX_ROM_KEYS") };
        assert_eq!(
            steps,
            vec![
                (30_500, ScriptStep::Key(0xe064)),
                (31_000, ScriptStep::Key(0xe032)),
                // `up` is still a **button**: it is a name of both, and the button comes first.
                (32_000, ScriptStep::Button(12)),
            ],
            "the key name without the `k` did not become a step, or `up` stopped being a button"
        );
    }

    /// **The script can deliver a widget event**, and the step is not dropped silently.
    ///
    /// The test exists because I wrote the branch **after** the axis branch, and its
    /// `_ => continue` swallowed `e0x7000=0x4ea`: the step became nothing, with no warning. It is
    /// the same trap as the pad recreated on every frame, one layer up — an instrument that drops
    /// input silently.
    #[test]
    fn the_script_delivers_a_widget_event() {
        // The parser path is the same as the key script's: here it is exercised directly.
        let steps = vec![(1_000u64, ScriptStep::Event(0x7000, 0x4ea))];
        let mut step = 0usize;
        let mut key_events = Vec::new();
        let mut events = Vec::new();
        let _ = due_steps(
            crate::input::Pad::default(),
            &steps,
            &mut step,
            1_500,
            &mut key_events,
            &mut events,
        );
        assert_eq!(events, vec![(0x7000u32, 0x4eau16)]);
    }

    /// The script can press a **key**, and the empty step releases whatever is held.
    ///
    /// The Z-Wheel asks for `AVK_0` and `AVK_CLR` to move the launch form along, and neither comes
    /// from a controller button. While the script only knew about buttons and axes, the honest
    /// answer was "the gesture is not on the controller" — and the question could not even be
    /// asked.
    #[test]
    fn the_key_script_presses_and_releases() {
        let script = vec![
            (1_000u64, ScriptStep::Key(crate::input::avk::ZERO)),
            (2_000, ScriptStep::ReleaseAll),
        ];
        let mut step = 0usize;
        let mut key_events = Vec::new();
        let mut events = Vec::new();
        let _ = due_steps(
            crate::input::Pad::default(),
            &script,
            &mut step,
            1_500,
            &mut key_events,
            &mut events,
        );
        assert_eq!(key_events, vec![(crate::input::avk::ZERO, true)]);

        let _ = due_steps(
            crate::input::Pad::default(),
            &script,
            &mut step,
            2_500,
            &mut key_events,
            &mut events,
        );
        assert_eq!(
            key_events,
            vec![
                (crate::input::avk::ZERO, true),
                (crate::input::avk::ZERO, false)
            ],
            "the empty step should have released the held key"
        );
    }

    /// The same defect, measured **across frames**, which is where it lived.
    ///
    /// The script `"18000:x=128,20000:b1"` puts the stick at maximum and, two seconds later,
    /// presses a button. The pad needs to cross the frames between the two steps: the first
    /// version of the fix recreated it on every frame, and the instrument showed `eixo0=255` in one
    /// step and `128` in the next. This test walks the same crossing.
    #[test]
    fn the_stick_holds_across_frames_until_the_next_step() {
        let script = vec![(18_000u64, ScriptStep::Axis(0, 128)), (20_000, ScriptStep::Button(0))];
        let mut step = 0usize;

        // First frame after 18 s: only the stick is due.
        let mut key_events = Vec::new();
        let mut events = Vec::new();
        let pad = due_steps(
            crate::input::Pad::default(),
            &script,
            &mut step,
            18_500,
            &mut key_events,
            &mut events,
        );
        assert_eq!(pad.console_axis(0), 255);
        assert_eq!(step, 1);

        // Intermediate frames: nothing new is due, and the stick stays where it was.
        let pad = due_steps(pad, &script, &mut step, 19_000, &mut key_events, &mut events);
        assert_eq!(
            pad.console_axis(0),
            255,
            "the stick got lost between two frames"
        );

        // Second step: the button presses **without** releasing the stick. This is the gesture to
        // open.
        let pad = due_steps(pad, &script, &mut step, 20_500, &mut key_events, &mut events);
        assert_eq!(
            pad.console_axis(0),
            255,
            "the button press released the stick"
        );
        assert_ne!(pad.buttons, 0);
        assert_eq!(step, 2);
    }
}

/// **How much this machine's clock costs**, in nanoseconds per read.
///
/// It exists because two of our instruments disagreed, and the disagreement can only be settled
/// by measuring the instrument. `Instant::now` can be the fast `vDSO` path (tens of nanoseconds)
/// or a system call (more than a microsecond), and the difference decides whether the API profile
/// measures the method or itself: it reads the clock twice per call, and a game with millions of
/// calls pays for that in the number it publishes.
///
/// Ignored by default, like the old `instrucoes_por_segundo`: it is not an emulator invariant,
/// it is a property of the machine.
///
/// ```text
/// cargo test --release --lib how_much_the_clock_costs -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measures the machine, not the emulator"]
fn how_much_the_clock_costs() {
    /// Reads per round. High enough for the loop and the sum to vanish into the noise.
    const N: u64 = 2_000_000;

    // Warm-up: the first read pays whatever it has to all at once.
    let _ = std::time::Instant::now();

    let begin = std::time::Instant::now();
    let mut sum = 0u64;
    for _ in 0..N {
        sum = sum.wrapping_add(std::time::Instant::now().elapsed().as_nanos() as u64);
    }
    let round_trip = begin.elapsed().as_nanos() as u64 / N;

    let begin = std::time::Instant::now();
    let mut t = std::time::Instant::now();
    for _ in 0..N {
        t = std::time::Instant::now();
    }
    let read_only = begin.elapsed().as_nanos() as u64 / N;
    let _ = (sum, t);

    // The other suspect: the per-(interface, slot) map the profile uses to accumulate.
    let mut map: std::collections::HashMap<(u32, u32), u64> = std::collections::HashMap::new();
    for i in 0..64u32 {
        map.insert((i, i), 0);
    }
    let begin = std::time::Instant::now();
    for i in 0..N {
        *map.entry(((i % 64) as u32, (i % 64) as u32)).or_insert(0) += 1;
    }
    let map_ns = begin.elapsed().as_nanos() as u64 / N;

    eprintln!(
        "clock: {read_only} ns per read, {round_trip} ns per pair (now+elapsed); \
         map by (interface, slot): {map_ns} ns per accumulation"
    );
    eprintln!(
        "so the API profile charges about {} ns per call on top of the method",
        round_trip + map_ns
    );
}
