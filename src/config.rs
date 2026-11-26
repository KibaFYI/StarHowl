//! Settings: what the user picked, saved as TOML in %APPDATA%\Starhowl\settings.toml.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Declares a "pick one" settings enum with display labels.
macro_rules! choice_enum {
    ($name:ident { $($var:ident => $label:expr),+ $(,)? } default $def:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
        pub enum $name { $($var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn label(self) -> &'static str { match self { $($name::$var => $label),+ } }
        }
        impl Default for $name { fn default() -> Self { $name::$def } }
    };
}

choice_enum!(Scaling {
    Fit => "Fit (keep shape)",
    Integer => "Pixel-perfect (whole steps)",
    Native => "Native 1x",
    Half => "1/2x (lossless)",
    Third => "1/3x (lossless)",
    Double => "2x",
    Stretch => "Stretch to window",
} default Fit);

choice_enum!(Filter {
    Sharp => "Sharp",
    Smooth => "Smooth",
    Pixel => "Pixel (nearest)",
} default Sharp);

choice_enum!(AlignX {
    Left => "Left",
    Center => "Center",
    Right => "Right",
} default Center);

choice_enum!(AlignY {
    Top => "Top",
    Middle => "Middle",
    Bottom => "Bottom",
} default Middle);

choice_enum!(ColorRange {
    Auto => "Auto",
    Limited => "Limited (16-235)",
    Full => "Full (0-255)",
} default Auto);

choice_enum!(ColorMatrix {
    Auto => "Auto",
    Bt601 => "BT.601 (SD)",
    Bt709 => "BT.709 (HD)",
} default Auto);

choice_enum!(Fill {
    Black => "Black",
    White => "White",
    Galaxy => "Galaxy",
    Image => "Custom image",
} default Galaxy);

choice_enum!(Zone {
    Background => "Same as background",
    Black => "Black",
    White => "White",
    Galaxy => "Galaxy",
    Image => "Custom image",
} default Background);

choice_enum!(FrameColor {
    None => "None",
    Black => "Black",
    White => "White",
    Purple => "Galaxy purple",
} default None);

choice_enum!(Stars {
    Minimal => "Minimal",
    Normal => "Normal",
    Dense => "Dense",
} default Minimal);

choice_enum!(ImageFit {
    Cover => "Fill (crop)",
    Stretch => "Stretch",
    Center => "Center (1:1)",
} default Cover);

choice_enum!(DiscordKind {
    Playing => "Playing",
    Watching => "Watching",
    Listening => "Listening to",
    Competing => "Competing in",
} default Playing);

impl DiscordKind {
    /// Discord's activity type number.
    pub fn id(self) -> u8 {
        match self {
            DiscordKind::Playing => 0,
            DiscordKind::Listening => 2,
            DiscordKind::Watching => 3,
            DiscordKind::Competing => 5,
        }
    }
}

/// Starhowl's Discord application ("StarHowl").
pub const DISCORD_APP_ID: &str = "1575306816475238602";

/// The galaxy wolf picture Discord shows when no other image link is set
/// (served from the project's GitHub repository).
pub const DISCORD_ART_URL: &str = "https://raw.githubusercontent.com/KibaFYI/StarHowl/main/assets/discord-large.png";

/// The small purple play badge shown in the corner of the big picture.
/// (`?v=2` makes Discord fetch it fresh: its image cache remembered the link
/// from before the file existed.)
pub const DISCORD_BADGE_URL: &str = "https://raw.githubusercontent.com/KibaFYI/StarHowl/main/assets/discord-small.png?v=2";

/// Discord shows this as "Watching Stream With StarHowl".
pub const DEFAULT_TITLE: &str = "Stream With StarHowl";

/// Settings from v0.2.4 and older stored "Capturing <this>" as just <this>,
/// and used "Streaming With Starhowl" (Playing) as the title. Carry them over.
fn migrate_discord(c: &mut Config, raw: &toml::Table) {
    if !raw.contains_key("discord_details") {
        if let Some(what) = raw.get("discord_capturing").and_then(|v| v.as_str()) {
            let line = format!("Capturing {}", what.trim()).trim().to_string();
            c.discord_details = line.clone();
            if !raw.contains_key("discord_large_text") {
                c.discord_large_text = line;
            }
        }
    }
    if c.discord_title == "Streaming With Starhowl" {
        c.discord_title = DEFAULT_TITLE.into();
        if c.discord_kind == DiscordKind::Playing {
            c.discord_kind = DiscordKind::Watching;
        }
    }
}

impl Zone {
    pub fn resolve(self, bg: Fill) -> Fill {
        match self {
            Zone::Background => bg,
            Zone::Black => Fill::Black,
            Zone::White => Fill::White,
            Zone::Galaxy => Fill::Galaxy,
            Zone::Image => Fill::Image,
        }
    }
}

impl Stars {
    /// Chance that a 22px sky cell holds a star.
    pub fn density(self) -> f32 {
        match self {
            Stars::Minimal => 0.035,
            Stars::Normal => 0.07,
            Stars::Dense => 0.14,
        }
    }
}

/// A keyboard shortcut. Stored in the config as text like "Alt+S".
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Hotkey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub vk: u16,
}

impl Hotkey {
    pub const fn new(ctrl: bool, shift: bool, alt: bool, vk: u16) -> Self {
        Self { ctrl, shift, alt, vk }
    }

    pub fn parse(s: &str) -> Option<Hotkey> {
        let mut h = Hotkey::new(false, false, false, 0);
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => h.ctrl = true,
                "shift" => h.shift = true,
                "alt" => h.alt = true,
                _ => h.vk = parse_key(part)?,
            }
        }
        // A lone "+" key would be split away; treat "Ctrl++" style as unsupported.
        (h.vk != 0).then_some(h)
    }
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        f.write_str(&key_name(self.vk))
    }
}

impl From<Hotkey> for String {
    fn from(h: Hotkey) -> String {
        h.to_string()
    }
}

impl TryFrom<String> for Hotkey {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        Hotkey::parse(&s).ok_or_else(|| format!("unknown shortcut '{s}'"))
    }
}

const NAMED_KEYS: &[(u16, &str)] = &[
    (0x20, "Space"),
    (0x0D, "Enter"),
    (0x09, "Tab"),
    (0x08, "Backspace"),
    (0x2D, "Insert"),
    (0x2E, "Delete"),
    (0x24, "Home"),
    (0x23, "End"),
    (0x21, "PageUp"),
    (0x22, "PageDown"),
    (0x25, "Left"),
    (0x26, "Up"),
    (0x27, "Right"),
    (0x28, "Down"),
    (0x13, "Pause"),
    (0x91, "ScrollLock"),
    (0x2C, "PrintScreen"),
    (0xC0, "`"),
    (0xBD, "-"),
    (0xBB, "="),
    (0xDB, "["),
    (0xDD, "]"),
    (0xDC, "\\"),
    (0xBA, ";"),
    (0xDE, "'"),
    (0xBC, ","),
    (0xBE, "."),
    (0xBF, "/"),
    (0x6A, "Num*"),
    (0x6B, "NumPlus"),
    (0x6D, "Num-"),
    (0x6E, "Num."),
    (0x6F, "Num/"),
];

pub fn key_name(vk: u16) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        0x60..=0x69 => format!("Num{}", vk - 0x60),
        _ => NAMED_KEYS
            .iter()
            .find(|(k, _)| *k == vk)
            .map(|(_, n)| n.to_string())
            .unwrap_or_else(|| format!("Key{vk:02X}")),
    }
}

pub fn parse_key(s: &str) -> Option<u16> {
    let up = s.to_ascii_uppercase();
    if up.len() == 1 {
        let c = up.as_bytes()[0];
        if c.is_ascii_uppercase() || c.is_ascii_digit() {
            return Some(c as u16);
        }
    }
    if let Some(n) = up.strip_prefix('F').and_then(|n| n.parse::<u16>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    if let Some(n) = up.strip_prefix("NUM").and_then(|n| n.parse::<u16>().ok()) {
        if n <= 9 {
            return Some(0x60 + n);
        }
    }
    if let Some(hex) = up.strip_prefix("KEY") {
        return u16::from_str_radix(hex, 16).ok();
    }
    NAMED_KEYS
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(s))
        .map(|(k, _)| *k)
}

/// The capture mode the user picked (None in Config = pick the best automatically).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeSel {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub format: String,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // Video
    /// Friendly name of the capture device ("" = pick automatically).
    pub video_device: String,
    /// Windows device path of the capture device, used to find the same unit again.
    pub video_link: String,
    pub scaling: Scaling,
    pub filter: Filter,
    /// Where the picture sits when there's spare room (room for overlays).
    pub align_x: AlignX,
    pub align_y: AlignY,
    pub color_range: ColorRange,
    pub color_matrix: ColorMatrix,

    // Audio
    /// "auto" (match the video device), "off", or a Windows audio endpoint id.
    pub audio_in: String,
    /// "" = system default output, or a Windows audio endpoint id.
    pub audio_out: String,
    /// 0.0 ..= 1.0
    pub volume: f32,
    pub muted: bool,
    pub audio_delay_ms: u32,
    /// When an input reports mono 96 kHz, read it as the stereo 48 kHz it really
    /// is (MS2109 dongles). Inputs reporting anything else are left alone.
    pub ms2109_audio_fix: bool,

    // Window
    pub borderless: bool,
    pub always_on_top: bool,
    pub hide_cursor: bool,
    /// Client area on screen: x, y, width, height.
    pub window: Option<[i32; 4]>,

    // Look
    pub background: Fill,
    pub left: Zone,
    pub right: Zone,
    pub top: Zone,
    pub bottom: Zone,
    pub frame: FrameColor,
    pub frame_px: u32,
    pub stars: Stars,
    pub twinkle: bool,
    pub image_path: String,
    pub image_fit: ImageFit,
    /// 0.0 ..= 0.9 darkening applied to the custom image.
    pub image_dim: f32,

    // Shortcuts
    pub key_settings: Hotkey,
    pub key_fullscreen: Hotkey,
    pub key_mute: Hotkey,

    // Discord Rich Presence. Every text is the user's own; blank = left out.
    pub discord_enabled: bool,
    pub discord_app_id: String,
    pub discord_kind: DiscordKind,
    /// Top text (the bold title), e.g. "Stream With StarHowl".
    pub discord_title: String,
    /// Middle text, e.g. "Capturing Xbox".
    pub discord_details: String,
    pub discord_extra_on: bool,
    /// Bottom text.
    pub discord_extra: String,
    /// Big picture link ("" = the galaxy wolf).
    pub discord_image: String,
    /// Big text: shown when hovering the big picture.
    pub discord_large_text: String,
    pub discord_small_on: bool,
    /// Small picture link ("" = the StarHowl badge).
    pub discord_small_image: String,
    /// Small text: shown when hovering the small picture.
    pub discord_small_text: String,
    pub discord_button_on: bool,
    pub discord_button_label: String,
    pub discord_button_url: String,
    pub discord_elapsed: bool,
    /// Clear the Discord status after a minute without a picture.
    pub discord_hide_no_signal: bool,

    // Kept last: TOML wants tables after plain values.
    pub mode: Option<ModeSel>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            video_device: String::new(),
            video_link: String::new(),
            scaling: Scaling::Fit,
            filter: Filter::Sharp,
            align_x: AlignX::Center,
            align_y: AlignY::Middle,
            color_range: ColorRange::Auto,
            color_matrix: ColorMatrix::Auto,
            audio_in: "auto".into(),
            audio_out: String::new(),
            volume: 1.0,
            muted: false,
            audio_delay_ms: 0,
            ms2109_audio_fix: true,
            borderless: false,
            always_on_top: false,
            hide_cursor: true,
            window: None,
            background: Fill::Galaxy,
            left: Zone::Background,
            right: Zone::Background,
            top: Zone::Background,
            bottom: Zone::Background,
            frame: FrameColor::None,
            frame_px: 4,
            stars: Stars::Minimal,
            twinkle: true,
            image_path: String::new(),
            image_fit: ImageFit::Cover,
            image_dim: 0.0,
            key_settings: Hotkey::new(false, false, true, b'S' as u16),
            key_fullscreen: Hotkey::new(false, false, false, 0x7A), // F11
            key_mute: Hotkey::new(true, false, false, b'M' as u16),
            discord_enabled: false,
            discord_app_id: DISCORD_APP_ID.into(),
            discord_kind: DiscordKind::Watching,
            discord_title: DEFAULT_TITLE.into(),
            discord_details: "Capturing Xbox".into(),
            discord_extra_on: false,
            discord_extra: String::new(),
            discord_image: String::new(),
            discord_large_text: "Capturing Xbox".into(),
            discord_small_on: true,
            discord_small_image: String::new(),
            discord_small_text: "StarHowl".into(),
            discord_button_on: false,
            discord_button_label: "Watch me".into(),
            discord_button_url: String::new(),
            discord_elapsed: true,
            discord_hide_no_signal: true,
            mode: None,
        }
    }
}

/// Set by STARHOWL_CONFIG_DIR: settings and log live there instead (portable
/// use, or a second copy for testing).
pub fn custom_config_dir() -> Option<PathBuf> {
    std::env::var_os("STARHOWL_CONFIG_DIR").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = custom_config_dir() {
        return dir;
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Starhowl")
}

impl Config {
    pub fn path() -> PathBuf {
        config_dir().join("settings.toml")
    }

    pub fn load() -> Config {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    if let Ok(raw) = text.parse::<toml::Table>() {
                        migrate_discord(&mut c, &raw);
                    }
                    c.volume = c.volume.clamp(0.0, 1.0);
                    c.image_dim = c.image_dim.clamp(0.0, 0.9);
                    c.frame_px = c.frame_px.clamp(1, 64);
                    c.audio_delay_ms = c.audio_delay_ms.min(1000);
                    c
                }
                Err(e) => {
                    crate::log!("settings.toml could not be read ({e}); using defaults");
                    Config::default()
                }
            },
            Err(_) => Config::default(),
        }
    }

    pub fn save(&self) {
        let dir = config_dir();
        let _ = std::fs::create_dir_all(&dir);
        match toml::to_string_pretty(self) {
            Ok(text) => {
                let tmp = dir.join("settings.toml.tmp");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, Self::path());
                }
            }
            Err(e) => crate::log!("could not save settings: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_round_trip() {
        for s in ["Alt+S", "F11", "Ctrl+M", "Ctrl+Shift+Alt+F5", "Shift+Num7", "Alt+Space", "Ctrl+/"] {
            let h = Hotkey::parse(s).unwrap();
            assert_eq!(h.to_string(), s);
        }
        assert_eq!(Hotkey::parse("alt+s"), Some(Hotkey::new(false, false, true, b'S' as u16)));
        assert!(Hotkey::parse("Ctrl+").is_none());
        assert!(Hotkey::parse("Alt+Banana").is_none());
    }

    #[test]
    fn config_survives_toml_round_trip() {
        let mut c = Config::default();
        c.mode = Some(ModeSel { width: 1920, height: 1080, fps_num: 60, fps_den: 1, format: "MJPG".into() });
        c.window = Some([10, 20, 1280, 720]);
        c.left = Zone::White;
        c.image_path = r"C:\pics\space.png".into();
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let c: Config = toml::from_str("borderless = true\nframe = \"Purple\"").unwrap();
        assert!(c.borderless);
        assert_eq!(c.frame, FrameColor::Purple);
        assert_eq!(c.key_settings.to_string(), "Alt+S");
        assert_eq!(c.background, Fill::Galaxy);
    }
}

#[cfg(test)]
mod migrate_tests {
    use super::*;

    fn load_str(text: &str) -> Config {
        let mut c: Config = toml::from_str(text).unwrap();
        migrate_discord(&mut c, &text.parse().unwrap());
        c
    }

    #[test]
    fn old_discord_settings_carry_over() {
        let c = load_str("discord_title = \"Streaming With Starhowl\"\ndiscord_kind = \"Playing\"\ndiscord_capturing = \"PS5\"\n");
        assert_eq!(c.discord_title, "Stream With StarHowl");
        assert_eq!(c.discord_kind, DiscordKind::Watching);
        assert_eq!(c.discord_details, "Capturing PS5");
        assert_eq!(c.discord_large_text, "Capturing PS5");
    }

    #[test]
    fn custom_choices_are_left_alone() {
        let c = load_str("discord_title = \"My stream\"\ndiscord_kind = \"Playing\"\ndiscord_details = \"Hello\"\ndiscord_capturing = \"PS5\"\n");
        assert_eq!(c.discord_title, "My stream");
        assert_eq!(c.discord_kind, DiscordKind::Playing);
        assert_eq!(c.discord_details, "Hello");
    }

    #[test]
    fn fresh_defaults_read_watching_stream_with_starhowl() {
        let c = Config::default();
        assert_eq!(c.discord_kind, DiscordKind::Watching);
        assert_eq!(c.discord_title, "Stream With StarHowl");
    }
}
