//! Classify each chunk as covered / related / gap and collect cards to unsuspend.

use crate::chunks::Chunk;
use crate::index::{DeckIndex, Hit};
use crate::text::{round_to, take_chars, tokenize};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use regex::Regex;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;

// A number with its unit: "3 mg/dL", "2.5 mg/dl", "40%", "6 weeks". Python's pattern starts
// with the lookbehind `(?<![\w.])`, which the regex crate lacks; `quantities` checks it instead.
static QUANTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)?)\s*(%|[A-Za-z\u{b5}\u{3bc}]+(?:/[A-Za-z]+)?)").unwrap());
const NOT_UNITS: [&str; 26] = [
    "and", "or", "of", "to", "the", "in", "a", "an", "x", "times", "th", "st", "nd", "rd", "is", "are", "with", "for",
    "per", "on", "at", "by", "from", "than", "more", "less",
];

/// `{unit: values}` (sorted, distinct) for the numbers with units in a text.
pub fn quantities(text: &str) -> BTreeMap<String, Vec<f64>> {
    let mut out: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut prev: Option<char> = None;
    let mut skip_to = 0;
    for (i, ch) in text.char_indices() {
        let before = prev;
        prev = Some(ch);
        if i < skip_to || !ch.is_numeric() || before.is_some_and(|b| b == '.' || b.is_alphanumeric() || b == '_') {
            continue;
        }
        let Some(c) = QUANTITY.captures(&text[i..]) else { continue };
        skip_to = i + c[0].len();
        let unit = c[2].to_lowercase();
        if NOT_UNITS.contains(&unit.as_str()) {
            continue;
        }
        let Ok(value) = ascii_digits(&c[1]).parse::<f64>() else { continue };
        let values = out.entry(unit).or_default();
        if !values.contains(&value) {
            values.push(value);
            values.sort_by(f64::total_cmp);
        }
    }
    out
}

static DIGIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d$").unwrap());

/// Any Unicode decimal digit as its ASCII digit (Python's `float` reads them all). Each
/// script's digits are a run of ten code points, 0 first.
fn ascii_digits(s: &str) -> String {
    let is_digit = |u: u32| char::from_u32(u).is_some_and(|c| DIGIT.is_match(c.encode_utf8(&mut [0; 4])));
    s.chars()
        .map(|c| {
            if c.is_ascii() {
                return c;
            }
            let mut k = 0;
            while k < 64 && is_digit(c as u32 - k - 1) {
                k += 1;
            }
            char::from_digit(k % 10, 10).unwrap_or(c)
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Quantity {
    pub unit: String,
    pub lecture: Vec<f64>,
    pub card: Vec<f64>,
}

/// Quantities stated with the same unit in both texts but with no value in common,
/// e.g. a lecture's "exceeds 3 mg/dL" against a card's "> 2.5 mg/dL".
pub fn conflicts(lecture: &str, card: &str) -> Vec<Quantity> {
    let (ours, theirs) = (quantities(lecture), quantities(card));
    ours.iter()
        .filter_map(|(u, l)| {
            let c = theirs.get(u)?;
            (!l.iter().any(|v| c.contains(v))).then(|| Quantity { unit: u.clone(), lecture: l.clone(), card: c.clone() })
        })
        .collect()
}

/// The card as the student sees it: the hidden answer blanked in the note's first field.
fn question(front: &str, answer: &str) -> String {
    let first = answer.split(" \u{2026} ").next().unwrap_or("");
    if !first.is_empty() && front.contains(first) {
        front.replacen(first, "[...]", 1)
    } else {
        front.to_string()
    }
}

/// `None` picks the calibrated default for the index (with or without embeddings),
/// as in the Python `Thresholds.resolve`: BM25 only 0.5 / 0.35 (calibrated on a real
/// AnKing export), with Nomic embeddings 0.7 / 0.62 (same labels).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Thresholds {
    pub covered: Option<f64>,
    pub related: Option<f64>,
    /// Existing notes accepted per covered chunk.
    pub per_chunk: usize,
    /// Below 'covered': notes per chunk offered for the host to judge.
    #[serde(default = "three")]
    pub candidates: usize,
    /// Concept neighbours listed per covered chunk.
    pub concepts: usize,
    /// Topic tags larger than this are too broad to expand.
    pub max_group: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { covered: None, related: None, per_chunk: 3, candidates: 3, concepts: 5, max_group: 150 }
    }
}

fn three() -> usize {
    3
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
    pub front: String,
    pub cards: Vec<CardQuestion>,
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
    pub answer_named: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tag: Option<String>,
}

/// One suspended card of a note: what the student is asked and what is hidden.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardQuestion {
    pub card_id: i64,
    pub question: String,
    pub answer: String,
}

/// A suspended match that was not accepted, for the host to judge (`include_note_ids`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub note_id: i64,
    pub score: f64,
    pub chunks: Vec<String>,
    pub preview: String,
    pub cards: Vec<CardQuestion>,
    pub suspended_card_ids: Vec<i64>,
    pub answer_named: bool,
    /// Some card's hidden answer is in the lecture line or its heading.
    #[serde(default)]
    pub answer_in_lecture: bool,
}

/// An accepted match that states a different number with the same unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Clash {
    pub note_id: i64,
    pub preview: String,
    pub quantities: Vec<Quantity>,
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
    #[serde(default)]
    pub conflicts: Vec<Clash>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub covered: usize,
    pub related: usize,
    pub gap: usize,
    #[serde(default)]
    pub conflict: usize,
    pub chunks: usize,
    pub notes_to_unsuspend: usize,
    pub cards_to_unsuspend: usize,
    /// Suspended matches below 'covered' offered for the host to judge.
    #[serde(default)]
    pub candidates_to_judge: usize,
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
    #[serde(default)]
    pub cards: Vec<CardQuestion>,
    #[serde(default)]
    pub answer_in_lecture: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Unsuspend {
    pub card_ids: Vec<i64>,
    pub notes: Vec<UnsuspendNote>,
    pub performed_anki_operations: usize,
    #[serde(default)]
    pub candidates: Vec<Candidate>,
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
    let front = note.front.as_deref().filter(|f| !f.is_empty()).unwrap_or(&note.text).to_string();
    let empty = indexmap::IndexMap::new();
    let answers = note.card_answers.as_ref().unwrap_or(&empty);
    let cards = note
        .suspended_card_ids
        .iter()
        .filter_map(|c| {
            answers.get(c).map(|a| CardQuestion { card_id: *c, question: question(&front, a), answer: a.clone() })
        })
        .collect();
    NoteSummary {
        note_id: note.note_id,
        deck: note.deck.clone(),
        suspended_card_ids: note.suspended_card_ids.clone(),
        front,
        cards,
        active: note.card_ids.len() > note.suspended_card_ids.len(),
        preview: take_chars(&note.text.replace('\n', " "), 160),
        score: hit.map(|h| h.score),
        overlap: hit.map(|h| h.overlap),
        cosine: hit.map(|h| h.cosine),
        answer_named: hit.map(|h| h.answer_named),
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
    let mut candidates: IndexMap<i64, Candidate> = IndexMap::new();
    let mut claim = |s: &NoteSummary, chunk: &Chunk, reason: &str| {
        let chunk_id = chunk.id.as_str();
        let mut pending: Vec<i64> = s.suspended_card_ids.iter().copied().filter(|c| !already.contains(c)).collect();
        skipped.extend(s.suspended_card_ids.iter().copied().filter(|c| already.contains(c)));
        if pending.is_empty() {
            return;
        }
        // A cloze note: propose only the siblings whose hidden answer is in the lecture line.
        let cards: Vec<&CardQuestion> = s.cards.iter().filter(|c| pending.contains(&c.card_id)).collect();
        let terms: HashSet<String> = tokenize(&format!("{} {}", chunk.text, chunk.context)).into_iter().collect();
        let testing: Vec<i64> = cards
            .iter()
            .filter(|c| tokenize(&c.answer).iter().any(|t| terms.contains(t)))
            .map(|c| c.card_id)
            .collect();
        if !testing.is_empty() {
            pending = testing.clone();
        }
        let entry = unsuspend.entry(s.note_id).or_insert_with(|| UnsuspendNote {
            note_id: s.note_id,
            card_ids: vec![],
            chunks: vec![],
            reason: reason.to_string(),
            preview: s.preview.clone(),
            cards: vec![],
            answer_in_lecture: false,
        });
        entry.card_ids.extend(pending);
        entry.card_ids.sort_unstable();
        entry.card_ids.dedup();
        entry.answer_in_lecture |= !testing.is_empty();
        let kept: Vec<CardQuestion> =
            cards.iter().filter(|c| entry.card_ids.contains(&c.card_id)).map(|c| (*c).clone()).collect();
        if !kept.is_empty() {
            entry.cards = kept;
        }
        if !entry.chunks.iter().any(|c| c == chunk_id) {
            entry.chunks.push(chunk_id.to_string());
        }
        if reason == "match" {
            entry.reason = "match".into();
        }
    };

    for chunk in chunks {
        let query =
            if chunk.context.is_empty() { chunk.text.clone() } else { format!("{}: {}", chunk.context, chunk.text) };
        let hits = index.search(&query, opts.k, 50)?;
        let best = hits.first().map_or(0.0, |h| h.score);
        let mut status = if best >= covered {
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
        let mut accepted: Vec<&NoteSummary> =
            matches.iter().filter(|m| m.score.unwrap_or(0.0) >= covered).take(t.per_chunk).collect();
        let clashes: Vec<Clash> = accepted
            .iter()
            .filter_map(|m| {
                let found = conflicts(&chunk.text, &m.front);
                (!found.is_empty()).then(|| Clash { note_id: m.note_id, preview: m.preview.clone(), quantities: found })
            })
            .collect();
        if !clashes.is_empty() {
            accepted.retain(|m| !clashes.iter().any(|c| c.note_id == m.note_id));
            if accepted.is_empty() {
                status = "conflict"; // the deck states a different number: card the lecture's version
            }
        }
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
            claim(m, chunk, "match");
        }
        let taken: HashSet<i64> =
            accepted.iter().map(|m| m.note_id).chain(clashes.iter().map(|c| c.note_id)).collect();
        let terms_here: HashSet<String> =
            tokenize(&format!("{} {}", chunk.text, chunk.context)).into_iter().collect();
        for m in matches.iter().filter(|m| !taken.contains(&m.note_id) && !m.suspended_card_ids.is_empty()).take(t.candidates) {
            // Offer only the cards whose hidden answer the line (or its heading) contains: a card that hides
            // something the lecture never says cannot test it. A named answer keeps its cards as they are.
            let open_cards: Vec<&CardQuestion> = m
                .cards
                .iter()
                .filter(|c| m.suspended_card_ids.contains(&c.card_id) && !already.contains(&c.card_id))
                .collect();
            let mut testing: Vec<&CardQuestion> =
                open_cards.iter().copied().filter(|c| tokenize(&c.answer).iter().any(|t| terms_here.contains(t))).collect();
            let in_line = !testing.is_empty();
            let named = m.answer_named.unwrap_or(false);
            if !in_line && named {
                testing = open_cards;
            }
            if testing.is_empty() {
                continue;
            }
            let entry = candidates.entry(m.note_id).or_insert_with(|| Candidate {
                note_id: m.note_id,
                score: m.score.unwrap_or(0.0),
                chunks: vec![],
                preview: m.preview.clone(),
                cards: vec![],
                suspended_card_ids: vec![],
                answer_named: named,
                answer_in_lecture: false,
            });
            entry.answer_in_lecture |= in_line;
            for c in testing {
                if !entry.cards.iter().any(|e| e.card_id == c.card_id) {
                    entry.cards.push(c.clone());
                }
            }
            entry.suspended_card_ids = entry.cards.iter().map(|c| c.card_id).collect();
            entry.suspended_card_ids.sort_unstable();
            entry.suspended_card_ids.dedup();
            entry.chunks.push(chunk.id.clone());
        }
        if opts.unsuspend_concepts && status == "covered" {
            for c in &concepts {
                claim(c, chunk, "concept");
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
            conflicts: clashes,
        });
    }
    let mut judge: Vec<Candidate> = candidates.into_values().filter(|c| !unsuspend.contains_key(&c.note_id)).collect();
    judge.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)); // stable, like Python
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
        conflict: count("conflict"),
        chunks: rows.len(),
        notes_to_unsuspend: plan.len(),
        cards_to_unsuspend: card_ids.len(),
        candidates_to_judge: judge.len(),
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
            candidates: judge,
            instructions: JUDGE.into(),
        },
    })
}

pub const JUDGE: &str = "Before showing this list, judge every note: read each card's question and answer (not the Extra) and keep it \
only if that card tests the lecture line it matched. Drop cards that are merely on the same topic, and pass the \
dropped notes as exclude_note_ids to prepare_unsuspend. Also judge the candidates (scored below covered; \
answer_named means the card hides a term the line names): add the ones that do test a lecture line with \
include_note_ids. Then show the user the list; after approval, unsuspend these CARD ids with the official Anki MCP.";
