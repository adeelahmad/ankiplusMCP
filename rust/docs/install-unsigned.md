# Installing AnkiPlus (test build, unsigned)

These test builds are not signed yet, so macOS and Windows ask once before they open them.

## Before you start

- macOS 13.4 (Ventura) or later, or Windows 10/11.
- Anki is installed and you have opened it at least once.
- Claude Desktop is installed and you are signed in (https://claude.ai/download).
- Card writing goes through the AnkiMCP add-on in Anki, which needs to be installed in Anki as before.

## macOS

1. Open `AnkiPlus_*.dmg` and drag **AnkiPlus** into **Applications**.
2. In Applications, **right-click** (or Control-click) AnkiPlus and choose **Open**, then **Open** again.
   A double-click on an unsigned app only shows "cannot be opened". The right-click is needed once.
3. On macOS 15 or later, if there is no Open button: open **System Settings → Privacy & Security**,
   scroll down to "AnkiPlus was blocked", click **Open Anyway** and enter your password.

Pick `AppleSilicon` for M1/M2/M3/M4 Macs and `Intel` for older ones. To check, open the Apple menu →
About This Mac and look for "Chip: Apple" or "Processor: Intel".

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
