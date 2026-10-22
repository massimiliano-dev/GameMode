use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::MacosLauncher;

mod artwork;
mod autostart;
mod catalog;
mod db;
mod gamepad;
mod launcher;
mod library;
mod model;
mod procinfo;
mod storefront;
mod system;
mod uninstall;
mod window;

use db::Db;
use launcher::{Launcher, SessionState};
use library::ScanError;
use model::{Game, ManualGame, Settings};

pub struct AppState {
    pub db: Db,
    pub launcher: Launcher,
    pub data_dir: PathBuf,
    pub native_gamepad: AtomicBool,
    pub artwork_running: AtomicBool,
    pub catalog: catalog::CatalogCache,
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
    let (manual, disabled) = {
        let data = state.db.lock();
        (data.manual_games.clone(), data.settings.disabled_stores.clone())
    };
    let result = library::scan_all(library::providers(manual), &disabled);
    let games = state
        .db
        .update(|d| {
            d.merge_scan(result.games);
            d.games.clone()
        })
        .unwrap_or_else(|e| {
            eprintln!("saving library failed: {e:#}");
            state.db.lock().games.clone()
        });
    let update = LibraryUpdate { games, errors: result.errors };
    let _ = app.emit("library://updated", &update);
    artwork::refresh(app, false);
    update
}

#[tauri::command]
fn get_library(state: State<AppState>) -> Vec<Game> {
    state.db.lock().games.clone()
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

fn update_game(state: &AppState, id: &str, f: impl FnOnce(&mut Game)) -> CmdResult<Game> {
    state
        .db
        .update(|d| {
            let game = d.game_mut(id)?;
            f(game);
            Some(game.clone())
        })
        .map_err(err)?
        .ok_or_else(|| "Gioco non trovato".into())
}

#[tauri::command]
fn set_favorite(state: State<AppState>, id: String, value: bool) -> CmdResult<Game> {
    update_game(&state, &id, |g| g.favorite = value)
}

#[tauri::command]
fn set_hidden(state: State<AppState>, id: String, value: bool) -> CmdResult<Game> {
    update_game(&state, &id, |g| g.hidden = value)
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

#[tauri::command]
fn resume_game(state: State<AppState>) -> bool {
    window::focus_process(&state.launcher.running_pids())
}

#[tauri::command]
fn close_game(app: AppHandle) -> bool {
    launcher::close_running(&app)
}

#[tauri::command]
fn power(action: system::PowerAction) -> CmdResult<()> {
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

#[tauri::command]
fn exit_to_desktop(app: AppHandle) {
    window::exit_to_desktop(&app);
}

#[tauri::command]
fn toggle_fullscreen(app: AppHandle) {
    window::toggle_fullscreen(&app);
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn native_gamepad(state: State<AppState>) -> bool {
    state.native_gamepad.load(Ordering::SeqCst)
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
fn install_catalog_item(app: AppHandle, id: String) -> CmdResult<()> {
    catalog::install(&app, &id).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn uninstall_game(app: AppHandle, id: String) -> CmdResult<uninstall::Outcome> {
    let game = app.state::<AppState>().db.lock().game(&id).cloned().ok_or("Gioco non trovato")?;
    let handle = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || uninstall::uninstall(&handle, &game))
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    if outcome == uninstall::Outcome::Removed {
        let handle = app.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || scan(&handle)).await;
    }
    Ok(outcome)
}

#[tauri::command]
fn refresh_artwork(app: AppHandle) {
    artwork::refresh(&app, true);
}

#[tauri::command]
fn list_storefronts() -> Vec<storefront::Storefront> {
    storefront::list()
}

#[tauri::command]
fn open_store(
    app: AppHandle,
    state: State<AppState>,
    store: model::Store,
    game_id: Option<String>,
) -> CmdResult<storefront::OpensIn> {
    let game = game_id.and_then(|id| state.db.lock().game(&id).cloned());
    storefront::open(&app, store, game.as_ref()).map_err(|e| format!("{e:#}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| window::bring_to_front(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![autostart::ARG])))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let db = Db::open(&data_dir)?;
            db.update(|d| d.upgrade_artwork(artwork::PIPELINE_VERSION))?;
            app.manage(AppState {
                db,
                launcher: Launcher::default(),
                data_dir,
                native_gamepad: AtomicBool::new(false),
                artwork_running: AtomicBool::new(false),
                catalog: catalog::CatalogCache::default(),
                scanning: AtomicBool::new(false),
            });

            let handle = app.handle().clone();
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
            if window::wants_fullscreen(&handle) {
                if let Some(w) = window::main_window(&handle) {
                    let _ = w.set_fullscreen(true);
                }
            }
            if app.state::<AppState>().db.lock().settings.preload_steam {
                std::thread::spawn(launcher::preload_steam);
            }
            let native = gamepad::start(handle.clone());
            app.state::<AppState>().native_gamepad.store(native, Ordering::SeqCst);

            // The saved library shows instantly; the fresh scan replaces it.
            std::thread::spawn(move || {
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
            resume_game,
            close_game,
            power,
            get_volume,
            set_volume,
            set_muted,
            exit_to_desktop,
            toggle_fullscreen,
            quit_app,
            native_gamepad,
            get_autostart,
            set_autostart,
            refresh_artwork,
            list_storefronts,
            open_store,
            get_catalog,
            install_catalog_item,
            uninstall_game,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
