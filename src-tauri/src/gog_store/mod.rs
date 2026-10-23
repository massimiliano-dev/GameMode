//! GOG without GOG Galaxy: sign in with GOG's own login page, list the
//! games the account owns, download their offline installers and install
//! or uninstall them silently. Games land in the GOG folder (settings), where
//! `library::gog_info` finds them through their `goggame-<id>.info`.

mod api;
mod auth;
mod install;
mod setup;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Result};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::catalog::{CatalogItem, Source};
use crate::direct;
use crate::model::{Game, Settings, Store};

pub use auth::signed_in;

/// Box art requests in flight at once while building the catalog.
const ART_CONCURRENCY: usize = 8;

/// Where GameMode installs GOG games.
pub fn install_root(settings: &Settings) -> PathBuf {
    settings.gog_install_dir.clone().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(default_root)
}

/// `C:\GOG Games`, GOG's own default.
pub fn default_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    PathBuf::from(format!(r"{drive}\GOG Games"))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GogStatus {
    pub signed_in: bool,
    pub install_dir: PathBuf,
}

pub fn status(app: &AppHandle) -> GogStatus {
    let settings = app.state::<crate::AppState>().db.lock().settings.clone();
    GogStatus { signed_in: signed_in(), install_dir: install_root(&settings) }
}

pub async fn login(app: &AppHandle) -> Result<bool> {
    let ok = auth::login(app).await?;
    if ok {
        // Owned games show up in the catalog right away.
        app.state::<crate::AppState>().catalog.invalidate();
    }
    Ok(ok)
}

pub async fn logout(app: &AppHandle) {
    auth::logout().await;
    app.state::<crate::AppState>().catalog.invalidate();
}

/// Owned Windows games that aren't installed; empty when not signed in.
pub async fn owned_items(client: &reqwest::Client, installed: &HashSet<String>) -> Vec<CatalogItem> {
    if !signed_in() {
        return Vec::new();
    }
    let access = match auth::access_token(client).await {
        Ok(token) => token,
        Err(err) => {
            log::warn!("GOG sign-in: {err:#}");
            return Vec::new();
        }
    };
    let ids = match api::owned_ids(client, &access).await {
        Ok(ids) => ids,
        Err(err) => {
            log::warn!("GOG owned games: {err:#}");
            return Vec::new();
        }
    };
    let ids: Vec<u64> = ids.into_iter().filter(|id| !installed.contains(&format!("gog:{id}"))).collect();
    let products: Vec<api::Product> = api::products(client, &ids).await.into_iter().filter(api::Product::installable_game).collect();

    let limit = Arc::new(tokio::sync::Semaphore::new(ART_CONCURRENCY));
    let tasks: Vec<_> = products
        .iter()
        .map(|p| {
            let (client, limit, id) = (client.clone(), limit.clone(), p.id);
            tauri::async_runtime::spawn(async move {
                let _permit = limit.acquire().await;
                api::box_art(&client, id).await
            })
        })
        .collect();
    let mut items = Vec::new();
    for (product, task) in products.into_iter().zip(tasks) {
        let cover = task.await.ok().flatten();
        items.push(CatalogItem {
            id: format!("gog:{}", product.id),
            source: Source::Gog,
            store: Store::Gog,
            name: product.title.trim().to_string(),
            cover,
            hero: product.images.background.as_deref().map(api::https),
            playtime_secs: 0,
            size_bytes: None,
        });
    }
    items
}

/// Starts downloading and installing GOG game `id` in the background;
/// progress goes through `direct`, the outcome through `direct://done`.
pub fn start_install(app: &AppHandle, id: &str, name: &str) -> Result<()> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        bail!("Gioco non valido");
    }
    let job = install::job_id(id);
    let Some(cancel) = direct::begin(&job) else { bail!("{name} si sta già installando") };
    let root = install_root(&app.state::<crate::AppState>().db.lock().settings);
    let (app, id, name) = (app.clone(), id.to_string(), name.to_string());
    tauri::async_runtime::spawn(async move {
        let result = install::run(&id, &root, &cancel).await;
        direct::finish(&job);
        let done = match result {
            Ok(title) => {
                let handle = app.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || crate::scan(&handle)).await;
                direct::Done { id: job, name: title, ok: true, error: None }
            }
            Err(err) => {
                let cancelled = cancel.load(std::sync::atomic::Ordering::SeqCst) || err.to_string() == direct::CANCELLED;
                if cancelled {
                    let _ = std::fs::remove_dir_all(install::staging_dir(&root, &id));
                } else {
                    log::warn!("GOG install of {id} failed: {err:#}");
                }
                direct::Done { id: job, name, ok: false, error: (!cancelled).then(|| format!("{err:#}")) }
            }
        };
        direct::announce(&app, done);
    });
    Ok(())
}

/// Silent uninstall through the game's own `unins000.exe`. `None` when the
/// game has none (then its registered uninstaller is used). Blocking.
pub fn uninstall(game: &Game) -> Result<Option<crate::uninstall::Outcome>> {
    use crate::uninstall::Outcome;
    let Some(dir) = game.install_dir.as_deref() else { return Ok(None) };
    let Some(uninstaller) = find_uninstaller(dir) else { return Ok(None) };
    let code = setup::run_and_wait(&uninstaller, setup::UNINSTALL_ARGS, setup::UNINSTALL_TIMEOUT)?;
    if code != 0 {
        bail!("La disinstallazione di {} non è riuscita (codice {code})", game.name);
    }
    // InnoSetup finishes from a copy of itself in %TEMP%: give it a moment.
    for _ in 0..60 {
        if !uninstaller.exists() {
            return Ok(Some(Outcome::Removed));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    Ok(Some(Outcome::Started))
}

/// The newest InnoSetup uninstaller in `dir` (`unins000.exe`, `unins001.exe`…).
fn find_uninstaller(dir: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                let n = n.to_ascii_lowercase();
                n.len() == "unins000.exe".len() && n.starts_with("unins") && n.ends_with(".exe") && n[5..8].chars().all(|c| c.is_ascii_digit())
            })
        })
        .collect();
    found.sort();
    found.pop()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_inno_setup_uninstaller() {
        let dir = std::env::temp_dir().join(format!("gm-gog-unins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(find_uninstaller(&dir).is_none());
        for name in ["unins000.exe", "unins000.dat", "unins001.exe", "uninstall.exe"] {
            std::fs::write(dir.join(name), b"").unwrap();
        }
        assert_eq!(find_uninstaller(&dir).unwrap().file_name().unwrap(), "unins001.exe");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
