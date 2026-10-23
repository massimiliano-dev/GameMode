use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;

mod artwork;
mod assist;
mod autostart;
mod catalog;
mod db;
mod devices;
mod display;
mod freeze;
mod gamepad;
mod launcher;
mod library;
mod live;
mod model;
mod overlay;
mod performance;
mod procinfo;
mod saves;
mod shell;
#[cfg(windows)]
mod sqlite;
mod steam_web;
mod storefront;
mod system;
mod tray;
mod uninstall;
mod updates;
mod wand;
mod window;

use db::Db;
use launcher::{Launcher, SessionState};
use library::ScanError;
use db::Data;
use model::{Collection, Game, ManualGame, Profile, Session, Settings};

pub struct AppState {
    pub db: Db,
    pub launcher: Launcher,
    pub data_dir: PathBuf,
    pub native_gamepad: AtomicBool,
    pub artwork_running: AtomicBool,
    pub catalog: catalog::CatalogCache,
    pub steam: steam_web::SteamCache,
    scanning: AtomicBool,
}

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LibraryUpdate {
    games: Vec<Game>,
    errors: Vec<ScanError>,
}

/// Scans every store, merges the result into the saved library and notifies
/// the frontend. Blocking: call it off the main thread.
fn scan(app: &AppHandle) -> LibraryUpdate {
    let state = app.state::<AppState>();
    let (manual, settings) = {
        let data = state.db.lock();
        (data.manual_games.clone(), data.settings.clone())
    };
    let result = library::scan_all(library::providers(manual, &settings), &settings.disabled_stores);
    let games = state
        .db
        .update(|d| {
            d.merge_scan(result.games);
            d.view()
        })
        .unwrap_or_else(|e| {
            eprintln!("saving library failed: {e:#}");
            state.db.lock().view()
        });
    let update = LibraryUpdate { games, errors: result.errors };
    let _ = app.emit("library://updated", &update);
    artwork::refresh(app, false);
    update
}

#[tauri::command]
fn get_library(state: State<AppState>) -> Vec<Game> {
    state.db.lock().view()
}

#[tauri::command]
fn wand_installed() -> bool {
    wand::installed()
}

#[tauri::command]
fn open_wand(app: AppHandle) {
    wand::open(&app);
}

#[tauri::command]
fn open_wand_app(app: AppHandle) {
    wand::open_app(&app);
}

#[tauri::command]
async fn get_game_updates(app: AppHandle) -> Vec<updates::GameUpdate> {
    let games = app.state::<AppState>().db.lock().view();
    tauri::async_runtime::spawn_blocking(move || updates::all(&games)).await.unwrap_or_default()
}

#[tauri::command]
async fn rescan_library(app: AppHandle) -> CmdResult<LibraryUpdate> {
    let state = app.state::<AppState>();
    if state.scanning.swap(true, Ordering::SeqCst) {
        return Err("Scansione già in corso".into());
    }
    let handle = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || scan(&handle)).await.map_err(err);
    state.scanning.store(false, Ordering::SeqCst);
    result
}

#[tauri::command]
fn launch_game(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<()> {
    let game = state.db.lock().game(&id).cloned().ok_or("Gioco non trovato")?;
    launcher::launch(&app, &game).map_err(|e| format!("{e:#}"))
}

/// Changes the active profile's state about one game and returns its new view.
fn update_profile_game(state: &AppState, id: &str, f: impl FnOnce(&mut Data)) -> CmdResult<Game> {
    state
        .db
        .update(|d| {
            d.game(id)?;
            f(d);
            d.view_game(id)
        })
        .map_err(err)?
        .ok_or_else(|| "Gioco non trovato".into())
}

#[tauri::command]
fn set_favorite(state: State<AppState>, id: String, value: bool) -> CmdResult<Game> {
    update_profile_game(&state, &id, |d| Data::toggle_in(&mut d.state_mut().favorites, &id, value))
}

#[tauri::command]
fn set_hidden(state: State<AppState>, id: String, value: bool) -> CmdResult<Game> {
    update_profile_game(&state, &id, |d| Data::toggle_in(&mut d.state_mut().hidden, &id, value))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Profiles {
    profiles: Vec<Profile>,
    active: String,
}

fn profiles_of(d: &Data) -> Profiles {
    Profiles { profiles: d.profiles.clone(), active: d.active_profile.clone() }
}

#[tauri::command]
fn get_profiles(state: State<AppState>) -> Profiles {
    profiles_of(&state.db.lock())
}

#[tauri::command]
fn add_profile(state: State<AppState>, name: String) -> CmdResult<Profiles> {
    if name.trim().is_empty() {
        return Err("Scegli un nome".into());
    }
    state.db.update(|d| {
        d.add_profile(&name);
        profiles_of(d)
    }).map_err(err)
}

#[tauri::command]
fn rename_profile(state: State<AppState>, id: String, name: String) -> CmdResult<Profiles> {
    state.db.update(|d| {
        if let Some(p) = d.profiles.iter_mut().find(|p| p.id == id) {
            p.name = name.trim().to_string();
        }
        profiles_of(d)
    }).map_err(err)
}

#[tauri::command]
fn remove_profile(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<Profiles> {
    let profiles = state.db.update(|d| d.remove_profile(&id).then(|| profiles_of(d))).map_err(err)?;
    let profiles = profiles.ok_or("Non puoi eliminare l'unico profilo")?;
    emit_view(&app);
    Ok(profiles)
}

#[tauri::command]
fn switch_profile(app: AppHandle, state: State<AppState>, id: String) -> CmdResult<Profiles> {
    let profiles = state
        .db
        .update(|d| {
            d.profiles.iter().any(|p| p.id == id).then(|| {
                d.active_profile = id.clone();
                profiles_of(d)
            })
        })
        .map_err(err)?
        .ok_or("Profilo non trovato")?;
    emit_view(&app);
    Ok(profiles)
}

/// Sends the active profile's view of the library to the frontend.
fn emit_view(app: &AppHandle) {
    let games = app.state::<AppState>().db.lock().view();
    let _ = app.emit("library://updated", &LibraryUpdate { games, errors: Vec::new() });
}

/// Play sessions of the active profile, for the statistics screen.
#[tauri::command]
fn get_sessions(state: State<AppState>) -> Vec<Session> {
    let data = state.db.lock();
    data.sessions.iter().filter(|s| s.profile_id == data.active_profile).cloned().collect()
}

#[tauri::command]
fn get_collections(state: State<AppState>) -> Vec<Collection> {
    state.db.lock().state().collections
}

#[tauri::command]
fn add_collection(state: State<AppState>, name: String) -> CmdResult<Collection> {
    if name.trim().is_empty() {
        return Err("Scegli un nome".into());
    }
    state.db.update(|d| d.add_collection(&name)).map_err(err)
}

#[tauri::command]
fn remove_collection(state: State<AppState>, id: String) -> CmdResult<()> {
    state.db.update(|d| d.state_mut().collections.retain(|c| c.id != id)).map_err(err)
}

#[tauri::command]
fn set_in_collection(state: State<AppState>, collection_id: String, game_id: String, value: bool) -> CmdResult<Vec<Collection>> {
    state
        .db
        .update(|d| {
            if let Some(c) = d.state_mut().collections.iter_mut().find(|c| c.id == collection_id) {
                Data::toggle_in(&mut c.game_ids, &game_id, value);
            }
            d.state().collections
        })
        .map_err(err)
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.db.lock().settings.clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> CmdResult<Settings> {
    let key_changed = state.db.lock().settings.steamgriddb_key != settings.steamgriddb_key;
    state.db.update(|d| d.settings = settings.clone()).map_err(err)?;
    if key_changed {
        artwork::refresh(&app, true);
    }
    Ok(settings)
}

#[tauri::command]
fn get_manual_games(state: State<AppState>) -> Vec<ManualGame> {
    state.db.lock().manual_games.clone()
}

#[tauri::command]
fn add_manual_game(state: State<AppState>, name: String, path: PathBuf, args: Vec<String>) -> CmdResult<ManualGame> {
    if !path.is_file() {
        return Err("Eseguibile non trovato".into());
    }
    let game = ManualGame { id: format!("{}", model::now()), name: name.trim().to_string(), path, args };
    state.db.update(|d| d.manual_games.push(game.clone())).map_err(err)?;
    Ok(game)
}

#[tauri::command]
fn remove_manual_game(state: State<AppState>, id: String) -> CmdResult<()> {
    state.db.update(|d| d.manual_games.retain(|m| m.id != id)).map_err(err)
}

#[tauri::command]
fn get_session(state: State<AppState>) -> Option<SessionState> {
    state.launcher.current()
}

/// The games frozen by Quick Resume.
#[tauri::command]
fn get_suspended(state: State<AppState>) -> Vec<SessionState> {
    state.launcher.suspended()
}

/// Quick Resume: freezes the running game and comes back to GameMode.
#[tauri::command]
async fn suspend_game(app: AppHandle) -> CmdResult<()> {
    let handle = app.clone();
    blocking(move || launcher::suspend_current(&handle)).await
}

#[tauri::command]
async fn resume_suspended(app: AppHandle, game_id: String) -> CmdResult<()> {
    let handle = app.clone();
    blocking(move || launcher::resume_suspended(&handle, &game_id)).await
}

#[tauri::command]
async fn close_suspended(app: AppHandle, game_id: String) -> CmdResult<()> {
    let handle = app.clone();
    blocking(move || launcher::close_suspended(&handle, &game_id)).await
}

#[tauri::command]
fn resume_game(app: AppHandle, state: State<AppState>) -> bool {
    window::resume_game(&app, &state.launcher.running_pids())
}

#[tauri::command]
fn close_game(app: AppHandle) -> bool {
    launcher::close_running(&app)
}

#[tauri::command]
fn power(app: AppHandle, action: system::PowerAction) -> CmdResult<()> {
    // Frozen games can't answer the shutdown: let them go first.
    if !matches!(action, system::PowerAction::Sleep) {
        launcher::thaw_all(&app);
    }
    system::power(action).map_err(err)
}

#[tauri::command]
fn get_volume() -> CmdResult<system::Volume> {
    system::volume().map_err(err)
}

#[tauri::command]
fn set_volume(level: f32) -> CmdResult<()> {
    system::set_volume(level).map_err(err)
}

#[tauri::command]
fn set_muted(muted: bool) -> CmdResult<()> {
    system::set_muted(muted).map_err(err)
}

/// Closes the overlay; `resume` hands the screen back to the game.
#[tauri::command]
fn overlay_hide(app: AppHandle, state: State<AppState>, resume: bool) {
    overlay::hide(&app);
    if resume {
        let pids = state.launcher.running_pids();
        window::resume_game(&app, &pids);
        overlay::after_resume(&app, pids);
    }
}

/// From the overlay to the full launcher.
#[tauri::command]
fn overlay_to_launcher(app: AppHandle) {
    overlay::hide(&app);
    window::bring_to_front(&app);
}

#[tauri::command]
fn exit_to_desktop(app: AppHandle) {
    // Nothing stays frozen behind GameMode's back.
    launcher::thaw_all(&app);
    // In console mode there is no desktop until Explorer runs.
    shell::start_explorer();
    window::exit_to_desktop(&app);
}

#[tauri::command]
fn get_console_mode() -> shell::ShellStatus {
    shell::status()
}

#[tauri::command]
fn set_console_mode(enabled: bool) -> CmdResult<shell::ShellStatus> {
    shell::set_enabled(enabled).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn toggle_fullscreen(app: AppHandle) {
    window::toggle_fullscreen(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    // Never leave a game frozen behind.
    wand::forget();
    launcher::thaw_all(&app);
    display::reset_mode();
    shell::start_explorer();
    app.exit(0);
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct DisplayInfo {
    modes: Vec<display::DisplayMode>,
    current: Option<display::DisplayMode>,
}

#[tauri::command]
fn display_modes() -> DisplayInfo {
    DisplayInfo { modes: display::modes(), current: display::current_mode() }
}

#[tauri::command]
fn audio_outputs() -> CmdResult<Vec<display::AudioOutput>> {
    display::audio_outputs().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
fn set_audio_output(id: String) -> CmdResult<()> {
    display::set_audio_output(&id).map_err(|e| format!("{e:#}"))
}

/// Applies TV mode right away, e.g. after changing it in Settings.
#[tauri::command]
fn apply_tv_mode(app: AppHandle) {
    window::apply_tv(&app);
}

/// Runs a blocking device call off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn get_radios() -> CmdResult<devices::Radios> {
    blocking(devices::radios).await
}

#[tauri::command]
async fn set_radio(bluetooth: bool, on: bool) -> CmdResult<()> {
    blocking(move || devices::set_radio(bluetooth, on)).await
}

#[tauri::command]
async fn wifi_networks() -> CmdResult<Vec<devices::WifiNetwork>> {
    blocking(devices::wifi_scan).await
}

#[tauri::command]
async fn wifi_connect(ssid: String, password: Option<String>) -> CmdResult<()> {
    blocking(move || devices::wifi_connect(&ssid, password.as_deref())).await
}

#[tauri::command]
async fn bluetooth_devices(discover: bool) -> CmdResult<Vec<devices::BtDevice>> {
    blocking(move || devices::bt_devices(discover)).await
}

#[tauri::command]
async fn bluetooth_pair(id: String) -> CmdResult<()> {
    blocking(move || devices::bt_pair(&id)).await
}

#[tauri::command]
async fn bluetooth_unpair(id: String) -> CmdResult<()> {
    blocking(move || devices::bt_unpair(&id)).await
}

#[tauri::command]
fn native_gamepad(state: State<AppState>) -> bool {
    state.native_gamepad.load(Ordering::SeqCst)
}

#[tauri::command]
fn get_controllers() -> Vec<gamepad::Controller> {
    gamepad::controllers()
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> bool {
    autostart::is_enabled(&app).unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> CmdResult<()> {
    autostart::set(&app, enabled).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn get_catalog(app: AppHandle, force: bool) -> catalog::Catalog {
    catalog::load(&app, force).await
}

#[tauri::command]
async fn steam_extras(app: AppHandle, appid: u32) -> steam_web::SteamExtras {
    steam_web::extras(&app, appid).await
}

#[tauri::command]
async fn wishlist_deals(app: AppHandle, force: bool) -> steam_web::Deals {
    steam_web::deals(&app, force).await
}

/// Opens a web page or store link and steps aside for it.
#[tauri::command]
async fn open_link(app: AppHandle, url: String) -> CmdResult<()> {
    let allowed = ["https://", "steam://", "ms-settings:"];
    if !allowed.iter().any(|p| url.starts_with(p)) {
        return Err("Link non supportato".into());
    }
    if url.starts_with("steam://") {
        start_assist(&app, !storefront::uses_big_picture(&app));
    }
    let handle = app.clone();
    blocking(move || storefront::open_uri(&handle, &url)).await
}

#[tauri::command]
async fn get_cloud_catalog(app: AppHandle, force: bool) -> Vec<catalog::CatalogItem> {
    catalog::cloud(&app, force).await
}

/// Starts a hand-off to a store window: holding View comes back, and when
/// the window doesn't handle a controller (`needs_keys`) the controller
/// types keys into it.
fn start_assist(app: &AppHandle, needs_keys: bool) {
    let keys = needs_keys && app.state::<AppState>().db.lock().settings.keyboard_assist;
    assist::enable(keys);
}

#[tauri::command]
async fn install_catalog_item(app: AppHandle, id: String) -> CmdResult<()> {
    // Cloud games run in Edge, which handles the controller itself and needs
    // View for the game; the Xbox app and Store handle the controller too.
    if !id.starts_with("cloud:") {
        let native_controller = id.starts_with("gamepass:") || (id.starts_with("steam:") && storefront::uses_big_picture(&app));
        start_assist(&app, !native_controller);
    }
    if let Some(appid) = id.strip_prefix("steam:") {
        catalog::watch_steam_install(app.clone(), appid.to_string());
    }
    // Launching may wait on Windows (WinRT launcher): keep it off the main thread.
    let handle = app.clone();
    blocking(move || catalog::install(&handle, &id)).await
}

#[tauri::command]
async fn uninstall_game(app: AppHandle, id: String) -> CmdResult<uninstall::Outcome> {
    let game = app.state::<AppState>().db.lock().game(&id).cloned().ok_or("Gioco non trovato")?;
    let handle = app.clone();
    let target = game.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || uninstall::uninstall(&handle, &target))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    if outcome == uninstall::Outcome::Started {
        // Big Picture handles the controller by itself.
        start_assist(&app, !(game.store == model::Store::Steam && storefront::uses_big_picture(&app)));
        uninstall::watch(app.clone(), game);
    }
    if outcome == uninstall::Outcome::Removed {
        let handle = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || scan(&handle)).await;
    }
    Ok(outcome)
}

#[tauri::command]
fn emulator_status(state: State<AppState>) -> library::emulators::EmulatorStatus {
    let settings = state.db.lock().settings.clone();
    library::emulators::status(settings.retroarch_path, &settings.rom_dirs)
}

#[tauri::command]
fn set_retroarch_path(state: State<AppState>, path: Option<PathBuf>) -> CmdResult<library::emulators::EmulatorStatus> {
    if path.as_ref().is_some_and(|p| !p.is_file()) {
        return Err("File non trovato".into());
    }
    state.db.update(|d| d.settings.retroarch_path = path).map_err(err)?;
    Ok(emulator_status(state))
}

#[tauri::command]
fn set_rom_dir(state: State<AppState>, system: String, path: Option<PathBuf>) -> CmdResult<library::emulators::EmulatorStatus> {
    if library::emulators::system(&system).is_none() {
        return Err("Sistema sconosciuto".into());
    }
    state
        .db
        .update(|d| {
            d.settings.rom_dirs.retain(|r| r.system != system);
            if let Some(path) = path {
                d.settings.rom_dirs.push(model::RomDir { system: system.clone(), path });
            }
        })
        .map_err(err)?;
    Ok(emulator_status(state))
}

fn backup_root(state: &AppState) -> PathBuf {
    state.db.lock().settings.backup_dir.clone().unwrap_or_else(|| state.data_dir.join("backups"))
}

/// Runs a blocking saves operation for one game.
async fn with_game<T: Send + 'static>(
    app: &AppHandle,
    game_id: &str,
    f: impl FnOnce(&std::path::Path, &std::path::Path, &Game) -> anyhow::Result<T> + Send + 'static,
) -> CmdResult<T> {
    let state = app.state::<AppState>();
    let game = state.db.lock().game(game_id).cloned().ok_or("Gioco non trovato")?;
    let (data_dir, root) = (state.data_dir.clone(), backup_root(&state));
    blocking(move || f(&data_dir, &root, &game)).await
}

#[tauri::command]
async fn save_info(app: AppHandle, game_id: String) -> CmdResult<saves::SaveInfo> {
    with_game(&app, &game_id, saves::info).await
}

#[tauri::command]
async fn backup_game(app: AppHandle, game_id: String) -> CmdResult<saves::BackupInfo> {
    with_game(&app, &game_id, |data, root, game| saves::backup(data, root, game, false)).await
}

#[tauri::command]
async fn restore_backup(app: AppHandle, game_id: String, backup_id: String) -> CmdResult<()> {
    with_game(&app, &game_id, move |_, root, game| saves::restore(root, &game.id, &backup_id)).await
}

#[tauri::command]
async fn delete_backup(app: AppHandle, game_id: String, backup_id: String) -> CmdResult<()> {
    with_game(&app, &game_id, move |_, root, game| saves::delete(root, &game.id, &backup_id)).await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BackupAll {
    saved: usize,
    skipped: usize,
}

/// Backs up every installed game the manifest knows and that has saves.
#[tauri::command]
async fn backup_all(app: AppHandle) -> CmdResult<BackupAll> {
    let state = app.state::<AppState>();
    let games = state.db.lock().games.clone();
    let (data_dir, root) = (state.data_dir.clone(), backup_root(&state));
    blocking(move || {
        let mut out = BackupAll { saved: 0, skipped: 0 };
        for game in &games {
            match saves::backup(&data_dir, &root, game, false) {
                Ok(_) => out.saved += 1,
                Err(_) => out.skipped += 1,
            }
        }
        Ok(out)
    })
    .await
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UpdateInfo {
    version: String,
    notes: Option<String>,
}

/// A newer signed release on GitHub, if any.
#[tauri::command]
async fn check_update(app: AppHandle) -> CmdResult<Option<UpdateInfo>> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(err)?.check().await.map_err(err)?;
    Ok(update.map(|u| UpdateInfo { version: u.version.clone(), notes: u.body.clone() }))
}

/// Downloads and installs the update, then restarts GameMode.
#[tauri::command]
async fn install_update(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_updater::UpdaterExt;
    let Some(update) = app.updater().map_err(err)?.check().await.map_err(err)? else {
        return Err("Nessun aggiornamento disponibile".into());
    };
    // The console-mode watchdog must not open the desktop during the restart.
    shell::mark_updating();
    update.download_and_install(|_, _| {}, || {}).await.map_err(err)?;
    display::reset_mode();
    app.restart();
}

#[tauri::command]
fn refresh_artwork(app: AppHandle) {
    artwork::refresh(&app, true);
}

#[tauri::command]
async fn list_storefronts() -> CmdResult<Vec<storefront::Storefront>> {
    blocking(|| Ok(storefront::list())).await
}

#[tauri::command]
async fn open_store(app: AppHandle, store: model::Store, game_id: Option<String>) -> CmdResult<storefront::OpensIn> {
    let game = game_id.and_then(|id| app.state::<AppState>().db.lock().game(&id).cloned());
    let native_controller = store == model::Store::Xbox || (store == model::Store::Steam && storefront::uses_big_picture(&app));
    start_assist(&app, !native_controller);
    let handle = app.clone();
    blocking(move || storefront::open(&handle, store, game.as_ref())).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The console-mode watchdog is this same executable with `--watchdog`.
    if let Some(pid) = shell::watchdog_pid() {
        shell::run_watchdog(pid);
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // On the desktop for once, Explorer runs the `Run` entries itself,
            // GameMode's included: that copy must not pull us out of the tray.
            if shell::status().desktop_once && args.iter().any(|a| a == autostart::ARG) {
                return;
            }
            window::bring_to_front(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![autostart::ARG])))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            live::init(&data_dir);
            let live_file = live::file_in(&data_dir);
            let db = Db::open(&data_dir)?;
            db.update(|d| d.upgrade_artwork(artwork::PIPELINE_VERSION))?;
            app.manage(AppState {
                db,
                launcher: Launcher::default(),
                data_dir,
                native_gamepad: AtomicBool::new(false),
                artwork_running: AtomicBool::new(false),
                catalog: catalog::CatalogCache::default(),
                steam: steam_web::SteamCache::default(),
                scanning: AtomicBool::new(false),
            });

            let handle = app.handle().clone();
            let console = shell::status().active;
            if console {
                // GameMode is the shell: do Explorer's sign-in chores.
                shell::spawn_watchdog(&live_file);
                shell::keep_desktop_hidden(handle.clone());
                // After an update the startup apps are already running.
                if !shell::restarted_for_update() {
                    std::thread::spawn(shell::run_startup_apps);
                }
                if let Some(w) = window::main_window(&handle) {
                    let _ = w.set_fullscreen(true);
                }
            } else if shell::status().desktop_once {
                // Signed in on the Windows desktop for once: GameMode waits
                // in the tray until asked for.
                tray::create(&handle);
                shell::guard_desktop_once(handle.clone());
                if let Some(w) = window::main_window(&handle) {
                    let _ = w.hide();
                    // The X goes back to the tray, like any tray app: a
                    // destroyed main window would leave the tray's "Apri
                    // GameMode" with nothing to show.
                    let hidden = w.clone();
                    w.on_window_event(move |e| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                            api.prevent_close();
                            let _ = hidden.hide();
                        }
                    });
                }
            }
            autostart::repair(&handle);
            if autostart::launched_at_login() {
                // Explorer is still settling at login and tends to take the
                // foreground back: claim it again once things calm down.
                let h = handle.clone();
                std::thread::spawn(move || {
                    for delay in [3, 8] {
                        std::thread::sleep(std::time::Duration::from_secs(delay));
                        if !window::is_foreground(&h) {
                            window::bring_to_front(&h);
                        }
                    }
                });
            }
            if window::wants_fullscreen(&handle) && !shell::status().desktop_once {
                window::apply_tv(&handle);
                if let Some(w) = window::main_window(&handle) {
                    let _ = w.set_fullscreen(true);
                }
            }
            if app.state::<AppState>().db.lock().settings.preload_steam {
                std::thread::spawn(launcher::preload_steam);
            }
            overlay::create(&handle);
            updates::watch_packages();
            let native = gamepad::start(handle.clone());
            app.state::<AppState>().native_gamepad.store(native, Ordering::SeqCst);

            // Games left running by a crash or an update come first; then the
            // saved library shows instantly and the fresh scan replaces it.
            std::thread::spawn(move || {
                launcher::reattach(&handle);
                let state = handle.state::<AppState>();
                if !state.scanning.swap(true, Ordering::SeqCst) {
                    scan(&handle);
                    state.scanning.store(false, Ordering::SeqCst);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library,
            rescan_library,
            launch_game,
            set_favorite,
            set_hidden,
            get_settings,
            save_settings,
            get_manual_games,
            add_manual_game,
            remove_manual_game,
            get_session,
            get_suspended,
            suspend_game,
            resume_suspended,
            close_suspended,
            resume_game,
            get_game_updates,
            wand_installed,
            open_wand,
            open_wand_app,
            close_game,
            power,
            get_volume,
            set_volume,
            set_muted,
            exit_to_desktop,
            get_console_mode,
            set_console_mode,
            overlay_hide,
            overlay_to_launcher,
            toggle_fullscreen,
            quit_app,
            display_modes,
            audio_outputs,
            set_audio_output,
            apply_tv_mode,
            get_radios,
            set_radio,
            wifi_networks,
            wifi_connect,
            bluetooth_devices,
            bluetooth_pair,
            bluetooth_unpair,
            native_gamepad,
            get_controllers,
            get_autostart,
            set_autostart,
            refresh_artwork,
            check_update,
            install_update,
            save_info,
            backup_game,
            restore_backup,
            delete_backup,
            backup_all,
            emulator_status,
            set_retroarch_path,
            set_rom_dir,
            list_storefronts,
            open_store,
            get_catalog,
            install_catalog_item,
            get_cloud_catalog,
            steam_extras,
            wishlist_deals,
            open_link,
            uninstall_game,
            get_profiles,
            add_profile,
            rename_profile,
            remove_profile,
            switch_profile,
            get_sessions,
            get_collections,
            add_collection,
            remove_collection,
            set_in_collection,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Whatever ends GameMode (its own quit, a closed window, the
            // session ending): nothing stays frozen behind it.
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                launcher::thaw_all(app);
            }
        });
}
