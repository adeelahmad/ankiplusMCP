//! Deterministic matching of lecture content against an existing deck (e.g. AnKing).
//!
//! Rust port of the Python `ankiplusmcp.match` package. No model calls and no Anki
//! connection: the deck is read from an export or a read-only collection copy, and
//! the results are plans the host executes through the official Anki MCP after the
//! user approves them.
//!
//! Pipeline: deck file -> [`DeckIndex`] (BM25 + optional embeddings, RRF k = 60) ->
//! chunks from the lecture -> per-chunk hybrid search -> covered / related / gap ->
//! unsuspend plan and a drafting prompt that lists only the gaps.

pub mod chunks;
pub mod deck;
pub mod evaluate;
pub mod index;
pub mod matcher;
pub mod pipeline;
pub mod prompt;
pub mod quotes;
pub mod store;
pub mod text;
pub mod unsuspend_log;

#[cfg(feature = "embed")]
pub mod embed;
#[cfg(feature = "pdf")]
pub mod pdf;
#[cfg(feature = "postgres")]
pub mod pgstore;

pub use chunks::{chunks_from_highlights, chunks_from_pages, chunks_from_slides, chunks_from_text, Chunk, Highlight, Slide};
pub use deck::{load_deck, LoadOptions, Note};
pub use evaluate::{evaluate, Evaluation, Label};
pub use index::{DeckIndex, Embedder, Hit};
pub use matcher::{match_chunks, MatchOptions, Report, Thresholds};
pub use prompt::build_prompt;
pub use quotes::{check_support, DraftCard, SupportReport};
pub use store::{FileIndexStore, IndexStore};
pub use text::{clean_text, tokenize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Deck(String),
    #[error("{0}")]
    Index(String),
    #[error("{0}")]
    Embed(String),
    #[error("{0}")]
    Source(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
