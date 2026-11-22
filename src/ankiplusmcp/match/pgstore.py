"""Embedded Postgres + pgvector index storage (optional ``pixeltable-pgserver``).

The server runs from a private data folder; no system Postgres, no network port
(Unix socket only). One schema per index: ``notes`` rows carry text, tags, card IDs
and the pgvector embedding (HNSW cosine index). Matching still scores in process,
so results are identical to the file store; the database is the durable store a
desktop shell (e.g. Tauri sidecar) and future multi-deck/AnkiHub updates share.
"""
import json
from pathlib import Path
import re
import threading

import numpy as np

from .index import IndexStore, FORMAT

_servers = {}
_lock = threading.Lock()


def available():
    try:
        import pixeltable_pgserver  # noqa: F401
        import psycopg  # noqa: F401
        return True
    except ImportError:
        return False


def server(data_dir):
    """One embedded server per data folder, started on first use."""
    import pixeltable_pgserver
    key = str(Path(data_dir).resolve())
    with _lock:
        if key not in _servers:
            Path(key).mkdir(parents=True, exist_ok=True)
            _servers[key] = pixeltable_pgserver.get_server(key, cleanup_mode='stop')
        return _servers[key]


def shutdown():
    with _lock:
        for srv in _servers.values():
            try: srv.cleanup()
            except Exception: pass
        _servers.clear()


def log_tail(data_dir, lines=40):
    """Last lines of the embedded server's log, to explain a crash (e.g. a backend killed by a signal)."""
    try:
        log = Path(server(data_dir).log)
        return '\n'.join(log.read_text(errors='replace').splitlines()[-lines:]) if log.is_file() else '(no server log)'
    except Exception as exc:  # the server itself may be gone
        return f'(server log unavailable: {exc})'


def _explain(exc, data_dir):
    exc.add_note('Embedded Postgres server log (tail):\n' + log_tail(data_dir))
    return exc


class PostgresIndexStore(IndexStore):
    def __init__(self, data_dir, name):
        if not re.fullmatch(r'[A-Za-z0-9_]{1,60}', name.replace('-', '_')):
            raise ValueError('Invalid index name.')
        self.data_dir = Path(data_dir)
        self.schema = 'idx_' + name.replace('-', '_').lower()

    def _connect(self):
        import psycopg
        return psycopg.connect(server(self.data_dir).get_uri(), autocommit=False)

    def save(self, index):
        import psycopg
        try:
            return self._save(index)
        except psycopg.Error as exc:
            raise _explain(exc, self.data_dir)

    def load(self, cls, embedder=None):
        import psycopg
        try:
            return self._load(cls, embedder)
        except psycopg.Error as exc:
            raise _explain(exc, self.data_dir)

    def _save(self, index):
        dim = None if index.vectors is None else int(index.vectors.shape[1])
        with self._connect() as db, db.cursor() as cur:
            cur.execute('create extension if not exists vector')
            cur.execute(f'drop schema if exists {self.schema} cascade')
            cur.execute(f'create schema {self.schema}')
            vec = f', embedding vector({dim})' if dim else ''
            cur.execute(f'''create table {self.schema}.notes (doc integer primary key, note_id bigint not null,
                card_ids bigint[] not null, suspended_card_ids bigint[] not null, deck text, tags text[],
                text text not null, front text, tsv tsvector{vec})''')
            cur.execute(f'create table {self.schema}.meta (key text primary key, value jsonb)')
            columns = 'doc, note_id, card_ids, suspended_card_ids, deck, tags, text, front' + (', embedding' if dim else '')
            with cur.copy(f'copy {self.schema}.notes ({columns}) from stdin') as copy:
                for i, n in enumerate(index.notes):
                    row = [i, n['note_id'], n['card_ids'], n['suspended_card_ids'], n.get('deck', ''), n.get('tags', []), n['text'], n.get('front')]
                    if dim:
                        row.append('[' + ','.join(f'{x:.6g}' for x in index.vectors[i]) + ']')
                    copy.write_row(row)
            cur.execute(f"update {self.schema}.notes set tsv = to_tsvector('english', text)")
            cur.execute(f'create index on {self.schema}.notes using gin (tsv)')
            if dim:
                cur.execute(f'create index on {self.schema}.notes using hnsw (embedding vector_cosine_ops)')
            meta = dict(index.meta, format=FORMAT, backend='postgres', notes=len(index.notes),
                        suspended_cards=sum(len(n['suspended_card_ids']) for n in index.notes),
                        embeddings=dim is not None)
            if not dim:
                meta.pop('embedding_model', None)
            cur.execute(f'insert into {self.schema}.meta values (%s, %s)', ('meta', json.dumps(meta)))
        return meta

    def _load(self, cls, embedder=None):
        with self._connect() as db, db.cursor() as cur:
            cur.execute(f"select value from {self.schema}.meta where key='meta'")
            meta = cur.fetchone()[0]
            if meta.get('format') != FORMAT:
                raise ValueError('Index format changed; rebuild it with index_deck.')
            vec = ', embedding::text' if meta.get('embeddings') else ''
            cur.execute(f'select note_id, card_ids, suspended_card_ids, deck, tags, text, front{vec} from {self.schema}.notes order by doc')
            rows = cur.fetchall()
        notes = [{'note_id': r[0], 'card_ids': list(r[1]), 'suspended_card_ids': list(r[2]), 'deck': r[3] or '',
                  'tags': list(r[4] or []), 'text': r[5], 'front': r[6]} for r in rows]
        index = cls(notes, embedder=embedder, meta=meta)
        if vec and embedder is not None and embedder.name == meta.get('embedding_model'):
            index.vectors = np.array([json.loads(r[7]) for r in rows], dtype=np.float32)
        return index
