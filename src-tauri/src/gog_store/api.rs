//! GOG's web API, as GOG Galaxy, Heroic and Lutris use it: OAuth with
//! Galaxy's public client, owned games, product details, installer links
//! and their checksum files. Parsing is kept pure for the tests.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tauri::Url;

/// GOG Galaxy's public OAuth client (the same one Heroic, Lutris and
/// minigalaxy use): not a secret of GameMode's or of the user's.
pub const CLIENT_ID: &str = "46899977096215655";
const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
pub const REDIRECT: &str = "https://embed.gog.com/on_login_success?origin=client";
const AUTH: &str = "https://auth.gog.com/auth";
const TOKEN: &str = "https://auth.gog.com/token";
const OWNED: &str = "https://embed.gog.com/user/data/games";
const PRODUCTS: &str = "https://api.gog.com/products";
const GAMES_V2: &str = "https://api.gog.com/v2/games";
/// The products endpoint takes at most 50 ids per call.
const PRODUCTS_BATCH: usize = 50;

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        // No total timeout: installers are gigabytes. A stalled transfer
        // still fails.
        .read_timeout(Duration::from_secs(60))
        .build()
        .unwrap_or_default()
}

pub fn login_url() -> String {
    Url::parse_with_params(
        AUTH,
        [("client_id", CLIENT_ID), ("redirect_uri", REDIRECT), ("response_type", "code"), ("layout", "client2")],
    )
    .map(String::from)
    .unwrap_or_default()
}

/// The login code, once the login page redirects to `REDIRECT`.
pub fn code_from_redirect(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if url.host_str() != Some("embed.gog.com") || url.path() != "/on_login_success" {
        return None;
    }
    url.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.into_owned()).filter(|c| !c.is_empty())
}

#[derive(Deserialize, Debug, Clone)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default = "default_expiry")]
    pub expires_in: u64,
    #[serde(default)]
    pub user_id: String,
}

fn default_expiry() -> u64 {
    3600
}

async fn token(client: &reqwest::Client, grant: &[(&str, &str)]) -> Result<Token> {
    let mut params = vec![("client_id", CLIENT_ID), ("client_secret", CLIENT_SECRET)];
    params.extend_from_slice(grant);
    let url = Url::parse_with_params(TOKEN, params)?;
    let resp = client.get(url).send().await.context("GOG non raggiungibile")?;
    let status = resp.status();
    if !status.is_success() {
        bail!(TokenRejected(status.as_u16()));
    }
    resp.json::<Token>().await.context("Risposta di GOG non valida")
}

/// GOG refused the code or refresh token (as opposed to a network error).
#[derive(Debug)]
pub struct TokenRejected(pub u16);

impl std::fmt::Display for TokenRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GOG ha rifiutato l'accesso ({})", self.0)
    }
}

impl std::error::Error for TokenRejected {}

pub async fn exchange_code(client: &reqwest::Client, code: &str) -> Result<Token> {
    token(client, &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT)]).await
}

pub async fn refresh(client: &reqwest::Client, refresh_token: &str) -> Result<Token> {
    token(client, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)]).await
}

// ---------- Owned games ----------

pub async fn owned_ids(client: &reqwest::Client, access: &str) -> Result<Vec<u64>> {
    let body = client.get(OWNED).bearer_auth(access).send().await?.error_for_status()?.bytes().await?;
    Ok(parse_owned(&body))
}

pub fn parse_owned(body: &[u8]) -> Vec<u64> {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Owned {
        owned: Vec<u64>,
    }
    serde_json::from_slice::<Owned>(body).unwrap_or_default().owned
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Product {
    pub id: u64,
    pub title: String,
    /// "game", "dlc" or "pack".
    pub game_type: String,
    pub is_installable: bool,
    pub content_system_compatibility: Compat,
    pub images: Images,
    pub downloads: Downloads,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Compat {
    pub windows: bool,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Images {
    pub background: Option<String>,
    pub logo2x: Option<String>,
}

impl Product {
    /// A Windows game that can be installed (not a DLC, bundle or movie).
    pub fn installable_game(&self) -> bool {
        self.game_type == "game" && self.is_installable && self.content_system_compatibility.windows && !self.title.trim().is_empty()
    }
}

/// Products for `ids`, in batches.
pub async fn products(client: &reqwest::Client, ids: &[u64]) -> Vec<Product> {
    let mut out = Vec::new();
    for batch in ids.chunks(PRODUCTS_BATCH) {
        let ids = batch.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let Ok(url) = Url::parse_with_params(PRODUCTS, [("ids", ids.as_str())]) else { continue };
        let body = match client.get(url).send().await.and_then(|r| r.error_for_status()) {
            Ok(resp) => resp.bytes().await.unwrap_or_default(),
            Err(err) => {
                log::warn!("GOG products: {err}");
                continue;
            }
        };
        out.extend(parse_products(&body));
    }
    out
}

pub fn parse_products(body: &[u8]) -> Vec<Product> {
    serde_json::from_slice(body).unwrap_or_default()
}

/// One product with its installers.
pub async fn product_with_downloads(client: &reqwest::Client, id: &str) -> Result<Product> {
    let url = format!("{PRODUCTS}/{id}?expand=downloads");
    let body = client.get(url).send().await?.error_for_status()?.bytes().await?;
    serde_json::from_slice(&body).context("Risposta di GOG non valida")
}

/// Vertical box art from the v2 game endpoint.
pub async fn box_art(client: &reqwest::Client, id: u64) -> Option<String> {
    let body = client.get(format!("{GAMES_V2}/{id}")).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
    parse_box_art(&body)
}

pub fn parse_box_art(body: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(body).ok()?;
    json.pointer("/_links/boxArtImage/href")?.as_str().filter(|s| !s.is_empty()).map(String::from)
}

/// GOG image links are protocol-relative (`//images-1.gog-statics.com/…`).
pub fn https(url: &str) -> String {
    match url.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => url.to_string(),
    }
}

// ---------- Installers ----------

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Downloads {
    pub installers: Vec<Installer>,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct Installer {
    pub id: String,
    pub os: String,
    pub language: String,
    pub version: Option<String>,
    pub total_size: u64,
    pub files: Vec<InstallerFile>,
}

#[derive(Deserialize, Default, Debug, Clone)]
#[serde(default)]
pub struct InstallerFile {
    pub id: String,
    /// Rounded by GOG: only good for progress, never for verification.
    pub size: u64,
    pub downlink: String,
}

/// The Windows installer in the first available language of `langs`,
/// else English, else any.
pub fn pick_installer<'a>(installers: &'a [Installer], langs: &[&str]) -> Option<&'a Installer> {
    let windows: Vec<&Installer> = installers.iter().filter(|i| i.os == "windows" && !i.files.is_empty()).collect();
    langs
        .iter()
        .chain(["en"].iter())
        .find_map(|lang| windows.iter().find(|i| i.language.eq_ignore_ascii_case(lang)).copied())
        .or_else(|| windows.first().copied())
}

#[derive(Deserialize, Debug, Clone)]
pub struct Downlink {
    pub downlink: String,
    #[serde(default)]
    pub checksum: Option<String>,
}

/// Resolves an installer file's API link to a signed CDN link.
pub async fn downlink(client: &reqwest::Client, access: &str, api_link: &str) -> Result<Downlink> {
    let body = client.get(api_link).bearer_auth(access).send().await?.error_for_status()?.bytes().await?;
    let link: Downlink = serde_json::from_slice(&body).context("Link di download non valido")?;
    if !link.downlink.starts_with("https://") {
        bail!("Link di download non sicuro");
    }
    Ok(link)
}

#[derive(Debug, PartialEq, Clone)]
pub struct Checksum {
    pub name: String,
    pub md5: Option<String>,
    pub total_size: Option<u64>,
}

/// GOG's checksum file: `<file name="setup_x.exe" md5="…" total_size="…">`.
pub fn parse_checksum(xml: &str) -> Option<Checksum> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}')).ok()?;
    let root = doc.root_element();
    if !root.has_tag_name("file") {
        return None;
    }
    let md5 = root
        .attribute("md5")
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit()));
    Some(Checksum {
        name: root.attribute("name").map(sanitize_file_name).unwrap_or_default(),
        md5,
        total_size: root.attribute("total_size").and_then(|s| s.parse().ok()),
    })
}

pub async fn checksum(client: &reqwest::Client, url: &str) -> Option<Checksum> {
    if !url.starts_with("https://") {
        return None;
    }
    let text = client.get(url).send().await.ok()?.error_for_status().ok()?.text().await.ok()?;
    parse_checksum(&text)
}

/// File name of a CDN link: its `path` parameter when it has one, else the
/// last segment of the URL path.
pub fn file_name_of(link: &str) -> Option<String> {
    let url = Url::parse(link).ok()?;
    let from_query = url.query_pairs().find(|(k, _)| k == "path").map(|(_, v)| v.into_owned());
    let path = from_query.unwrap_or_else(|| url.path().to_string());
    let last = path.rsplit('/').find(|s| !s.is_empty())?;
    let decoded = percent_decode(last);
    let name = sanitize_file_name(&decoded);
    (!name.is_empty()).then_some(name)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(b) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A bare file name: no folders, nothing Windows refuses.
pub fn sanitize_file_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') && !c.is_control()).collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_string();
    if cleaned == "." || cleaned == ".." { String::new() } else { cleaned }
}

/// Folder for a game, from its title: "STAR WARS™: Knights of the Old
/// Republic" -> "STAR WARS Knights of the Old Republic".
pub fn folder_name(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        let c = match c {
            '™' | '®' | '©' => continue,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => continue,
            c => c,
        };
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    let out = out.trim().trim_end_matches('.').trim().to_string();
    if out.is_empty() { "Gioco GOG".to_string() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_login_url_and_reads_the_code() {
        let url = login_url();
        assert!(url.starts_with("https://auth.gog.com/auth?client_id=46899977096215655"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient"));
        assert_eq!(
            code_from_redirect("https://embed.gog.com/on_login_success?origin=client&code=abc-123").as_deref(),
            Some("abc-123")
        );
        assert!(code_from_redirect("https://login.gog.com/?code=abc").is_none());
        assert!(code_from_redirect("https://embed.gog.com/on_login_success?origin=client").is_none());
    }

    #[test]
    fn reads_owned_games_and_products() {
        assert_eq!(parse_owned(br#"{"owned":[1207664643,1,2]}"#), vec![1207664643, 1, 2]);
        assert!(parse_owned(b"nope").is_empty());
        let body = br#"[
          {"id":1207664643,"title":"The Witcher 3","game_type":"game","is_installable":true,
           "content_system_compatibility":{"windows":true,"osx":false},
           "images":{"background":"//images-4.gog-statics.com/bg.jpg","logo2x":"//images-4.gog-statics.com/l.jpg"}},
          {"id":2,"title":"Some DLC","game_type":"dlc","is_installable":true,"content_system_compatibility":{"windows":true}},
          {"id":3,"title":"Mac only","game_type":"game","is_installable":true,"content_system_compatibility":{"windows":false}}
        ]"#;
        let products = parse_products(body);
        let games: Vec<_> = products.iter().filter(|p| p.installable_game()).collect();
        assert_eq!(games.len(), 1);
        assert_eq!(https(games[0].images.background.as_deref().unwrap()), "https://images-4.gog-statics.com/bg.jpg");
        assert_eq!(
            parse_box_art(br#"{"_links":{"boxArtImage":{"href":"https://images.gog-statics.com/box.jpg"}}}"#).as_deref(),
            Some("https://images.gog-statics.com/box.jpg")
        );
    }

    #[test]
    fn picks_the_windows_installer_by_language() {
        let body = br#"{"id":1,"title":"UT2004","downloads":{"installers":[
          {"id":"installer_mac_en","os":"mac","language":"en","files":[{"id":"m","size":1,"downlink":"x"}]},
          {"id":"installer_windows_de","os":"windows","language":"de","files":[{"id":"de1installer0","size":1,"downlink":"x"}]},
          {"id":"installer_windows_en","os":"windows","language":"en","total_size":2795503616,"files":[
            {"id":"en1installer0","size":1048576,"downlink":"https://api.gog.com/products/1/downlink/installer/en1installer0"},
            {"id":"en1installer1","size":2794455040,"downlink":"https://api.gog.com/products/1/downlink/installer/en1installer1"}]}
        ]}}"#;
        let product: Product = serde_json::from_slice(body).unwrap();
        let installers = &product.downloads.installers;
        assert_eq!(pick_installer(installers, &["it"]).unwrap().id, "installer_windows_en");
        assert_eq!(pick_installer(installers, &["de"]).unwrap().id, "installer_windows_de");
        assert_eq!(pick_installer(installers, &["it"]).unwrap().files.len(), 2);
        assert!(pick_installer(&installers[..1], &["it"]).is_none());
    }

    #[test]
    fn reads_checksum_files() {
        let xml = r#"<file name="setup_ut2004_2.0.0.6.exe" available="1" notavailablemsg="" md5="0D2F3A6B8C1E5F4A9B7C6D5E4F3A2B1C" chunks="2" timestamp="2020-01-01" total_size="1312344">
            <chunk id="0" from="0" to="10485759" method="md5">aaaa</chunk></file>"#;
        assert_eq!(
            parse_checksum(xml),
            Some(Checksum {
                name: "setup_ut2004_2.0.0.6.exe".into(),
                md5: Some("0d2f3a6b8c1e5f4a9b7c6d5e4f3a2b1c".into()),
                total_size: Some(1312344),
            })
        );
        assert_eq!(parse_checksum(r#"<file name="..\evil.exe" md5="nothex"/>"#).unwrap().name, "evil.exe");
        assert!(parse_checksum(r#"<file name="x.exe" md5="nothex"/>"#).unwrap().md5.is_none());
        assert!(parse_checksum("<html/>").is_none());
    }

    #[test]
    fn names_files_and_folders_safely() {
        assert_eq!(
            file_name_of("https://gog-cdn-fastly.gog.com/token=nva=1~hmac=2/secure/ut/setup_ut2004_2.0.0.6.exe?x=1").as_deref(),
            Some("setup_ut2004_2.0.0.6.exe")
        );
        assert_eq!(
            file_name_of("https://cdn.gog.com/download?path=%2Fsecure%2Fut%2Fsetup%20ut-1.bin&token=x").as_deref(),
            Some("setup ut-1.bin")
        );
        assert_eq!(sanitize_file_name(r"..\..\Windows\evil.exe"), "evil.exe");
        assert_eq!(sanitize_file_name(".."), "");
        assert_eq!(folder_name("STAR WARS™: Knights of the Old Republic"), "STAR WARS Knights of the Old Republic");
        assert_eq!(folder_name("Who? / What*"), "Who What");
        assert_eq!(folder_name("..."), "Gioco GOG");
    }
}
