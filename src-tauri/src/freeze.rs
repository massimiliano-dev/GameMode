//! Quick Resume: freezing a game's processes in place and thawing them later,
//! like a console suspends a game. `NtSuspendProcess` stops every thread of
//! a process, so a frozen game can't answer window messages: nothing here
//! sends it any (only asynchronous calls), and the launcher's monitor skips
//! window lookups while a game is frozen.
//!
//! Known limits: an elevated process or one guarded by an anti-cheat driver
//! can't be opened for suspension, and online games drop their connection
//! while frozen. Both are reported, not worked around.

use std::path::Path;

use anyhow::Result;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Helpers and services that belong to a game's process tree but must keep
/// running: freezing them hangs the store client or the anti-cheat service.
const KEEP_RUNNING: &[&str] = &["gamelaunchhelper.exe", "beservice.exe", "vgc.exe", "steamservice.exe", "gameoverlayui.exe"];

/// Folder and file names that mark an anti-cheat: suspending those games
/// gets the player kicked (or flagged), so it needs a confirmation first.
const ANTI_CHEAT_MARKERS: &[&str] = &["easyanticheat", "easyanticheat_eos", "battleye", "beservice.exe", "vgc.exe", "vanguard"];

/// Whether an entry name (folder or file, any case) marks an anti-cheat.
pub fn is_anti_cheat_marker(name: &str) -> bool {
    let name = name.to_lowercase();
    ANTI_CHEAT_MARKERS.iter().any(|m| name == *m || name.starts_with("easyanticheat"))
}

/// Looks for an anti-cheat in the game's install folder (three levels deep).
pub fn anti_cheat_in(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: u8) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        for entry in entries.flatten() {
            if is_anti_cheat_marker(&entry.file_name().to_string_lossy()) {
                return true;
            }
            if depth > 0 && entry.file_type().is_ok_and(|t| t.is_dir()) && walk(&entry.path(), depth - 1) {
                return true;
            }
        }
        false
    }
    walk(dir, 2)
}

/// The processes to freeze: `pids` plus their descendants, minus helpers
/// and services (session 0), in the same user session.
pub fn family(pids: &[u32]) -> Vec<u32> {
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let me = sys.process(Pid::from_u32(std::process::id())).and_then(|p| p.session_id());
    let mut out: Vec<u32> = pids.to_vec();
    let mut i = 0;
    while i < out.len() {
        let parent = Pid::from_u32(out[i]);
        for (pid, process) in sys.processes() {
            if process.parent() == Some(parent) && !out.contains(&pid.as_u32()) {
                out.push(pid.as_u32());
            }
        }
        i += 1;
    }
    out.retain(|pid| {
        sys.process(Pid::from_u32(*pid)).is_some_and(|p| {
            let name = p.name().to_string_lossy().to_lowercase();
            !KEEP_RUNNING.contains(&name.as_str()) && (me.is_none() || p.session_id() == me)
        })
    });
    out
}

/// Freezes `pids` (already expanded with `family`): the game window is
/// minimized out of the way, its audio muted, then every process stopped.
/// Fails without leaving anything frozen when a process can't be opened.
pub fn suspend(pids: &[u32], hwnd: Option<isize>) -> Result<()> {
    imp::suspend(pids, hwnd)
}

/// Thaws `pids` and unmutes them. Best effort: a process that is gone is skipped.
pub fn resume(pids: &[u32]) -> Result<()> {
    imp::resume(pids)
}

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};

    use anyhow::{anyhow, Context, Result};
    use windows::core::{s, w, PCSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, NTSTATUS};
    use windows::Win32::Media::Audio::{
        eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, ShowWindowAsync, SW_MINIMIZE, SW_RESTORE};

    use crate::window::win::hwnd;

    type NtProcessFn = unsafe extern "system" fn(HANDLE) -> NTSTATUS;

    /// `NtSuspendProcess` / `NtResumeProcess` aren't in the `windows` crate:
    /// resolved from ntdll by name.
    fn ntdll(name: PCSTR) -> Result<NtProcessFn> {
        unsafe {
            let module = match GetModuleHandleW(w!("ntdll.dll")) {
                Ok(m) => m,
                Err(_) => LoadLibraryW(w!("ntdll.dll")).context("ntdll")?,
            };
            let f = GetProcAddress(module, name).ok_or_else(|| anyhow!("funzione di sistema non trovata"))?;
            Ok(std::mem::transmute::<unsafe extern "system" fn() -> isize, NtProcessFn>(f))
        }
    }

    struct Process {
        pid: u32,
        handle: HANDLE,
    }

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    fn open(pid: u32) -> Result<Process> {
        let handle = unsafe { OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) }
            .map_err(|e| anyhow!("processo {pid} protetto ({})", e.message()))?;
        Ok(Process { pid, handle })
    }

    fn call(f: NtProcessFn, p: &Process) -> Result<()> {
        let status = unsafe { f(p.handle) };
        if status.is_ok() { Ok(()) } else { Err(anyhow!("processo {}: stato {:#x}", p.pid, status.0)) }
    }

    pub fn suspend(pids: &[u32], window: Option<isize>) -> Result<()> {
        let suspend = ntdll(s!("NtSuspendProcess"))?;
        let resume = ntdll(s!("NtResumeProcess"))?;
        // Open everything first: a protected process fails the whole thing
        // before any other is touched.
        let processes = pids.iter().map(|&pid| open(pid)).collect::<Result<Vec<_>>>()?;
        if processes.is_empty() {
            anyhow::bail!("nessun processo da sospendere");
        }
        if let Some(raw) = window {
            // Asynchronous, so a game that is slow to answer can't hang us;
            // give it a moment to leave the screen before it freezes.
            unsafe {
                let _ = ShowWindowAsync(hwnd(raw), SW_MINIMIZE);
            }
            let deadline = Instant::now() + Duration::from_secs(1);
            while Instant::now() < deadline && !unsafe { IsIconic(hwnd(raw)) }.as_bool() {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        set_muted(pids, true);
        let mut frozen: Vec<&Process> = Vec::new();
        for p in &processes {
            if let Err(err) = call(suspend, p) {
                for done in frozen {
                    let _ = call(resume, done);
                }
                set_muted(pids, false);
                if let Some(raw) = window {
                    unsafe {
                        let _ = ShowWindowAsync(hwnd(raw), SW_RESTORE);
                    }
                }
                return Err(err);
            }
            frozen.push(p);
        }
        Ok(())
    }

    pub fn resume(pids: &[u32]) -> Result<()> {
        let resume = ntdll(s!("NtResumeProcess"))?;
        let mut failed = Vec::new();
        for &pid in pids {
            match open(pid) {
                Ok(p) => {
                    if call(resume, &p).is_err() {
                        failed.push(pid);
                    }
                }
                // Gone in the meantime: nothing to thaw.
                Err(_) => {}
            }
        }
        set_muted(pids, false);
        if failed.is_empty() { Ok(()) } else { Err(anyhow!("processi non ripresi: {failed:?}")) }
    }

    /// Mutes (or unmutes) the audio sessions of `pids` on every active
    /// output, so a frozen game's last buffer doesn't loop.
    fn set_muted(pids: &[u32], muted: bool) {
        let result: Result<()> = (|| unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..devices.GetCount()? {
                let Ok(device) = devices.Item(i) else { continue };
                let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
                let Ok(sessions) = manager.GetSessionEnumerator() else { continue };
                for n in 0..sessions.GetCount().unwrap_or(0) {
                    let Ok(session) = sessions.GetSession(n) else { continue };
                    let Ok(control) = session.cast::<IAudioSessionControl2>() else { continue };
                    if !control.GetProcessId().is_ok_and(|pid| pids.contains(&pid)) {
                        continue;
                    }
                    if let Ok(volume) = session.cast::<ISimpleAudioVolume>() {
                        let _ = volume.SetMute(muted, std::ptr::null());
                    }
                }
            }
            Ok(())
        })();
        if let Err(err) = result {
            eprintln!("muting game audio failed: {err}");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    pub fn suspend(_pids: &[u32], _hwnd: Option<isize>) -> Result<()> {
        anyhow::bail!("la sospensione dei giochi funziona solo su Windows")
    }

    pub fn resume(_pids: &[u32]) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_anti_cheat_markers() {
        assert!(is_anti_cheat_marker("EasyAntiCheat"));
        assert!(is_anti_cheat_marker("EasyAntiCheat_EOS_Setup.exe"));
        assert!(is_anti_cheat_marker("BattlEye"));
        assert!(is_anti_cheat_marker("BEService.exe"));
        assert!(is_anti_cheat_marker("vgc.exe"));
        assert!(!is_anti_cheat_marker("Binaries"));
        assert!(!is_anti_cheat_marker("game.exe"));
    }

    #[test]
    fn finds_anti_cheat_folders_a_few_levels_down() {
        let root = std::env::temp_dir().join(format!("gamemode-ac-{}", std::process::id()));
        let deep = root.join("Game").join("Binaries").join("EasyAntiCheat");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(anti_cheat_in(&root));
        let clean = root.join("Clean");
        std::fs::create_dir_all(clean.join("Content")).unwrap();
        assert!(!anti_cheat_in(&clean));
        assert!(!anti_cheat_in(&root.join("missing")));
        let _ = std::fs::remove_dir_all(root);
    }
}
