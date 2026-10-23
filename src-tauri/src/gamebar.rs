//! The Xbox (Guide) button belongs to GameMode in console mode. Out of the
//! box Windows hands it to Game Bar, which then covers GameMode or the game
//! (GameMode only sees the press through `XInputGetStateEx`). While console
//! mode is on, and the "Il tasto Xbox apre GameMode" setting with it, the
//! Game Bar values below are switched off; what the user had is kept in
//! `gamebar_backup.json` and put back when console mode (or the setting)
//! goes off, also after a crash or an update in between.
//!
//! Steam's "Guide button focuses Steam" is left alone: it has no documented
//! place in Steam's files, and Steam rewrites its config at start anyway.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

const BACKUP_FILE: &str = "gamebar_backup.json";

/// DWORDs under `HKCU\Software\Microsoft\GameBar`:
/// - `UseNexusForGameBarEnabled`: the Xbox button opens Game Bar (Settings →
///   Gaming → Game Bar → "Allow your controller to open Game Bar").
/// - `GamepadNexusChordEnabled`: View + Menu work as the Xbox button in
///   apps; GameMode's overlay is View + Menu (Back+Start) during a game.
const VALUES: [&str; 2] = ["UseNexusForGameBarEnabled", "GamepadNexusChordEnabled"];

/// What the values were before GameMode switched them off; `None`: the value
/// wasn't there (Windows' own default).
type Backup = BTreeMap<String, Option<u32>>;

/// Where the values live (the registry; a map in tests and off Windows).
trait Values {
    fn get(&self, name: &str) -> Option<u32>;
    /// `None` deletes the value.
    fn set(&mut self, name: &str, value: Option<u32>) -> Result<()>;
}

/// Whether GameMode should hold the Xbox button.
pub fn wanted(console_mode: bool, setting: bool) -> bool {
    console_mode && setting
}

/// Brings Game Bar in line with `wanted`: switched off (keeping a backup),
/// or given back as it was. Cheap and idempotent: call it at start, when
/// console mode changes and when settings are saved.
pub fn apply(data_dir: &Path, wanted: bool) {
    let result = imp::Registry::open().and_then(|mut reg| sync(&mut reg, &data_dir.join(BACKUP_FILE), wanted));
    if let Err(err) = result {
        log::warn!("game bar ({}): {err:#}", if wanted { "take over" } else { "give back" });
    }
}

fn sync(reg: &mut impl Values, file: &Path, wanted: bool) -> Result<()> {
    if wanted {
        take_over(reg, file)
    } else {
        give_back(reg, file)
    }
}

/// Saves what the user had (only once: later calls find their own zeros),
/// then switches the values off. Nothing changes if the backup can't be saved.
fn take_over(reg: &mut impl Values, file: &Path) -> Result<()> {
    let existing = read_backup(file);
    let mut changed = existing.is_none();
    let mut backup = existing.unwrap_or_default();
    for name in VALUES {
        if !backup.contains_key(name) {
            backup.insert(name.to_string(), reg.get(name));
            changed = true;
        }
    }
    if changed {
        write_backup(file, &backup)?;
    }
    for name in VALUES {
        if reg.get(name) != Some(0) {
            reg.set(name, Some(0))?;
        }
    }
    Ok(())
}

/// Puts back what the backup holds, then forgets it. A value the user turned
/// on again by hand meanwhile (no longer GameMode's 0) stays as it is.
fn give_back(reg: &mut impl Values, file: &Path) -> Result<()> {
    let Some(backup) = read_backup(file) else { return Ok(()) };
    for name in VALUES {
        let before = backup.get(name).copied().flatten();
        if reg.get(name) == Some(0) && before != Some(0) {
            reg.set(name, before)?;
        }
    }
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// `None` when there is no backup; an unreadable one counts as empty (the
/// values then go back to Windows' defaults).
fn read_backup(file: &Path) -> Option<Backup> {
    let text = std::fs::read_to_string(file).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_default())
}

fn write_backup(file: &Path, backup: &Backup) -> Result<()> {
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(backup)?)?;
    std::fs::rename(&tmp, file)?;
    Ok(())
}

#[cfg(windows)]
mod imp {
    use anyhow::Result;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const KEY: &str = r"Software\Microsoft\GameBar";

    pub struct Registry(RegKey);

    impl Registry {
        pub fn open() -> Result<Self> {
            let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(KEY)?;
            Ok(Registry(key))
        }
    }

    impl super::Values for Registry {
        fn get(&self, name: &str) -> Option<u32> {
            self.0.get_value::<u32, _>(name).ok()
        }

        fn set(&mut self, name: &str, value: Option<u32>) -> Result<()> {
            match value {
                Some(v) => self.0.set_value(name, &v)?,
                None => match self.0.delete_value(name) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                    _ => {}
                },
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use anyhow::Result;

    /// Stand-in for the registry during development.
    static VALUES: Mutex<BTreeMap<String, u32>> = Mutex::new(BTreeMap::new());

    pub struct Registry;

    impl Registry {
        pub fn open() -> Result<Self> {
            Ok(Registry)
        }
    }

    impl super::Values for Registry {
        fn get(&self, name: &str) -> Option<u32> {
            VALUES.lock().unwrap().get(name).copied()
        }

        fn set(&mut self, name: &str, value: Option<u32>) -> Result<()> {
            let mut values = VALUES.lock().unwrap();
            match value {
                Some(v) => values.insert(name.to_string(), v),
                None => values.remove(name),
            };
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Map(BTreeMap<String, u32>);

    impl Values for Map {
        fn get(&self, name: &str) -> Option<u32> {
            self.0.get(name).copied()
        }

        fn set(&mut self, name: &str, value: Option<u32>) -> Result<()> {
            match value {
                Some(v) => self.0.insert(name.to_string(), v),
                None => self.0.remove(name),
            };
            Ok(())
        }
    }

    fn temp_file(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("gamemode-gamebar-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(BACKUP_FILE);
        let _ = std::fs::remove_file(&file);
        file
    }

    const NEXUS: &str = "UseNexusForGameBarEnabled";
    const CHORD: &str = "GamepadNexusChordEnabled";

    #[test]
    fn takes_over_and_gives_back_what_was_there() {
        let file = temp_file("roundtrip");
        let mut reg = Map::default();
        reg.set(NEXUS, Some(1)).unwrap();
        // CHORD missing: Windows' default.
        sync(&mut reg, &file, true).unwrap();
        assert_eq!((reg.get(NEXUS), reg.get(CHORD)), (Some(0), Some(0)));
        assert!(file.exists());
        sync(&mut reg, &file, false).unwrap();
        assert_eq!((reg.get(NEXUS), reg.get(CHORD)), (Some(1), None));
        assert!(!file.exists());
    }

    #[test]
    fn a_second_take_over_keeps_the_first_backup() {
        let file = temp_file("twice");
        let mut reg = Map::default();
        reg.set(NEXUS, Some(1)).unwrap();
        sync(&mut reg, &file, true).unwrap();
        // Next start in console mode: the zeros are GameMode's own.
        sync(&mut reg, &file, true).unwrap();
        sync(&mut reg, &file, false).unwrap();
        assert_eq!(reg.get(NEXUS), Some(1));
    }

    #[test]
    fn giving_back_without_a_backup_changes_nothing() {
        let file = temp_file("none");
        let mut reg = Map::default();
        reg.set(NEXUS, Some(0)).unwrap();
        sync(&mut reg, &file, false).unwrap();
        assert_eq!(reg.get(NEXUS), Some(0));
    }

    #[test]
    fn a_value_changed_by_hand_meanwhile_stays() {
        let file = temp_file("by-hand");
        let mut reg = Map::default();
        sync(&mut reg, &file, true).unwrap();
        // The user turned Game Bar's button on again in Windows' settings.
        reg.set(NEXUS, Some(1)).unwrap();
        sync(&mut reg, &file, false).unwrap();
        assert_eq!(reg.get(NEXUS), Some(1));
        // The other one was still GameMode's 0: back to missing.
        assert_eq!(reg.get(CHORD), None);
    }

    #[test]
    fn a_broken_backup_falls_back_to_windows_defaults() {
        let file = temp_file("broken");
        let mut reg = Map::default();
        sync(&mut reg, &file, true).unwrap();
        std::fs::write(&file, "not json").unwrap();
        sync(&mut reg, &file, false).unwrap();
        assert_eq!((reg.get(NEXUS), reg.get(CHORD)), (None, None));
    }

    #[test]
    fn only_console_mode_with_the_setting_takes_the_button() {
        assert!(wanted(true, true));
        assert!(!wanted(true, false));
        assert!(!wanted(false, true));
    }
}
