//! Tiny JSON-file persistence. The data set (a few hundred games, a few
//! thousand sessions) is small enough that rewriting one file is simpler and
//! sturdier than shipping a database engine.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{now, Game, ManualGame, Session, Settings};

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub games: Vec<Game>,
    pub manual_games: Vec<ManualGame>,
    pub sessions: Vec<Session>,
    pub settings: Settings,
    /// `artwork::PIPELINE_VERSION` the stored artwork lookups were made with.
    pub artwork_version: u32,
}

pub struct Db {
    path: PathBuf,
    data: Mutex<Data>,
}

impl Db {
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("library.json");
        let data = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
                // Keep the broken file around instead of silently losing playtime.
                eprintln!("library.json is corrupt ({err}), starting fresh");
                let _ = fs::rename(&path, dir.join("library.corrupt.json"));
                Data::default()
            }),
            Err(_) => Data::default(),
        };
        Ok(Db { path, data: Mutex::new(data) })
    }

    pub fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on the data and persists the result.
    pub fn update<T>(&self, f: impl FnOnce(&mut Data) -> T) -> Result<T> {
        let mut data = self.lock();
        let out = f(&mut data);
        self.save(&data)?;
        Ok(out)
    }

    fn save(&self, data: &Data) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(data)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

impl Data {
    pub fn game(&self, id: &str) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    pub fn game_mut(&mut self, id: &str) -> Option<&mut Game> {
        self.games.iter_mut().find(|g| g.id == id)
    }

    /// Replaces the library with a fresh scan, carrying over user state
    /// (favorites, playtime, artwork…) for games that are still installed.
    pub fn merge_scan(&mut self, scanned: Vec<Game>) {
        let mut old: HashMap<String, Game> =
            self.games.drain(..).map(|g| (g.id.clone(), g)).collect();
        let ts = now();
        self.games = scanned
            .into_iter()
            .map(|mut g| {
                match old.remove(&g.id) {
                    Some(prev) => g.inherit_user_state(&prev),
                    None => g.added_at = ts,
                }
                g
            })
            .collect();
        self.games.sort_by_key(|g| g.name.to_lowercase());
    }

    /// Clears the "already looked" marker of incomplete artwork, so a newer
    /// pipeline gets a fresh attempt at it. Returns whether anything changed.
    pub fn upgrade_artwork(&mut self, version: u32) -> bool {
        if self.artwork_version >= version {
            return false;
        }
        for game in self.games.iter_mut().filter(|g| !g.artwork.is_complete()) {
            game.artwork_checked_at = None;
        }
        self.artwork_version = version;
        true
    }

    pub fn record_session(&mut self, session: Session) {
        if let Some(game) = self.game_mut(&session.game_id) {
            game.playtime_secs += session.seconds;
            game.last_played = Some(session.end);
        }
        self.sessions.push(session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LaunchSpec, Store};

    fn game(id: &str) -> Game {
        Game::new(Store::Mock, id, id, LaunchSpec::Uri { uri: String::new() })
    }

    #[test]
    fn merge_keeps_user_state_and_drops_uninstalled() {
        let mut data = Data::default();
        let mut a = game("a");
        a.favorite = true;
        a.playtime_secs = 120;
        a.added_at = 5;
        data.games = vec![a, game("b")];

        data.merge_scan(vec![game("a"), game("c")]);

        let ids: Vec<_> = data.games.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["mock:a", "mock:c"]);
        let a = data.game("mock:a").unwrap();
        assert!(a.favorite);
        assert_eq!(a.playtime_secs, 120);
        assert_eq!(a.added_at, 5);
        assert!(data.game("mock:c").unwrap().added_at > 0);
    }

    #[test]
    fn session_updates_playtime() {
        let mut data = Data::default();
        data.games = vec![game("a")];
        data.record_session(Session { game_id: "mock:a".into(), start: 10, end: 70, seconds: 60 });
        let a = data.game("mock:a").unwrap();
        assert_eq!(a.playtime_secs, 60);
        assert_eq!(a.last_played, Some(70));
    }
}
