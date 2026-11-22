//! What the desktop app's first-run wizard shares with the Python MCP server.
//!
//! The wizard builds the server's named index (`<workspace root>/indexes/<name>`) in
//! the same format and with the same registry fields as `Matching._build_index`, so
//! the MCP tools in Claude match against it at once, and the server keeps it in step
//! with the Anki collection afterwards (only new or edited notes are re-embedded).

use crate::deck::{load_deck, LoadOptions};
use crate::index::DeckIndex;
use crate::pipeline::{get_embedder, EmbeddingMode};
use crate::store::{FileIndexStore, IndexStore};
use crate::{EmbedCounts, Error};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

const APP: &str = "ankiplusmcp";

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `platformdirs.user_data_path("ankiplusmcp")`, the Python server's data folder.
pub fn user_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join(APP)
    } else if cfg!(windows) {
        env_dir("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local")).join(APP).join(APP)
    } else {
        env_dir("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local").join("share")).join(APP)
    }
}

/// The server's default `--workspace-root` (`ANKIPLUS_WORKSPACE_ROOT` overrides it, as in Python).
pub fn workspace_root() -> PathBuf {
    env_dir("ANKIPLUS_WORKSPACE_ROOT").unwrap_or_else(|| user_data_dir().join("workspaces"))
}

/// Folder of the server's named index.
pub fn index_folder(root: &Path, name: &str) -> PathBuf {
    root.join("indexes").join(name)
}

#[derive(Debug, Clone)]
pub struct NamedIndex<'a> {
    pub collection: &'a Path,
    pub root: &'a Path,
    pub name: &'a str,
    pub deck_prefix: Option<&'a str>,
    pub embeddings: EmbeddingMode,
    /// The source is the student's own Anki collection (the server then re-syncs it).
    pub profile: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Built {
    pub index: String,
    pub notes: usize,
    pub suspended_cards: usize,
    pub embeddings: bool,
    pub embedding_model: Option<String>,
    pub source: String,
    pub embedded: usize,
    pub reused: usize,
}

/// Progress of a build: a stage ("reading deck", "embedding", "saving") and done/total.
pub type Progress<'a> = &'a mut dyn FnMut(&str, usize, usize);

/// Build (or update) the server's named index from a collection or deck export.
pub fn build_named_index(req: &NamedIndex, progress: Progress) -> Result<Built, Error> {
    let folder = index_folder(req.root, req.name);
    progress("reading deck", 0, 0);
    let notes = load_deck(req.collection, &LoadOptions { deck_prefix: req.deck_prefix, ..Default::default() })?;
    if notes.is_empty() {
        return Err(Error::Deck("No notes found (check the deck).".into()));
    }
    progress("loading the embedding model (the first run downloads about 0.5 GB)", 0, 0);
    let embedder = get_embedder(req.embeddings)?;
    let source_name = req.collection.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut meta = Map::new();
    meta.insert("name".into(), Value::String(req.name.into()));
    meta.insert("source".into(), Value::String(source_name));
    meta.insert("deck_prefix".into(), req.deck_prefix.map_or(Value::Null, |p| Value::String(p.into())));
    meta.insert("fields".into(), Value::Null);
    let name_of = embedder.as_ref().map(|e| e.name());
    let mut index = DeckIndex::new(notes, embedder, meta);
    let store = FileIndexStore::new(&folder);
    // Reuse vectors of an older build with the same model (an older format is rebuilt).
    let previous = match (&name_of, std::fs::read_to_string(folder.join("meta.json"))) {
        (Some(model), Ok(text)) => {
            let old: Map<String, Value> = serde_json::from_str(&text).unwrap_or_default();
            if old.get("embedding_model").and_then(Value::as_str) == Some(model.as_str()) {
                store.load(get_embedder(req.embeddings)?).ok()
            } else {
                None
            }
        }
        _ => None,
    };
    let counts: EmbedCounts = index.embed_all(64, |done, total| progress("embedding", done, total), previous.as_ref())?;
    progress("saving", 0, 0);
    let mut meta = store.save(&index)?;
    let stat = std::fs::metadata(req.collection)?;
    let mtime_ns = stat.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mode = req.embeddings.as_str();
    for (k, v) in [
        ("backend", Value::String("file".into())),
        ("source_path", Value::String(std::fs::canonicalize(req.collection)?.to_string_lossy().to_string())),
        ("profile", Value::Bool(req.profile)),
        ("deck_stamp", Value::String(format!("{}|{}", stat.len(), mtime_ns))),
        ("embeddings_mode", Value::String(mode.into())),
        ("model", Value::Null),
        ("requested_backend", Value::String("auto".into())),
    ] {
        meta.insert(k.into(), v);
    }
    std::fs::write(folder.join("meta.json"), serde_json::to_string_pretty(&meta)? + "\n")?;
    Ok(Built {
        index: req.name.into(),
        notes: index.notes.len(),
        suspended_cards: index.notes.iter().map(|n| n.suspended_card_ids.len()).sum(),
        embeddings: index.has_embeddings(),
        embedding_model: meta.get("embedding_model").and_then(Value::as_str).map(String::from),
        source: req.collection.to_string_lossy().to_string(),
        embedded: counts.embedded,
        reused: counts.reused,
    })
}

/// The server's named index, if one is built: its meta.json.
pub fn index_info(root: &Path, name: &str) -> Option<Map<String, Value>> {
    serde_json::from_str(&std::fs::read_to_string(index_folder(root, name).join("meta.json")).ok()?).ok()
}
