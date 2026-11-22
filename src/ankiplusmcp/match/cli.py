"""``ankiplusmcp match``: the deterministic pipeline from the command line, no AI host needed.

    ankiplusmcp match --deck AnKing.apkg --pdf lecture.pdf --out builds/L025
    ankiplusmcp match --deck AnKing.apkg --highlights highlights.json --out builds/L025
    ankiplusmcp match --deck AnKing.apkg --evaluate labels.json
    ankiplusmcp match --profile "User 1" --pdf lecture.pdf --out builds/L025   # no export

The deck index is cached per deck file under ``--index-dir``; when the file changes (an
AnkiHub sync, a new export), only new or edited notes are re-embedded.
"""
import argparse
import hashlib
import json
from pathlib import Path
import sys

from . import load_deck, DeckIndex, chunks_from_pdf, chunks_from_text, chunks_from_highlights, match_chunks, Thresholds, build_prompt, evaluate
from .index import get_embedder, FileIndexStore


def _store(args, key):
    if args.backend == 'postgres':
        from . import pgstore
        return pgstore.PostgresIndexStore(Path(args.index_dir) / 'pg', key)
    return FileIndexStore(Path(args.index_dir) / key)


def _index(args):
    if args.deck:
        deck = Path(args.deck).expanduser().resolve()
    else:
        from .profiles import collection_for
        deck = collection_for(args.profile)
    stat = deck.stat()
    stamp = f'{stat.st_size}|{stat.st_mtime_ns}'
    key = 'd' + hashlib.sha256(f'{deck}|{args.deck_prefix}|{args.embeddings}'.encode()).hexdigest()[:16]
    marker = Path(args.index_dir) / key / 'meta.json'
    embedder = get_embedder(args.embeddings)
    store = _store(args, key)
    previous = None
    if marker.is_file() and not args.rebuild:
        try:
            previous = DeckIndex.load(marker.parent, embedder=embedder, store=store)
        except ValueError:  # older index format: rebuild from scratch
            previous = None
        if previous is not None and json.loads(marker.read_text()).get('deck_stamp') == stamp:
            return previous
    print(f'indexing {deck.name} ...', file=sys.stderr)
    notes = load_deck(deck, deck_prefix=args.deck_prefix)
    index = DeckIndex(notes, embedder=embedder, meta={'source': deck.name, 'deck_prefix': args.deck_prefix})
    counts = index.embed_all(progress=lambda d, t: print(f'\rembedding {d}/{t}', end='', file=sys.stderr), previous=previous)
    if counts['reused']:
        print(f"\nre-embedded {counts['embedded']} new or changed notes, reused {counts['reused']}", file=sys.stderr)
    meta = index.save(marker.parent, store)
    marker.parent.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps(dict(meta, backend=args.backend, deck_stamp=stamp), indent=2) + '\n')
    return index


def main(argv=None):
    p = argparse.ArgumentParser(prog='ankiplusmcp match', description='Match a lecture against an existing deck without AI.')
    deck = p.add_mutually_exclusive_group(required=True)
    deck.add_argument('--deck', help='.apkg, .colpkg, .anki2 or .json/.jsonl note list')
    deck.add_argument('--profile', nargs='?', const='', help="read this Anki profile's collection directly (no export); no name = the only profile or 'User 1'")
    p.add_argument('--deck-prefix', help='only notes in this deck or its subdecks')
    src = p.add_mutually_exclusive_group(required=True)
    src.add_argument('--pdf')
    src.add_argument('--text')
    src.add_argument('--highlights', help='JSON list of {page, text, note}')
    src.add_argument('--evaluate', metavar='LABELS', help='JSON list of {text, note_ids}; prints recall and gap precision')
    p.add_argument('--highlighted', action='store_true', help='with --pdf: use only the text highlighted in the PDF')
    p.add_argument('--out', help='output folder (report.json, unsuspend.json, prompt.md)')
    p.add_argument('--index-dir', default=str(Path.home() / '.cache' / 'ankiplusmcp' / 'indexes'))
    p.add_argument('--backend', choices=['file', 'postgres'], default='file')
    p.add_argument('--embeddings', choices=['auto', 'off', 'required'], default='auto')
    p.add_argument('--covered', type=float, default=None, help='default: 0.5 BM25 only, 0.7 with embeddings')
    p.add_argument('--related', type=float, default=None, help='default: 0.35 BM25 only, 0.62 with embeddings')
    p.add_argument('--rebuild', action='store_true')
    args = p.parse_args(argv)
    try:
        index = _index(args)
        if args.evaluate:
            print(json.dumps(evaluate(index, json.loads(Path(args.evaluate).read_text()), covered=args.covered), indent=2))
            return 0
        if not args.out:
            p.error('--out is required unless --evaluate is used')
        if args.pdf and args.highlighted:
            from .chunks import highlights_from_pdf
            chunks, name = chunks_from_highlights(highlights_from_pdf(args.pdf)), Path(args.pdf).name
        elif args.pdf:
            chunks, name = chunks_from_pdf(args.pdf), Path(args.pdf).name
        elif args.text:
            chunks, name = chunks_from_text(Path(args.text).read_text()), Path(args.text).name
        else:
            chunks, name = chunks_from_highlights(json.loads(Path(args.highlights).read_text())), Path(args.highlights).name
        report = match_chunks(index, chunks, Thresholds(covered=args.covered, related=args.related))
        out = Path(args.out)
        out.mkdir(parents=True, exist_ok=True)
        (out / 'report.json').write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')
        (out / 'unsuspend.json').write_text(json.dumps(report['unsuspend'], indent=2) + '\n')
        (out / 'prompt.md').write_text(build_prompt(report, name))
        s = report['summary']
        print(f"{s['chunks']} chunks: {s['covered']} covered, {s['related']} related, {s['gap']} gaps, {s.get('conflict', 0)} conflicts; "
              f"{s['cards_to_unsuspend']} suspended cards to unsuspend -> {out}")
        return 0
    except (ValueError, OSError) as exc:  # FileNotFoundError (no Anki profile) is an OSError
        print(f'error: {exc}', file=sys.stderr)
        return 2
    finally:
        if args.backend == 'postgres':
            from . import pgstore
            pgstore.shutdown()
