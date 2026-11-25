//! Looks for a new version in the GitHub releases.
//!
//! The question is the list of releases, and the answer arrives on a thread, so the window does
//! not wait for the network. The tag can come as `0.1.0` or `v0.1.0`.
//!
//! **There are two channels, and the user picks one** ([`UpdateChannel`]):
//!
//! - *stable* is the releases: the highest version in the list that is neither a draft nor a
//!   pre-release;
//! - *nightly* is those plus the build made every day from the development branch, which lives in
//!   a single release, tagged [`NIGHTLY_TAG`], replaced on every run.
//!
//! **It is not `releases/latest`**, and a pre-release no longer counts. Every release up to 0.6.0
//! went out marked as a pre-release, which `latest` leaves out: the answer was 404, which became
//! "up to date". The list was the way around it, with an option to include them. From here on a
//! release is published as a release, and the pre-release mark is left for the nightly — which is
//! what keeps it away from `latest` and from anyone on the stable channel.
//!
//! **A nightly is not compared by version.** The version in `Cargo.toml` only moves when someone
//! moves it, and every nightly between two releases carries the same one. What tells two builds
//! apart is when each was made: CI stamps every build with [`BUILD_STAMP`], and the nightly's is
//! in its name (`0.7.0-nightly.202611200300`). See [`pick`] for the rule.
//!
//! With the `atualizador` feature, [`install`] downloads the new version and replaces this copy
//! with it, in the packages that know how to be replaced (see [`Installation`]). What is
//! downloaded is described in a manifest published in the release (`latest-qt.json`), with the
//! minisign signature of each package; the public key is in [`PUBLIC_KEY`], and a package that
//! does not check out is not installed.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use crate::ui::settings::UpdateChannel;

/// Where the releases are published.
pub const REPOSITORY: &str = "ZeebxTeam/zeebx-emu";

/// The version of this binary, as `Cargo.toml` has it. A nightly carries the same one as the
/// release it comes after: what it shows the user is [`build_version`].
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The tag of the one release the nightly lives in.
///
/// **A word, and not a version, on purpose.** The copies already installed compare tags by their
/// numbers, and a tag with none is worth `0.0.0`: they do not see it. Tagged `v0.7.0-nightly`, it
/// would be offered as 0.7.0 to everyone who has not updated yet.
pub const NIGHTLY_TAG: &str = "nightly";

/// When CI made this build, in UTC, as `YYYYMMDDHHMM`. A build made by hand has none.
///
/// A number that reads as a date and compares as one. `nightly.yml` and `release.yml` set
/// `ZEEBX_BUILD_STAMP`; the first also sets `ZEEBX_BUILD_CHANNEL=nightly`.
const BUILD_STAMP: Option<&str> = option_env!("ZEEBX_BUILD_STAMP");
const BUILD_CHANNEL: Option<&str> = option_env!("ZEEBX_BUILD_CHANNEL");

/// What a build is, for the purpose of deciding whether another one is newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Build {
    numbers: [u64; 3],
    /// Made from the development branch, and not from a tag.
    nightly: bool,
    /// Zero when unknown: a build made by hand is older than anything published.
    stamp: u64,
}

impl Build {
    fn this() -> Self {
        Self {
            numbers: numbers(CURRENT_VERSION),
            nightly: BUILD_CHANNEL == Some(NIGHTLY_TAG),
            stamp: BUILD_STAMP.map_or(0, stamp),
        }
    }
}

/// The version this build shows: `0.6.0`, or `0.6.0-nightly.202611200300` for a nightly. It is
/// also what the installer compares, and it is valid semver.
pub fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let build = Build::this();
        match build.nightly && build.stamp > 0 {
            true => nightly_version(CURRENT_VERSION, build.stamp),
            false => CURRENT_VERSION.to_string(),
        }
    })
}

/// The name of a nightly: what it shows, what its release is called and what its manifest says.
fn nightly_version(version: &str, stamp: u64) -> String {
    format!("{version}-{NIGHTLY_TAG}.{stamp}")
}

/// A release newer than this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the `v`: `0.2.0`, or `0.7.0-nightly.202611200300`.
    pub version: String,
    /// As it is on GitHub, with or without the `v`: it is what builds the address of the release
    /// files. For a nightly it is always [`NIGHTLY_TAG`].
    pub tag: String,
    /// The release page, with the notes and the installers.
    pub page: String,
}

/// What the search answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    NewVersion(Release),
    UpToDate,
    Failed(String),
}

/// Starts the search on a thread. The answer arrives through the channel, once.
pub fn search(channel: UpdateChannel) -> Receiver<Response> {
    let (sender, receive) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("atualizacao".into())
        .spawn(move || {
            let response = match query(channel) {
                Ok(Some(release)) => Response::NewVersion(release),
                Ok(None) => Response::UpToDate,
                Err(error) => Response::Failed(error),
            };
            let _ = sender.send(response);
        });
    receive
}

/// One question to the GitHub API. `Ok(None)` is a 404, which the caller reads its own way.
fn ask(path: &str) -> Result<Option<serde_json::Value>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/{path}");
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get(&url)
        .header("User-Agent", &format!("zeebx/{CURRENT_VERSION}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|error| error.to_string())?;
    match response.status().as_u16() {
        200 => {}
        404 => return Ok(None),
        code => return Err(format!("GitHub answered {code}")),
    }
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&text).map(Some).map_err(|error| error.to_string())
}

fn query(channel: UpdateChannel) -> Result<Option<Release>, String> {
    // With no release at all the list comes back empty, with 200. A 404 here is a wrong
    // repository, and has to show up as a failure, not as "up to date".
    let list = ask("releases?per_page=20")?.ok_or("GitHub answered 404")?;
    // The nightly is asked for by its tag, and not looked for in the list: the list is the 20
    // newest, and a release that is replaced in place keeps the date it was first created on.
    // Here a 404 is an honest answer — no nightly has been published yet.
    let nightly = match channel {
        UpdateChannel::Nightly => ask(&format!("releases/tags/{NIGHTLY_TAG}"))?,
        UpdateChannel::Stable => None,
    };
    Ok(pick(&list, nightly.as_ref(), Build::this()))
}

/// The release this build should move to, if there is one.
///
/// **A stable release** is newer when its version is higher. For a nightly build there is one
/// more case: the release of the very version it was counting towards. `0.7.0-nightly` becomes
/// 0.7.0 — but only if the release was published after the build was made, or a nightly from the
/// week after 0.7.0 would be sent back to it.
///
/// **A nightly** is newer when it was made later, and it is only looked at on the nightly
/// channel. Its version must not be lower than this build's: the nightly that was left behind by
/// a release is not an update to that release. The same goes for a nightly of the *same* version
/// made before that version was published — without this the two pointed at each other: the
/// nightly was offered the release published after it, and the release, stamped when its build
/// started, was offered that nightly back, on every launch.
///
/// With both on the table, the stable one wins when its version is higher — the release has
/// what that nightly had, and was looked at — and the nightly wins otherwise.
fn pick(list: &serde_json::Value, nightly: Option<&serde_json::Value>, this: Build) -> Option<Release> {
    let stable = list
        .as_array()
        .into_iter()
        .flatten()
        // A draft never counts — the API only shows them to whoever has write access, but the
        // filter costs nothing. Neither does a pre-release, which is how the nightly is marked.
        .filter(|release| release["draft"].as_bool() != Some(true))
        .filter(|release| release["prerelease"].as_bool() != Some(true))
        .filter_map(|release| Some((release["tag_name"].as_str()?.trim(), release)))
        .max_by_key(|(tag, _)| numbers(tag))
        .filter(|(tag, release)| {
            let published = release["published_at"].as_str().map_or(0, stamp);
            numbers(tag) > this.numbers
                || (this.nightly && numbers(tag) == this.numbers && published > this.stamp)
        });
    // When the version a nightly is named after went out, or zero if it has not yet.
    let released = |version: [u64; 3]| {
        list.as_array()
            .into_iter()
            .flatten()
            .filter(|release| release["draft"].as_bool() != Some(true))
            .filter(|release| release["prerelease"].as_bool() != Some(true))
            .filter(|release| release["tag_name"].as_str().is_some_and(|tag| numbers(tag) == version))
            .map(|release| release["published_at"].as_str().map_or(0, stamp))
            .max()
            .unwrap_or(0)
    };
    let nightly = nightly
        .filter(|release| release["draft"].as_bool() != Some(true))
        .and_then(|release| Some((release["name"].as_str()?.trim(), release)))
        .filter(|(name, _)| {
            let made = nightly_stamp(name);
            made > 0
                && made > this.stamp
                && numbers(name) >= this.numbers
                && made > released(numbers(name))
        });

    let page = |tag: &str, release: &serde_json::Value| {
        release["html_url"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases/tag/{tag}"))
    };
    match (stable, nightly) {
        (Some((tag, _)), Some((name, release))) if numbers(tag) <= numbers(name) => Some(Release {
            version: name.to_string(),
            tag: NIGHTLY_TAG.to_string(),
            page: page(NIGHTLY_TAG, release),
        }),
        (Some((tag, release)), _) => Some(Release {
            version: strip_v(tag).to_string(),
            tag: tag.to_string(),
            page: page(tag, release),
        }),
        (None, Some((name, release))) => Some(Release {
            version: name.to_string(),
            tag: NIGHTLY_TAG.to_string(),
            page: page(NIGHTLY_TAG, release),
        }),
        (None, None) => None,
    }
}

fn strip_v(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

/// The numbers of a version, `1.2.3` → `[1, 2, 3]`. What comes after `-` or `+` does not count,
/// and a part that is not a number is worth zero.
fn numbers(version: &str) -> [u64; 3] {
    let emu_core = strip_v(version).split(['-', '+']).next().unwrap_or("");
    let mut parts = emu_core.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0));
    [
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    ]
}

/// A moment as `YYYYMMDDHHMM`, out of that very number or out of the date GitHub gives a release
/// (`2026-11-20T03:00:12Z`): the first twelve digits, in both. Zero for what is neither.
fn stamp(text: &str) -> u64 {
    let digits: String = text.chars().filter(char::is_ascii_digit).take(12).collect();
    match digits.len() {
        12 => digits.parse().unwrap_or(0),
        _ => 0,
    }
}

/// When the nightly called `name` was made: what follows `-nightly.`. Zero if it is not one.
fn nightly_stamp(name: &str) -> u64 {
    name.split_once(&format!("-{NIGHTLY_TAG}."))
        .map_or(0, |(_, made)| stamp(made))
}

/// Whether `candidate` is newer than `current`, by the numbers alone.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    numbers(candidate) > numbers(current)
}

/// The minisign public key that signs the release packages, in the `cargo packager` format (the
/// whole `.key.pub`, in base64). The private one lives in the GitHub secrets
/// (`ZEEBX_ATUALIZADOR_CHAVE`); changing one without the other makes every update be refused.
pub const PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDAxMkMyQzgwMjQ3QUMyNTAKUldSUXdub2tnQ3dzQVFQNnpTNEt1N0txbFIvTXJYUG5IOEYxMS9WdGIzUXh0cWpFM29HK2R5L1gK";

/// How this copy was installed, which decides whether it can replace itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installation {
    /// An AppImage: the file is replaced in place.
    AppImage,
    /// The Windows NSIS installer: the new one runs in passive mode and reopens the emulator.
    WindowsInstaller,
    /// A `Zeebx.app` on macOS: the whole bundle is replaced.
    MacPackage,
    /// A package manager takes care of it (Flatpak). The emulator does not touch it, nor warn: the
    /// new version arrives through the manager, and a warning would only confuse.
    Managed,
    /// `.deb`, built by hand, or anything else: the files are not ours to replace (in the `.deb`
    /// they belong to root and dpkg). Only the warning remains, with the release page.
    Manual,
}

impl Installation {
    /// This copy's.
    pub fn current() -> Self {
        let exe = std::env::current_exe().ok();
        detect(
            cfg!(target_os = "linux"),
            cfg!(windows),
            cfg!(target_os = "macos"),
            std::env::var_os("FLATPAK_ID").is_some(),
            std::env::var_os("APPIMAGE").is_some(),
            exe.as_deref(),
        )
    }

    /// Whether the update button can download and install, instead of only opening the page.
    pub fn can_self_replace(self) -> bool {
        matches!(self, Self::AppImage | Self::WindowsInstaller | Self::MacPackage) && cfg!(feature = "atualizador")
    }

    /// Whether looking for a new version makes sense here.
    pub fn warn(self) -> bool {
        self != Self::Managed
    }
}

fn detect(
    linux: bool,
    windows: bool,
    mac: bool,
    flatpak: bool,
    appimage: bool,
    exe: Option<&std::path::Path>,
) -> Installation {
    if linux && flatpak {
        Installation::Managed
    } else if linux && appimage {
        Installation::AppImage
    } else if windows && exe.and_then(|e| e.parent()).is_some_and(|d| d.join("uninstall.exe").is_file()) {
        // `uninstall.exe` is what `cargo packager`'s NSIS leaves next to the executable. A loose
        // `.exe`, built by hand, does not have it, and running the installer over it would install
        // a second copy somewhere else.
        Installation::WindowsInstaller
    } else if mac && exe.and_then(mac_package).is_some() {
        Installation::MacPackage
    } else {
        Installation::Manual
    }
}

/// The `Zeebx.app` the executable is in, if it is in one: `.../Zeebx.app/Contents/MacOS/zeebx`.
fn mac_package(exe: &std::path::Path) -> Option<&std::path::Path> {
    let package = exe.parent()?.parent()?.parent()?;
    (package.extension()? == "app").then_some(package)
}

/// Where an installation in progress stands. The frontends read it every frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Downloading { downloaded: u64, total: Option<u64> },
    Installing,
    /// Installed. It still has to reopen: see [`restart`]. On Windows this is never reached — the
    /// installer closes the emulator and reopens the new one by itself.
    Ready,
    Failed(String),
}

/// An installation started by [`install`].
#[derive(Clone)]
pub struct UpdateTask(std::sync::Arc<std::sync::Mutex<Progress>>);

impl UpdateTask {
    pub fn progress(&self) -> Progress {
        self.0.lock().map(|a| a.clone()).unwrap_or(Progress::Failed("poisoned lock".into()))
    }

    /// From 0 to 1, while downloading with a known size.
    pub fn fraction(&self) -> Option<f32> {
        match self.progress() {
            Progress::Downloading { downloaded, total: Some(total) } if total > 0 => {
                Some((downloaded as f32 / total as f32).min(1.0))
            }
            _ => None,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.progress(), Progress::Ready | Progress::Failed(_))
    }
}

/// Downloads and installs `release` on a thread. `manifest` is the release file that describes
/// this frontend's packages: `latest-qt.json`. Without the `atualizador` feature (the headless,
/// which builds the same window) the answer is a failure, and the button does not even show up:
/// see [`Installation::can_self_replace`].
#[cfg(not(feature = "atualizador"))]
pub fn install(_: &Release, _: &str) -> UpdateTask {
    let failed = Progress::Failed("built without the `atualizador` feature".into());
    UpdateTask(std::sync::Arc::new(std::sync::Mutex::new(failed)))
}

/// Downloads and installs `release` on a thread. `manifest` is the release file that describes
/// this frontend's packages: `latest-qt.json`.
#[cfg(feature = "atualizador")]
pub fn install(release: &Release, manifest: &str) -> UpdateTask {
    use std::sync::{Arc, Mutex};
    let state = Arc::new(Mutex::new(Progress::Downloading { downloaded: 0, total: None }));
    let updater = UpdateTask(state.clone());
    let url = format!(
        "https://github.com/{REPOSITORY}/releases/download/{}/{manifest}",
        release.tag
    );
    let _ = std::thread::Builder::new()
        .name("instala-atualizacao".into())
        .spawn(move || {
            let put = |progress| {
                if let Ok(mut current) = state.lock() {
                    *current = progress;
                }
            };
            match download_and_install(&url, &state) {
                Ok(()) => put(Progress::Ready),
                Err(error) => put(Progress::Failed(error)),
            }
        });
    updater
}

#[cfg(feature = "atualizador")]
fn download_and_install(
    url: &str,
    state: &std::sync::Arc<std::sync::Mutex<Progress>>,
) -> Result<(), String> {
    use cargo_packager_updater::{Config, UpdaterBuilder, WindowsConfig, WindowsUpdateInstallMode};
    let config = Config {
        endpoints: vec![url.parse().map_err(|e| format!("{e}"))?],
        pubkey: PUBLIC_KEY.into(),
        windows: Some(WindowsConfig {
            installer_args: None,
            install_mode: Some(WindowsUpdateInstallMode::Passive),
        }),
    };
    let current: cargo_packager_updater::semver::Version =
        build_version().parse().map_err(|e| format!("{e}"))?;
    // No `timeout`: in `reqwest` it applies to the whole response, and a 100 MB AppImage on a slow
    // connection would go past any reasonable number.
    //
    // **Whether it is newer was already decided**, by [`pick`], and semver would decide it
    // differently: `0.6.0-nightly.…` is *lower* than 0.6.0, so a release could never move to the
    // nightly made after it. What is left to check here is that the manifest is not this very
    // build — the release was replaced between the search and the click.
    let updater = UpdaterBuilder::new(current, config)
        .version_comparator(|current, remote| remote.version != current)
        .build()
        .map_err(|e| e.to_string())?;
    let Some(package) = updater.check().map_err(|e| e.to_string())? else {
        return Err("the release manifest carries the version already installed".into());
    };
    let downloaded = std::cell::Cell::new(0u64);
    let bytes = package
        .download_extended(
            |chunk, total| {
                downloaded.set(downloaded.get() + chunk as u64);
                if let Ok(mut current) = state.lock() {
                    *current = Progress::Downloading { downloaded: downloaded.get(), total };
                }
            },
            || {
                if let Ok(mut current) = state.lock() {
                    *current = Progress::Installing;
                }
            },
        )
        .map_err(|e| e.to_string())?;
    // On Windows this call does not return: it opens the installer and ends the process.
    package.install(bytes).map_err(|e| e.to_string())
}

/// Opens the freshly installed version and ends this one. Only after [`Progress::Ready`]; the
/// caller saves whatever it has to save first, because there is no coming back from here.
pub fn restart() -> ! {
    let exe = std::env::current_exe().ok();
    let opened = if let Some(appimage) = std::env::var_os("APPIMAGE") {
        std::process::Command::new(appimage).spawn().is_ok()
    } else if let Some(package) = exe.as_deref().and_then(mac_package) {
        // `open -n` opens a new instance even with this one still up.
        std::process::Command::new("open").arg("-n").arg(package).spawn().is_ok()
    } else {
        exe.is_some_and(|exe| std::process::Command::new(exe).spawn().is_ok())
    };
    if !opened {
        eprintln!("could not open the new version; it opens next time");
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asks the real GitHub: `cargo test --release real_query -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn real_query() {
        println!("{:?}", query(UpdateChannel::Stable));
        println!("{:?}", query(UpdateChannel::Nightly));
    }

    #[test]
    fn compares_versions_with_and_without_v() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }

    fn release(tag: &str, draft: bool, pre: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPOSITORY}/releases/tag/{tag}"),
            "draft": draft,
            "prerelease": pre,
        })
    }

    /// The end-to-end installation, against a local server: `ZEEBX_TEST_MANIFEST` is the address
    /// of a manifest with a version higher than this one, and `APPIMAGE` the file to replace.
    /// The signature must have been made with the key of [`PUBLIC_KEY`].
    #[cfg(all(feature = "atualizador", target_os = "linux"))]
    #[test]
    #[ignore]
    fn installs_from_a_local_server() {
        let url = crate::config::env_var("ZEEBX_TEST_MANIFEST").expect("ZEEBX_TEST_MANIFEST");
        let state = std::sync::Arc::new(std::sync::Mutex::new(Progress::Installing));
        println!("{:?}", download_and_install(&url, &state));
        println!("{:?}", state.lock().unwrap());
    }

    #[test]
    fn detects_the_installation() {
        use std::path::Path;
        let mac = Path::new("/Applications/Zeebx.app/Contents/MacOS/zeebx");
        let loose = Path::new("/usr/bin/zeebx");
        assert_eq!(detect(true, false, false, true, true, Some(loose)), Installation::Managed);
        assert_eq!(detect(true, false, false, false, true, Some(loose)), Installation::AppImage);
        assert_eq!(detect(true, false, false, false, false, Some(loose)), Installation::Manual);
        assert_eq!(detect(false, false, true, false, false, Some(mac)), Installation::MacPackage);
        assert_eq!(detect(false, false, true, false, false, Some(loose)), Installation::Manual);
        // Without `uninstall.exe` next to it: a loose `.exe`.
        assert_eq!(
            detect(false, true, false, false, false, Some(Path::new("C:/x/zeebx.exe"))),
            Installation::Manual
        );
    }

    fn published(tag: &str, pre: bool, at: &str) -> serde_json::Value {
        let mut release = release(tag, false, pre);
        release["published_at"] = at.into();
        release
    }

    /// The nightly release as `nightly.yml` publishes it: the tag is the word, the name says
    /// which build it is.
    fn nightly(name: &str) -> serde_json::Value {
        let mut release = release(NIGHTLY_TAG, false, true);
        release["name"] = name.into();
        release
    }

    const STABLE: Build = Build { numbers: [0, 6, 0], nightly: false, stamp: 202611010000 };
    const NIGHTLY: Build = Build { numbers: [0, 6, 0], nightly: true, stamp: 202611200300 };

    fn version(found: Option<Release>) -> Option<String> {
        found.map(|release| release.version)
    }

    #[test]
    fn reads_a_moment_out_of_a_stamp_and_out_of_a_date() {
        assert_eq!(stamp("202611200300"), 202611200300);
        assert_eq!(stamp("2026-11-20T03:00:12Z"), 202611200300);
        assert_eq!(stamp("nightly"), 0);
        assert_eq!(stamp(""), 0);
        assert_eq!(nightly_stamp("0.7.0-nightly.202611200300"), 202611200300);
        assert_eq!(nightly_stamp("0.7.0"), 0);
        assert_eq!(nightly_stamp("nightly"), 0);
        // What the name carries is what the build shows, and it is semver for the installer.
        assert_eq!(nightly_version("0.7.0", 202611200300), "0.7.0-nightly.202611200300");
    }

    #[test]
    fn picks_the_highest_in_the_list() {
        // The API order is by date, not by version: the highest may not come first.
        let list = serde_json::json!([
            release("v0.6.1", false, false),
            release("v0.7.0", false, false),
            release("v0.6.2", false, false),
        ]);
        assert_eq!(
            pick(&list, None, STABLE),
            Some(Release {
                version: "0.7.0".into(),
                tag: "v0.7.0".into(),
                page: format!("https://github.com/{REPOSITORY}/releases/tag/v0.7.0"),
            })
        );
        let latest = Build { numbers: [0, 7, 0], ..STABLE };
        assert_eq!(pick(&list, None, latest), None);
    }

    #[test]
    fn a_prerelease_is_not_a_version() {
        // Up to 0.6.0 everything went out as a pre-release, and an option made it count.
        let list = serde_json::json!([
            release("v0.8.0", false, true),
            release("v0.6.1", false, false),
        ]);
        assert_eq!(version(pick(&list, None, STABLE)), Some("0.6.1".into()));
        // The nightly, in the list as a pre-release tagged with a word, is doubly invisible.
        let list = serde_json::json!([nightly("0.9.0-nightly.202612010300")]);
        assert_eq!(pick(&list, None, STABLE), None);
    }

    #[test]
    fn the_stable_channel_never_sees_the_nightly() {
        // `query` does not even ask for it; this is the same thing said from `pick`'s side.
        let list = serde_json::json!([release("v0.6.0", false, false)]);
        assert_eq!(pick(&list, None, STABLE), None);
    }

    #[test]
    fn the_nightly_channel_takes_a_build_made_later() {
        let list = serde_json::json!([release("v0.6.0", false, false)]);
        let tonight = nightly("0.6.0-nightly.202611210300");
        // From the release to the nightly made after it: same version, and semver would say no.
        let found = pick(&list, Some(&tonight), STABLE).unwrap();
        assert_eq!(found.version, "0.6.0-nightly.202611210300");
        assert_eq!(found.tag, NIGHTLY_TAG, "the files are under the word, not under a version");
        // From one nightly to the next.
        assert_eq!(
            version(pick(&list, Some(&tonight), NIGHTLY)),
            Some("0.6.0-nightly.202611210300".into())
        );
        // And not to itself, nor back.
        let same = nightly("0.6.0-nightly.202611200300");
        assert_eq!(pick(&list, Some(&same), NIGHTLY), None);
        let older = nightly("0.6.0-nightly.202611190300");
        assert_eq!(pick(&list, Some(&older), NIGHTLY), None);
    }

    #[test]
    fn a_build_made_by_hand_is_older_than_anything_published() {
        let by_hand = Build { numbers: [0, 6, 0], nightly: false, stamp: 0 };
        let tonight = nightly("0.6.0-nightly.202611210300");
        assert!(pick(&serde_json::json!([]), Some(&tonight), by_hand).is_some());
    }

    #[test]
    fn a_nightly_left_behind_by_a_release_is_not_an_update_to_it() {
        // 0.7.0 went out; the last nightly is still the 0.6.0 one from the day before, and it was
        // "made later" than nothing that matters.
        let released = Build { numbers: [0, 7, 0], nightly: false, stamp: 0 };
        let stale = nightly("0.6.0-nightly.202611210300");
        assert_eq!(pick(&serde_json::json!([]), Some(&stale), released), None);
    }

    #[test]
    fn a_release_and_the_nightly_before_it_do_not_offer_each_other() {
        // What 0.6.0 did: the nightly was made at 14:15, the release was published at 15:42, and
        // its own build had started at 14:13.
        let list = serde_json::json!([published("v0.6.0", false, "2026-11-19T15:42:15Z")]);
        let before = nightly("0.6.0-nightly.202611191415");
        let on_nightly = Build { numbers: [0, 6, 0], nightly: true, stamp: 202611191415 };
        let on_release = Build { numbers: [0, 6, 0], nightly: false, stamp: 202611191413 };
        assert_eq!(version(pick(&list, Some(&before), on_nightly)), Some("0.6.0".into()));
        assert_eq!(pick(&list, Some(&before), on_release), None, "it would go back, and loop");
        // The one made the morning after is an update to the release, and stays where it is.
        let after = nightly("0.6.0-nightly.202611200600");
        assert_eq!(
            version(pick(&list, Some(&after), on_release)),
            Some("0.6.0-nightly.202611200600".into())
        );
        let on_after = Build { numbers: [0, 6, 0], nightly: true, stamp: 202611200600 };
        assert_eq!(pick(&list, Some(&after), on_after), None);
    }

    #[test]
    fn a_nightly_becomes_the_release_it_was_counting_towards() {
        // Published after this build was made: it has everything this build has.
        let list = serde_json::json!([published("v0.6.0", false, "2026-11-22T10:00:00Z")]);
        assert_eq!(version(pick(&list, None, NIGHTLY)), Some("0.6.0".into()));
        // Published before: this nightly came after 0.6.0, and going back would be losing a week.
        let list = serde_json::json!([published("v0.6.0", false, "2026-11-01T10:00:00Z")]);
        assert_eq!(pick(&list, None, NIGHTLY), None);
        // A build from a tag is that release already.
        let list = serde_json::json!([published("v0.6.0", false, "2026-11-22T10:00:00Z")]);
        assert_eq!(pick(&list, None, STABLE), None);
    }

    #[test]
    fn with_both_on_the_table_the_higher_version_wins_and_the_nightly_breaks_the_tie() {
        let tonight = nightly("0.6.0-nightly.202611210300");
        let list = serde_json::json!([release("v0.7.0", false, false)]);
        assert_eq!(version(pick(&list, Some(&tonight), NIGHTLY)), Some("0.7.0".into()));

        let tonight = nightly("0.7.0-nightly.202611250300");
        assert_eq!(
            version(pick(&list, Some(&tonight), NIGHTLY)),
            Some("0.7.0-nightly.202611250300".into())
        );
    }

    #[test]
    fn a_nightly_without_a_name_that_says_when_is_not_offered() {
        // Someone renamed the release by hand: there is no telling whether it is newer.
        let renamed = nightly("Nightly build");
        assert_eq!(pick(&serde_json::json!([]), Some(&renamed), STABLE), None);
        let mut draft = nightly("0.6.0-nightly.202611210300");
        draft["draft"] = true.into();
        assert_eq!(pick(&serde_json::json!([]), Some(&draft), STABLE), None);
    }

    #[test]
    fn ignores_drafts_and_empty_lists() {
        let list = serde_json::json!([
            release("v0.7.0", true, false),
            release("v0.6.0", false, false),
        ]);
        assert_eq!(pick(&list, None, STABLE), None);
        assert_eq!(pick(&serde_json::json!([]), None, STABLE), None);
        assert_eq!(pick(&serde_json::json!({ "message": "rate limited" }), None, STABLE), None);
    }
}
