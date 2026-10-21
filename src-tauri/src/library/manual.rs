use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, ManualGame, Store};

/// Executables (games, emulators with ROM arguments…) added by hand.
pub struct ManualProvider(pub Vec<ManualGame>);

impl LibraryProvider for ManualProvider {
    fn store(&self) -> Store {
        Store::Manual
    }

    fn scan(&self) -> Result<Vec<Game>> {
        Ok(self.0.iter().map(to_game).collect())
    }
}

fn to_game(m: &ManualGame) -> Game {
    let cwd = m.path.parent().map(|p| p.to_path_buf());
    let mut game = Game::new(
        Store::Manual,
        &m.id,
        m.name.clone(),
        LaunchSpec::Exe { path: m.path.clone(), args: m.args.clone(), cwd: cwd.clone() },
    );
    game.install_dir = cwd;
    game
}
