//! Epic sign-in in a GameMode window, as Heroic does: Epic's own login
//! page, then the JSON page with the `authorizationCode` legendary needs
//! (`legendary auth --code`). A script on that page hands the code over by
//! navigating to an address the navigation handler catches. The window is
//! a controller hand-off: the controller types keys and the on-screen
//! keyboard opens on text fields.

use std::time::Duration;

use anyhow::Result;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use super::parse;

pub const LABEL: &str = "epic-login";

/// What https://legendary.gl/epiclogin redirects to: Epic's login, coming
/// back to the redirect API that prints the authorization code.
const LOGIN_URL: &str = "https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a%26responseType%3Dcode";

/// Runs on every page; acts only on the redirect API's JSON.
const SCRIPT: &str = r#"
(function () {
  if (location.hostname !== "www.epicgames.com" || location.pathname.indexOf("/id/api/redirect") !== 0) return;
  function grab() {
    var text = (document.body && document.body.innerText) || document.documentElement.textContent || "";
    var m = /"authorizationCode"\s*:\s*"([A-Za-z0-9]+)"/.exec(text);
    if (m) location.replace("https://gamemode.invalid/epic-login?code=" + encodeURIComponent(m[1]));
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", grab);
  else grab();
})();
"#;

pub fn open(app: &AppHandle) -> Result<()> {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.set_focus();
        return Ok(());
    }
    let handle = app.clone();
    let fullscreen = crate::window::main_window(app).and_then(|w| w.is_fullscreen().ok()).unwrap_or(false);
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(LOGIN_URL.parse()?))
        .title("Accedi a Epic Games")
        // Nothing of the Epic session stays on disk: legendary keeps the tokens.
        .incognito(true)
        .initialization_script(SCRIPT)
        .on_navigation(move |url| match parse::code_from_url(url) {
            Some(code) => {
                let app = handle.clone();
                std::thread::spawn(move || finish(&app, &code));
                false
            }
            None => true,
        })
        .inner_size(1100.0, 820.0)
        .center()
        .fullscreen(fullscreen)
        .focused(true)
        .build()?;
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        crate::window::force_foreground_raw(hwnd.0 as isize);
    }
    #[cfg(not(windows))]
    let _ = window;
    // A web page doesn't understand a controller: it types keys (if allowed).
    let keys = app.state::<crate::AppState>().db.lock().settings.keyboard_assist;
    crate::assist::enable(keys);
    watch(app.clone());
    Ok(())
}

fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.close();
    }
}

fn finish(app: &AppHandle, code: &str) {
    close(app);
    crate::storefront::return_to_gamemode(app);
    let result = super::auth_with_code(code);
    super::after_sign_in(app);
    let _ = app.emit("legendary://login", result.map_err(|e| format!("{e:#}")));
}

/// Back in GameMode without signing in (View held, Alt+Tab): the login
/// window goes away instead of lingering behind.
fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut back_for = 0;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            if app.get_webview_window(LABEL).is_none() {
                return;
            }
            back_for = if crate::window::is_foreground(&app) && !crate::assist::is_active() { back_for + 1 } else { 0 };
            if back_for >= 2 {
                close(&app);
                super::changed(&app);
                return;
            }
        }
    });
}
