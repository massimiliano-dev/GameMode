//! Pure parsing of what legendary prints and stores, unit tested.
//!
//! legendary logs to stderr as `[Module] LEVEL: message`; the download
//! manager prints a progress block every second, verification prints
//! `Verification progress:` on stdout with a carriage return.

use std::collections::HashMap;

use serde::Deserialize;

const MIB: f64 = 1024.0 * 1024.0;

/// What one line of output says about a running job.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Line {
    /// `Download size: 1234.56 MiB (Compression savings: …)`, in bytes.
    DownloadSize(u64),
    /// `= Progress: 12.34% (123/1000), Running for 00:01:02, ETA: 00:05:00`.
    Progress { fraction: f64, eta_secs: Option<u64> },
    /// ` - Downloaded: 123.45 MiB, Written: 234.56 MiB`, in bytes.
    Downloaded(u64),
    /// `Verification progress: 12/345 (3.5%) [12.3 MiB/s]`.
    Verifying(f64),
    /// An error worth showing to the user.
    Error,
    Other,
}

fn after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    line.find(marker).map(|i| &line[i + marker.len()..])
}

fn number(s: &str) -> Option<f64> {
    let end = s.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(s.len());
    s[..end].parse().ok()
}

fn mib(s: &str) -> Option<u64> {
    number(s.trim_start()).map(|v| (v * MIB).round() as u64)
}

/// `HH:MM:SS` → seconds.
fn clock(s: &str) -> Option<u64> {
    let mut parts = s.trim().splitn(3, ':').map(|p| p.trim().parse::<u64>().ok());
    let (h, m, sec) = (parts.next()??, parts.next()??, parts.next()??);
    Some(h * 3600 + m * 60 + sec)
}

pub fn parse_line(line: &str) -> Line {
    let line = line.trim();
    if let Some(rest) = after(line, "= Progress: ") {
        let Some(percent) = number(rest) else { return Line::Other };
        let eta_secs = after(rest, "ETA: ").and_then(|s| clock(s.get(..8).unwrap_or(s)));
        return Line::Progress { fraction: (percent / 100.0).clamp(0.0, 1.0), eta_secs };
    }
    if let Some(rest) = after(line, "Verification progress: ") {
        let percent = after(rest, "(").and_then(number);
        return percent.map_or(Line::Other, |p| Line::Verifying((p / 100.0).clamp(0.0, 1.0)));
    }
    if let Some(rest) = after(line, "Download size: ") {
        return mib(rest).map_or(Line::Other, Line::DownloadSize);
    }
    if let Some(rest) = after(line, "- Downloaded: ") {
        return mib(rest).map_or(Line::Other, Line::Downloaded);
    }
    if error_message(line).is_some() {
        return Line::Error;
    }
    Line::Other
}

/// The text of an error line (`[cli] ERROR: …`, `[cli] CRITICAL: …`,
/// ` ! Failure: …`), without legendary's prefix.
pub fn error_message(line: &str) -> Option<String> {
    let line = line.trim();
    let text = after(line, "] ERROR: ")
        .or_else(|| after(line, "] CRITICAL: "))
        .or_else(|| line.strip_prefix("! Failure:"))?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Splits raw output on `\n` and `\r` (verification redraws its line).
pub fn lines(chunk: &str) -> impl Iterator<Item = &str> {
    chunk.split(['\n', '\r']).filter(|l| !l.trim().is_empty())
}

// ---------- installed.json / list-installed --json ----------

/// One entry of `installed.json` (the same objects `list-installed --json` prints).
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Installed {
    pub app_name: String,
    pub title: String,
    pub install_path: String,
    pub version: String,
    pub is_dlc: bool,
    pub install_size: u64,
    pub platform: String,
    pub needs_verification: bool,
}

impl Default for Installed {
    fn default() -> Self {
        Installed {
            app_name: String::new(),
            title: String::new(),
            install_path: String::new(),
            version: String::new(),
            is_dlc: false,
            install_size: 0,
            platform: "Windows".into(),
            needs_verification: false,
        }
    }
}

/// `installed.json` is a map by app name, `list-installed --json` a list:
/// both are accepted. DLCs are left out (they live inside their game).
pub fn parse_installed(src: &str) -> Vec<Installed> {
    let value: serde_json::Value = serde_json::from_str(src).unwrap_or_default();
    let items: Vec<serde_json::Value> = match value {
        serde_json::Value::Object(map) => map.into_iter().map(|(_, v)| v).collect(),
        serde_json::Value::Array(list) => list,
        _ => Vec::new(),
    };
    let mut games: Vec<Installed> = items
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Installed>(v).ok())
        .filter(|g| !g.is_dlc && !g.app_name.is_empty() && !g.install_path.is_empty())
        .collect();
    games.sort_by(|a, b| a.app_name.cmp(&b.app_name));
    games
}

// ---------- assets.json ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Asset {
    app_name: String,
    build_version: String,
}

/// `assets.json`: platform → the latest build of every owned app.
/// Returns `(platform, app name) → build version`.
pub fn parse_assets(src: &str) -> HashMap<(String, String), String> {
    let parsed: HashMap<String, Vec<Asset>> = serde_json::from_str(src).unwrap_or_default();
    parsed
        .into_iter()
        .flat_map(|(platform, assets)| {
            assets.into_iter().map(move |a| ((platform.clone(), a.app_name), a.build_version))
        })
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

/// App names of installed games whose build is older than the latest one
/// legendary knows of (its asset list is refreshed by `list`/`status`).
pub fn outdated(installed: &[Installed], assets: &HashMap<(String, String), String>) -> Vec<String> {
    installed
        .iter()
        .filter(|g| {
            g.needs_verification
                || assets
                    .get(&(g.platform.clone(), g.app_name.clone()))
                    .is_some_and(|latest| *latest != g.version)
        })
        .map(|g| g.app_name.clone())
        .collect()
}

// ---------- list --json ----------

#[derive(Deserialize, Default)]
#[serde(default)]
struct ListGame {
    app_name: String,
    app_title: String,
    metadata: Metadata,
    asset_infos: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Metadata {
    id: String,
    namespace: String,
    key_images: Vec<KeyImage>,
    custom_attributes: HashMap<String, Attribute>,
    main_game_item: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct KeyImage {
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Attribute {
    value: String,
}

/// An owned game legendary can install.
#[derive(Clone, Debug, PartialEq)]
pub struct Owned {
    pub app_name: String,
    pub title: String,
    pub namespace: String,
    pub catalog_id: String,
    pub cover: Option<String>,
    pub hero: Option<String>,
}

fn image(images: &[KeyImage], kinds: &[&str]) -> Option<String> {
    kinds
        .iter()
        .find_map(|k| images.iter().find(|i| i.kind == *k && !i.url.is_empty()))
        .map(|i| i.url.clone())
}

/// Tall cover and wide hero from Epic's key images.
pub fn cover_and_hero_of(metadata: &serde_json::Value) -> (Option<String>, Option<String>) {
    let images: Vec<KeyImage> = metadata
        .get("keyImages")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    (cover(&images), hero(&images))
}

fn cover(images: &[KeyImage]) -> Option<String> {
    image(images, &["DieselGameBoxTall", "OfferImageTall", "Thumbnail"])
}

fn hero(images: &[KeyImage]) -> Option<String> {
    image(images, &["DieselGameBox", "OfferImageWide", "DieselStoreFrontWide"])
}

/// Games of `legendary list --json` that install on Windows: no DLC, no
/// titles another store (EA app, Ubisoft Connect) has to install.
pub fn parse_list(src: &str) -> Vec<Owned> {
    let games: Vec<ListGame> = serde_json::from_str(src).unwrap_or_default();
    games
        .into_iter()
        .filter(|g| !g.app_name.is_empty() && g.asset_infos.contains_key("Windows"))
        .filter(|g| g.metadata.main_game_item.is_none())
        .filter(|g| {
            let attrs = &g.metadata.custom_attributes;
            !["ThirdPartyManagedApp", "ThirdPartyManagedProvider"]
                .iter()
                .any(|k| attrs.get(*k).is_some_and(|a| !a.value.trim().is_empty()))
        })
        .map(|g| Owned {
            cover: cover(&g.metadata.key_images),
            hero: hero(&g.metadata.key_images),
            title: g.app_title.trim().to_string(),
            app_name: g.app_name,
            namespace: g.metadata.namespace,
            catalog_id: g.metadata.id,
        })
        .collect()
}

// ---------- user.json ----------

/// The signed-in account's display name, while its refresh token is valid.
/// `now_iso` is the current UTC time as `YYYY-MM-DDTHH:MM:SS`.
pub fn account(user_json: &str, now_iso: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(user_json).ok()?;
    let name = value.get("displayName")?.as_str()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    // ISO 8601 timestamps in UTC compare as strings.
    let expires = value.get("refresh_expires_at").and_then(|v| v.as_str()).unwrap_or("");
    if !expires.is_empty() && expires.get(..19).unwrap_or(expires) <= now_iso {
        return None;
    }
    Some(name)
}

/// The Epic Games Launcher's `GameUserSettings.ini` holds a remembered
/// sign-in (what `legendary auth --import` takes over).
pub fn egl_remembers_login(ini: &str) -> bool {
    let mut in_section = false;
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[RememberMe]");
        } else if in_section && line.strip_prefix("Data=").is_some_and(|v| !v.trim().is_empty()) {
            return true;
        }
    }
    false
}

// ---------- login ----------

/// Where the login page's script sends the authorization code, so the
/// navigation handler can catch it (the address never resolves).
pub const CATCH_HOST: &str = "gamemode.invalid";
pub const CATCH_PATH: &str = "/epic-login";

/// The authorization code out of a caught navigation.
pub fn code_from_url(url: &tauri::Url) -> Option<String> {
    if url.host_str() != Some(CATCH_HOST) || url.path() != CATCH_PATH {
        return None;
    }
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.trim().to_string())
        .filter(|c| !c.is_empty() && c.chars().all(|c| c.is_ascii_alphanumeric()))
}

// ---------- launch ----------

#[derive(Debug, PartialEq)]
pub enum LaunchResult {
    Started,
    /// The installed build is older than the latest: update first.
    OutOfDate,
    /// Epic couldn't be reached: try again offline.
    NoNetwork,
    Failed(String),
}

/// What `legendary launch` did, from its exit code and output.
pub fn classify_launch(success: bool, output: &str) -> LaunchResult {
    if output.contains("Game is out of date") {
        return LaunchResult::OutOfDate;
    }
    if success && !output.contains("] ERROR: ") && !output.contains("] CRITICAL: ") {
        return LaunchResult::Started;
    }
    let network = ["Login failed", "ConnectionError", "Max retries exceeded", "NameResolutionError", "timed out"];
    if network.iter().any(|n| output.contains(n)) {
        return LaunchResult::NoNetwork;
    }
    let message = output.lines().filter_map(error_message).last();
    LaunchResult::Failed(message.unwrap_or_else(|| "legendary non è riuscito ad avviare il gioco".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_download_progress() {
        let p = parse_line("[DLManager] INFO: = Progress: 12.34% (123/1000), Running for 00:01:02, ETA: 01:05:09");
        assert_eq!(p, Line::Progress { fraction: 0.1234, eta_secs: Some(3909) });
        assert_eq!(parse_line("[DLManager] INFO:  - Downloaded: 512.00 MiB, Written: 600.00 MiB"), Line::Downloaded(512 * 1024 * 1024));
        assert_eq!(
            parse_line("[cli] INFO: Download size: 1024.00 MiB (Compression savings: 41.3%)"),
            Line::DownloadSize(1024 * 1024 * 1024)
        );
        assert_eq!(parse_line("Verification progress: 12/345 (3.5%) [12.3 MiB/s]\t"), Line::Verifying(0.035));
        assert_eq!(parse_line("[DLManager] INFO:  + Download\t- 10.00 MiB/s (raw)"), Line::Other);
        assert_eq!(parse_line("[cli] INFO: Install size: 2048.00 MiB"), Line::Other);
        assert_eq!(parse_line("[cli] CRITICAL: Installation cannot proceed, exiting."), Line::Error);
        // Start of a download: no ETA yet.
        assert_eq!(
            parse_line("= Progress: 0.00% (0/10), Running for 00:00:00, ETA: 00:00:00"),
            Line::Progress { fraction: 0.0, eta_secs: Some(0) }
        );
        assert_eq!(parse_line("= Progress: n/a"), Line::Other);
    }

    #[test]
    fn extracts_error_messages_and_splits_redrawn_lines() {
        assert_eq!(
            error_message("[cli] ERROR: Login failed! Cannot continue with download process.").as_deref(),
            Some("Login failed! Cannot continue with download process.")
        );
        assert_eq!(error_message(" ! Failure: Not enough available disk space!").as_deref(), Some("Not enough available disk space!"));
        assert_eq!(error_message("[cli] INFO: fine"), None);
        let chunk = "Verification progress: 1/4 (25.0%) [1.0 MiB/s]\t\rVerification progress: 2/4 (50.0%) [1.0 MiB/s]\t\r\n";
        assert_eq!(lines(chunk).count(), 2);
    }

    #[test]
    fn reads_installed_games_from_the_file_or_the_cli() {
        let file = r#"{
          "Fortnite": {"app_name":"Fortnite","title":"Fortnite","install_path":"C:\\Games\\Fortnite","version":"++Fortnite+Release-30.00","install_size":42949672960,"platform":"Windows","is_dlc":false},
          "Pack": {"app_name":"Pack","title":"DLC","install_path":"C:\\Games\\Fortnite","version":"1","is_dlc":true}
        }"#;
        let games = parse_installed(file);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].install_size, 42_949_672_960);
        let cli = r#"[{"app_name":"Fortnite","title":"Fortnite","install_path":"C:\\Games\\Fortnite","version":"1"}]"#;
        assert_eq!(parse_installed(cli)[0].platform, "Windows");
        assert!(parse_installed("not json").is_empty());
    }

    #[test]
    fn finds_outdated_games() {
        let installed = parse_installed(
            r#"[{"app_name":"A","title":"A","install_path":"C:\\A","version":"1"},
                {"app_name":"B","title":"B","install_path":"C:\\B","version":"7"},
                {"app_name":"C","title":"C","install_path":"C:\\C","version":"1","needs_verification":true}]"#,
        );
        let assets = parse_assets(
            r#"{"Windows":[{"app_name":"A","build_version":"2"},{"app_name":"B","build_version":"7"}],
                "Mac":[{"app_name":"B","build_version":"9"}]}"#,
        );
        assert_eq!(outdated(&installed, &assets), ["A", "C"]);
    }

    #[test]
    fn lists_installable_owned_games() {
        let src = r#"[
          {"app_name":"Min","app_title":" Hades ","asset_infos":{"Windows":{"build_version":"1"}},
           "metadata":{"id":"c1","namespace":"ns1","keyImages":[{"type":"DieselGameBoxTall","url":"tall.jpg"},{"type":"DieselGameBox","url":"wide.jpg"}]}},
          {"app_name":"MacOnly","app_title":"Mac","asset_infos":{"Mac":{}},"metadata":{}},
          {"app_name":"Ea","app_title":"EA game","asset_infos":{"Windows":{}},
           "metadata":{"customAttributes":{"ThirdPartyManagedApp":{"type":"STRING","value":"the EA app"}}}},
          {"app_name":"Dlc","app_title":"DLC","asset_infos":{"Windows":{}},"metadata":{"mainGameItem":{"id":"x"}}}
        ]"#;
        let owned = parse_list(src);
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].title, "Hades");
        assert_eq!((owned[0].namespace.as_str(), owned[0].catalog_id.as_str()), ("ns1", "c1"));
        assert_eq!(owned[0].cover.as_deref(), Some("tall.jpg"));
        assert_eq!(owned[0].hero.as_deref(), Some("wide.jpg"));
    }

    #[test]
    fn reads_the_signed_in_account() {
        let user = r#"{"displayName":"Max","refresh_expires_at":"2026-11-20T10:00:00.000Z","access_token":"secret"}"#;
        assert_eq!(account(user, "2026-10-23T12:00:00").as_deref(), Some("Max"));
        assert_eq!(account(user, "2026-11-21T00:00:00"), None, "expired refresh token");
        assert_eq!(account("{}", "2026-10-23T12:00:00"), None);
    }

    #[test]
    fn sees_a_remembered_launcher_login() {
        assert!(egl_remembers_login("[Other]\nData=x\n[RememberMe]\nEnable=True\nData=abc\n"));
        assert!(!egl_remembers_login("[RememberMe]\nEnable=False\nData=\n[Other]\nData=x"));
        assert!(!egl_remembers_login(""));
    }

    #[test]
    fn catches_the_authorization_code() {
        let url = tauri::Url::parse("https://gamemode.invalid/epic-login?code=abc123DEF").unwrap();
        assert_eq!(code_from_url(&url).as_deref(), Some("abc123DEF"));
        let other = tauri::Url::parse("https://www.epicgames.com/epic-login?code=abc").unwrap();
        assert_eq!(code_from_url(&other), None);
        let junk = tauri::Url::parse("https://gamemode.invalid/epic-login?code=a%20b").unwrap();
        assert_eq!(code_from_url(&junk), None);
    }

    #[test]
    fn classifies_launch_results() {
        assert_eq!(classify_launch(true, "[cli] INFO: Launching Fortnite..."), LaunchResult::Started);
        assert_eq!(
            classify_launch(false, "[cli] ERROR: Game is out of date, please update or launch with update check skipping!"),
            LaunchResult::OutOfDate
        );
        assert_eq!(classify_launch(false, "[cli] ERROR: Login failed, cannot continue!"), LaunchResult::NoNetwork);
        assert_eq!(
            classify_launch(false, "[cli] CRITICAL: Install directory \"C:\\x\" appears to be deleted, cannot launch X!"),
            LaunchResult::Failed("Install directory \"C:\\x\" appears to be deleted, cannot launch X!".into())
        );
    }
}
