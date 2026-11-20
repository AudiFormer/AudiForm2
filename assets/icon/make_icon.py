# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (c) The Audiform27 contributors.
"""Builds the AudiForm 2 app icon from the logo mark's geometry.

Every size is drawn on its own pixel grid (not shrunk from the 256), as
five white-on-clear SVG layers, rasterised in one headless Edge run:

    tile    the rounded-square tile
    rim     a thin light along the tile's edge
    ink     the white of the A (slices, laser core, frame, ruler)
    main    the first light (cyan in Zero Point): chromatic A edge, the 2
    second  the second light (magenta): the laser, the 2's echo

From those come, per size:

    masks/mask-N.png  R = main, G = second, B = ink, A = tile
    masks/fx-N.png    R = main glow, G = second glow, B = rim, A = halo

which the app recolours per theme (app/src/app_icon.rs), plus the
Zero Point icon (audiform2.ico, audiform2-N.png), the themes' own icon
pictures (pictures/NAME-N.png) and preview sheets.

    python -I assets/icon/make_icon.py [preview-dir]
"""

import base64
import io
import math
import os
import struct
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image, ImageFilter

HERE = os.path.dirname(os.path.abspath(__file__))
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
LAYERS = ["tile", "rim", "ink", "main", "second", "deco"]
EDGE = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"


def level(n):
    if n <= 20:
        return 0
    if n <= 32:
        return 1
    if n <= 64:
        return 2
    return 3


# ------------------------------------------------------------------ geometry

def fmt(v):
    return ("%.3f" % v).rstrip("0").rstrip(".")


def pts(p):
    return " ".join(fmt(x) + "," + fmt(y) for x, y in p)


def superellipse(cx, cy, r, e=5.0, steps=256):
    out = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        out.append((cx + r * math.copysign(abs(c) ** (2 / e), c),
                    cy + r * math.copysign(abs(s) ** (2 / e), s)))
    return out


# The mark's right leg: nine sliced parallelograms (x at the top-left
# corner, y, in the 512 design space); 54 wide, 31 tall, sheared 11.4.
SLICES = [(234.1, 118), (257.7, 155), (254.3, 192), (292.8, 229),
          (276.4, 266), (306.0, 303), (341.5, 340), (325.1, 377)]
LEFT_LEG = [(234, 104), (278, 104), (173, 404), (119, 404)]
# Glitch offsets (design units) for the fewer, taller slices of small sizes.
JITTER = {3: [0, 8, 0], 4: [0, 12, -2, 8], 5: [0, 12, -4, 14, 0],
          6: [0, 10, -4, 14, -4, 10]}
SLICE_COUNT = {16: 4, 20: 4, 24: 4, 32: 4, 40: 5, 48: 5, 64: 6}


def design(n, variant="base"):
    """SVG bodies (pixel coordinates, viewBox 0 0 n n) for each layer."""
    lv = level(n)
    L = {k: [] for k in LAYERS}
    margin = {16: 0, 20: 0, 24: 0, 32: 0, 40: 1, 48: 1, 64: 2, 128: 5, 256: 10}[n]
    half = n / 2 - margin
    tile = superellipse(n / 2, n / 2, half, 5.0 if lv >= 2 else 4.2)
    tile_poly = '<polygon points="%s"/>' % pts(tile)
    L["tile"].append(tile_poly)
    rim_w = 1.0 if n < 128 else n / 128
    L["rim"].append(
        '<clipPath id="tc">%s</clipPath><polygon points="%s" fill="none" '
        'stroke="#fff" stroke-width="%s" clip-path="url(#tc)"/>'
        % (tile_poly, pts(tile), fmt(rim_w * 2)))

    if lv == 3:
        # The mark as drawn, scaled to sit in the tile.
        s = 0.98 * (2 * half) / 492 * 512 / n

        def T(x, y):
            return (n / 2 + (x - 256) * s * n / 512, n / 2 + (y - 256) * s * n / 512)
        u = s * n / 512           # px per design unit
        snap = lambda v: v
    else:
        k = {0: 1.34, 1: 1.24, 2: 1.12}[lv]
        cx, cy = 276, 256

        def T(x, y):
            return (n / 2 + (x - cx) * k * n / 512, n / 2 + (y - cy) * k * n / 512)
        u = k * n / 512
        snap = round

    def poly(p):
        return '<polygon points="%s"/>' % pts(p)

    # ---- the laser's band (cut out of everything above it)
    lx0, ly = T(0, 304)
    laser_w = {16: 1, 20: 1, 24: 1, 32: 2, 40: 2, 48: 2, 64: 3}.get(n, 10 * u)
    if lv < 3:
        ly = math.floor(ly) + 0.5 if laser_w % 2 == 1 else round(ly)
    band = (ly - laser_w / 2, ly + laser_w / 2)

    # ---- the A
    leg_f = {0: 1.5, 1: 1.3, 2: 1.12, 3: 1.0}[lv]
    top_y, bot_y = T(0, 104)[1], T(0, 404)[1]
    if lv < 3:
        top_y, bot_y = snap(top_y), snap(bot_y)

    def leg(dx):
        # widen about the leg's centre line
        (ax, _), (bx, _) = T(234, 104), T(278, 104)
        (cx_, _), (dx_, _) = T(173, 404), T(119, 404)
        tc, tw = (ax + bx) / 2, (bx - ax) / 2 * leg_f
        bc, bw = (cx_ + dx_) / 2, (cx_ - dx_) / 2 * leg_f
        return [(tc - tw + dx, top_y), (tc + tw + dx, top_y),
                (bc + bw + dx, bot_y), (bc - bw + dx, bot_y)]

    def slices(dx):
        out = []
        if lv == 3:
            for x, y in SLICES:
                p = [T(x, y), T(x + 54, y), T(x + 65.4, y + 31), T(x + 11.4, y + 31)]
                if y == 377:
                    p = [T(x, y), T(x + 54, y), T(x + 63.9, 404), T(x + 9.9, 404)]
                out.append([(a + dx, b) for a, b in p])
            return out
        cnt = SLICE_COUNT[n]
        y0 = snap(T(0, 118)[1])
        y1 = bot_y
        gap = 0 if n <= 24 else 1
        hs = (y1 - y0 - gap * (cnt - 1)) / cnt
        w = 54 * u * {0: 1.0, 1: 1.0, 2: 1.0}[lv]
        spread = {0: 1.45, 1: 1.35, 2: 1.2}[lv]
        for i in range(cnt):
            a = y0 + i * (hs + gap)
            b = a + hs
            a, b = round(a), round(b)
            ymid_design = 118 + (i + 0.5) / cnt * (404 - 118)
            xd = 234 + (ymid_design - 133.5) * 0.351 * spread + JITTER[cnt][i]
            xl = T(xd, 0)[0]
            if lv <= 1:
                xl = round(xl + dx)
                xr = xl + max(2, round(w))
            else:
                xl = round((xl + dx) * 2) / 2
                xr = xl + round(w * 2) / 2
            out.append([(xl, a), (xr, a), (xr, b), (xl, b)])
        return out

    A_ink = [poly(leg(0))] + [poly(p) for p in slices(0)]
    off = {0: 1, 1: 1, 2: 1.5}.get(lv, 6 * u)
    A_main = [poly(leg(-off))] + [poly(p) for p in slices(-off)]
    A_second = [poly(leg(off))] + [poly(p) for p in slices(off)]

    # ---- the 2
    def two(dx, dy, w):
        x0, y0 = T(314 + dx, 214 + dy)
        x1, y1 = T(410 + dx, 250 + dy)
        x2, y2 = T(318 + dx, 372 + dy)
        x3, y3 = T(426 + dx, 372 + dy)
        r = 54 * u * (1 if lv == 3 else 1.0)
        if lv < 3:
            yb = math.floor(y2) + 0.5 if round(w) % 2 == 1 else round(y2)
            y2 = y3 = yb
            x3 = round(x3)
        return ('<path d="M%s %s A%s %s 0 1 1 %s %s L%s %s L%s %s" fill="none" '
                'stroke="#fff" stroke-width="%s" stroke-linecap="square" '
                'stroke-linejoin="miter"/>'
                % tuple(fmt(v) for v in (x0, y0, r, r, x1, y1, x2, y2, x3, y3, w)))

    two_dx = {0: 0, 1: 30, 2: 22, 3: 0}[lv]
    two_w = {24: 1.0, 32: 1.5, 40: 1.75, 48: 2.0, 64: 2.5}.get(n, 12 * u)

    # ---- assemble
    cut = ('<mask id="cut" maskUnits="userSpaceOnUse" x="0" y="0" width="%d" height="%d">'
           '<rect width="%d" height="%d" fill="#fff"/>'
           '<rect x="0" y="%s" width="%d" height="%s" fill="#000"/></mask>'
           % (n, n, n, n, fmt(band[0]), n, fmt(band[1] - band[0])))

    # Below 128 px the 2 is drawn in front of the A (cut out of it), so
    # that it still reads as a 2 where it crosses the slices.
    front = ''
    if 32 <= n < 128:
        front = two(-3 + two_dx, -2, two_w + 1.5).replace('#fff', '#000')
    cut2 = cut.replace('id="cut"', 'id="cut2"').replace('</mask>', front + '</mask>')
    main, second, ink = [], [], []
    if n >= 32:
        main.append(two(-3 + two_dx, -2, two_w))
    if lv >= 2:
        second.append('<g opacity="0.9">%s</g>' % two(6 + two_dx, 4, two_w))
    main.append('<g opacity="0.8">%s</g>' % "".join(A_main))
    if lv >= 1:
        second.append('<g opacity="0.75">%s</g>' % "".join(A_second))
    ink.extend(A_ink)

    if lv == 3:
        # chamfered frame, broken where the laser passes
        fr = [T(60, 60), T(404, 60), T(452, 108), T(452, 452), T(108, 452), T(60, 404)]
        ink.append('<g opacity="0.55"><polygon points="%s" fill="none" stroke="#fff" '
                   'stroke-width="%s"/></g>' % (pts(fr), fmt(max(1.0, 5 * u))))
        bw = max(1.5, 6 * u)
        for a, b, c in [((70, 104), (70, 70), (104, 70)), ((442, 408), (442, 442), (408, 442))]:
            main.append('<polyline points="%s" fill="none" stroke="#fff" stroke-width="%s"/>'
                        % (pts([T(*a), T(*b), T(*c)]), fmt(bw)))
        if n >= 256:
            ticks = []
            for i, x in enumerate(range(116, 381, 24)):
                h = 16 if i % 4 == 0 else 8
                (xa, ya), (_, yb) = T(x, 60), T(x, 60 + h)
                ticks.append('<line x1="%s" y1="%s" x2="%s" y2="%s" stroke="#fff" stroke-width="%s"/>'
                             % (fmt(xa), fmt(ya), fmt(xa), fmt(yb), fmt(2 * u)))
            ink.append('<g opacity="0.45">%s</g>' % "".join(ticks))

    # the laser itself, edge to edge of the tile
    tl, tr = n / 2 - half, n / 2 + half
    if lv == 3:
        lx_end = tr - 16 * u
        second.append('<rect x="%s" y="%s" width="%s" height="%s"/>'
                      % (fmt(tl), fmt(band[0]), fmt(lx_end - tl), fmt(laser_w)))
        d = 12 * u
        second.append(poly([(lx_end + d, ly), (lx_end, ly - d), (lx_end - d, ly), (lx_end, ly + d)]))
        core = '<rect x="%s" y="%s" width="%s" height="%s"/>' % (
            fmt(tl), fmt(ly - u), fmt(lx_end - d - tl), fmt(2 * u))
        dia = '<polygon points="%s" fill="none" stroke="#fff" stroke-width="%s"/>' % (
            pts([(lx_end + d, ly), (lx_end, ly - d), (lx_end - d, ly), (lx_end, ly + d)]), fmt(2 * u))
        tail = [core, dia]
    else:
        second.append('<rect x="0" y="%s" width="%d" height="%s"/>'
                      % (fmt(band[0]), n, fmt(laser_w)))
        tail = []
        if n >= 32:
            d = {32: 2.5, 40: 3, 48: 3.5, 64: 4.5}[n]
            xe = tr - d - (0.5 if n < 48 else 1)
            second.append(poly([(xe + d, ly), (xe, ly - d), (xe - d, ly), (xe, ly + d)]))
        if n == 64:
            tail.append('<rect x="0" y="%s" width="%s" height="1"/>' % (fmt(ly - 0.5), fmt(tr - 9)))

    L["second"] = second
    L["main"] = ['<g mask="url(#cut)">%s</g>' % "".join(main)]
    L["ink"] = ['<g mask="url(#cut2)">%s</g>' % "".join(ink)] + tail
    L["_defs"] = cut + cut2

    # ---- decoration for the themes' own pictures
    if variant in ("pastel", "pastel-night") and lv >= 2:
        sx, sy = T(318, 196)
        r = 120 * u
        stripes = []
        yy = sy + r * 0.05
        hgt = max(0.75, r * 0.07)
        i = 0
        while yy < sy + r:
            stripes.append('<rect x="%s" y="%s" width="%s" height="%s" fill="#000"/>'
                           % (fmt(sx - r), fmt(yy), fmt(2 * r), fmt(hgt * (1 + i * 0.35))))
            yy += hgt * (1 + i * 0.35) + max(1.0, r * 0.12)
            i += 1
        L["deco"].append(
            '<mask id="sun" maskUnits="userSpaceOnUse" x="0" y="0" width="%d" height="%d">'
            '<rect width="%d" height="%d" fill="#fff"/>%s</mask>'
            '<circle cx="%s" cy="%s" r="%s" mask="url(#sun)"/>'
            % (n, n, n, n, "".join(stripes), fmt(sx), fmt(sy), fmt(r)))
    if variant == "undercurrent" and lv >= 2:
        sx, sy = T(170, 150)
        rings = []
        for i, rr in enumerate([40, 90, 150, 220, 300]):
            rings.append('<circle cx="%s" cy="%s" r="%s" fill="none" stroke="#fff" '
                         'stroke-width="%s" opacity="%s"/>'
                         % (fmt(sx), fmt(sy), fmt(rr * u * 1.1), fmt(max(1.0, 7 * u)),
                            fmt(0.9 - i * 0.14)))
        L["deco"].append('<g clip-path="url(#tc2)">%s</g><clipPath id="tc2">%s</clipPath>'
                         % ("".join(rings), tile_poly))
    return L


def svg_for(n, body, defs):
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="%d" height="%d" '
            'viewBox="0 0 %d %d"><defs>%s</defs><g fill="#fff">%s</g></svg>'
            % (n, n, n, n, defs, "".join(body)))


# ------------------------------------------------------------------ raster

def rasterise(variant):
    """{size: {layer: float32 coverage array}} via one headless Edge run."""
    imgs, rows, y = [], [], 0
    for n in SIZES:
        L = design(n, variant)
        for i, name in enumerate(LAYERS):
            svg = svg_for(n, L[name], L["_defs"])
            uri = "data:image/svg+xml;base64," + base64.b64encode(svg.encode()).decode()
            imgs.append('<img src="%s" style="position:absolute;left:%dpx;top:%dpx;'
                        'width:%dpx;height:%dpx">' % (uri, i * 260, y, n, n))
        rows.append((n, y))
        y += n + 4
    W, H = 260 * len(LAYERS), y
    tmp = tempfile.mkdtemp()
    page = os.path.join(tmp, "layers.html")
    shot = os.path.join(tmp, "layers.png")
    with open(page, "w") as f:
        f.write('<html><body style="margin:0;background:transparent">%s</body></html>'
                % "".join(imgs))
    subprocess.run([EDGE, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                    "--force-device-scale-factor=1", "--default-background-color=00000000",
                    "--window-size=%d,%d" % (W, H), "--screenshot=" + shot,
                    "file:///" + page.replace("\\", "/")],
                   check=True, capture_output=True)
    a = np.asarray(Image.open(shot).convert("RGBA")).astype(np.float32) / 255
    out = {}
    for n, y0 in rows:
        out[n] = {name: a[y0:y0 + n, i * 260:i * 260 + n, 3].copy()
                  for i, name in enumerate(LAYERS)}
    return out


def blur(a, sigma):
    if sigma <= 0:
        return a * 0
    im = Image.fromarray(np.clip(a * 255, 0, 255).astype(np.uint8), "L")
    return np.asarray(im.filter(ImageFilter.GaussianBlur(sigma))).astype(np.float32) / 255


def masks_for(cov, n):
    lv = level(n)
    tile = cov["tile"]
    ramp = np.linspace(1.0, 0.35, n, dtype=np.float32)[:, None]
    rim = cov["rim"] * ramp
    sig = n * 0.045
    g_main = np.clip(blur(cov["main"] * 0.8 + cov["ink"] * 0.25, sig) * 1.6, 0, 1)
    g_second = np.clip(blur(cov["second"], sig * 0.8) * 1.8, 0, 1)
    damp = {0: 0.25, 1: 0.4, 2: 0.55, 3: 1.0}[lv]
    g_main *= damp
    g_second *= damp
    halo = np.clip(blur(tile, max(n * 0.022, 0.5)) * 1.1 - tile, 0, 1) if lv >= 3 else tile * 0
    mask = np.stack([cov["main"], cov["second"], cov["ink"], tile], -1)
    fx = np.stack([g_main, g_second, rim, halo], -1)
    q = lambda x: np.clip(np.round(x * 255), 0, 255).astype(np.uint8)
    return q(mask), q(fx)


# ------------------------------------------------------------------ colour
# Mirrors app/src/app_icon.rs (keep the two in step).

def lum(c):
    def ch(v):
        v = v / 255
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def contrast(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


def mix(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def legible(c, tile, want):
    """c pushed toward white (dark tile) or black (light tile) until it
    stands out from the tile by `want`."""
    target = (255, 255, 255) if lum(tile) < 0.18 else (0, 0, 0)
    for i in range(21):
        m = mix(c, target, i / 20)
        if contrast(m, tile) >= want:
            return m
    return target


def palette(th):
    """Icon colours for a theme: dict(tile, ink, main, second, rim, halo, dark)."""
    dark = lum(th["background"]) < 0.18
    if dark:
        tile = th["background"]
        if lum(tile) > 0.03:
            tile = mix(tile, (0, 0, 0), 0.4)
        ink = th["text"] if contrast(th["text"], tile) >= 7 else (242, 248, 255)
        main = legible(th["accent"], tile, 3.0)
        second = legible(th["accent_alt"], tile, 3.0)
        rim = mix(main, (255, 255, 255), 0.35)
        halo = main
    else:
        tile = th["panel"]
        ink = th["text"]
        main = legible(th["accent"], tile, 3.0)
        second = legible(th["accent_alt"], tile, 3.0)
        rim = mix(tile, ink, 0.55)
        halo = ink
    return dict(tile=tile, ink=ink, main=main, second=second, rim=rim, halo=halo, dark=dark)


def composite(mask, fx, p, tile_grad=None, deco=None):
    m = mask.astype(np.float32) / 255
    f = fx.astype(np.float32) / 255
    n = m.shape[0]
    c = lambda v: np.array(v, np.float32)[None, None, :] / 255
    glow_k = 0.55 if p["dark"] else 0.25
    halo_k = 0.6 if p["dark"] else 0.35
    tile = c(p["tile"]) if tile_grad is None else tile_grad
    tile = tile + (c(p["main"]) - tile) * (f[..., 0:1] * glow_k)
    tile = tile + (c(p["second"]) - tile) * (f[..., 1:2] * glow_k)
    if deco is not None:
        dcol, dcov = deco
        tile = tile + (dcol - tile) * dcov[..., None]
    tile = tile + (c(p["rim"]) - tile) * (f[..., 2:3] * 0.6)
    rgb = np.zeros((n, n, 3), np.float32)
    a = np.zeros((n, n, 1), np.float32)

    def over(col, cov):
        nonlocal rgb, a
        cov = cov[..., None] if cov.ndim == 2 else cov
        rgb = col * cov + rgb * (1 - cov)
        a = cov + a * (1 - cov)
    over(c(p["halo"]), f[..., 3] * halo_k)
    over(tile, m[..., 3])
    over(c(p["second"]), m[..., 1])
    over(c(p["main"]), m[..., 0])
    over(c(p["ink"]), m[..., 2])
    out = np.where(a > 0, rgb / np.maximum(a, 1e-6), 0)
    img = np.concatenate([out, a], -1)
    return Image.fromarray(np.clip(np.round(img * 255), 0, 255).astype(np.uint8), "RGBA")


THEMES = {
    "Zero Point": dict(background=(3, 4, 10), panel=(8, 11, 24), text=(214, 232, 255),
                       accent=(0, 234, 255), accent_alt=(255, 46, 166)),
    "Solar Flare": dict(background=(8, 4, 2), panel=(16, 9, 4), text=(255, 228, 196),
                        accent=(255, 158, 24), accent_alt=(255, 60, 40)),
    "Daylight": dict(background=(236, 238, 245), panel=(246, 247, 252), text=(25, 28, 45),
                     accent=(30, 110, 255), accent_alt=(230, 40, 120)),
    "Grey Desktop '95": dict(background=(192, 192, 192), panel=(192, 192, 192), text=(0, 0, 0),
                             accent=(0, 0, 128), accent_alt=(0, 128, 128)),
    "Phosphor": dict(background=(4, 10, 5), panel=(7, 17, 8), text=(109, 255, 142),
                     accent=(141, 255, 170), accent_alt=(182, 255, 200)),
    "Moonlight Aero": dict(background=(8, 20, 48), panel=(10, 24, 52), text=(230, 244, 255),
                           accent=(79, 210, 255), accent_alt=(255, 138, 26)),
}

# The themes' own pictures: fixed palettes, a tile gradient, decoration.
PICTURES = {
    "pastel": dict(tile_top=(255, 122, 184), tile_bottom=(132, 98, 224),
                   p=dict(tile=(200, 110, 205), ink=(255, 255, 255), main=(95, 245, 216),
                          second=(72, 28, 160), rim=(255, 236, 248), halo=(255, 122, 184),
                          dark=True),
                   deco_top=(255, 236, 140), deco_bottom=(255, 92, 170), deco_a=0.95),
    "pastel-night": dict(tile_top=(52, 30, 104), tile_bottom=(14, 8, 34),
                         p=dict(tile=(30, 18, 64), ink=(255, 255, 255), main=(120, 255, 226),
                                second=(255, 92, 184), rim=(196, 170, 255), halo=(179, 156, 255),
                                dark=True),
                         deco_top=(255, 214, 120), deco_bottom=(255, 70, 170), deco_a=0.9),
    "undercurrent": dict(tile_top=(16, 66, 120), tile_bottom=(4, 10, 36),
                         p=dict(tile=(6, 22, 60), ink=(232, 244, 245), main=(76, 210, 226),
                                second=(216, 232, 74), rim=(150, 230, 238), halo=(76, 210, 226),
                                dark=True),
                         deco_top=(76, 210, 226), deco_bottom=(47, 111, 208), deco_a=0.35),
}


def picture(name, cov, mask, fx, n):
    pic = PICTURES[name]
    t = np.linspace(0, 1, n, dtype=np.float32)[:, None, None]
    top = np.array(pic["tile_top"], np.float32)[None, None, :] / 255
    bot = np.array(pic["tile_bottom"], np.float32)[None, None, :] / 255
    grad = top + (bot - top) * t
    dtop = np.array(pic["deco_top"], np.float32)[None, None, :] / 255
    dbot = np.array(pic["deco_bottom"], np.float32)[None, None, :] / 255
    dcol = dtop + (dbot - dtop) * t
    return composite(mask, fx, pic["p"], grad, (dcol, cov["deco"] * pic["deco_a"]))


# ------------------------------------------------------------------ ico

def write_ico(path, images):
    """Multi-size ICO: 32-bit BMP entries below 256, a PNG entry at 256."""
    blobs = []
    for im in images:
        w, h = im.size
        if w >= 256:
            b = io.BytesIO()
            im.save(b, "PNG", optimize=True)
            blobs.append((w, h, b.getvalue()))
            continue
        a = np.asarray(im)
        bgra = a[::-1, :, [2, 1, 0, 3]].tobytes()
        row = ((w + 31) // 32) * 4
        andm = bytearray(row * h)
        for y in range(h):
            for x in range(w):
                if a[h - 1 - y, x, 3] == 0:
                    andm[y * row + x // 8] |= 0x80 >> (x % 8)
        hdr = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(bgra) + len(andm), 0, 0, 0, 0)
        blobs.append((w, h, hdr + bgra + bytes(andm)))
    out = struct.pack("<HHH", 0, 1, len(blobs))
    offset = 6 + 16 * len(blobs)
    for w, h, data in blobs:
        out += struct.pack("<BBBBHHII", w % 256, h % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, _, data in blobs:
        out += data
    with open(path, "wb") as f:
        f.write(out)


# ------------------------------------------------------------------ sheets

def sheet(rows, path, zoom_small=True):
    """rows: [(label, {size: Image})]; each row on light and dark."""
    from PIL import ImageDraw
    pad = 12
    width_1x = sum(SIZES) + pad * (len(SIZES) + 1)
    zoom_sizes = [16, 20, 24, 32, 48]
    zw = sum(s * 4 for s in zoom_sizes) + pad * (len(zoom_sizes) + 1) if zoom_small else 0
    rowh = 256 + 2 * pad
    W = 170 + width_1x + zw
    H = rowh * 2 * len(rows)
    im = Image.new("RGBA", (W, H), (255, 255, 255, 255))
    d = ImageDraw.Draw(im)
    for r, (label, icons) in enumerate(rows):
        for b, bg in enumerate([(243, 243, 243, 255), (32, 32, 32, 255)]):
            y0 = (r * 2 + b) * rowh
            d.rectangle([0, y0, W, y0 + rowh], fill=bg)
            d.text((8, y0 + 8), label + (" / light" if b == 0 else " / dark"),
                   fill=(0, 0, 0, 255) if b == 0 else (255, 255, 255, 255))
            x = 170 + pad
            for s in SIZES:
                ic = icons[s]
                im.alpha_composite(ic, (x, y0 + pad + (256 - s)))
                x += s + pad
            if zoom_small:
                for s in zoom_sizes:
                    z = icons[s].resize((s * 4, s * 4), Image.NEAREST)
                    im.alpha_composite(z, (x, y0 + pad + (256 - s * 4)))
                    x += s * 4 + pad
    im.save(path)


def main():
    preview = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "preview")
    os.makedirs(preview, exist_ok=True)
    os.makedirs(os.path.join(HERE, "masks"), exist_ok=True)
    os.makedirs(os.path.join(HERE, "pictures"), exist_ok=True)
    cov = rasterise("base")
    masks = {}
    for n in SIZES:
        mask, fx = masks_for(cov[n], n)
        masks[n] = (mask, fx)
        Image.fromarray(mask, "RGBA").save(os.path.join(HERE, "masks", "mask-%d.png" % n), optimize=True)
        Image.fromarray(fx, "RGBA").save(os.path.join(HERE, "masks", "fx-%d.png" % n), optimize=True)
    rows = []
    for name, th in THEMES.items():
        p = palette(th)
        icons = {n: composite(*masks[n], p) for n in SIZES}
        rows.append((name, icons))
        if name == "Zero Point":
            zp = icons
    for n in SIZES:
        zp[n].save(os.path.join(HERE, "audiform2-%d.png" % n), optimize=True)
    write_ico(os.path.join(HERE, "audiform2.ico"), [zp[n] for n in SIZES])
    sheet(rows[:1], os.path.join(preview, "icon-sheet.png"))
    sheet(rows, os.path.join(preview, "icon-themes.png"))
    prow = []
    for name in PICTURES:
        pc = rasterise(name)
        icons = {}
        for n in SIZES:
            mask, fx = masks_for(pc[n], n)
            icons[n] = picture(name, pc[n], mask, fx, n)
            icons[n].save(os.path.join(HERE, "pictures", "%s-%d.png" % (name, n)), optimize=True)
        prow.append((name, icons))
    sheet(prow, os.path.join(preview, "icon-pictures.png"))
    print("ok", preview)


if __name__ == "__main__":
    main()
