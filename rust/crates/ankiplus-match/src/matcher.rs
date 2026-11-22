//! Classify each chunk as covered / related / gap and collect cards to unsuspend.

use crate::chunks::Chunk;
use crate::index::{DeckIndex, Hit};
use crate::text::{round_to, take_chars, tokenize};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// `None` picks the calibrated default for the index (with or without embeddings),
/// as in the Python `Thresholds.resolve`: BM25 only 0.5 / 0.35 (calibrated on a real
/// AnKing export), with Nomic embeddings 0.7 / 0.62 (same labels).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Thresholds {
    pub covered: Option<f64>,
    pub related: Option<f64>,
    /// Existing notes accepted per covered chunk.
    pub per_chunk: usize,
    /// Concept neighbours listed per covered chunk.
    pub concepts: usize,
    /// Topic tags larger than this are too broad to expand.
    pub max_group: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { covered: None, related: None, per_chunk: 3, concepts: 5, max_group: 150 }
    }
}

impl Thresholds {
    pub const BM25: (f64, f64) = (0.5, 0.35);
    pub const EMBEDDINGS: (f64, f64) = (0.7, 0.62);

    /// Fill unset thresholds with the calibrated defaults.
    pub fn resolve(&self, embeddings: bool) -> Thresholds {
        let (covered, related) = if embeddings { Self::EMBEDDINGS } else { Self::BM25 };
        Thresholds { covered: Some(self.covered.unwrap_or(covered)), related: Some(self.related.unwrap_or(related)), ..*self }
    }

    pub fn covered_or_default(&self, embeddings: bool) -> f64 {
        self.resolve(embeddings).covered.unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteSummary {
    pub note_id: i64,
    pub deck: String,
    pub suspended_card_ids: Vec<i64>,
    pub active: bool,
    pub preview: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub overlap: Option<f64>,
    /// `Some(None)` serialises as `null` (BM25 only), like the Python report.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cosine: Option<Option<f64>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkResult {
    pub id: String,
    pub page: i64,
    pub title: String,
    pub text: String,
    pub status: String,
    pub best_score: f64,
    pub matches: Vec<NoteSummary>,
    pub concepts: Vec<NoteSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub covered: usize,
    pub related: usize,
    pub gap: usize,
    pub chunks: usize,
    pub notes_to_unsuspend: usize,
    pub cards_to_unsuspend: usize,
    /// Distinct suspended CARD ids skipped because a previous lecture already unsuspended them.
    pub already_unsuspended: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnsuspendNote {
    pub note_id: i64,
    pub card_ids: Vec<i64>,
    pub chunks: Vec<String>,
    pub reason: String,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unsuspend {
    pub card_ids: Vec<i64>,
    pub notes: Vec<UnsuspendNote>,
    pub performed_anki_operations: usize,
    pub instructions: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub thresholds: Thresholds,
    pub embeddings: bool,
    pub summary: Summary,
    pub chunks: Vec<ChunkResult>,
    pub unsuspend: Unsuspend,
}

#[derive(Debug, Clone)]
pub struct MatchOptions {
    pub k: usize,
    pub expand_concepts: bool,
    pub unsuspend_concepts: bool,
    /// CARD ids a previous lecture already unsuspended; never proposed again.
    pub already_unsuspended: HashSet<i64>,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self { k: 5, expand_concepts: true, unsuspend_concepts: false, already_unsuspended: HashSet::new() }
    }
}

fn summary(index: &DeckIndex, doc: usize, hit: Option<&Hit>) -> NoteSummary {
    let note = &index.notes[doc];
    NoteSummary {
        note_id: note.note_id,
        deck: note.deck.clone(),
        suspended_card_ids: note.suspended_card_ids.clone(),
        active: note.card_ids.len() > note.suspended_card_ids.len(),
        preview: take_chars(&note.text.replace('\n', " "), 160),
        score: hit.map(|h| h.score),
        overlap: hit.map(|h| h.overlap),
        cosine: hit.map(|h| h.cosine),
        tag: None,
    }
}

pub fn match_chunks(
    index: &DeckIndex,
    chunks: &[Chunk],
    t: &Thresholds,
    opts: &MatchOptions,
) -> Result<Report, crate::Error> {
    let resolved = t.resolve(index.has_embeddings());
    let t = &resolved;
    let (covered, related) = (t.covered.unwrap(), t.related.unwrap());
    let mut rows = Vec::new();
    let mut unsuspend: IndexMap<i64, UnsuspendNote> = IndexMap::new();
    let already = &opts.already_unsuspended;
    let mut skipped: HashSet<i64> = HashSet::new();
    let mut claim = |s: &NoteSummary, chunk_id: &str, reason: &str| {
        let pending: Vec<i64> = s.suspended_card_ids.iter().copied().filter(|c| !already.contains(c)).collect();
        skipped.extend(s.suspended_card_ids.iter().copied().filter(|c| already.contains(c)));
        if pending.is_empty() {
            return;
        }
        let entry = unsuspend.entry(s.note_id).or_insert_with(|| UnsuspendNote {
            note_id: s.note_id,
            card_ids: pending,
            chunks: vec![],
            reason: reason.to_string(),
            preview: s.preview.clone(),
        });
        if !entry.chunks.iter().any(|c| c == chunk_id) {
            entry.chunks.push(chunk_id.to_string());
        }
        if reason == "match" {
            entry.reason = "match".into();
        }
    };

    for chunk in chunks {
        let hits = index.search(&chunk.text, opts.k, 50)?;
        let best = hits.first().map_or(0.0, |h| h.score);
        let status = if best >= covered {
            "covered"
        } else if best >= related {
            "related"
        } else {
            "gap"
        };
        let matches: Vec<NoteSummary> = hits
            .iter()
            .filter(|h| h.score >= related)
            .map(|h| summary(index, h.doc, Some(h)))
            .take(opts.k.max(t.per_chunk))
            .collect();
        let accepted: Vec<&NoteSummary> =
            matches.iter().filter(|m| m.score.unwrap_or(0.0) >= covered).take(t.per_chunk).collect();
        let mut concepts: Vec<NoteSummary> = Vec::new();
        if opts.expand_concepts && status != "gap" {
            let mut seen: HashSet<i64> = matches.iter().map(|m| m.note_id).collect();
            let terms: HashSet<String> = tokenize(&chunk.text).into_iter().collect();
            'outer: for hit in hits.iter().take(t.per_chunk) {
                if hit.score < related {
                    break;
                }
                let (tag, docs) = index.neighbours(hit.doc, t.max_group);
                let mut ranked: Vec<(usize, usize)> = docs
                    .into_iter()
                    .map(|d| {
                        let words: HashSet<String> = tokenize(&index.notes[d].text).into_iter().collect();
                        (d, terms.intersection(&words).count())
                    })
                    .collect();
                ranked.sort_by_key(|&(_, shared)| std::cmp::Reverse(shared));
                for (doc, _) in ranked {
                    if !seen.insert(index.notes[doc].note_id) {
                        continue;
                    }
                    let mut s = summary(index, doc, None);
                    s.tag = tag.clone();
                    concepts.push(s);
                    if concepts.len() >= t.concepts {
                        break 'outer;
                    }
                }
            }
        }
        for m in &accepted {
            claim(m, &chunk.id, "match");
        }
        if opts.unsuspend_concepts && status == "covered" {
            for c in &concepts {
                claim(c, &chunk.id, "concept");
            }
        }
        rows.push(ChunkResult {
            id: chunk.id.clone(),
            page: chunk.page,
            title: chunk.title.clone(),
            text: chunk.text.clone(),
            status: status.into(),
            best_score: round_to(best, 4),
            matches,
            concepts,
        });
    }
    let mut plan: Vec<UnsuspendNote> = unsuspend.into_values().collect();
    plan.sort_by_key(|e| e.note_id);
    let mut card_ids: Vec<i64> = plan.iter().flat_map(|e| e.card_ids.iter().copied()).collect();
    card_ids.sort_unstable();
    card_ids.dedup();
    let count = |s: &str| rows.iter().filter(|r| r.status == s).count();
    let summary = Summary {
        covered: count("covered"),
        related: count("related"),
        gap: count("gap"),
        chunks: rows.len(),
        notes_to_unsuspend: plan.len(),
        cards_to_unsuspend: card_ids.len(),
        already_unsuspended: skipped.len(),
    };
    Ok(Report {
        thresholds: *t,
        embeddings: index.has_embeddings(),
        summary,
        chunks: rows,
        unsuspend: Unsuspend {
            card_ids,
            notes: plan,
            performed_anki_operations: 0,
            instructions: "Show the user this list. After approval, unsuspend these CARD ids with the official Anki MCP."
                .into(),
        },
    })
}
