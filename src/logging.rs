//! The core's message log, in five levels.
//!
//! **Why it exists.** The core had no channel at all: diagnostics went out through `eprintln!`
//! in the interface, and the game had its own separate log. When the same problem showed up in
//! the Libretro core, on Android or in the scan, there was no way to say "I want to see the
//! warning level of this subsystem" — each frontend printed whatever it could, in whatever format.
//!
//! Here the message is born in the core, with a **level** and a **target** (the subsystem), and
//! the frontend decides where it goes: `eprintln!` on the desktop, `retro_log` in Libretro,
//! `logcat` on Android, the scan's baseline in the tests.
//!
//! **The filter costs one atomic read.** Callers use the [`logging!`] macro, which tests the
//! level before building the text: a game that emits a thousand debug messages per frame does
//! not pay for them when the level is `Warning` — which is the default.
//!
//! **The collector is a ring, and the ring reports when it overflows.** Keeping everything a
//! game under debugging says would fill memory; dropping without saying so would hide exactly
//! the part that went missing. The drop count shows up in the report.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// How much fits in the ring before the log starts dropping the oldest entry.
///
/// Three hundred lines comfortably cover an applet's startup and a scene change. It is in the
/// same spirit as the game log's cap ([`crate::machine`]), and for the same reason: the report
/// is meant to be read, not stored.
/// How many lines the ring keeps before overwriting the oldest ones.
///
/// **The number belongs to the instrument, not to the game.** With 300, a 40 s run in which the
/// game talks a lot at boot loses exactly the audio lines before anyone reads them: `run` drains
/// the ring at the end, and what it prints is only what was left. That is how one of my
/// measurements reported "zero sounds decoded" with 123 sounds requested. Four thousand lines cost
/// less than a megabyte and cover a whole run; drops are still counted, and the report shows
/// them.
const CAPACITY: usize = 4096;

/// The five levels, from the chattiest to the most severe.
///
/// They are the five that Libretro's `RETRO_LOG_LEVEL` and Rust's `log` expect, which is why the
/// order here is the same as there: comparing levels is comparing the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// The path of every decision. It is the usual "verbose" and "debug", together.
    Debug = 0,
    /// What happened and is worth knowing: opened the game, picked the rasterizer, pruned the
    /// cache.
    Info = 1,
    /// What went off plan but does not block anything.
    Warning = 2,
    /// What failed in an operation and was handled.
    Error = 3,
    /// What prevents continuing.
    Fatal = 4,
}

impl Level {
    /// All of them, in the order they appear in a list of options.
    pub const ALL: [Level; 5] = [
        Level::Debug,
        Level::Info,
        Level::Warning,
        Level::Error,
        Level::Fatal,
    ];

    /// The canonical name in Portuguese — what gets stored in the interface setting.
    ///
    /// **It is there so that there is only one.** [`Level::from_text`] accepts two vocabularies,
    /// the core's (Portuguese) and Rust's `log` (English), and storing whatever text came in would
    /// make the same level take two forms in `settings.json` and in `config.ini`. The reader
    /// accepts both, but two equal settings stopped being equal — and that is how the test of the
    /// factory `config.ini` caught the file's `log = warn` against the emulator's `warning`
    /// default.
    ///
    /// The names are the same ones the Libretro core's `zeebx_log` offers, on purpose: one level,
    /// one token, in the three places that write it.
    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "depuracao",
            Level::Info => "informacao",
            Level::Warning => "aviso",
            Level::Error => "erro",
            Level::Fatal => "fatal",
        }
    }

    /// The short name, in uppercase, as it appears in the frontend's log.
    pub fn tag(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warning => "WARN",
            Level::Error => "ERROR",
            Level::Fatal => "FATAL",
        }
    }

    /// The level from an option's text, in Portuguese or by the name Rust's `log` uses.
    ///
    /// It accepts both vocabularies on purpose: the Libretro core's user writes
    /// `debug`, and whoever reads `ZEEBX_LOG` from a script tends to write `debug`.
    pub fn from_text(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "depuracao" | "depuração" | "debug" | "verbose" => Some(Level::Debug),
            "informacao" | "informação" | "info" => Some(Level::Info),
            "aviso" | "warn" | "warning" => Some(Level::Warning),
            "erro" | "error" => Some(Level::Error),
            "fatal" | "critical" | "critico" | "crítico" => Some(Level::Fatal),
            _ => None,
        }
    }

    /// Whether this level passes the `active` filter.
    ///
    /// A pure function on purpose: it is the rule most often gotten wrong when written by hand,
    /// and this way it gets a test without touching the global.
    pub fn passes(self, active: Level) -> bool {
        self >= active
    }
}

/// What the option asked for: a severity threshold, or nothing.
///
/// **It exists because "off" and "I did not understand" are not the same thing.** Returning
/// `None` for both would make a `ZEEBX_LOG=banana` silently turn logging off, which is the worst
/// possible outcome for a debugging option: whoever mistyped it needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Threshold {
    /// Log nothing.
    Off,
    /// Log from this level up.
    UpTo(Level),
}

impl Threshold {
    /// The setting from an option's text. `None` means "not a token at all".
    pub fn from_text(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "desligado" | "off" | "nenhum" => Some(Threshold::Off),
            other => Level::from_text(other).map(Threshold::UpTo),
        }
    }

    /// Applies the setting to the global log.
    pub fn apply(self) {
        match self {
            Threshold::Off => disable(),
            Threshold::UpTo(level) => set_level(level),
        }
    }
}

/// Down to which level logging is on. The default is [`Level::Warning`].
///
/// `Warning` and not `Debug` because instrumentation must not cost much to those who did not ask
/// for it: whoever touches nothing keeps the silence they had before, and only sees what they
/// already saw.
static ACTIVE_LEVEL: AtomicU8 = AtomicU8::new(Level::Warning as u8);

/// A stored line, waiting for someone to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The severity.
    pub level: Level,
    /// The subsystem that wrote it — `"cpu"`, `"gl"`, `"midi"`, `"loader"`.
    pub target: String,
    /// The text, already built.
    pub text: String,
}

/// The ring. A single one for the whole core, because there is a single log worth reading.
static LINES: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());

/// How many lines were dropped because the ring was full. It never resets on its own: the reader
/// resets it, with [`reset_dropped`], so the count is not lost between two reads.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Whether logging is turned off completely.
static DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Starts logging from `level`, inclusive.
pub fn set_level(level: Level) {
    DISABLED.store(false, Ordering::Relaxed);
    ACTIVE_LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Turns logging off completely, whatever the level.
pub fn disable() {
    DISABLED.store(true, Ordering::Relaxed);
}

/// The currently active level.
pub fn level() -> Level {
    match ACTIVE_LEVEL.load(Ordering::Relaxed) {
        0 => Level::Debug,
        1 => Level::Info,
        2 => Level::Warning,
        3 => Level::Error,
        _ => Level::Fatal,
    }
}

/// Whether a message of this level would be logged — to the ring or to the session file.
///
/// It is the cheap test the [`logging!`] macro does before building the text — and it is the only
/// cost of a filtered-out message.
pub fn enabled(message_level: Level) -> bool {
    goes_to_ring(message_level) || goes_to_file(message_level)
}

fn goes_to_ring(message_level: Level) -> bool {
    !DISABLED.load(Ordering::Relaxed) && message_level.passes(level())
}

fn goes_to_file(message_level: Level) -> bool {
    RECORDING.load(Ordering::Relaxed) && message_level.passes(FILE_LEVEL)
}

/// Down to which level the session file records, whatever the ring's level.
///
/// **Independent of the ring on purpose.** The file is for someone who will leave the game open
/// for half an hour and send in the result: with the `Warning` default it would come out almost
/// empty, and the `gpu:` and `file:` lines that measure leaks and writes are `Info`. `Debug` is
/// left out because some games say a thousand lines per frame — half an hour of that is a file
/// nobody opens.
const FILE_LEVEL: Level = Level::Info;

/// Whether a session file is open. Kept apart from the lock so that the [`enabled`] test stays a
/// single atomic read.
static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The session file being recorded, and the number that identifies it.
///
/// The number exists because the Z-Wheel opens the next game **before** releasing its own running
/// game: without it, the end of the old running game would close the new one's file.
static LOG_FILE: Mutex<Option<(u64, std::io::LineWriter<std::fs::File>)>> = Mutex::new(None);
static NEXT_FILE: AtomicU64 = AtomicU64::new(1);

/// Starts recording to `path`, beginning with `header`. Returns the number of this recording,
/// which [`stop_recording`] checks.
///
/// A file that was already open is closed first, with a line saying where the session went on.
/// Each line goes to disk right away (`LineWriter`): what matters in this file is precisely what
/// came before the process died, and a buffer would keep that in memory.
pub fn write_to(log_path: &std::path::Path, header: &str) -> std::io::Result<u64> {
    use std::io::Write;
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::io::LineWriter::new(std::fs::File::create(log_path)?);
    file.write_all(header.as_bytes())?;
    let number = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
    install_panic_hook();
    let mut current = LOG_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, mut previous)) = current.take() {
        let _ = writeln!(
            previous,
            "[{}] INFO registro: the session continued in {}",
            timestamp(std::time::SystemTime::now()),
            log_path.display()
        );
    }
    *current = Some((number, file));
    RECORDING.store(true, Ordering::Relaxed);
    Ok(number)
}

/// Writes a block to the session file, as is, without timestamp or level. It is for the final
/// report, which already has its own shape.
pub fn write_block(number: u64, text: &str) {
    use std::io::Write;
    let mut current = LOG_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, file)) = current.as_mut().filter(|(n, _)| *n == number) {
        let _ = file.write_all(text.as_bytes());
    }
}

/// Ends recording `number` with one last line. If another recording has already replaced it,
/// does nothing.
pub fn stop_recording(number: u64, last: &str) {
    use std::io::Write;
    let mut current = LOG_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !current.as_ref().is_some_and(|(n, _)| *n == number) {
        return;
    }
    if let Some((_, mut file)) = current.take() {
        let _ = writeln!(
            file,
            "[{}] INFO registro: {last}",
            timestamp(std::time::SystemTime::now())
        );
        let _ = file.flush();
    }
    RECORDING.store(false, Ordering::Relaxed);
}

/// Puts a line in the session file, if there is one.
fn write_line(level: Level, target: &str, text: &str) {
    use std::io::Write;
    // A poisoned lock does not silence the file: the panic that poisoned it is precisely what we
    // want to see recorded after it.
    let mut current = LOG_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((_, file)) = current.as_mut() {
        let _ = writeln!(
            file,
            "[{}] {:<5} {target}: {text}",
            timestamp(std::time::SystemTime::now()),
            level.tag()
        );
    }
}

/// Records the panic in the session file before letting the previous hook do its usual thing.
///
/// Without this, a panic in the core left the file ending mid-run, without saying why. The
/// `try_lock` is on purpose: if the panic was raised while holding the file's lock, waiting for it
/// would hang the process instead of letting it go down.
///
/// A native crash — an invalid access in the video driver, in dynarmic — does not go through
/// here, and there is no way to catch it without a signal handler that would fight dynarmic's.
/// For that case, the file without the closing line at the end is the evidence itself.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if RECORDING.load(Ordering::Relaxed) {
                if let Ok(mut current) = LOG_FILE.try_lock() {
                    if let Some((_, file)) = current.as_mut() {
                        use std::io::Write;
                        let stack = std::backtrace::Backtrace::force_capture();
                        let _ = writeln!(
                            file,
                            "[{}] FATAL panic: {info}\n{stack}",
                            timestamp(std::time::SystemTime::now())
                        );
                        let _ = file.flush();
                    }
                }
            }
            previous(info);
        }));
    });
}

/// `yyyy-mm-dd hh:mm:ss UTC`.
pub fn timestamp(instant: std::time::SystemTime) -> String {
    let (data, [h, m, s]) = utc_date_and_time(instant);
    format!("{data} {h:02}:{m:02}:{s:02} UTC")
}

/// `yyyy-mm-dd_hh-mm-ss`, for a file name: without `:`, which Windows refuses, and in an order
/// that makes the folder listing chronological on its own.
pub fn file_timestamp(instant: std::time::SystemTime) -> String {
    let (data, [h, m, s]) = utc_date_and_time(instant);
    format!("{data}_{h:02}-{m:02}-{s:02}")
}

/// The date as `yyyy-mm-dd` and the time, in UTC.
///
/// Written by hand so as not to pull in a date library for two lines: it is Howard Hinnant's
/// `civil_from_days`, which holds for the whole proleptic Gregorian calendar.
fn utc_date_and_time(instant: std::time::SystemTime) -> (String, [u64; 3]) {
    let seconds = instant
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rest) = ((seconds / 86_400) as i64, seconds % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        format!("{year:04}-{month:02}-{day:02}"),
        [rest / 3600, rest % 3600 / 60, rest % 60],
    )
}

/// Puts a line in the ring and, if a session is being recorded, in the file.
///
/// The ring writes nowhere: it is the frontend that shows it, by calling [`drain`] at a safe point
/// — in Libretro, inside `retro_run`, because the frontend's log cannot be called from just
/// anywhere. The file, on the other hand, is written right here, right away: the line that matters
/// is the last one before the process goes down, and it cannot wait for the frontend to drain.
pub fn write(level: Level, target: &str, text: &str) {
    if goes_to_file(level) {
        write_line(level, target, text);
    }
    if !goes_to_ring(level) {
        return;
    }
    let line = Line {
        level,
        target: target.to_string(),
        text: text.to_string(),
    };
    // A poisoned lock here is no reason to bring down the emulator: the log is an instrument, and
    // an instrument that fails goes quiet instead of killing the game.
    let Ok(mut lines) = LINES.lock() else {
        return;
    };
    if push_to_ring(&mut lines, line) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Puts the line in the ring, removing the oldest one when it is already full.
///
/// Returns `true` when it **dropped** something. It is a function outside the global on purpose:
/// the cap rule is the one most in need of a test, and testing it against the process ring makes
/// the test depend on whoever else is writing to it.
fn push_to_ring(ring: &mut VecDeque<Line>, line: Line) -> bool {
    let dropped = ring.len() >= CAPACITY;
    if dropped {
        ring.pop_front();
    }
    ring.push_back(line);
    dropped
}

/// Takes everything out of the ring, in the order it was written.
pub fn drain() -> Vec<Line> {
    match LINES.lock() {
        Ok(mut lines) => lines.drain(..).collect(),
        Err(_) => Vec::new(),
    }
}

/// How many lines were dropped since the last [`reset_dropped`].
pub fn dropped() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Resets the drop count.
pub fn reset_dropped() {
    DROPPED.store(0, Ordering::Relaxed);
}

/// Empties the ring and the drop count.
pub fn clear() {
    if let Ok(mut lines) = LINES.lock() {
        lines.clear();
    }
    reset_dropped();
}

/// Reads the level from `ZEEBX_LOG` and applies it, when the variable exists.
///
/// It applies to every frontend at once — headless, scan, desktop — without depending on an
/// interface option, which is what serves someone debugging on the command line.
pub fn from_env() {
    let Ok(value) = std::env::var("ZEEBX_LOG") else {
        return;
    };
    match Threshold::from_text(&value) {
        Some(threshold) => threshold.apply(),
        None => write(
            Level::Warning,
            "registro",
            &format!("ZEEBX_LOG=`{value}` is not a level; keeping the default"),
        ),
    }
}

/// Shows on `stderr` what is in the ring, in the format `LEVEL target: text`.
///
/// It is the channel of the frontends that have no log of their own to receive the lines — the
/// desktop and headless — and the same one the scan uses for the report. Libretro does **not**
/// use this path: there the destination is the frontend's `retro_log`, which decides the file and
/// the verbosity, and that is why the dump lives in the core and not here.
pub fn dump_to_stderr() {
    for line in drain() {
        eprintln!("{} {}: {}", line.level.tag(), line.target, line.text);
    }
    let dropped = dropped();
    if dropped > 0 {
        reset_dropped();
        eprintln!("WARN registro: {dropped} line(s) dropped by the ring cap");
    }
}

/// Logs a message, if its level passes the filter.
///
/// The `if` sits outside the `format!` on purpose: a filtered-out debug message should not even
/// cost building the text.
#[macro_export]
macro_rules! logging {
    ($level:expr, $target:expr, $($arg:tt)*) => {
        if $crate::logging::enabled($level) {
            $crate::logging::write($level, $target, &format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the tests that touch the global ring and level.
    ///
    /// The tests of a binary run in parallel, and there is a single log for the whole process:
    /// without this, a test that drains steals the other's lines, and the one that measures
    /// overflow never fills up. That is how the first version of this test passed on its own and
    /// failed in the suite.
    static LOCK: Mutex<()> = Mutex::new(());

    /// The suite's lock, tolerant of a test that panicked before releasing it.
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The test line, with a target that does not collide with anyone else's.
    fn line(target: &str, text: &str) -> Line {
        Line {
            level: Level::Info,
            target: target.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn the_option_text_becomes_the_right_level() {
        assert_eq!(Level::from_text("depuracao"), Some(Level::Debug));
        assert_eq!(Level::from_text("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::from_text("verbose"), Some(Level::Debug));
        assert_eq!(Level::from_text("info"), Some(Level::Info));
        assert_eq!(Level::from_text(" warning "), Some(Level::Warning));
        assert_eq!(Level::from_text("erro"), Some(Level::Error));
        assert_eq!(Level::from_text("critical"), Some(Level::Fatal));
        assert_eq!(Level::from_text("banana"), None);
        // Off is not a level: treating the two alike turns the log off by mistake.
        assert_eq!(Level::from_text("desligado"), None);
    }

    /// "Off" and "I did not understand" must be distinguishable, or a typo turns logging off
    /// without warning.
    #[test]
    fn off_and_invalid_text_are_not_confused() {
        assert_eq!(Threshold::from_text("desligado"), Some(Threshold::Off));
        assert_eq!(Threshold::from_text("off"), Some(Threshold::Off));
        assert_eq!(Threshold::from_text("banana"), None);
        assert_eq!(Threshold::from_text("aviso"), Some(Threshold::UpTo(Level::Warning)));
    }

    #[test]
    fn the_setting_turns_logging_on_and_off() {
        let _alone = alone();
        Threshold::UpTo(Level::Error).apply();
        assert_eq!(level(), Level::Error);
        assert!(enabled(Level::Error));
        assert!(!enabled(Level::Warning));

        Threshold::Off.apply();
        assert!(!enabled(Level::Fatal), "off does not log even fatal");

        set_level(Level::Warning);
    }

    /// The filter is the rule most often gotten wrong by hand: the active level and the
    /// message's level are the same scale, and passing means "equal or more severe".
    #[test]
    fn the_filter_passes_the_active_level_and_above() {
        let warning = Level::Warning;
        assert!(!Level::Debug.passes(warning), "debug is chattier");
        assert!(!Level::Info.passes(warning));
        assert!(Level::Warning.passes(warning), "the level itself passes");
        assert!(Level::Error.passes(warning));
        assert!(Level::Fatal.passes(warning));

        let all = Level::Debug;
        assert!(Level::ALL.iter().all(|n| n.passes(all)));
    }

    /// `name` is the canonical token, and the other two vocabularies are still accepted.
    ///
    /// It is what makes `log = warn` in `config.ini` and `warning` in `settings.json` the **same**
    /// setting — before, the factory file of the windowless frontend described a default that was
    /// not the emulator's, and its test suite went red because of it.
    #[test]
    fn the_canonical_name_is_the_single_level_token() {
        assert_eq!(Level::Warning.name(), "aviso");
        assert_eq!(Level::Debug.name(), "depuracao");
        for level in Level::ALL {
            assert_eq!(
                Level::from_text(level.name()),
                Some(level),
                "{}",
                level.name()
            );
            assert_eq!(
                Level::from_text(level.tag()),
                Some(level),
                "the log tag is accepted too"
            );
        }
        // The two outside vocabularies still work, and land on the same name.
        assert_eq!(Level::from_text("warn"), Some(Level::Warning));
        assert_eq!(Level::from_text("warning"), Some(Level::Warning));
        assert_eq!(Level::from_text("DEBUG"), Some(Level::Debug));
        assert_eq!(Level::from_text("banana"), None);
    }

    #[test]
    fn the_tag_matches_rust_log() {
        assert_eq!(Level::Debug.tag(), "DEBUG");
        assert_eq!(Level::Info.tag(), "INFO");
        assert_eq!(Level::Warning.tag(), "WARN");
        assert_eq!(Level::Error.tag(), "ERROR");
        assert_eq!(Level::Fatal.tag(), "FATAL");
    }

    /// The ring keeps order and `drain` empties it. It does not use `clear` at the start so as not
    /// to erase what another test may be looking at: it looks for its own lines.
    #[test]
    fn the_ring_keeps_order_and_drain_empties_it() {
        let _alone = alone();
        clear();
        set_level(Level::Debug);
        write(Level::Info, "teste-do-anel", "primeira");
        write(Level::Error, "teste-do-anel", "segunda");

        let mine: Vec<Line> = drain()
            .into_iter()
            .filter(|l| l.target == "teste-do-anel")
            .collect();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].text, "primeira");
        assert_eq!(mine[1].text, "segunda");
        assert_eq!(mine[1].level, Level::Error);

        // After draining, what was there does not come back.
        let again: Vec<Line> = drain()
            .into_iter()
            .filter(|l| l.target == "teste-do-anel")
            .collect();
        assert!(again.is_empty());
    }

    /// **The cap rule, without the global.** The ring must drop the oldest line and say it did:
    /// overflowing silently would hide exactly the part that went missing.
    ///
    /// The rule is tested here, and not against the process ring, because the process ring is
    /// shared: the first version of this test filled 320 lines and failed in the suite, because
    /// another test drained in the middle.
    #[test]
    fn the_ring_overflows_at_the_end_and_counts_the_drop() {
        let mut ring: VecDeque<Line> = VecDeque::new();
        let mut dropped = 0;

        for i in 0..CAPACITY {
            assert!(!push_to_ring(&mut ring, line("teto", &format!("linha {i}"))));
        }
        assert_eq!(ring.len(), CAPACITY, "exactly the cap fits");

        // One more: the first one goes out, the new one comes in.
        assert!(push_to_ring(&mut ring, line("teto", "a que chega")));
        dropped += 1;
        assert_eq!(ring.len(), CAPACITY, "the cap does not grow");
        assert_eq!(dropped, 1);
        assert_eq!(
            ring.front().map(|l| l.text.as_str()),
            Some("linha 1"),
            "the one that went out was the oldest"
        );
        assert_eq!(ring.back().map(|l| l.text.as_str()), Some("a que chega"));

        // Twenty more: twenty drops, and the size stays at the cap.
        for i in 0..20 {
            if push_to_ring(&mut ring, line("teto", &format!("extra {i}"))) {
                dropped += 1;
            }
        }
        assert_eq!(dropped, 21);
        assert_eq!(ring.len(), CAPACITY);
    }

    /// The global ring's drop counter shows up and goes away when someone resets it.
    #[test]
    fn the_drop_counter_shows_up_and_resets() {
        let _alone = alone();
        clear();
        reset_dropped();
        // **The level must be set here.** Without this the test depended on what the previous one
        // left behind: a write at `Info` with the level at `Warning` does not enter the ring, and
        // there are no drops to count. It passed until the test order changed.
        set_level(Level::Debug);
        for i in 0..(CAPACITY + 5) {
            write(Level::Info, "teste-do-teto", &format!("linha {i}"));
        }
        assert!(dropped() >= 5, "the drops must show up");
        reset_dropped();
        assert_eq!(dropped(), 0);
        clear();
    }

    /// The dump empties the ring, and calling it with an empty ring does nothing.
    #[test]
    fn the_dump_empties_the_ring() {
        let _alone = alone();
        set_level(Level::Debug);
        write(Level::Warning, "teste-do-despejo", "para o stderr");
        dump_to_stderr();
        assert!(
            !drain().iter().any(|l| l.target == "teste-do-despejo"),
            "after dumping, the line does not come back"
        );
        // Second call with nothing pending: it must not panic or make up a line.
        dump_to_stderr();
    }

    #[test]
    fn the_timestamp_is_the_correct_utc_date() {
        let at = |seconds: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        assert_eq!(timestamp(at(0)), "1970-01-01 00:00:00 UTC");
        // February 29 of a leap year, and the last second of the day.
        assert_eq!(timestamp(at(951_868_799)), "2000-02-29 23:59:59 UTC");
        assert_eq!(timestamp(at(1_791_464_405)), "2026-10-08 13:00:05 UTC");
        assert_eq!(file_timestamp(at(1_791_464_405)), "2026-10-08_13-00-05");
    }

    /// The file records from `Info` up even with the ring at `Fatal`, and ending a recording that
    /// has already been replaced does not close the one in progress.
    #[test]
    fn the_session_file_writes_independently_of_the_ring() {
        let _alone = alone();
        clear();
        set_level(Level::Fatal);
        let dir = std::env::temp_dir().join(format!("zeebx-registro-{}", std::process::id()));
        let first = dir.join("primeiro.log");
        let second = dir.join("segundo.log");

        let one = write_to(&first, "CABEÇALHO\n").unwrap();
        write(Level::Info, "teste-do-arquivo", "entra no arquivo");
        write(Level::Debug, "teste-do-arquivo", "fica de fora");
        assert!(
            !drain().iter().any(|l| l.target == "teste-do-arquivo"),
            "the ring stays at its own level"
        );
        let two = write_to(&second, "OUTRO\n").unwrap();
        stop_recording(one, "não devia fechar o segundo");
        write(Level::Error, "teste-do-arquivo", "no segundo");
        stop_recording(two, "sessão encerrada");
        assert!(!enabled(Level::Error), "without a file, the ring at fatal does not accept error");

        // Looks for the lines instead of counting them: other tests in the suite write to the log
        // at the same time, and what they say also lands in the file.
        let loaded = std::fs::read_to_string(&first).unwrap();
        assert!(loaded.starts_with("CABEÇALHO\n"));
        let entry = loaded
            .lines()
            .find(|l| l.contains("teste-do-arquivo: entra no arquivo"))
            .expect("the info line must be in the file");
        assert!(entry.starts_with('[') && entry.contains(" UTC] INFO  teste-do-arquivo"));
        assert!(!loaded.contains("fica de fora"));
        assert!(loaded.trim_end().ends_with(&format!("the session continued in {}", second.display())));
        assert!(!loaded.contains("não devia fechar"));

        let loaded = std::fs::read_to_string(&second).unwrap();
        assert!(loaded.starts_with("OUTRO\n"));
        assert!(loaded.contains("ERROR teste-do-arquivo: no segundo"));
        assert!(loaded.trim_end().ends_with("sessão encerrada"));

        let _ = std::fs::remove_dir_all(&dir);
        set_level(Level::Warning);
    }

    /// A filtered-out message does not enter the ring: that is what backs the cost argument.
    #[test]
    fn a_message_below_the_level_stays_out_of_the_ring() {
        let _alone = alone();
        clear();
        set_level(Level::Fatal);
        assert!(!enabled(Level::Info));
        write(Level::Info, "teste-filtrado", "não devia entrar");
        let found = drain().iter().any(|l| l.target == "teste-filtrado");
        assert!(!found, "the filter must apply before storing");

        set_level(Level::Debug);
        assert!(enabled(Level::Info));
    }
}
