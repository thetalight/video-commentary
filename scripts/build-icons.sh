#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PYTHON="$ROOT/.venv-icon/bin/python"
if [[ ! -x "$PYTHON" ]]; then
  python3 -m venv "$ROOT/.venv-icon"
  "$ROOT/.venv-icon/bin/pip" install -q pillow
fi
"$PYTHON" scripts/generate-icon.py
npx tauri icon app-icon-source.png
mkdir -p public
cp src-tauri/icons/icon.png public/icon.png
echo "Done."
