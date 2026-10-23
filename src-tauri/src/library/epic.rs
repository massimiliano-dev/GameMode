use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, Store};

pub struct EpicProvider;

impl LibraryProvider for EpicProvider {
    fn store(&self) -> Store {
        Store::Epic
    }

    fn scan(&self) -> Result<Vec<Game>> {
        let Some(program_data) = std::env::var_os("ProgramData") else { return Ok(Vec::new()) };
        let dir = PathBuf::from(program_data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
        scan_manifests(&dir)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Manifest {
    display_name: String,
    install_location: String,
    app_name: String,
    catalog_namespace: String,
    catalog_item_id: String,
    #[serde(default)]
    main_game_app_name: Option<String>,
    #[serde(default)]
    app_categories: Vec<String>,
    #[serde(default, rename = "bIsIncompleteInstall")]
    is_incomplete_install: bool,
    /// Bytes on disk, written by the launcher after the install.
    #[serde(default)]
    install_size: Option<u64>,
}

pub fn scan_manifests(dir: &Path) -> Result<Vec<Game>> {
    let Ok(entries) = fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut games = Vec::new();
    for entry in entries.flatten() {
        if entry.path().extension().is_some_and(|e| e == "item") {
            if let Some(game) = fs::read_to_string(entry.path()).ok().and_then(|s| parse_manifest(&s)) {
                games.push(game);
            }
        }
    }
    Ok(games)
}

pub fn parse_manifest(src: &str) -> Option<Game> {
    let m: Manifest = serde_json::from_str(src).ok()?;
    let is_dlc = m.main_game_app_name.as_deref().is_some_and(|main| main != m.app_name);
    // Unreal Engine installs and plugins are listed alongside games.
    let is_game = m.app_categories.is_empty() || m.app_categories.iter().any(|c| c == "games");
    if m.is_incomplete_install || is_dlc || !is_game {
        return None;
    }
    let uri = format!(
        "com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true",
        m.catalog_namespace, m.catalog_item_id, m.app_name
    );
    let mut game = Game::new(Store::Epic, &m.app_name, m.display_name.trim(), LaunchSpec::Uri { uri })
        .with_install_dir(m.install_location);
    game.size_bytes = m.install_size.filter(|&b| b > 0);
    Some(game)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_game_manifests_and_skips_engine_and_dlc() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/epic");
        let games = scan_manifests(&dir).unwrap();
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(g.id, "epic:Fortnite");
        assert_eq!(g.name, "Fortnite");
        assert_eq!(g.size_bytes, Some(42_949_672_960));
        assert_eq!(
            g.launch,
            LaunchSpec::Uri {
                uri: "com.epicgames.launcher://apps/fn%3A4fe75bbc5a674f4f9b356b5c90567da5%3AFortnite?action=launch&silent=true".into()
            }
        );
    }
}
