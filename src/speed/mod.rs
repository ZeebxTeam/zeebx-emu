//! The game's pace against the wall clock: the frame limit and the draw skip.
//!
//! The game's clock is virtual (see `docs/implementation/04-time.md`): it advances with the work
//! the game does, and what ties it to the wall clock is the session, comparing the two on every
//! pass. This module keeps **the decisions** of that comparison — how fast the game may run and
//! which frames stop being drawn —, separate from the session so they apply the same in every
//! frontend and can be tested without any game. The overall design is in
//! `docs/implementation/24-speed.md`.

use serde::{Deserialize, Serialize};

pub mod rewind;
pub mod turbo;

/// The game's speed ceiling.
///
/// **60 does not mean "call the game 60 times per second"**: it means "do not let the virtual
/// clock get past the real clock", which is the throttle that keeps Crash, Zeebo Extreme and NFS
/// from running above the console's speed. The type was born in the Libretro core, and lives here
/// so that desktop, headless and Android offer the same three choices with the same meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FpsLimit {
    /// Console speed and normal presentation.
    #[default]
    #[serde(rename = "60")]
    Sixty,
    /// Console speed, but only one out of every two frames is drawn.
    ///
    /// **It does not slow down the logic**: the throttle stays at 1x, and the frame that is not
    /// drawn leaves the screen with the previous one. Changing the virtual vsync period would be
    /// the opposite — the game would advance 33 ms per frame and could speed up.
    #[serde(rename = "30")]
    Thirty,
    /// Unthrottled: the game runs as fast as the host can take. It is the boost for whoever wants
    /// NFS faster.
    #[serde(rename = "desligado")]
    Off,
}

impl FpsLimit {
    pub const ALL: [Self; 3] = [Self::Sixty, Self::Thirty, Self::Off];

    /// Reads an option's text. Accepts the Libretro core's (`desligado`) and the headless
    /// `config.ini`'s (`off`), which is English because whoever reads it is the foreign user.
    /// `None` for what is none of the three, and the caller keeps what it had.
    pub fn from_text(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "60" => Some(Self::Sixty),
            "30" => Some(Self::Thirty),
            "desligado" | "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// The label's key in the language catalog.
    pub fn key_id(self) -> &'static str {
        match self {
            Self::Sixty => "speed.fps_limit.60",
            Self::Thirty => "speed.fps_limit.30",
            Self::Off => "common.off",
        }
    }

    /// Whether the virtual clock is held against the real one.
    pub fn limit_speed(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// The draw skip policy.
///
/// Skipping **does not change the game's speed**: the logic runs in full, and only the 3D drawing
/// and the screen clear of that frame stop happening — see
/// [`crate::machine::Machine::set_skip_draw`]. A game that reads the screen back (`glReadPixels`)
/// never skips: handing the previous screen to whoever reads it would change what it decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frameskip {
    #[default]
    #[serde(rename = "desligado")]
    Off,
    /// Skips `n` frames out of every `n + 1` — `1` is half, `2` is a third, and so on.
    #[serde(rename = "fixo")]
    Fixed(u8),
    /// Skips only what would not reach the screen anyway. In the Libretro core the frontend's
    /// audio buffer warning decides; outside it, the lag the session already measures.
    #[serde(rename = "automatico")]
    Auto,
}

impl Frameskip {
    /// The largest `n` of [`Frameskip::Fixed`]. Six has been the core's ceiling since the option
    /// exists: one frame drawn out of every seven is already the image at 8 per second.
    pub const MAX_FIXED: u8 = 6;

    /// All the choices, in menu order: off, auto, and the fixed ones.
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Off, Self::Auto];
        all.extend((1..=Self::MAX_FIXED).map(Self::Fixed));
        all
    }

    /// Reads an option's text, like [`FpsLimit::from_text`]: `desligado`/`off`,
    /// `automatico`/`auto`, or a number from 1 to [`Frameskip::MAX_FIXED`].
    pub fn from_text(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "desligado" | "off" => Some(Self::Off),
            "automatico" | "auto" => Some(Self::Auto),
            other => other
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=Self::MAX_FIXED).contains(n))
                .map(Self::Fixed),
        }
    }

    /// The value within range: a hand-edited `settings.json` may bring `{"fixo": 40}`.
    pub fn normalized(self) -> Self {
        match self {
            Self::Fixed(0) => Self::Off,
            Self::Fixed(n) => Self::Fixed(n.min(Self::MAX_FIXED)),
            other => other,
        }
    }
}

/// How a session pass advances against the wall clock.
///
/// It is what the frontend hands the session on every pass, built from what the user chose. It is
/// a single value because the three things intersect: the limit decides the ratio and the
/// half-rate presentation, and the auto skip depends on the ratio existing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pace {
    /// How many times the console's speed. `None` is unthrottled.
    pub ratio: Option<f32>,
    pub frameskip: Frameskip,
    /// Draw only one out of every two frames — [`FpsLimit::Thirty`].
    pub half_rate_present: bool,
    /// Silence the sound while this pace applies: the fast-forward "no sound", and its unlimited
    /// mode.
    pub mute_sound: bool,
}

impl Pace {
    /// The console's speed, drawing everything: what `speed_limit` on always was.
    pub const CONSOLE: Self = Self {
        ratio: Some(1.0),
        frameskip: Frameskip::Off,
        half_rate_present: false,
        mute_sound: false,
    };

    /// Unthrottled, drawing everything: what `speed_limit` off always was.
    pub const UNTHROTTLED: Self = Self {
        ratio: None,
        frameskip: Frameskip::Off,
        half_rate_present: false,
        mute_sound: false,
    };

    /// The pace of the user's choices.
    pub fn from_choices(limit: FpsLimit, frameskip: Frameskip) -> Self {
        Self {
            ratio: limit.limit_speed().then_some(1.0),
            frameskip: frameskip.normalized(),
            half_rate_present: limit == FpsLimit::Thirty,
            mute_sound: false,
        }
    }

    /// This pace with fast-forward on top.
    ///
    /// The ratio becomes the fast-forward's, and the draw skip becomes **at least** auto: at 10x
    /// the pass runs ten frames to show one, and drawing the nine nobody sees is what would keep
    /// the host from getting there. A chosen fixed skip still applies. The half-rate presentation
    /// goes away: auto already skips more than it does.
    pub fn fast_forwarded(self, fast_forward: &FastForward) -> Self {
        Self {
            ratio: fast_forward.ratio(),
            frameskip: match self.frameskip {
                Frameskip::Off => Frameskip::Auto,
                other => other,
            },
            half_rate_present: false,
            mute_sound: fast_forward.mutes(),
        }
    }
}

/// How a hold shortcut behaves: only while pressed, or one press turns it on and another off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutMode {
    #[default]
    #[serde(rename = "segurar")]
    Hold,
    #[serde(rename = "alternar")]
    Toggle,
}

impl ShortcutMode {
    pub const ALL: [Self; 2] = [Self::Hold, Self::Toggle];

    pub fn key_id(self) -> &'static str {
        match self {
            Self::Hold => "speed.shortcut_mode.hold",
            Self::Toggle => "speed.shortcut_mode.toggle",
        }
    }
}

/// Fast-forward, as the user chose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FastForward {
    /// How many times the console's speed, from 2 to 10. **Zero is unlimited**: as much as the
    /// host can take, always muted.
    #[serde(rename = "proporcao")]
    pub ratio: u8,
    #[serde(rename = "modo")]
    pub mode: ShortcutMode,
    /// Silence the sound while fast-forwarding. Off, the sound speeds up along with the game.
    #[serde(rename = "sem_som")]
    pub mute: bool,
}

impl Default for FastForward {
    fn default() -> Self {
        Self {
            ratio: 3,
            mode: ShortcutMode::Hold,
            mute: false,
        }
    }
}

impl FastForward {
    /// The smallest and largest ratios offered. Ten is the ceiling of RALibretro, which served as
    /// the model.
    pub const MIN: u8 = 2;
    pub const MAX: u8 = 10;

    /// The choices, in menu order: 2x to 10x, and unlimited at the end.
    pub fn choices() -> Vec<u8> {
        let mut choices: Vec<u8> = (Self::MIN..=Self::MAX).collect();
        choices.push(0);
        choices
    }

    /// The ratio for the session. `None` is unlimited.
    pub fn ratio(&self) -> Option<f32> {
        match self.ratio {
            0 => None,
            n => Some(f32::from(n.clamp(Self::MIN, Self::MAX))),
        }
    }

    /// Whether the sound stays silent: by choice, or because unlimited has no ratio for it to
    /// follow.
    ///
    /// **Unlimited is always muted.** The real speed varies every instant — from light frame to
    /// heavy frame —, and sound that followed it would change pitch all the time.
    pub fn mutes(&self) -> bool {
        self.mute || self.ratio == 0
    }
}

/// A shortcut that is held or toggled, read once per pass.
///
/// It keeps what is needed to see the **edge** — the instant the button went down —, which is
/// what toggle mode counts. Holding a toggle shortcut does not switch it on and off every pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggle {
    enabled: bool,
    before: bool,
}

impl Toggle {
    /// Reads this pass's button and says whether the function is active.
    pub fn update(&mut self, pressed: bool, mode: ShortcutMode) -> bool {
        let went_down = pressed && !self.before;
        self.before = pressed;
        match mode {
            ShortcutMode::Hold => {
                self.enabled = false;
                pressed
            }
            ShortcutMode::Toggle => {
                if went_down {
                    self.enabled = !self.enabled;
                }
                self.enabled
            }
        }
    }

    /// Whether toggle mode is on now.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Back to the start: off, and the button taken as released. Closing the game must not leave
    /// a toggle on for the next one.
    pub fn disable(&mut self) {
        *self = Self::default();
    }
}

/// Which frames stop being drawn, decided **per frame**.
///
/// A counter that only moved when the user touched the option would skip — or not — forever from
/// the first reading, instead of one frame every `n + 1`. That is why the state lives here and
/// advances on every [`SkipCounter::decide`], which the caller does once per frame, before it
/// starts.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkipCounter {
    fixed_point: u32,
    half: u32,
}

/// Why a frame goes undrawn. The two reasons are kept separate because the Libretro core treats
/// the half-rate presentation one differently: it **duplicates** the previous frame for the
/// frontend, instead of delivering a screen that did not change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Skip {
    pub frameskip: bool,
    pub half: bool,
}

impl Skip {
    pub fn any(self) -> bool {
        self.frameskip || self.half
    }
}

impl SkipCounter {
    /// Decides the frame about to start.
    ///
    /// `surplus` is what [`Frameskip::Auto`] consults: whether this frame is **not** going to the
    /// screen anyway — because the pass will still run another one after it, or because the
    /// frontend warned that the audio is about to overflow. The fixed skip and the half-rate
    /// presentation do not look at it: they always apply.
    pub fn decide(&mut self, pace: &Pace, surplus: bool) -> Skip {
        let frameskip = match pace.frameskip.normalized() {
            Frameskip::Off => {
                self.fixed_point = 0;
                false
            }
            Frameskip::Fixed(n) => {
                let skip = self.fixed_point != 0;
                self.fixed_point = (self.fixed_point + 1) % (u32::from(n) + 1);
                skip
            }
            Frameskip::Auto => surplus,
        };
        let half = match pace.half_rate_present {
            true => {
                let skip = self.half != 0;
                self.half = (self.half + 1) % 2;
                skip
            }
            false => {
                self.half = 0;
                false
            }
        };
        Skip { frameskip, half }
    }
}

/// What the window writes over the game during fast-forward.
///
/// `target` is the requested ratio (`None` is unlimited) and `reached` the measured one, in
/// multiples of the console's speed. When the host does not reach 95% of the request, the
/// measured value appears beside it, so someone who chose 10x knows they are at 4x; unlimited, the
/// measured value is the label itself.
pub fn fast_forward_label(target: Option<f32>, reached: f32, decimal_comma: bool) -> String {
    let times = |value: f32| {
        let text = match value.fract().abs() < 0.05 {
            true => format!("{:.0}x", value.round()),
            false => format!("{value:.1}x"),
        };
        match decimal_comma {
            true => text.replace('.', ","),
            false => text,
        }
    };
    match target {
        Some(target) if reached >= target * 0.95 => format!("▶▶ {}", times(target)),
        Some(target) => format!("▶▶ {} ({})", times(target), times(reached)),
        None => format!("▶▶ {}", times(reached)),
    }
}

/// How far the game is ahead (positive) or behind (negative), in virtual milliseconds.
///
/// `game_ms` is how much the virtual clock advanced since the anchor, and `real_ms` how much the
/// wall clock advanced. With ratio 3, the game should have advanced three times the real one;
/// without a ratio there is no target, and the game is never ahead or behind.
pub fn lead_ms(game_ms: u64, real_ms: u64, ratio: Option<f32>) -> i64 {
    let Some(ratio) = ratio else {
        return 0;
    };
    let meta = (real_ms as f64 * f64::from(ratio)).round() as i64;
    game_ms as i64 - meta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_option_text_becomes_the_right_value() {
        assert_eq!(FpsLimit::from_text("60"), Some(FpsLimit::Sixty));
        assert_eq!(FpsLimit::from_text(" 30 "), Some(FpsLimit::Thirty));
        assert_eq!(FpsLimit::from_text("desligado"), Some(FpsLimit::Off));
        assert_eq!(FpsLimit::from_text("OFF"), Some(FpsLimit::Off));
        assert_eq!(FpsLimit::from_text("120"), None);

        assert_eq!(Frameskip::from_text("desligado"), Some(Frameskip::Off));
        assert_eq!(Frameskip::from_text("off"), Some(Frameskip::Off));
        assert_eq!(Frameskip::from_text("automatico"), Some(Frameskip::Auto));
        assert_eq!(Frameskip::from_text("auto"), Some(Frameskip::Auto));
        assert_eq!(Frameskip::from_text("1"), Some(Frameskip::Fixed(1)));
        assert_eq!(Frameskip::from_text("6"), Some(Frameskip::Fixed(6)));
        assert_eq!(Frameskip::from_text("0"), None);
        assert_eq!(Frameskip::from_text("7"), None);
        assert_eq!(Frameskip::from_text("4294967295"), None);
        assert_eq!(Frameskip::from_text(""), None);
    }

    /// `settings.json` stores the values with names someone can read and edit.
    #[test]
    fn the_file_stores_values_by_name() {
        assert_eq!(serde_json::to_string(&FpsLimit::Thirty).unwrap(), r#""30""#);
        assert_eq!(serde_json::to_string(&Frameskip::Fixed(2)).unwrap(), r#"{"fixo":2}"#);
        assert_eq!(serde_json::to_string(&Frameskip::Auto).unwrap(), r#""automatico""#);
        let loaded: Frameskip = serde_json::from_str(r#"{"fixo":40}"#).unwrap();
        assert_eq!(loaded.normalized(), Frameskip::Fixed(Frameskip::MAX_FIXED));
        let zero: Frameskip = serde_json::from_str(r#"{"fixo":0}"#).unwrap();
        assert_eq!(zero.normalized(), Frameskip::Off);
    }

    #[test]
    fn the_pace_of_each_choice() {
        let r = Pace::from_choices(FpsLimit::Sixty, Frameskip::Off);
        assert_eq!(r, Pace::CONSOLE);
        let r = Pace::from_choices(FpsLimit::Off, Frameskip::Off);
        assert_eq!(r, Pace::UNTHROTTLED);
        let r = Pace::from_choices(FpsLimit::Thirty, Frameskip::Fixed(2));
        assert_eq!(r.ratio, Some(1.0));
        assert!(r.half_rate_present);
        assert_eq!(r.frameskip, Frameskip::Fixed(2));
    }

    #[test]
    fn fast_forward_over_the_pace() {
        let fast_forward = FastForward::default();
        let r = Pace::from_choices(FpsLimit::Thirty, Frameskip::Off).fast_forwarded(&fast_forward);
        assert_eq!(r.ratio, Some(3.0));
        assert_eq!(r.frameskip, Frameskip::Auto, "fast-forward skips at least the surplus");
        assert!(!r.half_rate_present);
        assert!(!r.mute_sound, "the sound speeds up along by default");

        let fixed_point = Pace::from_choices(FpsLimit::Sixty, Frameskip::Fixed(3));
        assert_eq!(fixed_point.fast_forwarded(&fast_forward).frameskip, Frameskip::Fixed(3));

        let unlimited = FastForward {
            ratio: 0,
            ..FastForward::default()
        };
        let r = Pace::CONSOLE.fast_forwarded(&unlimited);
        assert_eq!(r.ratio, None);
        assert!(r.mute_sound, "unlimited is always muted");

        let muted = FastForward {
            mute: true,
            ..FastForward::default()
        };
        assert!(Pace::CONSOLE.fast_forwarded(&muted).mute_sound);
    }

    #[test]
    fn the_fast_forward_ratios() {
        assert_eq!(FastForward::choices(), [2, 3, 4, 5, 6, 7, 8, 9, 10, 0]);
        let com = |ratio| FastForward {
            ratio,
            ..FastForward::default()
        };
        assert_eq!(com(10).ratio(), Some(10.0));
        // A hand-edited file does not go past the range.
        assert_eq!(com(40).ratio(), Some(10.0));
        assert_eq!(com(1).ratio(), Some(2.0));
        assert_eq!(com(0).ratio(), None);
    }

    /// Hold applies while pressed; toggle switches only on the press edge, and holding does not
    /// blink.
    #[test]
    fn the_switch_holds_and_toggles() {
        let mut i = Toggle::default();
        let hold: Vec<bool> = [false, true, true, false]
            .into_iter()
            .map(|b| i.update(b, ShortcutMode::Hold))
            .collect();
        assert_eq!(hold, [false, true, true, false]);

        let mut i = Toggle::default();
        let toggle: Vec<bool> = [true, true, true, false, false, true, false]
            .into_iter()
            .map(|b| i.update(b, ShortcutMode::Toggle))
            .collect();
        assert_eq!(toggle, [true, true, true, true, true, false, false]);

        i.update(true, ShortcutMode::Toggle);
        assert!(i.enabled());
        i.disable();
        assert!(!i.enabled());
    }

    /// Fixed `n` draws one frame and skips `n`, and starts over: the decision is per frame.
    #[test]
    fn fixed_skip_draws_one_every_n_plus_one() {
        let pace = Pace {
            frameskip: Frameskip::Fixed(2),
            ..Pace::CONSOLE
        };
        let mut counter = SkipCounter::default();
        let skips: Vec<bool> = (0..7).map(|_| counter.decide(&pace, false).frameskip).collect();
        assert_eq!(skips, [false, true, true, false, true, true, false]);
    }

    /// 30 FPS draws one yes, one no — and the skip is of the "half" reason, which the core
    /// duplicates.
    #[test]
    fn half_presentation_alternates() {
        let pace = Pace::from_choices(FpsLimit::Thirty, Frameskip::Off);
        let mut counter = SkipCounter::default();
        let skips: Vec<Skip> = (0..4).map(|_| counter.decide(&pace, true)).collect();
        assert_eq!(
            skips.iter().map(|p| p.half).collect::<Vec<_>>(),
            [false, true, false, true]
        );
        assert!(skips.iter().all(|p| !p.frameskip));
    }

    /// Auto only skips the surplus, and turning it off resets the fixed counter.
    #[test]
    fn auto_follows_the_headroom_and_turning_off_resets() {
        let auto = Pace {
            frameskip: Frameskip::Auto,
            ..Pace::CONSOLE
        };
        let mut counter = SkipCounter::default();
        assert!(counter.decide(&auto, true).frameskip);
        assert!(!counter.decide(&auto, false).frameskip);

        let fixed_point = Pace {
            frameskip: Frameskip::Fixed(1),
            ..Pace::CONSOLE
        };
        counter.decide(&fixed_point, false);
        counter.decide(&Pace::CONSOLE, false);
        // It went back to drawing the first one: the counter restarted.
        assert!(!counter.decide(&fixed_point, false).frameskip);
    }

    #[test]
    fn the_fast_forward_label() {
        assert_eq!(fast_forward_label(Some(3.0), 2.98, true), "▶▶ 3x");
        assert_eq!(fast_forward_label(Some(10.0), 4.2, true), "▶▶ 10x (4,2x)");
        assert_eq!(fast_forward_label(Some(10.0), 4.2, false), "▶▶ 10x (4.2x)");
        assert_eq!(fast_forward_label(None, 6.0, true), "▶▶ 6x");
    }

    #[test]
    fn the_lead_respects_the_ratio() {
        // One real second, one second of game: on time.
        assert_eq!(lead_ms(1000, 1000, Some(1.0)), 0);
        // At 3x, one real second asks for three of game: with two, it is one behind.
        assert_eq!(lead_ms(2000, 1000, Some(3.0)), -1000);
        // At 3x, four of game in one real is one ahead.
        assert_eq!(lead_ms(4000, 1000, Some(3.0)), 1000);
        // Unthrottled there is no target.
        assert_eq!(lead_ms(9000, 1, None), 0);
    }
}
