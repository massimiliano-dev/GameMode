//! Uninstalling games. Store games go through their client or their own
//! registered uninstaller, so the store's bookkeeping stays right; Xbox
//! packages are removed through PackageManager.

#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::{bail, Result};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::library::Values;
use crate::model::{Game, Store};
use crate::{storefront, AppState};

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// An uninstaller or the store client took over; it may still ask questions.
    Started,
    /// The game is gone.
    Removed,
}

/// Blocking: the Xbox path waits for the removal to finish.
pub fn uninstall(app: &AppHandle, game: &Game) -> Result<Outcome> {
    match game.store {
        Store::Steam => {
            let Some(appid) = game.steam_appid else { bail!("Gioco Steam senza appid") };
            storefront::open_uri(app, &format!("steam://uninstall/{appid}"))?;
            Ok(Outcome::Started)
        }
        Store::Manual => {
            let id = native_id(game).to_string();
            app.state::<AppState>().db.update(|d| d.manual_games.retain(|m| m.id != id))?;
            Ok(Outcome::Removed)
        }
        Store::Xbox => xbox(app, native_id(game)),
        Store::Gog | Store::Ubisoft | Store::Ea | Store::Epic => registered_uninstaller(app, game),
        Store::Emulator => bail!("Per toglierlo, elimina il file della ROM dalla sua cartella"),
        Store::Mock => bail!("Non disponibile per i giochi demo"),
    }
}

/// Whether the game is still on disk (Steam: its manifest still exists).
pub fn still_installed(game: &Game) -> bool {
    let Some(dir) = game.install_dir.as_deref() else { return false };
    if game.store == Store::Steam {
        if let (Some(appid), Some(steamapps)) = (game.steam_appid, dir.parent().and_then(|p| p.parent())) {
            return steamapps.join(format!("appmanifest_{appid}.acf")).is_file();
        }
    }
    dir.exists()
}

/// Waits for a store's uninstaller to finish, then refreshes the library and
/// brings GameMode back, so nobody needs a mouse to return.
pub fn watch(app: AppHandle, game: Game) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(15 * 60) {
            std::thread::sleep(std::time::Duration::from_secs(2));
            if !still_installed(&game) {
                crate::scan(&app);
                if !crate::window::is_foreground(&app) {
                    crate::storefront::return_to_gamemode(&app);
                }
                let _ = app.emit("uninstall://done", &game.name);
                return;
            }
        }
    });
}

fn native_id(game: &Game) -> &str {
    game.id.split_once(':').map_or(game.id.as_str(), |(_, id)| id)
}

#[cfg(windows)]
fn xbox(app: &AppHandle, family: &str) -> Result<Outcome> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::PackageManager;

    let manager = PackageManager::new()?;
    let packages = manager.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family))?;
    let mut removed = false;
    for package in packages {
        let full_name = package.Id()?.FullName()?;
        match manager.RemovePackageAsync(&full_name).and_then(|op| op.join()) {
            Ok(_) => removed = true,
            Err(err) => {
                // Some Game Pass installs are owned by Gaming Services and can
                // only be removed from the Xbox app / Windows settings.
                eprintln!("RemovePackageAsync failed: {err}");
                storefront::open_uri(app, "ms-settings:appsfeatures")?;
                return Ok(Outcome::Started);
            }
        }
    }
    if !removed {
        bail!("Pacchetto non trovato");
    }
    Ok(Outcome::Removed)
}

#[cfg(not(windows))]
fn xbox(_app: &AppHandle, _family: &str) -> Result<Outcome> {
    bail!("Disponibile solo su Windows")
}

#[cfg(windows)]
fn registered_uninstaller(app: &AppHandle, game: &Game) -> Result<Outcome> {
    use std::os::windows::process::CommandExt;

    let entries = crate::library::registry::uninstall_entries();
    let Some(command) = find_uninstaller(&entries, game) else {
        bail!("Nessun programma di disinstallazione trovato: disinstallalo dal client dello store");
    };
    let (exe, args) = split_command(&command);
    std::process::Command::new(exe).raw_arg(args).spawn()?;
    crate::window::minimize(app);
    Ok(Outcome::Started)
}

#[cfg(not(windows))]
fn registered_uninstaller(_app: &AppHandle, _game: &Game) -> Result<Outcome> {
    bail!("Disponibile solo su Windows")
}

/// The "Programs and Features" uninstall command for `game`: by the store's
/// own key naming when it has one, else by matching the install folder.
pub fn find_uninstaller(entries: &[(String, Values)], game: &Game) -> Option<String> {
    let id = native_id(game);
    let by_key = match game.store {
        Store::Ubisoft => Some(format!("Uplay Install {id}")),
        Store::Gog => Some(format!("{id}_is1")),
        _ => None,
    };
    let command = |v: &Values| v.get("UninstallString").filter(|s| !s.trim().is_empty()).cloned();
    if let Some(key) = by_key {
        if let Some(found) = entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(&key)).and_then(|(_, v)| command(v)) {
            return Some(found);
        }
    }
    let dir = game.install_dir.as_deref().map(crate::launcher::normalize)?;
    entries.iter().find_map(|(_, v)| {
        let location = v.get("InstallLocation")?;
        let location = crate::launcher::normalize(std::path::Path::new(location.trim_matches('"')));
        (location == dir).then(|| command(v)).flatten()
    })
}

/// Splits `"C:\x\unins000.exe" /SILENT` or `MsiExec.exe /X{…}` into the
/// executable and the untouched rest of the command line.
pub fn split_command(command: &str) -> (String, String) {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    match command.to_lowercase().find(".exe") {
        Some(i) => (command[..i + 4].to_string(), command[i + 4..].trim().to_string()),
        None => (command.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchSpec;

    fn values(pairs: &[(&str, &str)]) -> Values {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn steam_games_count_as_installed_while_their_manifest_exists() {
        let root = std::env::temp_dir().join(format!("gm-uninst-{}", std::process::id()));
        let dir = root.join("steamapps").join("common").join("Hades");
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = root.join("steamapps").join("appmanifest_1145360.acf");
        std::fs::write(&manifest, b"").unwrap();
        let mut game = Game::new(Store::Steam, "1145360", "Hades", LaunchSpec::Uri { uri: String::new() }).with_install_dir(&dir);
        game.steam_appid = Some(1145360);
        assert!(still_installed(&game));
        std::fs::remove_file(&manifest).unwrap();
        assert!(!still_installed(&game), "leftover files don't matter once Steam dropped the manifest");
        let gog = Game::new(Store::Gog, "1", "X", LaunchSpec::Uri { uri: String::new() }).with_install_dir(root.join("gone"));
        assert!(!still_installed(&gog));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn splits_uninstall_commands() {
        assert_eq!(
            split_command(r#""C:\GOG Games\Witcher\unins000.exe" /SILENT"#),
            (r"C:\GOG Games\Witcher\unins000.exe".into(), "/SILENT".into())
        );
        assert_eq!(split_command("MsiExec.exe /X{ABC}"), ("MsiExec.exe".into(), "/X{ABC}".into()));
        assert_eq!(split_command(r#""C:\x\Uplay.exe""#), (r"C:\x\Uplay.exe".into(), String::new()));
    }

    #[test]
    fn finds_uninstaller_by_store_key_or_folder() {
        let entries = vec![
            ("1207664643_is1".to_string(), values(&[("UninstallString", r#""C:\GOG\unins000.exe""#)])),
            (
                "{EA-GUID}".to_string(),
                values(&[("InstallLocation", r"C:\EA Games\Apex\"), ("UninstallString", r#""C:\EA\Cleanup.exe" uninstall"#)]),
            ),
        ];
        let gog = Game::new(Store::Gog, "1207664643", "Witcher", LaunchSpec::Uri { uri: String::new() });
        assert_eq!(find_uninstaller(&entries, &gog).as_deref(), Some(r#""C:\GOG\unins000.exe""#));

        let apex = Game::new(Store::Ea, "196716", "Apex", LaunchSpec::Uri { uri: String::new() })
            .with_install_dir(r"C:\EA Games\Apex");
        assert!(find_uninstaller(&entries, &apex).unwrap().contains("Cleanup.exe"));

        let unknown = Game::new(Store::Epic, "Fortnite", "Fortnite", LaunchSpec::Uri { uri: String::new() })
            .with_install_dir(r"C:\Epic\Fortnite");
        assert!(find_uninstaller(&entries, &unknown).is_none());
    }
}
