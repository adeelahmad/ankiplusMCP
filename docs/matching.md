# Matching lectures against existing decks

Students often own a large pre-made deck (for example AnKing Step 1, ~30,000 notes, mostly
suspended). Before drafting new cards, AnkiPlus can find which lecture facts that deck already
covers, propose those suspended cards for unsuspension, and hand the AI only the gaps. This runs
**without any AI calls**, so it costs no model tokens.

```
deck export ──index_deck──▶ index (BM25 + local embeddings)
lecture PDF / highlights ──match_source──▶ report.json · unsuspend.json · prompt.md
```

## 1. Export the deck

In Anki: *File → Export*, choose **Anki Deck Package (.apkg)**, select the deck, untick
"Include media" (much smaller). Newer Anki versions write a compressed format; the optional
`zstandard` package reads it, or tick *Support older Anki versions*. A copy of
`collection.anki2` or a JSON note list saved from the Anki MCP (`notes_info`) also works.
Put the file inside a folder AnkiPlus may read (`--source-root`).

## 2. Index it once

```
index_deck {"workspace":"W","deck_path":"/abs/AnKing.apkg","deck_prefix":"AnKing"}
```

or from a terminal (no AI host needed):

```
ankiplusmcp match --deck AnKing.apkg --deck-prefix AnKing --pdf L025.pdf --out builds/L025
```

Install the optional extra for embeddings and embedded Postgres:
`pip install 'ankiplusmcp[match]'`. Without it, matching uses BM25 only and the index is kept
in files.

| Part | What it does |
|---|---|
| BM25 | Keyword ranking over note text plus topic-tag names. |
| Embeddings | `nomic-embed-text-v1.5` run locally through ONNX (fastembed), with `search_document:`/`search_query:` prefixes, cut to 256 dimensions. Downloaded once from Hugging Face. The same ONNX model runs in Rust (fastembed-rs) in the desktop app under `rust/` (see `rust/README.md`). |
| Fusion | Reciprocal rank fusion (k = 60) merges both candidate lists. |
| Storage | Embedded Postgres + pgvector (`pixeltable-pgserver`, private data folder, socket only) when installed, else files. Both sit behind `IndexStore`. |

## 3. Match a lecture

```
match_source {"workspace":"W","index":"anking"}
```

The lecture (or the user's PDF highlights, if given) is split into fact-sized chunks. Each chunk is:

- **covered**: the best note's score ≥ `covered` (default 0.5 with BM25 only, 0.7 with
  embeddings). Its suspended cards go into `unsuspend.json`.
- **related**: score ≥ `related` (default 0.35 / 0.62). The prompt shows the existing card so the AI
  drafts only what is missing.
- **gap**: no existing card. Listed in `prompt.md` for drafting.

Score = the geometric mean of two IDF-weighted shares: how much of the chunk the note contains,
and how much of the note's first field (its fact, not the Extra) the chunk contains. Both are
needed on a real deck, where long Extras contain most words of any short lecture line. A note
that shares fewer than two words with the chunk scores half, and numbers count a quarter of a
word. With embeddings the score is `0.3 × lexical + 0.7 × cosine` when that is higher.

**Concept mapping.** For covered and related chunks, notes sharing the most specific AnKing topic
tag (for example `…::Upper_Limb::Brachial_Plexus`) are listed as concept neighbours, so a lecture
on thoracic outlet syndrome also surfaces the brachial plexus cards. Question-bank ID tags and
tags with more than 150 notes are ignored. Neighbours are only unsuspended with
`unsuspend_concepts=true`.

**Nothing is changed in Anki.** The unsuspend flow is:

1. `prepare_unsuspend` lists each proposed note with the lecture lines it matched.
2. The user rejects any that don't fit; call it again with `exclude_note_ids`.
3. The host unsuspends the returned CARD ids through the Anki MCP.
4. `record_unsuspend` logs the result. Later lectures skip cards already unsuspended
   (`already_unsuspended` in the summary).

## Highlights: card only what the student marked

For textbooks and long PDFs, let the student choose. They highlight the facts in any PDF viewer
(Preview, Acrobat, Zotero, Edge), save, and AnkiPlus reads the highlights straight from the file:

```
match_source {"workspace":"W","index":"anking","pdf_highlights":true}
ankiplusmcp match --deck AnKing.apkg --pdf chapter5.pdf --highlighted --out builds/ch5
```

Each highlight becomes one chunk, kept whole. A comment attached to a highlight is kept as its
note, and boxes drawn over figures come back as regions (`read_highlights`) for image occlusion.
Highlights on a page with a broken text layer are OCR'd.

## 4. Check drafted cards

`check_source_support` flags cards whose cloze answers do not appear in the source, or whose
wording has little support on any page.

## Tuning the thresholds

**Real AnKing deck (2026-11-23).** The BM25 defaults come from a real AnKing export (AnKing Step
and MCAT decks plus one anatomy deck, 33,078 notes) and two real lectures: 126 extracted chunks,
labelled by hand against the deck (40 with an existing note, 86 true gaps such as timetables,
objectives and facts AnKing lacks). Precision counts accepted notes that were labelled correct.
The labels are incomplete, so it is a lower bound:

| BM25 only, covered | precision | recall |
|---|---|---|
| 0.35 (old default) with the old one-way score | 0.11 | 0.61 |
| 0.45 | 0.59 | 0.45 |
| **0.50 (default)** | 0.75 | 0.35 |
| 0.55 | 0.92 | 0.25 |

Most remaining BM25 errors share words but not meaning (for example "the tongue deviates toward
the lesion" against a card on the trachea deviating in atelectasis). With Nomic embeddings, on
the same labels:

| BM25 + Nomic, covered | precision | recall | true gaps marked covered |
|---|---|---|---|
| 0.55 with the old score (mean of both) | 0.43 | 0.68 | many |
| 0.66 | 0.62 | 0.55 | 6 |
| **0.70 (default)** | 0.88 | 0.40 | 0 |
| 0.72 | 0.89 | 0.30 | 0 |

Related defaults to 0.62 (recall 0.70), so most remaining matches still reach the AI as existing
cards to check. Nomic cosines on medical text sit in a narrow band (0.6–0.8), which is why the
embedding thresholds are higher than the BM25 ones. Embedding all 33,078 notes took 48 minutes
on 4 cloud CPUs; a recent laptop is faster, and it happens once per deck.

**Synthetic benchmark.** `scripts/match_benchmark.py` uses a deck written in AnKing cloze style
(72 notes, not AnKing content) and 54 facts paraphrased from two real lectures. At covered 0.5
(BM25 only) it gives precision 1.00 and recall 0.61.

To check another deck, label one lecture by hand
(`[{"text": "<fact>", "note_ids": [<AnKing note ids>]}]`, empty list for facts with no card) and run:

```
ankiplusmcp match --deck AnKing.apkg --evaluate labels.json
```

It reports recall@5, how many true gaps were marked as gaps, and the facts it missed.
