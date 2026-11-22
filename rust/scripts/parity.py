"""Parity check: the Rust `ankiplus` CLI must give the same results as the Python pipeline.

Builds a synthetic AnKing-like collection (seeded, a few thousand notes with nested
topic tags, HTML and cloze markup), then for several lectures runs both
implementations with embeddings off and compares report.json, unsuspend.json and
prompt.md exactly (floats to 1e-9). Also checks that each side loads the other's
cached file index, that `evaluate` agrees, and that the quote check agrees.

    .venv/bin/python rust/scripts/parity.py [--bin rust/target/debug/ankiplus] [--notes 3000]

Exit code 0 means parity.
"""
import argparse
import json
import random
import shutil
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'src'))

from ankiplusmcp.match import cli as pycli  # noqa: E402
from ankiplusmcp.match import chunks_from_text  # noqa: E402
from ankiplusmcp.match.quotes import check_support  # noqa: E402

VOCAB = '''brachial plexus trunk lower upper median ulnar radial nerve axillary artery vein thoracic outlet
cervical rib scalene palsy claw hand wrist drop sodium potassium chloride calcium channel pump atpase
troponin myocardial infarction ischemia angina aorta coronary ventricle atrium valve murmur stenosis
regurgitation kidney glomerular filtration afferent efferent arteriole tubule loop henle diuretic
insulin glucagon glucose liver glycogen pancreas islet beta cell receptor kinase phosphorylation
dna rna polymerase helicase ligase replication transcription translation ribosome codon mutation
deletion insertion frameshift nonsense missense collagen fibroblast inflammation neutrophil macrophage
cytokine interleukin tumor necrosis factor antibody antigen complement histamine mast cell asthma
bronchus alveolus surfactant pneumothorax effusion pleura diaphragm phrenic vagus recurrent laryngeal'''.split()
UNSEEN = '''sarcomere titin nebulin dystrophin spectrin ankyrin porphyria heme ferrochelatase urobilinogen
prion amyloid tau synuclein huntingtin frataxin ataxia chorea dystonia tremor'''.split()
TOPICS = ['Anatomy::Upper_Limb::Brachial_Plexus', 'Anatomy::Upper_Limb::Forearm', 'Anatomy::Thorax',
          'Physiology::Membrane', 'Physiology::Renal', 'Cardio::Ischemic_Heart_Disease', 'Cardio::Valves',
          'Biochem::DNA', 'Biochem::Metabolism', 'Immunology::Cytokines', 'Pulm::Pleura', 'Endocrine::Pancreas']


def sentence(rng, n):
    return ' '.join(rng.choice(VOCAB) for _ in range(n))


def make_deck(path, n_notes, seed=7):
    rng = random.Random(seed)
    db = sqlite3.connect(path)
    db.executescript('create table col (decks text, models text); create table notes (id integer, mid integer, tags text, flds text);'
                     'create table cards (id integer, nid integer, did integer, queue integer);')
    db.execute('insert into col values (?, ?)', (json.dumps({'1': {'name': 'AnKing::Step1'}, '2': {'name': 'Default'}}),
                                                 json.dumps({'9': {'name': 'AnKingOverhaul', 'flds': [{'name': 'Text', 'ord': 0}, {'name': 'Extra', 'ord': 1}]}})))
    card = 10_000
    for i in range(n_notes):
        nid = 1_000_000 + i
        words = sentence(rng, rng.randint(6, 18)).split()
        k = rng.randrange(len(words))
        words[k] = '{{c1::' + words[k] + '}}'
        text = ' '.join(words)
        if rng.random() < .3:
            text = text.replace(' ', ' <b>', 1) + '</b>'
        extra = sentence(rng, rng.randint(0, 6)) + ('<br>&nbsp;Pancoast &amp; co' if rng.random() < .1 else '')
        topic = rng.choice(TOPICS)
        tags = [f'#AK_Step1_v12::#B&B::{topic}', f'#AK_Step1_v12::#UWorld::Step::{rng.randint(1, 9999)}']
        if rng.random() < .2:
            tags.append(f'#AK_Step1_v12::!FreeMedia::{rng.randint(1, 99)}')
        db.execute('insert into notes values (?, 9, ?, ?)', (nid, ' '.join(tags), f'{text}\x1f{extra}'))
        for _ in range(rng.choice([1, 1, 2])):
            card += 1
            db.execute('insert into cards values (?, ?, ?, ?)', (card, nid, 1 if rng.random() < .95 else 2,
                                                                 -1 if rng.random() < .85 else 0))
    db.commit()
    db.close()
    return rng


def lectures(rng):
    out = {}
    for n in range(4):
        parts = []
        for page in range(1, 6):
            parts.append(f'# Slide {page} {sentence(rng, 2)}')
            parts.append('BMC Medical College')          # boilerplate on every page
            if rng.random() < .5:
                parts.append(f'Figure {page}: Netter atlas')
            for _ in range(rng.randint(2, 6)):
                bullet = rng.choice(['- ', '* ', '1. ', '• ', ''])
                line = sentence(rng, rng.randint(1, 14))
                if rng.random() < .35:  # facts the deck does not know -> related or gap
                    line = ' '.join(rng.choice(UNSEEN) for _ in range(rng.randint(3, 9))) + ' ' + sentence(rng, rng.randint(0, 2))
                roll = rng.random()
                if roll < .15:   # wrapped sentence: continuation starts lower case or follows 'and'
                    line = line + ' and\n' + sentence(rng, 4)
                elif roll < .25:
                    line = line + ' (' + sentence(rng, 2) + '\n' + sentence(rng, 2) + ')'
                elif roll < .35:
                    line = line + ' \u2022 ' + sentence(rng, 5)   # inline bullet
                elif roll < .4:
                    line = '\u00a2 ' + line + ' 3'               # OCR bullet glyph + list number
                if rng.random() < .05:   # bibliography line, dropped by both
                    line = 'Moore KL, Clinically Oriented Anatomy, 8th edn, Wolters Kluwer 2018'
                parts.append(bullet + line[:1].upper() + line[1:])
            parts.append(str(page))
        out[f'L{n:03d}.md'] = '\n'.join(parts) + '\n'
    return out


def close(a, b, path='$'):
    if isinstance(a, float) or isinstance(b, float):
        if a is None or b is None or abs(float(a) - float(b)) > 1e-9:
            return [f'{path}: {a!r} != {b!r}']
        return []
    if type(a) is not type(b):
        return [f'{path}: type {type(a).__name__} != {type(b).__name__}']
    if isinstance(a, dict):
        if list(a) != list(b):
            return [f'{path}: keys {list(a)} != {list(b)}']
        return [e for k in a for e in close(a[k], b[k], f'{path}.{k}')]
    if isinstance(a, list):
        if len(a) != len(b):
            return [f'{path}: len {len(a)} != {len(b)}']
        return [e for i, (x, y) in enumerate(zip(a, b)) for e in close(x, y, f'{path}[{i}]')]
    return [] if a == b else [f'{path}: {a!r} != {b!r}']


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--bin', default=str(ROOT / 'rust' / 'target' / 'debug' / 'ankiplus'))
    p.add_argument('--notes', type=int, default=3000)
    args = p.parse_args()
    failures = []
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        deck = tmp / 'collection.anki2'
        rng = make_deck(deck, args.notes)
        for name, text in lectures(rng).items():
            src = tmp / name
            src.write_text(text)
            for extra in ([], ['--covered', '0.5', '--related', '0.25'], ['--covered', '0.6']):
                py_out, rs_out = tmp / 'py' / name, tmp / 'rs' / name
                base = ['--deck', str(deck), '--deck-prefix', 'AnKing', '--embeddings', 'off', '--text', str(src)] + extra
                assert pycli.main(base + ['--out', str(py_out), '--index-dir', str(tmp / 'py-idx')]) == 0
                subprocess.run([args.bin, 'match', *base, '--out', str(rs_out), '--index-dir', str(tmp / 'rs-idx')],
                               check=True, capture_output=True)
                for f in ('report.json', 'unsuspend.json'):
                    errs = close(json.loads((py_out / f).read_text()), json.loads((rs_out / f).read_text()))
                    failures += [f'{name} {extra} {f} {e}' for e in errs[:5]]
                if (py_out / 'prompt.md').read_text() != (rs_out / 'prompt.md').read_text():
                    failures.append(f'{name} {extra} prompt.md differs')
                shutil.rmtree(tmp / 'py'); shutil.rmtree(tmp / 'rs')
        # Cross-loading: Rust reads the index Python cached, and the other way round.
        src = tmp / 'L000.md'
        base = ['--deck', str(deck), '--deck-prefix', 'AnKing', '--embeddings', 'off', '--text', str(src)]
        assert pycli.main(base + ['--out', str(tmp / 'a'), '--index-dir', str(tmp / 'shared')]) == 0
        r = subprocess.run([args.bin, 'match', *base, '--out', str(tmp / 'b'), '--index-dir', str(tmp / 'shared')],
                           capture_output=True, text=True, check=True)
        if 'indexing' in r.stderr:
            failures.append('Rust rebuilt the index instead of loading the Python cache (cache key differs)')
        failures += [f'cross-load {e}' for e in close(json.loads((tmp / 'a' / 'report.json').read_text()),
                                                      json.loads((tmp / 'b' / 'report.json').read_text()))[:5]]
        # Evaluate.
        labels = [{'text': sentence(rng, 8), 'note_ids': [1_000_000 + rng.randrange(args.notes)]} for _ in range(30)]
        labels += [{'text': 'completely unrelated words zebra quantum', 'note_ids': []}]
        (tmp / 'labels.json').write_text(json.dumps(labels))
        ev = ['--deck', str(deck), '--deck-prefix', 'AnKing', '--embeddings', 'off', '--evaluate', str(tmp / 'labels.json'),
              '--index-dir', str(tmp / 'shared')]
        import contextlib, io
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            pycli.main(ev)
        rs = subprocess.run([args.bin, 'match', *ev], capture_output=True, text=True, check=True).stdout
        failures += [f'evaluate {e}' for e in close(json.loads(buf.getvalue()), json.loads(rs))[:5]]
        # Calibration benchmark from tests/fixtures/match, default (calibrated) thresholds.
        fx = ROOT / 'tests' / 'fixtures' / 'match'
        ev = ['--deck', str(fx / 'synthetic_anking.json'), '--embeddings', 'off', '--evaluate', str(fx / 'labels.json'),
              '--index-dir', str(tmp / 'bench')]
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            pycli.main(ev)
        rs = subprocess.run([args.bin, 'match', *ev], capture_output=True, text=True, check=True).stdout
        failures += [f'benchmark evaluate {e}' for e in close(json.loads(buf.getvalue()), json.loads(rs))[:5]]
        # Quote check.
        chunks = chunks_from_text(src.read_text())
        cards = [{'id': i, 'fields': {'Text': ' '.join(c['text'].split()[:6]) + ' {{c1::' + w + '}}'},
                  'source_ref': [c['page']] if i % 2 else []}
                 for i, (c, w) in enumerate(zip(chunks, rng.choices(VOCAB, k=len(chunks))))]
        (tmp / 'cards.json').write_text(json.dumps(cards))
        rs = subprocess.run([args.bin, 'check', '--cards', str(tmp / 'cards.json'), '--text', str(src)],
                            capture_output=True, text=True, check=True).stdout
        failures += [f'check_support {e}' for e in close(check_support(cards, chunks), json.loads(rs))[:5]]
    if failures:
        print('PARITY FAILED')
        print('\n'.join(failures[:40]))
        return 1
    print(f'parity ok: 12 lecture runs, calibration benchmark, cross-loaded cache, evaluate and quote check on {args.notes} notes')
    return 0


if __name__ == '__main__':
    sys.exit(main())
