//! Phone companion: a small HTTP + WebSocket server on the LAN that serves
//! a one-page remote to the phone's browser (d-pad, keyboard, library,
//! power). Plain HTTP, reachable only on the local network: every request
//! carries a token (from the QR code, or obtained once with the 6-digit
//! pairing code shown in Settings) and the Host header must be an IP
//! literal, so a web page open on the PC can't reach it by DNS rebinding.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, EventId, Listener, Manager};
use tokio::sync::{broadcast, oneshot};

use crate::launcher::SessionState;
use crate::model::{Game, Store};
use crate::{assist, catalog, gamepad, launcher, overlay, system, window, AppState};

pub const PORT: u16 = 7373;
const PAGE: &str = include_str!("../companion/index.html");
/// Backend events forwarded to the phone over the WebSocket.
const FORWARDED: &[&str] = &["game://state", "library://updated", "library://game", "install://queued"];
/// Wrong pairing codes tolerated before a new one is drawn.
const MAX_PAIR_FAILURES: u32 = 5;
/// Sent on the event channel to close every WebSocket when the server stops.
const BYE: &str = "{\"event\":\"bye\"}";
#[cfg(windows)]
const FIREWALL_RULE: &str = "GameMode Companion";

struct Ctx {
    app: AppHandle,
    token: String,
    events: broadcast::Sender<String>,
    pin: Mutex<Pin>,
    listening: AtomicBool,
    error: Mutex<Option<String>>,
}

struct Pin {
    code: String,
    failures: u32,
}

struct Server {
    ctx: Arc<Ctx>,
    stop: Option<oneshot::Sender<()>>,
    listeners: Vec<EventId>,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Firewall {
    Ok,
    Missing,
    Unknown,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub running: bool,
    /// `http://<lan ip>:<port>`, to type by hand.
    pub address: Option<String>,
    /// The same with the token, what the QR code encodes.
    pub url: Option<String>,
    pub qr_svg: Option<String>,
    pub pin: Option<String>,
    pub firewall: Firewall,
    pub error: Option<String>,
}

// ---- lifecycle ------------------------------------------------------------

/// Starts or stops the server to match the setting. Idempotent.
pub fn sync(app: &AppHandle) {
    let enabled = app.state::<AppState>().db.lock().settings.companion_enabled;
    let mut server = SERVER.lock().unwrap();
    match (enabled, server.is_some()) {
        (true, false) => *server = Some(start(app)),
        (false, true) => {
            if let Some(s) = server.take() {
                s.shutdown(app);
            }
        }
        _ => {}
    }
    drop(server);
    let _ = app.emit("companion://changed", ());
}

fn start(app: &AppHandle) -> Server {
    let data_dir = app.state::<AppState>().data_dir.clone();
    let (events, _) = broadcast::channel(64);
    let ctx = Arc::new(Ctx {
        app: app.clone(),
        token: load_token(&data_dir),
        events,
        pin: Mutex::new(Pin { code: new_pin(), failures: 0 }),
        listening: AtomicBool::new(false),
        error: Mutex::new(None),
    });
    let listeners = FORWARDED
        .iter()
        .map(|name| {
            let tx = ctx.events.clone();
            let name = name.to_string();
            app.listen_any(name.clone(), move |event| {
                let payload = serde_json::from_str::<serde_json::Value>(event.payload()).unwrap_or(serde_json::Value::Null);
                let _ = tx.send(serde_json::json!({ "event": name, "payload": payload }).to_string());
            })
        })
        .collect();
    let (stop_tx, stop_rx) = oneshot::channel();
    tauri::async_runtime::spawn(serve(ctx.clone(), stop_rx));
    Server { ctx, stop: Some(stop_tx), listeners }
}

impl Server {
    fn shutdown(mut self, app: &AppHandle) {
        for id in self.listeners.drain(..) {
            app.unlisten(id);
        }
        let _ = self.ctx.events.send(BYE.to_string());
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

async fn serve(ctx: Arc<Ctx>, stop: oneshot::Receiver<()>) {
    let listener = match tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], PORT))).await {
        Ok(l) => l,
        Err(e) => {
            *ctx.error.lock().unwrap() = Some(format!("Porta {PORT} non disponibile: {e}"));
            let _ = ctx.app.emit("companion://changed", ());
            return;
        }
    };
    ctx.listening.store(true, Ordering::SeqCst);
    let _ = ctx.app.emit("companion://changed", ());
    let result = axum::serve(listener, router(ctx.clone()))
        .with_graceful_shutdown(async {
            let _ = stop.await;
        })
        .await;
    ctx.listening.store(false, Ordering::SeqCst);
    if let Err(e) = result {
        *ctx.error.lock().unwrap() = Some(e.to_string());
    }
}

fn router(ctx: Arc<Ctx>) -> Router {
    let api = Router::new()
        .route("/library", get(library))
        .route("/catalog", get(catalog_items))
        .route("/session", get(session))
        .route("/art/{id}/{kind}", get(art))
        .route("/launch", post(launch))
        .route("/install", post(install))
        .route("/nav", post(nav))
        .route("/text", post(text))
        .route("/power", post(power))
        .route("/game", post(game))
        .route("/events", get(events))
        .layer(middleware::from_fn_with_state(ctx.clone(), auth));
    Router::new()
        .route("/", get(page))
        .route("/pair", post(pair))
        .merge(api)
        .layer(middleware::from_fn(host_check))
        .with_state(ctx)
}

// ---- status ---------------------------------------------------------------

/// What Settings shows: address, QR code, pairing code and firewall state.
/// Blocking (runs netsh on Windows): call it off the main thread.
pub fn status(app: &AppHandle) -> Status {
    let enabled = app.state::<AppState>().db.lock().settings.companion_enabled;
    // Only the context under the lock: netsh/PowerShell and DNS below are
    // slow, and `sync` takes the same lock from `save_settings`.
    let ctx = SERVER.lock().unwrap().as_ref().map(|s| s.ctx.clone());
    let Some(ctx) = ctx else {
        return Status {
            enabled,
            running: false,
            address: None,
            url: None,
            qr_svg: None,
            pin: None,
            firewall: firewall_state(),
            error: None,
        };
    };
    let address = lan_ip().map(|ip| format!("http://{ip}:{PORT}"));
    let url = address.as_ref().map(|a| format!("{a}/?t={}", ctx.token));
    Status {
        enabled,
        running: ctx.listening.load(Ordering::SeqCst),
        qr_svg: url.as_deref().and_then(qr_svg),
        address,
        url,
        pin: Some(ctx.pin.lock().unwrap().code.clone()),
        firewall: firewall_state(),
        error: ctx.error.lock().unwrap().clone(),
    }
}

/// The LAN address: the source a UDP socket would use towards the internet
/// (nothing is sent). Falls back to the host name's addresses.
pub fn lan_ip() -> Option<IpAddr> {
    let probe = || -> Option<IpAddr> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("1.1.1.1:53").ok()?;
        Some(socket.local_addr().ok()?.ip())
    };
    if let Some(ip) = probe().filter(|ip| !ip.is_unspecified() && !ip.is_loopback()) {
        return Some(ip);
    }
    use std::net::ToSocketAddrs;
    let host = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok()?;
    (host.as_str(), 0u16)
        .to_socket_addrs()
        .ok()?
        .map(|a| a.ip())
        .find(|ip| matches!(ip, IpAddr::V4(v4) if v4.is_private()))
}

fn qr_svg(url: &str) -> Option<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(url.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(240, 240)
            .quiet_zone(true)
            .dark_color(svg::Color("#07090d"))
            .light_color(svg::Color("#ffffff"))
            .build(),
    )
}

// ---- secrets --------------------------------------------------------------

fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    if getrandom::fill(&mut bytes).is_err() {
        // Never a fixed token: mix the clock and the hasher's random seed.
        use std::hash::{BuildHasher, Hasher};
        let seed = std::collections::hash_map::RandomState::new();
        for (i, b) in bytes.iter_mut().enumerate() {
            let mut h = seed.build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            h.write_usize(i);
            *b = h.finish() as u8;
        }
    }
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_pin() -> String {
    let b = random_bytes(4);
    format!("{:06}", u32::from_le_bytes([b[0], b[1], b[2], b[3]]) % 1_000_000)
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    token: String,
}

/// The phone's token, created once and kept next to the library file so
/// paired phones survive restarts.
fn load_token(dir: &Path) -> String {
    let path = dir.join("companion.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(stored) = serde_json::from_str::<Stored>(&text) {
            if stored.token.len() >= 32 {
                return stored.token;
            }
        }
    }
    let token = hex(&random_bytes(16));
    let _ = std::fs::write(&path, serde_json::to_string(&Stored { token: token.clone() }).unwrap_or_default());
    token
}

/// Constant-time string comparison.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---- middleware -----------------------------------------------------------

/// Rejects requests whose Host is a DNS name: a page on the PC could reach
/// the server by DNS rebinding, but only through a name, never an IP.
async fn host_check(req: Request, next: Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    if host_allowed(host) { next.run(req).await } else { StatusCode::FORBIDDEN.into_response() }
}

pub fn host_allowed(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    name == "localhost" || name.parse::<IpAddr>().is_ok()
}

/// The token comes as an `X-Token` header (fetch) or `?t=` (images, WebSocket).
async fn auth(State(ctx): State<Arc<Ctx>>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get("x-token")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| query_param(req.uri().query(), "t"));
    if given.as_deref().is_some_and(|t| same(t, &ctx.token)) {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

// ---- handlers -------------------------------------------------------------

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

/// Trades the 6-digit code shown in Settings for the token.
async fn pair(State(ctx): State<Arc<Ctx>>, Json(body): Json<PairBody>) -> Response {
    let renewed = {
        let mut pin = ctx.pin.lock().unwrap();
        if same(body.code.trim(), &pin.code) {
            pin.failures = 0;
            return Json(serde_json::json!({ "token": ctx.token })).into_response();
        }
        pin.failures += 1;
        if pin.failures >= MAX_PAIR_FAILURES {
            *pin = Pin { code: new_pin(), failures: 0 };
            true
        } else {
            false
        }
    };
    if renewed {
        let _ = ctx.app.emit("companion://changed", ());
    }
    StatusCode::FORBIDDEN.into_response()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhoneGame {
    id: String,
    name: String,
    store: Store,
    cover: Option<String>,
    favorite: bool,
    last_played: Option<i64>,
    playtime_secs: u64,
}

/// How the phone loads a cover: a web URL as is, a cached file through `/art`.
pub fn cover_of(game: &Game) -> Option<String> {
    match &game.artwork.cover {
        Some(path) if path.starts_with("http") => Some(path.clone()),
        Some(_) => Some(format!("/art/{}/cover", game.id)),
        None => game.cover_url.clone(),
    }
}

async fn library(State(ctx): State<Arc<Ctx>>) -> Json<Vec<PhoneGame>> {
    let games = ctx.app.state::<AppState>().db.lock().view();
    Json(
        games
            .iter()
            .filter(|g| !g.hidden)
            .map(|g| PhoneGame {
                id: g.id.clone(),
                name: g.name.clone(),
                store: g.store,
                cover: cover_of(g),
                favorite: g.favorite,
                last_played: g.last_played,
                playtime_secs: g.playtime_secs,
            })
            .collect(),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhoneItem {
    id: String,
    name: String,
    store: Store,
    cover: Option<String>,
}

async fn catalog_items(State(ctx): State<Arc<Ctx>>) -> Json<Vec<PhoneItem>> {
    let catalog = catalog::load(&ctx.app, false).await;
    Json(
        catalog
            .items
            .into_iter()
            .map(|i| PhoneItem { id: i.id, name: i.name, store: i.store, cover: i.cover })
            .collect(),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionInfo {
    current: Option<SessionState>,
    suspended: Vec<SessionState>,
}

fn session_info(app: &AppHandle) -> SessionInfo {
    let launcher = &app.state::<AppState>().launcher;
    SessionInfo { current: launcher.current(), suspended: launcher.suspended() }
}

async fn session(State(ctx): State<Arc<Ctx>>) -> Json<SessionInfo> {
    Json(session_info(&ctx.app))
}

/// Cached artwork, only from GameMode's own artwork folder.
async fn art(State(ctx): State<Arc<Ctx>>, UrlPath((id, kind)): UrlPath<(String, String)>) -> Response {
    let (path, root) = {
        let state = ctx.app.state::<AppState>();
        let data = state.db.lock();
        let Some(game) = data.game(&id) else { return StatusCode::NOT_FOUND.into_response() };
        let path = match kind.as_str() {
            "cover" => &game.artwork.cover,
            "hero" => &game.artwork.hero,
            "logo" => &game.artwork.logo,
            _ => &None,
        };
        (path.clone().map(PathBuf::from), state.data_dir.join("artwork"))
    };
    let Some(path) = path.filter(|p| p.starts_with(&root)) else { return StatusCode::NOT_FOUND.into_response() };
    match tokio::fs::read(&path).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, mime_of(&path)), (header::CACHE_CONTROL, "private, max-age=86400")], bytes).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => "image/jpeg",
    }
}

fn fail(msg: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg }))).into_response()
}

fn done(result: tauri::Result<Result<(), String>>) -> Response {
    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => fail(&e),
        Err(e) => fail(&e.to_string()),
    }
}

#[derive(Deserialize)]
struct IdBody {
    id: String,
}

async fn launch(State(ctx): State<Arc<Ctx>>, Json(body): Json<IdBody>) -> Response {
    let app = ctx.app.clone();
    let game = app.state::<AppState>().db.lock().game(&body.id).cloned();
    let Some(game) = game else { return fail("Gioco non trovato") };
    done(tauri::async_runtime::spawn_blocking(move || launcher::launch(&app, &game).map_err(|e| format!("{e:#}"))).await)
}

async fn install(State(ctx): State<Arc<Ctx>>, Json(body): Json<IdBody>) -> Response {
    done(Ok(crate::install_catalog(&ctx.app, body.id).await))
}

#[derive(Deserialize)]
struct NavBody {
    action: gamepad::Action,
    #[serde(default)]
    repeat: bool,
}

/// A button of the remote: routed exactly like a controller press.
async fn nav(State(ctx): State<Arc<Ctx>>, Json(body): Json<NavBody>) -> StatusCode {
    gamepad::route(&ctx.app, body.action, body.repeat);
    StatusCode::NO_CONTENT
}

#[derive(Serialize, Deserialize, Clone)]
struct TextBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    enter: bool,
    #[serde(default)]
    backspace: bool,
}

/// Text from the phone's keyboard: GameMode's own windows get it as an
/// event (into the focused field), anything else is typed with SendInput.
async fn text(State(ctx): State<Arc<Ctx>>, Json(body): Json<TextBody>) -> StatusCode {
    let app = ctx.app.clone();
    let target = if overlay::is_open() {
        Some(overlay::LABEL)
    } else if window::is_foreground(&app) {
        Some("main")
    } else {
        None
    };
    if let Some(label) = target {
        let _ = app.emit_to(label, "remote://text", body);
    } else {
        let _ = tauri::async_runtime::spawn_blocking(move || {
            if body.backspace {
                assist::press(assist::Key::Backspace);
            }
            assist::type_text(&body.text);
            if body.enter {
                assist::press(assist::Key::Enter);
            }
        })
        .await;
    }
    StatusCode::NO_CONTENT
}

#[derive(Deserialize)]
struct PowerBody {
    action: system::PowerAction,
}

async fn power(State(ctx): State<Arc<Ctx>>, Json(body): Json<PowerBody>) -> Response {
    let app = ctx.app.clone();
    done(
        tauri::async_runtime::spawn_blocking(move || {
            // Frozen games can't answer the shutdown: let them go first.
            if !matches!(body.action, system::PowerAction::Sleep) {
                launcher::thaw_all(&app);
            }
            system::power(body.action).map_err(|e| format!("{e:#}"))
        })
        .await,
    )
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
enum GameBody {
    /// Quick Resume: freeze the running game.
    Suspend,
    Close,
    /// Bring the running game back to the screen.
    Front,
    /// Thaw a frozen game.
    Resume { id: String },
}

async fn game(State(ctx): State<Arc<Ctx>>, Json(body): Json<GameBody>) -> Response {
    let app = ctx.app.clone();
    done(
        tauri::async_runtime::spawn_blocking(move || match body {
            GameBody::Suspend => launcher::suspend_current(&app).map_err(|e| format!("{e:#}")),
            GameBody::Close => launcher::close_running(&app).then_some(()).ok_or_else(|| "Nessun gioco in corso".to_string()),
            GameBody::Front => {
                let pids = app.state::<AppState>().launcher.running_pids();
                window::resume_game(&app, &pids).then_some(()).ok_or_else(|| "Nessun gioco in corso".to_string())
            }
            GameBody::Resume { id } => launcher::resume_suspended(&app, &id).map_err(|e| format!("{e:#}")),
        })
        .await,
    )
}

async fn events(ws: WebSocketUpgrade, State(ctx): State<Arc<Ctx>>) -> Response {
    ws.on_upgrade(move |socket| push(socket, ctx))
}

/// Forwards backend events until the phone or the server goes away.
async fn push(mut socket: WebSocket, ctx: Arc<Ctx>) {
    let mut rx = ctx.events.subscribe();
    let hello = serde_json::json!({ "event": "hello", "payload": session_info(&ctx.app) }).to_string();
    if socket.send(Message::Text(hello.into())).await.is_err() {
        return;
    }
    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
            ev = rx.recv() => match ev {
                Ok(text) => {
                    if text == BYE || socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            },
        }
    }
}

// ---- firewall -------------------------------------------------------------

/// Windows blocks incoming connections until a rule allows them. Adding one
/// needs elevation: a single UAC prompt, from Settings.
pub fn firewall_state() -> Firewall {
    match fw::present() {
        Some(true) => Firewall::Ok,
        Some(false) => Firewall::Missing,
        None => Firewall::Unknown,
    }
}

/// Adds the firewall rule (UAC prompt) unless it exists. Returns whether
/// the rule is present afterwards; false when the user refused. Called
/// before the server binds, so Windows' own "allow access" alert (whose
/// Cancel adds block rules) doesn't show up.
pub fn firewall_allow() -> Result<bool> {
    // One prompt at a time: a second caller waits, then finds the rule.
    static PROMPT: Mutex<()> = Mutex::new(());
    let _prompt = PROMPT.lock().unwrap_or_else(|e| e.into_inner());
    if fw::present() == Some(true) {
        return Ok(true);
    }
    fw::add()
}

#[cfg(windows)]
mod fw {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use anyhow::{Context, Result};
    use base64::Engine;

    use super::{FIREWALL_RULE, PORT};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The current executable, quoted for a single-quoted PowerShell string.
    fn exe() -> Result<String> {
        Ok(std::env::current_exe().context("eseguibile")?.display().to_string().replace('\'', "''"))
    }

    fn powershell(script: &str) -> std::io::Result<std::process::Output> {
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    /// Whether our allow rule exists and no inbound block rule targets the
    /// executable (Windows adds those when its own alert is cancelled, and
    /// they win over any allow rule); `None` when PowerShell couldn't tell.
    /// Enum values, not netsh's localized text.
    pub fn present() -> Option<bool> {
        let exe = exe().ok()?;
        let script = format!(
            "$ErrorActionPreference = 'SilentlyContinue'\n\
             $block = Get-NetFirewallRule -Direction Inbound -Action Block -Enabled True | Get-NetFirewallApplicationFilter | Where-Object {{ $_.Program -eq '{exe}' }}\n\
             $allow = Get-NetFirewallRule -DisplayName '{FIREWALL_RULE}' | Where-Object {{ $_.Enabled -eq 'True' -and $_.Action -eq 'Allow' }}\n\
             if ($block) {{ 'blocked' }} elseif ($allow) {{ 'ok' }} else {{ 'missing' }}"
        );
        let out = powershell(&script).ok()?;
        match String::from_utf8_lossy(&out.stdout).trim() {
            "ok" => Some(true),
            "missing" | "blocked" => Some(false),
            _ => None,
        }
    }

    /// Runs netsh elevated through PowerShell's `Start-Process -Verb RunAs`
    /// and waits for it: the UAC prompt shows on the secure desktop, also
    /// in console mode. Every inbound rule of the executable goes first,
    /// block rules from Windows' alert included, then ours is added.
    pub fn add() -> Result<bool> {
        let exe = exe()?;
        let args = format!(
            "/c netsh advfirewall firewall delete rule name=all dir=in program=\"{exe}\" >nul & \
             netsh advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=TCP localport={PORT} profile=any program=\"{exe}\""
        );
        let script = format!(
            "$p = Start-Process -FilePath 'cmd.exe' -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '{args}'; exit $p.ExitCode"
        );
        let status = powershell(&script).context("powershell")?.status;
        if !status.success() {
            // UAC refused, or netsh failed.
            return Ok(false);
        }
        Ok(present().unwrap_or(true))
    }
}

#[cfg(not(windows))]
mod fw {
    use anyhow::Result;

    pub fn present() -> Option<bool> {
        Some(true)
    }

    pub fn add() -> Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchSpec;

    #[test]
    fn host_must_be_an_ip_literal() {
        assert!(host_allowed("192.168.1.10:7373"));
        assert!(host_allowed("192.168.1.10"));
        assert!(host_allowed("[fe80::1]:7373"));
        assert!(host_allowed("localhost:7373"));
        assert!(!host_allowed("gamemode.local:7373"));
        assert!(!host_allowed("evil.example.com"));
        assert!(!host_allowed(""));
    }

    #[test]
    fn token_from_query() {
        assert_eq!(query_param(Some("a=1&t=abc"), "t").as_deref(), Some("abc"));
        assert_eq!(query_param(Some("t=abc"), "t").as_deref(), Some("abc"));
        assert_eq!(query_param(Some("tt=abc"), "t"), None);
        assert_eq!(query_param(None, "t"), None);
    }

    #[test]
    fn same_is_exact() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert!(!same("", "a"));
    }

    #[test]
    fn pin_has_six_digits_and_token_is_random() {
        let pin = new_pin();
        assert_eq!(pin.len(), 6);
        assert!(pin.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(hex(&random_bytes(16)), hex(&random_bytes(16)));
    }

    #[test]
    fn token_persists_in_data_dir() {
        let dir = std::env::temp_dir().join(format!("gm-companion-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = load_token(&dir);
        let b = load_token(&dir);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn covers_go_through_art_route_unless_remote() {
        let mut game = Game::new(Store::Steam, "1", "A", LaunchSpec::Uri { uri: "steam://run/1".into() });
        assert_eq!(cover_of(&game), None);
        game.cover_url = Some("https://cdn/x.jpg".into());
        assert_eq!(cover_of(&game).as_deref(), Some("https://cdn/x.jpg"));
        game.artwork.cover = Some("C:\\data\\artwork\\steam_1\\cover.jpg".into());
        assert_eq!(cover_of(&game).as_deref(), Some("/art/steam:1/cover"));
        game.artwork.cover = Some("https://cdn/y.jpg".into());
        assert_eq!(cover_of(&game).as_deref(), Some("https://cdn/y.jpg"));
    }

    #[test]
    fn mime_by_extension() {
        assert_eq!(mime_of(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_of(Path::new("a.jpg")), "image/jpeg");
        assert_eq!(mime_of(Path::new("a")), "image/jpeg");
    }
}
