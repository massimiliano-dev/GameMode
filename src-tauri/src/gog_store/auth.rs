//! Signing in to GOG: GOG's own login page in a GameMode window, the code
//! from its redirect traded for tokens. Only the refresh token is kept, in
//! Windows Credential Manager (PasswordVault); the access token lives in
//! memory.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use super::api;

const LOGIN_WINDOW: &str = "gog-login";

struct Session {
    access: String,
    expires: Instant,
}

/// Serializes refreshes: GOG rotates the refresh token on every use.
static SESSION: LazyLock<tokio::sync::Mutex<Option<Session>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

pub fn signed_in() -> bool {
    vault::load().is_some()
}

/// A valid access token, refreshed when needed.
pub async fn access_token(client: &reqwest::Client) -> Result<String> {
    let mut session = SESSION.lock().await;
    if let Some(s) = session.as_ref() {
        if s.expires > Instant::now() + Duration::from_secs(60) {
            return Ok(s.access.clone());
        }
    }
    let Some(refresh_token) = vault::load() else { bail!("Accedi a GOG dalle Impostazioni") };
    let token = match api::refresh(client, &refresh_token).await {
        Ok(token) => token,
        Err(err) => {
            if err.downcast_ref::<api::TokenRejected>().is_some() {
                // Revoked or expired for good: sign in again.
                vault::clear();
                *session = None;
                bail!("L'accesso a GOG è scaduto: accedi di nuovo dalle Impostazioni");
            }
            return Err(err);
        }
    };
    let access = token.access_token.clone();
    *session = Some(remember(token)?);
    Ok(access)
}

fn remember(token: api::Token) -> Result<Session> {
    vault::save(&token.user_id, &token.refresh_token)?;
    Ok(Session { access: token.access_token, expires: Instant::now() + Duration::from_secs(token.expires_in) })
}

pub async fn logout() {
    vault::clear();
    *SESSION.lock().await = None;
}

type Reply = Arc<Mutex<Option<tokio::sync::oneshot::Sender<Option<String>>>>>;

/// First answer wins: the login code, or `None` when the window closed.
fn send(reply: &Reply, code: Option<String>) {
    if let Some(tx) = reply.lock().unwrap().take() {
        let _ = tx.send(code);
    }
}

/// Opens GOG's login page and waits until the user signs in (true) or
/// closes the window (false).
pub async fn login(app: &AppHandle) -> Result<bool> {
    if let Some(old) = app.get_webview_window(LOGIN_WINDOW) {
        let _ = old.destroy();
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let tx: Reply = Arc::new(Mutex::new(Some(tx)));

    let url: tauri::Url = api::login_url().parse()?;
    let (on_nav, on_load) = (tx.clone(), tx.clone());
    let window = WebviewWindowBuilder::new(app, LOGIN_WINDOW, WebviewUrl::External(url))
        .title("Accedi a GOG")
        .inner_size(520.0, 760.0)
        .center()
        .always_on_top(true)
        .focused(true)
        .on_navigation(move |url| match api::code_from_redirect(url.as_str()) {
            Some(code) => {
                send(&on_nav, Some(code));
                false
            }
            None => true,
        })
        // Redirects don't always go through `on_navigation`.
        .on_page_load(move |_, payload| {
            if let Some(code) = api::code_from_redirect(payload.url().as_str()) {
                send(&on_load, Some(code));
            }
        })
        .build()?;
    let on_close = tx.clone();
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            send(&on_close, None);
        }
    });

    let code = rx.await.ok().flatten();
    let _ = window.destroy();
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.set_focus();
    }
    let Some(code) = code else { return Ok(false) };
    let token = api::exchange_code(&api::client(), &code).await?;
    *SESSION.lock().await = Some(remember(token)?);
    Ok(true)
}

#[cfg(windows)]
mod vault {
    use anyhow::Result;
    use windows::core::HSTRING;
    use windows::Security::Credentials::{PasswordCredential, PasswordVault};

    const RESOURCE: &str = "GameMode GOG";

    fn credentials() -> Vec<PasswordCredential> {
        // Throws "element not found" when there is none.
        PasswordVault::new()
            .and_then(|v| v.FindAllByResource(&HSTRING::from(RESOURCE)))
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn load() -> Option<String> {
        let credential = credentials().into_iter().next()?;
        credential.RetrievePassword().ok()?;
        credential.Password().ok().map(|p| p.to_string_lossy()).filter(|p| !p.is_empty())
    }

    pub fn save(user: &str, refresh_token: &str) -> Result<()> {
        clear();
        let user = if user.is_empty() { "gog" } else { user };
        let credential =
            PasswordCredential::CreatePasswordCredential(&HSTRING::from(RESOURCE), &HSTRING::from(user), &HSTRING::from(refresh_token))?;
        PasswordVault::new()?.Add(&credential)?;
        Ok(())
    }

    pub fn clear() {
        let Ok(vault) = PasswordVault::new() else { return };
        for credential in credentials() {
            let _ = vault.Remove(&credential);
        }
    }
}

/// Elsewhere (development on macOS) the token is only kept for the run.
#[cfg(not(windows))]
mod vault {
    use std::sync::Mutex;

    use anyhow::Result;

    static TOKEN: Mutex<Option<String>> = Mutex::new(None);

    pub fn load() -> Option<String> {
        TOKEN.lock().unwrap().clone()
    }

    pub fn save(_user: &str, refresh_token: &str) -> Result<()> {
        *TOKEN.lock().unwrap() = Some(refresh_token.to_string());
        Ok(())
    }

    pub fn clear() {
        *TOKEN.lock().unwrap() = None;
    }
}
