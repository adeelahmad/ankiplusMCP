# Installing AnkiPlus (test build, unsigned)

These test builds are not signed yet, so macOS and Windows ask once before they open them.

## Before you start

- An Apple Silicon Mac with macOS 13.4 (Ventura) or later, or Windows 10/11 (64-bit).
- Anki is installed and you have opened it at least once.
- Claude Desktop is installed and you are signed in (https://claude.ai/download).
- Card writing goes through the AnkiMCP add-on in Anki, which needs to be installed in Anki as before.

## macOS

**Easiest: the installer package.** Download `AnkiPlus_*_aarch64.pkg` and open it.

1. macOS says it "cannot verify" the installer. Click **Done** (or OK), then open
   **System Settings → Privacy & Security**, scroll down to the message about AnkiPlus and click
   **Open Anyway**, then enter your password.
2. Click through the installer. It puts AnkiPlus in Applications and clears the security flag,
   so from then on AnkiPlus opens with a normal double-click.

**No prompts at all (one line in Terminal):** open **Terminal** (Applications → Utilities), paste this,
press Return, and enter your Mac password if asked. It downloads the newest build, installs it and opens it.

```
curl -fsSL https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install-desktop.sh | sh
```

**Or the disk image:** open `AnkiPlus_*.dmg`, drag **AnkiPlus** into **Applications**, then **right-click**
AnkiPlus → **Open** → **Open** (once). If there is no Open button, use **Open Anyway** in Privacy & Security
as above, or paste `xattr -dr com.apple.quarantine /Applications/AnkiPlus.app` into Terminal.

This build is for Apple Silicon Macs (M1 or newer). To check, open the Apple menu → About This Mac and
look for "Chip: Apple". Intel Macs are not supported yet.

## Windows

1. Run `AnkiPlus_*-setup.exe`.
2. If you see "Windows protected your PC", click **More info** and then **Run anyway**.

## Setup (both systems)

The app opens with three steps:

1. **Your Anki**: it finds your profile and lists your decks. Pick the deck to compare against (for example AnKing).
2. **Index my cards**: reads a private copy of your collection (Anki can stay open). The first time it downloads
   the search model, about 0.5 GB. A large deck takes a few minutes, and the bar shows progress.
3. **Connect Claude Desktop**: adds AnkiPlus to Claude Desktop. Your old Claude settings file is backed up next to
   it first. The app then downloads its tools once (Python and the AnkiPlus server, a few minutes).

Then **quit Claude Desktop completely** (Claude menu → Quit, or right-click the tray icon on Windows) and open it
again. From then on everything happens in Claude chat, for example:

> Make cards from MyLecture.pdf in Downloads; skip what my deck already has.

Claude can read lecture files in Documents, Downloads and Desktop. Change the list in step 3 if your PDFs are
somewhere else, then press Connect again.

## Updating and removing

- New test build: install it over the old one, open it and press **Connect Claude Desktop** again.
- To remove: delete the app, and remove the `ankiplusmcp` entry from Claude Desktop's settings
  (Claude → Settings → Developer → Edit Config).
