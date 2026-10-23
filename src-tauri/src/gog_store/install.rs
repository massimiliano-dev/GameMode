//! Downloading a GOG game's offline installer (resumable, verified) and
//! running it silently into the GOG folder.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use md5::{Digest, Md5};

use super::{api, auth, setup};
use crate::direct::{self, CANCELLED};
use crate::updates::UpdateState;

/// Installer languages tried first.
const LANGUAGES: &[&str] = &["it"];
/// Partial downloads live here (inside the GOG folder, so on the same
/// drive as the game) until the install is over.
pub const DOWNLOADS: &str = ".gamemode-download";

pub fn job_id(id: &str) -> String {
    format!("gog:{id}")
}

/// Runs the whole install; returns the game's title.
pub async fn run(id: &str, root: &Path, cancel: &Arc<AtomicBool>) -> Result<String> {
    let job = job_id(id);
    let client = api::client();
    let access = auth::access_token(&client).await?;
    let product = api::product_with_downloads(&client, id).await.context("Gioco non trovato su GOG")?;
    let Some(installer) = api::pick_installer(&product.downloads.installers, LANGUAGES) else {
        bail!("{}: nessun installer per Windows", product.title);
    };
    let title = product.title.trim().to_string();
    let target = root.join(api::folder_name(&title));
    if target.join(format!("goggame-{id}.info")).is_file() {
        bail!("{title} è già installato in {}", target.display());
    }
    let staging = root.join(DOWNLOADS).join(id);
    std::fs::create_dir_all(&staging).with_context(|| format!("Non riesco a creare {}", staging.display()))?;

    let mut total: u64 = installer.files.iter().map(|f| f.size).sum();
    let mut finished: u64 = 0;
    let mut files = Vec::new();
    for file in &installer.files {
        if cancel.load(Ordering::SeqCst) {
            bail!(CANCELLED);
        }
        let link = api::downlink(&client, &access, &file.downlink).await?;
        let checksum = match link.checksum.as_deref() {
            Some(url) if !url.is_empty() => api::checksum(&client, url).await,
            _ => None,
        };
        let name = checksum
            .as_ref()
            .map(|c| c.name.clone())
            .filter(|n| !n.is_empty())
            .or_else(|| api::file_name_of(&link.downlink))
            .unwrap_or_else(|| format!("{}.bin", api::sanitize_file_name(&file.id)));
        let expected = checksum.as_ref().and_then(|c| c.total_size);
        if let Some(exact) = expected {
            // GOG's listed sizes are rounded: use the exact one from now on.
            total = total - file.size.min(total) + exact;
        }
        let path = staging.join(&name);
        let base = finished;
        download(&client, &link.downlink, &path, expected, cancel, |done| {
            direct::set_bytes(&job, UpdateState::Downloading, base + done, total.max(base + done));
        })
        .await?;

        let md5 = checksum.as_ref().and_then(|c| c.md5.clone());
        let matched = match md5 {
            Some(want) => {
                direct::set_percent(&job, UpdateState::Installing, None);
                let hashed = path.clone();
                let got = tauri::async_runtime::spawn_blocking(move || md5_file(&hashed)).await??;
                if got != want {
                    let _ = std::fs::remove_file(&path);
                    bail!("{name}: il file scaricato è danneggiato (checksum diverso), riprova");
                }
                true
            }
            None => {
                log::warn!("GOG {id}: no checksum for {name}");
                false
            }
        };
        finished += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(file.size);
        files.push((path, matched));
    }

    let Some((exe, matched)) = files
        .iter()
        .find(|(p, _)| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))
        .cloned()
    else {
        bail!("{title}: l'installer scaricato non contiene un .exe");
    };
    direct::set_percent(&job, UpdateState::Installing, None);
    let args = setup::install_args(&target);
    let code = tauri::async_runtime::spawn_blocking(move || -> Result<u32> {
        setup::verify(&exe, matched)?;
        setup::run_and_wait(&exe, &args, setup::INSTALL_TIMEOUT)
    })
    .await??;
    if code != 0 {
        // Keep the downloaded files: trying again won't download them twice.
        bail!("L'installazione di {title} non è riuscita (codice {code})");
    }
    if let Err(err) = std::fs::remove_dir_all(&staging) {
        log::warn!("removing {}: {err}", staging.display());
    }
    let _ = std::fs::remove_dir(root.join(DOWNLOADS));
    if !target.join(format!("goggame-{id}.info")).is_file() {
        log::warn!("GOG {id}: installed but no goggame-{id}.info in {}", target.display());
    }
    Ok(title)
}

/// Downloads `url` into `dest`, continuing a partial file when the server
/// allows it. `on_progress` gets the bytes in `dest` so far.
async fn download(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected: Option<u64>,
    cancel: &Arc<AtomicBool>,
    mut on_progress: impl FnMut(u64),
) -> Result<()> {
    let mut have = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    match expected {
        Some(size) if have == size => {
            on_progress(have);
            return Ok(());
        }
        Some(size) if have > size => have = 0,
        _ => {}
    }
    let mut request = client.get(url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = request.send().await.context("Download non riuscito")?.error_for_status().context("Download non riuscito")?;
    let resume = have > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resume {
        have = 0;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resume)
        .truncate(!resume)
        .open(dest)
        .with_context(|| format!("Non riesco a scrivere {}", dest.display()))?;
    on_progress(have);
    while let Some(chunk) = resp.chunk().await.context("Download interrotto")? {
        if cancel.load(Ordering::SeqCst) {
            bail!(CANCELLED);
        }
        out.write_all(&chunk).context("Scrittura non riuscita: spazio esaurito?")?;
        have += chunk.len() as u64;
        on_progress(have);
    }
    out.flush()?;
    if let Some(size) = expected {
        if have != size {
            bail!("Download incompleto ({have} di {size} byte), riprova");
        }
    }
    Ok(())
}

pub fn md5_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Md5::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Leftover partial downloads of `id`, to drop when the user cancels.
pub fn staging_dir(root: &Path, id: &str) -> PathBuf {
    root.join(DOWNLOADS).join(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_files_with_md5() {
        let path = std::env::temp_dir().join(format!("gm-md5-{}", std::process::id()));
        std::fs::write(&path, b"The quick brown fox jumps over the lazy dog").unwrap();
        assert_eq!(md5_file(&path).unwrap(), "9e107d9d372bb6826bd81d3542a419d6");
        std::fs::remove_file(&path).unwrap();
    }
}
