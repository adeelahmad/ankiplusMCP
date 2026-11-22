//! End-to-end helpers shared by the CLI and the desktop app.
//!
//! The index cache key is the one the Python CLI uses
//! (`sha256("{deck}|{prefix}|{embeddings}")`, with the deck's size and mtime stored as
//! `deck_stamp`), so both implementations reuse each other's cached file indexes.

use crate::deck::{load_deck, LoadOptions, Note};
use crate::text::clean_text;
use crate::index::{DeckIndex, Embedder};
use crate::matcher::{match_chunks, MatchOptions, Report, Thresholds};
use crate::prompt::build_prompt;
use crate::store::{FileIndexStore, IndexStore};
use crate::Error;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingMode {
    Off,
    #[default]
    Auto,
    Required,
}

impl EmbeddingMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Required => "required",
        }
    }
}

impl std::str::FromStr for EmbeddingMode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "off" => Ok(Self::Off),
            "auto" => Ok(Self::Auto),
            "required" => Ok(Self::Required),
            _ => Err(Error::Embed(format!("Unknown embeddings mode: {s}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    File,
    Postgres,
}

/// 'off' -> None; 'auto' -> local Nomic ONNX when available, else None; 'required' -> it or an error.
pub fn get_embedder(mode: EmbeddingMode) -> Result<Option<Box<dyn Embedder + Send + Sync>>, Error> {
    if mode == EmbeddingMode::Off {
        return Ok(None);
    }
    #[cfg(feature = "embed")]
    {
        match crate::embed::FastEmbedder::new(Some(model_cache_dir()), 256) {
            Ok(e) => return Ok(Some(Box::new(e))),
            Err(e) if mode == EmbeddingMode::Required => {
                return Err(Error::Embed(format!(
                    "Embedding model could not be loaded ({e}). It downloads once from Hugging Face; check the network."
                )))
            }
            Err(_) => return Ok(None),
        }
    }
    #[cfg(not(feature = "embed"))]
    {
        if mode == EmbeddingMode::Required {
            return Err(Error::Embed("Embeddings need a build with the `embed` feature.".into()));
        }
        Ok(None)
    }
}

/// Where downloaded models live: `$ANKIPLUS_MODEL_DIR`, else `~/.cache/ankiplusmcp/models`.
pub fn model_cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ANKIPLUS_MODEL_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default();
    PathBuf::from(home).join(".cache").join("ankiplusmcp").join("models")
}

pub struct IndexRequest<'a> {
    pub deck: &'a Path,
    pub deck_prefix: Option<&'a str>,
    pub index_dir: &'a Path,
    pub embeddings: EmbeddingMode,
    pub backend: Backend,
    pub rebuild: bool,
}

/// Cache folder name: the Python key, `d` + 16 hex of sha256("{deck}|{prefix}|{embeddings}").
/// The deck's size and mtime live in `meta.json` as `deck_stamp`, so a changed deck
/// updates the same folder and re-embeds only new or edited notes.
pub fn cache_key(deck: &Path, prefix: Option<&str>, embeddings: EmbeddingMode) -> Result<String, Error> {
    let deck = std::fs::canonicalize(deck)?;
    let raw = format!("{}|{}|{}", deck.display(), prefix.unwrap_or("None"), embeddings.as_str());
    let digest = Sha256::digest(raw.as_bytes());
    Ok(format!("d{}", &hex(&digest)[..16]))
}

/// "{size}|{mtime_ns}" of the deck file, as the Python `deck_stamp`.
pub fn deck_stamp(deck: &Path) -> Result<String, Error> {
    let meta = std::fs::metadata(std::fs::canonicalize(deck)?)?;
    let mtime_ns = meta.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    Ok(format!("{}|{}", meta.len(), mtime_ns))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn store_for(req: &IndexRequest, key: &str) -> Result<Box<dyn IndexStore>, Error> {
    match req.backend {
        Backend::File => Ok(Box::new(FileIndexStore::new(req.index_dir.join(key)))),
        #[cfg(feature = "postgres")]
        Backend::Postgres => Ok(Box::new(crate::pgstore::PostgresIndexStore::new(req.index_dir.join("pg"), key)?)),
        #[cfg(not(feature = "postgres"))]
        Backend::Postgres => Err(Error::Index("Postgres storage needs a build with the `postgres` feature.".into())),
    }
}

fn write_marker(req: &IndexRequest, marker: &Path, mut meta: Map<String, Value>, stamp: Option<String>) -> Result<(), Error> {
    meta.insert(
        "backend".into(),
        Value::String(if req.backend == Backend::Postgres { "postgres" } else { "file" }.into()),
    );
    if let Some(stamp) = stamp {
        meta.insert("deck_stamp".into(), Value::String(stamp));
    }
    std::fs::create_dir_all(marker.parent().unwrap())?;
    std::fs::write(marker, serde_json::to_string_pretty(&meta)? + "\n")?;
    Ok(())
}

/// Load the cached index for a deck, or build (and cache) it. When the deck changed
/// since it was cached, only new or edited notes are re-embedded.
pub fn open_index(req: &IndexRequest, mut progress: impl FnMut(&str)) -> Result<DeckIndex, Error> {
    let key = cache_key(req.deck, req.deck_prefix, req.embeddings)?;
    let stamp = deck_stamp(req.deck)?;
    let marker: PathBuf = req.index_dir.join(&key).join("meta.json");
    let store = store_for(req, &key)?;
    let mut previous = None;
    if marker.is_file() && !req.rebuild {
        // An older index format is rebuilt from scratch.
        previous = store.load(get_embedder(req.embeddings)?).ok();
        if previous.is_some() {
            let cached: Value = serde_json::from_str(&std::fs::read_to_string(&marker)?)?;
            if cached.get("deck_stamp").and_then(Value::as_str) == Some(stamp.as_str()) {
                return Ok(previous.unwrap());
            }
        }
    }
    let name = req.deck.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    progress(&format!("indexing {name} ..."));
    let notes = load_deck(req.deck, &LoadOptions { deck_prefix: req.deck_prefix, ..Default::default() })?;
    let mut meta = Map::new();
    meta.insert("source".into(), Value::String(name));
    meta.insert("deck_prefix".into(), req.deck_prefix.map_or(Value::Null, |p| Value::String(p.into())));
    let mut index = DeckIndex::new(notes, get_embedder(req.embeddings)?, meta);
    let counts = index.embed_all(256, |done, total| progress(&format!("embedding {done}/{total}")), previous.as_ref())?;
    if counts.reused > 0 {
        progress(&format!("re-embedded {} new or changed notes, reused {}", counts.embedded, counts.reused));
    }
    let meta = store.save(&index)?;
    write_marker(req, &marker, meta, Some(stamp))?;
    Ok(index)
}

/// A card the host just wrote to Anki: the draft's fields (first two are indexed) and
/// the ids the Anki MCP reported.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WrittenCard {
    pub note_id: i64,
    #[serde(default)]
    pub card_ids: Vec<i64>,
    #[serde(default)]
    pub deck: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Field values in note-type order (e.g. Text, Extra).
    pub fields: Vec<String>,
}

/// Add cards just written to Anki to the cached index at once, so the next lecture
/// matches against them and never drafts them again (Python `index_written_cards`).
/// Only these notes are embedded; the next deck sync replaces them with the
/// collection's own copy. The deck stamp is kept, so that sync still happens.
pub fn index_written_cards(req: &IndexRequest, index: &DeckIndex, written: &[WrittenCard]) -> Result<(DeckIndex, crate::EmbedCounts, usize), Error> {
    let key = cache_key(req.deck, req.deck_prefix, req.embeddings)?;
    let marker: PathBuf = req.index_dir.join(&key).join("meta.json");
    let added: Vec<Note> = written
        .iter()
        .filter_map(|w| {
            let text = w.fields.iter().take(2).map(|v| clean_text(v)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n");
            (!text.is_empty()).then(|| Note {
                note_id: w.note_id,
                card_ids: w.card_ids.clone(),
                deck: w.deck.clone(),
                tags: w.tags.clone(),
                text,
                front: Some(w.fields.first().map(|f| clean_text(f)).unwrap_or_default()),
                ..Default::default()
            })
        })
        .collect();
    let count = added.len();
    let (fresh, counts) = index.with_notes(added, get_embedder(req.embeddings)?)?;
    // Read the marker first: the file store writes its own meta.json in the same folder.
    let mut meta: Map<String, Value> = match std::fs::read_to_string(&marker) {
        Ok(s) => serde_json::from_str(&s)?,
        Err(_) => Map::new(),
    };
    let stamp = meta.remove("deck_stamp").and_then(|v| v.as_str().map(String::from));
    meta.extend(store_for(req, &key)?.save(&fresh)?);
    write_marker(req, &marker, meta, stamp)?;
    Ok((fresh, counts, count))
}

/// Match and write report.json, unsuspend.json and prompt.md into `out`.
pub fn run_match(
    index: &DeckIndex,
    chunks: &[crate::Chunk],
    source_name: &str,
    thresholds: &Thresholds,
    opts: &MatchOptions,
    out: &Path,
) -> Result<Report, Error> {
    let report = match_chunks(index, chunks, thresholds, opts)?;
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&report)? + "\n")?;
    std::fs::write(out.join("unsuspend.json"), serde_json::to_string_pretty(&report.unsuspend)? + "\n")?;
    std::fs::write(out.join("prompt.md"), build_prompt(&report, source_name, 24000))?;
    Ok(report)
}
