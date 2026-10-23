//! Cover / hero / logo images, cached on disk. Steam titles come from the
//! local Steam cache or the Steam CDN, Xbox titles from the Microsoft Store
//! catalog, and everything else (or whatever is still missing) from SteamGridDB.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Semaphore;

use crate::library::steam;
use crate::model::{now, Artwork, Game, Store};
use crate::AppState;

const CONCURRENCY: usize = 4;
/// Games without artwork are retried at most once a week.
const RETRY_AFTER_SECS: i64 = 7 * 24 * 3600;
const SGDB: &str = "https://www.steamgriddb.com/api/v2";
/// Public Microsoft Store catalog, looked up by package family name.
const MS_CATALOG: &str = "https://displaycatalog.mp.microsoft.com/v7.0/products/lookup";
/// Bump when the pipeline gains new sources, so games that previously came
/// up empty are retried right away instead of after `RETRY_AFTER_SECS`.
pub const PIPELINE_VERSION: u32 = 2;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Kind {
    Cover,
    Hero,
    Logo,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Cover, Kind::Hero, Kind::Logo];

    fn name(self) -> &'static str {
        match self {
            Kind::Cover => "cover",
            Kind::Hero => "hero",
            Kind::Logo => "logo",
        }
    }

    /// File name in Steam's library cache and on its CDN.
    fn steam_file(self) -> &'static str {
        match self {
            Kind::Cover => "library_600x900",
            Kind::Hero => "library_hero",
            Kind::Logo => "logo",
        }
    }

    fn steam_ext(self) -> &'static str {
        if self == Kind::Logo { "png" } else { "jpg" }
    }

    fn sgdb_path(self) -> &'static str {
        match self {
            Kind::Cover => "grids/game/{id}?dimensions=600x900",
            Kind::Hero => "heroes/game/{id}",
            Kind::Logo => "logos/game/{id}",
        }
    }

    /// Microsoft Store image purposes, best first, and the width to request.
    fn ms_purposes(self) -> (&'static [&'static str], u32) {
        match self {
            Kind::Cover => (&["Poster", "BoxArt"], 600),
            Kind::Hero => (&["SuperHeroArt", "TitledHeroArt"], 1920),
            // The catalog has no transparent wordmark; the UI shows the title instead.
            Kind::Logo => (&[], 0),
        }
    }

    fn slot(self, art: &mut Artwork) -> &mut Option<String> {
        match self {
            Kind::Cover => &mut art.cover,
            Kind::Hero => &mut art.hero,
            Kind::Logo => &mut art.logo,
        }
    }
}

/// Fetches artwork for every game that lacks it, in the background.
pub fn refresh(app: &AppHandle, force: bool) {
    let state = app.state::<AppState>();
    if state.artwork_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let (games, key) = {
            let data = state.db.lock();
            let due = |g: &&Game| {
                !g.artwork.is_complete()
                    && (force || g.artwork_checked_at.is_none_or(|t| now() - t > RETRY_AFTER_SECS))
            };
            let games: Vec<Game> = data.games.iter().filter(due).cloned().collect();
            (games, data.settings.steamgriddb_key.clone().filter(|k| !k.trim().is_empty()))
        };
        let fetcher = Arc::new(Fetcher {
            client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default(),
            dir: state.data_dir.join("artwork"),
            steam_root: steam::find_steam_root(),
            sgdb_key: key,
        });
        let limit = Arc::new(Semaphore::new(CONCURRENCY));
        let mut tasks = Vec::new();
        for game in games {
            let (fetcher, limit, app) = (fetcher.clone(), limit.clone(), app.clone());
            tasks.push(tauri::async_runtime::spawn(async move {
                let _permit = limit.acquire().await;
                let artwork = fetcher.fetch(&game).await;
                save(&app, &game.id, artwork);
            }));
        }
        for task in tasks {
            let _ = task.await;
        }
        state.artwork_running.store(false, Ordering::SeqCst);
    });
}

fn save(app: &AppHandle, id: &str, artwork: Artwork) {
    let state = app.state::<AppState>();
    let updated = state.db.update(|d| {
        let game = d.game_mut(id)?;
        game.artwork = artwork;
        game.artwork_checked_at = Some(now());
        d.view_game(id)
    });
    if let Ok(Some(game)) = updated {
        let _ = app.emit("library://game", &game);
    }
}

struct Fetcher {
    client: reqwest::Client,
    dir: PathBuf,
    steam_root: Option<PathBuf>,
    sgdb_key: Option<String>,
}

impl Fetcher {
    async fn fetch(&self, game: &Game) -> Artwork {
        let mut art = game.artwork.clone();
        let dir = self.dir.join(safe_name(&game.id));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let mut sgdb_id: Option<Option<u64>> = None;
        let mut ms_images: Option<Vec<MsImage>> = None;

        for kind in Kind::ALL {
            if kind.slot(&mut art).as_ref().is_some_and(|p| Path::new(p).is_file()) {
                continue;
            }
            let mut found = None;
            if let Some(appid) = game.steam_appid {
                found = self.from_steam(appid, kind, &dir).await;
            }
            if found.is_none() && kind == Kind::Cover {
                if let Some(url) = &game.cover_url {
                    found = self.download(url, &dir, kind).await.ok();
                }
            }
            if found.is_none() && game.store == Store::Xbox {
                let images = match &ms_images {
                    Some(images) => images,
                    None => ms_images.insert(self.ms_catalog_images(game).await.unwrap_or_default()),
                };
                if let Some(url) = pick_ms_image(images, kind) {
                    found = self.download(&url, &dir, kind).await.ok();
                }
            }
            if found.is_none() && self.sgdb_key.is_some() {
                let id = match sgdb_id {
                    Some(id) => id,
                    None => *sgdb_id.insert(self.sgdb_game_id(game).await.ok()),
                };
                if let Some(id) = id {
                    found = self.from_sgdb(id, kind, &dir).await.ok();
                }
            }
            *kind.slot(&mut art) = found.map(|p| p.to_string_lossy().into_owned());
        }
        art
    }

    async fn from_steam(&self, appid: u32, kind: Kind, dir: &Path) -> Option<PathBuf> {
        let (file, ext) = (kind.steam_file(), kind.steam_ext());
        if let Some(root) = &self.steam_root {
            let cache = root.join("appcache").join("librarycache");
            for local in [cache.join(appid.to_string()).join(format!("{file}.{ext}")), cache.join(format!("{appid}_{file}.{ext}"))] {
                let target = dir.join(format!("{}.{ext}", kind.name()));
                if local.is_file() && tokio::fs::copy(&local, &target).await.is_ok() {
                    return Some(target);
                }
            }
        }
        let hi_res = if kind == Kind::Cover { "_2x" } else { "" };
        for host in [
            "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps",
            "https://cdn.cloudflare.steamstatic.com/steam/apps",
        ] {
            let url = format!("{host}/{appid}/{file}{hi_res}.{ext}");
            if let Ok(path) = self.download(&url, dir, kind).await {
                return Some(path);
            }
        }
        None
    }

    async fn sgdb_get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let key = self.sgdb_key.as_deref().unwrap_or_default();
        let resp = self.client.get(format!("{SGDB}/{path}")).bearer_auth(key).send().await?;
        if !resp.status().is_success() {
            bail!("SteamGridDB {}", resp.status());
        }
        let body: SgdbResponse<T> = resp.json().await?;
        if !body.success {
            bail!("SteamGridDB request failed");
        }
        Ok(body.data)
    }

    async fn sgdb_game_id(&self, game: &Game) -> Result<u64> {
        if let Some(appid) = game.steam_appid {
            if let Ok(found) = self.sgdb_get::<SgdbGame>(&format!("games/steam/{appid}")).await {
                return Ok(found.id);
            }
        }
        let query = urlencode(&search_name(&game.name));
        let results: Vec<SgdbGame> = self.sgdb_get(&format!("search/autocomplete/{query}")).await?;
        results.first().map(|g| g.id).ok_or_else(|| anyhow::anyhow!("no SteamGridDB match"))
    }

    async fn ms_catalog_images(&self, game: &Game) -> Result<Vec<MsImage>> {
        // Xbox game ids are `xbox:<PackageFamilyName>`.
        let pfn = game.id.split_once(':').map_or(game.id.as_str(), |(_, id)| id);
        let url = format!(
            "{MS_CATALOG}?alternateId=PackageFamilyName&value={}&market=IT&languages=it-IT,en-US,neutral&fieldsTemplate=details",
            urlencode(pfn)
        );
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("Microsoft Store catalog {}", resp.status());
        }
        Ok(parse_ms_catalog(&resp.bytes().await?))
    }

    async fn from_sgdb(&self, id: u64, kind: Kind, dir: &Path) -> Result<PathBuf> {
        let images: Vec<SgdbImage> = self.sgdb_get(&kind.sgdb_path().replace("{id}", &id.to_string())).await?;
        let Some(image) = images.first() else { bail!("no image") };
        self.download(&image.url, dir, kind).await
    }

    async fn download(&self, url: &str, dir: &Path, kind: Kind) -> Result<PathBuf> {
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            bail!("{url}: {}", resp.status());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let ext = match content_type.as_str() {
            "image/png" => "png",
            "image/webp" => "webp",
            "image/jpeg" | "image/jpg" => "jpg",
            _ => bail!("{url}: not an image ({content_type})"),
        };
        let bytes = resp.bytes().await?;
        let path = dir.join(format!("{}.{ext}", kind.name()));
        tokio::fs::write(&path, &bytes).await?;
        Ok(path)
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsCatalog {
    products: Vec<MsProduct>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsProduct {
    localized_properties: Vec<MsLocalized>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsLocalized {
    images: Vec<MsImage>,
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(rename_all = "PascalCase", default)]
pub(crate) struct MsImage {
    image_purpose: String,
    uri: String,
    width: u32,
}

fn parse_ms_catalog(body: &[u8]) -> Vec<MsImage> {
    let catalog: MsCatalog = serde_json::from_slice(body).unwrap_or_default();
    catalog
        .products
        .into_iter()
        .next()
        .and_then(|p| p.localized_properties.into_iter().next())
        .map(|l| l.images)
        .unwrap_or_default()
}

/// Best image for `kind`, as a URL asking the CDN for a sensible size.
pub(crate) fn pick_ms_image(images: &[MsImage], kind: Kind) -> Option<String> {
    let (purposes, width) = kind.ms_purposes();
    let image = purposes
        .iter()
        .find_map(|purpose| images.iter().filter(|i| i.image_purpose == *purpose).max_by_key(|i| i.width))?;
    let uri = if image.uri.starts_with("//") { format!("https:{}", image.uri) } else { image.uri.clone() };
    Some(if image.width > width { format!("{uri}?w={width}") } else { uri })
}

/// Store titles carry marks and edition suffixes that spoil name searches.
fn search_name(name: &str) -> String {
    let mut s: String = name.chars().filter(|c| !matches!(c, '™' | '®' | '©')).collect();
    for suffix in [" for Windows 10", " for Windows", " (PC)", " - Windows Edition", " Windows Edition", " PC"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.to_string();
        }
    }
    s.trim().to_string()
}

#[derive(Deserialize)]
struct SgdbResponse<T> {
    success: bool,
    data: T,
}

#[derive(Deserialize)]
struct SgdbGame {
    id: u64,
}

#[derive(Deserialize)]
struct SgdbImage {
    url: String,
}

pub(crate) fn safe_name(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_poster_and_hero_from_ms_catalog() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/xbox/catalog.json");
        let images = parse_ms_catalog(&std::fs::read(path).unwrap());
        let cover = pick_ms_image(&images, Kind::Cover).unwrap();
        assert!(cover.starts_with("https://store-images.s-microsoft.com/") && cover.ends_with("?w=600"));
        assert!(pick_ms_image(&images, Kind::Hero).unwrap().ends_with("?w=1920"));
        assert!(pick_ms_image(&images, Kind::Logo).is_none());
        assert!(parse_ms_catalog(b"not json").is_empty());
    }

    #[test]
    fn cleans_names_for_search() {
        assert_eq!(search_name("Microsoft Flight Simulator for Windows 10"), "Microsoft Flight Simulator");
        assert_eq!(search_name("Call of Duty®: Black Ops 6™"), "Call of Duty: Black Ops 6");
    }

    #[test]
    fn encodes_names_for_urls() {
        assert_eq!(urlencode("Baldur's Gate 3"), "Baldur%27s%20Gate%203");
        assert_eq!(safe_name("xbox:Microsoft.Forza_8wekyb3d8bbwe"), "xbox_Microsoft_Forza_8wekyb3d8bbwe");
    }
}
