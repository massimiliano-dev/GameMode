// Parsers and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{LibraryProvider, Values};
use crate::model::{Game, LaunchSpec, Store};

pub struct GogProvider {
    /// Folders holding games installed without Galaxy (`gog_info`).
    pub roots: Vec<PathBuf>,
}

impl LibraryProvider for GogProvider {
    fn store(&self) -> Store {
        Store::Gog
    }

    #[cfg(windows)]
    fn scan(&self) -> Result<Vec<Game>> {
        use super::registry;
        let galaxy = registry::hklm_value(r"GOG.com\GalaxyClient\paths", "client")
            .map(|dir| PathBuf::from(dir).join("GalaxyClient.exe"))
            .filter(|p| p.is_file());
        let mut games: Vec<Game> = registry::hklm_software(r"GOG.com\Games")
            .iter()
            .filter_map(|(_, v)| game_from_entry(v, galaxy.as_deref()))
            .collect();
        for game in super::gog_info::scan(&self.roots) {
            if !games.iter().any(|g| g.id == game.id) {
                games.push(game);
            }
        }
        Ok(games)
    }

    #[cfg(not(windows))]
    fn scan(&self) -> Result<Vec<Game>> {
        Ok(Vec::new())
    }
}

/// Builds a game from a `HKLM\SOFTWARE\GOG.com\Games\<id>` entry. Launching
/// through Galaxy keeps cloud saves and overlay; without it the exe is used.
pub fn game_from_entry(v: &Values, galaxy: Option<&Path>) -> Option<Game> {
    let id = v.get("gameID")?;
    let name = v.get("gameName")?;
    let path = v.get("path")?;
    if v.get("dependsOn").is_some_and(|d| !d.trim().is_empty()) {
        return None; // DLC
    }
    let launch = match galaxy {
        Some(client) => LaunchSpec::Exe {
            path: client.to_path_buf(),
            args: vec!["/command=runGame".into(), format!("/gameId={id}"), format!("/path={path}")],
            cwd: client.parent().map(Path::to_path_buf),
        },
        None => LaunchSpec::Exe {
            path: PathBuf::from(v.get("exe")?),
            args: v
                .get("launchParam")
                .map(|p| p.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            cwd: Some(PathBuf::from(v.get("workingDir").filter(|w| !w.is_empty()).unwrap_or(path))),
        },
    };
    Some(Game::new(Store::Gog, id, name.trim(), launch).with_install_dir(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pairs: &[(&str, &str)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn prefers_galaxy_and_skips_dlc() {
        let v = entry(&[
            ("gameID", "1207664643"),
            ("gameName", "The Witcher 3"),
            ("path", r"C:\GOG Games\The Witcher 3"),
            ("exe", r"C:\GOG Games\The Witcher 3\bin\x64\witcher3.exe"),
        ]);
        let galaxy = PathBuf::from(r"C:\Galaxy\GalaxyClient.exe");
        let g = game_from_entry(&v, Some(&galaxy)).unwrap();
        assert_eq!(g.id, "gog:1207664643");
        let LaunchSpec::Exe { args, .. } = &g.launch else { panic!() };
        assert_eq!(args[1], "/gameId=1207664643");

        let direct = game_from_entry(&v, None).unwrap();
        let LaunchSpec::Exe { path, .. } = &direct.launch else { panic!() };
        assert!(path.to_string_lossy().ends_with("witcher3.exe"));

        let mut dlc = v.clone();
        dlc.insert("dependsOn".into(), "1207664643".into());
        assert!(game_from_entry(&dlc, None).is_none());
    }
}
