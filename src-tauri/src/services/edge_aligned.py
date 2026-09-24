"""Invoked by the desktop app only; audio and word timing come from the same request."""
import asyncio
import json
import sys
import edge_tts

async def main():
    text, voice, output = sys.argv[1:4]
    words = []
    with open(output, "wb") as media:
        async for item in edge_tts.Communicate(text, voice, boundary="WordBoundary").stream():
            if item["type"] == "audio":
                media.write(item["data"])
            elif item["type"] == "WordBoundary":
                words.append({"start": item["offset"] / 10000000,
                              "end": (item["offset"] + item["duration"]) / 10000000,
                              "text": item["text"]})
    if not words:
        raise RuntimeError("Edge TTS returned no word boundaries; cannot claim aligned captions")
    with open(output + ".words.json", "w", encoding="utf-8") as handle:
        json.dump(words, handle, ensure_ascii=False)

asyncio.run(asyncio.wait_for(main(), timeout=120))
