#!/usr/bin/env python3
"""Rasterise ANSI text (SGR 38;2 / 48;2 / 38;5 + plain glyphs) to a PNG for eyeballing."""
import re, sys
from PIL import Image, ImageDraw, ImageFont

src, dst = sys.argv[1], sys.argv[2]
scale = int(sys.argv[3]) if len(sys.argv) > 3 else 14
font = ImageFont.truetype("/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", scale)
cw = round(font.getlength("M")); asc, desc = font.getmetrics(); ch = asc + desc
text = open(src, encoding="utf-8").read()
lines = text.split("\n")
rows = []
tok = re.compile(r"\x1b\[([0-9;]*)m|(.)", re.S)
def pal256(i):
    if i >= 232: v = 8 + 10 * (i - 232); return (v, v, v)
    if i >= 16:
        i -= 16; lv = [0, 95, 135, 175, 215, 255]; return (lv[i // 36], lv[(i // 6) % 6], lv[i % 6])
    return (200, 200, 200)
maxw = 0
for ln in lines:
    fg = (210, 210, 210); bg = None; cells = []
    for m in tok.finditer(ln):
        if m.group(2) is not None:
            cells.append((m.group(2), fg, bg))
        else:
            p = [int(x) if x else 0 for x in m.group(1).split(";")]
            i = 0
            while i < len(p):
                if p[i] == 0: fg = (210, 210, 210); bg = None
                elif p[i] == 38 and p[i+1] == 2: fg = tuple(p[i+2:i+5]); i += 4
                elif p[i] == 48 and p[i+1] == 2: bg = tuple(p[i+2:i+5]); i += 4
                elif p[i] == 38 and p[i+1] == 5: fg = pal256(p[i+2]); i += 2
                elif p[i] == 48 and p[i+1] == 5: bg = pal256(p[i+2]); i += 2
                i += 1
    rows.append(cells); maxw = max(maxw, len(cells))
im = Image.new("RGB", (maxw * cw, len(rows) * ch), (10, 9, 8))
d = ImageDraw.Draw(im)
for y, cells in enumerate(rows):
    for x, (c, f, b) in enumerate(cells):
        if b: d.rectangle([x*cw, y*ch, (x+1)*cw, (y+1)*ch], fill=b)
        if c != " ": d.text((x*cw, y*ch), c, font=font, fill=f)
im.save(dst)
