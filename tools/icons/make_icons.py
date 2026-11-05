#!/usr/bin/env python3
"""Draws the MesoVibe mark, a pixel M, and writes every icon file from it.

The mark is pixel art, so each size is drawn straight from the grid below at a whole number of
pixels per cell, never resampled, and stays sharp from 16px to 1024px. Run it from the repo root
after changing a grid:

    python3 tools/icons/make_icons.py

It writes app/icons/ (the PNGs, icon.ico, icon.icns and the Mac menu-bar template) and
web/favicon.svg. icon.icns needs macOS's iconutil; elsewhere that one file is skipped.
Standard library only.
"""

import pathlib
import shutil
import struct
import subprocess
import sys
import tempfile
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[2]
APP_ICONS = ROOT / "app" / "icons"
WEB = ROOT / "web"

# The app's color tokens: a celadon `accent` square and a graphite `surface` M. Celadon keeps the icon
# visible on a dark Dock or taskbar, where a graphite square disappears.
PALETTE = {
    ".": (0, 0, 0, 0),
    "B": (0x8C, 0xC5, 0xA8, 255),
    "M": (0x1E, 0x1F, 0x1B, 255),
}

# The app icon: the M on a square with the signature stepped corners (one cell cut).
ICON = [
    ".BBBBBBBBBBBBBB.",
    "BBBBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBBBB",
    "BBBMMBBBBBBMMBBB",
    "BBBMMMBBBBMMMBBB",
    "BBBMMMMBBMMMMBBB",
    "BBBMMBMMMMBMMBBB",
    "BBBMMBBMMBBMMBBB",
    "BBBMMBBBBBBMMBBB",
    "BBBMMBBBBBBMMBBB",
    "BBBMMBBBBBBMMBBB",
    "BBBBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBBBB",
    "BBBBBBBBBBBBBBBB",
    ".BBBBBBBBBBBBBB.",
]

# The Mac menu bar: the same M alone, as a one-color template that macOS tints to match the bar.
# 12 cells at 3px each make the 36x36 image the tray loads (18pt on a Retina screen).
TRAY = [
    "............",
    "............",
    ".MM......MM.",
    ".MMM....MMM.",
    ".MMMM..MMMM.",
    ".MM.MMMM.MM.",
    ".MM..MM..MM.",
    ".MM......MM.",
    ".MM......MM.",
    ".MM......MM.",
    "............",
    "............",
]
TRAY_PALETTE = {".": (0, 0, 0, 0), "M": (0, 0, 0, 255)}


def render(grid, palette, size):
    """RGBA rows for the grid drawn at `size` pixels, a whole number of pixels per cell."""
    cells = len(grid)
    if size % cells:
        raise ValueError(f"{size}px isn't a whole number of pixels per cell for a {cells}-cell grid")
    scale = size // cells
    rows = []
    for line in grid:
        row = b"".join(bytes(palette[c]) * scale for c in line)
        rows.extend([row] * scale)
    return rows


def png(rows):
    height, width = len(rows), len(rows[0]) // 4

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))

    raw = b"".join(b"\x00" + row for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico(sizes):
    """A Windows .ico holding one PNG per size (Vista and later read PNG entries)."""
    images = [png(render(ICON, PALETTE, s)) for s in sizes]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for size, data in zip(sizes, images):
        side = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(images)


def svg():
    """The mark as an SVG: one path per color, one rectangle per run of a color along a row."""
    paths = {}
    for y, line in enumerate(ICON):
        x = 0
        while x < len(line):
            c, run = line[x], 1
            while x + run < len(line) and line[x + run] == c:
                run += 1
            if c != ".":
                paths.setdefault(c, []).append(f"M{x} {y}h{run}v1h-{run}z")
            x += run
    body = "".join(
        f'\n  <path fill="#{"%02X%02X%02X" % PALETTE[c][:3]}" d="{"".join(d)}"/>' for c, d in paths.items()
    )
    n = len(ICON)
    return f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {n} {n}" shape-rendering="crispEdges">{body}\n</svg>\n'


def icns(path):
    if not shutil.which("iconutil"):
        print("iconutil not found (it ships with macOS): skipped icon.icns", file=sys.stderr)
        return
    with tempfile.TemporaryDirectory() as tmp:
        iconset = pathlib.Path(tmp) / "icon.iconset"
        iconset.mkdir()
        for points in (16, 32, 128, 256, 512):
            (iconset / f"icon_{points}x{points}.png").write_bytes(png(render(ICON, PALETTE, points)))
            (iconset / f"icon_{points}x{points}@2x.png").write_bytes(png(render(ICON, PALETTE, points * 2)))
        subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(path)], check=True)


def main():
    for name, size in {
        "32x32.png": 32,
        "128x128.png": 128,
        "128x128@2x.png": 256,
        "icon.png": 512,
        "source.png": 1024,
    }.items():
        (APP_ICONS / name).write_bytes(png(render(ICON, PALETTE, size)))
    (APP_ICONS / "icon.ico").write_bytes(ico([16, 32, 48, 64, 128, 256]))
    icns(APP_ICONS / "icon.icns")
    (APP_ICONS / "tray-template.png").write_bytes(png(render(TRAY, TRAY_PALETTE, 36)))
    (WEB / "favicon.svg").write_text(svg())
    print("Wrote app/icons/ and web/favicon.svg")


if __name__ == "__main__":
    main()
