//! Text helpers shared by every stage: HTML cleaning, tokenizing, tag handling.
//!
//! Each function mirrors its Python counterpart in `ankiplusmcp.match` so that
//! both implementations tokenize and score identically.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

pub(crate) static CLOZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{\{c\d+::(.*?)(?:::[^}]*)?\}\}").unwrap());
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());
static BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<\s*(br|/div|/p|/li|/tr|/h\d)\b[^>]*>").unwrap());
// Python uses one pattern with a backreference; the regex crate has none, so
// script and style are dropped by two patterns with the same effect.
static DROP_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<\s*script\b.*?</\s*script\s*>").unwrap());
static DROP_STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<\s*style\b.*?</\s*style\s*>").unwrap());
static DROP_SOUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[sound:[^\]]*\]").unwrap());
static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new("[ \t\u{a0}]+").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z0-9]+").unwrap());

pub static STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "a an and are as at be been but by can could did do does for from had has have \
     how if in into is it its may more most no not of on or such than that the their them then there these \
     they this those to was were what when where which while who whom why will with would also other only \
     very via per each both all any some ie eg vs use used using called known"
        .split_whitespace()
        .collect()
});

/// Python `str.splitlines()`: splits on every line boundary Python knows, drops the ends.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let boundary = matches!(
            c,
            '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}'
        );
        if boundary {
            out.push(&text[start..i]);
            let mut end = i + c.len_utf8();
            if c == '\r' {
                if let Some(&(j, '\n')) = chars.peek() {
                    chars.next();
                    end = j + 1;
                }
            }
            start = end;
        }
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// First `n` characters (Python `s[:n]`).
pub fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// HTML field -> plain text; cloze markers keep their answer text.
pub fn clean_text(value: &str) -> String {
    let v = DROP_SCRIPT.replace_all(value, " ");
    let v = DROP_STYLE.replace_all(&v, " ");
    let v = DROP_SOUND.replace_all(&v, " ");
    let v = CLOZE.replace_all(&v, "$1");
    let v = BREAK.replace_all(&v, "\n");
    let v = TAG.replace_all(&v, " ");
    let v = htmlize::unescape(v.as_ref());
    splitlines(&v)
        .into_iter()
        .map(|line| SPACE.replace_all(line, " ").trim().to_string())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn stem(word: &str) -> String {
    let n = word.chars().count();
    if n > 4 && word.ends_with("ies") {
        return format!("{}y", &word[..word.len() - 3]);
    }
    if n > 3 && word.ends_with('s') && !(word.ends_with("ss") || word.ends_with("us") || word.ends_with("is")) {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

pub fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    WORD.find_iter(&lower)
        .map(|m| m.as_str())
        .filter(|w| !(STOPWORDS.contains(w) || (w.len() < 2 && !w.chars().all(|c| c.is_ascii_digit()))))
        .map(stem)
        .collect()
}

/// Topic tags only: drops question-bank IDs (numeric leaves) and media/meta tags.
pub fn concept_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter(|tag| {
            let parts: Vec<&str> = tag.split("::").filter(|p| !p.is_empty()).collect();
            match parts.last() {
                None => false,
                Some(last) => {
                    let leaf = last.trim_start_matches('#');
                    let numeric = !leaf.is_empty() && leaf.chars().all(|c| c.is_numeric());
                    !numeric && !parts.iter().any(|p| p.starts_with('!') || p.starts_with('$'))
                }
            }
        })
        .cloned()
        .collect()
}

pub fn tag_words(tags: &[String]) -> Vec<String> {
    let mut words = Vec::new();
    for tag in concept_tags(tags) {
        let parts: Vec<&str> = tag.split("::").collect();
        let tail = parts[parts.len().saturating_sub(2)..].join(" ");
        words.extend(tokenize(&tail.replace('_', " ").replace('-', " ")));
    }
    words
}

/// Python `round(x, digits)`: round half to even on the exact binary value.
pub fn round_to(x: f64, digits: usize) -> f64 {
    format!("{x:.digits$}").parse().unwrap_or(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_cloze_and_html() {
        let t = clean_text("Erb palsy injures the {{c1::upper trunk::hint}}<br><b>waiter&nbsp;tip</b> &amp; more[sound:x.mp3]");
        assert_eq!(t, "Erb palsy injures the upper trunk\nwaiter tip & more");
    }

    #[test]
    fn tokenizes_like_python() {
        assert_eq!(tokenize("The arteries and bodies of 3 Sodiums, a b"), vec!["artery", "body", "3", "sodium"]);
        assert_eq!(stem("glass"), "glass");
        assert_eq!(stem("virus"), "virus");
    }

    #[test]
    fn concept_tags_drop_ids_and_meta() {
        let tags = vec![
            "#AK::#B&B::Anatomy::Brachial_Plexus".to_string(),
            "#AK::#UWorld::Step::1234".to_string(),
            "#AK::!FreeMedia::x".to_string(),
        ];
        assert_eq!(concept_tags(&tags), vec![tags[0].clone()]);
        assert_eq!(tag_words(&tags), vec!["anatomy", "brachial", "plexus"]);
    }

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines("a\r\nb\rc\n\nd"), vec!["a", "b", "c", "", "d"]);
        assert_eq!(splitlines("a\n"), vec!["a"]);
    }

    #[test]
    fn rounds_half_even() {
        assert_eq!(round_to(0.125, 2), 0.12);
        assert_eq!(round_to(0.123456, 4), 0.1235);
    }
}

/// A float as Python's `repr` prints it (`3.0`, `2.5`, `1e+16`, `1.5e-05`).
pub fn py_float(v: f64) -> String {
    let a = v.abs();
    if !v.is_finite() {
        return if v.is_nan() { "nan".into() } else if v > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if a == 0.0 || (1e-4..1e16).contains(&a) {
        return format!("{v:?}");
    }
    let s = format!("{v:e}");
    let (mantissa, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let (sign, digits) = exp.strip_prefix('-').map_or(("+", exp), |d| ("-", d));
    format!("{mantissa}e{sign}{digits:0>2}")
}
