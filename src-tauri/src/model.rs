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
            artwork: Artwork::default(),
            artwork_checked_at: None,
            favorite: false,
            hidden: false,
            added_at: 0,
            last_played: None,
            playtime_secs: 0,
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
    pub start: i64,
    pub end: i64,
    pub seconds: u64,
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            steamgriddb_key: None,
            disabled_stores: Vec::new(),
            start_fullscreen: true,
            preload_steam: true,
            steam_api_key: None,
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
