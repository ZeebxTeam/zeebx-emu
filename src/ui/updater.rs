//! Looks for a new version in the GitHub releases.
//!
//! The question is the list of releases, and the answer arrives on a thread, so the window does
//! not wait for the network. The tag can come as `0.1.0` or `v0.1.0`.
//!
//! **It is not `releases/latest`.** GitHub leaves pre-releases out of `latest`, and every Zeebx
//! release came out that way up to 0.4.1: the answer was 404, which became "up to date", and nobody
//! was ever told about a new version. From the list comes the highest version that is not a draft
//! — and that is not a pre-release, if the user turned the option off.
//!
//! With the `atualizador` feature, [`install`] downloads the new version and replaces this copy
//! with it, in the packages that know how to be replaced (see [`Installation`]). What is
//! downloaded is described in a manifest published in the release (`latest-qt.json`), with the
//! minisign signature of each package; the public key is in [`PUBLIC_KEY`], and a package that
//! does not check out is not installed.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Where the releases are published.
pub const REPOSITORY: &str = "ZeebxTeam/zeebx-emu";

/// The version of this binary.
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A release newer than this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the `v`: `0.2.0`.
    pub version: String,
    /// As it is on GitHub, with or without the `v`: it is what builds the address of the release
    /// files.
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

/// Starts the search on a thread. The answer arrives through the channel, once. `prereleases`
/// says whether a release marked as pre-release counts.
pub fn search(prereleases: bool) -> Receiver<Response> {
    let (sender, receive) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("atualizacao".into())
        .spawn(move || {
            let response = match query(prereleases) {
                Ok(Some(release)) => Response::NewVersion(release),
                Ok(None) => Response::UpToDate,
                Err(error) => Response::Failed(error),
            };
            let _ = sender.send(response);
        });
    receive
}

fn query(prereleases: bool) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=20");
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
    // With no release at all the list comes back empty, with 200. A 404 here is a wrong
    // repository, and has to show up as a failure, not as "up to date".
    let code = response.status().as_u16();
    if code != 200 {
        return Err(format!("GitHub answered {code}"));
    }
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok(pick(&json, CURRENT_VERSION, prereleases))
}

/// The highest release in the list, if it is newer than `current`. A draft never counts — the API
/// only shows them to whoever has write access, but the filter costs nothing.
fn pick(json: &serde_json::Value, current: &str, prereleases: bool) -> Option<Release> {
    let (tag, release) = json
        .as_array()?
        .iter()
        .filter(|release| release["draft"].as_bool() != Some(true))
        .filter(|release| prereleases || release["prerelease"].as_bool() != Some(true))
        .filter_map(|release| Some((release["tag_name"].as_str()?.trim(), release)))
        .max_by_key(|(tag, _)| numbers(tag))?;
    let version = strip_v(tag);
    if !is_newer(version, current) {
        return None;
    }
    let page = release["html_url"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORY}/releases/tag/{tag}"));
    Some(Release { version: version.to_string(), tag: tag.to_string(), page })
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

/// Whether `candidate` is newer than `current`.
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
    let current = CURRENT_VERSION.parse().map_err(|e| format!("{e}"))?;
    // No `timeout`: in `reqwest` it applies to the whole response, and a 100 MB AppImage on a slow
    // connection would go past any reasonable number.
    let updater = UpdaterBuilder::new(current, config).build().map_err(|e| e.to_string())?;
    let Some(package) = updater.check().map_err(|e| e.to_string())? else {
        return Err("the release manifest does not carry a newer version".into());
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
        println!("{:?}", query(true));
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

    #[test]
    fn picks_the_highest_in_the_list_even_in_prerelease() {
        // The API order is by date, not by version: the highest may not come first.
        let list = serde_json::json!([
            release("v0.4.0", false, true),
            release("v0.4.1", false, true),
            release("v0.3.0", false, true),
        ]);
        assert_eq!(
            pick(&list, "0.4.0", true),
            Some(Release {
                version: "0.4.1".into(),
                tag: "v0.4.1".into(),
                page: format!("https://github.com/{REPOSITORY}/releases/tag/v0.4.1"),
            })
        );
        assert_eq!(pick(&list, "0.4.1", true), None);
    }

    #[test]
    fn without_prerelease_the_latest_stable_stays() {
        let list = serde_json::json!([
            release("v0.5.0", false, true),
            release("v0.4.2", false, false),
        ]);
        assert_eq!(pick(&list, "0.4.1", false).map(|l| l.version), Some("0.4.2".into()));
        assert_eq!(pick(&list, "0.4.1", true).map(|l| l.version), Some("0.5.0".into()));
        assert_eq!(pick(&list, "0.4.2", false), None);
    }

    #[test]
    fn ignores_drafts_and_empty_lists() {
        let list = serde_json::json!([
            release("v0.5.0", true, false),
            release("v0.4.1", false, true),
        ]);
        assert_eq!(pick(&list, "0.4.1", true), None);
        assert_eq!(pick(&serde_json::json!([]), "0.1.0", true), None);
    }
}
