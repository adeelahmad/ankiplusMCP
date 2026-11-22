//! Build the drafting prompt from a match report: only the gaps go to the model.

use crate::matcher::Report;
use crate::text::take_chars;

const HEADER: &str = "# Draft cards for: {source}

Existing deck coverage was checked without AI. {covered} of {chunks} source chunks are
already covered by existing cards ({cards} suspended cards proposed for unsuspension),
{related} are partly covered, and {gaps} have no existing card.

## Instructions
- Draft cards only for the facts under \"Gaps\" and the missing parts of \"Partly covered\".
- Do not duplicate the existing cards quoted below; they will be unsuspended instead.
- Group closely related facts into one cloze note (AnKing Step deck style); do not make
  one card per sentence.
- Cite the page in `source_ref` for every card. Use the AnkiPlus tools for figures and
  image occlusion where a page needs an image.
";

pub fn build_prompt(report: &Report, source: &str, max_chars: usize) -> String {
    let s = &report.summary;
    let header = HEADER
        .replace("{source}", source)
        .replace("{chunks}", &s.chunks.to_string())
        .replace("{covered}", &s.covered.to_string())
        .replace("{related}", &s.related.to_string())
        .replace("{gaps}", &s.gap.to_string())
        .replace("{cards}", &s.cards_to_unsuspend.to_string());
    let mut parts = vec![header];
    let of = |status: &str| report.chunks.iter().filter(|r| r.status == status).collect::<Vec<_>>();
    let (gaps, related, covered) = (of("gap"), of("related"), of("covered"));
    if !gaps.is_empty() {
        let lines: Vec<String> = gaps.iter().map(|r| format!("- p{}: {}", r.page, r.text)).collect();
        parts.push(format!("## Gaps (no existing card)\n{}", lines.join("\n")));
    }
    if !related.is_empty() {
        let mut lines = Vec::new();
        for r in &related {
            lines.push(format!("- p{}: {}", r.page, r.text));
            for m in r.matches.iter().take(2) {
                lines.push(format!("  - existing {} ({:.2}): {}", m.note_id, m.score.unwrap_or(0.0), m.preview));
            }
        }
        parts.push(format!("## Partly covered (draft only what the existing card misses)\n{}", lines.join("\n")));
    }
    if !covered.is_empty() {
        let lines: Vec<String> = covered
            .iter()
            .filter_map(|r| {
                r.matches.first().map(|m| format!("- p{}: {} -> note {}", r.page, take_chars(&r.text, 120), m.note_id))
            })
            .collect();
        parts.push(format!("## Already covered (do not draft)\n{}", lines.join("\n")));
    }
    let text = parts.join("\n\n") + "\n";
    if text.chars().count() > max_chars {
        let cut = take_chars(&text, max_chars);
        let head = cut.rsplit_once('\n').map_or(cut.as_str(), |(h, _)| h);
        return format!("{head}\n\n[truncated: read match/report.json for the rest]\n");
    }
    text
}
