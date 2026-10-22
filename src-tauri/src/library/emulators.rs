//! ROMs played through RetroArch. Each ROM folder belongs to one system,
//! which fixes the libretro core and the libretro-thumbnails collection the
//! box art comes from.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, RomDir, Store};

pub struct System {
    pub key: &'static str,
    pub name: &'static str,
    /// Folder name on thumbnails.libretro.com.
    pub thumbnails: &'static str,
    pub extensions: &'static [&'static str],
    pub core: &'static str,
}

pub const SYSTEMS: &[System] = &[
    System { key: "nes", name: "NES", thumbnails: "Nintendo - Nintendo Entertainment System", extensions: &["nes", "zip"], core: "fceumm" },
    System { key: "snes", name: "Super Nintendo", thumbnails: "Nintendo - Super Nintendo Entertainment System", extensions: &["sfc", "smc", "zip"], core: "snes9x" },
    System { key: "n64", name: "Nintendo 64", thumbnails: "Nintendo - Nintendo 64", extensions: &["n64", "z64", "v64", "zip"], core: "mupen64plus_next" },
    System { key: "gb", name: "Game Boy", thumbnails: "Nintendo - Game Boy", extensions: &["gb", "zip"], core: "gambatte" },
    System { key: "gbc", name: "Game Boy Color", thumbnails: "Nintendo - Game Boy Color", extensions: &["gbc", "zip"], core: "gambatte" },
    System { key: "gba", name: "Game Boy Advance", thumbnails: "Nintendo - Game Boy Advance", extensions: &["gba", "zip"], core: "mgba" },
    System { key: "nds", name: "Nintendo DS", thumbnails: "Nintendo - Nintendo DS", extensions: &["nds", "zip"], core: "melonds" },
    System { key: "megadrive", name: "Mega Drive", thumbnails: "Sega - Mega Drive - Genesis", extensions: &["md", "gen", "smd", "bin", "zip"], core: "genesis_plus_gx" },
    System { key: "mastersystem", name: "Master System", thumbnails: "Sega - Master System - Mark III", extensions: &["sms", "zip"], core: "genesis_plus_gx" },
    System { key: "psx", name: "PlayStation", thumbnails: "Sony - PlayStation", extensions: &["cue", "chd", "m3u", "pbp"], core: "swanstation" },
    System { key: "psp", name: "PSP", thumbnails: "Sony - PlayStation Portable", extensions: &["iso", "cso", "pbp", "chd"], core: "ppsspp" },
    System { key: "arcade", name: "Arcade (FBNeo)", thumbnails: "FBNeo - Arcade Games", extensions: &["zip"], core: "fbneo" },
];

pub fn system(key: &str) -> Option<&'static System> {
    SYSTEMS.iter().find(|s| s.key == key)
}

pub struct EmulatorProvider {
    pub retroarch: Option<PathBuf>,
    pub rom_dirs: Vec<RomDir>,
}

impl LibraryProvider for EmulatorProvider {
    fn store(&self) -> Store {
        Store::Emulator
    }

    fn scan(&self) -> Result<Vec<Game>> {
        let Some(retroarch) = self.retroarch.clone().or_else(find_retroarch) else { return Ok(Vec::new()) };
        let mut games = Vec::new();
        for dir in &self.rom_dirs {
            let Some(system) = system(&dir.system) else { continue };
            for rom in roms(&dir.path, system) {
                games.push(rom_game(&retroarch, system, &rom));
            }
        }
        Ok(games)
    }
}

/// Usual RetroArch install locations on Windows.
pub fn find_retroarch() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(r"C:\RetroArch-Win64\retroarch.exe"),
        PathBuf::from(r"C:\RetroArch\retroarch.exe"),
    ];
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            candidates.push(PathBuf::from(&base).join("RetroArch").join("retroarch.exe"));
            candidates.push(PathBuf::from(&base).join("Programs").join("RetroArch").join("retroarch.exe"));
        }
    }
    if let Some(steam) = super::steam::find_steam_root() {
        candidates.push(steam.join("steamapps").join("common").join("RetroArch").join("retroarch.exe"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub fn core_path(retroarch: &Path, system: &System) -> PathBuf {
    let dir = retroarch.parent().unwrap_or(Path::new("."));
    dir.join("cores").join(format!("{}_libretro.dll", system.core))
}

/// ROM files of `system` in `dir` (and one level of subfolders, for multi-disc games).
pub fn roms(dir: &Path, system: &System) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let visit = |d: &Path, found: &mut Vec<PathBuf>| {
        let Ok(entries) = fs::read_dir(d) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if path.is_file() && system.extensions.contains(&ext.as_str()) {
                found.push(path);
            }
        }
    };
    visit(dir, &mut found);
    if let Ok(entries) = fs::read_dir(dir) {
        for sub in entries.flatten().filter(|e| e.path().is_dir()) {
            visit(&sub.path(), &mut found);
        }
    }
    // A playlist covers its discs: don't list them twice.
    let playlists: Vec<PathBuf> = found.iter().filter(|p| has_ext(p, "m3u")).cloned().collect();
    if !playlists.is_empty() {
        found.retain(|p| has_ext(p, "m3u") || !playlists.iter().any(|m| m.parent() == p.parent()));
    }
    found.sort();
    found
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn rom_game(retroarch: &Path, system: &System, rom: &Path) -> Game {
    let stem = rom.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let id = format!("{}:{:016x}", system.key, fnv1a(&rom.to_string_lossy()));
    let launch = LaunchSpec::Exe {
        path: retroarch.to_path_buf(),
        args: vec![
            "-f".into(),
            "-L".into(),
            core_path(retroarch, system).to_string_lossy().into_owned(),
            rom.to_string_lossy().into_owned(),
        ],
        cwd: retroarch.parent().map(Path::to_path_buf),
    };
    let mut game = Game::new(Store::Emulator, &id, display_name(&stem), launch);
    game.cover_url = Some(thumbnail_url(system, &stem));
    game
}

/// "Super Mario World (USA) [!]" -> "Super Mario World".
pub fn display_name(stem: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in stem.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let name = out.replace('_', " ");
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() { stem.to_string() } else { name }
}

/// libretro-thumbnails box art, which is named after the No-Intro ROM name.
pub fn thumbnail_url(system: &System, stem: &str) -> String {
    let safe: String = stem.chars().map(|c| if "&*/:`<>?\\|\"".contains(c) { '_' } else { c }).collect();
    format!(
        "https://thumbnails.libretro.com/{}/Named_Boxarts/{}.png",
        encode(system.thumbnails),
        encode(&safe)
    )
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    pub key: String,
    pub name: String,
    pub path: Option<PathBuf>,
    pub core: String,
    pub core_installed: bool,
    pub roms: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EmulatorStatus {
    pub retroarch: Option<PathBuf>,
    pub detected: bool,
    pub systems: Vec<SystemStatus>,
}

pub fn status(configured: Option<PathBuf>, dirs: &[RomDir]) -> EmulatorStatus {
    let detected = configured.is_none();
    let retroarch = configured.filter(|p| p.is_file()).or_else(find_retroarch);
    let systems = SYSTEMS
        .iter()
        .map(|s| {
            let path = dirs.iter().find(|d| d.system == s.key).map(|d| d.path.clone());
            SystemStatus {
                key: s.key.into(),
                name: s.name.into(),
                roms: path.as_deref().map(|p| roms(p, s).len()).unwrap_or(0),
                path,
                core: s.core.into(),
                core_installed: retroarch.as_deref().is_some_and(|r| core_path(r, s).is_file()),
            }
        })
        .collect();
    EmulatorStatus { retroarch, detected, systems }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_rom_names_and_builds_thumbnail_urls() {
        assert_eq!(display_name("Super Mario World (USA) [!]"), "Super Mario World");
        assert_eq!(display_name("legend_of_zelda"), "legend of zelda");
        let snes = system("snes").unwrap();
        assert_eq!(
            thumbnail_url(snes, "Mario & Luigi (USA)"),
            "https://thumbnails.libretro.com/Nintendo%20-%20Super%20Nintendo%20Entertainment%20System/Named_Boxarts/Mario%20_%20Luigi%20%28USA%29.png"
        );
    }

    #[test]
    fn finds_roms_and_prefers_playlists_over_discs() {
        let dir = std::env::temp_dir().join(format!("gm-roms-{}", std::process::id()));
        let game = dir.join("Final Fantasy VII");
        fs::create_dir_all(&game).unwrap();
        for f in ["Final Fantasy VII.m3u", "Disc 1.cue", "Disc 2.cue"] {
            fs::write(game.join(f), b"").unwrap();
        }
        fs::write(dir.join("Crash Bandicoot (Europe).chd"), b"").unwrap();
        fs::write(dir.join("readme.txt"), b"").unwrap();

        let psx = system("psx").unwrap();
        let found: Vec<String> = roms(&dir, psx).iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
        assert_eq!(found, ["Crash Bandicoot (Europe).chd", "Final Fantasy VII.m3u"]);

        let g = rom_game(Path::new("/ra/retroarch.exe"), psx, &dir.join("Crash Bandicoot (Europe).chd"));
        assert_eq!(g.name, "Crash Bandicoot");
        assert!(g.id.starts_with("emulator:psx:"));
        let LaunchSpec::Exe { args, .. } = &g.launch else { panic!() };
        assert!(args[2].ends_with("swanstation_libretro.dll"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
