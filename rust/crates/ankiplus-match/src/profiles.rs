//! Find the student's Anki profiles, so a deck can be indexed without exporting it.
//!
//! Port of the Python `match/profiles.py`. Anki keeps one folder per profile under its
//! base folder, each with `collection.anki2`:
//!
//! - macOS: `~/Library/Application Support/Anki2`
//! - Windows: `%APPDATA%\Anki2`
//! - Linux: `$XDG_DATA_HOME/Anki2` (default `~/.local/share/Anki2`)
//!
//! `ANKI_BASE` overrides it (Anki's own `-b` folder). The collection is only ever read
//! from a private copy (see `deck::load_deck`), so Anki can stay open.

use crate::Error;
use serde::Serialize;
use std::path::{Path, PathBuf};

const SKIP: [&str; 3] = ["addons21", "logs", "crash.log"];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Profile {
    pub profile: String,
    pub collection: String,
    pub bytes: u64,
    pub modified: i64,
}

fn expand_user(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            home.join(rest.trim_start_matches(['/', '\\']))
        }
        _ => PathBuf::from(p),
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// Anki's base folder for `platform` ("darwin", "win32", "linux", as Python's `sys.platform`).
pub fn anki_base_with(env: &dyn Fn(&str) -> Option<String>, platform: &str, home: &Path) -> PathBuf {
    let get = |k: &str| env(k).filter(|v| !v.is_empty());
    if let Some(base) = get("ANKI_BASE") {
        return expand_user(&base, home);
    }
    if platform == "darwin" {
        return home.join("Library").join("Application Support").join("Anki2");
    }
    if platform.starts_with("win") {
        return get("APPDATA").map(PathBuf::from).unwrap_or_else(|| home.join("AppData").join("Roaming")).join("Anki2");
    }
    get("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local").join("share")).join("Anki2")
}

pub fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(windows) {
        "win32"
    } else {
        "linux"
    }
}

pub fn anki_base() -> PathBuf {
    anki_base_with(&|k| std::env::var(k).ok(), current_platform(), &home_dir())
}

/// Profile folders (sorted by name) that contain `collection.anki2`.
pub fn list_profiles(base: Option<&Path>) -> Vec<Profile> {
    let base = base.map(Path::to_path_buf).unwrap_or_else(anki_base);
    let Ok(entries) = std::fs::read_dir(&base) else { return vec![] };
    let mut folders: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    folders.sort();
    folders
        .into_iter()
        .filter_map(|folder| {
            let name = folder.file_name()?.to_string_lossy().to_string();
            let col = folder.join("collection.anki2");
            if SKIP.contains(&name.as_str()) || !col.is_file() {
                return None;
            }
            let meta = std::fs::metadata(&col).ok()?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Some(Profile { profile: name, collection: col.to_string_lossy().to_string(), bytes: meta.len(), modified })
        })
        .collect()
}

fn names(found: &[Profile]) -> String {
    found.iter().map(|p| p.profile.as_str()).collect::<Vec<_>>().join(", ")
}

/// The collection file for `profile`; with no name, the only profile (or 'User 1').
pub fn collection_for(profile: Option<&str>, base: Option<&Path>) -> Result<PathBuf, Error> {
    let found = list_profiles(base);
    let not_found = Error::Source;
    if found.is_empty() {
        let base = base.map(Path::to_path_buf).unwrap_or_else(anki_base);
        return Err(not_found(format!(
            "No Anki profiles found in {}. Open Anki once, or set ANKI_BASE to its data folder.",
            base.display()
        )));
    }
    if let Some(name) = profile.filter(|p| !p.is_empty()) {
        return match found.iter().find(|p| p.profile == name) {
            Some(p) => Ok(PathBuf::from(&p.collection)),
            None => Err(not_found(format!("No Anki profile named '{name}'. Profiles: {}.", names(&found)))),
        };
    }
    if found.len() == 1 {
        return Ok(PathBuf::from(&found[0].collection));
    }
    if let Some(p) = found.iter().find(|p| p.profile == "User 1") {
        return Ok(PathBuf::from(&p.collection));
    }
    Err(not_found(format!("Several Anki profiles: {}. Name one.", names(&found))))
}
