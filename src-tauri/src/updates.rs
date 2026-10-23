//! Game updates, so GameMode can say "updating 45%" instead of a silent
//! "starting…": Steam's download state comes from its app manifests, Game
//! Pass / Microsoft Store updates from the package catalog's events.

use serde::Serialize;

use crate::model::{Game, Store};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum UpdateState {
    /// Waiting to be downloaded (Steam downloads it when the game starts).
    Available,
    Downloading,
    /// Unpacking, verifying or registering the new files.
    Installing,
    Paused,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GameUpdate {
    pub game_id: String,
    pub state: UpdateState,
    /// Bytes done and in total; 0 when the store doesn't say.
    pub done: u64,
    pub total: u64,
    /// 0…1, when known.
    pub progress: Option<f64>,
}

impl GameUpdate {
    /// Something is happening (worth waiting for), not just "available".
    pub fn active(&self) -> bool {
        self.state != UpdateState::Available
    }
}

/// Updates for the given games, as far as their stores tell.
pub fn all(games: &[Game]) -> Vec<GameUpdate> {
    let steam = steam_all();
    let epic = crate::legendary::updates();
    let mut list: Vec<GameUpdate> = games
        .iter()
        .filter_map(|game| match game.store {
            Store::Steam => {
                let appid = game.steam_appid?;
                steam.iter().find(|(id, _)| *id == appid).map(|(_, u)| with_id(u, game))
            }
            Store::Xbox => xbox_update(family(game)?).map(|u| with_id(&u, game)),
            Store::Epic => epic.iter().find(|u| u.game_id == game.id).cloned(),
            _ => None,
        })
        .collect();
    // legendary installs of games not in the library yet.
    list.extend(epic.into_iter().filter(|u| u.active() && !games.iter().any(|g| g.id == u.game_id)));
    list
}

/// The update of one game, looked at a bit harder (for the launch screen).
pub fn for_game(game: &Game) -> Option<GameUpdate> {
    match game.store {
        Store::Steam => steam_one(game.steam_appid?).map(|u| with_id(&u, game)),
        Store::Xbox => {
            let family = family(game)?;
            xbox_update(family).or_else(|| servicing(family)).map(|u| with_id(&u, game))
        }
        Store::Epic => crate::legendary::update_for(&game.id),
        _ => None,
    }
}

fn with_id(update: &GameUpdate, game: &Game) -> GameUpdate {
    GameUpdate { game_id: game.id.clone(), ..update.clone() }
}

/// Xbox game ids are `xbox:<PackageFamilyName>`.
fn family(game: &Game) -> Option<&str> {
    game.id.strip_prefix("xbox:")
}

// ---------- Steam ----------

const UPDATE_REQUIRED: u32 = 2;
const UPDATE_RUNNING: u32 = 256;
const UPDATE_PAUSED: u32 = 512;
const UPDATE_STARTED: u32 = 1024;
const VALIDATING: u32 = 1 << 17;
const ADDING_FILES: u32 = 1 << 18;
const PREALLOCATING: u32 = 1 << 19;
const DOWNLOADING: u32 = 1 << 20;
const STAGING: u32 = 1 << 21;
const COMMITTING: u32 = 1 << 22;

/// Update state from an `appmanifest_*.acf`: `(appid, update)`.
pub fn parse_steam_update(src: &str) -> Option<(u32, GameUpdate)> {
    let parsed = crate::library::vdf::parse(src).ok()?;
    let app = parsed.get("AppState")?;
    let appid: u32 = app.get_str("appid")?.parse().ok()?;
    let flags: u32 = app.get_str("StateFlags").and_then(|s| s.parse().ok()).unwrap_or(0);
    let bytes = |key: &str| app.get_str(key).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    let state = if flags & UPDATE_PAUSED != 0 {
        UpdateState::Paused
    } else if flags & (STAGING | COMMITTING | VALIDATING | ADDING_FILES | PREALLOCATING) != 0 {
        UpdateState::Installing
    } else if flags & (DOWNLOADING | UPDATE_RUNNING | UPDATE_STARTED) != 0 {
        UpdateState::Downloading
    } else if flags & UPDATE_REQUIRED != 0 {
        UpdateState::Available
    } else {
        return None;
    };
    let (staged, to_stage) = (bytes("BytesStaged"), bytes("BytesToStage"));
    let (done, total) = if state == UpdateState::Installing && to_stage > 0 {
        (staged, to_stage)
    } else {
        (bytes("BytesDownloaded"), bytes("BytesToDownload"))
    };
    let progress = (total > 0).then(|| (done as f64 / total as f64).clamp(0.0, 1.0));
    Some((appid, GameUpdate { game_id: format!("steam:{appid}"), state, done, total, progress }))
}

fn steam_manifests() -> Vec<std::path::PathBuf> {
    let Some(root) = crate::library::steam::find_steam_root() else { return Vec::new() };
    crate::library::steam::library_folders(&root)
        .into_iter()
        .flat_map(|folder| std::fs::read_dir(folder.join("steamapps")).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("appmanifest_") && n.ends_with(".acf")))
        .collect()
}

fn steam_all() -> Vec<(u32, GameUpdate)> {
    steam_manifests()
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|src| parse_steam_update(&src))
        .collect()
}

fn steam_one(appid: u32) -> Option<GameUpdate> {
    let name = format!("appmanifest_{appid}.acf");
    let root = crate::library::steam::find_steam_root()?;
    crate::library::steam::library_folders(&root)
        .iter()
        .find_map(|f| std::fs::read_to_string(f.join("steamapps").join(&name)).ok())
        .and_then(|src| parse_steam_update(&src))
        .map(|(_, u)| u)
}

// ---------- Microsoft Store / Game Pass ----------

#[cfg(windows)]
mod xbox {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use windows::core::{Ref, HSTRING};
    use windows::ApplicationModel::{Package, PackageCatalog, PackageStagingEventArgs, PackageUpdatingEventArgs};
    use windows::Foundation::TypedEventHandler;
    use windows::Management::Deployment::PackageManager;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::{GameUpdate, UpdateState};

    /// Without news for this long, an update is assumed over (or stuck).
    const STALE: Duration = Duration::from_secs(90);

    struct Entry {
        state: UpdateState,
        progress: f64,
        at: Instant,
    }

    /// Package family (lowercase) → its running update.
    static RUNNING: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

    fn record(package: windows::core::Result<Package>, state: UpdateState, progress: f64, complete: bool) {
        let Some(family) = package.and_then(|p| p.Id()?.FamilyName()).ok().map(|f| f.to_string().to_lowercase())
        else {
            return;
        };
        let mut guard = RUNNING.lock().unwrap();
        let map = guard.get_or_insert_with(HashMap::new);
        if complete {
            map.remove(&family);
        } else {
            // Windows reports 0…100.
            let progress = if progress > 1.0 { progress / 100.0 } else { progress }.clamp(0.0, 1.0);
            map.insert(family, Entry { state, progress, at: Instant::now() });
        }
    }

    pub fn watch() -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let catalog = PackageCatalog::OpenForCurrentUser()?;
        catalog.PackageStaging(&TypedEventHandler::<PackageCatalog, PackageStagingEventArgs>::new(
            |_, args: Ref<PackageStagingEventArgs>| {
                if let Ok(args) = args.ok() {
                    record(args.Package(), UpdateState::Downloading, args.Progress().unwrap_or(0.0), args.IsComplete().unwrap_or(false));
                }
                Ok(())
            },
        ))?;
        catalog.PackageUpdating(&TypedEventHandler::<PackageCatalog, PackageUpdatingEventArgs>::new(
            |_, args: Ref<PackageUpdatingEventArgs>| {
                if let Ok(args) = args.ok() {
                    record(args.TargetPackage(), UpdateState::Installing, args.Progress().unwrap_or(0.0), args.IsComplete().unwrap_or(false));
                }
                Ok(())
            },
        ))?;
        // Events stop when the catalog is released: keep it for the whole run.
        std::mem::forget(catalog);
        Ok(())
    }

    pub fn update(family: &str) -> Option<GameUpdate> {
        let mut guard = RUNNING.lock().unwrap();
        let map = guard.as_mut()?;
        map.retain(|_, e| e.at.elapsed() < STALE);
        let entry = map.get(&family.to_lowercase())?;
        Some(GameUpdate { game_id: String::new(), state: entry.state, done: 0, total: 0, progress: Some(entry.progress) })
    }

    /// The package is being serviced (updated or repaired) right now.
    pub fn servicing(family: &str) -> Option<GameUpdate> {
        let manager = PackageManager::new().ok()?;
        let package = manager
            .FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family))
            .ok()?
            .into_iter()
            .next()?;
        let status = package.Status().ok()?;
        let busy = status.Servicing().unwrap_or(false) || status.DeploymentInProgress().unwrap_or(false);
        busy.then(|| GameUpdate { game_id: String::new(), state: UpdateState::Installing, done: 0, total: 0, progress: None })
    }
}

/// Starts listening for Store package updates (once, at startup).
pub fn watch_packages() {
    #[cfg(windows)]
    std::thread::spawn(|| {
        if let Err(err) = xbox::watch() {
            log::warn!("package catalog: {err}");
        }
    });
}

fn xbox_update(family: &str) -> Option<GameUpdate> {
    #[cfg(windows)]
    {
        xbox::update(family)
    }
    #[cfg(not(windows))]
    {
        let _ = family;
        None
    }
}

fn servicing(family: &str) -> Option<GameUpdate> {
    #[cfg(windows)]
    {
        xbox::servicing(family)
    }
    #[cfg(not(windows))]
    {
        let _ = family;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(flags: u32, extra: &str) -> String {
        format!(r#""AppState" {{ "appid" "570" "name" "Dota 2" "StateFlags" "{flags}" {extra} }}"#)
    }

    #[test]
    fn reads_steam_update_states() {
        assert!(parse_steam_update(&manifest(4, "")).is_none());

        let (appid, available) = parse_steam_update(&manifest(6, "")).unwrap();
        assert_eq!(appid, 570);
        assert_eq!(available.state, UpdateState::Available);
        assert!(!available.active());

        let (_, downloading) = parse_steam_update(&manifest(
            4 | UPDATE_REQUIRED | UPDATE_STARTED | DOWNLOADING,
            r#""BytesToDownload" "1000" "BytesDownloaded" "250""#,
        ))
        .unwrap();
        assert_eq!(downloading.state, UpdateState::Downloading);
        assert_eq!((downloading.done, downloading.total), (250, 1000));
        assert_eq!(downloading.progress, Some(0.25));
        assert_eq!(downloading.game_id, "steam:570");

        let (_, staging) = parse_steam_update(&manifest(
            4 | UPDATE_STARTED | STAGING,
            r#""BytesToDownload" "1000" "BytesDownloaded" "1000" "BytesToStage" "400" "BytesStaged" "100""#,
        ))
        .unwrap();
        assert_eq!(staging.state, UpdateState::Installing);
        assert_eq!((staging.done, staging.total), (100, 400));

        let (_, paused) = parse_steam_update(&manifest(4 | UPDATE_STARTED | UPDATE_PAUSED, "")).unwrap();
        assert_eq!(paused.state, UpdateState::Paused);
        assert_eq!(paused.progress, None);
    }
}
