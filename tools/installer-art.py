"""Draws the images for the NSIS installer in the v9 style (light background, 26px grid, blue stripe, app icon).

Usage: python tools/installer-art.py   → src-tauri/windows/{header.bmp, sidebar.bmp}
NSIS needs 24-bit BMPs: header 150×57, sidebar 164×314 (Tauri sets these for MUI2).
"""
import pathlib
from PIL import Image, ImageDraw, ImageFont

ROOT = pathlib.Path(__file__).resolve().parent.parent
ICON = ROOT / "public" / "appicon.png"
OUT = ROOT / "src-tauri" / "windows"
FONT = ROOT / "node_modules" / "@fontsource" / "be-vietnam-pro" / "files" / "be-vietnam-pro-latin-700-normal.woff"
BG = (244, 246, 249)
GRID = (226, 231, 239)
BLUE = (59, 123, 233)
CYAN = (91, 200, 239)
PINK = (232, 137, 193)
INK = (34, 38, 47)
SUB = (120, 128, 142)


def font(size):
    try:
        return ImageFont.truetype(str(FONT), size)
    except OSError:
        return ImageFont.truetype("segoeuib.ttf", size)


def grid(img, step=26):
    d = ImageDraw.Draw(img)
    w, h = img.size
    for x in range(0, w, step):
        d.line([(x, 0), (x, h)], fill=GRID)
    for y in range(0, h, step):
        d.line([(0, y), (w, y)], fill=GRID)


def chamfer_icon(size):
    """App icon in a chamfered-corner frame (like the logo in the app's header bar)."""
    icon = Image.open(ICON).convert("RGBA").resize((size, size), Image.LANCZOS)
    mask = Image.new("L", (size, size), 0)
    c = max(3, size // 6)
    ImageDraw.Draw(mask).polygon([(c, 0), (size, 0), (size, size - c), (size - c, size), (0, size), (0, c)], fill=255)
    tile = Image.new("RGBA", (size, size), (255, 255, 255, 255))
    tile.alpha_composite(icon)
    tile.putalpha(mask)
    return tile


def gradient_bar(w, h):
    bar = Image.new("RGB", (w, h))
    px = bar.load()
    for x in range(w):
        t = x / max(1, w - 1)
        a, b = (BLUE, PINK) if t < 1 else (PINK, PINK)
        col = tuple(int(BLUE[i] + (PINK[i] - BLUE[i]) * t) for i in range(3))
        for y in range(h):
            px[x, y] = col
    return bar


def header():
    img = Image.new("RGB", (150, 57), BG)
    grid(img, 13)
    d = ImageDraw.Draw(img)
    img.paste(chamfer_icon(41).convert("RGB"), (102, 8), chamfer_icon(41))
    d.rectangle([0, 0, 3, 56], fill=BLUE)
    d.text((12, 13), "Shunguang", font=font(13), fill=INK)
    d.text((12, 29), "Decor", font=font(13), fill=BLUE)
    img.paste(gradient_bar(56, 2), (12, 47))
    return img


def sidebar():
    img = Image.new("RGB", (164, 314), BG)
    grid(img)
    d = ImageDraw.Draw(img)
    # polka dots in the bottom-right corner
    for i in range(6):
        for j in range(6):
            d.ellipse([110 + i * 8, 250 + j * 8, 112 + i * 8, 252 + j * 8], fill=(200, 210, 226))
    d.rectangle([0, 0, 5, 313], fill=BLUE)
    d.rectangle([0, 0, 5, 60], fill=CYAN)
    tile = chamfer_icon(96)
    img.paste(tile.convert("RGB"), (34, 34), tile)
    d.text((18, 150), "Shunguang", font=font(22), fill=INK)
    d.text((18, 176), "Decor", font=font(22), fill=BLUE)
    img.paste(gradient_bar(90, 3), (18, 208))
    d.text((18, 218), "DECORATE · YOUR · WORLD", font=font(8), fill=SUB)
    # faint vertical text on the right edge
    v = Image.new("RGBA", (300, 20), (0, 0, 0, 0))
    ImageDraw.Draw(v).text((0, 0), "SHUNGUANG · DECOR", font=font(12), fill=(190, 198, 212, 255))
    v = v.rotate(90, expand=True)
    img.paste(v.convert("RGB"), (140, 20), v)
    return img


OUT.mkdir(parents=True, exist_ok=True)
header().save(OUT / "header.bmp")
sidebar().save(OUT / "sidebar.bmp")
print("ok")
