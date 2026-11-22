"""Classify each chunk as covered / related / gap and collect cards to unsuspend."""
from dataclasses import dataclass, asdict, replace
from .index import tokenize


@dataclass(frozen=True)
class Thresholds:
    """``None`` picks the calibrated default for the index (with or without embeddings).

    BM25 only: covered 0.5, calibrated on a real AnKing export (33,078 notes) against 126
    hand-labelled chunks from two real lectures (precision 0.75 as a lower bound, recall 0.35;
    see docs/matching.md). With Nomic embeddings: covered 0.55, from the synthetic benchmark
    (scripts/match_benchmark.py); still to be re-checked on the real deck.
    """
    covered: float | None = None
    related: float | None = None
    per_chunk: int = 3          # existing notes accepted per covered chunk
    concepts: int = 5           # concept neighbours listed per covered chunk
    max_group: int = 150        # topic tags larger than this are too broad to expand

    DEFAULTS = {False: (0.5, 0.35), True: (0.55, 0.4)}

    def resolve(self, embeddings):
        covered, related = self.DEFAULTS[bool(embeddings)]
        return replace(self, covered=covered if self.covered is None else self.covered,
                       related=related if self.related is None else self.related)


def _summary(index, hit, chunk_terms=None):
    note = index.notes[hit['doc']] if isinstance(hit, dict) else index.notes[hit]
    out = {'note_id': note['note_id'], 'deck': note.get('deck', ''),
           'suspended_card_ids': note.get('suspended_card_ids', []),
           'active': len(note.get('card_ids', [])) > len(note.get('suspended_card_ids', [])),
           'preview': note['text'].replace('\n', ' ')[:160]}
    if isinstance(hit, dict):
        out.update(score=hit['score'], overlap=hit['overlap'], cosine=hit['cosine'])
    return out


def match_chunks(index, chunks, thresholds=None, k=5, expand_concepts=True, unsuspend_concepts=False, already_unsuspended=()):
    """``already_unsuspended``: CARD ids a previous lecture already unsuspended; never proposed again."""
    already = set(already_unsuspended)
    t = (thresholds or Thresholds()).resolve(index.vectors is not None)
    rows, unsuspend = [], {}

    skipped = set()

    def claim(summary, chunk_id, reason):
        pending = [c for c in summary['suspended_card_ids'] if c not in already]
        skipped.update(c for c in summary['suspended_card_ids'] if c in already)
        if not pending:
            return
        entry = unsuspend.setdefault(summary['note_id'], {'note_id': summary['note_id'], 'card_ids': pending,
                                                          'chunks': [], 'reason': reason, 'preview': summary['preview']})
        if chunk_id not in entry['chunks']:
            entry['chunks'].append(chunk_id)
        if reason == 'match':
            entry['reason'] = 'match'

    for chunk in chunks:
        hits = index.search(chunk['text'], k=k)
        best = hits[0]['score'] if hits else 0.0
        status = 'covered' if best >= t.covered else 'related' if best >= t.related else 'gap'
        matches = [_summary(index, h) for h in hits if h['score'] >= t.related][:max(k, t.per_chunk)]
        accepted = [m for m in matches if m['score'] >= t.covered][:t.per_chunk]
        concepts = []
        if expand_concepts and status != 'gap':
            seen = {m['note_id'] for m in matches}
            terms = set(tokenize(chunk['text']))
            for hit in hits[:t.per_chunk]:
                if hit['score'] < t.related:
                    break
                tag, docs = index.neighbours(hit['doc'], t.max_group)
                ranked = sorted(docs, key=lambda d: -len(terms & set(tokenize(index.notes[d]['text']))))
                for doc in ranked:
                    if index.notes[doc]['note_id'] in seen:
                        continue
                    seen.add(index.notes[doc]['note_id'])
                    concepts.append(dict(_summary(index, doc), tag=tag))
                    if len(concepts) >= t.concepts:
                        break
                if len(concepts) >= t.concepts:
                    break
        for m in accepted:
            claim(m, chunk['id'], 'match')
        if unsuspend_concepts and status == 'covered':
            for c in concepts:
                claim(c, chunk['id'], 'concept')
        rows.append({'id': chunk['id'], 'page': chunk['page'], 'title': chunk.get('title', ''), 'text': chunk['text'],
                     'status': status, 'best_score': round(best, 4), 'matches': matches, 'concepts': concepts})
    plan = sorted(unsuspend.values(), key=lambda e: e['note_id'])
    card_ids = sorted({c for e in plan for c in e['card_ids']})
    counts = {s: sum(r['status'] == s for r in rows) for s in ('covered', 'related', 'gap')}
    return {'thresholds': asdict(t), 'embeddings': index.vectors is not None,
            'summary': dict(counts, chunks=len(rows), notes_to_unsuspend=len(plan), cards_to_unsuspend=len(card_ids),
                            already_unsuspended=len(skipped)),
            'chunks': rows,
            'unsuspend': {'card_ids': card_ids, 'notes': plan, 'performed_anki_operations': 0,
                          'instructions': 'Show the user this list. After approval, unsuspend these CARD ids with the official Anki MCP.'}}
