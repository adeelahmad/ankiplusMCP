//! Check that drafted cards are supported by the source (citation-style verification).
//!
//! For each card: find the source page that best contains its text, then check every
//! cloze answer appears (fuzzily) in the source. Deterministic; no model calls. A low
//! score means "look at this card", not "this card is wrong".

use crate::chunks::Chunk;
use crate::text::{clean_text, round_to, tokenize, CLOZE};
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+").unwrap());
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{c(\d+)::").unwrap());
/// This many items behind one cloze number is a list card: one number per item instead.
pub const LIST_LIMIT: usize = 3;

/// Shape problems a mechanical check can see: a list hidden under one cloze number, and
/// text whose brackets don't pair up (a sign the sentence was cut while drafting).
pub fn form_problems(raw: &str) -> Vec<String> {
    let mut problems = Vec::new();
    // Python sorts the numbers as strings ("10" before "2").
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for c in NUMBERED.captures_iter(raw) {
        *counts.entry(c[1].to_string()).or_default() += 1;
    }
    for (number, n) in counts {
        if n >= LIST_LIMIT {
            problems.push(format!("c{number} hides {n} items at once; give each item its own cloze number"));
        }
    }
    let text = clean_text(raw);
    for (left, right) in [('(', ')'), ('[', ']')] {
        if text.matches(left).count() != text.matches(right).count() {
            problems.push(format!("unpaired {left}{right}: the sentence looks cut; check it against the source"));
        }
    }
    problems
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftCard {
    pub id: Value,
    #[serde(default)]
    pub fields: IndexMap<String, Value>,
    #[serde(default)]
    pub source_ref: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardSupport {
    pub card_id: Value,
    pub best_page: Option<i64>,
    pub term_support: f64,
    pub cited_pages: Vec<i64>,
    pub answers_not_in_source: Vec<String>,
    #[serde(default)]
    pub form_problems: Vec<String>,
    pub flag: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SupportReport {
    pub cards: Vec<CardSupport>,
    pub flagged: usize,
    pub checked: usize,
    pub note: String,
}

/// `difflib.SequenceMatcher(None, a, b).ratio()`, including its autojunk heuristic.
pub fn sequence_ratio(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let total = a.len() + b.len();
    if total == 0 {
        return 1.0;
    }
    let mut b2j: HashMap<char, Vec<usize>> = HashMap::new();
    for (j, &c) in b.iter().enumerate() {
        b2j.entry(c).or_default().push(j);
    }
    let n = b.len();
    if n >= 200 {
        let ntest = n / 100 + 1;
        b2j.retain(|_, idx| idx.len() <= ntest);
    }
    let longest = |alo: usize, ahi: usize, blo: usize, bhi: usize| -> (usize, usize, usize) {
        let (mut besti, mut bestj, mut bestsize) = (alo, blo, 0usize);
        let mut j2len: HashMap<usize, usize> = HashMap::new();
        for i in alo..ahi {
            let mut next: HashMap<usize, usize> = HashMap::new();
            if let Some(js) = b2j.get(&a[i]) {
                for &j in js {
                    if j < blo {
                        continue;
                    }
                    if j >= bhi {
                        break;
                    }
                    let k = j.checked_sub(1).and_then(|p| j2len.get(&p)).copied().unwrap_or(0) + 1;
                    next.insert(j, k);
                    if k > bestsize {
                        besti = i + 1 - k;
                        bestj = j + 1 - k;
                        bestsize = k;
                    }
                }
            }
            j2len = next;
        }
        while besti > alo && bestj > blo && a[besti - 1] == b[bestj - 1] {
            besti -= 1;
            bestj -= 1;
            bestsize += 1;
        }
        while besti + bestsize < ahi && bestj + bestsize < bhi && a[besti + bestsize] == b[bestj + bestsize] {
            bestsize += 1;
        }
        (besti, bestj, bestsize)
    };
    let mut matched = 0usize;
    let mut queue = vec![(0, a.len(), 0, b.len())];
    while let Some((alo, ahi, blo, bhi)) = queue.pop() {
        let (i, j, k) = longest(alo, ahi, blo, bhi);
        if k > 0 {
            matched += k;
            if alo < i && blo < j {
                queue.push((alo, i, blo, j));
            }
            if i + k < ahi && j + k < bhi {
                queue.push((i + k, ahi, j + k, bhi));
            }
        }
    }
    2.0 * matched as f64 / total as f64
}

/// Best similarity of `needle` against any window of the same number of words (0..1).
/// Word-bounded, so "thenar" does not match inside "hypothenar".
pub fn fuzzy_in(needle: &str, haystack: &str) -> f64 {
    let nl = needle.to_lowercase();
    let hl = haystack.to_lowercase();
    let words: Vec<&str> = WORDS.find_iter(&nl).map(|m| m.as_str()).collect();
    let hay: Vec<&str> = WORDS.find_iter(&hl).map(|m| m.as_str()).collect();
    if words.is_empty() {
        return 1.0;
    }
    let n = words.len();
    let target = words.join(" ");
    let mut best: f64 = 0.0;
    for start in 0..(hay.len() + 1).saturating_sub(n).max(1) {
        let window = hay[start.min(hay.len())..(start + n).min(hay.len())].join(" ");
        if window == target {
            return 1.0;
        }
        best = best.max(sequence_ratio(&target, &window));
    }
    best
}

fn page_of(v: &Value) -> Option<i64> {
    v.as_i64()
}

pub fn check_support(cards: &[DraftCard], chunks: &[Chunk], answer_threshold: f64) -> SupportReport {
    let mut pages: IndexMap<i64, Vec<&str>> = IndexMap::new();
    for c in chunks {
        pages.entry(c.page).or_default().push(&c.text);
    }
    let page_text: IndexMap<i64, String> = pages.into_iter().map(|(p, t)| (p, t.join("\n"))).collect();
    let page_terms: HashMap<i64, HashSet<String>> =
        page_text.iter().map(|(p, t)| (*p, tokenize(t).into_iter().collect())).collect();
    let mut out = Vec::new();
    for card in cards {
        let raw = card.fields.values().filter_map(Value::as_str).collect::<Vec<_>>().join("\n");
        let answers: Vec<String> = CLOZE.captures_iter(&raw).map(|c| clean_text(&c[1])).collect();
        let terms: HashSet<String> = tokenize(&clean_text(&raw)).into_iter().collect();
        let refs: Vec<i64> = card.source_ref.iter().filter_map(page_of).filter(|p| page_text.contains_key(p)).collect();
        let candidates: Vec<i64> = if refs.is_empty() { page_text.keys().copied().collect() } else { refs.clone() };
        let (mut best_page, mut best) = (None, 0.0);
        for p in candidates {
            let score = if terms.is_empty() {
                0.0
            } else {
                terms.intersection(&page_terms[&p]).count() as f64 / terms.len() as f64
            };
            if score > best {
                best_page = Some(p);
                best = score;
            }
        }
        let haystack = if refs.is_empty() {
            page_text.values().cloned().collect::<Vec<_>>().join("\n")
        } else {
            best_page.and_then(|p| page_text.get(&p).cloned()).unwrap_or_default()
        };
        let missing: Vec<String> = answers
            .into_iter()
            .filter(|a| !a.is_empty() && fuzzy_in(a, &haystack) < answer_threshold)
            .collect();
        // The cloze text field.
        let problems = form_problems(card.fields.values().next().and_then(Value::as_str).unwrap_or(""));
        out.push(CardSupport {
            card_id: card.id.clone(),
            best_page,
            term_support: round_to(best, 3),
            cited_pages: refs,
            flag: !missing.is_empty() || best < 0.5 || !problems.is_empty(),
            answers_not_in_source: missing.into_iter().take(10).collect(),
            form_problems: problems,
        });
    }
    SupportReport {
        flagged: out.iter().filter(|r| r.flag).count(),
        checked: out.len(),
        cards: out,
        note: "Mechanical check: flagged cards need a human or model look; unflagged cards are not proven correct.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_matches_difflib() {
        assert_eq!(sequence_ratio("abcd", "bcde"), 0.75);
        assert_eq!(sequence_ratio("", ""), 1.0);
        assert!((sequence_ratio("lower trunk", "lower trunks") - 0.9565217391304348).abs() < 1e-12);
    }

    #[test]
    fn fuzzy_is_word_bounded() {
        assert!(fuzzy_in("thenar", "the hypothenar eminence") < 0.8);
        assert_eq!(fuzzy_in("lower trunk", "compresses the lower trunk of"), 1.0);
    }
}
