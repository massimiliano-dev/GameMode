//! Read-only SQLite queries through the `winsqlite3.dll` that ships with
//! Windows 10+, so no SQLite engine has to be compiled into GameMode.

#![cfg(windows)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::Path;

use anyhow::{bail, Context, Result};
use windows::core::{s, PCSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x1;

type Open = unsafe extern "system" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Prepare = unsafe extern "system" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void, *mut *const c_char) -> c_int;
type Step = unsafe extern "system" fn(*mut c_void) -> c_int;
type ColumnText = unsafe extern "system" fn(*mut c_void, c_int) -> *const u8;
type Finalize = unsafe extern "system" fn(*mut c_void) -> c_int;
type Close = unsafe extern "system" fn(*mut c_void) -> c_int;

struct Api {
    open: Open,
    prepare: Prepare,
    step: Step,
    column_text: ColumnText,
    finalize: Finalize,
    close: Close,
}

fn api() -> Result<Api> {
    unsafe {
        let lib = LoadLibraryA(s!("winsqlite3.dll")).context("winsqlite3.dll non disponibile")?;
        let get = |name: PCSTR| GetProcAddress(lib, name).context("funzione SQLite mancante");
        Ok(Api {
            open: std::mem::transmute::<unsafe extern "system" fn() -> isize, Open>(get(s!("sqlite3_open_v2"))?),
            prepare: std::mem::transmute::<unsafe extern "system" fn() -> isize, Prepare>(get(s!("sqlite3_prepare_v2"))?),
            step: std::mem::transmute::<unsafe extern "system" fn() -> isize, Step>(get(s!("sqlite3_step"))?),
            column_text: std::mem::transmute::<unsafe extern "system" fn() -> isize, ColumnText>(get(s!("sqlite3_column_text"))?),
            finalize: std::mem::transmute::<unsafe extern "system" fn() -> isize, Finalize>(get(s!("sqlite3_finalize"))?),
            close: std::mem::transmute::<unsafe extern "system" fn() -> isize, Close>(get(s!("sqlite3_close"))?),
        })
    }
}

/// Runs `sql` against a copy of the database at `path` (the owning app may
/// hold locks on the original) and returns `columns` text columns per row.
pub fn query(path: &Path, sql: &str, columns: usize) -> Result<Vec<Vec<String>>> {
    let copy = std::env::temp_dir().join(format!("gamemode-{}.db", std::process::id()));
    std::fs::copy(path, &copy).with_context(|| format!("copia di {}", path.display()))?;
    for suffix in ["-wal", "-shm"] {
        let side = path.with_file_name(format!("{}{suffix}", path.file_name().unwrap_or_default().to_string_lossy()));
        if side.is_file() {
            let _ = std::fs::copy(&side, copy.with_file_name(format!("{}{suffix}", copy.file_name().unwrap_or_default().to_string_lossy())));
        }
    }
    let result = run(&copy, sql, columns);
    let _ = std::fs::remove_file(&copy);
    result
}

fn run(path: &Path, sql: &str, columns: usize) -> Result<Vec<Vec<String>>> {
    let api = api()?;
    let file = CString::new(path.to_string_lossy().as_bytes())?;
    let sql = CString::new(sql)?;
    let mut db: *mut c_void = std::ptr::null_mut();
    let mut stmt: *mut c_void = std::ptr::null_mut();
    unsafe {
        if (api.open)(file.as_ptr(), &mut db, SQLITE_OPEN_READONLY, std::ptr::null()) != SQLITE_OK {
            (api.close)(db);
            bail!("apertura del database non riuscita");
        }
        if (api.prepare)(db, sql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) != SQLITE_OK {
            (api.close)(db);
            bail!("query non valida per questa versione del database");
        }
        let mut rows = Vec::new();
        loop {
            match (api.step)(stmt) {
                SQLITE_ROW => rows.push(
                    (0..columns as c_int)
                        .map(|i| {
                            let text = (api.column_text)(stmt, i);
                            if text.is_null() {
                                String::new()
                            } else {
                                CStr::from_ptr(text as *const c_char).to_string_lossy().into_owned()
                            }
                        })
                        .collect(),
                ),
                SQLITE_DONE => break,
                _ => {
                    (api.finalize)(stmt);
                    (api.close)(db);
                    bail!("lettura del database interrotta");
                }
            }
        }
        (api.finalize)(stmt);
        (api.close)(db);
        Ok(rows)
    }
}
