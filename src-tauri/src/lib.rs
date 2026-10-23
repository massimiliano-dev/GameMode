use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;

mod artwork;
mod assist;
mod autostart;
mod catalog;
mod cheats;
mod db;
mod devices;
mod diag;
mod display;
mod freeze;
mod gamepad;
mod haptics;
mod hidpad;
mod keyboard;
mod launcher;
mod library;
mod live;
mod media;
mod metadata;
mod mixer;
mod model;
mod overlay;
mod performance;
mod procinfo;
mod saves;
mod shell;
mod shutdown;
#[cfg(windows)]
mod sqlite;
mod status;
mod steam_web;
mod storage;
mod storefront;
mod system;
mod tray;
mod uia;
mod uninstall;
mod updates;
mod wand;
mod window;
mod audio_events;
mod osd;
mod idle;
mod lifecycle;
mod winupdate;
mod capture;
mod capture_osd;
mod companion;
mod steam_direct;

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

/// The main window's X goes to the tray (desktop-once session) instead of
/// quitting GameMode.
static CLOSE_TO_TRAY: AtomicBool = AtomicBool::new(false);

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
    let mut providers = library::providers(manual, &settings);
    providers.push(Box::new(library::apps::AppsProvider { icon_dir: library::apps::icon_dir(&state.data_dir) }));
    let mut result = library::scan_all(providers, &settings.disabled_stores);
    library::apps::separate_from_games(&mut result.games);
    let games = state
        .db
        .update(|d| {
            d.merge_scan(result.games);
            d.view()
        })
        .unwrap_or_else(|e| {
            log::error!("saving library failed: {e:#}");
            state.db.lock().view()
        });
    let update = LibraryUpdate { games, errors: result.errors };
    let _ = app.emit("library://updated", &update);
    artwork::refresh(app, false);
    storage::measure(app);
    metadata::refresh(app, false);
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
async fn cheats_status(app: AppHandle) -> cheats::CheatsStatus {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || cheats::status(&handle))
        .await
        .unwrap_or_else(|_| cheats::CheatsStatus { available: false, reason: None, scan: None, frozen: Vec::new() })
}

#[tauri::command]
async fn cheats_search(app: AppHandle, value: String, kind: cheats::Kind, fresh: bool) -> CmdResult<cheats::ScanView> {
    blocking(move || cheats::search(&app, &value, kind, fresh)).await
}

#[tauri::command]
fn cheats_set(app: AppHandle, address: u64, value: String) -> CmdResult<()> {
    cheats::set(&app, address, &value).map_err(err)
}

#[tauri::command]
fn cheats_freeze(app: AppHandle, address: u64, value: Option<String>) -> CmdResult<()> {
    cheats::freeze(&app, address, value.as_deref()).map_err(err)
}

#[tauri::command]
fn cheats_unfreeze(address: u64) {
    cheats::unfreeze(address);
}

#[tauri::command]
fn cheats_reset() {
    cheats::reset();
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

/// Off the main thread: freezing the game an app makes room for, or
/// starting Explorer, takes seconds.
#[tauri::command]
async fn launch_game(app: AppHandle, id: String) -> CmdResult<()> {
    let game = app.state::<AppState>().db.lock().game(&id).cloned().ok_or("Gioco non trovato")?;
    log::info!("launching {} ({})", game.name, game.id);
    let handle = app.clone();
    blocking(move || {
        launcher::launch(&handle, &game).inspect_err(|e| log::warn!("launching {} failed: {e:#}", game.name))
    })
    .await
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
    let (key_changed, igdb_changed) = {
        let old = &state.db.lock().settings;
        (
            old.steamgriddb_key != settings.steamgriddb_key,
            old.igdb_client_id != settings.igdb_client_id || old.igdb_client_secret != settings.igdb_client_secret,
        )
    };
    state.db.update(|d| d.settings = settings.clone()).map_err(err)?;
    if key_changed {
        artwork::refresh(&app, true);
    }
    if igdb_changed && metadata::igdb_credentials(&settings).is_some() {
        metadata::refresh(&app, true);
    }
    companion::sync(&app);
    hidpad::set_enabled(settings.hid_pads);
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
async fn power(app: AppHandle, action: system::PowerAction) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || power_now(&app, action)).await.map_err(err)?.map_err(err)
}

/// Runs a power action, the session-ending ones after the teardown. Blocks
/// for seconds: call it off the main thread (the `power` command, the tray,
/// the inactivity thread).
pub(crate) fn power_now(app: &AppHandle, action: system::PowerAction) -> anyhow::Result<()> {
    // Games are closed and saved here, off the main thread and before
    // Windows asks: inside WM_QUERYENDSESSION a slow teardown counts as hung
    // and EWX_FORCEIFHUNG would kill GameMode halfway. Windows' later ask
    // finds the teardown done and gets its answer at once.
    let ends_session = cfg!(windows) && action.ends_session();
    if ends_session {
        shutdown::teardown(app, shutdown::Reason::Session);
    }
    let result = system::power(action);
    if result.is_err() && ends_session {
        shutdown::cancelled();
    }
    result
}

/// Which power actions this PC supports (sleep and hibernation can be off).
#[tauri::command]
fn power_options() -> system::PowerOptions {
    system::power_options()
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

/// What the system's media session plays (Spotify, a browser…), if anything.
#[tauri::command]
fn media_now_playing() -> Option<media::NowPlaying> {
    media::now_playing()
}

#[tauri::command]
async fn media_control(action: media::MediaAction) -> CmdResult<()> {
    blocking(move || media::control(action)).await
}

/// Opens Spotify; with a game running it goes right back behind the game.
#[tauri::command]
async fn open_music(app: AppHandle) -> CmdResult<()> {
    let pids = app.state::<AppState>().launcher.running_pids();
    let handle = app.clone();
    blocking(move || media::open_music(&handle, pids)).await
}

/// Per-app volumes on the default output, game first.
#[tauri::command]
async fn app_volumes(state: State<'_, AppState>) -> CmdResult<Vec<mixer::AppVolume>> {
    let pids = state.launcher.running_pids();
    blocking(move || mixer::app_volumes(&pids)).await
}

#[tauri::command]
async fn set_app_volume(state: State<'_, AppState>, key: String, level: f32) -> CmdResult<()> {
    let pids = state.launcher.running_pids();
    blocking(move || mixer::set_level(&key, level, &pids)).await
}

#[tauri::command]
async fn set_app_muted(state: State<'_, AppState>, key: String, muted: bool) -> CmdResult<()> {
    let pids = state.launcher.running_pids();
    blocking(move || mixer::set_muted(&key, muted, &pids)).await
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

/// From the overlay: the system keyboard over the game.
#[tauri::command]
fn overlay_keyboard(app: AppHandle, state: State<AppState>) {
    let pids = state.launcher.running_pids();
    keyboard::open_over_game(&app, pids);
}

#[tauri::command]
fn keyboard_hide(app: AppHandle) {
    keyboard::dismiss(&app);
}

/// The system keyboard types into the focused window.
#[tauri::command]
fn type_text(text: String) {
    keyboard::type_text(&text);
}

/// `key`: enter, backspace, escape, tab or space.
#[tauri::command]
fn type_key(key: String) -> CmdResult<()> {
    let key = keyboard::key_named(&key).ok_or_else(|| format!("unknown key: {key}"))?;
    assist::press(key);
    Ok(())
}

#[tauri::command]
fn exit_to_desktop(app: AppHandle) {
    log::info!("exit to desktop");
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
    log::info!("quit");
    shutdown::teardown(&app, shutdown::Reason::Quit);
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

/// Interface vibration. During a game only `tick` goes through: the game
/// drives the motors and the last writer wins.
#[tauri::command]
fn rumble(state: State<AppState>, pattern: String) {
    if !state.native_gamepad.load(Ordering::SeqCst) {
        return;
    }
    let Some(pattern) = haptics::Pattern::parse(&pattern) else { return };
    let playing = state.launcher.current().is_some_and(|s| s.phase == launcher::Phase::Running);
    if playing && pattern != haptics::Pattern::Tick {
        return;
    }
    let level = state.db.lock().settings.haptics;
    haptics::rumble(pattern, level);
}

#[tauri::command]
async fn get_system_status() -> status::SystemInfo {
    tauri::async_runtime::spawn_blocking(status::snapshot).await.unwrap_or_default()
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
    install_catalog(&app, id).await
}

/// Opens a catalog item's install dialog in its store, with the controller
/// hand-off; shared with the phone companion.
pub(crate) async fn install_catalog(app: &AppHandle, id: String) -> CmdResult<()> {
    // Cloud games run in Edge, which handles the controller itself and needs
    // View for the game; the Xbox app and Store handle the controller too.
    if !id.starts_with("cloud:") {
        let native_controller = id.starts_with("gamepass:") || (id.starts_with("steam:") && storefront::uses_big_picture(app));
        start_assist(app, !native_controller);
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
        // Applications have no saves.
        for game in games.iter().filter(|g| g.store != model::Store::App) {
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
    log::info!("installing update {}", update.version);
    // The console-mode watchdog must not open the desktop during the restart.
    shell::mark_updating();
    update.download_and_install(|_, _| {}, || {}).await.map_err(err)?;
    display::reset_mode();
    app.restart();
}

/// Windows Update's view: pending updates and whether a restart installs them.
/// `force` asks Microsoft's servers (slow); otherwise the cached answer.
#[tauri::command]
async fn system_updates(app: AppHandle, force: bool) -> winupdate::SystemUpdates {
    if !force {
        if let Some(cached) = winupdate::cached() {
            return cached;
        }
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || winupdate::refresh(&handle, force))
        .await
        .unwrap_or_else(|e| winupdate::SystemUpdates { state: winupdate::SystemUpdateState::Unknown, items: Vec::new(), checked_at: None, error: Some(e.to_string()) })
}

#[tauri::command]
fn refresh_artwork(app: AppHandle) {
    artwork::refresh(&app, true);
}

#[tauri::command]
fn refresh_metadata(app: AppHandle) {
    metadata::refresh(&app, true);
}

#[tauri::command]
fn metadata_running() -> bool {
    metadata::is_running()
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

/// "Shut down in…": a power action after `mode`, with a one-minute warning.
#[tauri::command]
fn schedule_power(app: AppHandle, action: system::PowerAction, mode: idle::PowerMode) -> idle::PowerSchedule {
    idle::schedule(&app, action, mode)
}

#[tauri::command]
fn cancel_power(app: AppHandle) {
    idle::cancel(&app);
}

#[tauri::command]
fn get_power_schedule() -> Option<idle::PowerSchedule> {
    idle::current_schedule()
}

/// Rest mode: screen off now, downloads go on, any input brings it back.
#[tauri::command]
fn rest_mode(app: AppHandle) {
    idle::rest(&app);
}

/// Leaves the screensaver or turns the display back on.
#[tauri::command]
fn idle_wake() {
    idle::wake();
}

/// The frontend saw input (Gamepad API fallback, dev machines).
#[tauri::command]
fn touch_input() {
    idle::touch_input();
}

/// Drives, their free space and what the library takes on each. With
/// `measure`, sizes still unknown get measured in the background meanwhile.
#[tauri::command]
async fn storage_overview(app: AppHandle, measure: Option<bool>) -> CmdResult<storage::Overview> {
    if measure == Some(true) {
        storage::measure(&app);
    }
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || storage::overview(&handle)).await.map_err(err)
}

/// What the cleanup would free; with `apply`, frees it.
#[tauri::command]
async fn storage_cleanup(app: AppHandle, apply: bool) -> CmdResult<storage::Cleanup> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || storage::cleanup(&handle, apply)).await.map_err(err)
}

/// Measures again every game folder walked before (not the sizes a store
/// manifest declares).
#[tauri::command]
fn storage_remeasure(app: AppHandle) {
    storage::remeasure(&app);
}

/// How this GameMode came up: a normal start, a restart by the watchdog
/// after a crash, or safe mode after crashes in a row.
#[tauri::command]
fn startup_info() -> diag::StartupInfo {
    diag::startup_info(shell::recovered_count(), shell::safe_mode())
}

/// The last `lines` lines of the log.
#[tauri::command]
fn read_log(lines: usize) -> String {
    diag::read_log(lines.clamp(1, 2000))
}

/// Saves the log where the user picks; `None` when the dialog is dismissed.
#[tauri::command]
async fn export_log(app: AppHandle) -> CmdResult<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_file_name("gamemode-log.txt").add_filter("Testo", &["txt"]).blocking_save_file()
    })
    .await
    .map_err(err)?;
    let Some(path) = picked else { return Ok(None) };
    let path = path.into_path().map_err(err)?;
    diag::export_to(&path).map_err(err)?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
fn clear_crashes() {
    diag::clear_crashes();
}

/// A screenshot of the running game, from the overlay. `None`: it failed and
/// the in-game card already says why.
#[tauri::command]
async fn take_screenshot(app: AppHandle) -> CmdResult<Option<capture::Capture>> {
    let handle = app.clone();
    blocking(move || capture::take(&handle)).await
}

/// Screenshots, newest first; of one game when `game_id` is given.
#[tauri::command]
async fn list_captures(app: AppHandle, game_id: Option<String>) -> Vec<capture::Capture> {
    let root = capture::root(&app.state::<AppState>().data_dir);
    tauri::async_runtime::spawn_blocking(move || capture::list(&root, game_id.as_deref())).await.unwrap_or_default()
}

#[tauri::command]
fn delete_capture(state: State<AppState>, path: PathBuf) -> CmdResult<()> {
    capture::delete(&capture::root(&state.data_dir), &path).map_err(|e| format!("{e:#}"))
}

/// Opens the screenshots folder (of one game, or all) in Explorer.
#[tauri::command]
fn open_captures_folder(app: AppHandle, state: State<AppState>, game_id: Option<String>) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = capture::folder_of(&capture::root(&state.data_dir), game_id.as_deref());
    std::fs::create_dir_all(&dir).map_err(err)?;
    app.opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(err)
}

/// What the in-game card is showing, for its page when it loads.
#[tauri::command]
fn capture_notice() -> Option<capture_osd::Notice> {
    capture_osd::current()
}

/// The phone companion's address, QR code, pairing code and firewall state.
#[tauri::command]
async fn companion_status(app: AppHandle) -> CmdResult<companion::Status> {
    blocking(move || Ok(companion::status(&app))).await
}

/// Adds the firewall rule for the companion (one UAC prompt). False when refused.
#[tauri::command]
async fn companion_firewall() -> CmdResult<bool> {
    blocking(companion::firewall_allow).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The console-mode watchdog is this same executable with `--watchdog`.
    if let Some(pid) = shell::watchdog_pid() {
        diag::init("watchdog");
        shell::run_watchdog(pid);
        return;
    }
    diag::init("app");
    diag::install_panic_hook();
    diag::quiet_crash_dialogs();
    log::info!(
        "GameMode {} starting, args {:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::args().skip(1).collect::<Vec<_>>()
    );
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
            db.update(|d| d.upgrade_meta(metadata::PIPELINE_VERSION))?;
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
            let safe = shell::safe_mode();
            log::info!(
                "console mode {}, recovered {}, safe mode {}, after update {}",
                if console { "on" } else { "off" },
                shell::recovered_count(),
                safe,
                shell::restarted_for_update()
            );
            if console {
                // GameMode is the shell: do Explorer's sign-in chores.
                shell::spawn_watchdog(&live_file);
                shell::keep_desktop_hidden(handle.clone());
                // After an update or a crash the startup apps are already running.
                if !shell::restarted_for_update() && shell::recovered_count() == 0 {
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
                    // After an update the user was looking at GameMode.
                    if !shell::restarted_for_update() {
                        let _ = w.hide();
                    }
                    // The X goes back to the tray, like any tray app: a
                    // destroyed main window would leave the tray's "Apri
                    // GameMode" with nothing to show. The process stays
                    // alive, so the TV mode must go now, not at exit.
                    let hidden = w.clone();
                    CLOSE_TO_TRAY.store(true, Ordering::SeqCst);
                    w.on_window_event(move |e| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                            api.prevent_close();
                            display::reset_mode();
                            let _ = hidden.set_fullscreen(false);
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
            // Safe mode stays a window over the desktop, with no TV mode.
            if !safe && window::wants_fullscreen(&handle) && (!shell::status().desktop_once || shell::restarted_for_update()) {
                window::apply_tv(&handle);
                if let Some(w) = window::main_window(&handle) {
                    let _ = w.set_fullscreen(true);
                }
            }
            // Safe mode starts as little as possible besides GameMode itself.
            if !safe && app.state::<AppState>().db.lock().settings.preload_steam {
                std::thread::spawn(launcher::preload_steam);
            }
            overlay::create(&handle);
            media::init(&handle);
            osd::create(&handle);
            audio_events::start(handle.clone());
            keyboard::create(&handle);
            keyboard::watch(handle.clone());
            shutdown::install(&handle);
            lifecycle::start(&handle);
            capture::init(&handle);
            companion::sync(&handle);
            updates::watch_packages();
            status::start(handle.clone());
            winupdate::watch(handle.clone());
            hidpad::set_enabled(app.state::<AppState>().db.lock().settings.hid_pads);
            let native = gamepad::start(handle.clone());
            app.state::<AppState>().native_gamepad.store(native, Ordering::SeqCst);
            idle::start(handle.clone());

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
            cheats_status,
            cheats_search,
            cheats_set,
            cheats_freeze,
            cheats_unfreeze,
            cheats_reset,
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
            rumble,
            get_system_status,
            media_now_playing,
            media_control,
            open_music,
            app_volumes,
            set_app_volume,
            set_app_muted,
            schedule_power,
            cancel_power,
            get_power_schedule,
            rest_mode,
            idle_wake,
            touch_input,
            overlay_keyboard,
            keyboard_hide,
            type_text,
            type_key,
            storage_overview,
            storage_cleanup,
            storage_remeasure,
            power_options,
            system_updates,
            startup_info,
            read_log,
            export_log,
            clear_crashes,
            refresh_metadata,
            metadata_running,
            take_screenshot,
            list_captures,
            delete_capture,
            open_captures_folder,
            capture_notice,
            companion_status,
            companion_firewall,
        ])
        .on_window_event(|window, event| {
            // Alt+F4 on the launcher: the hidden overlay window would keep
            // the process alive with no window, so this quits for real.
            // With the tray (desktop once) the X hides the window instead.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && !CLOSE_TO_TRAY.load(Ordering::SeqCst) {
                    api.prevent_close();
                    shutdown::teardown(window.app_handle(), shutdown::Reason::Quit);
                    window.app_handle().exit(0);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app, event| {
            // Whatever ends GameMode (its own quit, a closed window, an
            // update restart): put things back. Windows' own shutdown went
            // through `shutdown` already.
            let code = match event {
                tauri::RunEvent::ExitRequested { code, .. } => code,
                tauri::RunEvent::Exit => {
                    log::info!("exit");
                    None
                }
                _ => return,
            };
            let reason = if code == Some(tauri::RESTART_EXIT_CODE) { shutdown::Reason::Restart } else { shutdown::Reason::Quit };
            shutdown::teardown(app, reason);
        });
}
