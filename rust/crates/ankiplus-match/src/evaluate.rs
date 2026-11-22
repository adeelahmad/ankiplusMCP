//! Measure matching quality against hand labels, to calibrate thresholds and embeddings.
//!
//! Labels: `[{"text": "<lecture fact>", "note_ids": [<expected notes>]}]`; an empty
//! `note_ids` means the fact has no existing card (a true gap).

use crate::index::DeckIndex;
use crate::text::{round_to, take_chars};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub text: String,
    #[serde(default)]
    pub note_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Miss {
    pub text: String,
    pub missing: Vec<i64>,
    pub top: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evaluation {
    pub recall_at_k: Option<f64>,
    pub k: usize,
    pub gap_precision: Option<f64>,
    pub covered_facts_marked_gap: usize,
    pub labels: usize,
    pub embeddings: bool,
    pub misses: Vec<Miss>,
}

/// `covered: None` uses the calibrated default for the index.
pub fn evaluate(index: &DeckIndex, labels: &[Label], k: usize, covered: Option<f64>) -> Result<Evaluation, crate::Error> {
    let covered = crate::Thresholds { covered, ..Default::default() }.covered_or_default(index.has_embeddings());
    let (mut found, mut expected, mut true_gaps, mut flagged_gaps, mut false_gaps) = (0, 0, 0, 0, 0);
    let mut misses = Vec::new();
    for label in labels {
        let hits = index.search(&label.text, k, 50)?;
        let ids: Vec<i64> = hits.iter().map(|h| index.notes[h.doc].note_id).collect();
        let want: HashSet<i64> = label.note_ids.iter().copied().collect();
        let best = hits.first().map_or(0.0, |h| h.score);
        if want.is_empty() {
            true_gaps += 1;
            flagged_gaps += usize::from(best < covered);
            continue;
        }
        let got: HashSet<i64> = ids.iter().copied().filter(|i| want.contains(i)).collect();
        found += got.len();
        expected += want.len();
        false_gaps += usize::from(best < covered);
        if got.len() < want.len() {
            let mut missing: Vec<i64> = want.difference(&got).copied().collect();
            missing.sort_unstable();
            misses.push(Miss { text: take_chars(&label.text, 120), missing, top: ids.iter().take(3).copied().collect() });
        }
    }
    misses.truncate(20);
    Ok(Evaluation {
        recall_at_k: (expected > 0).then(|| round_to(found as f64 / expected as f64, 4)),
        k,
        gap_precision: (true_gaps > 0).then(|| round_to(flagged_gaps as f64 / true_gaps as f64, 4)),
        covered_facts_marked_gap: false_gaps,
        labels: labels.len(),
        embeddings: index.has_embeddings(),
        misses,
    })
}
