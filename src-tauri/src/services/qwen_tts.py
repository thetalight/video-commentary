"""Offline Qwen3-TTS 1.7B CustomVoice adapter. No downloads, cloning or other models."""
import argparse
import json
import os
from pathlib import Path

os.environ["HF_HUB_OFFLINE"] = "1"
os.environ["TRANSFORMERS_OFFLINE"] = "1"
os.environ["HF_HUB_DISABLE_TELEMETRY"] = "1"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--request", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    model_path = Path(args.model)
    required = {"model.safetensors": 3833402589, "speech_tokenizer/model.safetensors": 682293092}
    if any(not (model_path / name).is_file() or (model_path / name).stat().st_size != size for name, size in required.items()):
        raise RuntimeError("Approved Qwen CustomVoice checkpoint is incomplete; no download/fallback attempted")
    request = json.loads(Path(args.request).read_text())
    if not request["text"].strip() or len(request["text"]) > 1200:
        raise RuntimeError("Empty or oversized narration paragraph; split into natural paragraphs first")
    import mlx.core as mx
    import numpy as np
    from scipy.io import wavfile
    from mlx_audio.tts.utils import load_model

    model = load_model(str(model_path))
    mx.random.seed(42)
    samples = []
    rate = None
    for result in model.generate_custom_voice(
        text=request["text"], speaker=request["speaker"], language="Chinese",
        instruct=request["instruct"], temperature=0.7, max_tokens=2048,
        repetition_penalty=1.05, verbose=False,
    ):
        mx.eval(result.audio)
        if result.token_count >= 2048:
            raise RuntimeError("Audio reached token budget; refusing potentially truncated speech")
        rate = result.sample_rate
        samples.append(np.asarray(result.audio).reshape(-1))
    if not samples or not rate:
        raise RuntimeError("No waveform returned")
    audio = np.concatenate(samples)
    if not np.isfinite(audio).all() or audio.size == 0:
        raise RuntimeError("Invalid waveform")
    wavfile.write(args.output, rate, (np.clip(audio, -1, 1) * 32767).astype(np.int16))


if __name__ == "__main__":
    try:
        main()
    except Exception:
        # Never echo request prose, model internals or local configuration values.
        import sys
        print("Qwen TTS failed: verify complete local weights/dependencies and paragraph length; no fallback used", file=sys.stderr)
        raise SystemExit(1)
