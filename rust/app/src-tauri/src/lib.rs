//! Tauri shell around `ankiplus-match`.
//!
//! Commands the UI calls:
//! - `list_anki_profiles`: the Anki profiles on this machine,
//! - `index_deck`: load (or build and cache) the index of a deck export, or of a profile's
//!   own collection (no export); a changed deck re-embeds only new or edited notes,
//! - `index_written_cards`: add cards just written to Anki to the index at once,
//! - `match_source`: match lecture text, a PDF or highlights; returns the report and the gaps-only prompt,
//! - `check_support`: flag drafted cards whose cloze answers are not in the source.
//!
//! Nothing here touches Anki: the unsuspend list is shown to the user, who applies
//! it through the Anki MCP.

use ankiplus_match::pipeline::{index_written_cards as add_written, open_index, Backend, EmbeddingMode, IndexRequest, WrittenCard};
use ankiplus_match::{
    build_prompt, check_support as check, chunks_from_highlights, chunks_from_text, match_chunks, Chunk, DeckIndex,
    DraftCard, Highlight, MatchOptions, Report, SupportReport, Thresholds,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;

#[derive(Default)]
struct AppState {
    index: Mutex<Option<DeckIndex>>,
    /// The request that built `index`, so written cards go into the same cache folder.
    request: Mutex<Option<Indexed>>,
}

struct Indexed {
    deck: PathBuf,
    deck_prefix: Option<String>,
    index_dir: PathBuf,
    embeddings: EmbeddingMode,
    backend: Backend,
}

impl Indexed {
    fn request(&self) -> IndexRequest<'_> {
        IndexRequest {
            deck: &self.deck,
            deck_prefix: self.deck_prefix.as_deref(),
            index_dir: &self.index_dir,
            embeddings: self.embeddings,
            backend: self.backend,
            rebuild: false,
        }
    }
}

fn unsuspend_log(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app.path().app_cache_dir().map_err(|e| e.to_string())?.join("indexes").join("unsuspended.jsonl"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexArgs {
    /// A deck export; leave out (or give `profile`) to read the Anki profile's collection.
    deck_path: Option<String>,
    profile: Option<String>,
    deck_prefix: Option<String>,
    #[serde(default)]
    embeddings: Option<String>,
    #[serde(default)]
    postgres: bool,
    #[serde(default)]
    rebuild: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexInfo {
    notes: usize,
    suspended_cards: usize,
    embeddings: bool,
    source: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceArgs {
    text: Option<String>,
    pdf_path: Option<String>,
    highlights: Option<Vec<Highlight>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatchArgs {
    source: SourceArgs,
    source_name: Option<String>,
    covered: Option<f64>,
    related: Option<f64>,
    #[serde(default)]
    unsuspend_concepts: bool,
}

#[derive(Serialize)]
struct MatchResult {
    report: Report,
    prompt: String,
}

fn chunks(source: &SourceArgs) -> Result<Vec<Chunk>, String> {
    if let Some(items) = &source.highlights {
        return Ok(chunks_from_highlights(items));
    }
    if let Some(path) = &source.pdf_path {
        #[cfg(feature = "pdf")]
        return ankiplus_match::pdf::chunks_from_pdf(std::path::Path::new(path), None).map_err(|e| e.to_string());
        #[cfg(not(feature = "pdf"))]
        return Err(format!("PDF reading is not in this build ({path}); paste the text instead."));
    }
    match &source.text {
        Some(t) => Ok(chunks_from_text(t)),
        None => Err("Give lecture text, a PDF or highlights.".into()),
    }
}

#[tauri::command]
fn list_anki_profiles() -> serde_json::Value {
    let base = ankiplus_match::profiles::anki_base();
    serde_json::json!({"base": base.to_string_lossy(), "profiles": ankiplus_match::profiles::list_profiles(Some(&base))})
}

#[tauri::command]
fn index_deck(app: tauri::AppHandle, state: tauri::State<AppState>, args: IndexArgs) -> Result<IndexInfo, String> {
    let index_dir: PathBuf = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("indexes");
    let deck = match args.deck_path.as_deref().filter(|p| !p.is_empty() && args.profile.is_none()) {
        Some(p) => PathBuf::from(p),
        None => ankiplus_match::profiles::collection_for(args.profile.as_deref(), None).map_err(|e| e.to_string())?,
    };
    let embeddings: EmbeddingMode = args.embeddings.as_deref().unwrap_or("auto").parse().map_err(|e: ankiplus_match::Error| e.to_string())?;
    let indexed = Indexed {
        deck: deck.clone(),
        deck_prefix: args.deck_prefix.filter(|p| !p.is_empty()),
        index_dir,
        embeddings,
        backend: if args.postgres { Backend::Postgres } else { Backend::File },
    };
    let req = IndexRequest { rebuild: args.rebuild, ..indexed.request() };
    let index = open_index(&req, |_| {}).map_err(|e| e.to_string())?;
    let info = IndexInfo {
        notes: index.notes.len(),
        suspended_cards: index.notes.iter().map(|n| n.suspended_card_ids.len()).sum(),
        embeddings: index.has_embeddings(),
        source: deck.to_string_lossy().to_string(),
    };
    *state.index.lock().map_err(|e| e.to_string())? = Some(index);
    *state.request.lock().map_err(|e| e.to_string())? = Some(indexed);
    Ok(info)
}

/// Add cards the host just created in Anki ({note_id, card_ids, deck, tags, fields}) to the
/// index, so the next lecture matches against them. Only these notes are embedded.
#[tauri::command]
fn index_written_cards(state: tauri::State<AppState>, written: Vec<WrittenCard>) -> Result<serde_json::Value, String> {
    let mut guard = state.index.lock().map_err(|e| e.to_string())?;
    let request = state.request.lock().map_err(|e| e.to_string())?;
    let (index, indexed) = (guard.as_ref().ok_or("Index a deck first.")?, request.as_ref().ok_or("Index a deck first.")?);
    let (fresh, counts, added) = add_written(&indexed.request(), index, &written).map_err(|e| e.to_string())?;
    let notes = fresh.notes.len();
    *guard = Some(fresh);
    Ok(serde_json::json!({"added": added, "notes": notes, "embedded": counts.embedded, "reused": counts.reused}))
}

#[tauri::command]
fn match_source(app: tauri::AppHandle, state: tauri::State<AppState>, args: MatchArgs) -> Result<MatchResult, String> {
    let guard = state.index.lock().map_err(|e| e.to_string())?;
    let index = guard.as_ref().ok_or("Index a deck first.")?;
    let chunks = chunks(&args.source)?;
    let t = Thresholds { covered: args.covered, related: args.related, ..Thresholds::default() };
    let already = ankiplus_match::unsuspend_log::already_unsuspended(&unsuspend_log(&app)?);
    let opts = MatchOptions { unsuspend_concepts: args.unsuspend_concepts, already_unsuspended: already, ..MatchOptions::default() };
    let report = match_chunks(index, &chunks, &t, &opts).map_err(|e| e.to_string())?;
    let prompt = build_prompt(&report, args.source_name.as_deref().unwrap_or("lecture"), 24000);
    Ok(MatchResult { report, prompt })
}

#[tauri::command]
fn check_support(cards: Vec<DraftCard>, source: SourceArgs, answer_threshold: Option<f64>) -> Result<SupportReport, String> {
    Ok(check(&cards, &chunks(&source)?, answer_threshold.unwrap_or(0.8)))
}

/// Log CARD ids the user unsuspended through the Anki MCP; later matches skip them.
#[tauri::command]
fn record_unsuspend(app: tauri::AppHandle, card_ids: Vec<i64>, status: String, detail: Option<String>) -> Result<serde_json::Value, String> {
    ankiplus_match::unsuspend_log::record(&unsuspend_log(&app)?, &card_ids, &status, None, None, detail.as_deref().unwrap_or(""))
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![list_anki_profiles, index_deck, index_written_cards, match_source, check_support, record_unsuspend])
        .build(tauri::generate_context!())
        .expect("error while building the AnkiPlus app")
        .run(|_app, event| {
            #[cfg(feature = "postgres")]
            if let tauri::RunEvent::Exit = event {
                ankiplus_match::pgstore::shutdown();
            }
            let _ = event;
        });
}
