"""Hybrid deck index: BM25 over note text and tag names, plus optional dense embeddings.

Scores returned per hit:
- ``bm25``: raw Okapi BM25 (ranking only, not comparable across queries),
- ``overlap``: IDF-weighted share of the query's terms that the note contains (0..1),
- ``reverse``: IDF-weighted share of the note's first field (the fact itself, not the
  Extra) that the query contains (0..1),
- ``lexical``: geometric mean of ``overlap`` and ``reverse``. Both directions are needed on
  a real deck: long AnKing Extras contain most words of any short lecture line,
- ``cosine``: embedding similarity (0..1) when embeddings are available, else None,
- ``score``: the value thresholds use: ``lexical``, raised to the mean of
  ``lexical`` and ``cosine`` when embeddings exist and that is higher.
Numbers weigh a quarter of a word, so slide numbering and table cells don't match arithmetic cards,
and ``lexical`` is halved unless the note's first field shares at least two words with the query.
Candidates from both retrievers are merged by reciprocal rank fusion.
"""
from collections import Counter
import gzip
import json
import math
from pathlib import Path
import re

import numpy as np

FORMAT = 2
_WORD = re.compile(r'[a-z0-9]+')
STOPWORDS = frozenset('''a an and are as at be been but by can could did do does for from had has have
how if in into is it its may more most no not of on or such than that the their them then there these
they this those to was were what when where which while who whom why will with would also other only
very via per each both all any some ie eg vs use used using called known'''.split())


def stem(word):
    if len(word) > 4 and word.endswith('ies'):
        return word[:-3] + 'y'
    if len(word) > 3 and word.endswith('s') and not word.endswith(('ss', 'us', 'is')):
        return word[:-1]
    return word


def tokenize(text):
    out = []
    for word in _WORD.findall((text or '').lower()):
        if word in STOPWORDS or (len(word) < 2 and not word.isdigit()):
            continue
        out.append(stem(word))
    return out


def concept_tags(tags):
    """Topic tags only: drops question-bank IDs (numeric leaves) and media/meta tags."""
    keep = []
    for tag in tags:
        parts = [p for p in tag.split('::') if p]
        if not parts or parts[-1].lstrip('#').isdigit():
            continue
        if any(p.startswith(('!', '$')) for p in parts):
            continue
        keep.append(tag)
    return keep


def tag_words(tags):
    words = []
    for tag in concept_tags(tags):
        parts = tag.split('::')[-2:]
        words.extend(tokenize(' '.join(parts).replace('_', ' ').replace('-', ' ')))
    return words


class DeckIndex:
    def __init__(self, notes, embedder=None, k1=1.5, b=0.75, meta=None):
        self.notes = notes
        self.k1, self.b = k1, b
        self.meta = meta or {}
        self.embedder = embedder
        self.vectors = None
        self._build_bm25()
        self._build_tags()

    # -- construction -------------------------------------------------------
    def _build_bm25(self):
        self.postings = {}
        self.lengths = []
        for i, note in enumerate(self.notes):
            counts = Counter(tokenize(note['text']) + tag_words(note.get('tags', [])))
            self.lengths.append(sum(counts.values()))
            for term, tf in counts.items():
                self.postings.setdefault(term, []).append((i, tf))
        n = max(len(self.notes), 1)
        self.avgdl = (sum(self.lengths) / n) or 1.0
        self.idf = {t: math.log(1 + (n - len(p) + .5) / (len(p) + .5)) for t, p in self.postings.items()}
        self.unseen = math.log(1 + (n + .5) / .5)
        self.fronts = []
        for note in self.notes:
            front = set(tokenize(note.get('front') or note['text']))
            self.fronts.append((front, sum(self.weight(t) for t in front)))

    def weight(self, term):
        w = self.idf.get(term, self.unseen)
        return w * 0.25 if term.isdigit() else w

    def _build_tags(self):
        self.by_tag = {}
        for i, note in enumerate(self.notes):
            for tag in concept_tags(note.get('tags', [])):
                self.by_tag.setdefault(tag, []).append(i)

    def embed_all(self, batch=256, progress=None):
        if self.embedder is None:
            return
        parts = []
        for start in range(0, len(self.notes), batch):
            parts.append(self.embedder.embed_documents([n['text'] for n in self.notes[start:start + batch]]))
            if progress:
                progress(min(start + batch, len(self.notes)), len(self.notes))
        self.vectors = _normalise(np.vstack(parts)) if parts else np.zeros((0, 1), dtype=np.float32)
        self.meta['embedding_model'] = self.embedder.name

    # -- search -------------------------------------------------------------
    def search(self, text, k=10, pool=50):
        terms = Counter(tokenize(text))
        if not terms:
            return []
        # Terms the deck never uses still count (at maximum weight): a fact about them is not covered.
        weights = {t: self.weight(t) for t in terms}
        q_idf = sum(weights.values())
        bm25 = {}
        present = {}
        for term in terms:
            idf = self.idf.get(term)
            if idf is None:
                continue
            for doc, tf in self.postings[term]:
                norm = tf * (self.k1 + 1) / (tf + self.k1 * (1 - self.b + self.b * self.lengths[doc] / self.avgdl))
                bm25[doc] = bm25.get(doc, 0.0) + idf * norm
                present[doc] = present.get(doc, 0.0) + weights[term]
        lexical = sorted(bm25, key=bm25.get, reverse=True)[:pool]
        # Also keep the notes that share the most query weight: BM25 favours short notes,
        # and the coverage score below needs the long ones too.
        taken = set(lexical)
        lexical += [d for d in sorted(present, key=present.get, reverse=True)[:pool * 4] if d not in taken]
        cos = None
        dense = []
        if self.vectors is not None and self.embedder is not None and len(self.notes):
            query = _normalise(np.asarray([self.embedder.embed_query(text)]))[0]
            cos = self.vectors @ query
            dense = list(np.argsort(-cos)[:pool])
        fused = {}
        for ranks in (lexical, dense):
            for rank, doc in enumerate(ranks):
                fused[int(doc)] = fused.get(int(doc), 0.0) + 1.0 / (60 + rank)
        hits = []
        for doc in fused:
            overlap = present.get(doc, 0.0) / q_idf if q_idf else 0.0
            front, mass = self.fronts[doc]
            reverse = sum(self.weight(t) for t in front if t in terms) / mass if mass else 0.0
            lex = math.sqrt(overlap * reverse)
            if sum(1 for t in front if t in terms and not t.isdigit()) < 2:
                lex /= 2  # one shared word (or only numbers) is a coincidence, not the same fact
            cosine = None if cos is None else max(0.0, float(cos[doc]))
            score = lex if cosine is None else max(lex, (lex + cosine) / 2)
            hits.append({'doc': doc, 'score': round(score, 4), 'overlap': round(overlap, 4),
                         'reverse': round(reverse, 4), 'lexical': round(lex, 4),
                         'cosine': None if cosine is None else round(cosine, 4),
                         'bm25': round(bm25.get(doc, 0.0), 4), 'rrf': fused[doc]})
        hits.sort(key=lambda h: (h['score'], h['rrf']), reverse=True)
        return hits[:k]

    def neighbours(self, doc, max_group=150):
        """Notes sharing the most specific topic tag of ``doc`` (concept mapping)."""
        tags = sorted(concept_tags(self.notes[doc].get('tags', [])), key=lambda t: t.count('::'), reverse=True)
        for tag in tags:
            group = self.by_tag.get(tag, [])
            if 1 < len(group) <= max_group:
                return tag, [d for d in group if d != doc]
        return None, []

    # -- persistence (delegated to an IndexStore) ---------------------------
    def save(self, folder, store=None):
        return (store or FileIndexStore(folder)).save(self)

    @classmethod
    def load(cls, folder, embedder=None, store=None):
        return (store or FileIndexStore(folder)).load(cls, embedder)


class IndexStore:
    """Where an index lives. Swap in SQLite (FTS5 + sqlite-vec) or Postgres later
    without touching matching code: implement ``save(index) -> meta`` and
    ``load(cls, embedder) -> DeckIndex``."""
    def save(self, index):
        raise NotImplementedError

    def load(self, cls, embedder=None):
        raise NotImplementedError


class FileIndexStore(IndexStore):
    """Default: a folder with notes.jsonl.gz, meta.json and optional embeddings.npy (float16)."""
    KEYS = ('note_id', 'card_ids', 'suspended_card_ids', 'deck', 'tags', 'text', 'front')

    def __init__(self, folder):
        self.folder = Path(folder)

    def save(self, index):
        self.folder.mkdir(parents=True, exist_ok=True)
        with gzip.open(self.folder / 'notes.jsonl.gz', 'wt', encoding='utf-8') as out:
            for note in index.notes:
                out.write(json.dumps({k: note.get(k) for k in self.KEYS}, ensure_ascii=False) + '\n')
        vectors = self.folder / 'embeddings.npy'
        if index.vectors is not None:
            np.save(vectors, index.vectors.astype(np.float16))
        else:
            vectors.unlink(missing_ok=True)
        meta = dict(index.meta, format=FORMAT, notes=len(index.notes),
                    suspended_cards=sum(len(n['suspended_card_ids']) for n in index.notes),
                    embeddings=index.vectors is not None)
        if index.vectors is None:
            meta.pop('embedding_model', None)
        (self.folder / 'meta.json').write_text(json.dumps(meta, indent=2, ensure_ascii=False) + '\n')
        return meta

    def load(self, cls, embedder=None):
        meta = json.loads((self.folder / 'meta.json').read_text())
        if meta.get('format') != FORMAT:
            raise ValueError('Index format changed; rebuild it with index_deck.')
        with gzip.open(self.folder / 'notes.jsonl.gz', 'rt', encoding='utf-8') as src:
            notes = [json.loads(line) for line in src]
        index = cls(notes, embedder=embedder, meta=meta)
        vectors = self.folder / 'embeddings.npy'
        if vectors.is_file() and embedder is not None and embedder.name == meta.get('embedding_model'):
            index.vectors = np.load(vectors).astype(np.float32)
        return index


def _normalise(matrix):
    matrix = np.asarray(matrix, dtype=np.float32)
    norms = np.linalg.norm(matrix, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    return matrix / norms


class FastEmbedder:
    """Local ONNX embeddings through the optional ``fastembed`` package (no API calls).

    Nomic models want task prefixes and support Matryoshka truncation: vectors are
    layer-normed, cut to ``dim`` and re-normalised (smaller index, little recall loss).
    """
    DEFAULT = 'nomic-ai/nomic-embed-text-v1.5'
    MAX_CHARS = 800
    PREFIXES = {'nomic': ('search_document: ', 'search_query: ')}

    def __init__(self, model=None, dim=256):
        import os
        # Some networks allow huggingface.co but block its Xet storage, and the Xet client
        # retries for minutes before failing. Plain HTTPS works everywhere; set it to 0 to opt in.
        os.environ.setdefault('HF_HUB_DISABLE_XET', '1')
        from fastembed import TextEmbedding
        self.model_name = model or self.DEFAULT
        self.dim = dim if 'nomic' in self.model_name.lower() else None
        self.doc_prefix, self.query_prefix = next((v for k, v in self.PREFIXES.items() if k in self.model_name.lower()), ('', ''))
        self.name = self.model_name + (f'@{self.dim}' if self.dim else '')
        self._model = TextEmbedding(self.model_name)

    def _encode(self, texts):
        # Long AnKing Extras pad a whole batch to the model's 8k context and exhaust RAM:
        # embed the first ~200 tokens (the fact itself; Extras add little) in small batches.
        texts = [t[:self.MAX_CHARS] for t in texts]
        out = np.array(list(self._model.embed(texts, batch_size=16)), dtype=np.float32)
        if self.dim:
            out = (out - out.mean(axis=1, keepdims=True)) / (out.std(axis=1, keepdims=True) + 1e-5)
            out = out[:, :self.dim]
        return _normalise(out)

    def embed_documents(self, texts):
        return self._encode(self.doc_prefix + t for t in texts)

    def embed_query(self, text):
        return self._encode([self.query_prefix + text])[0]


def get_embedder(mode='auto', model=None):
    """'off' -> None; 'auto' -> fastembed when installed, else None; 'required' -> fastembed or error."""
    if mode == 'off':
        return None
    try:
        return FastEmbedder(model.split('@')[0] if model else None)
    except ImportError:
        if mode == 'required':
            raise ValueError("Embeddings need the optional fastembed package: pip install 'ankiplusmcp[match]'.")
        return None
    except Exception as exc:  # model download blocked or corrupt: BM25 still works
        if mode == 'required':
            raise ValueError(f'Embedding model could not be loaded ({exc}). It downloads once from Hugging Face; check the network.')
        return None
