//! Orderly exit: one teardown for every way GameMode ends — its own quit,
//! Alt+F4, an update restart, and Windows shutting down or signing out.
//!
//! Windows asks with `WM_QUERYENDSESSION`; tao doesn't surface it (and ends
//! the process itself on `WM_ENDSESSION`), so a window subclass on the main
//! window catches it first. Inside that handler the games get a chance to
//! close by themselves and save, the saves are backed up (when the setting
//! asks for it), performance mode and the display mode come back, and the
//! console-mode watchdog is told not to open the desktop. Windows shows the
//! block reason text if this takes more than a few seconds. A shutdown from
//! GameMode's own quick menu runs the teardown before asking Windows (see
//! the `power` command), so the handler then answers at once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::AppState;

/// Why GameMode is going away.
#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum Reason {
    /// The user closed GameMode: the games keep running on the desktop.
    Quit,
    /// GameMode restarts itself (update): the desktop is left alone.
    Restart,
    /// Windows is shutting down, restarting or signing the user out.
    Session,
}

/// How long games get to close by themselves before being terminated.
const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// How long the "session ending" mark stays valid: a shutdown takes
/// seconds, and a fast reboot must not find a stale mark.
const SESSION_MARK_SECS: i64 = 120;

static DONE: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Hooks the main window so Windows' own shutdown goes through `teardown`.
/// Call it from the main thread (setup), which owns the window.
pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
    // A mark left by the last shutdown must not fool this session's watchdog.
    clear_session_mark();
    #[cfg(windows)]
    if let Some(hwnd) = crate::window::main_window(app).and_then(|w| w.hwnd().ok()) {
        imp::install(hwnd.0 as isize);
    }
    // The backup on exit never downloads the saves index: get it (or a
    // fresh one) ready now, in the background.
    #[cfg(windows)]
    if app.state::<AppState>().db.lock().settings.backup_on_exit {
        let data_dir = app.state::<AppState>().data_dir.clone();
        std::thread::spawn(move || {
            if let Err(err) = crate::saves::index(&data_dir) {
                eprintln!("saves index preload: {err:#}");
            }
        });
    }
}

/// Puts things back before GameMode ends. Runs once: later calls (the exit
/// event after a quit, for instance) are no-ops.
pub fn teardown(app: &AppHandle, reason: Reason) {
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    // Nothing stays frozen behind GameMode's back, Wand's own freeze included.
    crate::wand::forget();
    crate::launcher::thaw_all(app);
    match reason {
        Reason::Quit => {
            crate::display::reset_mode();
            // In console mode there is no desktop until Explorer runs.
            crate::shell::start_explorer();
        }
        Reason::Restart => crate::display::reset_mode(),
        Reason::Session => {
            // The watchdog would open the desktop when this process dies.
            mark_session_ending();
            let games = crate::launcher::close_all_gently(app, CLOSE_GRACE);
            backup_on_exit(app, &games);
            crate::performance::end();
            crate::display::reset_mode();
        }
    }
}

/// Windows called the shutdown off (another app vetoed it): GameMode stays,
/// with its games already closed, and can go through a teardown again.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn cancelled() {
    clear_session_mark();
    DONE.store(false, Ordering::SeqCst);
}

/// Backs up the saves of the games that were open, when the setting asks.
fn backup_on_exit(app: &AppHandle, game_ids: &[String]) {
    let state = app.state::<AppState>();
    let (enabled, games, root) = {
        let data = state.db.lock();
        // Applications have no saves.
        let games: Vec<_> = game_ids.iter().filter_map(|id| data.game(id).cloned()).filter(|g| g.store != crate::model::Store::App).collect();
        let root = data.settings.backup_dir.clone().unwrap_or_else(|| state.data_dir.join("backups"));
        (data.settings.backup_on_exit, games, root)
    };
    if !enabled || games.is_empty() {
        return;
    }
    // Never download the manifest here: the network is going away and
    // Windows is waiting. An old index is fine, none means no backup.
    let Some(index) = crate::saves::cached_index(&state.data_dir) else {
        eprintln!("backup on exit: no saves index at hand");
        return;
    };
    for game in &games {
        if let Err(err) = crate::saves::backup_with(&index, &root, game, true) {
            eprintln!("backup on exit of {}: {err:#}", game.name);
        }
    }
}

fn session_mark() -> std::path::PathBuf {
    std::env::temp_dir().join("gamemode-session-ending")
}

fn mark_session_ending() {
    let _ = std::fs::write(session_mark(), crate::model::now().to_string());
}

fn clear_session_mark() {
    let _ = std::fs::remove_file(session_mark());
}

/// Windows is ending the session: read by the watchdog, which then leaves
/// the desktop alone.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn session_ending() -> bool {
    std::fs::read_to_string(session_mark())
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .is_some_and(|at| mark_is_recent(at, crate::model::now()))
}

/// A mark counts only for a little while after it was written.
fn mark_is_recent(written_at: i64, now: i64) -> bool {
    (0..SESSION_MARK_SECS).contains(&(now - written_at))
}

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{ENDSESSION_CLOSEAPP, WM_ENDSESSION, WM_QUERYENDSESSION};

    const SUBCLASS_ID: usize = 0x4753;

    pub fn install(raw: isize) {
        let hwnd = HWND(raw as *mut core::ffi::c_void);
        unsafe {
            if !SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0).as_bool() {
                eprintln!("shutdown hook: SetWindowSubclass failed");
            }
        }
    }

    /// Runs before tao's window procedure, on the main thread.
    unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        match msg {
            WM_QUERYENDSESSION => {
                // Restart Manager (an installer replacing files in use)
                // closes just GameMode with ENDSESSION_CLOSEAPP: Windows goes
                // on, so the games stay and the desktop comes back, as on a quit.
                let reason = if (lparam.0 as u32) & ENDSESSION_CLOSEAPP != 0 {
                    super::Reason::Quit
                } else {
                    super::Reason::Session
                };
                // Windows waits for this answer; past a few seconds it shows
                // this text instead of "not responding".
                unsafe {
                    let _ = ShutdownBlockReasonCreate(hwnd, w!("Chiusura dei giochi e salvataggio…"));
                }
                if let Some(app) = super::APP.get() {
                    super::teardown(app, reason);
                }
                unsafe {
                    let _ = ShutdownBlockReasonDestroy(hwnd);
                }
                // TRUE: fine by us. WM_ENDSESSION follows and tao ends the process.
                LRESULT(1)
            }
            WM_ENDSESSION => {
                if wparam.0 == 0 {
                    super::cancelled();
                }
                unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
            }
            _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_counts_only_for_a_short_while() {
        assert!(mark_is_recent(1000, 1000));
        assert!(mark_is_recent(1000, 1000 + SESSION_MARK_SECS - 1));
        assert!(!mark_is_recent(1000, 1000 + SESSION_MARK_SECS));
        // A clock set back must not keep a mark alive.
        assert!(!mark_is_recent(1000, 999));
    }
}
