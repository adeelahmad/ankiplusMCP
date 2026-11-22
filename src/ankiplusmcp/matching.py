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

    def _chunks(self, workspace, highlights):
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

    def match_source(self, workspace, index='anking', highlights=None, covered=0.6, related=0.35,
                     per_chunk=3, expand_concepts=True, unsuspend_concepts=False):
        deck = self._load_index(index)
        chunks, origin = self._chunks(workspace, highlights)
        if not chunks:
            raise DomainError('SOURCE', 'No text chunks found in the source.', 'Run extract_source for scanned PDFs (OCR), or pass highlights.')
        thresholds = match.Thresholds(covered=covered, related=related, per_chunk=per_chunk)
        report = match.match_chunks(deck, chunks, thresholds, expand_concepts=expand_concepts, unsuspend_concepts=unsuspend_concepts)
        report.update(index=index, chunk_origin=origin)
        name = (self.store.get(workspace).get('source') or {}).get('name', 'lecture')
        base = f'match/{uuid.uuid4().hex}'
        self.store.path(workspace, base).mkdir(parents=True)
        self.store.write_json(workspace, f'{base}/report.json', report)
        self.store.write_json(workspace, f'{base}/unsuspend.json', report['unsuspend'])
        from cards.common.files import atomic_write
        atomic_write(self.store.path(workspace, f'{base}/prompt.md'), match.build_prompt(report, name))
        self.store.write_json(workspace, 'latest-match.json', {'artifact': f'{base}/report.json'})
        return {'summary': report['summary'], 'chunk_origin': origin, 'embeddings': report['embeddings'],
                'artifacts': {'report': f'{base}/report.json', 'unsuspend': f'{base}/unsuspend.json', 'prompt': f'{base}/prompt.md'},
                'performed_anki_operations': 0,
                'next': 'Read the prompt artifact to draft only the gaps. Show the unsuspend list to the user before unsuspending through Anki MCP.'}

    def check_source_support(self, workspace, ids=None):
        from .match.quotes import check_support
        cards = self._select(workspace, ids)
        if not cards:
            raise DomainError('CARD', 'No cards selected.')
        chunks, origin = self._chunks(workspace, None)
        result = check_support(cards, chunks)
        result['chunk_origin'] = origin
        return result
