//! Epic games without the Epic Games Launcher, through legendary
//! (https://github.com/legendary-gl/legendary, GPL-3.0), as Heroic does:
//! install, update, repair, uninstall and launch, with progress in
//! GameMode's own download views.
//!
//! legendary is not shipped: `binary` downloads a pinned release on first
//! use and checks its SHA-256 before every first run. Its configuration
//! lives apart from any other legendary install, under GameMode's data
//! folder (`LEGENDARY_CONFIG_PATH`). Without it, Epic works as before
//! (Epic Games Launcher manifests and `com.epicgames.launcher://` links).

// The executable only exists for Windows: elsewhere most of this is idle.
#![cfg_attr(not(windows), allow(dead_code))]

pub mod binary;
pub mod jobs;
mod login;
pub mod parse;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::catalog::{CatalogItem, Source};
use crate::model::{Game, LaunchSpec, Store};
use crate::updates::{GameUpdate, UpdateState};

#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
/// The executable on disk matched the pinned hash in this run.
static VERIFIED: AtomicBool = AtomicBool::new(false);
/// `legendary list --json`, parsed.
static OWNED: Mutex<Option<(Instant, Vec<parse::Owned>)>> = Mutex::new(None);
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

/// The owned-games list is asked again after this long.
const OWNED_TTL: Duration = Duration::from_secs(30 * 60);
/// Background refresh of what's owned and its latest builds (for updates).
const REFRESH_EVERY: Duration = Duration::from_secs(6 * 3600);

pub fn init(data_dir: &Path, enabled: bool) {
    let _ = ROOT.set(data_dir.join("legendary"));
    ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

fn root() -> Option<&'static PathBuf> {
    ROOT.get()
}

pub fn exe_path() -> Option<PathBuf> {
    root().map(|r| r.join("legendary.exe"))
}

fn config_dir() -> Option<PathBuf> {
    root().map(|r| r.join("config"))
}

fn config_file(name: &str) -> Option<PathBuf> {
    config_dir().map(|d| d.join(name))
}

pub fn downloaded() -> bool {
    exe_path().is_some_and(|p| p.is_file())
}

/// Epic games go through legendary.
pub fn active() -> bool {
    cfg!(windows) && ENABLED.load(Ordering::SeqCst) && downloaded()
}

/// The legendary executable, checked against the pinned hash once per run.
fn verified_exe() -> Result<PathBuf> {
    let Some(exe) = exe_path().filter(|p| p.is_file()) else { bail!("legendary non è installato") };
    if !VERIFIED.load(Ordering::SeqCst) {
        binary::verify(&exe, binary::expected_sha256())?;
        VERIFIED.store(true, Ordering::SeqCst);
    }
    Ok(exe)
}

/// A legendary command with GameMode's own configuration, no console
/// window and no input (a question it would ask fails instead of hanging).
pub fn command(args: &[String]) -> Result<Command> {
    if !cfg!(windows) {
        bail!("Disponibile solo su Windows");
    }
    let exe = verified_exe()?;
    let config = config_dir().context("cartella dei dati non pronta")?;
    let mut cmd = Command::new(exe);
    // legendary starts the games: they get a clean environment.
    crate::webview2::scrub(&mut cmd);
    cmd.args(args)
        .env("LEGENDARY_CONFIG_PATH", config)
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(cmd)
}

struct Output {
    success: bool,
    stdout: String,
    /// stdout and stderr together, for messages.
    all: String,
}

fn run(args: &[&str]) -> Result<Output> {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let out = command(&args)?.stdout(Stdio::piped()).stderr(Stdio::piped()).output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    Ok(Output { success: out.status.success(), all: format!("{stdout}\n{stderr}"), stdout })
}

/// Tells the frontend to read the status again.
pub(crate) fn changed(app: &AppHandle) {
    let _ = app.emit("legendary://changed", ());
}

fn error(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("legendary://error", message.into());
}

// ---------- activation ----------

/// Downloads (or checks) the executable and prepares the configuration.
pub async fn ensure_ready(app: &AppHandle) -> Result<()> {
    if !cfg!(windows) {
        bail!("Disponibile solo su Windows");
    }
    let exe = exe_path().context("cartella dei dati non pronta")?;
    let good = exe.is_file() && binary::verify(&exe, binary::expected_sha256()).is_ok();
    if !good {
        if DOWNLOADING.swap(true, Ordering::SeqCst) {
            bail!("Download di legendary già in corso");
        }
        let handle = app.clone();
        let result = binary::download(&exe, move |p| {
            let _ = handle.emit("legendary://download", p);
        })
        .await;
        DOWNLOADING.store(false, Ordering::SeqCst);
        result?;
    }
    VERIFIED.store(true, Ordering::SeqCst);
    write_config()?;
    Ok(())
}

/// GameMode pins legendary's version: its self-update notices are off.
fn write_config() -> Result<()> {
    let dir = config_dir().context("cartella dei dati non pronta")?;
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("config.ini");
    if !file.exists() {
        std::fs::write(file, "[Legendary]\ndisable_update_check = true\ndisable_update_notice = true\n")?;
    }
    Ok(())
}

// ---------- status ----------

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Windows only.
    pub supported: bool,
    pub downloaded: bool,
    pub enabled: bool,
    /// Epic account legendary is signed in with.
    pub account: Option<String>,
    /// The Epic Games Launcher is signed in on this PC: its session can be imported.
    pub egl_signed_in: bool,
    /// Games the Epic Games Launcher installed that legendary doesn't manage yet.
    pub egl_games: u32,
    pub version: &'static str,
    pub project_url: &'static str,
    pub license_url: &'static str,
}

pub fn status() -> Status {
    let managed: HashSet<String> = read_installed().into_iter().map(|g| g.app_name.to_lowercase()).collect();
    let egl_games = egl_games().iter().filter(|g| !managed.contains(&native(&g.id).to_lowercase())).count() as u32;
    Status {
        supported: cfg!(windows),
        downloaded: downloaded(),
        enabled: ENABLED.load(Ordering::SeqCst),
        account: account(),
        egl_signed_in: egl_settings().and_then(|p| std::fs::read_to_string(p).ok()).is_some_and(|s| parse::egl_remembers_login(&s)),
        egl_games,
        version: binary::VERSION,
        project_url: binary::PROJECT_URL,
        license_url: binary::LICENSE_URL,
    }
}

pub fn account() -> Option<String> {
    let src = std::fs::read_to_string(config_file("user.json")?).ok()?;
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    parse::account(&src, &now)
}

/// Where the Epic Games Launcher keeps its remembered sign-in.
fn egl_settings() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join(r"EpicGamesLauncher\Saved\Config\Windows\GameUserSettings.ini"))
}

fn egl_games() -> Vec<Game> {
    let Some(program_data) = std::env::var_os("ProgramData") else { return Vec::new() };
    let dir = PathBuf::from(program_data).join(r"Epic\EpicGamesLauncher\Data\Manifests");
    crate::library::epic::scan_manifests(&dir).unwrap_or_default()
}

fn native(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, n)| n)
}

// ---------- sign-in ----------

pub fn login(app: &AppHandle) -> Result<()> {
    if !active() {
        bail!("Attiva prima la gestione dei giochi Epic");
    }
    login::open(app)
}

/// Signs in with an authorization code from the login page.
fn auth_with_code(code: &str) -> Result<String> {
    let out = run(&["auth", "--code", code])?;
    account().ok_or_else(|| {
        let message = out.all.lines().filter_map(parse::error_message).last();
        anyhow::anyhow!(message.map_or("Accesso a Epic non riuscito".into(), |m| format!("Accesso a Epic non riuscito: {m}")))
    })
}

/// Takes over the Epic Games Launcher's session (which signs it out).
pub fn import_auth(app: &AppHandle) -> Result<String> {
    if !active() {
        bail!("Attiva prima la gestione dei giochi Epic");
    }
    let out = run(&["auth", "--import"])?;
    let result = account().ok_or_else(|| {
        let message = out.all.lines().filter_map(parse::error_message).last();
        anyhow::anyhow!(message.map_or("Nessun accesso valido nell'Epic Games Launcher".into(), |m| format!("Importazione non riuscita: {m}")))
    });
    after_sign_in(app);
    result
}

fn after_sign_in(app: &AppHandle) {
    *OWNED.lock().unwrap() = None;
    changed(app);
    // Fills legendary's game list (needed to import and update) in the background.
    let app = app.clone();
    std::thread::spawn(move || {
        if owned(true).is_ok() {
            crate::catalog::invalidate(&app);
            changed(&app);
        }
    });
}

pub fn logout(app: &AppHandle) -> Result<()> {
    run(&["auth", "--delete"])?;
    *OWNED.lock().unwrap() = None;
    crate::catalog::invalidate(app);
    changed(app);
    Ok(())
}

// ---------- library ----------

pub(crate) fn read_installed() -> Vec<parse::Installed> {
    config_file("installed.json")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse::parse_installed(&s))
        .unwrap_or_default()
}

/// Games legendary installed or imported, launched through it. Read from
/// its `installed.json` (what `list-installed --json` prints), so a scan
/// spawns no process and works offline.
pub fn installed_games() -> Vec<Game> {
    let (true, Some(exe)) = (active(), exe_path()) else { return Vec::new() };
    read_installed()
        .into_iter()
        .map(|g| {
            let launch = LaunchSpec::Exe { path: exe.clone(), args: vec!["launch".into(), g.app_name.clone()], cwd: None };
            let mut game = Game::new(Store::Epic, &g.app_name, g.title.trim(), launch).with_install_dir(&g.install_path);
            game.size_bytes = Some(g.install_size).filter(|&b| b > 0);
            game.cover_url = cover_of(&g.app_name);
            game
        })
        .collect()
}

/// Cover from legendary's cached store metadata.
fn cover_of(app_name: &str) -> Option<String> {
    let path = config_dir()?.join("metadata").join(format!("{app_name}.json"));
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    parse::cover_and_hero_of(value.get("metadata")?).0
}

pub fn is_exe(path: &Path) -> bool {
    exe_path().is_some_and(|exe| exe == path)
}

/// The game is installed and launched through legendary.
pub fn manages(game: &Game) -> bool {
    game.store == Store::Epic && matches!(&game.launch, LaunchSpec::Exe { path, .. } if is_exe(path))
}

// ---------- updates ----------

/// Running operations, then games with a newer build available.
pub fn updates() -> Vec<GameUpdate> {
    if !active() {
        return Vec::new();
    }
    let mut list = jobs::updates();
    let assets = config_file("assets.json")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| parse::parse_assets(&s))
        .unwrap_or_default();
    for app_name in parse::outdated(&read_installed(), &assets) {
        let game_id = Store::Epic.game_id(&app_name);
        if !list.iter().any(|u| u.game_id == game_id) {
            list.push(GameUpdate { game_id, state: UpdateState::Available, done: 0, total: 0, progress: None });
        }
    }
    list
}

pub fn update_for(game_id: &str) -> Option<GameUpdate> {
    updates().into_iter().find(|u| u.game_id == game_id)
}

// ---------- launch ----------

/// Called by the launcher for `LaunchSpec::Exe` pointing at legendary:
/// the game starts in the background, after an update when one is due.
pub fn spawn_launch(app: &AppHandle, args: &[String]) -> Result<()> {
    let Some(app_name) = args.get(1).cloned() else { bail!("Gioco non valido") };
    verified_exe()?;
    let app = app.clone();
    std::thread::spawn(move || launch_flow(&app, &app_name));
    Ok(())
}

fn launch_flow(app: &AppHandle, app_name: &str) {
    // With the version check, as the Epic Games Launcher does: online games
    // refuse an old build anyway.
    let result = match run(&["launch", app_name]) {
        Ok(out) => parse::classify_launch(out.success, &out.all),
        Err(err) => parse::LaunchResult::Failed(format!("{err:#}")),
    };
    match result {
        parse::LaunchResult::Started => {}
        parse::LaunchResult::OutOfDate => {
            let title = title_of(app_name);
            let request = jobs::Request { app_name: app_name.into(), title, kind: jobs::Kind::Update, base_path: None, then_launch: true };
            // The launch screen shows the update's progress meanwhile.
            if let Err(err) = jobs::enqueue(app, request) {
                log::warn!("update before launch: {err:#}");
                launch_now(app, app_name, true);
            }
        }
        parse::LaunchResult::NoNetwork => {
            log::info!("Epic unreachable: launching {app_name} offline");
            match run(&["launch", app_name, "--offline"]) {
                Ok(out) if parse::classify_launch(out.success, &out.all) == parse::LaunchResult::Started => {}
                _ => error(app, "Epic non è raggiungibile e il gioco non si avvia offline"),
            }
        }
        parse::LaunchResult::Failed(message) => {
            log::warn!("legendary launch {app_name}: {message}");
            error(app, format!("Avvio non riuscito: {message}"));
        }
    }
}

/// Starts the game now; `skip_check` after an update (or a failed one).
pub(crate) fn launch_now(app: &AppHandle, app_name: &str, skip_check: bool) {
    let mut args = vec!["launch", app_name];
    if skip_check {
        args.push("--skip-version-check");
    }
    match run(&args) {
        Ok(out) => {
            if let parse::LaunchResult::Failed(message) = parse::classify_launch(out.success, &out.all) {
                error(app, format!("Avvio non riuscito: {message}"));
            }
        }
        Err(err) => error(app, format!("Avvio non riuscito: {err:#}")),
    }
}

fn title_of(app_name: &str) -> String {
    let cached = OWNED.lock().unwrap().as_ref().and_then(|(_, list)| list.iter().find(|o| o.app_name == app_name).map(|o| o.title.clone()));
    cached
        .or_else(|| read_installed().into_iter().find(|g| g.app_name == app_name).map(|g| g.title))
        .unwrap_or_else(|| app_name.to_string())
}

// ---------- operations ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstallTarget {
    pub path: String,
    /// Free bytes on its drive; 0 when unknown.
    pub free: u64,
    pub default: bool,
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".gamemode-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(probe);
    ok
}

/// The first folder games already live in that a normal user can write to
/// (Program Files can't be), else `fallback`.
pub fn pick_base(candidates: &[PathBuf], usable: impl Fn(&Path) -> bool, fallback: PathBuf) -> PathBuf {
    candidates.iter().find(|c| usable(c)).cloned().unwrap_or(fallback)
}

/// Default install folder: where legendary or the Epic Games Launcher put
/// games already, else `C:\Games`.
pub fn default_base() -> PathBuf {
    let parents: Vec<PathBuf> = read_installed()
        .iter()
        .map(|g| PathBuf::from(&g.install_path))
        .chain(egl_games().into_iter().filter_map(|g| g.install_dir))
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    pick_base(&parents, writable, PathBuf::from(format!(r"{drive}\Games")))
}

/// The default folder, then a `Games` folder on every other drive.
pub fn install_targets() -> Vec<InstallTarget> {
    let drives = crate::storage::drives();
    let free_of = |p: &Path| crate::storage::drive_of(&drives, p).map_or(0, |i| drives[i].available);
    let default = default_base();
    let mut targets = vec![InstallTarget { path: default.to_string_lossy().into_owned(), free: free_of(&default), default: true }];
    let default_drive = crate::storage::drive_of(&drives, &default);
    for (i, drive) in drives.iter().enumerate() {
        if Some(i) == default_drive || drive.available == 0 {
            continue;
        }
        let path = Path::new(&drive.mount).join("Games");
        targets.push(InstallTarget { path: path.to_string_lossy().into_owned(), free: drive.available, default: false });
    }
    targets
}

fn require_account() -> Result<()> {
    if !active() {
        bail!("Attiva prima la gestione dei giochi Epic in Impostazioni → Librerie");
    }
    if account().is_none() {
        bail!("Accedi al tuo account Epic in Impostazioni → Librerie");
    }
    Ok(())
}

pub fn install(app: &AppHandle, app_name: &str, base: Option<PathBuf>) -> Result<()> {
    require_account()?;
    let base = base.unwrap_or_else(default_base);
    // legendary refuses a base folder that doesn't exist.
    std::fs::create_dir_all(&base).with_context(|| format!("Non riesco a creare {}", base.display()))?;
    let request = jobs::Request { app_name: app_name.into(), title: title_of(app_name), kind: jobs::Kind::Install, base_path: Some(base), then_launch: false };
    jobs::enqueue(app, request).map(|_| ())
}

/// Update or repair of an installed game.
pub fn game_action(app: &AppHandle, game: &Game, kind: jobs::Kind) -> Result<()> {
    if !manages(game) {
        bail!("Questo gioco non è gestito da GameMode");
    }
    if !matches!(kind, jobs::Kind::Update | jobs::Kind::Repair) {
        bail!("Operazione non valida");
    }
    require_account()?;
    let request = jobs::Request { app_name: native(&game.id).into(), title: game.name.clone(), kind, base_path: None, then_launch: false };
    jobs::enqueue(app, request).map(|_| ())
}

/// Blocking: waits for its turn and for legendary to delete the files.
pub fn uninstall(app: &AppHandle, game: &Game) -> Result<()> {
    let request = jobs::Request { app_name: native(&game.id).into(), title: game.name.clone(), kind: jobs::Kind::Uninstall, base_path: None, then_launch: false };
    let id = jobs::enqueue(app, request)?;
    jobs::wait(id)
}

/// Takes over the games installed with the Epic Games Launcher, without
/// downloading them again.
pub fn import_games(app: &AppHandle) -> Result<()> {
    require_account()?;
    // egl-sync only imports games legendary knows the account owns.
    if !config_file("assets.json").is_some_and(|p| p.is_file()) {
        owned(true)?;
    }
    let request = jobs::Request {
        app_name: String::new(),
        title: "Giochi dell'Epic Games Launcher".into(),
        kind: jobs::Kind::Import,
        base_path: None,
        then_launch: false,
    };
    jobs::enqueue(app, request).map(|_| ())
}

// ---------- catalog ----------

/// `legendary list --json`: owned games (and, as a side effect, fresh
/// latest builds for update checks). Cached for a while.
fn owned(force: bool) -> Result<Vec<parse::Owned>> {
    if !force {
        if let Some((at, list)) = OWNED.lock().unwrap().as_ref() {
            if at.elapsed() < OWNED_TTL {
                return Ok(list.clone());
            }
        }
    }
    let out = run(&["list", "--json"])?;
    if !out.success {
        let message = out.all.lines().filter_map(parse::error_message).last();
        bail!(jobs::friendly(message.as_deref()));
    }
    let list = parse::parse_list(&out.stdout);
    *OWNED.lock().unwrap() = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Owned Epic games that aren't installed, for the Downloads screen; `None`
/// when legendary isn't in use (the launcher's cache is read instead).
pub fn catalog_items(installed: &HashSet<String>) -> Option<Vec<CatalogItem>> {
    if !active() || account().is_none() {
        return None;
    }
    let managed: HashSet<String> = read_installed().into_iter().map(|g| format!("epic:{}", g.app_name.to_lowercase())).collect();
    let owned = match owned(false) {
        Ok(list) => list,
        Err(err) => {
            log::warn!("legendary list: {err:#}");
            return None;
        }
    };
    Some(
        owned
            .into_iter()
            .filter(|o| {
                let id = format!("epic:{}", o.app_name.to_lowercase());
                !installed.contains(&id) && !managed.contains(&id)
            })
            .map(|o| CatalogItem {
                id: format!("epic:{}:{}:{}", o.namespace, o.catalog_id, o.app_name),
                source: Source::Epic,
                store: Store::Epic,
                name: o.title,
                cover: o.cover,
                hero: o.hero,
                playtime_secs: 0,
                size_bytes: None,
            })
            .collect(),
    )
}

/// Keeps the owned list and latest builds fresh, so updates show up
/// without opening Downloads.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            if active() && account().is_some() {
                match owned(true) {
                    Ok(_) => crate::catalog::invalidate(&app),
                    Err(err) => log::info!("legendary refresh: {err:#}"),
                }
            }
            std::thread::sleep(REFRESH_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_a_writable_folder_games_already_use() {
        let program_files = PathBuf::from(r"C:\Program Files\Epic Games");
        let games = PathBuf::from(r"D:\Epic");
        let fallback = PathBuf::from(r"C:\Games");
        let usable = |p: &Path| !p.to_string_lossy().starts_with(r"C:\Program Files");
        assert_eq!(pick_base(&[program_files.clone(), games.clone()], usable, fallback.clone()), games);
        assert_eq!(pick_base(&[program_files], usable, fallback.clone()), fallback);
        assert_eq!(pick_base(&[], usable, fallback.clone()), fallback);
    }

    #[test]
    fn manages_only_games_launched_through_its_executable() {
        let dir = std::env::temp_dir().join(format!("gm-legendary-mod-{}", std::process::id()));
        init(&dir, true);
        let exe = exe_path().unwrap();
        let ours = Game::new(Store::Epic, "Fortnite", "Fortnite", LaunchSpec::Exe { path: exe, args: vec![], cwd: None });
        let egl = Game::new(Store::Epic, "Fortnite", "Fortnite", LaunchSpec::Uri { uri: "com.epicgames.launcher://x".into() });
        assert!(manages(&ours));
        assert!(!manages(&egl));
        assert!(!active(), "nothing downloaded");
    }
}
