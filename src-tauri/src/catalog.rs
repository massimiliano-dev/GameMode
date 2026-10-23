//! Games you can install or stream: Steam games you own but haven't installed
//! (Steam Web API, which needs the user's free API key), owned Epic and GOG
//! games (read from their launchers' local caches), the PC Game Pass catalog
//! and Xbox Cloud Gaming (public Xbox endpoints). Installing hands off to the
//! store client.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::artwork::{pick_ms_image, Kind, MsImage};
use crate::library::{steam, vdf};
use crate::model::Store;
use crate::{storefront, AppState};

const TTL: Duration = Duration::from_secs(6 * 3600);
const STEAM_CDN: &str = "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps";
const GAMEPASS_PC: &str = "https://catalog.gamepass.com/sigls/v2?id=fdd9e2a7-0fee-49f6-ad69-4354098401ff&language=it-it&market=IT";
const MS_PRODUCTS: &str = "https://displaycatalog.mp.microsoft.com/v7.0/products";
const MS_BATCH: usize = 100;
/// Everything playable on play.xbox.com (Game Pass Ultimate and "stream your own game").
const CLOUD_ALL: &str = "https://catalog.gamepass.com/sigls/v2?id=1bf84c2b-0643-4591-893f-d9edb703f692&language=it-it&market=IT";
const CLOUD_CONCURRENCY: usize = 6;

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Gamepass,
    Epic,
    #[cfg_attr(not(windows), allow(dead_code))]
    Gog,
    Cloud,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    /// `<source>:<store ids>`: `steam:<appid>`, `gamepass:<productId>`,
    /// `epic:<namespace>:<catalogItemId>:<appName>`, `gog:<id>`, `cloud:<productId>:<slug>`.
    pub id: String,
    pub source: Source,
    pub store: Store,
    pub name: String,
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub playtime_secs: u64,
    /// Download size, when the catalog says (Game Pass only).
    pub size_bytes: Option<u64>,
}

#[derive(Serialize, Clone, Copy, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub enum SteamStatus {
    #[default]
    Ok,
    NotInstalled,
    NeedsKey,
    NoAccount,
    Failed,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub items: Vec<CatalogItem>,
    pub steam: SteamStatus,
    pub gamepass_ok: bool,
}

#[derive(Default)]
pub struct CatalogCache {
    catalog: Mutex<Option<(Instant, Catalog)>>,
    cloud: Mutex<Option<(Instant, Vec<CatalogItem>)>>,
}

pub async fn load(app: &AppHandle, force: bool) -> Catalog {
    let state = app.state::<AppState>();
    if !force {
        if let Some((at, catalog)) = state.catalog.catalog.lock().unwrap().as_ref() {
            if at.elapsed() < TTL {
                return catalog.clone();
            }
        }
    }
    let (installed, key) = {
        let data = state.db.lock();
        let ids: HashSet<String> = data.games.iter().map(|g| g.id.to_lowercase()).collect();
        (ids, data.settings.steam_api_key.clone().filter(|k| !k.trim().is_empty()))
    };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default();

    let (steam, gamepass) = tokio::join!(steam_owned(&client, key, &installed), gamepass(&client, &installed));
    let mut catalog = Catalog::default();
    match steam {
        Ok(items) => catalog.items.extend(items),
        Err(status) => catalog.steam = status,
    }
    match gamepass {
        Ok(items) => {
            catalog.gamepass_ok = true;
            catalog.items.extend(items);
        }
        Err(err) => eprintln!("Game Pass catalog failed: {err:#}"),
    }
    catalog.items.extend(epic_owned(&installed));
    catalog.items.extend(gog_owned(&installed));
    catalog.items.sort_by_key(|i| i.name.to_lowercase());
    *state.catalog.catalog.lock().unwrap() = Some((Instant::now(), catalog.clone()));
    catalog
}

/// Xbox Cloud Gaming titles. Kept apart from `load` because the list is long
/// (thousands of titles) and only needed when the user looks at it.
pub async fn cloud(app: &AppHandle, force: bool) -> Vec<CatalogItem> {
    let state = app.state::<AppState>();
    if !force {
        if let Some((at, items)) = state.catalog.cloud.lock().unwrap().as_ref() {
            if at.elapsed() < TTL {
                return items.clone();
            }
        }
    }
    let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default();
    let items = match cloud_titles(&client).await {
        Ok(items) => items,
        Err(err) => {
            eprintln!("cloud catalog failed: {err:#}");
            return Vec::new();
        }
    };
    *state.catalog.cloud.lock().unwrap() = Some((Instant::now(), items.clone()));
    items
}

async fn cloud_titles(client: &reqwest::Client) -> Result<Vec<CatalogItem>> {
    let ids = parse_sigl(&client.get(CLOUD_ALL).send().await?.error_for_status()?.bytes().await?);
    let limit = Arc::new(tokio::sync::Semaphore::new(CLOUD_CONCURRENCY));
    let none: Arc<HashSet<String>> = Arc::new(HashSet::new());
    let mut tasks = Vec::new();
    for batch in ids.chunks(MS_BATCH) {
        let url = format!("{MS_PRODUCTS}?bigIds={}&market=IT&languages=it-IT,neutral", batch.join(","));
        let (client, limit, none) = (client.clone(), limit.clone(), none.clone());
        tasks.push(tauri::async_runtime::spawn(async move {
            let _permit = limit.acquire().await;
            let body = client.get(url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
            Some(parse_products(&body, &none))
        }));
    }
    let mut items = Vec::new();
    for task in tasks {
        if let Ok(Some(batch)) = task.await {
            items.extend(batch.into_iter().map(as_cloud));
        }
    }
    items.sort_by_key(|i| i.name.to_lowercase());
    Ok(items)
}

fn as_cloud(mut item: CatalogItem) -> CatalogItem {
    let product = item.id.trim_start_matches("gamepass:").to_string();
    item.id = format!("cloud:{product}:{}", slug(&item.name));
    item.source = Source::Cloud;
    item
}

/// URL slug as xbox.com builds it: "EA SPORTS FC™ 25" -> "ea-sports-fc-25".
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() && !matches!(c, '™' | '®' | '©' | '\'' | '’') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Owned Epic games, from the launcher's catalog cache (base64-encoded JSON).
fn epic_owned(installed: &HashSet<String>) -> Vec<CatalogItem> {
    let Some(program_data) = std::env::var_os("ProgramData") else { return Vec::new() };
    let path = PathBuf::from(program_data).join(r"Epic\EpicGamesLauncher\Data\Catalog\catcache.bin");
    std::fs::read_to_string(path).map(|text| parse_epic_catcache(&text, installed)).unwrap_or_default()
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct EpicItem {
    id: String,
    namespace: String,
    title: String,
    categories: Vec<EpicCategory>,
    key_images: Vec<EpicImage>,
    release_info: Vec<EpicRelease>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EpicCategory {
    path: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct EpicImage {
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct EpicRelease {
    app_id: String,
    platform: Vec<String>,
}

pub fn parse_epic_catcache(text: &str, installed: &HashSet<String>) -> Vec<CatalogItem> {
    use base64::Engine;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(text.trim()) else { return Vec::new() };
    let items: Vec<EpicItem> = serde_json::from_slice(&bytes).unwrap_or_default();
    let image = |item: &EpicItem, kinds: &[&str]| {
        kinds.iter().find_map(|k| item.key_images.iter().find(|i| i.kind == *k).map(|i| i.url.clone()))
    };
    let mut seen = HashSet::new();
    items
        .iter()
        .filter(|i| i.categories.iter().any(|c| c.path == "games") && !i.categories.iter().any(|c| c.path == "addons"))
        .filter_map(|i| {
            let release = i.release_info.iter().find(|r| r.platform.iter().any(|p| p == "Windows"))?;
            let app = &release.app_id;
            if app.is_empty() || installed.contains(&format!("epic:{}", app.to_lowercase())) || !seen.insert(app.clone()) {
                return None;
            }
            Some(CatalogItem {
                id: format!("epic:{}:{}:{}", i.namespace, i.id, app),
                source: Source::Epic,
                store: Store::Epic,
                name: i.title.trim().to_string(),
                cover: image(i, &["DieselGameBoxTall", "OfferImageTall", "Thumbnail"]),
                hero: image(i, &["DieselGameBox", "OfferImageWide", "DieselStoreFrontWide"]),
                playtime_secs: 0,
                size_bytes: None,
            })
        })
        .collect()
}

/// Owned GOG games, from GOG Galaxy's local database.
#[cfg(windows)]
fn gog_owned(installed: &HashSet<String>) -> Vec<CatalogItem> {
    let Some(program_data) = std::env::var_os("ProgramData") else { return Vec::new() };
    let db = PathBuf::from(program_data).join(r"GOG.com\Galaxy\storage\galaxy-2.0.db");
    if !db.is_file() {
        return Vec::new();
    }
    let sql = "SELECT p.releaseKey, t.type, p.value FROM GamePieces p \
               JOIN GamePieceTypes t ON p.gamePieceTypeId = t.id \
               WHERE p.releaseKey LIKE 'gog_%' \
               AND t.type IN ('title', 'originalTitle', 'images', 'originalImages') \
               AND p.releaseKey IN (SELECT releaseKey FROM LibraryReleases)";
    match crate::sqlite::query(&db, sql, 3) {
        Ok(rows) => gog_items(&rows, installed),
        Err(err) => {
            eprintln!("GOG Galaxy database: {err:#}");
            Vec::new()
        }
    }
}

#[cfg(not(windows))]
fn gog_owned(_installed: &HashSet<String>) -> Vec<CatalogItem> {
    Vec::new()
}

/// Builds GOG items from `(releaseKey, type, json value)` rows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn gog_items(rows: &[Vec<String>], installed: &HashSet<String>) -> Vec<CatalogItem> {
    #[derive(Default)]
    struct Pieces {
        title: Option<String>,
        cover: Option<String>,
        hero: Option<String>,
    }
    let mut by_key: HashMap<&str, Pieces> = HashMap::new();
    for row in rows {
        let [key, kind, value] = [&row[0], &row[1], &row[2]];
        let json: serde_json::Value = serde_json::from_str(value).unwrap_or_default();
        let entry = by_key.entry(key.as_str()).or_default();
        let text = |k: &str| json.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
        match kind.as_str() {
            // The user's custom title wins over the original one.
            "title" => entry.title = text("title").or(entry.title.take()),
            "originalTitle" => entry.title = entry.title.take().or(text("title")),
            "images" | "originalImages" => {
                entry.cover = entry.cover.take().or(text("verticalCover"));
                entry.hero = entry.hero.take().or(text("background"));
            }
            _ => {}
        }
    }
    by_key
        .into_iter()
        .filter_map(|(key, p)| {
            let id = key.strip_prefix("gog_")?;
            if installed.contains(&format!("gog:{id}")) {
                return None;
            }
            Some(CatalogItem {
                id: format!("gog:{id}"),
                source: Source::Gog,
                store: Store::Gog,
                name: p.title?.trim().to_string(),
                cover: p.cover,
                hero: p.hero,
                playtime_secs: 0,
                size_bytes: None,
            })
        })
        .collect()
}

/// Opens the install page / dialog of a catalog item in its store client.
pub fn install(app: &AppHandle, id: &str) -> Result<()> {
    let Some((source, native)) = id.split_once(':') else { bail!("Gioco non valido") };
    let uri = match source {
        "steam" => format!("steam://install/{native}"),
        "gamepass" => {
            // Xbox app first (it handles Game Pass installs), then the
            // Microsoft Store, then the game's page on xbox.com.
            return storefront::open_first(
                app,
                &[
                    format!("msxbox://game/?productId={native}"),
                    format!("ms-windows-store://pdp/?ProductId={native}"),
                    format!("https://www.xbox.com/it-IT/games/store/game/{native}"),
                ],
            );
        }
        "epic" => {
            let parts: Vec<&str> = native.splitn(3, ':').collect();
            let [ns, item, app_name] = parts[..] else { bail!("Gioco non valido") };
            format!("com.epicgames.launcher://apps/{ns}%3A{item}%3A{app_name}?action=install")
        }
        "gog" if storefront::scheme_registered("goggalaxy") => format!("goggalaxy://openGameView/{native}"),
        "gog" => "https://www.gog.com/account".to_string(),
        "cloud" => {
            let (product, slug) = native.split_once(':').unwrap_or((native, "game"));
            return open_cloud(app, &format!("https://www.xbox.com/it-IT/play/games/{slug}/{product}"));
        }
        _ => bail!("Gioco non valido"),
    };
    storefront::open_uri(app, &uri)
}

/// After a Steam install dialog: as soon as Steam registers the game (its
/// manifest appears, i.e. the download was queued) GameMode comes back.
pub fn watch_steam_install(app: AppHandle, appid: String) {
    use tauri::Emitter;
    std::thread::spawn(move || {
        let Some(root) = steam::find_steam_root() else { return };
        let manifest = format!("appmanifest_{appid}.acf");
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(10 * 60) {
            std::thread::sleep(Duration::from_secs(2));
            // The user went back by hand: nothing left to do.
            if !crate::assist::is_active() && crate::window::is_foreground(&app) {
                return;
            }
            if steam::library_folders(&root).iter().any(|f| f.join("steamapps").join(&manifest).is_file()) {
                crate::storefront::return_to_gamemode(&app);
                let _ = app.emit("install://queued", &appid);
                return;
            }
        }
    });
}

/// Cloud games run in Edge's app mode, full screen: Edge has the user's
/// Microsoft sign-in and native controller support for xbox.com/play.
fn open_cloud(app: &AppHandle, url: &str) -> Result<()> {
    if let Some(edge) = edge_path() {
        std::process::Command::new(edge).args([&format!("--app={url}"), "--start-fullscreen"]).spawn()?;
        crate::window::minimize(app);
        return Ok(());
    }
    storefront::open_uri(app, url)
}

#[cfg(windows)]
fn edge_path() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    const APP_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\msedge.exe";
    [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER]
        .into_iter()
        .find_map(|root| winreg::RegKey::predef(root).open_subkey(APP_PATH).and_then(|k| k.get_value::<String, _>("")).ok())
        .map(PathBuf::from)
        .into_iter()
        .chain([PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe")])
        .find(|p| p.is_file())
}

#[cfg(not(windows))]
fn edge_path() -> Option<PathBuf> {
    None
}

async fn steam_owned(
    client: &reqwest::Client,
    key: Option<String>,
    installed: &HashSet<String>,
) -> std::result::Result<Vec<CatalogItem>, SteamStatus> {
    let root = steam::find_steam_root().ok_or(SteamStatus::NotInstalled)?;
    let key = key.ok_or(SteamStatus::NeedsKey)?;
    let users = std::fs::read_to_string(root.join("config").join("loginusers.vdf")).map_err(|_| SteamStatus::NoAccount)?;
    let user = steam_user(&users).ok_or(SteamStatus::NoAccount)?;
    let url = format!(
        "https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?key={key}&steamid={user}&include_appinfo=1&include_played_free_games=1&format=json"
    );
    let resp = client.get(url).send().await.map_err(|_| SteamStatus::Failed)?;
    if !resp.status().is_success() {
        eprintln!("Steam GetOwnedGames: {}", resp.status());
        return Err(SteamStatus::Failed);
    }
    let body = resp.bytes().await.map_err(|_| SteamStatus::Failed)?;
    Ok(parse_owned(&body, installed))
}

/// SteamID64 of the account that signed in last.
pub fn steam_user(loginusers: &str) -> Option<String> {
    let parsed = vdf::parse(loginusers).ok()?;
    let users = parsed.get("users")?.entries();
    users
        .iter()
        .find(|(_, u)| u.get_str("MostRecent") == Some("1"))
        .or(users.first())
        .map(|(id, _)| id.clone())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OwnedResponse {
    response: OwnedGames,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OwnedGames {
    games: Vec<OwnedGame>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct OwnedGame {
    appid: u32,
    name: String,
    /// Minutes.
    playtime_forever: u64,
}

fn parse_owned(body: &[u8], installed: &HashSet<String>) -> Vec<CatalogItem> {
    let parsed: OwnedResponse = serde_json::from_slice(body).unwrap_or_default();
    parsed
        .response
        .games
        .into_iter()
        .filter(|g| !g.name.is_empty() && !installed.contains(&format!("steam:{}", g.appid)))
        .map(|g| CatalogItem {
            id: format!("steam:{}", g.appid),
            source: Source::Steam,
            store: Store::Steam,
            name: g.name,
            cover: Some(format!("{STEAM_CDN}/{}/library_600x900_2x.jpg", g.appid)),
            hero: Some(format!("{STEAM_CDN}/{}/library_hero.jpg", g.appid)),
            playtime_secs: g.playtime_forever * 60,
            size_bytes: None,
        })
        .collect()
}

async fn gamepass(client: &reqwest::Client, installed: &HashSet<String>) -> Result<Vec<CatalogItem>> {
    let ids = parse_sigl(&client.get(GAMEPASS_PC).send().await?.error_for_status()?.bytes().await?);
    let mut items = Vec::new();
    for batch in ids.chunks(MS_BATCH) {
        let url = format!("{MS_PRODUCTS}?bigIds={}&market=IT&languages=it-IT,neutral", batch.join(","));
        let body = client.get(url).send().await?.error_for_status()?.bytes().await?;
        items.extend(parse_products(&body, installed));
    }
    Ok(items)
}

fn parse_sigl(body: &[u8]) -> Vec<String> {
    #[derive(Deserialize)]
    struct Entry {
        id: Option<String>,
    }
    let entries: Vec<Entry> = serde_json::from_slice(body).unwrap_or_default();
    entries.into_iter().filter_map(|e| e.id).collect()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Products {
    products: Vec<Product>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Product {
    product_id: String,
    localized_properties: Vec<Localized>,
    display_sku_availabilities: Vec<SkuAvailability>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Localized {
    product_title: String,
    images: Vec<MsImage>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct SkuAvailability {
    sku: Sku,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Sku {
    properties: SkuProperties,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct SkuProperties {
    packages: Vec<Package>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Package {
    package_family_name: Option<String>,
    max_download_size_in_bytes: Option<u64>,
}

fn parse_products(body: &[u8], installed: &HashSet<String>) -> Vec<CatalogItem> {
    let parsed: Products = serde_json::from_slice(body).unwrap_or_default();
    parsed
        .products
        .into_iter()
        .filter_map(|p| {
            let families: Vec<String> = p
                .display_sku_availabilities
                .iter()
                .flat_map(|s| s.sku.properties.packages.iter())
                .filter_map(|pk| pk.package_family_name.as_ref().map(|f| f.to_lowercase()))
                .collect();
            if families.iter().any(|f| installed.contains(&format!("xbox:{f}"))) {
                return None;
            }
            // Several packages per product: the PC build is the one with a
            // package family name, console builds have none.
            let packages: Vec<&Package> = p.display_sku_availabilities.iter().flat_map(|s| s.sku.properties.packages.iter()).collect();
            let size_of = |pc_only: bool| {
                packages
                    .iter()
                    .filter(|pk| !pc_only || pk.package_family_name.is_some())
                    .filter_map(|pk| pk.max_download_size_in_bytes)
                    .filter(|&b| b > 0)
                    .max()
            };
            let size_bytes = size_of(true).or_else(|| size_of(false));
            let local = p.localized_properties.into_iter().next()?;
            Some(CatalogItem {
                id: format!("gamepass:{}", p.product_id),
                source: Source::Gamepass,
                store: Store::Xbox,
                name: local.product_title.trim().to_string(),
                cover: pick_ms_image(&local.images, Kind::Cover),
                hero: pick_ms_image(&local.images, Kind::Hero),
                playtime_secs: 0,
                size_bytes,
            })
        })
        .filter(|i| !i.name.is_empty() && !i.id.ends_with(':'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog").join(name);
        std::fs::read(path).unwrap()
    }

    /// Hits the real Game Pass endpoints: `cargo test -- --ignored live_gamepass`.
    #[test]
    #[ignore]
    fn live_gamepass() {
        let client = reqwest::Client::new();
        let items = tauri::async_runtime::block_on(gamepass(&client, &HashSet::new())).unwrap();
        println!("{} Game Pass titles, e.g. {:?}", items.len(), items.first().map(|i| &i.name));
        let sized = items.iter().filter(|i| i.size_bytes.is_some()).count();
        println!("{sized} with a download size, e.g. {:?}", items.iter().find(|i| i.size_bytes.is_some()).map(|i| (&i.name, i.size_bytes)));
        assert!(items.len() > 100);
        assert!(sized * 2 > items.len(), "MaxDownloadSizeInBytes missing from most products");
        assert!(items.iter().filter(|i| i.cover.is_some()).count() * 10 > items.len() * 9);
    }

    /// `cargo test -- --ignored live_cloud`
    #[test]
    #[ignore]
    fn live_cloud() {
        let client = reqwest::Client::new();
        let items = tauri::async_runtime::block_on(cloud_titles(&client)).unwrap();
        println!("{} cloud titles, e.g. {:?}", items.len(), items.iter().take(3).map(|i| &i.id).collect::<Vec<_>>());
        assert!(items.len() > 500);
    }

    #[test]
    fn reads_owned_epic_games_from_the_catalog_cache() {
        use base64::Engine;
        let json = r#"[
          {"id":"c1","namespace":"ns1","title":"Hades","categories":[{"path":"games"}],
           "keyImages":[{"type":"DieselGameBoxTall","url":"tall.jpg"},{"type":"DieselGameBox","url":"wide.jpg"}],
           "releaseInfo":[{"appId":"Min","platform":["Windows"]}]},
          {"id":"c2","namespace":"ns1","title":"Hades Soundtrack","categories":[{"path":"addons"},{"path":"games"}],
           "releaseInfo":[{"appId":"Ost","platform":["Windows"]}]},
          {"id":"c3","namespace":"ns2","title":"Fortnite","categories":[{"path":"games"}],
           "releaseInfo":[{"appId":"Fortnite","platform":["Windows"]}]}
        ]"#;
        let text = base64::engine::general_purpose::STANDARD.encode(json);
        let installed: HashSet<String> = ["epic:fortnite".to_string()].into();
        let items = parse_epic_catcache(&text, &installed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "epic:ns1:c1:Min");
        assert_eq!(items[0].cover.as_deref(), Some("tall.jpg"));
        assert!(parse_epic_catcache("not base64!", &installed).is_empty());
    }

    #[test]
    fn builds_gog_items_from_galaxy_rows() {
        let row = |k: &str, t: &str, v: &str| vec![k.to_string(), t.to_string(), v.to_string()];
        let rows = vec![
            row("gog_1", "originalTitle", r#"{"title":"The Witcher 3"}"#),
            row("gog_1", "title", r#"{"title":"Witcher 3 GOTY"}"#),
            row("gog_1", "originalImages", r#"{"verticalCover":"v.webp","background":"b.webp"}"#),
            row("gog_2", "originalTitle", r#"{"title":"Installed one"}"#),
        ];
        let installed: HashSet<String> = ["gog:2".to_string()].into();
        let items = gog_items(&rows, &installed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Witcher 3 GOTY");
        assert_eq!(items[0].cover.as_deref(), Some("v.webp"));
    }

    #[test]
    fn slugs_titles_like_xbox_com() {
        assert_eq!(slug("EA SPORTS FC™ 25"), "ea-sports-fc-25");
        assert_eq!(slug("Marvel's Spider-Man"), "marvels-spider-man");
    }

    #[test]
    fn finds_the_most_recent_steam_account() {
        let src = String::from_utf8(fixture("loginusers.vdf")).unwrap();
        assert_eq!(steam_user(&src).as_deref(), Some("76561198000000042"));
    }

    #[test]
    fn lists_owned_steam_games_that_are_not_installed() {
        let installed: HashSet<String> = ["steam:570".to_string()].into();
        let items = parse_owned(&fixture("owned.json"), &installed);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Hades", "Stardew Valley"]);
        assert_eq!(items[1].playtime_secs, 90 * 60);
        assert!(items[0].cover.as_deref().unwrap().contains("/1145360/"));
    }

    #[test]
    fn lists_game_pass_titles_and_skips_installed_ones() {
        assert_eq!(parse_sigl(&fixture("sigl.json")).len(), 2);
        let none: HashSet<String> = HashSet::new();
        let items = parse_products(&fixture("products.json"), &none);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "gamepass:9NPDN9R45JX4");
        assert!(items[0].cover.as_deref().unwrap().ends_with("?w=600"));
        assert_eq!(items[0].size_bytes, Some(8_001_105_920), "the PC package, not the bigger console one");
        assert_eq!(items[1].size_bytes, None);

        let installed: HashSet<String> = ["xbox:surpriseattackptyltd.1000xresist_8k24hnfn3vvj0".to_string()].into();
        let items = parse_products(&fixture("products.json"), &installed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "33 Immortals");
    }
}
