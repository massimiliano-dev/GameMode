//! Running GOG's InnoSetup installers and uninstallers silently, after
//! checking that the installer really comes from GOG.

#![cfg_attr(not(windows), allow(dead_code))]

use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Result};

/// Arguments of a silent InnoSetup install into `dir`.
pub fn install_args(dir: &Path) -> String {
    let dir = dir.to_string_lossy();
    format!(r#"/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR="{}""#, dir.trim_end_matches(['\\', '/']))
}

pub const UNINSTALL_ARGS: &str = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART";

/// How long an installer may take before GameMode stops waiting.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(3 * 3600);
pub const UNINSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Signed by GOG itself ("GOG Sp. z o.o.", formerly "GOG Limited").
pub fn is_gog_signer(subject: &str) -> bool {
    let s = subject.to_ascii_lowercase();
    s.starts_with("gog sp. z o.o") || s.starts_with("gog limited") || s.starts_with("gog.com")
}

/// An installer runs only when its MD5 matches the one GOG published or it
/// carries a valid Authenticode signature by GOG (old installers have no
/// signature, some files have no published checksum).
pub fn trusted(md5_matched: bool, signer: Option<&str>) -> bool {
    md5_matched || signer.is_some_and(is_gog_signer)
}

pub fn verify(exe: &Path, md5_matched: bool) -> Result<()> {
    let signer = signer(exe);
    if trusted(md5_matched, signer.as_deref()) {
        return Ok(());
    }
    match signer {
        Some(other) => bail!("Installer firmato da \"{other}\", non da GOG: non lo eseguo"),
        None => bail!("Installer senza firma valida né checksum: non lo eseguo"),
    }
}

/// Runs `exe` (elevation prompt included) and waits for its exit code.
pub fn run_and_wait(exe: &Path, args: &str, timeout: Duration) -> Result<u32> {
    imp::run_and_wait(exe, args, timeout)
}

/// Subject of a valid Authenticode signature of `file`.
pub fn signer(file: &Path) -> Option<String> {
    imp::signer(file)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Duration;

    use anyhow::{bail, Result};
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE, HWND, WAIT_OBJECT_0};
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::{
        WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
        WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE,
        WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn run_and_wait(exe: &Path, args: &str, timeout: Duration) -> Result<u32> {
        let file = wide(exe.as_os_str());
        let params = wide(std::ffi::OsStr::new(args));
        let dir = exe.parent().map(|d| wide(d.as_os_str()));
        let verb = wide(std::ffi::OsStr::new("open"));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(params.as_ptr()),
            lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
            nShow: SW_HIDE.0,
            ..Default::default()
        };
        // "open" still shows the UAC prompt when the installer asks for admin rights.
        if let Err(err) = unsafe { ShellExecuteExW(&mut info) } {
            if err.code() == ERROR_CANCELLED.to_hresult() {
                bail!("Serve il permesso di amministratore: installazione annullata");
            }
            return Err(err.into());
        }
        let process = info.hProcess;
        if process.is_invalid() {
            bail!("Il programma di installazione non è partito");
        }
        let millis = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
        let waited = unsafe { WaitForSingleObject(process, millis) };
        let mut code = 0u32;
        let result = if waited == WAIT_OBJECT_0 {
            unsafe { GetExitCodeProcess(process, &mut code) }.map(|_| code).map_err(anyhow::Error::from)
        } else {
            unsafe {
                let _ = TerminateProcess(process, 1);
            }
            Err(anyhow::anyhow!("Il programma di installazione non risponde"))
        };
        unsafe {
            let _ = CloseHandle(process);
        }
        result
    }

    pub fn signer(file: &Path) -> Option<String> {
        let path = wide(file.as_os_str());
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR(path.as_ptr()),
            hFile: HANDLE::default(),
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut file_info },
            dwStateAction: WTD_STATEACTION_VERIFY,
            ..Default::default()
        };
        let mut action: GUID = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = unsafe { WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _) };
        let subject = (status == 0).then(|| unsafe { subject(data.hWVTStateData) }).flatten();
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe {
            WinVerifyTrust(HWND::default(), &mut action, &mut data as *mut _ as *mut _);
        }
        if status != 0 {
            log::info!("{}: no valid signature ({status:#x})", file.display());
        }
        subject
    }

    /// Leaf certificate's display name of the first signer.
    unsafe fn subject(state: HANDLE) -> Option<String> {
        unsafe {
            let provider = WTHelperProvDataFromStateData(state);
            if provider.is_null() {
                return None;
            }
            let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
            if signer.is_null() || (*signer).csCertChain == 0 || (*signer).pasCertChain.is_null() {
                return None;
            }
            let cert = (*(*signer).pasCertChain).pCert;
            if cert.is_null() {
                return None;
            }
            let mut buf = [0u16; 256];
            let len = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
            (len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;
    use std::time::Duration;

    use anyhow::{bail, Result};

    pub fn run_and_wait(_exe: &Path, _args: &str, _timeout: Duration) -> Result<u32> {
        bail!("Disponibile solo su Windows")
    }

    pub fn signer(_file: &Path) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_only_installers_from_gog() {
        assert!(trusted(true, None), "published checksum matched");
        assert!(trusted(false, Some("GOG Sp. z o.o.")));
        assert!(trusted(false, Some("GOG Limited")));
        assert!(!trusted(false, None));
        assert!(!trusted(false, Some("Evil Corp")));
        assert!(!trusted(false, Some("Not GOG Sp. z o.o.")));
    }

    #[test]
    fn quotes_the_install_folder() {
        let args = install_args(Path::new(r"C:\GOG Games\The Witcher 3\"));
        assert_eq!(args, r#"/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /SP- /DIR="C:\GOG Games\The Witcher 3""#);
    }
}
