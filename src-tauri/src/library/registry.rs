//! Windows registry helpers shared by the GOG, Ubisoft and EA providers.

use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
use winreg::{RegKey, HKEY};

use super::Values;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// All string and DWORD values of a key.
pub fn values(key: &RegKey) -> Values {
    let mut out = Values::new();
    for (name, _) in key.enum_values().flatten() {
        if let Ok(s) = key.get_value::<String, _>(&name) {
            out.insert(name, s);
        } else if let Ok(n) = key.get_value::<u32, _>(&name) {
            out.insert(name, n.to_string());
        }
    }
    out
}

/// Subkeys of `root\path` with their values, read through the given view
/// (`KEY_WOW64_32KEY` reaches `WOW6432Node`).
pub fn subkeys(root: HKEY, path: &str, view: u32) -> Vec<(String, Values)> {
    let Ok(key) = RegKey::predef(root).open_subkey_with_flags(path, KEY_READ | view) else {
        return Vec::new();
    };
    key.enum_keys()
        .flatten()
        .filter_map(|name| {
            let sub = key.open_subkey_with_flags(&name, KEY_READ | view).ok()?;
            Some((name, values(&sub)))
        })
        .collect()
}

/// Subkeys of `HKLM\SOFTWARE\<path>` in the 32-bit view (where most launchers
/// write), falling back to the 64-bit view.
pub fn hklm_software(path: &str) -> Vec<(String, Values)> {
    let full = format!(r"SOFTWARE\{path}");
    let entries = subkeys(HKEY_LOCAL_MACHINE, &full, KEY_WOW64_32KEY);
    if entries.is_empty() { subkeys(HKEY_LOCAL_MACHINE, &full, KEY_WOW64_64KEY) } else { entries }
}

pub fn hklm_value(path: &str, name: &str) -> Option<String> {
    let full = format!(r"SOFTWARE\{path}");
    [KEY_WOW64_32KEY, KEY_WOW64_64KEY].into_iter().find_map(|view| {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(&full, KEY_READ | view)
            .and_then(|k| k.get_value::<String, _>(name))
            .ok()
    })
}

/// "Programs and Features" entries from every registry view.
pub fn uninstall_entries() -> Vec<(String, Values)> {
    let mut all = subkeys(HKEY_LOCAL_MACHINE, UNINSTALL, KEY_WOW64_64KEY);
    all.extend(subkeys(HKEY_LOCAL_MACHINE, UNINSTALL, KEY_WOW64_32KEY));
    all.extend(subkeys(HKEY_CURRENT_USER, UNINSTALL, 0));
    all
}
