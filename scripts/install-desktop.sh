#!/bin/sh
# AnkiPlus desktop app for macOS (Apple Silicon), in one command, no security prompts:
#
#   curl -fsSL https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install-desktop.sh | sh
#
# Downloads the newest desktop build from the GitHub releases, copies AnkiPlus.app to
# /Applications (asks for your password only if that folder needs it), clears the
# quarantine flag (the app is not signed with an Apple Developer ID) and opens it.
set -eu
REPO="adeelahmad/ankiplusMCP"
APP="AnkiPlus.app"

[ "$(uname -s)" = Darwin ] || { echo "This installer is for macOS. On Windows, run the -setup.exe from the releases page." >&2; exit 1; }
[ "$(uname -m)" = arm64 ] || { echo "This build needs an Apple Silicon Mac (M1 or newer)." >&2; exit 1; }

echo "Finding the newest AnkiPlus build..."
url=$(curl -fsSL "https://api.github.com/repos/$REPO/releases?per_page=30" \
  | grep -o '"browser_download_url": *"[^"]*aarch64\.dmg"' | head -1 | sed 's/.*"\(https[^"]*\)"/\1/')
[ -n "$url" ] || { echo "No macOS build found at https://github.com/$REPO/releases" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'hdiutil detach -quiet "$tmp/mnt" 2>/dev/null || true; rm -rf "$tmp"' EXIT
echo "Downloading $(basename "$url")..."
curl -fL --progress-bar -o "$tmp/AnkiPlus.dmg" "$url"
hdiutil attach -quiet -nobrowse -mountpoint "$tmp/mnt" "$tmp/AnkiPlus.dmg"

osascript -e 'tell application "AnkiPlus" to quit' >/dev/null 2>&1 || true
if [ -w /Applications ]; then SUDO=""; else SUDO="sudo"; echo "Copying to /Applications needs your Mac password:"; fi
$SUDO rm -rf "/Applications/$APP"
$SUDO cp -R "$tmp/mnt/$APP" /Applications/
$SUDO xattr -dr com.apple.quarantine "/Applications/$APP" 2>/dev/null || true

echo "Installed /Applications/$APP. Opening it..."
open "/Applications/$APP"
