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

/// The game's own launch, from `goggame-<id>.info` in its folder: the
/// primary play task, started directly without Galaxy.
pub fn direct_launch(install_dir: &Path, id: &str) -> Option<LaunchSpec> {
    let src = std::fs::read_to_string(install_dir.join(format!("goggame-{id}.info"))).ok()?;
    parse_info(&src, install_dir)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    #[serde(default)]
    play_tasks: Vec<PlayTask>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlayTask {
    #[serde(default)]
    is_primary: bool,
    #[serde(rename = "type", default)]
    kind: String,
    path: Option<String>,
    arguments: Option<String>,
    working_dir: Option<String>,
}

/// Reads a `goggame-<id>.info` file: its primary `FileTask` becomes an exe
/// launch; `path` and `workingDir` are relative to the game folder (which is
/// also the working directory when none is given).
pub fn parse_info(src: &str, install_dir: &Path) -> Option<LaunchSpec> {
    let info: Info = serde_json::from_str(src.trim_start_matches('\u{feff}')).ok()?;
    let task = info.play_tasks.into_iter().find(|t| t.is_primary && (t.kind.is_empty() || t.kind == "FileTask"))?;
    let path = task.path.filter(|p| !p.trim().is_empty())?;
    let cwd = task.working_dir.filter(|w| !w.trim().is_empty()).map_or_else(|| install_dir.to_path_buf(), |w| in_dir(install_dir, &w));
    Some(LaunchSpec::Exe {
        path: in_dir(install_dir, &path),
        args: task.arguments.as_deref().map(split_args).unwrap_or_default(),
        cwd: Some(cwd),
    })
}

/// `rel` inside the game folder (GOG writes Windows separators).
fn in_dir(dir: &Path, rel: &str) -> PathBuf {
    let rel = Path::new(rel);
    if rel.is_absolute() {
        return rel.to_path_buf();
    }
    let mut out = dir.to_path_buf();
    out.extend(rel.to_string_lossy().split(['\\', '/']).filter(|p| !p.is_empty() && *p != "."));
    out
}

/// Splits a command line into arguments, keeping quoted parts together.
fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut started = false;
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    args
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

    #[test]
    fn reads_the_primary_play_task() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/gog/goggame-1207664643.info");
        let src = std::fs::read_to_string(fixture).unwrap();
        let dir = PathBuf::from("/games/witcher3");
        let spec = parse_info(&src, &dir).unwrap();
        assert_eq!(
            spec,
            LaunchSpec::Exe {
                path: dir.join("bin").join("x64").join("witcher3.exe"),
                args: vec!["-skipIntro".into(), "-config".into(), "my settings.ini".into()],
                cwd: Some(dir.join("bin").join("x64")),
            }
        );
    }

    #[test]
    fn play_task_defaults() {
        let dir = PathBuf::from("/games/hm");
        // No arguments nor working folder: the game folder; the primary
        // task wins over the ones listed before it.
        let src = r#"{"playTasks":[{"type":"FileTask","path":"Manual.pdf","category":"document"},{"isPrimary":true,"type":"FileTask","path":"HollowKnight.exe"}]}"#;
        assert_eq!(
            parse_info(src, &dir).unwrap(),
            LaunchSpec::Exe { path: dir.join("HollowKnight.exe"), args: vec![], cwd: Some(dir.clone()) }
        );
        // A primary link task isn't something to start.
        assert!(parse_info(r#"{"playTasks":[{"isPrimary":true,"type":"URLTask","link":"https://gog.com"}]}"#, &dir).is_none());
        assert!(parse_info(r#"{"playTasks":[]}"#, &dir).is_none());
        assert!(parse_info("not json", &dir).is_none());
        // A byte order mark in front is fine.
        assert!(parse_info("\u{feff}{\"playTasks\":[{\"isPrimary\":true,\"path\":\"a.exe\"}]}", &dir).is_some());
    }

    #[test]
    fn splits_quoted_arguments() {
        assert_eq!(split_args(r#" -a  "b c" d"e f"g "" "#), ["-a", "b c", "de fg", ""]);
        assert!(split_args("   ").is_empty());
    }
}
