//! Per-app volume mixer for the overlay: the audio sessions on the default
//! output (what Windows' Volume Mixer shows), grouped by app and labelled
//! "Gioco", "Musica", "Chat"… Each level is a multiplier under the master
//! volume, like in Windows' mixer. Outside Windows a fixed list stands in.

// The grouping and parsing helpers are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::HashMap;

use anyhow::Result;
use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    Game,
    Music,
    Chat,
    Browser,
    Other,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppVolume {
    /// Stable id to set the volume with: "game" or the root exe name.
    pub key: String,
    pub label: String,
    /// 0.0 – 1.0, relative to the master volume.
    pub level: f32,
    pub muted: bool,
    pub kind: AppKind,
    /// Something was heard from this app recently.
    pub active: bool,
}

/// One audio session as the system reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct RawSession {
    pub pid: u32,
    pub level: f32,
    pub muted: bool,
    pub active: bool,
}

/// What we know of a process: lowercase exe name and parent pid.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub exe: String,
    pub parent: Option<u32>,
}

/// Child processes that render audio on behalf of another app.
const WEBVIEW_HOSTS: &[&str] = &["msedgewebview2.exe"];

/// Helper executables that belong to a differently named app.
fn family(exe: &str) -> &str {
    match exe {
        "steamwebhelper.exe" => "steam.exe",
        "discordptb.exe" | "discordcanary.exe" => "discord.exe",
        other => other,
    }
}

/// Readable label and kind of a root executable.
fn describe(exe: &str, music_exe: Option<&str>) -> (String, AppKind) {
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let known: Option<(&str, AppKind)> = match stem {
        "spotify" => Some(("Spotify", AppKind::Music)),
        "discord" => Some(("Discord", AppKind::Chat)),
        "teamspeak3" | "ts3client_win64" => Some(("TeamSpeak", AppKind::Chat)),
        "chrome" => Some(("Chrome", AppKind::Browser)),
        "msedge" => Some(("Edge", AppKind::Browser)),
        "firefox" => Some(("Firefox", AppKind::Browser)),
        "brave" => Some(("Brave", AppKind::Browser)),
        "opera" => Some(("Opera", AppKind::Browser)),
        "steam" => Some(("Steam", AppKind::Other)),
        _ => None,
    };
    let (label, kind) = known.map(|(l, k)| (l.to_string(), k)).unwrap_or_else(|| (prettify(stem), AppKind::Other));
    // The app behind the media session is the music, whatever it is.
    let is_music = music_exe.is_some_and(|m| !m.is_empty() && (stem == m || stem.contains(m) || m.contains(stem)));
    let kind = if is_music { AppKind::Music } else { kind };
    if kind == AppKind::Music {
        return (format!("Musica · {label}"), kind);
    }
    (label, kind)
}

fn prettify(stem: &str) -> String {
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "App".to_string(),
    }
}

/// Walks up the parent chain while it stays inside the same app: a
/// browser's audio utility is a child with the same exe name, and WebView2
/// hosts belong to whoever spawned them. Returns the root pid and its exe.
fn root_of(pid: u32, procs: &HashMap<u32, Proc>) -> (u32, String) {
    let Some(start) = procs.get(&pid) else { return (pid, String::new()) };
    let mut current = (pid, start);
    for _ in 0..8 {
        let Some(parent_pid) = current.1.parent else { break };
        let Some(parent) = procs.get(&parent_pid) else { break };
        let same_app = parent.exe == current.1.exe || WEBVIEW_HOSTS.contains(&current.1.exe.as_str());
        if !same_app || parent_pid == current.0 {
            break;
        }
        current = (parent_pid, parent);
    }
    (current.0, family(&current.1.exe).to_string())
}

/// Whether `pid` or one of its ancestors is a game GameMode launched.
fn is_game(pid: u32, procs: &HashMap<u32, Proc>, game_pids: &[u32]) -> bool {
    let mut current = pid;
    for _ in 0..8 {
        if game_pids.contains(&current) {
            return true;
        }
        match procs.get(&current).and_then(|p| p.parent) {
            Some(parent) if parent != current => current = parent,
            _ => return false,
        }
    }
    false
}

/// The mixer rows: sessions grouped by app, game first, then music and chat.
/// Returns each row with the indexes of its sessions in `sessions`.
/// `self_pid` is GameMode itself: its own sounds (played by its WebView2
/// children) are not something to mix.
pub fn group(
    sessions: &[RawSession],
    procs: &HashMap<u32, Proc>,
    game_pids: &[u32],
    music_exe: Option<&str>,
    self_pid: u32,
) -> Vec<(AppVolume, Vec<usize>)> {
    let mut rows: Vec<(AppVolume, Vec<usize>)> = Vec::new();
    for (i, s) in sessions.iter().enumerate() {
        if s.pid == 0 || s.pid == self_pid {
            continue;
        }
        let (key, label, kind) = if is_game(s.pid, procs, game_pids) {
            ("game".to_string(), "Gioco".to_string(), AppKind::Game)
        } else {
            let (root, exe) = root_of(s.pid, procs);
            if exe.is_empty() || root == self_pid {
                continue;
            }
            let (label, kind) = describe(&exe, music_exe);
            (exe, label, kind)
        };
        match rows.iter_mut().find(|(r, _)| r.key == key) {
            Some((row, idx)) => {
                row.level = row.level.max(s.level);
                row.muted &= s.muted;
                row.active |= s.active;
                idx.push(i);
            }
            None => rows.push((AppVolume { key, label, level: s.level, muted: s.muted, kind, active: s.active }, vec![i])),
        }
    }
    rows.sort_by(|(a, _), (b, _)| a.kind.cmp(&b.kind).then(b.active.cmp(&a.active)).then(a.label.cmp(&b.label)));
    rows
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use anyhow::{anyhow, Result};
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    use windows::core::Interface;
    use windows::Win32::Foundation::S_OK;
    use windows::Win32::Media::Audio::{
        eConsole, eRender, AudioSessionStateActive, AudioSessionStateExpired, IAudioSessionControl2,
        IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    use super::{AppVolume, Proc, RawSession};

    /// Changes are applied one at a time, in the order they were asked for,
    /// so a later level never loses to an earlier one still in flight.
    static APPLY: Mutex<()> = Mutex::new(());

    /// The sessions on the default output with their volume controls.
    fn sessions() -> Result<Vec<(RawSession, ISimpleAudioVolume)>> {
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let manager = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)?;
            let list = manager.GetSessionEnumerator()?;
            let mut out = Vec::new();
            for i in 0..list.GetCount()? {
                let Ok(control) = list.GetSession(i) else { continue };
                let Ok(control2) = control.cast::<IAudioSessionControl2>() else { continue };
                // S_OK means system sounds (S_FALSE is a plain app session).
                if control2.IsSystemSoundsSession() == S_OK {
                    continue;
                }
                let state = control.GetState().unwrap_or(AudioSessionStateExpired);
                if state == AudioSessionStateExpired {
                    continue;
                }
                let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
                let session = RawSession {
                    pid: control2.GetProcessId().unwrap_or(0),
                    level: volume.GetMasterVolume().unwrap_or(1.0),
                    muted: volume.GetMute().map(|m| m.as_bool()).unwrap_or(false),
                    active: state == AudioSessionStateActive,
                };
                out.push((session, volume));
            }
            Ok(out)
        }
    }

    fn processes() -> HashMap<u32, Proc> {
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        sys.processes()
            .iter()
            .map(|(pid, p)| {
                (pid.as_u32(), Proc { exe: p.name().to_string_lossy().to_lowercase(), parent: p.parent().map(|p| p.as_u32()) })
            })
            .collect()
    }

    fn grouped(game_pids: &[u32]) -> Result<(Vec<(AppVolume, Vec<usize>)>, Vec<ISimpleAudioVolume>)> {
        let (raw, controls): (Vec<RawSession>, Vec<ISimpleAudioVolume>) = sessions()?.into_iter().unzip();
        let music = crate::media::source_exe();
        Ok((super::group(&raw, &processes(), game_pids, music.as_deref(), std::process::id()), controls))
    }

    pub fn app_volumes(game_pids: &[u32]) -> Result<Vec<AppVolume>> {
        Ok(grouped(game_pids)?.0.into_iter().map(|(row, _)| row).collect())
    }

    fn apply(key: &str, game_pids: &[u32], f: impl Fn(&ISimpleAudioVolume) -> windows::core::Result<()>) -> Result<()> {
        let _guard = APPLY.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, controls) = grouped(game_pids)?;
        let (_, idx) = rows.into_iter().find(|(r, _)| r.key == key).ok_or_else(|| anyhow!("Questa app non suona più"))?;
        for i in idx {
            f(&controls[i])?;
        }
        Ok(())
    }

    pub fn set_level(key: &str, level: f32, game_pids: &[u32]) -> Result<()> {
        let level = level.clamp(0.0, 1.0);
        apply(key, game_pids, |v| unsafe { v.SetMasterVolume(level, std::ptr::null()) })
    }

    pub fn set_muted(key: &str, muted: bool, game_pids: &[u32]) -> Result<()> {
        apply(key, game_pids, |v| unsafe { v.SetMute(muted, std::ptr::null()) })
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Mutex;

    use anyhow::Result;

    use super::{AppKind, AppVolume};

    static ROWS: Mutex<Vec<AppVolume>> = Mutex::new(Vec::new());

    fn rows() -> Vec<AppVolume> {
        let mut guard = ROWS.lock().unwrap();
        if guard.is_empty() {
            *guard = vec![
                AppVolume { key: "game".into(), label: "Gioco".into(), level: 1.0, muted: false, kind: AppKind::Game, active: true },
                AppVolume { key: "spotify.exe".into(), label: "Musica · Spotify".into(), level: 0.4, muted: false, kind: AppKind::Music, active: true },
                AppVolume { key: "discord.exe".into(), label: "Discord".into(), level: 0.8, muted: false, kind: AppKind::Chat, active: false },
            ];
        }
        guard.clone()
    }

    pub fn app_volumes(_game_pids: &[u32]) -> Result<Vec<AppVolume>> {
        Ok(rows())
    }

    pub fn set_level(key: &str, level: f32, _game_pids: &[u32]) -> Result<()> {
        rows();
        if let Some(r) = ROWS.lock().unwrap().iter_mut().find(|r| r.key == key) {
            r.level = level.clamp(0.0, 1.0);
        }
        Ok(())
    }

    pub fn set_muted(key: &str, muted: bool, _game_pids: &[u32]) -> Result<()> {
        rows();
        if let Some(r) = ROWS.lock().unwrap().iter_mut().find(|r| r.key == key) {
            r.muted = muted;
        }
        Ok(())
    }
}

/// The mixer rows. Blocking (COM, process list): call off the main thread.
pub fn app_volumes(game_pids: &[u32]) -> Result<Vec<AppVolume>> {
    imp::app_volumes(game_pids)
}

/// Sets every session of the row `key` to `level` (0–1 under the master).
pub fn set_level(key: &str, level: f32, game_pids: &[u32]) -> Result<()> {
    imp::set_level(key, level, game_pids)
}

pub fn set_muted(key: &str, muted: bool, game_pids: &[u32]) -> Result<()> {
    imp::set_muted(key, muted, game_pids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn procs(list: &[(u32, &str, Option<u32>)]) -> HashMap<u32, Proc> {
        list.iter().map(|(pid, exe, parent)| (*pid, Proc { exe: exe.to_string(), parent: *parent })).collect()
    }

    fn session(pid: u32, level: f32) -> RawSession {
        RawSession { pid, level, muted: false, active: true }
    }

    #[test]
    fn browser_audio_utility_joins_its_browser() {
        let procs = procs(&[(10, "chrome.exe", Some(1)), (11, "chrome.exe", Some(10)), (12, "chrome.exe", Some(10)), (1, "explorer.exe", None)]);
        let rows = group(&[session(11, 0.5), session(12, 0.9)], &procs, &[], None, 0);
        assert_eq!(rows.len(), 1);
        let (row, idx) = &rows[0];
        assert_eq!(row.key, "chrome.exe");
        assert_eq!(row.label, "Chrome");
        assert_eq!(row.kind, AppKind::Browser);
        assert_eq!(row.level, 0.9);
        assert_eq!(idx, &vec![0, 1]);
    }

    #[test]
    fn game_is_found_through_its_launcher_child() {
        let procs = procs(&[(100, "game.exe", Some(1)), (101, "game-audio.exe", Some(100)), (1, "steam.exe", None)]);
        let rows = group(&[session(101, 1.0), session(1, 1.0)], &procs, &[100], None, 0);
        assert_eq!(rows[0].0.key, "game");
        assert_eq!(rows[0].0.label, "Gioco");
        assert_eq!(rows[0].0.kind, AppKind::Game);
        assert_eq!(rows[1].0.label, "Steam");
    }

    #[test]
    fn music_player_is_labelled_from_media_session() {
        let procs = procs(&[(20, "spotify.exe", Some(1)), (21, "spotify.exe", Some(20)), (30, "msedge.exe", Some(1)), (31, "msedge.exe", Some(30))]);
        let rows = group(&[session(21, 0.3), session(31, 0.7)], &procs, &[], Some("msedge"), 0);
        let labels: Vec<_> = rows.iter().map(|(r, _)| (r.label.as_str(), r.kind)).collect();
        assert_eq!(labels, vec![("Musica · Edge", AppKind::Music), ("Musica · Spotify", AppKind::Music)]);
    }

    #[test]
    fn webview_host_belongs_to_its_parent() {
        let procs = procs(&[(40, "myapp.exe", Some(1)), (41, "msedgewebview2.exe", Some(40)), (42, "msedgewebview2.exe", Some(41))]);
        let rows = group(&[session(42, 1.0)], &procs, &[], None, 0);
        assert_eq!(rows[0].0.key, "myapp.exe");
        assert_eq!(rows[0].0.label, "Myapp");
    }

    #[test]
    fn order_is_game_music_chat_then_others_and_mute_needs_all() {
        let procs = procs(&[(1, "discord.exe", None), (2, "vlc.exe", None), (3, "spotify.exe", None), (4, "game.exe", None)]);
        let sessions = vec![
            session(2, 1.0),
            RawSession { pid: 1, level: 1.0, muted: true, active: false },
            RawSession { pid: 1, level: 0.5, muted: false, active: true },
            session(3, 1.0),
            session(4, 1.0),
        ];
        let rows = group(&sessions, &procs, &[4], None, 0);
        let keys: Vec<_> = rows.iter().map(|(r, _)| r.key.as_str()).collect();
        assert_eq!(keys, vec!["game", "spotify.exe", "discord.exe", "vlc.exe"]);
        let discord = &rows[2].0;
        assert!(!discord.muted);
        assert!(discord.active);
    }

    #[test]
    fn gamemode_own_sounds_are_hidden() {
        let procs = procs(&[(50, "gamemode.exe", Some(1)), (51, "msedgewebview2.exe", Some(50)), (52, "msedgewebview2.exe", Some(51)), (60, "vlc.exe", Some(1))]);
        let rows = group(&[session(52, 1.0), session(50, 1.0), session(60, 0.5)], &procs, &[], None, 50);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.key, "vlc.exe");
        // Another app's WebView2 host is still listed.
        let rows = group(&[session(52, 1.0)], &procs, &[], None, 999);
        assert_eq!(rows[0].0.label, "Gamemode");
    }

    #[test]
    fn unknown_and_system_sessions_are_skipped() {
        let procs = procs(&[(1, "x.exe", None)]);
        let rows = group(&[session(0, 1.0), session(999, 1.0), session(1, 1.0)], &procs, &[], None, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0.label, "X");
    }
}
