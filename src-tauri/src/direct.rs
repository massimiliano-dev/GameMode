//! Installs GameMode runs itself instead of handing off to a store client:
//! GOG offline installers (`gog_store`) and Microsoft Store / Game Pass
//! installs (`msstore`). Running jobs are keyed by catalog or game id
//! (`gog:<id>`, `gamepass:<productId>`, `xbox:<family>`) and show up next to
//! the stores' own updates (`updates::all`), so the status bar, the library
//! and the idle timer see them too.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::updates::{GameUpdate, UpdateState};

struct Job {
    state: UpdateState,
    done: u64,
    total: u64,
    /// Set when the store reports a percentage instead of bytes.
    percent: Option<f64>,
    cancel: Arc<AtomicBool>,
}

static JOBS: LazyLock<Mutex<HashMap<String, Job>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers a job; `None` when one with this id is already running.
pub fn begin(id: &str) -> Option<Arc<AtomicBool>> {
    let mut jobs = JOBS.lock().unwrap();
    if jobs.contains_key(id) {
        return None;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    jobs.insert(
        id.to_string(),
        Job { state: UpdateState::Downloading, done: 0, total: 0, percent: None, cancel: cancel.clone() },
    );
    Some(cancel)
}

pub fn set_bytes(id: &str, state: UpdateState, done: u64, total: u64) {
    if let Some(job) = JOBS.lock().unwrap().get_mut(id) {
        job.state = state;
        job.done = done;
        job.total = total;
        job.percent = None;
    }
}

/// `percent` is 0…1.
pub fn set_percent(id: &str, state: UpdateState, percent: Option<f64>) {
    if let Some(job) = JOBS.lock().unwrap().get_mut(id) {
        job.state = state;
        job.percent = percent.map(|p| p.clamp(0.0, 1.0));
    }
}

pub fn finish(id: &str) {
    JOBS.lock().unwrap().remove(id);
}

/// Asks a running job to stop; false when there is none.
pub fn cancel(id: &str) -> bool {
    match JOBS.lock().unwrap().get(id) {
        Some(job) => {
            job.cancel.store(true, Ordering::SeqCst);
            true
        }
        None => false,
    }
}

fn as_update(id: &str, job: &Job) -> GameUpdate {
    let progress = job.percent.or_else(|| (job.total > 0).then(|| (job.done as f64 / job.total as f64).clamp(0.0, 1.0)));
    GameUpdate { game_id: id.to_string(), state: job.state, done: job.done, total: job.total, progress }
}

pub fn get(id: &str) -> Option<GameUpdate> {
    JOBS.lock().unwrap().get(id).map(|job| as_update(id, job))
}

pub fn list() -> Vec<GameUpdate> {
    JOBS.lock().unwrap().iter().map(|(id, job)| as_update(id, job)).collect()
}

/// Told to the frontend when a job ends (`direct://done`).
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Done {
    pub id: String,
    pub name: String,
    pub ok: bool,
    /// Why it failed; `None` when it worked or was cancelled.
    pub error: Option<String>,
}

pub fn announce(app: &AppHandle, done: Done) {
    let _ = app.emit("direct://done", &done);
}

/// Message of an error that only means "the user cancelled".
pub const CANCELLED: &str = "Annullato";

/// Whether GameMode installs this catalog item by itself (no store client).
pub fn available(app: &AppHandle, id: &str) -> bool {
    match id.split_once(':') {
        Some(("gog", native)) => native.chars().all(|c| c.is_ascii_digit()) && crate::gog_store::signed_in(),
        Some(("gamepass", _)) => crate::msstore::supported() && app.state::<crate::AppState>().db.lock().settings.msstore_direct,
        _ => false,
    }
}

/// Starts a direct install. `None` when this item goes through its store
/// client after all (not available, or the Store refused it).
pub async fn start(app: &AppHandle, id: &str, name: &str) -> Option<Result<(), String>> {
    if !available(app, id) {
        return None;
    }
    match id.split_once(':') {
        Some(("gog", native)) => Some(crate::gog_store::start_install(app, native, name).map_err(|e| format!("{e:#}"))),
        Some(("gamepass", product)) => {
            let (handle, product, name) = (app.clone(), product.to_string(), name.to_string());
            let started = tauri::async_runtime::spawn_blocking(move || crate::msstore::install(&handle, &product, &name))
                .await
                .map_err(anyhow::Error::from)
                .and_then(|r| r);
            match started {
                Ok(()) => Some(Ok(())),
                Err(err) => {
                    log::warn!("Microsoft Store install of {id} failed, handing off: {err:#}");
                    None
                }
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_jobs_and_their_progress() {
        let id = "gog:test-jobs";
        let flag = begin(id).unwrap();
        assert!(begin(id).is_none(), "one job per id");
        set_bytes(id, UpdateState::Downloading, 25, 100);
        assert_eq!(get(id).unwrap().progress, Some(0.25));
        set_percent(id, UpdateState::Installing, Some(1.5));
        let update = get(id).unwrap();
        assert_eq!((update.state, update.progress), (UpdateState::Installing, Some(1.0)));
        assert!(list().iter().any(|u| u.game_id == id));
        assert!(cancel(id));
        assert!(flag.load(Ordering::SeqCst));
        finish(id);
        assert!(get(id).is_none());
        assert!(!cancel(id));
    }
}
