#!/usr/bin/env python3
"""Generates the RTM Viewer launcher icon (270-degree gauge on near-black) as PNGs into
android/app/src/main/res/mipmap-*/ic_launcher.png. Stdlib only. Run from viewer/:
    python3 test/ui/tools/gen_launcher_icon.py
"""
import math, os, struct, sys, zlib

BG = (0x0B, 0x0D, 0x10)
TRACK = (0x24, 0x2A, 0x31)
ACCENT = (0x2D, 0xD4, 0xBF)
SIZES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
SS = 4  # supersampling per axis


def sample(x, y):
    """Colour + alpha at unit coords (0..1). Rounded-square background with a gauge."""
    # rounded square, corner radius 0.22
    r = 0.22
    dx = max(abs(x - 0.5) - (0.5 - r), 0)
    dy = max(abs(y - 0.5) - (0.5 - r), 0)
    if dx * dx + dy * dy > r * r:
        return None
    cx, cy = x - 0.5, y - 0.52
    d = math.hypot(cx, cy)
    ring_r, half_w = 0.30, 0.055
    ang = math.degrees(math.atan2(cy, cx)) % 360  # 0 = +x, clockwise (y down)
    rel = (ang - 135) % 360  # gauge starts bottom-left at 135 deg, sweeps 270
    on_ring = abs(d - ring_r) <= half_w and rel <= 270
    # round caps
    for a in (135, 135 + 270 * 0.68, 135 + 270):
        px = 0.5 + ring_r * math.cos(math.radians(a))
        py = 0.52 + ring_r * math.sin(math.radians(a))
        if math.hypot(x - px, y - py) <= half_w:
            on_ring = True
            if a != 135 + 270:
                rel = min(rel, 270 * 0.68) if a != 135 else 0
    if on_ring:
        return ACCENT if rel <= 270 * 0.68 + 0.5 else TRACK
    # needle hub dot
    if d <= 0.06:
        return ACCENT
    return BG


def render(n):
    rows = []
    for py in range(n):
        row = bytearray([0])
        for px in range(n):
            acc = [0, 0, 0, 0]
            for sy in range(SS):
                for sx in range(SS):
                    c = sample((px + (sx + 0.5) / SS) / n, (py + (sy + 0.5) / SS) / n)
                    if c is not None:
                        acc[0] += c[0]; acc[1] += c[1]; acc[2] += c[2]; acc[3] += 1
            k = acc[3]
            if k:
                row += bytes([acc[0] // k, acc[1] // k, acc[2] // k, 255 * k // (SS * SS)])
            else:
                row += bytes([0, 0, 0, 0])
        rows.append(bytes(row))
    raw = b"".join(rows)

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def main():
    res = sys.argv[1] if len(sys.argv) > 1 else "android/app/src/main/res"
    for dpi, n in SIZES.items():
        path = os.path.join(res, f"mipmap-{dpi}", "ic_launcher.png")
        with open(path, "wb") as f:
            f.write(render(n))
        print(path, n)


if __name__ == "__main__":
    main()
