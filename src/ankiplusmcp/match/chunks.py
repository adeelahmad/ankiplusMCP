"""Split lecture content into fact-sized chunks for matching.

A chunk is ``{'id', 'page', 'title', 'text'}``. Bullets and lines become chunks;
lines too short to carry a fact are merged into their neighbour.
"""
import re
from .index import tokenize

MIN_TOKENS = 4
MAX_CHARS = 400
# '¢', '©', 'e' and '«' are how OCR commonly reads bullet glyphs.
_BULLET = re.compile(r'^\s*(?:[-*•▪●–—>o¢©«‣◦]|e(?=\s+[A-Z])|\d{1,2}[.)])\s+')
_INLINE_BULLET = re.compile(r'\s+[•¢▪●]\s+')
_CAPTION = re.compile(r'^(?:Figure|Fig\.|Image|Source)\s*\d*\s*[:.]', re.I)
_CITATION = re.compile(r'\b(?:ed\.|edn|editors?|Elsevier|Wolters|Kluwer|Springer|Saunders|McGraw|Thieme|Lippincott)\b.*\b(?:19|20)\d{2}\b|\b(?:19|20)\d{2}\b.*\b(?:Elsevier|Wolters|Kluwer|Springer|Saunders|Lippincott)\b', re.I)
_LIST_NUMBER = re.compile(r'(?<=[A-Za-z:)])\s+\d{1,2}$')
_CONTINUES = re.compile(r'[,(\-–/]$|\b(?:and|or|of|the|a|an|to|in|with|by|for|from|as|at|on|is|are)$', re.I)


def _lines(text):
    """Logical lines: bullets stripped, wrapped sentences re-joined, trailing list numbers dropped."""
    current = None
    for raw in (text or '').splitlines():
        for part in _INLINE_BULLET.split(raw):
            bulleted = bool(_BULLET.match(part))
            line = _LIST_NUMBER.sub('', _BULLET.sub('', part).strip())
            if not re.search(r'[A-Za-z]{2}', line) or _CAPTION.match(line) or _CITATION.search(line):
                continue   # page numbers, list numbers, figure credits, references
            if current and not bulleted and (_CONTINUES.search(current) or line[0].islower() or line[0] in '(['
                                            or current.count('(') > current.count(')')):
                current += ' ' + line
                continue
            if current:
                yield current
            current = line
    if current:
        yield current


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
    """Use an AnkiPlus extraction (``slides.json`` ``slides`` list: n, title, text).

    Lines repeated on many pages (headers, footers, college names) are dropped first.
    """
    from collections import Counter
    seen = Counter(line for slide in slides for line in set(_lines(slide.get('text', ''))))
    limit = max(3, len(slides) * .3)
    boiler = {line for line, n in seen.items() if n >= limit}
    out = []
    for slide in slides:
        text = '\n'.join(l for l in _lines(slide.get('text', '')) if l not in boiler)
        out.extend(_split(slide['n'], slide.get('title', ''), text))
    return out


_OK = set('abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 \n\t.,;:()[]-–—/\'"?!%+&·•→<>=°±×*#@$')


def looks_garbled(text):
    """True when a PDF text layer is unusable: empty, or a broken font encoding
    (e.g. shifted glyph codes that turn 'The' into '7KH')."""
    text = (text or '').strip()
    if len(text) < 20:
        return True
    printable = sum(c in _OK for c in text) / len(text)
    words = re.findall(r'[A-Za-z]{4,}', text)
    if not words:
        return printable < .9
    vowelled = sum(bool(re.search(r'[aeiouyAEIOUY]', w)) for w in words) / len(words)
    return printable < .9 or vowelled < .7


def ocr_page(page, zoom=3):
    """OCR one PDF page with the system tesseract (same program the extractor uses)."""
    import shutil, subprocess, tempfile, os
    if not shutil.which('tesseract'):
        return ''
    pix = page.get_pixmap(matrix=__import__('pymupdf').Matrix(zoom, zoom), alpha=False)
    with tempfile.TemporaryDirectory() as tmp:
        image = os.path.join(tmp, 'page.png')
        pix.save(image)
        run = subprocess.run(['tesseract', image, 'stdout', '--psm', '3', '-l', 'eng'], capture_output=True, text=True, timeout=120)
    return run.stdout if run.returncode == 0 else ''


def pdf_pages(path, pages=None, ocr=True):
    """Yield slide dicts with a usable text layer, OCR-ing pages whose text is garbled or empty."""
    import pymupdf
    with pymupdf.open(path) as pdf:
        for number, page in enumerate(pdf, start=1):
            if pages and number not in pages:
                continue
            text, method = page.get_text(), 'text'
            if ocr and looks_garbled(text):
                recovered = ocr_page(page)
                text, method = (recovered, 'ocr') if recovered.strip() else ('', 'unreadable')
            first = next(_lines(text), '')
            yield {'n': number, 'title': first[:100], 'text': text, 'method': method}


def chunks_from_pdf(path, pages=None, ocr=True):
    return chunks_from_slides(list(pdf_pages(path, pages, ocr)))


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
