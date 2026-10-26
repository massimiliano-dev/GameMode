//! legendary's long operations (install, update, repair, uninstall, import
//! from the Epic Games Launcher), one at a time: legendary itself refuses
//! to touch its install database from two processes at once. Progress is
//! parsed from its output and shown through `updates` and the Downloads
//! screen.

use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::parse::{self, Line};
use crate::updates::{GameUpdate, UpdateState};

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Install,
    Update,
    Repair,
    Uninstall,
    /// Taking over the Epic Games Launcher's installed games (`egl-sync`).
    Import,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Preparing,
    Verifying,
    Downloading,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: u64,
    pub app_name: String,
    pub title: String,
    pub kind: Kind,
    pub state: State,
    pub phase: Phase,
    /// 0…1, when known.
    pub progress: Option<f64>,
    /// Bytes downloaded and to download; 0 when unknown.
    pub done: u64,
    pub total: u64,
    pub eta_secs: Option<u64>,
    pub error: Option<String>,
    #[serde(skip)]
    base_path: Option<PathBuf>,
    /// Start the game once this (update) is over, whatever the outcome.
    #[serde(skip)]
    then_launch: bool,
    #[serde(skip)]
    finished_at: Option<Instant>,
}

impl Job {
    fn active(&self) -> bool {
        matches!(self.state, State::Queued | State::Running)
    }

    fn args(&self) -> Vec<String> {
        let app = self.app_name.clone();
        let mut args: Vec<String> = vec!["-y".into()];
        match self.kind {
            Kind::Install => {
                args.extend(["install".into(), app, "--skip-sdl".into(), "--with-dlcs".into()]);
                if let Some(base) = &self.base_path {
                    args.extend(["--base-path".into(), base.to_string_lossy().into_owned()]);
                }
            }
            Kind::Update => args.extend(["install".into(), app, "--update-only".into(), "--skip-sdl".into()]),
            Kind::Repair => args.extend(["repair".into(), app]),
            Kind::Uninstall => args.extend(["uninstall".into(), app]),
            Kind::Import => args.extend(["egl-sync".into(), "--import-only".into(), "--one-shot".into()]),
        }
        args
    }
}

pub struct Request {
    pub app_name: String,
    pub title: String,
    pub kind: Kind,
    pub base_path: Option<PathBuf>,
    pub then_launch: bool,
}

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static WORKER: AtomicBool = AtomicBool::new(false);
/// Process id of the running legendary, to cancel it.
static RUNNING_PID: Mutex<Option<u32>> = Mutex::new(None);
static LAST_EMIT: Mutex<Option<Instant>> = Mutex::new(None);

/// Finished jobs stay listed this long.
const KEEP_FINISHED: Duration = Duration::from_secs(30 * 60);
const EMIT_EVERY: Duration = Duration::from_millis(700);

pub fn list() -> Vec<Job> {
    let mut jobs = JOBS.lock().unwrap();
    jobs.retain(|j| j.finished_at.is_none_or(|at| at.elapsed() < KEEP_FINISHED));
    jobs.clone()
}

fn emit(app: &AppHandle, force: bool) {
    {
        let mut last = LAST_EMIT.lock().unwrap();
        if !force && last.is_some_and(|at| at.elapsed() < EMIT_EVERY) {
            return;
        }
        *last = Some(Instant::now());
    }
    let _ = app.emit("legendary://jobs", list());
}

fn update(id: u64, f: impl FnOnce(&mut Job)) {
    if let Some(job) = JOBS.lock().unwrap().iter_mut().find(|j| j.id == id) {
        f(job);
    }
}

/// Queues an operation; an identical one already waiting is reused.
pub fn enqueue(app: &AppHandle, request: Request) -> Result<u64> {
    let id = {
        let mut jobs = JOBS.lock().unwrap();
        if let Some(same) = jobs.iter_mut().find(|j| j.active() && j.app_name == request.app_name) {
            if same.kind != request.kind {
                bail!("C'è già un'operazione in corso per {}", same.title);
            }
            same.then_launch |= request.then_launch;
            return Ok(same.id);
        }
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        jobs.push(Job {
            id,
            app_name: request.app_name,
            title: request.title,
            kind: request.kind,
            state: State::Queued,
            phase: Phase::Preparing,
            progress: None,
            done: 0,
            total: 0,
            eta_secs: None,
            error: None,
            base_path: request.base_path,
            then_launch: request.then_launch,
            finished_at: None,
        });
        id
    };
    if !WORKER.swap(true, Ordering::SeqCst) {
        let app = app.clone();
        std::thread::spawn(move || worker(app));
    }
    emit(app, true);
    Ok(id)
}

pub fn cancel(app: &AppHandle, id: u64) {
    let running = {
        let mut jobs = JOBS.lock().unwrap();
        let Some(job) = jobs.iter_mut().find(|j| j.id == id && j.active()) else { return };
        let running = job.state == State::Running;
        job.state = State::Cancelled;
        job.finished_at = Some(Instant::now());
        running
    };
    if running {
        if let Some(pid) = *RUNNING_PID.lock().unwrap() {
            kill_tree(pid);
        }
    }
    emit(app, true);
}

/// Blocks until the job is over.
pub fn wait(id: u64) -> Result<()> {
    loop {
        let state = JOBS.lock().unwrap().iter().find(|j| j.id == id).map(|j| (j.state, j.error.clone()));
        match state {
            Some((State::Done, _)) | None => return Ok(()),
            Some((State::Failed, error)) => bail!(error.unwrap_or_else(|| "Operazione non riuscita".into())),
            Some((State::Cancelled, _)) => bail!("Operazione annullata"),
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn worker(app: AppHandle) {
    loop {
        let next = {
            let mut jobs = JOBS.lock().unwrap();
            match jobs.iter_mut().find(|j| j.state == State::Queued) {
                Some(job) => {
                    job.state = State::Running;
                    job.clone()
                }
                None => {
                    // Cleared under the lock: a job queued from now on starts a new worker.
                    WORKER.store(false, Ordering::SeqCst);
                    return;
                }
            }
        };
        emit(&app, true);
        let result = run(&app, &next);
        let cancelled = JOBS.lock().unwrap().iter().any(|j| j.id == next.id && j.state == State::Cancelled);
        if !cancelled {
            update(next.id, |j| {
                j.finished_at = Some(Instant::now());
                match &result {
                    Ok(()) => {
                        j.state = State::Done;
                        j.progress = Some(1.0);
                        j.eta_secs = None;
                    }
                    Err(err) => {
                        j.state = State::Failed;
                        j.error = Some(format!("{err:#}"));
                    }
                }
            });
        }
        if let Err(err) = &result {
            log::warn!("legendary {:?} {}: {err:#}", next.kind, next.app_name);
        }
        emit(&app, true);
        // An update the user cancelled doesn't start the game either.
        let then_launch = !cancelled && JOBS.lock().unwrap().iter().find(|j| j.id == next.id).is_some_and(|j| j.then_launch);
        if then_launch {
            super::launch_now(&app, &next.app_name, true);
        }
        // The library learns about installed and imported games; uninstall
        // rescans by itself once it returns.
        if result.is_ok() && next.kind != Kind::Uninstall {
            super::changed(&app);
            let app = app.clone();
            std::thread::spawn(move || {
                crate::scan(&app);
            });
        }
    }
}

/// Runs one job to its end, following its output.
fn run(app: &AppHandle, job: &Job) -> Result<()> {
    let mut cmd = super::command(&job.args())?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    *RUNNING_PID.lock().unwrap() = Some(child.id());

    let (tx, rx) = mpsc::channel::<String>();
    let readers: Vec<_> = [
        child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|mut stream| {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 || tx.send(String::from_utf8_lossy(&buf[..n]).into_owned()).is_err() {
                    break;
                }
            }
        })
    })
    .collect();
    drop(tx);

    let mut pending = String::new();
    let mut last_error: Option<String> = None;
    // "! Failure:" lines say why better than the CRITICAL that follows them.
    let mut failure: Option<String> = None;
    let mut critical = false;
    for chunk in rx {
        pending.push_str(&chunk);
        // Keep an unfinished last line for the next chunk.
        let cut = pending.rfind(['\n', '\r']).map_or(0, |i| i + 1);
        let complete: String = pending.drain(..cut).collect();
        for line in parse::lines(&complete) {
            log::debug!("legendary: {line}");
            if line.contains("] CRITICAL: ") {
                critical = true;
            }
            if let Some(message) = parse::error_message(line) {
                if line.trim_start().starts_with("! Failure:") {
                    failure = Some(message);
                } else {
                    last_error = Some(message);
                }
            }
            apply(job.id, parse::parse_line(line));
        }
        emit(app, false);
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait()?;
    *RUNNING_PID.lock().unwrap() = None;

    let installed = super::read_installed().iter().any(|g| g.app_name == job.app_name);
    let ok = status.success()
        && !critical
        && match job.kind {
            Kind::Install | Kind::Update => installed,
            Kind::Uninstall => !installed,
            Kind::Repair | Kind::Import => true,
        };
    if ok {
        return Ok(());
    }
    bail!(friendly(failure.or(last_error).as_deref()))
}

fn apply(id: u64, line: Line) {
    match line {
        Line::DownloadSize(bytes) => update(id, |j| {
            j.total = bytes;
            j.done = 0;
        }),
        Line::Progress { fraction, eta_secs } => update(id, |j| {
            j.phase = Phase::Downloading;
            j.progress = Some(fraction);
            j.eta_secs = eta_secs.filter(|&s| s > 0);
        }),
        Line::Downloaded(bytes) => update(id, |j| j.done = bytes.min(j.total.max(bytes))),
        Line::Verifying(fraction) => update(id, |j| {
            j.phase = Phase::Verifying;
            j.progress = Some(fraction);
            j.eta_secs = None;
        }),
        Line::Error | Line::Other => {}
    }
}

/// legendary's English error, said the way GameMode says things when it's
/// a common one.
pub fn friendly(error: Option<&str>) -> String {
    let Some(error) = error else { return "legendary si è fermato con un errore".into() };
    let lower = error.to_lowercase();
    if lower.contains("disk space") {
        "Spazio insufficiente sull'unità scelta".into()
    } else if lower.contains("login failed") || lower.contains("log in failed") {
        "Accesso a Epic non riuscito: controlla la connessione o accedi di nuovo da Impostazioni → Librerie".into()
    } else if lower.contains("third-party store") {
        "Questo gioco si installa da un altro store (EA app o Ubisoft Connect)".into()
    } else {
        format!("legendary: {error}")
    }
}

/// Queued and running jobs as game updates (installs of games not in the
/// library yet included, so idle keeps the PC awake for them).
pub fn updates() -> Vec<GameUpdate> {
    JOBS.lock()
        .unwrap()
        .iter()
        .filter(|j| j.active() && matches!(j.kind, Kind::Install | Kind::Update | Kind::Repair))
        .map(|j| GameUpdate {
            game_id: crate::model::Store::Epic.game_id(&j.app_name),
            state: if j.phase == Phase::Verifying { UpdateState::Installing } else { UpdateState::Downloading },
            done: j.done,
            total: j.total,
            progress: j.progress.filter(|_| j.state == State::Running),
        })
        .collect()
}

#[cfg(windows)]
fn kill_tree(pid: u32) {
    use std::os::windows::process::CommandExt;
    // The onefile executable runs Python in a child process, and the
    // downloader in more: the whole tree goes.
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(super::CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
fn kill_tree(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: Kind) -> Job {
        Job {
            id: 0,
            app_name: "Fortnite".into(),
            title: "Fortnite".into(),
            kind,
            state: State::Queued,
            phase: Phase::Preparing,
            progress: None,
            done: 0,
            total: 0,
            eta_secs: None,
            error: None,
            base_path: Some(PathBuf::from(r"D:\Games")),
            then_launch: false,
            finished_at: None,
        }
    }

    #[test]
    fn builds_non_interactive_command_lines() {
        assert_eq!(
            job(Kind::Install).args(),
            ["-y", "install", "Fortnite", "--skip-sdl", "--with-dlcs", "--base-path", r"D:\Games"]
        );
        assert_eq!(job(Kind::Update).args(), ["-y", "install", "Fortnite", "--update-only", "--skip-sdl"]);
        assert_eq!(job(Kind::Repair).args(), ["-y", "repair", "Fortnite"]);
        assert_eq!(job(Kind::Uninstall).args(), ["-y", "uninstall", "Fortnite"]);
        assert_eq!(job(Kind::Import).args(), ["-y", "egl-sync", "--import-only", "--one-shot"]);
    }

    #[test]
    fn follows_progress_lines() {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        JOBS.lock().unwrap().push(Job { id, state: State::Running, ..job(Kind::Install) });
        for line in [
            "[cli] INFO: Download size: 100.00 MiB (Compression savings: 10.0%)",
            "[DLManager] INFO: = Progress: 25.00% (1/4), Running for 00:00:10, ETA: 00:00:30",
            "[DLManager] INFO:  - Downloaded: 25.00 MiB, Written: 30.00 MiB",
        ] {
            apply(id, parse::parse_line(line));
        }
        let j = JOBS.lock().unwrap().iter().find(|j| j.id == id).cloned().unwrap();
        assert_eq!((j.done, j.total), (25 * 1024 * 1024, 100 * 1024 * 1024));
        assert_eq!((j.progress, j.eta_secs, j.phase), (Some(0.25), Some(30), Phase::Downloading));
        let update = updates().into_iter().find(|u| u.game_id == "epic:Fortnite").unwrap();
        assert_eq!(update.state, UpdateState::Downloading);
        assert_eq!(update.progress, Some(0.25));
        JOBS.lock().unwrap().retain(|j| j.id != id);
    }

    #[test]
    fn explains_common_errors() {
        assert_eq!(friendly(Some("Not enough available disk space!")), "Spazio insufficiente sull'unità scelta");
        assert!(friendly(Some("Something odd")).starts_with("legendary: "));
        assert!(!friendly(None).is_empty());
    }
}
