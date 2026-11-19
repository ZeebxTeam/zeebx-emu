//! Discord Rich Presence: what the emulator is doing, on the player's profile.
//!
//! Discord talks over a local socket (`discord-ipc-N`), and the connection can go missing at any
//! time — Discord closed, opened after the emulator, restarted. That is why what talks to it is a
//! thread of its own: the interface only says **what** to show, and the thread connects,
//! reconnects and resends without stalling a single frame.
//!
//! The images do not go over the socket. Discord accepts an image **key** registered in the
//! application (Developer Portal → Rich Presence → Art Assets) or an `https` URL. A game's cover
//! comes out of the ROM and lives only on disk, so it gets in one of these two ways: exported and
//! registered with the [`cover_key`] key, or published at an address given in the settings.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};

use crate::video::icon::Image;

/// The Zeebx application in the Developer Portal. It is what gives the presence its name and images.
pub const APPLICATION_ID: &str = "1548822136712466476";

/// The key of the emulator's icon among the application's images.
pub const ICON_KEY: &str = "zeebx";

/// How often to try again when Discord is not open.
const RETRY_INTERVAL: Duration = Duration::from_secs(15);

/// The side of an exported cover. The Developer Portal rejects art smaller than 512×512.
pub const COVER_SIDE: usize = 512;

/// The key under which a game's cover is registered in the application.
///
/// Discord only accepts lowercase letters, digits and underscores in keys, and the ClassID is the
/// only thing about the game that does not change from one ROM to another.
pub fn cover_key(clsid: u32) -> String {
    format!("jogo_{clsid:08x}")
}

/// What to show on the profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresenceActivity {
    /// The top line: "Playing Bóia Cross", "In the menu".
    pub details: String,
    /// The large image: a registered key or a URL.
    pub image: String,
    pub image_text: String,
    /// The small icon in the corner of the large image, with its text.
    pub icon_name: Option<(String, String)>,
    /// Since when, in Unix milliseconds. Discord counts the time from here.
    pub start_ms: i64,
}

/// The presence following what the interface shows: the menu, the Z-Wheel or the open game.
///
/// It came out of the egui `App` so the Qt interface shows the same on Discord. It keeps when the
/// current activity started — the profile's clock restarts when the game changes, and only then.
pub struct PresenceTracker {
    presence: Presence,
    /// The ClassID of the open game, or none in the menu, and the instant in Unix milliseconds.
    start: (Option<u32>, i64),
}

impl Default for PresenceTracker {
    fn default() -> Self {
        Self {
            presence: Presence::default(),
            start: (None, now_ms()),
        }
    }
}

impl PresenceTracker {
    /// Tells Discord what is happening. Cheap to call every frame: the presence only sends
    /// something when the text or the image changes.
    ///
    /// `class_id` is the ClassID of the open game; `title`, the name the library shows it with.
    pub fn update(
        &mut self,
        active: bool,
        catalog: &crate::ui::i18n::Catalog,
        class_id: Option<u32>,
        title: Option<String>,
        covers_url: &str,
    ) {
        if self.start.0 != class_id {
            self.start = (class_id, now_ms());
        }
        let activity = active.then(|| activity(catalog, class_id, title, covers_url, self.start.1));
        self.presence.define(activity);
    }

    /// Whether Discord answered and the presence is being shown.
    pub fn connected(&self) -> bool {
        self.presence.connected()
    }
}

/// What to show on the profile: in the menu, in the Z-Wheel, or playing `title`.
fn activity(
    catalog: &crate::ui::i18n::Catalog,
    class_id: Option<u32>,
    title: Option<String>,
    covers_url: &str,
    start_ms: i64,
) -> PresenceActivity {
    let icon_name = ICON_KEY.to_string();
    let menu = |key_id: &str| PresenceActivity {
        details: catalog.get(key_id).to_string(),
        image: icon_name.clone(),
        image_text: "Zeebx".to_string(),
        icon_name: None,
        start_ms,
    };
    let Some(class_id) = class_id else {
        return menu("discord.menu");
    };
    if class_id == crate::session::Z_WHEEL {
        return menu("discord.z_wheel");
    }
    let title = title.unwrap_or_else(|| catalog.get("library.unknown_title").to_string());
    let key_id = cover_key(class_id);
    let model = covers_url.trim();
    let image = match model.is_empty() {
        true => key_id,
        false => model
            .replace("{clsid}", &format!("{class_id:08x}"))
            .replace("{chave}", &key_id),
    };
    PresenceActivity {
        details: catalog.format("discord.playing", &[("name", &title)]),
        image,
        image_text: title,
        icon_name: Some((icon_name, "Zeebx".to_string())),
        start_ms,
    }
}

/// The current instant in Unix milliseconds, which is what Discord wants for the activity start.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

enum Request {
    Show(String, Option<PresenceActivity>),
    Shutdown,
}

/// The presence, on the interface side. It only sends requests when something changes.
pub struct Presence {
    sender: Option<Sender<Request>>,
    last: Option<(String, Option<PresenceActivity>)>,
    connected: Arc<AtomicBool>,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            sender: None,
            last: None,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Presence {
    /// Shows `activity`, or turns the presence off with `None`.
    pub fn define(&mut self, activity: Option<PresenceActivity>) {
        let id = match activity {
            Some(_) => APPLICATION_ID.to_string(),
            None => String::new(),
        };
        let request = (id.clone(), activity);
        if self.last.as_ref() == Some(&request) {
            return;
        }
        self.last = Some(request.clone());
        if id.is_empty() {
            self.shutdown();
            return;
        }
        let sender = self.sender.get_or_insert_with(|| {
            let (sender, receive) = mpsc::channel();
            let connected = Arc::clone(&self.connected);
            let _ = std::thread::Builder::new()
                .name("discord".into())
                .spawn(move || run_loop(receive, connected));
            sender
        });
        if sender.send(Request::Show(request.0, request.1)).is_err() {
            self.sender = None;
        }
    }

    /// Whether Discord answered and the presence is being shown.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    fn shutdown(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Request::Shutdown);
        }
        self.connected.store(false, Ordering::Relaxed);
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The connection thread: keeps the last request and insists until Discord accepts it.
fn run_loop(receive: Receiver<Request>, connected: Arc<AtomicBool>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut desired: Option<(String, Option<PresenceActivity>)> = None;
    let mut pending = false;
    let mut next_attempt = Instant::now();
    loop {
        let wait = match pending {
            true => next_attempt.saturating_duration_since(Instant::now()),
            false => Duration::from_secs(3600),
        };
        match receive.recv_timeout(wait) {
            Ok(Request::Show(id, activity)) => {
                // Switching applications is another connection: the id goes in the handshake.
                if client.as_ref().is_some_and(|c| c.client_id != id) {
                    close(&mut client, &connected);
                }
                desired = Some((id, activity));
                pending = true;
                next_attempt = Instant::now();
            }
            Ok(Request::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                if let Some(c) = client.as_mut() {
                    let _ = c.clear_activity();
                }
                close(&mut client, &connected);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        if !pending || Instant::now() < next_attempt {
            continue;
        }
        let Some((id, activity)) = &desired else {
            pending = false;
            continue;
        };
        if client.is_none() {
            let mut create = DiscordIpcClient::new(id);
            if create.connect().is_ok() {
                client = Some(create);
            }
        }
        let sent = client.as_mut().is_some_and(|c| {
            match activity {
                Some(activity) => c.set_activity(build(activity)),
                None => c.clear_activity(),
            }
            .is_ok()
        });
        if sent {
            connected.store(true, Ordering::Relaxed);
            pending = false;
        } else {
            // Connection dropped or never opened: start from scratch on the next attempt.
            close(&mut client, &connected);
            next_attempt = Instant::now() + RETRY_INTERVAL;
        }
    }
}

fn close(client: &mut Option<DiscordIpcClient>, connected: &AtomicBool) {
    if let Some(mut c) = client.take() {
        let _ = c.close();
    }
    connected.store(false, Ordering::Relaxed);
}

fn build(activity: &PresenceActivity) -> Activity<'_> {
    let mut assets = Assets::new()
        .large_image(activity.image.as_str())
        .large_text(activity.image_text.as_str());
    if let Some((image, text)) = &activity.icon_name {
        assets = assets.small_image(image.as_str()).small_text(text.as_str());
    }
    Activity::new()
        .activity_type(ActivityType::Playing)
        .details(activity.details.as_str())
        .assets(assets)
        .timestamps(Timestamps::new().start(activity.start_ms))
}

/// The cover in the square the Developer Portal accepts: the whole image, centered, uncropped.
///
/// Upscaling is nearest-neighbor — the cover in a `.mif` is a few dozen pixels across, and
/// blurring that up to 512 looks worse than big pixels.
pub fn square_cover(image: &Image) -> Image {
    let side = COVER_SIDE;
    let mut rgba = vec![0u8; side * side * 4];
    if image.width == 0 || image.height == 0 {
        return Image { width: side, height: side, rgba };
    }
    let scale = side as f64 / image.width.max(image.height) as f64;
    let width = ((image.width as f64 * scale).round() as usize).clamp(1, side);
    let height = ((image.height as f64 * scale).round() as usize).clamp(1, side);
    let (x0, y0) = ((side - width) / 2, (side - height) / 2);
    for y in 0..height {
        let sy = (y * image.height / height).min(image.height - 1);
        for x in 0..width {
            let sx = (x * image.width / width).min(image.width - 1);
            let from = (sy * image.width + sx) * 4;
            let to = ((y0 + y) * side + x0 + x) * 4;
            rgba[to..to + 4].copy_from_slice(&image.rgba[from..from + 4]);
        }
    }
    Image { width: side, height: side, rgba }
}

/// The image as PNG, to save and register in the application.
pub fn png(image: &Image) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut encoder = png::Encoder::new(
        std::io::Cursor::new(&mut output),
        image.width as u32,
        image.height as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&image.rgba).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shows "In the menu" on the open Discord for ten seconds. Needs Discord running:
    /// `cargo test --release real_presence -- --ignored`.
    #[test]
    #[ignore]
    fn real_presence() {
        let mut client = DiscordIpcClient::new(APPLICATION_ID);
        client.connect().expect("is Discord open?");
        let activity = PresenceActivity {
            details: "In the menu".into(),
            image: ICON_KEY.into(),
            image_text: "Zeebx".into(),
            icon_name: None,
            start_ms: 0,
        };
        client.set_activity(build(&activity)).expect("set_activity");
        std::thread::sleep(Duration::from_secs(10));
        let _ = client.clear_activity();
        let _ = client.close();
    }

    #[test]
    fn the_cover_key_only_has_what_discord_accepts() {
        let key_id = cover_key(0x0108FF13);
        assert_eq!(key_id, "jogo_0108ff13");
        assert!(key_id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'));
    }

    #[test]
    fn the_cover_becomes_a_centered_512_square_without_cropping() {
        // 2×1: red on the left, green on the right.
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        let cover = square_cover(&image);
        assert_eq!((cover.width, cover.height), (512, 512));
        let pixel = |x: usize, y: usize| &cover.rgba[(y * 512 + x) * 4..(y * 512 + x) * 4 + 4];
        // A 256-tall band in the middle; above it, transparent.
        assert_eq!(pixel(10, 10), &[0, 0, 0, 0]);
        assert_eq!(pixel(10, 256), &[255, 0, 0, 255]);
        assert_eq!(pixel(500, 256), &[0, 255, 0, 255]);
    }
}
