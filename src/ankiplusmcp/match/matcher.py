"""Classify each chunk as covered / related / gap and collect cards to unsuspend."""
from dataclasses import dataclass, asdict, replace
import re
from .index import tokenize

# A number with its unit: "3 mg/dL", "2.5 mg/dl", "40%", "6 weeks".
_QUANTITY = re.compile(r'(?<![\w.])(\d+(?:\.\d+)?)\s*(%|[A-Za-zµμ]+(?:/[A-Za-z]+)?)')
_NOT_UNITS = {'and', 'or', 'of', 'to', 'the', 'in', 'a', 'an', 'x', 'times', 'th', 'st', 'nd', 'rd', 'is', 'are', 'with',
              'for', 'per', 'on', 'at', 'by', 'from', 'than', 'more', 'less'}


def quantities(text):
    """``{unit: {values}}`` for the numbers with units in a text."""
    out = {}
    for value, unit in _QUANTITY.findall(text or ''):
        unit = unit.lower()
        if unit not in _NOT_UNITS:
            out.setdefault(unit, set()).add(float(value))
    return out


def conflicts(lecture, card):
    """Quantities stated with the same unit in both texts but with no value in common,
    e.g. a lecture's "exceeds 3 mg/dL" against a card's "> 2.5 mg/dL"."""
    ours, theirs = quantities(lecture), quantities(card)
    return [{'unit': u, 'lecture': sorted(ours[u]), 'card': sorted(theirs[u])}
            for u in sorted(ours.keys() & theirs.keys()) if not ours[u] & theirs[u]]


def _question(front, answer):
    """The card as the student sees it: the hidden answer blanked in the note's first field."""
    first = answer.split(' … ')[0]
    return front.replace(first, '[...]', 1) if first and first in front else front


@dataclass(frozen=True)
class Thresholds:
    """``None`` picks the calibrated default for the index (with or without embeddings).

    BM25 only: covered 0.5, calibrated on a real AnKing export (33,078 notes) against 126
    hand-labelled chunks from two real lectures (precision 0.75 as a lower bound, recall 0.35;
    see docs/matching.md). With Nomic embeddings, on the same labels: covered 0.7 (precision
    0.88, recall 0.40, no true gap marked covered); related 0.62 (recall 0.70).
    """
    covered: float | None = None
    related: float | None = None
    per_chunk: int = 3          # existing notes accepted per covered chunk
    candidates: int = 3         # below 'covered': notes per chunk offered for the host to judge
    concepts: int = 5           # concept neighbours listed per covered chunk
    max_group: int = 150        # topic tags larger than this are too broad to expand

    DEFAULTS = {False: (0.5, 0.35), True: (0.7, 0.62)}

    def resolve(self, embeddings):
        covered, related = self.DEFAULTS[bool(embeddings)]
        return replace(self, covered=covered if self.covered is None else self.covered,
                       related=related if self.related is None else self.related)


def _summary(index, hit, chunk_terms=None):
    note = index.notes[hit['doc']] if isinstance(hit, dict) else index.notes[hit]
    answers = {int(k): v for k, v in (note.get('card_answers') or {}).items()}
    front = note.get('front') or note['text']
    out = {'note_id': note['note_id'], 'deck': note.get('deck', ''),
           'suspended_card_ids': note.get('suspended_card_ids', []), 'front': front,
           'cards': [{'card_id': c, 'question': _question(front, answers[c]), 'answer': answers[c]}
                     for c in note.get('suspended_card_ids', []) if c in answers],
           'active': len(note.get('card_ids', [])) > len(note.get('suspended_card_ids', [])),
           'preview': note['text'].replace('\n', ' ')[:160]}
    if isinstance(hit, dict):
        out.update(score=hit['score'], overlap=hit['overlap'], cosine=hit['cosine'], answer_named=hit.get('answer_named', False))
    return out


def match_chunks(index, chunks, thresholds=None, k=5, expand_concepts=True, unsuspend_concepts=False, already_unsuspended=()):
    """``already_unsuspended``: CARD ids a previous lecture already unsuspended; never proposed again."""
    already = set(already_unsuspended)
    t = (thresholds or Thresholds()).resolve(index.vectors is not None)
    rows, unsuspend, candidates = [], {}, {}

    skipped = set()

    def claim(summary, chunk, reason):
        chunk_id = chunk['id']
        pending = [c for c in summary['suspended_card_ids'] if c not in already]
        skipped.update(c for c in summary['suspended_card_ids'] if c in already)
        if not pending:
            return
        # A cloze note: propose only the siblings whose hidden answer is in the lecture line.
        cards = [c for c in summary['cards'] if c['card_id'] in pending]
        terms = set(tokenize(chunk['text'] + ' ' + chunk.get('context', '')))
        testing = [c for c in cards if set(tokenize(c['answer'])) & terms]
        if testing:
            pending = [c['card_id'] for c in testing]
        entry = unsuspend.setdefault(summary['note_id'], {'note_id': summary['note_id'], 'card_ids': [],
                                                          'chunks': [], 'reason': reason, 'preview': summary['preview'],
                                                          'cards': [], 'answer_in_lecture': False})
        entry['card_ids'] = sorted(set(entry['card_ids']) | set(pending))
        entry['answer_in_lecture'] = entry['answer_in_lecture'] or bool(testing)
        entry['cards'] = [c for c in cards if c['card_id'] in entry['card_ids']] or entry['cards']
        if chunk_id not in entry['chunks']:
            entry['chunks'].append(chunk_id)
        if reason == 'match':
            entry['reason'] = 'match'

    for chunk in chunks:
        query = f"{chunk['context']}: {chunk['text']}" if chunk.get('context') else chunk['text']
        hits = index.search(query, k=k)
        best = hits[0]['score'] if hits else 0.0
        status = 'covered' if best >= t.covered else 'related' if best >= t.related else 'gap'
        matches = [_summary(index, h) for h in hits if h['score'] >= t.related][:max(k, t.per_chunk)]
        accepted = [m for m in matches if m['score'] >= t.covered][:t.per_chunk]
        clashes = []
        for m in accepted:
            found = conflicts(chunk['text'], m['front'])
            if found:
                clashes.append({'note_id': m['note_id'], 'preview': m['preview'], 'quantities': found})
        if clashes:
            clashing = {c['note_id'] for c in clashes}
            accepted = [m for m in accepted if m['note_id'] not in clashing]
            if not accepted:
                status = 'conflict'   # the deck states a different number: card the lecture's version
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
            claim(m, chunk, 'match')
        taken = {m['note_id'] for m in accepted} | {c['note_id'] for c in clashes}
        for m in [m for m in matches if m['note_id'] not in taken and m['suspended_card_ids']][:t.candidates]:
            entry = candidates.setdefault(m['note_id'], {'note_id': m['note_id'], 'score': m['score'], 'chunks': [],
                                                         'preview': m['preview'], 'cards': m['cards'],
                                                         'suspended_card_ids': [c for c in m['suspended_card_ids'] if c not in already],
                                                         'answer_named': m.get('answer_named', False)})
            entry['chunks'].append(chunk['id'])
        if unsuspend_concepts and status == 'covered':
            for c in concepts:
                claim(c, chunk, 'concept')
        rows.append({'id': chunk['id'], 'page': chunk['page'], 'title': chunk.get('title', ''), 'text': chunk['text'],
                     'status': status, 'best_score': round(best, 4), 'matches': matches, 'concepts': concepts,
                     'conflicts': clashes})
    plan = sorted(unsuspend.values(), key=lambda e: e['note_id'])
    card_ids = sorted({c for e in plan for c in e['card_ids']})
    counts = {s: sum(r['status'] == s for r in rows) for s in ('covered', 'related', 'gap', 'conflict')}
    return {'thresholds': asdict(t), 'embeddings': index.vectors is not None,
            'summary': dict(counts, chunks=len(rows), notes_to_unsuspend=len(plan), cards_to_unsuspend=len(card_ids),
                            candidates_to_judge=len([c for c in candidates if c not in unsuspend]),
                            already_unsuspended=len(skipped)),
            'chunks': rows,
            'unsuspend': {'card_ids': card_ids, 'notes': plan, 'performed_anki_operations': 0,
                          'candidates': sorted((c for c in candidates.values() if c['note_id'] not in unsuspend),
                                               key=lambda c: -c['score']),
                          'instructions': JUDGE}}


JUDGE = ('Before showing this list, judge every note: read each card\'s question and answer (not the Extra) and keep it '
         'only if that card tests the lecture line it matched. Drop cards that are merely on the same topic, and pass the '
         'dropped notes as exclude_note_ids to prepare_unsuspend. Also judge the candidates (scored below covered; '
         'answer_named means the card hides a term the line names): add the ones that do test a lecture line with '
         'include_note_ids. Then show the user the list; after approval, unsuspend these CARD ids with the official Anki MCP.')
