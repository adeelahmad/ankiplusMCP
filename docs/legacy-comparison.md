[Back to the README](../README.md)

# How AnkiPlus compares with the legacy workflow

Both tools ran on the same lectures: MED231 L001–L003 (oral cavity and tongue, anatomy) and L004 (common GIT symptoms). The legacy run used the original Lecture2Cards workflow with Sonnet 5.5 on medium effort, and AnkiPlus ran as v0.5.4. Measured on 2026-11-22.

## Verdict
- **AnkiPlus wins on token efficiency and on image occlusion.** Over time the gap grows, because the legacy method's cost rises with deck size and AnkiPlus does not.
- **Legacy wins on judging and text coverage today.** Claude read every candidate card's question and answer, and it wrote more definition cards. We have adopted both: the judging step and the coverage pass (v0.5.4).

## 1. Image occlusion (the big gap)
| | Legacy | AnkiPlus |
|---|---|---|
| IO notes | **0**. Tesseract failed, and the run went on without figures | **22 figures → 22 IO notes, 127 masks** |
| Labels detected | – | 238 → **265** after today's OCR fix (PR #30, shipped in 0.5.4) |

Quality, from the contact sheet the image-occlusion contact sheet from that run:
- Clean atlas figures are good (p11, p12, p13, p21, p22, p23).
- Weak spots: some labels are missed, some masks merge two labels, there are a few misplaced boxes (p14, p20), and about 25 labels have OCR typos.
- Automatic spelling correction is unsafe: it turned Foliate into Folate and Fungiform into Fusiform. Claude fixes the label text while it views the figure.

For anatomy this alone decides it: most of the testable labels are on figures.

## 2. Token efficiency, and how it scales
| | Legacy | AnkiPlus |
|---|---|---|
| Finding existing cards | Claude writes SQL and keyword searches and reads the results: 62 tool calls, about 184 KB of results (≈46k tokens) plus 178 KB of saved large outputs, all re-read as context | Search runs locally (BM25 + embeddings): **0 model tokens** |
| Judging | Claude reads everything the searches return | Claude reads a short list: **≈83 KB (≈21k tokens) for both lectures**, down from 110 KB in 0.5.3 |
| Growth with the deck | **Grows with the deck.** Every keyword search returns more hits as cards pile up on similar topics (AnKing plus your own cards from earlier lectures), and Claude has to read all of them | **Flat.** At most 8 matches + 3 candidates per lecture line, whatever the deck size. Cards already unsuspended are never offered again |

**Rough projection** for one lecture after a year of study. Say the deck holds 3× as many related cards:
- Legacy: about 3× the search output, so roughly 100–150k tokens a lecture. Risk of context overflow and missed cards.
- AnkiPlus: still about 10k tokens a lecture to judge, plus drafting.

The legacy approach of reasoning over bulk tag and keyword searches does not scale. Its accuracy came from the judging, not from the searching.

## 3. Custom cards: AnkiPlus vs legacy (fronts compared at cosine ≥ 0.8)
| | Legacy | AnkiPlus | Shared |
|---|---|---|---|
| Anatomy L001–L003 | 51 | 42 | 49 of legacy's 51 are covered by AnkiPlus; all 42 of AnkiPlus are in legacy |
| Pathology L004 | 34 | 22 | 24 of legacy's facts are shared |

- **Only legacy had these:**
  - Anatomy, mostly nerve and vessel facts: buccinator (VII), vallate taste (IX), lingual artery branches, sensation of the lower lip and cheek (V3), submental drainage of the tip, foramen cecum position, oral fissure, oropharyngeal isthmus.
  - Pathology, mostly symptom definitions: heartburn, acid reflux, flatulence, nausea, vomiting, anorexia, glossitis, candidiasis, causes of weight loss.
- **Only AnkiPlus had these:** icterus, melaena, the 3 mg/dL jaundice threshold, where flatus comes from, chronic diarrhoea, causes of haematemesis.
- Both runs had 0 list-cloze or cut-sentence problems. AnkiPlus had one garbled constipation card, from the old L004 OCR.

## 4. What we took from the legacy run (shipped in 0.5.4, PR #31)
1. **A judging step.** Claude reads each card's question and answer and keeps only the cards that test the lecture line. This was already in 0.5.3 and stays.
2. **A coverage pass in drafting.** Every term the lecture defines gets a definition card, and every structure gets its nerve, blood and lymph cards. It is in the drafting prompt and in the `/anki2` draft phase. This targets exactly the facts only legacy had.
3. **A smaller candidate list.** A candidate card is offered only if its hidden answer appears in the lecture line or its heading: 190 → 135 candidates. Legacy picks kept: 16 of 17.
4. **The OCR fix reaches installs.** Install from `release-v0.5.4`.

## Still open
- Redraft L004 from the fixed extraction, aiming at legacy's 34 notes.
- IO label typos and merged masks: Claude reviews these while viewing each figure.
- Compare against a legacy image-occlusion rerun once Tesseract works there.
