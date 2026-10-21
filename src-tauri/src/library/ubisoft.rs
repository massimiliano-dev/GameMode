// Parsers and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;

use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, Store};

pub struct UbisoftProvider;

impl LibraryProvider for UbisoftProvider {
    fn store(&self) -> Store {
        Store::Ubisoft
    }

    #[cfg(windows)]
    fn scan(&self) -> Result<Vec<Game>> {
        use super::registry;
        let names: std::collections::HashMap<String, String> = registry::uninstall_entries()
            .into_iter()
            .filter_map(|(key, v)| {
                let id = key.strip_prefix("Uplay Install ")?.to_string();
                Some((id, v.get("DisplayName")?.clone()))
            })
            .collect();
        Ok(registry::hklm_software(r"Ubisoft\Launcher\Installs")
            .into_iter()
            .filter_map(|(id, v)| {
                let dir = v.get("InstallDir")?;
                Path::new(dir).is_dir().then(|| game(&id, dir, names.get(&id).map(String::as_str)))
            })
            .collect())
    }

    #[cfg(not(windows))]
    fn scan(&self) -> Result<Vec<Game>> {
        Ok(Vec::new())
    }
}

pub fn game(id: &str, install_dir: &str, display_name: Option<&str>) -> Game {
    let dir = install_dir.trim_end_matches(['/', '\\']);
    let fallback = dir.rsplit(['/', '\\']).next().unwrap_or(id);
    let name = display_name.filter(|n| !n.trim().is_empty()).unwrap_or(fallback);
    Game::new(Store::Ubisoft, id, name.trim(), LaunchSpec::Uri { uri: format!("uplay://launch/{id}/0") })
        .with_install_dir(Path::new(dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_folder_name() {
        let g = game("5595", "C:/Program Files (x86)/Ubisoft/Games/Far Cry 6/", None);
        assert_eq!(g.name, "Far Cry 6");
        assert_eq!(g.launch, LaunchSpec::Uri { uri: "uplay://launch/5595/0".into() });
        assert_eq!(game("5595", "C:/x/Far Cry 6", Some("Far Cry® 6")).name, "Far Cry® 6");
    }
}
