"""Draws the Mall Pastel '95 neon signs for the menu bar and the splash:
`assets/themes/mall-{day,night}-neon.png`.

"Audiform" in a flowing 1980s neon script with a double swoosh under it.
The letters are hand-built tube centrelines (smooth curves through points
on a grid, no font), drawn at 4x the size the menu bar shows them. Each
tube is the unlit glass on its backing (a dark offset shadow), a wide soft
bloom, a coloured inner glow, the saturated glass and a white-hot core.

    python -I scripts/make-mall-neon.py
"""
import os

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "assets", "themes")

SCALE = 4  # pixels per point in the menu bar
XH = 9.0  # x-height, points
PAD = 12.0  # room round the sign for its bloom, points
SLANT = 0.24


def spline(pts, per=14):
    """A Catmull-Rom curve through `pts`."""
    p = [pts[0]] + list(pts) + [pts[-1]]
    out = []
    for i in range(1, len(p) - 2):
        p0, p1, p2, p3 = (np.array(q, float) for q in p[i - 1 : i + 3])
        for t in np.linspace(0, 1, per, endpoint=False):
            t2, t3 = t * t, t * t * t
            out.append(
                0.5
                * (
                    2 * p1
                    + (-p0 + p2) * t
                    + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t2
                    + (-p0 + 3 * p1 - 3 * p2 + p3) * t3
                )
            )
    out.append(np.array(pts[-1], float))
    return out


# Tube centrelines, x-height 1, baseline 0, y up. (points, colour index)
SCRIPT = [
    # A: a swash up to the apex and down into the u.
    ([(-0.05, 0.05), (0.35, 0.7), (0.8, 1.45), (1.02, 1.78), (1.18, 1.45),
      (1.38, 0.7), (1.55, 0.12), (1.78, 0.02), (1.98, 0.4), (2.08, 1.0)], 0),
    # its crossbar, with a flick
    ([(0.12, 0.62), (0.7, 0.8), (1.3, 0.82), (1.62, 0.74)], 0),
    # u
    ([(2.08, 1.0), (2.02, 0.35), (2.22, 0.0), (2.52, 0.18), (2.7, 0.62),
      (2.76, 1.0), (2.72, 0.35), (2.9, 0.0), (3.15, 0.25), (3.45, 0.72),
      (3.68, 0.92)], 0),
    # d: a round bowl, up its right side to the top of the stem, back down
    ([(3.68, 0.92), (3.36, 1.0), (3.1, 0.66), (3.12, 0.16), (3.4, 0.0),
      (3.66, 0.26), (3.76, 0.9), (3.88, 1.78), (3.8, 1.2), (3.74, 0.5),
      (3.74, 0.12), (3.92, 0.0), (4.16, 0.32), (4.3, 0.95)], 0),
    # i, then f with its loops
    ([(4.3, 0.95), (4.26, 0.3), (4.44, 0.0), (4.74, 0.3), (5.06, 0.95),
      (5.3, 1.6), (5.24, 1.86), (5.03, 1.62), (4.94, 0.9), (4.88, 0.0),
      (4.8, -0.62), (4.62, -0.78), (4.54, -0.55), (4.78, -0.22), (5.18, 0.05),
      (5.5, 0.5), (5.78, 0.95)], 0),
    # o
    ([(5.78, 0.95), (5.52, 0.82), (5.48, 0.25), (5.75, 0.0), (6.02, 0.3),
      (5.98, 0.82), (5.78, 0.95), (6.08, 0.9), (6.3, 0.95)], 0),
    # r
    ([(6.3, 0.95), (6.45, 1.04), (6.5, 0.55), (6.52, 0.02), (6.58, 0.55),
      (6.78, 0.93), (7.02, 0.98), (7.1, 0.9)], 0),
    # m, ending in a tail
    ([(7.1, 0.9), (7.22, 1.0), (7.26, 0.5), (7.26, 0.02), (7.32, 0.6),
      (7.56, 0.98), (7.8, 0.8), (7.82, 0.02), (7.88, 0.6), (8.12, 0.98),
      (8.36, 0.8), (8.38, 0.18), (8.55, 0.0), (8.85, 0.18)], 0),
    # the i's dot and the f's crossbar
    ([(4.33, 1.3), (4.36, 1.34)], 1),
    ([(4.7, 0.98), (5.5, 1.02)], 1),
    # two swooshes under it all
    ([(0.35, -0.42), (2.5, -0.5), (5.6, -0.48), (7.6, -0.3), (8.7, -0.08)], 1),
    ([(1.0, -0.72), (3.2, -0.8), (6.0, -0.76), (7.9, -0.56)], 1),
]


def render(colours, glow, name, backing, SCALE=SCALE):
    xs = [x + y * SLANT for pts, _ in SCRIPT for x, y in pts]
    ys = [y for pts, _ in SCRIPT for _, y in pts]
    u = XH * SCALE
    pad = PAD * SCALE
    W = int((max(xs) - min(xs)) * u + 2 * pad)
    H = int((max(ys) - min(ys)) * u + 2 * pad)
    ox, oy = pad - min(xs) * u, pad + max(ys) * u

    def to_px(x, y):
        return (ox + (x + y * SLANT) * u, oy - y * u)

    tube_w = 0.25 * u
    core_w = 0.085 * u
    masks = {}
    for pts, ci in SCRIPT:
        curve = [to_px(x, y) for x, y in spline(pts)]
        tm, cm = masks.setdefault(ci, (Image.new("L", (W, H), 0), Image.new("L", (W, H), 0)))
        for img, wd in ((tm, tube_w), (cm, core_w)):
            d = ImageDraw.Draw(img)
            d.line(curve, fill=255, width=int(round(wd)), joint="curve")
            r = wd / 2
            for px, py in (curve[0], curve[-1]):
                d.ellipse((px - r, py - r, px + r, py + r), fill=255)

    acc = np.zeros((H, W, 4))

    def over(rgb, alpha):
        a = np.clip(alpha, 0, 1)[..., None]
        src = np.concatenate([np.array(rgb, float)[None, None, :] / 255 * a, a], -1)
        acc[:] = src + acc * (1 - a)

    allt = np.zeros((H, W))
    for tm, _ in masks.values():
        allt = np.maximum(allt, np.array(tm) / 255)
    # The glass on its backing: a dark tube shadow, down and to the right.
    sh = Image.fromarray((allt * 255).astype(np.uint8))
    sh = sh.transform(sh.size, Image.AFFINE, (1, 0, -1.2 * SCALE, 0, 1, -1.6 * SCALE))
    sh = np.array(sh.filter(ImageFilter.GaussianBlur(0.9 * SCALE))) / 255
    over(backing[0], sh * backing[1])
    for ci, (tm, cm) in masks.items():
        rgb = colours[ci]
        t = np.array(tm) / 255
        bloom = np.array(tm.filter(ImageFilter.GaussianBlur(4.5 * SCALE))) / 255
        inner = np.array(tm.filter(ImageFilter.GaussianBlur(1.4 * SCALE))) / 255
        core = np.array(cm.filter(ImageFilter.GaussianBlur(0.4 * SCALE))) / 255
        over(rgb, np.clip(bloom * 1.5, 0, 1) * glow[0])
        over(rgb, np.clip(inner * 1.7, 0, 1) * glow[1])
        over(rgb, t)
        hot = tuple(int(c + (255 - c) * 0.88) for c in rgb)
        over(hot, core)
    # Fade to nothing inside the picture's edges, so no bloom is cut off.
    yy, xx = np.mgrid[0:H, 0:W]
    edge = np.minimum(np.minimum(xx, W - 1 - xx), np.minimum(yy, H - 1 - yy)) / (4.0 * SCALE)
    acc *= np.clip(edge, 0, 1)[..., None] ** 2
    a = acc[..., 3:4]
    rgb = np.where(a > 1e-6, acc[..., :3] / np.maximum(a, 1e-6), 0)
    img = Image.fromarray(np.clip(np.concatenate([rgb, a], -1) * 255 + 0.5, 0, 255).astype(np.uint8), "RGBA")
    path = os.path.join(OUT, name)
    img.save(path, optimize=True)
    print(path, img.size)


DAY = (((255, 46, 160), (124, 77, 255)), (0.6, 0.85))
NIGHT = (((255, 120, 190), (90, 240, 220)), (0.8, 1.0))
# Day: hot pink script, electric violet swooshes, on pale marble.
# Night: pink script, mint-cyan swooshes, glowing hard in the dark.
# The menu bar's at 4x, the splash's (shown about 6x larger) at 16x.
for big, scale in (("", SCALE), ("-big", 16)):
    render(*DAY, f"mall-day-neon{big}.png", ((70, 40, 110), 0.45), scale)
    render(*NIGHT, f"mall-night-neon{big}.png", ((5, 3, 12), 0.6), scale)
