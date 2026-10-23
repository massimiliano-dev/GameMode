//! Installed applications, packaged (MSIX/UWP) and desktop, as Windows lists
//! them in the Apps folder (`shell:AppsFolder`, the Start menu's catalogue).
//! They share the library with the games under `Store::App`, so favourites,
//! last-used times, profiles and search come for free; the UI keeps them on
//! their own screen. Icons are read through the shell's image factory and
//! cached as PNG under `artwork/app`.
//!
//! The enumeration is in-process COM (shell32): it works without Explorer,
//! which matters in console mode.
// The pure helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::LibraryProvider;
use crate::model::{Game, LaunchSpec, Store};

/// Side of the cached icons, in pixels.
pub const ICON_SIZE: i32 = 256;

pub struct AppsProvider {
    /// Where the icons are cached.
    pub icon_dir: PathBuf,
}

impl LibraryProvider for AppsProvider {
    fn store(&self) -> Store {
        Store::App
    }

    fn scan(&self) -> Result<Vec<Game>> {
        imp::scan(&self.icon_dir)
    }
}

/// The icon cache, inside the artwork folder the asset protocol may serve.
pub fn icon_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("artwork").join("app")
}

/// One entry of the Apps folder, before it becomes a `Game`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppEntry {
    pub aumid: String,
    pub name: String,
    /// Package family name of a packaged (MSIX/UWP) app.
    pub family: Option<String>,
    /// Executable a desktop shortcut points to, and the shortcut's arguments.
    pub target: Option<PathBuf>,
    pub args: Vec<String>,
}

impl AppEntry {
    pub fn is_packaged(&self) -> bool {
        self.family.as_deref().is_some_and(|f| !f.is_empty())
    }
}

/// Names of Start menu shortcuts that aren't applications.
const NOT_AN_APP: &[&str] = &["uninstall", "disinstall", "readme", "leggimi", "release notes", "note di rilascio"];

/// Whether an entry deserves a tile: packaged apps, and desktop shortcuts to
/// an executable, minus GameMode itself, uninstallers and documentation.
pub fn is_wanted(entry: &AppEntry) -> bool {
    if entry.aumid.trim().is_empty() || entry.name.trim().is_empty() {
        return false;
    }
    let name = entry.name.to_lowercase();
    if NOT_AN_APP.iter().any(|w| name.contains(w)) {
        return false;
    }
    if entry.is_packaged() {
        return true;
    }
    // Desktop entries without a known executable (Control Panel, Run, File
    // Explorer, web links) have nothing GameMode can start or follow.
    let Some(target) = entry.target.as_deref() else { return false };
    let file = file_name_lower(target);
    if !file.ends_with(".exe") {
        return false;
    }
    if file == "gamemode.exe" || file.starts_with("unins") || file.contains("uninstall") {
        return false;
    }
    true
}

/// The install folder to follow a desktop app's processes by: the
/// executable's folder, unless it is a shared system one (every process in
/// `C:\Windows\System32` would count as the app).
pub fn install_dir_of(target: &Path, windows_dir: Option<&Path>) -> Option<PathBuf> {
    // Split on either separator, so the logic reads the same on every host.
    let text = target.to_string_lossy();
    let parent = &text[..text.rfind(['\\', '/'])?];
    let normalized = crate::launcher::normalize(Path::new(parent));
    // A drive root is no install folder either.
    if normalized.len() <= 3 {
        return None;
    }
    if let Some(win) = windows_dir {
        let win = crate::launcher::normalize(win);
        if normalized == win || normalized.starts_with(&format!("{win}/")) {
            return None;
        }
    }
    Some(PathBuf::from(parent))
}

/// Keeps the apps apart from the library's games. A shortcut into a game's
/// folder is the game itself (EA and GOG installers add one) and goes. A
/// launcher whose folder holds games (Steam, GOG Galaxy, Ubisoft Connect)
/// loses its install folder and is followed by its own process only:
/// otherwise its session would take in the games below it, and closing the
/// app would close them too.
pub fn separate_from_games(games: &mut Vec<Game>) {
    use crate::launcher::normalize;
    let game_dirs: Vec<String> = games
        .iter()
        .filter(|g| g.store != Store::App)
        .filter_map(|g| g.install_dir.as_deref())
        .map(normalize)
        .filter(|d| d.len() > 3)
        .collect();
    let within = |path: &str, dir: &str| path == dir || path.starts_with(&format!("{dir}/"));
    games.retain(|g| match (&g.store, &g.launch) {
        (Store::App, LaunchSpec::Exe { path, .. }) => {
            let exe = normalize(path);
            !game_dirs.iter().any(|d| within(&exe, d))
        }
        _ => true,
    });
    for app in games.iter_mut().filter(|g| g.store == Store::App) {
        let holds_games = app.install_dir.as_deref().map(normalize).is_some_and(|dir| game_dirs.iter().any(|d| within(d, &dir)));
        if holds_games {
            app.install_dir = None;
        }
    }
}

/// Lowercase file name, whatever the separator.
fn file_name_lower(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase()
}

/// A library entry for the app. `icon` is its cached icon, when it exists.
pub fn to_game(entry: AppEntry, icon: Option<PathBuf>, windows_dir: Option<&Path>) -> Game {
    // Packaged apps start through the Apps folder even when the entry also
    // names an executable: the package identity is what Windows wants.
    let packaged = entry.is_packaged();
    let AppEntry { aumid, name, target, args, .. } = entry;
    let (launch, install_dir) = match target.filter(|_| !packaged) {
        Some(path) => {
            let dir = install_dir_of(&path, windows_dir);
            (LaunchSpec::Exe { path, args, cwd: None }, dir)
        }
        None => (LaunchSpec::AppsFolder { aumid: aumid.clone() }, None),
    };
    let mut game = Game::new(Store::App, &aumid, name, launch);
    game.install_dir = install_dir;
    if let Some(icon) = icon {
        game.artwork_checked_at = icon_version(&icon);
        game.artwork.cover = Some(icon.to_string_lossy().into_owned());
    }
    game
}

/// Cache busting for the icon's URL: the file's modification time.
fn icon_version(icon: &Path) -> Option<i64> {
    let modified = std::fs::metadata(icon).ok()?.modified().ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// The cached icon of one app: a stable name derived from its AUMID, which
/// may hold characters a file name can't.
pub fn icon_file(icon_dir: &Path, aumid: &str) -> PathBuf {
    // FNV-1a, 64 bit: short, stable, no extra crate.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in aumid.to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    icon_dir.join(format!("{hash:016x}.png"))
}

/// Splits a shortcut's argument string as the C runtime would: spaces
/// separate, double quotes group.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut pending = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                pending = true;
            }
            c if c.is_whitespace() && !quoted => {
                if pending {
                    out.push(std::mem::take(&mut current));
                    pending = false;
                }
            }
            c => {
                current.push(c);
                pending = true;
            }
        }
    }
    if pending {
        out.push(current);
    }
    out
}

/// Turns the pixels of a 32-bit top-down Windows bitmap (BGRA) into RGBA for
/// the PNG. The shell hands icons over with premultiplied alpha, but a
/// bitmap whose colours exceed their alpha can't be premultiplied and is
/// taken as straight; one with no alpha at all (24-bit art padded to 32) is
/// opaque.
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let pixels = bgra.chunks_exact(4);
    let any_alpha = pixels.clone().any(|p| p[3] != 0);
    let premultiplied = any_alpha && pixels.clone().all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]);
    let mut out = Vec::with_capacity(bgra.len());
    for p in pixels {
        let (b, g, r, a) = (p[0], p[1], p[2], p[3]);
        let (r, g, b, a) = if !any_alpha {
            (r, g, b, 255)
        } else if premultiplied && a > 0 && a < 255 {
            let un = |c: u8| ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            (un(r), un(g), un(b), a)
        } else {
            (r, g, b, a)
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

/// Writes RGBA pixels as a PNG file, atomically.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    anyhow::ensure!(rgba.len() == (width * height * 4) as usize, "pixel buffer doesn't match {width}x{height}");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("png.part");
    {
        let file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(rgba)?;
        writer.finish()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(windows)]
mod imp {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use anyhow::{Context, Result};
    use windows::core::{Interface, HSTRING, PWSTR};
    use windows::Management::Deployment::PackageManager;
    use windows::Win32::Foundation::{PROPERTYKEY, SIZE};
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
        HGDIOBJ,
    };
    use windows::Win32::Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_AppUserModel_IsDestListSeparator, PKEY_Link_Arguments, PKEY_Link_TargetParsingPath};
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IEnumShellItems, IShellItem, IShellItem2, IShellItemImageFactory, SHGetKnownFolderItem, BHID_EnumItems, FOLDERID_AppsFolder,
        KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    use super::{bgra_to_rgba, icon_file, is_wanted, split_args, to_game, write_png, AppEntry, ICON_SIZE};
    use crate::model::Game;

    /// `System.AppUserModel.PackageFamilyName`: not in the `windows` crate.
    const PKEY_PACKAGE_FAMILY_NAME: PROPERTYKEY =
        PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 17 };

    pub fn scan(icon_dir: &Path) -> Result<Vec<Game>> {
        // The scan runs in its own thread: a single-threaded apartment of
        // its own, as the shell prefers.
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let result = scan_in_apartment(icon_dir);
        if init.is_ok() {
            unsafe { CoUninitialize() };
        }
        result
    }

    fn scan_in_apartment(icon_dir: &Path) -> Result<Vec<Game>> {
        let windows_dir = std::env::var_os("SystemRoot").map(PathBuf::from);
        let packages = PackageManager::new().ok();
        let mut games = Vec::new();
        let mut seen = HashSet::new();
        for (item, entry) in entries()? {
            if !is_wanted(&entry) || !seen.insert(entry.aumid.to_lowercase()) {
                continue;
            }
            // Xbox / Game Pass titles are games: the Xbox provider has them.
            if entry.is_packaged() && packages.as_ref().is_some_and(|m| is_game_package(m, entry.family.as_deref().unwrap_or(""))) {
                continue;
            }
            let icon = icon_file(icon_dir, &entry.aumid);
            if !icon.is_file() {
                if let Err(err) = save_icon(&item, &icon) {
                    eprintln!("icon of {}: {err:#}", entry.name);
                }
            }
            let icon = icon.is_file().then_some(icon);
            games.push(to_game(entry, icon, windows_dir.as_deref()));
        }
        Ok(games)
    }

    /// Every item of the Apps folder with what GameMode needs to know about it.
    fn entries() -> Result<Vec<(IShellItem, AppEntry)>> {
        let mut out = Vec::new();
        unsafe {
            let folder: IShellItem =
                SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None).context("cartella delle app non disponibile")?;
            let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems).context("elenco delle app non disponibile")?;
            loop {
                let mut batch: [Option<IShellItem>; 16] = Default::default();
                let mut fetched = 0u32;
                let _ = items.Next(&mut batch, Some(&mut fetched));
                if fetched == 0 {
                    break;
                }
                for item in batch.into_iter().take(fetched as usize).flatten() {
                    if let Some(entry) = describe(&item) {
                        out.push((item, entry));
                    }
                }
            }
        }
        Ok(out)
    }

    unsafe fn describe(item: &IShellItem) -> Option<AppEntry> {
        unsafe {
            let props: IShellItem2 = item.cast().ok()?;
            if props.GetBool(&PKEY_AppUserModel_IsDestListSeparator).ok().is_some_and(|b| b.as_bool()) {
                return None;
            }
            let aumid = take_string(props.GetString(&PKEY_AppUserModel_ID).ok()?);
            // The display name is already localized (`ms-resource:` resolved).
            let name = take_string(item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?);
            let family = props.GetString(&PKEY_PACKAGE_FAMILY_NAME).ok().map(|p| take_string(p)).filter(|f| !f.is_empty());
            let target = props
                .GetString(&PKEY_Link_TargetParsingPath)
                .ok()
                .map(|p| take_string(p))
                .filter(|t| !t.is_empty())
                .map(PathBuf::from)
                .filter(|t| t.is_file());
            let args = props.GetString(&PKEY_Link_Arguments).ok().map(|p| take_string(p)).map(|a| split_args(&a)).unwrap_or_default();
            Some(AppEntry { aumid, name, family, target, args })
        }
    }

    /// Owns and frees a string the shell allocated.
    unsafe fn take_string(p: PWSTR) -> String {
        unsafe {
            if p.is_null() {
                return String::new();
            }
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    /// GDK titles ship a `MicrosoftGame.config`: the Xbox provider's criterion.
    fn is_game_package(manager: &PackageManager, family: &str) -> bool {
        let Ok(found) = manager.FindPackagesByUserSecurityIdPackageFamilyName(&HSTRING::new(), &HSTRING::from(family)) else {
            return false;
        };
        found
            .into_iter()
            .any(|p| p.InstalledPath().is_ok_and(|dir| Path::new(&dir.to_string_lossy()).join("MicrosoftGame.config").is_file()))
    }

    /// Renders the item's icon through the shell and caches it as PNG.
    fn save_icon(item: &IShellItem, path: &Path) -> Result<()> {
        unsafe {
            let factory: IShellItemImageFactory = item.cast()?;
            let bitmap = factory.GetImage(SIZE { cx: ICON_SIZE, cy: ICON_SIZE }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)?;
            let result = read_bitmap(bitmap).and_then(|(w, h, bgra)| write_png(path, w, h, &bgra_to_rgba(&bgra)));
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            result
        }
    }

    /// The bitmap's pixels as top-down 32-bit BGRA.
    unsafe fn read_bitmap(bitmap: HBITMAP) -> Result<(u32, u32, Vec<u8>)> {
        unsafe {
            let mut info = BITMAP::default();
            let size = std::mem::size_of::<BITMAP>() as i32;
            if GetObjectW(HGDIOBJ(bitmap.0), size, Some(&mut info as *mut BITMAP as *mut _)) == 0 {
                anyhow::bail!("GetObject failed");
            }
            let (width, height) = (info.bmWidth, info.bmHeight.abs());
            anyhow::ensure!(width > 0 && height > 0 && width <= 1024 && height <= 1024, "odd bitmap size {width}x{height}");
            let mut header = BITMAPINFO::default();
            header.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: top-down rows, as the PNG wants them.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            let hdc = GetDC(None);
            let lines = GetDIBits(hdc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr() as *mut _), &mut header, DIB_RGB_COLORS);
            ReleaseDC(None, hdc);
            anyhow::ensure!(lines == height, "GetDIBits read {lines} of {height} lines");
            Ok((width as u32, height as u32, pixels))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use anyhow::Result;

    use super::{to_game, AppEntry};
    use crate::model::Game;

    /// A few stand-ins, so the Apps screen has something to show on macOS.
    pub fn scan(_icon_dir: &Path) -> Result<Vec<Game>> {
        let apps = [
            ("Netflix", "4DF9E0F8.Netflix_mcm4njqhnhss8!App", Some("4DF9E0F8.Netflix_mcm4njqhnhss8")),
            ("Spotify", "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0")),
            ("Disney+", "Disney.37853ABE2AD56_6rarf9sa4v8jt!App", Some("Disney.37853ABE2AD56_6rarf9sa4v8jt")),
            ("VLC media player", r"{6D809377-6AF0-444B-8957-A3773F02200E}\VideoLAN\VLC\vlc.exe", None),
        ];
        Ok(apps
            .into_iter()
            .map(|(name, aumid, family)| {
                let mut entry = AppEntry { aumid: aumid.into(), name: name.into(), family: family.map(String::from), ..Default::default() };
                if family.is_none() {
                    // A short-lived process stands in for the app, like the mock games.
                    entry.target = Some("/bin/sleep".into());
                    entry.args = vec!["20".into()];
                }
                to_game(entry, None, None)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desktop(name: &str, exe: &str) -> AppEntry {
        AppEntry { aumid: format!("{{guid}}\\Vendor\\{exe}"), name: name.into(), target: Some(PathBuf::from(exe)), ..Default::default() }
    }

    #[test]
    fn keeps_packaged_apps_and_desktop_executables() {
        let packaged = AppEntry { aumid: "A.B_123!App".into(), name: "Netflix".into(), family: Some("A.B_123".into()), ..Default::default() };
        assert!(is_wanted(&packaged));
        assert!(is_wanted(&desktop("VLC media player", r"C:\Program Files\VideoLAN\VLC\vlc.exe")));
    }

    #[test]
    fn drops_uninstallers_documentation_and_gamemode() {
        assert!(!is_wanted(&desktop("Uninstall VLC", r"C:\Program Files\VideoLAN\VLC\uninstall.exe")));
        assert!(!is_wanted(&desktop("Disinstalla Foo", r"C:\Foo\foo.exe")));
        assert!(!is_wanted(&desktop("Foo", r"C:\Foo\unins000.exe")));
        assert!(!is_wanted(&desktop("Readme", r"C:\Foo\readme.txt")));
        assert!(!is_wanted(&desktop("Foo website", r"C:\Foo\site.url")));
        assert!(!is_wanted(&desktop("GameMode", r"C:\Users\x\AppData\Local\GameMode\gamemode.exe")));
        // Control Panel, Run…: no executable to start.
        assert!(!is_wanted(&AppEntry { aumid: "Microsoft.Windows.ControlPanel".into(), name: "Pannello di controllo".into(), ..Default::default() }));
        assert!(!is_wanted(&AppEntry { aumid: "x".into(), name: "  ".into(), family: Some("f".into()), ..Default::default() }));
    }

    #[test]
    fn apps_stay_apart_from_games() {
        let steam_game = {
            let mut g = Game::new(Store::Steam, "1", "Hades", LaunchSpec::Uri { uri: "steam://rungameid/1".into() });
            g.install_dir = Some(PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Hades"));
            g
        };
        let ea_game = {
            let mut g = Game::new(Store::Ea, "2", "FC", LaunchSpec::Uri { uri: "origin2://x".into() });
            g.install_dir = Some(PathBuf::from(r"C:\Program Files\EA Games\FC"));
            g
        };
        let app = |name: &str, exe: &str| to_game(desktop(name, exe), None, Some(Path::new(r"C:\Windows")));
        let mut games = vec![
            steam_game,
            ea_game,
            app("Steam", r"C:\Program Files (x86)\Steam\steam.exe"),
            app("FC", r"C:\Program Files\EA Games\FC\FC.exe"),
            app("FC tools", r"C:\Program Files\EA Games\FC\Tools\tool.exe"),
            app("VLC", r"C:\Program Files\VideoLAN\VLC\vlc.exe"),
        ];
        separate_from_games(&mut games);
        let names: Vec<&str> = games.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Hades", "FC", "Steam", "VLC"]);
        // The launcher is followed by its process only, not by its folder.
        assert_eq!(games[2].install_dir, None);
        assert_eq!(games[3].install_dir, Some(PathBuf::from(r"C:\Program Files\VideoLAN\VLC")));
        // The games keep theirs.
        assert!(games[0].install_dir.is_some());
    }

    #[test]
    fn install_dir_skips_system_folders() {
        let win = Path::new(r"C:\Windows");
        assert_eq!(
            install_dir_of(Path::new(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), Some(win)),
            Some(PathBuf::from(r"C:\Program Files\VideoLAN\VLC"))
        );
        assert_eq!(install_dir_of(Path::new(r"C:\Windows\System32\notepad.exe"), Some(win)), None);
        assert_eq!(install_dir_of(Path::new(r"C:\WINDOWS\notepad.exe"), Some(win)), None);
        assert_eq!(install_dir_of(Path::new(r"C:\tool.exe"), Some(win)), None);
    }

    #[test]
    fn packaged_apps_start_from_the_apps_folder_and_desktop_apps_from_their_exe() {
        let packaged = AppEntry { aumid: "A.B_123!App".into(), name: "Netflix".into(), family: Some("A.B_123".into()), ..Default::default() };
        let game = to_game(packaged, None, None);
        assert_eq!(game.id, "app:A.B_123!App");
        assert_eq!(game.store, Store::App);
        assert_eq!(game.launch, LaunchSpec::AppsFolder { aumid: "A.B_123!App".into() });
        assert_eq!(game.install_dir, None);

        let mut entry = desktop("Discord", r"C:\Users\x\AppData\Local\Discord\Update.exe");
        entry.args = vec!["--processStart".into(), "Discord.exe".into()];
        let game = to_game(entry, None, Some(Path::new(r"C:\Windows")));
        assert_eq!(
            game.launch,
            LaunchSpec::Exe {
                path: PathBuf::from(r"C:\Users\x\AppData\Local\Discord\Update.exe"),
                args: vec!["--processStart".into(), "Discord.exe".into()],
                cwd: None
            }
        );
        assert_eq!(game.install_dir, Some(PathBuf::from(r"C:\Users\x\AppData\Local\Discord")));
    }

    #[test]
    fn icon_file_is_stable_and_case_insensitive() {
        let dir = Path::new("/cache");
        let a = icon_file(dir, "A.B_123!App");
        assert_eq!(a, icon_file(dir, "a.b_123!app"));
        assert_ne!(a, icon_file(dir, "A.B_123!Other"));
        assert!(a.to_string_lossy().ends_with(".png"));
        assert_eq!(a.file_name().unwrap().len(), 16 + 4);
    }

    #[test]
    fn splits_shortcut_arguments() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("--processStart Discord.exe"), vec!["--processStart", "Discord.exe"]);
        assert_eq!(split_args(r#"-f "C:\My Files\a.txt"  --x"#), vec!["-f", r"C:\My Files\a.txt", "--x"]);
        assert_eq!(split_args(r#""""#), vec![""]);
    }

    #[test]
    fn converts_bgra_handling_alpha() {
        // Premultiplied: half-transparent red comes back as full red.
        let premultiplied = [0, 0, 128, 128, 0, 0, 0, 0];
        assert_eq!(bgra_to_rgba(&premultiplied), vec![255, 0, 0, 128, 0, 0, 0, 0]);
        // Straight alpha (colour above alpha): left alone.
        let straight = [0, 0, 255, 128, 10, 20, 30, 255];
        assert_eq!(bgra_to_rgba(&straight), vec![255, 0, 0, 128, 30, 20, 10, 255]);
        // No alpha channel at all: opaque.
        let opaque = [1, 2, 3, 0, 4, 5, 6, 0];
        assert_eq!(bgra_to_rgba(&opaque), vec![3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn writes_a_png() {
        let dir = std::env::temp_dir().join(format!("gamemode-apps-{}", std::process::id()));
        let path = dir.join("icon.png");
        let rgba = vec![255u8; 2 * 2 * 4];
        write_png(&path, 2, 2, &rgba).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(write_png(&path, 3, 3, &rgba).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
