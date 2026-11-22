//! Hybrid deck index: BM25 over note text and tag names, plus optional dense embeddings.
//!
//! Scores per hit (identical to the Python `DeckIndex.search`):
//! - `bm25`: raw Okapi BM25 (ranking only, not comparable across queries),
//! - `overlap`: IDF-weighted share of the query's terms the note contains (0..1),
//! - `cosine`: embedding similarity (0..1) when embeddings exist, else `None`,
//! - `score`: `overlap`, raised to the mean of `overlap` and `cosine` when that is higher.
//!
//! Candidates from both retrievers are merged by reciprocal rank fusion (k = 60).

use crate::deck::Note;
use crate::text::{concept_tags, round_to, tag_words, tokenize};
use indexmap::IndexMap;
use serde_json::{Map, Value};
use std::collections::HashMap;

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
    pub cosine: Option<f64>,
    pub bm25: f64,
    pub rrf: f64,
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
    pub fn embed_all(&mut self, batch: usize, mut progress: impl FnMut(usize, usize)) -> Result<(), crate::Error> {
        let Some(embedder) = &self.embedder else { return Ok(()) };
        let mut out = Vec::with_capacity(self.notes.len());
        let total = self.notes.len();
        for start in (0..total).step_by(batch.max(1)) {
            let texts: Vec<String> = self.notes[start..(start + batch).min(total)].iter().map(|n| n.text.clone()).collect();
            for mut v in embedder.embed_documents(&texts)? {
                normalise(&mut v);
                out.push(v);
            }
            progress((start + batch).min(total), total);
        }
        self.meta.insert("embedding_model".into(), Value::String(embedder.name()));
        self.vectors = Some(out);
        Ok(())
    }

    pub fn search(&self, text: &str, k: usize, pool: usize) -> Result<Vec<Hit>, crate::Error> {
        let mut terms: IndexMap<String, usize> = IndexMap::new();
        for t in tokenize(text) {
            *terms.entry(t).or_default() += 1;
        }
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let n = self.notes.len() as f64;
        // Terms the deck never uses still count: a fact about them is not covered.
        let unseen = (1.0 + (n + 0.5) / 0.5).ln();
        let q_idf: f64 = terms.keys().map(|t| self.idf.get(t).copied().unwrap_or(unseen)).sum();
        let mut bm25: IndexMap<usize, f64> = IndexMap::new();
        let mut present: HashMap<usize, f64> = HashMap::new();
        for term in terms.keys() {
            let Some(&idf) = self.idf.get(term) else { continue };
            for &(doc, tf) in &self.postings[term] {
                let tf = tf as f64;
                let norm = tf * (self.k1 + 1.0)
                    / (tf + self.k1 * (1.0 - self.b + self.b * self.lengths[doc] as f64 / self.avgdl));
                *bm25.entry(doc).or_default() += idf * norm;
                *present.entry(doc).or_default() += idf;
            }
        }
        let mut lexical: Vec<usize> = bm25.keys().copied().collect();
        lexical.sort_by(|a, b| bm25[b].partial_cmp(&bm25[a]).unwrap());
        lexical.truncate(pool);

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
                let cosine = cos.as_ref().map(|c| (c[doc] as f64).max(0.0));
                let score = match cosine {
                    None => overlap,
                    Some(c) => overlap.max((overlap + c) / 2.0),
                };
                Hit {
                    doc,
                    score: round_to(score, 4),
                    overlap: round_to(overlap, 4),
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
