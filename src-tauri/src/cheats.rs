//! Trucchi: a small memory editor for single-player games, in the overlay.
//! Search the game's memory for a value you see on screen (coins, health),
//! narrow the hits down as the value changes, then set or freeze it.
//!
//! Never for online games: anything with an anti-cheat is refused outright
//! (editing that memory gets accounts banned, and it's not what this is for).

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::AppState;

/// Above this, the first search was too vague to narrow down sensibly.
const MAX_HITS: usize = 4_000_000;
/// Hits listed in the overlay (with live values) once there are this few.
const SHOWN_HITS: usize = 20;
/// Bytes read at a time from the game.
const CHUNK: usize = 1 << 20;
/// How often frozen values are written back.
const FREEZE_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// 32-bit integer: most counters (coins, ammo, levels).
    Int,
    /// 32-bit float: health bars, stamina, positions.
    Float,
    /// 64-bit float.
    Double,
}

impl Kind {
    fn size(self) -> usize {
        match self {
            Kind::Int | Kind::Float => 4,
            Kind::Double => 8,
        }
    }

    fn align(self) -> usize {
        self.size()
    }

    fn decode(self, bytes: &[u8]) -> f64 {
        match self {
            Kind::Int => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            Kind::Float => f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            Kind::Double => f64::from_le_bytes(bytes[..8].try_into().unwrap()),
        }
    }

    fn encode(self, value: f64) -> Vec<u8> {
        match self {
            Kind::Int => (value.round() as i32).to_le_bytes().to_vec(),
            Kind::Float => (value as f32).to_le_bytes().to_vec(),
            Kind::Double => value.to_le_bytes().to_vec(),
        }
    }

    fn format(self, bytes: &[u8]) -> String {
        let v = self.decode(bytes);
        match self {
            Kind::Int => format!("{v:.0}"),
            _ if !v.is_finite() => "—".into(),
            _ => format!("{v:.2}").trim_end_matches('0').trim_end_matches('.').replace('.', ","),
        }
    }
}

/// What to look for: a value, with the slack a rounded on-screen number needs.
#[derive(Clone, Copy, Debug)]
pub struct Target {
    value: f64,
    tolerance: f64,
}

impl Target {
    /// "1250", "-3", "87,5" or "87.5". A whole number typed for a decimal
    /// kind matches anything that rounds to it (games round what they show).
    pub fn parse(text: &str, kind: Kind) -> Result<Self> {
        let clean = text.trim().replace(',', ".");
        let value: f64 = clean.parse().ok().filter(|v: &f64| v.is_finite()).context("Scrivi un numero")?;
        if kind == Kind::Int && (value.fract() != 0.0 || value.abs() > i32::MAX as f64) {
            bail!("Per i numeri interi scrivi un valore senza decimali");
        }
        let tolerance = match kind {
            Kind::Int => 0.0,
            _ if clean.contains('.') => 0.005,
            _ => 0.5,
        };
        Ok(Target { value, tolerance })
    }

    fn matches(&self, kind: Kind, bytes: &[u8]) -> bool {
        let v = kind.decode(bytes);
        v.is_finite() && (v - self.value).abs() <= self.tolerance
    }
}

/// A process' memory, as the scanner sees it (real game, or a fake in tests).
pub trait Memory {
    /// Writable committed regions: `(start, length)`.
    fn regions(&self) -> Vec<(u64, usize)>;
    fn read(&self, address: u64, buf: &mut [u8]) -> bool;
    fn write(&self, address: u64, data: &[u8]) -> bool;
}

/// First search: every aligned spot in writable memory holding the value.
pub fn first_scan(mem: &dyn Memory, kind: Kind, target: Target) -> Result<Vec<u64>> {
    let (size, align) = (kind.size(), kind.align());
    let mut hits = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    for (start, len) in mem.regions() {
        let mut offset = 0usize;
        while offset < len {
            let n = CHUNK.min(len - offset);
            let base = start + offset as u64;
            if mem.read(base, &mut buf[..n]) {
                let mut i = 0;
                while i + size <= n {
                    if target.matches(kind, &buf[i..i + size]) {
                        hits.push(base + i as u64);
                        if hits.len() > MAX_HITS {
                            bail!("Troppi risultati: cerca un valore meno comune (per esempio non 0, 1 o 100)");
                        }
                    }
                    i += align;
                }
            }
            offset += n;
        }
    }
    Ok(hits)
}

/// Next search: keeps the hits that now hold the new value. Reads a page at
/// a time, as hits come sorted.
pub fn next_scan(mem: &dyn Memory, kind: Kind, target: Target, hits: &[u64]) -> Vec<u64> {
    const PAGE: u64 = 4096;
    let size = kind.size();
    let mut page_at = u64::MAX;
    let mut page = vec![0u8; PAGE as usize];
    let mut page_ok = false;
    let mut kept = Vec::new();
    let mut value = vec![0u8; size];
    for &address in hits {
        let offset = (address % PAGE) as usize;
        if offset + size > PAGE as usize {
            // Straddles two pages: read it on its own.
            if mem.read(address, &mut value) && target.matches(kind, &value) {
                kept.push(address);
            }
            continue;
        }
        let start = address - offset as u64;
        if start != page_at {
            page_at = start;
            page_ok = mem.read(start, &mut page);
        }
        if page_ok && target.matches(kind, &page[offset..offset + size]) {
            kept.push(address);
        }
    }
    kept
}

// ---------- Live state ----------

struct Scan {
    pid: u32,
    kind: Kind,
    hits: Vec<u64>,
}

#[derive(Clone)]
struct Frozen {
    pid: u32,
    address: u64,
    kind: Kind,
    bytes: Vec<u8>,
}

static SCAN: Mutex<Option<Scan>> = Mutex::new(None);
static FROZEN: Mutex<Vec<Frozen>> = Mutex::new(Vec::new());
/// Anti-cheat verdict per game id (looking through the install folder is slow).
static GUARDED: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub address: u64,
    pub value: String,
    pub frozen: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ScanView {
    pub kind: Kind,
    pub count: usize,
    /// Listed only once the search is down to a handful.
    pub hits: Vec<Hit>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CheatsStatus {
    pub available: bool,
    /// Why not, when not available.
    pub reason: Option<String>,
    pub scan: Option<ScanView>,
    pub frozen: Vec<Hit>,
}

/// The game being played, as long as cheating in it is fine: its main
/// process (the one using the most memory).
fn game_process(app: &AppHandle) -> Result<u32> {
    let state = app.state::<AppState>();
    let session = state.launcher.current().context("Nessun gioco in corso")?;
    if session.phase != crate::launcher::Phase::Running {
        bail!("Aspetta che il gioco sia partito");
    }
    let pids = state.launcher.running_pids();
    if guarded(app, &session.game_id) {
        bail!("Trucchi disattivati: questo gioco ha un anti-cheat (gioco online)");
    }
    let mut sys = sysinfo::System::new();
    let targets: Vec<sysinfo::Pid> = pids.iter().map(|p| sysinfo::Pid::from_u32(*p)).collect();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&targets), true);
    pids.iter()
        .copied()
        .max_by_key(|p| sys.process(sysinfo::Pid::from_u32(*p)).map_or(0, |pr| pr.memory()))
        .context("Processo del gioco non trovato")
}

/// An anti-cheat in the install folder or among the game's processes.
fn guarded(app: &AppHandle, game_id: &str) -> bool {
    let mut cache = GUARDED.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(&known) = cache.get(game_id) {
        return known;
    }
    let dir = app
        .state::<AppState>()
        .db
        .lock()
        .view()
        .into_iter()
        .find(|g| g.id == game_id)
        .and_then(|g| g.install_dir);
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let service = sys.processes().values().any(|p| crate::freeze::is_anti_cheat_marker(&p.name().to_string_lossy()));
    let in_folder = dir.is_some_and(|d| crate::freeze::anti_cheat_in(&d));
    let verdict = service || in_folder;
    cache.insert(game_id.to_string(), verdict);
    verdict
}

fn view(mem: &dyn Memory, scan: &Scan, frozen: &[Frozen]) -> ScanView {
    let hits = if scan.hits.len() <= SHOWN_HITS {
        scan.hits.iter().map(|&a| hit(mem, scan.kind, a, frozen)).collect()
    } else {
        Vec::new()
    };
    ScanView { kind: scan.kind, count: scan.hits.len(), hits }
}

fn hit(mem: &dyn Memory, kind: Kind, address: u64, frozen: &[Frozen]) -> Hit {
    let mut bytes = vec![0u8; kind.size()];
    let value = if mem.read(address, &mut bytes) { kind.format(&bytes) } else { "—".into() };
    Hit { address, value, frozen: frozen.iter().any(|f| f.address == address) }
}

pub fn status(app: &AppHandle) -> CheatsStatus {
    let pid = match game_process(app) {
        Ok(pid) => pid,
        Err(err) => {
            return CheatsStatus { available: false, reason: Some(format!("{err:#}")), scan: None, frozen: Vec::new() }
        }
    };
    let Ok(mem) = imp::Process::open(pid) else {
        return CheatsStatus {
            available: false,
            reason: Some("GameMode non può leggere questo gioco (avviato come amministratore?)".into()),
            scan: None,
            frozen: Vec::new(),
        };
    };
    let frozen: Vec<Frozen> = FROZEN.lock().unwrap().iter().filter(|f| f.pid == pid).cloned().collect();
    let mut scan = SCAN.lock().unwrap();
    // A search from another game (or an earlier run) means nothing here.
    if scan.as_ref().is_some_and(|s| s.pid != pid) {
        *scan = None;
    }
    CheatsStatus {
        available: true,
        reason: None,
        scan: scan.as_ref().map(|s| view(&mem, s, &frozen)),
        frozen: frozen.iter().map(|f| hit(&mem, f.kind, f.address, &frozen)).collect(),
    }
}

/// A search: `fresh` starts over, otherwise it narrows the current hits.
pub fn search(app: &AppHandle, text: &str, kind: Kind, fresh: bool) -> Result<ScanView> {
    let pid = game_process(app)?;
    let mem = imp::Process::open(pid)?;
    let target = Target::parse(text, kind)?;
    let previous = SCAN.lock().unwrap().take().filter(|s| !fresh && s.pid == pid && s.kind == kind);
    let hits = match previous {
        Some(scan) => next_scan(&mem, kind, target, &scan.hits),
        None => first_scan(&mem, kind, target)?,
    };
    let scan = Scan { pid, kind, hits };
    let frozen = FROZEN.lock().unwrap().clone();
    let shown = view(&mem, &scan, &frozen);
    *SCAN.lock().unwrap() = Some(scan);
    Ok(shown)
}

fn scan_kind(pid: u32) -> Result<Kind> {
    SCAN.lock().unwrap().as_ref().filter(|s| s.pid == pid).map(|s| s.kind).context("Fai prima una ricerca")
}

/// Writes a new value once.
pub fn set(app: &AppHandle, address: u64, text: &str) -> Result<()> {
    let pid = game_process(app)?;
    let kind = scan_kind(pid)?;
    let target = Target::parse(text, kind)?;
    let bytes = kind.encode(target.value);
    if !imp::Process::open(pid)?.write(address, &bytes) {
        bail!("Il gioco non ha accettato il nuovo valore");
    }
    // A frozen value follows what was set.
    for f in FROZEN.lock().unwrap().iter_mut().filter(|f| f.pid == pid && f.address == address) {
        f.bytes = bytes.clone();
    }
    Ok(())
}

/// Keeps a value fixed: `text` or, when empty, what it is now.
pub fn freeze(app: &AppHandle, address: u64, text: Option<&str>) -> Result<()> {
    let pid = game_process(app)?;
    let kind = scan_kind(pid)?;
    let mem = imp::Process::open(pid)?;
    let bytes = match text.filter(|t| !t.trim().is_empty()) {
        Some(t) => kind.encode(Target::parse(t, kind)?.value),
        None => {
            let mut now = vec![0u8; kind.size()];
            if !mem.read(address, &mut now) {
                bail!("Valore non più leggibile: rifai la ricerca");
            }
            now
        }
    };
    let mut frozen = FROZEN.lock().unwrap();
    frozen.retain(|f| !(f.pid == pid && f.address == address));
    frozen.push(Frozen { pid, address, kind, bytes });
    start_freezer();
    Ok(())
}

pub fn unfreeze(address: u64) {
    FROZEN.lock().unwrap().retain(|f| f.address != address);
}

/// Forgets the search and every frozen value (the game ended).
pub fn reset() {
    *SCAN.lock().unwrap() = None;
    FROZEN.lock().unwrap().clear();
}

/// Writes frozen values back, over and over, while there are any.
fn start_freezer() {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        let mut open: HashMap<u32, imp::Process> = HashMap::new();
        loop {
            let frozen = FROZEN.lock().unwrap().clone();
            if frozen.is_empty() {
                break;
            }
            let mut gone = Vec::new();
            for f in &frozen {
                let process = match open.entry(f.pid) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => match imp::Process::open(f.pid) {
                        Ok(p) => e.insert(p),
                        Err(_) => {
                            gone.push(f.pid);
                            continue;
                        }
                    },
                };
                if !process.write(f.address, &f.bytes) && !process.alive() {
                    gone.push(f.pid);
                }
            }
            if !gone.is_empty() {
                FROZEN.lock().unwrap().retain(|f| !gone.contains(&f.pid));
                open.retain(|pid, _| !gone.contains(pid));
            }
            std::thread::sleep(FREEZE_EVERY);
        }
        RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
    use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, MEM_IMAGE, MEM_PRIVATE, PAGE_EXECUTE_READWRITE,
        PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_NOCACHE, PAGE_READWRITE, PAGE_WRITECOMBINE, PAGE_WRITECOPY,
    };
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
        PROCESS_VM_WRITE,
    };

    pub struct Process(HANDLE);

    // The handle is only a number; Windows handles are usable from any thread.
    unsafe impl Send for Process {}

    impl Process {
        pub fn open(pid: u32) -> Result<Self> {
            let access = PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION | PROCESS_QUERY_INFORMATION;
            let handle = unsafe { OpenProcess(access, false, pid) }
                .context("GameMode non può leggere questo gioco (avviato come amministratore?)")?;
            Ok(Process(handle))
        }

        pub fn alive(&self) -> bool {
            let mut code = 0u32;
            unsafe { GetExitCodeProcess(self.0, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32 }
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    impl super::Memory for Process {
        fn regions(&self) -> Vec<(u64, usize)> {
            let writable = PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
            let mut regions = Vec::new();
            let mut address = 0u64;
            loop {
                let mut info = MEMORY_BASIC_INFORMATION::default();
                let got = unsafe {
                    VirtualQueryEx(
                        self.0,
                        Some(address as *const core::ffi::c_void),
                        &mut info,
                        std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                if got == 0 {
                    break;
                }
                let base = info.BaseAddress as u64;
                let size = info.RegionSize;
                let usable = info.State == MEM_COMMIT
                    && (info.Type == MEM_PRIVATE || info.Type == MEM_IMAGE)
                    && (info.Protect & writable).0 != 0
                    && (info.Protect & (PAGE_GUARD | PAGE_NOCACHE | PAGE_WRITECOMBINE)).0 == 0;
                if usable {
                    regions.push((base, size));
                }
                let next = base.saturating_add(size as u64);
                if next <= address {
                    break;
                }
                address = next;
            }
            regions
        }

        fn read(&self, address: u64, buf: &mut [u8]) -> bool {
            let mut read = 0usize;
            let ok = unsafe {
                ReadProcessMemory(
                    self.0,
                    address as *const core::ffi::c_void,
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    buf.len(),
                    Some(&mut read),
                )
            };
            ok.is_ok() && read == buf.len()
        }

        fn write(&self, address: u64, data: &[u8]) -> bool {
            unsafe {
                WriteProcessMemory(
                    self.0,
                    address as *const core::ffi::c_void,
                    data.as_ptr() as *const core::ffi::c_void,
                    data.len(),
                    None,
                )
            }
            .is_ok()
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::{bail, Result};

    pub struct Process;

    impl Process {
        pub fn open(_pid: u32) -> Result<Self> {
            bail!("Trucchi disponibili solo su Windows")
        }

        pub fn alive(&self) -> bool {
            false
        }
    }

    impl super::Memory for Process {
        fn regions(&self) -> Vec<(u64, usize)> {
            Vec::new()
        }

        fn read(&self, _address: u64, _buf: &mut [u8]) -> bool {
            false
        }

        fn write(&self, _address: u64, _data: &[u8]) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// One region of fake memory starting at 0x10000.
    struct Fake(RefCell<Vec<u8>>);

    const BASE: u64 = 0x10000;

    impl Fake {
        fn put(&self, address: u64, bytes: &[u8]) {
            let at = (address - BASE) as usize;
            self.0.borrow_mut()[at..at + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl Memory for Fake {
        fn regions(&self) -> Vec<(u64, usize)> {
            vec![(BASE, self.0.borrow().len())]
        }
        fn read(&self, address: u64, buf: &mut [u8]) -> bool {
            let mem = self.0.borrow();
            let Some(at) = address.checked_sub(BASE).map(|a| a as usize) else { return false };
            if at + buf.len() > mem.len() {
                return false;
            }
            buf.copy_from_slice(&mem[at..at + buf.len()]);
            true
        }
        fn write(&self, address: u64, data: &[u8]) -> bool {
            self.put(address, data);
            true
        }
    }

    #[test]
    fn parses_what_the_screen_shows() {
        assert_eq!(Target::parse("1250", Kind::Int).unwrap().value, 1250.0);
        assert_eq!(Target::parse(" 87,5 ", Kind::Float).unwrap().value, 87.5);
        assert!(Target::parse("87,5", Kind::Int).is_err());
        assert!(Target::parse("abc", Kind::Int).is_err());
        // Whole numbers match decimals that round to them.
        let t = Target::parse("87", Kind::Float).unwrap();
        assert!(t.matches(Kind::Float, &87.4f32.to_le_bytes()));
        assert!(!t.matches(Kind::Float, &88.0f32.to_le_bytes()));
    }

    #[test]
    fn narrows_down_to_the_changing_value() {
        let mem = Fake(RefCell::new(vec![0u8; 3 * 4096]));
        let coins = BASE + 4096 + 12;
        let decoy = BASE + 64;
        mem.put(coins, &1250i32.to_le_bytes());
        mem.put(decoy, &1250i32.to_le_bytes());

        let hits = first_scan(&mem, Kind::Int, Target::parse("1250", Kind::Int).unwrap()).unwrap();
        assert_eq!(hits, vec![decoy, coins]);

        // Coins were spent; the decoy stayed put.
        mem.put(coins, &1100i32.to_le_bytes());
        let hits = next_scan(&mem, Kind::Int, Target::parse("1100", Kind::Int).unwrap(), &hits);
        assert_eq!(hits, vec![coins]);

        assert!(mem.write(coins, &Kind::Int.encode(9999.0)));
        let mut now = [0u8; 4];
        assert!(mem.read(coins, &mut now));
        assert_eq!(Kind::Int.format(&now), "9999");
    }

    #[test]
    fn formats_values_for_the_overlay() {
        assert_eq!(Kind::Float.format(&87.5f32.to_le_bytes()), "87,5");
        assert_eq!(Kind::Float.format(&100.0f32.to_le_bytes()), "100");
        assert_eq!(Kind::Int.format(&(-3i32).to_le_bytes()), "-3");
    }
}
