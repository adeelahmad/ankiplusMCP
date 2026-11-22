//! Tauri shell around `ankiplus-match`.
//!
//! Commands the UI calls:
//! - `index_deck`: load (or build and cache) the index of a deck export,
//! - `match_source`: match lecture text, a PDF or highlights; returns the report and the gaps-only prompt,
//! - `check_support`: flag drafted cards whose cloze answers are not in the source.
//!
//! Nothing here touches Anki: the unsuspend list is shown to the user, who applies
//! it through the Anki MCP.

use ankiplus_match::pipeline::{open_index, Backend, EmbeddingMode, IndexRequest};
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexArgs {
    deck_path: String,
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
fn index_deck(app: tauri::AppHandle, state: tauri::State<AppState>, args: IndexArgs) -> Result<IndexInfo, String> {
    let index_dir: PathBuf = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("indexes");
    let deck = PathBuf::from(&args.deck_path);
    let embeddings: EmbeddingMode = args.embeddings.as_deref().unwrap_or("auto").parse().map_err(|e: ankiplus_match::Error| e.to_string())?;
    let req = IndexRequest {
        deck: &deck,
        deck_prefix: args.deck_prefix.as_deref().filter(|p| !p.is_empty()),
        index_dir: &index_dir,
        embeddings,
        backend: if args.postgres { Backend::Postgres } else { Backend::File },
        rebuild: args.rebuild,
    };
    let index = open_index(&req, |_| {}).map_err(|e| e.to_string())?;
    let info = IndexInfo {
        notes: index.notes.len(),
        suspended_cards: index.notes.iter().map(|n| n.suspended_card_ids.len()).sum(),
        embeddings: index.has_embeddings(),
        source: deck.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
    };
    *state.index.lock().map_err(|e| e.to_string())? = Some(index);
    Ok(info)
}

#[tauri::command]
fn match_source(state: tauri::State<AppState>, args: MatchArgs) -> Result<MatchResult, String> {
    let guard = state.index.lock().map_err(|e| e.to_string())?;
    let index = guard.as_ref().ok_or("Index a deck first.")?;
    let chunks = chunks(&args.source)?;
    let defaults = Thresholds::default();
    let t = Thresholds {
        covered: args.covered.unwrap_or(defaults.covered),
        related: args.related.unwrap_or(defaults.related),
        ..defaults
    };
    let opts = MatchOptions { unsuspend_concepts: args.unsuspend_concepts, ..MatchOptions::default() };
    let report = match_chunks(index, &chunks, &t, &opts).map_err(|e| e.to_string())?;
    let prompt = build_prompt(&report, args.source_name.as_deref().unwrap_or("lecture"), 24000);
    Ok(MatchResult { report, prompt })
}

#[tauri::command]
fn check_support(cards: Vec<DraftCard>, source: SourceArgs, answer_threshold: Option<f64>) -> Result<SupportReport, String> {
    Ok(check(&cards, &chunks(&source)?, answer_threshold.unwrap_or(0.8)))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![index_deck, match_source, check_support])
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
