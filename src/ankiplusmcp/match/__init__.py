"""Deterministic matching of lecture content against an existing deck (e.g. AnKing).

No model calls and no Anki connection: the deck is read from an exported file or a
read-only copy of a collection, and the results are plans the host executes through
the official Anki MCP after the user approves them.

Pipeline: deck file -> DeckIndex (BM25 + optional embeddings) -> chunks from the
lecture -> per-chunk hybrid search -> covered / related / gap -> unsuspend plan
and a drafting prompt that lists only the gaps.
"""
from .deck import load_deck, clean_text
from .index import DeckIndex, tokenize
from .chunks import chunks_from_slides, chunks_from_pdf, chunks_from_text, chunks_from_highlights
from .matcher import match_chunks, Thresholds
from .prompt import build_prompt
from .evaluate import evaluate

__all__ = ['load_deck', 'clean_text', 'DeckIndex', 'tokenize', 'chunks_from_slides', 'chunks_from_pdf',
           'chunks_from_text', 'chunks_from_highlights', 'match_chunks', 'Thresholds', 'build_prompt', 'evaluate']
