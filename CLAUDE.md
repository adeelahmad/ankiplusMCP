# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

It serves two kinds of session. **Making or fixing cards** follows "Anki card workspace" below. **Changing the code** follows "Developing this repository" at the end, plus `AGENTS.md`. The runtime section is asserted by the frozen contract test `tests/contract/test_p6_runtime.py::test_runtime_claude_routes_and_gate`: keep its lines (notably the ADR-9 line and the `/anki2` skill path) verbatim.

# Anki card workspace

This folder is where cards are made and maintained with `/anki2` (`.claude/skills/anki2/SKILL.md`) for BUILD and resume. HARVEST and REPAIR stay on `/anki`.
Anki runs locally with the AnkiMCP add-on (MCP server `anki`, http://127.0.0.1:3141/). Note type for own cards: `ColdRecall Cloze` (fields Text, Extra).

## Always
- Use `/anki2` for lecture card builds and resume; follow its small core and load only the current phase file. Route HARVEST and REPAIR to `/anki`.
- One lecture (or one topic) per conversation.
- Each job gets a folder `builds/<YYYY-MM-DD>_<lecture-id-or-slug>/` for manifest, drafts.jsonl, model/, renders/, reports. Never put working files in the lecture folder or the vault.
- Use `.venv/bin/python` for all scripts. Rebuild the venv with `python3 -m venv .venv && .venv/bin/pip install -e '.[dev]'`.
- Writes happen without asking once `cards gate` passes (ADR-9)
- Never delete notes unless told "delete". Default for replacing cards: suspend + tag `#ColdRecall::!Flags::Replaced_<date>`.
- Every tag under `#ColdRecall::`. Never write into AnKing's tag trees or note types. Own cards use `ColdRecall Cloze`.
- AnKing first: before drafting, search the AnKing MCAT/Step decks for the same facts; an existing AnKing note becomes an unsuspend proposal, not a new card. MCAT deck fits M1 foundational science; Step deck fits clinical depth; one deck per concept.
- Anatomy is base-first: bones and landmarks → joints → movements → muscles → nerves and vessels.
- Note ids vs card ids: suspend/unsuspend take CARD ids.
- Never invent content for a source file you could not read. A cloud-only Drive file that won't open: tell the user to make it available offline.
- Honor Claude Code tool permissions. A permission refusal stops the run; never work around it.
- After finishing, remind the user to sync Anki.

## How a conversation starts → what to do
1. **"cards from <lecture/file/slides 5–12>"** → `/anki2` BUILD on that source (or only those slides). Resolve the lecture ID from the filename or the block's `session-index.md`.
2. **"cards from this: <pasted text / notes / a topic>"** → `/anki` BUILD with the pasted text as the source. The maintainer checks fidelity against that text only; nothing from general knowledge unless the user says "fill gaps". Source tag `#ColdRecall::#Source::Prompt::<YYYY-MM-DD>_<slug>`. Ask which deck if not obvious.
3. **"fix / adjust / redo <cards, deck, tag, or a search>"** → `/anki` REPAIR, following its review and approval rules.
4. **"suspend / unsuspend <lecture, tag, tier, search>"** → `/anki` for manual suspension requests, following its approval rules. Common searches:
   - a lecture: `tag:#ColdRecall::#Source::<COURSE>::L<nnn>*`
   - a lecture's tier 1–2: add `-tag:#ColdRecall::!Tier::3`
   - AnKing cards for a lecture: by the AnKing tag the user gives, or by keyword in the AnKing deck
5. **"what's due / how am I doing in <subject>"** → `/anki` READ (due today, weak cards below 0.85 retrievability). Other subjects get one line.
6. **Anything else card-related** → say which job it maps to before starting. If none fits, say so and propose the closest.

## Paths and IDs
- Lectures and books (READ-ONLY, never write): `~/Library/CloudStorage/GoogleDrive-<account>/<lecture folder>`
  - Lectures: `01-Blocks/<BLOCK>/Lectures/MED241_L025_2026-09-29_topic.pdf` (P practicals, B CBL, T skills). Each block has `session-index.md`.
  - `03-Resources` is senior material: say so whenever it is used. Books: `<lecture folder>/Books/`.
- Obsidian vault (understanding lives there, not cards): `<your notes vault>`. Read-only unless asked.
- Skills: `.claude/skills/anki2/` (BUILD/resume), `.claude/skills/anki/` (v1 HARVEST/REPAIR). Builds: `builds/`. Scripts: `scripts/`.
- `/anki2` deck convention: flat title resolved by `cards target` from slide 1. Preserve existing nested decks.

# Developing this repository

`AGENTS.md` is the authoritative developer brief; read it before changing code. Deeper references: `docs/developer-guide.md` (architecture, workspace storage, jobs), `docs/tools.md` (all MCP tools with implementation links), `docs/development/` (specs, plans, history), `docs/legacy/architecture.md` (the `cards` CLI phase/gate workflow).

## Commands
```sh
python3 -m venv .venv && .venv/bin/pip install -e '.[dev]'   # Python >=3.12; re-run after pyproject changes
./scripts/check.sh                 # full local check: privacy, version sync, npm test, then check-mcp.sh
./scripts/check-mcp.sh             # Python only: tests/mcp + tests/unit + p0 contract, contract hash check, wheel build
.venv/bin/python -m pytest -q tests/mcp/test_store.py::<test_name>   # single test
.venv/bin/python -m pytest -q tests/contract                         # CLI contract tests (need .venv/bin/cards)
npm test                           # Node launcher tests (node --test launcher/test/*.test.mjs)
.venv/bin/python tests/tools/cli_baseline.py compare <dir>           # `cards` CLI output must stay byte-identical
.venv/bin/python -m ankiplusmcp    # run the MCP server on stdio
```
Some contract tests need private lecture PDFs under `tests/fixtures/private/`. Locally a missing one fails (stop and ask; never skip it). CI sets `CARDS_PUBLIC_CI=1` so they skip. OCR tests need the system `tesseract`; render tests need Playwright Chromium.

## Architecture (big picture)
One Python package, two products, plus a Node launcher:
- `src/ankiplusmcp/`: the current MCP server. `server.py` is a FastMCP facade over `API`; it runs each sync method in a worker thread, bounds output to 64k chars and maps domain errors to `code/message/remedy`. `service.py` composes the `Sources`, `Cards`, `Review`, `Payloads` and `Artifacts` mixins around one `Store` (UUID workspace dirs, file locks, revision counter; `STALE` on revision mismatch) and `Jobs` (two-thread pool with persistent JSON job records for extraction, OCR, export and render). It reuses deterministic functions from `src/cards/` and never creates a legacy `build.json`.
- `src/cards/`: the legacy `cards` CLI (the `/anki2` pipeline) and deterministic core (extract, OCR, figures, check, render, write, meter, review). Its source may change only as an approved plan lists, and its stdout/files must stay byte-identical.
- `launcher/`: dependency-free `npx ankiplusmcp` (serve, init, doctor, setup, uninstall). It finds uv and launches the pinned Python package from the Git tag of `package.json`'s version; it does not proxy MCP traffic.

## Rules that are easy to break
- AnkiPlus never talks to Anki. Do not add Anki connection, proxy or collection-mutation code; the separate Anki MCP does that and the host model drives workflows.
- Frozen: contract tests hashed in `.codex/contract.sha256`, `.codex/`, `.agents/`, and existing `.claude/` skills (never touch `.claude/skills/anki/`). Hooks enforce this; if one blocks you, the change is wrong.
- No network import in `src/cards/` except `src/cards/write/direct.py`. `cards.common` imports nothing from `cards`; `cards.cli` holds no logic.
- `cards` subcommand output contract: line 1 `OK|FINDINGS|ERROR`, then at most 40 lines / 2,000 chars, then `→ <path>` lines; exit 0/1/2/3 = ok/findings/error/refused.
- Keep `package.json` and `pyproject.toml` versions equal (`scripts/check_versions.py`, and the version is hard-coded in `.github/workflows/ci.yml`). `scripts/check_privacy.py` rejects personal paths and emails in tracked files.
- Put new MCP tests in `tests/mcp/`. Preserve the four spec symlinks at `docs/` root.
- Project hooks in `.claude/settings.json` also apply to development sessions: Read of PDFs is blocked during an active build, Bash touching `127.0.0.1`/`localhost:` is blocked, and `fork` subagents are blocked.
