//! Disk space. Free space per drive (through sysinfo), how much each
//! installed game takes (the store manifest where it says so, a folder walk
//! otherwise, cached in the library) and cleanup of what GameMode leaves
//! around on its own: artwork of uninstalled games and old automatic save
//! backups.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::model::{now, Game, LaunchSpec, Store};
use crate::AppState;

/// Walked sizes are refreshed after a week.
const MAX_AGE_SECS: i64 = 7 * 24 * 3600;
/// Automatic backups the cleanup keeps per game.
pub const KEEP_AUTO_BACKUPS: usize = 3;
/// Folders a hand-added game may point into that are never walked.
const NEVER_WALK: &[&str] = &["windows", "program files", "program files (x86)", "windowsapps", "users", "programdata"];

static MEASURING: AtomicBool = AtomicBool::new(false);
/// A measure was asked for while one was running: run another round after it.
static RERUN: AtomicBool = AtomicBool::new(false);

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Drive {
    /// `C:\` on Windows.
    pub mount: String,
    /// Volume label, empty when the volume has none.
    pub label: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
    /// Bytes of the library's games on this drive (games with a known size).
    pub games_bytes: u64,
    /// Games installed on this drive, size known or not.
    pub games: u32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub drives: Vec<Drive>,
    /// Sizes are still being measured in the background.
    pub measuring: bool,
    /// Installed games whose size isn't known yet: a measure is pending.
    pub unknown: u32,
    /// Games already measured that came out without a size (folders Windows
    /// doesn't let us read, or shared with other games).
    pub unreadable: u32,
    /// Where the Xbox app installs, as far as we can tell: the drive holding
    /// the most Xbox games, else the one with the most free space.
    pub xbox_mount: Option<String>,
}

/// Fixed and removable drives with a known capacity.
pub fn drives() -> Vec<Drive> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut out: Vec<Drive> = disks
        .list()
        .iter()
        .filter(|d| d.total_space() > 0)
        .map(|d| Drive {
            mount: d.mount_point().to_string_lossy().into_owned(),
            label: d.name().to_string_lossy().trim().to_string(),
            total: d.total_space(),
            available: d.available_space(),
            removable: d.is_removable(),
            games_bytes: 0,
            games: 0,
        })
        .collect();
    // macOS lists every APFS volume of the system container: keep what a
    // person thinks of as a drive.
    #[cfg(not(windows))]
    out.retain(|d| d.mount == "/" || d.mount.starts_with("/Volumes/"));
    out.sort_by(|a, b| a.mount.cmp(&b.mount));
    out.dedup_by(|a, b| a.mount == b.mount);
    out
}

/// Lowercase, forward slashes, trailing slash: `C:\Games\` and `c:/games`
/// compare equal.
fn norm(path: &Path) -> String {
    let mut s = path.to_string_lossy().replace('\\', "/").to_lowercase();
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Index of the drive `path` is on: the longest mount that prefixes it.
pub fn drive_of(drives: &[Drive], path: &Path) -> Option<usize> {
    let path = norm(path);
    drives
        .iter()
        .enumerate()
        .filter(|(_, d)| path.starts_with(&norm(Path::new(&d.mount))))
        .max_by_key(|(_, d)| d.mount.len())
        .map(|(i, _)| i)
}

/// The folder (or ROM file) a game lives in, for the drive it sits on.
pub fn location(game: &Game) -> Option<PathBuf> {
    if let Some(dir) = &game.install_dir {
        return Some(dir.clone());
    }
    match &game.launch {
        LaunchSpec::Exe { args, .. } if game.store == Store::Emulator => args.last().map(PathBuf::from),
        LaunchSpec::Exe { path, .. } => path.parent().map(Path::to_path_buf),
        _ => None,
    }
}

/// What `attribute` finds besides the per-drive sums.
#[derive(Debug, Default, PartialEq)]
pub struct Attribution {
    /// Games without a size that `measure` will still look at.
    pub unknown: u32,
    /// Games without a size that measuring won't give one.
    pub unreadable: u32,
    /// The drive that looks like the Xbox app's install drive.
    pub xbox: Option<usize>,
}

/// Sums the library's games onto the drives.
pub fn attribute(drives: &mut [Drive], games: &[Game]) -> Attribution {
    let mut out = Attribution::default();
    let mut xbox = vec![0u32; drives.len()];
    // Applications aren't games: their folders count as "other" space.
    for game in games.iter().filter(|g| g.store != Store::App) {
        let Some(path) = location(game) else { continue };
        if game.size_bytes.is_none() {
            if game.install_dir.is_some() && game.size_checked_at.is_none() {
                out.unknown += 1;
            } else {
                out.unreadable += 1;
            }
        }
        let Some(i) = drive_of(drives, &path) else { continue };
        drives[i].games += 1;
        drives[i].games_bytes += game.size_bytes.unwrap_or(0);
        if game.store == Store::Xbox {
            xbox[i] += 1;
        }
    }
    let by_games = (0..drives.len()).filter(|&i| xbox[i] > 0).max_by_key(|&i| xbox[i]);
    let by_space = (0..drives.len()).filter(|&i| !drives[i].removable).max_by_key(|&i| drives[i].available);
    out.xbox = by_games.or(by_space);
    out
}

pub fn overview(app: &AppHandle) -> Overview {
    let games = app.state::<AppState>().db.lock().games.clone();
    let mut drives = drives();
    let found = attribute(&mut drives, &games);
    let xbox_mount = found.xbox.map(|i| drives[i].mount.clone());
    Overview {
        drives,
        measuring: MEASURING.load(Ordering::SeqCst),
        unknown: found.unknown,
        unreadable: found.unreadable,
        xbox_mount,
    }
}

// ---- measuring ----

#[derive(Debug, Default, PartialEq)]
pub struct Walk {
    pub bytes: u64,
    /// Something could not be read (permissions): the count is a lower bound.
    pub partial: bool,
}

/// Bytes of the files under `path`, without following symlinks or junctions
/// (WindowsApps and XboxGames are full of them) and tolerating what can't be
/// read. Iterative: game folders can be tens of thousands of files deep.
/// `path` itself is followed when it is a link: a game folder moved to
/// another drive often leaves a junction in its place.
pub fn walk(path: &Path) -> Walk {
    let mut out = Walk::default();
    let Ok(meta) = fs::metadata(path) else {
        out.partial = true;
        return out;
    };
    if meta.is_file() {
        out.bytes = meta.len();
        return out;
    }
    if !meta.is_dir() {
        out.partial = true;
        return out;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            out.partial = true;
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                out.partial = true;
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                out.partial = true;
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                match entry.metadata() {
                    Ok(m) => out.bytes += m.len(),
                    Err(_) => out.partial = true,
                }
            }
        }
    }
    out
}

/// Plain files and folders only; `dir_size` of saves.rs used to follow links.
pub fn dir_size(path: &Path) -> u64 {
    walk(path).bytes
}

/// Whether a size needs (re)measuring now.
pub fn is_due(game: &Game, now: i64) -> bool {
    match (game.size_bytes, game.size_checked_at) {
        // From the store manifest: refreshed by every scan.
        (Some(_), None) => false,
        (_, Some(checked)) => now - checked > MAX_AGE_SECS,
        (None, None) => true,
    }
}

/// A folder it makes sense to add up: not a drive root, not Windows itself,
/// not a user's Desktop or Downloads (where a hand-added exe may sit).
pub fn walkable(dir: &Path) -> bool {
    // By text, so Windows paths are judged the same way on every platform.
    let text = dir.to_string_lossy().replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty() && !(p.len() == 2 && p.ends_with(':'))).collect();
    let (Some(first), Some(last)) = (parts.first(), parts.last()) else { return false };
    if NEVER_WALK.contains(last) {
        return false;
    }
    match *first {
        "windows" => false,
        // C:\Users\<name>\Desktop: a game folder is at least one level deeper.
        "users" => parts.len() > 3,
        _ => true,
    }
}

/// Whether `dir` holds the folder of another game: walking it would count
/// that game twice (a hand-added `Steam\steam.exe`, `D:\Games\launcher.exe`).
pub fn holds_other_games(dir: &Path, others: &[PathBuf]) -> bool {
    let dir = norm(dir);
    others.iter().map(|o| norm(o)).any(|o| o != dir && o.starts_with(&dir))
}

/// Games to measure now: a folder, and a size unknown or stale.
fn due_games(app: &AppHandle) -> Vec<Game> {
    let ts = now();
    app.state::<AppState>().db.lock().games.iter().filter(|g| g.store != Store::App && g.install_dir.is_some() && is_due(g, ts)).cloned().collect()
}

/// Measures, in a background thread, every game whose size is unknown or
/// stale, saving each result as it comes. Does nothing while a game is
/// running (the walk competes for the disk) or when nothing is due, and
/// emits `storage://measured` only after a round that actually ran.
pub fn measure(app: &AppHandle) {
    if app.state::<AppState>().launcher.current().is_some() {
        return;
    }
    if MEASURING.swap(true, Ordering::SeqCst) {
        RERUN.store(true, Ordering::SeqCst);
        return;
    }
    let due = due_games(app);
    if due.is_empty() {
        MEASURING.store(false, Ordering::SeqCst);
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        lower_priority();
        let mut due = due;
        loop {
            measure_round(&app, due);
            MEASURING.store(false, Ordering::SeqCst);
            // Someone asked while this round ran (a remeasure, a scan): its
            // games weren't in this round's list.
            if !RERUN.swap(false, Ordering::SeqCst) || MEASURING.swap(true, Ordering::SeqCst) {
                break;
            }
            due = if app.state::<AppState>().launcher.current().is_some() { Vec::new() } else { due_games(&app) };
            if due.is_empty() {
                MEASURING.store(false, Ordering::SeqCst);
                break;
            }
        }
        let _ = app.emit("storage://measured", ());
    });
}

fn measure_round(app: &AppHandle, due: Vec<Game>) {
    let state = app.state::<AppState>();
    let folders: Vec<PathBuf> = state.db.lock().games.iter().filter_map(|g| g.install_dir.clone()).collect();
    for game in due {
        if state.launcher.current().is_some() {
            break;
        }
        let Some(dir) = game.install_dir.as_deref() else { continue };
        let result = if walkable(dir) && !holds_other_games(dir, &folders) { walk(dir) } else { Walk { bytes: 0, partial: true } };
        // Nothing readable at all (WindowsApps ACLs): unknown, not "0 MB".
        let size = (result.bytes > 0 || !result.partial).then_some(result.bytes);
        let updated = state.db.update(|d| {
            let g = d.game_mut(&game.id)?;
            g.size_bytes = size;
            g.size_checked_at = Some(now());
            g.size_partial = result.partial;
            d.view_game(&game.id)
        });
        if let Ok(Some(g)) = updated {
            let _ = app.emit("library://game", &g);
        }
    }
}

/// Forgets every walked size, so `measure` does them all again. Sizes from a
/// store manifest stay: they are refreshed by every scan.
pub fn remeasure(app: &AppHandle) {
    let state = app.state::<AppState>();
    let _ = state.db.update(|d| {
        for g in d.games.iter_mut().filter(|g| g.size_checked_at.is_some()) {
            g.size_bytes = None;
            g.size_checked_at = None;
            g.size_partial = false;
        }
    });
    measure(app);
}

/// Background mode: lower CPU and I/O priority, so a walk over a big HDD
/// doesn't slow down whatever the user is doing.
#[cfg(windows)]
fn lower_priority() {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

#[cfg(not(windows))]
fn lower_priority() {}

// ---- cleanup ----

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cleanup {
    /// Artwork folders of games no longer in the library.
    pub artwork_items: u32,
    pub artwork_bytes: u64,
    /// Automatic save backups beyond the last `KEEP_AUTO_BACKUPS` per game.
    pub backup_items: u32,
    pub backup_bytes: u64,
    /// The items were deleted (not just counted).
    pub applied: bool,
}

/// Counts (and with `apply`, deletes) what the cleanup would free.
pub fn cleanup(app: &AppHandle, apply: bool) -> Cleanup {
    let state = app.state::<AppState>();
    let (ids, backup_root) = {
        let data = state.db.lock();
        let mut ids: HashSet<String> = data.games.iter().map(|g| crate::artwork::safe_name(&g.id)).collect();
        // The applications' icons share one folder (library/apps.rs).
        ids.insert(crate::library::apps::ICON_FOLDER.to_string());
        (ids, data.settings.backup_dir.clone().unwrap_or_else(|| state.data_dir.join("backups")))
    };
    let mut out = Cleanup { applied: apply, ..Cleanup::default() };
    for dir in orphan_artwork(&state.data_dir.join("artwork"), &ids) {
        out.artwork_items += 1;
        out.artwork_bytes += dir_size(&dir);
        if apply {
            let _ = fs::remove_dir_all(&dir);
        }
    }
    for dir in old_auto_backups(&backup_root, KEEP_AUTO_BACKUPS) {
        out.backup_items += 1;
        out.backup_bytes += dir_size(&dir);
        if apply {
            let _ = fs::remove_dir_all(&dir);
        }
    }
    out
}

/// Artwork folders whose game isn't in `keep` (ids as `artwork::safe_name`).
pub fn orphan_artwork(artwork_dir: &Path, keep: &HashSet<String>) -> Vec<PathBuf> {
    fs::read_dir(artwork_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && !keep.contains(&*e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect()
}

/// Automatic backups older than the newest `keep` of each game.
pub fn old_auto_backups(root: &Path, keep: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for game_dir in fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let mut auto: Vec<(i64, PathBuf)> = fs::read_dir(&game_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let meta: crate::saves::BackupMeta = serde_json::from_slice(&fs::read(e.path().join("backup.json")).ok()?).ok()?;
                meta.automatic.then(|| (meta.created, e.path()))
            })
            .collect();
        auto.sort_by(|a, b| b.0.cmp(&a.0));
        out.extend(auto.into_iter().skip(keep).map(|(_, p)| p));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(mount: &str, available: u64) -> Drive {
        Drive { mount: mount.into(), label: String::new(), total: 1000, available, removable: false, games_bytes: 0, games: 0 }
    }

    fn game(store: Store, id: &str, dir: Option<&str>, size: Option<u64>) -> Game {
        let mut g = Game::new(store, id, id, LaunchSpec::Uri { uri: String::new() });
        g.install_dir = dir.map(PathBuf::from);
        g.size_bytes = size;
        g
    }

    #[test]
    fn games_land_on_the_longest_matching_mount() {
        let drives = vec![drive(r"C:\", 10), drive(r"D:\", 500), drive("/", 1), drive("/Volumes/Games", 2)];
        assert_eq!(drive_of(&drives, Path::new(r"d:\Games\Hades")), Some(1));
        assert_eq!(drive_of(&drives, Path::new(r"C:\Program Files\Epic")), Some(0));
        assert_eq!(drive_of(&drives, Path::new("/Volumes/Games/x")), Some(3));
        assert_eq!(drive_of(&drives, Path::new("/Users/max")), Some(2));
        assert_eq!(drive_of(&drives, Path::new(r"E:\x")), None);
    }

    #[test]
    fn attributes_sizes_and_guesses_the_xbox_drive() {
        let mut drives = vec![drive(r"C:\", 10), drive(r"D:\", 500)];
        let games = vec![
            game(Store::Steam, "1", Some(r"D:\SteamLibrary\steamapps\common\A"), Some(100)),
            game(Store::Gog, "2", Some(r"D:\GOG\B"), None),
            game(Store::Xbox, "3", Some(r"C:\XboxGames\C"), Some(40)),
            game(Store::Steam, "4", None, None),
        ];
        let found = attribute(&mut drives, &games);
        assert_eq!((found.unknown, found.unreadable), (1, 0), "games without a folder don't count as unknown");
        assert_eq!((drives[1].games, drives[1].games_bytes), (2, 100));
        assert_eq!((drives[0].games, drives[0].games_bytes), (1, 40));
        assert_eq!(found.xbox, Some(0));

        // No Xbox games: the roomiest fixed drive.
        assert_eq!(attribute(&mut drives, &games[..2]).xbox, Some(1));

        // Measured and still without a size: not waiting for anything.
        let mut protected = game(Store::Xbox, "5", Some(r"C:\Program Files\WindowsApps\X"), None);
        protected.size_checked_at = Some(1);
        let found = attribute(&mut drives, &[protected]);
        assert_eq!((found.unknown, found.unreadable), (0, 1));
    }

    #[test]
    fn manifest_sizes_are_never_walked_and_walked_ones_expire() {
        let ts = 1_000_000;
        let fresh = game(Store::Steam, "1", Some("x"), Some(5));
        assert!(!is_due(&fresh, ts));
        assert!(is_due(&game(Store::Gog, "2", Some("x"), None), ts));
        let mut walked = game(Store::Gog, "3", Some("x"), Some(5));
        walked.size_checked_at = Some(ts - 60);
        assert!(!is_due(&walked, ts));
        walked.size_checked_at = Some(ts - MAX_AGE_SECS - 1);
        assert!(is_due(&walked, ts));
    }

    #[test]
    fn emulator_roms_and_exes_locate_by_their_launch() {
        let rom = Game::new(
            Store::Emulator,
            "psx:1",
            "Crash",
            LaunchSpec::Exe { path: "/ra/retroarch.exe".into(), args: vec!["-L".into(), "core".into(), "/roms/psx/crash.chd".into()], cwd: None },
        );
        assert_eq!(location(&rom), Some(PathBuf::from("/roms/psx/crash.chd")));
        let steam = game(Store::Steam, "1", None, None);
        assert_eq!(location(&steam), None);
    }

    #[test]
    fn never_walks_roots_or_system_folders() {
        assert!(!walkable(Path::new(r"C:\")));
        assert!(!walkable(Path::new(r"C:\Windows")));
        assert!(!walkable(Path::new(r"C:\Program Files (x86)")));
        assert!(walkable(Path::new(r"C:\Program Files (x86)\Steam\steamapps\common\X")));
        assert!(walkable(Path::new(r"D:\Games")));
        assert!(!walkable(Path::new(r"C:\Windows\System32")));
        assert!(!walkable(Path::new(r"C:\Users\max\Desktop")));
        assert!(walkable(Path::new(r"C:\Users\max\Desktop\Celeste")));
        assert!(walkable(Path::new(r"C:\Users\max\AppData\Local\Programs\Game")));
    }

    #[test]
    fn folders_holding_other_games_are_not_walked() {
        let others = vec![
            PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Hades"),
            PathBuf::from(r"C:\Program Files (x86)\Steam"),
            PathBuf::from(r"D:\Games\Celeste"),
        ];
        assert!(holds_other_games(Path::new(r"C:\Program Files (x86)\Steam"), &others));
        assert!(holds_other_games(Path::new(r"d:\games\"), &others));
        assert!(!holds_other_games(Path::new(r"D:\Games\Celeste"), &others), "a game's own folder");
        assert!(!holds_other_games(Path::new(r"D:\Games\Cel"), &others), "a prefix by text only");
    }

    #[test]
    fn walks_without_following_links_and_finds_cleanup_candidates() {
        let tmp = std::env::temp_dir().join(format!("gm-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let game_dir = tmp.join("game");
        fs::create_dir_all(game_dir.join("sub")).unwrap();
        fs::write(game_dir.join("a.bin"), [0u8; 1000]).unwrap();
        fs::write(game_dir.join("sub").join("b.bin"), [0u8; 24]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&game_dir, game_dir.join("loop")).unwrap();
        assert_eq!(walk(&game_dir), Walk { bytes: 1024, partial: false });
        // The game folder itself moved elsewhere, a link left in its place.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&game_dir, tmp.join("moved")).unwrap();
            assert_eq!(walk(&tmp.join("moved")), Walk { bytes: 1024, partial: false });
        }
        assert_eq!(walk(&tmp.join("missing")), Walk { bytes: 0, partial: true });

        // Artwork: the folder of a game that is gone.
        let art = tmp.join("artwork");
        fs::create_dir_all(art.join("steam_1")).unwrap();
        fs::create_dir_all(art.join("steam_2")).unwrap();
        let keep: HashSet<String> = ["steam_1".to_string()].into();
        assert_eq!(orphan_artwork(&art, &keep), vec![art.join("steam_2")]);

        // Backups: of four automatic ones keep the newest three; manual ones stay.
        let backups = tmp.join("backups").join("steam_1");
        for (name, created, automatic) in [("10-auto", 10, true), ("20-auto", 20, true), ("30-auto", 30, true), ("40-auto", 40, true), ("5", 5, false)] {
            let dir = backups.join(name);
            fs::create_dir_all(&dir).unwrap();
            let meta = crate::saves::BackupMeta { game_id: "steam:1".into(), created, automatic, items: Vec::new() };
            fs::write(dir.join("backup.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        }
        assert_eq!(old_auto_backups(&tmp.join("backups"), 3), vec![backups.join("10-auto")]);
        fs::remove_dir_all(&tmp).unwrap();
    }
}
