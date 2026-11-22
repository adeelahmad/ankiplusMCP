"""Read notes and their cards from a deck export or a read-only collection copy.

Supported inputs:
- ``.apkg`` / ``.colpkg`` (zip; ``collection.anki21b`` needs zstd support,
  ``collection.anki21`` / ``collection.anki2`` are plain SQLite),
- ``.anki2`` / ``.anki21`` SQLite collections (copied first, never opened for writing),
- ``.json`` / ``.jsonl`` note lists, e.g. saved from the Anki MCP ``notes_info`` output.

Every note becomes a plain dict::

    {'note_id', 'card_ids', 'suspended_card_ids', 'deck', 'notetype', 'tags', 'fields', 'text'}
"""
from contextlib import closing
import html
import io
import json
from pathlib import Path
import re
import shutil
import sqlite3
import tempfile
import zipfile

SUSPENDED = -1
_CLOZE = re.compile(r'\{\{c\d+::(.*?)(?:::[^}]*)?\}\}', re.S)
_TAG = re.compile(r'<[^>]+>')
_BREAK = re.compile(r'<\s*(br|/div|/p|/li|/tr|/h\d)\b[^>]*>', re.I)
_DROP = re.compile(r'<\s*(script|style)\b.*?</\s*\1\s*>|\[sound:[^\]]*\]', re.I | re.S)
_SPACE = re.compile(r'[ \t ]+')


class DeckError(ValueError):
    pass


def clean_text(value):
    """HTML field -> plain text; cloze markers keep their answer text."""
    value = _DROP.sub(' ', value or '')
    value = _CLOZE.sub(lambda m: m.group(1), value)
    value = _BREAK.sub('\n', value)
    value = html.unescape(_TAG.sub(' ', value))
    lines = (_SPACE.sub(' ', line).strip() for line in value.splitlines())
    return '\n'.join(line for line in lines if line)


def load_deck(path, deck_prefix=None, fields=None, suspended_only=False):
    """Load notes. ``deck_prefix`` keeps notes with a card in a deck at or under it.

    ``fields`` names the fields used for the searchable text (default: the first two,
    which for AnKing are Text and Extra). Tags are kept separately for concept mapping.
    """
    path = Path(path).expanduser()
    if not path.is_file():
        raise DeckError(f'Deck file not found: {path}')
    suffix = path.suffix.lower()
    if suffix in ('.json', '.jsonl'):
        notes = _from_json(path)
    elif suffix in ('.apkg', '.colpkg'):
        notes = _from_package(path)
    elif suffix in ('.anki2', '.anki21', '.db', '.sqlite'):
        notes = _from_sqlite_copy(path)
    else:
        raise DeckError('Use an .apkg, .colpkg, .anki2 collection, or a .json/.jsonl note list.')
    out = []
    for note in notes:
        if deck_prefix and not _under(note['deck'], deck_prefix):
            continue
        if suspended_only and not note['suspended_card_ids']:
            continue
        chosen = _choose_fields(note['fields'], fields)
        note['text'] = '\n'.join(t for t in (clean_text(v) for v in chosen) if t)
        if note['text']:
            out.append(note)
    return out


def _under(deck, prefix):
    deck = deck or ''
    return deck == prefix or deck.startswith(prefix + '::')


def _choose_fields(values, names):
    if isinstance(values, dict):
        if names:
            return [values[n] for n in names if n in values]
        return list(values.values())[:2]
    return list(values)[:2]


def _from_json(path):
    text = path.read_text(encoding='utf-8')
    if path.suffix.lower() == '.jsonl':
        rows = [json.loads(line) for line in text.splitlines() if line.strip()]
    else:
        rows = json.loads(text)
        if isinstance(rows, dict):
            rows = rows.get('notes') or rows.get('result') or []
    notes = []
    for row in rows:
        fields = row.get('fields', {})
        # Anki MCP / AnkiConnect shape: {"Text": {"value": "...", "order": 0}}
        if isinstance(fields, dict) and fields and all(isinstance(v, dict) for v in fields.values()):
            fields = {k: v.get('value', '') for k, v in sorted(fields.items(), key=lambda kv: kv[1].get('order', 0))}
        if not fields and 'text' in row:
            fields = {'Text': row['text']}
        note_id = row.get('note_id', row.get('noteId'))
        if note_id is None:
            raise DeckError('Every JSON note needs note_id (or noteId).')
        cards = [int(c) for c in row.get('card_ids', row.get('cards', []))]
        notes.append({'note_id': int(note_id), 'card_ids': cards,
                      'suspended_card_ids': [int(c) for c in row.get('suspended_card_ids', [])],
                      'deck': row.get('deck', row.get('deckName', '')) or '',
                      'notetype': row.get('notetype', row.get('modelName', '')) or '',
                      'tags': list(row.get('tags', [])), 'fields': fields})
    return notes


def _from_package(path):
    with zipfile.ZipFile(path) as z, tempfile.TemporaryDirectory() as tmp:
        names = set(z.namelist())
        target = Path(tmp) / 'collection.db'
        if 'collection.anki21b' in names:
            _write_zstd(z.read('collection.anki21b'), target)
        elif 'collection.anki21' in names:
            target.write_bytes(z.read('collection.anki21'))
        elif 'collection.anki2' in names:
            target.write_bytes(z.read('collection.anki2'))
        else:
            raise DeckError('Package has no collection file.')
        return _from_sqlite(target)


def _write_zstd(data, target):
    try:
        from compression import zstd  # Python 3.14+
        target.write_bytes(zstd.decompress(data))
        return
    except ImportError:
        pass
    try:
        import zstandard
    except ImportError as exc:
        raise DeckError('This package uses the newer compressed format. Install zstandard '
                        "(pip install 'ankiplusmcp[match]'), or export with 'Support older Anki versions' ticked.") from exc
    with open(target, 'wb') as out:
        zstandard.ZstdDecompressor().copy_stream(io.BytesIO(data), out)


def _from_sqlite_copy(path):
    # Anki may hold the live file open; read a private copy, including any WAL.
    with tempfile.TemporaryDirectory() as tmp:
        target = Path(tmp) / 'collection.db'
        shutil.copy2(path, target)
        wal = path.with_name(path.name + '-wal')
        if wal.is_file():
            shutil.copy2(wal, target.with_name(target.name + '-wal'))
        return _from_sqlite(target)


def _from_sqlite(path):
    with closing(sqlite3.connect(f'file:{path}?mode=ro', uri=True)) as db:
        tables = {r[0] for r in db.execute("select name from sqlite_master where type='table'")}
        if 'notes' not in tables or 'cards' not in tables:
            raise DeckError('Not an Anki collection (no notes/cards tables).')
        decks, models, field_names = _names(db, tables)
        cards = {}
        for cid, nid, did, queue in db.execute('select id, nid, did, queue from cards'):
            entry = cards.setdefault(nid, {'card_ids': [], 'suspended': [], 'deck': decks.get(did, '')})
            entry['card_ids'].append(cid)
            if queue == SUSPENDED:
                entry['suspended'].append(cid)
        notes = []
        for nid, mid, tags, flds in db.execute('select id, mid, tags, flds from notes'):
            info = cards.get(nid, {'card_ids': [], 'suspended': [], 'deck': ''})
            values = flds.split('\x1f')
            names = field_names.get(mid) or [f'Field{i + 1}' for i in range(len(values))]
            notes.append({'note_id': nid, 'card_ids': info['card_ids'], 'suspended_card_ids': info['suspended'],
                          'deck': info['deck'], 'notetype': models.get(mid, ''),
                          'tags': tags.split(), 'fields': dict(zip(names, values))})
        return notes


def _names(db, tables):
    decks, models, fields = {}, {}, {}
    if 'decks' in tables:  # schema 18+
        decks = {i: n.replace('\x1f', '::') for i, n in db.execute('select id, name from decks')}
    if 'notetypes' in tables:
        models = dict(db.execute('select id, name from notetypes'))
        if 'fields' in tables:
            for ntid, name in db.execute('select ntid, name from fields order by ntid, ord'):
                fields.setdefault(ntid, []).append(name)
    if not decks or not models:
        row = db.execute('select decks, models from col').fetchone()
        if row:
            if not decks and row[0]:
                decks = {int(k): v['name'] for k, v in json.loads(row[0]).items()}
            if not models and row[1]:
                for k, v in json.loads(row[1]).items():
                    models[int(k)] = v['name']
                    fields[int(k)] = [f['name'] for f in sorted(v.get('flds', []), key=lambda f: f.get('ord', 0))]
    return decks, models, fields
