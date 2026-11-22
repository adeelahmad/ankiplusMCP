//! Hybrid deck index: BM25 over note text and tag names, plus optional dense embeddings.
//!
//! Scores per hit (identical to the Python `DeckIndex.search`):
//! - `bm25`: raw Okapi BM25 (ranking only, not comparable across queries),
//! - `overlap`: IDF-weighted share of the query's terms the note contains (0..1),
//! - `reverse`: IDF-weighted share of the note's first field (the fact, not the Extra)
//!   that the query contains (0..1),
//! - `lexical`: geometric mean of `overlap` and `reverse`, halved unless the first field
//!   shares at least two non-number words with the query,
//! - `cosine`: embedding similarity (0..1) when embeddings exist, else `None`,
//! - `score`: `lexical`, raised to `0.3 * lexical + 0.7 * cosine` when that is higher.
//!
//! Numbers weigh a quarter of a word. Candidates (BM25 top 50, the 200 notes sharing the
//! most query weight, dense top 50) are merged by reciprocal rank fusion (k = 60).

use crate::deck::Note;
use crate::text::{concept_tags, round_to, tag_words, tokenize};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub const RRF_K: f64 = 60.0;

/// Something that turns text into vectors. Vectors need not be normalised.
pub trait Embedder {
    /// Model name, stored with the index; vectors are reused only when it matches.
    fn name(&self) -> String;
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, crate::Error>;
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, crate::Error>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub doc: usize,
    pub score: f64,
    pub overlap: f64,
    pub reverse: f64,
    pub lexical: f64,
    pub cosine: Option<f64>,
    pub bm25: f64,
    pub rrf: f64,
}

/// How many vectors `embed_all` computed, and how many it took from the previous index.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct EmbedCounts {
    pub embedded: usize,
    pub reused: usize,
}

/// sha1 hex of a note's text, the reuse key (with the note id) in `embed_all`.
pub fn digest(text: &str) -> String {
    use sha1::{Digest, Sha1};
    Sha1::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

pub struct DeckIndex {
    pub notes: Vec<Note>,
    pub meta: Map<String, Value>,
    /// Row-major, L2-normalised, `notes.len()` rows.
    pub vectors: Option<Vec<Vec<f32>>>,
    pub embedder: Option<Box<dyn Embedder + Send + Sync>>,
    k1: f64,
    b: f64,
    postings: HashMap<String, Vec<(usize, usize)>>,
    lengths: Vec<usize>,
    avgdl: f64,
    idf: HashMap<String, f64>,
    unseen: f64,
    fronts: Vec<(HashSet<String>, f64)>,
    by_tag: HashMap<String, Vec<usize>>,
}

pub fn normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm != 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

impl DeckIndex {
    pub fn new(notes: Vec<Note>, embedder: Option<Box<dyn Embedder + Send + Sync>>, meta: Map<String, Value>) -> Self {
        let mut index = DeckIndex {
            notes,
            meta,
            vectors: None,
            embedder,
            k1: 1.5,
            b: 0.75,
            postings: HashMap::new(),
            lengths: vec![],
            avgdl: 1.0,
            idf: HashMap::new(),
            unseen: 0.0,
            fronts: vec![],
            by_tag: HashMap::new(),
        };
        index.build_bm25();
        index.build_tags();
        index
    }

    fn build_bm25(&mut self) {
        for (i, note) in self.notes.iter().enumerate() {
            let mut counts: IndexMap<String, usize> = IndexMap::new();
            for t in tokenize(&note.text).into_iter().chain(tag_words(&note.tags)) {
                *counts.entry(t).or_default() += 1;
            }
            self.lengths.push(counts.values().sum());
            for (term, tf) in counts {
                self.postings.entry(term).or_default().push((i, tf));
            }
        }
        let n = self.notes.len().max(1) as f64;
        let avg = self.lengths.iter().sum::<usize>() as f64 / n;
        self.avgdl = if avg == 0.0 { 1.0 } else { avg };
        self.idf = self
            .postings
            .iter()
            .map(|(t, p)| {
                let df = p.len() as f64;
                (t.clone(), (1.0 + (n - df + 0.5) / (df + 0.5)).ln())
            })
            .collect();
        self.unseen = (1.0 + (n + 0.5) / 0.5).ln();
        self.fronts = self
            .notes
            .iter()
            .map(|note| {
                let src = note.front.as_deref().filter(|f| !f.is_empty()).unwrap_or(&note.text);
                let front: HashSet<String> = tokenize(src).into_iter().collect();
                let mass = front.iter().map(|t| self.weight(t)).sum();
                (front, mass)
            })
            .collect();
    }

    /// IDF, or a quarter of it for numbers; terms the deck never uses get the maximum.
    pub fn weight(&self, term: &str) -> f64 {
        let w = self.idf.get(term).copied().unwrap_or(self.unseen);
        if term.bytes().all(|b| b.is_ascii_digit()) {
            w * 0.25
        } else {
            w
        }
    }

    fn build_tags(&mut self) {
        for (i, note) in self.notes.iter().enumerate() {
            for tag in concept_tags(&note.tags) {
                self.by_tag.entry(tag).or_default().push(i);
            }
        }
    }

    pub fn has_embeddings(&self) -> bool {
        self.vectors.is_some()
    }

    /// Embed every note with the index's embedder (no-op without one).
    ///
    /// With `previous` (an older index of the same deck, embedded with the same model),
    /// notes whose `(note_id, sha1(text))` is unchanged keep their old vector, so an
    /// AnkiHub update re-embeds only new and edited notes.
    pub fn embed_all(
        &mut self,
        batch: usize,
        mut progress: impl FnMut(usize, usize),
        previous: Option<&DeckIndex>,
    ) -> Result<EmbedCounts, crate::Error> {
        let Some(embedder) = &self.embedder else { return Ok(EmbedCounts::default()) };
        let name = embedder.name();
        let mut old: HashMap<(i64, String), &Vec<f32>> = HashMap::new();
        if let Some(prev) = previous {
            if let Some(vectors) = &prev.vectors {
                if prev.meta.get("embedding_model").and_then(Value::as_str) == Some(name.as_str()) {
                    for (n, v) in prev.notes.iter().zip(vectors) {
                        old.insert((n.note_id, digest(&n.text)), v);
                    }
                }
            }
        }
        let keys: Vec<(i64, String)> = self.notes.iter().map(|n| (n.note_id, digest(&n.text))).collect();
        let todo: Vec<usize> = (0..keys.len()).filter(|&i| !old.contains_key(&keys[i])).collect();
        let mut fresh: HashMap<usize, Vec<f32>> = HashMap::new();
        let batch = batch.max(1);
        for start in (0..todo.len()).step_by(batch) {
            let part = &todo[start..(start + batch).min(todo.len())];
            let texts: Vec<String> = part.iter().map(|&i| self.notes[i].text.clone()).collect();
            for (&i, mut v) in part.iter().zip(embedder.embed_documents(&texts)?) {
                normalise(&mut v);
                fresh.insert(i, v);
            }
            progress((start + batch).min(todo.len()), todo.len());
        }
        let out: Vec<Vec<f32>> = keys
            .iter()
            .enumerate()
            .map(|(i, key)| fresh.remove(&i).unwrap_or_else(|| {
                let mut v = old[key].clone();
                normalise(&mut v);
                v
            }))
            .collect();
        self.meta.insert("embedding_model".into(), Value::String(name));
        self.vectors = Some(out);
        Ok(EmbedCounts { embedded: todo.len(), reused: keys.len() - todo.len() })
    }

    /// A new index with `added` notes in place of any notes with the same note id
    /// (cards just written to Anki), embedding only those and reusing every other vector.
    pub fn with_notes(&self, added: Vec<Note>, embedder: Option<Box<dyn Embedder + Send + Sync>>) -> Result<(DeckIndex, EmbedCounts), crate::Error> {
        let ids: HashSet<i64> = added.iter().map(|n| n.note_id).collect();
        let notes: Vec<Note> = self.notes.iter().filter(|n| !ids.contains(&n.note_id)).cloned().chain(added).collect();
        let mut fresh = DeckIndex::new(notes, embedder, self.meta.clone());
        let counts = if fresh.embedder.is_some() && self.vectors.is_some() {
            fresh.embed_all(256, |_, _| {}, Some(self))?
        } else {
            EmbedCounts::default()
        };
        Ok((fresh, counts))
    }

    pub fn search(&self, text: &str, k: usize, pool: usize) -> Result<Vec<Hit>, crate::Error> {
        let mut terms: IndexMap<String, usize> = IndexMap::new();
        for t in tokenize(text) {
            *terms.entry(t).or_default() += 1;
        }
        if terms.is_empty() {
            return Ok(vec![]);
        }
        // Terms the deck never uses still count (at maximum weight): a fact about them is not covered.
        let weights: HashMap<&str, f64> = terms.keys().map(|t| (t.as_str(), self.weight(t))).collect();
        let q_idf: f64 = terms.keys().map(|t| weights[t.as_str()]).sum();
        let mut bm25: IndexMap<usize, f64> = IndexMap::new();
        let mut present: IndexMap<usize, f64> = IndexMap::new();
        for term in terms.keys() {
            let Some(&idf) = self.idf.get(term) else { continue };
            for &(doc, tf) in &self.postings[term] {
                let tf = tf as f64;
                let norm = tf * (self.k1 + 1.0)
                    / (tf + self.k1 * (1.0 - self.b + self.b * self.lengths[doc] as f64 / self.avgdl));
                *bm25.entry(doc).or_default() += idf * norm;
                *present.entry(doc).or_default() += weights[term.as_str()];
            }
        }
        let mut lexical: Vec<usize> = bm25.keys().copied().collect();
        lexical.sort_by(|a, b| bm25[b].partial_cmp(&bm25[a]).unwrap());
        lexical.truncate(pool);
        // Also keep the notes that share the most query weight: BM25 favours short notes,
        // and the coverage score needs the long ones too.
        let taken: HashSet<usize> = lexical.iter().copied().collect();
        let mut by_weight: Vec<usize> = present.keys().copied().collect();
        by_weight.sort_by(|a, b| present[b].partial_cmp(&present[a]).unwrap());
        lexical.extend(by_weight.into_iter().take(pool * 4).filter(|d| !taken.contains(d)));

        let mut cos: Option<Vec<f32>> = None;
        let mut dense: Vec<usize> = vec![];
        if let (Some(vectors), Some(embedder)) = (&self.vectors, &self.embedder) {
            if !self.notes.is_empty() {
                let mut q = embedder.embed_query(text)?;
                normalise(&mut q);
                let sims: Vec<f32> = vectors.iter().map(|v| v.iter().zip(&q).map(|(a, b)| a * b).sum()).collect();
                let mut order: Vec<usize> = (0..sims.len()).collect();
                order.sort_by(|a, b| sims[*b].partial_cmp(&sims[*a]).unwrap_or(std::cmp::Ordering::Equal));
                order.truncate(pool);
                dense = order;
                cos = Some(sims);
            }
        }
        let mut fused: IndexMap<usize, f64> = IndexMap::new();
        for ranks in [&lexical, &dense] {
            for (rank, &doc) in ranks.iter().enumerate() {
                *fused.entry(doc).or_default() += 1.0 / (RRF_K + rank as f64);
            }
        }
        let mut hits: Vec<Hit> = fused
            .iter()
            .map(|(&doc, &rrf)| {
                let overlap = if q_idf != 0.0 { present.get(&doc).copied().unwrap_or(0.0) / q_idf } else { 0.0 };
                let (front, mass) = &self.fronts[doc];
                let reverse = if *mass != 0.0 {
                    front.iter().filter(|t| terms.contains_key(*t)).map(|t| self.weight(t)).sum::<f64>() / mass
                } else {
                    0.0
                };
                let mut lex = (overlap * reverse).sqrt();
                let shared = front.iter().filter(|t| terms.contains_key(*t) && !t.bytes().all(|b| b.is_ascii_digit())).count();
                if shared < 2 {
                    lex /= 2.0; // one shared word (or only numbers) is a coincidence, not the same fact
                }
                let cosine = cos.as_ref().map(|c| (c[doc] as f64).max(0.0));
                let score = match cosine {
                    None => lex,
                    Some(c) => lex.max(0.3 * lex + 0.7 * c),
                };
                Hit {
                    doc,
                    score: round_to(score, 4),
                    overlap: round_to(overlap, 4),
                    reverse: round_to(reverse, 4),
                    lexical: round_to(lex, 4),
                    cosine: cosine.map(|c| round_to(c, 4)),
                    bm25: round_to(bm25.get(&doc).copied().unwrap_or(0.0), 4),
                    rrf,
                }
            })
            .collect();
        // Stable sort, descending by (score, rrf), like Python's sort(reverse=True).
        hits.sort_by(|a, b| {
            (b.score, b.rrf).partial_cmp(&(a.score, a.rrf)).unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Notes sharing the most specific topic tag of `doc` (concept mapping).
    pub fn neighbours(&self, doc: usize, max_group: usize) -> (Option<String>, Vec<usize>) {
        let mut tags = concept_tags(&self.notes[doc].tags);
        tags.sort_by_key(|t| std::cmp::Reverse(t.matches("::").count()));
        for tag in tags {
            if let Some(group) = self.by_tag.get(&tag) {
                if group.len() > 1 && group.len() <= max_group {
                    return (Some(tag), group.iter().copied().filter(|&d| d != doc).collect());
                }
            }
        }
        (None, vec![])
    }
}
