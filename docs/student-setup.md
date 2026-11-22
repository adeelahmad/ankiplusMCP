# Student setup (5 minutes)

[Back to the README](../README.md) · [Matching existing decks](matching.md)

You need: Anki with the AnkiConnect-based **Anki MCP**, and an AI app such as **Claude Desktop**.

## 1. Install

**Easiest: the desktop app.** Download it from the
[releases page](https://github.com/adeelahmad/ankiplusMCP/releases) (`.pkg` or `.dmg` for an Apple Silicon Mac,
`-setup.exe` for Windows). The builds are unsigned, so the first open needs **Open Anyway** (Mac) or
**More info → Run anyway** (Windows). See [installing the desktop app](../rust/docs/install-unsigned.md).
On a Mac, this one line installs it with no prompts:

```sh
curl -fsSL https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install-desktop.sh | sh
```

The app walks you through picking your deck, indexing it and connecting Claude Desktop. Then skip to step 3.

**Or run one command (no app):**

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

Already have an older AnkiPlus (for example from Hamza-Xoho/ankiplusMCP) in your AI app? The
command replaces that entry and keeps a backup of the app's config next to it.

## 2. Nothing to export

AnkiPlus reads your Anki collection directly (a private copy, so Anki can stay open). The
first index of AnKing takes a while (about an hour on a laptop, once). After an AnkiHub
sync, ask it to update the index: only new or changed cards are processed.

## 3. Restart your AI app and ask

> Index my AnKing deck.

Then, for each lecture:

> Match Lectures/L025.pdf against AnKing. Show me which cards to unsuspend, then draft cards only for the gaps.

Textbook or long PDF? Highlight what matters in any PDF viewer, save, and ask:

> Match only my highlights in Lectures/chapter5.pdf.

Nothing changes in Anki until you approve the list. Sync Anki afterwards.

## If something is missing

Run `npx -y github:adeelahmad/ankiplusMCP#release-v0.5.4 doctor`. Each missing item prints its fix.
