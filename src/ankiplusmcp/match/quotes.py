"""Check that drafted cards are supported by the source (citation-style verification).

For each card: find the source chunk that best contains its text, then check every
cloze answer appears (fuzzily) in the source. Deterministic; no model calls. A low
score means "look at this card", not "this card is wrong".
"""
from difflib import SequenceMatcher
import re
from .deck import clean_text
from .index import tokenize

_CLOZE = re.compile(r'\{\{c\d+::(.*?)(?:::[^}]*)?\}\}', re.S)
_NUMBERED = re.compile(r'\{\{c(\d+)::', re.S)
LIST_LIMIT = 3   # this many items behind one cloze number is a list card: one number per item instead


def form_problems(raw):
    """Shape problems a mechanical check can see: a list hidden under one cloze number, and
    text whose brackets don't pair up (a sign the sentence was cut while drafting)."""
    problems = []
    counts = {}
    for number in _NUMBERED.findall(raw):
        counts[number] = counts.get(number, 0) + 1
    for number, n in sorted(counts.items()):
        if n >= LIST_LIMIT:
            problems.append(f'c{number} hides {n} items at once; give each item its own cloze number')
    text = clean_text(raw)
    for left, right in ('()', '[]'):
        if text.count(left) != text.count(right):
            problems.append(f'unpaired {left}{right}: the sentence looks cut; check it against the source')
    return problems


def _fuzzy_in(needle, haystack):
    """Best similarity of ``needle`` against any window of the same number of words (0..1).

    Word-bounded, so "thenar" does not match inside "hypothenar"."""
    words = re.findall(r'\w+', needle.lower())
    hay = re.findall(r'\w+', haystack.lower())
    if not words:
        return 1.0
    n, target = len(words), ' '.join(words)
    best = 0.0
    for start in range(0, max(1, len(hay) - n + 1)):
        window = ' '.join(hay[start:start + n])
        if window == target:
            return 1.0
        best = max(best, SequenceMatcher(None, target, window).ratio())
    return best


def check_support(cards, chunks, answer_threshold=0.8):
    pages = {}
    for c in chunks:
        pages.setdefault(c['page'], []).append(c['text'])
    page_text = {p: '\n'.join(t) for p, t in pages.items()}
    page_terms = {p: set(tokenize(t)) for p, t in page_text.items()}
    out = []
    for card in cards:
        raw = '\n'.join(v for v in card.get('fields', {}).values() if isinstance(v, str))
        answers = [clean_text(a) for a in _CLOZE.findall(raw)]
        terms = set(tokenize(clean_text(raw)))
        refs = [p for p in card.get('source_ref', []) if p in page_text]
        candidates = refs or list(page_text)
        best_page, best = None, 0.0
        for p in candidates:
            score = len(terms & page_terms[p]) / len(terms) if terms else 0.0
            if score > best:
                best_page, best = p, score
        haystack = page_text.get(best_page, '') if refs else '\n'.join(page_text.values())
        missing = [a for a in answers if a and _fuzzy_in(a, haystack) < answer_threshold]
        first = next(iter(card.get('fields', {}).values()), '')   # the cloze text field
        problems = form_problems(first if isinstance(first, str) else '')
        out.append({'card_id': card['id'], 'best_page': best_page, 'term_support': round(best, 3),
                    'cited_pages': refs, 'answers_not_in_source': missing[:10], 'form_problems': problems,
                    'flag': bool(missing) or best < 0.5 or bool(problems)})
    return {'cards': out, 'flagged': sum(r['flag'] for r in out), 'checked': len(out),
            'note': 'Mechanical check: flagged cards need a human or model look; unflagged cards are not proven correct.'}
