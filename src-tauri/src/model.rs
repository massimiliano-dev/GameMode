use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    Steam,
    Xbox,
    Epic,
    Gog,
    Ea,
    Ubisoft,
    Manual,
    Emulator,
    Mock,
}

impl Store {
    pub fn prefix(self) -> &'static str {
        match self {
            Store::Steam => "steam",
            Store::Xbox => "xbox",
            Store::Epic => "epic",
            Store::Gog => "gog",
            Store::Ea => "ea",
            Store::Ubisoft => "ubisoft",
            Store::Manual => "manual",
            Store::Emulator => "emulator",
            Store::Mock => "mock",
        }
    }

    pub fn game_id(self, native_id: &str) -> String {
        format!("{}:{}", self.prefix(), native_id)
    }
}

/// How a game is started. Store games go through their client's URI scheme so
/// DRM, cloud saves and overlays keep working.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LaunchSpec {
    Uri {
        uri: String,
    },
    Exe {
        path: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        cwd: Option<PathBuf>,
    },
    /// Packaged (MSIX/UWP) app, started via `shell:AppsFolder\<AUMID>`.
    AppsFolder {
        aumid: String,
    },
}

/// Local file paths of cached artwork.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Artwork {
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub logo: Option<String>,
}

impl Artwork {
    pub fn is_complete(&self) -> bool {
        self.cover.is_some() && self.hero.is_some() && self.logo.is_some()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub store: Store,
    pub name: String,
    pub install_dir: Option<PathBuf>,
    pub launch: LaunchSpec,
    /// Steam appid, when known, used to fetch artwork from the Steam CDN.
    #[serde(default)]
    pub steam_appid: Option<u32>,
    /// A known cover image URL (e.g. libretro thumbnails for ROMs).
    #[serde(default)]
    pub cover_url: Option<String>,

    // User state, preserved across rescans.
    #[serde(default)]
    pub artwork: Artwork,
    /// Unix seconds of the last artwork lookup, to avoid hammering APIs for
    /// games that have no artwork available.
    #[serde(default)]
    pub artwork_checked_at: Option<i64>,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub added_at: i64,
    #[serde(default)]
    pub last_played: Option<i64>,
    #[serde(default)]
    pub playtime_secs: u64,
    /// Space the install takes, from the store's manifest or a folder walk.
    #[serde(default)]
    pub size_bytes: Option<u64>,
    /// Unix seconds of the last folder walk; `None` when `size_bytes` comes
    /// from the store's manifest (refreshed by every scan) or is unknown.
    #[serde(default)]
    pub size_checked_at: Option<i64>,
    /// The walk could not read everything (e.g. WindowsApps ACLs): an estimate.
    #[serde(default)]
    pub size_partial: bool,
}

impl Game {
    pub fn new(store: Store, native_id: &str, name: impl Into<String>, launch: LaunchSpec) -> Self {
        Game {
            id: store.game_id(native_id),
            store,
            name: name.into(),
            install_dir: None,
            launch,
            steam_appid: None,
            cover_url: None,
            artwork: Artwork::default(),
            artwork_checked_at: None,
            favorite: false,
            hidden: false,
            added_at: 0,
            last_played: None,
            playtime_secs: 0,
            size_bytes: None,
            size_checked_at: None,
            size_partial: false,
        }
    }

    pub fn with_install_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.install_dir = Some(dir.into());
        self
    }

    /// Copies everything the user (or the artwork fetcher) set on a previous
    /// scan onto a freshly scanned entry.
    pub fn inherit_user_state(&mut self, old: &Game) {
        self.artwork = old.artwork.clone();
        self.artwork_checked_at = old.artwork_checked_at;
        self.favorite = old.favorite;
        self.hidden = old.hidden;
        self.added_at = old.added_at;
        self.last_played = old.last_played;
        self.playtime_secs = old.playtime_secs;
        // A size measured by walking the folder survives rescans; one the
        // store manifest provides is fresh in `self` already.
        if self.size_bytes.is_none() {
            self.size_bytes = old.size_bytes;
            self.size_checked_at = old.size_checked_at;
            self.size_partial = old.size_partial;
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ManualGame {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub game_id: String,
    /// Profile that played; empty in data from before profiles existed.
    #[serde(default)]
    pub profile_id: String,
    pub start: i64,
    pub end: i64,
    pub seconds: u64,
}

/// One person in the household.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub name: String,
    /// Avatar colour, as a CSS hex value.
    pub color: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub game_ids: Vec<String>,
}

/// What each profile keeps for itself.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileState {
    pub favorites: Vec<String>,
    pub hidden: Vec<String>,
    pub collections: Vec<Collection>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub steamgriddb_key: Option<String>,
    pub disabled_stores: Vec<Store>,
    pub start_fullscreen: bool,
    /// Start Steam silently together with GameMode.
    pub preload_steam: bool,
    /// Steam Web API key, to list owned games that aren't installed.
    pub steam_api_key: Option<String>,
    /// Navigation clicks in the UI.
    pub ui_sounds: bool,
    /// RetroArch executable; detected when unset.
    pub retroarch_path: Option<PathBuf>,
    /// ROM folder per emulated system.
    pub rom_dirs: Vec<RomDir>,
    /// Display mode to run GameMode in ("TV mode"); `None` keeps the desktop's.
    pub tv_mode: Option<crate::display::DisplayMode>,
    /// Audio output (device id) to switch to when GameMode takes the screen.
    pub tv_audio: Option<String>,
    /// High-performance power plan and closing `close_apps` while playing.
    pub performance_mode: bool,
    /// Executable names, e.g. "chrome.exe".
    pub close_apps: Vec<String>,
    /// Guide during a game opens the overlay instead of the launcher.
    pub overlay_enabled: bool,
    /// Where save backups go; the app data folder when unset.
    pub backup_dir: Option<PathBuf>,
    /// The controller types Enter/Esc/arrows in store windows GameMode opens.
    pub keyboard_assist: bool,
    /// Open Steam's store/install/uninstall in Big Picture (controller UI).
    pub steam_big_picture: bool,
    /// In Steam games Guide is left to Steam (its overlay); Back+Start
    /// still opens GameMode's.
    pub steam_guide: bool,
    /// Wand's overlay shortcut is Alt+` instead of Alt+W.
    pub wand_backtick: bool,
    /// LB+RB held during a game opens Wand's overlay.
    pub wand_combo: bool,
    /// Freeze the game while Wand has the screen.
    pub wand_pause: bool,
    /// Controller vibration for the interface: 0 off, 1 light, 2 normal.
    pub haptics: u8,
    /// Now playing and the per-app mixer in the overlay and quick menu.
    pub music_controls: bool,
    /// Volume keys and View + D-pad show the volume over the game (OSD).
    pub osd_volume: bool,
    /// Minutes of inactivity before the ambient screensaver; 0 = never.
    pub idle_screensaver_min: u32,
    /// Minutes of inactivity before the display is turned off; 0 = never.
    pub idle_display_off_min: u32,
    /// Minutes of inactivity before `idle_action`; 0 = never.
    pub idle_sleep_min: u32,
    /// Sleep or shut down after `idle_sleep_min`.
    pub idle_action: crate::system::PowerAction,
    /// Inactivity never sleeps the PC while a game is downloading or updating.
    pub idle_wait_downloads: bool,
    /// The on-screen keyboard opens by itself when a text field takes the
    /// focus in a store window during a hand-off.
    pub keyboard_auto: bool,
    /// When Windows shuts down with a game open, back up its saves once it
    /// has closed.
    pub backup_on_exit: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RomDir {
    /// Key from `library::emulators::SYSTEMS`.
    pub system: String,
    pub path: PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            steamgriddb_key: None,
            disabled_stores: Vec::new(),
            start_fullscreen: true,
            preload_steam: true,
            steam_api_key: None,
            ui_sounds: true,
            retroarch_path: None,
            rom_dirs: Vec::new(),
            tv_mode: None,
            tv_audio: None,
            performance_mode: false,
            close_apps: Vec::new(),
            overlay_enabled: true,
            backup_dir: None,
            keyboard_assist: true,
            steam_big_picture: true,
            steam_guide: false,
            wand_backtick: false,
            wand_combo: false,
            wand_pause: true,
            haptics: 1,
            music_controls: true,
            osd_volume: true,
            idle_screensaver_min: 10,
            idle_display_off_min: 30,
            idle_sleep_min: 60,
            idle_action: crate::system::PowerAction::Sleep,
            idle_wait_downloads: true,
            keyboard_auto: true,
            backup_on_exit: true,
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
