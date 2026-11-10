#!/usr/bin/env python3
"""Draws the Speachy app and menu-bar icons into src-tauri/icons.

Requires Pillow. Run from the repository root: python3 scripts/make_icons.py
"""
from pathlib import Path
from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
# Relative heights of the five bars of the waveform glyph.
BARS = [0.34, 0.72, 1.0, 0.56, 0.26]


def bars(draw, box, color_at, scale=1.0):
    """Draws the waveform inside box=(x0, y0, x1, y1)."""
    x0, y0, x1, y1 = box
    width, height = x1 - x0, y1 - y0
    n = len(BARS)
    bar_w = width / (n + (n - 1) * 0.62)
    gap = bar_w * 0.62
    for i, rel in enumerate(BARS):
        h = height * rel * scale
        left = x0 + i * (bar_w + gap)
        top = y0 + (height - h) / 2
        draw.rounded_rectangle((left, top, left + bar_w, top + h), radius=bar_w / 2, fill=color_at(i / (n - 1)))


def lerp(a, b, t):
    return tuple(round(a[k] + (b[k] - a[k]) * t) for k in range(3))


def app_icon(size=1024):
    ss = 4  # supersampling
    s = size * ss
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    # macOS icon grid: the tile covers 824/1024 of the canvas.
    pad = s * 100 // 1024
    tile = (pad, pad, s - pad, s - pad)
    # Vertical gradient background, masked to a rounded tile.
    top, bottom = (38, 42, 54), (16, 18, 24)
    grad = Image.new("RGBA", (s, s))
    gd = ImageDraw.Draw(grad)
    for y in range(s):
        gd.line([(0, y), (s, y)], fill=lerp(top, bottom, y / s) + (255,))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle(tile, radius=(s - 2 * pad) * 0.225, fill=255)
    img.paste(grad, (0, 0), mask)
    draw = ImageDraw.Draw(img)
    inner = (s - 2 * pad) * 0.24
    amber, coral = (255, 190, 80), (255, 112, 86)
    bars(draw, (pad + inner, pad + inner, s - pad - inner, s - pad - inner), lambda t: lerp(amber, coral, t) + (255,))
    return img.resize((size, size), Image.LANCZOS)


def tray_icon(size=44):
    ss = 8
    s = size * ss
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    m = s * 0.14
    bars(ImageDraw.Draw(img), (m, m, s - m, s - m), lambda t: (0, 0, 0, 255))
    return img.resize((size, size), Image.LANCZOS)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    icon = app_icon()
    icon.save(OUT / "icon.png")
    for name, px in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)]:
        icon.resize((px, px), Image.LANCZOS).save(OUT / name)
    icon.save(OUT / "icon.icns", sizes=[(16, 16), (32, 32), (64, 64), (128, 128), (256, 256), (512, 512), (1024, 1024)])
    icon.save(OUT / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (256, 256)])
    tray_icon().save(OUT / "tray.png")
    print("wrote icons to", OUT)


if __name__ == "__main__":
    main()
