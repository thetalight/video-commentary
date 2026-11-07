#!/bin/bash
# Explicit opt-in installer for the ONE approved Qwen CustomVoice checkpoint.
set -euo pipefail
ROOT="$HOME/Library/Application Support/com.esun.video-commentary"
command -v uv >/dev/null || { echo 'Install uv first; no system Python packages will be changed.' >&2; exit 1; }
uv venv --python 3.12 "$ROOT/qwen-tts/venv" --allow-existing
uv pip install --python "$ROOT/qwen-tts/venv/bin/python" 'mlx-audio==0.6.4' socksio
HF_HUB_DISABLE_TELEMETRY=1 "$ROOT/qwen-tts/venv/bin/python" - <<'PY'
from pathlib import Path
from huggingface_hub import snapshot_download
snapshot_download('mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16', revision='52f4770fd9726457eae3d3b6aa92047a25a10776', local_dir=Path.home()/'Library/Application Support/com.esun.video-commentary/models/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16', max_workers=2)
PY
# No synthesis, voice cloning, task mutation or additional models are triggered here.
