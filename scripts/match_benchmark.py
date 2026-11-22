"""Calibrate matching thresholds on the synthetic AnKing-style benchmark.

    .venv/bin/python scripts/match_benchmark.py            # BM25 only
    .venv/bin/python scripts/match_benchmark.py --embed    # + Nomic ONNX (downloads once)

The deck (tests/fixtures/match/synthetic_anking.json) is written in AnKing cloze style but is
NOT AnKing content; labels are lecture facts paraphrased from two real lectures. Replace with a
real AnKing export + hand labels when available (ankiplusmcp match --evaluate).
"""
import argparse, json, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src'))
from ankiplusmcp import match
from ankiplusmcp.match.index import get_embedder

ROOT = Path(__file__).resolve().parents[1] / 'tests' / 'fixtures' / 'match'


def sweep(index, labels):
    scored = []
    for label in labels:
        hits = index.search(label['text'], k=5)
        ids = [index.notes[h['doc']]['note_id'] for h in hits]
        scored.append((label, hits[0]['score'] if hits else 0.0, ids))
    rows = []
    for covered in [x / 100 for x in range(20, 85, 5)]:
        tp = fp = fn = tn = 0
        for label, best, ids in scored:
            want = set(label['note_ids'])
            predicted_covered = best >= covered
            right = bool(want) and ids[:1] and ids[0] in want
            if want and predicted_covered and right: tp += 1
            elif predicted_covered and not right: fp += 1      # would unsuspend a wrong card
            elif want and not predicted_covered: fn += 1       # would draft a duplicate
            else: tn += 1
        precision = tp / (tp + fp) if tp + fp else 1.0
        recall = tp / (tp + fn) if tp + fn else 1.0
        rows.append({'covered': covered, 'precision': round(precision, 3), 'recall': round(recall, 3),
                     'wrong_unsuspends': fp, 'duplicates_drafted': fn})
    return rows


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--embed', action='store_true')
    args = p.parse_args()
    notes = match.load_deck(ROOT / 'synthetic_anking.json')
    labels = json.loads((ROOT / 'labels.json').read_text())
    index = match.DeckIndex(notes, embedder=get_embedder('required') if args.embed else None)
    index.embed_all()
    print(json.dumps({'retrieval': match.evaluate(index, labels, k=5), 'sweep': sweep(index, labels)}, indent=1))


if __name__ == '__main__':
    main()
