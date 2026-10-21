//! Xbox app / Game Pass games are MSIX packages. A package counts as a game
//! when its install folder contains `MicrosoftGame.config` (GDK titles).
// Parsers and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, Store};

pub struct XboxProvider;

impl LibraryProvider for XboxProvider {
    fn store(&self) -> Store {
        Store::Xbox
    }

    #[cfg(windows)]
    fn scan(&self) -> Result<Vec<Game>> {
        scan_packages()
    }

    #[cfg(not(windows))]
    fn scan(&self) -> Result<Vec<Game>> {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
fn scan_packages() -> Result<Vec<Game>> {
    use std::fs;
    use std::path::PathBuf;

    use windows::core::HSTRING;
    use windows::Management::Deployment::PackageManager;

    use crate::model::LaunchSpec;

    let manager = PackageManager::new()?;
    let mut games = Vec::new();
    // An empty SID means "the current user".
    for package in manager.FindPackagesByUserSecurityId(&HSTRING::new())? {
        if package.IsFramework().unwrap_or(true) || package.IsResourcePackage().unwrap_or(true) {
            continue;
        }
        let Ok(dir) = package.InstalledPath().map(|p| PathBuf::from(p.to_string_lossy())) else { continue };
        let Ok(config) = fs::read_to_string(dir.join("MicrosoftGame.config")) else { continue };
        let Some(manifest) = fs::read_to_string(dir.join("AppxManifest.xml")).ok() else { continue };
        let Some(app) = parse_manifest(&manifest) else { continue };
        let Ok(family) = package.Id().and_then(|id| id.FamilyName()) else { continue };
        let family = family.to_string_lossy();

        let name = package
            .DisplayName()
            .ok()
            .map(|n| n.to_string_lossy())
            .filter(|n| is_real_name(n))
            .or_else(|| parse_config_name(&config))
            .or(app.display_name)
            .unwrap_or_else(|| family.clone());

        games.push(
            Game::new(Store::Xbox, &family, name, LaunchSpec::AppsFolder { aumid: format!("{family}!{}", app.id) })
                .with_install_dir(dir),
        );
    }
    Ok(games)
}

fn is_real_name(name: &str) -> bool {
    !name.trim().is_empty() && !name.starts_with("ms-resource:")
}

#[derive(Debug, PartialEq)]
pub struct ManifestApp {
    pub id: String,
    pub display_name: Option<String>,
}

/// First `<Application>` of an `AppxManifest.xml`.
pub fn parse_manifest(xml: &str) -> Option<ManifestApp> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}')).ok()?;
    let app = doc.descendants().find(|n| n.has_tag_name("Application"))?;
    let display_name = app
        .children()
        .find(|n| n.has_tag_name("VisualElements"))
        .and_then(|n| n.attribute("DisplayName"))
        .filter(|n| is_real_name(n))
        .map(String::from);
    Some(ManifestApp { id: app.attribute("Id")?.to_string(), display_name })
}

/// `<ShellVisuals DefaultDisplayName>` of a `MicrosoftGame.config`.
pub fn parse_config_name(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}')).ok()?;
    doc.descendants()
        .find(|n| n.has_tag_name("ShellVisuals"))?
        .attribute("DefaultDisplayName")
        .filter(|n| is_real_name(n))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> String {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xbox").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn reads_app_id_and_names() {
        let app = parse_manifest(&fixture("AppxManifest.xml")).unwrap();
        assert_eq!(app, ManifestApp { id: "ForzaHorizon5".into(), display_name: Some("Forza Horizon 5".into()) });
        assert_eq!(parse_config_name(&fixture("MicrosoftGame.config")).as_deref(), Some("Forza Horizon 5"));
    }
}
