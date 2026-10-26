//! Starting Steam games without Steam's windows: Steam is brought up with
//! `-silent` (no library window), GameMode waits until it is signed in, and
//! the game is started with `steam.exe -applaunch <appid>`, which the running
//! instance takes over directly, without the protocol handler and without
//! opening the library.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::model::{Game, LaunchSpec, Store};

/// How long Steam may take to start and sign in before the game is
/// launched anyway (Steam then queues it itself).
const START_TIMEOUT: Duration = Duration::from_secs(30);
/// Steam's web helper up for this long with nobody signed in: Steam is
/// waiting for a login, not still signing in on its own.
const LOGIN_GRACE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(500);

/// What the launch is waiting on, for the launch screen.
pub const STAGE_STARTING: &str = "steamStarting";
pub const STAGE_LOGIN: &str = "steamLogin";

/// The Steam appid to start with `-applaunch`, for a Steam game launched
/// through `steam://rungameid/<appid>` (non-Steam shortcuts have 64-bit ids
/// `-applaunch` doesn't know, so they keep the link).
pub fn appid_of(game: &Game) -> Option<u32> {
    if game.store != Store::Steam {
        return None;
    }
    match &game.launch {
        LaunchSpec::Uri { uri } => uri.strip_prefix("steam://rungameid/")?.parse().ok(),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum SteamState {
    /// Running and signed in: the game can go.
    Ready,
    /// Still starting or signing in.
    Starting,
    /// Running, but nobody is signed in.
    NeedsLogin,
    /// Not ready in time: launch anyway.
    GaveUp,
}

/// Decides where Steam is. `active_user` is `ActiveProcess\ActiveUser`
/// (`None` when it can't be read), `helper_for` how long its web helper has
/// been up, `waited` the time since GameMode began waiting, `asked_login`
/// whether the user was already sent to sign in (then GameMode waits for
/// as long as the launch lasts).
fn steam_state(running: bool, active_user: Option<u32>, helper_for: Option<Duration>, waited: Duration, asked_login: bool) -> SteamState {
    let helper_up = helper_for.is_some();
    match active_user {
        Some(user) if running && user != 0 => return SteamState::Ready,
        // No registry to ask: the web helper is the best sign Steam is up.
        None if running && helper_up => return SteamState::Ready,
        _ => {}
    }
    if asked_login || (running && active_user == Some(0) && helper_for.is_some_and(|d| d >= LOGIN_GRACE)) {
        return SteamState::NeedsLogin;
    }
    if waited >= START_TIMEOUT {
        return SteamState::GaveUp;
    }
    SteamState::Starting
}

/// Gets Steam running and signed in, starting it silently if needed and
/// sending the user to Big Picture to sign in. `still_wanted` says whether
/// the launch is still on (the user may cancel it meanwhile); `stage` gets
/// what to show on the launch screen (`None`: nothing special). Returns
/// false when the launch was cancelled. Blocking: call off the main thread.
pub fn ensure_ready(exe: &Path, still_wanted: impl Fn() -> bool, mut stage: impl FnMut(Option<&'static str>)) -> bool {
    let mut sys = System::new();
    let started = Instant::now();
    let mut spawned = false;
    let mut asked_login = false;
    let mut helper_since: Option<Instant> = None;
    let mut shown: Option<&'static str> = None;
    loop {
        if !still_wanted() {
            return false;
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        let has = |name: &str| sys.processes().values().any(|p| p.name().eq_ignore_ascii_case(name));
        let running = has("steam.exe");
        if has("steamwebhelper.exe") {
            helper_since.get_or_insert_with(Instant::now);
        } else {
            helper_since = None;
        }
        if !running && !spawned {
            spawned = true;
            if let Err(err) = Command::new(exe).arg("-silent").spawn() {
                log::warn!("starting Steam failed: {err}");
                return true;
            }
        }
        let state = steam_state(running, imp::active_user(), helper_since.map(|t| t.elapsed()), started.elapsed(), asked_login);
        let next = match state {
            SteamState::Ready | SteamState::GaveUp => {
                if state == SteamState::GaveUp {
                    log::warn!("Steam not ready after {}s, launching anyway", START_TIMEOUT.as_secs());
                }
                if shown.is_some() {
                    stage(None);
                }
                return true;
            }
            SteamState::Starting => STAGE_STARTING,
            SteamState::NeedsLogin => {
                if !asked_login {
                    asked_login = true;
                    // Big Picture's sign-in can be driven with the controller;
                    // the launch screen steps aside for it like for any store window.
                    if let Err(err) = Command::new(exe).arg("steam://open/bigpicture").spawn() {
                        log::warn!("opening Big Picture failed: {err}");
                    }
                }
                STAGE_LOGIN
            }
        };
        if shown != Some(next) {
            shown = Some(next);
            stage(Some(next));
        }
        std::thread::sleep(POLL);
    }
}

/// Hands the game to the running Steam. `-silent` keeps Steam's window
/// away should it have closed in the meantime.
pub fn applaunch(exe: &Path, appid: u32) -> std::io::Result<()> {
    Command::new(exe).args(["-silent", "-applaunch", &appid.to_string()]).spawn().map(|_| ())
}

#[cfg(windows)]
mod imp {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// The signed-in Steam account (0: nobody), while Steam runs.
    pub fn active_user() -> Option<u32> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Valve\Steam\ActiveProcess")
            .and_then(|k| k.get_value::<u32, _>("ActiveUser"))
            .ok()
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn active_user() -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    #[test]
    fn appid_only_for_real_steam_games() {
        let mut game = Game::new(Store::Steam, "570", "Dota 2", LaunchSpec::Uri { uri: "steam://rungameid/570".into() });
        assert_eq!(appid_of(&game), Some(570));
        // Non-Steam shortcut: a 64-bit id `-applaunch` doesn't take.
        game.launch = LaunchSpec::Uri { uri: "steam://rungameid/17447428925087383552".into() };
        assert_eq!(appid_of(&game), None);
        game.launch = LaunchSpec::Uri { uri: "steam://run/570".into() };
        assert_eq!(appid_of(&game), None);
        let other = Game::new(Store::Manual, "x", "X", LaunchSpec::Uri { uri: "steam://rungameid/570".into() });
        assert_eq!(appid_of(&other), None);
    }

    #[test]
    fn ready_once_signed_in() {
        assert_eq!(steam_state(true, Some(123), None, S, false), SteamState::Ready);
        assert_eq!(steam_state(false, Some(123), None, S, false), SteamState::Starting);
        // Not signed in yet, but the web helper just came up: still signing in.
        assert_eq!(steam_state(true, Some(0), Some(S), 5 * S, false), SteamState::Starting);
        // No registry to read: the web helper decides.
        assert_eq!(steam_state(true, None, Some(S), S, false), SteamState::Ready);
        assert_eq!(steam_state(true, None, None, S, false), SteamState::Starting);
    }

    #[test]
    fn asks_for_login_and_then_waits_for_it() {
        assert_eq!(steam_state(true, Some(0), Some(LOGIN_GRACE), 12 * S, false), SteamState::NeedsLogin);
        // Sent to sign in: no giving up while the launch is on.
        assert_eq!(steam_state(true, Some(0), None, 120 * S, true), SteamState::NeedsLogin);
        assert_eq!(steam_state(true, Some(42), Some(S), 120 * S, true), SteamState::Ready);
    }

    #[test]
    fn gives_up_after_the_timeout() {
        assert_eq!(steam_state(false, Some(0), None, START_TIMEOUT, false), SteamState::GaveUp);
        assert_eq!(steam_state(true, Some(0), Some(S), START_TIMEOUT, false), SteamState::GaveUp);
    }
}
