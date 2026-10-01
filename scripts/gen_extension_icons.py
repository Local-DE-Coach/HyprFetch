#!/usr/bin/env python3
"""Generate HyprFetch Media Catcher toolbar icons (16/32/48/128 px).

Rounded-square gradient (violet -> teal) with a white download arrow —
matches the WebUI accent palette. Pure Pillow, no assets needed.
"""
from PIL import Image, ImageDraw
import os

OUT = os.path.join(os.path.dirname(__file__), "..", "extension", "icons")
os.makedirs(OUT, exist_ok=True)

ACCENT_TOP = (124, 92, 255)   # #7C5CFF
ACCENT_BOT = (56, 224, 176)   # #38E0B0


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def make(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    radius = max(2, int(size * 0.22))

    # Vertical gradient across the rounded square.
    grad = Image.new("RGBA", (size, size))
    gd = ImageDraw.Draw(grad)
    for y in range(size):
        gd.line([(0, y), (size, y)], fill=lerp(ACCENT_TOP, ACCENT_BOT, y / max(1, size - 1)) + (255,))
    mask = Image.new("L", (size, size), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    img.paste(grad, (0, 0), mask)

    # White download arrow (tray + shaft + head), stroke-scaled by size.
    w = max(1.0, size / 32.0)
    cx = size / 2
    stroke = max(2, int(size * 0.11))
    shaft_top = size * 0.22
    shaft_bot = size * 0.52
    d.line([(cx, shaft_top), (cx, shaft_bot)], fill=(255, 255, 255, 255), width=stroke)
    head = size * 0.20
    d.polygon(
        [
            (cx - head, shaft_bot - head * 0.7),
            (cx + head, shaft_bot - head * 0.7),
            (cx, shaft_bot + head * 0.55),
        ],
        fill=(255, 255, 255, 255),
    )
    tray_y = size * 0.72
    tray_half = size * 0.26
    d.line(
        [(cx - tray_half, tray_y), (cx + tray_half, tray_y)],
        fill=(255, 255, 255, 255),
        width=stroke,
    )
    return img


for s in (16, 32, 48, 128):
    make(s).save(os.path.join(OUT, f"icon{s}.png"))
    print(f"icon{s}.png written")
