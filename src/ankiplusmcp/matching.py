"""Deck indexing and lecture-to-deck matching for the MCP. No Anki connection, no model calls."""
import json
from pathlib import Path
import re
import uuid
from .store import DomainError
from . import match

MAX_DECK = 4 * 1024 ** 3
NAME = re.compile(r'^[A-Za-z0-9_-]{1,80}$')


class Matching:
    def _index_dir(self, name):
        if not NAME.fullmatch(name or ''):
            raise DomainError('INDEX', 'Index names use letters, digits, _ and - (max 80).')
        return self.store.root / 'indexes' / name

    def _deck_file(self, path):
        raw = Path(path).expanduser()
        if not raw.is_absolute():
            raise DomainError('SOURCE', 'Deck path must be absolute.')
        resolved = raw.resolve()
        roots = self.store.source_roots or ()
        if not any(resolved.is_relative_to(r) for r in roots):
            raise DomainError('SOURCE', 'Deck file is outside configured readable roots.',
                              'Add its folder with --source-root, or export the deck into a source root.')
        if not resolved.is_file() or resolved.stat().st_size > MAX_DECK:
            raise DomainError('SOURCE', 'Deck file missing or larger than 4 GiB.')
        return resolved

    def _backend(self, backend):
        from .match import pgstore
        if backend == 'auto':
            backend = 'postgres' if pgstore.available() else 'file'
        if backend == 'postgres' and not pgstore.available():
            raise DomainError('DEPENDENCY', 'Embedded Postgres needs the optional packages.', "pip install 'ankiplusmcp[match]'")
        return backend

    def _index_store(self, name, backend):
        from .match import pgstore
        from .match.index import FileIndexStore
        if backend == 'postgres':
            return pgstore.PostgresIndexStore(self.store.root / 'pg', name)
        return FileIndexStore(self._index_dir(name))

    def index_deck(self, workspace, deck_path, name='anking', deck_prefix=None, fields=None, embeddings='auto', model=None, backend='auto'):
        """Build (or rebuild) a named index from a deck export. Runs as a job hosted by ``workspace``."""
        self.store.get(workspace)
        backend = self._backend(backend)
        source = self._deck_file(deck_path)
        folder = self._index_dir(name)

        def run(cancel, report):
            report('reading deck')
            try:
                notes = match.load_deck(source, deck_prefix=deck_prefix, fields=fields)
            except match.deck.DeckError as exc:
                raise DomainError('DECK', str(exc)) from exc
            if not notes:
                raise DomainError('DECK', 'No notes found (check deck_prefix).')
            if cancel(): raise DomainError('CANCELLED', 'Cancelled.')
            try:
                embedder = match.index.get_embedder(embeddings, model)
            except ValueError as exc:
                raise DomainError('DEPENDENCY', str(exc)) from exc
            report(f'indexing {len(notes)} notes')
            index = match.DeckIndex(notes, embedder=embedder,
                                    meta={'name': name, 'source': source.name, 'deck_prefix': deck_prefix, 'fields': fields})
            index.embed_all(progress=lambda done, total: report(f'embedding {done}/{total}'))
            if cancel(): raise DomainError('CANCELLED', 'Cancelled.')
            meta = index.save(folder, self._index_store(name, backend))
            # The folder's meta.json is the registry entry for every backend.
            folder.mkdir(parents=True, exist_ok=True)
            (folder / 'meta.json').write_text(json.dumps(dict(meta, backend=backend), indent=2, ensure_ascii=False) + '\n')
            self.__dict__.get('_indexes', {}).pop(name, None)
            return {'index': name, 'backend': backend, **{k: meta[k] for k in ('notes', 'suspended_cards', 'embeddings')},
                    'embedding_model': meta.get('embedding_model')}
        return self.jobs.submit(workspace, 'index_deck', run)

    def list_indexes(self):
        root = self.store.root / 'indexes'
        out = []
        for folder in sorted(root.iterdir()) if root.is_dir() else []:
            meta = folder / 'meta.json'
            if meta.is_file():
                data = json.loads(meta.read_text())
                out.append({k: data.get(k) for k in ('name', 'backend', 'source', 'deck_prefix', 'notes', 'suspended_cards', 'embeddings', 'embedding_model')})
        return {'indexes': out}

    def _load_index(self, name):
        folder = self._index_dir(name)
        if not (folder / 'meta.json').is_file():
            raise DomainError('INDEX', f'No index named {name}.', 'Build one with index_deck.')
        cache = self.__dict__.setdefault('_indexes', {})
        stamp = (folder / 'meta.json').stat().st_mtime_ns
        if name not in cache or cache[name][0] != stamp:
            meta = json.loads((folder / 'meta.json').read_text())
            embedder = None
            if meta.get('embeddings'):
                try: embedder = match.index.get_embedder('auto', meta.get('embedding_model'))
                except ValueError: embedder = None
            store = self._index_store(name, meta.get('backend', 'file'))
            cache[name] = (stamp, match.DeckIndex.load(folder, embedder=embedder, store=store))
        return cache[name][1]

    def read_highlights(self, workspace):
        manifest = self.store.get(workspace)
        path = Path(manifest['source']['path']) if manifest['source'] else None
        if not path or path.suffix != '.pdf':
            raise DomainError('SOURCE', 'Highlights are read from a PDF source.', 'Create the workspace from the highlighted PDF.')
        found = match.highlights_from_pdf(path)
        return {'highlights': found, 'count': len(found), 'text_highlights': sum(h['kind'] == 'text' for h in found)}

    def _chunks(self, workspace, highlights, pdf_highlights=False):
        if pdf_highlights:
            found = self.read_highlights(workspace)['highlights']
            if not any(h['kind'] == 'text' for h in found):
                raise DomainError('SOURCE', 'The PDF has no text highlights.', 'Highlight the facts to card in a PDF viewer, save, and recreate the workspace.')
            return match.chunks_from_highlights(found), 'pdf_highlights'
        if highlights:
            return match.chunks_from_highlights(highlights), 'highlights'
        manifest = self.store.get(workspace)
        if self.published(workspace):
            _, data, _ = self.source_data(workspace)
            slides = data['slides']
            path = Path(manifest['source']['path']) if manifest['source'] else None
            broken = [s['n'] for s in slides if match.chunks.looks_garbled(s.get('text', ''))]
            if broken and path and path.suffix == '.pdf':
                # A broken font encoding passes extraction as garbage; OCR those pages instead.
                fixed = {s['n']: s for s in match.chunks.pdf_pages(path, pages=set(broken))}
                slides = [fixed.get(s['n'], s) for s in slides]
            return match.chunks_from_slides(slides), 'extraction'
        if not manifest['source']:
            raise DomainError('SOURCE', 'Workspace has no source.', 'Create the workspace with a source, or pass highlights.')
        path = Path(manifest['source']['path'])
        if path.suffix == '.pdf':
            return match.chunks_from_pdf(path), 'pdf_text'
        return match.chunks_from_text(path.read_text(encoding='utf-8', errors='replace')), 'text'

    def match_source(self, workspace, index='anking', highlights=None, covered=None, related=None,
                     per_chunk=3, expand_concepts=True, unsuspend_concepts=False, pdf_highlights=False):
        deck = self._load_index(index)
        chunks, origin = self._chunks(workspace, highlights, pdf_highlights)
        if not chunks:
            raise DomainError('SOURCE', 'No text chunks found in the source.', 'Run extract_source for scanned PDFs (OCR), or pass highlights.')
        thresholds = match.Thresholds(covered=covered, related=related, per_chunk=per_chunk)
        report = match.match_chunks(deck, chunks, thresholds, expand_concepts=expand_concepts, unsuspend_concepts=unsuspend_concepts,
                                    already_unsuspended=self._unsuspended(index))
        match_id = uuid.uuid4().hex
        report.update(index=index, chunk_origin=origin, match_id=match_id)
        name = (self.store.get(workspace).get('source') or {}).get('name', 'lecture')
        base = f'match/{match_id}'
        self.store.path(workspace, base).mkdir(parents=True)
        self.store.write_json(workspace, f'{base}/report.json', report)
        self.store.write_json(workspace, f'{base}/unsuspend.json', report['unsuspend'])
        from cards.common.files import atomic_write
        atomic_write(self.store.path(workspace, f'{base}/prompt.md'), match.build_prompt(report, name))
        self.store.write_json(workspace, 'latest-match.json', {'artifact': f'{base}/report.json'})
        return {'match_id': match_id, 'summary': report['summary'], 'chunk_origin': origin, 'embeddings': report['embeddings'],
                'artifacts': {'report': f'{base}/report.json', 'unsuspend': f'{base}/unsuspend.json', 'prompt': f'{base}/prompt.md'},
                'performed_anki_operations': 0,
                'next': 'Read the prompt artifact to draft only the gaps. Call prepare_unsuspend to show the user the cards, unsuspend the approved CARD ids through Anki MCP, then record_unsuspend.'}

    # -- unsuspend: preview -> user approval -> Anki MCP -> log -------------
    def _unsuspended_path(self, index):
        return self._index_dir(index) / 'unsuspended.jsonl'

    def _unsuspended(self, index):
        path = self._unsuspended_path(index)
        done = set()
        if path.is_file():
            for line in path.read_text().splitlines():
                try: row = json.loads(line)
                except ValueError: continue
                if row.get('status') == 'success':
                    done.update(row.get('card_ids', []))
        return done

    def _match_id(self, workspace, match_id):
        if match_id is None:
            match_id = self.store.read_json(workspace, 'latest-match.json')['artifact'].split('/')[1]
        if not NAME.fullmatch(match_id):
            raise DomainError('MATCH', 'Invalid match ID.')
        return match_id

    def prepare_unsuspend(self, workspace, match_id=None, exclude_note_ids=None, offset=0, limit=50):
        """The list to show the user: each note with the lecture lines it matched. Writes the approved CARD ids."""
        match_id = self._match_id(workspace, match_id)
        report = self.store.read_json(workspace, f'match/{match_id}/report.json')
        texts = {r['id']: f"p{r['page']}: {r['text'][:120]}" for r in report['chunks']}
        excluded = set(exclude_note_ids or [])
        done = self._unsuspended(report.get('index', 'anking'))
        notes = []
        for entry in report['unsuspend']['notes']:
            if entry['note_id'] in excluded:
                continue
            cards = [c for c in entry['card_ids'] if c not in done]
            if cards:
                notes.append({'note_id': entry['note_id'], 'card_ids': cards, 'reason': entry['reason'],
                              'card': entry['preview'], 'lecture': [texts[c] for c in entry['chunks'] if c in texts][:3]})
        card_ids = sorted({c for n in notes for c in n['card_ids']})
        artifact = f'match/{match_id}/unsuspend-approved.json'
        self.store.write_json(workspace, artifact, {'match_id': match_id, 'card_ids': card_ids, 'note_ids': [n['note_id'] for n in notes],
                                                    'excluded_note_ids': sorted(excluded), 'performed_anki_operations': 0})
        return {'match_id': match_id, 'cards': len(card_ids), 'notes': notes[offset:offset + limit], 'total_notes': len(notes),
                'card_ids': card_ids, 'artifact': artifact,
                'instructions': 'Show these notes to the user. Re-run with exclude_note_ids for any they reject. '
                                'After approval, unsuspend card_ids with the Anki MCP, then call record_unsuspend.'}

    def record_unsuspend(self, workspace, card_ids, status, match_id=None, detail=''):
        """Log what the host reports it unsuspended, so later lectures never propose those cards again."""
        from datetime import datetime, timezone
        match_id = self._match_id(workspace, match_id)
        report = self.store.read_json(workspace, f'match/{match_id}/report.json')
        row = {'time': datetime.now(timezone.utc).isoformat(timespec='seconds'), 'workspace': workspace, 'match_id': match_id,
               'source': (self.store.get(workspace).get('source') or {}).get('name'), 'status': status,
               'card_ids': sorted(set(card_ids)), 'detail': detail[:500], 'evidence_origin': 'caller_supplied'}
        line = json.dumps(row) + '\n'
        with self.store.lock(workspace):
            with open(self.store.path(workspace, f'match/{match_id}/unsuspend-log.jsonl'), 'a') as out:
                out.write(line)
        log = self._unsuspended_path(report.get('index', 'anking'))
        log.parent.mkdir(parents=True, exist_ok=True)
        with open(log, 'a') as out:
            out.write(line)
        return {'recorded': len(row['card_ids']), 'status': status, 'log': f'match/{match_id}/unsuspend-log.jsonl',
                'evidence_origin': 'caller_supplied', 'independently_verified': False}

    def check_source_support(self, workspace, ids=None):
        from .match.quotes import check_support
        cards = self._select(workspace, ids)
        if not cards:
            raise DomainError('CARD', 'No cards selected.')
        chunks, origin = self._chunks(workspace, None)
        result = check_support(cards, chunks)
        result['chunk_origin'] = origin
        return result
