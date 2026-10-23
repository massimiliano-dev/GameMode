//! Diagnostics: a persistent log file, the panic hook and the crash record
//! the console-mode watchdog keeps to decide between restarting GameMode
//! and falling back to safe mode. Everything here uses `std::fs` only, so
//! it works in the watchdog process too, which never starts Tauri.
//!
//! Files (same folders Tauri uses for the app):
//! - `%LOCALAPPDATA%\it.maxpolce.gamemode\logs\gamemode.log` (+ `.1`): the log,
//!   written by GameMode and by its watchdog;
//! - `%APPDATA%\it.maxpolce.gamemode\crash.json`: the crashes the watchdog saw;
//! - `%APPDATA%\it.maxpolce.gamemode\last-crash.txt`: the last panic, as the
//!   panic hook wrote it (message, location, backtrace).

// The crash record is only driven by the Windows watchdog.
#![cfg_attr(not(windows), allow(dead_code))]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// Bundle identifier from tauri.conf.json: the data folders are named after it.
const IDENTIFIER: &str = "it.maxpolce.gamemode";
const LOG_FILE: &str = "gamemode.log";
const OLD_LOG_FILE: &str = "gamemode.log.1";
const CRASH_FILE: &str = "crash.json";
const LAST_PANIC_FILE: &str = "last-crash.txt";
/// The log rotates past this size; one older copy is kept.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// Crashes closer than this count together toward safe mode.
pub const CRASH_WINDOW_SECS: i64 = 300;
/// Restarts GameMode gets within the window before safe mode; each waits
/// the matching number of seconds first.
pub const BACKOFF_SECS: [u64; 3] = [2, 5, 10];
/// Exit code the watchdog gives a GameMode it had to terminate for hanging.
pub const HUNG_EXIT_CODE: u32 = 0xE000_0001;
/// Crashes kept in `crash.json`.
const KEPT_CRASHES: usize = 20;

pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join(IDENTIFIER)
}

pub fn log_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join(IDENTIFIER).join("logs")
}

pub fn log_path() -> PathBuf {
    log_dir().join(LOG_FILE)
}

fn crash_path() -> PathBuf {
    data_dir().join(CRASH_FILE)
}

fn last_panic_path() -> PathBuf {
    data_dir().join(LAST_PANIC_FILE)
}

// ---------------------------------------------------------------- logging

/// `log` facade backend: one line per record, appended to the log file
/// (and to stderr in debug builds). Opening the file for every record keeps
/// GameMode and its watchdog from fighting over a handle.
struct FileLogger {
    lock: Mutex<()>,
}

static LOGGER: FileLogger = FileLogger { lock: Mutex::new(()) };
/// "app" or "watchdog": which process wrote the line.
static TAG: OnceLock<&'static str> = OnceLock::new();

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // Our own messages in full; dependencies only when something is wrong.
        let ours = metadata.target().starts_with("gamemode_lib");
        metadata.level() <= if ours { log::max_level() } else { log::LevelFilter::Warn }
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let target = record.target().strip_prefix("gamemode_lib::").unwrap_or(record.target());
        let line = format!(
            "{} [{}] {:<5} {}: {}\n",
            format_time(crate::model::now()),
            TAG.get().copied().unwrap_or("app"),
            record.level(),
            target,
            record.args()
        );
        #[cfg(debug_assertions)]
        eprint!("{line}");
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        append(&line);
    }

    fn flush(&self) {}
}

fn append(text: &str) {
    let path = log_path();
    let _ = fs::create_dir_all(log_dir());
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(text.as_bytes());
        if file.metadata().is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            drop(file);
            let _ = fs::rename(&path, log_dir().join(OLD_LOG_FILE));
        }
    }
}

/// Installs the file logger; `tag` tells the two processes apart in the
/// shared file ("app" or "watchdog").
pub fn init(tag: &'static str) {
    let level = if cfg!(debug_assertions) { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    let _ = TAG.set(tag);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

/// Writes the last panic to `last-crash.txt` and the log before the process
/// aborts (`panic = "abort"`: the hook still runs first). Nothing here waits
/// on the logger, so a panic inside it can't deadlock.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info.payload());
        let location = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let text = format!("{} panic at {location}: {message}\n{backtrace}\n", format_time(crate::model::now()));
        let _ = fs::create_dir_all(data_dir());
        let _ = fs::write(last_panic_path(), &text);
        append(&format!("{} [app] PANIC {location}: {message}\n", format_time(crate::model::now())));
        previous(info);
    }));
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

/// Windows shows "GameMode has stopped working" on a crash and keeps the
/// process around until someone clicks: in console mode nobody can, and
/// the watchdog would wait forever. Ask for a silent crash instead.
#[cfg(windows)]
pub fn quiet_crash_dialogs() {
    use windows::Win32::System::Diagnostics::Debug::{SetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX};
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
}

#[cfg(not(windows))]
pub fn quiet_crash_dialogs() {}

/// `YYYY-MM-DD HH:MM:SS` in UTC from Unix seconds (no timezone crate needed).
pub fn format_time(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ------------------------------------------------------------ crash record

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Crash {
    /// Unix seconds when the watchdog saw GameMode die.
    pub at: i64,
    pub exit_code: u32,
    /// The watchdog terminated GameMode because its window stopped answering.
    #[serde(default)]
    pub hung: bool,
}

impl Crash {
    /// What the exit code means, for people.
    pub fn description(&self) -> String {
        describe_exit(self.exit_code, self.hung)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CrashLog {
    pub crashes: Vec<Crash>,
}

/// What the watchdog does after a crash.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Recovery {
    /// Start GameMode again after `delay_secs`; `attempt` counts from 1.
    Restart { attempt: u32, delay_secs: u64 },
    /// Too many crashes in a row: start Explorer and GameMode in safe mode.
    SafeMode,
}

impl CrashLog {
    pub fn record(&mut self, crash: Crash) {
        self.crashes.push(crash);
        if self.crashes.len() > KEPT_CRASHES {
            self.crashes.drain(..self.crashes.len() - KEPT_CRASHES);
        }
    }

    /// Crashes within the last `CRASH_WINDOW_SECS` before `now`.
    pub fn recent(&self, now: i64) -> usize {
        self.crashes.iter().filter(|c| now - c.at < CRASH_WINDOW_SECS && c.at <= now).count()
    }

    pub fn last(&self) -> Option<&Crash> {
        self.crashes.last()
    }

    /// The recovery for a crash just recorded at `now`.
    pub fn plan(&self, now: i64) -> Recovery {
        plan_for(self.recent(now))
    }
}

/// Restarts with a growing wait; past `BACKOFF_SECS.len()` crashes in the
/// window, safe mode. `recent` includes the crash being handled.
pub fn plan_for(recent: usize) -> Recovery {
    match BACKOFF_SECS.get(recent.saturating_sub(1)) {
        Some(&delay_secs) if recent > 0 => Recovery::Restart { attempt: recent as u32, delay_secs },
        _ => Recovery::SafeMode,
    }
}

/// `app.exit(0)`, `quit_app` and the updater all leave with 0; anything
/// else is a crash. Note that "End task" and `taskkill` leave with 1 and are
/// treated as crashes too: in console mode a restart is what the user wants.
pub fn is_clean_exit(code: u32) -> bool {
    code == 0
}

pub fn describe_exit(code: u32, hung: bool) -> String {
    if hung {
        return "GameMode non rispondeva più ed è stato chiuso".into();
    }
    match code {
        0xC000_0409 => "Errore interno (panic)".into(),
        0xC000_0005 => "Accesso alla memoria non valido".into(),
        0xC000_013A => "Chiuso dal sistema".into(),
        0xC000_0135 | 0xC000_007B => "Libreria di sistema mancante o danneggiata".into(),
        0xC00D_36B4 | 0x8007_000E | 0xC000_0017 => "Memoria esaurita".into(),
        1 => "Terminato (Gestione attività o taskkill)".into(),
        HUNG_EXIT_CODE => "GameMode non rispondeva più ed è stato chiuso".into(),
        code if code >= 0xC000_0000 => format!("Errore di Windows 0x{code:08X}"),
        code => format!("Uscita con codice {code}"),
    }
}

fn read_crashes_from(path: &Path) -> CrashLog {
    fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_crashes_to(path: &Path, log: &CrashLog) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(log) {
        let _ = fs::write(path, json);
    }
}

pub fn crashes() -> CrashLog {
    read_crashes_from(&crash_path())
}

/// Records a crash seen now and says what to do about it.
pub fn record_crash(exit_code: u32, hung: bool) -> Recovery {
    let now = crate::model::now();
    let path = crash_path();
    let mut log = read_crashes_from(&path);
    log.record(Crash { at: now, exit_code, hung });
    write_crashes_to(&path, &log);
    log.plan(now)
}

/// Forgets the recorded crashes and the last panic.
pub fn clear_crashes() {
    let _ = fs::remove_file(crash_path());
    let _ = fs::remove_file(last_panic_path());
}

// ------------------------------------------------------------ for the UI

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StartupInfo {
    /// How many times in a row the watchdog restarted GameMode (0: a normal start).
    pub recovered: u32,
    pub safe_mode: bool,
    pub last_crash: Option<Crash>,
    /// `last_crash.description()`, ready to show.
    pub last_crash_description: Option<String>,
    /// First lines of the last panic (message and location).
    pub last_panic: Option<String>,
    /// Crashes recorded in all (the last `KEPT_CRASHES`).
    pub crash_count: usize,
    pub log_path: String,
}

pub fn startup_info(recovered: u32, safe_mode: bool) -> StartupInfo {
    let log = crashes();
    let last_crash = log.last().cloned();
    let last_panic = fs::read_to_string(last_panic_path()).ok().map(|t| t.lines().take(3).collect::<Vec<_>>().join("\n"));
    StartupInfo {
        recovered,
        safe_mode,
        last_crash_description: last_crash.as_ref().map(Crash::description),
        last_crash,
        last_panic,
        crash_count: log.crashes.len(),
        log_path: log_path().display().to_string(),
    }
}

/// The last `lines` lines of the log.
pub fn read_log(lines: usize) -> String {
    let text = fs::read_to_string(log_path()).unwrap_or_default();
    tail(&text, lines)
}

pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Copies the whole log (older part first) and the last panic to `dest`.
pub fn export_to(dest: &Path) -> std::io::Result<()> {
    let mut out = String::new();
    for name in [OLD_LOG_FILE, LOG_FILE] {
        if let Ok(text) = fs::read_to_string(log_dir().join(name)) {
            out.push_str(&text);
        }
    }
    if let Ok(panic) = fs::read_to_string(last_panic_path()) {
        out.push_str("\n--- last panic ---\n");
        out.push_str(&panic);
    }
    fs::write(dest, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_times() {
        assert_eq!(format_time(0), "1970-01-01 00:00:00");
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13:20");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00:00");
    }

    #[test]
    fn restarts_with_backoff_then_safe_mode() {
        assert_eq!(plan_for(0), Recovery::SafeMode);
        assert_eq!(plan_for(1), Recovery::Restart { attempt: 1, delay_secs: 2 });
        assert_eq!(plan_for(2), Recovery::Restart { attempt: 2, delay_secs: 5 });
        assert_eq!(plan_for(3), Recovery::Restart { attempt: 3, delay_secs: 10 });
        assert_eq!(plan_for(4), Recovery::SafeMode);
    }

    #[test]
    fn only_recent_crashes_count() {
        let mut log = CrashLog::default();
        log.record(Crash { at: 1000, exit_code: 1, hung: false });
        log.record(Crash { at: 1000 + CRASH_WINDOW_SECS + 10, exit_code: 1, hung: false });
        let now = 1000 + CRASH_WINDOW_SECS + 20;
        assert_eq!(log.recent(now), 1);
        assert_eq!(log.plan(now), Recovery::Restart { attempt: 1, delay_secs: 2 });
        for i in 0..3 {
            log.record(Crash { at: now + i, exit_code: 0xC000_0409, hung: false });
        }
        assert_eq!(log.recent(now + 3), 4);
        assert_eq!(log.plan(now + 3), Recovery::SafeMode);
        // A crash in the future (clock set back) doesn't count.
        assert_eq!(log.recent(0), 0);
    }

    #[test]
    fn keeps_the_last_crashes_only() {
        let mut log = CrashLog::default();
        for i in 0..(KEPT_CRASHES as i64 + 5) {
            log.record(Crash { at: i, exit_code: 1, hung: false });
        }
        assert_eq!(log.crashes.len(), KEPT_CRASHES);
        assert_eq!(log.last().unwrap().at, KEPT_CRASHES as i64 + 4);
    }

    #[test]
    fn describes_exit_codes() {
        assert!(is_clean_exit(0));
        assert!(!is_clean_exit(1));
        assert_eq!(describe_exit(0xC000_0409, false), "Errore interno (panic)");
        assert_eq!(describe_exit(0xC000_0999, false), "Errore di Windows 0xC0000999");
        assert_eq!(describe_exit(3, false), "Uscita con codice 3");
        assert!(describe_exit(5, true).contains("non rispondeva"));
        assert!(describe_exit(HUNG_EXIT_CODE, false).contains("non rispondeva"));
    }

    #[test]
    fn tails_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("a\nb", 10), "a\nb");
        assert_eq!(tail("", 3), "");
    }

    #[test]
    fn crash_log_roundtrips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("gamemode-diag-{}", std::process::id()));
        let path = dir.join(CRASH_FILE);
        let mut log = CrashLog::default();
        log.record(Crash { at: 42, exit_code: 0xC000_0005, hung: true });
        write_crashes_to(&path, &log);
        assert_eq!(read_crashes_from(&path), log);
        assert_eq!(read_crashes_from(&dir.join("missing.json")), CrashLog::default());
        let _ = fs::remove_dir_all(dir);
    }
}
