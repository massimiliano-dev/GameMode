//! The legendary executable: never shipped with GameMode, downloaded on
//! first use from a pinned GitHub release and checked against a pinned
//! SHA-256 before it is ever run.

use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

/// Pinned release of https://github.com/legendary-gl/legendary (GPL-3.0),
/// the project formerly at github.com/derrod/legendary.
pub const VERSION: &str = "0.21.1";
pub const LICENSE_URL: &str = "https://github.com/legendary-gl/legendary/blob/master/LICENSE";
pub const PROJECT_URL: &str = "https://github.com/legendary-gl/legendary";

/// Release asset and its SHA-256, as published by the release (and
/// checked by downloading the file, without running it).
#[cfg(target_arch = "aarch64")]
const ASSET: (&str, &str) = (
    "legendary_windows_arm64.exe",
    "5ca178d75713cb92ee24daa5df1905c75f05b8b403383669ca7aa32a5809fba3",
);
#[cfg(not(target_arch = "aarch64"))]
const ASSET: (&str, &str) = (
    "legendary_windows_x64.exe",
    "6be77857dc0a6dd33ea7442ce8b5291944ec6da53757f81ca95b57ce794baf5b",
);

pub fn url() -> String {
    format!("https://github.com/legendary-gl/legendary/releases/download/{VERSION}/{}", ASSET.0)
}

pub fn expected_sha256() -> &'static str {
    ASSET.1
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// The file at `path` is exactly the pinned release.
pub fn verify(path: &Path, expected: &str) -> Result<()> {
    let actual = sha256_file(path).with_context(|| format!("lettura di {}", path.display()))?;
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("legendary non corrisponde alla versione attesa (SHA-256 {actual}): non lo eseguo");
    }
    Ok(())
}

/// Downloads the pinned release to `dest`, reporting progress (0…1). The
/// file only takes its final name once its hash matches.
pub async fn download(dest: &Path, progress: impl Fn(f64)) -> Result<()> {
    let partial = dest.with_extension("part");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .unwrap_or_default();
    let mut resp = client
        .get(url())
        .header(reqwest::header::USER_AGENT, "GameMode")
        .send()
        .await?
        .error_for_status()
        .context("download di legendary")?;
    let total = resp.content_length().unwrap_or(0);
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::File::create(&partial)?;
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        done += chunk.len() as u64;
        if total > 0 {
            progress((done as f64 / total as f64).min(1.0));
        }
    }
    file.flush()?;
    drop(file);
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256()) {
        let _ = std::fs::remove_file(&partial);
        bail!("Il file scaricato non corrisponde all'impronta SHA-256 attesa: non lo eseguo");
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&partial, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_files_against_a_pinned_hash() {
        let dir = std::env::temp_dir().join(format!("gm-legendary-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("x.exe");
        std::fs::write(&file, b"abc").unwrap();
        // SHA-256("abc"), FIPS 180-2 test vector.
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&file, abc).is_ok());
        assert!(verify(&file, &abc.to_uppercase()).is_ok());
        assert!(verify(&file, expected_sha256()).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pins_a_release_asset() {
        assert_eq!(expected_sha256().len(), 64);
        assert!(url().starts_with("https://github.com/legendary-gl/legendary/releases/download/0.21.1/legendary_windows_"));
    }
}
