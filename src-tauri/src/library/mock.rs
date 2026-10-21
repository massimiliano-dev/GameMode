//! Fake library for developing the UI away from Windows. Steam appids are
//! real so the artwork pipeline has something to download.

use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, Store};

pub struct MockProvider;

const GAMES: &[(Store, &str, u32)] = &[
    (Store::Steam, "Elden Ring", 1245620),
    (Store::Steam, "Baldur's Gate 3", 1086940),
    (Store::Steam, "Hades II", 1145350),
    (Store::Steam, "Hollow Knight", 367520),
    (Store::Steam, "Stardew Valley", 413150),
    (Store::Steam, "Red Dead Redemption 2", 1174180),
    (Store::Xbox, "Forza Horizon 5", 1551360),
    (Store::Xbox, "Halo: The Master Chief Collection", 976730),
    (Store::Xbox, "Starfield", 1716740),
    (Store::Epic, "Alan Wake 2", 0),
    (Store::Epic, "Death Stranding", 1190460),
    (Store::Gog, "The Witcher 3: Wild Hunt", 292030),
    (Store::Gog, "Cyberpunk 2077", 1091500),
    (Store::Ubisoft, "Assassin's Creed Valhalla", 2208920),
    (Store::Ea, "Apex Legends", 1172470),
    (Store::Ea, "It Takes Two", 1426210),
];

impl LibraryProvider for MockProvider {
    fn store(&self) -> Store {
        Store::Mock
    }

    fn scan(&self) -> Result<Vec<Game>> {
        Ok(GAMES
            .iter()
            .map(|&(store, name, appid)| {
                // A short-lived process stands in for the game, so launch
                // tracking and the return-to-launcher flow can be exercised.
                let launch = LaunchSpec::Exe { path: "/bin/sleep".into(), args: vec!["20".into()], cwd: None };
                let mut game = Game::new(store, &format!("mock-{}", slug(name)), name, launch);
                game.steam_appid = (appid != 0).then_some(appid);
                game
            })
            .collect())
    }
}

fn slug(name: &str) -> String {
    name.chars()
        .filter_map(|c| if c.is_ascii_alphanumeric() { Some(c.to_ascii_lowercase()) } else if c == ' ' { Some('-') } else { None })
        .collect()
}
