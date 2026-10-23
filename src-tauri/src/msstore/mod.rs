//! Microsoft Store / PC Game Pass installs and updates without the Xbox app
//! or the Store window: the Store's own install service through
//! `Windows.ApplicationModel.Store.Preview.InstallControl.AppInstallManager`.
//!
//! Microsoft documents the class as needing a capability reserved to its
//! own *packaged* apps; unpackaged full-trust processes are not subject to
//! capability checks, and Chromium's (unpackaged) browser process calls
//! `StartProductInstallAsync` the same way to install the Windows App
//! Runtime. Every failure falls back to the old hand-off (Xbox app / Store),
//! and Settings can turn the whole thing off (`msstore_direct`).

#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(windows)]
#[allow(clippy::all, unused)]
mod bindings;

use anyhow::Result;
use tauri::AppHandle;

use crate::model::Game;
use crate::updates::UpdateState;

/// Where an install item is, as GameMode shows it.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Phase {
    Active(UpdateState),
    Completed,
    Canceled,
    Failed,
}

/// `AppInstallState` values.
pub fn phase(state: i32) -> Phase {
    match state {
        // Pending, Starting, AcquiringLicense, Downloading, ReadyToDownload
        0..=3 | 13 => Phase::Active(UpdateState::Downloading),
        // RestoringData, Installing
        4 | 5 => Phase::Active(UpdateState::Installing),
        6 => Phase::Completed,
        7 => Phase::Canceled,
        // Paused, PausedLowBattery, PausedWiFiRecommended, PausedWiFiRequired
        8 | 10..=12 => Phase::Active(UpdateState::Paused),
        _ => Phase::Failed,
    }
}

pub fn supported() -> bool {
    cfg!(windows)
}

/// Starts installing a Store product (`9NBLGGH4R315`-style id). Blocking:
/// returns once the Store has queued it, `Err` when it refused it.
pub fn install(app: &AppHandle, product: &str, name: &str) -> Result<()> {
    if product.is_empty() || !product.chars().all(|c| c.is_ascii_alphanumeric()) {
        anyhow::bail!("Prodotto non valido");
    }
    imp::install(app, product, name)
}

/// Starts updating an installed Store game. `Ok(false)`: no update.
pub fn update(app: &AppHandle, game: &Game) -> Result<bool> {
    let Some(family) = game.id.strip_prefix("xbox:") else { anyhow::bail!("Non è un gioco del Microsoft Store") };
    imp::update(app, &game.id, family, &game.name)
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use anyhow::{bail, Result};
    use tauri::AppHandle;
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    use super::bindings::{AppInstallItem, AppInstallManager};
    use super::{phase, Phase};
    use crate::direct;
    use crate::updates::UpdateState;

    /// How long to wait for the Store to accept or refuse an install
    /// before calling it started.
    const SETTLE: Duration = Duration::from_secs(8);
    const POLL: Duration = Duration::from_secs(1);

    fn com() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    pub fn install(app: &AppHandle, product: &str, name: &str) -> Result<()> {
        com();
        let job = format!("gamepass:{product}");
        let Some(cancel) = direct::begin(&job) else { bail!("{name} si sta già installando") };
        let started = (|| -> Result<(AppInstallManager, Vec<AppInstallItem>)> {
            let manager = AppInstallManager::new()?;
            let empty = HSTRING::new();
            let items = manager
                .StartProductInstallAsync(&HSTRING::from(product), &empty, &empty, &empty, false, false, &empty, None)?
                .join()?;
            Ok((manager, items.into_iter().collect()))
        })();
        let (manager, items) = match started {
            Ok(ok) => ok,
            Err(err) => {
                direct::finish(&job);
                return Err(err);
            }
        };
        let Some(item) = main_item(&items, product) else {
            direct::finish(&job);
            bail!("Il Microsoft Store non ha accodato {name}");
        };
        settle_then_watch(app, manager, item, job, name.to_string(), cancel)
    }

    pub fn update(app: &AppHandle, job: &str, family: &str, name: &str) -> Result<bool> {
        com();
        let Some(cancel) = direct::begin(job) else { return Ok(true) };
        let manager = match AppInstallManager::new() {
            Ok(m) => m,
            Err(err) => {
                direct::finish(job);
                return Err(err.into());
            }
        };
        // No update: the operation completes with no item.
        let item = manager.UpdateAppByPackageFamilyNameAsync(&HSTRING::from(family)).and_then(|op| op.join());
        match item {
            Ok(item) => settle_then_watch(app, manager, item, job.to_string(), name.to_string(), cancel).map(|_| true),
            Err(err) => {
                direct::finish(job);
                if err.code().is_ok() {
                    Ok(false)
                } else {
                    Err(err.into())
                }
            }
        }
    }

    fn main_item(items: &[AppInstallItem], product: &str) -> Option<AppInstallItem> {
        items
            .iter()
            .find(|i| i.ProductId().is_ok_and(|id| id.to_string().eq_ignore_ascii_case(product)))
            .or_else(|| items.first())
            .cloned()
    }

    /// Gives the Store a few seconds to refuse (no entitlement, blocked by
    /// policy…) so the caller can fall back, then follows it in a thread.
    fn settle_then_watch(
        app: &AppHandle,
        manager: AppInstallManager,
        item: AppInstallItem,
        job: String,
        name: String,
        cancel: Arc<AtomicBool>,
    ) -> Result<()> {
        let since = Instant::now();
        while since.elapsed() < SETTLE {
            let status = item.GetCurrentStatus()?;
            match phase(status.InstallState()?.0) {
                Phase::Failed => {
                    let code = status.ErrorCode().map(|c| c.0).unwrap_or(0);
                    direct::finish(&job);
                    bail!("Il Microsoft Store ha rifiutato {name} ({code:#x})");
                }
                Phase::Canceled => {
                    direct::finish(&job);
                    bail!("Il Microsoft Store ha annullato {name}");
                }
                Phase::Completed => break,
                Phase::Active(UpdateState::Downloading) if status.PercentComplete().unwrap_or(0.0) <= 0.0 => {}
                Phase::Active(_) => break,
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let app = app.clone();
        std::thread::spawn(move || watch(app, manager, item, job, name, cancel));
        Ok(())
    }

    fn watch(app: AppHandle, manager: AppInstallManager, item: AppInstallItem, job: String, name: String, cancel: Arc<AtomicBool>) {
        com();
        // The manager must outlive the install, or WinRT may drop it.
        let _manager = manager;
        let mut failures = 0;
        let mut cancel_sent = false;
        let outcome = loop {
            if cancel.load(Ordering::SeqCst) && !cancel_sent {
                cancel_sent = true;
                let _ = item.Cancel();
            }
            let status = match item.GetCurrentStatus() {
                Ok(s) => {
                    failures = 0;
                    s
                }
                Err(err) => {
                    failures += 1;
                    if failures > 10 {
                        log::warn!("Store install {job}: status unavailable ({err})");
                        break None;
                    }
                    std::thread::sleep(POLL);
                    continue;
                }
            };
            match status.InstallState().map(|s| phase(s.0)).unwrap_or(Phase::Failed) {
                Phase::Active(state) => {
                    let (done, total) = (status.BytesDownloaded().unwrap_or(0), status.DownloadSizeInBytes().unwrap_or(0));
                    if state == UpdateState::Downloading && total > 0 {
                        direct::set_bytes(&job, state, done, total);
                    } else {
                        let percent = status.PercentComplete().ok().filter(|p| *p > 0.0).map(|p| p / 100.0);
                        direct::set_percent(&job, state, percent);
                    }
                }
                Phase::Completed => break Some(Ok(())),
                Phase::Canceled => break Some(Err(None)),
                Phase::Failed => {
                    let code = status.ErrorCode().map(|c| c.0).unwrap_or(0);
                    break Some(Err(Some(format!("Il Microsoft Store non ha installato {name} ({code:#x})"))));
                }
            }
            std::thread::sleep(POLL);
        };
        direct::finish(&job);
        crate::scan(&app);
        let done = match outcome {
            Some(Ok(())) => direct::Done { id: job, name, ok: true, error: None },
            Some(Err(error)) => direct::Done { id: job, name, ok: false, error },
            None => return,
        };
        direct::announce(&app, done);
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{bail, Result};
    use tauri::AppHandle;

    pub fn install(_app: &AppHandle, _product: &str, _name: &str) -> Result<()> {
        bail!("Disponibile solo su Windows")
    }

    pub fn update(_app: &AppHandle, _job: &str, _family: &str, _name: &str) -> Result<bool> {
        bail!("Disponibile solo su Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_store_install_states() {
        assert_eq!(phase(0), Phase::Active(UpdateState::Downloading));
        assert_eq!(phase(3), Phase::Active(UpdateState::Downloading));
        assert_eq!(phase(5), Phase::Active(UpdateState::Installing));
        assert_eq!(phase(6), Phase::Completed);
        assert_eq!(phase(7), Phase::Canceled);
        assert_eq!(phase(9), Phase::Failed);
        assert_eq!(phase(11), Phase::Active(UpdateState::Paused));
        assert_eq!(phase(13), Phase::Active(UpdateState::Downloading));
        assert_eq!(phase(42), Phase::Failed);
    }
}
