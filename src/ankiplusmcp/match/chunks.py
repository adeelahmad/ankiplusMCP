"""Split lecture content into fact-sized chunks for matching.

A chunk is ``{'id', 'page', 'title', 'text'}``. Bullets and lines become chunks;
lines too short to carry a fact are merged into their neighbour.
"""
import re
from .index import tokenize

MIN_TOKENS = 4
MAX_CHARS = 400
MAX_LINES = 3   # a heading and its short bullets, not a whole column of them
# '¢', '©', 'e' and '«' are how OCR commonly reads bullet glyphs.
_BULLET = re.compile(r'^\s*(?:[-*•▪●–—>o¢©«‣◦]|e(?=\s+[A-Z])|\d{1,2}[.)])\s+')
_INLINE_BULLET = re.compile(r'\s+[•¢▪●]\s+')
_CAPTION = re.compile(r'^(?:Figure|Fig\.|Image|Source)\s*\d*\s*[:.]', re.I)
_CITATION = re.compile(r'\b(?:ed\.|edn|editors?|Elsevier|Wolters|Kluwer|Springer|Saunders|McGraw|Thieme|Lippincott)\b.*\b(?:19|20)\d{2}\b|\b(?:19|20)\d{2}\b.*\b(?:Elsevier|Wolters|Kluwer|Springer|Saunders|Lippincott)\b', re.I)
_LIST_NUMBER = re.compile(r'(?<=[A-Za-z:)])\s+\d{1,2}$')
_CONTINUES = re.compile(r'[,(\-–/]$|\b(?:and|or|of|the|a|an|to|in|with|by|for|from|as|at|on|is|are)$', re.I)


def _lines(text):
    """Logical lines: bullets stripped, wrapped sentences re-joined, trailing list numbers dropped."""
    return (line for line, _ in _marked(text))


def _marked(text):
    """``_lines`` with, for each line, whether it was a bullet."""
    current, mark = None, False
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
                yield current, mark
            current, mark = line, bulleted
    if current:
        yield current, mark


def _split(page, title, text, skip=()):
    pieces, joined, after_bullet = [], [], False
    contexts, lead = [], ''   # the heading a piece sits under ("Sialadenitis" for "Commonly due to mumps")
    for line, bulleted in _marked(text):
        if line in skip or (title and line.strip() == title.strip()):
            continue
        short = len(tokenize(line)) < MIN_TOKENS
        heading = not bulleted and (after_bullet or line.endswith(':'))   # starts the next group
        after_bullet = bulleted
        if pieces and not heading and (len(tokenize(pieces[-1])) < MIN_TOKENS or short) \
                and len(pieces[-1]) + len(line) < MAX_CHARS and joined[-1] < MAX_LINES:
            pieces[-1] = pieces[-1] + ' ' + line
            joined[-1] += 1
        else:
            if not bulleted and len(tokenize(line)) <= 6:
                lead = line.rstrip(':')
            contexts.append('' if (not bulleted and lead == line.rstrip(':')) else lead)
            pieces.append(line)
            joined.append(1)
    out = []
    for piece, context in zip(pieces, contexts):
        if len(tokenize(piece)) >= 2:
            chunk = {'id': f'p{page}-{len(out) + 1}', 'page': page, 'title': title or '', 'text': piece[:MAX_CHARS * 2]}
            if context and context.lower() not in piece.lower():
                chunk['context'] = context
            out.append(chunk)
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
        out.extend(_split(slide['n'], slide.get('title', ''), slide.get('text', ''), boiler))
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
    """OCR one PDF page with the system tesseract (same program the extractor uses).

    The page is rendered in grey (tesseract drops coloured headings such as red or
    purple slide titles from a colour image) and read column by column."""
    import shutil, subprocess, tempfile, os
    if not shutil.which('tesseract'):
        return ''
    import pymupdf
    pix = page.get_pixmap(matrix=pymupdf.Matrix(zoom, zoom), alpha=False, colorspace=pymupdf.csGRAY)
    with tempfile.TemporaryDirectory() as tmp:
        image = os.path.join(tmp, 'page.png')
        pix.save(image)
        run = subprocess.run(['tesseract', image, 'stdout', '--psm', '3', '-l', 'eng', 'tsv'], capture_output=True, text=True, timeout=120)
    return ocr_layout(run.stdout) if run.returncode == 0 else ''


def _segments(words):
    """Words of one tesseract line, split where the gap is wider than 2.5 line heights."""
    words.sort()
    h = max(1, sorted(w[3] for w in words)[len(words) // 2])
    segs, seg = [], [words[0]]
    for w in words[1:]:
        if w[0] - (seg[-1][0] + seg[-1][2]) > 2.5 * h:
            segs.append(seg); seg = []
        seg.append(w)
    segs.append(seg)
    return [{'left': s[0][0], 'right': s[-1][0] + s[-1][2], 'top': min(w[1] for w in s),
             'text': ' '.join(w[4] for w in s)} for s in segs]


def _gutters(segs):
    """x positions in the middle of a block that no narrow segment crosses."""
    lo, hi = min(s['left'] for s in segs), max(s['right'] for s in segs)
    span = hi - lo
    narrow = [s for s in segs if s['right'] - s['left'] <= span * .5]
    if span <= 0 or len(narrow) < 4:
        return []
    cover = bytearray(span + 1)
    for s in narrow:
        cover[s['left'] - lo:s['right'] - lo + 1] = b'\1' * (s['right'] - s['left'] + 1)
    found, start = [], None
    for x in range(int(span * .2), int(span * .8) + 1):
        if not cover[x] and start is None:
            start = x
        elif cover[x] and start is not None:
            if x - start >= span * .03:
                found.append(lo + (start + x) / 2)
            start = None
    return found


def _order(segs):
    """Column by column. A line that starts in the first column and crosses a gutter
    (a heading across the page) closes the band of columns above it."""
    cuts = _gutters(segs)
    out, band = [], []
    def flush():
        band.sort(key=lambda s: (sum(s['left'] > c for c in cuts), s['top'], s['left']))
        out.extend(band); band.clear()
    for s in sorted(segs, key=lambda s: (s['top'], s['left'])):
        if cuts and s['left'] < cuts[0] < s['right']:
            flush(); out.append(s)
        else:
            band.append(s)
    flush()
    return out


def ocr_layout(tsv):
    """Reading order for tesseract TSV output. Tesseract sometimes reads side-by-side
    columns straight across; lines are split at wide gaps and read column by column."""
    lines = {}
    for row in tsv.splitlines()[1:]:
        f = row.split('\t')
        if len(f) < 12 or f[0] != '5' or not f[11].strip():
            continue
        lines.setdefault(tuple(f[1:5]), []).append((*map(int, f[6:10]), f[11]))
    segs = [s for words in lines.values() for s in _segments(words)]
    return '\n'.join(s['text'] for s in _order(segs)) if segs else ''


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
        if item.get('kind') == 'region' and not item.get('text'):
            continue   # a box over a figure: no text to match
        text = ' '.join(_lines(item.get('text', '')))
        if not text:
            continue
        page = int(item.get('page') or 0)
        out.append({'id': f'h{i}', 'page': page, 'title': item.get('note', '') or '', 'text': text})
    return out


MARKUPS = ('Highlight', 'Underline', 'Squiggly')


def highlights_from_pdf(path, ocr=True):
    """Read highlights the student made in any PDF viewer (Preview, Acrobat, Zotero, ...).

    Returns ``[{'page', 'text', 'note', 'color', 'rect'}]`` in reading order. Text comes from
    the page under each highlighted line (OCR of that region when the text layer is broken);
    a comment attached to the highlight becomes ``note``. Boxes drawn over figures (Square
    annotations) are returned with ``kind='region'`` so figure tools can use them.
    """
    import pymupdf
    out = []
    with pymupdf.open(path) as pdf:
        for number, page in enumerate(pdf, start=1):
            broken = None
            for annot in page.annots() or []:
                kind = annot.type[1]
                info = annot.info or {}
                note = (info.get('content') or '').strip()
                color = (annot.colors or {}).get('stroke')
                if kind in MARKUPS:
                    vertices = annot.vertices or []
                    rects = [pymupdf.Quad(vertices[i:i + 4]).rect for i in range(0, len(vertices) - 3, 4)] or [annot.rect]
                    parts = [page.get_textbox(r + (-1, -1, 1, 1)).strip() for r in rects]
                    text = ' '.join(p for p in parts if p)
                    if ocr and looks_garbled(text):
                        if broken is None:
                            broken = looks_garbled(page.get_text())
                        if broken:
                            text = ' '.join(_ocr_clip(page, r) for r in rects).strip()
                    if text:
                        out.append({'page': number, 'text': ' '.join(text.split()), 'note': note, 'kind': 'text',
                                    'color': color, 'rect': [round(v, 1) for v in annot.rect], '_y': annot.rect.y0})
                elif kind == 'Square':
                    r = annot.rect
                    out.append({'page': number, 'text': note or '', 'note': note, 'kind': 'region', 'color': color,
                                'rect': [round(v, 1) for v in r],
                                'region': {'left': r.x0 / page.rect.width, 'top': r.y0 / page.rect.height,
                                           'width': r.width / page.rect.width, 'height': r.height / page.rect.height},
                                '_y': r.y0})
    out.sort(key=lambda h: (h['page'], h.pop('_y')))
    return out


def _ocr_clip(page, rect, zoom=4):
    import shutil, subprocess, tempfile, os
    import pymupdf
    if not shutil.which('tesseract'):
        return ''
    pix = page.get_pixmap(matrix=pymupdf.Matrix(zoom, zoom), clip=rect + (-2, -2, 2, 2), alpha=False, colorspace=pymupdf.csGRAY)
    with tempfile.TemporaryDirectory() as tmp:
        image = os.path.join(tmp, 'clip.png')
        pix.save(image)
        run = subprocess.run(['tesseract', image, 'stdout', '--psm', '7', '-l', 'eng'], capture_output=True, text=True, timeout=60)
    return run.stdout.strip() if run.returncode == 0 else ''
