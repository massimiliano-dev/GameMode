use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{vdf, LibraryProvider};
use crate::model::{Game, LaunchSpec, Store};

/// Steam tools that show up as installed apps but aren't games.
const NON_GAME_APPIDS: &[u32] = &[
    228980,  // Steamworks Common Redistributables
    250820,  // SteamVR
    1007,    // Steam client
    1070560, // Steam Linux Runtime
    1391110, // Steam Linux Runtime - Soldier
    1628350, // Steam Linux Runtime - Sniper
    1493710, // Proton Experimental
    2180100, // Proton Hotfix
];

const STATE_FULLY_INSTALLED: u32 = 4;

pub struct SteamProvider;

impl LibraryProvider for SteamProvider {
    fn store(&self) -> Store {
        Store::Steam
    }

    fn scan(&self) -> Result<Vec<Game>> {
        match find_steam_root() {
            Some(root) => scan_root(&root),
            None => Ok(Vec::new()),
        }
    }
}

pub fn find_steam_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let from_registry = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Valve\Steam")
            .and_then(|k| k.get_value::<String, _>("SteamPath"))
            .ok()
            .map(PathBuf::from);
        from_registry
            .into_iter()
            .chain([PathBuf::from(r"C:\Program Files (x86)\Steam")])
            .find(|p| p.join("steamapps").is_dir())
    }
    #[cfg(not(windows))]
    {
        let home = PathBuf::from(std::env::var_os("HOME")?);
        [
            home.join("Library/Application Support/Steam"),
            home.join(".local/share/Steam"),
            home.join(".steam/steam"),
        ]
        .into_iter()
        .find(|p| p.join("steamapps").is_dir())
    }
}

/// All library folders configured in Steam, including the main install.
pub fn library_folders(root: &Path) -> Vec<PathBuf> {
    let mut folders = vec![root.to_path_buf()];
    let vdf_path = root.join("steamapps").join("libraryfolders.vdf");
    if let Some(parsed) = fs::read_to_string(vdf_path).ok().and_then(|s| vdf::parse(&s).ok()) {
        if let Some(lf) = parsed.get("libraryfolders") {
            for (_, entry) in lf.entries() {
                // Old format: "1" "D:\\SteamLibrary"; new format: "1" { "path" "..." }
                let path = entry.as_str().or_else(|| entry.get_str("path"));
                if let Some(p) = path.map(PathBuf::from) {
                    if !folders.iter().any(|f| same_dir(f, &p)) {
                        folders.push(p);
                    }
                }
            }
        }
    }
    folders
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

pub fn scan_root(root: &Path) -> Result<Vec<Game>> {
    let mut games = Vec::new();
    for folder in library_folders(root) {
        let steamapps = folder.join("steamapps");
        let Ok(entries) = fs::read_dir(&steamapps) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let src = fs::read_to_string(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?;
            if let Some(game) = parse_manifest(&src, &steamapps) {
                if !games.iter().any(|g: &Game| g.id == game.id) {
                    games.push(game);
                }
            }
        }
    }
    Ok(games)
}

pub fn parse_manifest(src: &str, steamapps: &Path) -> Option<Game> {
    let parsed = vdf::parse(src).ok()?;
    let state = parsed.get("AppState")?;
    let appid: u32 = state.get_str("appid")?.parse().ok()?;
    let name = state.get_str("name")?.trim();
    let flags: u32 = state.get_str("StateFlags").and_then(|s| s.parse().ok()).unwrap_or(0);

    if flags & STATE_FULLY_INSTALLED == 0 || NON_GAME_APPIDS.contains(&appid) || is_tool_name(name) {
        return None;
    }

    let mut game = Game::new(
        Store::Steam,
        &appid.to_string(),
        name,
        LaunchSpec::Uri { uri: format!("steam://rungameid/{appid}") },
    );
    game.steam_appid = Some(appid);
    if let Some(dir) = state.get_str("installdir") {
        game.install_dir = Some(steamapps.join("common").join(dir));
    }
    // Steam keeps the installed size in the manifest; 0 means "not yet known".
    game.size_bytes = state.get_str("SizeOnDisk").and_then(|s| s.parse::<u64>().ok()).filter(|&b| b > 0);
    Some(game)
}

fn is_tool_name(name: &str) -> bool {
    ["Proton ", "Steam Linux Runtime", "Steamworks Common", "SteamVR"]
        .iter()
        .any(|p| name.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/steam")
    }

    #[test]
    fn scans_all_library_folders_and_skips_tools() {
        let root = fixtures().join("Steam");
        // libraryfolders.vdf points at Windows paths that don't exist here,
        // so the second library is scanned directly.
        let mut games = scan_root(&root).unwrap();
        let second = fixtures().join("SecondLibrary");
        games.extend(scan_root(&second).unwrap());
        games.sort_by_key(|g| g.name.clone());

        let names: Vec<_> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Dota 2", "Hades"]);

        let dota = &games[0];
        assert_eq!(dota.id, "steam:570");
        assert_eq!(dota.steam_appid, Some(570));
        assert_eq!(dota.launch, LaunchSpec::Uri { uri: "steam://rungameid/570".into() });
        assert!(dota.install_dir.as_ref().unwrap().ends_with("common/dota 2 beta"));
        assert_eq!(dota.size_bytes, Some(39_182_648_934));
        assert!(games[1].size_bytes.is_none(), "no SizeOnDisk in the manifest");
    }

    #[test]
    fn a_zero_size_counts_as_unknown() {
        let src = r#""AppState" { "appid" "10" "name" "Counter-Strike" "StateFlags" "4" "SizeOnDisk" "0" }"#;
        assert_eq!(parse_manifest(src, Path::new("x")).unwrap().size_bytes, None);
    }

    #[test]
    fn reads_library_folder_list() {
        let folders = library_folders(&fixtures().join("Steam"));
        assert_eq!(folders.len(), 3);
        assert!(folders[2].to_string_lossy().contains("SteamLibrary"));
    }

    #[test]
    fn skips_partially_installed_apps() {
        let src = r#""AppState" { "appid" "10" "name" "Counter-Strike" "StateFlags" "1026" }"#;
        assert!(parse_manifest(src, Path::new("x")).is_none());
    }
}
