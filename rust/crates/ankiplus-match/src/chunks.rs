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
// '¢', '©', 'e' and '«' are how OCR commonly reads bullet glyphs. The lookarounds
// need fancy-regex; everything else uses the regex crate.
static BULLET: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"^\s*(?:[-*•▪●–—>o¢©«‣◦]|e(?=\s+[A-Z])|\d{1,2}[.)])\s+").unwrap()
});
static INLINE_BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+[•¢▪●]\s+").unwrap());
static CAPTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:Figure|Fig\.|Image|Source)\s*\d*\s*[:.]").unwrap());
static LIST_NUMBER: LazyLock<fancy_regex::Regex> =
    LazyLock::new(|| fancy_regex::Regex::new(r"(?<=[A-Za-z:)])\s+\d{1,2}$").unwrap());
static CONTINUES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,(\-–/]$|\b(?:and|or|of|the|a|an|to|in|with|by|for|from|as|at|on|is|are)$").unwrap()
});
static TWO_LETTERS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z]{2}").unwrap());
static WORD4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z]{4,}").unwrap());
static VOWEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[aeiouyAEIOUY]").unwrap());
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

/// Logical lines: bullets stripped, wrapped sentences re-joined, trailing list numbers,
/// page numbers and figure credits dropped (Python `_lines`).
pub(crate) fn lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for raw in splitlines(text) {
        for part in INLINE_BULLET.split(raw) {
            let bulleted = BULLET.is_match(part).unwrap_or(false);
            let stripped = BULLET.replace(part, "");
            let line = LIST_NUMBER.replace(stripped.trim(), "").to_string();
            if !TWO_LETTERS.is_match(&line) || CAPTION.is_match(&line) {
                continue;
            }
            if let Some(cur) = current.as_mut() {
                let first = line.chars().next().unwrap();
                if !bulleted
                    && (CONTINUES.is_match(cur)
                        || first.is_lowercase()
                        || first == '('
                        || first == '['
                        || cur.matches('(').count() > cur.matches(')').count())
                {
                    cur.push(' ');
                    cur.push_str(&line);
                    continue;
                }
            }
            if let Some(cur) = current.take() {
                out.push(cur);
            }
            current = Some(line);
        }
    }
    out.extend(current);
    out
}

const OK_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 \n\t.,;:()[]-–—/'\"?!%+&·•→<>=°±×*#@$";

/// True when a PDF text layer is unusable: empty, or a broken font encoding
/// (e.g. shifted glyph codes that turn 'The' into '7KH').
pub fn looks_garbled(text: &str) -> bool {
    let text = text.trim();
    let n = text.chars().count();
    if n < 20 {
        return true;
    }
    let printable = text.chars().filter(|c| OK_CHARS.contains(*c)).count() as f64 / n as f64;
    let words: Vec<&str> = WORD4.find_iter(text).map(|m| m.as_str()).collect();
    if words.is_empty() {
        return printable < 0.9;
    }
    let vowelled = words.iter().filter(|w| VOWEL.is_match(w)).count() as f64 / words.len() as f64;
    printable < 0.9 || vowelled < 0.7
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
/// Lines repeated on many pages (headers, footers, college names) are dropped first.
pub fn chunks_from_slides(slides: &[Slide]) -> Vec<Chunk> {
    let per_slide: Vec<Vec<String>> = slides.iter().map(|s| lines(&s.text)).collect();
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for ls in &per_slide {
        let unique: std::collections::HashSet<&str> = ls.iter().map(String::as_str).collect();
        for l in unique {
            *seen.entry(l).or_default() += 1;
        }
    }
    let limit = (slides.len() as f64 * 0.3).max(3.0);
    let mut out = Vec::new();
    for (slide, ls) in slides.iter().zip(&per_slide) {
        let text = ls.iter().filter(|l| (seen[l.as_str()] as f64) < limit).cloned().collect::<Vec<_>>().join("\n");
        out.extend(split(slide.n, &slide.title, &text));
    }
    out
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
    fn garbled_text_and_line_joining() {
        let shifted: String = "The digestive system is responsible for breaking down food into absorbable nutrients"
            .chars()
            .map(|c| char::from_u32(c as u32 - 29).unwrap())
            .collect();
        assert!(looks_garbled(&shifted));
        assert!(!looks_garbled("The digestive system is responsible for breaking down food."));
        let bodies = [
            "\u{2022} Tongue becomes red, smooth, and sore (seen in vitamin\nB12, folate, and iron deficiency)\nFigure 3: Netter atlas",
            "\u{a2} Sulcus terminalis divides the oral part\nfrom the pharyngeal part",
            "Foramen cecum marks the thyroid descent site",
        ];
        let slides: Vec<Slide> = bodies
            .iter()
            .enumerate()
            .map(|(i, b)| Slide { n: i as i64 + 1, title: String::new(), text: format!("BMC Medical College\n{b}\n{}", i + 1) })
            .collect();
        let texts: Vec<String> = chunks_from_slides(&slides).into_iter().map(|c| c.text).collect();
        assert_eq!(texts, vec![
            "Tongue becomes red, smooth, and sore (seen in vitamin B12, folate, and iron deficiency)",
            "Sulcus terminalis divides the oral part from the pharyngeal part",
            "Foramen cecum marks the thyroid descent site",
        ]);
    }

    #[test]
    fn merges_short_lines_and_skips_title() {
        let chunks = chunks_from_text("# Heart\nHeart\n- Troponin rises within four hours after infarction\n- short one\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "p1-1");
        assert_eq!(chunks[0].text, "Troponin rises within four hours after infarction short one");
    }
}
