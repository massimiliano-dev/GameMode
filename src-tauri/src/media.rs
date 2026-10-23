//! Music in the background of a game: what Windows' media session (the
//! same one the volume flyout shows) is playing, transport controls for it,
//! and a shortcut to start Spotify. Outside Windows a fake track stands in.

// The grouping and parsing helpers are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Sent to every window whenever the track or its state changes.
pub const EVENT: &str = "media://now-playing";

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Readable name of the app that plays ("Spotify", "Edge").
    pub source: String,
    /// Lowercase executable stem of the player ("spotify", "msedge"), to
    /// find it in the mixer.
    pub source_exe: String,
    /// `data:` URL of the cover art, when the player publishes one.
    pub thumbnail: Option<String>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MediaAction {
    Play,
    Pause,
    Toggle,
    Next,
    Prev,
}

/// Last state read from the system, shared with the commands.
static CURRENT: Mutex<Option<NowPlaying>> = Mutex::new(None);

pub fn now_playing() -> Option<NowPlaying> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Executable stem of the current player, for the mixer's "Musica" label.
pub fn source_exe() -> Option<String> {
    now_playing().map(|n| n.source_exe).filter(|s| !s.is_empty())
}

fn publish(app: &AppHandle, now: Option<NowPlaying>) {
    let changed = {
        let mut guard = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
        let changed = *guard != now;
        *guard = now.clone();
        changed
    };
    if changed {
        let _ = app.emit(EVENT, &now);
    }
}

/// Apps known by their AppUserModelId (desktop exe or packaged app).
const KNOWN: &[(&str, &str, &str)] = &[
    ("spotify", "Spotify", "spotify"),
    ("msedge", "Edge", "msedge"),
    ("chrome", "Chrome", "chrome"),
    ("firefox", "Firefox", "firefox"),
    ("308046b0af4a39cb", "Firefox", "firefox"),
    ("brave", "Brave", "brave"),
    ("opera", "Opera", "opera"),
    ("vlc", "VLC", "vlc"),
    ("zunemusic", "Lettore multimediale", "music.ui"),
    ("microsoft.media.player", "Lettore multimediale", "microsoft.media.player"),
    ("groove", "Groove", "music.ui"),
    ("itunes", "iTunes", "itunes"),
    ("applemusic", "Apple Music", "applemusic"),
    ("amazon music", "Amazon Music", "amazon music"),
    ("tidal", "TIDAL", "tidal"),
    ("deezer", "Deezer", "deezer"),
    ("foobar2000", "foobar2000", "foobar2000"),
    ("musicbee", "MusicBee", "musicbee"),
];

/// Readable app name and executable stem from a media session's
/// AppUserModelId, e.g. `Spotify.exe`, `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify`
/// or `MSEdge`.
pub fn source_of(aumid: &str) -> (String, String) {
    let lower = aumid.to_lowercase();
    if let Some((_, name, exe)) = KNOWN.iter().find(|(needle, _, _)| lower.contains(needle)) {
        return (name.to_string(), exe.to_string());
    }
    // Packaged: `Publisher.App_hash!Id` → the app part; desktop: the file stem.
    let raw = match aumid.split_once('!') {
        Some((family, id)) => {
            let family_app = family.split_once('_').map(|(f, _)| f).unwrap_or(family);
            let family_app = family_app.rsplit('.').next().unwrap_or(family_app);
            if id.eq_ignore_ascii_case("app") || id.is_empty() { family_app } else { id }
        }
        None => aumid.rsplit(['\\', '/']).next().unwrap_or(aumid),
    };
    let stem = raw.strip_suffix(".exe").or_else(|| raw.strip_suffix(".EXE")).unwrap_or(raw);
    let stem = stem.trim();
    if stem.is_empty() {
        return ("Musica".to_string(), String::new());
    }
    (stem.to_string(), stem.to_lowercase())
}

/// Opens Spotify (the desktop app, else the web player). With a game in
/// front, Spotify's window is put away again right after it appears and
/// the game gets the screen back; otherwise GameMode steps aside for it.
/// Blocking: call off the main thread.
pub fn open_music(app: &AppHandle, game_pids: Vec<u32>) -> Result<()> {
    if game_pids.is_empty() {
        return crate::storefront::open_first(app, &["spotify:".to_string(), "https://open.spotify.com".to_string()]);
    }
    if !crate::storefront::launch_uri("spotify:") {
        anyhow::bail!("Spotify non è installato: aprilo da GameMode, fuori dal gioco");
    }
    let app = app.clone();
    std::thread::spawn(move || {
        // Spotify takes a moment to show its window; then it goes down and
        // keeps playing, and the game is back in front.
        for delay in [1500, 1500, 2000] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            if dismiss_spotify() {
                crate::window::resume_game(&app, &game_pids);
            }
        }
    });
    Ok(())
}

/// Minimizes Spotify's window if it is in front. Spotify keeps playing.
fn dismiss_spotify() -> bool {
    #[cfg(windows)]
    {
        crate::window::minimize_foreground_of(&["spotify.exe"])
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use anyhow::{anyhow, bail, Result};
    use base64::Engine;
    use tauri::AppHandle;
    use windows::Foundation::TypedEventHandler;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::DataReader;
    use windows::Win32::System::Com::{CoIncrementMTAUsage, CoInitializeEx, COINIT_MULTITHREADED};

    use super::{source_of, MediaAction, NowPlaying};

    /// Covers bigger than this are skipped (they go to the webview as a data URL).
    const MAX_THUMBNAIL: u64 = 1_500_000;

    static MANAGER: Mutex<Option<Manager>> = Mutex::new(None);
    /// The session being followed and its event tokens.
    static BOUND: Mutex<Option<(Session, i64, i64)>> = Mutex::new(None);
    /// Cover art of the last track, keyed by (title, artist, album): the
    /// thumbnail is read once per track, not on every play/pause. Only a
    /// cover actually read is kept, so a track whose art comes late is
    /// retried on its next event.
    static THUMB: Mutex<Option<((String, String, String), String)>> = Mutex::new(None);
    /// The events arrive on thread-pool threads, possibly at the same time:
    /// rebinding and reading happen one at a time, in arrival order, so the
    /// last published state is the latest one. The event handlers never take
    /// this themselves; they only call into `rebind`/`refresh`.
    static SYNC: Mutex<()> = Mutex::new(());

    /// Connects to the media session manager and follows the current
    /// session from now on; the events arrive on MTA threads.
    pub fn init(app: AppHandle) {
        std::thread::spawn(move || {
            unsafe {
                // Keeps the multithreaded apartment alive for the whole
                // process even though this thread ends right away: the
                // manager's proxies and event registrations live in it.
                if CoIncrementMTAUsage().is_err() {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                }
            }
            let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("media sessions unavailable: {e}");
                    return;
                }
            };
            // Each event has its own argument type, hence one closure each.
            let current = app.clone();
            let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                rebind(&current);
                Ok(())
            }));
            let sessions = app.clone();
            let _ = manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
                rebind(&sessions);
                Ok(())
            }));
            *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager);
            rebind(&app);
        });
    }

    /// Follows whatever session is current now and publishes its state.
    fn rebind(app: &AppHandle) {
        let _guard = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        let manager = MANAGER.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let session = manager.and_then(|m| m.GetCurrentSession().ok());
        let old = BOUND.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((s, a, b)) = old {
            let _ = s.RemovePlaybackInfoChanged(a);
            let _ = s.RemoveMediaPropertiesChanged(b);
        }
        if let Some(session) = session {
            let playback = app.clone();
            let a = session
                .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    refresh(&playback, false);
                    Ok(())
                }))
                .unwrap_or(0);
            let props = app.clone();
            let b = session
                .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    // The player may publish the title before the cover (or
                    // the previous cover with the new title): read it again.
                    refresh(&props, true);
                    Ok(())
                }))
                .unwrap_or(0);
            *BOUND.lock().unwrap_or_else(|e| e.into_inner()) = Some((session, a, b));
        }
        refresh_locked(app, false);
    }

    fn session() -> Option<Session> {
        BOUND.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(s, _, _)| s.clone())
    }

    /// Reads the followed session and publishes what it plays. `fresh`
    /// re-reads the cover even when it is cached for this track.
    fn refresh(app: &AppHandle, fresh: bool) {
        let _guard = SYNC.lock().unwrap_or_else(|e| e.into_inner());
        refresh_locked(app, fresh);
    }

    /// `refresh` for callers already holding `SYNC`.
    fn refresh_locked(app: &AppHandle, fresh: bool) {
        let now = session().and_then(|s| read(&s, fresh).ok());
        super::publish(app, now);
    }

    fn read(session: &Session, fresh: bool) -> Result<NowPlaying> {
        let (source, source_exe) = source_of(&session.SourceAppUserModelId()?.to_string());
        let playing = session.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|s| s == Status::Playing);
        let props = session.TryGetMediaPropertiesAsync()?.join()?;
        let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
        let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
        let album = props.AlbumTitle().map(|s| s.to_string()).unwrap_or_default();
        let key = (title.clone(), artist.clone(), album.clone());
        let cached = if fresh {
            None
        } else {
            THUMB.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|(k, _)| *k == key).map(|(_, t)| t)
        };
        let thumbnail = match cached {
            Some(t) => Some(t),
            None => {
                let t = props.Thumbnail().ok().and_then(|r| thumbnail(&r).ok());
                *THUMB.lock().unwrap_or_else(|e| e.into_inner()) = t.clone().map(|data| (key, data));
                t
            }
        };
        Ok(NowPlaying { title, artist, album, playing, source, source_exe, thumbnail })
    }

    fn thumbnail(reference: &windows::Storage::Streams::IRandomAccessStreamReference) -> Result<String> {
        let stream = reference.OpenReadAsync()?.join()?;
        let size = stream.Size()?;
        if size == 0 || size > MAX_THUMBNAIL {
            bail!("thumbnail of {size} bytes skipped");
        }
        let content_type = stream.ContentType().map(|s| s.to_string()).unwrap_or_default();
        let content_type = if content_type.is_empty() { "image/jpeg".to_string() } else { content_type };
        let reader = DataReader::CreateDataReader(&stream)?;
        let loaded = reader.LoadAsync(size as u32)?.join()?;
        let mut bytes = vec![0u8; loaded as usize];
        reader.ReadBytes(&mut bytes)?;
        Ok(format!("data:{content_type};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
    }

    pub fn control(action: MediaAction) -> Result<()> {
        let session = session().ok_or_else(|| anyhow!("Nessuna musica in riproduzione"))?;
        let done = match action {
            MediaAction::Play => session.TryPlayAsync()?.join()?,
            MediaAction::Pause => session.TryPauseAsync()?.join()?,
            MediaAction::Toggle => session.TryTogglePlayPauseAsync()?.join()?,
            MediaAction::Next => session.TrySkipNextAsync()?.join()?,
            MediaAction::Prev => session.TrySkipPreviousAsync()?.join()?,
        };
        if !done {
            bail!("Il lettore non ha risposto");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;
    use tauri::AppHandle;

    use super::{MediaAction, NowPlaying};

    fn fake(playing: bool) -> NowPlaying {
        NowPlaying {
            title: "Brano di prova".into(),
            artist: "Artista".into(),
            album: "Album".into(),
            playing,
            source: "Spotify".into(),
            source_exe: "spotify".into(),
            thumbnail: None,
        }
    }

    pub fn init(app: AppHandle) {
        super::publish(&app, Some(fake(true)));
    }

    pub fn control(action: MediaAction) -> Result<()> {
        let playing = super::now_playing().is_some_and(|n| n.playing);
        let next = match action {
            MediaAction::Play => true,
            MediaAction::Pause => false,
            MediaAction::Toggle => !playing,
            MediaAction::Next | MediaAction::Prev => playing,
        };
        *super::CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(fake(next));
        Ok(())
    }
}

/// Starts following the system's media session; cheap when there is none.
pub fn init(app: &AppHandle) {
    imp::init(app.clone());
}

/// Transport control on the current session. Blocking on Windows.
pub fn control(action: MediaAction) -> Result<()> {
    imp::control(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_exe_becomes_name_and_stem() {
        assert_eq!(source_of("Spotify.exe"), ("Spotify".into(), "spotify".into()));
        assert_eq!(source_of("C:\\Apps\\Player.exe"), ("Player".into(), "player".into()));
    }

    #[test]
    fn packaged_app_uses_known_table_or_app_part() {
        assert_eq!(source_of("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), ("Spotify".into(), "spotify".into()));
        assert_eq!(source_of("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), ("Lettore multimediale".into(), "music.ui".into()));
        assert_eq!(source_of("Some.Publisher.Player_abc123!App"), ("Player".into(), "player".into()));
        assert_eq!(source_of("Some.Publisher.Player_abc123!Radio"), ("Radio".into(), "radio".into()));
    }

    #[test]
    fn browsers_are_recognised() {
        assert_eq!(source_of("MSEdge"), ("Edge".into(), "msedge".into()));
        assert_eq!(source_of("Chrome"), ("Chrome".into(), "chrome".into()));
        assert_eq!(source_of("308046B0AF4A39CB"), ("Firefox".into(), "firefox".into()));
    }

    #[test]
    fn empty_id_falls_back() {
        assert_eq!(source_of(""), ("Musica".into(), String::new()));
    }
}
