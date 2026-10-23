//! Save-game backups. Where each game keeps its saves comes from the
//! Ludusavi manifest (built from PCGamingWiki), downloaded once and reduced
//! to a compact index of the Windows rules. Backups are plain folder copies:
//! `<backup dir>/<game>/<timestamp>/<n>/…` plus a `backup.json` that maps
//! each numbered folder back to where it came from.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{now, Game, Store};

const MANIFEST_URL: &str = "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/master/data/manifest.yaml";
const INDEX_FILE: &str = "saves-index.json";
/// Refresh the manifest weekly.
const INDEX_MAX_AGE_SECS: u64 = 7 * 24 * 3600;

/// One rule of the compact index: a path template, optionally only for one store.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rule {
    pub path: String,
    #[serde(default)]
    pub store: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub steam: Option<u64>,
    #[serde(default)]
    pub gog: Option<u64>,
    #[serde(default)]
    pub dirs: Vec<String>,
    pub rules: Vec<Rule>,
}

#[derive(Default)]
pub struct Index {
    entries: Vec<Entry>,
    by_steam: HashMap<u64, usize>,
    by_gog: HashMap<u64, usize>,
    by_name: HashMap<String, usize>,
    by_dir: HashMap<String, usize>,
}

static INDEX: Mutex<Option<std::sync::Arc<Index>>> = Mutex::new(None);

// ---- manifest → index ----

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawEntry {
    files: HashMap<String, RawRule>,
    #[serde(rename = "installDir")]
    install_dir: HashMap<String, serde::de::IgnoredAny>,
    steam: Option<RawId>,
    gog: Option<RawId>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawId {
    id: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawRule {
    tags: Vec<String>,
    when: Vec<RawWhen>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawWhen {
    os: Option<String>,
    store: Option<String>,
}

/// Keeps the save/config rules that can apply on Windows.
pub fn build_index(yaml: &str) -> Result<Vec<Entry>> {
    let raw: HashMap<String, RawEntry> = serde_yaml::from_str(yaml).context("manifest non valido")?;
    let mut entries: Vec<Entry> = raw
        .into_iter()
        .filter_map(|(name, e)| {
            let mut rules = Vec::new();
            for (path, rule) in e.files {
                let relevant = rule.tags.is_empty() || rule.tags.iter().any(|t| t == "save" || t == "config");
                if !relevant {
                    continue;
                }
                if rule.when.is_empty() {
                    rules.push(Rule { path, store: None });
                    continue;
                }
                for w in rule.when.iter().filter(|w| w.os.as_deref().is_none_or(|os| os == "windows")) {
                    rules.push(Rule { path: path.clone(), store: w.store.clone() });
                }
            }
            (!rules.is_empty()).then(|| Entry {
                name,
                steam: e.steam.and_then(|s| s.id),
                gog: e.gog.and_then(|g| g.id),
                dirs: e.install_dir.into_keys().collect(),
                rules,
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Lowercase, without trademark signs and punctuation, for name matching.
pub fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl Index {
    fn new(entries: Vec<Entry>) -> Self {
        let mut index = Index::default();
        for (i, e) in entries.iter().enumerate() {
            if let Some(id) = e.steam {
                index.by_steam.insert(id, i);
            }
            if let Some(id) = e.gog {
                index.by_gog.insert(id, i);
            }
            index.by_name.entry(normalize_name(&e.name)).or_insert(i);
            for d in &e.dirs {
                index.by_dir.entry(d.to_lowercase()).or_insert(i);
            }
        }
        index.entries = entries;
        index
    }

    pub fn find(&self, game: &Game) -> Option<&Entry> {
        let native = game.id.split_once(':').map_or("", |(_, id)| id);
        let i = game
            .steam_appid
            .and_then(|id| self.by_steam.get(&(id as u64)))
            .or_else(|| (game.store == Store::Gog).then(|| native.parse().ok()).flatten().and_then(|id: u64| self.by_gog.get(&id)))
            .or_else(|| self.by_name.get(&normalize_name(&game.name)))
            .or_else(|| {
                let dir = game.install_dir.as_deref()?.file_name()?.to_string_lossy().to_lowercase();
                self.by_dir.get(&dir)
            })?;
        self.entries.get(*i)
    }
}

/// Loads the index, downloading and rebuilding it when missing or stale.
pub fn index(data_dir: &Path) -> Result<std::sync::Arc<Index>> {
    if let Some(index) = INDEX.lock().unwrap().clone() {
        return Ok(index);
    }
    let path = data_dir.join(INDEX_FILE);
    let fresh = fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age.as_secs() < INDEX_MAX_AGE_SECS);
    let entries: Vec<Entry> = match fs::read(&path).ok().filter(|_| fresh).and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(entries) => entries,
        None => match download_index() {
            Ok(entries) => {
                let _ = fs::write(&path, serde_json::to_vec(&entries)?);
                entries
            }
            // Offline: an old index beats none.
            Err(err) => fs::read(&path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .ok_or(err)?,
        },
    };
    let index = std::sync::Arc::new(Index::new(entries));
    *INDEX.lock().unwrap() = Some(index.clone());
    Ok(index)
}

/// The index already at hand, in memory or on disk however old, never
/// downloaded: for exit paths, where the network may be going away.
pub fn cached_index(data_dir: &Path) -> Option<std::sync::Arc<Index>> {
    if let Some(index) = INDEX.lock().unwrap().clone() {
        return Some(index);
    }
    let entries: Vec<Entry> = serde_json::from_slice(&fs::read(data_dir.join(INDEX_FILE)).ok()?).ok()?;
    let index = std::sync::Arc::new(Index::new(entries));
    *INDEX.lock().unwrap() = Some(index.clone());
    Some(index)
}

fn download_index() -> Result<Vec<Entry>> {
    let yaml = tauri::async_runtime::block_on(async {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(120)).build()?;
        anyhow::Ok(client.get(MANIFEST_URL).send().await?.error_for_status()?.text().await?)
    })?;
    build_index(&yaml)
}

// ---- path expansion ----

/// Values for the manifest's placeholders on this PC.
pub struct Places {
    pub vars: Vec<(&'static str, String)>,
}

impl Places {
    pub fn detect() -> Self {
        let env = |k: &str| std::env::var(k).unwrap_or_default();
        let home = dirs::home_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| env("USERPROFILE"));
        let docs = dirs::document_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{home}\\Documents"));
        Places {
            vars: vec![
                ("<home>", home.clone()),
                ("<winAppData>", env("APPDATA")),
                ("<winLocalAppData>", env("LOCALAPPDATA")),
                ("<winLocalAppDataLow>", format!("{home}\\AppData\\LocalLow")),
                ("<winDocuments>", docs),
                ("<winPublic>", env("PUBLIC")),
                ("<winProgramData>", env("PROGRAMDATA")),
                ("<winDir>", env("WINDIR")),
                ("<osUserName>", env("USERNAME")),
            ],
        }
    }
}

fn store_tag(store: Store) -> Option<&'static str> {
    Some(match store {
        Store::Steam => "steam",
        Store::Gog => "gog",
        Store::Epic => "epic",
        Store::Xbox => "microsoft",
        Store::Ubisoft => "uplay",
        Store::Ea => "ea",
        _ => return None,
    })
}

/// Turns the rules into glob patterns for this game, or `None` per rule
/// that can't be resolved (unknown install folder, other store).
pub fn patterns(entry: &Entry, game: &Game, places: &Places) -> Vec<String> {
    let base = game.install_dir.as_ref().map(|p| p.to_string_lossy().into_owned());
    let mut out = Vec::new();
    for rule in &entry.rules {
        if rule.store.as_deref().is_some_and(|s| Some(s) != store_tag(game.store)) {
            continue;
        }
        let mut p = rule.path.clone();
        if p.contains("<base>") || p.contains("<root>") || p.contains("<game>") {
            let Some(base) = &base else { continue };
            if p.contains("<root>") && !p.contains("<base>") {
                continue;
            }
            p = p.replace("<base>", &glob::Pattern::escape(base));
        }
        for (key, value) in &places.vars {
            if p.contains(key) {
                if value.is_empty() {
                    p.clear();
                    break;
                }
                p = p.replace(key, &glob::Pattern::escape(value));
            }
        }
        // Per-account folders: any account.
        p = p.replace("<storeUserId>", "*");
        if !p.is_empty() && !p.contains('<') && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Existing files/folders matching the patterns, without nested duplicates.
pub fn existing(patterns: &[String]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = patterns
        .iter()
        .filter_map(|p| glob::glob(&p.replace('\\', "/")).ok())
        .flat_map(|paths| paths.flatten())
        .filter(|p| p.exists())
        .collect();
    found.sort();
    found.dedup();
    let copy = found.clone();
    found.retain(|p| !copy.iter().any(|q| q != p && p.starts_with(q)));
    found
}

// ---- backups ----

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Mapping {
    pub original: PathBuf,
    pub stored: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupMeta {
    pub game_id: String,
    pub created: i64,
    /// Made automatically before a restore.
    #[serde(default)]
    pub automatic: bool,
    pub items: Vec<Mapping>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: String,
    pub created: i64,
    pub automatic: bool,
    pub bytes: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SaveInfo {
    /// The manifest knows this game.
    pub known: bool,
    pub locations: Vec<PathBuf>,
    pub backups: Vec<BackupInfo>,
    pub backup_dir: PathBuf,
}

fn game_dir(root: &Path, game_id: &str) -> PathBuf {
    let safe: String = game_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    root.join(safe)
}

fn dir_size(path: &Path) -> u64 {
    if path.is_file() {
        return fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    }
    fs::read_dir(path).map(|it| it.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

fn copy_recursive(from: &Path, to: &Path) -> Result<()> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to).with_context(|| format!("copia di {}", from.display()))?;
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)?.flatten() {
        copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

pub fn list_backups(root: &Path, game_id: &str) -> Vec<BackupInfo> {
    let dir = game_dir(root, game_id);
    let mut list: Vec<BackupInfo> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let meta: BackupMeta = serde_json::from_slice(&fs::read(e.path().join("backup.json")).ok()?).ok()?;
            Some(BackupInfo {
                id: e.file_name().to_string_lossy().into_owned(),
                created: meta.created,
                automatic: meta.automatic,
                bytes: dir_size(&e.path()),
            })
        })
        .collect();
    list.sort_by(|a, b| b.created.cmp(&a.created));
    list
}

pub fn info(data_dir: &Path, root: &Path, game: &Game) -> Result<SaveInfo> {
    let index = index(data_dir)?;
    let entry = index.find(game);
    let locations = entry.map(|e| existing(&patterns(e, game, &Places::detect()))).unwrap_or_default();
    Ok(SaveInfo { known: entry.is_some(), locations, backups: list_backups(root, &game.id), backup_dir: game_dir(root, &game.id) })
}

pub fn backup(data_dir: &Path, root: &Path, game: &Game, automatic: bool) -> Result<BackupInfo> {
    backup_with(&*index(data_dir)?, root, game, automatic)
}

/// `backup` with an index already loaded (see `cached_index`).
pub fn backup_with(index: &Index, root: &Path, game: &Game, automatic: bool) -> Result<BackupInfo> {
    let Some(entry) = index.find(game) else { bail!("Non so dove questo gioco tiene i salvataggi") };
    let locations = existing(&patterns(entry, game, &Places::detect()));
    if locations.is_empty() {
        bail!("Nessun salvataggio trovato sul PC");
    }
    backup_paths(root, &game.id, &locations, automatic)
}

pub fn backup_paths(root: &Path, game_id: &str, locations: &[PathBuf], automatic: bool) -> Result<BackupInfo> {
    let created = now();
    let id = format!("{created}{}", if automatic { "-auto" } else { "" });
    let target = game_dir(root, game_id).join(&id);
    let mut items = Vec::new();
    for (i, original) in locations.iter().enumerate() {
        let stored = i.to_string();
        copy_recursive(original, &target.join(&stored))?;
        items.push(Mapping { original: original.clone(), stored });
    }
    let meta = BackupMeta { game_id: game_id.to_string(), created, automatic, items };
    fs::write(target.join("backup.json"), serde_json::to_vec_pretty(&meta)?)?;
    Ok(BackupInfo { id, created, automatic, bytes: dir_size(&target) })
}

/// Copies a backup back over the current saves, after making a safety
/// backup of what is there now.
pub fn restore(root: &Path, game_id: &str, backup_id: &str) -> Result<()> {
    let source = game_dir(root, game_id).join(backup_id);
    let meta: BackupMeta =
        serde_json::from_slice(&fs::read(source.join("backup.json")).context("backup non trovato")?)?;
    let current: Vec<PathBuf> = meta.items.iter().map(|m| m.original.clone()).filter(|p| p.exists()).collect();
    if !current.is_empty() {
        backup_paths(root, game_id, &current, true)?;
    }
    for item in &meta.items {
        let from = source.join(&item.stored);
        if from.is_dir() && item.original.is_dir() {
            fs::remove_dir_all(&item.original)?;
        }
        copy_recursive(&from, &item.original)?;
    }
    Ok(())
}

pub fn delete(root: &Path, game_id: &str, backup_id: &str) -> Result<()> {
    if backup_id.contains(['/', '\\']) || backup_id.contains("..") {
        bail!("backup non valido");
    }
    fs::remove_dir_all(game_dir(root, game_id).join(backup_id))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LaunchSpec;

    const YAML: &str = r#"
Hades:
  files:
    "<home>/Library/Application Support/Hades":
      tags: [save]
      when:
        - os: mac
    "<winDocuments>/Saved Games/Hades":
      tags: [config, save]
      when:
        - os: windows
    "<base>/logs":
      tags: [log]
  installDir:
    Hades: {}
  steam:
    id: 1145360
Old Game:
  files:
    "<base>/save/*.sav": {}
    "<winAppData>/Old/<storeUserId>":
      when:
        - store: gog
"#;

    fn game(store: Store, id: &str, name: &str) -> Game {
        Game::new(store, id, name, LaunchSpec::Uri { uri: String::new() })
    }

    fn places() -> Places {
        Places { vars: vec![("<winDocuments>", r"C:\Users\Max\Documents".into()), ("<winAppData>", r"C:\Users\Max\AppData\Roaming".into()), ("<home>", "".into())] }
    }

    /// `GM_MANIFEST=path/to/manifest.yaml cargo test --release -- --ignored real_manifest --nocapture`
    #[test]
    #[ignore]
    fn real_manifest() {
        let path = std::env::var("GM_MANIFEST").expect("GM_MANIFEST");
        let t = std::time::Instant::now();
        let entries = build_index(&fs::read_to_string(path).unwrap()).unwrap();
        let index = Index::new(entries);
        println!("{} entries in {:?}", index.entries.len(), t.elapsed());
        let mut hades = game(Store::Steam, "1145360", "Hades");
        hades.steam_appid = Some(1145360);
        let e = index.find(&hades).unwrap();
        println!("Hades rules: {:?}", e.rules);
        let json = serde_json::to_vec(&index.entries).unwrap();
        println!("index size: {} KB", json.len() / 1024);
    }

    #[test]
    fn indexes_windows_save_rules_only() {
        let entries = build_index(YAML).unwrap();
        let hades = entries.iter().find(|e| e.name == "Hades").unwrap();
        assert_eq!(hades.steam, Some(1145360));
        assert_eq!(hades.rules, vec![Rule { path: "<winDocuments>/Saved Games/Hades".into(), store: None }]);
        assert_eq!(entries.iter().find(|e| e.name == "Old Game").unwrap().rules.len(), 2);
    }

    #[test]
    fn finds_games_and_expands_paths() {
        let index = Index::new(build_index(YAML).unwrap());
        let mut hades = game(Store::Steam, "1145360", "Hades");
        hades.steam_appid = Some(1145360);
        let entry = index.find(&hades).unwrap();
        assert_eq!(patterns(entry, &hades, &places()), [r"C:\Users\Max\Documents/Saved Games/Hades"]);

        // By name, with the store-specific rule and the install folder.
        let old = game(Store::Gog, "77", "Old Game™").with_install_dir(r"D:\Games\Old [GOTY]");
        let entry = index.find(&old).unwrap();
        let mut p = patterns(entry, &old, &places());
        p.sort();
        assert_eq!(p, [r"C:\Users\Max\AppData\Roaming/Old/*", r"D:\Games\Old [[]GOTY[]]/save/*.sav"]);

        // Other store: the GOG-only rule doesn't apply.
        let steam_old = game(Store::Steam, "1", "Old Game");
        assert!(patterns(index.find(&steam_old).unwrap(), &steam_old, &places()).is_empty());
    }

    #[test]
    fn backs_up_and_restores_with_a_safety_copy() {
        let tmp = std::env::temp_dir().join(format!("gm-saves-{}", std::process::id()));
        let saves = tmp.join("Saved Games").join("Hades");
        fs::create_dir_all(&saves).unwrap();
        fs::write(saves.join("Profile1.sav"), b"v1").unwrap();
        let root = tmp.join("backups");

        let first = backup_paths(&root, "steam:1145360", &[saves.clone()], false).unwrap();
        fs::write(saves.join("Profile1.sav"), b"v2").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        restore(&root, "steam:1145360", &first.id).unwrap();

        assert_eq!(fs::read(saves.join("Profile1.sav")).unwrap(), b"v1");
        let list = list_backups(&root, "steam:1145360");
        assert_eq!(list.len(), 2);
        assert!(list[0].automatic, "the overwritten state was kept");
        delete(&root, "steam:1145360", &first.id).unwrap();
        assert_eq!(list_backups(&root, "steam:1145360").len(), 1);
        assert!(delete(&root, "steam:1145360", "../x").is_err());
        fs::remove_dir_all(&tmp).unwrap();
    }
}
