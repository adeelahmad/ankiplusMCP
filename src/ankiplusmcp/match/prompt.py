"""Build the drafting prompt from a match report: only the gaps go to the model."""

HEADER = """# Draft cards for: {source}

Existing deck coverage was checked without AI. {covered} of {chunks} source chunks are
already covered by existing cards ({cards} suspended cards proposed for unsuspension),
{related} are partly covered, and {gaps} have no existing card.

## Instructions
- Draft cards only for the facts under "Gaps" and the missing parts of "Partly covered".
- Do not duplicate the existing cards quoted below; they will be unsuspended instead.
- Group closely related facts into one cloze note (AnKing Step deck style); do not make
  one card per sentence.
- Cite the page in `source_ref` for every card. Use the AnkiPlus tools for figures and
  image occlusion where a page needs an image.
"""


def build_prompt(report, source='lecture', max_chars=24000):
    s = report['summary']
    parts = [HEADER.format(source=source, chunks=s['chunks'], covered=s['covered'], related=s['related'],
                           gaps=s['gap'], cards=s['cards_to_unsuspend'])]
    gaps = [r for r in report['chunks'] if r['status'] == 'gap']
    related = [r for r in report['chunks'] if r['status'] == 'related']
    covered = [r for r in report['chunks'] if r['status'] == 'covered']
    clashing = [r for r in report['chunks'] if r['status'] == 'conflict']
    if clashing:
        lines = []
        for r in clashing:
            lines.append(f'- p{r["page"]}: {r["text"]}')
            for c in r.get('conflicts', [])[:2]:
                said = '; '.join(f'lecture {q["lecture"]} vs card {q["card"]} {q["unit"]}' for q in c['quantities'])
                lines.append(f'  - existing {c["note_id"]} says otherwise ({said}): {c["preview"]}')
        parts.append('## Conflicts (the deck states a different value)\nDraft a card with the lecture\'s value, and name the '
                     'difference in Extra. Do not unsuspend the conflicting card.\n' + '\n'.join(lines))
    if gaps:
        parts.append('## Gaps (no existing card)\n' + '\n'.join(f'- p{r["page"]}: {r["text"]}' for r in gaps))
    if related:
        lines = []
        for r in related:
            lines.append(f'- p{r["page"]}: {r["text"]}')
            for m in r['matches'][:2]:
                lines.append(f'  - existing {m["note_id"]} ({m["score"]:.2f}): {m["preview"]}')
        parts.append('## Partly covered (draft only what the existing card misses)\n' + '\n'.join(lines))
    if covered:
        parts.append('## Already covered (do not draft)\n' + '\n'.join(
            f'- p{r["page"]}: {r["text"][:120]} -> note {r["matches"][0]["note_id"]}' for r in covered if r['matches']))
    text = '\n\n'.join(parts) + '\n'
    if len(text) > max_chars:
        text = text[:max_chars].rsplit('\n', 1)[0] + '\n\n[truncated: read match/report.json for the rest]\n'
    return text
