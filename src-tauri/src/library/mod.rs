use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

use crate::model::{Game, ManualGame, Settings, Store};

pub mod apps;
pub mod ea;
pub mod emulators;
pub mod epic;
pub mod gog;
pub mod manual;
#[cfg(not(windows))]
pub mod mock;
#[cfg(windows)]
pub mod registry;
pub mod steam;
pub mod ubisoft;
pub mod vdf;
pub mod xbox;

/// Registry values of one key, as strings.
#[cfg_attr(not(windows), allow(dead_code))]
pub type Values = HashMap<String, String>;

pub trait LibraryProvider: Send + Sync {
    fn store(&self) -> Store;
    fn scan(&self) -> Result<Vec<Game>>;
}

pub fn providers(manual: Vec<ManualGame>, settings: &Settings) -> Vec<Box<dyn LibraryProvider>> {
    #[cfg_attr(windows, allow(unused_mut))]
    let mut list: Vec<Box<dyn LibraryProvider>> = vec![
        Box::new(steam::SteamProvider),
        Box::new(xbox::XboxProvider),
        Box::new(epic::EpicProvider),
        Box::new(gog::GogProvider),
        Box::new(ea::EaProvider),
        Box::new(ubisoft::UbisoftProvider),
        Box::new(manual::ManualProvider(manual)),
        Box::new(emulators::EmulatorProvider {
            retroarch: settings.retroarch_path.clone(),
            rom_dirs: settings.rom_dirs.clone(),
        }),
    ];
    #[cfg(not(windows))]
    list.push(Box::new(mock::MockProvider));
    list
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ScanError {
    pub store: Store,
    pub message: String,
}

pub struct ScanResult {
    pub games: Vec<Game>,
    pub errors: Vec<ScanError>,
}

/// Runs every enabled provider in parallel. A failing store is reported but
/// never prevents the others from showing up.
pub fn scan_all(providers: Vec<Box<dyn LibraryProvider>>, disabled: &[Store]) -> ScanResult {
    let results: Vec<(Store, Result<Vec<Game>>)> = std::thread::scope(|s| {
        let handles: Vec<_> = providers
            .iter()
            .filter(|p| !disabled.contains(&p.store()))
            .map(|p| (p.store(), s.spawn(|| p.scan())))
            .collect();
        handles
            .into_iter()
            .map(|(store, h)| {
                let result = h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("scanner crashed")));
                (store, result)
            })
            .collect()
    });

    let mut out = ScanResult { games: Vec::new(), errors: Vec::new() };
    for (store, result) in results {
        match result {
            Ok(games) => {
                for game in games {
                    if !out.games.iter().any(|g| g.id == game.id) {
                        out.games.push(game);
                    }
                }
            }
            Err(err) => out.errors.push(ScanError { store, message: format!("{err:#}") }),
        }
    }
    out
}
