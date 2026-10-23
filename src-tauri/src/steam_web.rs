//! Extras from Steam's web APIs: official news, the player's achievements
//! (needs the Web API key) and discounted wishlist games.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::library::steam;
use crate::AppState;

const EXTRAS_TTL: Duration = Duration::from_secs(30 * 60);
const DEALS_TTL: Duration = Duration::from_secs(6 * 3600);
const PRICE_BATCH: usize = 50;

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NewsItem {
    pub title: String,
    pub url: String,
    /// Unix seconds.
    pub date: i64,
    pub snippet: String,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Achievement {
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub achieved: bool,
    pub unlock_time: i64,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AchievementStatus {
    Ok,
    /// No Web API key configured.
    NeedsKey,
    /// Game has no achievements.
    None,
    /// Steam refused (private profile, or the account doesn't own the game).
    Unavailable,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SteamExtras {
    pub news: Vec<NewsItem>,
    pub achievements: Vec<Achievement>,
    pub achievement_status: AchievementStatus,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Deal {
    pub appid: u32,
    pub name: String,
    pub discount: u32,
    pub initial: String,
    pub price: String,
    pub cover: String,
    pub hero: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum DealsStatus {
    #[default]
    Ok,
    NoAccount,
    /// Empty or private wishlist.
    Empty,
    Failed,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Deals {
    pub items: Vec<Deal>,
    pub status: DealsStatus,
}

#[derive(Default)]
pub struct SteamCache {
    extras: Mutex<HashMap<u32, (Instant, SteamExtras)>>,
    deals: Mutex<Option<(Instant, Deals)>>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap_or_default()
}

/// SteamID64 of the account signed in on this PC.
pub fn steam_account() -> Option<String> {
    let root = steam::find_steam_root()?;
    let users = std::fs::read_to_string(root.join("config").join("loginusers.vdf")).ok()?;
    crate::catalog::steam_user(&users)
}

pub async fn extras(app: &AppHandle, appid: u32) -> SteamExtras {
    let state = app.state::<AppState>();
    if let Some((at, cached)) = state.steam.extras.lock().unwrap().get(&appid) {
        if at.elapsed() < EXTRAS_TTL {
            return cached.clone();
        }
    }
    let key = state.db.lock().settings.steam_api_key.clone().filter(|k| !k.trim().is_empty());
    let client = client();
    let (news, achievements) = tokio::join!(news(&client, appid), achievements(&client, key, appid));
    let (achievements, achievement_status) = achievements;
    let extras = SteamExtras { news: news.unwrap_or_default(), achievements, achievement_status };
    state.steam.extras.lock().unwrap().insert(appid, (Instant::now(), extras.clone()));
    extras
}

async fn news(client: &reqwest::Client, appid: u32) -> Result<Vec<NewsItem>> {
    let url = format!(
        "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={appid}&count=4&maxlength=280&format=json&feeds=steam_community_announcements"
    );
    Ok(parse_news(&client.get(url).send().await?.error_for_status()?.bytes().await?))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewsResponse {
    appnews: NewsList,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct NewsList {
    newsitems: Vec<RawNews>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNews {
    title: String,
    url: String,
    date: i64,
    contents: String,
}

fn parse_news(body: &[u8]) -> Vec<NewsItem> {
    let parsed: NewsResponse = serde_json::from_slice(body).unwrap_or_default();
    parsed
        .appnews
        .newsitems
        .into_iter()
        .map(|n| NewsItem { title: n.title.trim().to_string(), url: n.url, date: n.date, snippet: plain_text(&n.contents) })
        .collect()
}

/// Strips BBCode, HTML tags and Steam image placeholders from news bodies.
pub fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' | '<' => depth += 1,
            ']' | '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let words: Vec<&str> = out.split_whitespace().filter(|w| !w.starts_with("{STEAM_CLAN_IMAGE}")).collect();
    words.join(" ")
}

async fn achievements(client: &reqwest::Client, key: Option<String>, appid: u32) -> (Vec<Achievement>, AchievementStatus) {
    let Some(key) = key else { return (Vec::new(), AchievementStatus::NeedsKey) };
    let Some(user) = steam_account() else { return (Vec::new(), AchievementStatus::Unavailable) };
    let player = format!(
        "https://api.steampowered.com/ISteamUserStats/GetPlayerAchievements/v1/?key={key}&steamid={user}&appid={appid}&l=italian"
    );
    let schema = format!("https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?key={key}&appid={appid}&l=italian");
    let (player, schema) = tokio::join!(fetch(client, &player), fetch(client, &schema));
    match (player, schema) {
        (Ok(player), Ok(schema)) => merge_achievements(&player, &schema),
        // GetPlayerAchievements answers 400 for games without stats.
        (Err(_), Ok(schema)) if parse_schema(&schema).is_empty() => (Vec::new(), AchievementStatus::None),
        _ => (Vec::new(), AchievementStatus::Unavailable),
    }
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        bail!("Steam {}", resp.status());
    }
    Ok(resp.bytes().await?.to_vec())
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerResponse {
    playerstats: PlayerStats,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerStats {
    success: bool,
    achievements: Vec<PlayerAchievement>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerAchievement {
    apiname: String,
    achieved: u8,
    unlocktime: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SchemaResponse {
    game: SchemaGame,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct SchemaGame {
    available_game_stats: SchemaStats,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SchemaStats {
    achievements: Vec<SchemaAchievement>,
}

#[derive(Deserialize, Default, Clone)]
#[serde(default, rename_all = "camelCase")]
struct SchemaAchievement {
    name: String,
    display_name: String,
    description: String,
    icon: String,
    icongray: String,
}

fn parse_schema(body: &[u8]) -> Vec<SchemaAchievement> {
    serde_json::from_slice::<SchemaResponse>(body).unwrap_or_default().game.available_game_stats.achievements
}

fn merge_achievements(player: &[u8], schema: &[u8]) -> (Vec<Achievement>, AchievementStatus) {
    let player: PlayerResponse = serde_json::from_slice(player).unwrap_or_default();
    if !player.playerstats.success {
        return (Vec::new(), AchievementStatus::Unavailable);
    }
    let schema: HashMap<String, SchemaAchievement> = parse_schema(schema).into_iter().map(|a| (a.name.clone(), a)).collect();
    let mut list: Vec<Achievement> = player
        .playerstats
        .achievements
        .into_iter()
        .map(|p| {
            let s = schema.get(&p.apiname);
            let achieved = p.achieved == 1;
            Achievement {
                name: s.map(|s| s.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(p.apiname.clone()),
                description: s.map(|s| s.description.clone()).unwrap_or_default(),
                icon: s.map(|s| if achieved { s.icon.clone() } else { s.icongray.clone() }).filter(|i| !i.is_empty()),
                achieved,
                unlock_time: p.unlocktime,
            }
        })
        .collect();
    if list.is_empty() {
        return (list, AchievementStatus::None);
    }
    // Most recent unlocks first, locked ones after.
    list.sort_by(|a, b| b.achieved.cmp(&a.achieved).then(b.unlock_time.cmp(&a.unlock_time)));
    (list, AchievementStatus::Ok)
}

pub async fn deals(app: &AppHandle, force: bool) -> Deals {
    let state = app.state::<AppState>();
    if !force {
        if let Some((at, cached)) = state.steam.deals.lock().unwrap().as_ref() {
            if at.elapsed() < DEALS_TTL {
                return cached.clone();
            }
        }
    }
    let deals = match steam_account() {
        None => Deals { items: Vec::new(), status: DealsStatus::NoAccount },
        Some(user) => match load_deals(&client(), &user).await {
            Ok(items) if items.is_empty() => Deals { items, status: DealsStatus::Empty },
            Ok(items) => Deals { items, status: DealsStatus::Ok },
            Err(err) => {
                log::warn!("wishlist deals failed: {err:#}");
                Deals { items: Vec::new(), status: DealsStatus::Failed }
            }
        },
    };
    *state.steam.deals.lock().unwrap() = Some((Instant::now(), deals.clone()));
    deals
}

async fn load_deals(client: &reqwest::Client, user: &str) -> Result<Vec<Deal>> {
    let wishlist = fetch(client, &format!("https://api.steampowered.com/IWishlistService/GetWishlist/v1/?steamid={user}")).await?;
    let appids = parse_wishlist(&wishlist);
    let mut discounted: Vec<(u32, u32, String, String)> = Vec::new();
    for batch in appids.chunks(PRICE_BATCH) {
        let ids: Vec<String> = batch.iter().map(u32::to_string).collect();
        let url = format!("https://store.steampowered.com/api/appdetails?appids={}&cc=it&filters=price_overview", ids.join(","));
        discounted.extend(parse_prices(&fetch(client, &url).await?));
    }
    if discounted.is_empty() {
        return Ok(Vec::new());
    }
    let names = item_names(client, discounted.iter().map(|d| d.0)).await.unwrap_or_default();
    let mut deals: Vec<Deal> = discounted
        .into_iter()
        .map(|(appid, discount, initial, price)| Deal {
            appid,
            name: names.get(&appid).cloned().unwrap_or_else(|| format!("App {appid}")),
            discount,
            initial,
            price,
            cover: format!("{}/{appid}/library_600x900_2x.jpg", STEAM_CDN),
            hero: format!("{}/{appid}/library_hero.jpg", STEAM_CDN),
        })
        .collect();
    deals.sort_by(|a, b| b.discount.cmp(&a.discount));
    Ok(deals)
}

const STEAM_CDN: &str = "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps";

fn parse_wishlist(body: &[u8]) -> Vec<u32> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Resp {
        response: Items,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Items {
        items: Vec<Item>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Item {
        appid: u32,
    }
    serde_json::from_slice::<Resp>(body).unwrap_or_default().response.items.into_iter().map(|i| i.appid).collect()
}

/// `(appid, discount %, original price, current price)` of discounted apps.
fn parse_prices(body: &[u8]) -> Vec<(u32, u32, String, String)> {
    let Ok(map) = serde_json::from_slice::<HashMap<String, serde_json::Value>>(body) else { return Vec::new() };
    map.into_iter()
        .filter_map(|(id, v)| {
            let price = v.get("data")?.get("price_overview")?;
            let discount = price.get("discount_percent")?.as_u64()? as u32;
            if discount == 0 {
                return None;
            }
            let text = |k: &str| price.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
            Some((id.parse().ok()?, discount, text("initial_formatted"), text("final_formatted")))
        })
        .collect()
}

async fn item_names(client: &reqwest::Client, appids: impl Iterator<Item = u32>) -> Result<HashMap<u32, String>> {
    let ids: Vec<serde_json::Value> = appids.map(|a| serde_json::json!({ "appid": a })).collect();
    let input = serde_json::json!({ "ids": ids, "context": { "language": "italian", "country_code": "IT" } });
    let url = reqwest::Url::parse_with_params(
        "https://api.steampowered.com/IStoreBrowseService/GetItems/v1/",
        &[("input_json", input.to_string())],
    )?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let body: serde_json::Value = resp.json().await?;
    Ok(body["response"]["store_items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["appid"].as_u64()? as u32, i["name"].as_str()?.to_string())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup_from_news() {
        assert_eq!(
            plain_text("[img]{STEAM_CLAN_IMAGE}/1/x.png[/img] Patch <b>1.2</b> is   out [url=https://x]now[/url]"),
            "Patch 1.2 is out now"
        );
    }

    #[test]
    fn merges_player_progress_with_schema() {
        let player = br#"{"playerstats":{"success":true,"achievements":[
            {"apiname":"A","achieved":1,"unlocktime":100},
            {"apiname":"B","achieved":0,"unlocktime":0},
            {"apiname":"C","achieved":1,"unlocktime":200}]}}"#;
        let schema = br#"{"game":{"availableGameStats":{"achievements":[
            {"name":"A","displayName":"Primo","description":"d","icon":"a.jpg","icongray":"a_g.jpg"},
            {"name":"B","displayName":"Secondo","description":"","icon":"b.jpg","icongray":"b_g.jpg"}]}}}"#;
        let (list, status) = merge_achievements(player, schema);
        assert_eq!(status, AchievementStatus::Ok);
        let names: Vec<_> = list.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["C", "Primo", "Secondo"]);
        assert_eq!(list[2].icon.as_deref(), Some("b_g.jpg"));
        assert_eq!(merge_achievements(br#"{"playerstats":{"success":false}}"#, schema).1, AchievementStatus::Unavailable);
    }

    #[test]
    fn keeps_only_discounted_prices() {
        let body = br#"{"10":{"success":true,"data":{"price_overview":{"discount_percent":40,"initial_formatted":"20,00 EUR","final_formatted":"12,00 EUR"}}},
                       "20":{"success":true,"data":{"price_overview":{"discount_percent":0,"initial_formatted":"","final_formatted":"9,99 EUR"}}},
                       "30":{"success":true,"data":[]}}"#;
        assert_eq!(parse_prices(body), vec![(10, 40, "20,00 EUR".to_string(), "12,00 EUR".to_string())]);
        assert_eq!(parse_wishlist(br#"{"response":{"items":[{"appid":5,"priority":1}]}}"#), vec![5]);
    }
}
