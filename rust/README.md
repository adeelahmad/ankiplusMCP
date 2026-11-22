# AnkiPlus in Rust (desktop app)

Rust port of the deck-matching pipeline in `src/ankiplusmcp/match`, plus a Tauri
desktop shell. The Python version stays the reference until this one reaches parity
everywhere; `scripts/parity.py` checks that.

```
rust/
  crates/ankiplus-match   core library (no AI calls, no Anki writes)
  crates/ankiplus-cli     `ankiplus` command line, same flags as `ankiplusmcp match`
  app/src-tauri           desktop app (Tauri 2), UI in app/ui
  scripts/parity.py       Python vs Rust comparison on a synthetic AnKing-like deck
```

## What is ported

| Piece | Python | Rust |
|---|---|---|
| Deck loading (.apkg incl. zstd `anki21b`, .colpkg, .anki2, JSON/JSONL) | `deck.py` | `deck.rs` |
| HTML/cloze cleaning, tokenizer, stopwords, stemming | `deck.py`, `index.py` | `text.rs` |
| BM25 + embeddings, two-way lexical score on the front field, reciprocal rank fusion (k = 60) | `index.py` | `index.rs` |
| Chunking (line re-joining, inline/OCR bullets, captions, repeated headers dropped) | `chunks.py` | `chunks.rs` |
| PDF text, garbled-layer detection, tesseract OCR fallback | `chunks.py` (PyMuPDF) | `pdf.rs` (docling.rs + `pdftoppm`) |
| covered / related / gap, unsuspend list, concept mapping via AnKing tags | `matcher.py` | `matcher.rs` |
| Gaps-only drafting prompt | `prompt.py` | `prompt.rs` |
| Quote check (`check_source_support`, difflib ratio ported exactly) | `quotes.py` | `quotes.rs` |
| Threshold evaluation on hand labels | `evaluate.py` | `evaluate.rs` |
| Anki profile discovery (index the collection without exporting) | `profiles.py` | `profiles.rs` |
| Re-embed only new or changed notes (`note_id` + sha1 of text), written cards added at once | `index.py`, `matching.py` | `index.rs`, `pipeline.rs` |
| File index store (same `notes.jsonl.gz`, `meta.json`, float16 `embeddings.npy`) | `FileIndexStore` | `store.rs` |
| Embedded Postgres + pgvector (same schema) | `pgstore.py` | `pgstore.rs` (feature `postgres`) |
| Local ONNX embeddings, nomic-embed-text-v1.5, prefixes, 256 dims | fastembed | fastembed-rs (feature `embed`) |
| PDF text | PyMuPDF | docling.rs text layer (feature `pdf`) |

The cache key is the Python one (deck path, prefix and embedding mode; the deck's size and
mtime are kept as `deck_stamp` in `meta.json`), so both sides reuse each other's cached indexes
under `~/.cache/ankiplusmcp/indexes`. Embedding vectors are compatible because both
run the same ONNX model with the same prefixes and Matryoshka truncation.

Known difference: PDF text comes from docling.rs instead of PyMuPDF, so line breaks
on a page can differ and chunk boundaries with them. Text, Markdown and highlight
inputs match exactly.

## Build

```
cd rust
cargo test                                  # core + CLI, no network needed
cargo build -p ankiplus-cli --features full # + embeddings, Postgres, PDF
cargo build -p ankiplus-desktop --features full
```

Features: `embed` downloads ONNX Runtime at build time and the Nomic model (~0.5 GB)
on first use from Hugging Face. `postgres` downloads PostgreSQL and the pgvector
package on first use into the index folder (`ANKIPLUS_PG_URL` uses an existing
server with pgvector instead). Models are cached in `~/.cache/ankiplusmcp/models`
(`ANKIPLUS_MODEL_DIR` to change). `pdf` needs no models; OCR of broken or image-only pages uses the system
`tesseract` and `pdftoppm` (`brew install tesseract poppler`), and is skipped without them.

On Linux the desktop app needs WebKitGTK (`libwebkit2gtk-4.1-dev`). On macOS nothing extra.
To make an installer: `cargo install tauri-cli` then `cargo tauri build` in `app/src-tauri`.

## CLI

```
ankiplus match --deck AnKing.apkg --deck-prefix AnKing --pdf L025.pdf --out builds/L025
ankiplus match --deck AnKing.apkg --highlights highlights.json --out builds/L025
ankiplus match --deck AnKing.apkg --evaluate labels.json
ankiplus match --profile "User 1" --pdf L025.pdf --out builds/L025   # no export; ANKI_BASE overrides the Anki folder
ankiplus check --cards drafts.json --text lecture.md
```

## Parity

```
cargo build -p ankiplus-cli
../.venv/bin/python scripts/parity.py
```

Last local run: identical on all checks. A manual run with embeddings on (300 notes,
Nomic 256-dim) also gave byte-identical reports; Postgres save/load matched the file store.

Runs both pipelines (embeddings off) on 3,000 synthetic notes and four lectures at two
threshold settings, and compares report.json, unsuspend.json, prompt.md, evaluate and
the quote check exactly, plus loading each other's cached index.
