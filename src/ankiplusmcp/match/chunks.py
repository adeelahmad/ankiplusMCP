"""Split lecture content into fact-sized chunks for matching.

A chunk is ``{'id', 'page', 'title', 'text'}``. Bullets and lines become chunks;
lines too short to carry a fact are merged into their neighbour.
"""
import re
from .index import tokenize

MIN_TOKENS = 4
MAX_CHARS = 400
_BULLET = re.compile(r'^\s*(?:[-*•▪●–—>o]|\d{1,2}[.)])\s+')


def _lines(text):
    for raw in (text or '').splitlines():
        line = _BULLET.sub('', raw).strip()
        if line:
            yield line


def _split(page, title, text):
    pieces = []
    for line in _lines(text):
        if title and line.strip() == title.strip():
            continue
        if pieces and (len(tokenize(pieces[-1])) < MIN_TOKENS or len(tokenize(line)) < MIN_TOKENS) \
                and len(pieces[-1]) + len(line) < MAX_CHARS:
            pieces[-1] = pieces[-1] + ' ' + line
        else:
            pieces.append(line)
    out = []
    for piece in pieces:
        if len(tokenize(piece)) >= 2:
            out.append({'id': f'p{page}-{len(out) + 1}', 'page': page, 'title': title or '', 'text': piece[:MAX_CHARS * 2]})
    return out


def chunks_from_slides(slides):
    """Use an AnkiPlus extraction (``slides.json`` ``slides`` list: n, title, text)."""
    out = []
    for slide in slides:
        out.extend(_split(slide['n'], slide.get('title', ''), slide.get('text', '')))
    return out


def chunks_from_pdf(path, pages=None):
    import pymupdf
    slides = []
    with pymupdf.open(path) as pdf:
        for number, page in enumerate(pdf, start=1):
            if pages and number not in pages:
                continue
            text = page.get_text()
            first = next(_lines(text), '')
            slides.append({'n': number, 'title': first[:100], 'text': text})
    return chunks_from_slides(slides)


def chunks_from_text(text):
    """Plain text or Markdown: each '#' heading starts a new section (used as the page number)."""
    slides, title, body = [], '', []
    for line in (text or '').splitlines():
        if line.lstrip().startswith('#'):
            if body:
                slides.append({'n': len(slides) + 1, 'title': title, 'text': '\n'.join(body)})
            title, body = line.lstrip('# ').strip(), []
        else:
            body.append(line)
    if body:
        slides.append({'n': len(slides) + 1, 'title': title, 'text': '\n'.join(body)})
    return chunks_from_slides(slides)


def chunks_from_highlights(highlights):
    """User highlights, e.g. from a PDF viewer: ``[{'page': 3, 'text': '...', 'note': '...'}]``.

    Each highlight is kept whole as one chunk: the user already chose what matters.
    """
    out = []
    for i, item in enumerate(highlights, start=1):
        text = ' '.join(_lines(item.get('text', '')))
        if not text:
            continue
        page = int(item.get('page') or 0)
        out.append({'id': f'h{i}', 'page': page, 'title': item.get('note', '') or '', 'text': text})
    return out
