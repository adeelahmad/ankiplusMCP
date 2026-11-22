#!/bin/sh
# One-command AnkiPlus install for students on macOS and Linux.
#
#   curl -LsSf https://raw.githubusercontent.com/adeelahmad/ankiplusMCP/main/scripts/install.sh | sh
#   curl -LsSf .../install.sh | sh -s -- claude-desktop "$HOME/Documents/Lectures"
#
# Installs what is missing (uv, Node, Tesseract), registers AnkiPlus in your AI app,
# downloads the matching model once, and runs a health check. Safe to run again: an older
# AnkiPlus entry (e.g. from Hamza-Xoho/ankiplusMCP) is replaced, and a backup of the config is kept.
set -eu
HOST="${1:-claude-desktop}"
LECTURES="${2:-$HOME/Documents/Lectures}"
REF="${ANKIPLUS_REF:-github:adeelahmad/ankiplusMCP#release-v0.5.3}"
say() { printf '\n==> %s\n' "$1"; }
have() { command -v "$1" >/dev/null 2>&1; }
sudo_() { if [ "$(id -u)" = 0 ]; then "$@"; else sudo "$@"; fi; }

say "1/5 uv (runs Python for AnkiPlus)"
if ! have uv; then
  curl -LsSf https://astral.sh/uv/install.sh | sh
  PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"; export PATH
fi
uv --version

say "2/5 Node.js 18+ (runs the installer)"
if ! have npx; then
  if have brew; then brew install node
  elif have apt-get; then sudo_ apt-get update -q && sudo_ apt-get install -y nodejs npm
  elif have dnf; then sudo_ dnf install -y nodejs npm
  else echo "Install Node.js 18+ from https://nodejs.org, then run this again."; exit 1
  fi
fi
node --version

say "3/5 Lecture folder: $LECTURES"
mkdir -p "$LECTURES"

say "4/5 Register AnkiPlus in $HOST"
npx -y "$REF" init --host "$HOST" --source-root "$LECTURES" --yes --force

say "5/5 Tesseract (OCR), preview browser and matching model"
npx -y "$REF" setup --yes --tesseract --model || true
npx -y "$REF" doctor || true

cat <<DONE

Done. Restart $HOST.
Put lecture PDFs in:
  $LECTURES
Your AnKing deck is read straight from Anki (no export needed).
Then ask your assistant: "Index my AnKing deck, then match my lecture and show me what to unsuspend."
DONE
