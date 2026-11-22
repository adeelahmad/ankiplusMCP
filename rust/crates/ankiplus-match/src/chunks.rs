//! Split lecture content into fact-sized chunks for matching.
//!
//! Bullets and lines become chunks; lines too short to carry a fact are merged
//! into their neighbour (same rules as the Python `chunks` module).

use crate::text::{splitlines, take_chars, tokenize};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub const MIN_TOKENS: usize = 4;
pub const MAX_CHARS: usize = 400;
static BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*•▪●–—>o]|\d{1,2}[.)])\s+").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chunk {
    pub id: String,
    pub page: i64,
    #[serde(default)]
    pub title: String,
    pub text: String,
}

/// One page or slide of extracted text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slide {
    pub n: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Highlight {
    #[serde(default)]
    pub page: Option<i64>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub note: Option<String>,
}

pub(crate) fn lines(text: &str) -> Vec<String> {
    splitlines(text)
        .into_iter()
        .map(|raw| BULLET.replace(raw, "").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn len_chars(s: &str) -> usize {
    s.chars().count()
}

fn split(page: i64, title: &str, text: &str) -> Vec<Chunk> {
    let mut pieces: Vec<String> = Vec::new();
    for line in lines(text) {
        if !title.is_empty() && line.trim() == title.trim() {
            continue;
        }
        let merge = match pieces.last() {
            Some(last) => {
                (tokenize(last).len() < MIN_TOKENS || tokenize(&line).len() < MIN_TOKENS)
                    && len_chars(last) + len_chars(&line) < MAX_CHARS
            }
            None => false,
        };
        if merge {
            let last = pieces.last_mut().unwrap();
            last.push(' ');
            last.push_str(&line);
        } else {
            pieces.push(line);
        }
    }
    let mut out: Vec<Chunk> = Vec::new();
    for piece in pieces {
        if tokenize(&piece).len() >= 2 {
            out.push(Chunk {
                id: format!("p{page}-{}", out.len() + 1),
                page,
                title: title.to_string(),
                text: take_chars(&piece, MAX_CHARS * 2),
            });
        }
    }
    out
}

/// From extracted slides (`slides.json` `slides` list: n, title, text).
pub fn chunks_from_slides(slides: &[Slide]) -> Vec<Chunk> {
    slides.iter().flat_map(|s| split(s.n, &s.title, &s.text)).collect()
}

/// From per-page text (page 1 first); the first line of a page is its title.
pub fn chunks_from_pages(pages: &[(i64, String)]) -> Vec<Chunk> {
    let slides: Vec<Slide> = pages
        .iter()
        .map(|(n, text)| Slide {
            n: *n,
            title: take_chars(lines(text).first().map(String::as_str).unwrap_or(""), 100),
            text: text.clone(),
        })
        .collect();
    chunks_from_slides(&slides)
}

/// Plain text or Markdown: each '#' heading starts a new section (used as the page number).
pub fn chunks_from_text(text: &str) -> Vec<Chunk> {
    let mut slides: Vec<Slide> = Vec::new();
    let mut title = String::new();
    let mut body: Vec<&str> = Vec::new();
    for line in splitlines(text) {
        if line.trim_start().starts_with('#') {
            if !body.is_empty() {
                slides.push(Slide { n: slides.len() as i64 + 1, title: title.clone(), text: body.join("\n") });
            }
            title = line.trim_start_matches(['#', ' ']).trim().to_string();
            body.clear();
        } else {
            body.push(line);
        }
    }
    if !body.is_empty() {
        slides.push(Slide { n: slides.len() as i64 + 1, title, text: body.join("\n") });
    }
    chunks_from_slides(&slides)
}

/// User highlights, e.g. from a PDF viewer. Each highlight is kept whole:
/// the user already chose what matters.
pub fn chunks_from_highlights(highlights: &[Highlight]) -> Vec<Chunk> {
    let mut out = Vec::new();
    for (i, item) in highlights.iter().enumerate() {
        let text = lines(&item.text).join(" ");
        if text.is_empty() {
            continue;
        }
        out.push(Chunk {
            id: format!("h{}", i + 1),
            page: item.page.unwrap_or(0),
            title: item.note.clone().unwrap_or_default(),
            text,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_short_lines_and_skips_title() {
        let chunks = chunks_from_text("# Heart\nHeart\n- Troponin rises within four hours after infarction\n- short one\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "p1-1");
        assert_eq!(chunks[0].text, "Troponin rises within four hours after infarction short one");
    }
}
