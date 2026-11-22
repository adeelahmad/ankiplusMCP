"""Measure matching quality against hand labels, to calibrate thresholds and embeddings.

Labels: ``[{'text': '<lecture fact>', 'note_ids': [<expected existing notes>]}]``;
an empty ``note_ids`` means the fact has no existing card (a true gap).
"""


def evaluate(index, labels, k=5, covered=0.6):
    found = expected = true_gaps = flagged_gaps = false_gaps = 0
    misses = []
    for label in labels:
        hits = index.search(label['text'], k=k)
        ids = [index.notes[h['doc']]['note_id'] for h in hits]
        want = set(label.get('note_ids', []))
        best = hits[0]['score'] if hits else 0.0
        if not want:
            true_gaps += 1
            flagged_gaps += best < covered
            continue
        got = want & set(ids)
        found += len(got)
        expected += len(want)
        false_gaps += best < covered
        if len(got) < len(want):
            misses.append({'text': label['text'][:120], 'missing': sorted(want - got), 'top': ids[:3]})
    return {'recall_at_k': round(found / expected, 4) if expected else None, 'k': k,
            'gap_precision': round(flagged_gaps / true_gaps, 4) if true_gaps else None,
            'covered_facts_marked_gap': false_gaps, 'labels': len(labels),
            'embeddings': index.vectors is not None, 'misses': misses[:20]}
