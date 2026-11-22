# Student setup (5 minutes)

[Back to the README](../README.md) · [Matching existing decks](matching.md)

You need: Anki with the AnkiConnect-based **Anki MCP**, and an AI app such as **Claude Desktop**.

## 1. Run one command

**Mac or Linux:** open Terminal and paste:

```sh
curl -LsSf https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install.sh | sh
```

**Windows:** open PowerShell and paste:

```powershell
irm https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install.ps1 | iex
```

It installs anything missing (uv, Node.js, Tesseract), registers AnkiPlus in Claude Desktop,
creates `Documents/Lectures`, downloads the matching model (about 130 MB, once) and checks
everything. Running it again is safe. To use another app or folder (macOS/Linux):

```sh
curl -LsSf https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install.sh | sh -s -- claude-code "$HOME/Med/Lectures"
```

On Windows set `$env:ANKIPLUS_HOST` / `$env:ANKIPLUS_LECTURES` before running it.

## 2. Export your AnKing deck once

In Anki: **File → Export**, format **Anki Deck Package (.apkg)**, choose the AnKing deck, untick
**Include media**, and save it into your `Lectures` folder.

## 3. Restart your AI app and ask

> Index my AnKing deck from Lectures/AnKing.apkg.

Then, for each lecture:

> Match Lectures/L025.pdf against AnKing. Show me which cards to unsuspend, then draft cards only for the gaps.

Textbook or long PDF? Highlight what matters in any PDF viewer, save, and ask:

> Match only my highlights in Lectures/chapter5.pdf.

Nothing changes in Anki until you approve the list. Sync Anki afterwards.

## If something is missing

Run `npx -y github:adeelahmad/ankiplusMCP#v0.4.0 doctor`. Each missing item prints its fix.
