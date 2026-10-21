//! EA app has no stable public manifest, so games are found through their
//! uninstall entries and the `__Installer\installerdata.xml` every EA install
//! ships, which carries the offer ids the EA app understands.
// Parsers and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, Store};

pub struct EaProvider;

impl LibraryProvider for EaProvider {
    fn store(&self) -> Store {
        Store::Ea
    }

    #[cfg(windows)]
    fn scan(&self) -> Result<Vec<Game>> {
        use std::path::PathBuf;
        let mut games: Vec<Game> = Vec::new();
        for (_, v) in super::registry::uninstall_entries() {
            let Some(location) = v.get("InstallLocation") else { continue };
            let dir = PathBuf::from(location.trim_matches('"'));
            let Ok(xml) = std::fs::read_to_string(dir.join("__Installer").join("installerdata.xml")) else {
                continue;
            };
            let Some(data) = parse_installer_data(&xml) else { continue };
            let name = v.get("DisplayName").cloned().or(data.title.clone());
            if let Some(game) = game(&data, name.as_deref(), &dir) {
                if !games.iter().any(|g| g.id == game.id) {
                    games.push(game);
                }
            }
        }
        Ok(games)
    }

    #[cfg(not(windows))]
    fn scan(&self) -> Result<Vec<Game>> {
        Ok(Vec::new())
    }
}

#[derive(Debug, PartialEq)]
pub struct InstallerData {
    pub title: Option<String>,
    pub content_ids: Vec<String>,
}

pub fn parse_installer_data(xml: &str) -> Option<InstallerData> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}')).ok()?;
    let content_ids: Vec<String> = doc
        .descendants()
        .filter(|n| n.has_tag_name("contentID"))
        .filter_map(|n| n.text().map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .collect();
    let titles: Vec<_> = doc.descendants().filter(|n| n.has_tag_name("gameTitle")).collect();
    let title = titles
        .iter()
        .find(|n| n.attribute("locale") == Some("en_US"))
        .or(titles.first())
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string());
    (!content_ids.is_empty()).then_some(InstallerData { title, content_ids })
}

pub fn game(data: &InstallerData, name: Option<&str>, install_dir: &std::path::Path) -> Option<Game> {
    let name = name.or(data.title.as_deref())?;
    let uri = format!("origin2://game/launch?offerIds={}&autoDownload=1", data.content_ids.join(","));
    Some(Game::new(Store::Ea, &data.content_ids[0], name.trim(), LaunchSpec::Uri { uri }).with_install_dir(install_dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_installer_data() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ea/installerdata.xml");
        let data = parse_installer_data(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(data.title.as_deref(), Some("Apex Legends"));
        assert_eq!(data.content_ids, ["196716", "1035052"]);

        let g = game(&data, None, Path::new("C:/EA/Apex")).unwrap();
        assert_eq!(g.id, "ea:196716");
        assert_eq!(
            g.launch,
            LaunchSpec::Uri { uri: "origin2://game/launch?offerIds=196716,1035052&autoDownload=1".into() }
        );
    }
}
