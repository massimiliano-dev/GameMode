//! A pinned WebView2 runtime ("Fixed Version"), so that an Edge update of
//! the system WebView2 can't break or change GameMode's UI.
//!
//! The runtime is a ~310 MB cab (~670 MB unpacked). It is not part of the
//! installer: that would turn every ~3 MB update into a ~230 MB download and
//! have each update rewrite hundreds of runtime files in place. Instead
//! GameMode downloads it once per pinned version into
//! `%LOCALAPPDATA%\<identifier>\webview2-fixed\<version>`, checks the pinned
//! SHA-256, unpacks it with Windows' `expand.exe` and uses it from the next
//! start on. Until then, in safe mode, or if a start with it never got as far
//! as running the UI, the system WebView2 is used, as before.
//!
//! The fixed runtime gets no security patches from Windows: keep the pin
//! current. The weekly `.github/workflows/webview2-check.yml` opens an issue
//! with the newer version, its URL and its SHA-256; copy them into `VERSION`,
//! `URL` and `SHA256` below and release. Installed copies download the new
//! runtime in the background and drop the old one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// The pinned Fixed Version runtime, x64 (from the list behind
/// https://developer.microsoft.com/microsoft-edge/webview2#download).
pub const VERSION: &str = "156.0.4314.42";
#[cfg_attr(not(windows), allow(dead_code))]
const URL: &str = "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/074c0930-36ad-4756-9d04-dfe0e503bd94/Microsoft.WebView2.FixedVersionRuntime.156.0.4314.42.x64.cab";
#[cfg_attr(not(windows), allow(dead_code))]
const SHA256: &str = "cb4612ce79a62a0be094301a72f73cf91bb250f5e83dc51886f66a083fc0694f";

/// Read by the WebView2 loader; Tauri sets it from the config.
const ENV: &str = "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER";
/// Folder (under the local data dir) holding one subfolder per version.
const DIR: &str = "webview2-fixed";
const EXE: &str = "msedgewebview2.exe";
/// Markers inside a version folder.
const READY: &str = "gamemode-ready"; // unpacked and checked
const STARTING: &str = "gamemode-starting"; // a start that hasn't run the UI yet
const WORKS: &str = "gamemode-works"; // the UI ran on it at least once
const FAILED: &str = "gamemode-failed"; // never use this copy again

/// `webview2-fixed` in the local data dir, when GameMode manages a runtime.
static ROOT: OnceLock<PathBuf> = OnceLock::new();
/// The version folder this run uses, if any.
static ACTIVE: OnceLock<PathBuf> = OnceLock::new();
static CONFIRMED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct DirState {
    exe: bool,
    ready: bool,
    failed: bool,
    starting: bool,
    works: bool,
}

impl DirState {
    fn read(dir: &Path) -> Self {
        Self {
            exe: dir.join(EXE).is_file(),
            ready: dir.join(READY).is_file(),
            failed: dir.join(FAILED).is_file(),
            starting: dir.join(STARTING).is_file(),
            works: dir.join(WORKS).is_file(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Start on the fixed runtime.
    Use,
    /// The last start on it never ran the UI: mark it failed, use the system one.
    GiveUp,
    /// Use the system runtime, for the given reason.
    System(&'static str),
}

fn decide(state: DirState, safe_mode: bool) -> Decision {
    if safe_mode {
        Decision::System("safe mode")
    } else if !(state.exe && state.ready) {
        Decision::System("not downloaded yet")
    } else if state.failed {
        Decision::System("it failed to start before")
    } else if state.starting && !state.works {
        Decision::GiveUp
    } else {
        Decision::Use
    }
}

/// Whether `folder` (the value of the env var) is a runtime GameMode put
/// there, as opposed to one the user chose.
fn is_ours(folder: &Path) -> bool {
    folder.parent().and_then(Path::file_name).is_some_and(|n| n == DIR)
}

fn touch(path: &Path) {
    if let Err(err) = std::fs::write(path, b"") {
        log::warn!("WebView2: writing {}: {err}", path.display());
    }
}

/// Picks the WebView2 runtime for this run, before Tauri creates any
/// webview: points the config at the fixed runtime when it's ready and safe.
pub fn prepare<R: tauri::Runtime>(mut ctx: tauri::Context<R>) -> tauri::Context<R> {
    if !cfg!(all(windows, target_arch = "x86_64")) {
        return ctx;
    }
    match std::env::var_os(ENV) {
        // Inherited from a GameMode parent (the watchdog's relaunch): decide afresh.
        // SAFETY: called at startup, before Tauri or our own threads read the environment.
        Some(v) if is_ours(Path::new(&v)) => unsafe { std::env::remove_var(ENV) },
        Some(v) => {
            log::info!("WebView2: {ENV} set to {v:?} by the user, leaving it alone");
            return ctx;
        }
        None => {}
    }
    let Some(base) = dirs::data_local_dir() else { return ctx };
    let root = base.join(&ctx.config().identifier).join(DIR);
    let _ = ROOT.set(root.clone());
    let dir = root.join(VERSION);
    let state = DirState::read(&dir);
    match decide(state, crate::shell::safe_mode()) {
        Decision::Use => {
            if !state.works {
                touch(&dir.join(STARTING));
            }
            log::info!("WebView2: fixed runtime {VERSION}");
            // Tauri joins this to the exe's folder; an absolute path replaces it.
            ctx.config_mut().bundle.windows.webview_install_mode =
                tauri::utils::config::WebviewInstallMode::FixedRuntime { path: dir.clone() };
            let _ = ACTIVE.set(dir);
        }
        Decision::GiveUp => {
            log::warn!("WebView2: the fixed runtime {VERSION} never ran the UI, back to the system one");
            touch(&dir.join(FAILED));
            let _ = std::fs::remove_file(dir.join(STARTING));
        }
        Decision::System(why) => log::info!("WebView2: system runtime ({why})"),
    }
    ctx
}

/// The UI is running: the fixed runtime in use (if any) works.
pub fn confirm() {
    let Some(dir) = ACTIVE.get() else { return };
    if CONFIRMED.swap(true, Ordering::SeqCst) {
        return;
    }
    if !dir.join(WORKS).is_file() {
        touch(&dir.join(WORKS));
        log::info!("WebView2: fixed runtime {VERSION} works");
    }
    let _ = std::fs::remove_file(dir.join(STARTING));
}

/// Keeps programs GameMode starts (games, launchers, Explorer in console
/// mode) off its runtime: the env var is inherited, and a WebView2 app
/// started with it would run on GameMode's copy and lock its files.
pub fn scrub(cmd: &mut Command) -> &mut Command {
    if std::env::var_os(ENV).is_some_and(|v| is_ours(Path::new(&v))) {
        cmd.env_remove(ENV);
    }
    cmd
}

/// In the background, a while after startup: drops runtimes of other
/// versions and downloads the pinned one if it isn't there yet.
pub fn spawn_install() {
    let Some(root) = ROOT.get().cloned() else { return };
    tauri::async_runtime::spawn(async move {
        // Leave the network and the disk to the startup work first.
        tokio::time::sleep(std::time::Duration::from_secs(90)).await;
        if let Err(err) = imp::install(&root).await {
            log::warn!("WebView2: getting the fixed runtime {VERSION} failed: {err:#}");
        }
    });
}

/// Marks (in `webview2-fixed`) a pinned cab whose SHA-256 didn't match, so
/// it isn't downloaded again at every start.
#[cfg_attr(not(windows), allow(dead_code))]
fn rejected() -> String {
    format!("{VERSION}.rejected")
}

/// Entries of `webview2-fixed` that aren't the pinned version: older
/// runtimes and leftovers of an interrupted download.
#[cfg_attr(not(windows), allow(dead_code))]
fn stale(names: impl IntoIterator<Item = String>) -> Vec<String> {
    names.into_iter().filter(|n| n != VERSION && *n != rejected()).collect()
}

#[cfg_attr(not(windows), allow(dead_code))]
fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use anyhow::{Context as _, Result, bail, ensure};
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub async fn install(root: &Path) -> Result<()> {
        std::fs::create_dir_all(root)?;
        let names = std::fs::read_dir(root)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned());
        for name in stale(names) {
            let path = root.join(&name);
            // A runtime still in use by a process stays until a later start.
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
        let dir = root.join(VERSION);
        if dir.join(READY).is_file() {
            return Ok(());
        }
        if root.join(rejected()).is_file() {
            log::warn!("WebView2: the pinned runtime {VERSION} didn't match its SHA-256 before, not downloading it again");
            return Ok(());
        }
        if dir.exists() {
            std::fs::remove_dir_all(&dir).context("removing an incomplete runtime")?;
        }
        let cab = root.join(format!("{VERSION}.cab"));
        log::info!("WebView2: downloading the fixed runtime {VERSION}");
        let result = download(&cab).await;
        let result = match result {
            Ok(()) => {
                let (root, cab) = (root.to_path_buf(), cab.clone());
                tauri::async_runtime::spawn_blocking(move || unpack(&root, &cab)).await?
            }
            Err(err) => Err(err),
        };
        let _ = std::fs::remove_file(&cab);
        result?;
        log::info!("WebView2: fixed runtime {VERSION} ready, in use from the next start");
        Ok(())
    }

    async fn download(cab: &Path) -> Result<()> {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()?;
        let mut response = client.get(URL).send().await?.error_for_status()?;
        let mut file = std::fs::File::create(cab)?;
        let mut hasher = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
        }
        file.sync_all()?;
        let got = to_hex(&hasher.finalize());
        if got != SHA256 {
            if let Some(root) = cab.parent() {
                touch(&root.join(rejected()));
            }
            bail!("SHA-256 mismatch: got {got}, expected {SHA256}");
        }
        Ok(())
    }

    fn system32(exe: &str) -> PathBuf {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        Path::new(&root).join("System32").join(exe)
    }

    fn run(cmd: &mut Command) -> Result<()> {
        let status = cmd
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .with_context(|| format!("starting {cmd:?}"))?;
        ensure!(status.success(), "{cmd:?} exited with {status}");
        Ok(())
    }

    /// Unpacks the cab next to the final folder, then moves it into place,
    /// so a half-unpacked runtime is never picked up.
    fn unpack(root: &Path, cab: &Path) -> Result<()> {
        let staging = root.join(format!("{VERSION}.unpack"));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let result = (|| {
            // Microsoft's documented way: Explorer may not keep the folder layout.
            run(Command::new(system32("expand.exe")).arg(cab).arg("-F:*").arg(&staging))?;
            let inner = staging.join(format!("Microsoft.WebView2.FixedVersionRuntime.{VERSION}.x64"));
            ensure!(inner.join(EXE).is_file(), "{EXE} missing after unpacking");
            // Windows 10 runs the renderer in an AppContainer since runtime 120:
            // it needs read access (Microsoft's distribution docs, Fixed Version).
            for sid in ["*S-1-15-2-2", "*S-1-15-2-1"] {
                run(Command::new(system32("icacls.exe")).arg(&inner).arg("/grant").arg(format!("{sid}:(OI)(CI)(RX)")))?;
            }
            let dir = root.join(VERSION);
            // An antivirus still scanning the new files can hold the folder for a moment.
            let mut moved = std::fs::rename(&inner, &dir);
            for _ in 0..5 {
                if moved.is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
                moved = std::fs::rename(&inner, &dir);
            }
            moved.context("moving the runtime into place")?;
            std::fs::write(dir.join(READY), VERSION)?;
            Ok(())
        })();
        let _ = std::fs::remove_dir_all(&staging);
        result
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub async fn install(_root: &Path) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY_STATE: DirState = DirState { exe: true, ready: true, failed: false, starting: false, works: false };

    #[test]
    fn uses_a_ready_runtime() {
        assert_eq!(decide(READY_STATE, false), Decision::Use);
        assert_eq!(decide(DirState { works: true, starting: true, ..READY_STATE }, false), Decision::Use);
    }

    #[test]
    fn falls_back_to_the_system_runtime() {
        assert_eq!(decide(READY_STATE, true), Decision::System("safe mode"));
        assert_eq!(decide(DirState::default(), false), Decision::System("not downloaded yet"));
        assert_eq!(decide(DirState { exe: false, ..READY_STATE }, false), Decision::System("not downloaded yet"));
        assert_eq!(decide(DirState { ready: false, ..READY_STATE }, false), Decision::System("not downloaded yet"));
        assert_eq!(decide(DirState { failed: true, ..READY_STATE }, false), Decision::System("it failed to start before"));
    }

    #[test]
    fn gives_up_after_a_start_that_never_ran_the_ui() {
        assert_eq!(decide(DirState { starting: true, ..READY_STATE }, false), Decision::GiveUp);
    }

    #[test]
    fn recognizes_its_own_runtime() {
        #[cfg(windows)]
        assert!(is_ours(Path::new(r"C:\Users\me\AppData\Local\it.maxpolce.gamemode\webview2-fixed\156.0.4314.42")));
        assert!(is_ours(Path::new("/data/webview2-fixed/156.0.4314.42")));
        assert!(!is_ours(Path::new("/tools/WebView2")));
        assert!(!is_ours(Path::new("webview2-fixed")));
    }

    #[test]
    fn scrub_removes_only_our_runtime() {
        let mut cmd = Command::new("x");
        scrub(&mut cmd);
        // Nothing to remove when the variable isn't ours (or isn't set).
        assert!(cmd.get_envs().all(|(k, _)| k != std::ffi::OsStr::new(ENV)));
    }

    #[test]
    fn stale_keeps_only_the_pinned_version() {
        let names = vec![
            VERSION.to_string(),
            rejected(),
            "155.0.4283.62".into(),
            "155.0.4283.62.rejected".into(),
            format!("{VERSION}.cab"),
            format!("{VERSION}.unpack"),
        ];
        let expected = ["155.0.4283.62", "155.0.4283.62.rejected", &format!("{VERSION}.cab"), &format!("{VERSION}.unpack")];
        assert_eq!(stale(names), expected.map(String::from));
    }

    #[test]
    fn pin_is_consistent() {
        assert!(URL.starts_with("https://"));
        assert!(URL.ends_with(&format!("Microsoft.WebView2.FixedVersionRuntime.{VERSION}.x64.cab")));
        assert_eq!(SHA256.len(), 64);
        assert!(SHA256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(to_hex(&[0x00, 0xab, 0x0f]), "00ab0f");
    }
}
