//! Screenshots of the running game, taken from the controller (View+RB held)
//! or from the overlay. Windows Graphics Capture grabs the monitor the game
//! is on — no code injection, no package identity, no consent prompt — with
//! a GDI copy of the screen as fallback. PNGs go to Pictures\GameMode\<game>,
//! one folder per game with a small sidecar saying which game it is, so the
//! folder can carry the game's name for people and still be matched by id.

// The controller trigger and the shutter sound only exist on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::launcher::Phase;
use crate::model::now;
use crate::{capture_osd, osd, overlay, window, AppState};

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub game_id: String,
    pub game_name: String,
    pub path: PathBuf,
    /// Unix seconds.
    pub taken_at: i64,
}

/// Written in each game's folder: the folder is named after the game for
/// people, this says which game it is for GameMode.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct FolderInfo {
    game_id: String,
    name: String,
}

const INFO_FILE: &str = ".gamemode.json";
/// File names are local times: "2026-10-23 21-05-42.png".
const TIME_FORMAT: &str = "%Y-%m-%d %H-%M-%S";
const STAMP_LEN: usize = 19;
/// Windows refuses these as folder names, whatever the extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];
/// After the overlay closes, how long the game takes to be back on screen.
const SETTLE: Duration = Duration::from_millis(350);
/// One screenshot at a time: holding the combo doesn't fire a burst.
static BUSY: AtomicBool = AtomicBool::new(false);

/// A decoded frame, 8-bit RGB rows top to bottom.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Where screenshots live: Pictures\GameMode, or the app data folder when
/// there is no Pictures folder.
pub fn root(data_dir: &Path) -> PathBuf {
    dirs::picture_dir().map(|p| p.join("GameMode")).unwrap_or_else(|| data_dir.join("captures"))
}

/// Creates the folder and lets the gallery load images from it.
pub fn init(app: &AppHandle) {
    let root = root(&app.state::<AppState>().data_dir);
    let _ = std::fs::create_dir_all(&root);
    if let Err(err) = app.asset_protocol_scope().allow_directory(&root, true) {
        eprintln!("captures folder not readable by the gallery: {err}");
    }
}

/// Takes a screenshot of the running game and tells the OSD and the
/// launcher about it. Blocking: call it off the main thread.
///
/// `Ok(None)` means the shot failed after GameMode's windows had already
/// left the screen: the failure is on the OSD, the only thing still in
/// view. `Err` is a failure before that (already busy, no game), for the
/// caller to show where the request came from.
pub fn take(app: &AppHandle) -> Result<Option<Capture>> {
    if BUSY.swap(true, Ordering::SeqCst) {
        bail!("Cattura già in corso");
    }
    let out = running(app).map(|game| match shoot(app, game) {
        Ok(capture) => Some(capture),
        Err(err) => {
            eprintln!("screenshot failed: {err:#}");
            capture_osd::show_error(app, &format!("{err:#}"));
            None
        }
    });
    BUSY.store(false, Ordering::SeqCst);
    out
}

/// `take` on its own thread, for the controller combo. Early failures stay
/// silent: "already busy" while a slow shot is still grabbing the screen
/// would otherwise end up in that very picture.
pub fn take_async(app: AppHandle) {
    std::thread::spawn(move || {
        if let Err(err) = take(&app) {
            eprintln!("screenshot not taken: {err:#}");
        }
    });
}

/// The game being played: its id, name and processes.
fn running(app: &AppHandle) -> Result<(String, String, Vec<u32>)> {
    let state = app.state::<AppState>();
    let session = state.launcher.current().filter(|s| s.phase == Phase::Running).context("Nessun gioco in corso")?;
    Ok((session.game_id, session.game_name, state.launcher.running_pids()))
}

fn shoot(app: &AppHandle, (game_id, game_name, pids): (String, String, Vec<u32>)) -> Result<Capture> {
    let state = app.state::<AppState>();
    let hwnd = window::game_window(&pids);
    // GameMode's own windows must not be in the picture: the card of a
    // previous shot and the volume indicator.
    capture_osd::hide(app);
    osd::hide(app);
    if overlay::is_open() {
        overlay::hide(app);
        window::resume_game(app, &pids);
        std::thread::sleep(SETTLE);
    }
    let frame = imp::grab(hwnd).context("Cattura dello schermo non riuscita")?;
    let root = root(&state.data_dir);
    let dir = folder_for(&root, &game_id, &game_name)?;
    let taken_at = now();
    let path = free_path(&dir, &stamp(taken_at));
    write_png(&path, &frame).with_context(|| format!("Salvataggio in {} non riuscito", path.display()))?;
    imp::shutter();
    let capture = Capture { game_id, game_name, path, taken_at };
    let _ = app.emit("capture://saved", &capture);
    capture_osd::show_saved(app, &capture, hwnd);
    Ok(capture)
}

/// Every screenshot, newest first; `game_id` keeps one game's only.
pub fn list(root: &Path, game_id: Option<&str>) -> Vec<Capture> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root) else { return out };
    for entry in dirs.flatten() {
        let dir = entry.path();
        let Some(info) = read_info(&dir) else { continue };
        if game_id.is_some_and(|id| id != info.game_id) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&dir) else { continue };
        for file in files.flatten() {
            let path = file.path();
            if !is_png(&path) {
                continue;
            }
            let taken_at = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(parse_stamp)
                .or_else(|| modified(&path))
                .unwrap_or(0);
            out.push(Capture { game_id: info.game_id.clone(), game_name: info.name.clone(), path, taken_at });
        }
    }
    out.sort_by(|a, b| b.taken_at.cmp(&a.taken_at).then_with(|| b.path.cmp(&a.path)));
    out
}

/// Removes one screenshot; only files inside the captures folder.
pub fn delete(root: &Path, path: &Path) -> Result<()> {
    if !is_png(path) || !inside(root, path) {
        bail!("Percorso non valido");
    }
    std::fs::remove_file(path).with_context(|| format!("eliminazione di {}", path.display()))
}

/// The folder to open for a game, or the root; whichever exists.
pub fn folder_of(root: &Path, game_id: Option<&str>) -> PathBuf {
    game_id.and_then(|id| find_folder(root, id)).unwrap_or_else(|| root.to_path_buf())
}

/// A folder name from a game's name: Windows-safe, trimmed, never empty.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = if c.is_control() || "<>:\"/\\|?*".contains(c) { ' ' } else { c };
        if c == ' ' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out: String = out.chars().take(60).collect();
    let out = out.trim().trim_end_matches('.').trim();
    if out.is_empty() {
        return "Gioco".into();
    }
    if RESERVED.contains(&out.to_ascii_uppercase().as_str()) {
        return format!("{out} (gioco)");
    }
    out.to_string()
}

fn stamp(at: i64) -> String {
    chrono::Local
        .timestamp_opt(at, 0)
        .single()
        .map(|t| t.format(TIME_FORMAT).to_string())
        .unwrap_or_else(|| at.to_string())
}

/// The time in a file name made by `stamp` (a " (2)" suffix is ignored).
pub fn parse_stamp(stem: &str) -> Option<i64> {
    let head: String = stem.chars().take(STAMP_LEN).collect();
    let naive = chrono::NaiveDateTime::parse_from_str(&head, TIME_FORMAT).ok()?;
    naive.and_local_timezone(chrono::Local).single().map(|t| t.timestamp())
}

fn is_png(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
}

fn inside(root: &Path, path: &Path) -> bool {
    match (root.canonicalize(), path.canonicalize()) {
        (Ok(root), Ok(path)) => path.starts_with(root),
        _ => false,
    }
}

fn modified(path: &Path) -> Option<i64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    time.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
}

fn read_info(dir: &Path) -> Option<FolderInfo> {
    let bytes = std::fs::read(dir.join(INFO_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn find_folder(root: &Path, game_id: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|dir| read_info(dir).is_some_and(|i| i.game_id == game_id))
}

/// The game's folder, created on first use. The name stays readable; a
/// second game with the same name gets a numbered folder.
fn folder_for(root: &Path, game_id: &str, name: &str) -> Result<PathBuf> {
    if let Some(dir) = find_folder(root, game_id) {
        return Ok(dir);
    }
    let base = slug(name);
    for attempt in 1..=20u32 {
        let dir = if attempt == 1 { root.join(&base) } else { root.join(format!("{base} ({attempt})")) };
        if read_info(&dir).is_some() {
            continue;
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creazione di {}", dir.display()))?;
        let info = FolderInfo { game_id: game_id.into(), name: name.into() };
        std::fs::write(dir.join(INFO_FILE), serde_json::to_vec_pretty(&info)?)?;
        return Ok(dir);
    }
    bail!("Troppe cartelle con lo stesso nome in {}", root.display())
}

/// `<stem>.png`, or `<stem> (2).png`… when that name is taken.
fn free_path(dir: &Path, stem: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} ({n}).png"))).find(|p| !p.exists()).unwrap()
}

pub fn write_png(path: &Path, frame: &Frame) -> Result<()> {
    if frame.rgb.len() != (frame.width * frame.height * 3) as usize {
        bail!("frame di {}×{} con {} byte", frame.width, frame.height, frame.rgb.len());
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&frame.rgb)?;
    writer.finish()?;
    Ok(())
}

/// A camera-like double click, as a 16-bit mono WAV in memory: nothing to
/// ship, and `PlaySound` plays it even while a game owns the audio session.
pub fn shutter_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    const MS: u32 = 130;
    let mut seed: u32 = 0x2545_F491;
    let mut noise = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    let click = |t: f32, start: f32, decay: f32| if t < start { 0.0 } else { (-(t - start) / decay).exp() };
    let samples: Vec<i16> = (0..RATE * MS / 1000)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let envelope = click(t, 0.0, 0.010) + 0.7 * click(t, 0.075, 0.014);
            (noise() * envelope * 0.32 * i16::MAX as f32) as i16
        })
        .collect();
    let data_len = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(windows)]
pub(crate) mod imp {
    use std::sync::OnceLock;
    use std::time::Duration;

    use anyhow::{anyhow, bail, Context, Result};
    use windows::core::{factory, Interface, Ref, PCWSTR};
    use windows::Foundation::TypedEventHandler;
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Win32::Foundation::{HMODULE, POINT};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11Texture2D, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetMonitorInfoW,
        MonitorFromPoint, MonitorFromWindow, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT,
        DIB_RGB_COLORS, HGDIOBJ, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, SRCCOPY,
    };
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
    use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

    use super::Frame;
    use crate::window::win::hwnd;

    /// How long to wait for the first frame of a capture.
    const FRAME_TIMEOUT: Duration = Duration::from_secs(3);

    /// The monitor a window is on, or the primary one.
    pub fn monitor_of(window: Option<isize>) -> HMONITOR {
        unsafe {
            match window {
                Some(raw) => MonitorFromWindow(hwnd(raw), MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY),
            }
        }
    }

    /// Physical bounds of a monitor: `(left, top, width, height)`.
    pub fn monitor_rect(monitor: HMONITOR) -> Option<(i32, i32, i32, i32)> {
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return None;
        }
        let r = info.rcMonitor;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }

    /// One frame of the monitor `window` is on.
    pub fn grab(window: Option<isize>) -> Result<Frame> {
        let monitor = monitor_of(window);
        match unsafe { graphics_capture(monitor) } {
            Ok(frame) => Ok(frame),
            Err(err) => {
                eprintln!("Windows Graphics Capture failed ({err:#}), copying the screen with GDI");
                unsafe { gdi(monitor) }.context("GDI")
            }
        }
    }

    /// Windows.Graphics.Capture of a monitor, from a plain desktop process
    /// (`IGraphicsCaptureItemInterop`, Windows 10 1903+).
    unsafe fn graphics_capture(monitor: HMONITOR) -> Result<Frame> {
        unsafe {
            // Already initialised threads answer S_FALSE or RPC_E_CHANGED_MODE: both fine.
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            let mut device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
            .context("D3D11CreateDevice")?;
            let device = device.ok_or_else(|| anyhow!("no D3D11 device"))?;
            let context = device.GetImmediateContext().context("immediate context")?;
            let dxgi: IDXGIDevice = device.cast().context("IDXGIDevice")?;
            let winrt_device: IDirect3DDevice =
                CreateDirect3D11DeviceFromDXGIDevice(&dxgi).context("WinRT device")?.cast().context("IDirect3DDevice")?;
            let interop: IGraphicsCaptureItemInterop =
                factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().context("capture interop")?;
            let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor).context("CreateForMonitor")?;
            let size = item.Size().context("item size")?;
            if size.Width <= 0 || size.Height <= 0 {
                bail!("empty capture item");
            }
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                &winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                1,
                size,
            )
            .context("frame pool")?;
            let session = pool.CreateCaptureSession(&item).context("capture session")?;
            // Older Windows 10 builds lack these: the capture works without.
            let _ = session.SetIsCursorCaptureEnabled(false);
            let _ = session.SetIsBorderRequired(false);

            let (tx, rx) = std::sync::mpsc::channel();
            let handler = TypedEventHandler::<Direct3D11CaptureFramePool, windows::core::IInspectable>::new(
                move |pool: Ref<Direct3D11CaptureFramePool>, _| {
                    if let Some(pool) = pool.as_ref() {
                        if let Ok(frame) = pool.TryGetNextFrame() {
                            let _ = tx.send(frame);
                        }
                    }
                    Ok(())
                },
            );
            let token = pool.FrameArrived(&handler).context("FrameArrived")?;
            session.StartCapture().context("StartCapture")?;
            let frame = rx.recv_timeout(FRAME_TIMEOUT);
            let _ = pool.RemoveFrameArrived(token);
            let _ = session.Close();
            let _ = pool.Close();
            let frame = frame.map_err(|_| anyhow!("no frame within {FRAME_TIMEOUT:?}"))?;

            let surface = frame.Surface().context("frame surface")?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().context("DXGI access")?;
            let texture: ID3D11Texture2D = access.GetInterface().context("frame texture")?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
                ..desc
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            device.CreateTexture2D(&staging_desc, None, Some(&mut staging)).context("staging texture")?;
            let staging = staging.ok_or_else(|| anyhow!("no staging texture"))?;
            context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).context("Map")?;
            // The texture may be padded beyond what the item shows.
            let content = frame.ContentSize().unwrap_or(size);
            let width = (content.Width.max(0) as u32).min(desc.Width) as usize;
            let height = (content.Height.max(0) as u32).min(desc.Height) as usize;
            let mut rgb = Vec::with_capacity(width * height * 3);
            let base = mapped.pData as *const u8;
            for y in 0..height {
                let row = std::slice::from_raw_parts(base.add(y * mapped.RowPitch as usize), width * 4);
                for px in row.chunks_exact(4) {
                    rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                }
            }
            context.Unmap(&staging, 0);
            let _ = frame.Close();
            Ok(Frame { width: width as u32, height: height as u32, rgb })
        }
    }

    /// Plain screen copy; black for exclusive-fullscreen games, washed out in
    /// HDR, but it works on every Windows.
    unsafe fn gdi(monitor: HMONITOR) -> Result<Frame> {
        let (left, top, width, height) = monitor_rect(monitor).ok_or_else(|| anyhow!("monitor bounds"))?;
        if width <= 0 || height <= 0 {
            bail!("empty monitor");
        }
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(mem, HGDIOBJ(bitmap.0));
            let copied = BitBlt(mem, 0, 0, width, height, Some(screen), left, top, SRCCOPY | CAPTUREBLT);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: rows top to bottom.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let lines = if copied.is_ok() {
                GetDIBits(mem, bitmap, 0, height as u32, Some(bgra.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS)
            } else {
                0
            };
            SelectObject(mem, previous);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            if lines != height {
                bail!("GetDIBits copied {lines} of {height} lines");
            }
            let mut rgb = Vec::with_capacity((width * height * 3) as usize);
            for px in bgra.chunks_exact(4) {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
            Ok(Frame { width: width as u32, height: height as u32, rgb })
        }
    }

    /// The shutter click, without blocking.
    pub fn shutter() {
        static WAV: OnceLock<Vec<u8>> = OnceLock::new();
        let wav = WAV.get_or_init(super::shutter_wav);
        unsafe {
            let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{bail, Result};

    use super::Frame;

    pub fn grab(_window: Option<isize>) -> Result<Frame> {
        bail!("Le catture funzionano solo su Windows")
    }

    pub fn shutter() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gamemode-captures-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slug_is_safe_for_windows_folders() {
        assert_eq!(slug("Elden Ring"), "Elden Ring");
        assert_eq!(slug("Half-Life: Alyx"), "Half-Life Alyx");
        assert_eq!(slug("  A/B\\C*D?  "), "A B C D");
        assert_eq!(slug("Trailing..."), "Trailing");
        assert_eq!(slug(""), "Gioco");
        assert_eq!(slug("***"), "Gioco");
        assert_eq!(slug("con"), "con (gioco)");
        assert!(slug(&"x".repeat(100)).len() <= 60);
    }

    #[test]
    fn stamp_round_trips_and_ignores_suffixes() {
        let at = 1_700_000_000;
        let s = stamp(at);
        assert_eq!(s.len(), STAMP_LEN);
        assert_eq!(parse_stamp(&s), Some(at));
        assert_eq!(parse_stamp(&format!("{s} (2)")), Some(at));
        assert_eq!(parse_stamp("not a time"), None);
    }

    #[test]
    fn folders_are_per_game_and_survive_renames() {
        let root = temp_root("folders");
        let a = folder_for(&root, "steam:1", "Elden Ring").unwrap();
        assert_eq!(a, root.join("Elden Ring"));
        // Same name, other game: numbered.
        let b = folder_for(&root, "gog:1", "Elden Ring").unwrap();
        assert_eq!(b, root.join("Elden Ring (2)"));
        // Same game, new name: the old folder is kept.
        assert_eq!(folder_for(&root, "steam:1", "ELDEN RING").unwrap(), a);
        assert_eq!(folder_of(&root, Some("gog:1")), b);
        assert_eq!(folder_of(&root, Some("none")), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_and_delete_stay_inside_the_root() {
        let root = temp_root("list");
        let dir = folder_for(&root, "steam:1", "Game").unwrap();
        let frame = Frame { width: 2, height: 2, rgb: vec![255; 12] };
        let older = dir.join(format!("{}.png", stamp(1_700_000_000)));
        let newer = free_path(&dir, &stamp(1_700_000_100));
        write_png(&older, &frame).unwrap();
        write_png(&newer, &frame).unwrap();
        // A second shot in the same second gets a suffix.
        assert_eq!(free_path(&dir, &stamp(1_700_000_100)), dir.join(format!("{} (2).png", stamp(1_700_000_100))));
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let all = list(&root, None);
        assert_eq!(all.iter().map(|c| c.path.clone()).collect::<Vec<_>>(), vec![newer.clone(), older.clone()]);
        assert_eq!(all[0].game_name, "Game");
        assert!(list(&root, Some("other")).is_empty());

        let outside = temp_root("outside").join("x.png");
        write_png(&outside, &frame).unwrap();
        assert!(delete(&root, &outside).is_err());
        assert!(delete(&root, &dir.join("notes.txt")).is_err());
        delete(&root, &older).unwrap();
        assert_eq!(list(&root, Some("steam:1")).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(outside.parent().unwrap());
    }

    #[test]
    fn png_rejects_wrong_sizes() {
        let root = temp_root("png");
        let bad = Frame { width: 2, height: 2, rgb: vec![0; 5] };
        assert!(write_png(&root.join("bad.png"), &bad).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shutter_is_a_valid_wav() {
        let wav = shutter_wav();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize;
        assert_eq!(wav.len(), 44 + data_len);
        assert!(wav[44..].iter().any(|&b| b != 0));
    }
}
