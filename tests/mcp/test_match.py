import json
import sqlite3
import time
import zipfile
import numpy as np
import pytest
from ankiplusmcp import match
from ankiplusmcp.match import pgstore
from ankiplusmcp.match.index import tokenize, FileIndexStore
from ankiplusmcp.service import Service

AK = '#AK_Step1_v12::#B&B::Anatomy::Upper_Limb::Brachial_Plexus'
NOTES = [
    # note_id, fields, tags, cards [(card_id, queue)]
    (101, ['Thoracic outlet syndrome compresses the {{c1::lower trunk}} of the brachial plexus', 'Pancoast tumour, cervical rib'],
     [AK, '#AK_Step1_v12::#UWorld::Step::1234'], [(1001, -1)]),
    (102, ['Erb palsy injures the {{c1::upper trunk}} (C5-C6) of the brachial plexus', '<b>waiter tip</b>'],
     [AK], [(1002, -1)]),
    (103, ['Klumpke palsy injures the lower trunk C8-T1 causing claw hand', ''], [AK], [(1003, 0)]),
    (104, ['Sodium potassium ATPase pumps 3 sodium out and 2 potassium in', ''],
     ['#AK_Step1_v12::#B&B::Physiology::Membrane'], [(1004, -1), (1005, -1)]),
    (105, ['Myocardial infarction troponin rises within 4 hours', ''], ['#AK_Step1_v12::#B&B::Cardio'], [(1006, -1)]),
]


def make_collection(path, deck='AnKing::Step1'):
    db = sqlite3.connect(path)
    db.executescript('create table col (decks text, models text); create table notes (id integer, mid integer, tags text, flds text);'
                     'create table cards (id integer, nid integer, did integer, queue integer);')
    db.execute('insert into col values (?, ?)', (json.dumps({'1': {'name': deck}, '2': {'name': 'Default'}}),
                                                 json.dumps({'9': {'name': 'AnKingOverhaul', 'flds': [{'name': 'Text', 'ord': 0}, {'name': 'Extra', 'ord': 1}]}})))
    for nid, fields, tags, cards in NOTES:
        db.execute('insert into notes values (?, 9, ?, ?)', (nid, ' '.join(tags), '\x1f'.join(fields)))
        for cid, queue in cards:
            db.execute('insert into cards values (?, ?, 1, ?)', (cid, nid, queue))
    db.execute("insert into notes values (200, 9, '', 'Own card about sodium channels\x1f')")
    db.execute('insert into cards values (2000, 200, 2, -1)')
    db.commit(); db.close()
    return path


class FakeEmbedder:
    """Deterministic bag-of-words hashing: similar wording -> similar vectors."""
    name = 'fake@32'
    def _vec(self, text):
        v = np.zeros(32, dtype=np.float32)
        for t in tokenize(text): v[hash(t) % 32] += 1
        return v
    def embed_documents(self, texts): return np.array([self._vec(t) for t in texts])
    def embed_query(self, text): return self._vec(text)


@pytest.fixture
def deck(tmp_path):
    return make_collection(tmp_path / 'collection.anki2')


def test_load_collection_and_package(tmp_path, deck):
    notes = match.load_deck(deck, deck_prefix='AnKing')
    assert [n['note_id'] for n in notes] == [101, 102, 103, 104, 105]
    first = notes[0]
    assert first['suspended_card_ids'] == [1001] and first['deck'] == 'AnKing::Step1'
    assert 'lower trunk' in first['text'] and '{{' not in first['text']
    assert 'waiter tip' in notes[1]['text'] and '<b>' not in notes[1]['text']
    assert len(match.load_deck(deck)) == 6
    apkg = tmp_path / 'AnKing.apkg'
    with zipfile.ZipFile(apkg, 'w') as z: z.write(deck, 'collection.anki21')
    assert len(match.load_deck(apkg, deck_prefix='AnKing')) == 5
    as_json = tmp_path / 'notes.json'
    as_json.write_text(json.dumps([{'noteId': 7, 'fields': {'Text': {'value': '{{c1::Na}} channel', 'order': 0}}, 'cards': [70], 'tags': []}]))
    assert match.load_deck(as_json)[0]['text'] == 'Na channel'


def test_search_and_concept_tags(deck):
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'))
    hits = index.search('sodium potassium pump moves 3 sodium out')
    assert index.notes[hits[0]['doc']]['note_id'] == 104 and hits[0]['overlap'] > .6
    tag, docs = index.neighbours(0)
    assert tag == AK and {index.notes[d]['note_id'] for d in docs} == {102, 103}   # UWorld id tag ignored


def test_match_unsuspend_prompt(deck):
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'))
    chunks = match.chunks_from_text('# Thoracic outlet\n- Thoracic outlet syndrome compresses the lower trunk of the brachial plexus\n'
                                    '- Glomerular filtration rate depends on afferent arteriole tone\n')
    strict = match.Thresholds(covered=.6, related=.35)
    report = match.match_chunks(index, chunks, strict)
    status = {r['text'].split()[0]: r['status'] for r in report['chunks']}
    assert status == {'Thoracic': 'covered', 'Glomerular': 'gap'}
    assert report['unsuspend']['card_ids'] == [1001]      # only the matched note; concepts listed, not unsuspended
    covered = report['chunks'][0]
    assert {c['note_id'] for c in covered['concepts']} >= {102}
    assert report['unsuspend']['performed_anki_operations'] == 0
    with_concepts = match.match_chunks(index, chunks, strict, unsuspend_concepts=True)
    assert set(with_concepts['unsuspend']['card_ids']) == {1001, 1002}   # 103 has no suspended card
    prompt = match.build_prompt(report, 'L025.pdf')
    gaps = prompt.split('## Gaps')[1].split('##')[0]
    assert 'Glomerular' in gaps and 'Thoracic' not in gaps
    assert '## Already covered' in prompt


def test_embeddings_and_file_store(tmp_path, deck):
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'), embedder=FakeEmbedder())
    index.embed_all()
    hit = index.search('troponin rises after myocardial infarction')[0]
    assert hit['cosine'] is not None and index.notes[hit['doc']]['note_id'] == 105
    meta = index.save(tmp_path / 'idx')
    assert meta['embeddings'] and meta['notes'] == 5
    loaded = match.DeckIndex.load(tmp_path / 'idx', embedder=FakeEmbedder())
    assert loaded.vectors.shape == (5, 32)
    assert loaded.search('troponin rises after myocardial infarction')[0]['doc'] == hit['doc']
    assert match.DeckIndex.load(tmp_path / 'idx').vectors is None   # no embedder -> BM25 only


@pytest.mark.skipif(not pgstore.available(), reason='embedded Postgres not installed')
def test_postgres_store_roundtrip(tmp_path, deck):
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'), embedder=FakeEmbedder())
    index.embed_all()
    store = pgstore.PostgresIndexStore(tmp_path / 'pg', 'anking-test')
    try:
        meta = index.save(None, store)
        assert meta['backend'] == 'postgres' and meta['embeddings']
        loaded = match.DeckIndex.load(None, embedder=FakeEmbedder(), store=store)
        assert [n['note_id'] for n in loaded.notes] == [101, 102, 103, 104, 105]
        assert np.allclose(loaded.vectors, index.vectors, atol=1e-4)
        assert loaded.notes[0]['suspended_card_ids'] == [1001]
    finally:
        pgstore.shutdown()


def test_evaluate(deck):
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'))
    result = match.evaluate(index, [{'text': 'Erb palsy upper trunk C5 C6', 'note_ids': [102]},
                                    {'text': 'Loop of Henle countercurrent multiplier', 'note_ids': []}])
    assert result['recall_at_k'] == 1.0 and result['gap_precision'] == 1.0


def wait(service, w, job):
    for _ in range(300):
        result = service.jobs.get(w, job['id'])
        if result['status'] not in ('queued', 'running'): break
        time.sleep(.02)
    assert result['status'] == 'completed', result
    return result['result']


def test_service_index_match_and_support(tmp_path, deck):
    s = Service(tmp_path / 'work', [tmp_path])
    try:
        w = s.store.create('lecture', text='# Plexus\n- Erb palsy injures the upper trunk C5-C6 of the brachial plexus\n'
                                           '- The ulnar nerve supplies the hypothenar muscles of the hand\n')['id']
        built = wait(s, w, s.index_deck(w, str(deck), name='anking', deck_prefix='AnKing', embeddings='off', backend='file'))
        assert built['notes'] == 5 and built['backend'] == 'file'
        assert s.list_indexes()['indexes'][0]['name'] == 'anking'
        result = s.match_source(w, 'anking')
        assert result['summary']['covered'] == 1 and result['summary']['gap'] == 1
        plan = s.store.read_json(w, result['artifacts']['unsuspend'])
        assert plan['card_ids'] == [1002]
        assert 'ulnar nerve' in s.store.path(w, result['artifacts']['prompt']).read_text()
        highlighted = s.match_source(w, 'anking', highlights=[{'page': 4, 'text': 'Klumpke palsy lower trunk claw hand', 'note': ''}])
        assert highlighted['chunk_origin'] == 'highlights' and highlighted['summary']['covered'] == 1
        s.save_cards(w, [{'id': 'c1', 'fields': {'Text': 'Erb palsy injures the {{c1::upper trunk}}', 'Extra': ''}, 'source_ref': [1]},
                         {'id': 'c2', 'fields': {'Text': 'The ulnar nerve supplies the {{c1::thenar}} muscles', 'Extra': ''}, 'source_ref': [1]}], 0)
        support = {r['card_id']: r for r in s.check_source_support(w)['cards']}
        assert not support['c1']['flag']
        assert support['c2']['flag'] and support['c2']['answers_not_in_source'] == ['thenar']
    finally:
        s.close()


def test_cli(tmp_path, deck, capsys):
    from ankiplusmcp.__main__ import main
    lecture = tmp_path / 'lecture.md'
    lecture.write_text('# Pump\n- The sodium potassium ATPase pumps 3 sodium out and 2 potassium in\n')
    code = main(['match', '--deck', str(deck), '--deck-prefix', 'AnKing', '--text', str(lecture), '--out', str(tmp_path / 'out'),
                 '--index-dir', str(tmp_path / 'idx'), '--embeddings', 'off'])
    assert code == 0 and '1 covered' in capsys.readouterr().out
    assert json.loads((tmp_path / 'out' / 'unsuspend.json').read_text())['card_ids'] == [1004, 1005]


@pytest.mark.skipif(not __import__('os').environ.get('ANKIPLUS_REAL_EMBEDDINGS'), reason='set ANKIPLUS_REAL_EMBEDDINGS=1 to download and run the Nomic ONNX model')
def test_real_nomic_embeddings_find_paraphrases(deck):
    from ankiplusmcp.match.index import get_embedder
    index = match.DeckIndex(match.load_deck(deck, deck_prefix='AnKing'), embedder=get_embedder('required'))
    index.embed_all()
    assert index.vectors.shape == (5, 256)
    hit = index.search('cardiac enzyme elevated after a heart attack')[0]   # no shared words with note 105
    assert index.notes[hit['doc']]['note_id'] == 105 and hit['overlap'] == 0 and hit['cosine'] > .55


def test_garbled_pdf_text_and_line_joining():
    from ankiplusmcp.match.chunks import looks_garbled
    # A broken font encoding seen in a real lecture: every glyph shifted by 29 code points.
    shifted = ''.join(chr(ord(c) - 29) for c in 'The digestive system is responsible for breaking down food into absorbable nutrients')
    assert looks_garbled(shifted) and not looks_garbled('The digestive system is responsible for breaking down food.')
    slides = [{'n': n, 'title': '', 'text': f'BMC Medical College\n{body}\n{n}'} for n, body in enumerate([
        '\u2022 Tongue becomes red, smooth, and sore (seen in vitamin\nB12, folate, and iron deficiency)\nFigure 3: Netter atlas',
        '\u00a2 Sulcus terminalis divides the oral part\nfrom the pharyngeal part',
        'Foramen cecum marks the thyroid descent site'], start=1)]
    texts = [c['text'] for c in match.chunks_from_slides(slides)]
    assert texts == ['Tongue becomes red, smooth, and sore (seen in vitamin B12, folate, and iron deficiency)',
                     'Sulcus terminalis divides the oral part from the pharyngeal part',
                     'Foramen cecum marks the thyroid descent site']


def test_threshold_defaults_follow_the_index_mode():
    t = match.Thresholds()
    assert (t.resolve(False).covered, t.resolve(False).related) == (.5, .35)
    assert (t.resolve(True).covered, t.resolve(True).related) == (.7, .62)
    assert match.Thresholds(covered=.7).resolve(True).covered == .7


def test_synthetic_benchmark_bm25_no_wrong_unsuspends():
    import importlib.util, pathlib
    root = pathlib.Path(__file__).resolve().parents[2]
    spec = importlib.util.spec_from_file_location('bench', root / 'scripts' / 'match_benchmark.py')
    bench = importlib.util.module_from_spec(spec); spec.loader.exec_module(bench)
    index = match.DeckIndex(match.load_deck(bench.ROOT / 'synthetic_anking.json'))
    labels = json.loads((bench.ROOT / 'labels.json').read_text())
    row = next(r for r in bench.sweep(index, labels) if r['covered'] == .5)
    assert row['wrong_unsuspends'] == 0 and row['recall'] >= .6
    assert match.evaluate(index, labels)['recall_at_k'] >= .9


def test_highlights_read_from_pdf_annotations(tmp_path, deck):
    import pymupdf
    pdf = tmp_path / 'highlighted.pdf'
    with pymupdf.open() as doc:
        page = doc.new_page()
        page.insert_text((50, 80), 'Erb palsy injures the upper trunk C5-C6 of the brachial plexus', fontsize=11)
        page.insert_text((50, 120), 'This sentence is not highlighted and should be ignored entirely', fontsize=11)
        page.insert_text((50, 160), 'Klumpke palsy injures the lower trunk C8-T1 causing claw hand', fontsize=11)
        for needle, note in [('Erb palsy injures the upper trunk C5-C6 of the brachial plexus', 'exam'),
                             ('Klumpke palsy injures the lower trunk C8-T1 causing claw hand', '')]:
            annot = page.add_highlight_annot(page.search_for(needle))
            annot.set_info(content=note); annot.update()
        page.add_rect_annot(pymupdf.Rect(300, 300, 400, 400))
        doc.save(pdf)
    found = match.highlights_from_pdf(pdf)
    texts = [h['text'] for h in found if h['kind'] == 'text']
    assert texts == ['Erb palsy injures the upper trunk C5-C6 of the brachial plexus',
                     'Klumpke palsy injures the lower trunk C8-T1 causing claw hand']
    assert found[0]['note'] == 'exam' and any(h['kind'] == 'region' for h in found)
    s = Service(tmp_path / 'work', [tmp_path])
    try:
        w = s.store.create('lecture', source_path=str(pdf))['id']
        wait(s, w, s.index_deck(w, str(deck), deck_prefix='AnKing', embeddings='off', backend='file'))
        result = s.match_source(w, 'anking', pdf_highlights=True)
        assert result['chunk_origin'] == 'pdf_highlights' and result['summary']['chunks'] == 2
        assert result['summary']['covered'] == 2
    finally:
        s.close()


def test_unsuspend_preview_approve_and_log(tmp_path, deck):
    s = Service(tmp_path / 'work', [tmp_path])
    try:
        text = '# Pump\n- The sodium potassium ATPase pumps 3 sodium out and 2 potassium in\n- Troponin rises within 4 hours of myocardial infarction\n'
        w = s.store.create('lecture', text=text)['id']
        wait(s, w, s.index_deck(w, str(deck), deck_prefix='AnKing', embeddings='off', backend='file'))
        first = s.match_source(w, 'anking')
        preview = s.prepare_unsuspend(w)
        assert preview['match_id'] == first['match_id'] and preview['card_ids'] == [1004, 1005, 1006]
        assert preview['notes'][0]['lecture'][0].startswith('p1: The sodium')
        trimmed = s.prepare_unsuspend(w, exclude_note_ids=[105])
        assert trimmed['card_ids'] == [1004, 1005]
        assert s.store.read_json(w, trimmed['artifact'])['card_ids'] == [1004, 1005]
        s.record_unsuspend(w, trimmed['card_ids'], 'success')
        # A later lecture covering the same fact does not propose those cards again.
        w2 = s.store.create('lecture', text=text)['id']
        again = s.match_source(w2, 'anking')
        assert again['summary']['already_unsuspended'] == 2
        assert s.prepare_unsuspend(w2)['card_ids'] == [1006]
    finally:
        s.close()


def test_deck_size_limit_allows_big_apkg(tmp_path, monkeypatch):
    from ankiplusmcp import matching
    from ankiplusmcp.store import DomainError
    s = Service(tmp_path / 'work', [tmp_path])
    try:
        apkg, jsn = tmp_path / 'AnKing.apkg', tmp_path / 'notes.json'
        apkg.write_bytes(b'x' * 10); jsn.write_bytes(b'x' * 10)
        monkeypatch.setattr(matching, 'MAX_DECK', 5)
        assert s._deck_file(str(apkg)) == apkg.resolve()
        with pytest.raises(DomainError):
            s._deck_file(str(jsn))
        monkeypatch.setattr(matching, 'MAX_ARCHIVE', 5)
        with pytest.raises(DomainError):
            s._deck_file(str(apkg))
    finally:
        s.close()


def test_score_needs_both_directions_and_real_words():
    notes = [{'note_id': 1, 'card_ids': [11], 'suspended_card_ids': [11], 'tags': [], 'text': '3 2 = 9', 'front': '3 2 = 9'},
             {'note_id': 2, 'card_ids': [12], 'suspended_card_ids': [12], 'tags': [],
              'front': 'The buccinator is the principal muscle of the cheek', 'text': 'The buccinator is the principal muscle of the cheek'},
             {'note_id': 3, 'card_ids': [13], 'suspended_card_ids': [13], 'tags': [],
              'front': 'Epistaxis most often arises from Kiesselbach plexus',
              'text': 'Epistaxis most often arises from Kiesselbach plexus\nsupplied by the labial artery of the facial artery and others'}]
    notes += [{'note_id': 100 + i, 'card_ids': [], 'suspended_card_ids': [], 'tags': [], 'text': f'filler word{i} muscle artery', 'front': f'filler word{i}'}
              for i in range(30)]
    index = match.DeckIndex(notes)
    top = lambda q: {index.notes[h['doc']]['note_id']: h['score'] for h in index.search(q, k=5)}
    assert top('3 Histology 9 2').get(1, 0) < .35
    assert top('Lateral wall: cheeks (buccinator muscle)')[2] >= .5
    # the note's Extra mentions the words, but its fact is about something else
    assert top('Blood supply of the lips: labial arteries from the facial artery').get(3, 0) < .35


def test_schema18_collection_with_unicase_indexes(tmp_path):
    # Real Anki 2.1.50+ exports index notetype/field names with Anki's own 'unicase' collation.
    path = tmp_path / 'collection.anki2'
    db = sqlite3.connect(path)
    db.create_collation('unicase', lambda a, b: (a.lower() > b.lower()) - (a.lower() < b.lower()))
    db.executescript('''create table col (decks text, models text);
        create table notes (id integer, mid integer, tags text, flds text);
        create table cards (id integer, nid integer, did integer, queue integer);
        create table decks (id integer, name text collate unicase);
        create table notetypes (id integer, name text collate unicase);
        create table fields (ntid integer, ord integer, name text collate unicase, primary key (ntid, ord)) without rowid;
        create unique index idx_fields_name_ntid on fields (name, ntid);
        create unique index idx_notetypes_name on notetypes (name);''')
    db.execute("insert into col values ('', '')")
    db.execute("insert into decks values (1, 'AnKing')")
    db.execute("insert into notetypes values (9, 'AnKingOverhaul')")
    db.executemany('insert into fields values (9, ?, ?)', [(0, 'Text'), (1, 'Extra')])
    db.execute("insert into notes values (5, 9, '', 'The buccinator is the muscle of the cheek\x1fextra')")
    db.execute('insert into cards values (50, 5, 1, -1)')
    db.commit(); db.close()
    notes = match.load_deck(path)
    assert notes[0]['fields']['Text'].startswith('The buccinator') and notes[0]['front'] == 'The buccinator is the muscle of the cheek'
