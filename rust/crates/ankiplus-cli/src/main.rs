//! `ankiplus`: the deterministic matching pipeline from a terminal, no AI host needed.
//!
//! ```text
//! ankiplus match --deck AnKing.apkg --pdf lecture.pdf --out builds/L025
//! ankiplus match --deck AnKing.apkg --highlights highlights.json --out builds/L025
//! ankiplus match --deck AnKing.apkg --evaluate labels.json
//! ankiplus check --cards drafts.json --text lecture.md
//! ```
//!
//! Same flags, cache folder and outputs as `ankiplusmcp match`.

use ankiplus_match::pipeline::{open_index, run_match, Backend, EmbeddingMode, IndexRequest};
use ankiplus_match::{
    chunks_from_highlights, chunks_from_text, evaluate, Chunk, DraftCard, Error, Highlight, Label, MatchOptions, Thresholds,
};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "ankiplus", version, about = "Match lectures against an existing Anki deck without AI.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Match a lecture (or highlights) against a deck; writes report.json, unsuspend.json, prompt.md.
    Match(MatchArgs),
    /// Flag drafted cards whose cloze answers are not in the source.
    Check(CheckArgs),
    /// Log CARD ids the host unsuspended, so later matches never propose them again.
    Record(RecordArgs),
}

#[derive(Args)]
struct RecordArgs {
    /// unsuspended.jsonl (shared format with the Python MCP server)
    #[arg(long)]
    log: PathBuf,
    /// Comma-separated CARD ids
    #[arg(long, value_delimiter = ',', required = true)]
    cards: Vec<i64>,
    #[arg(long, default_value = "success")]
    status: String,
    #[arg(long)]
    match_id: Option<String>,
    #[arg(long, default_value = "")]
    detail: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    File,
    Postgres,
}

#[derive(Clone, Copy, ValueEnum)]
enum EmbedArg {
    Auto,
    Off,
    Required,
}

#[derive(Args)]
#[group(id = "source", required = true, multiple = false)]
struct Source {
    #[arg(long)]
    pdf: Option<PathBuf>,
    #[arg(long)]
    text: Option<PathBuf>,
    /// JSON list of {page, text, note}
    #[arg(long)]
    highlights: Option<PathBuf>,
}

#[derive(Args)]
struct MatchArgs {
    /// .apkg, .colpkg, .anki2 or .json/.jsonl note list
    #[arg(long)]
    deck: PathBuf,
    /// Only notes in this deck or its subdecks
    #[arg(long)]
    deck_prefix: Option<String>,
    #[arg(long)]
    pdf: Option<PathBuf>,
    #[arg(long)]
    text: Option<PathBuf>,
    /// JSON list of {page, text, note}
    #[arg(long)]
    highlights: Option<PathBuf>,
    /// JSON list of {text, note_ids}; prints recall and gap precision
    #[arg(long, value_name = "LABELS")]
    evaluate: Option<PathBuf>,
    /// Output folder (report.json, unsuspend.json, prompt.md)
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    index_dir: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "file")]
    backend: BackendArg,
    #[arg(long, value_enum, default_value = "auto")]
    embeddings: EmbedArg,
    /// Default: 0.5 BM25 only, 0.7 with embeddings
    #[arg(long)]
    covered: Option<f64>,
    /// Default: 0.35 BM25 only, 0.62 with embeddings
    #[arg(long)]
    related: Option<f64>,
    /// Also unsuspend concept neighbours of covered chunks
    #[arg(long)]
    unsuspend_concepts: bool,
    #[arg(long)]
    rebuild: bool,
    /// unsuspended.jsonl: CARD ids logged as unsuspended are not proposed again
    #[arg(long)]
    unsuspended_log: Option<PathBuf>,
}

#[derive(Args)]
struct CheckArgs {
    /// JSON list of {id, fields, source_ref}
    #[arg(long)]
    cards: PathBuf,
    #[command(flatten)]
    source: Source,
    #[arg(long, default_value_t = 0.8)]
    answer_threshold: f64,
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn default_index_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default();
    PathBuf::from(home).join(".cache").join("ankiplusmcp").join("indexes")
}

fn load_chunks(pdf: &Option<PathBuf>, text: &Option<PathBuf>, highlights: &Option<PathBuf>) -> Result<(Vec<Chunk>, String), Error> {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if let Some(p) = pdf {
        #[cfg(feature = "pdf")]
        return Ok((ankiplus_match::pdf::chunks_from_pdf(p, None)?, name(p)));
        #[cfg(not(feature = "pdf"))]
        {
            let _ = p;
            return Err(Error::Source("PDF input needs a build with the `pdf` feature; use --text or --highlights.".into()));
        }
    }
    if let Some(p) = text {
        return Ok((chunks_from_text(&std::fs::read_to_string(p)?), name(p)));
    }
    if let Some(p) = highlights {
        let items: Vec<Highlight> = read_json(p)?;
        return Ok((chunks_from_highlights(&items), name(p)));
    }
    Err(Error::Source("Give --pdf, --text or --highlights.".into()))
}

fn run_match_cmd(a: MatchArgs) -> Result<(), Error> {
    let sources = [a.pdf.is_some(), a.text.is_some(), a.highlights.is_some(), a.evaluate.is_some()];
    if sources.iter().filter(|s| **s).count() != 1 {
        return Err(Error::Source("Give exactly one of --pdf, --text, --highlights or --evaluate.".into()));
    }
    let index_dir = a.index_dir.clone().unwrap_or_else(default_index_dir);
    let req = IndexRequest {
        deck: &a.deck,
        deck_prefix: a.deck_prefix.as_deref(),
        index_dir: &index_dir,
        embeddings: match a.embeddings {
            EmbedArg::Auto => EmbeddingMode::Auto,
            EmbedArg::Off => EmbeddingMode::Off,
            EmbedArg::Required => EmbeddingMode::Required,
        },
        backend: match a.backend {
            BackendArg::File => Backend::File,
            BackendArg::Postgres => Backend::Postgres,
        },
        rebuild: a.rebuild,
    };
    let index = open_index(&req, |msg| eprintln!("{msg}"))?;
    let thresholds = Thresholds { covered: a.covered, related: a.related, ..Thresholds::default() };
    if let Some(labels) = &a.evaluate {
        let labels: Vec<Label> = read_json(labels)?;
        println!("{}", serde_json::to_string_pretty(&evaluate(&index, &labels, 5, a.covered)?)?);
        return Ok(());
    }
    let out = a.out.clone().ok_or_else(|| Error::Source("--out is required unless --evaluate is used".into()))?;
    let (chunks, name) = load_chunks(&a.pdf, &a.text, &a.highlights)?;
    let already = a.unsuspended_log.as_deref().map(ankiplus_match::unsuspend_log::already_unsuspended).unwrap_or_default();
    let opts = MatchOptions { unsuspend_concepts: a.unsuspend_concepts, already_unsuspended: already, ..MatchOptions::default() };
    let report = run_match(&index, &chunks, &name, &thresholds, &opts, &out)?;
    let s = &report.summary;
    println!(
        "{} chunks: {} covered, {} related, {} gaps; {} suspended cards to unsuspend -> {}",
        s.chunks, s.covered, s.related, s.gap, s.cards_to_unsuspend, out.display()
    );
    Ok(())
}

fn run_check(a: CheckArgs) -> Result<(), Error> {
    let cards: Vec<DraftCard> = read_json(&a.cards)?;
    let (chunks, _) = load_chunks(&a.source.pdf, &a.source.text, &a.source.highlights)?;
    let report = ankiplus_match::check_support(&cards, &chunks, a.answer_threshold);
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Match(a) => run_match_cmd(a),
        Command::Check(a) => run_check(a),
        Command::Record(a) => ankiplus_match::unsuspend_log::record(&a.log, &a.cards, &a.status, a.match_id.as_deref(), None, &a.detail)
            .and_then(|row| Ok(println!("{}", serde_json::to_string(&row)?))),
    };
    #[cfg(feature = "postgres")]
    ankiplus_match::pgstore::shutdown();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
