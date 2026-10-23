//! Windows Update as seen from the couch. GameMode only *reads* what Windows
//! Update already knows (pending updates, staged downloads, a reboot waiting)
//! and offers "update and restart / shut down". Downloading and installing
//! need an elevated token, which the shell of a signed-in user never has, so
//! those stay with Windows itself (the Update Orchestrator downloads on its
//! own in the background).
//!
//! The WUA COM API (`wuapi.dll`, served by `wuauserv`) answers from a local
//! cache instantly and goes online only when asked; an online search can take
//! minutes, so every call runs on its own thread and the result is cached.

use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Event with the latest `SystemUpdates`, sent when the state changes.
pub const EVENT: &str = "system://update";
/// How often Windows Update is asked online for new updates.
const ONLINE_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// First look (from the local cache) a moment after startup, so the library
/// scan and the stores come first.
const FIRST_OFFLINE_AFTER: Duration = Duration::from_secs(20);
/// The first online search waits for the network and the startup rush.
const FIRST_ONLINE_AFTER: Duration = Duration::from_secs(3 * 60);

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SystemUpdateState {
    /// Nothing pending.
    None,
    /// Found; Windows downloads it on its own, nothing to do yet.
    Available,
    /// Everything pending is downloaded: a restart installs it.
    Ready,
    /// Installed already, Windows just needs a restart.
    RebootRequired,
    /// Windows Update didn't answer (service disabled, offline, timeout).
    Unknown,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SystemUpdateItem {
    pub title: String,
    /// Download size in bytes, 0 when Windows doesn't say.
    pub bytes: u64,
    pub downloaded: bool,
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SystemUpdates {
    pub state: SystemUpdateState,
    pub items: Vec<SystemUpdateItem>,
    /// Unix seconds of the last answer from Windows Update.
    pub checked_at: Option<i64>,
    /// Why the state is `Unknown`, for the settings page.
    pub error: Option<String>,
}

impl SystemUpdates {
    fn unknown(error: String) -> Self {
        SystemUpdates { state: SystemUpdateState::Unknown, items: Vec::new(), checked_at: None, error: Some(error) }
    }
}

/// The state Windows Update is in, from what it reports.
fn classify(reboot_required: bool, items: &[SystemUpdateItem]) -> SystemUpdateState {
    if reboot_required {
        SystemUpdateState::RebootRequired
    } else if items.is_empty() {
        SystemUpdateState::None
    } else if items.iter().all(|i| i.downloaded) {
        SystemUpdateState::Ready
    } else {
        SystemUpdateState::Available
    }
}

static CACHE: Mutex<Option<SystemUpdates>> = Mutex::new(None);

/// The last known state, if Windows Update has been asked already.
pub fn cached() -> Option<SystemUpdates> {
    CACHE.lock().unwrap().clone()
}

/// The answer from a search, given whether a reboot is already pending. A
/// pending reboot is worth more than a failed search: a restart installs it
/// whatever the network says.
fn from_search(reboot_required: bool, search: Result<Vec<SystemUpdateItem>>, now: i64) -> Result<SystemUpdates> {
    match search {
        Ok(items) => Ok(SystemUpdates {
            state: classify(reboot_required, &items),
            items,
            checked_at: Some(now),
            error: None,
        }),
        Err(err) if reboot_required => {
            eprintln!("windows update: search failed, reboot pending anyway: {err:#}");
            Ok(SystemUpdates {
                state: SystemUpdateState::RebootRequired,
                items: Vec::new(),
                checked_at: Some(now),
                error: Some(err.to_string()),
            })
        }
        Err(err) => Err(err),
    }
}

/// What to show when Windows Update didn't answer at all: a staged update or
/// a pending reboot known before stays (a network hiccup doesn't undo it),
/// anything else becomes `Unknown`.
fn after_failure(previous: Option<SystemUpdates>, error: String) -> SystemUpdates {
    match previous {
        Some(mut p) if matches!(p.state, SystemUpdateState::Ready | SystemUpdateState::RebootRequired) => {
            p.error = Some(error);
            p
        }
        _ => SystemUpdates::unknown(error),
    }
}

/// Asks Windows Update (online: also the Microsoft servers, slow) and caches
/// the answer. Never fails: an online search that fails falls back to WUA's
/// local cache, and when that fails too the last good state is kept with the
/// reason (or `Unknown`, see `after_failure`).
pub fn check(online: bool) -> SystemUpdates {
    let mut answer = imp::query(online);
    if online {
        if let Err(err) = &answer {
            eprintln!("windows update: online search failed, using the local cache: {err:#}");
            let reason = err.to_string();
            answer = imp::query(false).map(|mut s| {
                s.error.get_or_insert(reason);
                s
            });
        }
    }
    let status = match answer {
        Ok(s) => s,
        Err(err) => {
            eprintln!("windows update: {err:#}");
            after_failure(cached(), err.to_string())
        }
    };
    *CACHE.lock().unwrap() = Some(status.clone());
    status
}

/// Like `check`, and tells the UI when something changed.
pub fn refresh(app: &AppHandle, online: bool) -> SystemUpdates {
    let before = cached();
    let now = check(online);
    if before.as_ref() != Some(&now) {
        let _ = app.emit(EVENT, &now);
    }
    now
}

/// Background watcher: a quick look at the local cache soon after startup,
/// then an online search every few hours.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        std::thread::sleep(FIRST_OFFLINE_AFTER);
        refresh(&app, false);
        std::thread::sleep(FIRST_ONLINE_AFTER.saturating_sub(FIRST_OFFLINE_AFTER));
        loop {
            refresh(&app, true);
            std::thread::sleep(ONLINE_EVERY);
        }
    });
}

/// Restarts (or powers off) Windows installing the pending updates on the
/// way, like "Update and restart" in the Start menu.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shutdown(restart: bool) -> Result<()> {
    imp::shutdown(restart)
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result, anyhow};
    use windows::Win32::Foundation::{CloseHandle, DECIMAL, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::Shutdown::{
        InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OPERATINGSYSTEM, SHTDN_REASON_MINOR_UPGRADE,
        SHUTDOWN_INSTALL_UPDATES, SHUTDOWN_POWEROFF, SHUTDOWN_RESTART,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::Win32::System::UpdateAgent::{
        ISystemInformation, IUpdateSession, SystemInformation, UpdateSession, orcSucceeded, orcSucceededWithErrors,
        utDriver,
    };
    use windows::core::BSTR;

    use super::{SystemUpdateItem, SystemUpdates, from_search};

    /// Pending, visible, non-optional software updates. Drivers are left to
    /// the vendors (Windows 11 treats them as optional and WU's GPU drivers
    /// lag behind); optional ones (BrowseOnly: previews, feature upgrade
    /// offers) are never downloaded on their own, so they'd keep the state at
    /// `Available` forever.
    const CRITERIA: &str = "IsInstalled=0 and IsHidden=0 and Type='Software' and BrowseOnly=0";

    pub fn query(online: bool) -> Result<SystemUpdates> {
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let info: ISystemInformation =
                CoCreateInstance(&SystemInformation, None, CLSCTX_INPROC_SERVER).context("Windows Update non raggiungibile")?;
            let reboot_required = info.RebootRequired().map(|b| b.as_bool()).unwrap_or(false);
            from_search(reboot_required, search(online), crate::model::now())
        }
    }

    unsafe fn search(online: bool) -> Result<Vec<SystemUpdateItem>> {
        unsafe {
            let session: IUpdateSession =
                CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER).context("Windows Update non raggiungibile")?;
            let _ = session.SetClientApplicationID(&BSTR::from("GameMode"));
            let searcher = session.CreateUpdateSearcher()?;
            searcher.SetOnline(online.into())?;
            let result = searcher.Search(&BSTR::from(CRITERIA)).context("ricerca non riuscita")?;
            let code = result.ResultCode()?;
            if code != orcSucceeded && code != orcSucceededWithErrors {
                return Err(anyhow!("ricerca interrotta (codice {})", code.0));
            }
            let updates = result.Updates()?;
            let mut items = Vec::new();
            for i in 0..updates.Count().unwrap_or(0) {
                let Ok(update) = updates.get_Item(i) else { continue };
                if update.Type().map(|t| t == utDriver).unwrap_or(false) {
                    continue;
                }
                items.push(SystemUpdateItem {
                    title: update.Title().map(|t| t.to_string()).unwrap_or_default(),
                    bytes: update.MaxDownloadSize().map(decimal_to_u64).unwrap_or(0),
                    downloaded: update.IsDownloaded().map(|b| b.as_bool()).unwrap_or(false),
                });
            }
            Ok(items)
        }
    }

    /// WUA sizes are integral DECIMALs (scale 0, positive): the low 64 bits
    /// hold anything a download could be.
    fn decimal_to_u64(d: DECIMAL) -> u64 {
        unsafe {
            if d.Anonymous1.Anonymous.sign != 0 || d.Anonymous1.Anonymous.scale != 0 || d.Hi32 != 0 {
                return 0;
            }
            d.Anonymous2.Lo64
        }
    }

    pub fn shutdown(restart: bool) -> Result<()> {
        let mode = if restart { SHUTDOWN_RESTART } else { SHUTDOWN_POWEROFF };
        let flags = mode | SHUTDOWN_INSTALL_UPDATES;
        let reason = SHTDN_REASON_MAJOR_OPERATINGSYSTEM | SHTDN_REASON_MINOR_UPGRADE | SHTDN_REASON_FLAG_PLANNED;
        let code = unsafe {
            // Without the privilege InitiateShutdown fails and shutdown.exe
            // below still gets its turn.
            if let Err(err) = enable_shutdown_privilege() {
                eprintln!("windows update: cannot enable SeShutdownPrivilege: {err:#}");
            }
            InitiateShutdownW(None, None, 0, flags, reason)
        };
        if code == 0 {
            return Ok(());
        }
        // Plan B: a plain restart still finishes what is already installed.
        eprintln!("windows update: InitiateShutdown failed with {code}, falling back to shutdown.exe");
        std::process::Command::new("shutdown").args([if restart { "/r" } else { "/s" }, "/t", "0"]).spawn()?;
        Ok(())
    }

    /// The signed-in user holds SeShutdownPrivilege but it starts disabled.
    unsafe fn enable_shutdown_privilege() -> Result<()> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
            let adjusted = (|| -> Result<()> {
                let mut luid = LUID::default();
                LookupPrivilegeValueW(None, SE_SHUTDOWN_NAME, &mut luid)?;
                let privileges = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None)?;
                Ok(())
            })();
            let _ = CloseHandle(token);
            adjusted
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    use super::{SystemUpdates, from_search};

    pub fn query(_online: bool) -> Result<SystemUpdates> {
        from_search(false, Ok(Vec::new()), crate::model::now())
    }

    #[allow(dead_code)]
    pub fn shutdown(restart: bool) -> Result<()> {
        eprintln!("update and {} ignored outside Windows", if restart { "restart" } else { "shutdown" });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(downloaded: bool) -> SystemUpdateItem {
        SystemUpdateItem { title: "KB".into(), bytes: 1, downloaded }
    }

    #[test]
    fn nothing_pending() {
        assert_eq!(classify(false, &[]), SystemUpdateState::None);
    }

    #[test]
    fn reboot_wins_over_everything() {
        assert_eq!(classify(true, &[]), SystemUpdateState::RebootRequired);
        assert_eq!(classify(true, &[item(false)]), SystemUpdateState::RebootRequired);
    }

    #[test]
    fn ready_only_when_all_downloaded() {
        assert_eq!(classify(false, &[item(true), item(true)]), SystemUpdateState::Ready);
        assert_eq!(classify(false, &[item(true), item(false)]), SystemUpdateState::Available);
        assert_eq!(classify(false, &[item(false)]), SystemUpdateState::Available);
    }

    fn ready() -> SystemUpdates {
        from_search(false, Ok(vec![item(true)]), 1).unwrap()
    }

    #[test]
    fn failed_search_keeps_a_pending_reboot() {
        let s = from_search(true, Err(anyhow::anyhow!("offline")), 7).unwrap();
        assert_eq!(s.state, SystemUpdateState::RebootRequired);
        assert_eq!(s.checked_at, Some(7));
        assert!(from_search(false, Err(anyhow::anyhow!("offline")), 7).is_err());
    }

    #[test]
    fn failure_keeps_a_staged_update() {
        let s = after_failure(Some(ready()), "offline".into());
        assert_eq!(s.state, SystemUpdateState::Ready);
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.error.as_deref(), Some("offline"));
    }

    #[test]
    fn failure_without_anything_staged_is_unknown() {
        assert_eq!(after_failure(None, "offline".into()).state, SystemUpdateState::Unknown);
        let none = from_search(false, Ok(Vec::new()), 1).unwrap();
        assert_eq!(after_failure(Some(none), "offline".into()).state, SystemUpdateState::Unknown);
    }

    #[test]
    fn trouble_is_unknown_with_a_reason() {
        let s = SystemUpdates::unknown("offline".into());
        assert_eq!(s.state, SystemUpdateState::Unknown);
        assert_eq!(s.error.as_deref(), Some("offline"));
        assert!(s.items.is_empty());
    }
}
