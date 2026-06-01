#!/usr/bin/env python3
# Author app icons as crisp vector shapes, supersample-rasterize them to an
# anti-aliased grayscale COVERAGE atlas, and emit desktop-metal/src/icons.rs.
# The bare-metal renderer blits a coverage glyph tinted with the app's accent
# color (like the mockup's line-art icons) — "HD" icons without a runtime SVG
# engine. Output is committed so the no_std build needs no host tooling.
import sys
from PIL import Image, ImageDraw

PX = 24          # final icon size in px
SS = 4           # supersample factor
M  = PX * SS     # master canvas size
W  = int(SS * 1.7)  # stroke width in master space

def C(canvas):
    return ImageDraw.Draw(canvas)

def s(v):  # 0..24 logical -> master px
    return int(round(v * SS))

# Each icon draws white (255) line-art on a black canvas at master resolution.
def ic_term(d):
    d.rounded_rectangle([s(3),s(5),s(21),s(19)], radius=s(2.5), outline=255, width=W)
    d.line([s(7),s(9),s(10),s(12)], fill=255, width=W); d.line([s(10),s(12),s(7),s(15)], fill=255, width=W)
    d.line([s(12),s(15),s(17),s(15)], fill=255, width=W)

def ic_files(d):
    d.line([s(3),s(7),s(9),s(7)], fill=255, width=W)
    d.rounded_rectangle([s(3),s(7),s(21),s(19)], radius=s(2), outline=255, width=W)
    d.line([s(9),s(7),s(11),s(10)], fill=255, width=W); d.line([s(11),s(10),s(21),s(10)], fill=255, width=W)

def ic_edit(d):
    d.line([s(5),s(19),s(16),s(8)], fill=255, width=W+SS)
    d.line([s(16),s(8),s(19),s(11)], fill=255, width=W+SS)
    d.line([s(19),s(11),s(8),s(22)], fill=255, width=W+SS)
    d.polygon([s(4),s(20),s(7),s(19),s(5),s(22)], fill=255)

def ic_calc(d):
    d.rounded_rectangle([s(5),s(2),s(19),s(22)], radius=s(2.5), outline=255, width=W)
    d.rounded_rectangle([s(8),s(5),s(16),s(8)], radius=s(1), outline=255, width=max(2,W-SS))
    for ry in (12,15,18):
        for rx in (8.5,12,15.5):
            d.ellipse([s(rx)-W,s(ry)-W,s(rx)+W,s(ry)+W], fill=255)

def ic_settings(d):
    cx, cy = s(12), s(12)
    import math
    R, r = s(9), s(3.2)
    # 8 teeth
    for k in range(8):
        a = k*math.pi/4
        x1, y1 = cx+math.cos(a)*s(6), cy+math.sin(a)*s(6)
        x2, y2 = cx+math.cos(a)*s(10.5), cy+math.sin(a)*s(10.5)
        d.line([x1,y1,x2,y2], fill=255, width=W+SS)
    d.ellipse([cx-s(6),cy-s(6),cx+s(6),cy+s(6)], outline=255, width=W, fill=0)
    d.ellipse([cx-r,cy-r,cx+r,cy+r], outline=255, width=W)

def ic_tis(d):
    import math
    cx, cy = s(12), s(12)
    pts = [(cx+math.cos(math.radians(60*k-90))*s(9.5), cy+math.sin(math.radians(60*k-90))*s(9.5)) for k in range(6)]
    d.line([*[c for p in pts for c in p], *pts[0]], fill=255, width=W, joint="curve")
    d.line([cx,cy-s(5),cx,cy+s(5)], fill=255, width=W)
    d.line([cx-s(4),cy-s(2.5),cx,cy], fill=255, width=W); d.line([cx,cy,cx+s(4),cy-s(2.5)], fill=255, width=W)
    d.line([cx-s(4),cy+s(2.5),cx,cy], fill=255, width=W); d.line([cx,cy,cx+s(4),cy+s(2.5)], fill=255, width=W)

def ic_snake(d):
    d.line([s(6),s(6),s(6),s(12)], fill=255, width=W+SS)
    d.line([s(6),s(12),s(12),s(12)], fill=255, width=W+SS)
    d.line([s(12),s(12),s(12),s(18)], fill=255, width=W+SS)
    d.line([s(12),s(18),s(18),s(18)], fill=255, width=W+SS)
    d.ellipse([s(16),s(16),s(20),s(20)], fill=255)

def ic_mines(d):
    import math
    cx, cy = s(13), s(14)
    d.ellipse([cx-s(6),cy-s(6),cx+s(6),cy+s(6)], outline=255, width=W, fill=0)
    for a in range(0,360,45):
        x1,y1 = cx+math.cos(math.radians(a))*s(6), cy+math.sin(math.radians(a))*s(6)
        x2,y2 = cx+math.cos(math.radians(a))*s(9), cy+math.sin(math.radians(a))*s(9)
        d.line([x1,y1,x2,y2], fill=255, width=W)
    d.line([s(15),s(7),s(18),s(4)], fill=255, width=W)
    d.ellipse([s(17),s(2),s(21),s(6)], fill=255)

def ic_doom(d):
    d.rounded_rectangle([s(5),s(5),s(19),s(16)], radius=s(4), outline=255, width=W)
    d.ellipse([s(8),s(9),s(11),s(12)], fill=255); d.ellipse([s(13),s(9),s(16),s(12)], fill=255)
    pts=[s(6),s(16),s(8),s(20),s(10),s(16),s(12),s(20),s(14),s(16),s(16),s(20),s(18),s(16)]
    d.line(pts, fill=255, width=W)

def ic_web(d):
    cx, cy = s(12), s(12)
    d.ellipse([cx-s(9),cy-s(9),cx+s(9),cy+s(9)], outline=255, width=W)
    d.ellipse([cx-s(4),cy-s(9),cx+s(4),cy+s(9)], outline=255, width=max(2,W-SS))
    d.line([cx-s(8.5),cy-s(3.5),cx+s(8.5),cy-s(3.5)], fill=255, width=max(2,W-SS))
    d.line([cx-s(8.5),cy+s(3.5),cx+s(8.5),cy+s(3.5)], fill=255, width=max(2,W-SS))
    d.line([cx,cy-s(9),cx,cy+s(9)], fill=255, width=max(2,W-SS))

def ic_media(d):
    # Classic media "play" button — triangle inside a ring.
    cx, cy = s(12), s(12)
    d.ellipse([cx-s(9),cy-s(9),cx+s(9),cy+s(9)], outline=255, width=W)
    d.polygon([s(9.5),s(7),s(9.5),s(17),s(17),s(12)], fill=255)

def ic_shot(d):
    # Camera — body + lens + viewfinder bump (screenshot tool).
    d.rounded_rectangle([s(3),s(8),s(21),s(20)], radius=s(2.5), outline=255, width=W)
    d.rounded_rectangle([s(8),s(5),s(14),s(8)], radius=s(1), outline=255, width=max(2,W-SS))
    d.ellipse([s(9),s(11),s(15),s(17)], outline=255, width=W)

def ic_image(d):
    # Framed picture — sun + mountains (image viewer).
    d.rounded_rectangle([s(3),s(5),s(21),s(19)], radius=s(2), outline=255, width=W)
    d.ellipse([s(6.5),s(8),s(9.5),s(11)], outline=255, width=max(2,W-SS))   # sun
    d.line([s(4),s(18),s(10),s(12)], fill=255, width=W)                      # near peak
    d.line([s(10),s(12),s(13),s(15)], fill=255, width=W)
    d.line([s(12),s(14),s(17),s(9)], fill=255, width=W)                      # far peak
    d.line([s(17),s(9),s(20),s(13)], fill=255, width=W)

def ic_clock(d):
    cx, cy = s(12), s(12)
    d.ellipse([cx-s(9),cy-s(9),cx+s(9),cy+s(9)], outline=255, width=W)
    d.line([cx,cy,cx,cy-s(5.5)], fill=255, width=W+SS)    # hour hand
    d.line([cx,cy,cx+s(4.5),cy+s(2)], fill=255, width=W)  # minute hand

ICONS = [
    ("TERM", ic_term), ("FILES", ic_files), ("EDIT", ic_edit), ("CALC", ic_calc),
    ("SETTINGS", ic_settings), ("TIS", ic_tis), ("SNAKE", ic_snake),
    ("MINES", ic_mines), ("DOOM", ic_doom), ("WEB", ic_web), ("MEDIA", ic_media),
    ("SHOT", ic_shot), ("IMAGE", ic_image), ("CLOCK", ic_clock),
]

def main():
    cov = bytearray()
    meta = []  # (name, off)
    for name, fn in ICONS:
        big = Image.new("L", (M, M), 0)
        fn(C(big))
        small = big.resize((PX, PX), Image.LANCZOS)
        off = len(cov)
        cov.extend(small.tobytes())
        meta.append((name, off))
    with open("desktop-metal/src/icons.rs", "w") as f:
        f.write("// AUTO-GENERATED by scripts/gen_icons.py — do not edit by hand.\n")
        f.write("// Anti-aliased icon coverage atlas; blit tinted with an accent color.\n\n")
        f.write(f"pub const ICON_PX: usize = {PX};\n")
        f.write(f"pub const ICON_SZ: usize = {PX*PX};\n")
        for i,(name,off) in enumerate(meta):
            f.write(f"pub const IC_{name}: usize = {i};\n")
        f.write(f"\npub static ICON_OFF: [u32; {len(meta)}] = [")
        f.write(",".join(str(o) for _,o in meta)); f.write("];\n")
        f.write(f"pub static ICON_COV: [u8; {len(cov)}] = [")
        f.write(",".join(str(b) for b in cov)); f.write("];\n")
    print(f"icons={len(meta)} px={PX} cov={len(cov)}B", file=sys.stderr)

if __name__ == "__main__":
    main()
