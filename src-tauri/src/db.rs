//! Tiny JSON-file persistence. The data set (a few hundred games, a few
//! thousand sessions) is small enough that rewriting one file is simpler and
//! sturdier than shipping a database engine.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{now, Collection, Game, ManualGame, Profile, ProfileState, Session, Settings};

const PROFILE_COLORS: &[&str] = &["#5ef2c8", "#4aa3ff", "#f5b942", "#ff6b9a", "#b46ddc", "#6fd03b"];

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Data {
    pub games: Vec<Game>,
    pub manual_games: Vec<ManualGame>,
    pub sessions: Vec<Session>,
    pub settings: Settings,
    /// `artwork::PIPELINE_VERSION` the stored artwork lookups were made with.
    pub artwork_version: u32,
    pub profiles: Vec<Profile>,
    pub active_profile: String,
    pub profile_state: HashMap<String, ProfileState>,
    /// `metadata::PIPELINE_VERSION` the stored metadata lookups were made with.
    pub meta_version: u32,
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
        let mut data = data;
        data.ensure_profiles();
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

    /// Same as `upgrade_artwork`, for the store metadata pipeline.
    pub fn upgrade_meta(&mut self, version: u32) -> bool {
        if self.meta_version >= version {
            return false;
        }
        for game in self.games.iter_mut().filter(|g| g.meta.is_none()) {
            game.meta_checked_at = None;
        }
        self.meta_version = version;
        true
    }

    pub fn record_session(&mut self, mut session: Session) {
        if let Some(game) = self.game_mut(&session.game_id) {
            game.playtime_secs += session.seconds;
            game.last_played = Some(session.end);
        }
        session.profile_id = self.active_profile.clone();
        self.sessions.push(session);
    }

    /// Creates the first profile and moves pre-profile state into it.
    pub fn ensure_profiles(&mut self) {
        if !self.profiles.is_empty() {
            if !self.profiles.iter().any(|p| p.id == self.active_profile) {
                self.active_profile = self.profiles[0].id.clone();
            }
            return;
        }
        let first = Profile { id: "p1".into(), name: "Giocatore 1".into(), color: PROFILE_COLORS[0].into() };
        let state = ProfileState {
            favorites: self.games.iter().filter(|g| g.favorite).map(|g| g.id.clone()).collect(),
            hidden: self.games.iter().filter(|g| g.hidden).map(|g| g.id.clone()).collect(),
            collections: Vec::new(),
        };
        for session in self.sessions.iter_mut().filter(|s| s.profile_id.is_empty()) {
            session.profile_id = first.id.clone();
        }
        self.active_profile = first.id.clone();
        self.profile_state.insert(first.id.clone(), state);
        self.profiles.push(first);
    }

    pub fn state(&self) -> ProfileState {
        self.profile_state.get(&self.active_profile).cloned().unwrap_or_default()
    }

    pub fn state_mut(&mut self) -> &mut ProfileState {
        self.profile_state.entry(self.active_profile.clone()).or_default()
    }

    /// The library as the active profile sees it: its own favorites,
    /// hidden games and playtime.
    pub fn view(&self) -> Vec<Game> {
        let state = self.state();
        let mut played: HashMap<&str, (u64, i64)> = HashMap::new();
        for s in self.sessions.iter().filter(|s| s.profile_id == self.active_profile) {
            let entry = played.entry(s.game_id.as_str()).or_default();
            entry.0 += s.seconds;
            entry.1 = entry.1.max(s.end);
        }
        self.games
            .iter()
            .map(|g| {
                let mut g = g.clone();
                g.favorite = state.favorites.contains(&g.id);
                g.hidden = state.hidden.contains(&g.id);
                let (secs, last) = played.get(g.id.as_str()).copied().unwrap_or_default();
                g.playtime_secs = secs;
                g.last_played = (last > 0).then_some(last);
                g
            })
            .collect()
    }

    pub fn view_game(&self, id: &str) -> Option<Game> {
        self.game(id)?;
        self.view().into_iter().find(|g| g.id == id)
    }

    pub fn add_profile(&mut self, name: &str) -> Profile {
        let id = format!("p{}", now());
        let color = PROFILE_COLORS[self.profiles.len() % PROFILE_COLORS.len()].to_string();
        let profile = Profile { id: id.clone(), name: name.trim().to_string(), color };
        self.profiles.push(profile.clone());
        self.profile_state.insert(id, ProfileState::default());
        profile
    }

    /// Removes a profile and its state; the last profile can't go.
    pub fn remove_profile(&mut self, id: &str) -> bool {
        if self.profiles.len() <= 1 || !self.profiles.iter().any(|p| p.id == id) {
            return false;
        }
        self.profiles.retain(|p| p.id != id);
        self.profile_state.remove(id);
        self.sessions.retain(|s| s.profile_id != id);
        if self.active_profile == id {
            self.active_profile = self.profiles[0].id.clone();
        }
        true
    }

    pub fn toggle_in(list: &mut Vec<String>, id: &str, on: bool) {
        list.retain(|x| x != id);
        if on {
            list.push(id.to_string());
        }
    }

    pub fn add_collection(&mut self, name: &str) -> Collection {
        let collection = Collection { id: format!("c{}", now()), name: name.trim().to_string(), game_ids: Vec::new() };
        self.state_mut().collections.push(collection.clone());
        collection
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
    fn profiles_have_their_own_favorites_and_playtime() {
        let mut data = Data::default();
        let mut a = game("a");
        a.favorite = true;
        data.games = vec![a, game("b")];
        data.sessions.push(Session { game_id: "mock:a".into(), profile_id: String::new(), start: 0, end: 50, seconds: 50 });
        data.ensure_profiles();

        // Legacy state lands in the first profile.
        let view = data.view();
        assert!(view[0].favorite);
        assert_eq!(view[0].playtime_secs, 50);

        let second = data.add_profile(" Luca ");
        assert_eq!(second.name, "Luca");
        data.active_profile = second.id.clone();
        let view = data.view();
        assert!(!view[0].favorite);
        assert_eq!(view[0].playtime_secs, 0);

        data.record_session(Session { game_id: "mock:b".into(), profile_id: String::new(), start: 0, end: 90, seconds: 30 });
        assert_eq!(data.view_game("mock:b").unwrap().playtime_secs, 30);
        assert_eq!(data.view_game("mock:b").unwrap().last_played, Some(90));

        assert!(data.remove_profile(&second.id));
        assert_eq!(data.active_profile, "p1");
        assert!(!data.remove_profile("p1"), "the last profile stays");
    }

    #[test]
    fn session_updates_playtime() {
        let mut data = Data::default();
        data.games = vec![game("a")];
        data.record_session(Session { game_id: "mock:a".into(), profile_id: String::new(), start: 10, end: 70, seconds: 60 });
        let a = data.game("mock:a").unwrap();
        assert_eq!(a.playtime_secs, 60);
        assert_eq!(a.last_played, Some(70));
    }
}
