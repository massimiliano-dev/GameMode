//! GOG games found by their `goggame-<id>.info` file, the manifest every
//! GOG installer leaves in the game folder. Covers games installed without
//! GOG Galaxy (GameMode's own installs, offline installers) that the
//! registry scan misses.

// Only reached from the Windows scan (and the tests).
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::{Game, LaunchSpec, Store};

#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Info {
    pub game_id: String,
    pub root_game_id: Option<String>,
    pub name: String,
    pub play_tasks: Vec<PlayTask>,
}

#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayTask {
    pub is_primary: bool,
    #[serde(rename = "type")]
    pub kind: String,
    pub path: Option<String>,
    pub arguments: Option<String>,
    pub working_dir: Option<String>,
}

pub fn parse(json: &str) -> Option<Info> {
    serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()
}

/// The game in `dir` described by `info`; `None` for DLC and for games
/// without a file to start.
pub fn game_from_info(info: &Info, dir: &Path) -> Option<Game> {
    if info.game_id.is_empty() || info.root_game_id.as_deref().is_some_and(|root| !root.is_empty() && root != info.game_id) {
        return None;
    }
    let files = || info.play_tasks.iter().filter(|t| t.kind == "FileTask" && t.path.as_deref().is_some_and(|p| !p.is_empty()));
    let task = files().find(|t| t.is_primary).or_else(|| files().next())?;
    let relative = |p: &str| dir.join(p.replace('\\', std::path::MAIN_SEPARATOR_STR));
    let launch = LaunchSpec::Exe {
        path: relative(task.path.as_deref()?),
        args: task.arguments.as_deref().map(|a| a.split_whitespace().map(String::from).collect()).unwrap_or_default(),
        cwd: Some(task.working_dir.as_deref().filter(|w| !w.is_empty()).map(relative).unwrap_or_else(|| dir.to_path_buf())),
    };
    let name = if info.name.trim().is_empty() { dir.file_name()?.to_string_lossy().to_string() } else { info.name.trim().to_string() };
    Some(Game::new(Store::Gog, &info.game_id, name, launch).with_install_dir(dir))
}

/// The base game installed in `dir`, if any.
pub fn game_in(dir: &Path) -> Option<Game> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut infos: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("goggame-") && n.ends_with(".info")))
        .collect();
    infos.sort();
    infos
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|json| parse(&json))
        .find_map(|info| game_from_info(&info, dir))
}

/// Games directly inside each of `roots` (`<root>\<game>\goggame-*.info`).
pub fn scan(roots: &[PathBuf]) -> Vec<Game> {
    let mut games: Vec<Game> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for dir in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            if let Some(game) = game_in(&dir) {
                if !games.iter().any(|g| g.id == game.id) {
                    games.push(game);
                }
            }
        }
    }
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    const WITCHER: &str = r#"{
      "buildId": "1", "clientId": "x", "gameId": "1207664643", "rootGameId": "1207664643",
      "language": "English", "name": "The Witcher 3: Wild Hunt",
      "playTasks": [
        {"category": "document", "name": "Manual", "path": "manual.pdf", "type": "FileTask"},
        {"category": "game", "isPrimary": true, "name": "The Witcher 3", "path": "bin\\x64\\witcher3.exe",
         "arguments": "-debugscripts -net", "workingDir": "bin\\x64", "type": "FileTask"},
        {"category": "tool", "name": "Site", "link": "https://gog.com", "type": "URLTask"}
      ]
    }"#;

    #[test]
    fn reads_the_primary_play_task() {
        let dir = PathBuf::from("/games/Witcher 3");
        let game = game_from_info(&parse(WITCHER).unwrap(), &dir).unwrap();
        assert_eq!(game.id, "gog:1207664643");
        assert_eq!(game.name, "The Witcher 3: Wild Hunt");
        let LaunchSpec::Exe { path, args, cwd } = &game.launch else { panic!() };
        assert!(path.to_string_lossy().ends_with("witcher3.exe"));
        assert!(path.starts_with(&dir));
        assert_eq!(args, &["-debugscripts", "-net"]);
        assert!(cwd.as_ref().unwrap().to_string_lossy().ends_with("x64"));
    }

    #[test]
    fn skips_dlc_and_games_without_an_executable() {
        let dlc = r#"{"gameId": "2", "rootGameId": "1207664643", "name": "DLC",
                      "playTasks": [{"isPrimary": true, "path": "x.exe", "type": "FileTask"}]}"#;
        assert!(game_from_info(&parse(dlc).unwrap(), Path::new("/g")).is_none());
        let nothing = r#"{"gameId": "3", "name": "Links only", "playTasks": [{"type": "URLTask"}]}"#;
        assert!(game_from_info(&parse(nothing).unwrap(), Path::new("/g")).is_none());
        assert!(parse("not json").is_none());
    }

    #[test]
    fn scans_game_folders_under_a_root() {
        let root = std::env::temp_dir().join(format!("gm-goginfo-{}", std::process::id()));
        let game = root.join("The Witcher 3");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(root.join("Empty")).unwrap();
        std::fs::write(game.join("goggame-1207664643.info"), WITCHER).unwrap();
        std::fs::write(
            game.join("goggame-2.info"),
            r#"{"gameId": "2", "rootGameId": "1207664643", "name": "DLC", "playTasks": [{"isPrimary": true, "path": "x.exe", "type": "FileTask"}]}"#,
        )
        .unwrap();
        let games = scan(&[root.clone(), root.join("missing")]);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].install_dir.as_deref(), Some(game.as_path()));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
