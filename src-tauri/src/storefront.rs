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
    for uri in apps {
        if launch_uri(uri) {
            window::minimize(app);
            return Ok(());
        }
    }
    open_uri(app, last)
}

/// Big Picture was opened for the current hand-off and should close on return.
static STEAM_BIG_PICTURE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Time Steam needs to switch to Big Picture before it takes the next link.
const BIG_PICTURE_DELAY: std::time::Duration = std::time::Duration::from_millis(1800);

pub fn uses_big_picture(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().db.lock().settings.steam_big_picture
}

/// Opens a store client link and steps aside so its window gets the screen.
/// Steam links go through Big Picture, which the controller can drive.
/// Blocking (waits for Big Picture): call off the main thread.
pub fn open_uri(app: &AppHandle, uri: &str) -> Result<()> {
    if uri.starts_with("steam://") && uses_big_picture(app) {
        app.opener().open_url("steam://open/bigpicture", None::<&str>)?;
        STEAM_BIG_PICTURE.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(BIG_PICTURE_DELAY);
    }
    app.opener().open_url(uri, None::<&str>)?;
    window::minimize(app);
    Ok(())
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
