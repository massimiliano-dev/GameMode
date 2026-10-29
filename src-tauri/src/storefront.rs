//! Opening the stores' own shops. Purchases always happen inside the official
//! client (or website): GameMode never touches accounts or payments.

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::model::{Game, Store};
use crate::window;

struct Shop {
    store: Store,
    /// URI scheme of the store client, used to tell whether it is installed.
    scheme: Option<&'static str>,
    /// Shop front inside the client, when the client has a URI for it.
    client_home: Option<&'static str>,
    web_home: &'static str,
}

const SHOPS: &[Shop] = &[
    Shop {
        store: Store::Steam,
        scheme: Some("steam"),
        client_home: Some("steam://store"),
        web_home: "https://store.steampowered.com/",
    },
    Shop {
        store: Store::Xbox,
        scheme: Some("ms-windows-store"),
        client_home: Some("ms-windows-store://home"),
        web_home: "https://www.xbox.com/it-IT/games/all-games/pc",
    },
    Shop {
        store: Store::Epic,
        scheme: Some("com.epicgames.launcher"),
        client_home: Some("com.epicgames.launcher://store"),
        web_home: "https://store.epicgames.com/it/",
    },
    Shop {
        store: Store::Gog,
        scheme: Some("goggalaxy"),
        client_home: None,
        web_home: "https://www.gog.com/it/games",
    },
    Shop {
        store: Store::Ubisoft,
        scheme: Some("uplay"),
        client_home: None,
        web_home: "https://store.ubisoft.com/it/home",
    },
    Shop {
        store: Store::Ea,
        scheme: Some("origin2"),
        client_home: None,
        web_home: "https://www.ea.com/it-it/games",
    },
];

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum OpensIn {
    Client,
    Browser,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Storefront {
    pub store: Store,
    pub opens_in: OpensIn,
    pub client_installed: bool,
}

pub fn list() -> Vec<Storefront> {
    SHOPS
        .iter()
        .map(|shop| {
            let installed = client_installed(shop);
            let opens_in = if installed && shop.client_home.is_some() { OpensIn::Client } else { OpensIn::Browser };
            Storefront { store: shop.store, opens_in, client_installed: installed }
        })
        .collect()
}

/// Opens the shop for `store`, or the store page of `game` when the client
/// supports deep links, then steps aside so the shop gets the screen.
pub fn open(app: &AppHandle, store: Store, game: Option<&Game>) -> Result<OpensIn> {
    let Some(shop) = SHOPS.iter().find(|s| s.store == store) else {
        anyhow::bail!("Nessun negozio per questo store");
    };
    let (target, opens_in) = resolve(shop, game, client_installed(shop));
    open_uri(app, &target)?;
    Ok(opens_in)
}

fn resolve(shop: &Shop, game: Option<&Game>, installed: bool) -> (String, OpensIn) {
    if installed {
        if let Some(uri) = game.and_then(|g| game_uri(shop.store, g)) {
            return (uri, OpensIn::Client);
        }
        if let Some(home) = shop.client_home {
            return (home.to_string(), OpensIn::Client);
        }
    }
    let web = game.and_then(|g| game_web(shop.store, g)).unwrap_or_else(|| shop.web_home.to_string());
    (web, OpensIn::Browser)
}

/// Native id of a game (the part after `store:`).
fn native_id(game: &Game) -> &str {
    game.id.split_once(':').map_or(game.id.as_str(), |(_, id)| id)
}

fn game_uri(store: Store, game: &Game) -> Option<String> {
    match store {
        Store::Steam => game.steam_appid.map(|id| format!("steam://store/{id}")),
        Store::Xbox => Some(format!("ms-windows-store://pdp/?PFN={}", native_id(game))),
        Store::Gog => Some(format!("goggalaxy://openGameView/{}", native_id(game))),
        _ => None,
    }
}

fn game_web(store: Store, game: &Game) -> Option<String> {
    match store {
        Store::Steam => game.steam_appid.map(|id| format!("https://store.steampowered.com/app/{id}/")),
        _ => None,
    }
}

fn client_installed(shop: &Shop) -> bool {
    shop.scheme.is_some_and(scheme_registered)
}

/// Whether some installed app handles `scheme://` links: classic apps
/// register in the registry, packaged ones (Xbox, Microsoft Store) only show
/// up through the WinRT launcher. Blocking: call off the main thread.
#[cfg(windows)]
pub fn scheme_registered(scheme: &str) -> bool {
    use winreg::enums::HKEY_CLASSES_ROOT;
    let classic = winreg::RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(scheme)
        .and_then(|k| k.get_value::<String, _>("URL Protocol"))
        .is_ok();
    classic || {
        use windows::core::HSTRING;
        use windows::Foundation::Uri;
        use windows::System::{LaunchQuerySupportStatus, LaunchQuerySupportType, Launcher};
        Uri::CreateUri(&HSTRING::from(format!("{scheme}://")))
            .and_then(|u| Launcher::QueryUriSupportAsync(&u, LaunchQuerySupportType::Uri)?.join())
            .is_ok_and(|s| s == LaunchQuerySupportStatus::Available)
    }
}

#[cfg(not(windows))]
pub fn scheme_registered(scheme: &str) -> bool {
    // Only Steam has a macOS/Linux client worth deep-linking into.
    scheme == "steam" && crate::library::steam::find_steam_root().is_some()
}

/// Opens `uri` through Windows' own launcher, which also reaches packaged
/// apps (Xbox app, Microsoft Store) that the classic registry check misses.
/// Asks first, so Windows never pops "you need an app to open this link".
/// Blocking: call off the main thread.
#[cfg(windows)]
pub fn launch_uri(uri: &str) -> bool {
    use windows::core::HSTRING;
    use windows::Foundation::Uri;
    use windows::System::{LaunchQuerySupportStatus, LaunchQuerySupportType, Launcher};
    let Ok(target) = Uri::CreateUri(&HSTRING::from(uri)) else { return false };
    let supported = Launcher::QueryUriSupportAsync(&target, LaunchQuerySupportType::Uri)
        .and_then(|op| op.join())
        .is_ok_and(|s| s == LaunchQuerySupportStatus::Available);
    supported && Launcher::LaunchUriAsync(&target).and_then(|op| op.join()).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn launch_uri(_uri: &str) -> bool {
    false
}

/// Opens the first link some installed app accepts; the last one (a web
/// page) always opens in the browser.
pub fn open_first(app: &AppHandle, uris: &[String]) -> Result<()> {
    let Some((last, apps)) = uris.split_last() else { anyhow::bail!("nessun link") };
    mark_hand_off();
    let before = handles(&window::other_windows());
    for uri in apps {
        crate::shell::prepare_for(uri);
        if launch_uri(uri) {
            window::minimize(app);
            hand_over(app, before);
            return Ok(());
        }
    }
    open_uri(app, last)
}

/// When GameMode last opened a store window on purpose.
static HANDED_OFF: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn mark_hand_off() {
    *HANDED_OFF.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
}

/// GameMode opened a store window on purpose after `since` (the user went
/// to a shop or Steam's downloads during a game): that window is wanted.
pub fn handed_off_after(since: std::time::Instant) -> bool {
    HANDED_OFF.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|at| at > since)
}

/// Big Picture was opened for the current hand-off and should close on return.
static STEAM_BIG_PICTURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How long Steam may take to get to Big Picture (it may be starting, or
/// signing in) before the link goes anyway.
const BIG_PICTURE_WAIT: std::time::Duration = std::time::Duration::from_secs(25);
/// Once its window is up, Big Picture needs a moment before it takes links:
/// earlier, Steam opens them in its desktop window.
const BIG_PICTURE_SETTLE: std::time::Duration = std::time::Duration::from_millis(900);
const BIG_PICTURE_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Big Picture's window is on screen ("Steam Big Picture Mode", also in
/// the other languages Steam names it in).
fn big_picture_up() -> bool {
    window::titles_of(&["steam.exe", "steamwebhelper.exe"]).iter().any(|t| t.to_lowercase().contains("big picture"))
}

/// Opens Big Picture and waits until it is really there: a fixed pause was
/// too short whenever Steam was starting or busy, and the link then opened
/// in Steam's desktop window. Blocking.
fn ensure_big_picture(app: &AppHandle) -> Result<()> {
    if big_picture_up() {
        return Ok(());
    }
    app.opener().open_url("steam://open/bigpicture", None::<&str>)?;
    let start = std::time::Instant::now();
    while start.elapsed() < BIG_PICTURE_WAIT {
        std::thread::sleep(BIG_PICTURE_POLL);
        if big_picture_up() {
            std::thread::sleep(BIG_PICTURE_SETTLE);
            return Ok(());
        }
    }
    log::warn!("Big Picture not up after {} s: opening the link anyway", BIG_PICTURE_WAIT.as_secs());
    Ok(())
}

pub fn uses_big_picture(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().db.lock().settings.steam_big_picture
}

/// Opens a store client link and steps aside so its window gets the screen.
/// Steam links go through Big Picture, which the controller can drive.
/// Blocking (waits for Big Picture): call off the main thread.
pub fn open_uri(app: &AppHandle, uri: &str) -> Result<()> {
    mark_hand_off();
    let before = handles(&window::other_windows());
    if uri.starts_with("steam://") && uses_big_picture(app) {
        // Already up: it was the user's own, don't close it on return.
        if !big_picture_up() {
            STEAM_BIG_PICTURE.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        ensure_big_picture(app)?;
    }
    crate::shell::prepare_for(uri);
    app.opener().open_url(uri, None::<&str>)?;
    window::minimize(app);
    hand_over(app, before);
    Ok(())
}

/// Browsers the web shops open in, besides the store clients (`LAUNCHERS`).
const BROWSERS: &[&str] =
    &["msedge.exe", "chrome.exe", "firefox.exe", "brave.exe", "opera.exe", "opera_gx.exe", "vivaldi.exe", "librewolf.exe"];
/// Packaged apps some links open (Microsoft Store, Windows settings).
const PACKAGED: &[&str] = &["winstore.app.exe", "systemsettings.exe"];
/// How long to wait for the store's window to show up.
const HAND_OVER_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const HAND_OVER_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// Attempts at putting the window in front once it's there.
const HAND_OVER_TRIES: u32 = 6;

fn handles(windows: &[(isize, Option<String>)]) -> Vec<isize> {
    windows.iter().map(|(h, _)| *h).collect()
}

fn is_shop_process(name: &str) -> bool {
    crate::launcher::LAUNCHERS.contains(&name) || BROWSERS.contains(&name) || PACKAGED.contains(&name)
}

/// The window a shop link opened, among the visible windows of other
/// processes (`now`) given those up `before` the link: a new window of a
/// store client or browser first, then any other new window (an unknown
/// browser), then a client's window that was already up behind GameMode.
/// Windows' own desktop and pushy background tools never count.
fn shop_window(before: &[isize], now: &[(isize, Option<String>)]) -> Option<isize> {
    let known = |(_, name): &(isize, Option<String>)| name.as_deref().is_some_and(is_shop_process);
    let fresh = |(h, name): &(isize, Option<String>)| {
        !before.contains(h) && !name.as_deref().is_some_and(|n| n == "explorer.exe" || window::is_pushy_tool(n))
    };
    now.iter()
        .find(|w| fresh(w) && known(w))
        .or_else(|| now.iter().find(|w| fresh(w)))
        .or_else(|| now.iter().find(|w| known(w)))
        .map(|(h, _)| *h)
}

/// Puts the window a shop link opened in front of GameMode. In console mode
/// GameMode stays fullscreen (no desktop to step aside to) and a client
/// that was already running (Steam, Galaxy, the browser…) has no right to
/// take the foreground from it: left alone, its window would come up behind
/// GameMode and nothing would seem to happen.
fn hand_over(app: &AppHandle, before: Vec<isize>) {
    let app = app.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        let mut tries = 0;
        while start.elapsed() < HAND_OVER_WAIT {
            std::thread::sleep(HAND_OVER_POLL);
            // The user came back (or never left the overlay): nothing to hand over.
            if crate::overlay::is_open() || (tries == 0 && !window::is_foreground(&app)) {
                return;
            }
            let Some(hwnd) = shop_window(&before, &window::other_windows()) else { continue };
            if window::is_hwnd_foreground(hwnd) || tries >= HAND_OVER_TRIES {
                return;
            }
            tries += 1;
            window::force_foreground_raw(hwnd);
        }
    });
}

/// Closes Big Picture if GameMode opened it for a hand-off (a game's
/// downloads, an install). Returns whether it did. Big Picture left open
/// takes the screen back from the game that starts afterwards.
pub fn close_our_big_picture(app: &AppHandle) -> bool {
    if !STEAM_BIG_PICTURE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        return false;
    }
    let _ = app.opener().open_url("steam://close/bigpicture", None::<&str>);
    true
}

/// Comes back from a hand-off: closes Big Picture if GameMode opened it (so
/// it doesn't sit over later games) and brings GameMode to the front.
pub fn return_to_gamemode(app: &AppHandle) {
    crate::assist::disable();
    if STEAM_BIG_PICTURE.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = app.opener().open_url("steam://close/bigpicture", None::<&str>);
        let app = app.clone();
        // Leaving Big Picture brings the Steam window up: wait for it.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(900));
            window::bring_to_front(&app);
        });
        return;
    }
    window::bring_to_front(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchSpec;

    fn shop(store: Store) -> &'static Shop {
        SHOPS.iter().find(|s| s.store == store).unwrap()
    }

    fn game(store: Store, id: &str, appid: Option<u32>) -> Game {
        let mut g = Game::new(store, id, "Gioco", LaunchSpec::Uri { uri: String::new() });
        g.steam_appid = appid;
        g
    }

    #[test]
    fn deep_links_into_installed_clients() {
        let hades = game(Store::Steam, "1145360", Some(1145360));
        assert_eq!(resolve(shop(Store::Steam), Some(&hades), true), ("steam://store/1145360".into(), OpensIn::Client));
        let forza = game(Store::Xbox, "Microsoft.624F8B84B80_8wekyb3d8bbwe", None);
        assert_eq!(
            resolve(shop(Store::Xbox), Some(&forza), true).0,
            "ms-windows-store://pdp/?PFN=Microsoft.624F8B84B80_8wekyb3d8bbwe"
        );
    }

    #[test]
    fn picks_the_window_the_shop_opened() {
        let w = |h: isize, name: &str| (h, Some(name.to_string()));
        let before = [1, 2];
        // A new browser window beats an unknown new window and an old client window.
        let now = [w(1, "rtss.exe"), w(2, "steam.exe"), w(3, "someapp.exe"), w(4, "msedge.exe")];
        assert_eq!(shop_window(&before, &now), Some(4));
        // Unknown new window (another browser): still the one to hand over to.
        assert_eq!(shop_window(&before, &[w(2, "steam.exe"), w(3, "zen.exe")]), Some(3));
        // Nothing new: the client was already up behind GameMode.
        assert_eq!(shop_window(&before, &[w(1, "rtss.exe"), w(2, "steam.exe")]), Some(2));
        // The desktop and pushy tools are never the shop.
        assert_eq!(shop_window(&before, &[w(5, "explorer.exe"), w(6, "rtss.exe")]), None);
        assert_eq!(shop_window(&before, &[]), None);
    }

    #[test]
    fn falls_back_to_the_web_shop() {
        let hades = game(Store::Steam, "1145360", Some(1145360));
        assert_eq!(
            resolve(shop(Store::Steam), Some(&hades), false),
            ("https://store.steampowered.com/app/1145360/".into(), OpensIn::Browser)
        );
        // Ubisoft's client has no shop URI: always the website.
        assert_eq!(resolve(shop(Store::Ubisoft), None, true).1, OpensIn::Browser);
        assert_eq!(resolve(shop(Store::Epic), None, true), ("com.epicgames.launcher://store".into(), OpensIn::Client));
    }
}
