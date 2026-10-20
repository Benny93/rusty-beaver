#!/usr/bin/env python3
"""Pixel-art app icon (Metal Slug flavoured) for Rusty Beaver, stdlib only.

A tough beaver soldier in an army helmet hugs a database cylinder, in front of a dusty desert
tile. The art is a 64x64 grid built from shaded, outlined parts (part()/bayer() helpers, same
approach as oogway-rs/scripts/pixel_icon.py); PNGs are written by hand with zlib + struct and
scaled nearest-neighbour by whole pixels. Small sizes are area-averaged from the 64px sprite.

Writes (relative to the repo root):
  assets/icon.png (512), assets/icon-1024.png, assets/splash.png (384, no tile, transparent),
  assets/hicolor/<n>x<n>/apps/rusty-beaver.png (16..512), assets/rusty-beaver.ico,
  assets/rusty-beaver.icns (via iconutil, when available).

usage: python3 scripts/pixel_icon.py
"""

import math
import os
import shutil
import struct
import subprocess
import tempfile
import zlib

N = 64
PAL = {
    "ol": "#1b1216",
    # desert sky + sand
    "d0": "#f7dca0", "d1": "#f0c47e", "d2": "#e3a462", "d3": "#cf8448",
    "n0": "#e9bb6c", "n1": "#d49c50", "n2": "#b57a3a", "n3": "#8e572a", "n4": "#673a1f",
    "rk": "#7a4a2a",
    # fur
    "f0": "#3a2014", "f1": "#5e3620", "f2": "#8a5530", "f3": "#b67a45", "f4": "#dda36b",
    # muzzle / belly
    "u0": "#9c6a42", "u1": "#c99566", "u2": "#e6bd8a", "u3": "#f8e0b4",
    # tail (leathery, cross-hatched)
    "t0": "#2e1d1c", "t1": "#553833", "t2": "#7d5a4c", "t3": "#a8826a",
    # helmet (olive drab)
    "h0": "#242d16", "h1": "#3c4a22", "h2": "#5a6c32", "h3": "#869846", "h4": "#c4d27a",
    # teeth, eyes, nose
    "w0": "#fffbea", "w1": "#e2d3a8", "ew": "#fffaf0", "no": "#2a1718", "nh": "#8a6060",
    # database cylinder (steel blue / cyan)
    "c0": "#18263f", "c1": "#25466e", "c2": "#356e9e", "c3": "#55a0cc", "c4": "#94dcf0", "c5": "#e6fbff",
    # LEDs
    "a0": "#7a4a14", "a1": "#d0902e", "a2": "#ffe08a",
    "lg": "#7dff6a", "lr": "#ff5a4a", "ly": "#ffd84a",
}

grid = [[None] * N for _ in range(N)]


def put(x, y, c):
    if 0 <= x < N and 0 <= y < N and c:
        grid[y][x] = c


def get(x, y):
    return grid[y][x] if 0 <= x < N and 0 <= y < N else None


def part(mask, color_of, outline=True):
    """Fill a mask with per-pixel colours, then a 1px outline around it."""
    for (x, y) in mask:
        put(x, y, color_of(x, y))
    if outline:
        for (x, y) in mask:
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                if (x + dx, y + dy) not in mask:
                    put(x + dx, y + dy, "ol")


def bayer(x, y, level):
    """4x4 ordered dither: True for `level` (0..16) of 16 pixels."""
    m = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]]
    return m[y % 4][x % 4] < level


def ellipse(cx, cy, rx, ry, cond=None):
    return {(x, y) for y in range(N) for x in range(N)
            if ((x + 0.5 - cx) / rx) ** 2 + ((y + 0.5 - cy) / ry) ** 2 <= 1 and (cond is None or cond(x, y))}


def capsule(a, b, r):
    (ax, ay), (bx, by) = a, b
    out = set()
    for y in range(N):
        for x in range(N):
            px, py = x + 0.5, y + 0.5
            vx, vy = bx - ax, by - ay
            t = max(0, min(1, ((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)))
            if math.hypot(px - ax - t * vx, py - ay - t * vy) <= r:
                out.add((x, y))
    return out


def shader(cx, cy, rx, ry, tones, lx=-0.6, ly=-0.75, cuts=(0.75, 0.35, -0.05, -0.45)):
    """Round-volume shading lit from the upper left; dithered seams between the tone bands.
    tones: light..dark, len(cuts) + 1 entries."""
    def f(x, y):
        nx, ny = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
        nz = math.sqrt(max(0.0, 1 - nx * nx - ny * ny))
        lit = lx * nx + ly * ny + 0.45 * nz
        for i, c in enumerate(cuts):
            if lit > c + 0.08:
                return tones[i]
            if lit > c - 0.08:
                return tones[i] if bayer(x, y, 8) else tones[i + 1]
        return tones[-1]
    return f


def render(tile=True):
    for y in range(N):
        grid[y] = [None] * N

    if tile:
        # ---- desert sky: dithered vertical gradient
        bands = ["d0", "d1", "d2", "d3"]
        for y in range(46):
            t = y / 46 * 3
            i = min(2, int(t))
            frac = t - i
            for x in range(N):
                put(x, y, bands[i + 1] if bayer(x, y, int(16 * frac)) else bands[i])
        # sun glow, upper left
        for (x, y) in ellipse(9, 9, 6.5, 6.5):
            put(x, y, "d0")
        for (x, y) in ellipse(9, 9, 8.5, 8.5) - ellipse(9, 9, 6.5, 6.5):
            if bayer(x, y, 8):
                put(x, y, "d0")
        # far dunes
        for x in range(N):
            h = int(40 + 2.5 * math.sin(x / 7.0 + 1.2) + 1.5 * math.sin(x / 3.1))
            for y in range(h, 46):
                put(x, y, "n1" if y > h else "n0")
        # sand floor
        for y in range(46, N):
            t = (y - 46) / 18
            for x in range(N):
                put(x, y, "n2" if bayer(x, y, int(16 * min(1, t * 1.6))) else "n1")
        for x in range(N):
            put(x, 46, "n0")
        for (x, y) in [(5, 58), (6, 58), (58, 50), (33, 61), (34, 61), (52, 60)]:
            put(x, y, "rk")
            put(x, y - 1, "n0")
        # contact shadow
        for (x, y) in ellipse(33, 56, 29, 3.6):
            put(x, y, "n4" if bayer(x, y, 10) else "n3")

    # ---- flat beaver tail (cross-hatched), sticking out behind on the left
    tail = ellipse(6.5, 46.5, 5.2, 7.5)
    tail_sh = shader(6.5, 46.5, 5.2, 7.5, ["t3", "t2", "t2", "t1", "t0"])

    def tail_c(x, y):
        if (x + y) % 3 == 0 or (x - y) % 3 == 0:
            return {"t3": "t2", "t2": "t1"}.get(tail_sh(x, y), "t0")
        return tail_sh(x, y)
    part(tail, tail_c)

    # ---- database cylinder: three stacked disks
    CX, RX, RY, YT, YB = 47.5, 10.5, 3.6, 21, 52

    def sag(x):
        nx = (x + 0.5 - CX) / RX
        return RY * math.sqrt(max(0.0, 1 - nx * nx))

    cyl = {(x, y) for x in range(N) for y in range(N)
           if abs(x + 0.5 - CX) <= RX and YT - sag(x) <= y + 0.5 <= YB + sag(x)}

    def side(x, y):
        nx = (x + 0.5 - CX) / RX
        if -0.62 <= nx <= -0.42:
            return "c5" if -0.56 <= nx <= -0.48 else "c4"
        lit = -0.9 * nx + 0.15
        if lit > 0.55:
            return "c4" if bayer(x, y, 6) else "c3"
        if lit > 0.2:
            return "c3"
        if lit > -0.1:
            return "c3" if bayer(x, y, 8) else "c2"
        if lit > -0.55:
            return "c2"
        if lit > -0.72:
            return "c2" if bayer(x, y, 8) else "c1"
        return "c1"
    part(cyl, side)
    # top face
    top = {(x, y) for (x, y) in cyl if y + 0.5 <= YT + sag(x)}

    def top_c(x, y):
        nx, ny = (x + 0.5 - CX) / RX, (y + 0.5 - YT) / RY
        if nx * nx + ny * ny > 0.55:
            return "c4" if nx < 0.3 else "c3"
        return "c5" if nx < -0.2 and ny < 0.3 else ("c4" if bayer(x, y, 10) else "c5")
    part(top, top_c, outline=False)
    for x in range(N):
        if abs(x + 0.5 - CX) <= RX:
            yy = int(YT + sag(x))
            if (x, yy) in cyl:
                put(x, yy, "ol")
    # disk seams + rim highlight below each seam + LEDs
    for k in (1, 2):
        base = YT + (YB - YT) * k / 3
        for x in range(N):
            if abs(x + 0.5 - CX) <= RX - 0.3:
                yy = int(base + sag(x))
                put(x, yy, "ol")
                if (x, yy + 1) in cyl:
                    nx = (x + 0.5 - CX) / RX
                    put(x, yy + 1, "c4" if nx < 0.1 else "c2")
    for k in range(3):
        base = YT + (YB - YT) * k / 3 + 5.5
        for (dx, c) in ((3, "lg"), (5, "lg"), (7, "ly")):
            x = int(CX) + dx
            yy = int(base + sag(x))
            put(x, yy, c)
            put(x, yy + 1, "c0")
    # glints on the top rim
    for (x, y) in [(41, 19), (42, 18), (43, 18)]:
        put(x, y, "c5")

    # ---- beaver body
    BX = 20.5
    body = ellipse(BX, 43, 12, 11.5)
    part(body, shader(BX, 43, 12, 11.5, ["f4", "f3", "f2", "f1", "f0"]))
    belly = ellipse(BX + 1, 45.5, 6.5, 7.5)
    part(belly, shader(BX + 1, 45.5, 6.5, 7.5, ["u3", "u2", "u1", "u0", "u0"]), outline=False)
    # fur tufts on belly edge
    for (x, y) in [(15, 41), (15, 45), (15, 49), (27, 43), (27, 47)]:
        if get(x, y) in ("u1", "u0", "u2"):
            put(x, y, "f2")

    # bandolier with brass rounds, shoulder to hip
    strap = set()
    for i in range(0, 15):
        x, y = 12 + i, 33 + round(i * 1.05)
        strap |= {(x, y), (x, y + 1), (x, y + 2)}
    strap = {(x, y) for (x, y) in strap if (x, y) in body}
    part(strap, lambda x, y: "h2" if (x, y - 1) not in strap else "h1", outline=False)
    for (x, y) in strap:
        if (x, y + 1) not in strap and (x, y + 1) in body:
            put(x, y + 1, "ol")
        if (x, y - 1) not in strap and (x, y - 1) in body:
            put(x, y - 1, "ol")
    for i in range(1, 14, 3):
        x, y = 12 + i, 33 + round(i * 1.05)
        if (x, y) in strap:
            put(x, y - 1, "a2"); put(x + 1, y - 1, "a1")
            put(x, y, "a1"); put(x + 1, y, "a0")
            put(x, y - 2, "ol"); put(x + 1, y - 2, "ol")

    # feet
    for fx in (14.5, 27.5):
        foot = ellipse(fx, 54.5, 4.2, 2.2)
        part(foot, shader(fx, 54.5, 4.2, 2.2, ["f3", "f2", "f1", "f1", "f0"]))
        for d in (-1.5, 0.5, 2.5):
            put(int(fx + d), 55, "f0")

    # ---- head: round skull from an ellipse, face hand-drawn (Metal Slug style sprite rows)
    head = ellipse(BX, 24, 12.5, 9.4)
    part(head, shader(BX, 24, 12.5, 9.4, ["f4", "f3", "f2", "f1", "f0"]))
    # ears poking out under the helmet brim
    for ex in (7.5, 33.5):
        ear = ellipse(ex, 20.5, 2.5, 2.5)
        part(ear, lambda x, y, ex=ex: "f1" if abs(x + 0.5 - ex) < 1.1 and abs(y + 0.5 - 20.5) < 1.1 else "f2")
    # O outline, a-e fur dark..light, q/m/n/p muzzle dark..light, W eye white, G glint,
    # N/H nose + shine, T/t teeth light/shade, '.' keep what is underneath
    FACE = [
        "OddcbOOOOcbbbbbcOOOObbbaO",
        "OeddcccOOOOcccOOOOcccbbaO",
        "OedcOWWGOcccccccOGWWObbaO",
        "OeddOWWOOcccccccOOWWObbaO",
        "OeddcbbbbONHHNNObbbbbcbaO",
        "OednppppppONNNOnnnnmmbbaO",
        "OdnpppppOpppOnnnOnmmmqbaO",
        "OdnpppnnnOOOOOOOnmmmqqbaO",
        ".OcnnnmmmOTtOTtOmmqqqbaO.",
        "..OcmmmqqOTtOTtOqqqqbaO..",
        "...OccqqqOTtOTtOqqbbaO...",
        "....OOcccOTtOTtObbaOO....",
        "......OOOOttOttOOOO......",
        "..........OOOOO..........",
    ]
    FCOL = {"O": "ol", "a": "f0", "b": "f1", "c": "f2", "d": "f3", "e": "f4", "q": "u0", "m": "u1",
            "n": "u2", "p": "u3", "W": "ew", "G": "w0", "N": "no", "H": "nh", "T": "w0", "t": "w1"}
    for dy, row in enumerate(FACE):
        for dx, ch in enumerate(row):
            if ch != ".":
                put(8 + dx, 20 + dy, FCOL[ch])
    # cheek scar (tough guy)
    put(29, 25, "f0"); put(30, 24, "f0")


    # ---- army helmet
    HX = BX
    dome = ellipse(HX, 18.5, 12.6, 10.5, cond=lambda x, y: y <= 17)
    part(dome, shader(HX, 18.5, 12.6, 10.5, ["h4", "h3", "h2", "h1", "h0"], cuts=(0.85, 0.45, 0.05, -0.4)))
    brim = {(x, y) for x in range(6, 35) for y in range(17, 20)} - {(6, 19), (34, 19), (6, 17), (34, 17)}
    part(brim, lambda x, y: "h3" if y == 17 else ("h2" if y == 18 else "h1"))
    # band around the helmet + star decal
    for x in range(N):
        if (x, 14) in dome and (x, 15) in dome:
            put(x, 15, "h1")
            put(x, 14, "h0" if get(x, 14) in ("h1", "h0") else get(x, 14))
    star = [(20, 9), (19, 10), (20, 10), (21, 10), (18, 11), (19, 11), (20, 11), (21, 11), (22, 11),
            (19, 12), (20, 12), (21, 12), (19, 13), (21, 13)]
    for (x, y) in star:
        put(x, y, "w1" if y >= 12 else "w0")
    # specular glints
    for (x, y) in [(13, 10), (14, 9), (15, 9), (12, 11)]:
        put(x, y, "h4")
    # rivets on the brim
    for x in (9, 31):
        put(x, 18, "h4")
    # ---- arm hugging the cylinder (drawn last, over the DB)
    arm = capsule((28.5, 37.5), (38, 40), 2.7)
    part(arm, shader(33, 37.5, 7, 3.5, ["f4", "f3", "f2", "f1", "f0"]))
    paw = ellipse(40, 40, 3.2, 3.2)
    part(paw, shader(40, 40, 3.2, 3.2, ["f4", "f3", "f2", "f1", "f1"]))
    for (x, y) in [(41, 38), (42, 40), (41, 42)]:
        put(x, y, "f0")
    for (x, y) in [(43, 38), (44, 40), (43, 42)]:
        put(x, y, "w1")  # claws
    if tile:
        # ---- rounded tile corners + dark rim
        R = 9

        def inside(x, y):
            cx = R if x < R else N - 1 - R if x > N - 1 - R else x
            cy = R if y < R else N - 1 - R if y > N - 1 - R else y
            return math.hypot(x - cx, y - cy) <= R + 0.3
        rim = []
        for y in range(N):
            for x in range(N):
                if not inside(x, y):
                    grid[y][x] = None
                elif any(not inside(x + dx, y + dy) or not (0 <= x + dx < N and 0 <= y + dy < N)
                         for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1))):
                    rim.append((x, y))
        for (x, y) in rim:
            grid[y][x] = "ol"
    return [[hex_rgba(c) for c in row] for row in grid]


# Hand-drawn 16x16 sprite for the 16px sizes: the 64px art turns to mush there.
SMALL = [
    "..OOOOOOOOOOOO..",
    ".OssOOOOssssssO.",
    "OssOggGGOsOOOOOO",
    "OsOgggGGGOpccccO",
    "OhhhhhhhhOLBBBBO",
    "OFwOffOwOOLCCCBO",
    "OFfuNNuFOOLCCgBO",
    "OsfuwWuFOOBBBBBO",
    "OsOFwWFOOOOCCCBO",
    "OFfuufffffdOCgBO",
    "OTfuufFOOOOBBBBO",
    "OTfuufFOsOLCCCBO",
    "OTOfFFOssOLCCgBO",
    "OOFFOFFOSSSOOOOO",
    ".OSSSSSSSSSSSSO.",
    "..OOOOOOOOOOOO..",
]
SCOL = {"O": "ol", "s": "d1", "S": "n2", "g": "h3", "G": "h2", "h": "h1", "f": "f3", "F": "f1", "d": "f4",
        "u": "u2", "w": "w0", "W": "w1", "N": "no", "T": "t2", "L": "c4", "C": "c3", "B": "c1", "p": "c5",
        "c": "c4"}


def small_icon():
    img = [[hex_rgba(SCOL[ch]) if ch != "." else (0, 0, 0, 0) for ch in row] for row in SMALL]
    for (x, y) in [(13, 6), (13, 9), (13, 12)]:
        img[y][x] = hex_rgba("lg")
    return img


def hex_rgba(c):
    if c is None:
        return (0, 0, 0, 0)
    h = PAL[c]
    return (int(h[1:3], 16), int(h[3:5], 16), int(h[5:7], 16), 255)


# ---------------------------------------------------------------- image helpers
def scale_int(img, k):
    return [[px for px in row for _ in range(k)] for row in img for _ in range(k)]


def resample(img, size):
    """Area-average (premultiplied) resample of a square RGBA image to size x size."""
    n = len(img)
    f = n / size
    out = []
    for oy in range(size):
        row = []
        y0, y1 = oy * f, (oy + 1) * f
        for ox in range(size):
            x0, x1 = ox * f, (ox + 1) * f
            acc = [0.0] * 4
            wsum = 0.0
            for sy in range(int(y0), min(n, math.ceil(y1))):
                wy = min(y1, sy + 1) - max(y0, sy)
                for sx in range(int(x0), min(n, math.ceil(x1))):
                    w = wy * (min(x1, sx + 1) - max(x0, sx))
                    r, g, b, a = img[sy][sx]
                    acc[0] += r * a * w; acc[1] += g * a * w; acc[2] += b * a * w; acc[3] += a * w
                    wsum += w
            a = acc[3] / wsum
            if a <= 0:
                row.append((0, 0, 0, 0))
            else:
                row.append(tuple(round(acc[i] / acc[3]) for i in range(3)) + (round(a),))
        out.append(row)
    return out


def pad(img, size):
    n = len(img)
    o = (size - n) // 2
    out = [[(0, 0, 0, 0)] * size for _ in range(size)]
    for y in range(n):
        out[y + o][o:o + n] = img[y]
    return out


def sized(img, size):
    n = len(img)
    if size == 16 and n == N:
        return small_icon()
    if size % n == 0:
        return scale_int(img, size // n)
    if size > n:  # integer upscale, then area-resample the rest
        return resample(scale_int(img, size // n + 1), size)
    return resample(img, size)


def png_bytes(img):
    h, w = len(img), len(img[0])
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in img)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def write_png(path, img):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(png_bytes(img))


def write_ico(path, entries):
    data = [png_bytes(img) for img in entries]
    off = 6 + 16 * len(data)
    out = struct.pack("<HHH", 0, 1, len(data))
    for img, d in zip(entries, data):
        s = len(img)
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(d), off)
        off += len(d)
    with open(path, "wb") as f:
        f.write(out + b"".join(data))


def apple(icon, size):
    """Apple grid: tile ~80% of the canvas, whole-pixel scaling where it lands close enough."""
    k = int(0.82 * size / N)
    tile = scale_int(icon, k) if k >= 1 and k * N >= 0.74 * size else sized(icon, round(0.8 * size))
    return pad(tile, size)


if __name__ == "__main__":
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
    A = os.path.join(root, "assets")
    icon = render(tile=True)
    sprite = render(tile=False)
    write_png(f"{A}/icon.png", scale_int(icon, 8))
    write_png(f"{A}/icon-1024.png", scale_int(icon, 16))
    write_png(f"{A}/splash.png", scale_int(sprite, 6))
    for s in (16, 32, 48, 64, 128, 256, 512):
        write_png(f"{A}/hicolor/{s}x{s}/apps/rusty-beaver.png", sized(icon, s))
    write_ico(f"{A}/rusty-beaver.ico", [sized(icon, s) for s in (16, 24, 32, 48, 64, 128, 256)])
    if shutil.which("iconutil"):
        with tempfile.TemporaryDirectory() as tmp:
            iset = os.path.join(tmp, "rusty-beaver.iconset")
            os.makedirs(iset)
            for s in (16, 32, 128, 256, 512):
                write_png(f"{iset}/icon_{s}x{s}.png", apple(icon, s))
                write_png(f"{iset}/icon_{s}x{s}@2x.png", apple(icon, 2 * s))
            subprocess.run(["iconutil", "-c", "icns", iset, "-o", f"{A}/rusty-beaver.icns"], check=True)
    else:
        print("iconutil not found: skipped rusty-beaver.icns")
    print("wrote assets/icon.png, icon-1024.png, splash.png, hicolor/*, rusty-beaver.ico, rusty-beaver.icns")
