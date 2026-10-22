//! Games you can install: Steam games you own but haven't installed (Steam
//! Web API, which needs the user's free API key) and the PC Game Pass
//! catalog (public Xbox endpoints). Installing hands off to the store client.

use std::collections::HashSet;
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

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Steam,
    Gamepass,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    /// `steam:<appid>` or `gamepass:<productId>`.
    pub id: String,
    pub source: Source,
    pub store: Store,
    pub name: String,
    pub cover: Option<String>,
    pub hero: Option<String>,
    pub playtime_secs: u64,
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
pub struct CatalogCache(Mutex<Option<(Instant, Catalog)>>);

pub async fn load(app: &AppHandle, force: bool) -> Catalog {
    let state = app.state::<AppState>();
    if !force {
        if let Some((at, catalog)) = state.catalog.0.lock().unwrap().as_ref() {
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
    catalog.items.sort_by_key(|i| i.name.to_lowercase());
    *state.catalog.0.lock().unwrap() = Some((Instant::now(), catalog.clone()));
    catalog
}

/// Opens the install page / dialog of a catalog item in its store client.
pub fn install(app: &AppHandle, id: &str) -> Result<()> {
    let Some((source, native)) = id.split_once(':') else { bail!("Gioco non valido") };
    let uri = match source {
        "steam" => format!("steam://install/{native}"),
        "gamepass" if storefront::scheme_registered("msxbox") => format!("msxbox://game/?productId={native}"),
        "gamepass" => format!("ms-windows-store://pdp/?ProductId={native}"),
        _ => bail!("Gioco non valido"),
    };
    storefront::open_uri(app, &uri)
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
            let local = p.localized_properties.into_iter().next()?;
            Some(CatalogItem {
                id: format!("gamepass:{}", p.product_id),
                source: Source::Gamepass,
                store: Store::Xbox,
                name: local.product_title.trim().to_string(),
                cover: pick_ms_image(&local.images, Kind::Cover),
                hero: pick_ms_image(&local.images, Kind::Hero),
                playtime_secs: 0,
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
        assert!(items.len() > 100);
        assert!(items.iter().filter(|i| i.cover.is_some()).count() * 10 > items.len() * 9);
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

        let installed: HashSet<String> = ["xbox:surpriseattackptyltd.1000xresist_8k24hnfn3vvj0".to_string()].into();
        let items = parse_products(&fixture("products.json"), &installed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "33 Immortals");
    }
}
