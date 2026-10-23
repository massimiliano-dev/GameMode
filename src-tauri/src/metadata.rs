//! Store page facts for the library: description, genres, developer, release
//! date, critic score, screenshots and, through IGDB, how long a game takes.
//! Steam, the Microsoft Store and GOG answer without keys; IGDB fills the
//! gaps (and covers Epic/EA/Ubisoft/manual games) once the user registered a
//! Twitch app. Everything is plain HTTPS, nothing Windows-specific.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, Semaphore};

use crate::artwork::{safe_name, search_name, urlencode};
use crate::model::{now, Game, GameMeta, Settings, Store, TimeToBeat};
use crate::AppState;

const CONCURRENCY: usize = 3;
/// Games nobody knows about are retried at most once a week.
const RETRY_AFTER_SECS: i64 = 7 * 24 * 3600;
const MAX_SCREENSHOTS: usize = 4;
/// Steam's store API tolerates roughly 200 requests per 5 minutes per IP;
/// one request every 1.6 s stays well under that.
const STEAM_PACE: Duration = Duration::from_millis(1600);
/// How long to wait after Steam says "too many requests" before one retry.
const STEAM_BACKOFF: Duration = Duration::from_secs(60);
/// IGDB allows 4 requests per second.
const IGDB_PACE: Duration = Duration::from_millis(300);
const MS_CATALOG: &str = "https://displaycatalog.mp.microsoft.com/v7.0/products/lookup";
const GOG_API: &str = "https://api.gog.com/products";
const IGDB_API: &str = "https://api.igdb.com/v4";
const TWITCH_TOKEN: &str = "https://id.twitch.tv/oauth2/token";
/// IGDB platform id of "PC (Microsoft Windows)".
const IGDB_PC: u32 = 6;
/// Bump when the pipeline gains new sources, so games that previously came
/// up empty are retried right away instead of after `RETRY_AFTER_SECS`.
pub const PIPELINE_VERSION: u32 = 1;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Looks up every game without a card, in the background. `force` also
/// completes partial cards through IGDB (after its keys were entered).
pub fn refresh(app: &AppHandle, force: bool) {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let (games, igdb) = {
            let data = state.db.lock();
            let igdb = igdb_credentials(&data.settings);
            let due = |g: &&Game| is_due(g, force, igdb.is_some());
            let games: Vec<Game> = data.games.iter().filter(due).cloned().collect();
            (games, igdb)
        };
        if games.is_empty() {
            RUNNING.store(false, Ordering::SeqCst);
            return;
        }
        let fetcher = Arc::new(Fetcher {
            client: reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default(),
            dir: state.data_dir.join("artwork"),
            igdb: igdb.map(|(id, secret)| Igdb { client_id: id, client_secret: secret, token: Mutex::new(None) }),
            steam_gate: Mutex::new(()),
            steam_blocked: AtomicBool::new(false),
            igdb_gate: Mutex::new(()),
        });
        let limit = Arc::new(Semaphore::new(CONCURRENCY));
        let mut tasks = Vec::new();
        for game in games {
            let (fetcher, limit, app) = (fetcher.clone(), limit.clone(), app.clone());
            tasks.push(tauri::async_runtime::spawn(async move {
                let _permit = limit.acquire().await;
                match fetcher.fetch(&game).await {
                    Ok(meta) => save(&app, &game.id, meta),
                    // Network trouble (no Wi-Fi yet after sign-in, rate limits):
                    // leave the game unmarked so the next scan tries again.
                    Err(err) => eprintln!("metadata for {} failed: {err:#}", game.id),
                }
            }));
        }
        for task in tasks {
            let _ = task.await;
        }
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// Whether the pipeline should look at `game` now.
fn is_due(game: &Game, force: bool, igdb: bool) -> bool {
    match &game.meta {
        None => force || game.meta_checked_at.is_none_or(|t| now() - t > RETRY_AFTER_SECS),
        // A card from a store misses what only IGDB knows (time to beat,
        // critic score); ask once, when keys are there.
        Some(meta) => igdb && !meta.igdb_checked && is_incomplete(meta),
    }
}

fn is_incomplete(meta: &GameMeta) -> bool {
    meta.time_to_beat.is_none() || meta.critic_score.is_none() || meta.summary.is_none() || meta.genres.is_empty()
}

fn igdb_credentials(settings: &Settings) -> Option<(String, String)> {
    let id = settings.igdb_client_id.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    let secret = settings.igdb_client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty())?;
    Some((id.to_string(), secret.to_string()))
}

/// `None` means every source answered and none knows the game.
fn save(app: &AppHandle, id: &str, meta: Option<GameMeta>) {
    let state = app.state::<AppState>();
    let updated = state.db.update(|d| {
        let game = d.game_mut(id)?;
        if meta.is_some() {
            game.meta = meta;
        }
        game.meta_checked_at = Some(now());
        d.view_game(id)
    });
    if let Ok(Some(game)) = updated {
        let _ = app.emit("library://game", &game);
    }
}

/// A card plus the screenshots still to download.
#[derive(Default, Debug, PartialEq)]
struct Found {
    meta: GameMeta,
    shots: Vec<String>,
}

struct Igdb {
    client_id: String,
    client_secret: String,
    token: Mutex<Option<String>>,
}

struct Fetcher {
    client: reqwest::Client,
    dir: PathBuf,
    igdb: Option<Igdb>,
    /// Serialises Steam requests so they can be paced.
    steam_gate: Mutex<()>,
    /// Set once Steam rate-limited us twice: the rest waits for the next scan.
    steam_blocked: AtomicBool,
    igdb_gate: Mutex<()>,
}

impl Fetcher {
    /// `Err` is a network problem (nothing gets marked), `Ok(None)` means the
    /// game is unknown everywhere.
    async fn fetch(&self, game: &Game) -> Result<Option<GameMeta>> {
        let mut found: Option<Found> = game.meta.clone().map(|meta| Found { meta, shots: Vec::new() });
        if found.is_none() {
            found = match game.steam_appid {
                Some(appid) => self.from_steam(appid).await?,
                None => match game.store {
                    Store::Xbox => self.from_xbox(game).await?,
                    Store::Gog => self.from_gog(game).await?,
                    _ => None,
                },
            };
        }
        if let Some(igdb) = &self.igdb {
            if found.as_ref().is_none_or(|f| !f.meta.igdb_checked && is_incomplete(&f.meta)) {
                match self.from_igdb(igdb, game).await {
                    Ok(Some(extra)) => match &mut found {
                        Some(f) => merge(f, extra),
                        None => found = Some(extra),
                    },
                    Ok(None) => {}
                    Err(err) if found.is_some() => eprintln!("IGDB for {} failed: {err:#}", game.id),
                    Err(err) => return Err(err),
                }
                if let Some(f) = &mut found {
                    f.meta.igdb_checked = true;
                }
            }
        }
        let Some(mut found) = found else { return Ok(None) };
        if found.meta.screenshots.is_empty() && !found.shots.is_empty() {
            found.meta.screenshots = self.download_shots(&game.id, &found.shots).await;
        }
        Ok(Some(found.meta))
    }

    async fn from_steam(&self, appid: u32) -> Result<Option<Found>> {
        if self.steam_blocked.load(Ordering::SeqCst) {
            bail!("Steam store API is rate limiting us");
        }
        let url = format!("https://store.steampowered.com/api/appdetails?appids={appid}&cc=it&l=italian");
        let body = {
            let _gate = self.steam_gate.lock().await;
            let body = self.steam_get(&url).await;
            tokio::time::sleep(STEAM_PACE).await;
            body
        }?;
        Ok(parse_steam(&body, appid))
    }

    async fn steam_get(&self, url: &str) -> Result<Vec<u8>> {
        for attempt in 0..2 {
            let resp = self.client.get(url).send().await?;
            let status = resp.status();
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status == reqwest::StatusCode::FORBIDDEN {
                if attempt == 0 {
                    tokio::time::sleep(STEAM_BACKOFF).await;
                    continue;
                }
                self.steam_blocked.store(true, Ordering::SeqCst);
                bail!("Steam {status}");
            }
            if !status.is_success() {
                bail!("Steam {status}");
            }
            return Ok(resp.bytes().await?.to_vec());
        }
        unreachable!()
    }

    async fn from_xbox(&self, game: &Game) -> Result<Option<Found>> {
        // Xbox game ids are `xbox:<PackageFamilyName>`.
        let pfn = native_id(game);
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

    async fn from_gog(&self, game: &Game) -> Result<Option<Found>> {
        // GOG game ids are `gog:<product id>`.
        let url = format!("{GOG_API}/{}?expand=description,screenshots&locale=it-IT", urlencode(native_id(game)));
        let resp = self.client.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("GOG {}", resp.status());
        }
        Ok(parse_gog(&resp.bytes().await?))
    }

    async fn from_igdb(&self, igdb: &Igdb, game: &Game) -> Result<Option<Found>> {
        let name = search_name(&game.name);
        let query = format!(
            "search \"{}\"; fields name,summary,first_release_date,aggregated_rating,rating,version_parent,\
             genres.name,involved_companies.developer,involved_companies.company.name,screenshots.image_id; \
             where platforms = ({IGDB_PC}); limit 8;",
            name.replace(['"', '\\'], " ")
        );
        let body = self.igdb_post(igdb, "games", &query).await?;
        let Some(hit) = pick_igdb(parse_igdb_games(&body), &name) else { return Ok(None) };
        let ttb = match self.igdb_post(igdb, "game_time_to_beats", &format!("fields hastily,normally,completely; where game_id = {}; limit 1;", hit.id)).await {
            Ok(body) => parse_igdb_ttb(&body),
            Err(err) => {
                eprintln!("IGDB time to beat for {} failed: {err:#}", game.id);
                None
            }
        };
        Ok(Some(igdb_found(hit, ttb)))
    }

    async fn igdb_post(&self, igdb: &Igdb, endpoint: &str, query: &str) -> Result<Vec<u8>> {
        let _gate = self.igdb_gate.lock().await;
        for attempt in 0..2 {
            let token = self.igdb_token(igdb, attempt > 0).await?;
            let resp = self
                .client
                .post(format!("{IGDB_API}/{endpoint}"))
                .header("Client-ID", &igdb.client_id)
                .bearer_auth(&token)
                .header(reqwest::header::ACCEPT, "application/json")
                .body(query.to_string())
                .send()
                .await?;
            tokio::time::sleep(IGDB_PACE).await;
            let status = resp.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                continue; // expired token: fetch a new one and retry
            }
            if !status.is_success() {
                bail!("IGDB {status}");
            }
            return Ok(resp.bytes().await?.to_vec());
        }
        unreachable!()
    }

    /// App access token from Twitch (client credentials), cached for this run.
    async fn igdb_token(&self, igdb: &Igdb, renew: bool) -> Result<String> {
        let mut cached = igdb.token.lock().await;
        if let (false, Some(token)) = (renew, cached.as_ref()) {
            return Ok(token.clone());
        }
        let url = format!(
            "{TWITCH_TOKEN}?client_id={}&client_secret={}&grant_type=client_credentials",
            urlencode(&igdb.client_id),
            urlencode(&igdb.client_secret)
        );
        let resp = self.client.post(url).send().await?;
        if !resp.status().is_success() {
            bail!("Twitch token {} (controlla Client ID e Client Secret)", resp.status());
        }
        #[derive(Deserialize)]
        struct Token {
            access_token: String,
        }
        let token: Token = resp.json().await?;
        *cached = Some(token.access_token.clone());
        Ok(token.access_token)
    }

    /// Caches up to `MAX_SCREENSHOTS` images next to the game's artwork.
    async fn download_shots(&self, game_id: &str, urls: &[String]) -> Vec<String> {
        let dir = self.dir.join(safe_name(game_id));
        let _ = tokio::fs::create_dir_all(&dir).await;
        let mut paths = Vec::new();
        for (i, url) in urls.iter().take(MAX_SCREENSHOTS).enumerate() {
            match self.download(url, &dir, i).await {
                Ok(path) => paths.push(path.to_string_lossy().into_owned()),
                Err(err) => eprintln!("screenshot {url} failed: {err:#}"),
            }
        }
        paths
    }

    async fn download(&self, url: &str, dir: &Path, index: usize) -> Result<PathBuf> {
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
        let path = dir.join(format!("shot_{index}.{ext}"));
        tokio::fs::write(&path, &bytes).await?;
        Ok(path)
    }
}

fn native_id(game: &Game) -> &str {
    game.id.split_once(':').map_or(game.id.as_str(), |(_, id)| id)
}

/// Fills what `base` lacks with `extra` (IGDB after a store).
fn merge(base: &mut Found, extra: Found) {
    let (m, e) = (&mut base.meta, extra.meta);
    if m.summary.is_none() {
        m.summary = e.summary;
    }
    if m.genres.is_empty() {
        m.genres = e.genres;
    }
    if m.developer.is_none() {
        m.developer = e.developer;
    }
    if m.release_date.is_none() {
        m.release_date = e.release_date;
    }
    if m.critic_score.is_none() {
        m.critic_score = e.critic_score;
        m.score_source = e.score_source;
    }
    if m.time_to_beat.is_none() {
        m.time_to_beat = e.time_to_beat;
    }
    if base.shots.is_empty() {
        base.shots = extra.shots;
    }
}

// ---------- Steam ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamEntry {
    success: bool,
    data: SteamData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamData {
    short_description: String,
    genres: Vec<SteamGenre>,
    developers: Vec<String>,
    release_date: SteamRelease,
    metacritic: Option<SteamMetacritic>,
    screenshots: Vec<SteamShot>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamGenre {
    description: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamRelease {
    date: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamMetacritic {
    score: u8,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SteamShot {
    path_full: String,
}

fn parse_steam(body: &[u8], appid: u32) -> Option<Found> {
    let mut map: std::collections::HashMap<String, SteamEntry> = serde_json::from_slice(body).ok()?;
    let entry = map.remove(&appid.to_string())?;
    if !entry.success {
        return None;
    }
    let d = entry.data;
    let score = d.metacritic.map(|m| m.score.min(100));
    Some(Found {
        meta: GameMeta {
            summary: non_empty(strip_html(&d.short_description)),
            genres: d.genres.into_iter().map(|g| g.description).filter(|g| !g.is_empty()).collect(),
            developer: d.developers.into_iter().find(|s| !s.trim().is_empty()),
            release_date: non_empty(d.release_date.date),
            critic_score: score,
            score_source: score.map(|_| "metacritic".to_string()),
            ..GameMeta::default()
        },
        shots: d.screenshots.into_iter().map(|s| s.path_full).filter(|u| !u.is_empty()).collect(),
    })
}

// ---------- Microsoft Store ----------

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsCatalog {
    products: Vec<MsProduct>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsProduct {
    localized_properties: Vec<MsLocalized>,
    properties: MsProperties,
    market_properties: Vec<MsMarket>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsLocalized {
    short_description: String,
    product_description: String,
    developer_name: String,
    publisher_name: String,
    images: Vec<MsImage>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsImage {
    image_purpose: String,
    uri: String,
    width: u32,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsProperties {
    category: String,
    categories: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct MsMarket {
    original_release_date: String,
}

fn parse_ms_catalog(body: &[u8]) -> Option<Found> {
    let catalog: MsCatalog = serde_json::from_slice(body).ok()?;
    let product = catalog.products.into_iter().next()?;
    let local = product.localized_properties.into_iter().next().unwrap_or_default();
    let mut genres = product.properties.categories;
    if genres.is_empty() && !product.properties.category.is_empty() {
        genres.push(product.properties.category);
    }
    let summary = non_empty(strip_html(&local.short_description)).or_else(|| non_empty(strip_html(&local.product_description)));
    let shots = local
        .images
        .iter()
        .filter(|i| i.image_purpose == "Screenshot")
        .map(|i| {
            let uri = if i.uri.starts_with("//") { format!("https:{}", i.uri) } else { i.uri.clone() };
            if i.width > 1280 { format!("{uri}?w=1280") } else { uri }
        })
        .collect();
    Some(Found {
        meta: GameMeta {
            summary,
            genres,
            developer: non_empty(local.developer_name).or_else(|| non_empty(local.publisher_name)),
            release_date: product.market_properties.first().and_then(|m| iso_date(&m.original_release_date)),
            ..GameMeta::default()
        },
        shots,
    })
}

// ---------- GOG ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct GogProduct {
    release_date: Option<String>,
    description: GogDescription,
    screenshots: Vec<GogShot>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GogDescription {
    lead: String,
    full: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GogShot {
    formatter_template_url: String,
}

fn parse_gog(body: &[u8]) -> Option<Found> {
    let p: GogProduct = serde_json::from_slice(body).ok()?;
    let summary = non_empty(strip_html(&p.description.lead)).or_else(|| non_empty(strip_html(&p.description.full)));
    Some(Found {
        meta: GameMeta {
            summary,
            release_date: p.release_date.as_deref().and_then(iso_date),
            ..GameMeta::default()
        },
        shots: p
            .screenshots
            .iter()
            .filter(|s| s.formatter_template_url.contains("{formatter}"))
            .map(|s| s.formatter_template_url.replace("{formatter}", "ggvgm"))
            .collect(),
    })
}

// ---------- IGDB ----------

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
struct IgdbGame {
    id: u64,
    name: String,
    summary: Option<String>,
    first_release_date: Option<i64>,
    aggregated_rating: Option<f64>,
    rating: Option<f64>,
    version_parent: Option<u64>,
    genres: Vec<IgdbNamed>,
    involved_companies: Vec<IgdbInvolved>,
    screenshots: Vec<IgdbImage>,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
struct IgdbNamed {
    name: String,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
struct IgdbInvolved {
    developer: bool,
    company: IgdbNamed,
}

#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
struct IgdbImage {
    image_id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct IgdbTtb {
    hastily: Option<u64>,
    normally: Option<u64>,
    completely: Option<u64>,
}

fn parse_igdb_games(body: &[u8]) -> Vec<IgdbGame> {
    serde_json::from_slice(body).unwrap_or_default()
}

fn parse_igdb_ttb(body: &[u8]) -> Option<TimeToBeat> {
    let list: Vec<IgdbTtb> = serde_json::from_slice(body).ok()?;
    let t = list.into_iter().next()?;
    let ttb = TimeToBeat { main: t.hastily, extra: t.normally, complete: t.completely };
    (ttb != TimeToBeat::default()).then_some(ttb)
}

/// Letters and digits only, lowercase: "Baldur's Gate 3" == "Baldurs Gate 3".
fn fold(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// IGDB's search ranks remasters and editions loosely; prefer the exact
/// title, then a released, original (not a "version of") entry.
fn pick_igdb(games: Vec<IgdbGame>, name: &str) -> Option<IgdbGame> {
    let wanted = fold(name);
    games.into_iter().max_by_key(|g| {
        let folded = fold(&g.name);
        (
            folded == wanted,
            folded.starts_with(&wanted) || wanted.starts_with(&folded),
            g.first_release_date.is_some(),
            g.version_parent.is_none(),
            g.aggregated_rating.is_some(),
        )
    })
}

fn igdb_found(g: IgdbGame, ttb: Option<TimeToBeat>) -> Found {
    let score = g.aggregated_rating.or(g.rating).map(|r| r.round().clamp(0.0, 100.0) as u8);
    Found {
        meta: GameMeta {
            summary: g.summary.and_then(non_empty),
            genres: g.genres.into_iter().map(|n| n.name).filter(|n| !n.is_empty()).collect(),
            developer: g.involved_companies.into_iter().find(|c| c.developer && !c.company.name.is_empty()).map(|c| c.company.name),
            release_date: g.first_release_date.map(unix_to_iso),
            critic_score: score,
            score_source: score.map(|_| "igdb".to_string()),
            time_to_beat: ttb,
            ..GameMeta::default()
        },
        shots: g
            .screenshots
            .into_iter()
            .filter(|s| !s.image_id.is_empty())
            .map(|s| format!("https://images.igdb.com/igdb/image/upload/t_screenshot_big/{}.jpg", s.image_id))
            .collect(),
    }
}

// ---------- Text helpers ----------

fn non_empty(s: String) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Drops tags and the usual entities; store blurbs are mostly plain text
/// with the odd `<br>` or `&quot;`.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "2021-11-09T00:00:00.0000000Z" → "2021-11-09".
fn iso_date(s: &str) -> Option<String> {
    let date = s.get(..10)?;
    let ok = date.len() == 10 && date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() });
    ok.then(|| date.to_string())
}

/// Unix seconds → "YYYY-MM-DD" (proleptic Gregorian, UTC).
fn unix_to_iso(ts: i64) -> String {
    let days = ts.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchSpec;

    #[test]
    fn parses_steam_app_details() {
        let body = br#"{"1245620":{"success":true,"data":{"short_description":"Un nuovo GdR fantasy &quot;epico&quot;.<br>Alzati, Senzaluce.","developers":["FromSoftware Inc."],"genres":[{"id":"1","description":"Azione"},{"id":"3","description":"GdR"}],"metacritic":{"score":94,"url":"x"},"release_date":{"coming_soon":false,"date":"25 feb 2022"},"screenshots":[{"id":0,"path_thumbnail":"t.jpg","path_full":"https://cdn/full0.jpg"},{"id":1,"path_thumbnail":"t.jpg","path_full":"https://cdn/full1.jpg"}]}}}"#;
        let found = parse_steam(body, 1245620).unwrap();
        assert_eq!(found.meta.summary.as_deref(), Some("Un nuovo GdR fantasy \"epico\". Alzati, Senzaluce."));
        assert_eq!(found.meta.genres, ["Azione", "GdR"]);
        assert_eq!(found.meta.developer.as_deref(), Some("FromSoftware Inc."));
        assert_eq!(found.meta.release_date.as_deref(), Some("25 feb 2022"));
        assert_eq!(found.meta.critic_score, Some(94));
        assert_eq!(found.meta.score_source.as_deref(), Some("metacritic"));
        assert_eq!(found.shots, ["https://cdn/full0.jpg", "https://cdn/full1.jpg"]);
        assert!(!found.meta.igdb_checked);

        assert!(parse_steam(br#"{"1245620":{"success":false}}"#, 1245620).is_none());
        assert!(parse_steam(br#"{"1":{"success":true,"data":{}}}"#, 1245620).is_none());
        assert!(parse_steam(b"not json", 1).is_none());
    }

    #[test]
    fn parses_ms_catalog_details() {
        let body = br#"{"Products":[{"LocalizedProperties":[{"DeveloperName":"Playground Games","PublisherName":"Xbox Game Studios","ProductTitle":"Forza Horizon 5","ShortDescription":"La tua avventura Horizon ti aspetta!","ProductDescription":"Lunga descrizione","Images":[{"ImagePurpose":"Poster","Uri":"//store-images.s-microsoft.com/p.png","Width":720},{"ImagePurpose":"Screenshot","Uri":"//store-images.s-microsoft.com/s1.png","Width":3840},{"ImagePurpose":"Screenshot","Uri":"https://store-images.s-microsoft.com/s2.png","Width":1280}]}],"Properties":{"Category":"Racing & flying","Categories":["Racing & flying","Simulation"]},"MarketProperties":[{"OriginalReleaseDate":"2021-11-09T00:00:00.0000000Z"}]}]}"#;
        let found = parse_ms_catalog(body).unwrap();
        assert_eq!(found.meta.summary.as_deref(), Some("La tua avventura Horizon ti aspetta!"));
        assert_eq!(found.meta.genres, ["Racing & flying", "Simulation"]);
        assert_eq!(found.meta.developer.as_deref(), Some("Playground Games"));
        assert_eq!(found.meta.release_date.as_deref(), Some("2021-11-09"));
        assert_eq!(found.meta.critic_score, None);
        assert_eq!(found.shots, ["https://store-images.s-microsoft.com/s1.png?w=1280", "https://store-images.s-microsoft.com/s2.png"]);
        assert!(parse_ms_catalog(br#"{"Products":[]}"#).is_none());
    }

    #[test]
    fn parses_gog_product() {
        let body = br#"{"id":1207664663,"title":"The Witcher 3","release_date":"2015-05-18T22:00:00+0300","description":{"lead":"<p>Diventa un <b>cacciatore</b> di mostri.</p>","full":"..."},"screenshots":[{"image_id":"abc","formatter_template_url":"https://images.gog-statics.com/abc_{formatter}.png"}]}"#;
        let found = parse_gog(body).unwrap();
        assert_eq!(found.meta.summary.as_deref(), Some("Diventa un cacciatore di mostri."));
        assert_eq!(found.meta.release_date.as_deref(), Some("2015-05-18"));
        assert_eq!(found.shots, ["https://images.gog-statics.com/abc_ggvgm.png"]);
    }

    #[test]
    fn picks_the_exact_igdb_title_over_editions() {
        let body = br#"[{"id":2,"name":"The Witcher 3: Wild Hunt - Game of the Year Edition","first_release_date":1472515200,"version_parent":1},{"id":1,"name":"The Witcher 3: Wild Hunt","summary":"Geralt.","first_release_date":1431993600,"aggregated_rating":91.6,"genres":[{"name":"Role-playing (RPG)"}],"involved_companies":[{"developer":false,"company":{"name":"Bandai Namco"}},{"developer":true,"company":{"name":"CD Projekt RED"}}],"screenshots":[{"image_id":"sc1"}]},{"id":3,"name":"The Witcher 3: Wild Hunt - Complete Edition"}]"#;
        let games = parse_igdb_games(body);
        assert_eq!(games.len(), 3);
        let hit = pick_igdb(games.clone(), "The Witcher 3: Wild Hunt").unwrap();
        assert_eq!(hit.id, 1);
        // No exact title: the released original wins over the edition.
        let hit = pick_igdb(games, "Witcher 3").unwrap();
        assert_eq!(hit.id, 1);
        assert!(pick_igdb(Vec::new(), "x").is_none());

        let ttb = parse_igdb_ttb(br#"[{"id":9,"game_id":1,"hastily":185400,"normally":369000,"completely":624600}]"#).unwrap();
        assert_eq!(ttb, TimeToBeat { main: Some(185400), extra: Some(369000), complete: Some(624600) });
        assert!(parse_igdb_ttb(b"[]").is_none());
        assert!(parse_igdb_ttb(br#"[{"id":9,"game_id":1}]"#).is_none());

        let found = igdb_found(parse_igdb_games(body).swap_remove(1), Some(ttb));
        assert_eq!(found.meta.summary.as_deref(), Some("Geralt."));
        assert_eq!(found.meta.genres, ["Role-playing (RPG)"]);
        assert_eq!(found.meta.developer.as_deref(), Some("CD Projekt RED"));
        assert_eq!(found.meta.release_date.as_deref(), Some("2015-05-19"));
        assert_eq!(found.meta.critic_score, Some(92));
        assert_eq!(found.meta.score_source.as_deref(), Some("igdb"));
        assert_eq!(found.shots, ["https://images.igdb.com/igdb/image/upload/t_screenshot_big/sc1.jpg"]);
    }

    #[test]
    fn merge_only_fills_gaps() {
        let mut base = Found {
            meta: GameMeta { summary: Some("Steam".into()), critic_score: Some(80), score_source: Some("metacritic".into()), ..Default::default() },
            shots: vec!["s.jpg".into()],
        };
        let extra = Found {
            meta: GameMeta {
                summary: Some("IGDB".into()),
                genres: vec!["Azione".into()],
                critic_score: Some(70),
                score_source: Some("igdb".into()),
                time_to_beat: Some(TimeToBeat { main: Some(3600), ..Default::default() }),
                ..Default::default()
            },
            shots: vec!["i.jpg".into()],
        };
        merge(&mut base, extra);
        assert_eq!(base.meta.summary.as_deref(), Some("Steam"));
        assert_eq!(base.meta.genres, ["Azione"]);
        assert_eq!(base.meta.critic_score, Some(80));
        assert_eq!(base.meta.score_source.as_deref(), Some("metacritic"));
        assert_eq!(base.meta.time_to_beat.as_ref().and_then(|t| t.main), Some(3600));
        assert_eq!(base.shots, ["s.jpg"]);
    }

    #[test]
    fn due_games_and_credentials() {
        let mut game = Game::new(Store::Epic, "x", "X", LaunchSpec::Uri { uri: String::new() });
        assert!(is_due(&game, false, false));
        game.meta_checked_at = Some(now());
        assert!(!is_due(&game, false, false));
        assert!(is_due(&game, true, false));
        game.meta_checked_at = Some(now() - RETRY_AFTER_SECS - 1);
        assert!(is_due(&game, false, false));
        // A store card without time to beat is completed once IGDB keys exist, once.
        game.meta = Some(GameMeta { summary: Some("s".into()), ..Default::default() });
        assert!(!is_due(&game, true, false));
        assert!(is_due(&game, false, true));
        game.meta.as_mut().unwrap().igdb_checked = true;
        assert!(!is_due(&game, true, true));

        let mut settings = Settings::default();
        assert!(igdb_credentials(&settings).is_none());
        settings.igdb_client_id = Some(" id ".into());
        settings.igdb_client_secret = Some(String::new());
        assert!(igdb_credentials(&settings).is_none());
        settings.igdb_client_secret = Some("secret".into());
        assert_eq!(igdb_credentials(&settings), Some(("id".into(), "secret".into())));
    }

    #[test]
    fn text_helpers() {
        assert_eq!(strip_html("A &amp; B<br/>C  <i>D</i>"), "A & B C D");
        assert_eq!(iso_date("2021-11-09T00:00:00.0000000Z").as_deref(), Some("2021-11-09"));
        assert!(iso_date("9 nov 2021").is_none());
        assert_eq!(unix_to_iso(0), "1970-01-01");
        assert_eq!(unix_to_iso(1645747200), "2022-02-25");
        assert_eq!(fold("Baldur's Gate 3"), "baldursgate3");
    }
}
