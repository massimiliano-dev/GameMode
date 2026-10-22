//! What we need to know about a process to tell whether it belongs to a game:
//! its executable and, for packaged (Xbox / Game Pass) apps, its package
//! family. On Windows both are read with `PROCESS_QUERY_LIMITED_INFORMATION`,
//! which works for elevated and packaged processes where sysinfo's
//! full-access query comes back empty.

use std::collections::HashMap;
use std::path::PathBuf;

use sysinfo::{Pid, Process};

#[derive(Default, Clone, Debug)]
pub struct ProcInfo {
    pub exe: Option<PathBuf>,
    pub family: Option<String>,
}

/// Caches lookups across polls; keyed by pid and start time so a reused pid
/// isn't mistaken for the old process.
#[derive(Default)]
pub struct Probe {
    cache: HashMap<(u32, u64), ProcInfo>,
}

impl Probe {
    pub fn info(&mut self, pid: Pid, process: &Process) -> &ProcInfo {
        self.cache
            .entry((pid.as_u32(), process.start_time()))
            .or_insert_with(|| query(pid.as_u32(), process))
    }

    /// Drops entries of processes that are gone.
    pub fn retain_alive(&mut self, processes: &HashMap<Pid, Process>) {
        self.cache.retain(|(pid, start), _| {
            processes.get(&Pid::from_u32(*pid)).is_some_and(|p| p.start_time() == *start)
        });
    }
}

fn from_sysinfo(process: &Process) -> Option<PathBuf> {
    process.exe().filter(|p| !p.as_os_str().is_empty()).map(PathBuf::from)
}

#[cfg(not(windows))]
fn query(_pid: u32, process: &Process) -> ProcInfo {
    ProcInfo { exe: from_sysinfo(process), family: None }
}

#[cfg(windows)]
fn query(pid: u32, process: &Process) -> ProcInfo {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
    use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return ProcInfo { exe: from_sysinfo(process), family: None };
    };
    let mut info = ProcInfo::default();
    unsafe {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok() {
            info.exe = Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])));
        }
        // Package family names are at most 64 + 1 + 13 characters.
        let mut fam = [0u16; 256];
        let mut fam_len = fam.len() as u32;
        if GetPackageFamilyName(handle, &mut fam_len, Some(PWSTR(fam.as_mut_ptr()))) == ERROR_SUCCESS && fam_len > 1 {
            info.family = Some(String::from_utf16_lossy(&fam[..fam_len as usize - 1]));
        }
        let _ = CloseHandle(handle);
    }
    if info.exe.is_none() {
        info.exe = from_sysinfo(process);
    }
    info
}
