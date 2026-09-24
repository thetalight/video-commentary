#!/usr/bin/env python3
"""Generate app icon: rounded square + blue fill + play mark."""

from __future__ import annotations

import sys
from pathlib import Path

try:
    from PIL import Image, ImageDraw
except ImportError:
    print("Install Pillow: python3 -m pip install --user pillow", file=sys.stderr)
    sys.exit(1)

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "app-icon-source.png"

SIZE = 1024
BOX_MARGIN = 112
CORNER_RADIUS = 200
BORDER_WIDTH = 14
FILL_COLOR = (30, 58, 138, 255)
BORDER_COLOR = (147, 197, 253, 255)
TRI_COLOR = (255, 255, 255, 255)
TRI_W, TRI_H = 360, 420


def main() -> None:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)

    box = [BOX_MARGIN, BOX_MARGIN, SIZE - BOX_MARGIN, SIZE - BOX_MARGIN]
    draw.rounded_rectangle(
        box,
        radius=CORNER_RADIUS,
        fill=FILL_COLOR,
        outline=BORDER_COLOR,
        width=BORDER_WIDTH,
    )

    cx, cy = SIZE // 2, SIZE // 2
    left = cx - TRI_W // 3
    draw.polygon(
        [
            (left, cy - TRI_H // 2),
            (left, cy + TRI_H // 2),
            (cx + TRI_W // 2, cy),
        ],
        fill=TRI_COLOR,
    )

    img.save(OUT)
    print(f"Wrote {OUT}")


if __name__ == "__main__":
    main()
